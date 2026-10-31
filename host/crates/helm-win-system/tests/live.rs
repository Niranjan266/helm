//! Live, read-only checks against this PC.
//!
//! These never pause/play media, never change volume or brightness except to
//! write back the value just read, and never run power actions. Anything with
//! a visible side effect is `#[ignore]`d.
#![cfg(windows)]

use std::sync::mpsc;
use std::time::Duration;

use helm_win_system::{audio, display, info, media, net};

#[test]
fn media_state_reads() {
    let s = media::media_state().expect("media_state");
    if let Some(s) = &s {
        if let (Some(p), Some(d)) = (s.position_ms, s.duration_ms) {
            assert!(p >= 0 && p <= d);
        }
    }
    // Artwork read must not error either (it may be None).
    media::media_artwork().expect("media_artwork");
}

#[test]
fn media_watcher_starts_and_stops() {
    let (tx, rx) = mpsc::channel();
    let w = media::MediaWatcher::start(move |s| {
        let _ = tx.send(s);
    })
    .expect("watcher start");
    // The initial state is always delivered.
    rx.recv_timeout(Duration::from_secs(5)).expect("initial media state");
    drop(w);
}

#[test]
fn volume_reads_in_range_and_round_trips() {
    let v = audio::volume().expect("volume");
    assert!((0.0..=1.0).contains(&v.level), "{v:?}");
    // Write back exactly what we read: no audible change.
    audio::set_volume(v.level).expect("set_volume same");
    audio::set_muted(v.muted).expect("set_muted same");
    let v2 = audio::volume().expect("volume again");
    assert!((v2.level - v.level).abs() < 0.01, "{v:?} vs {v2:?}");
    assert_eq!(v2.muted, v.muted);
}

#[test]
fn volume_watcher_starts_and_stops() {
    let w = audio::VolumeWatcher::start(|_| {}).expect("watcher start");
    drop(w);
}

#[test]
fn audio_sessions_list() {
    let sessions = audio::list_sessions().expect("list_sessions");
    for s in &sessions {
        assert!(!s.id.is_empty());
        assert!((0.0..=1.0).contains(&s.level));
    }
}

#[test]
fn brightness_list_and_round_trip() {
    let list = display::brightness_list().expect("brightness_list");
    for d in &list {
        assert!(d.level <= 100);
        if d.supported {
            display::set_brightness(&d.id, d.level).expect("set same brightness");
        }
    }
    // Unknown ids are reported, not ignored.
    assert!(display::set_brightness("ddc:nope#0", 50).is_err());
    assert!(display::set_brightness("bogus", 50).is_err());
}

#[test]
fn mac_list_non_empty() {
    let macs = net::mac_addresses().expect("mac_addresses");
    assert!(!macs.is_empty(), "no adapters with a MAC");
    for a in &macs {
        net::parse_mac(&a.mac).expect("well-formed MAC");
    }
}

#[test]
fn os_info_and_uptime() {
    let os = info::os_info().expect("os_info");
    assert!(os.product_name.contains("Windows"), "{os:?}");
    assert!(os.build > 0);
    assert!(info::uptime_s().expect("uptime") > 0);
    let _ = info::battery().expect("battery");
}

// ----- side effects: never run by default ---------------------------------

#[test]
#[ignore = "toggles the user's media playback twice"]
fn media_toggle_twice() {
    media::media_control(media::MediaAction::TogglePlayPause).unwrap();
    std::thread::sleep(Duration::from_secs(1));
    media::media_control(media::MediaAction::TogglePlayPause).unwrap();
}

#[test]
#[ignore = "audibly changes the master volume, then restores it"]
fn volume_change_and_restore() {
    let v = audio::volume().unwrap();
    let target = if v.level > 0.5 { v.level - 0.1 } else { v.level + 0.1 };
    audio::set_volume(target).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    assert!((audio::volume().unwrap().level - target).abs() < 0.01);
    audio::set_volume(v.level).unwrap();
}
