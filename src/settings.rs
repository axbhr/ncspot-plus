//! Live settings of the cover view. They start from `config.toml`, are
//! changed in the settings dialog and saved to `cover.toml`.

#![cfg_attr(not(feature = "cover"), allow(dead_code))]

use std::fs;
use std::sync::RwLock;

use log::warn;
use serde::{Deserialize, Serialize};

use crate::config::{Config, config_path};

pub const FILE_NAME: &str = "cover.toml";

/// Enum with a fixed set of named values that can be stepped through.
macro_rules! choice {
    ($name:ident { $($variant:ident => $label:literal),+ $(,)? }) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
        pub enum $name {
            $(#[serde(rename = $label)] $variant),+
        }

        impl $name {
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];

            pub fn label(self) -> &'static str {
                match self {
                    $(Self::$variant => $label),+
                }
            }

            #[allow(dead_code)]
            pub fn from_label(label: &str) -> Option<Self> {
                Self::ALL.iter().copied().find(|value| value.label() == label)
            }

            pub fn step(self, forward: bool) -> Self {
                let count = Self::ALL.len();
                let index = Self::ALL.iter().position(|value| *value == self).unwrap_or(0);
                let next = if forward { index + 1 } else { index + count - 1 };
                Self::ALL[next % count]
            }
        }
    };
}

choice!(Visual {
    Bars => "bars",
    Mirror => "mirror",
    Dots => "dots",
    Line => "line",
    Stereo => "stereo",
    Waterfall => "waterfall",
    Circle => "circle",
    Meter => "meter",
    Wave => "wave",
    Off => "off",
});

choice!(ColorMode {
    Solid => "solid",
    Gradient => "gradient",
    Rainbow => "rainbow",
    Heat => "heat",
});

choice!(Palette {
    Theme => "theme",
    Cover => "cover",
    Spotify => "spotify",
    Sunset => "sunset",
    Ocean => "ocean",
    Mono => "mono",
});

choice!(PanelMode {
    OnDemand => "on-demand",
    Always => "always",
    Off => "off",
});

choice!(Decay {
    Slow => "slow",
    Normal => "normal",
    Fast => "fast",
});

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct CoverSettings {
    pub visualizer: Visual,
    pub color_mode: ColorMode,
    pub palette: Palette,
    pub visualizer_height: usize,
    /// Dynamic range shown by the visualizer in dB.
    pub sensitivity: u8,
    pub decay: Decay,
    pub peaks: bool,
    pub bar_width: usize,
    pub bar_gap: bool,
    pub fps: u32,
    pub queue_panel: bool,
    pub recent_panel: PanelMode,
    pub thumbnails: bool,
    /// Share of the free width used by the cover.
    pub split: f32,
    pub autoplay: bool,
}

impl Default for CoverSettings {
    fn default() -> Self {
        Self {
            visualizer: Visual::Bars,
            color_mode: ColorMode::Gradient,
            palette: Palette::Theme,
            visualizer_height: 8,
            sensitivity: 45,
            decay: Decay::Normal,
            peaks: true,
            bar_width: 1,
            bar_gap: true,
            fps: 30,
            queue_panel: true,
            recent_panel: PanelMode::OnDemand,
            thumbnails: true,
            split: 0.55,
            autoplay: true,
        }
    }
}

static SETTINGS: RwLock<Option<CoverSettings>> = RwLock::new(None);

/// Load the settings: `cover.toml` if it exists, otherwise `config.toml`.
pub fn init(config: &Config) {
    let values = config.values();
    let mut settings = CoverSettings::default();
    if let Some(visual) = values.visualizer.as_deref().and_then(Visual::from_label) {
        settings.visualizer = visual;
    }
    if let Some(height) = values.visualizer_height {
        settings.visualizer_height = height.clamp(2, 20);
    }
    if let Some(queue) = values.cover_queue {
        settings.queue_panel = queue;
    }
    if let Some(split) = values.cover_split {
        settings.split = split.clamp(0.3, 0.75);
    }
    if let Some(autoplay) = values.autoplay {
        settings.autoplay = autoplay;
    }

    if let Ok(text) = fs::read_to_string(config_path(FILE_NAME)) {
        match toml::from_str(&text) {
            Ok(saved) => settings = saved,
            Err(e) => warn!("Ignoring invalid {FILE_NAME}: {e}"),
        }
    }
    *SETTINGS.write().unwrap() = Some(settings);
}

pub fn get() -> CoverSettings {
    SETTINGS.read().unwrap().clone().unwrap_or_default()
}

/// Change the settings and save them to `cover.toml`.
pub fn update(change: impl FnOnce(&mut CoverSettings)) {
    let mut guard = SETTINGS.write().unwrap();
    let settings = guard.get_or_insert_with(CoverSettings::default);
    change(settings);
    match toml::to_string_pretty(settings) {
        Ok(text) => {
            let text = format!("# Written by the cover view settings dialog.\n{text}");
            if let Err(e) = fs::write(config_path(FILE_NAME), text) {
                warn!("Could not save {FILE_NAME}: {e}");
            }
        }
        Err(e) => warn!("Could not serialize the cover settings: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choices_step_around() {
        assert_eq!(Visual::Bars.step(false), Visual::Off);
        assert_eq!(Visual::Off.step(true), Visual::Bars);
        assert_eq!(
            PanelMode::from_label("on-demand"),
            Some(PanelMode::OnDemand)
        );
        assert_eq!(Visual::from_label("peaks"), None);
    }

    #[test]
    fn partial_file_keeps_defaults() {
        let settings: CoverSettings = toml::from_str("visualizer = \"circle\"").unwrap();
        assert_eq!(settings.visualizer, Visual::Circle);
        assert_eq!(settings.recent_panel, PanelMode::OnDemand);
    }
}
