//! /api/plugins: the Plugins page (Settings → Server → Plugins). The catalog and what is installed, with the
//! sandbox level; per plugin: install, update, check, enable, disable, remove, test, its status, its log, its
//! settings card (through the settings schema); upload a plugin zip; install from a URL or `owner/repo@ref`.
//! Admins only, and every change carries the CSRF header (the `Admin` extractor checks both).
//!
//! The old Download tools routes (`/api/system/tools*`, routes/system.rs) stay for one release, answering in
//! their old shape.
use crate::auth::Admin;
use crate::state::AppState;
use axum::extract::{DefaultBodyLimit, Multipart, Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use encore_core::error::{err, ApiError, ApiResult};
use encore_db::RowExt;
use encore_plugins::manifest::UPLOAD_MAX_BYTES;
use encore_plugins::Refused;
use serde::Deserialize;
use serde_json::{json, Map, Value};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/plugins", get(list))
        .route("/api/plugins/check", post(check_all))
        .route("/api/plugins/upload", post(upload).layer(DefaultBodyLimit::max(UPLOAD_MAX_BYTES as usize + (1 << 20))))
        .route("/api/plugins/install-url", post(install_url))
        .route("/api/plugins/{id}", get(one))
        .route("/api/plugins/{id}/install", post(install))
        .route("/api/plugins/{id}/update", post(update))
        .route("/api/plugins/{id}/check", post(check))
        .route("/api/plugins/{id}/enable", post(enable))
        .route("/api/plugins/{id}/disable", post(disable))
        .route("/api/plugins/{id}/remove", post(remove))
        .route("/api/plugins/{id}/test", post(test))
        .route("/api/plugins/{id}/auto-update", post(auto_update))
        .route("/api/plugins/{id}/logs", get(logs))
        .route("/api/plugins/{id}/settings", get(settings).put(save_settings).post(save_settings))
}

/// A refusal as the API answers it: `{detail, problems}` with its status.
fn refused(r: Refused) -> ApiError {
    let e = err(r.status, r.message);
    if r.problems.is_empty() { e } else { e.with(json!({ "problems": r.problems })) }
}

/// Long work – install, update, remove, test, the update checks, an upload or a URL install – runs in a task of
/// its own: a client that stops waiting (a proxy's read limit, a closed tab, a phone going to sleep) does not stop
/// it half way. The answer still comes when the work is done; meanwhile the card's `busy` says what runs, and
/// the pages ask again while it is set.
async fn detached<T: Send + 'static>(work: impl std::future::Future<Output = T> + Send + 'static) -> ApiResult<T> {
    tokio::spawn(work).await.map_err(|e| err(500, format!("The plugin task stopped: {e}")))
}

fn known(st: &AppState, id: &str) -> ApiResult<()> {
    if st.plugins.manifest(id).is_none() {
        return Err(err(404, "Unknown plugin"));
    }
    Ok(())
}

/// The catalog and the installed plugins, with the sandbox level.
async fn list(State(st): State<AppState>, Admin(_): Admin) -> ApiResult<Json<Value>> {
    let p = st.plugins.clone();
    Ok(Json(tokio::task::spawn_blocking(move || p.list()).await.map_err(|e| err(500, e.to_string()))?))
}

/// One plugin's card, with its status asked now (an installed plugin).
async fn one(State(st): State<AppState>, Admin(_): Admin, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    known(&st, &id)?;
    if st.plugins.installed(&id) {
        st.plugins.status(&id).await.map_err(refused)?;
    }
    st.plugins.card(&id).map(Json).ok_or_else(|| err(404, "Unknown plugin"))
}

async fn check_all(State(st): State<AppState>, Admin(_): Admin) -> ApiResult<Json<Value>> {
    let p = st.plugins.clone();
    Ok(Json(detached(async move { p.check_all().await }).await?))
}

async fn install(State(st): State<AppState>, Admin(u): Admin, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    known(&st, &id)?;
    st.db.log("info", &format!("{} started the install of plugin {id}", u.s("username")));
    let p = st.plugins.clone();
    detached(async move { p.install(&id).await }).await?.map(Json).map_err(refused)
}

async fn update(State(st): State<AppState>, Admin(u): Admin, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    known(&st, &id)?;
    st.db.log("info", &format!("{} started an update of plugin {id}", u.s("username")));
    let p = st.plugins.clone();
    detached(async move { p.update(&id).await }).await?.map(Json).map_err(refused)
}

async fn check(State(st): State<AppState>, Admin(_): Admin, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    known(&st, &id)?;
    let p = st.plugins.clone();
    detached(async move { p.check(&id).await }).await?.map(Json).map_err(refused)
}

async fn enable(State(st): State<AppState>, Admin(u): Admin, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    known(&st, &id)?;
    st.db.log("info", &format!("{} switched plugin {id} on", u.s("username")));
    st.plugins.enable(&id, true).await.map(Json).map_err(refused)
}

async fn disable(State(st): State<AppState>, Admin(u): Admin, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    known(&st, &id)?;
    st.db.log("info", &format!("{} switched plugin {id} off", u.s("username")));
    st.plugins.enable(&id, false).await.map(Json).map_err(refused)
}

async fn remove(State(st): State<AppState>, Admin(u): Admin, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    known(&st, &id)?;
    st.db.log("info", &format!("{} removed plugin {id}", u.s("username")));
    let p = st.plugins.clone();
    detached(async move { p.remove(&id).await }).await?.map(Json).map_err(refused)
}

async fn test(State(st): State<AppState>, Admin(_): Admin, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    known(&st, &id)?;
    let p = st.plugins.clone();
    detached(async move { p.test(&id).await }).await?.map(Json).map_err(refused)
}

#[derive(Deserialize)]
struct On {
    on: bool,
}

async fn auto_update(State(st): State<AppState>, Admin(_): Admin, Path(id): Path<String>, Json(b): Json<On>) -> ApiResult<Json<Value>> {
    known(&st, &id)?;
    st.plugins.set_auto_update(&id, b.on).map(Json).map_err(refused)
}

#[derive(Deserialize)]
struct Lines {
    lines: Option<usize>,
}

async fn logs(State(st): State<AppState>, Admin(_): Admin, Path(id): Path<String>, Query(q): Query<Lines>) -> ApiResult<Json<Value>> {
    st.plugins.logs(&id, q.lines.unwrap_or(200)).map(Json).map_err(refused)
}

async fn settings(State(st): State<AppState>, Admin(_): Admin, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    st.plugins.settings(&id).map(Json).map_err(refused)
}

/// Save the card: `{key: value, …}` (or `{values: {…}}`); a masked secret is left as it is.
async fn save_settings(State(st): State<AppState>, Admin(u): Admin, Path(id): Path<String>, Json(body): Json<Map<String, Value>>) -> ApiResult<Json<Value>> {
    known(&st, &id)?;
    let values = match body.get("values") {
        Some(Value::Object(v)) if body.len() == 1 => v.clone(),
        _ => body,
    };
    let mut keys: Vec<&String> = values.keys().collect();
    keys.sort();
    st.db.log("info", &format!("{} updated the settings of plugin {id}: {}", u.s("username"), keys.iter().map(|k| k.as_str()).collect::<Vec<_>>().join(", ")));
    let out = st.plugins.save_settings(&id, &values).map_err(refused)?;
    if id == "slskd" {
        st.requests.slskd.poke(); // a changed Soulseek login or port takes effect now
    }
    Ok(Json(out))
}

/// Upload a plugin zip (multipart, the first file part; at most 200 MB): unpacked with its names checked, its
/// manifest checked, `describe` run in the sandbox, then installed and switched on.
async fn upload(State(st): State<AppState>, Admin(u): Admin, mut form: Multipart) -> ApiResult<Json<Value>> {
    use tokio::io::AsyncWriteExt;
    let dir = st.plugins.scratch().map_err(|e| err(500, e.to_string()))?;
    let file = dir.join("upload.zip");
    let mut size: u64 = 0;
    let streamed: Result<(), ApiError> = async {
        let mut out = tokio::fs::File::create(&file).await.map_err(|e| err(500, e.to_string()))?;
        while let Some(mut f) = form.next_field().await.map_err(|e| err(400, e.to_string()))? {
            if size > 0 {
                break;
            }
            while let Some(chunk) = f.chunk().await.map_err(|e| err(400, e.to_string()))? {
                size += chunk.len() as u64;
                if size > UPLOAD_MAX_BYTES {
                    return Err(err(413, format!("The plugin zip is larger than {} MB", UPLOAD_MAX_BYTES >> 20)));
                }
                out.write_all(&chunk).await.map_err(|e| err(500, e.to_string()))?;
            }
        }
        out.flush().await.map_err(|e| err(500, e.to_string()))?;
        out.sync_all().await.map_err(|e| err(500, e.to_string()))?;
        Ok(())
    }
    .await;
    if let Err(e) = streamed {
        let _ = std::fs::remove_dir_all(&dir);
        return Err(e);
    }
    if size == 0 {
        let _ = std::fs::remove_dir_all(&dir);
        return Err(err(400, "No file came with the upload"));
    }
    st.db.log("info", &format!("{} uploaded a plugin ({} KB)", u.s("username"), size / 1024));
    let p = st.plugins.clone();
    detached(async move {
        let res = p.upload(&file).await;
        let _ = std::fs::remove_dir_all(&dir);
        res
    })
    .await?
    .map(Json)
    .map_err(refused)
}

#[derive(Deserialize)]
struct Url {
    url: String,
}

/// Install from an https address (a zip, or a raw plugin.toml whose [update] names the zip) or `owner/repo@ref`.
async fn install_url(State(st): State<AppState>, Admin(u): Admin, Json(b): Json<Url>) -> ApiResult<Json<Value>> {
    st.db.log("info", &format!("{} started the install of a plugin from {}", u.s("username"), encore_plugins::fetch::shown(&b.url)));
    let p = st.plugins.clone();
    detached(async move { p.install_url(&b.url).await }).await?.map(Json).map_err(refused)
}
