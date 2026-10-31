//! Pure state machine deciding when to launch, kill and relaunch the agent.
//!
//! The Windows driver feeds it [`Event`]s (session changes, child exits,
//! launch failures, timer ticks, stop requests) together with a monotonic
//! `now`, and carries out the returned [`Action`]s. No OS calls happen here,
//! so every transition is unit-testable.

use std::time::Duration;

use crate::backoff::{Backoff, BackoffPolicy};

/// How often to retry while the console session has no signed-in user.
pub const NO_USER_POLL: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// The active console session is now `active` (`None`: no console
    /// session). Sent at start-up and on every SCM session-change notice.
    /// `logon` is true when a user just signed in to that session.
    SessionChanged { active: Option<u32>, logon: bool },
    /// The agent process exited after running for `ran_for`.
    ChildExited { ran_for: Duration },
    /// A requested launch did not produce a process. `no_user` means the
    /// session has no signed-in user yet (WTSQueryUserToken: ERROR_NO_TOKEN).
    LaunchFailed { no_user: bool },
    /// Periodic timer.
    Tick,
    /// SCM stop / system shutdown.
    StopRequested,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Start the agent in this session.
    Launch { session: u32 },
    /// Kill the running agent; a `ChildExited` event must follow.
    Terminate,
    /// Report "stopped" to the SCM and leave the service loop.
    Exit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Nothing to run in: no console session, or nobody signed in to it.
    WaitingForUser {
        session: Option<u32>,
        retry_at: Duration,
    },
    /// A launch was requested / the agent is running in `session`.
    Running {
        session: u32,
        since: Duration,
    },
    /// The agent is being killed; afterwards launch in `next` (if any).
    Switching {
        next: Option<u32>,
    },
    /// Waiting out a restart delay.
    Backoff {
        session: u32,
        until: Duration,
    },
    /// Stop requested, waiting for the agent to exit.
    Stopping,
    Stopped,
}

#[derive(Debug, Clone)]
pub struct Supervisor {
    phase: Phase,
    backoff: Backoff,
}

impl Supervisor {
    pub fn new(policy: BackoffPolicy) -> Self {
        Self {
            phase: Phase::WaitingForUser {
                session: None,
                retry_at: Duration::MAX,
            },
            backoff: Backoff::new(policy),
        }
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn is_stopped(&self) -> bool {
        self.phase == Phase::Stopped
    }

    /// The session the agent is (or is about to be) running in.
    pub fn running_session(&self) -> Option<u32> {
        match self.phase {
            Phase::Running { session, .. } => Some(session),
            _ => None,
        }
    }

    pub fn handle(&mut self, now: Duration, event: Event) -> Vec<Action> {
        use Phase as P;
        match (self.phase, event) {
            (P::Stopped, _) => vec![],

            // ---- stop
            (P::Running { .. } | P::Switching { .. }, Event::StopRequested) => {
                self.phase = P::Stopping;
                vec![Action::Terminate]
            }
            (P::Stopping, Event::StopRequested) => vec![],
            (_, Event::StopRequested) => self.stop(),
            (P::Stopping, Event::ChildExited { .. } | Event::LaunchFailed { .. }) => self.stop(),
            (P::Stopping, _) => vec![],

            // ---- session changes
            (P::Running { session, .. }, Event::SessionChanged { active, .. }) => {
                if active == Some(session) {
                    // Lock/unlock, or a logon notice for the session we are
                    // already in (a logoff always ends the agent first).
                    vec![]
                } else {
                    // Another session took the console: move over there.
                    self.phase = P::Switching { next: active };
                    vec![Action::Terminate]
                }
            }
            (P::Switching { .. }, Event::SessionChanged { active, .. }) => {
                self.phase = P::Switching { next: active };
                vec![]
            }
            (P::Backoff { session, .. }, Event::SessionChanged { active, logon })
                if active == Some(session) && !logon =>
            {
                vec![] // same session: keep honouring the crash-loop delay
            }
            (
                P::Backoff { .. } | P::WaitingForUser { .. },
                Event::SessionChanged { active, .. },
            ) => match active {
                Some(s) => {
                    self.backoff.reset();
                    self.launch(now, s)
                }
                None => {
                    self.phase = P::WaitingForUser {
                        session: None,
                        retry_at: Duration::MAX,
                    };
                    vec![]
                }
            },

            // ---- child lifecycle
            (P::Running { session, .. }, Event::ChildExited { ran_for }) => {
                let delay = self.backoff.on_exit(ran_for);
                self.phase = P::Backoff {
                    session,
                    until: now.saturating_add(delay),
                };
                vec![]
            }
            (P::Running { session, .. }, Event::LaunchFailed { no_user: true }) => {
                self.phase = P::WaitingForUser {
                    session: Some(session),
                    retry_at: now.saturating_add(NO_USER_POLL),
                };
                vec![]
            }
            (P::Running { session, .. }, Event::LaunchFailed { no_user: false }) => {
                let delay = self.backoff.on_launch_failure();
                self.phase = P::Backoff {
                    session,
                    until: now.saturating_add(delay),
                };
                vec![]
            }
            (P::Switching { next }, Event::ChildExited { .. } | Event::LaunchFailed { .. }) => {
                self.backoff.reset();
                match next {
                    Some(s) => self.launch(now, s),
                    None => {
                        self.phase = P::WaitingForUser {
                            session: None,
                            retry_at: Duration::MAX,
                        };
                        vec![]
                    }
                }
            }

            // ---- timers
            (P::Backoff { session, until }, Event::Tick) if now >= until => {
                self.launch(now, session)
            }
            (
                P::WaitingForUser {
                    session: Some(s),
                    retry_at,
                },
                Event::Tick,
            ) if now >= retry_at => self.launch(now, s),

            // Everything else (stray exits, early ticks) changes nothing.
            _ => vec![],
        }
    }

    fn launch(&mut self, now: Duration, session: u32) -> Vec<Action> {
        self.phase = Phase::Running {
            session,
            since: now,
        };
        vec![Action::Launch { session }]
    }

    fn stop(&mut self) -> Vec<Action> {
        self.phase = Phase::Stopped;
        vec![Action::Exit]
    }
}
