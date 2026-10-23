//! /api/roon: Roon zones for the devices list – what plays where, transport, volume, covers – and
//! "Play in Roon" (a search in Roon's library started in a zone). The settings card's status comes from
//! /api/system/test/roon.
use crate::auth::User;
use encore_core::error::{err, ApiResult};
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

/// The Roon routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/roon/zones", get(zones))
        .route("/api/roon/zones/{id}/command", post(command))
        .route("/api/roon/zones/{id}/play", post(play))
        .route("/api/roon/image/{key}", get(image))
}

/// GET /api/roon/zones: the Roon zones ([] while Roon is off).
async fn zones(State(st): State<AppState>, User(_): User) -> Json<Value> {
    if !st.cfg.bool("roon_enabled") { return Json(json!([])); }
    Json(json!(st.playback.roon.zones()))
}

/// `POST /api/roon/zones/{id}/command` body.
#[derive(Deserialize)]
struct Cmd {
    /// play, pause, stop, toggle, next, prev, seek or volume
    action: String,
    /// the seek position (seconds) or the volume
    #[serde(default)]
    value: Option<f64>,
}

/// POST /api/roon/zones/{id}/command: control a zone.
async fn command(State(st): State<AppState>, User(_): User, Path(id): Path<String>, Json(c): Json<Cmd>) -> ApiResult<Json<Value>> {
    st.playback.roon.control(&id, &c.action, c.value).await.map_err(|e| err(502, e.to_string()))?;
    Ok(Json(json!({ "ok": true })))
}

/// `POST /api/roon/zones/{id}/play` body.
#[derive(Deserialize)]
struct PlayQ {
    /// what to search for and play
    query: String,
}

/// POST /api/roon/zones/{id}/play: search Roon's library and play the first album (or song) it finds in a zone.
async fn play(State(st): State<AppState>, User(_): User, Path(id): Path<String>, Json(b): Json<PlayQ>) -> ApiResult<Json<Value>> {
    let q = b.query.trim();
    if q.is_empty() { return Err(err(400, "Type what to play")); }
    let what = st.playback.roon.play_search(&id, q).await.map_err(|e| err(502, e.to_string()))?;
    Ok(Json(json!({ "ok": true, "playing": what })))
}

/// `GET /api/roon/image/{key}` query.
#[derive(Deserialize, Default)]
#[serde(default)]
struct SizeQ {
    /// the size in pixels (0 = 300; 32 to 1200)
    s: u32,
}

/// GET /api/roon/image/{key}?s=: a Roon image, cached by the browser for 10 minutes.
async fn image(State(st): State<AppState>, User(_): User, Path(key): Path<String>, Query(q): Query<SizeQ>) -> ApiResult<Response> {
    let size = if q.s == 0 { 300 } else { q.s.clamp(32, 1200) };
    let (ct, bytes) = st.playback.roon.image(&key, size).await.map_err(|e| err(502, e.to_string()))?;
    Ok(([(header::CONTENT_TYPE, ct), (header::CACHE_CONTROL, "private, max-age=600".to_string())], bytes).into_response())
}
