//! Prints clipboard changes (made by other programs) until Enter is pressed.
//!
//! `cargo run --example clipboard_watch`

use helm_win_input::{ClipboardWatcher, clipboard};

fn main() {
    let watcher = ClipboardWatcher::start(|change| {
        print!("clipboard #{}: ", change.sequence);
        if change.has_text {
            match clipboard::get_text() {
                Ok(Some(t)) => {
                    let preview: String = t.chars().take(60).collect();
                    println!("text ({} chars): {preview:?}", t.chars().count());
                }
                Ok(None) => println!("text (empty)"),
                Err(e) => println!("text (read failed: {e})"),
            }
        } else if change.has_image {
            match clipboard::get_image_png() {
                Ok(Some(png)) => println!("image ({} bytes as PNG)", png.len()),
                Ok(None) => println!("image (gone)"),
                Err(e) => println!("image (convert failed: {e})"),
            }
        } else {
            println!("other formats");
        }
    });
    let mut watcher = match watcher {
        Ok(w) => w,
        Err(e) => {
            eprintln!("could not start the clipboard watcher: {e}");
            std::process::exit(1);
        }
    };
    println!("Copy something in another program. Press Enter to quit.");
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
    watcher.stop();
}
