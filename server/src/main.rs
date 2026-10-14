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
mod config;
mod connections;
mod covers;
mod db;
mod dsp;
mod email;
mod dupes;
mod error;
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
mod roon;
mod routes;
mod scheduler;
mod scrobble;
mod slskd_managed;
mod sources;
mod sync;
mod state;
mod tagger;
mod tools;
mod vpn;
mod worker;

use axum::extract::Request;
use axum::http::{header, HeaderValue};
use axum::middleware::{self, Next};
use axum::response::Response;
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
    let cache = if path.starts_with("/api/") { "no-store" } else { "no-cache" };   // app files: cheap 304s, updates show at once
    h.entry(header::CACHE_CONTROL).or_insert(HeaderValue::from_static(cache));
    resp
}

pub fn app(st: AppState) -> Router {
    let static_dir = st.static_dir.clone();
    let spa = ServeDir::new(&static_dir).fallback(ServeFile::new(static_dir.join("index.html")));
    // the Rust/WebAssembly web app (web-rs, built by trunk into static/next) – same routes, same look
    let next = ServeDir::new(static_dir.join("next")).fallback(ServeFile::new(static_dir.join("next").join("index.html")));
    Router::new()
        .nest_service("/next", next)
        .route("/api/health", get(|| async { Json(json!({ "ok": true, "version": crate::VERSION, "server": "rust" })) }))
        .merge(routes::auth::router())
        .merge(routes::admin::router())
        .merge(routes::library::router())
        .merge(routes::discover::router())
        .merge(routes::foryou::router())
        .merge(routes::catalog::router())
        .merge(routes::requests::router())
        .merge(routes::picker::router())
        .merge(routes::soulseek::router())
        .merge(routes::system::router())
        .merge(routes::tagging::router())
        .merge(routes::playlists::router())
        .merge(routes::stats::router())
        .merge(routes::remote::router())
        .merge(routes::roon::router())
        .merge(routes::playback::router())
        .with_state(st.clone())
        // albums and songs in JSON answers gain `cover_v`, so clients can use cacheable cover URLs
        .layer(middleware::from_fn_with_state(st, covers::stamp_layer))
        .fallback_service(spa)
        .layer(middleware::from_fn(security_headers))
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
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app(st).into_make_service_with_connect_info::<SocketAddr>()).await?;
    Ok(())
}

#[cfg(test)]
pub static TEST_ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
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
