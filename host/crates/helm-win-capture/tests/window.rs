//! G3 live test: frame rectangle, thumbnail and low-latency stream of a window this test
//! creates itself (never another app's window). Ignored by default:
//!
//! ```text
//! cargo test --release --test window -- --ignored --nocapture
//! ```

#![cfg(windows)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use helm_win_capture::annexb::{self, nal_type};
use helm_win_capture::{Error, StreamConfig, StreamSource, Streamer, window_frame_rect, window_thumbnail_png};

fn decode_png(bytes: &[u8]) -> (u32, u32, Vec<u8>) {
    let mut reader = png::Decoder::new(std::io::Cursor::new(bytes)).read_info().expect("PNG header");
    let mut buf = vec![0; reader.output_buffer_size().expect("size")];
    let info = reader.next_frame(&mut buf).expect("PNG data");
    buf.truncate(info.buffer_size());
    (info.width, info.height, buf)
}

/// A solid red top-level window on its own UI thread; dropped = destroyed.
struct TestWindow {
    hwnd: isize,
    done: std::sync::mpsc::Sender<u32>,
    ui: Option<std::thread::JoinHandle<()>>,
}

const CMD_QUIT: u32 = 0;
const CMD_MINIMIZE: u32 = 1;
const CMD_RESTORE: u32 = 2;

impl TestWindow {
    fn show() -> TestWindow {
        use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, WPARAM};
        use windows::Win32::Graphics::Gdi::CreateSolidBrush;
        use windows::Win32::System::LibraryLoader::GetModuleHandleW;
        use windows::Win32::UI::WindowsAndMessaging::*;
        use windows::core::w;

        unsafe extern "system" fn wndproc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
            unsafe { DefWindowProcW(h, m, w, l) }
        }
        let (hwnd_tx, hwnd_rx) = std::sync::mpsc::channel::<isize>();
        let (done, rx) = std::sync::mpsc::channel::<u32>();
        let ui = std::thread::spawn(move || unsafe {
            let hinst = GetModuleHandleW(None).unwrap();
            let class = WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: hinst.into(),
                lpszClassName: w!("HelmG3WindowTest"),
                hbrBackground: CreateSolidBrush(COLORREF(0x0000_00FF)), // 0x00BBGGRR = red
                ..Default::default()
            };
            RegisterClassW(&class);
            let hwnd = CreateWindowExW(
                WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                w!("HelmG3WindowTest"),
                w!("helm g3 window test"),
                WS_OVERLAPPEDWINDOW,
                60,
                60,
                640,
                400,
                None,
                None,
                Some(hinst.into()),
                None,
            )
            .unwrap();
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            hwnd_tx.send(hwnd.0 as isize).unwrap();
            let mut msg = MSG::default();
            loop {
                match rx.try_recv() {
                    Ok(CMD_MINIMIZE) => {
                        let _ = ShowWindow(hwnd, SW_SHOWMINNOACTIVE);
                    }
                    Ok(CMD_RESTORE) => {
                        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                    }
                    Ok(_) | Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
                    Err(std::sync::mpsc::TryRecvError::Empty) => {}
                }
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    DispatchMessageW(&msg);
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            let _ = DestroyWindow(hwnd);
        });
        let hwnd = hwnd_rx.recv().unwrap();
        std::thread::sleep(Duration::from_millis(400));
        TestWindow { hwnd, done, ui: Some(ui) }
    }

    fn command(&self, cmd: u32) {
        self.done.send(cmd).unwrap();
        std::thread::sleep(Duration::from_millis(400));
    }
}

impl Drop for TestWindow {
    fn drop(&mut self) {
        let _ = self.done.send(CMD_QUIT);
        if let Some(t) = self.ui.take() {
            let _ = t.join();
        }
    }
}

#[test]
#[ignore = "shows a small red test window for a few seconds; run with --ignored"]
fn own_window_rect_thumbnail_and_low_latency_stream() {
    let win = TestWindow::show();

    // Frame rectangle: visible frame, smaller than or equal to the outer 640x400 (scaled by DPI).
    let rect = window_frame_rect(win.hwnd).expect("frame rect");
    println!("frame rect {rect:?}");
    assert!(rect.width >= 300 && rect.height >= 150, "{rect:?}");
    assert!(window_frame_rect(0).is_none());

    // Thumbnail: longer side = 160, the middle is the red background.
    let png = window_thumbnail_png(win.hwnd, 160).expect("thumbnail");
    let (w, h, rgba) = decode_png(&png);
    println!("thumbnail {w}x{h}, {} bytes", png.len());
    assert_eq!(w.max(h), 160);
    let expect_h = (rect.height as f64 * 160.0 / rect.width as f64).round() as i64;
    assert!((h as i64 - expect_h).abs() <= 1, "aspect: {w}x{h} vs {rect:?}");
    let mid = ((h / 2 * w + w / 2) * 4) as usize;
    let px = &rgba[mid..mid + 4];
    assert!(px[0] > 200 && px[1] < 60 && px[2] < 60, "centre pixel {px:?} is not red");
    assert!(matches!(window_thumbnail_png(win.hwnd, 4), Err(Error::InvalidConfig(_))));

    // Stream it with the gaming tuning (fastest encoder setting, long GOP, 120 fps asked).
    let units = Arc::new(Mutex::new(Vec::<(usize, bool, u64)>::new()));
    let sink = units.clone();
    let cfg = StreamConfig {
        source: StreamSource::Window(win.hwnd),
        max_width: 1280,
        max_height: 720,
        fps: 120,
        gop_frames: 600,
        quality_vs_speed: 0,
        ..Default::default()
    };
    let s = Streamer::start(cfg, move |d, k, pts| {
        assert_eq!(k, annexb::is_keyframe(d));
        if k {
            assert!(annexb::contains_nal_type(d, nal_type::SPS));
        }
        sink.lock().unwrap().push((d.len(), k, pts));
    })
    .expect("Streamer::start on own window");
    let info = s.info().clone();
    println!(
        "stream: {} hw={} {}x{} from {}x{} @{} settings={:?}",
        info.encoder_name, info.hardware, info.width, info.height, info.source_width, info.source_height, info.fps,
        info.codec_settings
    );
    // Window capture content = the DWM frame rectangle (that is what `ma` maps onto).
    assert!(info.source_width.abs_diff(rect.width) <= 2, "{info:?} vs {rect:?}");
    assert!(info.source_height.abs_diff(rect.height) <= 2, "{info:?} vs {rect:?}");
    assert!(info.codec_settings.iter().any(|c| c.starts_with("QualityVsSpeed")));
    std::thread::sleep(Duration::from_millis(1500));
    s.request_keyframe();
    std::thread::sleep(Duration::from_millis(500));
    let stats = s.stop();
    println!("stats: {stats:?}");
    let units = units.lock().unwrap().clone();
    assert!(units.len() >= 40, "only {} access units", units.len());
    assert!(units[0].1, "first access unit is a keyframe");
    assert!(units.windows(2).all(|w| w[1].2 > w[0].2), "pts strictly increasing");
    assert!(units.iter().filter(|u| u.1).count() >= 2, "requested keyframe missing");

    // Minimized: nothing to render.
    win.command(CMD_MINIMIZE);
    assert!(matches!(window_thumbnail_png(win.hwnd, 160), Err(Error::Unsupported(_))));
    win.command(CMD_RESTORE);
    assert!(window_thumbnail_png(win.hwnd, 160).is_ok());
}
