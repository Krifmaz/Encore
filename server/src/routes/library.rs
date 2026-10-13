//! /api/library: browse, play, download, share, likes, storage, and removing things.
use crate::auth::{self, Admin, User};
use crate::db::{now, RowExt};
use crate::error::{err, ApiResult};
use crate::library::{self as lib, delete_tracks, prune_empty};
use crate::p;
use crate::state::AppState;
use axum::body::Body;
use axum::extract::{Path, Query, Request, State};
use axum::http::{header, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::Path as FsPath;
use tower::ServiceExt;
use tower_http::services::ServeFile;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/library/stats", get(stats))
        .route("/api/library/scan", post(scan))
        .route("/api/library/storage", get(storage))
        .route("/api/library/albums", get(albums))
        .route("/api/library/artists", get(artists).delete(delete_artist))
        .route("/api/library/tracks", get(tracks))
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
        .route("/api/library/tracks/{id}/share", post(share))
        .route("/api/library/albums/{id}", get(album).delete(delete_album))
        .route("/api/library/albums/{id}/cover", get(cover))
        .route("/api/library/albums/{id}/download", get(album_zip))
        .route("/api/library/verify-manual", post(verify_manual))
        .route("/api/library/mismatches", get(mismatches))
        .route("/api/likes", get(likes))
        .route("/api/likes/{id}", post(like).delete(unlike))
        .route("/api/share/{token}", get(shared))
        .route("/api/share/{token}/stream", get(shared_stream))
        .route("/api/share/{token}/cover", get(shared_cover))
        .route("/s/{token}", get(share_page))
}

fn order(table: &[(&str, &str, &str)], key: &str, dir: &str, default: &str) -> String {
    let (_, expr, dflt) = table.iter().find(|(k, _, _)| *k == key).or_else(|| table.iter().find(|(k, _, _)| *k == default)).unwrap();
    let d = if dir == "asc" || dir == "desc" { dir } else { dflt };
    expr.replace("{d}", &d.to_uppercase())
}

const ALBUM_SORTS: &[(&str, &str, &str)] = &[
    ("name", "lower(albumartist) {d}, year, lower(album)", "asc"),
    ("artist", "lower(albumartist) {d}, year", "asc"),
    ("album", "lower(album) {d}", "asc"),
    ("year", "year {d}, lower(albumartist)", "desc"),
    ("released", "year {d}, lower(albumartist)", "desc"),
    ("added", "added {d}", "desc"),
    ("duration", "duration {d}", "desc"),
    ("size", "size {d}", "desc"),
];
const TRACK_SORTS: &[(&str, &str, &str)] = &[
    ("artist", "lower(artist) {d}, lower(album), disc, track", "asc"),
    ("title", "lower(title) {d}", "asc"),
    ("album", "lower(album) {d}, disc, track", "asc"),
    ("added", "added_at {d}", "desc"),
    ("released", "year {d}, lower(artist)", "desc"),
    ("duration", "duration {d}", "desc"),
];
const ARTIST_SORTS: &[(&str, &str, &str)] = &[
    ("name", "lower(albumartist) {d}", "asc"),
    ("albums", "albums {d}", "desc"),
    ("tracks", "tracks {d}", "desc"),
    ("added", "added {d}", "desc"),
];

async fn stats(State(st): State<AppState>, User(_): User) -> Json<Value> {
    let mut s = lib::stats(&st.db);
    let scan = st.library.scan.lock().unwrap().clone();
    s["scan"] = json!(scan);
    s["acoustid"] = json!(!st.cfg.str("acoustid_api_key").is_empty());
    for k in ["bulk", "verify", "artwork", "organize"] {
        s[k] = crate::jobs::get(k);
    }
    Json(s)
}

#[derive(Deserialize)]
struct ScanQ {
    #[serde(default)]
    full: bool,
}

async fn scan(State(st): State<AppState>, Admin(_): Admin, Query(q): Query<ScanQ>) -> ApiResult<Json<Value>> {
    if st.library.scan.lock().unwrap().scanning {
        return Err(err(409, "A scan is already running"));
    }
    let l = st.library.clone();
    let db = st.db.clone();
    let root = st.cfg.str("library_path");
    tokio::spawn(async move { l.scan(db, root, q.full).await });
    Ok(Json(json!({ "ok": true })))
}

fn dir_size(path: &str) -> u64 {
    if path.is_empty() {
        return 0;
    }
    walkdir::WalkDir::new(path).into_iter().filter_map(|e| e.ok()).filter(|e| e.file_type().is_file()).filter_map(|e| e.metadata().ok()).map(|m| m.len()).sum()
}

fn disk_usage(path: &str) -> Option<Value> {
    let out = std::process::Command::new("df").args(["-Pk", path]).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let cols: Vec<&str> = text.lines().nth(1)?.split_whitespace().collect();
    let k = |i: usize| cols.get(i).and_then(|v| v.parse::<u64>().ok()).map(|v| v * 1024);
    Some(json!({ "total": k(1)?, "used": k(2)?, "free": k(3)? }))
}

async fn storage(State(st): State<AppState>, User(u): User) -> ApiResult<Json<Value>> {
    if !auth::can(&st, &u, "storage") {
        return Err(err(403, "Storage isn't shared with your account"));
    }
    let libp = st.cfg.str("library_path");
    let (slsk, staging) = (st.cfg.str("slskd_download_path"), st.cfg.str("staging_path"));
    let (disk, s1, s2) = tokio::task::spawn_blocking(move || (disk_usage(&libp), dir_size(&slsk), dir_size(&staging))).await.unwrap_or((None, 0, 0));
    let q = |sql: &str| st.db.query(sql, p![]).unwrap_or_default();
    Ok(Json(json!({
        "disk": disk,
        "library": st.db.one("SELECT COALESCE(SUM(size),0) size, COUNT(*) tracks, COUNT(DISTINCT album_id) albums FROM tracks", p![])?,
        "formats": q("SELECT upper(format) format, COUNT(*) tracks, COALESCE(SUM(size),0) size FROM tracks GROUP BY format ORDER BY size DESC"),
        "artists": q("SELECT MAX(albumartist) name, COALESCE(SUM(size),0) size, COUNT(*) tracks FROM tracks GROUP BY lower(albumartist) ORDER BY size DESC LIMIT 15"),
        "albums": q("SELECT album_id id, MAX(album) album, MAX(albumartist) albumartist, COALESCE(SUM(size),0) size, COUNT(*) tracks FROM tracks GROUP BY album_id ORDER BY size DESC LIMIT 12"),
        "downloads": [{ "name": "soulseek", "size": s1 }, { "name": "staging", "size": s2 }],
    })))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct AlbumQ {
    q: String,
    sort: String,
    filter: String,
    artist: String,
    limit: Option<i64>,
    offset: i64,
    dir: String,
    kind: String,
}

async fn albums(State(st): State<AppState>, User(_): User, Query(a): Query<AlbumQ>) -> ApiResult<Json<Value>> {
    let mut wh = vec!["1=1".to_string()];
    let mut params: Vec<Value> = vec![];
    if !a.q.is_empty() {
        wh.push("(album LIKE ? OR albumartist LIKE ? OR artist LIKE ?)".into());
        let like = format!("%{}%", a.q);
        params.extend([json!(like), json!(like), json!(like)]);
    }
    if !a.artist.is_empty() {
        wh.push("lower(albumartist)=lower(?)".into());
        params.push(json!(a.artist));
    }
    let mut having: Vec<&str> = vec![];
    match a.filter.as_str() {
        "untagged" => having.push("SUM(CASE WHEN mb_albumid IS NULL OR mb_albumid='' THEN 1 ELSE 0 END) > 0"),
        "tagged" => having.push("SUM(CASE WHEN mb_albumid IS NULL OR mb_albumid='' THEN 1 ELSE 0 END) = 0"),
        "lossless" => having.push("SUM(CASE WHEN format IN ('flac','wav','alac','aiff','ape','wv') THEN 1 ELSE 0 END) = COUNT(*)"),
        _ => {}
    }
    match a.kind.as_str() {
        "singles" => having.push("COUNT(*) <= 2"),
        "albums" => having.push("COUNT(*) > 2"),
        _ => {}
    }
    let having = if having.is_empty() { String::new() } else { format!("HAVING {}", having.join(" AND ")) };
    let w = wh.join(" AND ");
    let sql = format!(
        "SELECT album_id AS id, MAX(album) album, MAX(albumartist) albumartist, MAX(year) year, MAX(genre) genre,
                COUNT(*) tracks, SUM(duration) duration, SUM(size) size, MAX(added_at) added,
                GROUP_CONCAT(DISTINCT format) formats, MAX(mb_albumid) mb_albumid, MAX(mb_releasegroupid) mb_releasegroupid,
                SUM(CASE WHEN mb_albumid IS NULL OR mb_albumid='' THEN 1 ELSE 0 END) untagged
         FROM tracks WHERE {w} GROUP BY album_id {having} ORDER BY {} LIMIT ? OFFSET ?",
        order(ALBUM_SORTS, &a.sort, &a.dir, "name")
    );
    let mut p2 = params.clone();
    p2.push(json!(a.limit.unwrap_or(120).clamp(1, 500)));
    p2.push(json!(a.offset.max(0)));
    let items = st.db.query(&sql, p2)?;
    let total = st.db.one(&format!("SELECT COUNT(*) n FROM (SELECT album_id FROM tracks WHERE {w} GROUP BY album_id {having})"), params)?.map(|r| r.i("n")).unwrap_or(0);
    Ok(Json(json!({ "total": total, "items": items })))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct SimpleQ {
    q: String,
    sort: String,
    dir: String,
    limit: Option<i64>,
    offset: i64,
    /// `/tracks?ids=1,2,3`: these songs, in this order (unknown ids are left out)
    ids: Option<String>,
}

/// Most songs one `/tracks?ids=` call returns (a restored queue is fetched in pages of this size).
const MAX_IDS: usize = 300;

const TRACK_COLS: &str = "id, title, artist, album, albumartist, album_id, track, disc, year, duration, format, bitrate, verify_status, verify_detail, request_id, added_at";

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

async fn artists(State(st): State<AppState>, User(_): User, Query(a): Query<SimpleQ>) -> ApiResult<Json<Value>> {
    let (wh, params) = if a.q.is_empty() { ("", p![]) } else { ("WHERE albumartist LIKE ?", p![format!("%{}%", a.q)]) };
    Ok(Json(json!(st.db.query(&format!(
        "SELECT albumartist AS name, COUNT(DISTINCT album_id) albums, COUNT(*) tracks, MAX(mb_albumartistid) mbid, MIN(album_id) cover_album,
                SUM(size) size, MAX(added_at) added FROM tracks {wh} GROUP BY lower(albumartist) ORDER BY {} LIMIT 2000",
        order(ARTIST_SORTS, &a.sort, &a.dir, "name")
    ), params)?)))
}

async fn tracks(State(st): State<AppState>, User(_): User, Query(a): Query<SimpleQ>) -> ApiResult<Json<Value>> {
    if let Some(raw) = a.ids.as_deref() {
        let ids = parse_ids(raw)?;
        if ids.is_empty() {
            return Ok(Json(json!([])));
        }
        let marks = vec!["?"; ids.len()].join(",");
        let rows = st.db.query(&format!("SELECT {TRACK_COLS} FROM tracks WHERE id IN ({marks})"), ids.iter().map(|i| json!(i)).collect())?;
        let mut by_id: std::collections::HashMap<i64, crate::db::Row> = rows.into_iter().map(|r| (r.i("id"), r)).collect();
        return Ok(Json(json!(ids.iter().filter_map(|i| by_id.remove(i)).collect::<Vec<_>>())));
    }
    let mut params = vec![];
    let wh = if a.q.is_empty() { "" } else {
        let l = format!("%{}%", a.q);
        params.extend([json!(l), json!(l), json!(l)]);
        "WHERE title LIKE ? OR artist LIKE ? OR album LIKE ?"
    };
    params.push(json!(a.limit.unwrap_or(200).clamp(1, 1000)));
    params.push(json!(a.offset.max(0)));
    Ok(Json(json!(st.db.query(&format!(
        "SELECT {TRACK_COLS} FROM tracks {wh} ORDER BY {} LIMIT ? OFFSET ?", order(TRACK_SORTS, &a.sort, &a.dir, "artist")
    ), params)?)))
}

async fn recent(State(st): State<AppState>, User(_): User, Query(a): Query<SimpleQ>) -> ApiResult<Json<Value>> {
    Ok(Json(json!(st.db.query(
        "SELECT album_id AS id, MAX(album) album, MAX(albumartist) albumartist, MAX(year) year, MAX(added_at) added, COUNT(*) tracks
         FROM tracks GROUP BY album_id ORDER BY added DESC LIMIT ?", p![a.limit.unwrap_or(18).clamp(1, 200)])?)))
}

const SHUFFLE_COLS: &str = "id, title, artist, album, album_id, format, bitrate, duration";

#[derive(Deserialize, Default)]
#[serde(default)]
struct ShuffleQ {
    limit: Option<i64>,
    exclude: String,
}

async fn shuffle_get(State(st): State<AppState>, User(_): User, Query(q): Query<ShuffleQ>) -> ApiResult<Json<Value>> {
    let ex: Vec<i64> = q.exclude.split(',').filter_map(|x| x.trim().parse().ok()).take(500).collect();
    shuffle_inner(&st, q.limit.unwrap_or(50), &ex)
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ShuffleBody {
    limit: Option<i64>,
    exclude: Vec<i64>,
}

/// Random songs nobody heard yet this session (the web player sends what it already played).
async fn shuffle_post(State(st): State<AppState>, User(_): User, Json(b): Json<ShuffleBody>) -> ApiResult<Json<Value>> {
    shuffle_inner(&st, b.limit.unwrap_or(50), &b.exclude[..b.exclude.len().min(20000)])
}

fn shuffle_inner(st: &AppState, limit: i64, exclude: &[i64]) -> ApiResult<Json<Value>> {
    let ex: std::collections::HashSet<i64> = exclude.iter().copied().collect();
    let rows = st.db.query(&format!("SELECT {SHUFFLE_COLS} FROM tracks ORDER BY RANDOM()"), p![])?;
    Ok(Json(json!(rows.into_iter().filter(|r| !ex.contains(&r.i("id"))).take(limit.clamp(1, 200) as usize).collect::<Vec<_>>())))
}

#[derive(Deserialize)]
struct PlayBody {
    track_id: i64,
    #[serde(default)]
    seconds: f64,
}

async fn plays(State(st): State<AppState>, User(u): User, Json(b): Json<PlayBody>) -> ApiResult<Json<Value>> {
    let t = st.db.one("SELECT artist, title, album FROM tracks WHERE id=?", p![b.track_id])?.ok_or_else(|| err(404, "Track not found"))?;
    let secs = b.seconds.clamp(0.0, 7200.0);
    st.db.execute("INSERT INTO plays (user_id, track_id, artist, title, album, seconds, played_at) VALUES (?,?,?,?,?,?,?)",
        p![u.i("id"), b.track_id, t.s("artist"), t.s("title"), t.s("album"), secs, now()])?;
    crate::scrobble::scrobble(&st, u.i("id"), b.track_id, (now() - secs) as i64);
    Ok(Json(json!({ "ok": true })))
}

/// A song started: shows as "now playing" on Last.fm / ListenBrainz.
async fn now_playing(State(st): State<AppState>, User(u): User, Json(b): Json<PlayBody>) -> Json<Value> {
    crate::scrobble::now_playing(&st, u.i("id"), b.track_id);
    Json(json!({ "ok": true }))
}

/// Users with the "fix" permission may act on songs that came from their own requests.
fn owns_request(st: &AppState, u: &crate::db::Row, request_id: i64) -> bool {
    request_id > 0 && auth::can(st, u, "fix")
        && st.db.one("SELECT user_id FROM requests WHERE id=?", p![request_id]).ok().flatten().is_some_and(|r| r.i("user_id") == u.i("id"))
}

async fn delete_track(State(st): State<AppState>, User(u): User, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    let t = st.db.one("SELECT path, artist, title, request_id FROM tracks WHERE id=?", p![id])?.ok_or_else(|| err(404, "Track not found"))?;
    if !auth::is_admin(&u) && !owns_request(&st, &u, t.i("request_id")) {
        return Err(err(403, "You can only remove songs you requested"));
    }
    delete_tracks(&st.db, &[id], None);
    if let Some(d) = FsPath::new(&t.s("path")).parent() {
        prune_empty(d, FsPath::new(&st.cfg.str("library_path")));
    }
    st.db.log("info", &format!("{} deleted {} – {}", u.s("username"), t.s("artist"), t.s("title")));
    Ok(Json(json!({ "ok": true })))
}

fn safe_name(s: &str) -> String {
    s.chars().map(|c| if "\\/:*?\"<>|".contains(c) || c.is_control() { '_' } else { c }).collect::<String>().trim().to_string()
}

pub(crate) async fn serve_file(path: &str, req: Request, content_type: Option<&str>, download_name: Option<String>) -> ApiResult<Response> {
    let mut svc = ServeFile::new(path);
    if let Some(ct) = content_type {
        svc = ServeFile::new_with_mime(path, &ct.parse().map_err(|_| err(500, "bad mime"))?);
    }
    let mut resp = svc.oneshot(req).await.map_err(|_| err(404, "File not found"))?.map(Body::new);
    if let Some(n) = download_name {
        let v = format!("attachment; filename*=UTF-8''{}", urlencoding::encode(&n));
        resp.headers_mut().insert(header::CONTENT_DISPOSITION, HeaderValue::from_str(&v).unwrap_or(HeaderValue::from_static("attachment")));
    }
    Ok(resp)
}

async fn track_file(State(st): State<AppState>, User(_): User, Path(id): Path<i64>, req: Request) -> ApiResult<Response> {
    let t = st.db.one("SELECT path, artist, title FROM tracks WHERE id=?", p![id])?.ok_or_else(|| err(404, "File not found"))?;
    let path = t.s("path");
    let ext = FsPath::new(&path).extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    serve_file(&path, req, None, Some(safe_name(&format!("{} - {}{ext}", t.s("artist"), t.s("title"))))).await
}

fn browser_type(fmt: &str) -> Option<&'static str> {
    Some(match fmt {
        "mp3" => "audio/mpeg", "flac" => "audio/flac", "m4a" | "mp4" => "audio/mp4", "aac" => "audio/aac",
        "ogg" | "opus" | "oga" => "audio/ogg", "wav" => "audio/wav", "webm" => "audio/webm",
        _ => return None,
    })
}

/// Apple Lossless hides inside .m4a – browsers can't play it.
fn is_alac(path: &str) -> bool {
    use lofty::file::AudioFile;
    let Ok(mut f) = std::fs::File::open(path) else { return false };
    lofty::mp4::Mp4File::read_from(&mut f, lofty::config::ParseOptions::new())
        .map(|m| matches!(m.properties().codec(), Some(lofty::mp4::Mp4Codec::ALAC)))
        .unwrap_or(false)
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct StreamQ {
    /// convert to FLAC (formats a browser can't play)
    transcode: bool,
    /// start a converted stream at this many seconds (the player seeks converted streams through the server)
    t: f64,
    /// "aac"/"opus": a lossy stream at `kbps` (Auto's adaptive rungs 256–32, or Data saver's 96)
    q: String,
    /// bitrate of the lossy stream in kbps (32–320, default 256; the player steps down to 32 when the connection is slow)
    kbps: u32,
    /// upsample on the server with Cadenza (admin switch + `server_dsp` permission; never on share links)
    #[serde(deserialize_with = "flag")]
    dsp: bool,
    /// Cadenza output rate in Hz (0 = the highest multiple of the source rate the server allows)
    rate: u32,
    /// Cadenza filter id (empty = server default)
    filter: String,
    /// Cadenza dither id (default TPDF)
    dither: String,
}

/// A query switch: `1`, `true`, `yes` or `on` (an empty or missing value is off).
fn flag<'de, D: serde::Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    let s = String::deserialize(d)?;
    Ok(matches!(s.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
}

/// The original file when the browser can play it (seekable, bit-perfect); otherwise FLAC made on the fly,
/// or – when the player asks for it – a lossy AAC stream the connection can keep up with, or a stream
/// upsampled on the server by Cadenza. `user` is None for share links, which never get server processing.
pub async fn stream_track(st: &AppState, id: i64, q: &StreamQ, req: Request, user: Option<&crate::db::Row>) -> ApiResult<Response> {
    let tr = st.db.one("SELECT path, format FROM tracks WHERE id=?", p![id])?.ok_or_else(|| err(404, "Track not found"))?;
    let (path, fmt) = (tr.s("path"), tr.s("format"));
    if !FsPath::new(&path).is_file() {
        return Err(err(404, "Track not found"));
    }
    let lossy = matches!(q.q.as_str(), "aac" | "lossy" | "opus");
    // the lossy stream (Data saver / Auto's fallback) wins over server processing
    if let (true, false, Some(u)) = (q.dsp, lossy, user) {
        return dsp_stream(st, id, path, q, u, req).await;
    }
    let alac = if fmt == "m4a" || fmt == "mp4" { let p2 = path.clone(); tokio::task::spawn_blocking(move || is_alac(&p2)).await.unwrap_or(false) } else { false };
    if let (false, false, false, Some(ct)) = (q.transcode, lossy, alac, browser_type(&fmt)) {
        let mut r = serve_file(&path, req, Some(ct), None).await?;
        r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("private, max-age=3600"));
        r.headers_mut().insert(header::HeaderName::from_static("x-encore-stream"), HeaderValue::from_static("original"));
        return Ok(r);
    }
    let mut cmd = tokio::process::Command::new("ffmpeg");
    cmd.args(["-v", "error", "-nostdin"]);
    if q.t > 0.0 {
        cmd.args(["-ss", &format!("{:.2}", q.t)]);
    }
    cmd.args(["-i", &path, "-map", "0:a:0"]);
    // DSD has no PCM sample rate: decode it to 24-bit / 176.4 kHz, the usual DSD → PCM choice and a rate browsers play
    if matches!(fmt.as_str(), "dsf" | "dff") && !lossy {
        cmd.args(["-af", "aresample=176400", "-sample_fmt", "s32"]);
    }
    let (ct, tag) = if lossy {
        let kbps = if q.kbps == 0 { 256 } else { q.kbps.clamp(32, 320) };
        if q.q == "opus" {
            // Opus in Ogg for browsers without an AAC decoder (Chromium builds, Firefox on Linux)
            cmd.args(["-c:a", "libopus", "-b:a", &format!("{kbps}k"), "-vbr", "on", "-application", "audio", "-f", "ogg", "-flush_packets", "1", "pipe:1"]);
            ("audio/ogg", format!("opus-{kbps}"))
        } else {
            // AAC in an ADTS stream: starts fast and needs no container index
            cmd.args(["-c:a", "aac", "-b:a", &format!("{kbps}k"), "-f", "adts", "-flush_packets", "1", "pipe:1"]);
            ("audio/aac", format!("aac-{kbps}"))
        }
    } else {
        cmd.args(["-c:a", "flac", "-compression_level", "0", "-f", "flac", "-flush_packets", "1", "pipe:1"]);
        ("audio/flac", "flac".to_string())
    };
    cmd.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).kill_on_drop(true);
    let mut child = cmd.spawn().map_err(|e| err(500, format!("ffmpeg: {e}")))?;
    let out = child.stdout.take().ok_or_else(|| err(500, "ffmpeg gave no output"))?;
    // the child is killed when the response (and with it the stream) is dropped
    let stream = futures_util::StreamExt::map(tokio_util::io::ReaderStream::new(out), move |c| { let _keep = &child; c });
    let mut r = ([(header::CONTENT_TYPE, ct)], Body::from_stream(stream)).into_response();
    r.headers_mut().insert(header::HeaderName::from_static("x-transcoded"), HeaderValue::from_str(&tag).unwrap_or(HeaderValue::from_static("flac")));
    r.headers_mut().insert(header::HeaderName::from_static("x-encore-stream"), HeaderValue::from_str(&tag).unwrap_or(HeaderValue::from_static("flac")));
    Ok(r)
}

async fn stream(State(st): State<AppState>, User(u): User, Path(id): Path<i64>, Query(q): Query<StreamQ>, req: Request) -> ApiResult<Response> {
    stream_track(&st, id, &q, req, Some(&u)).await
}

/// Cadenza on the server: lossless 24-bit FLAC at the chosen rate. With the render cache on
/// (`cadenza_cache_mb` > 0) the song is rendered once to a file and served from it
/// (`x-encore-dsp: cached` – a plain file with Range support – or `rendering` while it is being made);
/// otherwise, or for a seek past what a running render has reached, it is processed live (`live`).
async fn dsp_stream(st: &AppState, id: i64, path: String, q: &StreamQ, u: &crate::db::Row, req: Request) -> ApiResult<Response> {
    let lim = crate::dsp::Limits::of(st);
    if !lim.enabled {
        return Err(err(403, "Server processing is turned off on this server"));
    }
    if !auth::can(st, u, "server_dsp") {
        return Err(err(403, "Your account can't use server processing"));
    }
    let spec = crate::dsp::render::Spec::new(st, id, path, q.rate, &q.filter, &q.dither).await;
    let t = if q.t.is_finite() { q.t.max(0.0) } else { 0.0 };
    let ctx = crate::dsp::render::Ctx::of(st);
    if let Some(ctx) = &ctx {
        if let Some(r) = crate::dsp::render::serve(ctx, &spec, u.i("id"), t, req).await? {
            return Ok(r);
        }
    }
    // A seek or track change often arrives before the old connection's drop has freed its slot,
    // so wait briefly for one before saying the server is busy.
    let Some(slot) = crate::dsp::acquire_with_retry(lim.max_streams).await else {
        // no slot: a render of this song that is running (faster than real time) soon reaches the seek
        if let Some(ctx) = &ctx {
            if let Some(r) = crate::dsp::render::wait_for_render(ctx, &spec, t).await? {
                return Ok(r);
            }
        }
        return Err(err(503, "Server processing is busy – playing the original"));
    };
    // this request has its slot: now render the song in the background for later seeks
    if let Some(ctx) = &ctx {
        crate::dsp::render::start_background(ctx, &spec, u.i("id"));
    }
    let mut r = crate::dsp::stream(crate::dsp::cache(st), spec.live(t), slot).await?;
    r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("private, no-cache"));
    Ok(r)
}

/// What's in the file, for the player's signal path: codec, sample rate, bit depth, channels, bitrate.
pub fn audio_info(path: &str) -> Value {
    use lofty::file::AudioFile;
    use lofty::prelude::*;
    let Ok(f) = lofty::read_from_path(path) else { return json!({}) };
    let p = f.properties();
    let ft = format!("{:?}", f.file_type()).to_lowercase();
    let lossless = matches!(ft.as_str(), "flac" | "wav" | "aiff" | "ape" | "wavpack") || ft == "mp4" && is_alac(path);
    let codec = match ft.as_str() {
        "mpeg" => "mp3", "mp4" => if lossless { "alac" } else { "aac" }, "vorbis" => "ogg vorbis", "wavpack" => "wavpack", "ape" => "ape",
        other => other,
    };
    json!({
        "codec": codec,
        "sample_rate": p.sample_rate(), "bit_depth": p.bit_depth(), "channels": p.channels(),
        "bitrate": p.audio_bitrate().or(p.overall_bitrate()), "duration": p.duration().as_secs_f64(), "lossless": lossless,
    })
}

/// One song as the lists return it (the car player rebuilds its queue from ids).
async fn track(State(st): State<AppState>, User(u): User, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    let mut t = st.db.one("SELECT id, title, artist, album, albumartist, album_id, track, disc, year, duration, format, bitrate, path FROM tracks WHERE id=?", p![id])?.ok_or_else(|| err(404, "Track not found"))?;
    if !auth::is_admin(&u) { t.remove("path"); }
    Ok(Json(Value::Object(t)))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct LimitQ {
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
    Ok(Json(json!(rows)))
}

async fn track_info(State(st): State<AppState>, User(_): User, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    let t = st.db.one("SELECT id, path, format, size, bitrate, duration FROM tracks WHERE id=?", p![id])?.ok_or_else(|| err(404, "Track not found"))?;
    let path = t.s("path");
    let mut info = tokio::task::spawn_blocking(move || audio_info(&path)).await.unwrap_or(json!({}));
    info["id"] = json!(t.i("id"));
    info["format"] = json!(t.s("format"));
    info["size"] = json!(t.i("size"));
    if info["bitrate"].is_null() { info["bitrate"] = json!(t.i("bitrate")); }
    if info["duration"].is_null() { info["duration"] = json!(t.f("duration")); }
    Ok(Json(info))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct LyricsQ {
    fetch: Option<bool>,
    force: Option<bool>,
}

/// Lyrics next to the song (.lrc synced, .txt plain), looked up on LRCLIB and saved when missing.
async fn lyrics(State(st): State<AppState>, User(_): User, Path(id): Path<i64>, Query(q): Query<LyricsQ>) -> ApiResult<Json<Value>> {
    let t = st.db.one("SELECT id, path, artist, title, album, duration FROM tracks WHERE id=?", p![id])?.ok_or_else(|| err(404, "Track not found"))?;
    if !q.fetch.unwrap_or(true) {
        return Ok(Json(match crate::lyrics::sidecar(FsPath::new(&t.s("path"))) {
            Some((text, synced)) => json!({ "lyrics": text, "synced": synced, "source": "file" }),
            None => json!({ "lyrics": null, "synced": false, "source": null }),
        }));
    }
    crate::lyrics::fetch_for_track(&st, &t, q.force.unwrap_or(false)).await.map(Json).map_err(|e| err(502, format!("Lyrics lookup failed: {e}")))
}

async fn album(State(st): State<AppState>, User(u): User, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    // who asked for it: the request that brought the file in, when there was one
    let rows = st.db.query("SELECT t.*, u.username requested_by, r.created_at requested_at FROM tracks t LEFT JOIN requests r ON r.id=t.request_id LEFT JOIN users u ON u.id=r.user_id WHERE t.album_id=? ORDER BY t.disc, t.track, t.path", p![id])?;
    if rows.is_empty() {
        return Err(err(404, "Album not found"));
    }
    let libp = format!("{}/", st.cfg.str("library_path").trim_end_matches('/'));
    let admin = auth::is_admin(&u);
    let tracks: Vec<Value> = rows.iter().map(|r| {
        let mut r = r.clone();
        let p = r.s("path");
        r.insert("relpath".into(), json!(p.strip_prefix(&libp).unwrap_or(&p)));
        if !admin {
            r.remove("path");
        }
        Value::Object(r)
    }).collect();
    let f = &rows[0];
    Ok(Json(json!({
        "id": id, "album": f.get("album"), "albumartist": f.get("albumartist"), "year": f.get("year"), "genre": f.get("genre"),
        "mb_albumid": f.get("mb_albumid"), "mb_releasegroupid": f.get("mb_releasegroupid"), "mb_albumartistid": f.get("mb_albumartistid"),
        "tracks": tracks, "duration": rows.iter().map(|r| r.f("duration")).sum::<f64>(), "size": rows.iter().map(|r| r.i("size")).sum::<i64>(),
    })))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct CoverQ {
    s: u32,
}

pub async fn cover_response(st: &AppState, album_id: &str, size: u32, req: Request) -> ApiResult<Response> {
    let (st2, aid) = (st.clone(), album_id.to_string());
    let path = tokio::task::spawn_blocking(move || {
        let p = st2.library.find_cover(&st2.db, &aid)?;
        Some(if size > 0 { lib::thumbnail(&st2.db, &p, &aid, size) } else { p })
    }).await.ok().flatten().ok_or_else(|| err(404, "No cover"))?;
    let mut r = serve_file(&path.to_string_lossy(), req, None, None).await?;
    r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("private, no-cache"));
    Ok(r)
}

async fn cover(State(st): State<AppState>, User(_): User, Path(id): Path<String>, Query(q): Query<CoverQ>, req: Request) -> ApiResult<Response> {
    cover_response(&st, &id, q.s, req).await
}

async fn album_zip(State(st): State<AppState>, User(_): User, Path(id): Path<String>, req: Request) -> ApiResult<Response> {
    let rows = st.db.query("SELECT path, albumartist, album FROM tracks WHERE album_id=? ORDER BY disc, track, path", p![id])?;
    if rows.is_empty() {
        return Err(err(404, "Album not found"));
    }
    let name = safe_name(&format!("{} - {}.zip", rows[0].s("albumartist"), rows[0].s("album")));
    let st2 = st.clone();
    let tmp = tokio::task::spawn_blocking(move || -> anyhow::Result<std::path::PathBuf> {
        let tmp = std::env::temp_dir().join(format!("encore-{}.zip", auth::random_hex(8)));
        let mut z = zip::ZipWriter::new(std::fs::File::create(&tmp)?);
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored).large_file(true);
        for r in &rows {
            let p = r.s("path");
            if let (Ok(mut f), Some(n)) = (std::fs::File::open(&p), FsPath::new(&p).file_name()) {
                z.start_file(n.to_string_lossy(), opts)?;
                std::io::copy(&mut f, &mut z)?;
            }
        }
        if let Some(c) = st2.library.find_cover(&st2.db, &id) {
            if let Ok(mut f) = std::fs::File::open(&c) {
                let ext = c.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or("jpg".into());
                z.start_file(format!("cover.{ext}"), opts)?;
                std::io::copy(&mut f, &mut z)?;
            }
        }
        z.finish()?;
        Ok(tmp)
    }).await.map_err(|e| err(500, e.to_string()))?.map_err(|e| err(500, format!("Could not build the zip: {e}")))?;
    let resp = serve_file(&tmp.to_string_lossy(), req, Some("application/zip"), Some(name)).await;
    // the open file handle keeps the data readable after the name is gone (unix)
    let _ = std::fs::remove_file(&tmp);
    resp
}

async fn delete_album(State(st): State<AppState>, Admin(u): Admin, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    let rows = st.db.query("SELECT id, path FROM tracks WHERE album_id=?", p![id])?;
    if rows.is_empty() {
        return Err(err(404, "Album not found"));
    }
    let ids: Vec<i64> = rows.iter().map(|r| r.i("id")).collect();
    delete_tracks(&st.db, &ids, None);
    let stop = st.cfg.str("library_path");
    for r in &rows {
        if let Some(d) = FsPath::new(&r.s("path")).parent() {
            prune_empty(d, FsPath::new(&stop));
        }
    }
    st.library.forget_cover(Some(&id));
    st.db.log("info", &format!("{} deleted album {id} ({} files)", u.s("username"), rows.len()));
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct NameQ {
    name: String,
}

/// Remove an artist added by mistake (e.g. a wrong download): files, follow, pending requests.
async fn delete_artist(State(st): State<AppState>, Admin(u): Admin, Query(q): Query<NameQ>) -> ApiResult<Json<Value>> {
    let rows = st.db.query(
        "SELECT id, path FROM tracks WHERE lower(albumartist)=lower(?) OR (lower(artist)=lower(?) AND (albumartist IS NULL OR albumartist='' OR albumartist='Unknown Artist'))",
        p![q.name, q.name],
    )?;
    let ids: Vec<i64> = rows.iter().map(|r| r.i("id")).collect();
    delete_tracks(&st.db, &ids, None);
    let stop = st.cfg.str("library_path");
    for r in &rows {
        if let Some(d) = FsPath::new(&r.s("path")).parent() {
            prune_empty(d, FsPath::new(&stop));
        }
    }
    st.db.execute("DELETE FROM follows WHERE lower(name)=lower(?)", p![q.name])?;
    st.db.execute("UPDATE requests SET status='cancelled', message='Artist removed' WHERE lower(artist)=lower(?) AND status IN ('pending','queued')", p![q.name])?;
    st.db.log("info", &format!("{} removed artist {} ({} songs)", u.s("username"), q.name, rows.len()));
    Ok(Json(json!({ "ok": true, "removed": rows.len() })))
}

#[derive(Deserialize)]
struct ManualVerify {
    track_ids: Vec<i64>,
    #[serde(default = "yes")]
    correct: bool,
}
fn yes() -> bool {
    true
}

/// "This is the right song" – overrides the fingerprint check (e.g. matched to Deezer, not MusicBrainz).
async fn verify_manual(State(st): State<AppState>, User(u): User, Json(b): Json<ManualVerify>) -> ApiResult<Json<Value>> {
    for id in b.track_ids {
        let Some(t) = st.db.one("SELECT request_id FROM tracks WHERE id=?", p![id])? else { continue };
        if !auth::is_admin(&u) && !owns_request(&st, &u, t.i("request_id")) {
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

async fn mismatches(State(st): State<AppState>, Admin(_): Admin) -> ApiResult<Json<Value>> {
    Ok(Json(json!(st.db.query(
        "SELECT id, title, artist, album, album_id, verify_detail, format, bitrate, duration FROM tracks WHERE verify_status='mismatch' ORDER BY lower(artist), lower(album)", p![])?)))
}

// ---- likes ------------------------------------------------------------------------------------------

fn liked_playlist(st: &AppState, uid: i64) -> ApiResult<i64> {
    if let Some(r) = st.db.one("SELECT id FROM playlists WHERE user_id=? AND kind='liked'", p![uid])? {
        return Ok(r.i("id"));
    }
    Ok(st.db.execute("INSERT INTO playlists (user_id, name, kind, source, sync_targets, created_at, updated_at) VALUES (?, 'Liked Songs', 'liked', 'manual', '[]', ?, ?)", p![uid, now(), now()])?)
}

async fn likes(State(st): State<AppState>, User(u): User) -> ApiResult<Json<Value>> {
    let pid = liked_playlist(&st, u.i("id"))?;
    Ok(Json(json!(st.db.query("SELECT track_id FROM playlist_items WHERE playlist_id=? AND track_id IS NOT NULL", p![pid])?.iter().map(|r| r.i("track_id")).collect::<Vec<_>>())))
}

async fn like(State(st): State<AppState>, User(u): User, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    let t = st.db.one("SELECT title, artist, album, duration, isrc, mb_trackid FROM tracks WHERE id=?", p![id])?.ok_or_else(|| err(404, "Track not found"))?;
    let pid = liked_playlist(&st, u.i("id"))?;
    if st.db.one("SELECT 1 FROM playlist_items WHERE playlist_id=? AND track_id=?", p![pid, id])?.is_none() {
        let pos = st.db.one("SELECT COALESCE(MAX(position)+1,0) n FROM playlist_items WHERE playlist_id=?", p![pid])?.map(|r| r.i("n")).unwrap_or(1);
        st.db.execute("INSERT INTO playlist_items (playlist_id, position, title, artist, album, duration, isrc, mbid, source, track_id, match_reason, added_at) VALUES (?,?,?,?,?,?,?,?,'library',?,'added from library',?)",
            p![pid, pos, t.s("title"), t.s("artist"), t.s("album"), t.f("duration"), t.get("isrc"), t.get("mb_trackid"), id, now()])?;
        st.db.execute("UPDATE playlists SET updated_at=? WHERE id=?", p![now(), pid])?;
        return Ok(Json(json!({ "liked": true, "added": 1 })));
    }
    Ok(Json(json!({ "liked": true, "added": 0 })))
}

async fn unlike(State(st): State<AppState>, User(u): User, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    let pid = liked_playlist(&st, u.i("id"))?;
    st.db.execute("DELETE FROM playlist_items WHERE playlist_id=? AND track_id=?", p![pid, id])?;
    let ids = st.db.query("SELECT id FROM playlist_items WHERE playlist_id=? ORDER BY position, id", p![pid])?;
    for (i, r) in ids.iter().enumerate() {
        st.db.execute("UPDATE playlist_items SET position=? WHERE id=?", p![i as i64, r.i("id")])?;
    }
    Ok(Json(json!({ "liked": false })))
}

// ---- share links ----------------------------------------------------------------------------------

async fn share(State(st): State<AppState>, User(u): User, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    if st.db.one("SELECT 1 FROM tracks WHERE id=?", p![id])?.is_none() {
        return Err(err(404, "Track not found"));
    }
    let token = match st.db.one("SELECT token FROM shares WHERE track_id=? AND user_id=?", p![id, u.i("id")])? {
        Some(r) => r.s("token"),
        None => {
            let t = auth::random_token(12);
            st.db.execute("INSERT INTO shares VALUES (?,?,?,?)", p![t, id, u.i("id"), now()])?;
            t
        }
    };
    let base = st.cfg.str("base_url");
    Ok(Json(json!({ "token": token, "path": format!("/s/{token}"), "url": format!("{}/s/{token}", base.trim_end_matches('/')) })))
}

fn shared_track(st: &AppState, token: &str) -> ApiResult<crate::db::Row> {
    let s = st.db.one("SELECT track_id FROM shares WHERE token=?", p![token])?.ok_or_else(|| err(404, "This link doesn't work any more"))?;
    st.db.one("SELECT id, title, artist, album, albumartist, album_id, year, track, duration, format, bitrate, path FROM tracks WHERE id=?", p![s.i("track_id")])?.ok_or_else(|| err(404, "This link doesn't work any more"))
}

/// Everything the share page shows about the song (no file paths).
async fn shared_meta(st: &AppState, token: &str) -> ApiResult<Value> {
    let t = shared_track(st, token)?;
    let path = t.s("path");
    let info = tokio::task::spawn_blocking(move || audio_info(&path)).await.unwrap_or(json!({}));
    Ok(json!({
        "title": t.get("title"), "artist": t.get("artist"), "album": t.get("album"), "albumartist": t.get("albumartist"), "year": t.get("year"), "track": t.get("track"),
        "duration": t.get("duration"), "format": t.get("format"), "bitrate": t.get("bitrate"),
        "sample_rate": info["sample_rate"], "bit_depth": info["bit_depth"], "channels": info["channels"], "lossless": info["lossless"],
        "app_name": st.cfg.str("app_name"),
    }))
}

async fn shared(State(st): State<AppState>, Path(token): Path<String>) -> ApiResult<Json<Value>> {
    Ok(Json(shared_meta(&st, &token).await?))
}

async fn shared_stream(State(st): State<AppState>, Path(token): Path<String>, Query(q): Query<StreamQ>, req: Request) -> ApiResult<Response> {
    let t = shared_track(&st, &token)?;
    // share links play the file (or a plain conversion) – never server processing
    stream_track(&st, t.i("id"), &q, req, None).await
}

async fn shared_cover(State(st): State<AppState>, Path(token): Path<String>, Query(q): Query<CoverQ>, req: Request) -> ApiResult<Response> {
    let t = shared_track(&st, &token)?;
    cover_response(&st, &t.s("album_id"), q.s, req).await
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

fn trim_khz(hz: u64) -> String {
    let k = hz as f64 / 1000.0;
    let s = format!("{k:.1}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn mmss(secs: f64) -> String {
    let s = secs.max(0.0).round() as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// The share page, with Open Graph / Twitter tags filled in on the server so chat apps show the song,
/// artist, album and cover in the link preview before anyone opens it.
async fn share_page(State(st): State<AppState>, Path(token): Path<String>, req: Request) -> ApiResult<Response> {
    let html = tokio::fs::read_to_string(st.static_dir.join("share.html")).await.map_err(|_| err(404, "Not found"))?;
    let meta = match shared_meta(&st, &token).await {
        Ok(m) => m,
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
    let e = |v: &Value| crate::routes::html_escape(v.as_str().unwrap_or(""));
    let title = format!("{} – {}", e(&meta["title"]), e(&meta["artist"]));
    let mut desc = vec![];
    if let Some(a) = meta["album"].as_str().filter(|a| !a.is_empty()) {
        desc.push(match meta["year"].as_str().filter(|y| !y.is_empty()) { Some(y) => format!("{} ({})", crate::routes::html_escape(a), crate::routes::html_escape(y)), None => crate::routes::html_escape(a) });
    }
    let q = quality_line(&meta);
    if !q.is_empty() { desc.push(q); }
    if let Some(d) = meta["duration"].as_f64().filter(|d| *d > 0.0) { desc.push(mmss(d)); }
    desc.push(format!("Shared from {}", e(&meta["app_name"])));
    let tok = crate::routes::html_escape(&token);
    let tags = format!(
        r#"<title>{title}</title>
<meta property="og:type" content="music.song">
<meta property="og:site_name" content="{app}">
<meta property="og:title" content="{title}">
<meta property="og:description" content="{desc}">
<meta property="og:url" content="{base}/s/{tok}">
<meta property="og:image" content="{base}/api/share/{tok}/cover?s=600">
<meta property="og:image:width" content="600"><meta property="og:image:height" content="600">
<meta property="og:audio" content="{base}/api/share/{tok}/stream">
<meta property="og:audio:type" content="{mime}">
<meta property="music:musician" content="{artist}">
<meta property="music:album" content="{album}">
<meta property="music:duration" content="{dur}">
<meta name="twitter:card" content="summary_large_image">
<meta name="twitter:title" content="{title}">
<meta name="twitter:description" content="{desc}">
<meta name="twitter:image" content="{base}/api/share/{tok}/cover?s=600">
<meta name="description" content="{desc}">"#,
        app = e(&meta["app_name"]), desc = desc.join(" · "), artist = e(&meta["artist"]), album = e(&meta["album"]),
        dur = meta["duration"].as_f64().unwrap_or(0.0).round() as u64,
        mime = browser_type(meta["format"].as_str().unwrap_or("")).unwrap_or("audio/flac"),
    );
    let html = html.replace("<title>Shared song</title>", "").replace("<!--META-->", &tags);
    Ok(([(header::CONTENT_TYPE, "text/html; charset=utf-8"), (header::CACHE_CONTROL, "no-cache")], html).into_response())
}
