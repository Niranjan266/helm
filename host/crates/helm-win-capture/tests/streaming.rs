//! End-to-end streaming tests. They capture the real screen for a couple of seconds and need a
//! GPU session, so they are `#[ignore]`d by default:
//!
//! ```text
//! cargo test --release -- --ignored --test-threads=1
//! ```

#![cfg(windows)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use helm_win_capture::annexb::{self, nal_type};
use helm_win_capture::{EncoderPreference, StreamConfig, StreamSource, Streamer};

#[derive(Default)]
struct Collected {
    units: Vec<(Vec<u8>, bool, u64)>,
}

fn record(cfg: StreamConfig, secs: f64) -> (Collected, helm_win_capture::StreamStats) {
    let out = Arc::new(Mutex::new(Collected::default()));
    let sink = out.clone();
    let s = Streamer::start(cfg, move |d, k, pts| sink.lock().unwrap().units.push((d.to_vec(), k, pts)))
        .expect("Streamer::start");
    std::thread::sleep(Duration::from_secs_f64(secs / 2.0));
    s.request_keyframe();
    std::thread::sleep(Duration::from_secs_f64(secs / 2.0));
    let stats = s.stop();
    let c = std::mem::take(&mut *out.lock().unwrap());
    (c, stats)
}

fn check_stream(c: &Collected, stats: &helm_win_capture::StreamStats, min_frames: usize) {
    assert!(c.units.len() >= min_frames, "only {} access units ({stats:?})", c.units.len());
    let (first, first_key, first_pts) = &c.units[0];
    assert!(*first_key, "first access unit is a keyframe");
    assert_eq!(*first_pts, 0);
    assert!(annexb::contains_nal_type(first, nal_type::SPS));
    assert!(annexb::contains_nal_type(first, nal_type::PPS));
    for (au, key, _) in &c.units {
        assert_eq!(*key, annexb::is_keyframe(au));
        if *key {
            assert!(annexb::contains_nal_type(au, nal_type::SPS), "keyframe without SPS");
        }
        assert!(annexb::nal_units(au).all(|n| n.slice_type() != Some(annexb::slice_type::B)));
    }
    assert!(c.units.windows(2).all(|w| w[1].2 > w[0].2), "pts strictly increasing");
    // 1 initial + 1 requested at least.
    assert!(c.units.iter().filter(|u| u.1).count() >= 2, "requested keyframe missing");
    assert_eq!(stats.frames_encoded as usize, c.units.len());
}

#[test]
#[ignore = "captures the screen for a few seconds; run with --ignored"]
fn streams_primary_monitor_auto_encoder() {
    let cfg = StreamConfig { max_width: 1280, max_height: 720, fps: 30, ..Default::default() };
    let (c, stats) = record(cfg, 2.0);
    check_stream(&c, &stats, 30);
}

#[test]
#[ignore = "captures the screen for a few seconds; run with --ignored"]
fn streams_primary_monitor_software_encoder() {
    let cfg = StreamConfig {
        max_width: 960,
        max_height: 540,
        fps: 30,
        encoder: EncoderPreference::SoftwareOnly,
        ..Default::default()
    };
    let (c, stats) = record(cfg, 2.0);
    check_stream(&c, &stats, 30);
}

/// Captures a window this test creates itself (never another app's window).
#[test]
#[ignore = "shows a small test window for two seconds; run with --ignored"]
fn streams_own_window() {
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::*;
    use windows::core::w;

    unsafe extern "system" fn wndproc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
        unsafe { DefWindowProcW(h, m, w, l) }
    }

    let (hwnd_tx, hwnd_rx) = std::sync::mpsc::channel::<isize>();
    let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
    let ui = std::thread::spawn(move || unsafe {
        let hinst = GetModuleHandleW(None).unwrap();
        let class = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinst.into(),
            lpszClassName: w!("HelmCaptureWindowTest"),
            ..Default::default()
        };
        RegisterClassW(&class);
        let hwnd = CreateWindowExW(
            WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
            w!("HelmCaptureWindowTest"),
            w!("helm window capture test"),
            WS_OVERLAPPEDWINDOW,
            40,
            40,
            480,
            320,
            None,
            None,
            Some(hinst.into()),
            None,
        )
        .unwrap();
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        hwnd_tx.send(hwnd.0 as isize).unwrap();
        let mut msg = MSG::default();
        while done_rx.try_recv().is_err() {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                DispatchMessageW(&msg);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let _ = DestroyWindow(hwnd);
    });
    let hwnd = hwnd_rx.recv().unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let cfg = StreamConfig { source: StreamSource::Window(hwnd), fps: 30, ..Default::default() };
    let (c, stats) = record(cfg, 2.0);
    done_tx.send(()).unwrap();
    ui.join().unwrap();
    // A static window yields few fresh frames; repeats keep the stream going.
    check_stream(&c, &stats, 20);
    let sps = annexb::nal_units(&c.units[0].0).find(|n| n.nal_type() == nal_type::SPS).unwrap();
    assert!(annexb::sps_profile_level(sps.data).is_some());
}
