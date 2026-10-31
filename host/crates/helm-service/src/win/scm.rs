//! install / uninstall / start / stop / status through the Service Control Manager.

use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use windows_service::service::{
    ServiceAccess, ServiceAction, ServiceActionType, ServiceErrorControl, ServiceFailureActions,
    ServiceFailureResetPeriod, ServiceInfo, ServiceStartType, ServiceState, ServiceType,
};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

use crate::cli::{self, StartMode};
use crate::{DISPLAY_NAME, SERVICE_NAME};

type Result<T> = std::result::Result<T, String>;

fn err(what: &str) -> impl Fn(windows_service::Error) -> String + '_ {
    move |e| format!("{what}: {e} (run from an elevated prompt?)")
}

fn manager(access: ServiceManagerAccess) -> Result<ServiceManager> {
    ServiceManager::local_computer(None::<&str>, access).map_err(err("open service manager"))
}

pub fn install(agent: &Path, start: StartMode, agent_args: &[String]) -> Result<()> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    if !agent.is_file() {
        return Err(format!("agent not found at {}", agent.display()));
    }
    let m = manager(ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE)?;
    let info = ServiceInfo {
        name: OsString::from(SERVICE_NAME),
        display_name: OsString::from(DISPLAY_NAME),
        service_type: ServiceType::OWN_PROCESS,
        start_type: match start {
            StartMode::Auto => ServiceStartType::AutoStart,
            StartMode::Demand => ServiceStartType::OnDemand,
        },
        error_control: ServiceErrorControl::Normal,
        executable_path: exe,
        launch_arguments: cli::run_arguments(Some(agent), agent_args)
            .into_iter()
            .map(OsString::from)
            .collect(),
        dependencies: vec![],
        account_name: None, // LocalSystem
        account_password: None,
    };
    let svc = m
        .create_service(&info, ServiceAccess::CHANGE_CONFIG)
        .map_err(err("create service"))?;
    svc.set_description("Runs the Helm agent in the signed-in user's session so a paired phone can control this PC.")
        .map_err(err("set description"))?;
    // If the service process itself crashes, let the SCM restart it.
    let restart = ServiceAction {
        action_type: ServiceActionType::Restart,
        delay: Duration::from_secs(5),
    };
    svc.update_failure_actions(ServiceFailureActions {
        reset_period: ServiceFailureResetPeriod::After(Duration::from_secs(24 * 3600)),
        reboot_msg: None,
        command: None,
        actions: Some(vec![restart.clone(), restart.clone(), restart]),
    })
    .map_err(err("set failure actions"))?;
    println!("{SERVICE_NAME} installed (agent: {})", agent.display());
    Ok(())
}

pub fn uninstall() -> Result<()> {
    let m = manager(ServiceManagerAccess::CONNECT)?;
    let svc = m
        .open_service(
            SERVICE_NAME,
            ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::DELETE,
        )
        .map_err(err("open service"))?;
    if svc
        .query_status()
        .map_err(err("query status"))?
        .current_state
        != ServiceState::Stopped
    {
        let _ = svc.stop();
        wait_for(&svc, ServiceState::Stopped, Duration::from_secs(20))?;
    }
    svc.delete().map_err(err("delete service"))?;
    println!("{SERVICE_NAME} removed");
    Ok(())
}

pub fn start() -> Result<()> {
    let m = manager(ServiceManagerAccess::CONNECT)?;
    let svc = m
        .open_service(
            SERVICE_NAME,
            ServiceAccess::START | ServiceAccess::QUERY_STATUS,
        )
        .map_err(err("open service"))?;
    svc.start::<&OsStr>(&[]).map_err(err("start service"))?;
    wait_for(&svc, ServiceState::Running, Duration::from_secs(20))?;
    println!("{SERVICE_NAME} running");
    Ok(())
}

pub fn stop() -> Result<()> {
    let m = manager(ServiceManagerAccess::CONNECT)?;
    let svc = m
        .open_service(
            SERVICE_NAME,
            ServiceAccess::STOP | ServiceAccess::QUERY_STATUS,
        )
        .map_err(err("open service"))?;
    svc.stop().map_err(err("stop service"))?;
    wait_for(&svc, ServiceState::Stopped, Duration::from_secs(20))?;
    println!("{SERVICE_NAME} stopped");
    Ok(())
}

pub fn status() -> Result<()> {
    let m = manager(ServiceManagerAccess::CONNECT)?;
    let svc = m
        .open_service(SERVICE_NAME, ServiceAccess::QUERY_STATUS)
        .map_err(err("open service"))?;
    let s = svc.query_status().map_err(err("query status"))?;
    println!("{SERVICE_NAME}: {:?}", s.current_state);
    Ok(())
}

fn wait_for(
    svc: &windows_service::service::Service,
    want: ServiceState,
    timeout: Duration,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        let state = svc
            .query_status()
            .map_err(err("query status"))?
            .current_state;
        if state == want {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(format!("timed out waiting for {want:?} (now {state:?})"));
        }
        thread::sleep(Duration::from_millis(250));
    }
}
