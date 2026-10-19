//! /api/youtube: Settings → YouTube (the sign-in yt-dlp uses) and the browser extension's cookie upload.
//! The sign-in itself lives in encore_requests::ytauth.
use crate::auth::Admin;
use encore_core::error::{err, ApiResult};
use crate::state::AppState;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Json, Router};
use encore_db::RowExt;
use encore_requests::ytauth::*;
use serde::Deserialize;
use serde_json::{json, Value};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/youtube/auth", get(get_status))
        .route("/api/youtube/refresh", post(refresh_now))
        .route("/api/youtube/signout", post(sign_out))
        .route("/api/youtube/extension-token", post(new_token))
        .route("/api/youtube/extension-cookies", post(extension_cookies).options(preflight))
}

async fn get_status(State(st): State<AppState>, Admin(_): Admin) -> ApiResult<Json<Value>> {
    Ok(Json(tokio::task::spawn_blocking(move || status(&st)).await.map_err(|e| err(500, e.to_string()))?))
}

/// "Refresh now": the source in use (the hand-off session and pasted cookies are rotated; the
/// extension's are only checked).
async fn refresh_now(State(st): State<AppState>, Admin(u): Admin) -> ApiResult<Json<Value>> {
    let act = active(&st);
    let res = match act {
        Some(Kind::Handoff) => refresh_handoff(&st).await,
        Some(Kind::Extension) => check_extension(&st).await.map(|ok| Refresh { signed_in: ok, rotated: None }),
        // rotated only with the light refresh on (and never cookies from the environment)
        Some(Kind::Pasted) => refresh_pasted(&st, st.cfg.bool("youtube_light_refresh")).await,
        None => return Err(err(400, "No YouTube sign-in to refresh – hand one over from the Encore extension or paste cookies")),
    };
    let r = res.map_err(|e| err(400, e.to_string()))?;
    let what = match (r.signed_in, r.rotated) {
        (false, _) => "signed out",
        (true, Some(0)) => "signed in, nothing renewed",
        (true, Some(_)) => "signed in, cookies renewed",
        (true, None) => "signed in",
    };
    st.db.log("info", &format!("{} refreshed the YouTube sign-in ({what})", u.s("username")));
    let st2 = st.clone();
    let mut v = tokio::task::spawn_blocking(move || status(&st2)).await.map_err(|e| err(500, e.to_string()))?;
    v["refreshed"] = json!(r.signed_in);
    // what the refresh renewed (0: Google sent no cookies back, the sign-in was only checked; null: nothing was rotated on purpose)
    v["rotated"] = json!(r.rotated);
    Ok(Json(v))
}

#[derive(Deserialize, Default)]
struct SignOutBody {
    /// "handoff" | "extension" | "all" (default: all but the pasted cookies, which have their own clear)
    source: Option<String>,
}

async fn sign_out(State(st): State<AppState>, Admin(u): Admin, body: Option<Json<SignOutBody>>) -> ApiResult<Json<Value>> {
    let which = body.and_then(|b| b.0.source).unwrap_or_else(|| "all".into());
    if !["handoff", "extension", "all"].contains(&which.as_str()) {
        return Err(err(400, "Unknown sign-in"));
    }
    encore_requests::ytauth::sign_out(&st, &which).await;
    st.db.log("info", &format!("{} signed Encore out of YouTube ({which})", u.s("username")));
    let st2 = st.clone();
    Ok(Json(tokio::task::spawn_blocking(move || status(&st2)).await.map_err(|e| err(500, e.to_string()))?))
}

/// A new extension token (shown once; the old one stops working). It can only upload cookies.
async fn new_token(State(st): State<AppState>, Admin(u): Admin) -> ApiResult<Json<Value>> {
    let token = encore_requests::ytauth::new_token(&st);
    st.db.log("info", &format!("{} made a new YouTube extension token", u.s("username")));
    Ok(Json(json!({ "token": token })))
}

async fn preflight() -> Response {
    let mut r = Response::new(axum::body::Body::empty());
    cors(r.headers_mut());
    r
}

fn cors(h: &mut HeaderMap) {
    use axum::http::HeaderValue;
    h.insert("access-control-allow-origin", HeaderValue::from_static("*"));
    h.insert("access-control-allow-methods", HeaderValue::from_static("POST, OPTIONS"));
    h.insert("access-control-allow-headers", HeaderValue::from_static("authorization, content-type, x-encore-token"));
}

/// The browser extension's upload: `{cookies: [chrome.cookies.Cookie…], agent?, handoff?}` with the token.
/// `handoff: true`: the cookies of a private window signed in for Encore only – they become the
/// server-owned session. Otherwise: the everyday browser's cookies (never rotated by Encore).
async fn extension_cookies(State(st): State<AppState>, headers: HeaderMap, Json(body): Json<Value>) -> Response {
    use axum::response::IntoResponse;
    let s = load(st.data_dir());
    let mut resp = if !token_ok(&headers, s.ext_token.as_deref()) {
        err(401, "Unknown token – make a new one under Settings → YouTube → Connect the browser extension").into_response()
    } else {
        let r = if body["handoff"].as_bool() == Some(true) { accept_handoff(&st, &body).await } else { accept_extension(&st, &body).await };
        match r {
            Ok(v) => Json(v).into_response(),
            Err(e) => err(400, e.to_string()).into_response(),
        }
    };
    cors(resp.headers_mut());
    resp
}
