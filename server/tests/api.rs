//! The server through its router, as the apps reach it: a TestServer (crates/api, feature
//! `test-support`) on a config folder of its own, no network and no process environment.
use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use encore_api::test_support::TestServer;
use serde_json::{json, Value};
use tower::ServiceExt;

async fn call(app: &Router, req: Request<Body>) -> (StatusCode, Value) {
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let body = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

async fn get(app: &Router, path: &str) -> (StatusCode, Value) {
    call(app, Request::builder().uri(path).body(Body::empty()).unwrap()).await
}

/// Health and the sign-in page's facts answer without a session; the library does not.
#[tokio::test]
async fn health_and_sign_in_facts_answer_without_a_session() {
    let s = TestServer::new("it-health");
    let app = s.app();
    let (code, v) = get(&app, "/api/health").await;
    assert_eq!(code, StatusCode::OK);
    // `web` names the web build served (null here: the test server has no static folder)
    assert_eq!(v, json!({ "ok": true, "server": "rust", "version": encore_api::VERSION, "web": null }));
    let (code, v) = get(&app, "/api/auth/info").await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!((v["server"].as_str(), v["api"].as_i64()), (Some("rust"), Some(2)));
    let (code, _) = get(&app, "/api/library/albums").await;
    assert_eq!(code, StatusCode::UNAUTHORIZED);
    s.remove();
}

/// An admin signs in and sees an empty library and no requests.
#[tokio::test]
async fn an_admin_signs_in_and_sees_an_empty_library() {
    let s = TestServer::new("it-owner");
    encore_api::auth::create_user(&s.st, "tester", "a-long-password-1", "admin", None, None).unwrap();
    let app = s.app();
    let login = Request::builder().method("POST").uri("/api/auth/login").header("x-requested-with", "encore").header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({ "username": "tester", "password": "a-long-password-1" }).to_string())).unwrap();
    let resp = app.clone().oneshot(login).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let cookie = resp.headers().get(header::SET_COOKIE).and_then(|v| v.to_str().ok()).unwrap_or("").split(';').next().unwrap().to_string();
    let (code, v) = call(&app, Request::builder().uri("/api/library/albums").header(header::COOKIE, &cookie).body(Body::empty()).unwrap()).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(v["items"], json!([]), "{v}");
    let (code, v) = call(&app, Request::builder().uri("/api/requests").header(header::COOKIE, &cookie).body(Body::empty()).unwrap()).await;
    assert_eq!(code, StatusCode::OK, "{v}");
    s.remove();
}

/// The apps' two addresses: `/api/auth/addresses` needs a session and answers Public URL and Local network address
/// without their trailing `/`, with the same `server_id` as `/api/auth/info` (which the apps compare before switching).
#[tokio::test]
async fn addresses_answer_signed_in_users_with_the_server_id() {
    let s = TestServer::new("it-addresses");
    encore_api::auth::create_user(&s.st, "tester", "a-long-password-1", "user", None, None).unwrap();
    s.st.cfg.set_many(&json!({ "base_url": "https://music.example.com/", "local_url": " http://10.0.1.7:8080/ " }));
    let app = s.app();
    // nobody else learns the home network's address
    let (code, _) = get(&app, "/api/auth/addresses").await;
    assert_eq!(code, StatusCode::UNAUTHORIZED);
    let login = Request::builder().method("POST").uri("/api/auth/login").header("x-requested-with", "encore").header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({ "username": "tester", "password": "a-long-password-1" }).to_string())).unwrap();
    let resp = app.clone().oneshot(login).await.unwrap();
    let cookie = resp.headers().get(header::SET_COOKIE).and_then(|v| v.to_str().ok()).unwrap_or("").split(';').next().unwrap().to_string();
    let (code, v) = call(&app, Request::builder().uri("/api/auth/addresses").header(header::COOKIE, &cookie).body(Body::empty()).unwrap()).await;
    assert_eq!(code, StatusCode::OK, "{v}");
    assert_eq!((v["public"].as_str(), v["local"].as_str()), (Some("https://music.example.com"), Some("http://10.0.1.7:8080")));
    let (_, info) = get(&app, "/api/auth/info").await;
    let id = info["server_id"].as_str().unwrap_or("");
    assert_eq!(id.len(), 32, "made at the first start: {info}");
    assert_eq!(v["server_id"].as_str(), Some(id));
    assert!(info["features"].as_array().unwrap().contains(&json!("addresses")));
    s.remove();
}
