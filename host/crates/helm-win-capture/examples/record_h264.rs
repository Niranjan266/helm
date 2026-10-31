//! Record the primary monitor to a raw H.264 (Annex-B) file and print pipeline statistics.
//!
//! ```text
//! cargo run --release --example record_h264 -- [--seconds 5] [--fps 60] [--max 1920x1080]
//!     [--bitrate 12000] [--software | --hardware] [--no-repeat] [--animate] [--out PATH]
//! ```
//!
//! `--animate` shows a small, click-through, non-activating colour square in the bottom-right
//! corner of the primary monitor for the duration of the recording, so the screen changes every
//! frame and the capture → encoded latency is measured on real new frames. It never touches
//! other windows.
//!
//! The output defaults to `%TEMP%\helm-win-capture\out.h264`. The file is validated by parsing
//! its NAL units (SPS, PPS, IDR present, picture count ≈ fps × seconds); if `ffprobe` is on PATH
//! it is run too.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use helm_win_capture::{
    EncoderPreference, StreamConfig, StreamSource, Streamer, annexb, dpi, list_h264_encoders, monitors,
};

struct Args {
    seconds: f64,
    fps: u32,
    max_w: u32,
    max_h: u32,
    bitrate: u32,
    encoder: EncoderPreference,
    repeat: bool,
    animate: bool,
    out: Option<PathBuf>,
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args {
        seconds: 5.0,
        fps: 60,
        max_w: 1920,
        max_h: 1080,
        bitrate: 12_000,
        encoder: EncoderPreference::Auto,
        repeat: true,
        animate: false,
        out: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        let mut val = || it.next().ok_or_else(|| format!("{k} needs a value"));
        match k.as_str() {
            "--seconds" => a.seconds = val()?.parse().map_err(|e| format!("--seconds: {e}"))?,
            "--fps" => a.fps = val()?.parse().map_err(|e| format!("--fps: {e}"))?,
            "--bitrate" => a.bitrate = val()?.parse().map_err(|e| format!("--bitrate: {e}"))?,
            "--max" => {
                let v = val()?;
                let (w, h) = v.split_once('x').ok_or("--max must be WxH")?;
                a.max_w = w.parse().map_err(|e| format!("--max: {e}"))?;
                a.max_h = h.parse().map_err(|e| format!("--max: {e}"))?;
            }
            "--software" => a.encoder = EncoderPreference::SoftwareOnly,
            "--hardware" => a.encoder = EncoderPreference::HardwareOnly,
            "--no-repeat" => a.repeat = false,
            "--animate" => a.animate = true,
            "--out" => a.out = Some(PathBuf::from(val()?)),
            "-h" | "--help" => {
                return Err("usage: record_h264 [--seconds N] [--fps N] [--max WxH] [--bitrate KBPS] \
                            [--software|--hardware] [--no-repeat] [--animate] [--out PATH]"
                    .into());
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(a)
}

/// A tiny animated square that never takes focus or input (WS_EX_NOACTIVATE + click-through).
mod animator {
    use super::*;
    use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows::Win32::Graphics::Gdi::{CreateSolidBrush, DeleteObject, FillRect, GetDC, ReleaseDC};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::*;
    use windows::core::w;

    unsafe extern "system" fn wndproc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
        // SAFETY: default handling for every message.
        unsafe { DefWindowProcW(h, m, w, l) }
    }

    pub fn spawn(stop: Arc<AtomicBool>, x: i32, y: i32, size: i32) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            dpi::set_process_per_monitor_v2();
            // SAFETY: ordinary window creation / GDI painting on this thread's own window.
            unsafe {
                let hinst = GetModuleHandleW(None).unwrap_or_default();
                let class = WNDCLASSW {
                    lpfnWndProc: Some(wndproc),
                    hInstance: hinst.into(),
                    lpszClassName: w!("HelmCaptureAnimator"),
                    ..Default::default()
                };
                RegisterClassW(&class);
                let ex = WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED | WS_EX_TRANSPARENT;
                let Ok(hwnd) = CreateWindowExW(
                    ex,
                    w!("HelmCaptureAnimator"),
                    w!("helm capture test"),
                    WS_POPUP,
                    x,
                    y,
                    size,
                    size,
                    None,
                    None,
                    Some(hinst.into()),
                    None,
                ) else {
                    eprintln!("animator: CreateWindowExW failed");
                    return;
                };
                let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);
                let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                let start = Instant::now();
                let mut msg = MSG::default();
                while !stop.load(Ordering::Relaxed) {
                    while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                        DispatchMessageW(&msg);
                    }
                    let t = start.elapsed().as_secs_f64();
                    let r = (127.0 + 127.0 * (t * 5.0).sin()) as u32;
                    let g = (127.0 + 127.0 * (t * 7.0).sin()) as u32;
                    let b = (127.0 + 127.0 * (t * 11.0).sin()) as u32;
                    let dc = GetDC(Some(hwnd));
                    let brush = CreateSolidBrush(COLORREF(r | (g << 8) | (b << 16)));
                    FillRect(dc, &RECT { left: 0, top: 0, right: size, bottom: size }, brush);
                    let _ = DeleteObject(brush.into());
                    ReleaseDC(Some(hwnd), dc);
                    std::thread::sleep(Duration::from_millis(4));
                }
                let _ = DestroyWindow(hwnd);
            }
        })
    }
}

fn which(exe: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(exe)).find(|p| p.is_file())
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    // What the agent should do at startup (or declare it in the manifest).
    dpi::set_process_per_monitor_v2();

    println!("monitors:");
    for m in monitors() {
        println!(
            "  {} \"{}\" {}x{} at ({}, {}) scale {:.2}{}",
            m.id,
            m.name,
            m.width,
            m.height,
            m.x,
            m.y,
            m.scale,
            if m.is_primary { " [primary]" } else { "" }
        );
    }
    match list_h264_encoders() {
        Ok(list) => {
            println!("H.264 encoder MFTs:");
            for e in list {
                println!("  {} ({})", e.name, if e.hardware { "hardware" } else { "software" });
            }
        }
        Err(e) => println!("encoder enumeration failed: {e}"),
    }

    let out = args.out.clone().unwrap_or_else(|| std::env::temp_dir().join("helm-win-capture").join("out.h264"));
    if let Some(dir) = out.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let file = match File::create(&out) {
        Ok(f) => Arc::new(Mutex::new(BufWriter::new(f))),
        Err(e) => {
            eprintln!("cannot create {}: {e}", out.display());
            return ExitCode::FAILURE;
        }
    };

    let stop_anim = Arc::new(AtomicBool::new(false));
    let anim = if args.animate {
        let primary = monitors().into_iter().find(|m| m.is_primary);
        primary.map(|m| {
            let size = (160.0 * m.scale) as i32;
            let x = m.x + m.width as i32 - size - (40.0 * m.scale) as i32;
            let y = m.y + m.height as i32 - size - (120.0 * m.scale) as i32;
            let h = animator::spawn(stop_anim.clone(), x, y, size);
            std::thread::sleep(Duration::from_millis(300));
            h
        })
    } else {
        None
    };

    let cfg = StreamConfig {
        source: StreamSource::PrimaryMonitor,
        max_width: args.max_w,
        max_height: args.max_h,
        fps: args.fps,
        bitrate_kbps: args.bitrate,
        gop_frames: 120,
        repeat_idle_frames: args.repeat,
        encoder: args.encoder,
    };
    let sink = file.clone();
    let keyframe_pts = Arc::new(Mutex::new(Vec::<u64>::new()));
    let kf = keyframe_pts.clone();
    let streamer = match Streamer::start(cfg, move |data, key, pts| {
        let _ = sink.lock().unwrap().write_all(data);
        if key {
            kf.lock().unwrap().push(pts);
        }
    }) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Streamer::start failed: {e}");
            stop_anim.store(true, Ordering::Relaxed);
            return ExitCode::FAILURE;
        }
    };
    let info = streamer.info().clone();
    println!("stream:");
    println!(
        "  encoder      {} ({}, {}, {} input, profile {})",
        info.encoder_name,
        if info.hardware { "hardware" } else { "software" },
        if info.async_mft { "async MFT" } else { "sync MFT" },
        if info.gpu_input { "D3D11 texture" } else { "system memory" },
        info.profile
    );
    println!("  adapter      {}", info.adapter);
    println!(
        "  size         {}x{} -> {}x{} @ {} fps, {} kbps CBR",
        info.source_width, info.source_height, info.width, info.height, info.fps, info.bitrate_kbps
    );
    println!("  border off   {}, MinUpdateInterval set {}", info.border_disabled, info.min_update_interval_set);
    println!("  codec api    {}", info.codec_settings.join(", "));

    let started = Instant::now();
    let total = Duration::from_secs_f64(args.seconds);
    let mut forced = false;
    while started.elapsed() < total {
        std::thread::sleep(Duration::from_millis(50));
        if !forced && started.elapsed() >= total / 2 {
            streamer.request_keyframe();
            forced = true;
            println!("  (keyframe requested at {:.2} s)", started.elapsed().as_secs_f64());
        }
        if !streamer.is_running() {
            break;
        }
    }
    let stats = streamer.stop();
    stop_anim.store(true, Ordering::Relaxed);
    if let Some(h) = anim {
        let _ = h.join();
    }
    let _ = file.lock().unwrap().flush();

    println!("stats:");
    println!("  elapsed      {:.2} s", stats.elapsed_s);
    println!(
        "  frames       encoded {} ({:.1} fps), fresh {} ({:.1} fps), repeated {}, dropped {}, paced out {}",
        stats.frames_encoded,
        stats.encoded_fps,
        stats.frames_captured,
        stats.captured_fps,
        stats.frames_repeated,
        stats.frames_dropped,
        stats.frames_paced_out
    );
    println!(
        "  latency      median {:.2} ms, p95 {:.2} ms, max {:.2} ms ({} fresh frames)",
        stats.latency_median_ms, stats.latency_p95_ms, stats.latency_max_ms, stats.latency_samples
    );
    println!("  keyframes    {} at pts(ms) {:?}", stats.keyframes, {
        keyframe_pts.lock().unwrap().iter().map(|p| p / 1000).collect::<Vec<_>>()
    });
    println!("  output       {} bytes, {:.0} kbps", stats.bytes, stats.bitrate_kbps);
    if let Some(r) = &stats.ended_reason {
        println!("  ended        {r}");
    }

    // Validate the file.
    let bytes = match std::fs::read(&out) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("cannot read back {}: {e}", out.display());
            return ExitCode::FAILURE;
        }
    };
    let s = annexb::summarize(&bytes);
    let sps_info = annexb::nal_units(&bytes)
        .find(|n| n.nal_type() == annexb::nal_type::SPS)
        .and_then(|n| annexb::sps_profile_level(n.data));
    println!("file {} ({} bytes):", out.display(), bytes.len());
    println!(
        "  NAL units {}, SPS {}, PPS {}, IDR pictures {}, pictures {}, first NAL type {:?}, profile/level {:?}",
        s.nal_units, s.sps, s.pps, s.idr_pictures, s.pictures, s.first_nal_type, sps_info
    );
    println!("  slices: I {}, P {}, B {}", s.i_slices, s.p_slices, s.b_slices);
    let mut ok = true;
    let mut check = |cond: bool, what: &str| {
        println!("  [{}] {what}", if cond { "ok" } else { "FAIL" });
        ok &= cond;
    };
    check(s.sps >= 1 && s.pps >= 1, "SPS and PPS present");
    check(s.idr_pictures >= 1, "IDR present");
    check(matches!(s.first_nal_type, Some(7) | Some(9)), "stream starts with SPS (or AUD)");
    check(s.pictures as u64 == stats.frames_encoded, "picture count == access units delivered");
    let expected = args.fps as f64 * stats.elapsed_s;
    if !(args.repeat && args.animate) {
        // Without --animate the frame count depends on what the user's screen is doing.
        println!("  [info] picture count {} vs fps × duration {expected:.0}", s.pictures);
    } else {
        check(
            (s.pictures as f64) >= expected * 0.9 && (s.pictures as f64) <= expected * 1.1 + 2.0,
            &format!("picture count {} ≈ fps × duration ({expected:.0})", s.pictures),
        );
    }
    check(s.sps >= s.idr_pictures, "every IDR carries SPS/PPS");
    check(s.b_slices == 0, "no B-frames");

    if let Some(ffprobe) = which("ffprobe.exe").or_else(|| which("ffprobe")) {
        println!("ffprobe ({}):", ffprobe.display());
        match std::process::Command::new(ffprobe)
            .args([
                "-v",
                "error",
                "-count_frames",
                "-select_streams",
                "v:0",
                "-show_entries",
                "stream=codec_name,profile,width,height,nb_read_frames",
                "-of",
                "default=nw=1",
            ])
            .arg(&out)
            .output()
        {
            Ok(o) => {
                print!("{}", String::from_utf8_lossy(&o.stdout));
                eprint!("{}", String::from_utf8_lossy(&o.stderr));
                check(o.status.success(), "ffprobe decodes the stream");
            }
            Err(e) => println!("  could not run ffprobe: {e}"),
        }
    } else {
        println!("ffprobe not on PATH; validated with the built-in NAL parser only");
    }
    if ok { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}
