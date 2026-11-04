//! A server for tests, without the process environment: [`TestServer`] opens a config folder of its own
//! (a database, the settings with overrides and no environment variables over them, the services' addresses
//! that tests point at stand-ins), and gives the state and the router; [`send`] and [`call`]
//! make requests the way the web app does, [`user`] adds an account. Tests in this crate use it, and other
//! crates' tests can too (feature `test-support`).
use crate::state::AppState;
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use axum::Router;
use encore_db::{p, Row};
use serde_json::Value;
use tower::ServiceExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

/// A server on its own config folder.
pub struct TestServer {
    /// the server's state
    pub st: AppState,
    /// the config folder (database, caches); the web app's folder is `static` inside it
    pub dir: PathBuf,
    /// the builtin plugins' adapters reach the state through this (crate::builtins::register)
    _plugins: std::sync::Arc<AppState>,
}

/// Numbers the test folders, so parallel tests never share one.
static SEQ: AtomicUsize = AtomicUsize::new(0);

impl TestServer {
    /// A new, empty config folder under the system's temp folder (named after `name`).
    pub fn new(name: &str) -> TestServer {
        let dir = std::env::temp_dir().join(format!("encore-{name}-{}-{}", std::process::id(), SEQ.fetch_add(1, Ordering::SeqCst)));
        let _ = std::fs::remove_dir_all(&dir);
        TestServer::in_dir(&dir)
    }

    /// The server on the config folder `dir` (created when missing; what is in it stays). No environment
    /// variable reaches its settings.
    pub fn in_dir(dir: &Path) -> TestServer {
        std::fs::create_dir_all(dir).unwrap();
        let st = AppState::open_with(dir, dir.join("static"), |db| encore_config::Config::load_with(db, |_| None)).unwrap();
        let plugins = crate::builtins::register(&st);
        TestServer { st, dir: dir.to_path_buf(), _plugins: plugins }
    }

    /// The same with these catalog plugins installed and switched on, as an upgraded server has them adopted
    /// (rows only: nothing is downloaded; a program the test needs is its own stand-in).
    pub fn with_plugins(self, ids: &[&str]) -> TestServer {
        for id in ids {
            self.st.plugins.adopt(id, None).unwrap();
        }
        self
    }

    /// The same with every download source's plugin and every download client's installed and switched on
    /// ([`SOURCE_PLUGINS`]): the sources then answer to their settings alone, as before plugins.
    pub fn with_sources(self) -> TestServer {
        with_sources(&self.st);
        self
    }

    /// The same with settings changed first (`{"library_path": …, "musicbrainz_url": …}`).
    pub fn with(self, settings: Value) -> TestServer {
        self.st.cfg.set_many(&settings);
        self
    }

    /// The same with the services it calls at other addresses (stand-ins for Tidal, Qobuz, YouTube Music).
    pub fn with_urls(mut self, urls: encore_http::Urls) -> TestServer {
        self.st.urls = std::sync::Arc::new(urls);
        // the adapters run on the state as it is now
        self._plugins = crate::builtins::register(&self.st);
        self
    }

    /// The same with its Docker socket at `sock` (a stand-in, or a path with nothing there), so a test never
    /// reaches the Docker of the machine it runs on.
    pub fn with_docker_socket(mut self, sock: impl Into<PathBuf>) -> TestServer {
        self.st.system.docker_socket = sock.into();
        // the adapters run on the state as it is now
        self._plugins = crate::builtins::register(&self.st);
        self
    }

    /// The API and the web app, as the binary serves them.
    pub fn app(&self) -> Router {
        crate::app(self.st.clone())
    }

    /// Removes the config folder.
    pub fn remove(self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A new account `name` with `role` ("user", "admin" …) and the password `a-long-password-1`: its row.
pub fn user(st: &AppState, name: &str, role: &str) -> Row {
    let id = crate::auth::create_user(st, name, "a-long-password-1", role, None, None).unwrap();
    st.db.one("SELECT * FROM users WHERE id=?", p![id]).unwrap().unwrap()
}

/// An answer of the router, read whole.
pub struct Resp {
    /// the status code
    pub status: StatusCode,
    /// the headers
    pub headers: HeaderMap,
    /// the body
    pub body: Vec<u8>,
}

impl Resp {
    /// A header's value ("" when there is none).
    pub fn h(&self, name: &str) -> String {
        self.headers.get(name).and_then(|v| v.to_str().ok()).unwrap_or("").to_string()
    }
    /// The body as JSON (null when it is not JSON).
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
    /// The body as text.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// One request the way the web app makes it (`x-requested-with: encore`), as the user whose session `cookie`
/// is ("" for none), with a JSON body when there is one and the extra `headers`.
pub async fn send(app: &Router, method: &str, uri: &str, cookie: &str, body: Option<Value>, headers: &[(&str, &str)]) -> Resp {
    let mut b = Request::builder().method(method).uri(uri).header("x-requested-with", "encore");
    if !cookie.is_empty() {
        b = b.header("cookie", cookie);
    }
    for (k, v) in headers {
        b = b.header(*k, *v);
    }
    let body = match body {
        Some(v) => {
            b = b.header("content-type", "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    let resp = app.clone().oneshot(b.body(body).unwrap()).await.unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let body = axum::body::to_bytes(resp.into_body(), 64 << 20).await.unwrap().to_vec();
    Resp { status, headers, body }
}

/// [`send`] without extra headers → (status, the JSON answer, the `set-cookie` header).
pub async fn call(app: &Router, method: &str, uri: &str, cookie: &str, body: Option<Value>) -> (StatusCode, Value, String) {
    let r = send(app, method, uri, cookie, body, &[]).await;
    (r.status, r.json(), r.h("set-cookie"))
}

/// The catalog plugins of the download sources and their download clients.
pub const SOURCE_PLUGINS: [&str; 12] =
    ["ytdlp", "streamrip", "slskd", "torznab", "qbittorrent", "transmission", "deluge", "rtorrent", "torrent-blackhole", "sabnzbd", "nzbget", "usenet-blackhole"];

/// Install and switch on every source's and client's plugin on `st` ([`SOURCE_PLUGINS`]; rows only).
pub fn with_sources(st: &AppState) {
    for id in SOURCE_PLUGINS {
        st.plugins.adopt(id, None).unwrap();
    }
}

/// The state of a server on `dir`: `TestServer::in_dir(dir).st`.
pub fn state_in(dir: &Path) -> AppState {
    TestServer::in_dir(dir).st
}

/// The state of a server on a new temp folder (named after `name`) that is removed again when this goes out
/// of scope, also when the test fails. It derefs to the state; pass `&*st` where a function takes `&impl Ctx`.
pub struct TempState {
    /// the state
    st: AppState,
    /// the folder removed on drop
    dir: PathBuf,
}

impl TempState {
    /// A state on a new folder named after `name`.
    pub fn new(name: &str) -> TempState {
        let ts = TestServer::new(name);
        TempState { st: ts.st.clone(), dir: ts.dir }
    }
}

impl std::ops::Deref for TempState {
    type Target = AppState;
    fn deref(&self) -> &AppState {
        &self.st
    }
}

impl Drop for TempState {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
