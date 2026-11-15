//! Drawing of the visualizer styles.

use cursive::Printer;
use cursive::theme::{Color, ColorStyle, ColorType, PaletteColor};

use super::Frame;
use super::colors::Paint;
use crate::settings::{CoverSettings, Visual};

const BLOCKS: [char; 9] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
const HORIZONTAL: [char; 9] = [' ', '▏', '▎', '▍', '▌', '▋', '▊', '▉', '█'];

pub fn draw(printer: &Printer<'_, '_>, frame: &Frame, settings: &CoverSettings, paint: &Paint) {
    match settings.visualizer {
        Visual::Bars => bars(printer, frame, settings, paint),
        Visual::Mirror => mirror(printer, &frame.bands, &frame.bands, settings, paint),
        Visual::Stereo => mirror(printer, &frame.bands, &frame.bands_right, settings, paint),
        Visual::Dots => dots(printer, frame, settings, paint),
        Visual::Line => line(printer, frame, paint),
        Visual::Waterfall => waterfall(printer, frame, paint),
        Visual::Circle => circle(printer, frame, paint),
        Visual::Meter => meter(printer, frame, paint),
        Visual::Wave => wave(printer, frame, paint),
        Visual::Off => {}
    }
}

fn put(printer: &Printer<'_, '_>, x: usize, y: usize, glyph: char, color: Color) {
    let style = ColorStyle::new(ColorType::Color(color), ColorType::InheritParent);
    printer.with_color(style, |p| p.print((x, y), glyph.encode_utf8(&mut [0; 4])));
}

fn put_on(printer: &Printer<'_, '_>, x: usize, y: usize, glyph: char, fg: Color, bg: Color) {
    let style = ColorStyle::new(ColorType::Color(fg), ColorType::Color(bg));
    printer.with_color(style, |p| p.print((x, y), glyph.encode_utf8(&mut [0; 4])));
}

fn fraction(index: usize, count: usize) -> f32 {
    index as f32 / count.max(1) as f32
}

/// Bottom-anchored bars with eighth-block resolution and optional peaks.
fn bars(printer: &Printer<'_, '_>, frame: &Frame, settings: &CoverSettings, paint: &Paint) {
    let (width, height) = (printer.size.x, printer.size.y);
    let bar = settings.bar_width.max(1);
    let stride = bar + settings.bar_gap as usize;
    let count = frame.bands.len();

    for (i, value) in frame.bands.iter().enumerate() {
        let x0 = i * stride;
        let fx = fraction(i, count);
        let eighths = (value * (height * 8) as f32).round() as usize;
        for row in 0..height {
            let fill = eighths.saturating_sub(row * 8).min(8);
            if fill == 0 {
                break;
            }
            let color = paint.at(fx, (row as f32 + 0.5) / height as f32, *value);
            for x in (x0..x0 + bar).filter(|x| *x < width) {
                put(printer, x, height - 1 - row, BLOCKS[fill], color);
            }
        }

        let peak = frame.peaks.get(i).copied().unwrap_or_default();
        if settings.peaks && peak > 0.02 {
            let row = (((1.0 - peak) * height as f32) as usize).min(height - 1);
            for x in (x0..x0 + bar).filter(|x| *x < width) {
                put(printer, x, row, '▔', paint.peak());
            }
        }
    }
}

/// Bars growing up from the middle for `upper` and down for `lower`.
fn mirror(
    printer: &Printer<'_, '_>,
    upper: &[f32],
    lower: &[f32],
    settings: &CoverSettings,
    paint: &Paint,
) {
    let (width, height) = (printer.size.x, printer.size.y);
    let half = height.div_ceil(2);
    let below = height - half;
    let bar = settings.bar_width.max(1);
    let stride = bar + settings.bar_gap as usize;
    let count = upper.len();

    for (i, up) in upper.iter().copied().enumerate() {
        let x0 = i * stride;
        let fx = fraction(i, count);
        let down = lower.get(i).copied().unwrap_or(up);

        let eighths = (up * (half * 8) as f32).round() as usize;
        for row in 0..half {
            let fill = eighths.saturating_sub(row * 8).min(8);
            if fill == 0 {
                break;
            }
            let color = paint.at(fx, (row as f32 + 0.5) / half as f32, up);
            for x in (x0..x0 + bar).filter(|x| *x < width) {
                put(printer, x, half - 1 - row, BLOCKS[fill], color);
            }
        }

        let eighths = (down * (below * 8) as f32).round() as usize;
        for row in 0..below {
            let fill = eighths.saturating_sub(row * 8).min(8);
            let glyph = match fill {
                0 => break,
                8 => '█',
                4..=7 => '▀',
                _ => '▔',
            };
            let color = paint.at(fx, (row as f32 + 0.5) / below as f32, down);
            for x in (x0..x0 + bar).filter(|x| *x < width) {
                put(printer, x, half + row, glyph, color);
            }
        }
    }
}

/// Braille dot canvas with 2x4 dots per cell.
struct Braille {
    width: usize,
    height: usize,
    cells: Vec<u8>,
}

impl Braille {
    const BITS: [[u8; 4]; 2] = [[0x01, 0x02, 0x04, 0x40], [0x08, 0x10, 0x20, 0x80]];

    fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            cells: vec![0; width * height],
        }
    }

    fn dots(&self) -> (usize, usize) {
        (self.width * 2, self.height * 4)
    }

    fn set(&mut self, x: usize, y: usize) {
        if x < self.width * 2 && y < self.height * 4 {
            self.cells[(y / 4) * self.width + x / 2] |= Self::BITS[x % 2][y % 4];
        }
    }

    fn set_f(&mut self, x: f32, y: f32) {
        if x >= 0.0 && y >= 0.0 {
            self.set(x.round() as usize, y.round() as usize);
        }
    }

    /// Set every dot between rows `a` and `b` in column `x`.
    fn column(&mut self, x: usize, a: usize, b: usize) {
        for y in a.min(b)..=a.max(b) {
            self.set(x, y);
        }
    }

    fn print(&self, printer: &Printer<'_, '_>, color: impl Fn(usize, usize) -> Color) {
        for (i, bits) in self.cells.iter().enumerate() {
            if *bits != 0 {
                let (x, y) = (i % self.width, i / self.width);
                let glyph = char::from_u32(0x2800 + *bits as u32).unwrap_or(' ');
                put(printer, x, y, glyph, color(x, y));
            }
        }
    }
}

/// Spectrum bars made of braille dots, four times finer than blocks.
fn dots(printer: &Printer<'_, '_>, frame: &Frame, settings: &CoverSettings, paint: &Paint) {
    let (width, height) = (printer.size.x, printer.size.y);
    let mut canvas = Braille::new(width, height);
    let (_, dots_y) = canvas.dots();
    let bar = settings.bar_width.max(1);
    let stride = bar + settings.bar_gap as usize;

    for (i, value) in frame.bands.iter().enumerate() {
        let top = dots_y - (value * dots_y as f32).round() as usize;
        for x in i * stride..i * stride + bar {
            if top < dots_y {
                canvas.column(x, top, dots_y - 1);
            }
            let peak = frame.peaks.get(i).copied().unwrap_or_default();
            if settings.peaks && peak > 0.02 {
                canvas.set(x, dots_y - 1 - (peak * (dots_y - 1) as f32) as usize);
            }
        }
    }

    let count = frame.bands.len();
    canvas.print(printer, |x, y| {
        let band = ((x * 2) / stride).min(count.saturating_sub(1));
        let value = frame.bands.get(band).copied().unwrap_or_default();
        paint.at(fraction(x, width), 1.0 - fraction(y, height), value)
    });
}

/// One continuous spectrum curve.
fn line(printer: &Printer<'_, '_>, frame: &Frame, paint: &Paint) {
    let (width, height) = (printer.size.x, printer.size.y);
    let mut canvas = Braille::new(width, height);
    let (dots_x, dots_y) = canvas.dots();
    let count = frame.bands.len();
    if count == 0 {
        return;
    }

    let mut previous = None;
    for x in 0..dots_x {
        let position = x as f32 * (count - 1) as f32 / (dots_x - 1).max(1) as f32;
        let (i, t) = (position as usize, position.fract());
        let next = frame.bands.get(i + 1).copied().unwrap_or(frame.bands[i]);
        let value = frame.bands[i] * (1.0 - t) + next * t;
        let y = dots_y - 1 - (value * (dots_y - 1) as f32).round() as usize;
        canvas.column(x, previous.unwrap_or(y), y);
        previous = Some(y);
    }

    canvas.print(printer, |x, y| {
        let level = 1.0 - fraction(y, height);
        paint.at(fraction(x, width), level, level)
    });
}

/// Spectrogram scrolling upwards, intensity as color.
fn waterfall(printer: &Printer<'_, '_>, frame: &Frame, paint: &Paint) {
    let (width, height) = (printer.size.x, printer.size.y);
    let background = printer.theme.palette[PaletteColor::Background];
    let rows = frame.history.len();
    let value = |row: Option<usize>, x: usize| {
        row.and_then(|r| frame.history.get(r))
            .and_then(|bands| bands.get(x))
            .copied()
            .unwrap_or_default()
    };
    let color = |level: f32, x: usize| {
        if level < 0.03 {
            background
        } else {
            paint.heat(fraction(x, width), level)
        }
    };

    for y in 0..height {
        // Each cell shows two history rows, the newest at the bottom.
        let newer = (rows + 2 * y + 1).checked_sub(2 * height);
        let older = (rows + 2 * y).checked_sub(2 * height);
        for x in 0..width {
            let (top, bottom) = (value(older, x), value(newer, x));
            if top < 0.03 && bottom < 0.03 {
                continue;
            }
            put_on(printer, x, y, '▀', color(top, x), color(bottom, x));
        }
    }
}

/// Spectrum drawn as rays around a ring.
fn circle(printer: &Printer<'_, '_>, frame: &Frame, paint: &Paint) {
    let (width, height) = (printer.size.x, printer.size.y);
    let mut canvas = Braille::new(width, height);
    let (dots_x, dots_y) = canvas.dots();
    let (cx, cy) = (dots_x as f32 / 2.0, dots_y as f32 / 2.0);
    let outer = cx.min(cy) - 1.0;
    let inner = outer * 0.4;
    let count = frame.bands.len();
    if count == 0 || outer < 2.0 {
        return;
    }

    const RAYS: usize = 96;
    for k in 0..RAYS {
        let angle = k as f32 / RAYS as f32 * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
        let (sin, cos) = angle.sin_cos();
        // Mirror the spectrum so both halves of the ring match.
        let half = RAYS / 2;
        let t = if k <= half { k } else { RAYS - k } as f32 / half as f32;
        let value = frame.bands[((t * (count - 1) as f32) as usize).min(count - 1)];
        let end = inner + value * (outer - inner);
        let mut radius = inner;
        while radius <= end {
            canvas.set_f(cx + radius * cos, cy + radius * sin);
            radius += 0.5;
        }
    }

    canvas.print(printer, |x, y| {
        let dx = (x * 2 + 1) as f32 - cx;
        let dy = (y * 4 + 2) as f32 - cy;
        let distance = ((dx * dx + dy * dy).sqrt() / outer).clamp(0.0, 1.0);
        paint.at(fraction(x, width), distance, distance)
    });
}

/// Two horizontal level meters for the left and right channel.
fn meter(printer: &Printer<'_, '_>, frame: &Frame, paint: &Paint) {
    let (width, height) = (printer.size.x, printer.size.y);
    if width < 8 {
        return;
    }
    let length = width - 2;
    let first = height.saturating_sub(3) / 2;
    let rows = [first, (first + 2).min(height - 1)];
    let label = ColorStyle::secondary();

    for (channel, row) in rows.iter().enumerate() {
        printer.with_color(label, |p| p.print((0, *row), ["L", "R"][channel]));
        let eighths = (frame.levels[channel] * (length * 8) as f32).round() as usize;
        for x in 0..length {
            let fill = eighths.saturating_sub(x * 8).min(8);
            if fill > 0 {
                let position = fraction(x, length);
                put(
                    printer,
                    x + 2,
                    *row,
                    HORIZONTAL[fill],
                    paint.at(position, position, position),
                );
            }
        }
        let peak = frame.level_peaks[channel];
        if peak > 0.02 {
            let x = ((peak * length as f32) as usize).min(length - 1);
            put(printer, x + 2, *row, '▕', paint.peak());
        }
    }

    if height >= 6 && width >= 30 {
        let scale_row = rows[1] + 2;
        printer.with_color(label, |p| {
            for db in [-36, -24, -12, 0] {
                let position = (db as f32 + 48.0) / 48.0;
                let text = format!("{db}");
                let x = (2 + (position * length as f32) as usize).saturating_sub(text.len());
                p.print((x, scale_row), &text);
            }
        });
    }
}

/// Oscilloscope of the waveform.
fn wave(printer: &Printer<'_, '_>, frame: &Frame, paint: &Paint) {
    let (width, height) = (printer.size.x, printer.size.y);
    if frame.wave.is_empty() {
        return;
    }
    let mut canvas = Braille::new(width, height);
    let (dots_x, dots_y) = canvas.dots();
    let to_y = |sample: f32| {
        let y = (0.5 - sample.clamp(-1.0, 1.0) * 0.45) * (dots_y - 1) as f32;
        y.round() as usize
    };

    let mut previous = None;
    for x in 0..dots_x {
        let y = to_y(frame.wave[x * frame.wave.len() / dots_x]);
        canvas.column(x, previous.unwrap_or(y), y);
        previous = Some(y);
    }

    canvas.print(printer, |x, y| {
        let distance = ((fraction(y, height) - 0.5).abs() * 2.0).min(1.0);
        paint.at(fraction(x, width), distance, distance)
    });
}
