//! /api/playlists, /api/connections, /api/sync
use crate::auth::{Admin, User};
use crate::db::{now, Row, RowExt};
use crate::error::{err, ApiResult};
use crate::matcher::LibraryIndex;
use crate::sources::links;
use crate::state::AppState;
use crate::sync::{self, Adapter};
use crate::{connections, jobs, p, playlists};
use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/playlists", get(list).post(create))
        .route("/api/playlists/preview", post(preview))
        .route("/api/playlists/import", post(import))
        .route("/api/playlists/merge", post(merge))
        .route("/api/playlists/{pid}", get(detail).patch(patch).delete(remove))
        .route("/api/playlists/{pid}/dedupe", post(dedupe))
        .route("/api/playlists/{pid}/rematch", post(rematch))
        .route("/api/playlists/{pid}/request-missing", post(request_missing))
        .route("/api/playlists/{pid}/refresh", post(refresh))
        .route("/api/playlists/{pid}/tracks", post(add_tracks))
        .route("/api/playlists/{pid}/items/{iid}", delete(remove_item))
        .route("/api/playlists/{pid}/move", post(move_item))
        .route("/api/playlists/{pid}/sync", post(sync_me))
        .route("/api/connections", get(list_connections))
        .route("/api/connections/plex/check", post(plex_check))
        .route("/api/connections/ytmusic/check", post(ytmusic_check))
        .route("/api/connections/{service}", post(connect).patch(patch_connection).delete(disconnect))
        .route("/api/connections/{service}/callback", get(callback))
        .route("/api/connections/{service}/playlists", get(remote_playlists))
        .route("/api/connections/{service}/playlists/{rid}/import", post(import_remote))
        .route("/api/sync", post(sync_me_all))
        .route("/api/sync/all", post(sync_everyone))
}

fn own(st: &AppState, pid: i64, u: &Row) -> ApiResult<Row> {
    st.db.one("SELECT * FROM playlists WHERE id=?", p![pid])?.filter(|p| p.i("user_id") == u.i("id")).ok_or_else(|| err(404, "Playlist not found"))
}

fn bad(e: anyhow::Error) -> crate::error::ApiError {
    err(400, e.to_string())
}

async fn list(State(st): State<AppState>, User(u): User) -> ApiResult<Json<Value>> {
    playlists::get_liked(&st, u.i("id")).map_err(bad)?;
    let rows = st.db.query("SELECT * FROM playlists WHERE user_id=? ORDER BY kind='liked' DESC, updated_at DESC", p![u.i("id")])?;
    Ok(Json(json!(rows.iter().map(|p| playlists::summary(&st, p)).collect::<Vec<_>>())))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ImportBody {
    url: Option<String>,
    text: Option<String>,
    name: Option<String>,
    follow: bool,
    into: Option<i64>,
    request_missing: bool,
    album_mode: Option<String>,
}

async fn fetch_source(st: &AppState, b: &ImportBody) -> ApiResult<Value> {
    match b.text.as_deref().filter(|t| !t.trim().is_empty()) {
        Some(t) => links::parse_text(t).map_err(bad),
        None => links::fetch(st, b.url.as_deref().unwrap_or("")).await.map_err(bad),
    }
}

async fn preview(State(st): State<AppState>, User(_): User, Json(b): Json<ImportBody>) -> ApiResult<Json<Value>> {
    let data = fetch_source(&st, &b).await?;
    let mut rows: Vec<Row> = data["tracks"].as_array().cloned().unwrap_or_default().into_iter().filter_map(|v| v.as_object().cloned()).collect();
    let matched = playlists::match_rows(&mut rows, &LibraryIndex::load(&st.db));
    let (_, dupes) = playlists::dedupe_rows(rows.clone());
    rows.truncate(300);
    Ok(Json(json!({ "name": data["name"], "source": data["source"], "cover": data["cover"], "description": data["description"],
        "count": data["tracks"].as_array().map(|a| a.len()).unwrap_or(0), "in_library": matched, "duplicates": dupes, "tracks": rows })))
}

async fn import_inner(st: &AppState, u: &Row, b: ImportBody) -> ApiResult<Value> {
    let data = fetch_source(st, &b).await?;
    let tracks = data["tracks"].as_array().cloned().unwrap_or_default();
    let source = data["source"].as_str().unwrap_or("web").to_string();
    let mut res = match b.into {
        Some(pid) => {
            let pl = own(st, pid, u)?;
            let (added, dupes) = playlists::add_tracks(st, pl.i("id"), &tracks, &source).map_err(bad)?;
            json!({ "id": pid, "added": added, "duplicates_skipped": dupes })
        }
        None => {
            let name = b.name.clone().filter(|n| !n.trim().is_empty()).or(data["name"].as_str().map(String::from)).unwrap_or("Imported playlist".into());
            playlists::create(st, u.i("id"), &name, &tracks, &source, b.url.as_deref(), data["description"].as_str().unwrap_or(""), data["cover"].as_str(),
                b.follow && b.url.is_some()).map_err(bad)?
        }
    };
    if b.request_missing {
        res["requested"] = json!(playlists::request_missing(st, u, res["id"].as_i64().unwrap_or(0), false, b.album_mode).map_err(bad)?);
    }
    st.db.log("info", &format!("{} imported '{}' from {source} ({} tracks, {} duplicates skipped)", u.s("username"), data["name"].as_str().unwrap_or(""), tracks.len(), res["duplicates_skipped"]));
    Ok(res)
}

async fn import(State(st): State<AppState>, User(u): User, Json(b): Json<ImportBody>) -> ApiResult<Json<Value>> {
    Ok(Json(import_inner(&st, &u, b).await?))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Patch {
    name: Option<String>,
    description: Option<String>,
    follow: Option<bool>,
    sync_targets: Option<Vec<String>>,
}

async fn create(State(st): State<AppState>, User(u): User, Json(b): Json<Patch>) -> ApiResult<Json<Value>> {
    let name = b.name.filter(|n| !n.trim().is_empty()).unwrap_or("New playlist".into());
    Ok(Json(playlists::create(&st, u.i("id"), &name, &[], "manual", None, b.description.as_deref().unwrap_or(""), None, false).map_err(bad)?))
}

async fn detail(State(st): State<AppState>, User(u): User, Path(pid): Path<i64>) -> ApiResult<Json<Value>> {
    let pl = own(&st, pid, &u)?;
    let mut v = playlists::summary(&st, &pl);
    v["items"] = json!(st.db.query("SELECT i.*, t.album_id, t.format, t.bitrate, t.verify_status, r.status AS request_status
        FROM playlist_items i LEFT JOIN tracks t ON t.id=i.track_id LEFT JOIN requests r ON r.id=i.request_id
        WHERE i.playlist_id=? ORDER BY i.position, i.id", p![pid])?);
    v["remote_ids"] = serde_json::from_str(&pl.s("remote_ids")).unwrap_or(json!({}));
    Ok(Json(v))
}

async fn patch(State(st): State<AppState>, User(u): User, Path(pid): Path<i64>, Json(b): Json<Patch>) -> ApiResult<Json<Value>> {
    let pl = own(&st, pid, &u)?;
    let mut sets: Vec<(&str, Value)> = vec![];
    if let (Some(n), false) = (b.name, pl.s("kind") == "liked") {
        sets.push(("name", json!(if n.trim().is_empty() { pl.s("name") } else { n.trim().to_string() })));
    }
    if let Some(d) = b.description {
        sets.push(("description", json!(d)));
    }
    if let Some(f) = b.follow {
        sets.push(("follow", json!((f && !pl.s("source_url").is_empty()) as i64)));
    }
    if let Some(t) = b.sync_targets {
        let src = pl.s("source");
        let ok: Vec<String> = t.into_iter().filter(|x| ["jellyfin", "plex", "navidrome", "listenbrainz", "ytmusic"].contains(&x.as_str()) && *x != src).collect();
        sets.push(("sync_targets", json!(json!(ok).to_string())));
    }
    if !sets.is_empty() {
        sets.push(("updated_at", json!(now())));
        let sql = format!("UPDATE playlists SET {} WHERE id=?", sets.iter().map(|(k, _)| format!("{k}=?")).collect::<Vec<_>>().join(", "));
        let mut params: Vec<Value> = sets.into_iter().map(|(_, v)| v).collect();
        params.push(json!(pid));
        st.db.execute(&sql, params)?;
    }
    Ok(Json(playlists::summary(&st, &own(&st, pid, &u)?)))
}

async fn remove(State(st): State<AppState>, User(u): User, Path(pid): Path<i64>) -> ApiResult<Json<Value>> {
    if own(&st, pid, &u)?.s("kind") == "liked" {
        return Err(err(400, "Liked Songs can't be deleted"));
    }
    st.db.execute("DELETE FROM playlist_items WHERE playlist_id=?", p![pid])?;
    st.db.execute("DELETE FROM playlists WHERE id=?", p![pid])?;
    Ok(Json(json!({ "ok": true })))
}

async fn dedupe(State(st): State<AppState>, User(u): User, Path(pid): Path<i64>) -> ApiResult<Json<Value>> {
    own(&st, pid, &u)?;
    playlists::rematch(&st, Some(pid), false);
    Ok(Json(json!({ "removed": playlists::dedupe(&st, pid) })))
}

async fn rematch(State(st): State<AppState>, User(u): User, Path(pid): Path<i64>) -> ApiResult<Json<Value>> {
    own(&st, pid, &u)?;
    Ok(Json(json!({ "matched": playlists::rematch(&st, Some(pid), false) })))
}

async fn request_missing(State(st): State<AppState>, User(u): User, Path(pid): Path<i64>, body: Option<Json<Value>>) -> ApiResult<Json<Value>> {
    own(&st, pid, &u)?;
    crate::routes::requests::quota(&st, &u, 1)?;
    let mode = body.and_then(|b| b["album_mode"].as_str().map(String::from));
    Ok(Json(json!({ "requested": playlists::request_missing(&st, &u, pid, false, mode).map_err(bad)? })))
}

async fn refresh(State(st): State<AppState>, User(u): User, Path(pid): Path<i64>) -> ApiResult<Json<Value>> {
    let pl = own(&st, pid, &u)?;
    if pl.s("source_url").is_empty() {
        return Err(err(400, "This playlist has no source to refresh from"));
    }
    let (added, dupes) = playlists::refresh_followed(&st, &pl).await.map_err(|e| err(502, e.to_string()))?;
    Ok(Json(json!({ "added": added, "duplicates_skipped": dupes })))
}

#[derive(Deserialize)]
struct AddTracks {
    track_ids: Vec<i64>,
}

async fn add_tracks(State(st): State<AppState>, User(u): User, Path(pid): Path<i64>, Json(b): Json<AddTracks>) -> ApiResult<Json<Value>> {
    own(&st, pid, &u)?;
    let (added, dupes) = playlists::add_library_tracks(&st, pid, &b.track_ids).map_err(bad)?;
    Ok(Json(json!({ "added": added, "duplicates_skipped": dupes })))
}

async fn remove_item(State(st): State<AppState>, User(u): User, Path((pid, iid)): Path<(i64, i64)>) -> ApiResult<Json<Value>> {
    own(&st, pid, &u)?;
    st.db.execute("DELETE FROM playlist_items WHERE id=? AND playlist_id=?", p![iid, pid])?;
    playlists::renumber(&st, pid);
    Ok(Json(json!({ "ok": true })))
}

async fn move_item(State(st): State<AppState>, User(u): User, Path(pid): Path<i64>, Json(b): Json<Value>) -> ApiResult<Json<Value>> {
    own(&st, pid, &u)?;
    let mut ids: Vec<i64> = playlists::items(&st, pid).iter().map(|r| r.i("id")).collect();
    let iid = b["item_id"].as_i64().or(b["item_id"].as_str().and_then(|x| x.parse().ok())).unwrap_or(0);
    let to = b["to"].as_i64().or(b["to"].as_str().and_then(|x| x.parse().ok())).unwrap_or(0).max(0) as usize;
    let Some(at) = ids.iter().position(|i| *i == iid) else { return Err(err(404, "Item not found")) };
    ids.remove(at);
    ids.insert(to.min(ids.len()), iid);
    for (pos, i) in ids.iter().enumerate() {
        st.db.execute("UPDATE playlist_items SET position=? WHERE id=?", p![pos, i])?;
    }
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct MergeBody {
    ids: Vec<i64>,
    name: String,
    #[serde(default)]
    delete_sources: bool,
}

async fn merge(State(st): State<AppState>, User(u): User, Json(b): Json<MergeBody>) -> ApiResult<Json<Value>> {
    if b.ids.len() < 2 {
        return Err(err(400, "Pick at least two playlists"));
    }
    let name = if b.name.trim().is_empty() { "Merged playlist".to_string() } else { b.name.trim().to_string() };
    let res = playlists::merge(&st, u.i("id"), &b.ids, &name).map_err(bad)?;
    if b.delete_sources {
        for pid in &b.ids {
            if st.db.one("SELECT kind FROM playlists WHERE id=? AND user_id=?", p![pid, u.i("id")])?.is_some_and(|p| p.s("kind") != "liked") {
                st.db.execute("DELETE FROM playlist_items WHERE playlist_id=?", p![pid])?;
                st.db.execute("DELETE FROM playlists WHERE id=?", p![pid])?;
            }
        }
    }
    Ok(Json(res))
}

async fn sync_me(State(st): State<AppState>, User(u): User, Path(pid): Path<i64>) -> ApiResult<Json<Value>> {
    own(&st, pid, &u)?;
    let mut log = vec![];
    let mut s = sync::sync_user(&st, u.i("id"), &mut log).await;
    s["log"] = json!(log);
    Ok(Json(s))
}

async fn sync_me_all(State(st): State<AppState>, User(u): User) -> ApiResult<Json<Value>> {
    let mut log = vec![];
    let mut s = sync::sync_user(&st, u.i("id"), &mut log).await;
    s["log"] = json!(log);
    Ok(Json(s))
}

async fn sync_everyone(State(st): State<AppState>, Admin(_): Admin) -> ApiResult<Json<Value>> {
    tokio::spawn(sync::sync_all(st));
    Ok(Json(json!({ "ok": true })))
}

// ---- connections --------------------------------------------------------------------------------

async fn list_connections(State(st): State<AppState>, User(u): User) -> ApiResult<Json<Value>> {
    let have: serde_json::Map<String, Value> = connections::list_for(&st, u.i("id")).iter().map(|c| (c.s("service"), connections::public(c))).collect();
    let has = |k: &str| !st.cfg.str(k).is_empty();
    let avail = json!({ "jellyfin": has("jellyfin_url"), "plex": has("plex_url"), "navidrome": has("navidrome_url"), "spotify": has("spotify_client_id"),
        "tidal": has("tidal_client_id"), "deezer": true, "listenbrainz": true, "ytmusic": true, "lastfm": has("lastfm_api_key") });
    let services: serde_json::Map<String, Value> = connections::SERVICES.iter().map(|(k, l, kind)| (k.to_string(), json!({ "label": l, "kind": kind }))).collect();
    let sync_state = jobs::get("sync");
    Ok(Json(json!({ "connections": have, "available": avail, "services": services, "sync": { "running": sync_state["running"], "last": sync_state["last"] },
        "base_url": st.cfg.str("base_url") })))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ConnectBody {
    username: Option<String>,
    password: Option<String>,
    token: Option<String>,
    profile: Option<String>,
    headers: Option<String>,
}

async fn connect(State(st): State<AppState>, User(u): User, Path(service): Path<String>, Json(b): Json<ConnectBody>) -> ApiResult<Json<Value>> {
    let uid = u.i("id");
    let s = |o: &Option<String>| o.clone().unwrap_or_default().trim().to_string();
    let res = match service.as_str() {
        "jellyfin" => connections::connect_jellyfin(&st, uid, &s(&b.username), &b.password.clone().unwrap_or_default()).await,
        "navidrome" => connections::connect_navidrome(&st, uid, &s(&b.username), &b.password.clone().unwrap_or_default()).await,
        "listenbrainz" => connections::connect_listenbrainz(&st, uid, &s(&b.token)).await,
        "lastfm" => match s(&b.username) {
            n if !n.is_empty() => connections::connect_lastfm(&st, uid, &n).await,
            _ => connections::lastfm_start(&st).map(|url| json!({ "url": url })),
        },
        "deezer" => connections::connect_deezer(&st, uid, &s(&b.profile)).await,
        "plex" => connections::plex_pin_start(&st).await,
        "ytmusic" => match b.headers.as_deref().filter(|h| !h.trim().is_empty()) {
            Some(h) => connections::ytmusic_connect_browser(&st, uid, h).await,
            None => connections::ytmusic_oauth_start(&st).await,
        },
        "spotify" | "tidal" => connections::oauth_start(&st, uid, &service).map(|url| json!({ "url": url })),
        _ => return Err(err(404, "Unknown service")),
    };
    res.map(Json).map_err(bad)
}

async fn plex_check(State(st): State<AppState>, User(u): User, Json(b): Json<Value>) -> ApiResult<Json<Value>> {
    let pin = b["pin_id"].as_i64().or(b["pin_id"].as_str().and_then(|x| x.parse().ok())).unwrap_or(0);
    connections::plex_pin_check(&st, u.i("id"), pin).await.map(Json).map_err(bad)
}

async fn ytmusic_check(State(st): State<AppState>, User(u): User, Json(b): Json<Value>) -> ApiResult<Json<Value>> {
    connections::ytmusic_oauth_check(&st, u.i("id"), b["device_code"].as_str().unwrap_or("")).await.map(Json).map_err(bad)
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct CallbackQ {
    code: String,
    token: String,
    state: String,
    error: String,
}

async fn callback(State(st): State<AppState>, User(u): User, Path(service): Path<String>, Query(q): Query<CallbackQ>) -> Response {
    if !q.error.is_empty() {
        return Redirect::to(&format!("/#/connections?error={}", urlencoding::encode(&q.error))).into_response();
    }
    let done = if service == "lastfm" {
        connections::lastfm_finish(&st, u.i("id"), &q.token).await.map(|_| ())
    } else {
        connections::oauth_finish(&st, &service, &q.code, &q.state, u.i("id")).await.map(|_| ())
    };
    match done {
        Ok(_) => Redirect::to(&format!("/#/connections?connected={service}")).into_response(),
        Err(e) => Redirect::to(&format!("/#/connections?error={}", urlencoding::encode(&e.to_string().chars().take(200).collect::<String>()))).into_response(),
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ConnPatch {
    scrobble: Option<bool>,
    sync_likes: Option<bool>,
    sync_playlists: Option<bool>,
}

async fn patch_connection(State(st): State<AppState>, User(u): User, Path(service): Path<String>, Json(b): Json<ConnPatch>) -> ApiResult<Json<Value>> {
    let c = connections::get(&st, u.i("id"), &service).ok_or_else(|| err(404, "Not connected"))?;
    if let Some(v) = b.sync_likes {
        st.db.execute("UPDATE connections SET sync_likes=? WHERE id=?", p![v as i64, c.i("id")])?;
    }
    if let Some(v) = b.scrobble {
        let mut d: Value = serde_json::from_str(&c.s("data")).unwrap_or(json!({}));
        d["scrobble"] = json!(v);
        connections::update_data(&st, c.i("id"), &d);
    }
    if let Some(v) = b.sync_playlists {
        st.db.execute("UPDATE connections SET sync_playlists=? WHERE id=?", p![v as i64, c.i("id")])?;
    }
    Ok(Json(json!({ "ok": true })))
}

async fn disconnect(State(st): State<AppState>, User(u): User, Path(service): Path<String>) -> ApiResult<Json<Value>> {
    if let Some(c) = connections::get(&st, u.i("id"), &service) {
        st.db.execute("DELETE FROM sync_state WHERE connection_id=?", p![c.i("id")])?;
        st.db.execute("DELETE FROM connections WHERE id=?", p![c.i("id")])?;
    }
    Ok(Json(json!({ "ok": true })))
}

async fn remote_playlists(State(st): State<AppState>, User(u): User, Path(service): Path<String>) -> ApiResult<Json<Value>> {
    let c = connections::get(&st, u.i("id"), &service).ok_or_else(|| err(404, "Not connected"))?;
    Adapter::new(&st, c).playlists().await.map(|v| Json(json!(v))).map_err(|e| err(502, e.to_string().chars().take(300).collect::<String>()))
}

async fn import_remote(State(st): State<AppState>, User(u): User, Path((service, rid)): Path<(String, String)>, Json(b): Json<Value>) -> ApiResult<Json<Value>> {
    connections::get(&st, u.i("id"), &service).ok_or_else(|| err(404, "Not connected"))?;
    let name = b["name"].as_str().filter(|n| !n.is_empty()).unwrap_or("Imported playlist").to_string();
    let follow = b["follow"].as_bool().unwrap_or(false);
    if let Some(url) = b["url"].as_str().filter(|x| !x.is_empty()) {
        return Ok(Json(import_inner(&st, &u, ImportBody { url: Some(url.into()), name: Some(name), follow, ..Default::default() }).await?));
    }
    let res = playlists::create(&st, u.i("id"), &name, &[], &service, Some(&format!("{service}:{rid}")), "", None, follow).map_err(bad)?;
    let pl = own(&st, res["id"].as_i64().unwrap_or(0), &u)?;
    let (added, dupes) = playlists::refresh_followed(&st, &pl).await.map_err(|e| err(502, e.to_string()))?;
    Ok(Json(json!({ "id": pl.i("id"), "added": added, "duplicates_skipped": dupes })))
}
