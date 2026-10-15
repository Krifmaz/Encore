//! Port of web/js/api.js: `fetch('/api' + path)` with the same headers, the same connection events
//! (encore:server-down / encore:server-up / encore:unauthorized) and the same error messages.

use crate::dom;
use crate::json::J;
use serde_json::Value;
use web_sys::RequestCredentials;

#[derive(Clone, Debug)]
pub struct ApiError {
    pub status: u16,
    pub message: String,
    pub data: Value,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(&self.message) }
}
impl std::error::Error for ApiError {}

pub type ApiResult = Result<Value, ApiError>;

thread_local! {
    /// `coverColors`: album id → its cover's main colour ("#rrggbb", the server's `cover_color`), painted while the cover loads.
    static COVER_COLORS: std::cell::RefCell<std::collections::HashMap<String, String>> = Default::default();
}

/// `coverColors.get(id)`.
pub fn cover_color(album_id: &str) -> Option<String> { COVER_COLORS.with(|m| m.borrow().get(album_id).cloned()) }

fn is_album_id(s: &str) -> bool { s.len() == 16 && s.bytes().all(|b| b.is_ascii_hexdigit()) }

fn is_hex_color(s: &str) -> bool { s.len() == 7 && s.starts_with('#') && s[1..].bytes().all(|b| b.is_ascii_hexdigit()) }

/// `noteCoverColors(v)`: records every `cover_color` in a JSON answer, under the album whose cover the object shows.
fn note_cover_colors(v: &Value) {
    match v {
        Value::Array(a) => a.iter().for_each(note_cover_colors),
        Value::Object(o) => {
            if let Some(c) = o.get("cover_color").and_then(|c| c.as_str()).filter(|c| is_hex_color(c)) {
                let nonempty = |k: &str| o.get(k).and_then(|x| x.as_str()).filter(|x| !x.is_empty()).map(str::to_string);
                let id = nonempty("album_id").or_else(|| nonempty("cover_album"))
                    .or_else(|| nonempty("id").filter(|id| is_album_id(id) && (o.contains_key("album") || o.contains_key("albumartist"))));
                if let Some(id) = id { COVER_COLORS.with(|m| m.borrow_mut().insert(id, c.to_string())); }
            }
            o.values().filter(|x| x.is_object() || x.is_array()).for_each(note_cover_colors);
        }
        _ => {}
    }
}

async fn request(method: &str, path: &str, body: Option<Value>) -> ApiResult {
    use gloo_net::http::RequestBuilder;
    let url = format!("/api{path}");
    let mut req = RequestBuilder::new(&url).method(gloo_net::http::Method::from_bytes(method.as_bytes()).unwrap_or(gloo_net::http::Method::GET))
        .credentials(RequestCredentials::SameOrigin)
        .header("X-Requested-With", "encore");
    if body.is_some() {
        req = req.header("Content-Type", "application/json");
    }
    let built = match body {
        Some(b) => req.body(b.to_string()),
        None => req.build(),
    };
    let res = match built {
        Ok(r) => match r.send().await {
            Ok(r) => r,
            Err(_) => {
                dom::dispatch("encore:server-down", None);
                return Err(ApiError { status: 0, message: "Not connected to the server".into(), data: Value::Null });
            }
        },
        Err(_) => return Err(ApiError { status: 0, message: "Bad request".into(), data: Value::Null }),
    };
    let status = res.status();
    dom::dispatch(if (502..=504).contains(&status) { "encore:server-down" } else { "encore:server-up" }, None);
    let text = res.text().await.unwrap_or_default();
    let data: Value = if text.is_empty() { Value::Null } else { serde_json::from_str(&text).unwrap_or(Value::String(text.clone())) };
    if res.ok() && text.contains("\"cover_color\"") { note_cover_colors(&data); }
    if !res.ok() {
        if status == 401 && !path.starts_with("/auth/") {
            dom::dispatch("encore:unauthorized", None);
        }
        let mut msg = match data.get("detail") {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Array(a)) => a.iter().map(|m| m.s("msg")).collect::<Vec<_>>().join(", "),
            _ => String::new(),
        };
        if msg.is_empty() { msg = res.status_text(); }
        if msg.is_empty() { msg = "Request failed".into(); }
        return Err(ApiError { status, message: msg, data });
    }
    Ok(data)
}

pub async fn get(path: &str) -> ApiResult { request("GET", path, None).await }
pub async fn post(path: &str, body: Value) -> ApiResult { request("POST", path, Some(body)).await }
pub async fn put(path: &str, body: Value) -> ApiResult { request("PUT", path, Some(body)).await }
pub async fn patch(path: &str, body: Value) -> ApiResult { request("PATCH", path, Some(body)).await }
pub async fn del(path: &str) -> ApiResult { request("DELETE", path, None).await }
/// `api.post(p)` with the default empty body.
pub async fn post_empty(path: &str) -> ApiResult { request("POST", path, Some(serde_json::json!({}))).await }
