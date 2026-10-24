//! Encore's web API: the routers by URL prefix, the signed-in user extractors, the middleware (security
//! headers, the busy gate), the enrichment pass the handlers run on their answers (covers, credits,
//! translations: [`enrich`]), the server state ([`state::AppState`]) and the web app (web-rs) served at `/`.
//! The `encore` binary builds the state and serves [`app`].
/// Release version (e.g. 2026.10.11.1), set by the release build; local builds use Cargo's.
pub use encore_core::VERSION;

pub mod auth;
pub mod builtins;
pub mod contribute;
pub mod covers;
pub mod email;
pub mod enrich;
/// The HTTP routes, one router per area.
pub mod routes;
pub mod scheduler;
pub mod search;
pub mod signin;
/// The server state every handler gets.
pub mod state;
pub mod timing;
pub mod translate;

#[cfg(any(test, feature = "test-support"))]
pub mod test_support;

use axum::extract::Request;
use axum::http::{header, HeaderValue};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::response::Redirect;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::json;
use state::AppState;
use std::path::PathBuf;
use tower_http::compression::predicate::{DefaultPredicate, NotForContentType, Predicate};
use tower_http::compression::CompressionLayer;
use tower_http::services::{ServeDir, ServeFile};

/// Middleware: the security headers, the `x-encore` mark and the cache policy on every answer.
async fn security_headers(req: Request, next: Next) -> Response {
    let path = req.uri().path().to_string();
    let mut resp = next.run(req).await;
    let h = resp.headers_mut();
    h.entry("x-content-type-options").or_insert(HeaderValue::from_static("nosniff"));
    // share pages may be framed: X shows a "player" card by loading the page in a frame
    if !path.starts_with("/s/") {
        h.entry("x-frame-options").or_insert(HeaderValue::from_static("DENY"));
    }
    h.entry("referrer-policy").or_insert(HeaderValue::from_static("same-origin"));
    // every answer says it is Encore's: the web app's service worker and offline mode tell a proxy's error page
    // (Caddy's 502, Traefik's 404, Cloudflare's 530 while Encore is down) from Encore's own answers by it
    h.insert("x-encore", HeaderValue::from_static("1"));
    let cache = if path.starts_with("/api/") {
        "no-store"
    } else if hashed_asset(&path) {
        "public, max-age=31536000, immutable"   // the web-rs build names files by their content
    } else {
        "no-cache"   // app files: cheap 304s, updates show at once
    };
    h.entry(header::CACHE_CONTROL).or_insert(HeaderValue::from_static(cache));
    resp
}

/// `/encore-web-f92ab5b84b305a59_bg.wasm`: a file whose name carries a hash of its content (trunk's build,
/// next to index.html at the root), so it never changes under that name. Trunk writes the hash without its
/// leading zeros (one build in 16 has 15 digits), so 12 digits are enough; web-rs/static/sw.js has the same rule.
fn hashed_asset(path: &str) -> bool {
    let Some(name) = path.strip_prefix('/') else { return false };
    let stem = name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name);
    let stem = stem.strip_suffix("_bg").unwrap_or(stem);
    !name.contains('/') && stem.rsplit_once('-').is_some_and(|(_, h)| h.len() >= 12 && h.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// The build id of the web app in `static_dir`: the content hash in the name of trunk's script,
/// `encore-web-<hash>.js` (the same hash sw.js makes its cache name from). `/api/health` sends it as `web`, so
/// an open tab can tell that the server now serves another build than the one it runs (web-rs/src/update.rs)
/// and reload itself. None when the folder has no such file (a server without the web app, the tests).
pub fn web_build(static_dir: &std::path::Path) -> Option<String> {
    let mut found: Vec<String> = std::fs::read_dir(static_dir).ok()?
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter_map(|name| web_build_of(&name).map(str::to_string))
        .collect();
    // one build per folder; should an old script linger next to the new one, the answer is still stable
    found.sort();
    found.pop()
}

/// The hash of a file name `encore-web-<hash>.js` (not the `_bg.wasm`, not a `.js.br` copy); None for any
/// other name. Trunk drops leading zeros of the hash, so 12 hex digits or more count, as in [`hashed_asset`].
fn web_build_of(name: &str) -> Option<&str> {
    let hash = name.strip_prefix("encore-web-")?.strip_suffix(".js")?;
    (hash.len() >= 12 && hash.bytes().all(|b| b.is_ascii_hexdigit())).then_some(hash)
}

/// `/next/` was the Rust UI's address while the JavaScript UI lived at `/`; an alias for one release (0.26).
/// `/next` and `/next/` go to `/`; `/next/<path>` goes to `/<path>` with its query string, so an old tab
/// reloading its hashed script, a bookmarked `/next/share.html` and a desktop app whose saved address ends in
/// `/next/` (it checks `/next/api/health`) all reach the file they asked for. The browser keeps the `#/page`
/// part across the redirect.
async fn next_alias(uri: axum::http::Uri) -> Redirect {
    let rest = uri.path().strip_prefix("/next").unwrap_or("").trim_start_matches('/');
    let query = uri.query().map(|q| format!("?{q}")).unwrap_or_default();
    Redirect::temporary(&format!("/{rest}{query}"))
}

/// The whole app: every router, the middlewares, and the web app's files served at `/`.
pub fn app(st: AppState) -> Router {
    let static_dir = st.static_dir.clone();
    let busy = st.busy.clone();
    let perf = st.web.perf.clone();
    // read once: a new web build only ever arrives with a new server process (an update, a new image)
    let web = web_build(&static_dir);
    // the web app (web-rs, built by trunk into static/): one folder, index.html for every path that is not a file.
    // The image keeps each text file brotli- and gzip-compressed next to it (`app.css.br`, `app.css.gz`): a
    // browser that takes one gets it, anything else the plain file; caches keep the variants apart (Vary).
    let index = ServeFile::new(static_dir.join("index.html")).precompressed_br().precompressed_gzip();
    let spa = ServeDir::new(&static_dir).precompressed_br().precompressed_gzip().fallback(index);
    let spa = tower_http::set_header::SetResponseHeader::if_not_present(spa, header::VARY, HeaderValue::from_static("accept-encoding"));
    let router = Router::new()
        .route("/next", get(next_alias))
        .route("/next/", get(next_alias))
        .route("/next/{*rest}", get(next_alias))
        // `web`: the build of the web app served here (null without one); open tabs reload when it changes
        .route("/api/health", get(move || async move { Json(json!({ "ok": true, "version": crate::VERSION, "server": "rust", "web": web })) }))
        .merge(routes::auth::router())
        .merge(routes::admin::router())
        .merge(routes::library::router())
        .merge(routes::discover::router())
        .merge(routes::foryou::router())
        .merge(routes::catalog::router())
        .merge(routes::requests::router())
        .merge(routes::report::router())
        .merge(routes::own_file::router())
        .merge(routes::picker::router())
        .merge(routes::surround::router())
        .merge(routes::soulseek::router())
        .merge(routes::system::router())
        .merge(routes::tagging::router())
        .merge(routes::playlists::router())
        .merge(routes::playlist_links::router())
        .merge(routes::playlist_collab::router())
        .merge(routes::stats::router())
        .merge(routes::bugreports::router())
        .merge(routes::remote::router())
        .merge(routes::group::router())
        .merge(routes::roon::router())
        .merge(routes::playback::router())
        .merge(contribute::router())
        .merge(signin::router())
        .merge(translate::router())
        .merge(routes::ytauth::router())
        .merge(routes::perf::router())
        .merge(routes::files::router())
        .merge(routes::plugins::router())
        .merge(routes::wrapper::router())
        .merge(routes::changelog::router())
        // the handlers add covers, credits and translations to their answers themselves (crate::enrich)
        .with_state(st)
        // someone waits for every API call: its MusicBrainz lookups go first in the line (encore_http::mb)
        .layer(middleware::from_fn(musicbrainz_first))
        // every API call timed per route (the web app's files below are not): a slow one is logged, and all
        // of them show in /api/admin/perf
        .layer(middleware::from_fn_with_state(perf, timing::layer));
    router
        .fallback_service(spa)
        .layer(middleware::from_fn(security_headers))
        // API calls hold background passes (cover thumbnails …) to one low-priority thread
        .layer(middleware::from_fn_with_state(busy, encore_jobs::busy::layer))
        // gzip for JSON and pages – never for audio: it only burns CPU and gets in the way of range requests
        .layer(CompressionLayer::new().compress_when(DefaultPredicate::new().and(NotForContentType::new("audio/"))))
}

/// An API call's MusicBrainz lookups are for a page someone is looking at: they go before imports, requests
/// and background work in MusicBrainz's line (work the call starts in a task of its own is not hurried).
async fn musicbrainz_first(req: Request, next: Next) -> Response {
    encore_http::mb::with_priority(encore_http::mb::Priority::Interactive, next.run(req)).await
}

/// What the first requests after a start would otherwise wait for, made in the background right away: the
/// artist index (every answer with songs needs it; about a second on a library of 60 000 songs), the download
/// tools' versions (a Python start each for yt-dlp and streamrip: the picker, Diagnostics and the request
/// worker ask for them) and Home's new releases (60 Deezer look-ups). Requests that arrive while one of these
/// is being made wait for it rather than making their own. Also starts the refresh of MusicBrainz answers
/// shown from disk past their hour (encore_http::mb::Mb::refresh_stale).
pub fn warm_up(st: &AppState) {
    let s = st.clone();
    tokio::task::spawn_blocking(move || {
        s.library.artists.index(&s.db);
    });
    let s = st.clone();
    tokio::task::spawn_blocking(move || {
        encore_system::tools::cached(&s);
        encore_requests::sources::streamrip::binary(&s);
    });
    let s = st.clone();
    tokio::spawn(async move { routes::discover::warm_new_releases(&s).await });
    // MusicBrainz look-ups a page was shown from disk past their hour are asked again in the background
    let s = st.clone();
    tokio::spawn(async move { s.mb.refresh_stale(&s).await });
    // 0.36, once: who uploaded or imported the songs the file manager brought in before songs kept it
    let s = st.clone();
    tokio::task::spawn_blocking(move || {
        let roots = encore_files::roots::all(&s.cfg, s.data_dir(), &encore_requests::slskd_managed::download_dir(&s));
        let n = encore_files::backfill_added_from_log(&s, &roots);
        if n > 0 {
            tracing::info!("who added it: {n} file manager lines read back from the activity log");
        }
    });
}

/// The server's state from the environment: the config folder `ENCORE_CONFIG_DIR` (default /config) and the web
/// app in `ENCORE_STATIC_DIR` (default /app/static).
/// The plugins at start: the builtin adapters registered (keep the answer while the server runs: they reach
/// the state through it), what was in use before plugins adopted (the first start after an upgrade; rows only,
/// at once), then – in the background – the programs that were in use but are missing installed, tried again
/// after 1, 2, 5, 10 and 30 minutes, then every hour, until they are (no network at the first start).
pub fn start_plugins(st: &AppState) -> std::sync::Arc<AppState> {
    let anchor = builtins::register(st);
    let adopted = st.plugins.adopt_in_use();
    if !adopted.is_empty() {
        tracing::info!("plugins: adopted {}", adopted.join(", "));
    }
    let reg = st.plugins.clone();
    tokio::spawn(async move {
        let r = reg.migrate_until_done(encore_plugins::migrate::retry_after).await;
        if r["installed"].as_array().is_some_and(|a| !a.is_empty()) || r["failed"].as_array().is_some_and(|a| !a.is_empty()) {
            tracing::info!("plugins: {r}");
        }
    });
    anchor
}

/// The server state from `ENCORE_CONFIG_DIR` (default /config) and `ENCORE_STATIC_DIR` (default /app/static).
pub fn build_state() -> anyhow::Result<AppState> {
    let data = PathBuf::from(std::env::var("ENCORE_CONFIG_DIR").unwrap_or_else(|_| "/config".into()));
    let static_dir = PathBuf::from(std::env::var("ENCORE_STATIC_DIR").unwrap_or_else(|_| "/app/static".into()));
    AppState::open(&data, static_dir)
}

#[cfg(test)]
pub mod import_tests;

#[cfg(test)]
mod server_tests;

#[cfg(test)]
mod tests {
    #[test]
    fn hashed_assets_are_cached_for_good() {
        assert!(super::hashed_asset("/encore-web-f92ab5b84b305a59.js"));
        assert!(super::hashed_asset("/encore-web-f92ab5b84b305a59_bg.wasm"));
        // a hash that started with a zero
        assert!(super::hashed_asset("/encore-web-59d563296e62da3_bg.wasm"));
        for plain in ["/", "/index.html", "/sw.js", "/share.html", "/manifest.webmanifest", "/favicon.svg", "/css/app.css", "/icons/apple-touch-icon.png", "/js/app.js"] {
            assert!(!super::hashed_asset(plain), "{plain}");
        }
        // only the root level: trunk puts the hashed files next to index.html, and /next/ is a redirect now
        assert!(!super::hashed_asset("/next/encore-web-f92ab5b84b305a59.js"));
        assert!(!super::hashed_asset("/a/encore-web-f92ab5b84b305a59.js"));
    }

    #[test]
    fn the_web_build_is_the_scripts_hash() {
        assert_eq!(super::web_build_of("encore-web-f92ab5b84b305a59.js"), Some("f92ab5b84b305a59"));
        // a hash that started with a zero (15 digits)
        assert_eq!(super::web_build_of("encore-web-59d563296e62da3.js"), Some("59d563296e62da3"));
        for other in ["encore-web-f92ab5b84b305a59_bg.wasm", "encore-web-f92ab5b84b305a59.js.br", "encore-web-f92ab5b84b305a59.js.gz",
                      "encore-web-short.js", "encore-web-f92ab5b8.js", "index.html", "sw.js", "encore-web-.js"] {
            assert_eq!(super::web_build_of(other), None, "{other}");
        }
        let dir = std::env::temp_dir().join(format!("encore-web-build-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(super::web_build(&dir), None, "an empty folder");
        for name in ["index.html", "encore-web-f92ab5b84b305a59.js", "encore-web-f92ab5b84b305a59.js.br", "encore-web-f92ab5b84b305a59_bg.wasm"] {
            std::fs::write(dir.join(name), "x").unwrap();
        }
        assert_eq!(super::web_build(&dir).as_deref(), Some("f92ab5b84b305a59"));
        assert_eq!(super::web_build(&dir.join("missing")), None, "no folder");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The Rust UI's trunk output served at `/`: index.html for the root and for unknown paths, its own files
    /// (css, sw.js) from the same folder, the hashed files cached for good, `/next/…` redirected to `/…`.
    #[tokio::test]
    async fn the_rust_ui_is_the_website() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use tower::ServiceExt;
        let dir = std::env::temp_dir().join(format!("encore-ui-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let dist = dir.join("static");
        std::fs::create_dir_all(dist.join("css")).unwrap();
        std::fs::write(dist.join("index.html"), "<meta name=\"application-name\" content=\"Encore\">rust ui").unwrap();
        std::fs::write(dist.join("encore-web-f92ab5b84b305a59.js"), "// glue").unwrap();
        std::fs::write(dist.join("css/app.css"), "body{}").unwrap();
        std::fs::write(dist.join("sw.js"), "// sw").unwrap();
        let rust = {
            let mut st = crate::test_support::state_in(&dir.join("config"));
            st.static_dir = dist;
            super::app(st)
        };
        let get = |app: &super::Router, path: &str| {
            let app = app.clone();
            let req = Request::builder().uri(path).body(Body::empty()).unwrap();
            async move {
                let resp = app.oneshot(req).await.unwrap();
                let status = resp.status();
                let headers = resp.headers().clone();
                let body = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
                (status, headers, String::from_utf8_lossy(&body).to_string())
            }
        };
        let (s, h, b) = get(&rust, "/").await;
        assert_eq!((s, b.as_str()), (StatusCode::OK, "<meta name=\"application-name\" content=\"Encore\">rust ui"));
        assert_eq!(h.get("cache-control").unwrap(), "no-cache");
        let (s, _, b) = get(&rust, "/some/deep/link").await;
        assert_eq!((s, b.as_str()), (StatusCode::OK, "<meta name=\"application-name\" content=\"Encore\">rust ui"), "index.html for paths that are not files");
        let (s, h, _) = get(&rust, "/encore-web-f92ab5b84b305a59.js").await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(h.get("cache-control").unwrap(), "public, max-age=31536000, immutable");
        // the alias: /next and /next/ go to /, everything under it keeps its path (and query string)
        for (alias, to) in [("/next", "/"), ("/next/", "/"), ("/next/encore-web-old.js", "/encore-web-old.js"), ("/next/api/health", "/api/health"),
                            ("/next/share.html?s=abc", "/share.html?s=abc")] {
            let (s, h, _) = get(&rust, alias).await;
            assert_eq!(s, StatusCode::TEMPORARY_REDIRECT, "{alias}");
            assert_eq!(h.get("location").unwrap(), to, "{alias}");
        }
        // the Rust UI's own files come from the same folder
        let (s, _, b) = get(&rust, "/css/app.css").await;
        assert_eq!((s, b.as_str()), (StatusCode::OK, "body{}"));
        let (s, _, b) = get(&rust, "/sw.js").await;
        assert_eq!((s, b.as_str()), (StatusCode::OK, "// sw"));
        // /api/health names the build served here, so open tabs notice a new one
        let (s, _, b) = get(&rust, "/api/health").await;
        let v: serde_json::Value = serde_json::from_str(&b).unwrap();
        assert_eq!((s, v["web"].as_str()), (StatusCode::OK, Some("f92ab5b84b305a59")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The image keeps `<file>.br` and `<file>.gz` next to each text file of the web app: a client that takes
    /// brotli gets the .br as it is (gzip: the .gz), a client that says nothing gets the plain file; index.html
    /// the same, also for a deep link; a file without siblings is plain; the cache rules stay.
    #[tokio::test]
    async fn the_web_app_is_served_precompressed() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use tower::ServiceExt;
        let dir = std::env::temp_dir().join(format!("encore-precompressed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let dist = dir.join("static");
        std::fs::create_dir_all(dist.join("css")).unwrap();
        // the siblings' bytes are made up: the server sends them as they are and never decodes them
        for (name, text) in [("index.html", "<!doctype html>encore"), ("encore-web-f92ab5b84b305a59.js", "// glue"), ("css/app.css", "body{}")] {
            std::fs::write(dist.join(name), text).unwrap();
            std::fs::write(dist.join(format!("{name}.br")), format!("br:{text}")).unwrap();
            std::fs::write(dist.join(format!("{name}.gz")), format!("gz:{text}")).unwrap();
        }
        std::fs::write(dist.join("favicon.svg"), "<svg/>").unwrap();
        let app = {
            let mut st = crate::test_support::state_in(&dir.join("config"));
            st.static_dir = dist;
            super::app(st)
        };
        let get = |path: &str, accept: Option<&str>| {
            let app = app.clone();
            let mut req = Request::builder().uri(path);
            if let Some(a) = accept {
                req = req.header("accept-encoding", a);
            }
            let req = req.body(Body::empty()).unwrap();
            async move {
                let resp = app.oneshot(req).await.unwrap();
                let status = resp.status();
                let h = resp.headers().clone();
                let body = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
                let header = move |name: &str| h.get(name).map(|v| v.to_str().unwrap().to_string());
                (status, header, String::from_utf8_lossy(&body).to_string())
            }
        };
        // brotli when the client takes it, before gzip
        let (s, h, b) = get("/css/app.css", Some("gzip, deflate, br")).await;
        assert_eq!((s, b.as_str()), (StatusCode::OK, "br:body{}"));
        assert_eq!(h("content-encoding").as_deref(), Some("br"));
        assert!(h("content-type").unwrap().starts_with("text/css"), "the type of the file, not of the sibling");
        assert_eq!(h("cache-control").as_deref(), Some("no-cache"));
        assert_eq!(h("vary").as_deref(), Some("accept-encoding"), "caches keep the variants apart");
        let (_, h, b) = get("/css/app.css", Some("gzip")).await;
        assert_eq!((h("content-encoding").as_deref(), b.as_str()), (Some("gzip"), "gz:body{}"));
        // no Accept-Encoding (or only identity): the plain file
        for accept in [None, Some("identity")] {
            let (s, h, b) = get("/css/app.css", accept).await;
            assert_eq!((s, b.as_str()), (StatusCode::OK, "body{}"), "{accept:?}");
            assert_eq!(h("content-encoding"), None, "{accept:?}");
            assert_eq!(h("vary").as_deref(), Some("accept-encoding"), "{accept:?}");
        }
        // the hashed glue: still cached for good
        let (_, h, b) = get("/encore-web-f92ab5b84b305a59.js", Some("br")).await;
        assert_eq!((h("content-encoding").as_deref(), b.as_str()), (Some("br"), "br:// glue"));
        assert_eq!(h("cache-control").as_deref(), Some("public, max-age=31536000, immutable"));
        // index.html for the root and, through the fallback, for a deep link
        for path in ["/", "/some/deep/link"] {
            let (s, h, b) = get(path, Some("br, gzip")).await;
            assert_eq!((s, b.as_str()), (StatusCode::OK, "br:<!doctype html>encore"), "{path}");
            assert_eq!(h("content-encoding").as_deref(), Some("br"), "{path}");
            assert!(h("content-type").unwrap().starts_with("text/html"), "{path}");
            assert_eq!(h("cache-control").as_deref(), Some("no-cache"), "{path}");
            let (_, h, b) = get(path, None).await;
            assert_eq!((h("content-encoding"), b.as_str()), (None, "<!doctype html>encore"), "{path}");
        }
        // no siblings: the plain file, whatever the client takes
        let (s, h, b) = get("/favicon.svg", Some("br, gzip")).await;
        assert_eq!((s, h("content-encoding"), b.as_str()), (StatusCode::OK, None, "<svg/>"));
        // the API is not touched by any of it
        let (_, h, _) = get("/api/health", Some("br, gzip")).await;
        assert_eq!(h("cache-control").as_deref(), Some("no-store"));
        // every answer, the app's files and the API alike, says it is Encore's (sw.js tells proxy pages by it)
        assert_eq!(h("x-encore").as_deref(), Some("1"));
        let (_, h, _) = get("/some/deep/link", None).await;
        assert_eq!(h("x-encore").as_deref(), Some("1"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn routes_do_not_clash() {
        // axum panics on overlapping routes when the router is built
        let s = crate::test_support::TestServer::new("routes");
        let _ = s.app();
        s.remove();
    }
}
