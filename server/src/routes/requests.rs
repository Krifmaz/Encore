//! /api/requests: ask for artists, albums, songs or any link; approve, retry, cancel, clear.
use crate::auth::{self, Admin, User};
use crate::db::{now, Row, RowExt};
use crate::error::{err, ApiError, ApiResult};
use crate::state::AppState;
use crate::worker::{self, ACTIVE};
use crate::{library, notify, p};
use axum::extract::{Path, Query, State};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use regex::Regex;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::LazyLock;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/requests", get(list).post(create))
        .route("/api/requests/from-link", post(from_link))
        .route("/api/requests/link", post(link))
        .route("/api/requests/clear", post(clear))
        .route("/api/requests/events", get(crate::req_events::events))
        .route("/api/requests/{rid}", delete(remove))
        .route("/api/requests/{rid}/approve", post(approve))
        .route("/api/requests/{rid}/deny", post(deny))
        .route("/api/requests/{rid}/retry", post(retry))
        .route("/api/requests/{rid}/cancel", post(cancel))
}

const EXT_SOURCES: [&str; 5] = ["deezer", "apple", "ytm", "bandcamp", "discogs"];

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
pub struct NewRequest {
    pub kind: String,
    pub mbid: Option<String>,
    pub artist: Option<String>,
    pub title: Option<String>,
    pub album: Option<String>,
    pub year: Option<String>,
    pub cover_url: Option<String>,
    pub url: Option<String>,
    pub length: Option<f64>,
    pub isrc: Option<String>,
    pub album_mode: Option<String>,
    pub ext: Option<Value>,
    /// "Request discography": this request's release types ({"kinds": [...], "skip": [...]}), else the setting's
    pub release_filter: Option<Value>,
    /// "Request discography": exactly these releases – the boxes ticked in the dialog, as MusicBrainz release
    /// group ids and Discover "ext-…" ids – instead of what the release types pick (owned and requested ones
    /// are still left out)
    pub release_ids: Option<Vec<String>>,
}


fn nz(s: &Option<String>) -> Option<&str> {
    s.as_deref().filter(|x| !x.is_empty())
}

pub fn quota(st: &AppState, u: &Row, n: i64) -> ApiResult<()> {
    if auth::is_admin(u) {
        return Ok(());
    }
    let limit = u.i("request_limit");
    if limit == 0 {
        return Ok(());
    }
    let used = st.db.one("SELECT COUNT(*) n FROM requests WHERE user_id=? AND created_at > ? AND parent_id IS NULL", p![u.i("id"), now() - 7.0 * 86400.0])?.map(|r| r.i("n")).unwrap_or(0);
    if used + n > limit {
        return Err(err(429, format!("Weekly request limit reached ({limit}). Ask an admin to raise it.")));
    }
    Ok(())
}

/// 409 when the song is already requested; the body carries the existing request's `id` and `status`, so a
/// caller (Download missing, say) can link to it instead of skipping the song.
fn dup(st: &AppState, sql: &str, params: Vec<Value>) -> ApiResult<()> {
    if let Some(d) = st.db.one(sql, params)? {
        return Err(err(409, format!("Already requested (#{}, {})", d.i("id"), d.s("status"))).with(json!({ "id": d.i("id"), "status": d.s("status") })));
    }
    Ok(())
}

/// The catalogue ids in a Discover "ext-…" album id ("ext-d302127", "ext-d2-a3": Deezer 2, Apple 3).
pub fn ext_from_id(id: &str) -> Option<Value> {
    let rest = id.strip_prefix("ext-")?;
    let mut ext = serde_json::Map::new();
    for part in rest.split('-') {
        let (k, n) = part.split_at(part.chars().next().map_or(0, |c| c.len_utf8()));
        let key = match k { "d" => "deezer", "a" => "apple", _ => continue };
        if let Ok(n) = n.parse::<i64>() {
            ext.entry(key).or_insert(json!(n));
        }
    }
    (!ext.is_empty()).then_some(Value::Object(ext))
}

/// How many of the previous attempt's log lines a reused row keeps, and how long each may be.
const KEPT_LOG_LINES: usize = 40;
const KEPT_LOG_CHARS: usize = 500;

/// A new request row – or, when the same song was asked for before and that request failed or was
/// cancelled, that row again (reset like Retry, `detail.attempts` one higher, the old message in its log),
/// so the Requests list has one row per song. Returns the row's id.
pub fn create_request(st: &AppState, u: &Row, b: &NewRequest, parent: Option<i64>, extra: Option<Value>) -> ApiResult<i64> {
    create_or_reuse(st, u, b, parent, extra, None)
}

/// The failed or cancelled top-level request for the same song (same kind and MusicBrainz id, or the same
/// artist and title when there is none), newest first.
fn failed_twin(st: &AppState, b: &NewRequest, artist: Option<&str>, title: Option<&str>) -> ApiResult<Option<Row>> {
    let ended = "status IN ('failed','cancelled') AND parent_id IS NULL ORDER BY id DESC LIMIT 1";
    if let Some(m) = nz(&b.mbid) {
        return Ok(st.db.one(&format!("SELECT * FROM requests WHERE kind=? AND mbid=? AND {ended}"), p![b.kind, m])?);
    }
    if !matches!(b.kind.as_str(), "album" | "track") {
        return Ok(None);
    }
    let (Some(a), Some(t)) = (artist, title) else { return Ok(None) };
    Ok(st.db.one(&format!("SELECT * FROM requests WHERE kind=? AND mbid IS NULL AND lower(artist)=lower(?) AND lower(title)=lower(?) AND {ended}"), p![b.kind, a, t])?)
}

/// The `detail` of a request that is asked for again: the new request's detail plus the attempt count and
/// the previous attempt's log, ending with how it ended.
pub fn detail_for_retry(old_detail: &Value, old_status: &str, old_message: &str, mut detail: Value) -> Value {
    let attempts = old_detail["attempts"].as_u64().unwrap_or(1);
    let mut log: Vec<Value> = old_detail["log"].as_array().cloned().unwrap_or_default().into_iter()
        .map(|l| match l.as_str() { Some(s) if s.chars().count() > KEPT_LOG_CHARS => json!(s.chars().take(KEPT_LOG_CHARS).collect::<String>() + "…"), _ => l }).collect();
    let ended = format!("attempt {attempts} {old_status}: {}", old_message.chars().take(KEPT_LOG_CHARS).collect::<String>()).trim_end_matches([':', ' ']).to_string();
    log.push(json!(ended));
    if log.len() > KEPT_LOG_LINES {
        log.drain(..log.len() - KEPT_LOG_LINES);
    }
    detail["attempts"] = json!(attempts + 1);
    detail["log"] = json!(log);
    detail
}

/// `create_request` with the row to reuse known (a manual pick for an existing request), or found among
/// the failed and cancelled requests.
pub fn create_or_reuse(st: &AppState, u: &Row, b: &NewRequest, parent: Option<i64>, extra: Option<Value>, reuse: Option<i64>) -> ApiResult<i64> {
    // an "ext-…" id is a Deezer / Apple album from Discover search, not a MusicBrainz id (clients that
    // don't know ext albums send it as the mbid): request it as the ext album it is
    let fixed;
    let b = match nz(&b.mbid).filter(|m| m.starts_with("ext-")) {
        Some(m) => {
            let ext = match (b.kind.as_str(), ext_from_id(m)) {
                ("album", Some(e)) => e,
                _ => return Err(err(400, "That isn't a MusicBrainz id")),
            };
            let mut nb = b.clone();
            nb.mbid = None;
            let mut merged = b.ext.clone().filter(|e| e.is_object()).unwrap_or(json!({}));
            for (k, v) in ext.as_object().into_iter().flatten() {
                if merged[k].is_null() {
                    merged[k] = v.clone();
                }
            }
            nb.ext = Some(merged);
            fixed = nb;
            &fixed
        }
        None => b,
    };
    let has_extra = extra.is_some();
    let ext_ok = b.ext.as_ref().is_some_and(|e| EXT_SOURCES.iter().any(|k| !e[k].is_null() && e[k] != ""));
    let (artist, title) = (nz(&b.artist), nz(&b.title).or(nz(&b.album)));
    match b.kind.as_str() {
        "url" => {
            if !b.url.as_deref().is_some_and(|u| u.starts_with("http://") || u.starts_with("https://")) {
                return Err(err(400, "A valid http(s) URL is required"));
            }
        }
        // an album with no MusicBrainz id is fine when another catalogue knows it, or when a manual
        // pick (extra) says exactly what to download – a Soulseek folder, say
        "album" if nz(&b.mbid).is_none() && (ext_ok || has_extra) => {
            if artist.is_none() || title.is_none() {
                return Err(err(400, "Artist and album title are required"));
            }
            if !has_extra {
                dup(st, "SELECT id, status FROM requests WHERE kind='album' AND mbid IS NULL AND lower(artist)=lower(?) AND lower(title)=lower(?)
                         AND status NOT IN ('failed','denied','cancelled')", p![artist, title])?;
            }
        }
        "artist" | "album" if nz(&b.mbid).is_none() => return Err(err(400, "A MusicBrainz id is required")),
        "track" if nz(&b.mbid).is_none() && (artist.is_none() || nz(&b.title).is_none()) => return Err(err(400, "Track needs a MusicBrainz id or artist + title")),
        "artist" | "album" | "track" => {}
        _ => return Err(err(400, "Invalid request kind")),
    }
    if b.kind == "track" && nz(&b.mbid).is_none() && !has_extra {
        dup(st, "SELECT id, status FROM requests WHERE kind='track' AND lower(artist)=lower(?) AND lower(title)=lower(?) AND status NOT IN ('failed','denied','cancelled')",
            p![artist, b.title])?;
    }
    if let (Some(m), false) = (nz(&b.mbid), has_extra) {
        dup(st, "SELECT id, status FROM requests WHERE mbid=? AND kind=? AND status NOT IN ('failed','denied','cancelled')", p![m, b.kind])?;
    }
    let twin = match reuse {
        Some(id) => st.db.one("SELECT * FROM requests WHERE id=?", p![id])?,
        None if parent.is_none() => failed_twin(st, b, artist, title)?,
        None => None,
    };
    let approved = auth::is_admin(u) || u.b("auto_approve") || st.cfg.bool("auto_approve");
    let mut detail = json!({});
    if let Some(l) = b.length.filter(|l| *l > 0.0) {
        detail["length"] = json!(l);
    }
    if let Some(i) = nz(&b.isrc) {
        detail["isrc"] = json!(i);
    }
    if let Some(m) = b.album_mode.as_deref().filter(|m| *m == "song" || *m == "album") {
        detail["album_mode"] = json!(m);
    }
    if let (true, Some(f)) = (b.kind == "artist", b.release_filter.as_ref().filter(|f| f.is_object())) {
        detail["release_filter"] = crate::release_types::ReleaseFilter::from_value(f, &crate::release_types::ReleaseFilter::from_cfg(&st.cfg)).to_value();
    }
    if let (true, Some(ids)) = (b.kind == "artist", b.release_ids.as_ref()) {
        let mut seen: Vec<&str> = vec![];
        for id in ids.iter().map(|i| i.trim()).filter(|i| !i.is_empty()) {
            if !seen.contains(&id) {
                seen.push(id);
            }
        }
        if seen.is_empty() {
            return Err(err(400, "Tick at least one release"));
        }
        detail["release_ids"] = json!(seen);
    }

    if let Some(e) = &b.ext {
        let mut keep = serde_json::Map::new();
        for k in EXT_SOURCES.iter().chain(["upc"].iter()) {
            if !e[*k].is_null() && e[*k] != "" {
                keep.insert(k.to_string(), e[*k].clone());
            }
        }
        detail["ext"] = Value::Object(keep);
    }
    if let Some(Value::Object(x)) = extra {
        for (k, v) in x {
            detail[k] = v;
        }
    }
    let t = now();
    let what = title.or(artist).or(nz(&b.url)).unwrap_or("");
    if let Some(old) = twin {
        let id = old.i("id");
        let old_detail: Value = serde_json::from_str(&old.s("detail")).unwrap_or(json!({}));
        let detail = detail_for_retry(&old_detail, &old.s("status"), &old.s("message"), detail);
        st.db.execute(
            "UPDATE requests SET user_id=?, status=?, message=?, progress=0, source=NULL, detail=?, artist=COALESCE(?, artist), title=COALESCE(?, title),
             album=COALESCE(?, album), year=COALESCE(?, year), cover_url=COALESCE(?, cover_url), url=COALESCE(?, url), updated_at=? WHERE id=?",
            p![u.i("id"), if approved { "queued" } else { "pending" }, if approved { "Requested again" } else { "Waiting for admin approval" }, detail.to_string(),
               artist, title, nz(&b.album), nz(&b.year), nz(&b.cover_url), nz(&b.url), t, id])?;
        st.db.log("info", &format!("{} requested {} again (#{id}, attempt {}): {what}", u.s("username"), b.kind, detail["attempts"]));
        crate::req_events::emit(&st.db, id);
        return Ok(id);
    }
    let id = st.db.execute(
        "INSERT INTO requests (user_id, kind, mbid, artist, title, album, year, cover_url, url, status, message, detail, parent_id, created_at, updated_at)
         VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
        p![u.i("id"), b.kind, nz(&b.mbid), artist, title, nz(&b.album), nz(&b.year), nz(&b.cover_url), nz(&b.url),
           if approved { "queued" } else { "pending" }, if approved { "" } else { "Waiting for admin approval" }, detail.to_string(), parent, t, t])?;
    st.db.log("info", &format!("{} requested {}: {what}", u.s("username"), b.kind));
    crate::req_events::emit(&st.db, id);
    Ok(id)
}

fn get_request(st: &AppState, id: i64) -> ApiResult<Value> {
    let mut r = st.db.one("SELECT * FROM requests WHERE id=?", p![id])?.ok_or_else(|| err(404, "Request not found"))?;
    let d: Value = serde_json::from_str(&r.s("detail")).unwrap_or(json!({}));
    r.insert("detail".into(), d);
    Ok(Value::Object(r))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ListQ {
    status: Option<String>,
    mine: bool,
    limit: Option<i64>,
}

async fn list(State(st): State<AppState>, User(u): User, Query(q): Query<ListQ>) -> ApiResult<Json<Value>> {
    let mut sql = "SELECT r.*, u.username FROM requests r LEFT JOIN users u ON u.id=r.user_id WHERE 1=1".to_string();
    let mut params = vec![];
    if !auth::is_admin(&u) || q.mine {
        sql += " AND r.user_id=?";
        params.push(json!(u.i("id")));
    }
    match q.status.as_deref() {
        Some("active") => sql += " AND r.status IN ('pending','queued','searching','downloading','monitoring','importing')",
        Some(s) if !s.is_empty() => {
            sql += " AND r.status=?";
            params.push(json!(s));
        }
        _ => {}
    }
    sql += " ORDER BY r.id DESC LIMIT ?";
    params.push(json!(q.limit.unwrap_or(200).min(1000)));
    let rows = st.db.query(&sql, params)?;
    let out: Vec<Value> = rows.into_iter().map(|mut r| {
        let d: Value = serde_json::from_str(&r.s("detail")).unwrap_or(json!({}));
        r.remove("detail");
        r.insert("log".into(), d.get("log").cloned().unwrap_or(json!([])));
        r.insert("length".into(), d["length"].clone());
        r.insert("attempts".into(), json!(d["attempts"].as_u64().unwrap_or(1)));
        r.insert("picked".into(), json!(d["pick"].is_object()));
        let mut cand = Value::Null;
        let idx = d["slskd"]["idx"].as_i64().unwrap_or(-1);
        if let Some(c) = d["slskd"]["candidates"].as_array().and_then(|a| a.get(idx.max(0) as usize)).filter(|_| idx >= 0) {
            let dir = c["directory"].as_str().unwrap_or("").replace('\\', "/");
            cand = json!({ "username": c["username"], "format": c["format"], "directory": dir.rsplit('/').next().unwrap_or(""),
                           "files": c["files"].as_array().map(|a| a.len()).unwrap_or(0) });
        }
        let tidx = d["torznab"]["idx"].as_i64().unwrap_or(-1);
        if let Some(c) = d["torznab"]["candidates"].as_array().and_then(|a| a.get(tidx.max(0) as usize)).filter(|_| tidx >= 0) {
            cand = json!({ "username": c["indexer"], "format": if c["lossless"].as_bool().unwrap_or(false) { "flac" } else { "" }, "directory": c["title"], "files": null });
        }
        r.insert("candidate".into(), cand);
        Value::Object(r)
    }).collect();
    Ok(Json(json!(out)))
}

async fn create(State(st): State<AppState>, User(u): User, Json(b): Json<NewRequest>) -> ApiResult<Json<Value>> {
    quota(&st, &u, 1)?;
    let id = create_request(&st, &u, &b, None, None)?;
    let r = get_request(&st, id)?;
    if r["status"] == "pending" {
        let what = [&r["title"], &r["artist"], &r["url"]].iter().find_map(|v| v.as_str().filter(|s| !s.is_empty())).unwrap_or("").to_string();
        let (st2, who, kind, cover) = (st.clone(), u.s("username"), b.kind.clone(), b.cover_url.clone());
        tokio::spawn(async move {
            notify::send(&st2, "🎵 New request awaiting approval", &format!("{who} requested {kind}: {what}"), cover.as_deref()).await;
        });
    }
    Ok(Json(r))
}

#[derive(Deserialize)]
struct LinkBody {
    url: String,
    album_mode: Option<String>,
}

/// Request every song behind a link (one song, an album or a playlist) from the best source.
async fn from_link_inner(st: &AppState, u: &Row, url: &str, album_mode: Option<String>) -> ApiResult<Value> {
    let data = crate::sources::links::fetch_as(st, url, Some(u.i("id"))).await.map_err(|e| err(400, e.to_string()))?;
    quota(st, u, 1)?;
    let (mut made, mut skipped) = (vec![], 0);
    for t in data["tracks"].as_array().cloned().unwrap_or_default().iter().take(500) {
        let s = |k: &str| t[k].as_str().filter(|x| !x.is_empty()).map(String::from);
        let b = NewRequest { kind: "track".into(), artist: s("artist"), title: s("title"), album: s("album"), cover_url: s("cover"),
            length: t["duration"].as_f64(), isrc: s("isrc"), mbid: s("mbid"), album_mode: album_mode.clone(), ..Default::default() };
        match create_request(st, u, &b, None, None) {
            Ok(id) => made.push(id),
            Err(e) if e.status.as_u16() == 409 => skipped += 1,
            Err(e) => return Err(e),
        }
    }
    let status = match made.first() { Some(id) => get_request(st, *id)?["status"].clone(), None => Value::Null };
    Ok(json!({ "requested": made.len(), "already_requested": skipped, "name": data["name"], "status": status, "ids": made }))
}

async fn from_link(State(st): State<AppState>, User(u): User, Json(b): Json<LinkBody>) -> ApiResult<Json<Value>> {
    Ok(Json(from_link_inner(&st, &u, b.url.trim(), b.album_mode).await?))
}

// links that only describe music (no downloadable audio) → matched song by song
static METADATA_ONLY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)open\.spotify\.com|spotify\.link|^spotify:|music\.apple\.com|listenbrainz\.org|last\.fm/|tidal\.com|qobuz\.com|deezer\.com|deezer\.page\.link|music\.amazon\.|amazon\.[a-z.]+/music/player/|amzn\.to/|music\.youtube\.com/(playlist|browse)").unwrap());
static RIP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"qobuz\.com|tidal\.com|deezer\.com").unwrap());

/// One box for any link: streaming-service links are matched song by song (best quality from any source);
/// Qobuz/Tidal/Deezer rip directly when streamrip is set up; everything else downloads as-is with yt-dlp.
async fn link(State(st): State<AppState>, User(u): User, Json(b): Json<LinkBody>) -> ApiResult<Json<Value>> {
    let url = b.url.trim().to_string();
    if !(url.starts_with("http://") || url.starts_with("https://") || url.starts_with("spotify:")) {
        return Err(err(400, "Paste a full link (https://…)"));
    }
    let rip = RIP.is_match(&url) && crate::sources::streamrip::enabled(&st);
    if METADATA_ONLY.is_match(&url) && !rip {
        let mut r = from_link_inner(&st, &u, &url, b.album_mode).await?;
        r["mode"] = json!("matched");
        return Ok(Json(r));
    }
    quota(&st, &u, 1)?;
    let id = create_request(&st, &u, &NewRequest { kind: "url".into(), url: Some(url), ..Default::default() }, None, None)?;
    Ok(Json(json!({ "requested": 1, "already_requested": 0, "ids": [id], "status": get_request(&st, id)?["status"], "mode": "direct" })))
}

pub fn get_owned(st: &AppState, id: i64, u: &Row) -> ApiResult<Value> {
    let r = get_request(st, id)?;
    if !auth::is_admin(u) && r["user_id"].as_i64() != Some(u.i("id")) {
        return Err(err(403, "Not your request"));
    }
    Ok(r)
}

fn family(st: &AppState, id: i64) -> Vec<i64> {
    let mut v = vec![id];
    v.extend(st.db.query("SELECT id FROM requests WHERE parent_id=?", p![id]).unwrap_or_default().into_iter().map(|r| r.i("id")));
    v
}

fn set(st: &AppState, id: i64, status: &str, message: &str) -> Result<i64, ApiError> {
    let n = st.db.execute("UPDATE requests SET status=?, message=?, updated_at=? WHERE id=?", p![status, message, now(), id])?;
    crate::req_events::emit(&st.db, id);
    Ok(n)
}

async fn approve(State(st): State<AppState>, Admin(_): Admin, Path(rid): Path<i64>) -> ApiResult<Json<Value>> {
    for i in family(&st, rid) {
        if let Ok(r) = get_request(&st, i) {
            if r["status"] == "pending" {
                set(&st, i, if r["kind"] == "playlist" { "completed" } else { "queued" }, "Approved")?;
            }
        }
    }
    Ok(Json(json!({ "ok": true })))
}

async fn deny(State(st): State<AppState>, Admin(a): Admin, Path(rid): Path<i64>) -> ApiResult<Json<Value>> {
    for i in family(&st, rid) {
        if let Ok(r) = get_request(&st, i) {
            if r["status"] == "pending" {
                set(&st, i, "denied", &format!("Denied by {}", a.s("username")))?;
            }
        }
    }
    Ok(Json(json!({ "ok": true })))
}

async fn retry(State(st): State<AppState>, User(u): User, Path(rid): Path<i64>) -> ApiResult<Json<Value>> {
    let r = get_owned(&st, rid, &u)?;
    let reset = "UPDATE requests SET status='queued', progress=0, message='Retrying', source=NULL, detail=?, updated_at=? WHERE id=?";
    if r["kind"] == "playlist" {
        for c in st.db.query("SELECT id FROM requests WHERE parent_id=? AND status IN ('failed','cancelled')", p![rid])? {
            st.db.execute(reset, p!["{}", now(), c.i("id")])?;
            crate::req_events::emit(&st.db, c.i("id"));
        }
        return Ok(Json(json!({ "ok": true })));
    }
    let status = r["status"].as_str().unwrap_or("");
    if !["failed", "cancelled", "completed"].contains(&status) {
        return Err(err(400, "Only finished requests can be retried"));
    }
    let admin = auth::is_admin(&u);
    if !admin && !auth::can(&st, &u, "fix") {
        return Err(err(403, "Ask an admin to retry"));
    }
    if status == "cancelled" && !admin && !(u.b("auto_approve") || st.cfg.bool("auto_approve")) {
        return Err(err(403, "Ask an admin to retry"));
    }
    let mut keep = json!({});
    for k in ["length", "isrc"] {
        if !r["detail"][k].is_null() {
            keep[k] = r["detail"][k].clone();
        }
    }
    let keep = detail_for_retry(&r["detail"], status, r["message"].as_str().unwrap_or(""), keep);
    st.db.execute(reset, p![keep.to_string(), now(), rid])?;
    crate::req_events::emit(&st.db, rid);
    Ok(Json(json!({ "ok": true })))
}

async fn cancel(State(st): State<AppState>, User(u): User, Path(rid): Path<i64>) -> ApiResult<Json<Value>> {
    get_owned(&st, rid, &u)?;
    for i in family(&st, rid) {
        if let Ok(c) = get_request(&st, i) {
            let s = c["status"].as_str().unwrap_or("");
            if s == "pending" || ACTIVE.contains(&s) {
                worker::cancel(&st, i).await;
                set(&st, i, "cancelled", &format!("Cancelled by {}", u.s("username")))?;
            }
        }
    }
    Ok(Json(json!({ "ok": true })))
}

/// Remove what a request left on disk: imported songs (completed) and staging/partial files.
fn delete_files(st: &AppState, r: &Value) -> usize {
    let id = r["id"].as_i64().unwrap_or(0);
    let ids: Vec<i64> = st.db.query("SELECT id FROM tracks WHERE request_id IN (SELECT id FROM requests WHERE id=? OR parent_id=?)", p![id, id])
        .unwrap_or_default().into_iter().map(|t| t.i("id")).collect();
    let mut n = library::delete_tracks(&st.db, &ids, None);
    for i in family(st, id) {
        let stage = std::path::PathBuf::from(st.cfg.str("staging_path")).join(format!("request-{i}"));
        if stage.is_dir() {
            let _ = std::fs::remove_dir_all(&stage);
            n += 1;
        }
    }
    let idx = r["detail"]["slskd"]["idx"].as_i64().unwrap_or(-1);
    if let (Some(c), true) = (r["detail"]["slskd"]["candidates"].get(idx.max(0) as usize), idx >= 0 && r["status"] != "completed") {
        let root = crate::slskd_managed::download_dir(st); // where the files landed (the fallback folder when that is in use)
        for f in c["files"].as_array().cloned().unwrap_or_default() {
            if let Some(pth) = crate::sources::slskd::local_path(st, f["filename"].as_str().unwrap_or("")) {
                let _ = std::fs::remove_file(&pth);
                library::prune_empty(pth.parent().unwrap(), &root);
                n += 1;
            }
        }
    }
    n
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct FilesQ {
    files: bool,
}

/// Remove a request from the list; files=true also deletes what it downloaded (admins only).
async fn remove(State(st): State<AppState>, User(u): User, Path(rid): Path<i64>, Query(q): Query<FilesQ>) -> ApiResult<Json<Value>> {
    let r = get_owned(&st, rid, &u)?;
    if q.files && !auth::is_admin(&u) {
        return Err(err(403, "Only admins can delete files"));
    }
    if ACTIVE.contains(&r["status"].as_str().unwrap_or("")) {
        worker::cancel(&st, rid).await;
    }
    let n = if q.files { delete_files(&st, &r) } else { 0 };
    st.db.execute("DELETE FROM requests WHERE id=? OR parent_id=?", p![rid, rid])?;
    if n > 0 {
        st.db.log("info", &format!("{} deleted request #{rid} and {n} file(s)", u.s("username")));
    }
    Ok(Json(json!({ "ok": true, "deleted_files": n })))
}

#[derive(Deserialize)]
#[serde(default)]
struct ClearBody {
    statuses: Vec<String>,
    files: bool,
    mine: bool,
}
impl Default for ClearBody {
    fn default() -> Self {
        ClearBody { statuses: vec!["completed".into(), "denied".into(), "cancelled".into()], files: false, mine: false }
    }
}

/// Clear finished requests from the list (everyone's for admins), optionally deleting their files.
async fn clear(State(st): State<AppState>, User(u): User, body: Option<Json<ClearBody>>) -> ApiResult<Json<Value>> {
    let b = body.map(|j| j.0).unwrap_or_default();
    let statuses: Vec<String> = b.statuses.into_iter().filter(|s| ["completed", "failed", "denied", "cancelled"].contains(&s.as_str())).collect();
    if statuses.is_empty() {
        return Err(err(400, "Nothing to clear"));
    }
    if b.files && !auth::is_admin(&u) {
        return Err(err(403, "Only admins can delete files"));
    }
    let mut sql = format!("SELECT id FROM requests WHERE parent_id IS NULL AND status IN ({})", vec!["?"; statuses.len()].join(","));
    let mut params: Vec<Value> = statuses.iter().map(|s| json!(s)).collect();
    if !auth::is_admin(&u) || b.mine {
        sql += " AND user_id=?";
        params.push(json!(u.i("id")));
    }
    let (mut removed, mut files) = (0, 0);
    for row in st.db.query(&sql, params)? {
        let id = row.i("id");
        if b.files {
            if let Ok(r) = get_request(&st, id) {
                files += delete_files(&st, &r);
            }
        }
        st.db.execute("DELETE FROM requests WHERE id=? OR parent_id=?", p![id, id])?;
        removed += 1;
    }
    Ok(Json(json!({ "ok": true, "removed": removed, "deleted_files": files })))
}

#[cfg(test)]
#[path = "labels_tests.rs"]
mod labels_tests;

#[cfg(test)]
#[path = "requests_tests.rs"]
mod requests_tests;

#[cfg(test)]
mod ext_id_tests {
    use super::*;

    #[test]
    fn ext_ids_name_their_catalogues() {
        assert_eq!(ext_from_id("ext-d302127"), Some(json!({ "deezer": 302127 })));
        assert_eq!(ext_from_id("ext-d2-a3"), Some(json!({ "deezer": 2, "apple": 3 })));
        assert_eq!(ext_from_id("ext-x"), None);
        assert_eq!(ext_from_id("9e1f2c84-1111-2222-3333-444455556666"), None);
    }
}
