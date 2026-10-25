//! /api/discover/…/catalog: an artist's releases that MusicBrainz is missing (Deezer, Apple Music, Bandcamp,
//! Discogs) and the track lists of such releases, marked with the library and the requests. The merge
//! itself lives in encore_library::catalog.
use crate::enrich::enrich;
use crate::auth::User;
use encore_core::error::{err, ApiResult};
use crate::state::AppState;
use encore_http::mb;
use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use encore_core::json::s;
use encore_library::catalog::*;
use encore_db::{p, RowExt};
use serde_json::{json, Value};

/// The Discover catalog routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/discover/artist/{mbid}/catalog", get(artist_catalog))
        .route("/api/discover/catalog/tracks", post(catalog_tracks))
        .route("/api/discover/album-rest/request", post(request_rest))
}

/// Track list of a Deezer / Apple / Bandcamp / Discogs album, each song marked with its library copy
/// (`in_library`, `library`, playable) and any request for it. The body is the album's `ext`, plus
/// `artist` for track lists without artists.
async fn catalog_tracks(State(st): State<AppState>, User(_): User, Json(ext): Json<Value>) -> ApiResult<Json<Value>> {
    let mut tracks = tracklist(&st, &ext).await.map_err(|e| err(502, format!("Could not load the track list: {e}")))?;
    let fallback = s(&ext["artist"]);
    st.blocking(move |st| {
        annotate_tracks(st, &mut tracks, &fallback);
        Ok(Json(json!(tracks)))
    }).await
}

/// Each song of a track list from another catalogue (or typed in by hand): its library copy (`in_library`,
/// `library`, playable) and any request for it (`request_status`, `request_label`). A song without an artist
/// gets `fallback` (the album's artist). One query per 500 songs for the library copies and one for the
/// requests (encore_library::lookup).
pub(crate) fn annotate_tracks(st: &AppState, tracks: &mut [Value], fallback: &str) {
    let keys: Vec<(String, String, String, Option<f64>)> = tracks.iter().map(|t| {
        let artist = Some(s(&t["artist"])).filter(|a| !a.is_empty()).unwrap_or(fallback.to_string());
        (s(&t["isrc"]), artist, s(&t["title"]), t["length"].as_f64())
    }).collect();
    let libs = encore_library::lookup::library_tracks_for(&st.db, &keys);
    let pairs: Vec<(String, String)> = keys.iter().map(|k| (k.1.clone(), k.2.clone())).collect();
    let reqs = encore_library::lookup::Requests::read(&st.db, "track", &pairs, &[]);
    for ((t, lib), (_, artist, title, _)) in tracks.iter_mut().zip(libs).zip(keys) {
        t["in_library"] = json!(lib.is_some());
        t["library"] = json!(lib);
        if s(&t["artist"]).is_empty() {
            t["artist"] = json!(artist);
        }
        let rq = reqs.by_name(&artist, &title, true, false).map(|(status, message)| (status.clone(), encore_requests::events::label(&status, &message).to_string()));
        t["request_status"] = rq.as_ref().map(|r| json!(r.0)).unwrap_or(Value::Null);
        t["request_label"] = rq.map(|r| json!(r.1)).unwrap_or(Value::Null);
    }
}

/// `POST /api/discover/album-rest/request` body: the album and the songs to fill it in with.
#[derive(serde::Deserialize, Default)]
#[serde(default)]
struct RestBody {
    /// the album's artist (each song's own artist, when it has one, is used for that song)
    artist: String,
    /// the album's title
    album: String,
    /// the album's year, for the requests
    year: Option<String>,
    /// the album's cover, for the requests
    cover: Option<String>,
    /// the album's songs (`title`, and `artist`, `length`, `isrc` when known): a catalogue's track list
    tracks: Vec<Value>,
    /// the album's songs typed in by hand, one per line (numbers and lengths are read off,
    /// [`encore_library::rest_of_album::typed_tracklist`]); used when `tracks` is empty
    text: String,
    /// lines of `text` may be "Artist - Title" (a compilation, guests)
    artist_lines: bool,
}

/// POST /api/discover/album-rest/request: "Get the rest of the album" for an album MusicBrainz doesn't list –
/// a request for each song of the track list (another catalogue's, or typed in by hand) that isn't in the
/// library yet and isn't already requested. Each is a song request ("only this song"), found by artist and
/// title at the download sources.
///
/// → `{requested, already_requested, in_library, status, ids}`: how many were asked for, how many were
/// already asked for, how many are in the library, and the status of the new requests ("queued" or
/// "pending" approval; null when none was made).
async fn request_rest(State(st): State<AppState>, User(u): User, Json(b): Json<RestBody>) -> ApiResult<Json<Value>> {
    use encore_library::rest_of_album::{typed_tracklist, MAX_TRACKS};
    use encore_requests::create::{create_request, quota, NewRequest};
    let (artist, album) = (b.artist.trim().to_string(), b.album.trim().to_string());
    if artist.is_empty() || album.is_empty() {
        return Err(err(400, "Artist and album title are required"));
    }
    let mut tracks: Vec<Value> = if b.tracks.is_empty() { typed_tracklist(&b.text, b.artist_lines) } else { b.tracks.into_iter().take(MAX_TRACKS).collect() };
    tracks.retain(|t| !s(&t["title"]).trim().is_empty());
    if tracks.is_empty() {
        return Err(err(400, "Add the album's songs, one per line"));
    }
    st.blocking(move |st| {
        annotate_tracks(st, &mut tracks, &artist);
        let have = tracks.iter().filter(|t| t["in_library"] == true).count();
        let missing: Vec<&Value> = tracks.iter().filter(|t| t["in_library"] != true).collect();
        if !missing.is_empty() {
            // (the weekly limit counts the album once, as a link's songs are)
            quota(st, &u, 1)?;
        }
        let nz = |v: &Value| v.as_str().map(str::trim).filter(|x| !x.is_empty()).map(String::from);
        let (mut made, mut already) = (vec![], 0);
        for t in missing {
            // a song already requested (its badge shows it) is counted, not asked for again
            if !t["request_status"].is_null() {
                already += 1;
                continue;
            }
            let req = NewRequest {
                kind: "track".into(), artist: nz(&t["artist"]).or(Some(artist.clone())), title: nz(&t["title"]), album: Some(album.clone()),
                year: b.year.clone().filter(|y| !y.is_empty()), cover_url: b.cover.clone().filter(|c| !c.is_empty()),
                length: t["length"].as_f64().filter(|l| *l > 0.0), isrc: nz(&t["isrc"]), album_mode: Some("song".into()), ..Default::default()
            };
            match create_request(st, &u, &req, None, None) {
                Ok(id) => made.push(id),
                Err(e) if e.status.as_u16() == 409 => already += 1,
                Err(e) => return Err(e),
            }
        }
        let status = match made.first() { Some(id) => crate::routes::requests::get_request(st, *id)?["status"].clone(), None => Value::Null };
        Ok(Json(json!({ "requested": made.len(), "already_requested": already, "in_library": have, "status": status, "ids": made })))
    }).await
}

/// Albums of other catalogues: their library album (by title and artist) and any request for them (album
/// requests without a MusicBrainz id). One query per 500 albums for each (encore_library::lookup).
pub(crate) fn annotate(st: &AppState, items: &mut [Value]) {
    use encore_library::lookup;
    let keys: Vec<(String, String, String)> = items.iter().map(|m| (String::new(), s(&m["artist"]), s(&m["title"]))).collect();
    let libs = lookup::album_ids_for(&st.db, &keys);
    let pairs: Vec<(String, String)> = keys.iter().map(|k| (k.1.clone(), k.2.clone())).collect();
    let reqs = lookup::Requests::read(&st.db, "album", &pairs, &[]);
    for ((m, lib), (_, artist, title)) in items.iter_mut().zip(libs).zip(keys) {
        m["in_library"] = json!(lib.is_some());
        m["library_album_id"] = lib.map(|a| json!(a)).unwrap_or(Value::Null);
        let rq = reqs.by_name(&artist, &title, false, true);
        m["request_status"] = rq.as_ref().map(|r| json!(r.0)).unwrap_or(Value::Null);
        m["request_label"] = rq.map(|(status, message)| json!(encore_requests::events::label(&status, &message))).unwrap_or(Value::Null);
    }
}

/// GET /api/discover/artist/{mbid}/catalog: the artist's releases on other services that MusicBrainz lacks, enriched.
async fn artist_catalog(State(st): State<AppState>, User(viewer): User, Path(mbid): Path<String>) -> ApiResult<Json<Value>> {
    let mut v = artist_catalog_answer(State(st.clone()), User(viewer.clone()), Path(mbid)).await?;
    enrich(&st, Some(&viewer), &mut v).await;
    Ok(Json(v))
}

/// The catalog answer before enrichment: the releases found elsewhere, marked with library copies and requests,
/// without those already in MusicBrainz.
async fn artist_catalog_answer(State(st): State<AppState>, User(_): User, Path(mbid): Path<String>) -> ApiResult<Value> {
    if !st.cfg.bool("discover_catalog_merge") {
        return Ok(json!({ "enabled": false, "releases": [] }));
    }
    let filter = encore_config::release_types::ReleaseFilter::for_artist(&st.cfg, &mbid);
    let a = mb::artist(&st, &mbid).await.map_err(crate::routes::discover::mb_err)?;
    // a dozen MusicBrainz look-ups (the artist's barcodes, ISRCs) for a section shown under the page: after any
    // page someone opens meanwhile (the album they tap next doesn't wait for them)
    let found = mb::with_priority(mb::Priority::Section, merged(&st, &a)).await;
    let rgs: std::collections::HashSet<String> = a["release_groups"].as_array().into_iter().flatten().map(|g| s(&g["id"])).collect();
    // the library and the requests for every release: on the blocking pool
    let mut rel = st.blocking(move |st| {
        let mut rel = found;
        annotate(st, &mut rel);
        // a release whose library album is one of the artist's MusicBrainz release groups (its copy from this
        // service was merged into the MusicBrainz album, encore_library::merge) is in MusicBrainz: not shown twice
        let in_mb = |m: &Value| m["library_album_id"].as_str().is_some_and(|id| {
            st.db.query("SELECT DISTINCT mb_releasegroupid rg FROM tracks WHERE album_id=? AND mb_releasegroupid IS NOT NULL", p![id])
                .unwrap_or_default().iter().any(|r| rgs.contains(&r.s("rg")))
        });
        rel.retain(|m| !in_mb(m));
        Ok(rel)
    }).await?;
    crate::routes::discover::mark_skipped(&filter, &mut rel);
    // `releases` flat for the tiles, `groups` by canonical kind (with `allowed` per release) for the dialog
    let groups = grouped(&filter, &rel);
    Ok(json!({ "enabled": true, "releases": rel, "groups": groups }))
}
