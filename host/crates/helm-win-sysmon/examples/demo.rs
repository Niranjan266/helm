//! Read-only dump of what the crate sees: stats, top processes, monitors and a
//! few seconds of foreground changes, evaluated against sample alert rules.
//!
//! `cargo run --example demo`

use std::time::{Duration, Instant};

use helm_win_sysmon::alerts::{Alerts, Condition, Rule};
use helm_win_sysmon::foreground::ForegroundWatcher;
use helm_win_sysmon::stats::Sampler;
use helm_win_sysmon::{process, window};

fn mb(b: u64) -> u64 {
    b >> 20
}

fn main() {
    let sampler = Sampler::new();
    let mut alerts = Alerts::new(vec![
        Rule { id: "cpu".into(), condition: Condition::CpuAbove { pct: 90.0, for_s: 2 } },
        Rule { id: "ram".into(), condition: Condition::RamAbove { pct: 90.0 } },
        Rule { id: "disk".into(), condition: Condition::DiskFreeBelowPct { mount: None, pct: 10.0 } },
    ]);
    let t0 = Instant::now();

    for _ in 0..3 {
        std::thread::sleep(Duration::from_secs(1));
        let s = sampler.sample().expect("sample");
        println!(
            "CPU {:5.1}%  cores [{}]",
            s.cpu_total,
            s.cpu_per_core.iter().map(|c| format!("{c:.0}")).collect::<Vec<_>>().join(" ")
        );
        println!("RAM {} / {} MB ({:.0}%)", mb(s.ram_used), mb(s.ram_total), s.ram_pct());
        for d in &s.disks {
            println!(
                "  disk {:4} {:>8} / {:>8} MB  r {:>8} B/s  w {:>8} B/s",
                d.mount,
                mb(d.used),
                mb(d.total),
                d.read_bps,
                d.write_bps
            );
        }
        for n in &s.net {
            println!("  net  {:30} rx {:>9} B/s  tx {:>9} B/s", n.name, n.rx_bps, n.tx_bps);
        }
        for g in &s.gpu {
            println!("  gpu  {:30} {:5.1}%  VRAM {} / {} MB", g.name, g.load_pct, mb(g.vram_used), mb(g.vram_total));
        }
        for ev in alerts.evaluate(&s, t0.elapsed().as_millis() as u64) {
            println!("  ALERT {}: {}", ev.rule_id, ev.message);
        }
        println!();
    }

    let mut procs = process::processes().expect("processes");
    procs.sort_by_key(|p| std::cmp::Reverse(p.ram_bytes));
    println!("{} processes; top 8 by RAM:", procs.len());
    for p in procs.iter().take(8) {
        println!(
            "  {:>6} {:28} {:5.1}% {:>6} MB elevated={:?} prio={:?}",
            p.pid,
            p.name,
            p.cpu_pct,
            mb(p.ram_bytes),
            p.is_elevated,
            p.priority
        );
    }

    println!("\nmonitors:");
    for m in window::monitors().unwrap_or_default() {
        println!("  #{} {} primary={} work {:?}", m.index, m.device, m.is_primary, m.work);
    }

    println!("\nforeground changes for 5 s (switch windows to see more):");
    let w = ForegroundWatcher::start(|f| println!("  {} [{}] {:?}", f.exe_name, f.pid, f.title)).expect("watcher");
    std::thread::sleep(Duration::from_secs(5));
    drop(w);
}
