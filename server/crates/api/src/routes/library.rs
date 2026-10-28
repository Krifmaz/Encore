//! /api/library: browse, play, download, share, likes, storage, and removing things.
use crate::auth::{self, Admin, MaybeUser, User};
use encore_db::{now, Conn, RowExt};
use crate::enrich::{enrich, enrich_now};
use encore_core::error::{err, ApiResult};
use encore_library::{self as lib, album_types, delete_tracks, prune_empty};
use encore_db::p;
use crate::state::AppState;
use axum::extract::{Path, Query, Request, State};
use axum::http::{header, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path as FsPath;
use std::sync::Arc;
// how a stream is served (the one decision, codec negotiation, files with Range support) is encore_playback::stream
use encore_playback::stream::{decoders, plan, serve_file, serve_stream, Listener, StreamQ};

/// The library routes: lists, songs, albums, streams, covers, likes and share links.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/library/stats", get(stats))
        .route("/api/library/scan", post(scan))
        .route("/api/library/storage", get(storage))
        .route("/api/library/albums", get(albums))
        .route("/api/library/album-types", get(album_type_counts))
        .route("/api/library/albums/trash", post(trash_albums))
        .route("/api/library/albums/{id}/type", axum::routing::put(set_album_type))
        .route("/api/library/artists/{name}/one-offs", get(library_artist::one_offs))
        .route("/api/library/tracks/{id}/added", get(track_added))
        .route("/api/library/artists", get(artists).delete(delete_artist))
        .route("/api/library/tracks", get(tracks))
        .route("/api/library/artist-tracks", get(artist_tracks))
        .route("/api/library/artists/{name}/liked", get(library_artist::liked))
        .route("/api/library/artists/{name}/tracks", get(library_artist::songs))
        .route("/api/library/search", get(crate::search::library_search_route))
        .route("/api/library/index", get(library_index::index))
        .route("/api/library/recent", get(recent))
        .route("/api/library/shuffle", get(shuffle_get).post(shuffle_post))
        .route("/api/library/plays", post(plays))
        .route("/api/library/now-playing", post(now_playing))
        .route("/api/library/tracks/{id}", axum::routing::delete(delete_track))
        .route("/api/library/tracks/{id}/file", get(track_file))
        .route("/api/library/tracks/{id}/stream", get(stream))
        .route("/api/library/tracks/{id}/info", get(track_info))
        .route("/api/library/tracks/{id}", get(track))
        .route("/api/library/plays/recent", get(recent_plays))
        .route("/api/library/tracks/{id}/lyrics", get(lyrics))
        .route("/api/library/tracks/{id}/lyrics/shift", axum::routing::put(lyrics_shift))
        .route("/api/library/tracks/{id}/edges", get(track_edges::edges))
        .route("/api/library/tracks/{id}/share", post(share))
        .route("/api/library/albums/{id}/share", post(share_album))
        .route("/api/library/artists/share", post(share_artist))
        .route("/api/library/albums/{id}", get(album).delete(delete_album))
        .route("/api/library/albums/{id}/cover", get(cover))
        .route("/api/library/albums/{id}/download", get(album_zip))
        .route("/api/library/albums/{id}/upgrade", post(upgrade_album))
        .route("/api/library/verify-manual", post(verify_manual))
        .route("/api/library/mismatches", get(mismatches))
        .route("/api/likes", get(likes))
        .route("/api/likes/{id}", post(like).delete(unlike))
        .route("/api/share/{token}", get(shared))
        .route("/api/share/{token}/stream", get(shared_stream))
        .route("/api/share/{token}/cover", get(shared_cover))
        .route("/api/share/{token}/video.mp4", get(shared_video))
        .route("/api/share/{token}/copy", post(shared_copy))
        .route("/s/{token}", get(share_page))
}

/// GET /api/library/index: the side scrubber's sections.
#[path = "library_index.rs"]
mod library_index;

/// GET /api/library/artists/{name}/liked and /tracks: an artist page's liked songs and its songs in the library.
#[path = "library_artist.rs"]
mod library_artist;

/// GET /api/library/tracks/{id}/edges: where a song's sound starts and stops (Smart overlap transitions).
#[path = "track_edges.rs"]
mod track_edges;

#[cfg(test)]
#[path = "library_types_tests.rs"]
mod library_types_tests;

// ---- cached list answers ---------------------------------------------------------------------------

/// How long a cached list answer is kept at most (a safety net: any write to the songs drops it anyway).
const LIST_TTL: std::time::Duration = std::time::Duration::from_secs(600);

/// How long a page waits for a list that is being made again (after a delete, an import or a scan) before it
/// is given the list as it was: on a slow server a big library's lists take seconds each, and the apps give
/// up after 30.
const LIST_WAIT: std::time::Duration = std::time::Duration::from_secs(3);

/// A library list's whole answer (albums, songs, artists, the side bar's sections) for as long as the
/// songs are unchanged: `Db::library_version` moves with every write to them. With 57k songs these are
/// GROUP BYs over every row – once per change, not per page. After a change the list is made again on the
/// blocking pool; a page that would wait longer than `LIST_WAIT` for it gets the answer as it was (once,
/// while it is made), so a delete never makes every page load slow at once.
pub(super) fn cached_list<F>(st: &AppState, key: &str, make: F) -> ApiResult<Arc<Value>>
where
    F: FnOnce() -> ApiResult<Value> + Send + 'static,
{
    let version = st.db.library_version();
    if let Some(v) = cached_at(st, key, version) {
        return Ok(v);
    }
    if let (Some(old), Ok(rt)) = (st.web.lists.get(key), tokio::runtime::Handle::try_current()) {
        let Some(building) = start_build(st, key) else { return Ok(old) };
        let (tx, rx) = std::sync::mpsc::channel();
        let (st2, k) = (st.clone(), key.to_string());
        rt.spawn_blocking(move || {
            let _building = building;
            let r = make().map(Arc::new);
            if let Ok(v) = &r {
                keep_list(&st2, k, version, v.clone());
            }
            let _ = tx.send(r);
        });
        return rx.recv_timeout(LIST_WAIT).unwrap_or(Ok(old));
    }
    let value = Arc::new(make()?);
    keep_list(st, key.to_string(), version, value.clone());
    Ok(value)
}

/// [`cached_list`] for a list that is only SQL: built on a read connection off the async runtime
/// (`Db::read`).
pub(super) async fn cached_read<F>(st: &AppState, key: String, make: F) -> ApiResult<Arc<Value>>
where
    F: FnOnce(&Conn) -> rusqlite::Result<Value> + Send + 'static,
{
    let version = st.db.library_version();
    if let Some(v) = cached_at(st, &key, version) {
        return Ok(v);
    }
    if let Some(old) = st.web.lists.get(&key) {
        let Some(building) = start_build(st, &key) else { return Ok(old) };
        let st2 = st.clone();
        let task = tokio::spawn(async move {
            let _building = building;
            let value = Arc::new(st2.db.read(make).await?);
            keep_list(&st2, key, version, value.clone());
            ApiResult::Ok(value)
        });
        return match tokio::time::timeout(LIST_WAIT, task).await {
            Ok(Ok(r)) => r,
            _ => Ok(old),
        };
    }
    let value = Arc::new(st.db.read(make).await?);
    keep_list(st, key, version, value.clone());
    Ok(value)
}

/// A list being made again in the background; the mark goes when it is dropped (also when the build panicked).
struct Building(AppState, String);

impl Drop for Building {
    fn drop(&mut self) {
        self.0.web.list_builds.lock().unwrap_or_else(|e| e.into_inner()).remove(&self.1);
    }
}

/// Marks `key` as being made again, or None when that is running already.
fn start_build(st: &AppState, key: &str) -> Option<Building> {
    let mut b = st.web.list_builds.lock().unwrap_or_else(|e| e.into_inner());
    b.insert(key.to_string()).then(|| Building(st.clone(), key.to_string()))
}

/// The cached answer for `key` when it was made at this library version.
fn cached_at(st: &AppState, key: &str, version: u64) -> Option<Arc<Value>> {
    st.web.lists.get_if(key, |v| v == version)
}

/// Keeps a list answer made at `version` (read before its rows: a write meanwhile makes the next call
/// build it again), in place of the key's older one. Answers of an older state of the library stay until
/// they are made again (they are what a page gets while that runs); the cache's byte bound
/// (`state::LIST_CACHE_BYTES`) pushes out the least recently used ones.
fn keep_list(st: &AppState, k: String, version: u64, value: Arc<Value>) {
    let bytes = k.len() + encore_core::cache::json_bytes(&value);
    st.web.lists.insert(k, value, bytes, Some(LIST_TTL), version);
}

#[cfg(test)]
mod list_wait_tests {
    use super::*;
    use crate::test_support::TempState;
    use std::time::{Duration, Instant};

    fn songs_written(st: &AppState) {
        st.db.execute("UPDATE tracks SET bitrate=bitrate WHERE id=0", p![]).unwrap();
    }

    async fn list(st: &AppState, make: impl FnOnce() -> ApiResult<Value> + Send + 'static) -> (Value, Duration) {
        let st = st.clone();
        let t = Instant::now();
        let v = tokio::task::spawn_blocking(move || cached_list(&st, "k", make)).await.unwrap().unwrap();
        ((*v).clone(), t.elapsed())
    }

    /// After a write to the songs a list is made again: a quick build is waited for; a slow one gives the
    /// list as it was after `LIST_WAIT` (and at once to pages that come while it runs), and the pages after
    /// it the new list.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_slow_rebuild_gives_the_old_list_meanwhile() {
        let tmp = TempState::new("list-wait");
        let st: AppState = (*tmp).clone();
        assert_eq!(list(&st, || Ok(json!(1))).await.0, json!(1));
        songs_written(&st);
        assert_eq!(list(&st, || Ok(json!(2))).await.0, json!(2), "a quick build is waited for");

        songs_written(&st);
        let slow = list(&st, || {
            std::thread::sleep(LIST_WAIT + Duration::from_millis(1500));
            Ok(json!(3))
        });
        let meanwhile = async {
            tokio::time::sleep(Duration::from_millis(300)).await;
            list(&st, || panic!("one build at a time")).await
        };
        let ((v, waited), (v2, waited2)) = tokio::join!(slow, meanwhile);
        assert_eq!(v, json!(2), "the list as it was");
        assert!(waited >= LIST_WAIT && waited < LIST_WAIT + Duration::from_millis(1000), "{waited:?}");
        assert_eq!(v2, json!(2));
        assert!(waited2 < Duration::from_millis(500), "a page during the build doesn't wait: {waited2:?}");

        tokio::time::sleep(Duration::from_millis(2000)).await;
        assert_eq!(list(&st, || panic!("made already")).await.0, json!(3), "the new list once it is made");
    }
}

/// Items `offset..offset+limit` of a cached JSON array.
fn page(all: &Value, offset: i64, limit: usize) -> Vec<Value> {
    all.as_array().map(|a| a.iter().skip(offset.max(0) as usize).take(limit).cloned().collect()).unwrap_or_default()
}

use encore_library::lists::{order, ALBUM_SORTS, ARTISTS_MAX, TRACK_COLS, TRACK_SORTS};

/// Settings → Library's numbers (polled every few seconds while the page is open): the totals are four full
/// scans, made on a read connection off the async runtime once per state of the library.
async fn stats(State(st): State<AppState>, User(_): User) -> Json<Value> {
    let mut s = match cached_read(&st, "stats".into(), |c| Ok(lib::stats(c))).await {
        Ok(v) => (*v).clone(),
        Err(_) => st.db.read(|c| Ok::<_, rusqlite::Error>(lib::stats(c))).await.unwrap_or_default(),
    };
    let scan = st.library.scan.lock().unwrap().clone();
    s["scan"] = json!(scan);
    s["acoustid"] = json!(!st.cfg.str("acoustid_api_key").is_empty());
    for k in ["bulk", "verify", "artwork", "organize"] {
        s[k] = st.jobs.get(k);
    }
    Json(s)
}

/// `POST /api/library/scan` query.
#[derive(Deserialize)]
struct ScanQ {
    /// read every file again (else only new and changed ones)
    #[serde(default)]
    full: bool,
}

/// POST /api/library/scan: scan the library in the background (admins).
async fn scan(State(st): State<AppState>, Admin(_): Admin, Query(q): Query<ScanQ>) -> ApiResult<Json<Value>> {
    if st.library.scan.lock().unwrap().scanning {
        return Err(err(409, "A scan is already running"));
    }
    // the library folder, the extra library folders and the Surround folder, in one scanning session
    let st2 = st.clone();
    tokio::spawn(async move { st2.library.scan_library(st2.db.clone(), &st2.cfg, q.full).await });
    Ok(Json(json!({ "ok": true })))
}

/// The bytes under a folder (0 when it isn't set).
fn dir_size(path: &str) -> u64 {
    if path.is_empty() {
        return 0;
    }
    walkdir::WalkDir::new(path).into_iter().filter_map(|e| e.ok()).filter(|e| e.file_type().is_file()).filter_map(|e| e.metadata().ok()).map(|m| m.len()).sum()
}

/// `df` for the file system a folder is on: `{total, used, free}` in bytes.
fn disk_usage(path: &str) -> Option<Value> {
    let out = std::process::Command::new("df").args(["-Pk", path]).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let cols: Vec<&str> = text.lines().nth(1)?.split_whitespace().collect();
    let k = |i: usize| cols.get(i).and_then(|v| v.parse::<u64>().ok()).map(|v| v * 1024);
    Some(json!({ "total": k(1)?, "used": k(2)?, "free": k(3)? }))
}

/// GET /api/library/storage: the library's and the downloads' sizes and the disks they are on.
async fn storage(State(st): State<AppState>, User(u): User) -> ApiResult<Json<Value>> {
    if !auth::can(&st, &u, "storage") {
        return Err(err(403, "Storage isn't shared with your account"));
    }
    let libp = st.cfg.str("library_path");
    let (slsk, staging) = (st.cfg.str("slskd_download_path"), st.cfg.str("staging_path"));
    // `df` and the walks of the download folders: the last answer for these folders, worked out again in the
    // background (while it is shown) once it is a minute old or the library changed (a song deleted or
    // imported moved the disk numbers); only the first look at these folders waits
    let key = format!("{libp}\u{1f}{slsk}\u{1f}{staging}");
    let (disk, s1, s2) = st.web.storage.get_blocking_at(&key, st.db.library_version(), move || (disk_usage(&libp), dir_size(&slsk), dir_size(&staging))).await;
    // the library's own numbers: four GROUP BYs over every song, made on a read connection once per state of
    // the library
    let lib = cached_read(&st, "storage".into(), |c| {
        let q = |sql: &str| c.query(sql, p![]).unwrap_or_default();
        Ok(json!({
            "library": c.one("SELECT COALESCE(SUM(size),0) size, COUNT(*) tracks, COUNT(DISTINCT album_id) albums FROM tracks", p![])?,
            "formats": q("SELECT upper(format) format, COUNT(*) tracks, COALESCE(SUM(size),0) size FROM tracks GROUP BY format ORDER BY size DESC"),
            "artists": q("SELECT MAX(albumartist) name, COALESCE(SUM(size),0) size, COUNT(*) tracks FROM tracks GROUP BY lower(albumartist) ORDER BY size DESC LIMIT 15"),
            "albums": q("SELECT album_id id, MAX(album) album, MAX(albumartist) albumartist, COALESCE(SUM(size),0) size, COUNT(*) tracks FROM tracks GROUP BY album_id ORDER BY size DESC LIMIT 12"),
        }))
    }).await?;
    let mut v = json!({
        "disk": disk,
        "library": lib["library"],
        "formats": lib["formats"],
        "artists": lib["artists"],
        "albums": lib["albums"],
        "downloads": [{ "name": "soulseek", "size": s1 }, { "name": "staging", "size": s2 }],
    });
    enrich(&st, Some(&u), &mut v).await;
    Ok(Json(v))
}

/// `GET /api/library/albums` query.
#[derive(Deserialize, Default)]
#[serde(default)]
struct AlbumQ {
    /// a search text
    q: String,
    /// the sort key
    sort: String,
    /// the albums filter
    filter: String,
    /// only this artist's albums
    artist: String,
    /// the page size
    limit: Option<i64>,
    /// where the page starts
    offset: i64,
    /// the sort direction
    dir: String,
    /// `albums` (format Album), `singles` (Singles & EPs), or every album but the One-offs
    kind: String,
    /// the Type filter: one key or a comma list (encore_library::album_types::TYPES, or `ep` / `single`); wins over `kind`
    #[serde(rename = "type")]
    types: String,
    /// `1`: `{total, ids}` of the whole filtered list (Select all), at most `IDS_MAX`
    ids_only: String,
}

/// Most album ids one `ids_only` answer gives (Select all).
const IDS_MAX: usize = 5000;

/// The albums list (the web's Albums tab, an artist's albums, the phone's library). The artist index (a
/// search's ranks, the artist filter) and the queries run on the blocking pool.
async fn albums(State(st): State<AppState>, User(u): User, Query(a): Query<AlbumQ>) -> ApiResult<Json<Value>> {
    st.blocking(move |st| {
        let mut v = albums_answer(st, &a)?;
        enrich_now(st, Some(&u), &mut v);
        Ok(Json(v))
    }).await
}

/// Every album of a list (an artist's, a filter's) in list order, each with its Type filter key (`type`), once per
/// state of the library: the kinds and types are slices of it in memory, and so are the pages.
fn album_list(st: &AppState, artist: &str, filter: &str, sort: &str, dir: &str) -> ApiResult<Arc<Value>> {
    let mut wh = vec!["1=1".to_string()];
    let mut params: Vec<Value> = vec![];
    if !artist.is_empty() {
        // every spelling of the artist, its aliases and the collaborations it is part of
        let (cond, ps) = st.library.artists.album_filter(&st.db, artist);
        wh.push(cond);
        params.extend(ps);
    }
    let having = match filter {
        "untagged" => "HAVING SUM(CASE WHEN mb_albumid IS NULL OR mb_albumid='' THEN 1 ELSE 0 END) > 0",
        "tagged" => "HAVING SUM(CASE WHEN mb_albumid IS NULL OR mb_albumid='' THEN 1 ELSE 0 END) = 0",
        "lossless" => "HAVING SUM(CASE WHEN format IN ('flac','wav','alac','aiff','ape','wv') THEN 1 ELSE 0 END) = COUNT(*)",
        _ => "",
    };
    let w = wh.join(" AND ");
    let sql = format!(
        "SELECT album_id AS id, MAX(album) album, MAX(albumartist) albumartist, MAX(year) year, MAX(genre) genre,
                COUNT(*) tracks, SUM(duration) duration, SUM(size) size, MAX(added_at) added,
                GROUP_CONCAT(DISTINCT format) formats, MAX(mb_albumid) mb_albumid, MAX(mb_releasegroupid) mb_releasegroupid,
                SUM(CASE WHEN mb_albumid IS NULL OR mb_albumid='' THEN 1 ELSE 0 END) untagged
         FROM tracks WHERE {w} GROUP BY album_id {having} ORDER BY {}",
        order(ALBUM_SORTS, sort, dir, "name")
    );
    let key = format!("albums\u{1f}{sql}\u{1f}{}", Value::Array(params.clone()));
    let st2 = st.clone();
    cached_list(st, &key, move || {
        let mut rows = st2.db.query(&sql, params)?;
        // albums of one name (Weezer's self-titled ones) say which they are: "Weezer (Blue Album)"
        encore_library::split::stamp(&mut rows, &encore_library::split::editions(&st2.db));
        // the type is part of the list: both follow the library's state
        let types = st2.library.types.get(&st2.db);
        for r in rows.iter_mut() {
            let key = types.get(r.get("id").and_then(|v| v.as_str()).unwrap_or("")).map(|c| c.key()).unwrap_or("album");
            r.insert("type".into(), json!(key));
        }
        Ok(json!(rows))
    })
}

/// The albums answer for a query (on the blocking pool).
fn albums_answer(st: &AppState, a: &AlbumQ) -> ApiResult<Value> {
    let searching = !a.q.trim().is_empty();
    let pick = album_types::Pick::of(&a.types, &a.kind, searching).map_err(|e| err(400, e))?;
    // a search is ranked in memory (exact > starts with > a word starts with > contains, accents ignored)
    let ranked = searching.then(|| {
        let mut r = encore_library::search::library_album_ranks(&st.library.artists.index(&st.db), &a.q);
        r.truncate(30000);
        r
    });
    let all = album_list(st, &a.artist, &a.filter, &a.sort, &a.dir)?;
    let types = st.library.types.get(&st.db);
    let mut items: Vec<&Value> = all.as_array().map(|v| v.iter().filter(|i| pick.wants(i["id"].as_str().and_then(|id| types.get(id)))).collect()).unwrap_or_default();
    if let Some(r) = ranked {
        // a search: the matches out of the same list, best rank first (the sort breaks ties; an album's
        // numbers don't depend on the search, which picks whole albums), instead of asking the database
        // again with every matching album id in an IN list
        let tier: HashMap<String, u8> = r.into_iter().collect();
        items.retain(|i| i["id"].as_str().is_some_and(|id| tier.contains_key(id)));
        items.sort_by_key(|i| i["id"].as_str().and_then(|id| tier.get(id)).copied().unwrap_or(9));   // stable
    }
    let total = items.len();
    if matches!(a.ids_only.as_str(), "1" | "true") {
        let ids: Vec<&Value> = items.iter().take(IDS_MAX).map(|i| &i["id"]).collect();
        return Ok(json!({ "total": total, "ids": ids, "capped": total > IDS_MAX }));
    }
    let limit = a.limit.unwrap_or(120).clamp(1, 500) as usize;
    let page: Vec<&Value> = items.into_iter().skip(a.offset.max(0) as usize).take(limit).collect();
    Ok(json!({ "total": total, "items": page }))
}

/// `GET /api/library/album-types` query.
#[derive(Deserialize, Default)]
#[serde(default)]
struct TypeCountsQ {
    /// only this artist's albums
    artist: String,
    /// the albums filter
    filter: String,
}

/// GET /api/library/album-types: the Type dropdown's entries – every filter key in its order with its label and
/// how many albums it has (0 too), over every album (One-offs included) under the same `artist` / `filter`.
async fn album_type_counts(State(st): State<AppState>, User(_): User, Query(q): Query<TypeCountsQ>) -> ApiResult<Json<Value>> {
    st.blocking(move |st| {
        let all = album_list(st, &q.artist, &q.filter, "", "")?;
        let mut counts: HashMap<&str, usize> = HashMap::new();
        let items = all.as_array().map(|a| a.as_slice()).unwrap_or(&[]);
        for i in items {
            *counts.entry(i["type"].as_str().unwrap_or("album")).or_default() += 1;
        }
        let types: Vec<Value> = album_types::TYPES.iter().map(|(k, l)| json!({ "key": k, "label": l, "count": counts.get(k).copied().unwrap_or(0) })).collect();
        Ok(Json(json!({ "total": items.len(), "types": types })))
    }).await
}

/// `PUT /api/library/albums/{id}/type` body.
#[derive(Deserialize)]
struct TypeBody {
    /// Album, EP, Single or One-off; none: automatic again
    #[serde(default)]
    format: Option<String>,
}

/// PUT /api/library/albums/{id}/type (admins): the owner's format for an album – Album, EP, Single or One-off – or
/// null for automatic again. → the album's `type` object.
async fn set_album_type(State(st): State<AppState>, Admin(u): Admin, Path(id): Path<String>, Json(b): Json<TypeBody>) -> ApiResult<Json<Value>> {
    let format: Option<&'static str> = match b.format.as_deref() {
        None => None,
        Some(f) => Some(album_types::FORMATS.iter().find(|x| x.0 == f).map(|x| x.0).ok_or_else(|| err(400, "Pick Album, EP, Single or One-off"))?),
    };
    st.blocking(move |st| {
        if st.db.one("SELECT 1 x FROM tracks WHERE album_id=? LIMIT 1", p![id])?.is_none() {
            return Err(err(404, "Album not found"));
        }
        album_types::choose(&st.db, &id, format, u.i("id"))?;
        Ok(Json(st.library.types.of(&st.db, &id).json()))
    }).await
}

/// The folders the file manager may use (what Trash's items are relative to).
fn file_roots(st: &AppState) -> Vec<encore_files::roots::Root> {
    encore_files::roots::all(&st.cfg, st.data_dir(), &encore_requests::slskd_managed::download_dir(st))
}

/// `POST /api/library/albums/trash` body.
#[derive(Deserialize)]
struct TrashBody {
    /// the albums
    #[serde(default)]
    ids: Vec<String>,
    /// only say what it would do
    #[serde(default)]
    dry_run: bool,
}

/// POST /api/library/albums/trash (admins) `{ids, dry_run}`: what moving these albums to the Trash would do, or
/// doing it (encore_files::albums).
async fn trash_albums(State(st): State<AppState>, Admin(u): Admin, Json(b): Json<TrashBody>) -> ApiResult<Json<Value>> {
    st.blocking(move |st| {
        let roots = file_roots(st);
        encore_files::albums::trash_albums(st, &encore_files::Who::of(&u), &roots, &b.ids, b.dry_run).map(Json)
    }).await
}

/// GET /api/library/tracks/{id}/added: who brought a song in (the song ⋯ menu's first line).
async fn track_added(State(st): State<AppState>, User(_): User, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    let r = st.db.one(&format!("SELECT t.added_by, t.added_via, t.requested_at, t.added_at, u.username FROM {} t LEFT JOIN users u ON u.id=t.added_by WHERE t.id=?", table_of(id)), p![id])?
        .ok_or_else(|| err(404, "Track not found"))?;
    let by_id = r.get("added_by").filter(|v| !v.is_null()).cloned();
    let name = r.get("username").filter(|v| !v.is_null()).cloned();
    Ok(Json(json!({
        "via": r.get("added_via").cloned().unwrap_or(Value::Null), "by": name.clone().unwrap_or(Value::Null), "by_id": by_id.clone().unwrap_or(Value::Null),
        "by_deleted": by_id.is_some() && name.is_none(), "requested_at": r.get("requested_at").cloned().unwrap_or(Value::Null),
        "added_at": r.get("added_at").cloned().unwrap_or(Value::Null),
    })))
}

/// A list query: search, sort and page.
#[derive(Deserialize, Default)]
#[serde(default)]
struct SimpleQ {
    /// a search text
    q: String,
    /// the sort key
    sort: String,
    /// the sort direction
    dir: String,
    /// the page size
    limit: Option<i64>,
    /// where the page starts
    offset: i64,
    /// `/tracks?ids=1,2,3`: these songs, in this order (unknown ids are left out)
    ids: Option<String>,
}

/// Most songs one `/tracks?ids=` call returns (a restored queue is fetched in pages of this size).
const MAX_IDS: usize = 300;


/// Parses `1,2,3` (duplicates dropped, order kept).
fn parse_ids(raw: &str) -> ApiResult<Vec<i64>> {
    let mut out: Vec<i64> = Vec::new();
    for part in raw.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let id: i64 = part.parse().map_err(|_| err(400, format!("Not a song id: {}", part.chars().take(20).collect::<String>())))?;
        if !out.contains(&id) {
            out.push(id);
        }
    }
    if out.len() > MAX_IDS {
        return Err(err(400, format!("At most {MAX_IDS} ids per call")));
    }
    Ok(out)
}


/// Artists (merged spellings, collaborations split into their artists, see `artists`). With `q`: ranked
/// by name / alias, then artists whose albums or songs match; each has "match".
async fn artists(State(st): State<AppState>, User(u): User, Query(a): Query<SimpleQ>) -> ApiResult<Json<Value>> {
    st.blocking(move |st| {
        let idx = st.library.artists.index(&st.db);
        let limit = a.limit.unwrap_or(ARTISTS_MAX as i64).clamp(1, ARTISTS_MAX as i64) as usize;
        let mut v = if a.q.trim().is_empty() {
            let (ix, sort, dir) = (idx.clone(), a.sort.clone(), a.dir.clone());
            let all = cached_list(st, &format!("artists\u{1f}{}\u{1f}{}", a.sort, a.dir), move || Ok(json!(ix.list(&sort, &dir))))?;
            json!(page(&all, a.offset, limit))
        } else {
            json!(encore_library::search::library_artists(&idx, &a.q, limit))
        };
        enrich_now(st, Some(&u), &mut v);
        Ok(Json(v))
    }).await
}

/// Every song of a library artist (all spellings, aliases and collaborations), for Play / Shuffle.
async fn artist_tracks(State(st): State<AppState>, User(u): User, Query(q): Query<NameQ>) -> ApiResult<Json<Value>> {
    if q.name.trim().is_empty() {
        return Ok(Json(json!([])));
    }
    st.blocking(move |st| {
        let (cond, params) = st.library.artists.track_filter(&st.db, &q.name);
        let mut v = json!(st.db.query(&format!(
            "SELECT {TRACK_COLS} FROM tracks WHERE ({cond}) AND {} ORDER BY year DESC, lower(album), disc, track LIMIT 5000", encore_library::twins::SHOWN), params)?);
        enrich_now(st, Some(&u), &mut v);
        Ok(Json(v))
    }).await
}

#[cfg(test)]
#[path = "search_tests.rs"]
mod search_tests;

/// `cols` of these songs, in this order: one indexed lookup by id (at most `MAX_IDS`, or a page of a list).
pub(super) fn rows_by_id(db: &impl encore_db::Query, cols: &str, ids: &[i64]) -> rusqlite::Result<Vec<encore_db::Row>> {
    if ids.is_empty() {
        return Ok(vec![]);
    }
    let rows = db.query(&format!("SELECT {cols} FROM tracks WHERE id IN ({})", encore_library::artists::marks(ids.len())), ids.iter().map(|i| json!(i)).collect())?;
    let mut by_id: HashMap<i64, encore_db::Row> = rows.into_iter().map(|r| (r.i("id"), r)).collect();
    Ok(ids.iter().filter_map(|i| by_id.remove(i)).collect())
}

/// Every song's id in list order, kept until the songs change (see `cached_list`): one sorted pass over
/// the table per sort, and every page of the list is a slice of it. `order_by` is empty for "any order".
fn track_ids(st: &AppState, order_by: &str) -> ApiResult<Arc<Value>> {
    // each song once: not the copies of a song the library has from MusicBrainz (encore_library::twins)
    let shown = encore_library::twins::SHOWN;
    let sql = if order_by.is_empty() { format!("SELECT id FROM tracks WHERE {shown}") } else { format!("SELECT id FROM tracks WHERE {shown} ORDER BY {order_by}") };
    let db = st.db.clone();
    cached_list(st, &format!("track-ids\u{1f}{order_by}"), move || Ok(json!(db.query(&sql, p![])?.iter().map(|r| r.i("id")).collect::<Vec<i64>>())))
}

/// Items `offset..offset+limit` of a cached id list.
fn page_ids(all: &Value, offset: i64, limit: usize) -> Vec<i64> {
    all.as_array().map(|a| a.iter().skip(offset.max(0) as usize).take(limit).filter_map(|v| v.as_i64()).collect()).unwrap_or_default()
}

/// GET /api/library/tracks: the songs list (or `?ids=` those songs in that order).
async fn tracks(State(st): State<AppState>, User(u): User, Query(a): Query<SimpleQ>) -> ApiResult<Json<Value>> {
    if let Some(raw) = a.ids.as_deref() {
        let ids = parse_ids(raw)?;
        let mut v = json!(st.db.read(move |c| rows_by_id(c, TRACK_COLS, &ids)).await?);
        enrich(&st, Some(&u), &mut v).await;
        return Ok(Json(v));
    }
    st.blocking(move |st| {
        let ids: Vec<i64> = if !a.q.trim().is_empty() {
            // ranked in memory, the same song twice shown once (see search::library_track_ids)
            encore_library::search::library_track_ids(&st.library.artists.index(&st.db), &a.q, &a.sort, &a.dir).into_iter()
                .skip(a.offset.max(0) as usize).take(a.limit.unwrap_or(200).clamp(1, 1000) as usize).collect()
        } else {
            let (limit, offset) = (a.limit.unwrap_or(200).clamp(1, 1000) as usize, a.offset.max(0));
            // the id list is sorted once per sort and kept; a page fetches its rows by id (the 0.25.3 WIP cached
            // each page on its own, so every new page was another sort of every song)
            let all = track_ids(st, &order(TRACK_SORTS, &a.sort, &a.dir, "artist"))?;
            page_ids(&all, offset, limit)
        };
        let mut v = json!(rows_by_id(&st.db, TRACK_COLS, &ids)?);
        enrich_now(st, Some(&u), &mut v);
        Ok(Json(v))
    }).await
}

/// GET /api/library/recent: the albums added last (18 by default, at most 200).
async fn recent(State(st): State<AppState>, User(u): User, Query(a): Query<SimpleQ>) -> ApiResult<Json<Value>> {
    let limit = a.limit.unwrap_or(18).clamp(1, 200);
    let rows = cached_read(&st, format!("recent\u{1f}{limit}"), move |c| Ok(json!(c.query(
        "SELECT album_id AS id, MAX(album) album, MAX(albumartist) albumartist, MAX(year) year, MAX(added_at) added, COUNT(*) tracks
         FROM tracks GROUP BY album_id ORDER BY added DESC LIMIT ?", p![limit])?))).await?;
    let mut v = (*rows).clone();
    enrich(&st, Some(&u), &mut v).await;
    Ok(Json(v))
}

/// The song columns a shuffle answer has.
const SHUFFLE_COLS: &str = "id, title, artist, album, album_id, format, bitrate, duration";

/// `GET /api/library/shuffle` query.
#[derive(Deserialize, Default)]
#[serde(default)]
struct ShuffleQ {
    /// how many (default 50)
    limit: Option<i64>,
    /// song ids to leave out, comma separated (at most 500)
    exclude: String,
}

/// GET /api/library/shuffle: random songs.
async fn shuffle_get(State(st): State<AppState>, User(u): User, Query(q): Query<ShuffleQ>) -> ApiResult<Json<Value>> {
    let ex: Vec<i64> = q.exclude.split(',').filter_map(|x| x.trim().parse().ok()).take(500).collect();
    let Json(mut v) = shuffle_inner(&st, u.i("id"), q.limit.unwrap_or(50), &ex)?;
    enrich(&st, Some(&u), &mut v).await;
    Ok(Json(v))
}

/// `POST /api/library/shuffle` body.
#[derive(Deserialize, Default)]
#[serde(default)]
struct ShuffleBody {
    /// how many (default 50)
    limit: Option<i64>,
    /// song ids to leave out
    exclude: Vec<i64>,
}

/// Random songs nobody heard yet this session (the web player sends what it already played), with their
/// loudness for normalization (the player queues them as they are).
async fn shuffle_post(State(st): State<AppState>, User(u): User, Json(b): Json<ShuffleBody>) -> ApiResult<Json<Value>> {
    let Json(mut v) = shuffle_inner(&st, u.i("id"), b.limit.unwrap_or(50), &b.exclude[..b.exclude.len().min(20000)])?;
    encore_library::loudness::stamp(&st.db, &mut v);
    Ok(Json(v))
}

/// Random songs from the cached id list, without `exclude` and without what user `uid` disliked (songs, and
/// every song of a disliked artist: see routes::taste).
fn shuffle_inner(st: &AppState, uid: i64, limit: i64, exclude: &[i64]) -> ApiResult<Json<Value>> {
    let mut ex: std::collections::HashSet<i64> = exclude.iter().copied().collect();
    ex.extend(super::taste::disliked_song_ids(st, uid)?);
    // random picks from the cached id list (`ORDER BY RANDOM()` read and sorted every song for 50 of them)
    let all = track_ids(st, "")?;
    let ids = draw(all.as_array().map(|a| a.as_slice()).unwrap_or(&[]), limit.clamp(1, 200) as usize, &ex);
    Ok(Json(json!(rows_by_id(&st.db, SHUFFLE_COLS, &ids)?)))
}

/// `n` distinct random ids out of `pool` that are not in `exclude` (fewer when the pool runs out):
/// random picks while most of the pool is still unheard, then the remainder listed and shuffled.
fn draw(pool: &[Value], n: usize, exclude: &std::collections::HashSet<i64>) -> Vec<i64> {
    use rand::seq::SliceRandom;
    let mut out: Vec<i64> = Vec::with_capacity(n);
    let mut taken: std::collections::HashSet<i64> = std::collections::HashSet::with_capacity(n);
    if pool.is_empty() {
        return out;
    }
    let mut misses = 0;
    while out.len() < n && misses < n * 8 + 64 {
        match pool[rand::random_range(0..pool.len())].as_i64() {
            Some(id) if !exclude.contains(&id) && taken.insert(id) => out.push(id),
            _ => misses += 1,
        }
    }
    if out.len() < n {
        let mut rest: Vec<i64> = pool.iter().filter_map(|v| v.as_i64()).filter(|i| !exclude.contains(i) && !taken.contains(i)).collect();
        rest.shuffle(&mut rand::rng());
        out.extend(rest.into_iter().take(n - out.len()));
    }
    out
}

/// One play as a client reports it. `played_at` (unix seconds) is when the play was logged: an offline client
/// stamps it when the song is heard and sends it when the server is back; missing means now.
#[derive(Deserialize)]
struct Play {
    /// the song played
    track_id: i64,
    /// how long it played (seconds)
    #[serde(default)]
    seconds: f64,
    /// when it was played (unix seconds; default now)
    #[serde(default)]
    played_at: Option<f64>,
}

/// At most this many plays in one call (413 above): an offline client sends a long outbox in parts.
const MAX_PLAYS: usize = 500;
/// The oldest a reported play may be (400 days); an older `played_at` counts as this old.
const MAX_PLAY_AGE: f64 = 400.0 * 86400.0;
/// The time between two scrobbles of a list, so Last.fm and ListenBrainz don't get hundreds at once.
const SCROBBLE_GAP: std::time::Duration = std::time::Duration::from_millis(250);

/// POST /api/library/plays: one play `{track_id, seconds}` (404 for an unknown song, `{ok: true}`, as always),
/// or the list an offline client sends when the server is back – `{plays: [{track_id, seconds, played_at}]}`
/// or the bare array – answered `{ok: true, saved, skipped}`. In a list, unknown songs are skipped, and so is
/// a play that is already saved (the same song at the same `played_at`: a list sent twice after a lost answer).
/// Each play is saved and scrobbled with its own time, clamped to the last 400 days.
async fn plays(State(st): State<AppState>, User(u): User, Json(b): Json<Value>) -> ApiResult<Json<Value>> {
    let uid = u.i("id");
    let (single, raw) = match b {
        Value::Array(a) => (false, a),
        Value::Object(mut o) if o.contains_key("plays") => match o.remove("plays") {
            Some(Value::Array(a)) => (false, a),
            _ => return Err(err(422, "plays must be a list")),
        },
        one => (true, vec![one]),
    };
    if raw.len() > MAX_PLAYS {
        return Err(err(413, format!("At most {MAX_PLAYS} plays in one call")));
    }
    let list = raw.into_iter().map(serde_json::from_value::<Play>).collect::<Result<Vec<_>, _>>().map_err(|e| err(422, format!("Bad play: {e}")))?;
    let ids: Vec<i64> = list.iter().map(|p| p.track_id).collect();
    let known: HashMap<i64, encore_db::Row> = st.db.query("SELECT id, artist, title, album FROM tracks WHERE id IN (SELECT value FROM json_each(?))", p![json!(ids).to_string()])?
        .into_iter().map(|r| (r.i("id"), r)).collect();
    if single && known.is_empty() {
        // a Surround song: played, but not counted (plays, stats and scrobbles are the stereo library's)
        if list.iter().all(|p| encore_library::surround::is_track(p.track_id)) {
            return Ok(Json(json!({ "ok": true, "saved": 0, "skipped": list.len() })));
        }
        return Err(err(404, "Track not found"));
    }
    let now = now();
    let mut rows: Vec<(i64, f64, f64)> = Vec::with_capacity(list.len()); // (track, seconds, played_at)
    let mut seen = std::collections::HashSet::new();
    let mut skipped = 0;
    for p in list {
        if !known.contains_key(&p.track_id) {
            skipped += 1;
            continue;
        }
        let secs = p.seconds.clamp(0.0, 7200.0);
        let at = p.played_at.filter(|t| t.is_finite()).map_or(now, |t| t.clamp(now - MAX_PLAY_AGE, now));
        if p.played_at.is_some() && !seen.insert((p.track_id, at.to_bits())) {
            skipped += 1;
            continue;
        }
        rows.push((p.track_id, secs, at));
    }
    let mut saved: Vec<(i64, f64, f64)> = vec![];
    if !rows.is_empty() {
        // one statement on the writer: the whole list is saved or none of it, and a play already saved (the same
        // song at the same time) is left out inside it, so two calls with the same list at the same moment (two
        // tabs back online together) can't both get past the check; RETURNING names what went in
        let sql = format!(
            "INSERT INTO plays (user_id, track_id, artist, title, album, seconds, played_at) \
             SELECT v.column1, v.column2, v.column3, v.column4, v.column5, v.column6, v.column7 FROM (VALUES {}) AS v \
             WHERE NOT EXISTS (SELECT 1 FROM plays p WHERE p.user_id = v.column1 AND p.played_at = v.column7 AND p.track_id = v.column2) \
             RETURNING track_id, seconds, played_at",
            vec!["(?,?,?,?,?,?,?)"; rows.len()].join(",")
        );
        let mut params = Vec::with_capacity(rows.len() * 7);
        for (tid, secs, at) in &rows {
            let t = &known[tid];
            params.extend(p![uid, tid, t.s("artist"), t.s("title"), t.s("album"), secs, at]);
        }
        saved = st.db.query(&sql, params)?.iter().map(|r| (r.i("track_id"), r.f("seconds"), r.f("played_at"))).collect();
        skipped += rows.len() - saved.len();
    }
    let starts: Vec<(i64, i64)> = saved.iter().map(|(tid, secs, at)| (*tid, (at - secs) as i64)).collect();
    if single {
        for (tid, start) in starts {
            encore_sync::scrobble::scrobble(&st, uid, tid, start);
        }
        return Ok(Json(json!({ "ok": true })));
    }
    if !starts.is_empty() {
        let st = st.clone();
        tokio::spawn(async move {
            for (tid, start) in starts {
                encore_sync::scrobble::scrobble(&st, uid, tid, start);
                tokio::time::sleep(SCROBBLE_GAP).await;
            }
        });
    }
    Ok(Json(json!({ "ok": true, "saved": saved.len(), "skipped": skipped })))
}

/// A song started: shows as "now playing" on Last.fm / ListenBrainz.
async fn now_playing(State(st): State<AppState>, User(u): User, Json(b): Json<Play>) -> Json<Value> {
    encore_sync::scrobble::now_playing(&st, u.i("id"), b.track_id);
    Json(json!({ "ok": true }))
}

/// Users with the "fix" permission may act on songs they requested: the request is theirs, or – after Clear
/// finished deleted it – the song says they asked for it (`added_by`, a request-like `added_via`).
fn owns_song(st: &AppState, u: &encore_db::Row, t: &encore_db::Row) -> bool {
    if !auth::can(st, u, "fix") {
        return false;
    }
    let me = u.i("id");
    let request_id = t.i("request_id");
    (request_id > 0 && st.db.one("SELECT user_id FROM requests WHERE id=?", p![request_id]).ok().flatten().is_some_and(|r| r.i("user_id") == me))
        || (t.i("added_by") == me && matches!(t.s("added_via").as_str(), "request" | "link" | "soulseek"))
}

/// DELETE /api/library/tracks/{id}: delete a song and its file (admins, or who brought it in).
async fn delete_track(State(st): State<AppState>, User(u): User, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    let t = st.db.one("SELECT path, artist, title, request_id, added_by, added_via FROM tracks WHERE id=?", p![id])?.ok_or_else(|| err(404, "Track not found"))?;
    if !encore_config::perms::is_admin(&u) && !owns_song(&st, &u, &t) {
        return Err(err(403, "You can only remove songs you requested"));
    }
    delete_tracks(&st, &[id], None);
    if let Some(d) = FsPath::new(&t.s("path")).parent() {
        prune_empty(d, FsPath::new(&st.cfg.str("library_path")));
    }
    st.db.log("info", &format!("{} deleted {} – {}", u.s("username"), t.s("artist"), t.s("title")));
    Ok(Json(json!({ "ok": true })))
}

/// The table a song id is in: the Surround library's ids start at a billion (encore_library::surround).
fn table_of(id: i64) -> &'static str {
    if encore_library::surround::is_track(id) { "surround_tracks" } else { "tracks" }
}

/// GET /api/library/tracks/{id}/file: the song's file as it is (a download).
async fn track_file(State(st): State<AppState>, User(_): User, Path(id): Path<i64>, req: Request) -> ApiResult<Response> {
    let t = st.db.one(&format!("SELECT path, artist, title FROM {} WHERE id=?", table_of(id)), p![id])?.ok_or_else(|| err(404, "File not found"))?;
    let path = t.s("path");
    let ext = FsPath::new(&path).extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    serve_file(&path, req, None, Some(encore_core::fs::safe_file_name(&format!("{} - {}{ext}", t.s("artist"), t.s("title"))))).await
}

use encore_core::audio::mime as browser_type;
use encore_media::Ffmpeg;

/// A library song's stream: what `encore_playback::stream::plan_stream` decides (the original, a conversion
/// made on the fly, or server processing by Cadenza). `user` is None for share links, which never get
/// server processing.
pub async fn stream_track(st: &AppState, id: i64, q: &StreamQ, req: Request, user: Option<&encore_db::Row>) -> ApiResult<Response> {
    // a Surround song's format is its codec (eac3, truehd …): no browser plays those, so they are converted
    let tr = st.db.one(&format!("SELECT path, format FROM {} WHERE id=?", table_of(id)), p![id])?.ok_or_else(|| err(404, "Track not found"))?;
    // the permission is only looked at when processing is asked for
    let listener = user.map(|u| Listener { user: u.i("id"), may_process: q.dsp && auth::can(st, u, "server_dsp") });
    serve_stream(st, id, &tr.s("path"), &tr.s("format"), q, req, listener).await
}

/// GET /api/library/tracks/{id}/stream: the song as the player streams it (as is, or transcoded / processed).
async fn stream(State(st): State<AppState>, User(u): User, Path(id): Path<i64>, Query(q): Query<StreamQ>, req: Request) -> ApiResult<Response> {
    stream_track(&st, id, &q, req, Some(&u)).await
}

/// One song as the lists return it (the car player rebuilds its queue from ids).
async fn track(State(st): State<AppState>, User(u): User, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    if encore_library::surround::is_track(id) {
        let r = encore_library::surround::track(&st.db, id).ok_or_else(|| err(404, "Track not found"))?;
        return Ok(Json(encore_library::surround::track_json(&r)));
    }
    let mut t = st.db.one("SELECT id, title, artist, album, albumartist, album_id, track, disc, year, duration, format, bitrate, path FROM tracks WHERE id=?", p![id])?.ok_or_else(|| err(404, "Track not found"))?;
    if !encore_config::perms::is_admin(&u) { t.remove("path"); }
    let mut v = Value::Object(t);
    enrich(&st, Some(&u), &mut v).await;
    Ok(Json(v))
}

/// A row limit query.
#[derive(Deserialize, Default)]
#[serde(default)]
struct LimitQ {
    /// how many (0 = the default)
    limit: i64,
}

/// What this user played most recently (one row per song, newest first).
async fn recent_plays(State(st): State<AppState>, User(u): User, Query(q): Query<LimitQ>) -> ApiResult<Json<Value>> {
    let limit = if q.limit <= 0 { 50 } else { q.limit.min(200) };
    let rows = st.db.query(
        "SELECT t.id, t.title, t.artist, t.album, t.albumartist, t.album_id, t.track, t.disc, t.year, t.duration, t.format, t.bitrate, MAX(p.played_at) played_at \
         FROM plays p JOIN tracks t ON t.id=p.track_id WHERE p.user_id=? GROUP BY t.id ORDER BY played_at DESC LIMIT ?",
        p![u.i("id"), limit],
    )?;
    let mut v = json!(rows);
    enrich(&st, Some(&u), &mut v).await;
    Ok(Json(v))
}

/// `GET /api/library/tracks/{id}/info` query.
#[derive(Deserialize, Default)]
#[serde(default)]
struct InfoQ {
    /// the codecs the client decodes, as `?codecs=` on the stream URL (or send `X-Encore-Decoders`)
    codecs: String,
}

/// What's in the file and, under `stream`, how its plain stream is served for this client (`kind` is the
/// `x-encore-stream` label the stream would carry, `codec` the file's codec, `reason` why it is not the
/// original) – given the same `X-Encore-Decoders` header or `?codecs=` list as the stream request.
/// `offline_ok` is true when that stream is the original, so a copy saved for offline play is the file itself.
/// `gain` (0.31, when anything is known): the song's loudness as its track rows carry it, with `source`
/// ("tags" | "measured") and `lufs` (see encore_library::loudness).
async fn track_info(State(st): State<AppState>, User(_): User, Path(id): Path<i64>, Query(q): Query<InfoQ>, headers: axum::http::HeaderMap) -> ApiResult<Json<Value>> {
    let surround = encore_library::surround::is_track(id);
    let t = st.db.one(&format!("SELECT id, path, format, size, bitrate, duration FROM {} WHERE id=?", table_of(id)), p![id])?.ok_or_else(|| err(404, "Track not found"))?;
    let (path, media) = (t.s("path"), st.media.clone());
    let mut info = tokio::task::spawn_blocking(move || media.audio_info(&path)).await.unwrap_or(json!({}));
    let plan = plan(&t.s("format"), &info, decoders(&headers, &q.codecs).as_deref());
    info["stream"] = json!({ "kind": plan.kind, "codec": plan.codec, "reason": plan.reason });
    // otherwise a client saves a conversion (`?q=aac` or `?q=opus`, its quality marked "(converted)")
    info["offline_ok"] = json!(plan.kind == "original");
    info["id"] = json!(t.i("id"));
    info["format"] = json!(t.s("format"));
    info["size"] = json!(t.i("size"));
    if info["bitrate"].is_null() { info["bitrate"] = json!(t.i("bitrate")); }
    if info["duration"].is_null() { info["duration"] = json!(t.f("duration")); }
    if surround {
        // what the receiver gets: "Dolby Atmos (E-AC-3 5.1)"; no quality check or loudness for surround songs
        info["class"] = json!("surround");
        if let Some(r) = encore_library::surround::track(&st.db, id) {
            info["surround"] = json!({ "label": encore_library::surround::track_json(&r)["quality"], "layout": r.s("layout"), "atmos": r.i("atmos") == 1 });
        }
        return Ok(Json(info));
    }
    info["quality"] = encore_media::quality::for_track(&st, id); // "Fake lossless? cutoff 16 kHz" in the signal path
    // "Your own file, uploaded by alex" in the signal path (⋯ → Replace with my own file)
    info["own_file"] = encore_library::own_file::latest(&st, id).unwrap_or(Value::Null);
    if let Some(gain) = encore_library::loudness::info_gain(&st.db, id) {
        info["gain"] = gain;
    }
    Ok(Json(info))
}

/// `GET /api/library/tracks/{id}/lyrics` query.
#[derive(Deserialize, Default)]
#[serde(default)]
struct LyricsQ {
    /// look them up when missing (default on)
    fetch: Option<bool>,
    /// look them up again even when there are some
    force: Option<bool>,
}

/// Lyrics next to the song (.lrc synced, .txt plain), looked up on LRCLIB and saved when missing; with
/// `shift_ms`, how much later the time-synced lines are to be shown (the song's saved nudge).
async fn lyrics(State(st): State<AppState>, User(_): User, Path(id): Path<i64>, Query(q): Query<LyricsQ>) -> ApiResult<Json<Value>> {
    let (id, answer) = lyrics_answer(&st, id, q.fetch.unwrap_or(true), q.force.unwrap_or(false)).await?;
    Ok(Json(encore_library::lyrics::with_shift(&st, id, answer)))
}

/// The lyrics of song `id` (a Surround song: the .lrc beside it, else its stereo song's), looked up when
/// missing if `fetch` (again even when there are some if `force`).
/// → (the id the lyrics and their nudge belong to, {lyrics, synced, source, …})
async fn lyrics_answer(st: &AppState, id: i64, fetch: bool, force: bool) -> ApiResult<(i64, Value)> {
    if encore_library::surround::is_track(id) {
        // a Surround song: the .lrc beside it (gamdl writes one), else its stereo song's
        let r = encore_library::surround::track(&st.db, id).ok_or_else(|| err(404, "Track not found"))?;
        if let Some((text, synced)) = encore_library::lyrics::sidecar(FsPath::new(&r.s("path"))) {
            return Ok((id, json!({ "lyrics": text, "synced": synced, "source": "file" })));
        }
        if r.i("stereo_track_id") <= 0 {
            return Ok((id, json!({ "lyrics": null, "synced": false, "source": null })));
        }
    }
    let id = if encore_library::surround::is_track(id) {
        encore_library::surround::track(&st.db, id).map(|r| r.i("stereo_track_id")).unwrap_or(0)
    } else {
        id
    };
    let t = st.db.one("SELECT id, path, artist, title, album, duration FROM tracks WHERE id=?", p![id])?.ok_or_else(|| err(404, "Track not found"))?;
    if !fetch {
        return Ok((id, match encore_library::lyrics::sidecar(FsPath::new(&t.s("path"))) {
            Some((text, synced)) => json!({ "lyrics": text, "synced": synced, "source": "file" }),
            None => json!({ "lyrics": null, "synced": false, "source": null }),
        }));
    }
    let answer = encore_library::lyrics::fetch_for_track(st, &t, force).await.map_err(|e| err(502, format!("Lyrics lookup failed: {e}")))?;
    Ok((id, answer))
}

/// `PUT /api/library/tracks/{id}/lyrics/shift` body.
#[derive(Deserialize)]
struct ShiftBody {
    /// how much later every time-synced line is to be shown, ms (negative: earlier; 0: as the lyrics say).
    /// The total the lyrics view shows: an `[offset:]` tag of the .lrc counts in it and is taken out here.
    shift_ms: i64,
}

/// Nudge a song's time-synced lyrics (the lyrics view's Earlier / Later), for every device and listener.
/// → the lyrics answer with the new `shift_ms`
async fn lyrics_shift(State(st): State<AppState>, User(_): User, Path(id): Path<i64>, Json(b): Json<ShiftBody>) -> ApiResult<Json<Value>> {
    // the lyrics shown now (the file next to the song; with saving files off, what LRCLIB answers again)
    let (id, answer) = lyrics_answer(&st, id, true, false).await?;
    let text = answer["lyrics"].as_str().filter(|_| answer["synced"].as_bool().unwrap_or(false))
        .ok_or_else(|| err(409, "Only time-synced lyrics can be nudged"))?
        .to_string();
    // the file's own [offset:] tag stays in the file; what is saved is the rest
    let saved = b.shift_ms - encore_library::lyrics::lrc_offset_shift(&text);
    encore_library::lyrics::set_shift(&st, id, &text, saved).map_err(|e| err(500, e.to_string()))?;
    Ok(Json(encore_library::lyrics::with_shift(&st, id, answer)))
}

/// GET /api/library/albums/{id}: an album with its songs.
async fn album(State(st): State<AppState>, User(u): User, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    if encore_library::surround::is_album(&id) {
        return surround_album(&st, &u, &id).await;
    }
    // who brought each song in: kept on the song (0.36), so Clear finished – which deletes the request – doesn't
    // lose the name; the request's own user and day for rows from before
    let aid = id.clone();
    let rows = st.db.read(move |c| c.query("SELECT t.*, ua.username added_by_name, u.username request_user, r.created_at request_created,
                            q.status quality_status, q.summary quality_note FROM tracks t LEFT JOIN requests r ON r.id=t.request_id
                            LEFT JOIN users u ON u.id=r.user_id LEFT JOIN users ua ON ua.id=t.added_by LEFT JOIN track_quality q ON q.track_id=t.id
                            WHERE t.album_id=? ORDER BY t.disc, t.track, t.path", p![aid])).await?;
    if rows.is_empty() {
        return Err(err(404, "Album not found"));
    }
    // an album merged from copies (encore_library::merge): each song once, in its best copy
    // (its size and when it came are of every copy)
    let copies = encore_library::merge::other_copies(&rows);
    let (size, added_at) = (rows.iter().map(|r| r.i("size")).sum::<i64>(), rows.iter().filter_map(|r| r.get("added_at").and_then(|v| v.as_f64())).reduce(f64::min));
    let rows: Vec<_> = rows.into_iter().filter(|r| !copies.contains(&r.i("id"))).collect();
    let libp = format!("{}/", st.cfg.str("library_path").trim_end_matches('/'));
    let admin = encore_config::perms::is_admin(&u);
    let tracks: Vec<Value> = rows.iter().map(|r| {
        let mut r = r.clone();
        let p = r.s("path");
        r.insert("relpath".into(), json!(p.strip_prefix(&libp).unwrap_or(&p)));
        if !admin {
            r.remove("path");
        }
        who_added(&mut r);
        // the raw credit tags: songs carry `artists` (each artist, split) instead
        r.remove("artist_credit");
        r.remove("albumartist_credit");
        Value::Object(r)
    }).collect();
    // the album's name and ids from its MusicBrainz copy when it has one
    let f = rows.iter().find(|r| !r.s("mb_releasegroupid").is_empty()).unwrap_or(&rows[0]);
    let class = {
        let id = id.clone();
        st.blocking(move |st| Ok(st.library.types.of(&st.db, &id))).await?
    };
    // an album of one name with others (Weezer's self-titled ones): which it is, "Weezer (Blue Album)"
    let (title, rg) = (f.s("album"), f.s("mb_releasegroupid"));
    let edition = {
        let (id, title, rg) = (id.clone(), title.clone(), rg.clone());
        st.blocking(move |st| {
            let e = encore_library::split::edition_of(&st.db, &id, &title);
            // not named by MusicBrainz yet: asked in the background, the next look shows it
            let asked = st.db.one("SELECT 1 x FROM rg_names WHERE rg=?", p![rg])?.is_some();
            Ok((e, !rg.is_empty() && !asked))
        }).await?
    };
    if edition.1 {
        let st2 = st.clone();
        tokio::spawn(async move { encore_library::split::look_up_names(&st2, 12).await });
    }
    let mut v = json!({
        "id": id, "album": encore_library::split::shown_title(&title, &edition.0), "album_title": title, "edition": edition.0,
        "albumartist": f.get("albumartist"), "year": f.get("year"), "genre": f.get("genre"),
        "mb_albumid": f.get("mb_albumid"), "mb_releasegroupid": f.get("mb_releasegroupid"), "mb_albumartistid": f.get("mb_albumartistid"),
        "tracks": tracks, "duration": rows.iter().map(|r| r.f("duration")).sum::<f64>(), "size": size,
        "type": class.json(), "history": history(&tracks_of(&rows)), "other_copies": copies.len(), "added_at": added_at,
        // how good the album's copy is (its worst song) and what ⋯ → Upgrade quality would ask for
        "quality_summary": encore_requests::upgrade::album_quality(&rows),
    });
    enrich(&st, Some(&u), &mut v).await;
    // its Surround versions (0.35): the surround albums, the request fetching one, whether Apple has one; and
    // each song's surround version
    v["surround"] = encore_requests::surround::album_state(&st, &id);
    let ids: Vec<i64> = rows.iter().map(|r| r.i("id")).collect();
    let versions = encore_library::surround::for_stereo_tracks(&st.db, &ids);
    if let Some(ts) = v["tracks"].as_array_mut() {
        for t in ts {
            if let Some(s) = t["id"].as_i64().and_then(|i| versions.get(&i)) {
                t["surround_id"] = json!(s);
            }
        }
    }
    Ok(Json(v))
}

/// POST /api/library/albums/{id}/upgrade (admins): ⋯ → Upgrade quality. Asks for the next better copy of the
/// album – lossless when it is lossy, hi-res (Qobuz/Tidal through streamrip) when it is CD quality – and
/// answers `{request_id, tier, text}`; 409 when it is hi-res already or an upgrade runs, 422 when it can't be
/// looked for (not matched to MusicBrainz, or hi-res without streamrip).
async fn upgrade_album(State(st): State<AppState>, Admin(_): Admin, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    use encore_requests::upgrade::{queue_album, Queued, Tier};
    let done = st.blocking(move |st| queue_album(st, &id).map_err(|e| err(500, e.to_string()))).await?;
    match done {
        Queued::Requested(rid, tier) => {
            let text = if tier == Tier::HiRes { "Looking for a hi-res copy on Qobuz/Tidal" } else { "Looking for a lossless copy" };
            Ok(Json(json!({ "request_id": rid, "tier": tier.key(), "text": text })))
        }
        Queued::Running(rid) => Err(err(409, format!("An upgrade of this album is already under way (request #{rid})"))),
        Queued::Best => Err(err(409, "This album is already hi-res – there is nothing better to look for")),
        Queued::Unmatched => Err(err(422, "This album isn't matched to MusicBrainz yet – use Tag / match metadata first")),
        Queued::NoHiresSource => Err(err(422, "A hi-res copy comes from Qobuz or Tidal – set up streamrip with a Qobuz or Tidal account first")),
        Queued::NotFound => Err(err(404, "Album not found")),
    }
}

/// Is this way of adding a request (a request, a link, a Soulseek-page download)?
fn request_like(via: &str) -> bool {
    matches!(via, "request" | "link" | "soulseek")
}

/// A song row of the album page with who added it: `added_via`, `added_by` (id), `added_by_name`,
/// `added_by_deleted`, and – for older apps – `requested_by` / `requested_at` (the adder of a request-like song, else
/// the request's user; the song's request day, else the request's).
fn who_added(r: &mut encore_db::Row) {
    let via = r.s("added_via");
    let name = r.get("added_by_name").filter(|v| !v.is_null()).cloned();
    let by = r.get("added_by").filter(|v| !v.is_null()).cloned();
    let requester = r.remove("request_user").filter(|v| !v.is_null());
    let request_day = r.remove("request_created").filter(|v| !v.is_null());
    r.insert("added_by_deleted".into(), json!(by.is_some() && name.is_none()));
    let requested_by = if request_like(&via) { name.clone() } else { name.clone().or(requester) };
    r.insert("requested_by".into(), requested_by.unwrap_or(Value::Null));
    let at = r.get("requested_at").filter(|v| !v.is_null()).cloned().or(request_day);
    r.insert("requested_at".into(), at.unwrap_or(Value::Null));
    r.insert("added_by_name".into(), name.unwrap_or(Value::Null));
    // the files' type tags are the server's business (the album's `type` says what they mean)
    for k in ["releasetype", "tracktotal", "compilation"] {
        r.remove(k);
    }
}

/// (via, by id, by name, requested_at, added_at) of a song, for [`history`].
type Added = (String, Option<i64>, Option<String>, Option<f64>, f64);

/// [`Added`] of each song.
fn tracks_of(rows: &[encore_db::Row]) -> Vec<Added> {
    rows.iter().map(|r| {
        (r.s("added_via"), r.get("added_by").and_then(|v| v.as_i64()), r.get("added_by_name").and_then(|v| v.as_str()).map(String::from),
         r.get("requested_at").and_then(|v| v.as_f64()).or_else(|| r.get("request_created").and_then(|v| v.as_f64())), r.f("added_at"))
    }).collect()
}

/// The album page's history line, as groups: the songs by (how they came – requests, links and Soulseek-page
/// downloads are one – and who), most songs first, then the earliest. `[{via, by, by_deleted, requested_at, songs}]`
fn history(songs: &[Added]) -> Value {
    struct G {
        via: String,
        by_id: Option<i64>,
        by: Option<String>,
        at: Option<f64>,
        first: f64,
        songs: usize,
    }
    let mut groups: Vec<G> = vec![];
    for (via, by_id, by, at, added) in songs {
        if via.is_empty() {
            continue;
        }
        fn class(v: &str) -> &str {
            if request_like(v) { "request" } else { v }
        }
        match groups.iter_mut().find(|g| class(&g.via) == class(via) && g.by_id == *by_id) {
            Some(g) => {
                g.songs += 1;
                g.at = match (g.at, *at) { (Some(a), Some(b)) => Some(a.min(b)), (a, b) => a.or(b) };
                g.first = g.first.min(*added);
            }
            None => groups.push(G { via: via.clone(), by_id: *by_id, by: by.clone(), at: *at, first: *added, songs: 1 }),
        }
    }
    groups.sort_by(|a, b| b.songs.cmp(&a.songs).then(a.at.unwrap_or(a.first).partial_cmp(&b.at.unwrap_or(b.first)).unwrap_or(std::cmp::Ordering::Equal)));
    json!(groups.iter().map(|g| json!({
        "via": g.via, "by": g.by, "by_deleted": g.by_id.is_some() && g.by.is_none(), "requested_at": g.at, "songs": g.songs,
    })).collect::<Vec<_>>())
}

/// A Surround album: its songs (what each file is: "Dolby Atmos (E-AC-3 5.1)") and the stereo album it belongs to.
async fn surround_album(st: &AppState, u: &encore_db::Row, id: &str) -> ApiResult<Json<Value>> {
    let mut tracks = encore_library::surround::album_tracks(&st.db, id);
    if tracks.is_empty() {
        return Err(err(404, "Album not found"));
    }
    let admin = encore_config::perms::is_admin(u);
    if admin {
        for t in tracks.iter_mut() {
            if let Some(r) = t["id"].as_i64().and_then(|i| encore_library::surround::track(&st.db, i)) {
                t["path"] = json!(r.s("path"));
            }
        }
    }
    let f = tracks[0].clone();
    let stereo = tracks.iter().find_map(|t| t["stereo_album_id"].as_str().filter(|s| !s.is_empty()).map(String::from));
    Ok(Json(json!({
        "id": id, "class": "surround", "album": f["album"], "albumartist": f["albumartist"], "year": f["year"],
        "quality": f["quality"], "atmos": tracks.iter().any(|t| t["atmos"] == true),
        "stereo": stereo.map(|s| json!({ "album_id": s })),
        "duration": tracks.iter().filter_map(|t| t["duration"].as_f64()).sum::<f64>(),
        "tracks": tracks,
    })))
}

/// A cover query.
#[derive(Deserialize, Default)]
#[serde(default)]
struct CoverQ {
    /// the size in pixels (0 = the original)
    s: u32,
    /// the cover's version (`cover_v` from the JSON): the answer may then be cached for good
    v: String,
}

/// Whether `If-None-Match` names this ETag (or `*`).
fn etag_matches(headers: &axum::http::HeaderMap, etag: &str) -> bool {
    headers.get(header::IF_NONE_MATCH).and_then(|h| h.to_str().ok())
        .is_some_and(|h| h.split(',').any(|t| { let t = t.trim(); t == etag || t == "*" || t.strip_prefix("W/") == Some(etag) }))
}

/// A file answered after the ETag check: 304 when `If-None-Match` names `etag`. A request that sends an ETag
/// is answered by it alone (as HTTP says): the file's date is not looked at, since the same URL may serve
/// another, older file now (another album's cover under a playlist link).
async fn serve_tagged(path: &std::path::Path, mut req: Request, content_type: Option<&str>, etag: &str) -> ApiResult<Response> {
    let mut r = if etag_matches(req.headers(), etag) {
        axum::http::StatusCode::NOT_MODIFIED.into_response()
    } else {
        if req.headers().contains_key(header::IF_NONE_MATCH) {
            req.headers_mut().remove(header::IF_MODIFIED_SINCE);
        }
        serve_file(&path.to_string_lossy(), req, content_type, None).await?
    };
    r.headers_mut().insert(header::ETAG, HeaderValue::from_str(etag).unwrap_or(HeaderValue::from_static("\"0\"")));
    Ok(r)
}

/// A cover (or a thumbnail of it, WebP when the client takes it). With the current `v` the answer is
/// immutable; without it, clients keep it a day and revalidate with ETag / Last-Modified. The ETag names the
/// album too, so one URL that stands for different albums over time (a playlist link) never gets a 304 for
/// another album's cover.
pub async fn cover_response(st: &AppState, album_id: &str, size: u32, v: &str, req: Request) -> ApiResult<Response> {
    let size = encore_media::covers::snap(size);
    let webp = size > 0 && req.headers().get(header::ACCEPT).and_then(|a| a.to_str().ok()).is_some_and(|a| a.contains("image/webp"));
    let (st2, aid) = (st.clone(), album_id.to_string());
    let (path, cur) = tokio::task::spawn_blocking(move || {
        let (p, cv) = st2.library.covers.get(&st2.db, &aid)?;
        Some((if size > 0 { encore_media::covers::thumbnail(&st2.db, &p, &aid, cv, size, webp) } else { p }, cv))
    }).await.ok().flatten().ok_or_else(|| err(404, "No cover"))?;
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let album: String = album_id.chars().filter(char::is_ascii_alphanumeric).collect();
    let etag = format!("\"{album}-{cur:x}-{size}-{ext}\"");
    let cache = if !v.is_empty() && v == cur.to_string() { "private, max-age=31536000, immutable" } else { "private, max-age=86400, stale-while-revalidate=604800" };
    let mut r = serve_tagged(&path, req, None, &etag).await?;
    let h = r.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    if size > 0 {
        h.insert(header::VARY, HeaderValue::from_static("Accept"));
    }
    Ok(r)
}

/// GET /api/library/albums/{id}/cover: the album's cover.
async fn cover(State(st): State<AppState>, User(_): User, Path(id): Path<String>, Query(q): Query<CoverQ>, req: Request) -> ApiResult<Response> {
    cover_response(&st, &id, q.s, &q.v, req).await
}

/// GET /api/library/albums/{id}/download: the album's files as a zip.
async fn album_zip(State(st): State<AppState>, User(_): User, Path(id): Path<String>, req: Request) -> ApiResult<Response> {
    let rows = st.db.query("SELECT path, albumartist, album FROM tracks WHERE album_id=? ORDER BY disc, track, path", p![id])?;
    if rows.is_empty() {
        return Err(err(404, "Album not found"));
    }
    let name = encore_core::fs::safe_file_name(&format!("{} - {}.zip", rows[0].s("albumartist"), rows[0].s("album")));
    let st2 = st.clone();
    let tmp = tokio::task::spawn_blocking(move || -> anyhow::Result<std::path::PathBuf> {
        let tmp = std::env::temp_dir().join(format!("encore-{}.zip", encore_core::ids::random_hex(8)));
        let songs: Vec<String> = rows.iter().map(|r| r.s("path")).collect();
        write_zip(&tmp, &songs, || st2.library.find_cover(&st2.db, &id))?;
        Ok(tmp)
    }).await.map_err(|e| err(500, e.to_string()))?.map_err(|e| err(500, format!("Could not build the zip: {e}")))?;
    let resp = serve_file(&tmp.to_string_lossy(), req, Some("application/zip"), Some(name)).await;
    // the open file handle keeps the data readable after the name is gone (unix)
    let _ = std::fs::remove_file(&tmp);
    resp
}

/// Writes the songs (the ones that can be opened) and then the cover to a new zip at `to`. When that fails
/// part way (two songs with the same file name, a full disk), the part already written is removed again,
/// so a failed download leaves nothing behind in the temp folder.
fn write_zip(to: &FsPath, songs: &[String], cover: impl FnOnce() -> Option<std::path::PathBuf>) -> anyhow::Result<()> {
    let r = (|| -> anyhow::Result<()> {
        let mut z = zip::ZipWriter::new(std::fs::File::create(to)?);
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored).large_file(true);
        for p in songs {
            if let (Ok(mut f), Some(n)) = (std::fs::File::open(p), FsPath::new(p).file_name()) {
                z.start_file(n.to_string_lossy(), opts)?;
                std::io::copy(&mut f, &mut z)?;
            }
        }
        if let Some(c) = cover() {
            if let Ok(mut f) = std::fs::File::open(&c) {
                let ext = c.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or("jpg".into());
                z.start_file(format!("cover.{ext}"), opts)?;
                std::io::copy(&mut f, &mut z)?;
            }
        }
        z.finish()?;
        Ok(())
    })();
    if r.is_err() {
        let _ = std::fs::remove_file(to);
    }
    r
}

#[cfg(test)]
mod write_zip_tests {
    use super::write_zip;

    #[test]
    fn a_zip_that_cannot_be_built_leaves_no_file() {
        let d = encore_core::tmp::dir("albumzip");
        for (sub, body) in [("music", "one"), ("music-live", "two")] {
            std::fs::create_dir_all(d.join(sub)).unwrap();
            std::fs::write(d.join(sub).join("01 - Song 1.flac"), body).unwrap();
        }
        std::fs::write(d.join("music").join("02 - Song 2.flac"), "three").unwrap();
        std::fs::write(d.join("cover.png"), "png").unwrap();
        let song = |sub: &str, n: &str| d.join(sub).join(n).to_string_lossy().into_owned();

        // two songs with one file name: the zip can't hold both, and the half-written file is gone
        let bad = d.join("bad.zip");
        let same = [song("music", "01 - Song 1.flac"), song("music-live", "01 - Song 1.flac")];
        assert!(write_zip(&bad, &same, || None).is_err());
        assert!(!bad.exists());

        // the songs that open, then the cover; a song that is gone is left out as before
        let good = d.join("good.zip");
        let songs = [song("music", "01 - Song 1.flac"), song("music", "02 - Song 2.flac"), song("music", "gone.flac")];
        write_zip(&good, &songs, || Some(d.join("cover.png"))).unwrap();
        let z = zip::ZipArchive::new(std::fs::File::open(&good).unwrap()).unwrap();
        let mut names: Vec<String> = z.file_names().map(|n| n.unwrap().into_owned()).collect();
        names.sort();
        assert_eq!(names, ["01 - Song 1.flac", "02 - Song 2.flac", "cover.png"]);
        let _ = std::fs::remove_dir_all(&d);
    }
}

/// DELETE /api/library/albums/{id} (admins): the album to the Trash (restorable from Files → Trash), as the
/// Library's Delete… moves several (encore_files::albums).
async fn delete_album(State(st): State<AppState>, Admin(u): Admin, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    st.blocking(move |st| {
        if st.db.one("SELECT 1 x FROM tracks WHERE album_id=? LIMIT 1", p![id])?.is_none() {
            return Err(err(404, "Album not found"));
        }
        let roots = file_roots(st);
        let r = encore_files::albums::trash_albums(st, &encore_files::Who::of(&u), &roots, std::slice::from_ref(&id), false)?;
        if r["albums"].as_u64().unwrap_or(0) == 0 {
            let why = r["failed"][0]["reason"].as_str().unwrap_or("It couldn't be moved to the Trash").to_string();
            return Err(err(409, why));
        }
        Ok(Json(json!({ "ok": true, "text": r["text"], "items": r["items"] })))
    }).await
}

/// An artist name query.
#[derive(Deserialize)]
struct NameQ {
    /// the artist's name
    name: String,
}

/// Remove an artist added by mistake (e.g. a wrong download): files, follow, pending requests. Every
/// spelling the library merged into the artist goes (its own and its alias spellings); albums it made
/// with other artists stay, since they belong to those artists too. The artist index, the deletes and the
/// file work run on the blocking pool.
async fn delete_artist(State(st): State<AppState>, Admin(u): Admin, Query(q): Query<NameQ>) -> ApiResult<Json<Value>> {
    let (v, cancelled) = st.blocking(move |st| delete_artist_now(st, &u, &q)).await?;
    // its requests that were not done stop (a waiting one would import the artist again later); only
    // Encore's own staging folders of them go
    for id in cancelled {
        encore_requests::worker::cancel(&st, id).await;
        encore_requests::events::emit(&st, id);
    }
    Ok(v)
}

/// Delete an artist's songs, follows and pending requests (see [`delete_artist`]): the answer and the deleted song ids.
fn delete_artist_now(st: &AppState, u: &encore_db::Row, q: &NameQ) -> ApiResult<(Json<Value>, Vec<i64>)> {
    let idx = st.library.artists.index(&st.db);
    // (an alias that is also another artist's name, kept apart by MusicBrainz, stays with that artist)
    let mut names: Vec<String> = idx.find(&q.name).map(|a| a.spellings.iter().chain(a.aliases.iter().filter(|x| idx.find(x).is_some_and(|b| b.key == a.key))).cloned().collect()).unwrap_or_default();
    drop(idx);
    if !names.iter().any(|n| n.to_lowercase() == q.name.to_lowercase()) {
        names.push(q.name.clone());
    }
    let lower: Vec<Value> = names.iter().map(|n| json!(n.to_lowercase())).collect();
    let m = encore_library::artists::marks(lower.len());
    let mut params = lower.clone();
    params.extend(lower.iter().cloned());
    let rows = st.db.query(&format!(
        "SELECT id, path FROM tracks WHERE lower(albumartist) IN ({m}) OR (lower(artist) IN ({m}) AND (albumartist IS NULL OR albumartist='' OR albumartist='Unknown Artist'))"),
        params,
    )?;
    let ids: Vec<i64> = rows.iter().map(|r| r.i("id")).collect();
    delete_tracks(st, &ids, None);
    let stop = st.cfg.str("library_path");
    for r in &rows {
        if let Some(d) = FsPath::new(&r.s("path")).parent() {
            prune_empty(d, FsPath::new(&stop));
        }
    }
    st.db.execute(&format!("DELETE FROM follows WHERE lower(name) IN ({m})"), lower.clone())?;
    let open: Vec<i64> = st.db.query(&format!("SELECT id FROM requests WHERE lower(artist) IN ({m}) AND status IN ('pending','queued','waiting')"), lower)?
        .iter().map(|r| r.i("id")).collect();
    for id in &open {
        st.db.execute("UPDATE requests SET status='cancelled', message='Artist removed', updated_at=? WHERE id=?", p![now(), id])?;
    }
    st.library.artists.invalidate();
    // the artist index made again now, by the delete, instead of by the next page someone opens
    let _ = st.library.artists.index(&st.db);
    st.db.log("info", &format!("{} removed artist {} ({} songs)", u.s("username"), q.name, rows.len()));
    Ok((Json(json!({ "ok": true, "removed": rows.len() })), open))
}

/// `POST /api/library/verify-manual` body.
#[derive(Deserialize)]
struct ManualVerify {
    /// the songs
    track_ids: Vec<i64>,
    /// right (default) or wrong
    #[serde(default = "yes")]
    correct: bool,
}
/// The default for flags that are on: true.
fn yes() -> bool {
    true
}

/// "This is the right song" – overrides the fingerprint check (e.g. matched to Deezer, not MusicBrainz).
async fn verify_manual(State(st): State<AppState>, User(u): User, Json(b): Json<ManualVerify>) -> ApiResult<Json<Value>> {
    for id in b.track_ids {
        let Some(t) = st.db.one("SELECT request_id, added_by, added_via FROM tracks WHERE id=?", p![id])? else { continue };
        if !encore_config::perms::is_admin(&u) && !owns_song(&st, &u, &t) {
            return Err(err(403, "You can only confirm songs you requested"));
        }
        if b.correct {
            st.db.execute("UPDATE tracks SET verify_status='manual', verify_detail=?, verified_at=? WHERE id=?", p![format!("Confirmed by {}", u.s("username")), now(), id])?;
        } else {
            st.db.execute("UPDATE tracks SET verify_status=NULL, verify_detail=NULL WHERE id=?", p![id])?;
        }
    }
    Ok(Json(json!({ "ok": true })))
}

/// GET /api/library/mismatches: the songs the fingerprint check calls wrong (admins).
async fn mismatches(State(st): State<AppState>, Admin(u): Admin) -> ApiResult<Json<Value>> {
    let mut v = json!(st.db.query(
        "SELECT id, title, artist, album, album_id, verify_detail, format, bitrate, duration FROM tracks WHERE verify_status='mismatch' ORDER BY lower(artist), lower(album)", p![])?);
    enrich(&st, Some(&u), &mut v).await;
    Ok(Json(v))
}

// ---- likes ------------------------------------------------------------------------------------------

/// The user's Liked Songs playlist, made on first use.
fn liked_playlist(st: &AppState, uid: i64) -> ApiResult<i64> {
    if let Some(r) = st.db.one("SELECT id FROM playlists WHERE user_id=? AND kind='liked'", p![uid])? {
        return Ok(r.i("id"));
    }
    Ok(st.db.execute("INSERT INTO playlists (user_id, name, kind, source, sync_targets, created_at, updated_at) VALUES (?, 'Liked Songs', 'liked', 'manual', '[]', ?, ?)", p![uid, now(), now()])?)
}

/// GET /api/likes: the user's liked songs.
async fn likes(State(st): State<AppState>, User(u): User) -> ApiResult<Json<Value>> {
    st.blocking(move |st| {
        let pid = liked_playlist(st, u.i("id"))?;
        Ok(Json(json!(st.db.query("SELECT track_id FROM playlist_items WHERE playlist_id=? AND track_id IS NOT NULL", p![pid])?.iter().map(|r| r.i("track_id")).collect::<Vec<_>>())))
    }).await
}

/// POST /api/likes/{id}: like a song.
async fn like(State(st): State<AppState>, User(u): User, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    let t = st.db.one("SELECT title, artist, album, duration, isrc, mb_trackid FROM tracks WHERE id=?", p![id])?.ok_or_else(|| err(404, "Track not found"))?;
    let pid = liked_playlist(&st, u.i("id"))?;
    if st.db.one("SELECT 1 FROM playlist_items WHERE playlist_id=? AND track_id=?", p![pid, id])?.is_none() {
        let pos = st.db.one("SELECT COALESCE(MAX(position)+1,0) n FROM playlist_items WHERE playlist_id=?", p![pid])?.map(|r| r.i("n")).unwrap_or(1);
        st.db.execute("INSERT INTO playlist_items (playlist_id, position, title, artist, album, duration, isrc, mbid, source, track_id, match_reason, added_at) VALUES (?,?,?,?,?,?,?,?,'library',?,'added from library',?)",
            p![pid, pos, t.s("title"), t.s("artist"), t.s("album"), t.f("duration"), t.get("isrc"), t.get("mb_trackid"), id, now()])?;
        st.db.execute("UPDATE playlists SET updated_at=? WHERE id=?", p![now(), pid])?;
        // a like takes back a dislike of the song (the two buttons are opposites)
        super::taste::forget_dislike(&st, u.i("id"), id)?;
        return Ok(Json(json!({ "liked": true, "added": 1 })));
    }
    Ok(Json(json!({ "liked": true, "added": 0 })))
}

/// DELETE /api/likes/{id}: unlike a song.
async fn unlike(State(st): State<AppState>, User(u): User, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    remove_like(&st, u.i("id"), id).await?;
    Ok(Json(json!({ "liked": false })))
}

/// Takes song `id` out of user `uid`'s Liked Songs (nothing when it isn't there): the unlike button, and a
/// dislike (routes::taste), which is the opposite of a like.
pub(super) async fn remove_like(st: &AppState, uid: i64, id: i64) -> ApiResult<()> {
    let pid = liked_playlist(st, uid)?;
    // the song goes and the list is numbered 0, 1, 2 … again in one transaction on the write connection (off
    // the async runtime); only the rows whose position moves are written
    st.db.write(move |c| c.transaction(|c| {
        c.execute("DELETE FROM playlist_items WHERE playlist_id=? AND track_id=?", p![pid, id])?;
        let rows = c.query("SELECT id, position FROM playlist_items WHERE playlist_id=? ORDER BY position, id", p![pid])?;
        for (i, r) in rows.iter().enumerate() {
            if r.get("position").and_then(Value::as_i64) != Some(i as i64) {
                c.execute("UPDATE playlist_items SET position=? WHERE id=?", p![i as i64, r.i("id")])?;
            }
        }
        Ok::<_, rusqlite::Error>(())
    })).await?;
    Ok(())
}

// ---- share links ----------------------------------------------------------------------------------

/// The 404 of a share link that no longer works.
pub(super) fn link_gone() -> encore_core::error::ApiError {
    err(404, "This link doesn't work any more")
}

/// The link for one thing a user shares: one token per user and song / album / artist / playlist.
pub(super) fn share_link(st: &AppState, uid: i64, kind: &str, track_id: i64, r: &str) -> ApiResult<Json<Value>> {
    let found = if kind == "track" {
        st.db.one("SELECT token FROM shares WHERE track_id=? AND user_id=? AND COALESCE(kind,'track')='track'", p![track_id, uid])?
    } else {
        st.db.one("SELECT token FROM shares WHERE kind=? AND ref=? AND user_id=?", p![kind, r, uid])?
    };
    let token = match found {
        Some(row) => row.s("token"),
        None => {
            let t = encore_core::ids::random_token(12);
            st.db.execute("INSERT INTO shares (token, track_id, user_id, created_at, kind, ref) VALUES (?,?,?,?,?,?)",
                p![t, track_id, uid, now(), kind, if kind == "track" { Value::Null } else { json!(r) }])?;
            t
        }
    };
    Ok(Json(share_json(st, &token)))
}

/// A link as the apps get it: `{token, path, url}` (`url` starts with the public URL from Settings, when set).
pub(super) fn share_json(st: &AppState, token: &str) -> Value {
    let base = st.cfg.str("base_url");
    json!({ "token": token, "path": format!("/s/{token}"), "url": format!("{}/s/{token}", base.trim_end_matches('/')) })
}

/// POST /api/library/tracks/{id}/share: a share link for a song.
async fn share(State(st): State<AppState>, User(u): User, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    if st.db.one("SELECT 1 FROM tracks WHERE id=?", p![id])?.is_none() {
        return Err(err(404, "Track not found"));
    }
    share_link(&st, u.i("id"), "track", id, "")
}

/// POST /api/library/albums/{id}/share: a share link for an album.
async fn share_album(State(st): State<AppState>, User(u): User, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    if st.db.one("SELECT 1 FROM tracks WHERE album_id=? LIMIT 1", p![id])?.is_none() {
        return Err(err(404, "Album not found"));
    }
    share_link(&st, u.i("id"), "album", 0, &id)
}

/// `POST /api/library/artists/share` body.
#[derive(Deserialize, Default)]
#[serde(default)]
struct ShareArtistB {
    /// the artist's name as the library shows it
    name: String,
}

/// An artist link names the artist as the library shows it (any spelling or alias finds it).
async fn share_artist(State(st): State<AppState>, User(u): User, Json(b): Json<ShareArtistB>) -> ApiResult<Json<Value>> {
    let name = b.name.trim();
    if name.is_empty() {
        return Err(err(400, "Which artist?"));
    }
    let name = name.to_string();
    // the artist index (built on the blocking pool when the library changed) and the look-up off the async runtime
    st.blocking(move |st| {
        let display = match st.library.artists.index(&st.db).find(&name) {
            Some(a) if !a.name.is_empty() => a.name.clone(),
            _ => {
                if st.db.one("SELECT 1 FROM tracks WHERE lower(albumartist)=lower(?) OR lower(artist)=lower(?) LIMIT 1", p![name, name])?.is_none() {
                    return Err(err(404, "Artist not found"));
                }
                name
            }
        };
        share_link(st, u.i("id"), "artist", 0, &display)
    }).await
}

/// What a token points at: kind "track" (track_id), "album" (ref = album id), "artist" (ref = name) or
/// "playlist" (ref = playlist id). `user_id` is who made the link.
pub(super) struct Shared {
    /// "track", "album", "artist" or "playlist"
    pub(super) kind: String,
    /// what it shares: the album id, the artist's name or the playlist id
    pub(super) r: String,
    /// the song (track links)
    pub(super) track_id: i64,
    /// who made the link
    pub(super) user_id: i64,
}

/// The share link with this token; 404 when it doesn't work.
pub(super) fn share_row(st: &AppState, token: &str) -> ApiResult<Shared> {
    let s = st.db.one("SELECT track_id, user_id, COALESCE(kind,'track') kind, COALESCE(ref,'') ref FROM shares WHERE token=?", p![token])?.ok_or_else(link_gone)?;
    Ok(Shared { kind: s.s("kind"), r: s.s("ref"), track_id: s.i("track_id"), user_id: s.i("user_id") })
}

/// The song columns a share answer has.
const SHARE_COLS: &str = "id, title, artist, album, albumartist, album_id, year, track, disc, duration, format, bitrate, path";

/// Most songs an artist link lists.
const SHARE_ARTIST_MAX: usize = 100;

/// Songs in one page of a playlist link (`?limit=` asks for up to [`SHARE_PAGE_MAX`]).
const SHARE_PAGE: usize = 200;
/// The most songs a playlist link page may ask for.
const SHARE_PAGE_MAX: usize = 500;

/// A shared playlist as it is now.
pub(super) struct SharedPlaylist {
    /// the playlist's id
    pub(super) id: i64,
    /// "Liked Songs" for the liked list
    pub(super) name: String,
    /// whether it is the owner's Liked Songs
    pub(super) liked: bool,
    /// its description
    pub(super) description: String,
    /// the owner's user name (the page shows it)
    pub(super) owner: String,
}

/// The playlist a link shares, looked at every time: it is still there, still the link maker's, and its
/// owner is not disabled or deleted (the page names the owner, so a link doesn't outlive them). Otherwise
/// the link is gone. Only the owner's own playlists and Liked Songs are shared: someone else's playlist added
/// to theirs (kind `linked`, see [`super::playlist_links`]) never is, even through an old shares row.
pub(super) fn shared_playlist(st: &AppState, s: &Shared) -> ApiResult<SharedPlaylist> {
    let pid: i64 = s.r.parse().map_err(|_| link_gone())?;
    let r = st.db.one(
        "SELECT p.id, p.name, p.kind, COALESCE(p.description,'') description, u.username FROM playlists p JOIN users u ON u.id = p.user_id
         WHERE p.id = ? AND p.user_id = ? AND p.kind IN ('playlist','liked') AND u.disabled = 0", p![pid, s.user_id])?.ok_or_else(link_gone)?;
    let liked = r.s("kind") == "liked";
    Ok(SharedPlaylist { id: r.i("id"), name: if liked { "Liked Songs".into() } else { r.s("name") }, liked, description: public_description(&r.s("description")), owner: r.s("username") })
}

/// Most characters of a playlist's description a link shows (the page shows two lines).
const SHARE_DESCRIPTION_MAX: usize = 300;

/// A playlist's description as its link shows it: plain text on one line (the tags of a service's HTML
/// description, such as Spotify's `<a href=spotify:…>`, taken out), at most [`SHARE_DESCRIPTION_MAX`]
/// characters. The note older Spotify imports left there for the owner is not shown.
pub(crate) fn public_description(d: &str) -> String {
    static TAG: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| regex::Regex::new(r"</?[A-Za-z][^<>]*>").unwrap());
    let text = TAG.replace_all(d, " ").split_whitespace().collect::<Vec<_>>().join(" ");
    if text.starts_with("Imported without Spotify login") {
        return String::new();
    }
    match text.char_indices().nth(SHARE_DESCRIPTION_MAX) {
        Some((cut, _)) => format!("{}…", text[..cut].trim_end()),
        None => text,
    }
}

/// The order a playlist link lists its songs in: Liked Songs newest first (a like goes to the end of the
/// list and an unlike numbers it again, so the highest position is the latest like), any other playlist in
/// its own order.
pub(super) fn playlist_order(pl: &SharedPlaylist) -> &'static str {
    if pl.liked { "i.position DESC, i.id DESC" } else { "i.position, i.id" }
}

/// The ids of the songs a playlist link lists, in list order (the first `limit`, or all): only items that are
/// in the library, and a song listed twice is there twice.
fn playlist_song_ids(st: &AppState, pl: &SharedPlaylist, limit: Option<usize>) -> ApiResult<Vec<i64>> {
    let limit = limit.map_or(-1, |l| l as i64);
    let rows = st.db.query(&format!("SELECT i.track_id FROM playlist_items i JOIN tracks t ON t.id = i.track_id WHERE i.playlist_id = ? ORDER BY {} LIMIT ?", playlist_order(pl)),
        p![pl.id, limit])?;
    Ok(rows.iter().map(|r| r.i("track_id")).collect())
}

/// `cols` of these songs in this order, like [`rows_by_id`], but a song listed twice comes back twice (a
/// playlist may hold one song twice).
fn rows_in_order(db: &impl encore_db::Query, cols: &str, ids: &[i64]) -> rusqlite::Result<Vec<encore_db::Row>> {
    let mut uniq = ids.to_vec();
    uniq.sort_unstable();
    uniq.dedup();
    let by_id: HashMap<i64, encore_db::Row> = rows_by_id(db, cols, &uniq)?.into_iter().map(|r| (r.i("id"), r)).collect();
    Ok(ids.iter().filter_map(|i| by_id.get(i).cloned()).collect())
}

/// The songs a link plays, in page order: the song; the album's songs; the artist's songs (every
/// spelling, alias and collaboration), most played first, else newest albums first; the first page of a
/// playlist. Only a playlist may have none: it is empty, or none of its songs is in the library.
fn shared_tracks(st: &AppState, s: &Shared) -> ApiResult<Vec<encore_db::Row>> {
    let rows = match s.kind.as_str() {
        "playlist" => {
            let pl = shared_playlist(st, s)?;
            return Ok(rows_in_order(&st.db, SHARE_COLS, &playlist_song_ids(st, &pl, Some(SHARE_PAGE))?)?);
        }
        "album" => st.db.query(&format!("SELECT {SHARE_COLS} FROM tracks WHERE album_id=? ORDER BY disc, track, lower(title) LIMIT 500"), p![s.r])?,
        "artist" => {
            let (cond, params) = st.library.artists.track_filter(&st.db, &s.r);
            st.db.query(&format!(
                "SELECT {SHARE_COLS} FROM tracks LEFT JOIN (SELECT track_id, COUNT(*) n FROM plays WHERE track_id IS NOT NULL GROUP BY track_id) pc ON pc.track_id=tracks.id
                 WHERE {cond} ORDER BY COALESCE(pc.n,0) DESC, year DESC, lower(album), disc, track LIMIT {SHARE_ARTIST_MAX}"), params)?
        }
        _ => st.db.query(&format!("SELECT {SHARE_COLS} FROM tracks WHERE id=?"), p![s.track_id])?,
    };
    if rows.is_empty() {
        return Err(link_gone());
    }
    Ok(rows)
}

/// Whether a song belongs to what the link shares: the song, a song of the album, of the artist, or one the
/// playlist lists now (a song taken out of it, or unliked, stops playing at once).
fn shared_has(st: &AppState, s: &Shared, id: i64) -> ApiResult<bool> {
    Ok(match s.kind.as_str() {
        "album" => st.db.one("SELECT 1 FROM tracks WHERE album_id=? AND id=?", p![s.r, id])?.is_some(),
        "artist" => {
            let (cond, mut params) = st.library.artists.track_filter(&st.db, &s.r);
            params.push(json!(id));
            st.db.one(&format!("SELECT 1 FROM tracks WHERE {cond} AND id=?"), params)?.is_some()
        }
        "playlist" => {
            let pl = shared_playlist(st, s)?;
            st.db.one("SELECT 1 FROM playlist_items i JOIN tracks t ON t.id = i.track_id WHERE i.playlist_id = ? AND i.track_id = ? LIMIT 1", p![pl.id, id])?.is_some()
        }
        _ => id == s.track_id && st.db.one("SELECT 1 FROM tracks WHERE id=?", p![id])?.is_some(),
    })
}

/// The album whose cover stands for a song, album or artist link: the album, the artist's picture as the
/// library shows it (its cover album), or the song's album.
fn shared_cover_album(st: &AppState, s: &Shared, tracks: &[encore_db::Row]) -> String {
    let first = || tracks.first().map(|t| t.s("album_id")).unwrap_or_default();
    match s.kind.as_str() {
        "album" => s.r.clone(),
        "artist" => st.library.artists.index(&st.db).find(&s.r).map(|a| a.cover_album.clone()).filter(|c| !c.is_empty()).unwrap_or_else(first),
        _ => first(),
    }
}

/// What stands for a link in its page and preview: an album's cover, or a playlist's mosaic.
enum LinkCover {
    /// an album's cover (the album id)
    Album(String),
    /// a playlist's mosaic picture
    Mosaic(std::path::PathBuf),
}

/// How many of a playlist's albums are looked at for covers before its picture settles for fewer than four.
const MOSAIC_LOOK: usize = 64;

/// The covers of the first four albums, in list order, among a playlist link's songs that have one:
/// (album id, cover file, cover version).
fn playlist_covers(st: &AppState, pl: &SharedPlaylist) -> ApiResult<Vec<(String, std::path::PathBuf, u64)>> {
    let rows = st.db.query(&format!("SELECT t.album_id FROM playlist_items i JOIN tracks t ON t.id = i.track_id WHERE i.playlist_id = ? ORDER BY {} LIMIT 2000", playlist_order(pl)),
        p![pl.id])?;
    let mut seen = std::collections::HashSet::new();
    let mut out = vec![];
    for aid in rows.iter().map(|r| r.s("album_id")) {
        if out.len() == 4 || seen.len() >= MOSAIC_LOOK {
            break;
        }
        if aid.is_empty() || !seen.insert(aid.clone()) {
            continue;
        }
        if let Some((path, v)) = st.library.covers.get(&st.db, &aid) {
            out.push((aid, path, v));
        }
    }
    Ok(out)
}

/// Whether a shared playlist has a picture (a listed song with a cover): an entry for it asks for
/// `/api/share/<token>/cover` only then, so a list without art draws its letter without a failed request.
pub(super) fn playlist_has_cover(st: &AppState, pl: &SharedPlaylist) -> bool {
    playlist_covers(st, pl).is_ok_and(|c| !c.is_empty())
}

/// A playlist link's picture: a mosaic of the first four album covers in list order, or with fewer, the
/// first of them. None when no listed song has a cover (or nothing is listed). The playlist's own picture
/// from where it was imported (a Spotify address) is never used: the viewer's browser would ask another
/// site for it, and those addresses expire.
fn playlist_cover(st: &AppState, token: &str, pl: &SharedPlaylist) -> ApiResult<Option<LinkCover>> {
    let covers = playlist_covers(st, pl)?;
    if covers.len() == 4 {
        if let Some(m) = playlist_mosaic(st, token, &covers) {
            return Ok(Some(LinkCover::Mosaic(m)));
        }
    }
    Ok(covers.into_iter().next().map(|(aid, _, _)| LinkCover::Album(aid)))
}

/// The mosaic of four covers, made once per set of four (album and cover version) and kept under
/// <config>/cache/share-cover/<token>.<album>-<version>….jpg; this link's older mosaics and those of links
/// that are gone are tidied away before a new one is made. None when one of the four can't be read.
fn playlist_mosaic(st: &AppState, token: &str, covers: &[(String, std::path::PathBuf, u64)]) -> Option<std::path::PathBuf> {
    let dir = st.db.data_dir.join("cache").join("share-cover");
    std::fs::create_dir_all(&dir).ok()?;
    let name = format!("{token}.{}.jpg", covers.iter().map(|(aid, _, v)| format!("{aid}-{v:x}")).collect::<Vec<_>>().join("."));
    let out = dir.join(&name);
    if out.is_file() {
        return Some(out);
    }
    share_video_tidy(st, &dir, token, &name);
    // the 400 px thumbnails (made ahead of time for every album) are plenty for 300 px squares
    let thumbs: Vec<std::path::PathBuf> = covers.iter().map(|(aid, path, v)| encore_media::covers::thumbnail(&st.db, path, aid, *v, 400, false)).collect();
    encore_media::covers::mosaic(&thumbs, &out).then_some(out)
}

/// What stands for a link (see [`LinkCover`]): a playlist's picture, else the album of [`shared_cover_album`].
fn link_cover(st: &AppState, token: &str, s: &Shared, tracks: &[encore_db::Row]) -> ApiResult<Option<LinkCover>> {
    if s.kind == "playlist" {
        return playlist_cover(st, token, &shared_playlist(st, s)?);
    }
    Ok(Some(LinkCover::Album(shared_cover_album(st, s, tracks))))
}

/// The picture of a link's preview video: a playlist's mosaic or cover, else the cover of its album.
fn shared_cover_file(st: &AppState, token: &str, s: &Shared, tracks: &[encore_db::Row]) -> ApiResult<Option<std::path::PathBuf>> {
    Ok(match link_cover(st, token, s, tracks)? {
        Some(LinkCover::Mosaic(path)) => Some(path),
        Some(LinkCover::Album(aid)) => st.library.find_cover(&st.db, &aid),
        None => None,
    })
}

/// `GET /api/share/{token}` of a playlist link: which page of its songs. Song, album and artist links
/// ignore it.
#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct SharePageQ {
    /// the first song of the page (0 = the top)
    offset: i64,
    /// songs in the page: 200 when not given, at most 500
    limit: Option<i64>,
    /// a seed: the whole list in a random order that is the same for the same seed, so the pages of one
    /// shuffle fit together
    shuffle: Option<u64>,
}

/// A playlist link's data: the playlist as it is now, its count and length over the whole list, and one
/// page of its songs (no file paths).
fn playlist_meta(st: &AppState, s: &Shared, q: &SharePageQ) -> ApiResult<Value> {
    use rand::seq::SliceRandom;
    use rand::SeedableRng;
    let pl = shared_playlist(st, s)?;
    let mut ids = playlist_song_ids(st, &pl, None)?;
    let count = ids.len();
    let duration = st.db.one("SELECT COALESCE(SUM(t.duration), 0.0) d FROM playlist_items i JOIN tracks t ON t.id = i.track_id WHERE i.playlist_id = ?", p![pl.id])?
        .map_or(0.0, |r| r.f("d"));
    // the first song in list order is what the link plays first (its stream, the preview's audio)
    let format = match ids.first() {
        Some(id) => st.db.one("SELECT format FROM tracks WHERE id=?", p![id])?.map(|r| r.s("format")).unwrap_or_default(),
        None => String::new(),
    };
    if let Some(seed) = q.shuffle {
        ids.shuffle(&mut rand::rngs::StdRng::seed_from_u64(seed));
    }
    let offset = q.offset.max(0) as usize;
    let limit = q.limit.unwrap_or(SHARE_PAGE as i64).clamp(1, SHARE_PAGE_MAX as i64) as usize;
    let page: Vec<i64> = ids.iter().skip(offset).take(limit).copied().collect();
    let tracks = rows_in_order(&st.db, "id, title, artist, album, track, disc, duration", &page)?;
    Ok(json!({
        "kind": "playlist", "liked": pl.liked, "title": pl.name, "owner": pl.owner, "description": pl.description,
        "count": count, "duration": duration,
        "offset": offset, "limit": limit, "has_more": offset + page.len() < count, "shuffle": q.shuffle,
        "tracks": tracks, "format": format, "app_name": st.cfg.str("app_name"),
    }))
}

/// Everything the share page shows (no file paths), and who made the link. Songs: the song and its
/// quality; albums and artists: title, artist, year and the song list; playlists: see [`playlist_meta`].
async fn shared_meta(st: &AppState, token: &str, q: &SharePageQ) -> ApiResult<(Value, i64)> {
    // the link, its songs and, for an artist, the name the library shows (the artist index): on the blocking
    // pool, since anyone with the link (and every link-preview bot) can ask
    let (token, q) = (token.to_string(), q.clone());
    let (s, tracks, artist_name, playlist) = st.blocking(move |st| {
        let s = share_row(st, &token)?;
        if s.kind == "playlist" {
            let v = playlist_meta(st, &s, &q)?;
            return Ok((s, vec![], None, Some(v)));
        }
        let tracks = shared_tracks(st, &s)?;
        let name = (s.kind == "artist").then(|| st.library.artists.index(&st.db).find(&s.r).map(|a| a.name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| s.r.clone()));
        Ok((s, tracks, name, None))
    }).await?;
    if let Some(v) = playlist {
        return Ok((v, s.user_id));
    }
    let app = st.cfg.str("app_name");
    let first = tracks.first().ok_or_else(link_gone)?;
    if s.kind != "album" && s.kind != "artist" {
        let t = first;
        let path = t.s("path");
        let media = st.media.clone();
        let info = tokio::task::spawn_blocking(move || media.audio_info(&path)).await.unwrap_or(json!({}));
        return Ok((json!({
            "kind": "track",
            "title": t.get("title"), "artist": t.get("artist"), "album": t.get("album"), "albumartist": t.get("albumartist"), "year": t.get("year"), "track": t.get("track"),
            "duration": t.get("duration"), "format": t.get("format"), "bitrate": t.get("bitrate"),
            "sample_rate": info["sample_rate"], "bit_depth": info["bit_depth"], "channels": info["channels"], "lossless": info["lossless"],
            "app_name": app,
        }), s.user_id));
    }
    let list: Vec<Value> = tracks.iter().map(|t| json!({
        "id": t.get("id"), "title": t.get("title"), "artist": t.get("artist"), "album": t.get("album"),
        "track": t.get("track"), "disc": t.get("disc"), "duration": t.get("duration"),
    })).collect();
    let duration: f64 = tracks.iter().map(|t| t.f("duration")).sum();
    let (title, artist, album, year) = if s.kind == "album" {
        let year = tracks.iter().map(|t| t.s("year")).filter(|y| !y.is_empty()).max().unwrap_or_default();
        let aa = first.s("albumartist");
        (first.s("album"), if aa.is_empty() { first.s("artist") } else { aa }, first.s("album"), year)
    } else {
        let name = artist_name.unwrap_or_else(|| s.r.clone());
        (name.clone(), name, String::new(), String::new())
    };
    Ok((json!({
        "kind": s.kind, "title": title, "artist": artist, "album": album, "year": year,
        "duration": duration, "count": list.len(), "tracks": list,
        "format": first.get("format"), "app_name": app,
    }), s.user_id))
}

/// A share link's data, for anyone who has the link: a signed-in listener who has translations on gets
/// theirs too. A playlist link answers one page of its songs ([`SharePageQ`]) and `viewer`: whether the
/// viewer is signed in on this server and made the link (the page offers Add to my playlists and Save a copy
/// to the others), and for a signed-in viewer who didn't make it, whether they added it to their playlists
/// already (`added`, `playlist_id`: their entry). The owner's id is never in the answer.
async fn shared(State(st): State<AppState>, MaybeUser(me): MaybeUser, Path(token): Path<String>, Query(q): Query<SharePageQ>) -> ApiResult<Json<Value>> {
    let (mut v, owner) = shared_meta(&st, &token, &q).await?;
    if v["kind"] == "playlist" {
        v["viewer"] = match &me {
            Some(u) if u.i("id") != owner => {
                let entry = super::playlist_links::entry_for(&st, u.i("id"), &token)?;
                json!({ "signed_in": true, "own": false, "added": entry.is_some(), "playlist_id": entry })
            }
            Some(_) => json!({ "signed_in": true, "own": true }),
            None => json!({ "signed_in": false, "own": false }),
        };
    }
    enrich(&st, me.as_ref(), &mut v).await;
    Ok(Json(v))
}

/// `GET /api/share/{token}/stream` query.
#[derive(Deserialize, Default)]
#[serde(default)]
struct SharedTrackQ {
    /// album, artist and playlist links: which of the listed songs (default the first)
    track: Option<i64>,
}

/// GET /api/share/{token}/stream: a song of what the link shares, streamed.
async fn shared_stream(State(st): State<AppState>, Path(token): Path<String>, Query(q): Query<StreamQ>, Query(w): Query<SharedTrackQ>, req: Request) -> ApiResult<Response> {
    // only songs of what the link shares (checked on its own: the artist list changes with plays and a
    // playlist with its owner, so a song a page shows may have left it since); an artist link reads the
    // artist index, so on the blocking pool
    let id = st.blocking(move |st| {
        let s = share_row(st, &token)?;
        match w.track {
            Some(id) if shared_has(st, &s, id)? => Ok(id),
            Some(_) => Err(err(404, "Track not found")),
            None => shared_tracks(st, &s)?.first().map(|t| t.i("id")).ok_or_else(|| err(404, "Track not found")),
        }
    }).await?;
    // share links play the file (or a plain conversion) – never server processing
    stream_track(&st, id, &q, req, None).await
}

/// `Cache-Control` of a playlist link's picture and preview video. The playlist changes under the same URL and
/// Stop sharing ends the link at once, so no shared cache (a CDN, a proxy) keeps them, and the browser asks
/// again each time (a 304 while the file is the same). Song, album and artist links never change.
const SHARE_LIVE_CACHE: &str = "private, no-cache";

/// A playlist link's mosaic or preview video ([`SHARE_LIVE_CACHE`]). The ETag stands for the file's name,
/// which says which covers, song and versions it holds.
async fn share_live_file(path: &std::path::Path, req: Request, content_type: &str) -> ApiResult<Response> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let etag = format!("\"{}\"", &encore_core::ids::sha256_hex(&name)[..20]);
    let mut r = serve_tagged(path, req, Some(content_type), &etag).await?;
    r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static(SHARE_LIVE_CACHE));
    Ok(r)
}

/// GET /api/share/{token}/cover: the cover of what the link shares.
async fn shared_cover(State(st): State<AppState>, Path(token): Path<String>, Query(q): Query<CoverQ>, req: Request) -> ApiResult<Response> {
    let (cover, live) = st.blocking(move |st| {
        let s = share_row(st, &token)?;
        // a playlist's picture comes from its own list (an empty one is not gone, it has no picture)
        let live = s.kind == "playlist";
        let tracks = if live { vec![] } else { shared_tracks(st, &s)? };
        Ok((link_cover(st, &token, &s, &tracks)?.ok_or_else(|| err(404, "No cover"))?, live))
    }).await?;
    match cover {
        LinkCover::Album(album) => {
            let mut r = cover_response(&st, &album, q.s, &q.v, req).await?;
            if live {
                r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static(SHARE_LIVE_CACHE));
            }
            Ok(r)
        }
        // always 600x600 (`s` is not looked at)
        LinkCover::Mosaic(path) => share_live_file(&path, req, "image/jpeg").await,
    }
}

/// Save a copy: a signed-in listener on this server keeps a playlist link as a playlist of their own, with
/// every song of the link in its order (not shuffled) at this moment; it doesn't follow the link afterwards.
/// Liked Songs is saved as "Liked Songs · <owner>", never into the viewer's own Liked Songs. Body `{}`, or
/// `{"name": …}` for another name.
async fn shared_copy(State(st): State<AppState>, User(u): User, Path(token): Path<String>, body: Option<Json<Value>>) -> ApiResult<Json<Value>> {
    let name = body.and_then(|Json(b)| b["name"].as_str().map(|n| n.trim().to_string())).filter(|n| !n.is_empty());
    st.blocking(move |st| {
        let s = share_row(st, &token)?;
        if s.kind != "playlist" {
            return Err(err(400, "Only playlists can be copied"));
        }
        let pl = shared_playlist(st, &s)?;
        copy_link(st, &u, &pl, name).map(Json)
    }).await
}

/// Save a copy of a shared playlist as it is now (from its link, or from the viewer's entry for it): a new
/// playlist of `u`'s with every listed song in the link's order, described "Shared by <owner>". Liked Songs is
/// named "Liked Songs · <owner>" unless `name` says otherwise. → `{id, name, added, duplicates_skipped}`
pub(super) fn copy_link(st: &AppState, u: &encore_db::Row, pl: &SharedPlaylist, name: Option<String>) -> ApiResult<Value> {
    let ids = playlist_song_ids(st, pl, None)?;
    let name = name.unwrap_or_else(|| super::playlist_links::link_playlist_name(pl));
    let bad = |e: anyhow::Error| err(400, e.to_string());
    let made = encore_sync::playlists::create(st, u.i("id"), &name, &[], "library", None, &format!("Shared by {}", pl.owner), None, false).map_err(bad)?;
    let pid = made["id"].as_i64().unwrap_or(0);
    let (added, dupes) = encore_sync::playlists::add_library_tracks(st, pid, &ids).map_err(bad)?;
    st.db.log("info", &format!("{} saved a copy of {}'s playlist '{}' ({added} songs)", u.s("username"), pl.owner, pl.name));
    Ok(json!({ "id": pid, "name": name, "added": added, "duplicates_skipped": dupes }))
}

/// Largest preview video: chat apps (Discord) embed files up to 8 MB.
const SHARE_VIDEO_MAX: u64 = 8 * 1000 * 1000;

/// AAC bitrate (kbps) and length (seconds) of the preview video: the whole song at 128 kbps when it fits
/// a 7.5 MB budget (room for the still picture and the container), else 96 kbps, else cut at the budget.
pub(crate) fn share_video_budget(duration: f64) -> (u32, f64) {
    const BUDGET_BITS: f64 = 7_500_000.0 * 8.0;
    const VIDEO_KBPS: f64 = 48.0;
    let d = if duration > 0.0 { duration } else { f64::MAX };
    for kbps in [128u32, 96] {
        if d * (kbps as f64 + VIDEO_KBPS) * 1000.0 <= BUDGET_BITS {
            return (kbps, d);
        }
    }
    (96, BUDGET_BITS / ((96.0 + VIDEO_KBPS) * 1000.0))
}

/// How long a failed build is remembered before it is tried again.
const SHARE_VIDEO_RETRY: std::time::Duration = std::time::Duration::from_secs(3600);

/// "mtime-size" of a file, "0" when it isn't there.
fn file_version(p: &std::path::Path) -> String {
    std::fs::metadata(p).ok().map(|m| {
        let t = m.modified().ok().and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
        format!("{t}-{}", m.len())
    }).unwrap_or_else(|| "0".into())
}

/// The "playable" link preview: the cover as a 600x600 still and the first listed song as AAC, in an
/// MP4 that starts playing before it has fully loaded. Cached under <config>/cache/share-video/ by
/// token, song, file and cover version; None when ffmpeg can't make it (remembered for an hour). The
/// build runs in its own task, so a caller that stops waiting (a chat app that gave up) doesn't stop it.
async fn share_video(st: &AppState, token: &str) -> Option<std::path::PathBuf> {
    let (st, token) = (st.clone(), token.to_string());
    tokio::spawn(async move { share_video_build(&st, &token).await }).await.ok().flatten()
}

/// Build the link's preview video: its songs, its cover and the first song's audio.
async fn share_video_build(st: &AppState, token: &str) -> Option<std::path::PathBuf> {
    // the link's songs, its cover album (an artist link reads the artist index) or a playlist's picture, and
    // the cover file, on the blocking pool
    let tk = token.to_string();
    let (tracks, cover) = st.blocking(move |st| {
        let s = share_row(st, &tk)?;
        let tracks = shared_tracks(st, &s)?;
        let cover = shared_cover_file(st, &tk, &s, &tracks)?;
        Ok((tracks, cover))
    }).await.ok()?;
    // (an empty playlist has no song to play)
    let t = tracks.first()?;
    let audio = t.s("path");
    let meta = std::fs::metadata(&audio).ok()?;
    let mtime = meta.modified().ok().and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
    let cover_v = cover.as_deref().map(file_version).unwrap_or_else(|| "none".into());
    let dir = st.db.data_dir.join("cache").join("share-video");
    std::fs::create_dir_all(&dir).ok()?;
    let stem = format!("{token}.{}.{mtime}.{}.{cover_v}", t.i("id"), meta.len());
    let out = dir.join(format!("{stem}.mp4"));
    let failed = dir.join(format!("{stem}.fail"));
    let failed_lately = || std::fs::metadata(&failed).ok().and_then(|m| m.modified().ok()).and_then(|m| m.elapsed().ok()).is_some_and(|age| age < SHARE_VIDEO_RETRY);
    if out.is_file() {
        return Some(out);
    }
    if failed_lately() {
        return None;
    }
    // one build per link: wait for one that is running, then use what it made
    let lock = {
        let mut g = st.web.share_video_builds.lock().unwrap_or_else(|e| e.into_inner());
        g.entry(token.to_string()).or_default().clone()
    };
    let res = {
        let _one = lock.lock().await;
        if out.is_file() {
            Some(out.clone())
        } else if failed_lately() {
            None
        } else {
            let _slot = st.web.share_video_slots.acquire().await.ok()?;
            share_video_tidy(st, &dir, token, &format!("{stem}.mp4"));
            let made = share_video_make(&dir, cover.as_deref(), &audio, t.f("duration"), &out).await;
            if made.is_none() {
                let _ = std::fs::write(&failed, b"");
            }
            made
        }
    };
    {
        let mut g = st.web.share_video_builds.lock().unwrap_or_else(|e| e.into_inner());
        // nobody else holds it: forget the lock
        if g.get(token).is_some_and(|l| Arc::strong_count(l) <= 2) {
            g.remove(token);
        }
    }
    res
}

/// Before a build of `keep` in `dir` (the preview videos, or the playlist pictures): drops this link's older
/// files (song, file or cover changed), files of links that no longer exist, and half-made files a stopped
/// build left behind.
fn share_video_tidy(st: &AppState, dir: &std::path::Path, token: &str, keep: &str) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut known: HashMap<String, bool> = HashMap::new();
    let prefix = format!("{token}.");
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let gone = if name == keep {
            false
        } else if name.starts_with('.') {
            name.contains(".tmp.") && e.metadata().ok().and_then(|m| m.modified().ok()).and_then(|m| m.elapsed().ok()).is_some_and(|a| a.as_secs() > 900)
        } else if name.starts_with(&prefix) {
            true
        } else {
            let tok = name.split('.').next().unwrap_or("").to_string();
            !*known.entry(tok.clone()).or_insert_with(|| st.db.one("SELECT 1 FROM shares WHERE token=?", p![tok]).ok().flatten().is_some())
        };
        if gone {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// Runs ffmpeg: the cover (or a dark square) and the song into `out`.
/// The picture of a preview video for a link without a cover: the card colour of Encore's own look
/// (`--panel` of `:root[data-theme="encore"]` in web-rs/static/css/app.css; a test keeps the two in step).
pub(crate) const SHARE_VIDEO_BLANK: &str = "#222224";

async fn share_video_make(dir: &std::path::Path, cover: Option<&std::path::Path>, audio: &str, duration: f64, out: &std::path::Path) -> Option<std::path::PathBuf> {
    let (kbps, secs) = share_video_budget(duration);
    let tmp = dir.join(format!(".{}.tmp.mp4", encore_core::ids::random_token(6)));
    // H.264 when this ffmpeg has it (what chat apps play), else ffmpeg's default MP4 video codec
    for vcodec in [Some("libx264"), None] {
        let mut ff = Ffmpeg::new().quiet().arg("-y").silent();
        let cmd = ff.command();
        match cover {
            Some(c) => { cmd.args(["-loop", "1", "-framerate", "1", "-i"]).arg(c); }
            // no cover: a plain square in the panel colour of the "encore" theme (`--panel` in web-rs app.css)
            None => { cmd.args(["-f", "lavfi", "-i", &format!("color=c=0x{}:s=600x600:r=1", SHARE_VIDEO_BLANK.trim_start_matches('#'))]); }
        }
        cmd.arg("-i").arg(audio);
        cmd.args(["-map", "0:v:0", "-map", "1:a:0", "-vf", "scale=600:600:force_original_aspect_ratio=increase,crop=600:600,format=yuv420p", "-r", "1", "-g", "10"]);
        if let Some(v) = vcodec {
            cmd.args(["-c:v", v, "-tune", "stillimage"]);
        }
        cmd.args(["-c:a", "aac", "-b:a", &format!("{kbps}k"), "-ac", "2", "-t", &format!("{secs:.2}"), "-shortest", "-movflags", "+faststart", "-f", "mp4"]).arg(&tmp);
        let ok = tokio::time::timeout(std::time::Duration::from_secs(300), ff.status()).await.ok().and_then(|r| r.ok()).map(|s| s.success()).unwrap_or(false);
        let size = std::fs::metadata(&tmp).map(|m| m.len()).unwrap_or(0);
        if ok && size > 0 && size <= SHARE_VIDEO_MAX && std::fs::rename(&tmp, out).is_ok() {
            return Some(out.to_path_buf());
        }
        let _ = std::fs::remove_file(&tmp);
        if ok {
            break; // made but too big: another codec won't help
        }
    }
    None
}

/// Only in "playable" mode: the other modes never start ffmpeg for a public link.
async fn shared_video(State(st): State<AppState>, Path(token): Path<String>, req: Request) -> ApiResult<Response> {
    let s = share_row(&st, &token)?;
    if st.cfg.str("share_preview").trim() != "playable" {
        return Err(err(404, "No video for this link"));
    }
    let path = share_video(&st, &token).await.ok_or_else(|| err(404, "No video for this link"))?;
    // a playlist's video holds its first song now: never kept where Stop sharing or a removed song can't reach
    if s.kind == "playlist" {
        return share_live_file(&path, req, "video/mp4").await;
    }
    let mut r = serve_file(&path.to_string_lossy(), req, Some("video/mp4"), None).await?;
    r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=3600"));
    Ok(r)
}

/// "FLAC · 16-bit / 44.1 kHz" / "MP3 · 320 kbps" – the one-line quality description shared pages and link previews use.
pub fn quality_line(m: &Value) -> String {
    let fmt = m["format"].as_str().unwrap_or("").to_uppercase();
    let mut parts = vec![];
    if !fmt.is_empty() { parts.push(fmt); }
    if m["lossless"].as_bool().unwrap_or(false) {
        if let (Some(b), Some(r)) = (m["bit_depth"].as_u64(), m["sample_rate"].as_u64()) {
            parts.push(format!("{b}-bit / {} kHz", trim_khz(r)));
        } else if let Some(r) = m["sample_rate"].as_u64() {
            parts.push(format!("{} kHz", trim_khz(r)));
        }
    } else if let Some(k) = m["bitrate"].as_u64().filter(|k| *k > 0) {
        parts.push(format!("{k} kbps"));
    }
    parts.join(" · ")
}

/// Hz as kHz without trailing zeros ("44.1", "48").
fn trim_khz(hz: u64) -> String {
    let k = hz as f64 / 1000.0;
    let s = format!("{k:.1}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// Seconds as "m:ss".
fn mmss(secs: f64) -> String {
    let s = secs.max(0.0).round() as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// Seconds the share page waits for its preview video.
const SHARE_PAGE_VIDEO_WAIT: u64 = 6;

/// The share page, with Open Graph / Twitter tags filled in on the server so chat apps show the song,
/// album, artist or playlist with its cover in the link preview before anyone opens it. The `share_preview`
/// setting picks the card: "small" (small cover), "large" (big picture) or "playable" (a video with
/// the cover and the song, falling back to "small" when it can't be made).
async fn share_page(State(st): State<AppState>, Path(token): Path<String>, req: Request) -> ApiResult<Response> {
    let html = tokio::fs::read_to_string(st.static_dir.join("share.html")).await.map_err(|_| err(404, "Not found"))?;
    let meta = match shared_meta(&st, &token, &SharePageQ::default()).await {
        Ok((m, _)) => m,
        Err(_) => return Ok(([(header::CONTENT_TYPE, "text/html; charset=utf-8")], html.replace("<!--META-->", "")).into_response()),
    };
    // absolute URLs for the preview image and audio: the public URL from Settings, else what the browser asked for
    let base = {
        let b = st.cfg.str("base_url");
        if !b.trim().is_empty() { b.trim_end_matches('/').to_string() } else {
            let h = req.headers();
            let host = h.get("x-forwarded-host").or(h.get(header::HOST)).and_then(|v| v.to_str().ok()).unwrap_or("localhost");
            let proto = h.get("x-forwarded-proto").and_then(|v| v.to_str().ok()).unwrap_or(if host.starts_with("localhost") || host.starts_with("127.") { "http" } else { "https" });
            format!("{proto}://{host}")
        }
    };
    let mode = st.cfg.str("share_preview");
    // a chat app waits a few seconds for the page: past that, the small card now and the video (still
    // being made in the background) next time
    let video = mode.trim() == "playable"
        && tokio::time::timeout(std::time::Duration::from_secs(SHARE_PAGE_VIDEO_WAIT), share_video(&st, &token)).await.ok().flatten().is_some();
    let discord = req.headers().get(header::USER_AGENT).and_then(|v| v.to_str().ok()).is_some_and(is_discord_bot);
    Ok(([(header::CONTENT_TYPE, "text/html; charset=utf-8"), (header::CACHE_CONTROL, "no-cache"), (header::VARY, "User-Agent")],
        share_page_html(&html, &meta, &base, &token, mode.trim(), video, discord)).into_response())
}

/// Discord's link-preview crawler. Given `og:audio` or a `music.*` type it builds an "audio" embed that its
/// Android app can't read ("EmbedType does not contain element with name 'audio'"), and the whole chat crashes.
pub(crate) fn is_discord_bot(ua: &str) -> bool {
    ua.to_ascii_lowercase().contains("discordbot")
}

/// The page with its preview tags (see `share_page`); `video`: the preview video exists; `discord`: for
/// Discord's crawler, a plain picture (or video) card with no audio or music tags (see `is_discord_bot`).
pub(crate) fn share_page_html(html: &str, meta: &Value, base: &str, token: &str, mode: &str, video: bool, discord: bool) -> String {
    use encore_core::text::html_escape as esc;
    let e = |v: &Value| esc(v.as_str().unwrap_or(""));
    let kind = meta["kind"].as_str().unwrap_or("track");
    let ystr = match &meta["year"] { Value::String(s) => s.clone(), Value::Number(n) => n.to_string(), _ => String::new() };
    let title = match kind {
        "artist" => e(&meta["title"]),
        // "Liked Songs · anna", "Road trip · anna"
        "playlist" => format!("{} · {}", e(&meta["title"]), e(&meta["owner"])),
        _ => format!("{} – {}", e(&meta["title"]), e(&meta["artist"])),
    };
    let mut desc = vec![];
    match kind {
        // a playlist's length is left out: it can be hundreds of hours
        "album" | "artist" | "playlist" => {
            if kind == "album" && !ystr.is_empty() { desc.push(esc(&ystr)); }
            if kind == "artist" { desc.push("Artist".to_string()); }
            let n = meta["count"].as_u64().unwrap_or(0);
            desc.push(format!("{n} {}", if n == 1 { "song" } else { "songs" }));
            if kind == "album" {
                if let Some(d) = meta["duration"].as_f64().filter(|d| *d > 0.0) { desc.push(mmss(d)); }
            }
        }
        _ => {
            if let Some(a) = meta["album"].as_str().filter(|a| !a.is_empty()) {
                desc.push(if ystr.is_empty() { esc(a) } else { format!("{} ({})", esc(a), esc(&ystr)) });
            }
            let q = quality_line(meta);
            if !q.is_empty() { desc.push(q); }
            if let Some(d) = meta["duration"].as_f64().filter(|d| *d > 0.0) { desc.push(mmss(d)); }
        }
    }
    desc.push(format!("Shared from {}", e(&meta["app_name"])));
    let desc = desc.join(" · ");
    let tok = esc(token);
    let base = esc(base);
    let app = e(&meta["app_name"]);
    let og_type = if video { "video.other" } else if discord { "website" } else { match kind { "album" => "music.album", "artist" => "profile", "playlist" => "music.playlist", _ => "music.song" } };
    let card = if video { "player" } else if mode == "large" { "summary_large_image" } else { "summary" };
    let mime = browser_type(meta["format"].as_str().unwrap_or("")).unwrap_or("audio/flac");
    // a playlist with no song in the library has nothing to play and no picture
    let empty = kind == "playlist" && meta["count"].as_u64().unwrap_or(0) == 0;
    let mut tags = format!(
        r#"<title>{title}</title>
<meta property="og:type" content="{og_type}">
<meta property="og:site_name" content="{app}">
<meta property="og:title" content="{title}">
<meta property="og:description" content="{desc}">
<meta property="og:url" content="{base}/s/{tok}">
"#);
    if !empty {
        tags.push_str(&format!(
            r#"<meta property="og:image" content="{base}/api/share/{tok}/cover?s=600">
<meta property="og:image:width" content="600"><meta property="og:image:height" content="600">
"#));
        if !discord {
            tags.push_str(&format!(
                r#"<meta property="og:audio" content="{base}/api/share/{tok}/stream">
<meta property="og:audio:type" content="{mime}">
"#));
        }
    }
    match kind {
        _ if discord => {}
        "album" => tags.push_str(&format!("<meta property=\"music:musician\" content=\"{}\">\n", e(&meta["artist"]))),
        "artist" | "playlist" => {}
        _ => tags.push_str(&format!(
            "<meta property=\"music:musician\" content=\"{}\">\n<meta property=\"music:album\" content=\"{}\">\n<meta property=\"music:duration\" content=\"{}\">\n",
            e(&meta["artist"]), e(&meta["album"]), meta["duration"].as_f64().unwrap_or(0.0).round() as u64)),
    }
    if video {
        tags.push_str(&format!(
            r#"<meta property="og:video" content="{base}/api/share/{tok}/video.mp4">
<meta property="og:video:secure_url" content="{base}/api/share/{tok}/video.mp4">
<meta property="og:video:type" content="video/mp4">
<meta property="og:video:width" content="600"><meta property="og:video:height" content="600">
<meta name="twitter:player" content="{base}/s/{tok}">
<meta name="twitter:player:width" content="600"><meta name="twitter:player:height" content="600">
<meta name="twitter:player:stream" content="{base}/api/share/{tok}/video.mp4">
<meta name="twitter:player:stream:content_type" content="video/mp4">
"#));
    }
    tags.push_str(&format!(
        r#"<meta name="twitter:card" content="{card}">
<meta name="twitter:title" content="{title}">
<meta name="twitter:description" content="{desc}">
"#));
    if !empty {
        tags.push_str(&format!("<meta name=\"twitter:image\" content=\"{base}/api/share/{tok}/cover?s=600\">\n"));
    }
    tags.push_str(&format!(r#"<meta name="description" content="{desc}">"#));
    html.replace("<title>Shared song</title>", "").replace("<!--META-->", &tags)
}

#[cfg(test)]
#[path = "share_tests.rs"]
mod share_tests;

#[cfg(test)]
#[path = "stream_tests.rs"]
mod stream_tests;

#[cfg(test)]
#[path = "library_plays_tests.rs"]
mod library_plays_tests;

#[cfg(test)]
#[path = "library_lyrics_shift_tests.rs"]
mod library_lyrics_shift_tests;
