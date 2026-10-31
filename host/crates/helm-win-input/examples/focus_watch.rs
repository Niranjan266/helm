//! Prints text-focus changes for 10 seconds. Click into and out of text boxes to try it.
//!
//! `cargo run --example focus_watch`

use std::time::{Duration, Instant};

use helm_win_input::TextFocusWatcher;

fn main() {
    let start = Instant::now();
    let mut watcher = match TextFocusWatcher::start(move |focused| {
        println!(
            "[{:>5} ms] text focus: {}",
            start.elapsed().as_millis(),
            if focused {
                "YES (keyboard wanted)"
            } else {
                "no"
            }
        );
    }) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("could not start the focus watcher: {e}");
            std::process::exit(1);
        }
    };
    println!("Watching keyboard focus for 10 s...");
    std::thread::sleep(Duration::from_secs(10));
    watcher.stop();
    println!("Stopped.");
}
