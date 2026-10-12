//! /api/playback: what the server can do for playback (server-side Cadenza processing).
use crate::auth::{self, User};
use crate::dsp;
use crate::state::AppState;
use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{json, Value};

pub fn router() -> Router<AppState> {
    Router::new().route("/api/playback/dsp", get(dsp_info))
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
    }))
}
