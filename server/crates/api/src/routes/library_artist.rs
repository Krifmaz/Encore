//! GET /api/library/artists/{name}/liked and GET /api/library/artists/{name}/tracks (0.31, client-shapes
//! §3.2 and §3.3): an artist page's "Liked from this artist" and the songs of its "In your library", and the
//! car's artist node.
//!
//! - The artist is matched as `/api/library/artist-tracks?name=` matches it (every spelling, alias and
//!   collaboration, every credited artist of a song; see encore_library::artists), plus the songs credited to
//!   `mbid` (a MusicBrainz artist id) when one is given, so a catalogue page whose MusicBrainz name is spelled
//!   differently from the library still finds them.
//! - `{name}` is one path segment; `?name=` (the same name) wins when present, because some proxies refuse or
//!   decode `%2F` in a path (the phone puts `_` in the path for "AC/DC" and the real name in the query).
//! - An unknown artist, or one with nothing liked, is `{"total": 0, ...}`, not 404. Rows go through the usual
//!   enrichment (cover_v, artists, translations).
use super::rows_by_id;
use crate::auth::User;
use crate::enrich::enrich_now;
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::Json;
use encore_core::error::ApiResult;
use encore_core::ids::is_mbid;
use encore_db::{p, RowExt};
use encore_library::lists::TRACK_COLS;
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize, Default)]
#[serde(default)]
pub(super) struct ArtistSongsQ {
    name: Option<String>,
    mbid: String,
    offset: i64,
    limit: Option<i64>,
}

impl ArtistSongsQ {
    /// The artist's name: `?name=` when given, else the path segment.
    fn artist(&self, path: &str) -> String {
        match self.name.as_deref().map(str::trim) {
            Some(n) if !n.is_empty() => n.to_string(),
            _ => path.trim().to_string(),
        }
    }
}

/// SQL condition on `tracks` (and its values) for the artist's songs; None when there is neither a name nor a
/// MusicBrainz id to match.
fn artist_filter(st: &AppState, name: &str, mbid: &str) -> Option<(String, Vec<Value>)> {
    let mbid = mbid.trim().to_ascii_lowercase();
    let by_id = is_mbid(&mbid);
    if name.is_empty() && !by_id {
        return None;
    }
    let (mut cond, mut params) = if name.is_empty() { ("0".to_string(), vec![]) } else { st.library.artists.track_filter(&st.db, name) };
    if by_id {
        cond = format!("({cond} OR mb_albumartistid=? OR mb_artistid=?)");
        params.extend(p![mbid.clone(), mbid]);
    }
    Some((cond, params))
}

/// `{total, tracks}`: the caller's liked songs by this artist that are in the library, newest like first.
/// `limit` 1–500 (default 200); `total` counts them all.
pub(super) async fn liked(State(st): State<AppState>, User(u): User, Path(path): Path<String>, Query(q): Query<ArtistSongsQ>) -> ApiResult<Json<Value>> {
    let name = q.artist(&path);
    let limit = q.limit.unwrap_or(200).clamp(1, 500) as usize;
    st.blocking(move |st| {
        let Some((cond, mut params)) = artist_filter(st, &name, &q.mbid) else {
            return Ok(Json(json!({ "total": 0, "tracks": [] })));
        };
        // the artist's songs (indexed by name and id), each looked up in the liked list by playlist_items(track_id);
        // a GET does not make the Liked Songs playlist: none yet means nothing liked
        params.push(json!(u.i("id")));
        let ids: Vec<i64> = st.db.query(&format!(
            "SELECT track_id, MAX(added_at) liked, MIN(position) pos FROM playlist_items
             WHERE track_id IN (SELECT id FROM tracks WHERE {cond})
               AND playlist_id IN (SELECT id FROM playlists WHERE user_id=? AND kind='liked')
             GROUP BY track_id ORDER BY liked DESC, pos, track_id"), params)?
            .iter().map(|r| r.i("track_id")).collect();
        let total = ids.len();
        let page: Vec<i64> = ids.into_iter().take(limit).collect();
        let mut v = json!({ "total": total, "tracks": rows_by_id(&st.db, TRACK_COLS, &page)? });
        enrich_now(st, Some(&u), &mut v);
        Ok(Json(v))
    }).await
}

/// `{total, offset, tracks}`: a page of the artist's songs in the library, by album year (oldest first, no
/// year last), then album, disc and track. `offset` (default 0), `limit` 1–500 (default 50); `total` counts
/// every song of the artist.
pub(super) async fn songs(State(st): State<AppState>, User(u): User, Path(path): Path<String>, Query(q): Query<ArtistSongsQ>) -> ApiResult<Json<Value>> {
    let name = q.artist(&path);
    let (offset, limit) = (q.offset.max(0), q.limit.unwrap_or(50).clamp(1, 500));
    st.blocking(move |st| {
        let Some((cond, params)) = artist_filter(st, &name, &q.mbid) else {
            return Ok(Json(json!({ "total": 0, "offset": offset, "tracks": [] })));
        };
        let total = st.db.one(&format!("SELECT COUNT(*) n FROM tracks WHERE {cond}"), params.clone())?.map(|r| r.i("n")).unwrap_or(0);
        let mut ps = params;
        ps.extend(p![limit, offset]);
        let rows = st.db.query(&format!(
            "SELECT {TRACK_COLS} FROM tracks WHERE {cond}
             ORDER BY CASE WHEN year IS NULL OR trim(year) IN ('', '0') THEN 1 ELSE 0 END, year, lower(album), album_id, disc, track, id
             LIMIT ? OFFSET ?"), ps)?;
        let mut v = json!({ "total": total, "offset": offset, "tracks": rows });
        enrich_now(st, Some(&u), &mut v);
        Ok(Json(v))
    }).await
}

/// `{total, offset, tracks}`: the artist's songs whose album is a One-off (a song or two of a compilation, a remix
/// album, a soundtrack …: encore_library::album_types), newest first, each with its `release_label` ("Compilation",
/// "Various artists", "Album" …). `offset` (default 0), `limit` 1–500 (default 50).
pub(super) async fn one_offs(State(st): State<AppState>, User(u): User, Path(path): Path<String>, Query(q): Query<ArtistSongsQ>) -> ApiResult<Json<Value>> {
    let name = q.artist(&path);
    let (offset, limit) = (q.offset.max(0) as usize, q.limit.unwrap_or(50).clamp(1, 500) as usize);
    st.blocking(move |st| {
        let Some((cond, params)) = artist_filter(st, &name, &q.mbid) else {
            return Ok(Json(json!({ "total": 0, "offset": offset, "tracks": [] })));
        };
        let types = st.library.types.get(&st.db);
        let found: Vec<(i64, String)> = st.db.query(&format!("SELECT id, album_id FROM tracks WHERE {cond} ORDER BY added_at DESC, id DESC"), params)?
            .iter().map(|r| (r.i("id"), r.s("album_id"))).filter(|(_, a)| types.get(a).is_some_and(|c| c.format == "one_off")).collect();
        let page: Vec<i64> = found.iter().skip(offset).take(limit).map(|(id, _)| *id).collect();
        let mut tracks = rows_by_id(&st.db, TRACK_COLS, &page)?;
        for t in tracks.iter_mut() {
            let label = types.get(&t.s("album_id")).map(|c| c.release_label.clone()).unwrap_or_default();
            t.insert("release_label".into(), json!(label));
        }
        let mut v = json!({ "total": found.len(), "offset": offset, "tracks": tracks });
        enrich_now(st, Some(&u), &mut v);
        Ok(Json(v))
    }).await
}
