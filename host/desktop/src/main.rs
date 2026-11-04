//! `Helm.exe`: the Helm desktop app for Windows. Runs the Helm agent in-process, shows its
//! status in a window, and stays in the notification area when the window is closed.

// A GUI program: no console window, ever (logs go to agent.log in the data folder).
#![windows_subsystem = "windows"]

mod activity;
mod args;
mod autostart;
mod commands;
mod controller;
mod phone;
mod settings;
mod status;
mod tray;
mod updater;

use std::path::Path;
use std::sync::Arc;

use tauri::{Manager, RunEvent, WindowEvent};

use crate::args::SecondLaunch;
use crate::controller::Controller;
use crate::status::RunState;

fn init_logging(data_dir: &Path) {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    if let Ok((file, _)) = helm_agent::desktop::open_log(data_dir) {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_target(false)
            .with_ansi(false)
            .with_writer(std::sync::Mutex::new(file))
            .try_init();
    }
}

fn main() {
    // Physical pixels for capture, cursor and absolute input (see helm-win-capture).
    helm_agent::screen::make_process_dpi_aware();
    let launch = args::parse(std::env::args());
    let data_dir = helm_agent::default_data_dir();
    init_logging(&data_dir);
    tracing::info!(
        "Helm desktop {} starting (hidden={})",
        env!("CARGO_PKG_VERSION"),
        launch.hidden
    );

    let app = tauri::Builder::default()
        // Must be first: a second Helm.exe hands its arguments to us and exits.
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            match args::second_launch(&argv) {
                SecondLaunch::Ignore => {}
                SecondLaunch::Show => tray::show_main(app),
                SecondLaunch::ShowPairing => tray::show_pairing(app),
            }
        }))
        .manage(updater::Updates::default())
        .manage(phone::FramePump::default())
        .invoke_handler(tauri::generate_handler![
            commands::get_status,
            commands::get_phones,
            commands::set_tier,
            commands::revoke_phone,
            commands::get_pairing,
            commands::new_pairing,
            commands::get_code_pairing,
            commands::new_code_pairing,
            commands::get_pending_pairings,
            commands::resolve_pairing,
            commands::set_require_approval,
            commands::ring_phone,
            commands::get_activity,
            commands::get_settings,
            commands::set_autostart,
            commands::set_port,
            commands::set_relay,
            commands::set_paused,
            commands::get_update,
            commands::check_updates,
            commands::install_update,
            commands::open_data_dir,
            commands::quit_app,
            phone::phone_targets,
            phone::phone_request,
            phone::phone_stop,
            phone::phone_ack,
            phone::phone_keyframe,
            phone::phone_input,
        ])
        .setup(move |app| {
            let ctl = Arc::new(Controller::new(app.handle().clone(), data_dir.clone()));
            app.manage(ctl.clone());
            tray::create(app.handle())?;
            updater::start_background(app.handle().clone());
            if !launch.hidden {
                tray::show_main(app.handle());
            }
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                ctl.start().await;
                // First run (nothing paired) or --pair: have a code ready on screen.
                let nothing_paired = ctl.status().phones_total == 0;
                if launch.pair || (nothing_paired && !launch.hidden) {
                    tray::show_pairing(&handle);
                }
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the window keeps Helm (and the phones' connection) running in the tray.
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .build(tauri::generate_context!())
        .expect("could not start the Helm window");

    app.run(|app, event| match event {
        // Only an explicit Quit (exit code set) ends Helm, never "last window closed".
        RunEvent::ExitRequested { api, code, .. } if code.is_none() => api.prevent_exit(),
        RunEvent::Exit => {
            if let Some(ctl) = app.try_state::<Arc<Controller>>() {
                let ctl = ctl.inner().clone();
                tauri::async_runtime::block_on(ctl.stop(RunState::Stopping));
            }
            tracing::info!("Helm desktop stopped");
        }
        _ => {}
    });
}
