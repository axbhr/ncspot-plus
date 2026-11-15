//! Cover view: the album art, recently played playlists on the left (Ctrl+←),
//! the queue with a visualizer on the right (Ctrl+→) and a settings dialog.

mod dialog;
mod images;
mod panels;

use std::collections::HashSet;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use cursive::event::{Event, EventResult, Key};
use cursive::theme::{Color, ColorStyle, ColorType, PaletteColor};
use cursive::{Cursive, Printer, Vec2, View};
use ioctl_rs::{TIOCGWINSZ, ioctl};
use log::{debug, error};

use crate::command::{Command, GotoMode};
use crate::commands::CommandResult;
use crate::config::Config;
use crate::library::Library;
use crate::model::album::Album;
use crate::model::playable::Playable;
use crate::model::playlist::Playlist;
use crate::queue::Queue;
use crate::recent::{self, RecentEntry};
use crate::settings::{self, CoverSettings, Palette, PanelMode, Visual};
use crate::spotify::PlayerEvent;
use crate::traits::{IntoBoxedViewExt, ListItem, ViewExt};
use crate::ui::album::AlbumView;
use crate::ui::artist::ArtistView;
use crate::ui::layout::Layout;
use crate::visualizer::Visualizer;
use crate::visualizer::colors::{Paint, Rgb, dominant};
use images::{Images, Placement};
use panels::{Column, Entry};

/// Below this width the queue column is hidden.
const MIN_QUEUE_WIDTH: usize = 90;
/// Below this width only one side column is shown at a time.
const MIN_THREE_COLUMNS: usize = 130;
const VOLUME_STEP: u16 = (u16::MAX / 100) * 5;
/// Played tracks kept visible above the current one.
const PLAYED_SHOWN: usize = 2;

/// Which part of the cover view receives the arrow keys.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pane {
    Recent,
    Cover,
    Queue,
}

/// Horizontal position and width of the visible columns.
struct Columns {
    recent: Option<(usize, usize)>,
    cover: (usize, usize),
    queue: Option<(usize, usize)>,
}

/// Cover drawn by viuer on terminals without the kitty protocol.
#[derive(Clone, PartialEq, Eq)]
struct CoverRequest {
    path: PathBuf,
    offset: Vec2,
    size: Vec2,
}

pub struct CoverView {
    queue: Arc<Queue>,
    library: Arc<Library>,
    loading: Arc<RwLock<HashSet<String>>>,
    cover_max_scale: Option<f32>,
    font_size: Vec2,
    pane: Pane,
    selected_queue: usize,
    selected_recent: usize,
    visualizer: Arc<Visualizer>,
    /// Set while the view is on screen, drives the animation ticker.
    active: Arc<AtomicBool>,
    last_size: RwLock<Vec2>,
    kitty: bool,
    images: RwLock<Images>,
    desired_images: RwLock<Vec<Placement>>,
    desired_cover: RwLock<Option<CoverRequest>>,
    rendered_cover: RwLock<Option<CoverRequest>>,
    /// Dominant color of the current cover, by cover URL.
    accent: Arc<RwLock<Option<(String, Rgb)>>>,
}

impl CoverView {
    pub fn new(queue: Arc<Queue>, library: Arc<Library>, config: &Config) -> Self {
        // Determine size of window both in pixels and chars
        let (rows, cols, xpixels, ypixels) = unsafe {
            let mut query: (u16, u16, u16, u16) = (0, 0, 0, 0);
            ioctl(1, TIOCGWINSZ, &mut query);
            query
        };
        debug!("Determined window dimensions: {xpixels}x{ypixels}, {cols}x{rows}");

        // Some terminals report physical pixels here, but the aspect ratio
        // is still useful when mapping images to terminal cells.
        let font_size = if cols == 0 || rows == 0 || xpixels == 0 || ypixels == 0 {
            Vec2::new(8, 16)
        } else {
            Vec2::new(
                std::cmp::max(1, xpixels / cols) as usize,
                std::cmp::max(1, ypixels / rows) as usize,
            )
        };

        let visualizer = Arc::new(Visualizer::new());
        let active = Arc::new(AtomicBool::new(false));
        spawn_ticker(
            queue.clone(),
            library.clone(),
            visualizer.clone(),
            active.clone(),
        );

        Self {
            queue,
            library,
            loading: Arc::new(RwLock::new(HashSet::new())),
            cover_max_scale: config.values().cover_max_scale,
            font_size,
            pane: Pane::Cover,
            selected_queue: 0,
            selected_recent: 0,
            visualizer,
            active,
            last_size: RwLock::new(Vec2::zero()),
            kitty: images::supported(),
            images: RwLock::new(Images::new(font_size)),
            desired_images: RwLock::new(Vec::new()),
            desired_cover: RwLock::new(None),
            rendered_cover: RwLock::new(None),
            accent: Arc::new(RwLock::new(None)),
        }
    }

    /// Downloaded file of a cover, starting the download if needed.
    fn cache_path(&self, url: &str) -> Option<PathBuf> {
        let path = crate::utils::cache_path_for_url(url.to_string());
        let mut loading = self.loading.write().unwrap();
        if loading.contains(url) {
            return None;
        }
        if path.exists() {
            return Some(path);
        }

        loading.insert(url.to_string());
        let loading = self.loading.clone();
        let library = self.library.clone();
        let url = url.to_string();
        std::thread::spawn(move || {
            if let Err(e) = crate::utils::download(url.clone(), path) {
                error!("Failed to download cover: {e}");
            }
            loading.write().unwrap().remove(&url);
            library.trigger_redraw();
        });
        None
    }

    /// Dominant color of the cover at `path`, computed in the background.
    fn accent_for(&self, url: &str, path: PathBuf) -> Option<Rgb> {
        let current = self.accent.read().unwrap().clone();
        if let Some((known, color)) = &current
            && known == url
        {
            return Some(*color);
        }

        let previous = current.map(|(_, color)| color);
        *self.accent.write().unwrap() = Some((url.to_string(), previous.unwrap_or((46, 158, 244))));
        let accent = self.accent.clone();
        let library = self.library.clone();
        let url = url.to_string();
        std::thread::spawn(move || {
            if let Some(color) = dominant(&path) {
                *accent.write().unwrap() = Some((url, color));
                library.trigger_redraw();
            }
        });
        previous
    }

    fn columns(&self, width: usize, settings: &CoverSettings) -> Columns {
        let mut show_recent = match settings.recent_panel {
            PanelMode::Always => true,
            PanelMode::OnDemand => self.pane == Pane::Recent,
            PanelMode::Off => false,
        };
        let mut show_queue = settings.queue_panel && width >= MIN_QUEUE_WIDTH;
        if show_recent && show_queue && width < MIN_THREE_COLUMNS {
            if self.pane == Pane::Queue {
                show_recent = false;
            } else {
                show_queue = false;
            }
        }

        let recent_width = (width / 4).clamp(24, 40).min(width);
        let x = if show_recent { recent_width + 1 } else { 0 };
        let rest = width.saturating_sub(x);
        if !show_queue {
            return Columns {
                recent: show_recent.then_some((0, recent_width)),
                cover: (x, rest),
                queue: None,
            };
        }
        let cover_width = (rest as f32 * settings.split) as usize;
        Columns {
            recent: show_recent.then_some((0, recent_width)),
            cover: (x, cover_width),
            queue: Some((x + cover_width + 2, rest.saturating_sub(cover_width + 3))),
        }
    }

    fn queue_possible(&self) -> bool {
        settings::get().queue_panel && self.last_size.read().unwrap().x >= MIN_QUEUE_WIDTH
    }

    /// Cells used by the cover inside `area`, centered and in aspect.
    fn fit_cover(&self, path: &PathBuf, mut offset: Vec2, area: Vec2) -> Option<(Vec2, Vec2)> {
        if area.x <= 1 || area.y <= 1 {
            return None;
        }
        let (width, height) = image::image_dimensions(path).unwrap_or((640, 640));
        let mut available = area;
        if let Some(scale) = self.cover_max_scale {
            available.x = available
                .x
                .min((((width as f32 * scale) / self.font_size.x as f32) as usize).max(1));
            available.y = available
                .y
                .min((((height as f32 * scale) / self.font_size.y as f32) as usize).max(1));
        }
        let size = fit_image_to_cells(available, self.font_size, width, height);
        offset.x += (area.x - size.x) / 2;
        offset.y += (area.y - size.y) / 2;
        Some((offset, size))
    }

    /// The whole queue in play order, the autoplay divider position and the
    /// position of the current track.
    fn queue_entries(&self) -> (Vec<Entry>, Option<usize>, Option<usize>) {
        let (order, current) = self.queue.play_order();
        let tracks = self.queue.queue.read().unwrap();
        let autoplay = self.queue.autoplay_start();
        let mut divider = None;
        let entries = order
            .iter()
            .enumerate()
            .filter_map(|(position, index)| {
                if autoplay == Some(*index) {
                    divider = Some(position);
                }
                let item = tracks.get(*index)?;
                let (title, subtitle) = describe(item);
                Some(Entry {
                    title,
                    subtitle,
                    right: item.duration_str(),
                    cover_url: item.cover_url(),
                    playing: current == Some(position),
                    played: current.is_some_and(|current| position < current),
                })
            })
            .collect();
        (entries, divider, current)
    }

    fn recent_entries(&self) -> Vec<Entry> {
        let context = self.queue.context_name();
        recent::entries()
            .into_iter()
            .map(|entry| Entry {
                playing: context.as_deref() == Some(entry.name.as_str()),
                subtitle: format!("{} · {}", entry.kind(), entry.age()),
                title: entry.name,
                right: String::new(),
                cover_url: entry.cover_url,
                played: false,
            })
            .collect()
    }

    fn previous(&self) {
        let spotify = self.queue.get_spotify();
        if spotify.get_current_progress() < Duration::from_secs(3) {
            self.queue.previous();
        } else {
            spotify.seek(0);
        }
    }

    fn change_volume(&self, up: bool) {
        let spotify = self.queue.get_spotify();
        let volume = if up {
            spotify.volume().saturating_add(VOLUME_STEP)
        } else {
            spotify.volume().saturating_sub(VOLUME_STEP)
        };
        spotify.set_volume(volume, true);
    }

    fn on_cover_event(&mut self, event: Event) -> EventResult {
        match event {
            Event::Char(' ') | Event::Key(Key::Enter) => self.queue.toggleplayback(),
            Event::Key(Key::Right) => self.queue.next(true),
            Event::Key(Key::Left) => self.previous(),
            Event::Shift(Key::Right) => self.queue.get_spotify().seek_relative(10_000),
            Event::Shift(Key::Left) => self.queue.get_spotify().seek_relative(-10_000),
            Event::Key(Key::Up) => self.change_volume(true),
            Event::Key(Key::Down) => self.change_volume(false),
            _ => return EventResult::Ignored,
        }
        EventResult::consumed()
    }

    fn on_queue_event(&mut self, event: Event) -> EventResult {
        let (order, current) = self.queue.play_order();
        let last = order.len().saturating_sub(1);
        match event {
            Event::Key(Key::Up) | Event::Char('k') => {
                self.selected_queue = self.selected_queue.saturating_sub(1)
            }
            Event::Key(Key::Down) | Event::Char('j') => {
                self.selected_queue = (self.selected_queue + 1).min(last)
            }
            Event::Key(Key::Home) => self.selected_queue = 0,
            Event::Key(Key::End) => self.selected_queue = last,
            Event::Key(Key::Enter) => {
                if let Some(index) = order.get(self.selected_queue) {
                    self.queue.play(*index, false, false);
                }
            }
            Event::Key(Key::Del) | Event::Char('d') if current != Some(self.selected_queue) => {
                if let Some(index) = order.get(self.selected_queue) {
                    self.queue.remove(*index);
                    self.selected_queue = self.selected_queue.min(last.saturating_sub(1));
                }
            }
            Event::Char(' ') => self.queue.toggleplayback(),
            Event::Key(Key::Esc) => self.pane = Pane::Cover,
            _ => return EventResult::Ignored,
        }
        EventResult::consumed()
    }

    fn on_recent_event(&mut self, event: Event) -> EventResult {
        let entries = recent::entries();
        let last = entries.len().saturating_sub(1);
        match event {
            Event::Key(Key::Up) | Event::Char('k') => {
                self.selected_recent = self.selected_recent.saturating_sub(1)
            }
            Event::Key(Key::Down) | Event::Char('j') => {
                self.selected_recent = (self.selected_recent + 1).min(last)
            }
            Event::Key(Key::Home) => self.selected_recent = 0,
            Event::Key(Key::End) => self.selected_recent = last,
            Event::Key(Key::Enter) => {
                if let Some(entry) = entries.get(self.selected_recent) {
                    self.play_recent(entry.clone());
                }
            }
            Event::Char(' ') => self.queue.toggleplayback(),
            Event::Key(Key::Esc) => self.pane = Pane::Cover,
            _ => return EventResult::Ignored,
        }
        EventResult::consumed()
    }

    /// Load and play a recent playlist or album without blocking the UI.
    fn play_recent(&self, entry: RecentEntry) {
        let queue = self.queue.clone();
        let library = self.library.clone();
        std::thread::spawn(move || {
            if let Some(id) = entry.uri.strip_prefix("spotify:playlist:") {
                let known = library
                    .playlists
                    .read()
                    .unwrap()
                    .iter()
                    .find(|playlist| playlist.id == id)
                    .cloned();
                let mut playlist = known.unwrap_or_else(|| Playlist {
                    id: id.to_string(),
                    name: entry.name.clone(),
                    owner_id: String::new(),
                    owner_name: None,
                    snapshot_id: String::new(),
                    num_tracks: 0,
                    tracks: None,
                    collaborative: false,
                });
                playlist.play(&queue);
            } else if let Some(id) = entry.uri.strip_prefix("spotify:album:") {
                match queue.get_spotify().api.album(id) {
                    Ok(album) => Album::from(&album).play(&queue),
                    Err(_) => error!("Could not load album {id}"),
                }
            }
            library.trigger_redraw();
        });
    }

    pub fn render_to_terminal(&self) {
        if self.kitty {
            let desired = self.desired_images.read().unwrap().clone();
            self.images.write().unwrap().show(&desired, dialog::area());
            return;
        }

        let desired = if dialog::is_open() {
            None
        } else {
            self.desired_cover.read().unwrap().clone()
        };
        let mut rendered = self.rendered_cover.write().unwrap();
        if *rendered == desired {
            return;
        }
        if let Some(cover) = rendered.as_ref() {
            clear_terminal_area(cover.offset, cover.size);
        }
        if let Some(cover) = desired.as_ref()
            && let Err(e) = render_cover_to_terminal(cover)
        {
            error!("Failed to draw cover: {e}");
            return;
        }
        *rendered = desired;
    }
}

fn describe(item: &Playable) -> (String, String) {
    match item {
        Playable::Track(track) => (track.title.clone(), track.artists.join(", ")),
        Playable::Episode(episode) => (episode.name.clone(), String::new()),
    }
}

fn status_message(message: String) -> EventResult {
    EventResult::with_cb(move |s| {
        let message = message.clone();
        s.call_on_name("main", move |layout: &mut Layout| {
            layout.set_result(Ok(Some(message)));
        });
    })
}

/// Redraws the view while the visualizer animates.
fn spawn_ticker(
    queue: Arc<Queue>,
    library: Arc<Library>,
    visualizer: Arc<Visualizer>,
    active: Arc<AtomicBool>,
) {
    std::thread::spawn(move || {
        loop {
            let settings = settings::get();
            std::thread::sleep(Duration::from_millis(
                1000 / settings.fps.clamp(10, 60) as u64,
            ));
            if !active.load(Ordering::Relaxed) || settings.visualizer == Visual::Off {
                continue;
            }
            let playing = matches!(
                queue.get_spotify().get_current_status(),
                PlayerEvent::Playing(_)
            );
            if playing || !visualizer.is_idle() {
                library.trigger_redraw();
            }
        }
    });
}

fn render_cover_to_terminal(cover: &CoverRequest) -> Result<(), viuer::ViuError> {
    let config = viuer::Config {
        x: to_u16(cover.offset.x)?,
        y: to_i16(cover.offset.y)?,
        width: Some(cover.size.x as u32),
        height: Some(cover.size.y as u32),
        absolute_offset: true,
        restore_cursor: true,
        use_kitty: false,
        use_sixel: !is_iterm_terminal(),
        ..Default::default()
    };

    let image = image::ImageReader::open(&cover.path)?
        .with_guessed_format()?
        .decode()?;

    viuer::print(&image, &config).map(|_| ())
}

fn is_iterm_terminal() -> bool {
    std::env::var("TERM_PROGRAM").is_ok_and(|term| term.contains("iTerm"))
        || std::env::var("LC_TERMINAL").is_ok_and(|term| term.contains("iTerm"))
}

fn fit_image_to_cells(
    available_size: Vec2,
    font_size: Vec2,
    image_width: u32,
    image_height: u32,
) -> Vec2 {
    if available_size.x == 0 || available_size.y == 0 || font_size.x == 0 || font_size.y == 0 {
        return Vec2::new(0, 0);
    }
    if image_width == 0 || image_height == 0 {
        return available_size;
    }

    let image_aspect = image_width as f32 / image_height as f32;
    let cell_aspect = font_size.x as f32 / font_size.y as f32;
    let width_for_full_height =
        (available_size.y as f32 * image_aspect / cell_aspect).floor() as usize;

    if width_for_full_height <= available_size.x {
        Vec2::new(std::cmp::max(1, width_for_full_height), available_size.y)
    } else {
        let height_for_full_width =
            (available_size.x as f32 * cell_aspect / image_aspect).floor() as usize;
        Vec2::new(available_size.x, std::cmp::max(1, height_for_full_width))
    }
}

/// Blank the cells of a sixel or block cover. Attributes are reset first so
/// the blanks do not inherit a background color from the last drawn cell.
fn clear_terminal_area(offset: Vec2, size: Vec2) {
    let mut stdout = std::io::stdout();
    let _ = stdout.write_all(b"\x1b7\x1b[0m");
    for y in offset.y..offset.y + size.y {
        let _ = write!(
            stdout,
            "\x1b[{};{}H{}",
            y + 1,
            offset.x + 1,
            " ".repeat(size.x)
        );
    }
    let _ = stdout.write_all(b"\x1b8");
    let _ = stdout.flush();
}

fn to_u16(value: usize) -> Result<u16, viuer::ViuError> {
    u16::try_from(value).map_err(|_| {
        viuer::ViuError::InvalidConfiguration("cover coordinate is too large".to_string())
    })
}

fn to_i16(value: usize) -> Result<i16, viuer::ViuError> {
    i16::try_from(value).map_err(|_| {
        viuer::ViuError::InvalidConfiguration("cover coordinate is too large".to_string())
    })
}

impl View for CoverView {
    fn draw(&self, printer: &Printer<'_, '_>) {
        self.active.store(true, Ordering::Relaxed);
        let settings = settings::get();
        let size = printer.size;
        {
            let mut last = self.last_size.write().unwrap();
            if *last != size {
                *last = size;
                self.images.write().unwrap().invalidate();
            }
        }

        // Completely blank out screen
        let style = ColorStyle::new(
            ColorType::Palette(PaletteColor::Background),
            ColorType::Palette(PaletteColor::Background),
        );
        printer.with_color(style, |printer| {
            for i in 0..printer.size.y {
                printer.print_hline((0, i), printer.size.x, " ");
            }
        });

        let palette = &printer.theme.palette;
        let theme_color = palette
            .custom("visualizer")
            .copied()
            .unwrap_or(Color::TerminalDefault);
        let theme_peak = palette
            .custom("visualizer_peak")
            .copied()
            .unwrap_or(theme_color);
        let playing_color = palette
            .custom("playing")
            .copied()
            .unwrap_or(Color::TerminalDefault);

        let columns = self.columns(size.x, &settings);
        let mut placements = Vec::new();
        let cover_url = self.queue.get_current().and_then(|t| t.cover_url());
        let cover_path = cover_url.as_deref().and_then(|url| self.cache_path(url));

        let accent = match (&cover_url, &cover_path) {
            (Some(url), Some(path)) if settings.palette == Palette::Cover => {
                self.accent_for(url, path.clone())
            }
            _ => None,
        };
        let paint = Paint::new(&settings, theme_color, theme_peak, accent);
        let list_accent = match settings.palette {
            Palette::Theme => playing_color,
            _ => paint.accent(),
        };

        let (cover_x, cover_width) = columns.cover;
        let fitted = cover_path.as_ref().and_then(|path| {
            let area = Vec2::new(cover_width, size.y);
            self.fit_cover(path, printer.offset + (cover_x, 0), area)
                .map(|(offset, size)| (path.clone(), offset, size))
        });
        let mut desired_cover = None;
        if let Some((path, offset, size)) = fitted {
            if self.kitty {
                placements.push(Placement { path, offset, size });
            } else {
                desired_cover = Some(CoverRequest { path, offset, size });
            }
        }
        *self.desired_cover.write().unwrap() = desired_cover;

        let thumbnails = self.kitty && settings.thumbnails;
        let cover_lookup = |url: &str| self.cache_path(url);
        let separator = ColorStyle::secondary();

        if let Some((x, width)) = columns.recent {
            printer.with_color(separator, |p| p.print_vline((x + width, 0), size.y, "│"));
            let selected = (self.pane == Pane::Recent).then_some(self.selected_recent);
            let column = Column {
                header: "RECENTLY PLAYED".to_string(),
                first: panels::scroll(selected, 0, panels::rows(size.y)),
                selected,
                divider: None,
                accent: list_accent,
                thumbnails,
            };
            let area = printer
                .offset((x + 1, 0))
                .cropped((width.saturating_sub(2), size.y));
            panels::draw(
                &area,
                &self.recent_entries(),
                &column,
                &cover_lookup,
                &mut placements,
            );
        }

        if let Some((x, width)) = columns.queue {
            printer.with_color(separator, |p| p.print_vline((x - 2, 0), size.y, "│"));
            let area = printer.offset((x, 0)).cropped((width, size.y));
            let visualizer_height = match settings.visualizer {
                Visual::Off => 0,
                _ => settings.visualizer_height.min(area.size.y / 2),
            };
            let list_height = area.size.y - visualizer_height;

            let (entries, divider, current) = self.queue_entries();
            // One row less when the autoplay divider takes space.
            let rows = panels::rows(list_height).saturating_sub(usize::from(divider.is_some()));
            let selected = (self.pane == Pane::Queue).then_some(self.selected_queue);
            let anchor = current.unwrap_or(0).saturating_sub(PLAYED_SHOWN);
            let header = match self.queue.context_name() {
                Some(name) => format!("UP NEXT · {name}"),
                None => "UP NEXT".to_string(),
            };
            let column = Column {
                header,
                first: panels::scroll(selected, anchor, rows),
                selected,
                divider: divider.map(|at| (at, "Autoplay")),
                accent: list_accent,
                thumbnails,
            };
            let list = area.cropped((area.size.x, list_height));
            panels::draw(&list, &entries, &column, &cover_lookup, &mut placements);

            if visualizer_height > 0 {
                self.visualizer
                    .draw(&area.offset((0, list_height)), &settings, &paint);
            }
        }

        if self.kitty {
            // Pictures are only visible above cells with the default
            // background, everything else (like dialogs) covers them.
            let clear = ColorStyle::new(
                ColorType::Color(Color::TerminalDefault),
                ColorType::Color(Color::TerminalDefault),
            );
            printer.with_color(clear, |p| {
                for placement in &placements {
                    let x = placement.offset.x.saturating_sub(printer.offset.x);
                    let y = placement.offset.y.saturating_sub(printer.offset.y);
                    for row in y..y + placement.size.y {
                        p.print_hline((x, row), placement.size.x, " ");
                    }
                }
            });
        }
        *self.desired_images.write().unwrap() = placements;
    }

    fn on_event(&mut self, event: Event) -> EventResult {
        let settings = settings::get();
        match event {
            Event::Ctrl(Key::Left) => {
                self.pane = match self.pane {
                    Pane::Queue => Pane::Cover,
                    Pane::Cover if settings.recent_panel != PanelMode::Off => {
                        self.selected_recent = 0;
                        Pane::Recent
                    }
                    pane => pane,
                };
                EventResult::consumed()
            }
            Event::Ctrl(Key::Right) => {
                self.pane = match self.pane {
                    Pane::Recent => Pane::Cover,
                    Pane::Cover if self.queue_possible() => {
                        let (order, current) = self.queue.play_order();
                        let next = current.map(|current| current + 1).unwrap_or(0);
                        self.selected_queue = next.min(order.len().saturating_sub(1));
                        Pane::Queue
                    }
                    pane => pane,
                };
                EventResult::consumed()
            }
            Event::Char(',') => EventResult::with_cb(dialog::open),
            Event::Char('v') => {
                settings::update(|s| s.visualizer = s.visualizer.step(true));
                status_message(format!(
                    "Visualizer: {}",
                    settings::get().visualizer.label()
                ))
            }
            _ => match self.pane {
                Pane::Recent => self.on_recent_event(event),
                Pane::Queue if self.queue_possible() => self.on_queue_event(event),
                _ => self.on_cover_event(event),
            },
        }
    }

    fn required_size(&mut self, constraint: Vec2) -> Vec2 {
        Vec2::new(constraint.x, 2)
    }
}

impl ViewExt for CoverView {
    fn title(&self) -> String {
        "Cover".to_string()
    }

    fn on_leave(&self) {
        self.active.store(false, Ordering::Relaxed);
        self.desired_images.write().unwrap().clear();
        *self.desired_cover.write().unwrap() = None;
        self.render_to_terminal();
    }

    fn on_command(&mut self, _s: &mut Cursive, cmd: &Command) -> Result<CommandResult, String> {
        match cmd {
            Command::Save => {
                if let Some(mut track) = self.queue.get_current() {
                    track.save(&self.library);
                }
            }
            Command::Delete => {
                if let Some(mut track) = self.queue.get_current() {
                    track.unsave(&self.library);
                }
            }
            #[cfg(feature = "share_clipboard")]
            Command::Share(_mode) => {
                let url = self
                    .queue
                    .get_current()
                    .and_then(|t| t.as_listitem().share_url());

                if let Some(url) = url {
                    crate::sharing::write_share(url).ok();
                }

                return Ok(CommandResult::Consumed(None));
            }
            Command::Goto(mode) => {
                if let Some(track) = self.queue.get_current() {
                    let queue = self.queue.clone();
                    let library = self.library.clone();

                    match mode {
                        GotoMode::Album => {
                            if let Some(album) = track.album(&queue) {
                                let view =
                                    AlbumView::new(queue, library, &album).into_boxed_view_ext();
                                return Ok(CommandResult::View(view));
                            }
                        }
                        GotoMode::Artist => {
                            if let Some(artists) = track.artists() {
                                return match artists.len() {
                                    0 => Ok(CommandResult::Consumed(None)),
                                    // Always choose the first artist even with more because
                                    // the cover image really doesn't play nice with the menu
                                    _ => {
                                        let view = ArtistView::new(queue, library, &artists[0])
                                            .into_boxed_view_ext();
                                        Ok(CommandResult::View(view))
                                    }
                                };
                            }
                        }
                    }
                }
            }
            _ => {}
        };

        Ok(CommandResult::Ignored)
    }
}
