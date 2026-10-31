//! Live checks against this PC.
//!
//! Non-ignored tests are read-only. Tests that kill processes, change
//! priority, move windows or switch desktops are `#[ignore]`d and only touch
//! processes / windows they create themselves (run with
//! `cargo test -- --ignored`; the virtual desktop test also visibly creates
//! and closes a desktop).
#![cfg(windows)]

use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use helm_win_sysmon::foreground::{self, ForegroundWatcher};
use helm_win_sysmon::process::{self, Priority};
use helm_win_sysmon::stats::Sampler;
use helm_win_sysmon::window::{self, Snap, VirtualDesktop};

#[test]
fn sampler_twice_stats_in_range() {
    let s = Sampler::new();
    let a = s.sample().expect("first sample");
    std::thread::sleep(Duration::from_millis(300));
    let b = s.sample().expect("second sample");
    for st in [&a, &b] {
        assert!((0.0..=100.0).contains(&st.cpu_total), "cpu {}", st.cpu_total);
        assert!(!st.cpu_per_core.is_empty());
        assert!(st.cpu_per_core.iter().all(|c| (0.0..=100.0).contains(c)));
        assert!(st.ram_total > 0 && st.ram_used <= st.ram_total);
        assert!(!st.disks.is_empty(), "no disks");
        assert!(st.disks.iter().all(|d| d.used <= d.total));
        for g in &st.gpu {
            assert!((0.0..=100.0).contains(&g.load_pct));
            assert!(!g.name.is_empty());
        }
        assert!(st.cpu_temp_c.is_none());
    }
    assert_eq!(a.interval_ms, 0);
    assert!(b.interval_ms >= 250, "interval {}", b.interval_ms);
    assert!(b.timestamp_ms >= a.timestamp_ms);
    // Rates are bytes/s over ~0.3 s: anything above 100 GB/s is a math bug.
    let max = 100u64 << 30;
    assert!(b.disks.iter().all(|d| d.read_bps < max && d.write_bps < max));
    assert!(b.net.iter().all(|n| n.rx_bps < max && n.tx_bps < max));
}

#[test]
fn sampler_is_shareable_across_threads() {
    let s = std::sync::Arc::new(Sampler::new());
    let s2 = s.clone();
    let h = std::thread::spawn(move || s2.sample().map(|st| st.cpu_per_core.len()));
    let here = s.sample().unwrap().cpu_per_core.len();
    assert_eq!(h.join().unwrap().unwrap(), here);
}

#[test]
fn processes_non_empty_and_include_self() {
    let list = process::processes().expect("processes");
    assert!(list.len() > 5);
    let me = list.iter().find(|p| p.pid == std::process::id()).expect("self in list");
    assert!(me.ram_bytes > 0);
    assert!(me.exe.is_some());
    assert!(me.is_elevated.is_some());
    assert!(list.iter().all(|p| (0.0..=100.0).contains(&p.cpu_pct)));
    let total: f32 = list.iter().map(|p| p.cpu_pct).sum();
    assert!(total <= 100.0 * 1.5, "sum of cpu_pct {total}");
    assert!(process::priority(std::process::id()).is_ok());
}

#[test]
fn foreground_current_and_watcher_start_stop() {
    let _ = foreground::current().expect("current");
    let (tx, rx) = mpsc::channel();
    let w = ForegroundWatcher::start(move |i| {
        let _ = tx.send(i.clone());
    })
    .expect("start");
    // The initial event arrives unless nothing is in the foreground (locked).
    if let Ok(info) = rx.recv_timeout(Duration::from_secs(2)) {
        assert!(info.hwnd != 0);
    }
    drop(w);
}

#[test]
fn monitors_listed_in_order() {
    let m = window::monitors().expect("monitors");
    assert!(!m.is_empty());
    assert!(m[0].is_primary);
    assert!(m.iter().enumerate().all(|(i, mon)| mon.index == i && mon.work.width() > 0));
}

// ---------------------------------------------------------------- side effects

/// A child process we own: a hidden `ping` that idles for a minute.
fn spawn_idle_child() -> Child {
    Command::new("ping")
        .args(["-n", "60", "127.0.0.1"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn ping")
}

#[test]
#[ignore = "kills a process (only one this test spawns)"]
fn kill_own_child() {
    let mut child = spawn_idle_child();
    process::kill(child.id()).expect("kill");
    let status = child.wait().expect("wait");
    assert_eq!(status.code(), Some(1));
}

#[test]
#[ignore = "changes priority (only of a process this test spawns)"]
fn set_priority_own_child() {
    let mut child = spawn_idle_child();
    let pid = child.id();
    process::set_priority(pid, Priority::BelowNormal).expect("set below normal");
    assert_eq!(process::priority(pid).unwrap(), Priority::BelowNormal);
    process::set_priority(pid, Priority::Idle).expect("set idle");
    assert_eq!(process::priority(pid).unwrap(), Priority::Idle);
    let _ = child.kill();
    let _ = child.wait();
}

/// A plain window owned by the calling thread, destroyed on drop.
struct TestWindow(windows::Win32::Foundation::HWND);

impl TestWindow {
    fn new() -> Self {
        use windows::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, WINDOW_EX_STYLE, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
        };
        use windows::core::w;
        // SAFETY: STATIC is a predefined class; the window lives on this thread.
        let h = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!("helm-win-sysmon test window"),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                120,
                120,
                420,
                300,
                None,
                None,
                None,
                None,
            )
        }
        .expect("CreateWindowExW");
        Self(h)
    }
    fn id(&self) -> u64 {
        self.0.0 as usize as u64
    }
    fn is_zoomed(&self) -> bool {
        // SAFETY: our own live window.
        unsafe { windows::Win32::UI::WindowsAndMessaging::IsZoomed(self.0).as_bool() }
    }
}

impl Drop for TestWindow {
    fn drop(&mut self) {
        // SAFETY: destroying our own window on its thread.
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::DestroyWindow(self.0);
        }
    }
}

#[test]
#[ignore = "moves / snaps a window (only one this test creates)"]
fn move_and_snap_own_window() {
    let w = TestWindow::new();
    let mons = window::monitors().unwrap();
    for m in &mons {
        window::move_to_monitor(w.id(), m.index).expect("move");
        let r = window::window_rect(w.id()).unwrap();
        let cx = (r.left + r.right) / 2;
        let cy = (r.top + r.bottom) / 2;
        assert!(
            cx >= m.bounds.left && cx < m.bounds.right && cy >= m.bounds.top && cy < m.bounds.bottom,
            "window {r:?} not on monitor {m:?}"
        );
    }
    assert!(window::move_to_monitor(w.id(), mons.len()).is_err());

    window::move_to_monitor(w.id(), 0).unwrap();
    let work = mons[0].work;
    window::snap(w.id(), Snap::Left).expect("snap left");
    let r = window::window_rect(w.id()).unwrap();
    // Outer rect includes invisible borders (~7 px); visible part is the left half.
    assert!((r.left - work.left).abs() <= 16 && (r.right - (work.left + work.width() / 2)).abs() <= 16, "{r:?}");
    window::snap(w.id(), Snap::Right).expect("snap right");
    let r = window::window_rect(w.id()).unwrap();
    assert!((r.right - work.right).abs() <= 16, "{r:?}");
    window::snap(w.id(), Snap::Maximize).unwrap();
    assert!(w.is_zoomed());
    // Moving a maximized window keeps it maximized.
    window::move_to_monitor(w.id(), mons.len() - 1).unwrap();
    assert!(w.is_zoomed());
    window::snap(w.id(), Snap::Restore).unwrap();
    assert!(!w.is_zoomed());
}

#[test]
fn window_ops_reject_bad_hwnd() {
    assert!(matches!(window::snap(0, Snap::Left), Err(helm_win_sysmon::Error::NotFound(_))));
    assert!(matches!(window::move_to_monitor(0, 0), Err(helm_win_sysmon::Error::NotFound(_))));
}

#[test]
#[ignore = "visibly creates a virtual desktop and closes it again"]
fn virtual_desktop_new_then_close() {
    window::virtual_desktop(VirtualDesktop::New).expect("new");
    std::thread::sleep(Duration::from_millis(800));
    window::virtual_desktop(VirtualDesktop::Close).expect("close");
}
