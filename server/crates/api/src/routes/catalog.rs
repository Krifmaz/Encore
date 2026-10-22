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

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/discover/artist/{mbid}/catalog", get(artist_catalog))
        .route("/api/discover/catalog/tracks", post(catalog_tracks))
}

/// Track list of a Deezer / Apple / Bandcamp / Discogs album, each song marked with its library copy
/// (`in_library`, `library`, playable) and any request for it. The body is the album's `ext`, plus
/// `artist` for track lists without artists.
async fn catalog_tracks(State(st): State<AppState>, User(_): User, Json(ext): Json<Value>) -> ApiResult<Json<Value>> {
    let mut tracks = tracklist(&st, &ext).await.map_err(|e| err(502, format!("Could not load the track list: {e}")))?;
    let fallback = s(&ext["artist"]);
    st.blocking(move |st| {
        let keys: Vec<(String, String, String, Option<f64>)> = tracks.iter().map(|t| {
            let artist = Some(s(&t["artist"])).filter(|a| !a.is_empty()).unwrap_or(fallback.clone());
            (s(&t["isrc"]), artist, s(&t["title"]), t["length"].as_f64())
        }).collect();
        // one query per 500 songs for the library copies and one for the requests (encore_library::lookup)
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
        Ok(Json(json!(tracks)))
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

async fn artist_catalog(State(st): State<AppState>, User(viewer): User, Path(mbid): Path<String>) -> ApiResult<Json<Value>> {
    let mut v = artist_catalog_answer(State(st.clone()), User(viewer.clone()), Path(mbid)).await?;
    enrich(&st, Some(&viewer), &mut v).await;
    Ok(Json(v))
}

async fn artist_catalog_answer(State(st): State<AppState>, User(_): User, Path(mbid): Path<String>) -> ApiResult<Value> {
    if !st.cfg.bool("discover_catalog_merge") {
        return Ok(json!({ "enabled": false, "releases": [] }));
    }
    let a = mb::artist(&st, &mbid).await.map_err(|e| err(502, format!("MusicBrainz error: {e}")))?;
    let found = merged(&st, &a).await;
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
    let filter = encore_config::release_types::ReleaseFilter::from_cfg(&st.cfg);
    crate::routes::discover::mark_skipped(&filter, &mut rel);
    // `releases` flat for the tiles, `groups` by canonical kind (with `allowed` per release) for the dialog
    let groups = grouped(&filter, &rel);
    Ok(json!({ "enabled": true, "releases": rel, "groups": groups }))
}
