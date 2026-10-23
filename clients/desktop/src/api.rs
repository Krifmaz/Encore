//! The server: a reqwest client with a cookie store, `X-Requested-With: encore` on every
//! mutating request (the server's CSRF rule), the same error words as the web app
//! (web-rs/src/api.rs), the session (`/api/auth/me` + `/api/auth/info`) and app tokens.
//!
//! Paths are the web's: `api.get("/library/recent?limit=12")` fetches `<server>/api/library/…`.
//! Every call returns `serde_json::Value`; pages read fields with the helpers in [`crate::fmt`].

use reqwest::cookie::{CookieStore, Jar};
use reqwest::header::{HeaderMap, HeaderValue};
use reqwest::{Method, StatusCode, Url};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// What the server gets as `User-Agent`: it lists the device as "Encore desktop on <OS>".
pub fn user_agent() -> String {
    let os = if cfg!(target_os = "windows") {
        "Windows"
    } else if cfg!(target_os = "macos") {
        "Macintosh; Mac OS X"
    } else {
        "Linux"
    };
    format!("Encore desktop/{VERSION} ({os}) EncoreDesktop")
}

/// A failed request: `status` 0 means the server could not be reached.
#[derive(Clone, Debug, PartialEq)]
pub struct ApiError {
    pub status: u16,
    /// The server's sentence (`detail`), or the connection problem in plain words.
    pub message: String,
    /// The whole error body (`data.two_factor` and the like).
    pub data: Value,
}

impl ApiError {
    pub fn new(status: u16, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
            data: Value::Null,
        }
    }

    /// The server is not answering (as opposed to answering with an error).
    pub fn unreachable(&self) -> bool {
        self.status == 0
    }

    /// `data.<key>` is truthy.
    pub fn flag(&self, key: &str) -> bool {
        crate::fmt::truthy(&self.data, key)
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ApiError {}

pub type ApiResult = Result<Value, ApiError>;

/// The signed-in user and the server's `/api/auth/info`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Session {
    pub user: Value,
    pub info: Value,
}

impl Session {
    pub fn signed_in(&self) -> bool {
        self.user.is_object()
    }

    pub fn username(&self) -> &str {
        self.user["username"].as_str().unwrap_or("")
    }

    pub fn user_id(&self) -> i64 {
        crate::fmt::i(&self.user, "id")
    }

    pub fn is_admin(&self) -> bool {
        crate::fmt::truthy(&self.user, "is_admin")
            || matches!(self.user["role"].as_str(), Some("admin" | "owner"))
    }

    pub fn is_owner(&self) -> bool {
        self.user["role"].as_str() == Some("owner")
    }

    /// "Owner", "Admin", "Member" – the sidebar foot.
    pub fn role_label(&self) -> &'static str {
        if self.is_owner() {
            "Owner"
        } else if self.is_admin() {
            "Admin"
        } else {
            "Member"
        }
    }

    /// `perms.<name>` of the user.
    pub fn perm(&self, name: &str) -> bool {
        crate::fmt::truthy(&self.user["perms"], name)
    }

    /// The server lists this feature in `/api/auth/info` (`soulseek`, `roon`, …).
    pub fn has_feature(&self, name: &str) -> bool {
        self.info["features"]
            .as_array()
            .is_some_and(|a| a.iter().any(|f| f.as_str() == Some(name)))
    }

    /// The server's name for itself ("Encore" unless renamed).
    pub fn app_name(&self) -> String {
        match self.info["app_name"].as_str() {
            Some(n) if !n.is_empty() => n.to_string(),
            _ => "Encore".to_string(),
        }
    }
}

/// The client. One per process, shared as `Arc<Api>`; the base URL can change (Change server).
pub struct Api {
    client: reqwest::Client,
    /// The same cookies without the 60 s read limit: calls the server answers only when its work is done
    /// (a plugin install or update, an uploaded plugin unpacked and checked). See [`Api::post_slow`].
    slow: reqwest::Client,
    jar: Arc<Jar>,
    base: RwLock<String>,
    /// Offline mode: the server couldn't be reached, so `GET`s are answered from the kept copies
    /// ([`crate::api_cache`]) without asking it; [`Api::check_back`] turns it off again.
    offline: AtomicBool,
}

impl std::fmt::Debug for Api {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Api({})", self.base())
    }
}

/// The name of the server's session cookie.
pub const COOKIE: &str = "encore_session";

/// The longest a JSON call may take, start to end.
const JSON_TIMEOUT: Duration = Duration::from_secs(60);

/// The longest a slow call ([`Api::post_slow`]) may take: a plugin install fetches a program (slskd, a
/// Python venv, a translation model of gigabytes) before the server answers.
const SLOW_TIMEOUT: Duration = Duration::from_secs(3 * 3600);

impl Api {
    pub fn new(base: &str) -> Arc<Api> {
        let jar = Arc::new(Jar::default());
        let client = reqwest::Client::builder()
            .user_agent(user_agent())
            .cookie_provider(Arc::clone(&jar))
            .connect_timeout(Duration::from_secs(8))
            // A stall, not a total deadline: an album download or a backup upload over a slow
            // link takes as long as it takes. The JSON calls keep a total limit (`request`).
            .read_timeout(Duration::from_secs(60))
            .build()
            .expect("reqwest client");
        // the server says nothing until the install is done, so no read limit here; a dropped call would
        // also stop the server's work half way
        let slow = reqwest::Client::builder()
            .user_agent(user_agent())
            .cookie_provider(Arc::clone(&jar))
            .connect_timeout(Duration::from_secs(8))
            .build()
            .expect("reqwest client");
        Arc::new(Api {
            client,
            slow,
            jar,
            base: RwLock::new(crate::config::normalize(base)),
            offline: AtomicBool::new(false),
        })
    }

    /// "http://host:port" (no trailing slash; "" before a server is chosen).
    pub fn base(&self) -> String {
        self.base.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Point the client at another server. Cookies of the old one stay in the jar (they are
    /// keyed by host) so switching back needs no new sign-in.
    pub fn set_base(&self, base: &str) {
        let b = if base.trim().is_empty() {
            String::new()
        } else {
            crate::config::normalize(base)
        };
        *self.base.write().unwrap_or_else(|e| e.into_inner()) = b;
    }

    /// `<base>/api<path>`; a full `http(s)://` URL is used as is.
    pub fn url(&self, path: &str) -> String {
        if path.starts_with("http://") || path.starts_with("https://") {
            return path.to_string();
        }
        let base = self.base();
        if path.starts_with("/api/") {
            format!("{base}{path}")
        } else {
            format!("{base}/api{path}")
        }
    }

    fn base_url(&self) -> Option<Url> {
        Url::parse(&self.base()).ok()
    }

    /// Put a saved `encore_session` value in the jar for the current server.
    pub fn set_cookie(&self, value: &str) {
        if value.is_empty() {
            return;
        }
        if let Some(u) = self.base_url() {
            self.jar
                .add_cookie_str(&format!("{COOKIE}={value}; Path=/; HttpOnly"), &u);
        }
    }

    /// Forget the current server's session cookie (sign out).
    pub fn clear_cookie(&self) {
        if let Some(u) = self.base_url() {
            self.jar.add_cookie_str(
                &format!("{COOKIE}=; Path=/; Max-Age=0; Expires=Thu, 01 Jan 1970 00:00:00 GMT"),
                &u,
            );
        }
    }

    /// The current server's `encore_session` cookie, to remember between starts.
    pub fn session_cookie(&self) -> Option<String> {
        let u = self.base_url()?;
        let header = self.jar.cookies(&u)?;
        let s = header.to_str().ok()?;
        s.split(';')
            .map(str::trim)
            .find_map(|kv| kv.strip_prefix(&format!("{COOKIE}=")))
            .filter(|v| !v.is_empty())
            .map(str::to_string)
    }

    fn headers(mutating: bool, json: bool) -> HeaderMap {
        let mut h = HeaderMap::new();
        if mutating {
            h.insert("X-Requested-With", HeaderValue::from_static("encore"));
        }
        if json {
            h.insert(reqwest::header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
        }
        h.insert(reqwest::header::ACCEPT, HeaderValue::from_static("application/json"));
        h
    }

    /// The server couldn't be reached and pages show what this computer kept (the banner shows).
    pub fn is_offline(&self) -> bool {
        self.offline.load(Ordering::Relaxed)
    }

    /// Asks `/api/health` once (8 s at most, never from the kept copies); when the server answers,
    /// offline mode ends. True when it answered.
    pub async fn check_back(&self) -> bool {
        let base = self.base();
        if base.is_empty() {
            return false;
        }
        let up = match self
            .client
            .get(format!("{base}/api/health"))
            .timeout(Duration::from_secs(8))
            .send()
            .await
        {
            Ok(r) => !matches!(
                r.status(),
                StatusCode::BAD_GATEWAY | StatusCode::SERVICE_UNAVAILABLE | StatusCode::GATEWAY_TIMEOUT
            ),
            Err(_) => false,
        };
        if up {
            self.offline.store(false, Ordering::Relaxed);
        }
        up
    }

    /// Forgets the answers kept for the current session (sign out); call before the cookie goes.
    pub fn forget_kept(&self) {
        let cookie = self.session_cookie();
        crate::api_cache::forget(&crate::api_cache::owner(&self.base(), cookie.as_deref()));
    }

    /// One JSON call. A `GET` of the server's `/api/` is also kept ([`crate::api_cache`]) and
    /// answered from there when the server can't be reached: at once in offline mode (no wait for
    /// a timeout), and after a failed try otherwise, which starts offline mode. A page never
    /// opened before says so instead of failing with a connection error.
    async fn request(&self, method: Method, path: &str, body: Option<Value>) -> ApiResult {
        let base = self.base();
        if base.is_empty() {
            return Err(ApiError::new(0, "No server chosen"));
        }
        let url = self.url(path);
        let owner = if method == Method::GET && crate::api_cache::cacheable(&url, &base) {
            crate::api_cache::owner(&base, self.session_cookie().as_deref())
        } else {
            String::new()
        };
        if owner.is_empty() {
            return self.send(method, &url, body).await.map(|(v, _)| v);
        }
        if self.is_offline() {
            return kept(&owner, &url).ok_or_else(|| ApiError::new(0, NOT_KEPT));
        }
        match self.send(method, &url, body).await {
            Ok((v, text)) => {
                crate::api_cache::put(&owner, &url, &text);
                Ok(v)
            }
            Err(e) if e.unreachable() => match kept(&owner, &url) {
                Some(v) => {
                    self.offline.store(true, Ordering::Relaxed);
                    Ok(v)
                }
                None => Err(e),
            },
            Err(e) => Err(e),
        }
    }

    /// Sends one JSON call to `url`; on success the parsed answer and the text the server sent.
    async fn send(&self, method: Method, url: &str, body: Option<Value>) -> Result<(Value, String), ApiError> {
        let mutating = method != Method::GET;
        let mut req = self
            .client
            .request(method, url)
            .timeout(JSON_TIMEOUT)
            .headers(Self::headers(mutating, body.is_some()));
        if let Some(b) = body {
            req = req.body(b.to_string());
        }
        finish_text(req.send().await).await
    }

    async fn finish(&self, sent: Result<reqwest::Response, reqwest::Error>) -> ApiResult {
        finish_text(sent).await.map(|(v, _)| v)
    }

    pub async fn get(&self, path: &str) -> ApiResult {
        self.request(Method::GET, path, None).await
    }

    pub async fn post(&self, path: &str, body: Value) -> ApiResult {
        self.request(Method::POST, path, Some(body)).await
    }

    /// `api.post(p)` with the default empty body.
    pub async fn post_empty(&self, path: &str) -> ApiResult {
        self.request(Method::POST, path, Some(json!({}))).await
    }

    pub async fn put(&self, path: &str, body: Value) -> ApiResult {
        self.request(Method::PUT, path, Some(body)).await
    }

    pub async fn patch(&self, path: &str, body: Value) -> ApiResult {
        self.request(Method::PATCH, path, Some(body)).await
    }

    pub async fn del(&self, path: &str) -> ApiResult {
        self.request(Method::DELETE, path, None).await
    }

    /// A multipart upload (avatar, bug report picture, playlist file).
    pub async fn post_form(&self, path: &str, form: reqwest::multipart::Form) -> ApiResult {
        if self.base().is_empty() {
            return Err(ApiError::new(0, "No server chosen"));
        }
        let req = self
            .client
            .post(self.url(path))
            .headers(Self::headers(true, false))
            .multipart(form);
        self.finish(req.send().await).await
    }

    /// A POST the server answers only when its work is done (install, update or remove a plugin, install one
    /// from a URL): the same as [`Api::post`] without the 60 s limits, up to [`SLOW_TIMEOUT`].
    pub async fn post_slow(&self, path: &str, body: Value) -> ApiResult {
        if self.base().is_empty() {
            return Err(ApiError::new(0, "No server chosen"));
        }
        let req = self
            .slow
            .post(self.url(path))
            .timeout(SLOW_TIMEOUT)
            .headers(Self::headers(true, true))
            .body(body.to_string());
        self.finish(req.send().await).await
    }

    /// [`Api::post_form`] for an upload the server unpacks and checks before it answers (a plugin zip).
    pub async fn post_form_slow(&self, path: &str, form: reqwest::multipart::Form) -> ApiResult {
        if self.base().is_empty() {
            return Err(ApiError::new(0, "No server chosen"));
        }
        let req = self
            .slow
            .post(self.url(path))
            .timeout(SLOW_TIMEOUT)
            .headers(Self::headers(true, false))
            .multipart(form);
        self.finish(req.send().await).await
    }

    /// A multipart part from a file on disk (`field` is the form field name).
    pub async fn file_part(path: &std::path::Path) -> Result<reqwest::multipart::Part, ApiError> {
        let bytes = tokio::fs::read(path)
            .await
            .map_err(|e| ApiError::new(0, format!("Cannot read {}: {e}", path.display())))?;
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "file".into());
        let mime = match path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref()
        {
            Some("png") => "image/png",
            Some("jpg" | "jpeg") => "image/jpeg",
            Some("webp") => "image/webp",
            Some("gif") => "image/gif",
            Some("json") => "application/json",
            Some("m3u" | "m3u8") => "audio/x-mpegurl",
            Some("csv") => "text/csv",
            Some("zip") => "application/zip",
            _ => "application/octet-stream",
        };
        reqwest::multipart::Part::bytes(bytes)
            .file_name(name)
            .mime_str(mime)
            .map_err(|e| ApiError::new(0, e.to_string()))
    }

    /// Raw bytes held in memory (covers, pictures). `path` may be a full URL. Files the user
    /// saves go through [`Api::download_to`], which does not hold them in memory.
    pub async fn get_bytes(&self, path: &str) -> Result<Vec<u8>, ApiError> {
        if self.base().is_empty() && !path.starts_with("http") {
            return Err(ApiError::new(0, "No server chosen"));
        }
        let res = self
            .client
            .get(self.url(path))
            .send()
            .await
            .map_err(|e| connection_error(&e))?;
        let status = res.status();
        if !status.is_success() {
            return Err(ApiError::new(
                status.as_u16(),
                status.canonical_reason().unwrap_or("Request failed"),
            ));
        }
        res.bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|e| connection_error(&e))
    }

    /// Save `path` (an API path or a full URL) to `dest` as it arrives: the body goes to
    /// `<dest>.part`, which becomes `dest` once complete (and is removed when the download
    /// fails), so an album zip of a few GB is never held in memory and a cut-off download never
    /// looks finished. Returns the number of bytes written.
    pub async fn download_to(&self, path: &str, dest: &std::path::Path) -> Result<u64, ApiError> {
        use tokio::io::AsyncWriteExt;
        if self.base().is_empty() && !path.starts_with("http") {
            return Err(ApiError::new(0, "No server chosen"));
        }
        let mut res = self
            .client
            .get(self.url(path))
            .send()
            .await
            .map_err(|e| connection_error(&e))?;
        let status = res.status();
        if !status.is_success() {
            return Err(ApiError::new(
                status.as_u16(),
                status.canonical_reason().unwrap_or("Request failed"),
            ));
        }
        let part = part_path(dest);
        let disk = |e: std::io::Error| ApiError::new(0, format!("Cannot write {}: {e}", dest.display()));
        let result = async {
            let mut file = tokio::fs::File::create(&part).await.map_err(disk)?;
            let mut written = 0u64;
            while let Some(chunk) = res.chunk().await.map_err(|e| connection_error(&e))? {
                file.write_all(&chunk).await.map_err(disk)?;
                written += chunk.len() as u64;
            }
            file.flush().await.map_err(disk)?;
            file.sync_all().await.map_err(disk)?;
            drop(file);
            tokio::fs::rename(&part, dest).await.map_err(disk)?;
            Ok(written)
        }
        .await;
        if result.is_err() {
            let _ = tokio::fs::remove_file(&part).await;
        }
        result
    }

    // ----- auth ------------------------------------------------------------------------

    /// `GET /api/auth/info`: also the reachability probe of the server picker.
    pub async fn info(&self) -> ApiResult {
        let v = self.get("/auth/info").await?;
        if v.get("server").is_none() && v.get("features").is_none() {
            return Err(ApiError::new(0, "That address is not an Encore server"));
        }
        Ok(v)
    }

    pub async fn me(&self) -> ApiResult {
        self.get("/auth/me").await
    }

    /// `/api/auth/me` + `/api/auth/info` together.
    pub async fn session(&self) -> Result<Session, ApiError> {
        let user = self.me().await?;
        let info = self.info().await.unwrap_or(Value::Null);
        Ok(Session { user, info })
    }

    /// `POST /api/auth/login`; `code` is the authenticator code when the account has two-step
    /// sign-in (the error carries `two_factor: true` when it is needed).
    pub async fn login(&self, username: &str, password: &str, code: Option<&str>) -> ApiResult {
        let mut body = json!({ "username": username.trim(), "password": password });
        if let Some(c) = code.map(str::trim).filter(|c| !c.is_empty()) {
            body["code"] = Value::String(c.to_string());
        }
        self.post("/auth/login", body).await
    }

    pub async fn logout(&self) -> ApiResult {
        let r = self.post_empty("/auth/logout").await;
        self.clear_cookie();
        r
    }

    /// End the app session of `token` on `server` (any remembered server, not only the current
    /// one). The cookie logout leaves it alone: the server ends an app session only when the
    /// request carries it as a bearer, and then touches nothing else.
    pub async fn revoke_app_token(&self, server: &str, token: &str) -> ApiResult {
        if server.trim().is_empty() || token.is_empty() {
            return Ok(Value::Null);
        }
        let url = format!("{}/api/auth/logout", crate::config::normalize(server));
        let req = self
            .client
            .post(url)
            .timeout(Duration::from_secs(15))
            .headers(Self::headers(true, true))
            .bearer_auth(token)
            .body("{}");
        self.finish(req.send().await).await
    }

    /// `POST /api/auth/app-token`: a bearer token for the engine link, listed under the
    /// account's sessions as `device_name`.
    pub async fn app_token(&self, device_id: &str, device_name: &str) -> Result<String, ApiError> {
        let v = self
            .post(
                "/auth/app-token",
                json!({ "device_id": device_id, "device_name": device_name }),
            )
            .await?;
        v["token"]
            .as_str()
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .ok_or_else(|| ApiError::new(500, "The server gave no token"))
    }
}

/// `<dest>.part`, next to `dest`.
fn part_path(dest: &std::path::Path) -> std::path::PathBuf {
    let mut name = dest.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(".part");
    dest.with_file_name(name)
}

/// A page asked for in offline mode that this computer never kept an answer for.
pub const NOT_KEPT: &str = "Offline – this page wasn't opened on this computer before, so nothing of it is saved here.";

/// The kept answer to `GET url`, parsed (None when there is none or it no longer parses).
fn kept(owner: &str, url: &str) -> Option<Value> {
    let text = crate::api_cache::get(owner, url)?;
    if text.is_empty() {
        return Some(Value::Null);
    }
    serde_json::from_str(&text).ok()
}

/// The answer of a sent call: its JSON (and the text it came from, to keep), or the error in the
/// web app's words (502–504 from a proxy count as "not connected").
async fn finish_text(sent: Result<reqwest::Response, reqwest::Error>) -> Result<(Value, String), ApiError> {
    let res = match sent {
        Ok(r) => r,
        Err(e) => return Err(connection_error(&e)),
    };
    let status = res.status();
    let text = res.text().await.unwrap_or_default();
    let data: Value = if text.is_empty() {
        Value::Null
    } else {
        serde_json::from_str(&text).unwrap_or(Value::String(text.clone()))
    };
    if status.is_success() {
        return Ok((data, text));
    }
    let mut msg = match data.get("detail") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(a)) => a
            .iter()
            .map(|m| crate::fmt::s(m, "msg"))
            .collect::<Vec<_>>()
            .join(", "),
        _ => String::new(),
    };
    if msg.is_empty() {
        msg = status
            .canonical_reason()
            .unwrap_or("Request failed")
            .to_string();
    }
    if matches!(
        status,
        StatusCode::BAD_GATEWAY | StatusCode::SERVICE_UNAVAILABLE | StatusCode::GATEWAY_TIMEOUT
    ) {
        return Err(ApiError {
            status: 0,
            message: "Not connected to the server".into(),
            data,
        });
    }
    Err(ApiError {
        status: status.as_u16(),
        message: msg,
        data,
    })
}

fn connection_error(e: &reqwest::Error) -> ApiError {
    let message = if e.is_timeout() {
        "The server took too long to answer".to_string()
    } else if e.is_connect() {
        "Not connected to the server".to_string()
    } else if e.is_decode() {
        "Bad answer from the server".to_string()
    } else {
        let s = e.to_string();
        let s = s.split(": ").last().unwrap_or(&s).to_string();
        if s.is_empty() {
            "Not connected to the server".to_string()
        } else {
            s
        }
    };
    ApiError::new(0, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_and_cookies() {
        let api = Api::new("http://127.0.0.1:18085/");
        assert_eq!(api.base(), "http://127.0.0.1:18085");
        assert_eq!(api.url("/library/recent?limit=1"), "http://127.0.0.1:18085/api/library/recent?limit=1");
        assert_eq!(api.url("/api/auth/info"), "http://127.0.0.1:18085/api/auth/info");
        assert_eq!(api.url("https://x/y"), "https://x/y");
        assert_eq!(api.session_cookie(), None);
        api.set_cookie("abc");
        assert_eq!(api.session_cookie().as_deref(), Some("abc"));
        api.clear_cookie();
        assert_eq!(api.session_cookie(), None);
        api.set_base("nas:8080");
        assert_eq!(api.base(), "http://nas:8080");
    }

    #[tokio::test]
    async fn downloads_reach_the_disk_whole_or_not_at_all() {
        use std::io::{Read, Write};
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for (i, s) in l.incoming().enumerate() {
                let Ok(mut s) = s else { continue };
                let mut buf = [0u8; 2048];
                let _ = s.read(&mut buf);
                let reply: &[u8] = if i == 0 {
                    b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello"
                } else {
                    // promises 100 bytes, sends 3 and hangs up
                    b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\nabc"
                };
                let _ = s.write_all(reply);
            }
        });
        let api = Api::new(&format!("http://127.0.0.1:{port}"));
        let d = tempfile::tempdir().unwrap();
        let dest = d.path().join("album.zip");
        assert_eq!(api.download_to("/library/albums/1/download", &dest).await.unwrap(), 5);
        assert_eq!(std::fs::read(&dest).unwrap(), b"hello");
        assert!(!part_path(&dest).exists());
        let cut = d.path().join("cut.zip");
        assert!(api.download_to("/library/albums/2/download", &cut).await.is_err());
        assert!(!cut.exists() && !part_path(&cut).exists(), "a cut-off download leaves nothing");
    }

    #[test]
    fn session_reads_the_user() {
        let s = Session {
            user: json!({ "username": "admin", "role": "owner", "is_admin": true, "perms": { "pick": true } }),
            info: json!({ "features": ["soulseek"], "app_name": "" }),
        };
        assert!(s.signed_in() && s.is_admin() && s.is_owner() && s.perm("pick"));
        assert_eq!(s.role_label(), "Owner");
        assert!(s.has_feature("soulseek") && !s.has_feature("roon"));
        assert_eq!(s.app_name(), "Encore");
        assert!(user_agent().contains("EncoreDesktop"));
    }
}
