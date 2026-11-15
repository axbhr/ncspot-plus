//! Settings dialog of the cover view, opened with `,`.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use cursive::direction::Direction;
use cursive::event::{Event, EventResult, Key};
use cursive::theme::ColorStyle;
use cursive::view::CannotFocus;
use cursive::views::Panel;
use cursive::{Cursive, Printer, Vec2, View};

use crate::settings::{self, CoverSettings};

const WIDTH: usize = 44;
/// Rows from this index on belong to the layout section.
const LAYOUT_SECTION: usize = 10;

static OPEN: AtomicBool = AtomicBool::new(false);
static AREA: Mutex<Option<(Vec2, Vec2)>> = Mutex::new(None);

/// True while the dialog is shown.
pub fn is_open() -> bool {
    OPEN.load(Ordering::Relaxed)
}

/// Screen area of the open dialog including its border.
pub fn area() -> Option<(Vec2, Vec2)> {
    if is_open() {
        *AREA.lock().unwrap()
    } else {
        None
    }
}

pub fn open(s: &mut Cursive) {
    OPEN.store(true, Ordering::Relaxed);
    s.add_layer(Panel::new(SettingsDialog { selected: 0 }).title("Cover view"));
}

fn close(s: &mut Cursive) {
    OPEN.store(false, Ordering::Relaxed);
    *AREA.lock().unwrap() = None;
    s.pop_layer();
}

type Value = fn(&CoverSettings) -> String;
type Step = fn(&mut CoverSettings, bool);

fn on_off(value: bool) -> String {
    if value { "on" } else { "off" }.to_string()
}

fn step_number(value: usize, forward: bool, min: usize, max: usize, step: usize) -> usize {
    if forward {
        (value + step).min(max)
    } else {
        value.saturating_sub(step).max(min)
    }
}

fn rows() -> Vec<(&'static str, Value, Step)> {
    vec![
        (
            "Visualizer",
            |s| s.visualizer.label().into(),
            |s, f| s.visualizer = s.visualizer.step(f),
        ),
        (
            "Colors",
            |s| s.color_mode.label().into(),
            |s, f| s.color_mode = s.color_mode.step(f),
        ),
        (
            "Palette",
            |s| s.palette.label().into(),
            |s, f| s.palette = s.palette.step(f),
        ),
        (
            "Height",
            |s| s.visualizer_height.to_string(),
            |s, f| s.visualizer_height = step_number(s.visualizer_height, f, 2, 20, 1),
        ),
        (
            "Sensitivity",
            |s| format!("{} dB", s.sensitivity),
            |s, f| s.sensitivity = step_number(s.sensitivity as usize, f, 25, 70, 5) as u8,
        ),
        (
            "Decay",
            |s| s.decay.label().into(),
            |s, f| s.decay = s.decay.step(f),
        ),
        ("Peaks", |s| on_off(s.peaks), |s, _| s.peaks = !s.peaks),
        (
            "Bar width",
            |s| s.bar_width.to_string(),
            |s, f| s.bar_width = step_number(s.bar_width, f, 1, 3, 1),
        ),
        (
            "Bar gap",
            |s| on_off(s.bar_gap),
            |s, _| s.bar_gap = !s.bar_gap,
        ),
        (
            "Frame rate",
            |s| format!("{} fps", s.fps),
            |s, _| s.fps = if s.fps >= 60 { 30 } else { 60 },
        ),
        (
            "Queue column",
            |s| on_off(s.queue_panel),
            |s, _| s.queue_panel = !s.queue_panel,
        ),
        (
            "Recent column",
            |s| s.recent_panel.label().into(),
            |s, f| s.recent_panel = s.recent_panel.step(f),
        ),
        (
            "Thumbnails",
            |s| on_off(s.thumbnails),
            |s, _| s.thumbnails = !s.thumbnails,
        ),
        (
            "Cover width",
            |s| format!("{:.0} %", s.split * 100.0),
            |s, f| {
                let percent = step_number((s.split * 100.0).round() as usize, f, 30, 75, 5);
                s.split = percent as f32 / 100.0
            },
        ),
        (
            "Autoplay",
            |s| on_off(s.autoplay),
            |s, _| s.autoplay = !s.autoplay,
        ),
    ]
}

struct SettingsDialog {
    selected: usize,
}

impl View for SettingsDialog {
    fn draw(&self, printer: &Printer<'_, '_>) {
        *AREA.lock().unwrap() =
            Some((printer.offset.saturating_sub((1, 1)), printer.size + (2, 2)));
        let settings = settings::get();
        let mut y = 0;
        for (i, (label, value, _)) in rows().iter().enumerate() {
            if i == LAYOUT_SECTION {
                printer.with_color(ColorStyle::secondary(), |p| {
                    p.print_hline((1, y), WIDTH - 2, "─");
                });
                y += 1;
            }
            let style = if i == self.selected {
                ColorStyle::highlight()
            } else {
                ColorStyle::primary()
            };
            let value = format!("‹ {} ›", value(&settings));
            printer.with_color(style, |p| {
                p.print_hline((0, y), WIDTH, " ");
                p.print((1, y), label);
                p.print((WIDTH - 1 - value.chars().count(), y), &value);
            });
            y += 1;
        }
        printer.with_color(ColorStyle::secondary(), |p| {
            p.print((1, y + 1), "↑↓ select  ←→ change  Esc close");
        });
    }

    fn required_size(&mut self, _constraint: Vec2) -> Vec2 {
        Vec2::new(WIDTH, rows().len() + 3)
    }

    fn take_focus(&mut self, _source: Direction) -> Result<EventResult, CannotFocus> {
        Ok(EventResult::consumed())
    }

    fn on_event(&mut self, event: Event) -> EventResult {
        let count = rows().len();
        let change = |selected: usize, forward: bool| {
            let step = rows()[selected].2;
            settings::update(|s| step(s, forward));
        };
        match event {
            Event::Key(Key::Up) | Event::Char('k') => {
                self.selected = (self.selected + count - 1) % count;
            }
            Event::Key(Key::Down) | Event::Char('j') => {
                self.selected = (self.selected + 1) % count;
            }
            Event::Key(Key::Left) | Event::Char('h') => change(self.selected, false),
            Event::Key(Key::Right)
            | Event::Char('l')
            | Event::Key(Key::Enter)
            | Event::Char(' ') => change(self.selected, true),
            Event::Key(Key::Esc) | Event::Char(',') | Event::Char('q') => {
                return EventResult::with_cb(close);
            }
            _ => {}
        }
        EventResult::consumed()
    }
}
