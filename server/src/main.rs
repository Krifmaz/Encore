//! Encore server.
/// Release version (e.g. 2026.10.11.1), set by the release build; local builds use Cargo's.
pub const VERSION: &str = match option_env!("ENCORE_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

mod acoustid;
mod artists;
mod auth;
mod backup;
mod busy;
mod config;
mod connections;
mod contribute;
mod covers;
mod db;
mod docker;
mod dsp;
mod email;
mod dupes;
mod error;
mod fsinfo;
mod jobs;
mod library;
mod lyrics;
mod mb;
mod meta;
mod matcher;
mod nlsearch;
mod notify;
mod organize;
mod playlists;
mod quality;
mod release_types;
mod req_events;
mod roon;
mod routes;
mod scheduler;
mod scrobble;
mod search;
mod selfupdate;
mod slskd_managed;
mod sources;
mod sync;
mod state;
mod tagger;
mod tools;
mod translate;
mod update;
mod vpn;
mod worker;
mod ytauth;

use axum::extract::Request;
use axum::http::{header, HeaderValue};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::response::Redirect;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::json;
use state::AppState;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tower_http::compression::predicate::{DefaultPredicate, NotForContentType, Predicate};
use tower_http::compression::CompressionLayer;
use tower_http::services::{ServeDir, ServeFile};

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

pub fn app(st: AppState) -> Router {
    let static_dir = st.static_dir.clone();
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
        .route("/api/health", get(|| async { Json(json!({ "ok": true, "version": crate::VERSION, "server": "rust" })) }))
        .merge(routes::auth::router())
        .merge(routes::admin::router())
        .merge(routes::library::router())
        .merge(routes::discover::router())
        .merge(routes::foryou::router())
        .merge(routes::catalog::router())
        .merge(routes::requests::router())
        .merge(routes::report::router())
        .merge(routes::picker::router())
        .merge(routes::soulseek::router())
        .merge(routes::system::router())
        .merge(routes::tagging::router())
        .merge(routes::playlists::router())
        .merge(routes::stats::router())
        .merge(routes::bugreports::router())
        .merge(routes::remote::router())
        .merge(routes::roon::router())
        .merge(routes::playback::router())
        .merge(contribute::router())
        .merge(translate::router())
        .merge(ytauth::router())
        .with_state(st.clone())
        // albums and songs in JSON answers gain `cover_v`, so clients can use cacheable cover URLs
        .layer(middleware::from_fn_with_state(st.clone(), covers::stamp_layer))
        // titles, albums and artists gain `_t` / `_r` (translation, romanization) for listeners who asked
        .layer(middleware::from_fn_with_state(st, translate::stamp_layer));
    router
        .fallback_service(spa)
        .layer(middleware::from_fn(security_headers))
        // API calls hold background passes (cover thumbnails …) to one low-priority thread
        .layer(middleware::from_fn(busy::layer))
        // gzip for JSON and pages – never for audio: it only burns CPU and gets in the way of range requests
        .layer(CompressionLayer::new().compress_when(DefaultPredicate::new().and(NotForContentType::new("audio/"))))
}

pub fn build_state() -> anyhow::Result<AppState> {
    let data = PathBuf::from(std::env::var("ENCORE_CONFIG_DIR").unwrap_or_else(|_| "/config".into()));
    let db = db::Db::open(&data)?;
    let cfg = config::Config::load(db.clone())?;
    let static_dir = PathBuf::from(std::env::var("ENCORE_STATIC_DIR").unwrap_or_else(|_| "/app/static".into()));
    let http = reqwest::Client::builder()
        // MetaBrainz (ListenBrainz, MusicBrainz) turn away clients without a way to contact them
        .user_agent({
            let contact = cfg.str("contact_email");
            format!("Encore/{} ( {} )", crate::VERSION, if contact.trim().is_empty() { "https://github.com/Krifmaz/Encore".to_string() } else { contact })
        })
        .timeout(std::time::Duration::from_secs(30))
        .build()?;
    let st = AppState { db, cfg, http, limits: Arc::new(auth::RateLimiter::default()), static_dir, library: library::Library::default(), mb: Default::default(), cache: Default::default(), tasks: Default::default() };
    auth::ensure_owner(&st)?;
    Ok(st)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into())).init();
    // `encore self-update-helper …`: the container swap of "Update now" (no database, no web server)
    if let Some(code) = selfupdate::main_if_helper().await {
        std::process::exit(code);
    }
    let st = match build_state() {
        Ok(st) => st,
        Err(e) => {
            if let Some(n) = e.downcast_ref::<db::NewerDatabase>() {
                // an older image was started against a newer database: say so plainly and leave it alone
                tracing::error!("{n}");
                eprintln!("\n{n}\n");
                std::process::exit(db::EXIT_NEWER_DB);
            }
            return Err(e);
        }
    };
    // a copy of the database at least once a day, even when the server is restarted all the time
    if backup::newest_copy(st.data_dir()).map(|b| db::now() - b.time > 86400.0).unwrap_or(true) {
        match st.db.backup(backup::Kind::Scheduled) {
            Ok(b) => { st.db.log("info", &format!("Database backed up to backups/{}", b.name)); backup::prune(st.data_dir()); }
            Err(e) => tracing::warn!("startup backup failed: {e}"),
        }
    }
    let port: u16 = std::env::var("ENCORE_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(8080);
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    tracing::info!("Encore {} listening on {addr}", crate::VERSION);
    // cover thumbnails and colours for albums that have none yet (a new install, a cleared cache, an
    // update that added sizes): at once, not only after the scan below
    st.library.warm_covers(st.db.clone());
    // library scan at start; later scans (and every other background job) come from the scheduler
    {
        let st = st.clone();
        tokio::spawn(async move { st.library.scan(st.db.clone(), st.cfg.str("library_path"), false).await });
    }
    tokio::spawn(worker::run(st.clone()));
    tokio::spawn(artists::alias_worker(st.clone()));
    tokio::spawn(vpn::supervisor(st.clone()));
    tokio::spawn(slskd_managed::supervisor(st.clone()));
    tokio::spawn(roon::supervisor(st.clone()));
    tokio::spawn(contribute::worker(st.clone()));
    tokio::spawn(ytauth::supervisor(st.clone()));
    tokio::spawn(translate::worker(st.clone()));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    // until "Restart server": then a graceful stop and exit code 75 (update.rs)
    update::serve(listener, app(st.clone()), st).await?;
    Ok(())
}

#[cfg(test)]
mod perf_bench;

#[cfg(test)]
pub static TEST_ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
            let _env = super::TEST_ENV.lock().unwrap_or_else(|e| e.into_inner());
            std::env::set_var("ENCORE_CONFIG_DIR", dir.join("config"));
            let mut st = super::build_state().unwrap();
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
            let _env = super::TEST_ENV.lock().unwrap_or_else(|e| e.into_inner());
            std::env::set_var("ENCORE_CONFIG_DIR", dir.join("config"));
            let mut st = super::build_state().unwrap();
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
        let _env = super::TEST_ENV.lock().unwrap_or_else(|e| e.into_inner());
        // axum panics on overlapping routes when the router is built
        let dir = std::env::temp_dir().join(format!("encore-routes-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("ENCORE_CONFIG_DIR", &dir);
        let st = super::build_state().unwrap();
        let _ = super::app(st);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
