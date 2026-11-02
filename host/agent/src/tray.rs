//! Notification-area (tray) icon with a small menu, on its own thread with a Win32 message
//! loop, plus the "Start with Windows" switch (HKCU Run key).

use std::path::Path;
use std::thread::JoinHandle;

use anyhow::Context;
use tokio::sync::mpsc::UnboundedSender;
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIconBuilder};
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, MSG, PostThreadMessageW, TranslateMessage, WM_QUIT,
};

/// What the user picked in the tray menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayCommand {
    ShowPairing,
    /// g4: show a 6-digit pairing code (phone camera can't scan).
    ShowPairingCode,
    PairedDevices,
    ToggleAutostart,
    CheckUpdates,
    /// g6: ring every connected phone.
    RingPhone,
    Quit,
}

pub struct Tray {
    thread_id: u32,
    join: Option<JoinHandle<()>>,
}

impl Tray {
    /// Show the icon. Menu picks are sent to `commands`.
    pub fn start(commands: UnboundedSender<TrayCommand>) -> anyhow::Result<Tray> {
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<anyhow::Result<u32>>();
        let join = std::thread::Builder::new()
            .name("helm-tray".into())
            .spawn(move || tray_thread(commands, ready_tx))?;
        let thread_id = ready_rx.recv().context("tray thread exited early")??;
        Ok(Tray {
            thread_id,
            join: Some(join),
        })
    }

    /// Remove the icon and end its thread.
    pub fn stop(mut self) {
        self.stop_inner();
    }

    fn stop_inner(&mut self) {
        if let Some(j) = self.join.take() {
            // SAFETY: posting a message to a thread id we own.
            unsafe {
                let _ = PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
            }
            let _ = j.join();
        }
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        self.stop_inner();
    }
}

fn tray_thread(
    commands: UnboundedSender<TrayCommand>,
    ready: std::sync::mpsc::Sender<anyhow::Result<u32>>,
) {
    // SAFETY: plain query of the current thread id.
    let thread_id = unsafe { GetCurrentThreadId() };
    let built = (|| -> anyhow::Result<_> {
        let menu = Menu::new();
        let show = MenuItem::new("Show pairing QR", true, None);
        let show_code = MenuItem::new("Show pairing code", true, None);
        let devices = MenuItem::new("Paired devices", true, None);
        let autostart = CheckMenuItem::new("Start with Windows", true, autostart_enabled(), None);
        let updates = MenuItem::new("Check for updates", true, None);
        let ring = MenuItem::new("Ring my phone", true, None);
        let quit = MenuItem::new("Quit Helm", true, None);
        menu.append_items(&[
            &show,
            &show_code,
            &devices,
            &ring,
            &PredefinedMenuItem::separator(),
            &autostart,
            &updates,
            &PredefinedMenuItem::separator(),
            &quit,
        ])?;
        let ids = [
            (show.id().clone(), TrayCommand::ShowPairing),
            (show_code.id().clone(), TrayCommand::ShowPairingCode),
            (devices.id().clone(), TrayCommand::PairedDevices),
            (autostart.id().clone(), TrayCommand::ToggleAutostart),
            (updates.id().clone(), TrayCommand::CheckUpdates),
            (ring.id().clone(), TrayCommand::RingPhone),
            (quit.id().clone(), TrayCommand::Quit),
        ];
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            if let Some((_, cmd)) = ids.iter().find(|(id, _)| *id == e.id) {
                let _ = commands.send(*cmd);
            }
        }));
        let icon = Icon::from_rgba(icon_rgba(32), 32, 32)?;
        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("Helm agent")
            .with_icon(icon)
            .build()?;
        // Keep the menu items alive as long as the icon.
        Ok((tray, show, show_code, devices, autostart, updates, quit, ring))
    })();
    let keep = match built {
        Ok(k) => {
            let _ = ready.send(Ok(thread_id));
            k
        }
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    let mut msg = MSG::default();
    // SAFETY: standard message loop on this thread.
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    MenuEvent::set_event_handler(None::<fn(MenuEvent)>);
    drop(keep);
}

/// A simple ship's-wheel icon: ring, hub and eight spokes, white on transparent.
pub fn icon_rgba(size: u32) -> Vec<u8> {
    let mut px = vec![0u8; (size * size * 4) as usize];
    let c = (size as f32 - 1.0) / 2.0;
    let r_out = size as f32 * 0.42;
    let r_in = size as f32 * 0.32;
    let hub = size as f32 * 0.12;
    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 - c;
            let dy = y as f32 - c;
            let d = (dx * dx + dy * dy).sqrt();
            let spoke = (0..8).any(|k| {
                let a = k as f32 * std::f32::consts::FRAC_PI_4;
                let off = (dx * a.sin() - dy * a.cos()).abs();
                off < size as f32 * 0.05 && (dx * a.cos() + dy * a.sin()) > 0.0
            });
            let on = (d <= r_out && d >= r_in) || d <= hub || (spoke && d <= size as f32 * 0.49);
            if on {
                let i = ((y * size + x) * 4) as usize;
                px[i..i + 4].copy_from_slice(&[0x4F, 0xC3, 0xF7, 0xFF]);
            }
        }
    }
    px
}

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "Helm";

/// The command line stored in the Run key.
pub fn autostart_command(exe: &Path) -> String {
    format!("\"{}\" --background", exe.display())
}

/// Whether the HKCU Run entry exists.
pub fn autostart_enabled() -> bool {
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegGetValueW};
    use windows::core::HSTRING;
    let key = HSTRING::from(RUN_KEY);
    let value = HSTRING::from(RUN_VALUE);
    // SAFETY: size-only query, no buffer.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            &key,
            &value,
            RRF_RT_REG_SZ,
            None,
            None,
            None,
        )
    };
    status == ERROR_SUCCESS
}

/// Add or remove the HKCU Run entry for this executable.
pub fn set_autostart(on: bool) -> anyhow::Result<()> {
    use windows::Win32::System::Registry::{
        HKEY_CURRENT_USER, REG_SZ, RegDeleteKeyValueW, RegSetKeyValueW,
    };
    use windows::core::HSTRING;
    let key = HSTRING::from(RUN_KEY);
    let value = HSTRING::from(RUN_VALUE);
    if on {
        let exe = std::env::current_exe()?;
        let cmd: Vec<u16> = autostart_command(&exe)
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        // SAFETY: data points at a NUL-terminated UTF-16 buffer of the given byte length.
        unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                &key,
                &value,
                REG_SZ.0,
                Some(cmd.as_ptr() as *const core::ffi::c_void),
                (cmd.len() * 2) as u32,
            )
            .ok()?;
        }
    } else if autostart_enabled() {
        // SAFETY: plain registry call with valid strings.
        unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, &key, &value).ok()? };
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn autostart_command_quotes_path() {
        assert_eq!(
            autostart_command(Path::new(r"C:\Program Files\Helm\helm-agent.exe")),
            "\"C:\\Program Files\\Helm\\helm-agent.exe\" --background"
        );
    }

    #[test]
    fn icon_has_pixels() {
        let px = icon_rgba(32);
        assert_eq!(px.len(), 32 * 32 * 4);
        assert!(px.chunks(4).any(|p| p[3] == 0xFF));
        assert!(px.chunks(4).any(|p| p[3] == 0));
    }
}
