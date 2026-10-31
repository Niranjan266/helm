//! List monitors and save PNG screenshots of each monitor and of the whole virtual desktop.
//!
//! ```text
//! cargo run --release --example screenshot -- [OUT_DIR]
//! ```
//! Defaults to `%TEMP%\helm-win-capture`.

use std::time::Instant;

use helm_win_capture::{dpi, monitors, screenshot_png};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    dpi::set_process_per_monitor_v2();
    let dir = std::env::args()
        .nth(1)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("helm-win-capture"));
    std::fs::create_dir_all(&dir)?;
    for m in monitors() {
        let t = Instant::now();
        let png = screenshot_png(Some(&m.id))?;
        // `\\.\DISPLAY1` -> `DISPLAY1` (backslashes are not allowed in file names).
        let name: String = m.id.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
        let path = dir.join(format!("{name}.png"));
        std::fs::write(&path, &png)?;
        println!(
            "{} \"{}\" {}x{} scale {:.2}{} -> {} ({} KiB, {} ms)",
            m.id,
            m.name,
            m.width,
            m.height,
            m.scale,
            if m.is_primary { " [primary]" } else { "" },
            path.display(),
            png.len() / 1024,
            t.elapsed().as_millis()
        );
    }
    let t = Instant::now();
    let png = screenshot_png(None)?;
    let path = dir.join("virtual-desktop.png");
    std::fs::write(&path, &png)?;
    println!("virtual desktop -> {} ({} KiB, {} ms)", path.display(), png.len() / 1024, t.elapsed().as_millis());
    Ok(())
}
