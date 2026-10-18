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
mod slskd_managed;
mod sources;
mod sync;
mod state;
mod tagger;
mod tools;
mod translate;
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
/// next to index.html at the root), so it never changes under that name.
fn hashed_asset(path: &str) -> bool {
    let Some(name) = path.strip_prefix('/') else { return false };
    let stem = name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name);
    let stem = stem.strip_suffix("_bg").unwrap_or(stem);
    !name.contains('/') && stem.rsplit_once('-').is_some_and(|(_, h)| h.len() >= 16 && h.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// `ENCORE_LEGACY_UI=1`: serve the frozen JavaScript UI (web/ of 0.25, without this release's interface fixes)
/// at `/` instead of the Rust UI, for this one release.
fn legacy_ui() -> bool {
    matches!(std::env::var("ENCORE_LEGACY_UI").as_deref(), Ok("1") | Ok("true") | Ok("yes"))
}

/// Where the JavaScript UI lives when `ENCORE_LEGACY_UI=1`: `ENCORE_LEGACY_STATIC_DIR`, or the Rust UI's folder
/// with `-legacy` on its name (`/app/static` → `/app/static-legacy`, as the image lays them out).
pub fn legacy_dir(static_dir: &std::path::Path) -> PathBuf {
    if let Ok(d) = std::env::var("ENCORE_LEGACY_STATIC_DIR") {
        return PathBuf::from(d);
    }
    let mut name = static_dir.file_name().map(|n| n.to_os_string()).unwrap_or_else(|| "static".into());
    name.push("-legacy");
    static_dir.with_file_name(name)
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
    // the web app (web-rs, built by trunk into static/): one folder, index.html for every path that is not a file
    let spa = ServeDir::new(&static_dir).fallback(ServeFile::new(static_dir.join("index.html")));
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
    let router = if legacy_ui() {
        let legacy = legacy_dir(&static_dir);
        tracing::warn!("ENCORE_LEGACY_UI=1: serving the JavaScript UI from {} – it has none of this release's interface fixes", legacy.display());
        // the files both UIs share (css, icons, manifest, share.html, sw.js) live in the Rust UI's folder only
        let shared = ServeDir::new(&static_dir).fallback(ServeFile::new(legacy.join("index.html")));
        router.fallback_service(ServeDir::new(&legacy).fallback(shared))
    } else {
        router.fallback_service(spa)
    };
    router
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
    axum::serve(listener, app(st).into_make_service_with_connect_info::<SocketAddr>()).await?;
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
        for plain in ["/", "/index.html", "/sw.js", "/share.html", "/manifest.webmanifest", "/favicon.svg", "/css/app.css", "/icons/apple-touch-icon.png", "/js/app.js"] {
            assert!(!super::hashed_asset(plain), "{plain}");
        }
        // only the root level: trunk puts the hashed files next to index.html, and /next/ is a redirect now
        assert!(!super::hashed_asset("/next/encore-web-f92ab5b84b305a59.js"));
        assert!(!super::hashed_asset("/a/encore-web-f92ab5b84b305a59.js"));
    }

    #[test]
    fn the_legacy_folder_sits_next_to_the_static_one() {
        let _env = super::TEST_ENV.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("ENCORE_LEGACY_STATIC_DIR");
        assert_eq!(super::legacy_dir(std::path::Path::new("/app/static")), std::path::PathBuf::from("/app/static-legacy"));
        assert_eq!(super::legacy_dir(std::path::Path::new("./static")), std::path::PathBuf::from("./static-legacy"));
        std::env::set_var("ENCORE_LEGACY_STATIC_DIR", "/srv/old-ui");
        assert_eq!(super::legacy_dir(std::path::Path::new("/app/static")), std::path::PathBuf::from("/srv/old-ui"));
        std::env::remove_var("ENCORE_LEGACY_STATIC_DIR");
    }

    /// The Rust UI's trunk output served at `/`: index.html for the root and for unknown paths, the hashed files
    /// cached for good, `/next/…` redirected to `/…`, and with ENCORE_LEGACY_UI=1 the JavaScript UI's folder at `/`
    /// with the shared files still taken from the Rust UI's folder.
    #[tokio::test]
    async fn the_rust_ui_is_the_website_and_the_legacy_switch_brings_the_old_one_back() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use tower::ServiceExt;
        let dir = std::env::temp_dir().join(format!("encore-ui-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let dist = dir.join("static");
        let legacy = dir.join("static-legacy");
        std::fs::create_dir_all(dist.join("css")).unwrap();
        std::fs::create_dir_all(legacy.join("js")).unwrap();
        std::fs::write(dist.join("index.html"), "<meta name=\"application-name\" content=\"Encore\">rust ui").unwrap();
        std::fs::write(dist.join("encore-web-f92ab5b84b305a59.js"), "// glue").unwrap();
        std::fs::write(dist.join("css/app.css"), "body{}").unwrap();
        std::fs::write(dist.join("sw.js"), "// sw").unwrap();
        std::fs::write(legacy.join("index.html"), "legacy ui").unwrap();
        std::fs::write(legacy.join("js/app.js"), "// app").unwrap();
        let (rust, old) = {
            let _env = super::TEST_ENV.lock().unwrap_or_else(|e| e.into_inner());
            std::env::set_var("ENCORE_CONFIG_DIR", dir.join("config"));
            std::env::remove_var("ENCORE_LEGACY_UI");
            let mut st = super::build_state().unwrap();
            st.static_dir = dist.clone();
            let rust = super::app(st.clone());
            std::env::set_var("ENCORE_LEGACY_UI", "1");
            let old = super::app(st);
            std::env::remove_var("ENCORE_LEGACY_UI");
            (rust, old)
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
        // the switch: the JavaScript UI at /, its own files from its folder, the shared ones from the Rust UI's
        let (s, _, b) = get(&old, "/").await;
        assert_eq!((s, b.as_str()), (StatusCode::OK, "legacy ui"));
        let (s, _, b) = get(&old, "/js/app.js").await;
        assert_eq!((s, b.as_str()), (StatusCode::OK, "// app"));
        let (s, _, b) = get(&old, "/css/app.css").await;
        assert_eq!((s, b.as_str()), (StatusCode::OK, "body{}"));
        let (s, _, b) = get(&old, "/sw.js").await;
        assert_eq!((s, b.as_str()), (StatusCode::OK, "// sw"));
        let (s, _, b) = get(&old, "/#/library".split('#').next().unwrap()).await;
        assert_eq!((s, b.as_str()), (StatusCode::OK, "legacy ui"));
        let (s, _, b) = get(&old, "/not/a/file").await;
        assert_eq!((s, b.as_str()), (StatusCode::OK, "legacy ui"));
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
