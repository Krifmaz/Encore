//! /api/system/wrapper through the router: admins only; the card's actions want the plugin installed; with no
//! Docker socket the card and the plugin's status say how to mount it; an upload that is not an Android app is
//! refused, an APK is kept; the external mode's sign-in reaches its wrapper's address (none here: a network
//! error, never a token or a password in the answer).
use crate::test_support::{call, send, user, TestServer};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use std::io::Write;
use tower::ServiceExt;

/// A multipart upload of one file to the app route.
async fn upload(app: &axum::Router, cookie: &str, name: &str, bytes: Vec<u8>) -> (StatusCode, Value) {
    let b = "encoreWrapperBoundary7";
    let mut body = Vec::new();
    body.extend(format!("--{b}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\nContent-Type: application/octet-stream\r\n\r\n").as_bytes());
    body.extend(bytes);
    body.extend(format!("\r\n--{b}--\r\n").as_bytes());
    let req = Request::builder().method("POST").uri("/api/system/wrapper/app").header("x-requested-with", "encore").header("cookie", cookie)
        .header("content-type", format!("multipart/form-data; boundary={b}")).body(Body::from(body)).unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

/// A zip with one member.
fn zip_with(name: &str, bytes: &[u8]) -> Vec<u8> {
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut z = zip::ZipWriter::new(&mut buf);
        z.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
        z.write_all(bytes).unwrap();
        z.finish().unwrap();
    }
    buf.into_inner()
}

#[tokio::test]
async fn the_wrapper_card_without_docker() {
    let ts = TestServer::new("wrapper-card");
    // this server's Docker socket: one that is not there
    let sock = ts.dir.join("no-docker.sock");
    let ts = ts.with_docker_socket(sock);
    user(&ts.st, "boss-wrapper", "admin");
    user(&ts.st, "plain-wrapper", "user");
    let app = ts.app();
    let login = |name: &'static str| {
        let app = app.clone();
        async move {
            let (code, _, cookie) = call(&app, "POST", "/api/auth/login", "", Some(json!({ "username": name, "password": "a-long-password-1" }))).await;
            assert_eq!(code, StatusCode::OK);
            cookie.split(';').next().unwrap_or("").to_string()
        }
    };
    let (admin, plain) = (login("boss-wrapper").await, login("plain-wrapper").await);
    assert_eq!(send(&app, "GET", "/api/system/wrapper", &plain, None, &[]).await.status, StatusCode::FORBIDDEN);
    // not installed: the card's actions say so
    let r = send(&app, "POST", "/api/system/wrapper/set-up", &admin, Some(json!({})), &[]).await;
    assert_eq!(r.status, StatusCode::CONFLICT, "{}", r.text());
    assert_eq!(send(&app, "POST", "/api/plugins/wrapper/install", &admin, Some(json!({})), &[]).await.status, StatusCode::OK);
    // the overview: managed, no socket, nothing uploaded
    let v = send(&app, "GET", "/api/system/wrapper", &admin, None, &[]).await.json();
    assert_eq!(v["mode"], "managed");
    assert_eq!(v["docker"]["ok"], false);
    assert!(v["docker"]["reason"].as_str().unwrap().contains("/var/run/docker.sock"), "{v}");
    assert!(v["bundle"].is_null());
    let card = send(&app, "GET", "/api/plugins/wrapper", &admin, None, &[]).await.json();
    assert!(card["status"]["message"].as_str().unwrap_or("").contains("Docker socket"), "{card}");
    // Set up runs in the background and fails on the socket
    let r = send(&app, "POST", "/api/system/wrapper/set-up", &admin, Some(json!({})), &[]).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let mut job = Value::Null;
    for _ in 0..50 {
        job = send(&app, "GET", "/api/system/wrapper", &admin, None, &[]).await.json()["job"].clone();
        if job["running"] == false {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(job["error"].as_str().unwrap_or("").contains("Docker socket"), "{job}");
    // uploads: not an app, then an APK
    let (code, v) = upload(&app, &admin, "notes.zip", zip_with("readme.txt", b"hi")).await;
    assert_eq!(code, StatusCode::BAD_REQUEST, "{v}");
    let (code, v) = upload(&app, &admin, "apple-music.apk", zip_with("lib/x86_64/libandroidappmusic.so", b"lib")).await;
    assert_eq!(code, StatusCode::OK, "{v}");
    assert_eq!((v["bundle"]["kind"].as_str(), v["bundle"]["name"].as_str()), (Some("APK"), Some("apple-music.apk")));
    assert_eq!(send(&app, "GET", "/api/system/wrapper", &admin, None, &[]).await.json()["bundle"]["kind"], "APK");
    // external mode without its address: the sign-in says what is missing
    ts.st.cfg.set_many(&json!({ "wrapper_mode": "external" }));
    let r = send(&app, "POST", "/api/system/wrapper/sign-in", &admin, Some(json!({ "apple_id": "me@example.com", "password": "pw-1" })), &[]).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.text());
    assert!(r.text().contains("address"), "{}", r.text());
    // an address nobody answers on: a network error, and the password is nowhere in it
    ts.st.cfg.set_many(&json!({ "wrapper_url": "http://127.0.0.1:9" }));
    let r = send(&app, "POST", "/api/system/wrapper/sign-in", &admin, Some(json!({ "apple_id": "me@example.com", "password": "pw-secret-1" })), &[]).await;
    assert_eq!(r.status, StatusCode::BAD_GATEWAY, "{}", r.text());
    assert!(!r.text().contains("pw-secret-1"));
    let log = send(&app, "GET", "/api/plugins/wrapper/logs?lines=50", &admin, None, &[]).await.text();
    assert!(!log.contains("pw-secret-1"), "no password in the plugin's log");
    // gamdl: the external address
    let r = send(&app, "POST", "/api/system/wrapper/gamdl", &admin, Some(json!({})), &[]).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(ts.st.cfg.str("gamdl_wrapper_url"), "http://127.0.0.1:9");
    ts.remove();
}
