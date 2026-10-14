//! /api/library tagging tools: match an album to a release (any provider), cover art, lyrics, fix a song,
//! organize, bulk auto-tag, verify by audio, duplicates.
use crate::auth::{Admin, User};
use crate::db::{now, RowExt};
use crate::error::{err, ApiResult};
use crate::state::AppState;
use crate::{acoustid, dupes, jobs, library, lyrics, meta, organize, p, tagger};
use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::path::PathBuf;

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
        .route("/api/library/tracks/{id}/verify", post(verify_one))
        .route("/api/library/verify", post(verify_all))
        .route("/api/library/duplicates", get(duplicates))
        .route("/api/library/duplicates/resolve", post(resolve))
        .route("/api/library/duplicates/resolve-all", post(resolve_all))
        .route("/api/library/duplicates/ignore", post(ignore))
}

fn album_paths(st: &AppState, id: &str) -> ApiResult<Vec<PathBuf>> {
    let paths: Vec<PathBuf> = st.db.query("SELECT path FROM tracks WHERE album_id=? ORDER BY disc, track, path", p![id])?.iter().map(|r| PathBuf::from(r.s("path"))).collect();
    if paths.is_empty() {
        return Err(err(404, "Album not found"));
    }
    Ok(paths)
}

fn album_of(st: &AppState, path: &std::path::Path) -> Value {
    st.db.one("SELECT album_id FROM tracks WHERE path=?", p![path.to_string_lossy()]).ok().flatten().map(|r| json!(r.s("album_id"))).unwrap_or(Value::Null)
}

/// You picked this release yourself: that's your confirmation, so the fingerprint check stops calling it wrong.
fn mark_manual(st: &AppState, paths: &[PathBuf], who: &str) {
    for pth in paths {
        let _ = st.db.execute("UPDATE tracks SET verify_status='manual', verify_detail=?, verified_at=? WHERE path=?", p![format!("Confirmed by {who}"), now(), pth.to_string_lossy()]);
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct MatchQ {
    artist: Option<String>,
    album: Option<String>,
    provider: Option<String>,
}

async fn match_album(State(st): State<AppState>, Admin(_): Admin, Path(id): Path<String>, Query(q): Query<MatchQ>) -> ApiResult<Json<Value>> {
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
    Ok(Json(json!({ "artist": artist, "album": album, "track_count": n, "candidates": res, "provider": provider })))
}

#[derive(Deserialize)]
struct PreviewQ {
    release_id: String,
}

async fn preview(State(st): State<AppState>, Admin(_): Admin, Path(id): Path<String>, Query(q): Query<PreviewQ>) -> ApiResult<Json<Value>> {
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
    Ok(Json(pv))
}

#[derive(Deserialize)]
struct RetagBody {
    release_id: String,
    rename: Option<bool>,
}

async fn retag(State(st): State<AppState>, Admin(u): Admin, Path(id): Path<String>, Json(b): Json<RetagBody>) -> ApiResult<Json<Value>> {
    let paths = tagger::retag_album(&st, &id, &b.release_id, b.rename).await.map_err(|e| err(500, format!("Tagging failed: {e}")))?;
    mark_manual(&st, &paths, &u.s("username"));
    st.library.forget_cover(None);
    Ok(Json(json!({ "ok": true, "album_id": paths.first().map(|p| album_of(&st, p)).unwrap_or(Value::Null) })))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ArtQ {
    q: String,
}

async fn art_candidates(State(st): State<AppState>, Admin(_): Admin, Path(id): Path<String>, Query(q): Query<ArtQ>) -> ApiResult<Json<Value>> {
    let a = st.db.one("SELECT MAX(albumartist) artist, MAX(album) album, MAX(mb_releasegroupid) rg, MAX(mb_albumid) rel FROM tracks WHERE album_id=?", p![id])?
        .filter(|a| !a.s("artist").is_empty()).ok_or_else(|| err(404, "Album not found"))?;
    let album = if q.q.is_empty() { a.s("album") } else { q.q };
    Ok(Json(json!(meta::art_candidates(&st, &a.s("artist"), &album, Some(&a.s("rg")), Some(&a.s("rel"))).await)))
}

/// Embed a picture in every file of the album, cache it and save cover.jpg.
fn embed_album_art(st: &AppState, id: &str, data: &[u8]) -> usize {
    let paths: Vec<PathBuf> = st.db.query("SELECT path FROM tracks WHERE album_id=?", p![id]).unwrap_or_default().iter().map(|r| PathBuf::from(r.s("path"))).collect();
    let n = paths.iter().filter(|pth| tagger::write_tags(pth, &Map::new(), Some(data)).is_ok()).count();
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
    let dirs: std::collections::HashSet<PathBuf> = paths.iter().filter_map(|p| p.parent().map(|d| d.to_path_buf())).collect();
    if st.cfg.bool("save_cover") && dirs.len() == 1 {
        let _ = std::fs::write(dirs.into_iter().next().unwrap().join("cover.jpg"), data);
    }
    st.library.forget_cover(Some(id));
    // indexed only now: it starts the cover pass, which must find the new picture, not the old one
    tagger::index_paths(st, &paths);
    n
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

#[derive(Deserialize, Default)]
#[serde(default)]
struct ArtBody {
    url: Option<String>,
    data: Option<String>,
}

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
    let n = embed_album_art(&st, &id, &data);
    st.db.log("info", &format!("{} changed the artwork of album {id}", u.s("username")));
    Ok(Json(json!({ "embedded": n })))
}

async fn album_artwork(State(st): State<AppState>, Admin(_): Admin, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    let data = lookup_art(&st, &id).await.ok_or_else(|| err(404, "No artwork found on Cover Art Archive, Deezer or iTunes"))?;
    Ok(Json(json!({ "embedded": embed_album_art(&st, &id, &data) })))
}

/// Find artwork for every album without a cover (also a scheduled task).
pub async fn find_missing_artwork(st: AppState) -> String {
    if !jobs::start("artwork", json!({ "done": 0, "total": 0, "found": 0 })) {
        return "already running".into();
    }
    let ids: Vec<String> = st.db.query("SELECT DISTINCT album_id FROM tracks", p![]).unwrap_or_default().iter().map(|r| r.s("album_id")).collect();
    let missing: Vec<String> = ids.into_iter().filter(|a| st.library.find_cover(&st.db, a).is_none()).collect();
    jobs::set("artwork", json!({ "total": missing.len() }));
    for a in &missing {
        if let Some(d) = lookup_art(&st, a).await {
            embed_album_art(&st, a, &d);
            jobs::add("artwork", "found", 1);
        }
        jobs::add("artwork", "done", 1);
    }
    let msg = format!("found {} of {} missing covers", jobs::get("artwork")["found"], missing.len());
    if !missing.is_empty() {
        st.db.log("info", &format!("Artwork: {msg}"));
    }
    jobs::set("artwork", json!({ "running": false }));
    msg
}

async fn missing_artwork(State(st): State<AppState>, Admin(_): Admin) -> ApiResult<Json<Value>> {
    if jobs::running("artwork") {
        return Err(err(409, "Already running"));
    }
    tokio::spawn(find_missing_artwork(st));
    Ok(Json(json!({ "ok": true })))
}

async fn album_lyrics(State(st): State<AppState>, User(_): User, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    let ids: Vec<i64> = st.db.query("SELECT id FROM tracks WHERE album_id=?", p![id])?.iter().map(|r| r.i("id")).collect();
    Ok(Json(lyrics::fetch_missing(&st, Some(&ids), ids.len().max(1)).await))
}

#[derive(Deserialize)]
struct TagEdit {
    track_ids: Vec<i64>,
    values: Map<String, Value>,
    /// move the files to match the new names (naming template)
    #[serde(default)]
    rename: bool,
    /// keep automatic tagging away from this album afterwards (default on)
    #[serde(default = "yes")]
    lock: bool,
}

async fn edit_tags(State(st): State<AppState>, Admin(_): Admin, Json(b): Json<TagEdit>) -> ApiResult<Json<Value>> {
    let mut paths = tagger::edit_tags(&st, &b.track_ids, &b.values).map_err(|e| err(500, format!("Could not write tags: {e}")))?;
    if b.lock {
        for p in &paths {
            let t = crate::library::read_tags(&p.to_string_lossy());
            tagger::mark_manual(&st, &t.s("albumartist"), &t.s("album"));
        }
    }
    if b.rename {
        paths = tagger::rename_to_tags(&st, &paths).await;
    }
    tagger::index_paths(&st, &paths);
    Ok(Json(json!({ "ok": true, "album_id": paths.first().map(|p| album_of(&st, p)).unwrap_or(Value::Null) })))
}

/// Match every untagged album to MusicBrainz (only confident matches are applied).
async fn run_bulk(st: AppState) {
    let albums = st.db.query("SELECT album_id, MAX(albumartist) artist, MAX(album) album FROM tracks GROUP BY album_id
        HAVING SUM(CASE WHEN mb_albumid IS NULL OR mb_albumid='' THEN 1 ELSE 0 END) > 0", p![]).unwrap_or_default();
    if !jobs::start("bulk", json!({ "done": 0, "total": albums.len(), "tagged": 0, "skipped": 0, "current": "" })) {
        return;
    }
    for a in albums {
        let label = format!("{} – {}", a.s("artist"), a.s("album"));
        jobs::set("bulk", json!({ "current": label }));
        let paths: Vec<PathBuf> = st.db.query("SELECT path FROM tracks WHERE album_id=?", p![a.s("album_id")]).unwrap_or_default().iter().map(|r| PathBuf::from(r.s("path"))).collect();
        if tagger::is_manual(&st, &a.s("artist"), &a.s("album")) {
            jobs::add("bulk", "skipped", 1);
            continue;
        }
        let artist = if a.s("artist") == "Unknown Artist" { String::new() } else { a.s("artist") };
        let res: anyhow::Result<bool> = async {
            let Some(pv) = tagger::auto_match(&st, &paths, &artist, &a.s("album"), None).await? else { return Ok(false) };
            let new = tagger::apply_preview(&st, &pv, st.cfg.bool("rename_on_import"), None, None).await?;
            let old: Vec<PathBuf> = pv["rows"].as_array().cloned().unwrap_or_default().iter().map(|r| PathBuf::from(r["path"].as_str().unwrap_or(""))).collect();
            tagger::relink(&st, &old, &new);
            tagger::index_paths(&st, &new);
            Ok(true)
        }.await;
        match res {
            Ok(true) => jobs::add("bulk", "tagged", 1),
            Ok(false) => jobs::add("bulk", "skipped", 1),
            Err(e) => {
                jobs::add("bulk", "skipped", 1);
                st.db.log("warning", &format!("Auto-tag failed for {label}: {e}"));
            }
        }
        jobs::add("bulk", "done", 1);
    }
    let j = jobs::get("bulk");
    st.db.log("info", &format!("Bulk auto-tag finished: {} tagged, {} need manual matching", j["tagged"], j["skipped"]));
    jobs::set("bulk", json!({ "running": false, "current": "" }));
}

async fn autotag(State(st): State<AppState>, Admin(_): Admin) -> ApiResult<Json<Value>> {
    if jobs::running("bulk") {
        return Err(err(409, "Auto-tagging already running"));
    }
    tokio::spawn(run_bulk(st));
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct OrganizeQ {
    #[serde(default = "yes")]
    fill_missing: bool,
}
fn yes() -> bool {
    true
}

async fn organize_files(State(st): State<AppState>, Admin(_): Admin, Query(q): Query<OrganizeQ>) -> ApiResult<Json<Value>> {
    if jobs::running("organize") {
        return Err(err(409, "Already organizing"));
    }
    tokio::spawn(organize::organize_library(st, q.fill_missing));
    Ok(Json(json!({ "ok": true })))
}

/// What this file really is, by its audio (AcoustID) – to fix a wrong match.
async fn identify(State(st): State<AppState>, Admin(_): Admin, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    let t = st.db.one("SELECT path FROM tracks WHERE id=?", p![id])?.ok_or_else(|| err(404, "Track not found"))?;
    if !acoustid::available(&st) {
        return Err(err(400, "Add an AcoustID API key (Settings → API keys) to identify songs by their audio"));
    }
    let Some((d, fp)) = acoustid::fingerprint(std::path::Path::new(&t.s("path"))).await else { return Ok(Json(json!([]))) };
    let res = acoustid::lookup(&st, d, &fp).await.map_err(|e| err(502, e.to_string()))?;
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
    Ok(Json(json!(out)))
}

#[derive(Deserialize)]
struct TrackFix {
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
    let now_tags = crate::library::read_tags(&old.to_string_lossy());
    tagger::mark_manual(&st, &now_tags.s("albumartist"), &now_tags.s("album"));
    tagger::index_paths(&st, std::slice::from_ref(&old));
    let new = if b.r#move { organize::organize_track(&st, id).await.unwrap_or(old) } else { old };
    st.db.log("info", &format!("{} fixed {} – {}", u.s("username"), vals.get("artist").and_then(|v| v.as_str()).unwrap_or(""), vals.get("title").and_then(|v| v.as_str()).unwrap_or("")));
    Ok(Json(json!({ "ok": true, "album_id": album_of(&st, &new) })))
}

async fn verify_one(State(st): State<AppState>, Admin(_): Admin, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    let t = st.db.one("SELECT id, path, title, artist, duration, mb_trackid FROM tracks WHERE id=?", p![id])?.ok_or_else(|| err(404, "Track not found"))?;
    if !acoustid::available(&st) {
        return Err(err(400, "AcoustID is not configured"));
    }
    dupes::verify_track(&st, &t).await.map(Json).map_err(|e| err(502, e.to_string()))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct AllQ {
    all: bool,
}

async fn verify_all(State(st): State<AppState>, Admin(_): Admin, Query(q): Query<AllQ>) -> ApiResult<Json<Value>> {
    if !acoustid::available(&st) {
        return Err(err(400, "Add an AcoustID API key in Settings → Verification (and make sure fpcalc is installed)"));
    }
    if jobs::running("verify") {
        return Err(err(409, "Verification already running"));
    }
    tokio::spawn(dupes::verify_library(st, !q.all));
    Ok(Json(json!({ "ok": true })))
}

async fn duplicates(State(st): State<AppState>, Admin(_): Admin) -> ApiResult<Json<Value>> {
    let s2 = st.clone();
    let groups = tokio::task::spawn_blocking(move || dupes::find_duplicates(&s2)).await.map_err(|e| err(500, e.to_string()))?;
    Ok(Json(json!(groups)))
}

#[derive(Deserialize)]
struct DupResolve {
    keep: i64,
    delete: Vec<i64>,
}

async fn resolve(State(st): State<AppState>, Admin(u): Admin, Json(b): Json<DupResolve>) -> ApiResult<Json<Value>> {
    if b.delete.contains(&b.keep) {
        return Err(err(400, "Can't keep and delete the same track"));
    }
    let n = library::delete_tracks(&st.db, &b.delete, Some(b.keep));
    st.db.log("info", &format!("{} removed {n} duplicate file(s)", u.s("username")));
    Ok(Json(json!({ "deleted": n })))
}

async fn resolve_all(State(st): State<AppState>, Admin(u): Admin) -> ApiResult<Json<Value>> {
    let groups = dupes::find_duplicates(&st);
    let mut n = 0;
    for g in &groups {
        let keep = g["keep"].as_i64().unwrap_or(0);
        let del: Vec<i64> = g["tracks"].as_array().cloned().unwrap_or_default().iter().filter_map(|t| t["id"].as_i64()).filter(|i| *i != keep).collect();
        n += library::delete_tracks(&st.db, &del, Some(keep));
    }
    st.db.log("info", &format!("{} auto-resolved {} duplicate groups ({n} files removed)", u.s("username"), groups.len()));
    Ok(Json(json!({ "groups": groups.len(), "deleted": n })))
}

async fn ignore(State(st): State<AppState>, Admin(_): Admin, Json(b): Json<Value>) -> ApiResult<Json<Value>> {
    st.db.execute("INSERT OR IGNORE INTO dup_ignore VALUES (?,?)", p![b["key"].as_str().unwrap_or(""), now()])?;
    Ok(Json(json!({ "ok": true })))
}
