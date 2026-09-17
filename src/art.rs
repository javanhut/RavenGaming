//! The painted parts of the window: the backdrop, the hero scene, and the
//! ring gauges.
//!
//! # Why these are drawn and not shipped as images
//!
//! A photograph would have to come from somewhere, be licensed, and be
//! carried in the repository at a megabyte a time — and it would be one
//! fixed picture under every accent colour the desktop offers. Everything
//! here is a few hundred lines of Cairo instead: it weighs nothing, it
//! scales to any window without a second asset, and it takes the person's
//! accent as a parameter, so the scene behind "Ready to play" belongs to
//! their desktop rather than to a stock library.
//!
//! # Why it never moves
//!
//! The scene is generated from a fixed seed, so the same window always
//! draws the same mountains. A landscape that reshuffled itself on every
//! resize would be the most distracting thing in a room full of live
//! telemetry.

use std::f64::consts::{PI, TAU};

use gtk::cairo;

/// A colour, 0..1 per channel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgb {
    pub r: f64,
    pub g: f64,
    pub b: f64,
}

impl Rgb {
    pub const fn new(r: f64, g: f64, b: f64) -> Rgb {
        Rgb { r, g, b }
    }

    /// The colour used when an accent cannot be read.
    ///
    /// Exactly the default Raven Settings writes, byte for byte, so a
    /// window with no accent configured paints the same as one that has
    /// the default set explicitly. An approximation here would make the
    /// two differ by a shade for no reason anyone could find.
    pub fn fallback() -> Rgb {
        Rgb {
            r: 0x7A as f64 / 255.0,
            g: 0xA2 as f64 / 255.0,
            b: 0xF7 as f64 / 255.0,
        }
    }

    /// `#RRGGBB`, falling back to the Raven blue for anything else, so a
    /// malformed accent in `desktop.toml` can never paint a black window.
    pub fn from_hex(hex: &str) -> Rgb {
        let parse = |range: std::ops::Range<usize>| -> Option<f64> {
            u8::from_str_radix(hex.get(range)?, 16)
                .ok()
                .map(|v| v as f64 / 255.0)
        };
        match (parse(1..3), parse(3..5), parse(5..7)) {
            (Some(r), Some(g), Some(b)) if hex.starts_with('#') && hex.len() == 7 => {
                Rgb { r, g, b }
            }
            _ => Rgb::fallback(),
        }
    }

    /// Toward `other` by `amount`, 0..1.
    pub fn mix(self, other: Rgb, amount: f64) -> Rgb {
        let t = amount.clamp(0.0, 1.0);
        Rgb {
            r: self.r + (other.r - self.r) * t,
            g: self.g + (other.g - self.g) * t,
            b: self.b + (other.b - self.b) * t,
        }
    }

    /// Scaled toward black (`factor` below 1) or white (above 1).
    pub fn shade(self, factor: f64) -> Rgb {
        if factor <= 1.0 {
            Rgb {
                r: self.r * factor,
                g: self.g * factor,
                b: self.b * factor,
            }
        } else {
            self.mix(Rgb::new(1.0, 1.0, 1.0), factor - 1.0)
        }
    }
}

fn set(cr: &cairo::Context, colour: Rgb, alpha: f64) {
    cr.set_source_rgba(colour.r, colour.g, colour.b, alpha);
}

// ---- deterministic noise -------------------------------------------------

/// A small integer hash, used as the only source of randomness here.
///
/// Not a general-purpose hash and not trying to be: it has to produce the
/// same ridge line on every machine and every redraw, which rules out
/// anything seeded from the clock or from a hash map's random state.
fn hash(seed: u64, index: u64) -> f64 {
    let mut x = seed
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(index.wrapping_mul(0xBF58_476D_1CE4_E5B9));
    x ^= x >> 30;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^= x >> 31;
    (x >> 11) as f64 / (1u64 << 53) as f64
}

/// Smoothstep, for interpolating between control points without the
/// corners a straight line would put on every peak.
fn smooth(t: f64) -> f64 {
    t * t * (3.0 - 2.0 * t)
}

/// A mountain outline: `samples` heights in 0..1, from `control` random
/// control points smoothed together, plus a finer second octave so the
/// ridge has detail as well as shape.
pub fn ridge(seed: u64, control: usize, samples: usize) -> Vec<f64> {
    if samples == 0 {
        return Vec::new();
    }
    let control = control.max(2);
    let octave = |seed: u64, control: usize, position: f64| -> f64 {
        let scaled = position * (control - 1) as f64;
        let index = scaled.floor() as u64;
        let t = smooth(scaled - scaled.floor());
        let a = hash(seed, index);
        let b = hash(seed, index + 1);
        a + (b - a) * t
    };
    (0..samples)
        .map(|i| {
            let position = i as f64 / (samples - 1).max(1) as f64;
            let coarse = octave(seed, control, position);
            let fine = octave(seed ^ 0xA5A5, control * 3, position);
            (coarse * 0.75 + fine * 0.25).clamp(0.0, 1.0)
        })
        .collect()
}

/// One mountain layer: where its feet sit, how tall its peaks are, and
/// what colour it is.
struct Layer {
    seed: u64,
    /// The foot of the ridge, as a multiple of the area's height. Over 1.0
    /// puts it below the bottom edge, which is how a near ridge is drawn.
    base: f64,
    /// Peak height, as a multiple of the area's height.
    amplitude: f64,
    colour: Rgb,
    alpha: f64,
}

/// Fills a mountain layer: the ridge line across the top, down to the
/// bottom of the area.
fn mountains(cr: &cairo::Context, width: f64, height: f64, layer: Layer) {
    let Layer {
        seed,
        base,
        amplitude,
        colour,
        alpha,
    } = layer;
    let samples = (width / 6.0).clamp(24.0, 260.0) as usize;
    let heights = ridge(seed, 7, samples);
    cr.move_to(0.0, height);
    for (i, h) in heights.iter().enumerate() {
        let x = i as f64 / (samples - 1).max(1) as f64 * width;
        cr.line_to(x, height * base - h * height * amplitude);
    }
    cr.line_to(width, height);
    cr.close_path();
    set(cr, colour, alpha);
    let _ = cr.fill();
}

// ---- the hero scene ------------------------------------------------------

/// Clips everything after it to a rounded rectangle of the whole area.
///
/// The hero rounds its own corners here rather than letting the widget
/// clip them. A widget-level clip would round the painting and cut the
/// text over it in the same stroke, so a banner squeezed by a short window
/// would silently lose its button — which is exactly what it did before
/// this existed.
fn round_clip(cr: &cairo::Context, width: f64, height: f64, radius: f64) {
    let r = radius.min(width / 2.0).min(height / 2.0).max(0.0);
    cr.new_path();
    cr.arc(width - r, r, r, -PI / 2.0, 0.0);
    cr.arc(width - r, height - r, r, 0.0, PI / 2.0);
    cr.arc(r, height - r, r, PI / 2.0, PI);
    cr.arc(r, r, r, PI, 1.5 * PI);
    cr.close_path();
    cr.clip();
}

/// The picture behind the hero banner: a night sky over four ridges, with
/// a moon and a scrim so the text over it stays readable.
pub fn hero(cr: &cairo::Context, width: f64, height: f64, accent: Rgb) {
    if width <= 0.0 || height <= 0.0 {
        return;
    }
    let _ = cr.save();
    round_clip(cr, width, height, 20.0);
    // Sky. The violets are fixed — a night sky is a night sky — but the
    // upper band is pulled a little toward the accent so the banner
    // belongs to whatever colour the desktop is set to.
    let high = Rgb::new(0.055, 0.043, 0.129).mix(accent, 0.20);
    let sky = cairo::LinearGradient::new(0.0, 0.0, 0.0, height);
    sky.add_color_stop_rgba(0.0, high.r, high.g, high.b, 1.0);
    sky.add_color_stop_rgba(0.42, 0.184, 0.110, 0.341, 1.0);
    sky.add_color_stop_rgba(0.66, 0.404, 0.231, 0.541, 1.0);
    sky.add_color_stop_rgba(0.84, 0.478, 0.298, 0.573, 1.0);
    sky.add_color_stop_rgba(1.0, 0.180, 0.118, 0.298, 1.0);
    let _ = cr.set_source(&sky);
    let _ = cr.paint();

    // Stars, thinning out toward the horizon and stopping before the
    // ridges — a star drawn over a mountain reads as a dead pixel.
    for i in 0..90u64 {
        let x = hash(0x57A45, i) * width;
        let y = hash(0x57A45, i + 1000).powf(1.7) * height * 0.60;
        let size = 0.4 + hash(0x57A45, i + 2000) * 1.0;
        let brightness = (0.25 + hash(0x57A45, i + 3000) * 0.65) * (1.0 - y / (height * 0.75));
        cr.arc(x, y, size, 0.0, TAU);
        cr.set_source_rgba(1.0, 1.0, 1.0, brightness.clamp(0.0, 1.0));
        let _ = cr.fill();
    }

    // The moon: a glow, then the disc. Placed right of centre, clear of
    // the text on the left and of the feature list further right.
    // Tucked into the top-right corner and half cropped by it. The words
    // hold the left of the banner and the three feature lines hold the
    // right from a third of the way down; a moon anywhere else in a strip
    // this shallow ends up behind one or the other of them.
    let (moon_x, moon_y, moon_r) = (width * 0.955, height * -0.02, height * 0.30);
    let glow = cairo::RadialGradient::new(moon_x, moon_y, 0.0, moon_x, moon_y, moon_r * 3.4);
    glow.add_color_stop_rgba(0.0, 0.78, 0.74, 0.95, 0.42);
    glow.add_color_stop_rgba(0.45, 0.55, 0.45, 0.80, 0.14);
    glow.add_color_stop_rgba(1.0, 0.0, 0.0, 0.0, 0.0);
    let _ = cr.set_source(&glow);
    let _ = cr.paint();
    let disc = cairo::RadialGradient::new(
        moon_x - moon_r * 0.3,
        moon_y - moon_r * 0.3,
        0.0,
        moon_x,
        moon_y,
        moon_r,
    );
    disc.add_color_stop_rgba(0.0, 0.96, 0.94, 1.0, 0.95);
    disc.add_color_stop_rgba(1.0, 0.70, 0.66, 0.88, 0.80);
    cr.arc(moon_x, moon_y, moon_r, 0.0, TAU);
    let _ = cr.set_source(&disc);
    let _ = cr.fill();

    // Four ridges, each nearer and darker than the last.
    let far = Rgb::new(0.243, 0.157, 0.353);
    for layer in [
        Layer {
            seed: 0x1111,
            base: 0.86,
            amplitude: 0.30,
            colour: far,
            alpha: 0.85,
        },
        Layer {
            seed: 0x2222,
            base: 0.94,
            amplitude: 0.26,
            colour: far.shade(0.62),
            alpha: 0.92,
        },
        Layer {
            seed: 0x3333,
            base: 1.02,
            amplitude: 0.24,
            colour: far.shade(0.34),
            alpha: 0.96,
        },
        Layer {
            seed: 0x4444,
            base: 1.12,
            amplitude: 0.22,
            colour: Rgb::new(0.031, 0.024, 0.063),
            alpha: 1.0,
        },
    ] {
        mountains(cr, width, height, layer);
    }

    // The scrim. Text sits on the left third, and unscrimmed white on a
    // lilac sky is exactly the kind of thing that looks fine in a mockup
    // and is unreadable on a real screen.
    let scrim = cairo::LinearGradient::new(0.0, 0.0, width, 0.0);
    scrim.add_color_stop_rgba(0.0, 0.016, 0.020, 0.047, 0.88);
    scrim.add_color_stop_rgba(0.42, 0.016, 0.020, 0.047, 0.50);
    // The right third carries the three feature lines, so it keeps enough
    // scrim for white text on it. A picture nobody can read words over is
    // not decoration, it is damage.
    scrim.add_color_stop_rgba(1.0, 0.016, 0.020, 0.047, 0.44);
    let _ = cr.set_source(&scrim);
    let _ = cr.paint();
    let _ = cr.restore();
}

// ---- the window backdrop -------------------------------------------------

/// The wash behind the whole window: a dark base, two soft clouds of
/// colour, and a low ridge along the bottom. Everything else in the window
/// is translucent glass sitting on this, which is where the depth comes
/// from.
pub fn backdrop(cr: &cairo::Context, width: f64, height: f64, accent: Rgb) {
    if width <= 0.0 || height <= 0.0 {
        return;
    }
    let base = cairo::LinearGradient::new(0.0, 0.0, width * 0.35, height);
    base.add_color_stop_rgba(0.0, 0.035, 0.043, 0.078, 1.0);
    base.add_color_stop_rgba(0.55, 0.043, 0.047, 0.086, 1.0);
    base.add_color_stop_rgba(1.0, 0.055, 0.043, 0.098, 1.0);
    let _ = cr.set_source(&base);
    let _ = cr.paint();

    let cloud = |cx: f64, cy: f64, radius: f64, colour: Rgb, alpha: f64| {
        let gradient = cairo::RadialGradient::new(cx, cy, 0.0, cx, cy, radius);
        gradient.add_color_stop_rgba(0.0, colour.r, colour.g, colour.b, alpha);
        gradient.add_color_stop_rgba(1.0, colour.r, colour.g, colour.b, 0.0);
        let _ = cr.set_source(&gradient);
        let _ = cr.paint();
    };
    cloud(
        width * 0.12,
        height * 1.02,
        height * 0.85,
        accent.mix(Rgb::new(0.55, 0.25, 0.85), 0.55),
        0.22,
    );
    cloud(width * 0.92, height * -0.06, height * 0.70, accent, 0.14);

    // A ridge along the foot of the window, mostly hidden behind the
    // cards — it is there to give the bottom of the sidebar somewhere to
    // fade into.
    mountains(
        cr,
        width,
        height,
        Layer {
            seed: 0x7777,
            base: 1.26,
            amplitude: 0.20,
            colour: Rgb::new(0.043, 0.035, 0.086),
            alpha: 0.9,
        },
    );
}

/// The smaller version at the foot of the sidebar.
pub fn sidebar_footer(cr: &cairo::Context, width: f64, height: f64, accent: Rgb) {
    if width <= 0.0 || height <= 0.0 {
        return;
    }
    let _ = cr.save();
    let haze = cairo::LinearGradient::new(0.0, 0.0, 0.0, height);
    haze.add_color_stop_rgba(0.0, accent.r, accent.g, accent.b, 0.0);
    haze.add_color_stop_rgba(1.0, accent.r, accent.g, accent.b, 0.16);
    let _ = cr.set_source(&haze);
    let _ = cr.paint();
    for layer in [
        Layer {
            seed: 0x5151,
            base: 1.05,
            amplitude: 0.55,
            colour: Rgb::new(0.35, 0.24, 0.52),
            alpha: 0.35,
        },
        Layer {
            seed: 0x6262,
            base: 1.18,
            amplitude: 0.50,
            colour: Rgb::new(0.13, 0.10, 0.24),
            alpha: 0.75,
        },
    ] {
        mountains(cr, width, height, layer);
    }
}

// ---- gauges --------------------------------------------------------------

/// A ring gauge: a faint full circle, and an arc over it for the value,
/// starting at twelve o'clock and going clockwise.
pub fn ring(cr: &cairo::Context, size: f64, fraction: f64, colour: Rgb) {
    let stroke = (size * 0.10).clamp(3.0, 6.0);
    let radius = (size - stroke) / 2.0;
    let (cx, cy) = (size / 2.0, size / 2.0);
    if radius <= 0.0 {
        return;
    }
    cr.set_line_width(stroke);
    cr.set_line_cap(cairo::LineCap::Round);

    cr.arc(cx, cy, radius, 0.0, TAU);
    cr.set_source_rgba(1.0, 1.0, 1.0, 0.10);
    let _ = cr.stroke();

    let fraction = fraction.clamp(0.0, 1.0);
    if fraction <= 0.0 {
        return;
    }
    let start = -PI / 2.0;
    cr.arc(cx, cy, radius, start, start + TAU * fraction);
    set(cr, colour, 0.95);
    let _ = cr.stroke();
}

/// The colour a load reads at: the accent while there is headroom, amber
/// as it runs out, red when there is none. A gauge that is the same colour
/// at 10% and at 99% is a decoration, not a reading.
pub fn load_colour(fraction: f64, accent: Rgb) -> Rgb {
    match fraction {
        f if f >= 0.90 => Rgb::new(1.0, 0.271, 0.227),
        f if f >= 0.75 => Rgb::new(1.0, 0.624, 0.039),
        _ => accent,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hex_accent_becomes_a_colour() {
        let white = Rgb::from_hex("#FFFFFF");
        assert!((white.r - 1.0).abs() < 1e-9 && (white.b - 1.0).abs() < 1e-9);
        let raven = Rgb::from_hex("#7AA2F7");
        assert!((raven.r - 0.478).abs() < 0.01);
        assert!((raven.g - 0.635).abs() < 0.01);
        assert!((raven.b - 0.969).abs() < 0.01);
    }

    #[test]
    fn a_bad_accent_never_paints_a_black_window() {
        for bad in ["", "red", "#FFF", "#GGGGGG", "7AA2F7", "#7AA2F7FF"] {
            assert_eq!(Rgb::from_hex(bad), Rgb::fallback(), "accepted {bad:?}");
        }
    }

    #[test]
    fn the_fallback_is_the_desktops_own_default() {
        // Not an approximation of it: parsing the constant Raven Settings
        // writes must give exactly the colour used when nothing is set.
        assert_eq!(
            Rgb::from_hex(crate::desktop::DEFAULT_ACCENT),
            Rgb::fallback()
        );
    }

    #[test]
    fn mixing_and_shading_stay_in_range() {
        let a = Rgb::new(0.0, 0.0, 0.0);
        let b = Rgb::new(1.0, 1.0, 1.0);
        assert_eq!(a.mix(b, 0.0), a);
        assert_eq!(a.mix(b, 1.0), b);
        assert_eq!(a.mix(b, 2.0), b, "amount is clamped");
        let half = a.mix(b, 0.5);
        assert!((half.r - 0.5).abs() < 1e-9);
        assert!(b.shade(0.5).r < b.r);
        assert!(a.shade(1.5).r > a.r);
    }

    #[test]
    fn the_same_seed_always_draws_the_same_mountains() {
        let first = ridge(0x1111, 7, 64);
        let second = ridge(0x1111, 7, 64);
        assert_eq!(first, second);
        assert_ne!(first, ridge(0x2222, 7, 64));
    }

    #[test]
    fn a_ridge_stays_inside_the_area_it_is_drawn_in() {
        for seed in [0u64, 1, 0x1111, 0x7777, u64::MAX] {
            for height in ridge(seed, 7, 256) {
                assert!(
                    (0.0..=1.0).contains(&height),
                    "seed {seed} produced {height}"
                );
            }
        }
    }

    #[test]
    fn a_ridge_has_shape_rather_than_being_flat() {
        let heights = ridge(0x3333, 7, 128);
        let low = heights.iter().cloned().fold(f64::MAX, f64::min);
        let high = heights.iter().cloned().fold(f64::MIN, f64::max);
        assert!(high - low > 0.25, "range was only {}", high - low);
    }

    #[test]
    fn degenerate_sizes_are_asked_for_and_must_not_panic() {
        assert!(ridge(1, 7, 0).is_empty());
        assert_eq!(ridge(1, 7, 1).len(), 1);
        // A control count below two would divide by zero.
        assert_eq!(ridge(1, 0, 4).len(), 4);
    }

    #[test]
    fn a_rounded_clip_copes_with_an_area_smaller_than_its_radius() {
        // A hero squeezed to twenty pixels must not ask Cairo for a
        // negative radius; it just comes out less rounded.
        let surface = cairo::ImageSurface::create(cairo::Format::ARgb32, 20, 8).unwrap();
        let cr = cairo::Context::new(&surface).unwrap();
        round_clip(&cr, 20.0, 8.0, 20.0);
        let extents = cr.clip_extents().unwrap();
        assert!(extents.2 - extents.0 > 0.0 && extents.3 - extents.1 > 0.0);
    }

    #[test]
    fn every_painting_copes_with_a_zero_sized_area() {
        let surface = cairo::ImageSurface::create(cairo::Format::ARgb32, 1, 1).unwrap();
        let cr = cairo::Context::new(&surface).unwrap();
        let accent = Rgb::fallback();
        hero(&cr, 0.0, 0.0, accent);
        backdrop(&cr, 0.0, 0.0, accent);
        sidebar_footer(&cr, -5.0, 10.0, accent);
        ring(&cr, 0.0, 0.5, accent);
    }

    #[test]
    fn a_painting_leaves_the_context_as_it_found_it() {
        // Each one clips or transforms; a leaked clip would silently crop
        // whatever is drawn next in the same snapshot.
        let surface = cairo::ImageSurface::create(cairo::Format::ARgb32, 64, 48).unwrap();
        let cr = cairo::Context::new(&surface).unwrap();
        let before = cr.clip_extents().unwrap();
        hero(&cr, 64.0, 48.0, Rgb::fallback());
        sidebar_footer(&cr, 64.0, 48.0, Rgb::fallback());
        let after = cr.clip_extents().unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn a_gauge_warns_before_it_runs_out() {
        let accent = Rgb::new(0.0, 0.0, 1.0);
        assert_eq!(load_colour(0.10, accent), accent);
        assert_eq!(load_colour(0.74, accent), accent);
        assert_ne!(load_colour(0.80, accent), accent);
        assert_ne!(load_colour(0.95, accent), load_colour(0.80, accent));
    }
}
