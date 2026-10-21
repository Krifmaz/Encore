//! POST /api/library/match-local: which songs from a listener's own folders (scanned by the desktop, phone or
//! web app, on that device) the library already has. Only the tags come here, never the files: the apps show
//! a matched song once, as the library's song played from the device's file, and keep the rest under *On
//! this device*.
//!
//! Body `{songs: [{key, title, artist, duration?, isrc?, mbid?}]}` (at most [`MAX_SONGS`] per call; `key` is
//! the app's own name for the file, sent back as it came). Answer `{matches: {key: track row}}` with the
//! library's row (the `/tracks` columns, enriched like it) for each song found; songs not found are left
//! out. Matching is the library's own (`matcher::LibraryIndex::find`): MusicBrainz recording, then ISRC,
//! then the version-aware title, artist and length match, so a remix never stands in for the original.
use crate::auth::User;
use crate::enrich::enrich_now;
use crate::state::AppState;
use axum::extract::State;
use axum::Json;
use encore_core::error::{err, ApiResult};
use encore_db::RowExt;
use encore_library::lists::TRACK_COLS;
use encore_library::matcher::describe;
use serde::Deserialize;
use serde_json::{json, Map, Value};

/// Most songs one call matches (the apps send a big folder in parts).
pub const MAX_SONGS: usize = 2000;

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct Body {
    songs: Vec<Song>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Song {
    key: String,
    title: String,
    artist: String,
    duration: Option<f64>,
    isrc: Option<String>,
    mbid: Option<String>,
}

pub async fn match_local(State(st): State<AppState>, User(u): User, Json(b): Json<Body>) -> ApiResult<Json<Value>> {
    if b.songs.len() > MAX_SONGS {
        return Err(err(400, format!("At most {MAX_SONGS} songs at a time")));
    }
    st.blocking(move |st| {
        let ix = st.library.songs.get(&st.db);
        let mut found: Vec<(String, i64)> = Vec::new();
        for s in b.songs.iter().filter(|s| !s.key.is_empty() && !s.title.trim().is_empty()) {
            let want = describe(&s.title, &s.artist, s.duration, s.isrc.as_deref(), s.mbid.as_deref().map(str::trim));
            if let Some((row, _)) = ix.find(&want) {
                found.push((s.key.clone(), row.i("id")));
            }
        }
        let mut ids: Vec<i64> = found.iter().map(|(_, id)| *id).collect();
        ids.sort_unstable();
        ids.dedup();
        let mut rows = json!(super::rows_by_id(&st.db, TRACK_COLS, &ids)?);
        enrich_now(st, Some(&u), &mut rows);
        let by_id: std::collections::HashMap<i64, Value> =
            rows.as_array().into_iter().flatten().filter_map(|r| Some((r["id"].as_i64()?, r.clone()))).collect();
        let mut matches = Map::new();
        for (key, id) in found {
            if let Some(r) = by_id.get(&id) {
                matches.insert(key, r.clone());
            }
        }
        Ok(Json(json!({ "matches": matches })))
    }).await
}

#[cfg(test)]
#[path = "local_match_tests.rs"]
mod tests;
