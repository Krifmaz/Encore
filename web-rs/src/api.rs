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

/// What a failed call says when the server could not be reached.
const NOT_CONNECTED: &str = "Not connected to the server";

fn not_connected() -> ApiError { ApiError { status: 0, message: NOT_CONNECTED.into(), data: Value::Null } }

/// An answer from something in front of the server, not from the server: Encore marks every answer with
/// `x-encore` (`marked`) and sends its errors as JSON objects, while a proxy in front of a server that is down
/// answers with its own page – Caddy and nginx 502, Traefik 404 (its route went with the container),
/// Cloudflare 52x or 530. It means the same as no answer at all.
pub fn proxy_answer(status: u16, marked: bool, body: &str) -> bool {
    (status == 404 || status >= 500) && !marked && !serde_json::from_str::<Value>(body).is_ok_and(|v| v.is_object())
}

async fn request(method: &str, path: &str, body: Option<Value>) -> ApiResult { send(method, path, body, None).await }

/// A call given up after `within` ms (None: no limit). An answer that doesn't come in time counts as none
/// (status 0), and the request is aborted.
async fn send(method: &str, path: &str, body: Option<Value>, within: Option<u32>) -> ApiResult {
    let Some(ms) = within else { return exchange(method, path, body, None).await };
    let ctl = web_sys::AbortController::new().ok();
    let call = Box::pin(exchange(method, path, body, ctl.as_ref().map(|c| c.signal())));
    match futures::future::select(call, Box::pin(gloo_timers::future::TimeoutFuture::new(ms))).await {
        futures::future::Either::Left((r, _)) => r,
        futures::future::Either::Right(_) => {
            if let Some(c) = &ctl { c.abort(); }
            dom::dispatch("encore:server-down", None);
            Err(not_connected())
        }
    }
}

async fn exchange(method: &str, path: &str, body: Option<Value>, signal: Option<web_sys::AbortSignal>) -> ApiResult {
    use gloo_net::http::RequestBuilder;
    let url = format!("/api{path}");
    let mut req = RequestBuilder::new(&url).method(gloo_net::http::Method::from_bytes(method.as_bytes()).unwrap_or(gloo_net::http::Method::GET))
        .credentials(RequestCredentials::SameOrigin)
        .header("X-Requested-With", "encore")
        .abort_signal(signal.as_ref());
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
            // given up by its caller (get_abortable): not a sign the server is down
            Err(_) if signal.as_ref().is_some_and(|s| s.aborted()) => return Err(ApiError { status: 0, message: "Cancelled".into(), data: Value::Null }),
            Err(_) => {
                dom::dispatch("encore:server-down", None);
                return Err(not_connected());
            }
        },
        Err(_) => return Err(ApiError { status: 0, message: "Bad request".into(), data: Value::Null }),
    };
    answer(res, path).await
}

/// What the server answered, read the same way for JSON and form bodies: the JSON (or the text), or an
/// [`ApiError`] with the server's sentence (`detail`) and the whole answer in `data`.
async fn answer(res: gloo_net::http::Response, path: &str) -> ApiResult {
    let status = res.status();
    let text = res.text().await.unwrap_or_default();
    // a proxy's error page while the server is down: the same as no answer
    let proxy = proxy_answer(status, res.headers().get("x-encore").is_some(), &text);
    dom::dispatch(if proxy || (502..=504).contains(&status) { "encore:server-down" } else { "encore:server-up" }, None);
    if proxy { return Err(not_connected()); }
    let data: Value = if text.is_empty() { Value::Null } else { serde_json::from_str(&text).unwrap_or_else(|_| Value::String(text.clone())) };
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
/// `get` that `signal` can give up (an [`web_sys::AbortController`] the caller keeps): Discover's search drops
/// the search of the words typed before, whose MusicBrainz look-ups would otherwise go first in the server's
/// one-a-second line. A call given up this way is an error with status 0 and no "server down" event.
pub async fn get_abortable(path: &str, signal: Option<&web_sys::AbortSignal>) -> ApiResult { exchange("GET", path, None, signal.cloned()).await }
/// `get`, given up after `ms`: for the calls that tell whether the server can be reached at all (a server
/// whose address drops packets would otherwise keep them waiting for the connect timeout, a minute or more).
pub async fn get_within(path: &str, ms: u32) -> ApiResult { send("GET", path, None, Some(ms)).await }
/// `post_empty`, given up after `ms` (see `get_within`).
pub async fn post_empty_within(path: &str, ms: u32) -> ApiResult { send("POST", path, Some(serde_json::json!({})), Some(ms)).await }
pub async fn post(path: &str, body: Value) -> ApiResult { request("POST", path, Some(body)).await }
pub async fn put(path: &str, body: Value) -> ApiResult { request("PUT", path, Some(body)).await }
pub async fn patch(path: &str, body: Value) -> ApiResult { request("PATCH", path, Some(body)).await }
pub async fn del(path: &str) -> ApiResult { request("DELETE", path, None).await }
/// `api.post(p)` with the default empty body.
pub async fn post_empty(path: &str) -> ApiResult { request("POST", path, Some(serde_json::json!({}))).await }

/// POST a multipart form (a file upload): the browser sets the content type with its boundary. The answer and
/// the errors read as for [`post`] (the server's `detail`, the whole answer in `ApiError::data`).
pub async fn post_form(path: &str, form: web_sys::FormData) -> ApiResult {
    use gloo_net::http::RequestBuilder;
    let built = RequestBuilder::new(&format!("/api{path}"))
        .method(gloo_net::http::Method::POST)
        .credentials(RequestCredentials::SameOrigin)
        .header("X-Requested-With", "encore")
        .body(form);
    let res = match built {
        Ok(r) => match r.send().await {
            Ok(r) => r,
            Err(_) => {
                dom::dispatch("encore:server-down", None);
                return Err(not_connected());
            }
        },
        Err(_) => return Err(ApiError { status: 0, message: "Bad request".into(), data: Value::Null }),
    };
    answer(res, path).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_proxy_page_counts_as_no_answer() {
        // Caddy and nginx while Encore is down, Traefik once its route went, Cloudflare
        assert!(proxy_answer(502, false, ""));
        assert!(proxy_answer(502, false, "<html><body>Bad Gateway</body></html>"));
        assert!(proxy_answer(404, false, "404 page not found\n"));
        assert!(proxy_answer(530, false, "<!DOCTYPE html><title>Origin DNS error</title>"));
        assert!(proxy_answer(521, false, "error code: 521"));
        // Encore's own answers: marked, or its JSON errors
        assert!(!proxy_answer(404, true, ""));
        assert!(!proxy_answer(404, false, r#"{"detail":"Track not found"}"#));
        assert!(!proxy_answer(500, true, "Internal Server Error"));
        assert!(!proxy_answer(503, false, r#"{"detail":"busy"}"#));
        // errors that only Encore (or a sign-in proxy) gives, and answers that went through
        assert!(!proxy_answer(401, false, "<html>Unauthorized</html>"));
        assert!(!proxy_answer(403, false, ""));
        assert!(!proxy_answer(200, false, "<!doctype html>"));
    }
}
