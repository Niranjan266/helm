//! "Stay reachable after restart": a second, separate service (`HelmSignIn`) that runs the
//! agent itself, as LocalSystem, while the PC's owner is **not** signed in to Windows, and
//! steps aside the moment they sign in so the desktop app (`Helm.exe`, started from the
//! user's Run key) can take port 47820 back.
//!
//! This module is the pure part: the service's command line and the hand-over state
//! machine. The SCM glue lives in `win::prelogin`; what "the agent" is (start / stop) is
//! supplied by the caller (the desktop app runs `helm_agent` in-process).

use std::path::PathBuf;
use std::time::Duration;

use crate::backoff::{Backoff, BackoffPolicy};

/// SCM service name. Different from [`crate::SERVICE_NAME`] (the console agent's session
/// launcher), so the two never collide.
pub const SERVICE_NAME: &str = "HelmSignIn";
/// Name shown in services.msc.
pub const DISPLAY_NAME: &str = "Helm (reachable before sign-in)";
pub const DESCRIPTION: &str = "Keeps this PC reachable from your paired phone after a \
    restart, before anyone signs in (lock, sleep, restart and shut down only). Stops when \
    you sign in and the Helm app takes over.";

/// How long to wait after the owner signed out before serving, so their Helm.exe has let
/// go of the port.
pub const SIGN_OUT_GRACE: Duration = Duration::from_secs(3);
/// How often the service re-checks who is signed in (session-change notices are the fast
/// path; this is the fallback).
pub const POLL: Duration = Duration::from_secs(2);

/// What the SCM is told to start: `<exe> <service flag> --data-dir <dir> --owner <sid>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunArgs {
    /// The owner's data folder (`%APPDATA%\Helm` of the user who turned this on). A service
    /// runs as SYSTEM, whose own %APPDATA% is somewhere else entirely, so it is passed in.
    pub data_dir: PathBuf,
    /// The owner's SID (`S-1-5-21-...`): while a session of this user exists, stay idle.
    pub owner_sid: String,
}

/// `S-1-<authority>-<sub>...`, digits only, sane length.
pub fn valid_sid(sid: &str) -> bool {
    let Some(rest) = sid.strip_prefix("S-1-") else {
        return false;
    };
    sid.len() <= 184
        && rest.split('-').count() >= 2
        && rest
            .split('-')
            .all(|p| !p.is_empty() && p.len() <= 10 && p.bytes().all(|b| b.is_ascii_digit()))
}

impl RunArgs {
    pub fn to_args(&self) -> Vec<String> {
        vec![
            "--data-dir".into(),
            self.data_dir.display().to_string(),
            "--owner".into(),
            self.owner_sid.clone(),
        ]
    }

    /// Parse what [`RunArgs::to_args`] produced.
    pub fn parse(args: &[String]) -> Result<RunArgs, String> {
        let mut data_dir = None;
        let mut owner = None;
        let mut it = args.iter();
        while let Some(a) = it.next() {
            match a.as_str() {
                "--data-dir" => data_dir = Some(it.next().ok_or("--data-dir needs a path")?),
                "--owner" => owner = Some(it.next().ok_or("--owner needs a SID")?),
                other => return Err(format!("unexpected argument {other:?}")),
            }
        }
        let data_dir = PathBuf::from(data_dir.ok_or("--data-dir is required")?);
        if !data_dir.is_absolute() || data_dir.display().to_string().contains('"') {
            return Err("--data-dir must be an absolute path without quotes".into());
        }
        let owner_sid = owner.ok_or("--owner is required")?.clone();
        if !valid_sid(&owner_sid) {
            return Err(format!("{owner_sid:?} is not a SID"));
        }
        Ok(RunArgs {
            data_dir,
            owner_sid,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Start the agent (bind the port, connect to the relay).
    Start,
    /// Stop the agent and free the port: the owner is here.
    Stop,
}

/// Decides when the service's own agent should run.
///
/// Rule: serve only while the owner has no Windows session at all (signed out, or the PC
/// just booted). A locked or disconnected session still counts as signed in: their
/// Helm.exe is (or may be) running there and owns the port.
#[derive(Debug, Clone)]
pub struct Handover {
    serving: bool,
    /// Don't start before this (after a sign-out, or after a failed start).
    not_before: Duration,
    backoff: Backoff,
}

impl Default for Handover {
    fn default() -> Self {
        Self::new()
    }
}

impl Handover {
    pub fn new() -> Self {
        Handover {
            serving: false,
            not_before: Duration::ZERO,
            backoff: Backoff::new(BackoffPolicy::default()),
        }
    }

    pub fn serving(&self) -> bool {
        self.serving
    }

    /// Feed the current presence of the owner (`None` = could not tell: keep as is).
    pub fn step(&mut self, now: Duration, owner_signed_in: Option<bool>) -> Option<Step> {
        match owner_signed_in? {
            true => {
                self.not_before = now.saturating_add(SIGN_OUT_GRACE);
                self.backoff.reset();
                if self.serving {
                    self.serving = false;
                    Some(Step::Stop)
                } else {
                    None
                }
            }
            false if !self.serving && now >= self.not_before => {
                self.serving = true;
                Some(Step::Start)
            }
            false => None,
        }
    }

    /// The agent did not start, or stopped on its own: retry later with backoff.
    pub fn agent_failed(&mut self, now: Duration, ran_for: Duration) {
        self.serving = false;
        let delay = self.backoff.on_exit(ran_for);
        self.not_before = now.saturating_add(delay);
    }
}
