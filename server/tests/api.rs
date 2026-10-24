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
