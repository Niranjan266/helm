//! Command-line parsing for `helm-service.exe` (pure, testable).

use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartMode {
    /// Start at boot.
    Auto,
    /// Start only when asked (`helm-service start`).
    Demand,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Register HelmService with the SCM (needs an elevated prompt).
    Install {
        agent: Option<PathBuf>,
        start: StartMode,
        agent_args: Vec<String>,
    },
    Uninstall,
    Start,
    Stop,
    Status,
    /// Entry point used by the SCM; not meant to be run by hand.
    Run {
        agent: Option<PathBuf>,
        agent_args: Vec<String>,
    },
    Help,
}

pub const USAGE: &str = "\
helm-service: runs the Helm agent in the signed-in user's session

usage:
  helm-service install [--agent <path>] [--start auto|demand] [-- <agent args>...]
  helm-service uninstall
  helm-service start | stop | status
  helm-service run [--agent <path>] [-- <agent args>...]    (used by the SCM)

--agent defaults to helm-agent.exe next to helm-service.exe.
install/uninstall/start/stop need an elevated (administrator) prompt.";

/// Parses the arguments after the program name.
pub fn parse(args: &[String]) -> Result<Command, String> {
    let Some((cmd, rest)) = args.split_first() else {
        return Ok(Command::Help);
    };
    match cmd.as_str() {
        "install" => {
            let o = parse_options(rest, true)?;
            Ok(Command::Install {
                agent: o.agent,
                start: o.start,
                agent_args: o.agent_args,
            })
        }
        "run" => {
            let o = parse_options(rest, false)?;
            Ok(Command::Run {
                agent: o.agent,
                agent_args: o.agent_args,
            })
        }
        "uninstall" | "start" | "stop" | "status" if !rest.is_empty() => {
            Err(format!("{cmd} takes no arguments"))
        }
        "uninstall" => Ok(Command::Uninstall),
        "start" => Ok(Command::Start),
        "stop" => Ok(Command::Stop),
        "status" => Ok(Command::Status),
        "help" | "-h" | "--help" | "/?" => Ok(Command::Help),
        other => Err(format!("unknown command {other:?}")),
    }
}

struct Options {
    agent: Option<PathBuf>,
    start: StartMode,
    agent_args: Vec<String>,
}

fn parse_options(args: &[String], allow_start: bool) -> Result<Options, String> {
    let mut o = Options {
        agent: None,
        start: StartMode::Auto,
        agent_args: Vec::new(),
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--" => {
                o.agent_args = it.cloned().collect();
                break;
            }
            "--agent" => {
                let p = it.next().ok_or("--agent needs a path")?;
                if p.contains('"') {
                    return Err("--agent path must not contain quotes".into());
                }
                o.agent = Some(PathBuf::from(p));
            }
            "--start" if allow_start => {
                o.start = match it.next().map(String::as_str) {
                    Some("auto") => StartMode::Auto,
                    Some("demand") => StartMode::Demand,
                    _ => return Err("--start needs auto or demand".into()),
                };
            }
            other => return Err(format!("unexpected argument {other:?}")),
        }
    }
    Ok(o)
}

/// Arguments to store in the SCM so it starts us as `helm-service run ...`.
pub fn run_arguments(agent: Option<&std::path::Path>, agent_args: &[String]) -> Vec<String> {
    let mut v = vec!["run".to_string()];
    if let Some(p) = agent {
        v.push("--agent".into());
        v.push(p.display().to_string());
    }
    if !agent_args.is_empty() {
        v.push("--".into());
        v.extend(agent_args.iter().cloned());
    }
    v
}
