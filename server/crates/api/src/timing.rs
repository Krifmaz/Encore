//! How long the API takes: every call to a route is timed up to its answer (the status and headers; a
//! stream's body runs on after that), per route and method. A call that took `SLOW_MS` or more is written to
//! the log as a warning (method, path, status, ms) and kept in a list of the last `SLOW_KEPT` such calls.
//! `GET /api/admin/perf` (routes::perf) shows that list, each route's p50 and p95 over its last
//! `SAMPLES` calls, the caches' sizes and the database's waits for a connection.
use axum::extract::{MatchedPath, Request, State};
use axum::middleware::Next;
use axum::response::Response;
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// A call this slow or slower is logged and kept.
pub const SLOW_MS: f64 = 250.0;
/// Slow calls kept (the newest).
pub const SLOW_KEPT: usize = 500;
/// Times kept per route for its p50 / p95 (the newest).
const SAMPLES: usize = 512;

/// One slow call.
#[derive(Clone, Debug)]
struct Slow {
    at: f64,
    method: String,
    path: String,
    route: String,
    status: u16,
    ms: f64,
}

#[derive(Default)]
struct RouteTimes {
    count: u64,
    slow: u64,
    max_ms: f64,
    last: VecDeque<f32>,
}

#[derive(Default)]
struct Inner {
    slow: VecDeque<Slow>,
    routes: HashMap<String, RouteTimes>,
}

/// The server's request times. Clones share them.
#[derive(Clone)]
pub struct Perf {
    inner: Arc<Mutex<Inner>>,
    since: f64,
}

impl Default for Perf {
    fn default() -> Self {
        Perf { inner: Default::default(), since: encore_db::now() }
    }
}

/// The path as it may be shown: a share link's token or an invite code stays out of the log and the list
/// (the route's pattern stands in for it).
fn shown_path(path: &str, route: &str) -> String {
    if route.contains("{token}") || route.contains("{code}") {
        route.to_string()
    } else {
        path.to_string()
    }
}

/// The value at quantile `q` (0..1) of sorted times: the nearest rank.
fn quantile(sorted: &[f32], q: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = ((q * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len());
    sorted[rank - 1] as f64
}

fn round1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

impl Perf {
    /// Notes one call of `route` (`GET /api/…` pattern) that took `ms`.
    fn record(&self, method: &str, path: &str, route: &str, status: u16, ms: f64) {
        let key = format!("{method} {route}");
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let r = g.routes.entry(key).or_default();
        r.count += 1;
        r.max_ms = r.max_ms.max(ms);
        if r.last.len() >= SAMPLES {
            r.last.pop_front();
        }
        r.last.push_back(ms as f32);
        if ms >= SLOW_MS {
            r.slow += 1;
            if g.slow.len() >= SLOW_KEPT {
                g.slow.pop_front();
            }
            g.slow.push_back(Slow { at: encore_db::now(), method: method.to_string(), path: shown_path(path, route), route: route.to_string(), status, ms });
        }
    }

    /// The slow calls (newest first) and every route's numbers (slowest p95 first).
    pub fn report(&self) -> Value {
        let g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let slow: Vec<Value> = g.slow.iter().rev().map(|s| json!({
            "at": s.at, "method": s.method, "path": s.path, "route": s.route, "status": s.status, "ms": round1(s.ms),
        })).collect();
        let mut routes: Vec<(f64, Value)> = g.routes.iter().map(|(k, r)| {
            let mut sorted: Vec<f32> = r.last.iter().copied().collect();
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let (method, route) = k.split_once(' ').unwrap_or(("", k));
            let p95 = quantile(&sorted, 0.95);
            (p95, json!({
                "method": method, "route": route, "count": r.count, "slow": r.slow,
                "p50_ms": round1(quantile(&sorted, 0.5)), "p95_ms": round1(p95), "max_ms": round1(r.max_ms), "samples": sorted.len(),
            }))
        }).collect();
        routes.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        json!({
            "since": self.since,
            "slow_ms": SLOW_MS,
            "slow": slow,
            "routes": routes.into_iter().map(|r| r.1).collect::<Vec<_>>(),
        })
    }
}

/// The methods the table keeps apart. Any other method (an extension method a client made up) is counted as
/// `OTHER`: axum answers it with the route's own 405, so without this every new method name would add a row
/// to the table, and anyone could do that without signing in.
const METHODS: [&str; 7] = ["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"];

/// The method as the table counts it (see [`METHODS`]).
fn method_key(method: &str) -> &str {
    METHODS.iter().copied().find(|m| *m == method).unwrap_or("OTHER")
}

/// The timing middleware (on the API's routes; the web app's files are not timed).
pub async fn layer(State(perf): State<Perf>, req: Request, next: Next) -> Response {
    let start = Instant::now();
    let method = method_key(req.method().as_str()).to_string();
    let path = req.uri().path().to_string();
    let route = req.extensions().get::<MatchedPath>().map(|m| m.as_str().to_string()).unwrap_or_else(|| path.clone());
    let resp = next.run(req).await;
    let ms = start.elapsed().as_secs_f64() * 1000.0;
    let status = resp.status().as_u16();
    if ms >= SLOW_MS {
        tracing::warn!("slow request: {method} {} {status} {ms:.0} ms", shown_path(&path, &route));
    }
    perf.record(&method, &path, &route, status, ms);
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_get_their_p50_and_p95_and_slow_calls_are_kept() {
        let p = Perf::default();
        for i in 1..=100 {
            p.record("GET", "/api/library/albums", "/api/library/albums", 200, i as f64);
        }
        p.record("GET", "/api/share/abc123", "/api/share/{token}", 200, 400.0);
        p.record("POST", "/api/library/scan", "/api/library/scan", 409, 251.0);
        let r = p.report();
        let albums = r["routes"].as_array().unwrap().iter().find(|x| x["route"] == "/api/library/albums").unwrap().clone();
        assert_eq!((albums["count"].as_u64(), albums["p50_ms"].as_f64(), albums["p95_ms"].as_f64(), albums["max_ms"].as_f64()), (Some(100), Some(50.0), Some(95.0), Some(100.0)));
        assert_eq!(albums["slow"], 0);
        // slowest p95 first
        assert_eq!(r["routes"][0]["route"], "/api/share/{token}");
        // newest slow call first; a share token is not shown
        let slow = r["slow"].as_array().unwrap();
        assert_eq!(slow.len(), 2);
        assert_eq!((slow[0]["method"].as_str(), slow[0]["path"].as_str(), slow[0]["status"].as_u64()), (Some("POST"), Some("/api/library/scan"), Some(409)));
        assert_eq!(slow[1]["path"], "/api/share/{token}");
        // only the newest SLOW_KEPT are kept, and only the newest SAMPLES times per route
        for _ in 0..SLOW_KEPT + 10 {
            p.record("GET", "/api/x", "/api/x", 200, 300.0);
        }
        for _ in 0..SAMPLES {
            p.record("GET", "/api/library/albums", "/api/library/albums", 200, 1.0);
        }
        let r = p.report();
        assert_eq!(r["slow"].as_array().unwrap().len(), SLOW_KEPT);
        let albums = r["routes"].as_array().unwrap().iter().find(|x| x["route"] == "/api/library/albums").unwrap().clone();
        assert_eq!((albums["samples"].as_u64(), albums["p95_ms"].as_f64(), albums["count"].as_u64()), (Some(SAMPLES as u64), Some(1.0), Some(100 + SAMPLES as u64)));
    }

    /// Made-up methods share one row: the table can't be grown by sending new method names.
    #[tokio::test]
    async fn unknown_methods_share_one_row() {
        use axum::body::Body;
        use axum::routing::get;
        use tower::ServiceExt;
        let perf = Perf::default();
        let app = axum::Router::new().route("/api/health", get(|| async { "ok" })).layer(axum::middleware::from_fn_with_state(perf.clone(), layer));
        for m in ["GET", "M1", "M2", "M3", "POST", "DELETE"] {
            let req = axum::http::Request::builder().method(m).uri("/api/health").body(Body::empty()).unwrap();
            let _ = app.clone().oneshot(req).await.unwrap();
        }
        let r = perf.report();
        let mut rows: Vec<String> = r["routes"].as_array().unwrap().iter().map(|x| format!("{} {}", x["method"].as_str().unwrap(), x["route"].as_str().unwrap())).collect();
        rows.sort();
        assert_eq!(rows, ["DELETE /api/health", "GET /api/health", "OTHER /api/health", "POST /api/health"]);
        let other = r["routes"].as_array().unwrap().iter().find(|x| x["method"] == "OTHER").unwrap().clone();
        assert_eq!(other["count"], 3);
    }

    #[test]
    fn quantiles_take_the_nearest_rank() {
        assert_eq!(quantile(&[], 0.5), 0.0);
        assert_eq!(quantile(&[7.0], 0.95), 7.0);
        assert_eq!(quantile(&[1.0, 2.0, 3.0, 4.0], 0.5), 2.0);
        assert_eq!(quantile(&[1.0, 2.0, 3.0, 4.0], 0.95), 4.0);
    }
}
