//! The update and restart routes through the router: admins only, the CSRF header, the kept GitHub answer and
//! its ETag, the diagnostics' machine block and the scheduled task.
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tower::ServiceExt;

const PW: &str = "update-pass-12";

async fn call(app: &axum::Router, method: &str, uri: &str, cookie: &str, csrf: bool, body: Option<Value>) -> (StatusCode, Value, String) {
    let mut b = Request::builder().method(method).uri(uri);
    if csrf {
        b = b.header("x-requested-with", "encore");
    }
    if !cookie.is_empty() {
        b = b.header("cookie", cookie);
    }
    let req = match body {
        Some(j) => b.header("content-type", "application/json").body(Body::from(j.to_string())).unwrap(),
        None => b.body(Body::empty()).unwrap(),
    };
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let set_cookie = resp.headers().get("set-cookie").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let bytes = axum::body::to_bytes(resp.into_body(), 10 << 20).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null), set_cookie)
}

async fn login(app: &axum::Router, name: &str) -> String {
    let r = call(app, "POST", "/api/auth/login", "", true, Some(json!({ "username": name, "password": PW }))).await;
    assert_eq!(r.0, StatusCode::OK, "{:?}", r.1);
    r.2.split(';').next().unwrap().to_string()
}

#[derive(Clone, Default)]
struct Hits {
    full: Arc<AtomicUsize>,
    conditional: Arc<AtomicUsize>,
}

/// GitHub's releases API as far as the check uses it: the release list (one release) with an ETag, 304 for a repeat.
async fn github_latest(axum::extract::State(h): axum::extract::State<Hits>, headers: axum::http::HeaderMap) -> axum::response::Response {
    use axum::response::IntoResponse;
    if headers.get("if-none-match").and_then(|v| v.to_str().ok()) == Some("\"rel-1\"") {
        h.conditional.fetch_add(1, Ordering::SeqCst);
        return (StatusCode::NOT_MODIFIED, [("etag", "\"rel-1\"")]).into_response();
    }
    h.full.fetch_add(1, Ordering::SeqCst);
    let body = json!([{ "tag_name": "v99.0.0", "name": "Encore 99.0.0", "html_url": "https://github.com/Krifmaz/Encore/releases/tag/v99.0.0",
                       "published_at": "2026-11-02T10:00:00Z", "body": format!("## What changed\n\n{}", "x".repeat(6000)) }]);
    (StatusCode::OK, [("etag", "\"rel-1\""), ("content-type", "application/json")], body.to_string()).into_response()
}

/// GET /api/system/update for admins only, from GitHub's answer, kept for an hour (force asks again, through
/// the ETag); self-update unavailable without a socket, with the reason; the diagnostics' machine block and
/// the scheduled task see the kept answer; POST update refuses without a socket; restart needs the CSRF header.
#[tokio::test]
async fn the_update_endpoints() {
    let hits = Hits::default();
    let gh = axum::Router::new().route("/repos/Krifmaz/Encore/releases", axum::routing::get(github_latest)).with_state(hits.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, gh).await.unwrap() });
    let dir = std::env::temp_dir().join(format!("encore-update-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut st = crate::test_support::state_in(&dir);
    // GitHub and the Docker socket of this server only: a stand-in, and a socket that is not there
    st.system.github_api = base;
    st.system.docker_socket = dir.join("no-docker.sock");
    crate::auth::create_user(&st, "boss", PW, "admin", None, None).unwrap();
    crate::auth::create_user(&st, "listener", PW, "user", None, None).unwrap();
    let app = crate::app(st.clone());
    let admin = login(&app, "boss").await;
    let user = login(&app, "listener").await;
    let n = |h: &Arc<AtomicUsize>| h.load(Ordering::SeqCst);

    // nothing kept yet: the diagnostics block says no update
    assert_eq!(crate::routes::system::server_block(&st)["encore_update_available"], json!(false));
    let (s, _, _) = call(&app, "GET", "/api/system/update", &user, false, None).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "admins only");
    let (s, v, _) = call(&app, "GET", "/api/system/update", &admin, false, None).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["current"], json!(crate::VERSION));
    assert_eq!((v["latest"].as_str(), v["is_newer"].as_bool()), (Some("99.0.0"), Some(true)));
    assert_eq!(v["notes"].as_str().unwrap().chars().count(), 4000);
    assert_eq!(v["url"], json!("https://github.com/Krifmaz/Encore/releases/tag/v99.0.0"));
    assert_eq!(v["published_at"], json!("2026-11-02T10:00:00Z"));
    assert!(v["checked_at"].as_i64().unwrap() > 1_700_000_000);
    assert_eq!((v["restart"].as_str(), v["self_update"].as_str()), (Some("ready"), Some("unavailable")));
    assert_eq!(v["self_update_reason"], json!("The Docker socket is not mounted"));
    assert_eq!(v["image"], json!("ghcr.io/krifmaz/encore:99.0.0"));
    assert_eq!(v["error"], Value::Null);
    assert_eq!((n(&hits.full), n(&hits.conditional)), (1, 0));
    // within the hour: the kept answer, no call
    let (_, v, _) = call(&app, "GET", "/api/system/update", &admin, false, None).await;
    assert_eq!(v["latest"], json!("99.0.0"));
    assert_eq!((n(&hits.full), n(&hits.conditional)), (1, 0));
    // Check now: asks again, GitHub says nothing changed
    let (_, v, _) = call(&app, "GET", "/api/system/update?force=1", &admin, false, None).await;
    assert_eq!(v["latest"], json!("99.0.0"));
    assert_eq!((n(&hits.full), n(&hits.conditional)), (1, 1));
    // the Settings banner reads the kept answer from the diagnostics' machine block
    let m = crate::routes::system::server_block(&st);
    assert_eq!((m["encore_update_available"].clone(), m["encore_latest"].clone()), (json!(true), json!("99.0.0")));
    // checks switched off: an old answer is shown without asking GitHub, Check now still asks
    st.db.execute("UPDATE sync_state SET value=json_set(value, '$.checked_at', 1) WHERE scope='encore:update'", encore_db::p![]).unwrap();
    st.cfg.set_many(&json!({ "check_updates": false }));
    let (_, v, _) = call(&app, "GET", "/api/system/update", &admin, false, None).await;
    assert_eq!((v["latest"].as_str(), v["check_updates"].as_bool(), v["checked_at"].as_i64()), (Some("99.0.0"), Some(false), Some(1)));
    assert_eq!((n(&hits.full), n(&hits.conditional)), (1, 1));
    let (_, v, _) = call(&app, "GET", "/api/system/update?force=1", &admin, false, None).await;
    assert!(v["checked_at"].as_i64().unwrap() > 1);
    assert_eq!((n(&hits.full), n(&hits.conditional)), (1, 2));
    // the scheduled task: once a day, on with check_updates
    let task = crate::scheduler::listing(&st).into_iter().find(|t| t["key"] == "update").unwrap();
    assert_eq!((task["enabled"].as_bool(), task["minutes"].as_i64()), (Some(false), Some(1440)));
    st.cfg.set_many(&json!({ "check_updates": true }));
    assert_eq!(crate::scheduler::listing(&st).into_iter().find(|t| t["key"] == "update").unwrap()["enabled"], json!(true));
    assert_eq!(encore_system::update::scheduled(&st).await.unwrap(), "Encore 99.0.0 is available");

    // Update now: no socket here
    let (s, v, _) = call(&app, "POST", "/api/system/update", &admin, true, Some(json!({ "tag": "0.29.0" }))).await;
    assert_eq!((s, v["detail"].as_str()), (StatusCode::CONFLICT, Some("The Docker socket is not mounted")));
    let (s, _, _) = call(&app, "POST", "/api/system/update", &user, true, None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _, _) = call(&app, "POST", "/api/system/update", &admin, false, None).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "CSRF header");

    // Restart: admins, with the CSRF header; the answer comes before the restart
    let (s, _, _) = call(&app, "POST", "/api/system/restart", &admin, false, None).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "CSRF header");
    let (s, _, _) = call(&app, "POST", "/api/system/restart", &user, true, None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert!(!st.system.restart.is_requested());
    let (s, v, _) = call(&app, "POST", "/api/system/restart", &admin, true, None).await;
    assert_eq!((s, v), (StatusCode::ACCEPTED, json!({ "ok": true, "message": "Restarting" })));
    tokio::time::timeout(Duration::from_secs(3), st.system.restart.requested()).await.expect("the restart is asked for after half a second");
    let (s, _, _) = call(&app, "POST", "/api/system/restart", &admin, true, None).await;
    assert_eq!(s, StatusCode::ACCEPTED, "asking twice is fine");

    let _ = std::fs::remove_dir_all(&dir);
}
