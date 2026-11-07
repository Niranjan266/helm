//! The `HelmSignIn` service: runs a caller-supplied agent while the owner is not signed in
//! (see [`crate::prelogin`]), plus install / uninstall through the SCM.

use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use windows_service::service::{
    ServiceAccess, ServiceAction, ServiceActionType, ServiceControl, ServiceControlAccept,
    ServiceErrorControl, ServiceExitCode, ServiceFailureActions, ServiceFailureResetPeriod,
    ServiceInfo, ServiceStartType, ServiceState, ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
use windows_service::{define_windows_service, service_dispatcher};

use super::sessions::owner_signed_in;
use crate::prelogin::{DESCRIPTION, DISPLAY_NAME, Handover, POLL, RunArgs, SERVICE_NAME, Step};

/// The agent the service runs. `start` should return once it is listening.
pub trait Agent: Send {
    fn start(&mut self) -> Result<(), String>;
    /// Stop and free the port. Must be safe to call when not running.
    fn stop(&mut self);
    /// `false` once a started agent has ended on its own.
    fn alive(&mut self) -> bool;
}

/// Builds the agent once the service is running.
pub type Factory = Box<dyn FnOnce(&RunArgs) -> Box<dyn Agent> + Send>;
/// Receives one line per state change.
pub type LogFn = Box<dyn Fn(&str) + Send + Sync>;

static SETUP: OnceLock<Mutex<Option<(RunArgs, Factory, LogFn)>>> = OnceLock::new();

define_windows_service!(ffi_service_main, service_main);

/// Hand the process to the SCM. `make` builds the agent once the service is running; `log`
/// receives one line per state change. Returns when the service stops.
pub fn run_dispatcher(args: RunArgs, make: Factory, log: LogFn) -> windows_service::Result<()> {
    let _ = SETUP.set(Mutex::new(Some((args, make, log))));
    service_dispatcher::start(SERVICE_NAME, ffi_service_main)
}

fn service_main(_args: Vec<OsString>) {
    let Some((args, make, log)) = SETUP
        .get()
        .and_then(|m| m.lock().ok().and_then(|mut g| g.take()))
    else {
        return;
    };
    if let Err(e) = run_service(&args, make, &*log) {
        log(&format!("service error: {e}"));
    }
}

enum Msg {
    Stop,
    Sessions,
}

fn run_service(args: &RunArgs, make: Factory, log: &dyn Fn(&str)) -> windows_service::Result<()> {
    let (tx, rx) = mpsc::channel::<Msg>();
    let handler = move |control| -> ServiceControlHandlerResult {
        match control {
            ServiceControl::Stop | ServiceControl::Shutdown | ServiceControl::Preshutdown => {
                let _ = tx.send(Msg::Stop);
                ServiceControlHandlerResult::NoError
            }
            ServiceControl::SessionChange(_) => {
                let _ = tx.send(Msg::Sessions);
                ServiceControlHandlerResult::NoError
            }
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            _ => ServiceControlHandlerResult::NotImplemented,
        }
    };
    let status = service_control_handler::register(SERVICE_NAME, handler)?;
    let set = |state: ServiceState, accept: ServiceControlAccept| {
        status.set_service_status(ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: state,
            controls_accepted: accept,
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Duration::from_secs(10),
            process_id: None,
        })
    };
    set(
        ServiceState::Running,
        ServiceControlAccept::STOP
            | ServiceControlAccept::SHUTDOWN
            | ServiceControlAccept::SESSION_CHANGE,
    )?;
    log(&format!("started for {}", args.data_dir.display()));

    let mut agent = make(args);
    let epoch = Instant::now();
    let mut handover = Handover::new();
    let mut started_at = Instant::now();
    loop {
        if handover.serving() && !agent.alive() {
            log("agent ended on its own");
            agent.stop();
            handover.agent_failed(epoch.elapsed(), started_at.elapsed());
        }
        match handover.step(epoch.elapsed(), owner_signed_in(&args.owner_sid)) {
            Some(Step::Start) => {
                started_at = Instant::now();
                match agent.start() {
                    Ok(()) => log("owner not signed in: agent serving"),
                    Err(e) => {
                        log(&format!("agent failed to start: {e}"));
                        agent.stop();
                        handover.agent_failed(epoch.elapsed(), Duration::ZERO);
                    }
                }
            }
            Some(Step::Stop) => {
                agent.stop();
                log("owner signed in: agent stopped, Helm app takes over");
            }
            None => {}
        }
        match rx.recv_timeout(POLL) {
            Ok(Msg::Stop) | Err(RecvTimeoutError::Disconnected) => break,
            Ok(Msg::Sessions) | Err(RecvTimeoutError::Timeout) => {}
        }
    }
    set(ServiceState::StopPending, ServiceControlAccept::empty())?;
    agent.stop();
    log("stopped");
    set(ServiceState::Stopped, ServiceControlAccept::empty())?;
    Ok(())
}

// ---------------------------------------------------------------- SCM

fn err(what: &str) -> impl Fn(windows_service::Error) -> String + '_ {
    move |e| format!("{what}: {e}")
}

/// Whether `HelmSignIn` is registered (no elevation needed).
pub fn is_installed() -> bool {
    ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .and_then(|m| m.open_service(SERVICE_NAME, ServiceAccess::QUERY_STATUS))
        .is_ok()
}

/// Register (or re-register) `exe <service_flag> <args>` as an auto-start LocalSystem
/// service and start it. Needs elevation.
pub fn install(exe: &Path, service_flag: &str, args: &RunArgs) -> Result<(), String> {
    if is_installed() {
        uninstall()?;
    }
    let m = ServiceManager::local_computer(
        None::<&str>,
        ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE,
    )
    .map_err(err("open service manager"))?;
    let mut launch = vec![OsString::from(service_flag)];
    launch.extend(args.to_args().into_iter().map(OsString::from));
    let info = ServiceInfo {
        name: OsString::from(SERVICE_NAME),
        display_name: OsString::from(DISPLAY_NAME),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: exe.to_path_buf(),
        launch_arguments: launch,
        dependencies: vec![],
        account_name: None, // LocalSystem
        account_password: None,
    };
    let svc = m
        .create_service(
            &info,
            ServiceAccess::CHANGE_CONFIG | ServiceAccess::START | ServiceAccess::QUERY_STATUS,
        )
        .map_err(err("create service"))?;
    svc.set_description(DESCRIPTION)
        .map_err(err("set description"))?;
    let restart = ServiceAction {
        action_type: ServiceActionType::Restart,
        delay: Duration::from_secs(10),
    };
    svc.update_failure_actions(ServiceFailureActions {
        reset_period: ServiceFailureResetPeriod::After(Duration::from_secs(24 * 3600)),
        reboot_msg: None,
        command: None,
        actions: Some(vec![restart.clone(), restart.clone(), restart]),
    })
    .map_err(err("set failure actions"))?;
    svc.start::<&OsStr>(&[]).map_err(err("start service"))?;
    Ok(())
}

fn open(access: ServiceAccess) -> Result<Option<windows_service::service::Service>, String> {
    let m = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .map_err(err("open service manager"))?;
    Ok(m.open_service(SERVICE_NAME, access).ok())
}

fn stop_and_wait(svc: &windows_service::service::Service) {
    let _ = svc.stop();
    let deadline = Instant::now() + Duration::from_secs(20);
    while svc
        .query_status()
        .is_ok_and(|s| s.current_state != ServiceState::Stopped)
        && Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Stop and delete `HelmSignIn` (no-op when absent). Needs elevation.
pub fn uninstall() -> Result<(), String> {
    let Some(svc) =
        open(ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::DELETE)?
    else {
        return Ok(());
    };
    stop_and_wait(&svc);
    svc.delete().map_err(err("delete service"))?;
    Ok(())
}

/// Stop the service, run `swap` (e.g. replace its executable), start it again. Keeps the
/// registered command line. No-op (and `swap` not run) when absent. Needs elevation.
pub fn restart_with(swap: impl FnOnce() -> Result<(), String>) -> Result<(), String> {
    let Some(svc) = open(ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::START)?
    else {
        return Ok(());
    };
    stop_and_wait(&svc);
    let swapped = swap();
    svc.start::<&OsStr>(&[]).map_err(err("start service"))?;
    swapped
}
