//! /api/playback: what the server can do for playback (server-side Cadenza processing).
use crate::auth::{self, Admin, User};
use crate::error::{err, ApiResult};
use crate::db::RowExt;
use crate::dsp;
use crate::state::AppState;
use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/playback/dsp", get(dsp_info))
        .route("/api/playback/dsp/prepare", post(dsp_prepare))
        .route("/api/playback/dsp/cache/clear", post(dsp_cache_clear))
}

/// Whether this user can have streams upsampled on the server, and the choices on offer.
async fn dsp_info(State(st): State<AppState>, User(u): User) -> Json<Value> {
    let lim = dsp::Limits::of(&st);
    let filters: Vec<Value> = dsp::filters(lim.max_taps).iter().map(|f| json!([f.id(), f.label()])).collect();
    let dithers: Vec<Value> = cadenza_dsp::DitherKind::catalog().iter().map(|d| json!([d.id(), d.label()])).collect();
    Json(json!({
        "enabled": lim.enabled,
        "allowed": lim.enabled && auth::can(&st, &u, "server_dsp"),
        "max_rate": lim.max_rate,
        "max_taps": lim.max_taps,
        "active": dsp::active(),
        "max_streams": lim.max_streams,
        "filters": filters,
        "dithers": dithers,
        "default_filter": dsp::default_filter(lim.max_taps).id(),
        "default_dither": cadenza_dsp::DitherKind::Tpdf.id(),
        "cache": dsp::render::usage(&st),
    }))
}

#[derive(serde::Deserialize, Default)]
#[serde(default)]
struct PrepareQ {
    track_id: i64,
    /// output rate in Hz (0 = the highest multiple of the source rate the server allows)
    rate: u32,
    filter: String,
    dither: String,
}

/// Render a song ahead (the player asks for the next one as the current one starts), so the stream
/// request later finds it cached or rendering. {state: cached | rendering | queued | busy}.
async fn dsp_prepare(State(st): State<AppState>, User(u): User, Json(q): Json<PrepareQ>) -> ApiResult<Json<Value>> {
    let lim = dsp::Limits::of(&st);
    if !lim.enabled {
        return Err(err(403, "Server processing is turned off on this server"));
    }
    if !auth::can(&st, &u, "server_dsp") {
        return Err(err(403, "Your account can't use server processing"));
    }
    let t = st.db.one("SELECT path FROM tracks WHERE id=?", crate::p![q.track_id])?.ok_or_else(|| err(404, "Track not found"))?;
    let path = t.s("path");
    if !std::path::Path::new(&path).is_file() {
        return Err(err(404, "Track not found"));
    }
    let Some(ctx) = dsp::render::Ctx::of(&st) else {
        return Ok(Json(json!({"state": "busy", "reason": "cache_off"})));
    };
    let spec = dsp::render::Spec::new(&st, q.track_id, path, q.rate, &q.filter, &q.dither).await;
    Ok(Json(dsp::render::prepare(&ctx, &spec, u.i("id"))))
}

/// Admin: stop every render and delete the rendered songs (filter coefficients stay).
async fn dsp_cache_clear(State(st): State<AppState>, Admin(_): Admin) -> Json<Value> {
    let dir = dsp::render::renders_dir(st.data_dir());
    let (files, mb) = tokio::task::spawn_blocking(move || dsp::render::clear(&dir)).await.unwrap_or((0, 0.0));
    Json(json!({"ok": true, "removed": files, "freed_mb": mb, "cache": dsp::render::usage(&st)}))
}
