//! /api/system/wrapper: the Apple Music wrapper's card (Settings → Plugins → Apple Music wrapper), the parts
//! beyond its fields, Test and log (those are /api/plugins'): what it stands at, the Apple Music app upload,
//! Set up (build and start), Start, Stop, the sign-in with its two-factor code, sign out, "Use with gamdl" and
//! the container's log. Admins only, and every change carries the CSRF header (the `Admin` extractor checks
//! both). The code is encore_requests::wrapper.
use crate::auth::Admin;
use crate::state::AppState;
use axum::extract::{DefaultBodyLimit, Multipart, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use encore_core::error::{err, ApiError, ApiResult};
use encore_db::RowExt;
use encore_requests::wrapper;
use serde::Deserialize;
use serde_json::{json, Value};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/system/wrapper", get(overview))
        .route("/api/system/wrapper/app", post(upload_app).layer(DefaultBodyLimit::max(wrapper::BUNDLE_MAX_BYTES as usize + (1 << 20))))
        .route("/api/system/wrapper/set-up", post(set_up))
        .route("/api/system/wrapper/start", post(start))
        .route("/api/system/wrapper/stop", post(stop))
        .route("/api/system/wrapper/sign-in", post(sign_in).delete(sign_out))
        .route("/api/system/wrapper/sign-in/code", post(sign_in_code))
        .route("/api/system/wrapper/gamdl", post(use_with_gamdl))
        .route("/api/system/wrapper/logs", get(logs))
}

/// A plugin failure as the API answers it (its words; a refused sign-in is the owner's to fix: 400).
fn failed(f: encore_plugins::Failure) -> ApiError {
    let status = match f.code {
        encore_plugins::Code::Auth | encore_plugins::Code::Unsupported => 400,
        encore_plugins::Code::NotFound => 404,
        encore_plugins::Code::Network => 502,
        _ => 500,
    };
    err(status, f.message)
}

/// The plugin has to be installed for its card's actions.
fn installed(st: &AppState) -> ApiResult<()> {
    if !st.plugins.installed(wrapper::ID) {
        return Err(err(409, "Install the Apple Music wrapper plugin first (Settings → Plugins)"));
    }
    Ok(())
}

/// Everything the card shows (never a token).
async fn overview(State(st): State<AppState>, Admin(_): Admin) -> ApiResult<Json<Value>> {
    Ok(Json(wrapper::overview(&st).await))
}

/// The owner's Apple Music for Android app (`.apk` or `.apkm`), as one multipart file, streamed to disk.
async fn upload_app(State(st): State<AppState>, Admin(u): Admin, mut form: Multipart) -> ApiResult<Json<Value>> {
    installed(&st)?;
    let dir = wrapper::data(&st);
    let tmp = dir.join(format!("upload-{}.part", encore_core::ids::random_hex(8)));
    let mut size: u64 = 0;
    let mut name = String::new();
    // every way out of the loop with an error removes the .part file
    let streamed: Result<(), ApiError> = async {
        use tokio::io::AsyncWriteExt;
        let mut out = tokio::fs::File::create(&tmp).await.map_err(|e| err(500, e.to_string()))?;
        while let Some(mut f) = form.next_field().await.map_err(|e| err(400, e.to_string()))? {
            if size > 0 {
                break;
            }
            name = f.file_name().unwrap_or("apple-music").to_string();
            while let Some(chunk) = f.chunk().await.map_err(|e| err(400, e.to_string()))? {
                size += chunk.len() as u64;
                if size > wrapper::BUNDLE_MAX_BYTES {
                    return Err(err(413, format!("The app is larger than {} GB", wrapper::BUNDLE_MAX_BYTES >> 30)));
                }
                out.write_all(&chunk).await.map_err(|e| err(500, e.to_string()))?;
            }
        }
        out.flush().await.map_err(|e| err(500, e.to_string()))?;
        Ok(())
    }
    .await;
    if let Err(e) = streamed {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(e);
    }
    if size == 0 {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(err(400, "No file came: choose the Apple Music .apk or .apkm"));
    }
    let st2 = st.clone();
    let info = tokio::task::spawn_blocking(move || wrapper::keep_bundle(&st2, &tmp, &name)).await.map_err(|e| err(500, e.to_string()))?.map_err(|e| err(400, e))?;
    tracing::info!("{} uploaded the Apple Music app for the wrapper", u.s("username"));
    Ok(Json(json!({ "ok": true, "bundle": info })))
}

#[derive(Deserialize, Default)]
struct SetUp {
    /// build again even when the image is there (after a new app upload)
    #[serde(default)]
    rebuild: bool,
}

/// Set up: download, stage, build (or pull) and start, in the background. The card asks /api/system/wrapper
/// for the job's progress.
async fn set_up(State(st): State<AppState>, Admin(_): Admin, body: Option<Json<SetUp>>) -> ApiResult<Json<Value>> {
    installed(&st)?;
    if wrapper::mode(&st) != wrapper::Mode::Managed {
        return Err(err(400, "Set up is for the wrapper Encore runs: choose \"Encore runs it\" first"));
    }
    let rebuild = body.map(|b| b.0.rebuild).unwrap_or(false);
    if !wrapper::set_up(&st, rebuild) {
        return Err(err(409, "The setup runs already"));
    }
    Ok(Json(wrapper::overview(&st).await))
}

async fn start(State(st): State<AppState>, Admin(_): Admin) -> ApiResult<Json<Value>> {
    installed(&st)?;
    let a = wrapper::start(&st).await.map_err(|e| err(400, e))?;
    Ok(Json(json!({ "ok": true, "url": a.url, "message": format!("The wrapper runs at {}", a.url) })))
}

async fn stop(State(st): State<AppState>, Admin(_): Admin) -> ApiResult<Json<Value>> {
    installed(&st)?;
    wrapper::stop(&st).await.map_err(|e| err(400, e))?;
    Ok(Json(json!({ "ok": true, "message": "The wrapper is stopped" })))
}

#[derive(Deserialize)]
struct SignIn {
    #[serde(default)]
    apple_id: String,
    #[serde(default)]
    password: String,
}

/// The first sign-in step. The password is passed on to the wrapper and kept nowhere (not in the settings,
/// not in a log).
async fn sign_in(State(st): State<AppState>, Admin(_): Admin, Json(b): Json<SignIn>) -> ApiResult<Json<Value>> {
    installed(&st)?;
    let v = wrapper::sign_in(&st, &b.apple_id, &b.password).await.map_err(failed)?;
    st.plugins.log(wrapper::ID).line("sign-in", &format!("sign-in: {}", v["state"].as_str().unwrap_or("?")));
    Ok(Json(v))
}

#[derive(Deserialize)]
struct Code {
    #[serde(default)]
    code: String,
}

/// The second step: the code Apple sent to the owner's devices.
async fn sign_in_code(State(st): State<AppState>, Admin(_): Admin, Json(b): Json<Code>) -> ApiResult<Json<Value>> {
    installed(&st)?;
    let v = wrapper::sign_in_code(&st, &b.code).await.map_err(failed)?;
    st.plugins.log(wrapper::ID).line("sign-in", &format!("code: {}", v["state"].as_str().unwrap_or("?")));
    Ok(Json(v))
}

async fn sign_out(State(st): State<AppState>, Admin(_): Admin) -> ApiResult<Json<Value>> {
    installed(&st)?;
    wrapper::sign_out(&st).await.map_err(failed)?;
    st.plugins.log(wrapper::ID).line("sign-in", "signed out");
    Ok(Json(json!({ "ok": true, "message": "Signed out of Apple Music" })))
}

/// "Use with gamdl": gamdl's Wrapper URL and decrypt address set to this wrapper's.
async fn use_with_gamdl(State(st): State<AppState>, Admin(_): Admin) -> ApiResult<Json<Value>> {
    installed(&st)?;
    let m = wrapper::fill_gamdl(&st).map_err(|e| err(400, e))?;
    Ok(Json(json!({ "ok": true, "message": m })))
}

#[derive(Deserialize)]
struct Lines {
    lines: Option<usize>,
}

/// The managed wrapper's container log (the plugin's own log, with the build's lines, is
/// /api/plugins/wrapper/logs).
async fn logs(State(st): State<AppState>, Admin(_): Admin, Query(q): Query<Lines>) -> ApiResult<Json<Value>> {
    installed(&st)?;
    let text = wrapper::container_logs(&st, q.lines.unwrap_or(300)).await.map_err(|e| err(400, e))?;
    Ok(Json(json!({ "text": text })))
}
