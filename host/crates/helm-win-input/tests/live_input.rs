//! Live injection tests. They move the real pointer and type into the focused window, so
//! they are all `#[ignore]`d: run them deliberately with
//! `cargo test --test live_input -- --ignored --test-threads=1`
//! with a scratch Notepad window focused.
#![cfg(windows)]

use std::thread::sleep;
use std::time::Duration;

use helm_win_input::{Injector, InputEvent, MouseButton, name_to_vk};

#[test]
#[ignore = "moves the real mouse pointer"]
fn live_mouse_move_and_scroll() {
    let inj = Injector::new().unwrap();
    for _ in 0..40 {
        inj.inject(&InputEvent::MouseMove { dx: 2.5, dy: 0.0 })
            .unwrap();
        sleep(Duration::from_millis(5));
    }
    for _ in 0..40 {
        inj.inject(&InputEvent::MouseMove { dx: -2.5, dy: 0.0 })
            .unwrap();
        sleep(Duration::from_millis(5));
    }
    inj.inject(&InputEvent::Scroll { dx: 0.0, dy: -0.5 })
        .unwrap();
    inj.inject(&InputEvent::Scroll { dx: 0.0, dy: 0.5 })
        .unwrap();
}

#[test]
#[ignore = "clicks the real mouse"]
fn live_click_and_release_all() {
    let inj = Injector::new().unwrap();
    inj.inject(&InputEvent::MouseButton {
        button: MouseButton::Left,
        down: true,
    })
    .unwrap();
    assert_eq!(inj.held_buttons(), vec![MouseButton::Left]);
    inj.release_all().unwrap();
    assert!(inj.held_buttons().is_empty());
}

#[test]
#[ignore = "types into the focused window"]
fn live_type_text_and_shortcut() {
    let inj = Injector::new().unwrap();
    inj.inject(&InputEvent::Text {
        text: "Helm ✓ 😀\nsecond line".into(),
    })
    .unwrap();
    let shift = name_to_vk("shift").unwrap();
    let home = name_to_vk("home").unwrap();
    inj.inject(&InputEvent::Shortcut {
        keys: vec![shift, home],
    })
    .unwrap();
    inj.inject(&InputEvent::KeyDown {
        vk: name_to_vk("delete").unwrap(),
    })
    .unwrap();
    inj.inject(&InputEvent::KeyUp {
        vk: name_to_vk("delete").unwrap(),
    })
    .unwrap();
    assert!(inj.held_keys().is_empty());
}
