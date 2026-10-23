//! /api/library tagging tools: match an album to a release (any provider), cover art, lyrics, fix a song,
//! organize, bulk auto-tag, verify by audio, duplicates.
use crate::enrich::enrich;
use crate::auth::{Admin, User};
use encore_db::{now, RowExt};
use encore_core::error::{err, ApiResult};
use crate::state::AppState;
use encore_library::acoustid;
use encore_library::dupes;
use encore_library::lyrics;
use encore_library::meta;
use encore_library::organize;
use encore_db::p;
use encore_library::tagger;
use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::path::PathBuf;

/// The tagging, artwork, organizing, verification and duplicate routes (mostly for admins).
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/library/albums/{id}/match", get(match_album))
        .route("/api/library/albums/{id}/preview", get(preview))
        .route("/api/library/albums/{id}/retag", post(retag))
        .route("/api/library/albums/{id}/art-candidates", get(art_candidates))
        .route("/api/library/albums/{id}/art", post(set_art))
        .route("/api/library/albums/{id}/artwork", post(album_artwork))
        .route("/api/library/albums/{id}/lyrics", post(album_lyrics))
        .route("/api/library/artwork", post(missing_artwork))
        .route("/api/library/tags", post(edit_tags))
        .route("/api/library/autotag", post(autotag))
        .route("/api/library/organize", post(organize_files))
        .route("/api/library/tracks/{id}/identify", get(identify))
        .route("/api/library/tracks/{id}/fix", post(fix_track))
        .route("/api/library/verify", post(verify_all))
        .route("/api/library/duplicates", get(duplicates))
        .route("/api/library/duplicates/resolve", post(resolve))
        .route("/api/library/duplicates/resolve-all", post(resolve_all))
        .route("/api/library/duplicates/ignore", post(ignore))
}

/// An album's files in disc and track order; 404 when it has none.
fn album_paths(st: &AppState, id: &str) -> ApiResult<Vec<PathBuf>> {
    let paths: Vec<PathBuf> = st.db.query("SELECT path FROM tracks WHERE album_id=? ORDER BY disc, track, path", p![id])?.iter().map(|r| PathBuf::from(r.s("path"))).collect();
    if paths.is_empty() {
        return Err(err(404, "Album not found"));
    }
    Ok(paths)
}

/// The album id of the song at `path`, or null.
fn album_of(st: &AppState, path: &std::path::Path) -> Value {
    st.db.one("SELECT album_id FROM tracks WHERE path=?", p![path.to_string_lossy()]).ok().flatten().map(|r| json!(r.s("album_id"))).unwrap_or(Value::Null)
}

/// You picked this release yourself: that's your confirmation, so the fingerprint check stops calling it wrong.
fn mark_manual(st: &AppState, paths: &[PathBuf], who: &str) {
    for pth in paths {
        let _ = st.db.execute("UPDATE tracks SET verify_status='manual', verify_detail=?, verified_at=? WHERE path=?", p![format!("Confirmed by {who}"), now(), pth.to_string_lossy()]);
    }
}

/// `GET /api/library/albums/{id}/match` query: other words than the album's tags.
#[derive(Deserialize, Default)]
#[serde(default)]
struct MatchQ {
    /// the artist to search for
    artist: Option<String>,
    /// the album to search for
    album: Option<String>,
    /// the metadata source (default MusicBrainz)
    provider: Option<String>,
}

/// GET /api/library/albums/{id}/match: releases from a metadata source that may be the album.
async fn match_album(State(st): State<AppState>, Admin(viewer): Admin, Path(id): Path<String>, Query(q): Query<MatchQ>) -> ApiResult<Json<Value>> {
    let provider = q.provider.unwrap_or("musicbrainz".into());
    let name = meta::PROVIDERS.iter().find(|(k, _)| *k == provider).map(|x| x.1).ok_or_else(|| err(400, "Unknown metadata source"))?;
    let tracks = st.db.query("SELECT path, albumartist, album FROM tracks WHERE album_id=? ORDER BY disc, track, path", p![id])?;
    let first = tracks.first().ok_or_else(|| err(404, "Album not found"))?;
    let artist = q.artist.unwrap_or(first.s("albumartist"));
    let album = q.album.unwrap_or_else(|| { let a = first.s("album"); if a.is_empty() { PathBuf::from(first.s("path")).parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into()).unwrap_or_default() } else { a } });
    let paths: Vec<PathBuf> = tracks.iter().map(|t| PathBuf::from(t.s("path"))).collect();
    let n = paths.len();
    let a = if artist == "Unknown Artist" { "" } else { artist.as_str() };
    let mut res = meta::search(&st, &provider, a, &album, n, &paths).await.map_err(|e| err(502, format!("{name} error: {e}")))?;
    for r in res.iter_mut() {
        let tc = r["track_count"].as_i64().filter(|x| *x > 0).unwrap_or(n as i64);
        let score = r["score"].as_f64().unwrap_or(0.0);
        let fit = if tc == n as i64 { 0.5 } else { (0.5 - (tc - n as i64).abs() as f64 * 0.1).max(0.0) };
        r["match"] = json!(((score / 100.0 * 0.5 + fit) * 1000.0).round() / 1000.0);
    }
    res.sort_by(|a, b| b["match"].as_f64().partial_cmp(&a["match"].as_f64()).unwrap_or(std::cmp::Ordering::Equal));
    let mut v = json!({ "artist": artist, "album": album, "track_count": n, "candidates": res, "provider": provider });
    enrich(&st, Some(&viewer), &mut v).await;
    Ok(Json(v))
}

/// `GET /api/library/albums/{id}/preview` query.
#[derive(Deserialize)]
struct PreviewQ {
    /// the release to preview
    release_id: String,
}

/// GET /api/library/albums/{id}/preview: the album's tags (and file names) as a release would make them.
async fn preview(State(st): State<AppState>, Admin(viewer): Admin, Path(id): Path<String>, Query(q): Query<PreviewQ>) -> ApiResult<Json<Value>> {
    let paths = album_paths(&st, &id)?;
    let mut pv = tagger::preview(&st, &paths, &q.release_id, false).await.map_err(|e| err(502, format!("Metadata lookup failed: {e}")))?;
    let lib = format!("{}/", st.cfg.str("library_path").trim_end_matches('/'));
    if let Some(rows) = pv["rows"].as_array_mut() {
        for row in rows.iter_mut() {
            row.as_object_mut().unwrap().remove("values");
            for k in ["path", "new_path"] {
                if let Some(s) = row[k].as_str().and_then(|s| s.strip_prefix(&lib)).map(String::from) {
                    row[k] = json!(s);
                }
            }
        }
    }
    enrich(&st, Some(&viewer), &mut pv).await;
    Ok(Json(pv))
}

/// `POST /api/library/albums/{id}/retag` body.
#[derive(Deserialize)]
struct RetagBody {
    /// the release to tag with
    release_id: String,
    /// also rename the files (default: the setting)
    rename: Option<bool>,
}

/// POST /api/library/albums/{id}/retag: tag the album from a release, confirming it as picked by hand.
async fn retag(State(st): State<AppState>, Admin(u): Admin, Path(id): Path<String>, Json(b): Json<RetagBody>) -> ApiResult<Json<Value>> {
    let paths = tagger::retag_album(&st, &id, &b.release_id, b.rename).await.map_err(|e| err(500, format!("Tagging failed: {e}")))?;
    mark_manual(&st, &paths, &u.s("username"));
    st.library.forget_cover(None);
    Ok(Json(json!({ "ok": true, "album_id": paths.first().map(|p| album_of(&st, p)).unwrap_or(Value::Null) })))
}

/// `GET /api/library/albums/{id}/art-candidates` query.
#[derive(Deserialize, Default)]
#[serde(default)]
struct ArtQ {
    /// an album title to search for instead of the album's
    q: String,
}

/// GET /api/library/albums/{id}/art-candidates: covers to choose from (Cover Art Archive, Deezer, iTunes …).
async fn art_candidates(State(st): State<AppState>, Admin(viewer): Admin, Path(id): Path<String>, Query(q): Query<ArtQ>) -> ApiResult<Json<Value>> {
    let a = st.db.one("SELECT MAX(albumartist) artist, MAX(album) album, MAX(mb_releasegroupid) rg, MAX(mb_albumid) rel FROM tracks WHERE album_id=?", p![id])?
        .filter(|a| !a.s("artist").is_empty()).ok_or_else(|| err(404, "Album not found"))?;
    let album = if q.q.is_empty() { a.s("album") } else { q.q };
    let mut v = json!(meta::art_candidates(&st, &a.s("artist"), &album, Some(&a.s("rg")), Some(&a.s("rel"))).await);
    enrich(&st, Some(&viewer), &mut v).await;
    Ok(Json(v))
}

/// Embed a picture in every file of the album, cache it and save cover.jpg.
fn embed_album_art(st: &AppState, id: &str, data: &[u8]) -> usize {
    let paths: Vec<PathBuf> = st.db.query("SELECT path FROM tracks WHERE album_id=?", p![id]).unwrap_or_default().iter().map(|r| PathBuf::from(r.s("path"))).collect();
    let n = paths.iter().filter(|pth| encore_media::tags::write_tags(pth, &Map::new(), Some(data)).is_ok()).count();
    keep_album_art(st, id, data);
    let dirs: std::collections::HashSet<PathBuf> = paths.iter().filter_map(|p| p.parent().map(|d| d.to_path_buf())).collect();
    if st.cfg.bool("save_cover") && dirs.len() == 1 {
        let _ = std::fs::write(dirs.into_iter().next().unwrap().join("cover.jpg"), data);
    }
    st.library.forget_cover(Some(id));
    // indexed only now: it starts the cover pass, which must find the new picture, not the old one
    tagger::index_paths(st, &paths);
    n
}

/// The album's picture in Encore's own cover store (`<config>/covers/<album>.jpg`), its old thumbnails gone.
fn keep_album_art(st: &AppState, id: &str, data: &[u8]) {
    let covers = st.data_dir().join("covers");
    let _ = std::fs::create_dir_all(&covers);
    let _ = std::fs::write(covers.join(format!("{id}.jpg")), data);
    if let Ok(rd) = std::fs::read_dir(covers.join("thumbs")) {
        for e in rd.flatten() {
            if e.file_name().to_string_lossy().starts_with(&format!("{id}_")) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

/// Cover Art Archive for MusicBrainz-tagged albums, otherwise Deezer/iTunes by artist + album/title.
async fn lookup_art(st: &AppState, id: &str) -> Option<Vec<u8>> {
    let t = st.db.one("SELECT mb_albumid, mb_releasegroupid, albumartist, album, title, (SELECT COUNT(*) FROM tracks WHERE album_id=?) n FROM tracks WHERE album_id=? LIMIT 1", p![id, id]).ok()??;
    if !t.s("mb_albumid").is_empty() || !t.s("mb_releasegroupid").is_empty() {
        if let Some(d) = tagger::fetch_cover(st, Some(&t.s("mb_albumid")), Some(&t.s("mb_releasegroupid"))).await {
            return Some(d);
        }
    }
    let title = t.s("title");
    let album = t.s("album");
    tagger::find_art(st, &t.s("albumartist"), Some(&album), if t.i("n") == 1 { Some(&title) } else { None }, None).await.map(|x| x.0)
}

/// `POST /api/library/albums/{id}/art` body: an image by link, or its data.
#[derive(Deserialize, Default)]
#[serde(default)]
struct ArtBody {
    /// the image's URL
    url: Option<String>,
    /// the image as base64 (a data URL works too)
    data: Option<String>,
}

/// POST /api/library/albums/{id}/art: set an album's cover.
async fn set_art(State(st): State<AppState>, Admin(u): Admin, Path(id): Path<String>, Json(b): Json<ArtBody>) -> ApiResult<Json<Value>> {
    let data = if let Some(d) = b.data.filter(|d| !d.is_empty()) {
        let raw = d.split_once(',').map(|x| x.1).unwrap_or(&d);
        base64::engine::general_purpose::STANDARD.decode(raw.trim()).map_err(|_| err(400, "Invalid image"))?
    } else if let Some(url) = b.url.filter(|u| u.starts_with("http://") || u.starts_with("https://")) {
        let r = st.http.get(&url).send().await.map_err(|e| err(502, e.to_string()))?;
        if !r.status().is_success() {
            return Err(err(502, format!("Could not download the picture ({})", r.status().as_u16())));
        }
        r.bytes().await.map_err(|e| err(502, e.to_string()))?.to_vec()
    } else {
        return Err(err(400, "Choose a picture or upload one"));
    };
    if !(data.starts_with(b"\xff\xd8\xff") || data.starts_with(b"\x89PNG\r\n\x1a\n")) {
        return Err(err(400, "Only JPEG and PNG pictures can be embedded"));
    }
    if data.len() > 15 * 1024 * 1024 {
        return Err(err(400, "Picture is larger than 15 MB"));
    }
    let n = embed_off(&st, &id, data).await;
    st.db.log("info", &format!("{} changed the artwork of album {id}", u.s("username")));
    Ok(Json(json!({ "embedded": n })))
}

/// [`embed_album_art`] on the blocking pool: it writes the tags of every file of the album, the cover files
/// and looks through the thumbnails.
async fn embed_off(st: &AppState, id: &str, data: Vec<u8>) -> usize {
    let id = id.to_string();
    st.blocking(move |st| Ok(embed_album_art(st, &id, &data))).await.unwrap_or(0)
}

/// POST /api/library/albums/{id}/artwork: find the album's cover and embed it.
async fn album_artwork(State(st): State<AppState>, Admin(_): Admin, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    let data = lookup_art(&st, &id).await.ok_or_else(|| err(404, "No artwork found on Cover Art Archive, Deezer or iTunes"))?;
    Ok(Json(json!({ "embedded": embed_off(&st, &id, data).await })))
}

/// Find artwork for every album without a cover (also a scheduled task).
pub async fn find_missing_artwork(st: AppState) -> String {
    if !st.jobs.start("artwork", json!({ "done": 0, "total": 0, "found": 0 })) {
        return "already running".into();
    }
    // every album and whether it has a cover (a file look per album): on the blocking pool
    let missing: Vec<String> = st.blocking(|st| {
        let ids: Vec<String> = st.db.query("SELECT DISTINCT album_id FROM tracks", p![]).unwrap_or_default().iter().map(|r| r.s("album_id")).collect();
        Ok(ids.into_iter().filter(|a| st.library.find_cover(&st.db, a).is_none()).collect())
    }).await.unwrap_or_default();
    st.jobs.set("artwork", json!({ "total": missing.len() }));
    for a in &missing {
        // an album in an extra library folder is kept as it is: its picture goes into Encore's own cover
        // store only, nothing is written into its files or its folder
        let paths = st.db.query("SELECT path FROM tracks WHERE album_id=?", p![a]).unwrap_or_default();
        let kept = paths.iter().any(|r| !encore_library::folders::in_library_path(&st.cfg, std::path::Path::new(&r.s("path"))));
        if let Some(d) = lookup_art(&st, a).await {
            if kept {
                let id = a.clone();
                let _ = st.blocking(move |st| {
                    keep_album_art(st, &id, &d);
                    st.library.forget_cover(Some(&id));
                    st.library.refresh_covers(st.db.clone(), vec![id]);
                    Ok(())
                }).await;
            } else {
                embed_off(&st, a, d).await;
            }
            st.jobs.add("artwork", "found", 1);
        }
        st.jobs.add("artwork", "done", 1);
    }
    let msg = format!("found {} of {} missing covers", st.jobs.get("artwork")["found"], missing.len());
    if !missing.is_empty() {
        st.db.log("info", &format!("Artwork: {msg}"));
    }
    st.jobs.set("artwork", json!({ "running": false }));
    msg
}

/// POST /api/library/artwork: find covers for every album without one, in the background.
async fn missing_artwork(State(st): State<AppState>, Admin(_): Admin) -> ApiResult<Json<Value>> {
    if st.jobs.running("artwork") {
        return Err(err(409, "Already running"));
    }
    tokio::spawn(find_missing_artwork(st));
    Ok(Json(json!({ "ok": true })))
}

/// POST /api/library/albums/{id}/lyrics: fetch the album's missing lyrics.
async fn album_lyrics(State(st): State<AppState>, User(_): User, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    let ids: Vec<i64> = st.db.query("SELECT id FROM tracks WHERE album_id=?", p![id])?.iter().map(|r| r.i("id")).collect();
    Ok(Json(lyrics::fetch_missing(&st, Some(&ids), ids.len().max(1)).await))
}

/// `POST /api/library/tags` body.
#[derive(Deserialize)]
struct TagEdit {
    /// the songs to edit
    track_ids: Vec<i64>,
    /// the tags to set, by name
    values: Map<String, Value>,
    /// move the files to match the new names (naming template)
    #[serde(default)]
    rename: bool,
    /// keep automatic tagging away from this album afterwards (default on)
    #[serde(default = "yes")]
    lock: bool,
}

/// POST /api/library/tags: edit songs' tags (and with `rename`, move their files).
async fn edit_tags(State(st): State<AppState>, Admin(_): Admin, Json(b): Json<TagEdit>) -> ApiResult<Json<Value>> {
    // the albums the songs were in: when every song of one was edited into one new album, its type (the owner's
    // choice) follows it
    let before = album_ids_of(&st, &b.track_ids);
    let mut paths = tagger::edit_tags(&st, &b.track_ids, &b.values).map_err(|e| err(500, format!("Could not write tags: {e}")))?;
    if b.lock {
        for p in &paths {
            let t = encore_media::tags::read_tags(&p.to_string_lossy());
            tagger::mark_manual(&st, &t.s("albumartist"), &t.s("album"));
        }
    }
    if b.rename {
        paths = tagger::rename_to_tags(&st, &paths).await;
    }
    tagger::index_paths(&st, &paths);
    for old in before {
        encore_library::album_types::carry(&st.db, &old, &b.track_ids, false);
    }
    Ok(Json(json!({ "ok": true, "album_id": paths.first().map(|p| album_of(&st, p)).unwrap_or(Value::Null) })))
}

/// The albums these songs are in.
fn album_ids_of(st: &AppState, ids: &[i64]) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for chunk in ids.chunks(500) {
        let marks = encore_library::artists::marks(chunk.len());
        for r in st.db.query(&format!("SELECT DISTINCT album_id FROM tracks WHERE id IN ({marks})"), chunk.iter().map(|i| json!(i)).collect()).unwrap_or_default() {
            let a = r.s("album_id");
            if !a.is_empty() && !out.contains(&a) {
                out.push(a);
            }
        }
    }
    out
}

/// Match every untagged album to MusicBrainz (only confident matches are applied).
async fn run_bulk(st: AppState) {
    let albums = st.db.query("SELECT album_id, MAX(albumartist) artist, MAX(album) album FROM tracks GROUP BY album_id
        HAVING SUM(CASE WHEN mb_albumid IS NULL OR mb_albumid='' THEN 1 ELSE 0 END) > 0", p![]).unwrap_or_default();
    if !st.jobs.start("bulk", json!({ "done": 0, "total": albums.len(), "tagged": 0, "skipped": 0, "current": "" })) {
        return;
    }
    for a in albums {
        let label = format!("{} – {}", a.s("artist"), a.s("album"));
        st.jobs.set("bulk", json!({ "current": label }));
        let paths: Vec<PathBuf> = st.db.query("SELECT path FROM tracks WHERE album_id=?", p![a.s("album_id")]).unwrap_or_default().iter().map(|r| PathBuf::from(r.s("path"))).collect();
        // fixed by hand, or in an extra library folder (kept where and as it is): left alone
        if tagger::is_manual(&st, &a.s("artist"), &a.s("album")) || paths.iter().any(|p| !encore_library::folders::in_library_path(&st.cfg, p)) {
            st.jobs.add("bulk", "skipped", 1);
            st.jobs.add("bulk", "done", 1);
            continue;
        }
        let artist = if a.s("artist") == "Unknown Artist" { String::new() } else { a.s("artist") };
        let res: anyhow::Result<bool> = async {
            let Some(pv) = tagger::auto_match(&st, &paths, &artist, &a.s("album"), None).await? else { return Ok(false) };
            let ids: Vec<i64> = st.db.query("SELECT id FROM tracks WHERE album_id=?", p![a.s("album_id")])?.iter().map(|r| r.i("id")).collect();
            let new = tagger::apply_preview(&st, &pv, st.cfg.bool("rename_on_import"), None, None).await?;
            let old: Vec<PathBuf> = pv["rows"].as_array().cloned().unwrap_or_default().iter().map(|r| PathBuf::from(r["path"].as_str().unwrap_or(""))).collect();
            tagger::relink(&st, &old, &new);
            tagger::index_paths(&st, &new);
            // the owner's type choice follows the album to its new id; the facts are the release's
            encore_library::album_types::carry(&st.db, &a.s("album_id"), &ids, true);
            tagger::record_release(&st, &pv, &new);
            Ok(true)
        }.await;
        match res {
            Ok(true) => st.jobs.add("bulk", "tagged", 1),
            Ok(false) => st.jobs.add("bulk", "skipped", 1),
            Err(e) => {
                st.jobs.add("bulk", "skipped", 1);
                st.db.log("warning", &format!("Auto-tag failed for {label}: {e}"));
            }
        }
        st.jobs.add("bulk", "done", 1);
    }
    let j = st.jobs.get("bulk");
    st.db.log("info", &format!("Bulk auto-tag finished: {} tagged, {} need manual matching", j["tagged"], j["skipped"]));
    st.jobs.set("bulk", json!({ "running": false, "current": "" }));
}

/// POST /api/library/autotag: tag the whole library from MusicBrainz in the background.
async fn autotag(State(st): State<AppState>, Admin(_): Admin) -> ApiResult<Json<Value>> {
    if st.jobs.running("bulk") {
        return Err(err(409, "Auto-tagging already running"));
    }
    tokio::spawn(run_bulk(st));
    Ok(Json(json!({ "ok": true })))
}

/// `POST /api/library/organize` query.
#[derive(Deserialize)]
struct OrganizeQ {
    /// fill missing tags while organizing (default on)
    #[serde(default = "yes")]
    fill_missing: bool,
}
/// The default for flags that are on: true.
fn yes() -> bool {
    true
}

/// POST /api/library/organize: move the files to the naming template in the background.
async fn organize_files(State(st): State<AppState>, Admin(_): Admin, Query(q): Query<OrganizeQ>) -> ApiResult<Json<Value>> {
    if st.jobs.running("organize") {
        return Err(err(409, "Already organizing"));
    }
    tokio::spawn(organize::organize_library(st, q.fill_missing));
    Ok(Json(json!({ "ok": true })))
}

/// What this file really is, by its audio (AcoustID) – to fix a wrong match.
async fn identify(State(st): State<AppState>, Admin(viewer): Admin, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    let mut v = identify_answer(State(st.clone()), Admin(viewer.clone()), Path(id)).await?;
    enrich(&st, Some(&viewer), &mut v).await;
    Ok(Json(v))
}

/// The identify answer: recordings AcoustID finds for the song's fingerprint, each once.
async fn identify_answer(State(st): State<AppState>, Admin(_): Admin, Path(id): Path<i64>) -> ApiResult<Value> {
    let t = st.db.one("SELECT path FROM tracks WHERE id=?", p![id])?.ok_or_else(|| err(404, "Track not found"))?;
    if !acoustid::available(&st) {
        return Err(err(400, "Add an AcoustID API key (Settings → Library → Verification & duplicates) to identify songs by their audio"));
    }
    let Some((d, fp)) = acoustid::fingerprint(std::path::Path::new(&t.s("path"))).await else { return Ok(json!([])) };
    let res = encore_http::acoustid::lookup(&st, d, &fp).await.map_err(|e| err(502, e.to_string()))?;
    let mut seen = std::collections::HashSet::new();
    let mut out = vec![];
    for r in &res {
        for rec in r["recordings"].as_array().cloned().unwrap_or_default() {
            let (title, artist) = (rec["title"].as_str().unwrap_or("").to_string(), rec["artist"].as_str().unwrap_or("").to_string());
            if !title.is_empty() && seen.insert((title.clone(), artist.clone())) {
                out.push(json!({ "title": title, "artist": artist, "album": rec["release_groups"].get(0).cloned().unwrap_or(json!("")),
                    "score": (r["score"].as_f64().unwrap_or(0.0) * 100.0).round(), "mbid": rec["id"] }));
            }
        }
    }
    out.truncate(12);
    Ok(json!(out))
}

/// `POST /api/library/tracks/{id}/fix` body.
#[derive(Deserialize)]
struct TrackFix {
    /// the tags to set
    values: Map<String, Value>,
    #[serde(default = "yes")]
    r#move: bool,
}

/// Correct one song: write its tags, then (optionally) move it to where those tags say it belongs.
/// Never merged into another file – this is how a different version is split out of a "duplicate".
async fn fix_track(State(st): State<AppState>, Admin(u): Admin, Path(id): Path<i64>, Json(b): Json<TrackFix>) -> ApiResult<Json<Value>> {
    let t = st.db.one("SELECT path FROM tracks WHERE id=?", p![id])?.ok_or_else(|| err(404, "Track not found"))?;
    let vals: Map<String, Value> = b.values.into_iter().filter(|(_, v)| !v.is_null())
        .map(|(k, v)| { let s = match v { Value::String(s) => s, x => x.to_string() }; (k, json!(s.trim())) }).collect();
    tagger::edit_tags(&st, &[id], &vals).map_err(|e| err(500, format!("Could not write tags: {e}")))?;
    let old = PathBuf::from(t.s("path"));
    let now_tags = encore_media::tags::read_tags(&old.to_string_lossy());
    tagger::mark_manual(&st, &now_tags.s("albumartist"), &now_tags.s("album"));
    tagger::index_paths(&st, std::slice::from_ref(&old));
    let new = if b.r#move { organize::organize_track(&st, id).await.unwrap_or(old) } else { old };
    st.db.log("info", &format!("{} fixed {} – {}", u.s("username"), vals.get("artist").and_then(|v| v.as_str()).unwrap_or(""), vals.get("title").and_then(|v| v.as_str()).unwrap_or("")));
    Ok(Json(json!({ "ok": true, "album_id": album_of(&st, &new) })))
}

/// `POST /api/library/verify` query.
#[derive(Deserialize, Default)]
#[serde(default)]
struct AllQ {
    /// check every song again (else only those not checked yet)
    all: bool,
}

/// POST /api/library/verify: check the library's songs by fingerprint in the background.
async fn verify_all(State(st): State<AppState>, Admin(_): Admin, Query(q): Query<AllQ>) -> ApiResult<Json<Value>> {
    if !acoustid::available(&st) {
        return Err(err(400, "Add an AcoustID API key in Settings → Library → Verification & duplicates (and make sure fpcalc is installed)"));
    }
    if st.jobs.running("verify") {
        return Err(err(409, "Verification already running"));
    }
    tokio::spawn(dupes::verify_library(st, !q.all));
    Ok(Json(json!({ "ok": true })))
}

/// GET /api/library/duplicates: the groups of duplicate songs.
async fn duplicates(State(st): State<AppState>, Admin(viewer): Admin) -> ApiResult<Json<Value>> {
    let s2 = st.clone();
    let groups = tokio::task::spawn_blocking(move || dupes::find_duplicates(&s2)).await.map_err(|e| err(500, e.to_string()))?;
    let mut v = json!(groups);
    enrich(&st, Some(&viewer), &mut v).await;
    Ok(Json(v))
}

/// `POST /api/library/duplicates/resolve` body.
#[derive(Deserialize)]
struct DupResolve {
    /// the song to keep
    keep: i64,
    /// the songs to delete
    delete: Vec<i64>,
}

/// POST /api/library/duplicates/resolve: keep one song of a group and delete the others.
async fn resolve(State(st): State<AppState>, Admin(u): Admin, Json(b): Json<DupResolve>) -> ApiResult<Json<Value>> {
    if b.delete.contains(&b.keep) {
        return Err(err(400, "Can't keep and delete the same track"));
    }
    let n = encore_library::delete_tracks(&st, &b.delete, Some(b.keep));
    st.db.log("info", &format!("{} removed {n} duplicate file(s)", u.s("username")));
    Ok(Json(json!({ "deleted": n })))
}

/// POST /api/library/duplicates/resolve-all: resolve every group the way it suggests.
async fn resolve_all(State(st): State<AppState>, Admin(u): Admin) -> ApiResult<Json<Value>> {
    let groups = dupes::find_duplicates(&st);
    let mut n = 0;
    for g in &groups {
        let keep = g["keep"].as_i64().unwrap_or(0);
        let del: Vec<i64> = g["tracks"].as_array().cloned().unwrap_or_default().iter().filter_map(|t| t["id"].as_i64()).filter(|i| *i != keep).collect();
        n += encore_library::delete_tracks(&st, &del, Some(keep));
    }
    st.db.log("info", &format!("{} auto-resolved {} duplicate groups ({n} files removed)", u.s("username"), groups.len()));
    Ok(Json(json!({ "groups": groups.len(), "deleted": n })))
}

/// POST /api/library/duplicates/ignore: never show this group (`{key}`) again.
async fn ignore(State(st): State<AppState>, Admin(_): Admin, Json(b): Json<Value>) -> ApiResult<Json<Value>> {
    st.db.execute("INSERT OR IGNORE INTO dup_ignore VALUES (?,?)", p![b["key"].as_str().unwrap_or(""), now()])?;
    Ok(Json(json!({ "ok": true })))
}
