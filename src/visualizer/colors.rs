//! Color palettes and modes for the visualizer.

use std::path::Path;

use cursive::theme::{BaseColor, Color};

use crate::settings::{ColorMode, CoverSettings, Palette};

pub type Rgb = (u8, u8, u8);

const FALLBACK: Rgb = (46, 158, 244);

pub struct Paint {
    pub low: Rgb,
    pub high: Rgb,
    pub peak: Rgb,
    mode: ColorMode,
}

impl Paint {
    /// Colors for `settings`; `theme` and `theme_peak` come from the ncspot
    /// theme, `cover` is the dominant color of the current album cover.
    pub fn new(
        settings: &CoverSettings,
        theme: Color,
        theme_peak: Color,
        cover: Option<Rgb>,
    ) -> Self {
        let (low, high, peak) = match settings.palette {
            Palette::Theme => {
                let high = to_rgb(theme);
                (scale(high, 0.35), high, to_rgb(theme_peak))
            }
            Palette::Cover => {
                let high = cover.map(vivid).unwrap_or(FALLBACK);
                (scale(high, 0.35), high, mix(high, (255, 255, 255), 0.6))
            }
            Palette::Spotify => ((8, 64, 32), (30, 215, 96), (255, 255, 255)),
            Palette::Sunset => ((90, 20, 90), (255, 94, 58), (255, 214, 102)),
            Palette::Ocean => ((0, 40, 80), (0, 180, 216), (202, 240, 248)),
            Palette::Mono => ((60, 60, 60), (220, 220, 220), (255, 255, 255)),
        };
        Self {
            low,
            high,
            peak,
            mode: settings.color_mode,
        }
    }

    /// Color at horizontal position `x` and height `y` (both 0..1, y = 1 at
    /// the top) for a band with intensity `level`.
    pub fn at(&self, x: f32, y: f32, level: f32) -> Color {
        let rgb = match self.mode {
            ColorMode::Solid => self.high,
            ColorMode::Gradient => mix(self.low, self.high, y),
            ColorMode::Rainbow => hsv(x * 300.0, 0.75, 0.55 + 0.45 * y),
            ColorMode::Heat => self.heat_rgb(level),
        };
        rgb_color(rgb)
    }

    /// Color for an intensity alone, used where position carries no meaning.
    pub fn heat(&self, x: f32, level: f32) -> Color {
        match self.mode {
            ColorMode::Rainbow => rgb_color(hsv(x * 300.0, 0.8, level.clamp(0.0, 1.0))),
            _ => rgb_color(self.heat_rgb(level)),
        }
    }

    fn heat_rgb(&self, level: f32) -> Rgb {
        let level = level.clamp(0.0, 1.0);
        if level < 0.5 {
            mix((0, 0, 0), self.low, level * 2.0)
        } else if level < 0.85 {
            mix(self.low, self.high, (level - 0.5) / 0.35)
        } else {
            mix(self.high, self.peak, (level - 0.85) / 0.15)
        }
    }

    pub fn peak(&self) -> Color {
        rgb_color(self.peak)
    }

    pub fn accent(&self) -> Color {
        rgb_color(self.high)
    }
}

pub fn rgb_color((r, g, b): Rgb) -> Color {
    Color::Rgb(r, g, b)
}

fn to_rgb(color: Color) -> Rgb {
    match color {
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Dark(base) => base_rgb(base, 0.7),
        Color::Light(base) => base_rgb(base, 1.0),
        _ => FALLBACK,
    }
}

fn base_rgb(base: BaseColor, intensity: f32) -> Rgb {
    let full = match base {
        BaseColor::Black => (40, 40, 40),
        BaseColor::Red => (240, 70, 70),
        BaseColor::Green => (70, 210, 120),
        BaseColor::Yellow => (240, 220, 80),
        BaseColor::Blue => FALLBACK,
        BaseColor::Magenta => (210, 100, 210),
        BaseColor::Cyan => (60, 200, 220),
        BaseColor::White => (235, 235, 235),
    };
    scale(full, intensity)
}

pub fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    let lerp = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    (lerp(a.0, b.0), lerp(a.1, b.1), lerp(a.2, b.2))
}

fn scale(c: Rgb, factor: f32) -> Rgb {
    let s = |x: u8| (x as f32 * factor).clamp(0.0, 255.0) as u8;
    (s(c.0), s(c.1), s(c.2))
}

fn hsv(hue: f32, saturation: f32, value: f32) -> Rgb {
    let c = value * saturation;
    let h = (hue.rem_euclid(360.0)) / 60.0;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = value - c;
    let to = |v: f32| ((v + m) * 255.0).round() as u8;
    (to(r), to(g), to(b))
}

fn to_hsv((r, g, b): Rgb) -> (f32, f32, f32) {
    let (r, g, b) = (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
    let max = r.max(g).max(b);
    let delta = max - r.min(g).min(b);
    let hue = if delta == 0.0 {
        0.0
    } else if max == r {
        60.0 * ((g - b) / delta).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / delta + 2.0)
    } else {
        60.0 * ((r - g) / delta + 4.0)
    };
    let saturation = if max == 0.0 { 0.0 } else { delta / max };
    (hue, saturation, max)
}

/// Brighten and saturate a color so it stands out on a black background.
fn vivid(color: Rgb) -> Rgb {
    let (hue, saturation, _) = to_hsv(color);
    if saturation < 0.15 {
        return (220, 220, 220);
    }
    hsv(hue, saturation.max(0.55), 0.95)
}

/// The most prominent saturated color of an image.
pub fn dominant(path: &Path) -> Option<Rgb> {
    let image = image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()?
        .thumbnail(48, 48)
        .to_rgb8();

    const BUCKETS: usize = 24;
    let mut weights = [0.0f32; BUCKETS];
    let mut sums = [[0.0f32; 3]; BUCKETS];
    for pixel in image.pixels() {
        let rgb = (pixel[0], pixel[1], pixel[2]);
        let (hue, saturation, value) = to_hsv(rgb);
        if saturation < 0.25 || value < 0.2 {
            continue;
        }
        let bucket = (hue / 360.0 * BUCKETS as f32) as usize % BUCKETS;
        let weight = saturation * value;
        weights[bucket] += weight;
        for (sum, channel) in sums[bucket].iter_mut().zip([rgb.0, rgb.1, rgb.2]) {
            *sum += channel as f32 * weight;
        }
    }

    let (bucket, weight) = weights
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))?;
    if *weight <= 0.0 {
        return Some((200, 200, 200));
    }
    let [r, g, b] = sums[bucket].map(|sum| (sum / weight) as u8);
    Some((r, g, b))
}
