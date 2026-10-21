//! The Surround library's API (0.35): its albums, and getting a stereo album's or song's Dolby Atmos version.
//! Surround songs and albums play and show through the library's own routes (`/api/library/tracks/{id}/…` and
//! `/api/library/albums/{id}`), which tell them apart by id (encore_library::surround). docs/contracts/surround.md.
//!
//! - `GET /api/surround/albums?sort=added|name|artist|year&q=&limit=&offset=` → `{albums, stats}`
//! - `GET /api/library/albums/{id}/surround` → `{have, request, available, can_fetch}` (everyone)
//! - `POST /api/library/albums/{id}/surround` → `{request_id}` (admins): 404 when Apple Music has no Atmos version,
//!   409 when one is already being fetched or gamdl can't fetch Atmos
//! - `GET|POST /api/library/tracks/{id}/surround`: the same for a song
use crate::auth::{Admin, User};
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use encore_core::error::{err, ApiResult};
use encore_library::surround;
use serde::Deserialize;
use serde_json::{json, Value};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/surround/albums", get(albums))
        .route("/api/library/albums/{id}/surround", get(album_state).post(album_fetch))
        .route("/api/library/tracks/{id}/surround", get(track_state).post(track_fetch))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ListQ {
    sort: String,
    q: String,
    limit: i64,
    offset: i64,
}

async fn albums(State(st): State<AppState>, User(_): User, Query(q): Query<ListQ>) -> ApiResult<Json<Value>> {
    st.blocking(move |st| {
        let limit = if q.limit <= 0 { 100 } else { q.limit };
        let albums = surround::albums(&st.db, &q.sort, &q.q, limit, q.offset);
        Ok(Json(json!({ "albums": albums, "stats": surround::stats(&st.db) })))
    })
    .await
}

async fn album_state(State(st): State<AppState>, User(_): User, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    if surround::is_album(&id) {
        return Err(err(400, "This is a Surround album already"));
    }
    Ok(Json(encore_requests::surround::album_state(&st, &id)))
}

async fn album_fetch(State(st): State<AppState>, Admin(u): Admin, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    if surround::is_album(&id) {
        return Err(err(400, "This is a Surround album already"));
    }
    let rid = encore_requests::surround::request_album(&st, encore_db::RowExt::i(&u, "id"), &id).await?;
    Ok(Json(json!({ "request_id": rid })))
}

async fn track_state(State(st): State<AppState>, User(_): User, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    if surround::is_track(id) {
        return Err(err(400, "This is a Surround song already"));
    }
    Ok(Json(encore_requests::surround::track_state(&st, id)))
}

async fn track_fetch(State(st): State<AppState>, Admin(u): Admin, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    if surround::is_track(id) {
        return Err(err(400, "This is a Surround song already"));
    }
    let rid = encore_requests::surround::request_track(&st, encore_db::RowExt::i(&u, "id"), id).await?;
    Ok(Json(json!({ "request_id": rid })))
}
