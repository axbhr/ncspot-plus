//! Spotify's internal APIs through the librespot session. Unlike the Web API
//! they are not rate limited and also serve Spotify's own playlists.

use std::collections::HashSet;
use std::sync::{Mutex, RwLock};

use bytes::Bytes;
use futures::{StreamExt, stream};
use http::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderValue};
use http::{Method, Request};
use librespot_core::session::Session;
use librespot_core::spotify_uri::SpotifyUri;
use librespot_metadata::{Metadata, Playlist as SpotifyPlaylist, Track as SpotifyTrack};
use librespot_protocol::autoplay_context_request::AutoplayContextRequest;
use log::{debug, warn};
use serde_json::{Value, json};

use crate::application::ASYNC_RUNTIME;
use crate::model::playable::Playable;
use crate::model::playlist::Playlist;
use crate::model::track::Track;

const PATHFINDER_URL: &str = "https://api-partner.spotify.com/pathfinder/v2/query";
/// Persisted `browsePage` query, rotated by Spotify from time to time.
const BROWSE_PAGE_HASH: &str = "f5c4e6d668f5716464a231c1cc8b22c1cbf6ad68b09929fd7de813a30581298b";
const MADE_FOR_YOU_PAGE: &str = "spotify:page:0JQ5DAt0tbjZptfcdMSKl3";
/// The AI DJ is listed on the page but cannot be played by librespot.
const DJ_PLAYLIST: &str = "spotify:playlist:37i9dQZF1EYkqdzj48dyYq";
const METADATA_CONCURRENCY: usize = 10;
const AUTOPLAY_TRACKS: usize = 30;

static SESSION: RwLock<Option<Session>> = RwLock::new(None);
static REFRESHED: Mutex<Option<HashSet<String>>> = Mutex::new(None);

/// A shelf of the Made For You page.
pub struct Shelf {
    pub title: String,
    pub playlists: Vec<Playlist>,
}

pub fn set_session(session: Session) {
    *SESSION.write().unwrap() = Some(session);
}

fn session() -> Option<Session> {
    SESSION.read().unwrap().clone()
}

/// True the first time it is called for `playlist_id` in this run.
pub fn first_load(playlist_id: &str) -> bool {
    REFRESHED
        .lock()
        .unwrap()
        .get_or_insert_with(HashSet::new)
        .insert(playlist_id.to_string())
}

fn block_on<F: Future>(future: F) -> F::Output {
    let runtime = ASYNC_RUNTIME.get().expect("async runtime is started");
    if tokio::runtime::Handle::try_current().is_ok() {
        tokio::task::block_in_place(|| runtime.block_on(future))
    } else {
        runtime.block_on(future)
    }
}

fn convert(track: SpotifyTrack) -> Option<Track> {
    let id = track.id.to_id().ok()?;
    let cover_url = track
        .album
        .covers
        .iter()
        .max_by_key(|cover| cover.width)
        .and_then(|cover| cover.id.to_base16().ok())
        .map(|hex| format!("https://i.scdn.co/image/{hex}"));

    Some(Track {
        uri: format!("spotify:track:{id}"),
        url: format!("https://open.spotify.com/track/{id}"),
        id: Some(id),
        title: track.name,
        track_number: track.number.max(0) as u32,
        disc_number: track.disc_number,
        duration: track.duration.max(0) as u32,
        artists: track.artists.iter().map(|a| a.name.clone()).collect(),
        artist_ids: track
            .artists
            .iter()
            .filter_map(|a| a.id.to_id().ok())
            .collect(),
        album: Some(track.album.name.clone()),
        album_id: track.album.id.to_id().ok(),
        album_artists: track.album.artists.iter().map(|a| a.name.clone()).collect(),
        cover_url,
        added_at: None,
        list_index: 0,
        is_local: false,
        is_playable: Some(true),
    })
}

async fn fetch_tracks(session: &Session, uris: Vec<SpotifyUri>) -> Vec<Playable> {
    let tracks: Vec<Option<Track>> = stream::iter(uris)
        .map(|uri| async move {
            match SpotifyTrack::get(session, &uri).await {
                Ok(track) => convert(track),
                Err(e) => {
                    warn!("Failed to load track metadata: {e}");
                    None
                }
            }
        })
        .buffered(METADATA_CONCURRENCY)
        .collect()
        .await;

    tracks
        .into_iter()
        .flatten()
        .enumerate()
        .map(|(index, mut track)| {
            track.list_index = index;
            Playable::Track(track)
        })
        .collect()
}

/// All tracks of a playlist, loaded without the Web API.
pub fn playlist_tracks(playlist_id: &str) -> Option<Vec<Playable>> {
    let session = session()?;
    block_on(async {
        let uri = SpotifyUri::from_uri(&format!("spotify:playlist:{playlist_id}")).ok()?;
        let playlist = SpotifyPlaylist::get(&session, &uri)
            .await
            .map_err(|e| warn!("Failed to load playlist {playlist_id}: {e}"))
            .ok()?;
        let uris = playlist
            .contents
            .items
            .iter()
            .map(|item| item.id.clone())
            .filter(|id| id.item_type() == "track")
            .collect();
        Some(fetch_tracks(&session, uris).await)
    })
}

fn collect_track_uris(value: &Value, uris: &mut Vec<String>) {
    match value {
        Value::String(s) if s.starts_with("spotify:track:") => uris.push(s.clone()),
        Value::Array(items) => items.iter().for_each(|v| collect_track_uris(v, uris)),
        Value::Object(map) => map.values().for_each(|v| collect_track_uris(v, uris)),
        _ => {}
    }
}

/// Tracks Spotify would autoplay after `context`, avoiding `recent`.
pub async fn autoplay_tracks(context: String, recent: Vec<String>) -> Vec<Playable> {
    let Some(session) = session() else {
        return Vec::new();
    };

    let request = AutoplayContextRequest {
        context_uri: Some(context.clone()),
        recent_track_uri: recent.clone(),
        ..Default::default()
    };
    let mut uris: Vec<String> = match session.spclient().get_autoplay_context(&request).await {
        Ok(autoplay) => autoplay
            .pages
            .iter()
            .flat_map(|page| page.tracks.iter())
            .map(|track| track.uri().to_string())
            .filter(|uri| uri.starts_with("spotify:track:"))
            .collect(),
        Err(e) => {
            warn!("Autoplay context for {context} failed: {e}");
            Vec::new()
        }
    };

    if uris.is_empty()
        && let Some(seed) = recent.last()
    {
        debug!("Falling back to a radio station for {seed}");
        match session
            .spclient()
            .get_apollo_station("stations", seed, Some(AUTOPLAY_TRACKS), Vec::new(), true)
            .await
        {
            Ok(body) => match serde_json::from_slice::<Value>(&body) {
                Ok(station) => collect_track_uris(&station, &mut uris),
                Err(e) => warn!("Unexpected radio station answer: {e}"),
            },
            Err(e) => warn!("Radio station for {seed} failed: {e}"),
        }
    }

    let mut seen: HashSet<String> = recent.into_iter().collect();
    let uris = uris
        .into_iter()
        .filter(|uri| seen.insert(uri.clone()))
        .take(AUTOPLAY_TRACKS)
        .filter_map(|uri| SpotifyUri::from_uri(&uri).ok())
        .collect();
    fetch_tracks(&session, uris).await
}

async fn pathfinder_query(
    session: &Session,
    operation: &str,
    hash: &str,
    variables: Value,
) -> Result<Value, librespot_core::Error> {
    let body = json!({
        "variables": variables,
        "operationName": operation,
        "extensions": {"persistedQuery": {"version": 1, "sha256Hash": hash}},
    })
    .to_string();
    let token = session.login5().auth_token().await?;
    let mut request = Request::builder()
        .method(Method::POST)
        .uri(PATHFINDER_URL)
        .header(CONTENT_TYPE, "application/json;charset=UTF-8")
        .header(ACCEPT, "application/json")
        .header(
            AUTHORIZATION,
            format!("{} {}", token.token_type, token.access_token),
        )
        .header("app-platform", "Win32_x86_64")
        .header(
            "spotify-app-version",
            librespot_core::version::spotify_semantic_version(),
        )
        .body(Bytes::from(body))?;
    if let Ok(client_token) = session.spclient().client_token().await {
        request
            .headers_mut()
            .insert("client-token", HeaderValue::from_str(&client_token)?);
    }
    let body = session.http_client().request_body(request).await?;
    Ok(serde_json::from_slice(&body)?)
}

/// The shelves of Spotify's Made For You page.
pub fn made_for_you(hash: Option<&str>) -> Result<Vec<Shelf>, String> {
    let session = (0..40)
        .find_map(|_| {
            session().or_else(|| {
                std::thread::sleep(std::time::Duration::from_millis(250));
                None
            })
        })
        .ok_or("Not connected to Spotify yet")?;
    let variables = json!({
        "uri": MADE_FOR_YOU_PAGE,
        "pagePagination": {"offset": 0, "limit": 20},
        "sectionPagination": {"offset": 0, "limit": 20},
        "browseEndUserIntegration": "INTEGRATION_WEB_PLAYER",
        "includeEpisodeContentRatingsV2": false,
    });
    let hash = hash.unwrap_or(BROWSE_PAGE_HASH);
    let answer = block_on(pathfinder_query(&session, "browsePage", hash, variables))
        .map_err(|e| format!("Could not load For You: {e}"))?;

    let page = &answer["data"]["browse"];
    if page["__typename"] != "BrowseSectionContainer" {
        warn!("browsePage answered {}", answer["errors"]);
        return Err("Spotify rejected the query, pathfinder_browse_hash may be outdated".into());
    }
    Ok(shelves(page))
}

fn shelves(page: &Value) -> Vec<Shelf> {
    page["sections"]["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|section| {
            let title = &section["data"]["title"];
            let title = title["transformedLabel"]
                .as_str()
                .or_else(|| title["text"].as_str())?;
            let playlists: Vec<Playlist> = section["sectionItems"]["items"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|item| playlist(&item["content"]["data"]))
                .collect();
            (!playlists.is_empty()).then(|| Shelf {
                title: title.to_string(),
                playlists,
            })
        })
        .collect()
}

fn playlist(data: &Value) -> Option<Playlist> {
    if data["__typename"] != "Playlist" {
        return None;
    }
    let uri = data["uri"].as_str()?;
    if uri == DJ_PLAYLIST {
        return None;
    }
    Some(Playlist {
        id: uri.strip_prefix("spotify:playlist:")?.to_string(),
        name: data["name"].as_str().unwrap_or_default().to_string(),
        owner_id: "spotify".to_string(),
        owner_name: Some("Spotify".to_string()),
        snapshot_id: String::new(),
        num_tracks: 0,
        tracks: None,
        collaborative: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shelves_keep_playlists_only() {
        let page = json!({"sections": {"items": [
            {"data": {"title": {"text": "Your top mixes"}}, "sectionItems": {"items": [
                {"content": {"data": {"__typename": "Playlist",
                    "uri": "spotify:playlist:abc", "name": "Daily Mix 1"}}},
                {"content": {"data": {"__typename": "Playlist", "uri": DJ_PLAYLIST}}},
                {"content": {"data": {"__typename": "Album", "uri": "spotify:album:x"}}}
            ]}},
            {"data": {"title": {"text": "Empty"}}, "sectionItems": {"items": []}}
        ]}});
        let shelves = shelves(&page);
        assert_eq!(shelves.len(), 1);
        assert_eq!(shelves[0].title, "Your top mixes");
        assert_eq!(shelves[0].playlists.len(), 1);
        assert_eq!(shelves[0].playlists[0].id, "abc");
        assert_eq!(shelves[0].playlists[0].owner_id, "spotify");
    }

    #[test]
    fn track_uris_are_found_anywhere() {
        let station = json!({"tracks": [{"uri": "spotify:track:a"}, {"x": ["spotify:track:b"]}],
            "seed": "spotify:artist:c"});
        let mut uris = Vec::new();
        collect_track_uris(&station, &mut uris);
        assert_eq!(uris, ["spotify:track:a", "spotify:track:b"]);
    }
}
