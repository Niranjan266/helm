//! Pure motion maths: pointer acceleration and fractional accumulation.

/// One wheel notch in Windows wheel units (`WHEEL_DELTA`).
pub const WHEEL_DELTA: f32 = 120.0;

/// Speed (pixels per event) below which motion is not accelerated.
const ACCEL_THRESHOLD: f32 = 1.5;
/// Extra gain per pixel of speed above the threshold.
const ACCEL_SLOPE: f32 = 0.09;
/// Upper bound for the acceleration gain (before `speed`).
const ACCEL_MAX_GAIN: f32 = 3.5;

/// Optional pointer-acceleration curve for touchpad deltas.
///
/// The gain depends on the length of the delta (a proxy for finger speed when events
/// arrive at a steady rate): slow, precise movements stay 1:1, fast flicks are amplified
/// up to `ACCEL_MAX_GAIN`×. The whole result is then scaled by `speed` (the user's speed
/// slider, 1.0 = default). Direction is always preserved. Non-finite input yields `(0, 0)`.
pub fn accelerate(dx: f32, dy: f32, speed: f32) -> (f32, f32) {
    if !dx.is_finite() || !dy.is_finite() || !speed.is_finite() || speed <= 0.0 {
        return (0.0, 0.0);
    }
    let mag = dx.hypot(dy);
    if mag == 0.0 {
        return (0.0, 0.0);
    }
    let gain = (1.0 + ACCEL_SLOPE * (mag - ACCEL_THRESHOLD).max(0.0)).min(ACCEL_MAX_GAIN);
    (dx * gain * speed, dy * gain * speed)
}

/// Accumulates fractional values and hands out whole units, keeping the remainder.
///
/// Used for sub-pixel mouse motion (unit = 1 px) and fractional scroll notches
/// (scale = `WHEEL_DELTA`). Non-finite input is ignored.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Accumulator {
    rem: f64,
}

impl Accumulator {
    pub const fn new() -> Self {
        Self { rem: 0.0 }
    }

    /// Adds `value * scale` and returns the whole part (truncated toward zero).
    pub fn push(&mut self, value: f32, scale: f32) -> i32 {
        let add = value as f64 * scale as f64;
        if !add.is_finite() {
            return 0;
        }
        // Clamp so a single absurd value cannot overflow i32.
        let total = (self.rem + add).clamp(-1.0e9, 1.0e9);
        let whole = total.trunc();
        self.rem = total - whole;
        whole as i32
    }

    /// The fractional remainder still waiting.
    pub fn remainder(&self) -> f64 {
        self.rem
    }

    pub fn reset(&mut self) {
        self.rem = 0.0;
    }
}

/// Two-axis accumulator.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Accumulator2 {
    pub x: Accumulator,
    pub y: Accumulator,
}

impl Accumulator2 {
    pub fn push(&mut self, dx: f32, dy: f32, scale: f32) -> (i32, i32) {
        (self.x.push(dx, scale), self.y.push(dy, scale))
    }

    pub fn reset(&mut self) {
        self.x.reset();
        self.y.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn accel_zero_and_invalid() {
        assert_eq!(accelerate(0.0, 0.0, 1.0), (0.0, 0.0));
        assert_eq!(accelerate(f32::NAN, 1.0, 1.0), (0.0, 0.0));
        assert_eq!(accelerate(1.0, f32::INFINITY, 1.0), (0.0, 0.0));
        assert_eq!(accelerate(3.0, 4.0, 0.0), (0.0, 0.0));
        assert_eq!(accelerate(3.0, 4.0, -1.0), (0.0, 0.0));
    }

    #[test]
    fn accel_slow_motion_is_linear_in_speed() {
        assert_eq!(accelerate(1.0, 0.0, 1.0), (1.0, 0.0));
        assert_eq!(accelerate(0.6, -0.8, 2.0), (1.2, -1.6));
    }

    #[test]
    fn accel_preserves_direction() {
        for &(dx, dy) in &[(3.0, 4.0), (-10.0, 2.0), (0.0, -25.0), (40.0, 40.0)] {
            let (ox, oy) = accelerate(dx, dy, 1.3);
            assert!(close(ox * dy, oy * dx), "{dx},{dy} -> {ox},{oy}");
            assert_eq!((ox.signum(), oy.signum()), (dx.signum(), dy.signum()));
            assert!(ox.hypot(oy) >= dx.hypot(dy) * 1.3 - 1e-4);
        }
    }

    #[test]
    fn accel_is_monotonic_and_capped() {
        let mut last = 0.0;
        for i in 1..400 {
            let d = i as f32 * 0.25;
            let (out, _) = accelerate(d, 0.0, 1.0);
            assert!(out >= last, "not monotonic at {d}");
            assert!(out <= d * ACCEL_MAX_GAIN + 1e-3);
            last = out;
        }
        let (big, _) = accelerate(1000.0, 0.0, 1.0);
        assert!(close(big, 1000.0 * ACCEL_MAX_GAIN));
    }

    #[test]
    fn accumulator_sub_pixel() {
        let mut a = Accumulator::new();
        let outs: Vec<i32> = (0..10).map(|_| a.push(0.3, 1.0)).collect();
        assert_eq!(outs.iter().sum::<i32>(), 3);
        assert_eq!(outs[..3], [0, 0, 0]);
        assert_eq!(outs[3], 1);
        assert!(a.remainder().abs() < 1e-6);
    }

    #[test]
    fn accumulator_negative_and_mixed() {
        let mut a = Accumulator::new();
        assert_eq!(a.push(-0.6, 1.0), 0);
        assert_eq!(a.push(-0.6, 1.0), -1);
        assert_eq!(a.push(0.5, 1.0), 0);
        assert!((a.remainder() - 0.3).abs() < 1e-6);
        assert_eq!(a.push(2.75, 1.0), 3);
        assert_eq!(a.push(f32::NAN, 1.0), 0);
    }

    #[test]
    fn scroll_fractional_notches() {
        let mut s = Accumulator2::default();
        // Quarter notches give 30 wheel units each, horizontally too.
        assert_eq!(s.push(0.25, -0.25, WHEEL_DELTA), (30, -30));
        // 0.001 notch = 0.12 units: nothing yet, but it adds up.
        let mut total = 0;
        for _ in 0..100 {
            total += s.push(0.0, 0.001, WHEEL_DELTA).1;
        }
        assert_eq!(total, 12);
        assert_eq!(s.push(1.0, 1.0, WHEEL_DELTA), (120, 120));
        s.reset();
        assert_eq!(s, Accumulator2::default());
    }
}
