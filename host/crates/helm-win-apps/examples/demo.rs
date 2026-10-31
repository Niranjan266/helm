//! Read-only demo: prints the first 20 apps and the current windows, and writes one icon
//! PNG to a temp directory. Run with `cargo run --example demo`.

fn main() {
    let apps = match helm_win_apps::list_apps() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("list_apps failed: {e}");
            return;
        }
    };
    println!("{} apps (first 20):", apps.len());
    for a in apps.iter().take(20) {
        println!(
            "  {:<40} {:?}  {}  {}",
            a.name,
            a.kind,
            a.app_id,
            a.target_path.as_deref().unwrap_or("-")
        );
    }

    match helm_win_apps::list_windows() {
        Ok(ws) => {
            println!("\n{} windows:", ws.len());
            for w in ws {
                println!(
                    "  0x{:08x} pid {:<6} {}{} {:?}  app={}  exe={}",
                    w.hwnd,
                    w.pid,
                    if w.is_foreground { "[fg] " } else { "" },
                    if w.is_minimized { "[min] " } else { "" },
                    w.title,
                    w.app_id.as_deref().unwrap_or("-"),
                    w.exe_path.as_deref().unwrap_or("-"),
                );
            }
        }
        Err(e) => eprintln!("list_windows failed: {e}"),
    }

    if let Some(first) = apps.first() {
        let dir = std::env::temp_dir().join("helm-win-apps-demo");
        match helm_win_apps::IconCache::new(&dir).and_then(|c| {
            let png = c.icon_png(&first.app_id, 64)?;
            Ok((c.path_for(&first.app_id, 64), png.len()))
        }) {
            Ok((path, len)) => println!(
                "\nicon for '{}': {} bytes -> {}",
                first.name,
                len,
                path.display()
            ),
            Err(e) => eprintln!("icon failed: {e}"),
        }
    }
}
