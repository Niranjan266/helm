//! The SCM-facing service: control handler + supervision loop.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use windows_service::service::{
    ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::{define_windows_service, service_dispatcher};

use super::launch::{Child, LaunchError, active_console_session, launch_in_session};
use super::log::log;
use crate::SERVICE_NAME;
use crate::backoff::BackoffPolicy;
use crate::cmdline;
use crate::supervisor::{Action, Event, Supervisor};

/// What `helm-service run` was told to start.
#[derive(Debug, Clone)]
pub struct RunConfig {
    pub agent: PathBuf,
    pub agent_args: Vec<String>,
}

static CONFIG: OnceLock<RunConfig> = OnceLock::new();

const TICK: Duration = Duration::from_millis(500);

enum Msg {
    Stop,
    Session { logon: bool },
}

define_windows_service!(ffi_service_main, service_main);

/// Hands the process to the SCM dispatcher; returns when the service stops.
pub fn run_dispatcher(config: RunConfig) -> windows_service::Result<()> {
    let _ = CONFIG.set(config);
    service_dispatcher::start(SERVICE_NAME, ffi_service_main)
}

fn service_main(_args: Vec<OsString>) {
    if let Err(e) = run_service() {
        log(&format!("service error: {e}"));
    }
}

fn run_service() -> windows_service::Result<()> {
    let config = CONFIG.get().cloned().expect("config set before dispatch");
    let (tx, rx) = mpsc::channel::<Msg>();

    let handler = move |control| -> ServiceControlHandlerResult {
        match control {
            ServiceControl::Stop | ServiceControl::Shutdown => {
                let _ = tx.send(Msg::Stop);
                ServiceControlHandlerResult::NoError
            }
            ServiceControl::SessionChange(p) => {
                let logon = matches!(
                    p.reason,
                    windows_service::service::SessionChangeReason::SessionLogon
                );
                let _ = tx.send(Msg::Session { logon });
                ServiceControlHandlerResult::NoError
            }
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            _ => ServiceControlHandlerResult::NotImplemented,
        }
    };
    let status = service_control_handler::register(SERVICE_NAME, handler)?;
    let set = |state: ServiceState, accept: ServiceControlAccept, wait: Duration| {
        status.set_service_status(ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: state,
            controls_accepted: accept,
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: wait,
            process_id: None,
        })
    };
    set(
        ServiceState::Running,
        ServiceControlAccept::STOP
            | ServiceControlAccept::SHUTDOWN
            | ServiceControlAccept::SESSION_CHANGE,
        Duration::ZERO,
    )?;
    log(&format!("started; agent = {}", config.agent.display()));

    let command_line = cmdline::build(&config.agent.display().to_string(), &config.agent_args);
    let epoch = Instant::now();
    let mut sup = Supervisor::new(BackoffPolicy::default());
    let mut child: Option<Child> = None;
    let mut pending = VecDeque::from([Event::SessionChanged {
        active: active_console_session(),
        logon: false,
    }]);

    while !sup.is_stopped() {
        // Drain events, executing actions; actions may produce more events.
        while let Some(ev) = pending.pop_front() {
            for action in sup.handle(epoch.elapsed(), ev) {
                match action {
                    Action::Launch { session } => {
                        match launch_in_session(session, &config.agent, &command_line) {
                            Ok(c) => {
                                log(&format!("agent pid {} started in session {session}", c.pid));
                                child = Some(c);
                            }
                            Err(e) => {
                                if !matches!(e, LaunchError::NoUser) {
                                    log(&format!("launch in session {session} failed: {e}"));
                                }
                                pending.push_back(Event::LaunchFailed {
                                    no_user: matches!(e, LaunchError::NoUser),
                                });
                            }
                        }
                    }
                    Action::Terminate => {
                        if let Some(c) = &child {
                            log(&format!("stopping agent pid {}", c.pid));
                            c.kill();
                            c.wait_ms(5_000);
                        }
                        // Report the exit right away (a no-op `None` child
                        // counts as already exited).
                        if child.as_ref().is_none_or(|c| c.try_wait().is_some()) {
                            let ran_for =
                                child.take().map_or(Duration::ZERO, |c| c.started.elapsed());
                            pending.push_back(Event::ChildExited { ran_for });
                        }
                    }
                    Action::Exit => {}
                }
            }
        }
        if sup.is_stopped() {
            break;
        }

        let msg = match rx.recv_timeout(TICK) {
            Ok(Msg::Stop) | Err(RecvTimeoutError::Disconnected) => Event::StopRequested,
            Ok(Msg::Session { logon }) => Event::SessionChanged {
                active: active_console_session(),
                logon,
            },
            Err(RecvTimeoutError::Timeout) => Event::Tick,
        };
        // An exit that already happened is handled before the new message.
        if let Some(code) = child.as_ref().and_then(Child::try_wait) {
            let c = child.take().expect("checked above");
            log(&format!(
                "agent pid {} (session {}) exited with code {code}",
                c.pid, c.session
            ));
            pending.push_back(Event::ChildExited {
                ran_for: c.started.elapsed(),
            });
        }
        pending.push_back(msg);
    }

    drop(child);
    log("stopped");
    set(
        ServiceState::Stopped,
        ServiceControlAccept::empty(),
        Duration::ZERO,
    )?;
    Ok(())
}
