//! GET /api/admin/perf: how the server is doing since it started – the last slow requests, each route's p50
//! and p95, the sizes of the caches, the database's waits for a connection and the age of the answers kept
//! by the background refreshers (see crate::timing).
use crate::auth::Admin;
use crate::state::AppState;
use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{json, Value};

pub fn router() -> Router<AppState> {
    Router::new().route("/api/admin/perf", get(perf))
}

async fn perf(State(st): State<AppState>, Admin(_): Admin) -> Json<Value> {
    let mut v = st.web.perf.report();
    let age = |a: Option<std::time::Duration>| a.map(|d| (d.as_secs_f64() * 10.0).round() / 10.0);
    let (done, total, running) = st.library.covers.progress();
    v["caches"] = json!({
        "lists": st.web.lists.stats(),
        "remote": st.cache.stats(),
        "musicbrainz": st.mb.cache_stats(),
        // MusicBrainz look-ups kept on disk across restarts (null: memory only)
        "musicbrainz_disk_answers": st.mb.stored(),
        "soulseek_browse": st.web.soulseek_browsed.stats(),
    });
    v["db"] = st.db.pool_waits();
    v["background"] = json!({
        "storage_age_s": age(st.web.storage.age()),
        "slskd_status_age_s": age(st.web.slskd_probe.age()),
        "tool_lines": st.system.tools.kept(),
        "cover_pass": { "done": done, "total": total, "running": running },
        // who waits for MusicBrainz at each priority (a page, an import, a request's lookup, background work)
        "musicbrainz_line": st.mb.line_stats(),
    });
    Json(v)
}

#[cfg(test)]
mod tests {
    use crate::test_support::TestServer;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    /// Calls to the API are timed per route; the page is for admins.
    #[tokio::test]
    async fn the_perf_page_counts_the_routes_for_admins() {
        let s = TestServer::new("perf");
        let app = s.app();
        let call = |path: &str, cookie: Option<&str>| {
            let mut r = Request::builder().uri(path);
            if let Some(c) = cookie {
                r = r.header("cookie", c);
            }
            app.clone().oneshot(r.body(Body::empty()).unwrap())
        };
        assert_eq!(call("/api/health", None).await.unwrap().status(), StatusCode::OK);
        assert_eq!(call("/api/admin/perf", None).await.unwrap().status(), StatusCode::UNAUTHORIZED);
        // the owner made at the first start, with the password it was given
        let first = std::fs::read_to_string(s.dir.join("initial_admin_password.txt")).unwrap();
        let (user, pass) = (first.lines().next().unwrap(), first.lines().nth(1).unwrap());
        let login = Request::builder().method("POST").uri("/api/auth/login").header("x-requested-with", "encore").header("content-type", "application/json")
            .body(Body::from(serde_json::json!({ "username": user, "password": pass }).to_string())).unwrap();
        let resp = app.clone().oneshot(login).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let cookie = resp.headers().get("set-cookie").and_then(|v| v.to_str().ok()).unwrap().split(';').next().unwrap().to_string();
        let resp = call("/api/admin/perf", Some(&cookie)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let v: serde_json::Value = serde_json::from_slice(&axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap()).unwrap();
        let health = v["routes"].as_array().unwrap().iter().find(|r| r["route"] == "/api/health").expect("the health call was timed").clone();
        assert_eq!((health["method"].as_str(), health["count"].as_u64()), (Some("GET"), Some(1)));
        assert!(health["p95_ms"].as_f64().is_some());
        assert_eq!(v["slow_ms"], 250.0);
        for k in ["lists", "remote", "musicbrainz", "soulseek_browse"] {
            assert!(v["caches"][k]["max_bytes"].as_u64().unwrap_or(0) > 0, "{k}: {v}");
        }
        assert!(v["db"]["write"]["waits"].is_u64(), "{v}");
        assert!(v["background"]["cover_pass"].is_object());
        for k in ["interactive", "section", "import", "request", "background"] {
            assert_eq!(v["background"]["musicbrainz_line"][k]["waiting"], 0, "{v}");
        }
        // the web app's files are not timed
        let _ = call("/index.html", None).await.unwrap();
        let v = s.st.web.perf.report();
        assert!(v["routes"].as_array().unwrap().iter().all(|r| r["route"].as_str().unwrap_or("").starts_with("/api/")), "{v}");
        s.remove();
    }
}
