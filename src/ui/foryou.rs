//! Spotify's Made For You page with one tab per shelf.

use std::sync::{Arc, RwLock};

use cursive::direction::Direction;
use cursive::event::{Event, EventResult};
use cursive::theme::ColorStyle;
use cursive::view::CannotFocus;
use cursive::{Cursive, Printer, Vec2, View};

use crate::command::Command;
use crate::commands::CommandResult;
use crate::config::Config;
use crate::library::Library;
use crate::queue::Queue;
use crate::spotify_internal::{self, Shelf};
use crate::traits::ViewExt;
use crate::ui::listview::ListView;
use crate::ui::tabbedview::TabbedView;

type LoadResult = Arc<RwLock<Option<Result<Vec<Shelf>, String>>>>;

pub struct ForYouView {
    queue: Arc<Queue>,
    library: Arc<Library>,
    hash: Option<String>,
    loaded: LoadResult,
    loading: bool,
    tabs: Option<TabbedView>,
    error: Option<String>,
}

impl ForYouView {
    pub fn new(queue: Arc<Queue>, library: Arc<Library>, config: &Config) -> Self {
        Self {
            queue,
            library,
            hash: config.values().pathfinder_browse_hash.clone(),
            loaded: Arc::new(RwLock::new(None)),
            loading: false,
            tabs: None,
            error: None,
        }
    }

    fn start_loading(&mut self) {
        if self.loading {
            return;
        }
        self.loading = true;
        let loaded = self.loaded.clone();
        let library = self.library.clone();
        let hash = self.hash.clone();
        std::thread::spawn(move || {
            let result = spotify_internal::made_for_you(hash.as_deref());
            *loaded.write().unwrap() = Some(result);
            library.trigger_redraw();
        });
    }

    fn take_result(&mut self) {
        let Some(result) = self.loaded.write().unwrap().take() else {
            return;
        };
        match result {
            Ok(shelves) => {
                let mut tabs = TabbedView::new();
                for shelf in shelves {
                    let playlists = Arc::new(RwLock::new(shelf.playlists));
                    let list = ListView::new(playlists, self.queue.clone(), self.library.clone());
                    tabs.add_tab(shelf.title, list);
                }
                self.tabs = Some(tabs);
            }
            Err(e) => self.error = Some(e),
        }
    }
}

impl View for ForYouView {
    fn layout(&mut self, size: Vec2) {
        self.start_loading();
        if self.tabs.is_none() && self.error.is_none() {
            self.take_result();
        }
        if let Some(tabs) = self.tabs.as_mut() {
            tabs.layout(size);
        }
    }

    fn draw(&self, printer: &Printer<'_, '_>) {
        match (&self.tabs, &self.error) {
            (Some(tabs), _) => tabs.draw(printer),
            (None, Some(error)) => {
                printer.with_color(ColorStyle::highlight(), |p| p.print((1, 1), error));
                printer.with_color(ColorStyle::secondary(), |p| {
                    p.print((1, 3), "Shift+U reloads");
                });
            }
            (None, None) => {
                printer.with_color(ColorStyle::secondary(), |p| {
                    p.print((1, 1), "Loading For You…");
                });
            }
        }
    }

    fn on_event(&mut self, event: Event) -> EventResult {
        match self.tabs.as_mut() {
            Some(tabs) => tabs.on_event(event),
            None => EventResult::Ignored,
        }
    }

    fn required_size(&mut self, constraint: Vec2) -> Vec2 {
        match self.tabs.as_mut() {
            Some(tabs) => tabs.required_size(constraint),
            None => constraint,
        }
    }

    fn take_focus(&mut self, source: Direction) -> Result<EventResult, CannotFocus> {
        match self.tabs.as_mut() {
            Some(tabs) => tabs.take_focus(source),
            None => Err(CannotFocus),
        }
    }
}

impl ViewExt for ForYouView {
    fn title(&self) -> String {
        "For You".to_string()
    }

    fn on_command(&mut self, s: &mut Cursive, cmd: &Command) -> Result<CommandResult, String> {
        if let Command::UpdateLibrary = cmd {
            self.tabs = None;
            self.error = None;
            self.loading = false;
            return Ok(CommandResult::Consumed(None));
        }
        match self.tabs.as_mut() {
            Some(tabs) => tabs.on_command(s, cmd),
            None => Ok(CommandResult::Ignored),
        }
    }
}
