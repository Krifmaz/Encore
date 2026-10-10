//! Encore server.
mod auth;
mod config;
mod db;
mod email;
mod error;
mod routes;
mod state;

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
use tower_http::compression::CompressionLayer;
use tower_http::services::{ServeDir, ServeFile};

async fn security_headers(req: Request, next: Next) -> Response {
    let path = req.uri().path().to_string();
    let mut resp = next.run(req).await;
    let h = resp.headers_mut();
    h.entry("x-content-type-options").or_insert(HeaderValue::from_static("nosniff"));
    h.entry("x-frame-options").or_insert(HeaderValue::from_static("DENY"));
    h.entry("referrer-policy").or_insert(HeaderValue::from_static("same-origin"));
    let cache = if path.starts_with("/api/") { "no-store" } else { "no-cache" };   // app files: cheap 304s, updates show at once
    h.entry(header::CACHE_CONTROL).or_insert(HeaderValue::from_static(cache));
    resp
}

pub fn app(st: AppState) -> Router {
    let static_dir = st.static_dir.clone();
    let spa = ServeDir::new(&static_dir).fallback(ServeFile::new(static_dir.join("index.html")));
    Router::new()
        .route("/api/health", get(|| async { Json(json!({ "ok": true, "version": env!("CARGO_PKG_VERSION"), "server": "rust" })) }))
        .merge(routes::auth::router())
        .merge(routes::admin::router())
        .with_state(st)
        .fallback_service(spa)
        .layer(middleware::from_fn(security_headers))
        .layer(CompressionLayer::new())
}

pub fn build_state() -> anyhow::Result<AppState> {
    let data = PathBuf::from(std::env::var("ENCORE_CONFIG_DIR").unwrap_or_else(|_| "/config".into()));
    let db = db::Db::open(&data)?;
    let cfg = config::Config::load(db.clone())?;
    let static_dir = PathBuf::from(std::env::var("ENCORE_STATIC_DIR").unwrap_or_else(|_| "/app/static".into()));
    let http = reqwest::Client::builder()
        .user_agent(format!("Encore/{} ( {} )", env!("CARGO_PKG_VERSION"), cfg.str("contact_email")))
        .timeout(std::time::Duration::from_secs(30))
        .build()?;
    let st = AppState { db, cfg, http, limits: Arc::new(auth::RateLimiter::default()), static_dir };
    auth::ensure_owner(&st)?;
    Ok(st)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into())).init();
    let st = build_state()?;
    let port: u16 = std::env::var("ENCORE_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(8080);
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    tracing::info!("Encore {} listening on {addr}", env!("CARGO_PKG_VERSION"));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app(st).into_make_service_with_connect_info::<SocketAddr>()).await?;
    Ok(())
}
