//! Live tests against the real shell.
//!
//! The default (non-ignored) tests are read-only. Tests marked `#[ignore]` start a
//! throwaway `notepad.exe`, act only on that process's own window, and close it; run them
//! deliberately with `cargo test -- --ignored --test-threads=1`.
#![cfg(windows)]

use helm_win_apps::{AppKind, Error};

#[test]
fn list_apps_has_entries_and_packaged() {
    let apps = helm_win_apps::list_apps().unwrap();
    assert!(apps.len() > 10, "only {} apps", apps.len());
    assert!(
        apps.iter().any(|a| a.kind == AppKind::Packaged),
        "no packaged apps"
    );
    assert!(
        apps.iter().any(|a| a.kind == AppKind::Desktop),
        "no desktop apps"
    );
    // Sorted and unique.
    let mut ids: Vec<String> = apps.iter().map(|a| a.app_id.to_lowercase()).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), apps.len());
    for w in apps.windows(2) {
        assert!(w[0].name.to_lowercase() <= w[1].name.to_lowercase());
    }
    // Round-trips through serde with snake_case fields.
    let json = serde_json::to_string(&apps[0]).unwrap();
    assert!(json.contains("\"app_id\"") && json.contains("\"target_path\""));
}

#[test]
fn list_windows_works() {
    let ws = helm_win_apps::list_windows().unwrap();
    for w in &ws {
        assert!(w.hwnd != 0);
        assert!(!w.title.trim().is_empty());
    }
    assert!(ws.iter().filter(|w| w.is_foreground).count() <= 1);
}

#[test]
fn icon_for_first_app_is_png() {
    let apps = helm_win_apps::list_apps().unwrap();
    let png = helm_win_apps::app_icon_png(&apps[0].app_id, 48).unwrap();
    let mut reader = png::Decoder::new(std::io::Cursor::new(png))
        .read_info()
        .unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    assert!(info.width <= 48 && info.height <= 48 && info.width > 0);
}

#[test]
fn bad_ids_are_errors_not_panics() {
    assert!(matches!(
        helm_win_apps::app_icon_png("", 32),
        Err(Error::InvalidArgument(_))
    ));
    assert!(matches!(
        helm_win_apps::app_icon_png("No.Such.App_0000000000000!Nope", 32),
        Err(Error::NotFound(_))
    ));
    assert!(matches!(
        helm_win_apps::launch_app("..\\..\\Windows\\notepad.exe"),
        Err(Error::NotFound(_))
    ));
    assert!(matches!(
        helm_win_apps::focus_window(0),
        Err(Error::NotFound(_))
    ));
    assert!(matches!(
        helm_win_apps::close_window(0xDEAD_BEEF),
        Err(Error::NotFound(_))
    ));
    assert!(matches!(
        helm_win_apps::minimize_window(1),
        Err(Error::NotFound(_))
    ));
    assert!(matches!(
        helm_win_apps::open_target("ftp://example.com"),
        Err(Error::InvalidArgument(_))
    ));
    assert!(matches!(
        helm_win_apps::open_target("C:\\no\\such\\helm\\file.txt"),
        Err(Error::NotFound(_))
    ));
}

#[test]
fn callable_from_many_threads() {
    let handles: Vec<_> = (0..4)
        .map(|_| std::thread::spawn(helm_win_apps::list_windows))
        .collect();
    for h in handles {
        h.join().unwrap().unwrap();
    }
}

// ---- side-effecting tests (manual) ------------------------------------------------------

struct Notepad(std::process::Child);
impl Drop for Notepad {
    fn drop(&mut self) {
        let _ = self.0.kill();
    }
}

/// Starts a throwaway classic notepad and waits for its window. Uses System32 notepad.exe;
/// on Windows 11 it may hand off to the packaged Notepad, so windows are matched by title
/// of a unique temp file we open.
fn start_notepad() -> (Notepad, u64, std::path::PathBuf) {
    let file = std::env::temp_dir().join(format!("helm-win-apps-test-{}.txt", std::process::id()));
    std::fs::write(&file, "helm-win-apps test file").unwrap();
    let stem = file.file_stem().unwrap().to_string_lossy().to_string();
    let child = std::process::Command::new("notepad.exe")
        .arg(&file)
        .spawn()
        .unwrap();
    let np = Notepad(child);
    for _ in 0..100 {
        std::thread::sleep(std::time::Duration::from_millis(100));
        if let Some(w) = helm_win_apps::list_windows()
            .unwrap()
            .into_iter()
            .find(|w| w.title.contains(&stem))
        {
            return (np, w.hwnd, file);
        }
    }
    panic!("notepad window did not appear");
}

fn window(hwnd: u64) -> Option<helm_win_apps::WindowInfo> {
    helm_win_apps::list_windows()
        .unwrap()
        .into_iter()
        .find(|w| w.hwnd == hwnd)
}

fn settle() {
    std::thread::sleep(std::time::Duration::from_millis(400));
}

#[test]
#[ignore = "opens and closes a throwaway notepad window"]
fn notepad_window_control_roundtrip() {
    let (_np, hwnd, file) = start_notepad();

    helm_win_apps::minimize_window(hwnd).unwrap();
    settle();
    assert!(window(hwnd).unwrap().is_minimized);

    let r = helm_win_apps::focus_window(hwnd).unwrap();
    println!("focus from minimized: {r:?}");
    settle();
    let w = window(hwnd).unwrap();
    assert!(!w.is_minimized);
    assert!(r.focused && w.is_foreground, "{r:?}");

    helm_win_apps::maximize_window(hwnd).unwrap();
    settle();
    helm_win_apps::restore_window(hwnd).unwrap();
    settle();

    helm_win_apps::close_window(hwnd).unwrap();
    for _ in 0..50 {
        settle();
        if window(hwnd).is_none() {
            break;
        }
    }
    assert!(window(hwnd).is_none(), "window still open");
    let _ = std::fs::remove_file(file);
}

#[test]
#[ignore = "opens and closes a throwaway notepad window via open_target"]
fn open_target_opens_file() {
    let file = std::env::temp_dir().join(format!("helm-win-apps-open-{}.txt", std::process::id()));
    std::fs::write(&file, "x").unwrap();
    let stem = file.file_stem().unwrap().to_string_lossy().to_string();
    helm_win_apps::open_target(file.to_str().unwrap()).unwrap();
    let mut found = None;
    for _ in 0..100 {
        std::thread::sleep(std::time::Duration::from_millis(100));
        found = helm_win_apps::list_windows()
            .unwrap()
            .into_iter()
            .find(|w| w.title.contains(&stem));
        if found.is_some() {
            break;
        }
    }
    let w = found.expect("editor window did not appear");
    println!("opened in: {w:?}");
    helm_win_apps::close_window(w.hwnd).unwrap();
    settle();
    let _ = std::fs::remove_file(file);
}

#[test]
#[ignore = "launches Calculator via the packaged path and closes only the window it created"]
fn launch_packaged_calculator() {
    let apps = helm_win_apps::list_apps().unwrap();
    let Some(calc) = apps
        .iter()
        .find(|a| a.app_id.starts_with("Microsoft.WindowsCalculator_"))
    else {
        eprintln!("Calculator not installed; skipping");
        return;
    };
    let before: Vec<u64> = helm_win_apps::list_windows()
        .unwrap()
        .iter()
        .map(|w| w.hwnd)
        .collect();
    if helm_win_apps::list_windows()
        .unwrap()
        .iter()
        .any(|w| w.app_id.as_deref() == Some(&calc.app_id))
    {
        eprintln!("Calculator already open; skipping so the user's window is not touched");
        return;
    }
    helm_win_apps::launch_app(&calc.app_id).unwrap();
    let mut new_win = None;
    for _ in 0..100 {
        std::thread::sleep(std::time::Duration::from_millis(100));
        new_win = helm_win_apps::list_windows()
            .unwrap()
            .into_iter()
            .find(|w| !before.contains(&w.hwnd) && w.app_id.as_deref() == Some(&calc.app_id));
        if new_win.is_some() {
            break;
        }
    }
    let w = new_win.expect("calculator window did not appear");
    println!("launched: {w:?}");
    helm_win_apps::close_window(w.hwnd).unwrap();
}
