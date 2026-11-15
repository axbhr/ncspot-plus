//! Playlists and albums recently started in ncspot, newest first.

#![cfg_attr(not(feature = "cover"), allow(dead_code))]

use std::fs;
use std::sync::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};

use log::warn;
use serde::{Deserialize, Serialize};

use crate::config::cache_path;

const FILE_NAME: &str = "recent.json";
const LIMIT: usize = 30;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecentEntry {
    pub uri: String,
    pub name: String,
    pub cover_url: Option<String>,
    /// Seconds since the Unix epoch.
    pub played_at: u64,
}

impl RecentEntry {
    pub fn kind(&self) -> &'static str {
        if self.uri.starts_with("spotify:album:") {
            "Album"
        } else {
            "Playlist"
        }
    }

    /// Short relative time like "5 min ago".
    pub fn age(&self) -> String {
        let seconds = now().saturating_sub(self.played_at);
        match seconds {
            0..60 => "just now".to_string(),
            60..3600 => format!("{} min ago", seconds / 60),
            3600..86400 => format!("{} h ago", seconds / 3600),
            _ => format!("{} d ago", seconds / 86400),
        }
    }
}

static ENTRIES: RwLock<Option<Vec<RecentEntry>>> = RwLock::new(None);

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

fn load() -> Vec<RecentEntry> {
    fs::read_to_string(cache_path(FILE_NAME))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

pub fn entries() -> Vec<RecentEntry> {
    ENTRIES.write().unwrap().get_or_insert_with(load).clone()
}

pub fn record(uri: &str, name: &str, cover_url: Option<String>) {
    let mut guard = ENTRIES.write().unwrap();
    let entries = guard.get_or_insert_with(load);
    entries.retain(|entry| entry.uri != uri);
    entries.insert(
        0,
        RecentEntry {
            uri: uri.to_string(),
            name: name.to_string(),
            cover_url,
            played_at: now(),
        },
    );
    entries.truncate(LIMIT);
    match serde_json::to_string(entries) {
        Ok(text) => {
            if let Err(e) = fs::write(cache_path(FILE_NAME), text) {
                warn!("Could not save {FILE_NAME}: {e}");
            }
        }
        Err(e) => warn!("Could not serialize recent entries: {e}"),
    }
}
