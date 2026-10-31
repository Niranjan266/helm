//! Record system audio (WASAPI loopback) to an Ogg/Opus file, or a raw packet file, and print
//! stream statistics. Read-only: it never plays anything.
//!
//! ```text
//! cargo run --release --example record_opus                      # 5 s → %TEMP%\helm-win-audio\out.opus
//! cargo run --release --example record_opus -- --seconds 10 --bitrate 96 --out C:\tmp\a.opus
//! cargo run --release --example record_opus -- --raw             # out.hpk: [u32 len][u64 pts_us][packet]...
//! ```

#[cfg(windows)]
fn main() {
    use std::io::Write;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use helm_win_audio::ogg::OggOpusWriter;
    use helm_win_audio::{AudioConfig, AudioStreamer, Encoder, OpusMode, default_device};

    let mut seconds = 5.0f64;
    let mut out: Option<std::path::PathBuf> = None;
    let mut raw = false;
    let mut cfg = AudioConfig::default();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--seconds" => seconds = args.next().and_then(|v| v.parse().ok()).expect("--seconds N"),
            "--out" => out = Some(args.next().expect("--out PATH").into()),
            "--bitrate" => cfg.bitrate_kbps = args.next().and_then(|v| v.parse().ok()).expect("--bitrate KBPS"),
            "--frame-ms" => cfg.frame_ms = args.next().and_then(|v| v.parse().ok()).expect("--frame-ms MS"),
            "--mono" => cfg.channels = 1,
            "--cbr" => cfg.cbr = true,
            "--audio-mode" => cfg.mode = OpusMode::Audio,
            "--no-fill" => cfg.fill_silence = false,
            "--raw" => raw = true,
            other => {
                eprintln!("unknown argument {other}");
                std::process::exit(2);
            }
        }
    }
    if let Err(e) = cfg.validate() {
        eprintln!("{e}");
        std::process::exit(2);
    }
    let path = out.unwrap_or_else(|| {
        let dir = std::env::temp_dir().join("helm-win-audio");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(if raw { "out.hpk" } else { "out.opus" })
    });
    match default_device() {
        Ok((_, name)) => println!("device: {name}"),
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }

    // The callback only copies into a bounded channel; the file is written on this thread.
    let (tx, rx) = mpsc::sync_channel::<(Vec<u8>, u64)>(256);
    let started = Instant::now();
    let streamer = AudioStreamer::start(cfg.clone(), move |p: &[u8], pts: u64| {
        let _ = tx.try_send((p.to_vec(), pts));
    })
    .unwrap_or_else(|e| {
        eprintln!("start failed: {e}");
        std::process::exit(1);
    });
    println!(
        "started in {:.1} ms, recording {seconds} s to {}",
        started.elapsed().as_secs_f64() * 1000.0,
        path.display()
    );

    let file = std::io::BufWriter::new(std::fs::File::create(&path).expect("create output"));
    let pre_skip = Encoder::new(&cfg).and_then(|mut e| e.lookahead()).unwrap_or(0);
    let pre_skip_48k = (pre_skip as u64 * 48_000 / cfg.sample_rate as u64) as u16;
    enum Out {
        Ogg(OggOpusWriter<std::io::BufWriter<std::fs::File>>),
        Raw(std::io::BufWriter<std::fs::File>),
    }
    let mut w = if raw {
        Out::Raw(file)
    } else {
        Out::Ogg(
            OggOpusWriter::new(
                file,
                0x4865_6C6D,
                cfg.channels as u8,
                cfg.sample_rate,
                cfg.frame_samples(),
                pre_skip_48k,
            )
            .expect("write headers"),
        )
    };
    let deadline = Instant::now() + Duration::from_secs_f64(seconds);
    let mut last_pts = None;
    let mut gaps = 0u64;
    let write = |w: &mut Out, p: &[u8], pts: u64| match w {
        Out::Ogg(o) => o.write_packet(p).expect("write"),
        Out::Raw(f) => {
            f.write_all(&(p.len() as u32).to_le_bytes()).unwrap();
            f.write_all(&pts.to_le_bytes()).unwrap();
            f.write_all(p).unwrap();
        }
    };
    let frame_us = cfg.frame_ms as u64 * 1000;
    while Instant::now() < deadline {
        if let Ok((p, pts)) = rx.recv_timeout(Duration::from_millis(100)) {
            if let Some(l) = last_pts
                && pts != l + frame_us
            {
                gaps += 1;
            }
            last_pts = Some(pts);
            write(&mut w, &p, pts);
        }
    }
    let stats = streamer.stop();
    while let Ok((p, pts)) = rx.try_recv() {
        write(&mut w, &p, pts);
    }
    match w {
        Out::Ogg(o) => {
            o.finish().expect("finish");
        }
        Out::Raw(mut f) => f.flush().unwrap(),
    }
    println!("{stats:#?}");
    println!(
        "wrote {} ({} bytes), pts gaps: {gaps}",
        path.display(),
        std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0)
    );
}

#[cfg(not(windows))]
fn main() {
    eprintln!("record_opus needs Windows (WASAPI)");
}
