//! Encore's own updates (Settings → Diagnostics → Updates):
//!   GET  /api/system/update[?force=1]  (admin) → {current, latest, is_newer, notes, url, published_at, checked_at,
//!        restart: "ready", self_update: "ready" | "unavailable", self_update_reason, image, …}
//!   POST /api/system/update {tag?}     (admin) → 202: the new image is pulled and the helper container started
//!   POST /api/system/restart           (admin) → 202: the server stops gracefully and exits with code 75
//! What they do is encore_system::update's.
use crate::auth::Admin;
use encore_db::RowExt;
use encore_core::error::{err, ApiResult};
use crate::state::AppState;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::Json;
use encore_system::update;
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize)]
pub struct UpdateQ {
    force: Option<String>,
}

pub async fn get_update(State(st): State<AppState>, Admin(_): Admin, Query(q): Query<UpdateQ>) -> ApiResult<Json<Value>> {
    let force = matches!(q.force.as_deref(), Some("1" | "true" | "yes"));
    Ok(Json(update::status(&st, force).await))
}

#[derive(Deserialize, Default)]
pub struct UpdateBody {
    tag: Option<String>,
}

pub async fn post_update(State(st): State<AppState>, Admin(u): Admin, body: Option<Json<UpdateBody>>) -> ApiResult<(StatusCode, Json<Value>)> {
    match update::update_now(&st, &u.s("username"), body.and_then(|b| b.0.tag)).await {
        Ok(v) => Ok((StatusCode::ACCEPTED, Json(v))),
        Err(update::Refused(code, why)) => Err(err(code, why)),
    }
}

/// "Restart server": 202 at once, the restart half a second later.
pub async fn post_restart(State(st): State<AppState>, Admin(u): Admin) -> ApiResult<(StatusCode, Json<Value>)> {
    update::restart(&st, &u.s("username"));
    Ok((StatusCode::ACCEPTED, Json(json!({ "ok": true, "message": "Restarting" }))))
}

#[cfg(test)]
#[path = "update_tests.rs"]
mod tests;
