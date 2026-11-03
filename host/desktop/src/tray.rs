//! Notification-area icon and menu (the desktop app owns the tray; the agent runs without one).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, Wry};

use crate::controller::{Controller, NAVIGATE_EVENT};
use crate::status::{self, RunState};
use crate::updater::{self, UpdateView};

pub const TRAY_ID: &str = "helm";
pub const MAIN_WINDOW: &str = "main";

/// Menu items whose text changes.
pub struct TrayItems {
    pause: MenuItem<Wry>,
    menu: Menu<Wry>,
    /// "Update available vX → Install": in the menu only while an update is on offer.
    update: MenuItem<Wry>,
    update_shown: AtomicBool,
}

pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open Helm", true, None::<&str>)?;
    let pair = MenuItem::with_id(app, "pair", "Show pairing QR", true, None::<&str>)?;
    let pause = MenuItem::with_id(app, "pause", "Pause connections", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Helm", true, None::<&str>)?;
    let update = MenuItem::with_id(app, "update", "Install update", true, None::<&str>)?;
    let sep1 = PredefinedMenuItem::separator(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&open, &pair, &sep1, &pause, &sep2, &quit])?;
    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("Helm")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => show_main(app),
            "pair" => show_pairing(app),
            "pause" => toggle_pause(app),
            "update" => updater::install_from_tray(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    app.manage(TrayItems {
        pause,
        menu,
        update,
        update_shown: AtomicBool::new(false),
    });
    Ok(())
}

/// Show, relabel or remove the update item to match the update state.
pub fn set_update(app: &AppHandle, view: &UpdateView) {
    let Some(items) = app.try_state::<TrayItems>() else {
        return;
    };
    let installing = view.state == "installing";
    let text = match (&view.version, installing) {
        (Some(_), true) => Some("Installing update…".to_string()),
        (Some(v), false) => Some(updater::tray_label(v)),
        (None, _) => None,
    };
    let _ = items.update.set_enabled(!installing);
    match text {
        Some(text) => {
            let _ = items.update.set_text(text);
            if !items.update_shown.swap(true, Ordering::Relaxed) {
                let _ = items.menu.insert(&items.update, 0);
            }
        }
        None => {
            if items.update_shown.swap(false, Ordering::Relaxed) {
                let _ = items.menu.remove(&items.update);
            }
        }
    }
}

/// Update the tooltip and the pause item after a status change.
pub fn refresh(app: &AppHandle, run: RunState, online: usize) {
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray.set_tooltip(Some(status::tooltip(run, online)));
    }
    if let Some(items) = app.try_state::<TrayItems>() {
        let _ = items.pause.set_text(status::pause_label(run));
    }
}

pub fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(MAIN_WINDOW) {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// Show the window on the Pair page with an active code.
pub fn show_pairing(app: &AppHandle) {
    show_main(app);
    if let Some(ctl) = app.try_state::<Arc<Controller>>()
        && let Err(e) = ctl.ensure_pairing()
    {
        tracing::warn!("no pairing code: {e}");
    }
    let _ = app.emit(NAVIGATE_EVENT, "pair");
}

fn toggle_pause(app: &AppHandle) {
    let Some(ctl) = app.try_state::<Arc<Controller>>() else {
        return;
    };
    let ctl = ctl.inner().clone();
    let stop = status::pause_stops(ctl.run_state());
    tauri::async_runtime::spawn(async move { ctl.set_paused(stop).await });
}
