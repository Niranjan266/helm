//! Live, read-only test: captures 1 s of WASAPI loopback from the default playback device.
//! Works whether or not anything is playing (silence is synthesised on the QPC clock).
#![cfg(windows)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use helm_win_audio::{AudioConfig, AudioStreamer, Decoder, Error, default_device, qpc_now_us};

type Packets = Arc<Mutex<Vec<(Vec<u8>, u64)>>>;

#[test]
fn loopback_one_second() {
    match default_device() {
        Ok((id, name)) => eprintln!("default playback device: {name} ({id})"),
        Err(Error::NoDevice) => {
            eprintln!("no playback device on this machine: skipping");
            return;
        }
        Err(e) => panic!("{e}"),
    }
    let packets: Packets = Arc::default();
    let sink = packets.clone();
    let t0 = qpc_now_us();
    let streamer = AudioStreamer::start(AudioConfig::default(), move |p: &[u8], pts: u64| {
        sink.lock().unwrap().push((p.to_vec(), pts));
    })
    .expect("start loopback");
    let start_ms = (qpc_now_us() - t0) as f64 / 1000.0;
    std::thread::sleep(Duration::from_millis(1000));
    assert!(streamer.is_running());
    let mid = streamer.stats();
    let stats = streamer.stop();
    eprintln!("start {start_ms:.1} ms; {stats:#?}");

    let packets = packets.lock().unwrap();
    // 1 s at 20 ms per packet = 50; allow for start-up and the 10 ms silence-fill lag.
    assert!(packets.len() >= 40 && packets.len() <= 60, "{} packets", packets.len());
    assert_eq!(stats.packets as usize, packets.len());
    assert!(mid.packets <= stats.packets);
    assert!(stats.ended_reason.is_none(), "{:?}", stats.ended_reason);
    let dev = stats.device.as_ref().expect("device info");
    assert!(dev.sample_rate >= 8000 && dev.channels >= 1, "{dev:?}");
    assert!(stats.qpc_origin_us.is_some());
    let origin = stats.qpc_origin_us.unwrap();
    assert!((origin - t0).abs() < 2_000_000, "origin {origin} vs start {t0}");

    // pts: strictly increasing, multiples of 20 ms.
    for w in packets.windows(2) {
        assert!(w[1].1 > w[0].1, "pts {} then {}", w[0].1, w[1].1);
    }
    assert!(packets.iter().all(|(_, pts)| pts % 20_000 == 0));
    assert_eq!(packets[0].1, 0);

    // Every packet decodes to 20 ms of stereo.
    let mut dec = Decoder::new(48_000, 2).unwrap();
    let mut pcm = Vec::new();
    for (p, _) in packets.iter() {
        assert!(!p.is_empty() && p.len() < 1500);
        assert_eq!(dec.decode_f32(p, &mut pcm).unwrap(), 960);
    }
    let peak = pcm.iter().fold(0f32, |m, v| m.max(v.abs()));
    eprintln!(
        "decoded {} samples, peak {peak:.3}, silent packets {}",
        pcm.len() / 2,
        stats.silent_packets
    );
}
