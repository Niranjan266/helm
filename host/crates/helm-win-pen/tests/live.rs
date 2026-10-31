//! Live pen injection. Ignored by default: it moves a real pen cursor and draws a short stroke
//! wherever the screen centre is. Run only on an idle test machine:
//! `cargo test -- --ignored live_pen_stroke`

#![cfg(windows)]

use helm_win_pen::{Pen, PenPhase, PenSample};

#[test]
#[ignore = "injects real pen input into the desktop"]
fn live_pen_stroke() {
    let pen = Pen::new().expect("CreateSyntheticPointerDevice");
    let (cx, cy) = (800, 500);
    pen.inject(&PenSample::new(cx, cy, PenPhase::Hover)).unwrap();
    for i in 0..20 {
        let mut s = PenSample::new(cx + i * 5, cy, PenPhase::Contact);
        s.pressure = i as f32 / 20.0;
        s.tilt_x = 20.0;
        pen.inject(&s).unwrap();
        assert!(pen.in_contact());
        std::thread::sleep(std::time::Duration::from_millis(8));
    }
    pen.inject(&PenSample::new(cx + 100, cy, PenPhase::Up)).unwrap();
    pen.inject(&PenSample::new(cx + 100, cy, PenPhase::Leave)).unwrap();
    assert!(!pen.in_contact());
}
