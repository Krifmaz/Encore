//! Report a problem with a song or an album (⋯ → Report a problem): the exact source the file came from
//! is blocked (never picked again), the file goes to <config>/trash (kept 7 days), and the same song or
//! album is requested again with that source left out and other users / services tried first. The
//! replacement takes over the reported track's id, so plays, likes, playlists and resume spots stay.
//! Settings → Blocked lists the blocks (with Undo), the reports and the songs the quality checks flagged.
//! Wrong lyrics / Lyrics out of sync keep the song: only its lyrics are rejected and looked up again.
use crate::enrich::enrich;
use crate::auth::{Admin, User};
use encore_db::RowExt;
use encore_core::error::{err, ApiResult};
use crate::state::AppState;
use encore_db::p;
use axum::extract::{Path, State};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde_json::{json, Value};

// The report itself (blocks, the trash, the replacement) lives in encore_requests::report.
use encore_requests::report::*;

/// The report routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/report", post(report))
        .route("/api/reports", get(list))
        .route("/api/reports/{id}/retry", post(retry))
        .route("/api/reports/{id}/dismiss", post(dismiss))
        .route("/api/blocks", get(blocks))
        .route("/api/blocks/{id}", delete(unblock))
        .route("/api/quality/suspect", get(suspect))
        .route("/api/quality/tracks/{id}/check", post(check_track))
}

/// POST /api/report: file a report on songs or an album (a lyrics reason answers once other lyrics
/// were looked up, with them as `lyrics`).
async fn report(State(st): State<AppState>, User(u): User, Json(b): Json<ReportBody>) -> ApiResult<Json<Value>> {
    Ok(Json(file(&st, &u, &b).await?))
}

/// Admins see every report, everyone else their own.
async fn list(State(st): State<AppState>, User(u): User) -> ApiResult<Json<Value>> {
    reconcile(&st);
    let admin = encore_config::perms::is_admin(&u);
    let ids = if admin {
        st.db.query("SELECT id FROM reports ORDER BY id DESC LIMIT 200", p![])?
    } else {
        st.db.query("SELECT id FROM reports WHERE user_id=? ORDER BY id DESC LIMIT 100", p![u.i("id")])?
    };
    let items: Vec<Value> = ids.iter().filter_map(|r| report_json(&st, r.i("id")).ok()).collect();
    let mut v = json!({ "reports": items, "reasons": REASONS.iter().map(|(k, l)| json!({ "key": k, "label": l, "lyrics": is_lyrics_reason(k) })).collect::<Vec<_>>() });
    enrich(&st, Some(&u), &mut v).await;
    Ok(Json(v))
}

/// Try again: the songs that came back are parked again and requested once more (blocks stay).
async fn retry(State(st): State<AppState>, User(u): User, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    let r = st.db.one("SELECT * FROM reports WHERE id=?", p![id])?.ok_or_else(|| err(404, "Report not found"))?;
    if !encore_config::perms::is_admin(&u) && r.i("user_id") != u.i("id") {
        return Err(err(403, "Not your report"));
    }
    if is_lyrics_reason(&r.s("reason")) {
        if r.s("status") == "lyrics_searching" && r.f("updated_at") > encore_db::now() - 60.0 {
            return Err(err(409, "Already looking for other lyrics"));
        }
        return Ok(Json(retry_lyrics(&st, &r).await?));
    }
    Ok(Json(retry_report(&st, &u, &r)?))
}

/// POST /api/reports/{id}/dismiss: dismiss a report (admins).
async fn dismiss(State(st): State<AppState>, Admin(_): Admin, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    dismiss_report(&st, id).await?;
    Ok(Json(json!({ "ok": true })))
}

/// GET /api/blocks: the source blocks reports made, newest first (admins, at most 500).
async fn blocks(State(st): State<AppState>, Admin(viewer): Admin) -> ApiResult<Json<Value>> {
    let rows = st.db.query("SELECT b.*, u.username, r.reason, r.artist, r.title, r.kind report_kind FROM source_block b LEFT JOIN users u ON u.id=b.user_id
                            LEFT JOIN reports r ON r.id=b.report_id ORDER BY b.id DESC LIMIT 500", p![])?;
    let items: Vec<Value> = rows.into_iter().map(|b| {
        let mut v = Value::Object(b);
        v["reason_label"] = json!(REASONS.iter().find(|(k, _)| *k == v["reason"]).map(|(_, l)| *l).unwrap_or(""));
        v
    }).collect();
    let mut v = json!({ "blocks": items });
    enrich(&st, Some(&viewer), &mut v).await;
    Ok(Json(v))
}

/// Undo a block: that source may be picked again.
async fn unblock(State(st): State<AppState>, Admin(u): Admin, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    let b = st.db.one("SELECT * FROM source_block WHERE id=?", p![id])?.ok_or_else(|| err(404, "Block not found"))?;
    st.db.execute("DELETE FROM source_block WHERE id=?", p![id])?;
    st.db.log("info", &format!("{} unblocked {}", u.s("username"), b.s("label")));
    Ok(Json(json!({ "ok": true })))
}

/// Songs the quality checks flagged (newest first), for Settings → Blocked.
async fn suspect(State(st): State<AppState>, Admin(viewer): Admin) -> ApiResult<Json<Value>> {
    let rows = st.db.query("SELECT t.id, t.title, t.artist, t.album, t.album_id, t.format, t.bitrate, q.summary, q.issues, q.checked_at FROM track_quality q
                            JOIN tracks t ON t.id=q.track_id WHERE q.status='suspect' ORDER BY q.checked_at DESC LIMIT 300", p![])?;
    let items: Vec<Value> = rows.into_iter().map(|r| {
        let mut v = Value::Object(r);
        v["issues"] = serde_json::from_str(v["issues"].as_str().unwrap_or("[]")).unwrap_or(json!([]));
        v
    }).collect();
    let mut v = json!({ "tracks": items });
    enrich(&st, Some(&viewer), &mut v).await;
    Ok(Json(v))
}

/// Check one song now (Signal path / track info show the result).
async fn check_track(State(st): State<AppState>, Admin(_): Admin, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    let t = st.db.one("SELECT id, path FROM tracks WHERE id=?", p![id])?.ok_or_else(|| err(404, "Song not found"))?;
    let rep = encore_media::quality::analyze(&st.media, std::path::Path::new(&t.s("path")), None, encore_media::quality::min_lufs(&st), false).await;
    encore_media::quality::store(&st, std::path::Path::new(&t.s("path")), &rep);
    Ok(Json(encore_media::quality::for_track(&st, id)))
}

#[cfg(test)]
#[path = "report_tests.rs"]
mod tests;
