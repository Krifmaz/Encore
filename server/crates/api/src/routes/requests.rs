//! /api/requests: ask for artists, albums, songs or any link; approve, retry, cancel, clear.
use crate::enrich::enrich_now;
use crate::auth::{self, Admin, User};
use encore_db::{now, Row, RowExt};
use encore_core::error::{err, ApiError, ApiResult};
use crate::state::AppState;
use encore_requests::worker::{self, ACTIVE};
use encore_requests::notify;
use encore_db::p;
use axum::extract::{Path, Query, State};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use regex::Regex;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::LazyLock;

// The new request itself (checks, the row) lives in encore_requests::create.
use encore_requests::create::*;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/requests", get(list).post(create))
        .route("/api/requests/from-link", post(from_link))
        .route("/api/requests/link", post(link))
        .route("/api/requests/clear", post(clear))
        .route("/api/requests/retry-failed", post(retry_failed))
        .route("/api/requests/events", get(crate::routes::request_events::events))
        .route("/api/requests/{rid}", delete(remove))
        .route("/api/requests/{rid}/approve", post(approve))
        .route("/api/requests/{rid}/deny", post(deny))
        .route("/api/requests/{rid}/retry", post(retry))
        .route("/api/requests/{rid}/cancel", post(cancel))
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
    if !encore_config::perms::is_admin(&u) || q.mine {
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
    // one hop to the blocking pool for the rows, their `detail` JSON (up to 1000 of them, with Soulseek's
    // candidate lists) and the enrichment
    st.blocking(move |st| {
        let rows = st.db.read_now(|c| c.query(&sql, params))?;
        let mut v = json!(list_rows(rows));
        enrich_now(st, Some(&u), &mut v);
        Ok(Json(v))
    }).await
}

/// The requests list's rows as the page shows them: the `detail` JSON read for the log, the length, the
/// attempts, whether it was picked by hand and the candidate being downloaded.
fn list_rows(rows: Vec<Row>) -> Vec<Value> {
    rows.into_iter().map(|mut r| {
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
    }).collect()
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
    let data = encore_requests::sources::links::fetch_as(st, url, Some(u.i("id"))).await.map_err(|e| err(400, e.to_string()))?;
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
    // whether streamrip runs asks its program for its version the first time: on the blocking pool
    let rip = RIP.is_match(&url) && st.blocking(|st| Ok(encore_requests::sources::streamrip::enabled(st))).await?;
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
    if !encore_config::perms::is_admin(u) && r["user_id"].as_i64() != Some(u.i("id")) {
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
    encore_requests::events::emit(st, id);
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
    retry_request(&st, &u, rid, &r)?;
    Ok(Json(json!({ "ok": true })))
}

/// Retry failed (the button beside Clear): every failed top-level request the caller may retry is queued
/// again in one go. Admins retry everyone's unless `mine` is set.
#[derive(serde::Deserialize, Default)]
struct RetryFailedBody {
    #[serde(default)]
    mine: bool,
}
async fn retry_failed(State(st): State<AppState>, User(u): User, body: Option<Json<RetryFailedBody>>) -> ApiResult<Json<Value>> {
    let b = body.map(|j| j.0).unwrap_or_default();
    let mut sql = "SELECT id FROM requests WHERE parent_id IS NULL AND status='failed'".to_string();
    let mut params: Vec<Value> = vec![];
    if !encore_config::perms::is_admin(&u) || b.mine {
        sql += " AND user_id=?";
        params.push(json!(u.i("id")));
    }
    let (mut retried, mut skipped) = (0, 0);
    let mut why: Option<String> = None;
    for row in st.db.query(&sql, params)? {
        let id = row.i("id");
        let Ok(r) = get_request(&st, id) else { continue };
        match retry_request(&st, &u, id, &r) {
            Ok(()) => retried += 1,
            Err(e) => { skipped += 1; why.get_or_insert(e.detail.clone()); }
        }
    }
    Ok(Json(json!({ "retried": retried, "skipped": skipped, "why": why })))
}

/// Queue one finished request again (the per-row Retry and Retry failed share this).
fn retry_request(st: &AppState, u: &Row, rid: i64, r: &Value) -> ApiResult<()> {
    let reset = "UPDATE requests SET status='queued', progress=0, message='Retrying', source=NULL, detail=?, updated_at=? WHERE id=?";
    if r["kind"] == "playlist" {
        for c in st.db.query("SELECT id FROM requests WHERE parent_id=? AND status IN ('failed','cancelled')", p![rid])? {
            st.db.execute(reset, p!["{}", now(), c.i("id")])?;
            encore_requests::events::emit(st, c.i("id"));
        }
        return Ok(());
    }
    let status = r["status"].as_str().unwrap_or("");
    if !["failed", "cancelled", "completed"].contains(&status) {
        return Err(err(400, "Only finished requests can be retried"));
    }
    let admin = encore_config::perms::is_admin(u);
    if !admin && !auth::can(st, u, "fix") {
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
    encore_requests::events::emit(st, rid);
    Ok(())
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
    let mut n = encore_library::delete_tracks(st, &ids, None);
    for i in family(st, id) {
        let stage = std::path::PathBuf::from(st.cfg.str("staging_path")).join(format!("request-{i}"));
        if stage.is_dir() {
            let _ = std::fs::remove_dir_all(&stage);
            n += 1;
        }
    }
    let idx = r["detail"]["slskd"]["idx"].as_i64().unwrap_or(-1);
    if let (Some(c), true) = (r["detail"]["slskd"]["candidates"].get(idx.max(0) as usize), idx >= 0 && r["status"] != "completed") {
        let root = encore_requests::slskd_managed::download_dir(st); // where the files landed (the fallback folder when that is in use)
        for f in c["files"].as_array().cloned().unwrap_or_default() {
            if let Some(pth) = encore_requests::sources::slskd::local_path(st, f["filename"].as_str().unwrap_or("")) {
                let _ = std::fs::remove_file(&pth);
                encore_library::prune_empty(pth.parent().unwrap(), &root);
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
    if q.files && !encore_config::perms::is_admin(&u) {
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
    if b.files && !encore_config::perms::is_admin(&u) {
        return Err(err(403, "Only admins can delete files"));
    }
    let mut sql = format!("SELECT id FROM requests WHERE parent_id IS NULL AND status IN ({})", vec!["?"; statuses.len()].join(","));
    let mut params: Vec<Value> = statuses.iter().map(|s| json!(s)).collect();
    if !encore_config::perms::is_admin(&u) || b.mine {
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
