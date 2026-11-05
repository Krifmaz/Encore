//! /api/admin/lantern: the Lantern card of Settings → Connections (admins only; every change carries the
//! CSRF header, which the `Admin` extractor checks). Save or forget Lantern's address and API key, and the
//! one-time move of the download settings into Lantern: its dry run, and the move itself. The connection
//! test is the kit's POST /api/system/test/lantern (routes::system::run_test). The code is
//! encore_requests::lantern.
use crate::auth::Admin;
use crate::state::AppState;
use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use encore_core::error::{err, ApiResult};
use encore_db::RowExt;
use encore_requests::lantern::{self, migrate};
use serde::Deserialize;
use serde_json::{json, Value};

/// The routes of the Lantern card.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/admin/lantern", get(state).put(save).delete(disconnect))
        .route("/api/admin/lantern/migrate", post(migrate_settings))
}

/// GET /api/admin/lantern: the connection's state (never the key).
async fn state(State(st): State<AppState>, Admin(_): Admin) -> ApiResult<Json<Value>> {
    Ok(Json(lantern::state(&st)))
}

/// `PUT /api/admin/lantern` body.
#[derive(Deserialize)]
struct Save {
    /// Lantern's address
    url: String,
    /// its API key; left out (or the masked form) keeps the saved one
    #[serde(default)]
    api_key: Option<String>,
}

/// PUT /api/admin/lantern: save the address and the key; the state after. 400 for an address that can't
/// be one, 409 while an environment variable sets them (the card shows them locked).
async fn save(State(st): State<AppState>, Admin(u): Admin, Json(b): Json<Save>) -> ApiResult<Json<Value>> {
    if st.cfg.env_locked("lantern_url") || st.cfg.env_locked("lantern_api_key") {
        return Err(err(409, "Lantern's address and key are set by environment variables (LANTERN_URL, LANTERN_API_KEY)"));
    }
    lantern::save(&st, &b.url, b.api_key.as_deref()).map_err(|why| err(400, why))?;
    st.db.log("info", &format!("{} connected Lantern at {}", u.s("username"), lantern::base(&b.url)));
    Ok(Json(lantern::state(&st)))
}

/// DELETE /api/admin/lantern: forget the connection; requests go through Encore's own sources again and
/// their settings, which the move never deleted, show again. The state after.
async fn disconnect(State(st): State<AppState>, Admin(u): Admin) -> ApiResult<Json<Value>> {
    if st.cfg.env_locked("lantern_url") || st.cfg.env_locked("lantern_api_key") {
        return Err(err(409, "Lantern's address and key are set by environment variables (LANTERN_URL, LANTERN_API_KEY)"));
    }
    lantern::disconnect(&st);
    st.db.log("info", &format!("{} disconnected Lantern", u.s("username")));
    Ok(Json(lantern::state(&st)))
}

/// `POST /api/admin/lantern/migrate` body.
#[derive(Deserialize)]
struct Migrate {
    /// true: only say what the move would carry (the default, so a bare POST changes nothing)
    #[serde(default = "yes")]
    dry_run: bool,
}

/// The default of [`Migrate::dry_run`].
fn yes() -> bool {
    true
}

/// POST /api/admin/lantern/migrate: the dry run (`{dry_run: true}`, the default) answers `{dry_run, plan}`
/// with what the move would carry, secrets masked; the move itself (`{dry_run: false}`) needs a connection
/// (409 without one) and Lantern 1.0's API (501 until then, nothing changed).
async fn migrate_settings(State(st): State<AppState>, Admin(u): Admin, Json(b): Json<Migrate>) -> ApiResult<Json<Value>> {
    let plan = migrate::plan(&st.cfg);
    if b.dry_run {
        return Ok(Json(json!({ "dry_run": true, "plan": plan })));
    }
    if !lantern::connected(&st) {
        return Err(err(409, "Connect Lantern first (its address and API key)"));
    }
    migrate::apply().map_err(|why| err(501, why))?;
    st.db.log("info", &format!("{} moved the download settings into Lantern", u.s("username")));
    Ok(Json(json!({ "dry_run": false, "plan": plan, "state": lantern::state(&st) })))
}
