//! Laser-pointer drawing, independent of Windows: normalized → overlay pixel mapping and a
//! premultiplied BGRA canvas with a glowing red dot and a short fading trail. The Windows
//! overlay (`win.rs`) copies the dirty rectangle into its layered window.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// How long trail points stay visible.
pub const TRAIL: Duration = Duration::from_millis(220);
/// Most trail points kept.
pub const MAX_TRAIL: usize = 24;
/// Glow reaches this many core radii.
const GLOW: f32 = 2.6;

/// Map normalized (`x`,`y`) in 0..1 to a pixel centre inside a `w`×`h` overlay.
/// Values outside 0..1 (or NaN) are clamped to the edge.
pub fn to_local(x: f32, y: f32, w: u32, h: u32) -> (f32, f32) {
    let c = |v: f32| if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) };
    let max_x = w.saturating_sub(1) as f32;
    let max_y = h.saturating_sub(1) as f32;
    ((c(x) * w as f32).min(max_x), (c(y) * h as f32).min(max_y))
}

/// Core dot radius for a monitor `h` pixels tall (about 1 % of the height).
pub fn dot_radius(h: u32) -> f32 {
    (h as f32 / 110.0).clamp(6.0, 22.0)
}

/// Pixel rectangle `[x0,x1) × [y0,y1)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IRect {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

impl IRect {
    pub fn is_empty(&self) -> bool {
        self.x1 <= self.x0 || self.y1 <= self.y0
    }
    pub fn union(self, o: IRect) -> IRect {
        if self.is_empty() {
            return o;
        }
        if o.is_empty() {
            return self;
        }
        IRect {
            x0: self.x0.min(o.x0),
            y0: self.y0.min(o.y0),
            x1: self.x1.max(o.x1),
            y1: self.y1.max(o.y1),
        }
    }
    pub fn clip(self, w: u32, h: u32) -> IRect {
        IRect {
            x0: self.x0.clamp(0, w as i32),
            y0: self.y0.clamp(0, h as i32),
            x1: self.x1.clamp(0, w as i32),
            y1: self.y1.clamp(0, h as i32),
        }
    }
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x0 && x < self.x1 && y >= self.y0 && y < self.y1
    }
}

fn union_opt(a: Option<IRect>, b: Option<IRect>) -> Option<IRect> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.union(b)),
        (a, b) => a.or(b),
    }
}

/// Premultiplied BGRA pixels (`0xAARRGGBB` as little-endian u32), top-down rows.
pub struct Canvas {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u32>,
}

impl Canvas {
    pub fn new(width: u32, height: u32) -> Canvas {
        Canvas {
            width,
            height,
            pixels: vec![0; width as usize * height as usize],
        }
    }

    pub fn pixel(&self, x: u32, y: u32) -> u32 {
        self.pixels[(y * self.width + x) as usize]
    }

    pub fn clear(&mut self, r: IRect) {
        let r = r.clip(self.width, self.height);
        for y in r.y0..r.y1 {
            let row = y as usize * self.width as usize;
            self.pixels[row + r.x0 as usize..row + r.x1 as usize].fill(0);
        }
    }

    /// Draw one glowing dot (core `radius`, `strength` 0..1) "over" what is there.
    /// Returns the touched rectangle (clipped).
    pub fn dot(&mut self, cx: f32, cy: f32, radius: f32, strength: f32) -> IRect {
        let reach = radius * GLOW;
        let r = IRect {
            x0: (cx - reach).floor() as i32,
            y0: (cy - reach).floor() as i32,
            x1: (cx + reach).ceil() as i32 + 1,
            y1: (cy + reach).ceil() as i32 + 1,
        }
        .clip(self.width, self.height);
        let strength = strength.clamp(0.0, 1.0);
        for y in r.y0..r.y1 {
            for x in r.x0..r.x1 {
                let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
                let (a, rgb) = dot_sample(d, radius);
                let a = a * strength;
                if a <= 0.002 {
                    continue;
                }
                let i = (y as u32 * self.width + x as u32) as usize;
                self.pixels[i] = over(self.pixels[i], a, rgb);
            }
        }
        r
    }
}

/// Alpha and colour of the dot at distance `d` from its centre: a white-hot centre, a solid
/// red core with a soft edge, then a red glow fading to nothing at `GLOW` radii.
fn dot_sample(d: f32, radius: f32) -> (f32, [f32; 3]) {
    let red = [1.0, 0.12, 0.08];
    if d <= radius {
        let t = (d / radius).clamp(0.0, 1.0);
        // Centre 35 % blends to white.
        let hot = (1.0 - t / 0.35).clamp(0.0, 1.0) * 0.85;
        let rgb = [
            red[0],
            red[1] + (1.0 - red[1]) * hot,
            red[2] + (1.0 - red[2]) * hot,
        ];
        // Anti-aliased rim over the last pixel.
        let edge = (radius - d + 0.5).clamp(0.0, 1.0);
        let a = edge.max(glow(d, radius));
        (a, rgb)
    } else {
        (glow(d, radius), red)
    }
}

fn glow(d: f32, radius: f32) -> f32 {
    let reach = radius * GLOW;
    if d >= reach {
        return 0.0;
    }
    let t = ((d - radius).max(0.0)) / (reach - radius);
    0.55 * (1.0 - t).powi(2)
}

/// Premultiplied "source over destination".
fn over(dst: u32, a: f32, rgb: [f32; 3]) -> u32 {
    let da = ((dst >> 24) & 0xFF) as f32 / 255.0;
    let dr = ((dst >> 16) & 0xFF) as f32 / 255.0;
    let dg = ((dst >> 8) & 0xFF) as f32 / 255.0;
    let db = (dst & 0xFF) as f32 / 255.0;
    let k = 1.0 - a;
    let oa = a + da * k;
    let or = rgb[0] * a + dr * k;
    let og = rgb[1] * a + dg * k;
    let ob = rgb[2] * a + db * k;
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
    (q(oa) << 24) | (q(or) << 16) | (q(og) << 8) | q(ob)
}

/// The laser on one overlay: head position, trail and what was drawn last frame.
pub struct Laser {
    pub canvas: Canvas,
    radius: f32,
    points: VecDeque<(f32, f32, Instant)>,
    drawn: Option<IRect>,
}

impl Laser {
    pub fn new(width: u32, height: u32) -> Laser {
        Laser {
            canvas: Canvas::new(width, height),
            radius: dot_radius(height),
            points: VecDeque::new(),
            drawn: None,
        }
    }

    pub fn radius(&self) -> f32 {
        self.radius
    }

    /// New head position (normalized). Without `trail` only the head is kept.
    pub fn push(&mut self, x: f32, y: f32, trail: bool, now: Instant) {
        let (lx, ly) = to_local(x, y, self.canvas.width, self.canvas.height);
        if !trail {
            self.points.clear();
        }
        self.points.push_back((lx, ly, now));
        while self.points.len() > MAX_TRAIL {
            self.points.pop_front();
        }
    }

    /// Nothing visible is left to draw.
    pub fn is_blank(&self) -> bool {
        self.points.is_empty() && self.drawn.is_none()
    }

    /// The trail is still fading (keep redrawing).
    pub fn animating(&self) -> bool {
        self.points.len() > 1
    }

    /// Redraw: drop expired trail points (the head always stays), clear what was drawn
    /// last time, draw trail + head. Returns the rectangle that changed.
    pub fn render(&mut self, now: Instant) -> Option<IRect> {
        while self.points.len() > 1
            && self
                .points
                .front()
                .is_some_and(|p| now.saturating_duration_since(p.2) >= TRAIL)
        {
            self.points.pop_front();
        }
        let old = self.drawn.take();
        if let Some(r) = old {
            self.canvas.clear(r);
        }
        let mut new: Option<IRect> = None;
        let n = self.points.len();
        for (i, &(x, y, t)) in self.points.iter().enumerate() {
            let is_head = i + 1 == n;
            let (radius, strength) = if is_head {
                (self.radius, 1.0)
            } else {
                let age = now.saturating_duration_since(t).as_secs_f32() / TRAIL.as_secs_f32();
                let life = (1.0 - age).clamp(0.0, 1.0);
                (self.radius * (0.35 + 0.45 * life), 0.6 * life)
            };
            if strength <= 0.0 {
                continue;
            }
            let r = self.canvas.dot(x, y, radius, strength);
            new = union_opt(new, Some(r));
        }
        self.drawn = new;
        union_opt(old, new)
    }

    /// Remove everything. Returns the rectangle that changed.
    pub fn clear(&mut self) -> Option<IRect> {
        self.points.clear();
        let old = self.drawn.take();
        if let Some(r) = old {
            self.canvas.clear(r);
        }
        old
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alpha(p: u32) -> u32 {
        p >> 24
    }

    #[test]
    fn normalized_coordinates_map_into_the_overlay() {
        assert_eq!(to_local(0.0, 0.0, 1920, 1080), (0.0, 0.0));
        assert_eq!(to_local(0.5, 0.5, 1920, 1080), (960.0, 540.0));
        assert_eq!(to_local(1.0, 1.0, 1920, 1080), (1919.0, 1079.0));
        assert_eq!(to_local(-3.0, 7.0, 1920, 1080), (0.0, 1079.0));
        assert_eq!(to_local(f32::NAN, 0.25, 800, 600), (0.0, 150.0));
        assert_eq!(to_local(0.5, 0.5, 0, 0), (0.0, 0.0));
    }

    #[test]
    fn radius_scales_with_monitor() {
        assert!((dot_radius(1080) - 9.818).abs() < 0.01);
        assert_eq!(dot_radius(300), 6.0);
        assert_eq!(dot_radius(10_000), 22.0);
    }

    #[test]
    fn dot_is_opaque_red_in_the_middle_and_clear_far_away() {
        let mut c = Canvas::new(200, 100);
        let r = c.dot(100.0, 50.0, 10.0, 1.0);
        assert!(r.contains(100, 50) && r.contains(76, 50) && !r.contains(150, 50));
        let centre = c.pixel(100, 50);
        assert_eq!(alpha(centre), 255);
        let rim = c.pixel(108, 50);
        assert_eq!(alpha(rim), 255);
        assert!((rim >> 16) & 0xFF > 200 && (rim >> 8) & 0xFF < 80, "red core {rim:08x}");
        let glow = c.pixel(115, 50);
        assert!(alpha(glow) > 0 && alpha(glow) < 200, "glow {glow:08x}");
        assert_eq!(c.pixel(150, 50), 0);
        assert_eq!(c.pixel(0, 0), 0);
        // Premultiplied: no channel exceeds alpha.
        for p in &c.pixels {
            let a = alpha(*p);
            assert!((p >> 16) & 0xFF <= a && (p >> 8) & 0xFF <= a && p & 0xFF <= a);
        }
    }

    #[test]
    fn dot_at_the_edge_is_clipped() {
        let mut c = Canvas::new(50, 50);
        let r = c.dot(0.0, 49.0, 8.0, 1.0);
        assert_eq!((r.x0, r.y1), (0, 50));
        assert!(alpha(c.pixel(0, 49)) == 255);
    }

    #[test]
    fn moving_clears_the_old_position() {
        let t0 = Instant::now();
        let mut l = Laser::new(400, 200);
        l.push(0.25, 0.5, false, t0);
        let first = l.render(t0).unwrap();
        assert!(first.contains(100, 100));
        assert_eq!(alpha(l.canvas.pixel(100, 100)), 255);
        l.push(0.75, 0.5, false, t0);
        let dirty = l.render(t0).unwrap();
        assert!(dirty.contains(100, 100) && dirty.contains(300, 100));
        assert_eq!(l.canvas.pixel(100, 100), 0);
        assert_eq!(alpha(l.canvas.pixel(300, 100)), 255);
        assert!(!l.animating());
        let cleared = l.clear().unwrap();
        assert!(cleared.contains(300, 100));
        assert!(l.canvas.pixels.iter().all(|p| *p == 0));
        assert!(l.is_blank());
    }

    #[test]
    fn trail_fades_out() {
        let t0 = Instant::now();
        let mut l = Laser::new(400, 200);
        l.push(0.1, 0.5, true, t0);
        l.push(0.5, 0.5, true, t0 + Duration::from_millis(50));
        l.render(t0 + Duration::from_millis(60));
        assert!(l.animating());
        assert!(alpha(l.canvas.pixel(40, 100)) > 0, "trail point drawn");
        l.render(t0 + TRAIL + Duration::from_millis(1));
        assert!(!l.animating());
        assert_eq!(l.canvas.pixel(40, 100), 0, "trail point gone");
        assert_eq!(alpha(l.canvas.pixel(200, 100)), 255, "head stays");
        for i in 0..40 {
            l.push(i as f32 / 40.0, 0.5, true, t0);
        }
        assert_eq!(l.points.len(), MAX_TRAIL);
    }
}
