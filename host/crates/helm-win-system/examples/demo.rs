//! Prints what helm-win-system sees on this PC. Read-only: changes nothing.
//!
//! cargo run --example demo

use helm_win_system::{audio, display, info, media, net};

fn main() {
    println!("== OS");
    match info::os_info() {
        Ok(os) => println!("{} {} (build {}.{}, {})", os.product_name, os.display_version, os.build, os.ubr, os.version),
        Err(e) => println!("error: {e}"),
    }
    match info::uptime_s() {
        Ok(s) => println!("uptime: {}h {}m", s / 3600, (s % 3600) / 60),
        Err(e) => println!("uptime error: {e}"),
    }
    match info::battery() {
        Ok(Some(b)) => println!("battery: {b:?}"),
        Ok(None) => println!("battery: none"),
        Err(e) => println!("battery error: {e}"),
    }

    println!("\n== Media");
    match media::media_state() {
        Ok(Some(s)) => println!("{s:#?}"),
        Ok(None) => println!("no media session"),
        Err(e) => println!("error: {e}"),
    }
    match media::media_artwork() {
        Ok(Some(a)) => println!("artwork: {} bytes, {}", a.data.len(), a.mime),
        Ok(None) => println!("artwork: none"),
        Err(e) => println!("artwork error: {e}"),
    }

    println!("\n== Volume");
    match audio::volume() {
        Ok(v) => println!("master: {:.0}%{}", v.level * 100.0, if v.muted { " (muted)" } else { "" }),
        Err(e) => println!("error: {e}"),
    }
    match audio::list_sessions() {
        Ok(list) => {
            for s in list {
                println!(
                    "  session {:<24} pid {:<6} {:>3.0}%{}{}",
                    s.name,
                    s.pid,
                    s.level * 100.0,
                    if s.muted { " muted" } else { "" },
                    if s.active { " [active]" } else { "" }
                );
            }
        }
        Err(e) => println!("sessions error: {e}"),
    }

    println!("\n== Displays");
    let t = std::time::Instant::now();
    match display::brightness_list() {
        Ok(list) if list.is_empty() => println!("no displays"),
        Ok(list) => {
            for d in list {
                println!(
                    "  {:?} {:<28} level {:>3} supported {:<5} id {}",
                    d.kind, d.name, d.level, d.supported, d.id
                );
            }
        }
        Err(e) => println!("error: {e}"),
    }
    println!("  (took {} ms)", t.elapsed().as_millis());

    println!("\n== Network adapters");
    match net::mac_addresses() {
        Ok(list) => {
            for a in list {
                println!(
                    "  {:<24} {} up {:<5} ipv4 {:?} ({})",
                    a.adapter_name, a.mac, a.is_up, a.ipv4, a.description
                );
            }
        }
        Err(e) => println!("error: {e}"),
    }
}
