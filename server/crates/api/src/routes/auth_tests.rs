//! App tokens (bearer sign-in for the native app) end to end through the real router, plus
//! `/api/library/tracks?ids=` and the `api`/`app_tokens` advertisement in `/api/auth/info`.
use encore_db::{now, RowExt};
use encore_db::p;
use crate::state::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use serde_json::{json, Value};
use tower::ServiceExt;

const PW: &str = "listener-pass-1";

struct Env {
    st: AppState,
    app: Router,
    dir: std::path::PathBuf,
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn setup(name: &str) -> Env {
    let dir = std::env::temp_dir().join(format!("encore-apptok-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let st = crate::test_support::state_in(&dir);
    crate::auth::create_user(&st, "listener", PW, "user", None, None).unwrap();
    let app = crate::app(st.clone());
    Env { st, app, dir }
}

struct Resp {
    status: StatusCode,
    headers: axum::http::HeaderMap,
    body: Value,
}

async fn call(app: &Router, method: &str, uri: &str, headers: &[(&str, &str)], body: Option<Value>) -> Resp {
    let mut b = Request::builder().method(method).uri(uri);
    for (k, v) in headers {
        b = b.header(*k, *v);
    }
    let req = match body {
        Some(j) => b.header("content-type", "application/json").body(Body::from(j.to_string())).unwrap(),
        None => b.body(Body::empty()).unwrap(),
    };
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = axum::body::to_bytes(resp.into_body(), 10 << 20).await.unwrap();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    Resp { status, headers, body }
}

async fn app_login(app: &Router, device_id: &str, device_name: &str) -> Resp {
    call(
        app,
        "POST",
        "/api/auth/login",
        &[("x-requested-with", "encore"), ("user-agent", "EncoreAndroid/1.0 (Android 35; Pixel 9 Pro)")],
        Some(json!({ "username": "listener", "password": PW, "app": { "device_id": device_id, "device_name": device_name } })),
    )
    .await
}

async fn web_login(app: &Router) -> String {
    let r = call(app, "POST", "/api/auth/login", &[("x-requested-with", "encore")], Some(json!({ "username": "listener", "password": PW }))).await;
    assert_eq!(r.status, StatusCode::OK);
    let c = r.headers.get("set-cookie").expect("web login sets a cookie").to_str().unwrap().to_string();
    c.split(';').next().unwrap().to_string() // "encore_session=…"
}

fn bearer(token: &str) -> String {
    format!("Bearer {token}")
}

#[tokio::test]
async fn app_login_returns_token_without_cookie_and_stores_only_a_digest() {
    let e = setup("login");
    let r = app_login(&e.app, "dev-1", "Pixel 9 Pro").await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert!(r.headers.get("set-cookie").is_none(), "app sign-in must not set a cookie");
    let token = r.body["token"].as_str().expect("token").to_string();
    assert!(token.len() >= 40);
    assert_eq!(r.body["user"]["username"], "listener");
    assert!(r.body["user"].get("password_hash").is_none());

    let rows = e.st.db.query("SELECT * FROM sessions WHERE kind='app'", p![]).unwrap();
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row.s("token"), format!("sha256:{}", encore_core::ids::sha256_hex(&token)));
    assert!(!row.s("token").contains(&token));
    assert_eq!(row.s("device_id"), "dev-1");
    assert_eq!(row.s("device_name"), "Pixel 9 Pro");
    assert!(row.f("last_seen_at") > 0.0);
    // 365 days by default, counted from now
    assert!((row.f("expires_at") - now() - 365.0 * 86400.0).abs() < 60.0);
    // the raw token isn't anywhere in the table
    assert!(e.st.db.one("SELECT token FROM sessions WHERE token=?", p![token]).unwrap().is_none());

    // web login is unchanged: a cookie and the user object itself
    let cookie = web_login(&e.app).await;
    assert!(cookie.starts_with("encore_session="));
    assert_eq!(e.st.db.one("SELECT COUNT(*) n FROM sessions WHERE kind='web'", p![]).unwrap().unwrap().i("n"), 1);
}

#[tokio::test]
async fn bearer_works_on_get_and_on_writes_without_csrf_header() {
    let e = setup("bearer");
    let token = app_login(&e.app, "dev-1", "Pixel").await.body["token"].as_str().unwrap().to_string();
    let auth = bearer(&token);

    let r = call(&e.app, "GET", "/api/auth/me", &[("authorization", &auth)], None).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.body["username"], "listener");
    // lower-case scheme is fine too
    let r = call(&e.app, "GET", "/api/auth/me", &[("authorization", &format!("bearer {token}"))], None).await;
    assert_eq!(r.status, StatusCode::OK);

    // a POST and a PUT with no X-Requested-With
    let r = call(&e.app, "POST", "/api/auth/2fa/setup", &[("authorization", &auth)], Some(json!({}))).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let r = call(&e.app, "PUT", "/api/auth/prefs", &[("authorization", &auth)], Some(json!({ "theme": "oled" }))).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(r.body["theme"], "oled");

    // cookies still need the CSRF header
    let cookie = web_login(&e.app).await;
    let r = call(&e.app, "PUT", "/api/auth/prefs", &[("cookie", &cookie)], Some(json!({ "theme": "light" }))).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);

    // a wrong bearer is a 401 even with a valid cookie next to it (no fall back)
    let r = call(&e.app, "GET", "/api/auth/me", &[("authorization", "Bearer nope"), ("cookie", &cookie)], None).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    // the stored digest is not a cookie
    let digest = format!("encore_session=sha256:{}", encore_core::ids::sha256_hex(&token));
    let r = call(&e.app, "GET", "/api/auth/me", &[("cookie", &digest)], None).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    // nor is a cookie value a bearer token
    let r = call(&e.app, "GET", "/api/auth/me", &[("authorization", &bearer(cookie.trim_start_matches("encore_session=")))], None).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn same_device_relogin_replaces_the_row() {
    let e = setup("relogin");
    let t1 = app_login(&e.app, "dev-1", "Pixel").await.body["token"].as_str().unwrap().to_string();
    let t2 = app_login(&e.app, "dev-1", "Pixel (renamed)").await.body["token"].as_str().unwrap().to_string();
    assert_ne!(t1, t2);
    let n = |dev: &str| e.st.db.one("SELECT COUNT(*) n FROM sessions WHERE kind='app' AND device_id=?", p![dev]).unwrap().unwrap().i("n");
    assert_eq!(n("dev-1"), 1);
    assert_eq!(call(&e.app, "GET", "/api/auth/me", &[("authorization", &bearer(&t1))], None).await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(call(&e.app, "GET", "/api/auth/me", &[("authorization", &bearer(&t2))], None).await.status, StatusCode::OK);
    // another device is another row
    app_login(&e.app, "dev-2", "Tablet").await;
    assert_eq!(n("dev-2"), 1);
    assert_eq!(e.st.db.one("SELECT COUNT(*) n FROM sessions WHERE kind='app'", p![]).unwrap().unwrap().i("n"), 2);
    // a missing device id is refused before anything happens
    let r = call(&e.app, "POST", "/api/auth/login", &[], Some(json!({ "username": "listener", "password": PW, "app": { "device_id": " " } }))).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn sessions_list_and_revoke() {
    let e = setup("revoke");
    let t1 = app_login(&e.app, "dev-1", "Pixel 9 Pro").await.body["token"].as_str().unwrap().to_string();
    let t2 = app_login(&e.app, "dev-2", "Tablet").await.body["token"].as_str().unwrap().to_string();
    let cookie = web_login(&e.app).await;

    let r = call(&e.app, "GET", "/api/auth/sessions", &[("authorization", &bearer(&t1))], None).await;
    assert_eq!(r.status, StatusCode::OK);
    let rows = r.body.as_array().unwrap().clone();
    assert_eq!(rows.len(), 3);
    let me = rows.iter().find(|s| s["current"] == true).expect("current row");
    assert_eq!(me["kind"], "app");
    assert_eq!(me["device_name"], "Pixel 9 Pro");
    assert!(me["last_seen_at"].as_f64().unwrap() > 0.0);
    assert_eq!(me["device"], "Encore app on Android");
    assert!(me["id"].as_str().unwrap().starts_with("sha256:"));
    assert_eq!(me["id"].as_str().unwrap().len(), 17);
    let web = rows.iter().find(|s| s["kind"] == "web").unwrap();
    assert_eq!(web["id"].as_str().unwrap().len(), 10);
    assert!(web["device_name"].is_null() && web["last_seen_at"].is_null());

    // the web revokes the tablet
    let tablet = rows.iter().find(|s| s["device_name"] == "Tablet").unwrap()["id"].as_str().unwrap().to_string();
    let r = call(&e.app, "DELETE", &format!("/api/auth/sessions/{tablet}"), &[("cookie", &cookie), ("x-requested-with", "encore")], None).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(call(&e.app, "GET", "/api/auth/me", &[("authorization", &bearer(&t2))], None).await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(call(&e.app, "GET", "/api/auth/me", &[("authorization", &bearer(&t1))], None).await.status, StatusCode::OK);
    // a too-short id can't wipe every app session at once
    let r = call(&e.app, "DELETE", "/api/auth/sessions/sha256:", &[("authorization", &bearer(&t1))], None).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(call(&e.app, "GET", "/api/auth/me", &[("authorization", &bearer(&t1))], None).await.status, StatusCode::OK);

    // the app signs itself out: its row goes, the web session stays
    let r = call(&e.app, "POST", "/api/auth/logout", &[("authorization", &bearer(&t1))], None).await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.headers.get("set-cookie").is_none());
    assert_eq!(call(&e.app, "GET", "/api/auth/me", &[("authorization", &bearer(&t1))], None).await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(call(&e.app, "GET", "/api/auth/me", &[("cookie", &cookie)], None).await.status, StatusCode::OK);

    // sign out everywhere takes app tokens too
    let t3 = app_login(&e.app, "dev-3", "Phone").await.body["token"].as_str().unwrap().to_string();
    let r = call(&e.app, "POST", "/api/auth/logout-all", &[("cookie", &cookie), ("x-requested-with", "encore")], None).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(call(&e.app, "GET", "/api/auth/me", &[("authorization", &bearer(&t3))], None).await.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn password_change_from_the_app_keeps_the_app_signed_in() {
    let e = setup("pwchange");
    let t1 = app_login(&e.app, "dev-1", "Pixel").await.body["token"].as_str().unwrap().to_string();
    let t2 = app_login(&e.app, "dev-2", "Tablet").await.body["token"].as_str().unwrap().to_string();
    let r = call(&e.app, "POST", "/api/auth/password", &[("authorization", &bearer(&t1))], Some(json!({ "current": PW, "new": "another-pass-2" }))).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(call(&e.app, "GET", "/api/auth/me", &[("authorization", &bearer(&t1))], None).await.status, StatusCode::OK);
    assert_eq!(call(&e.app, "GET", "/api/auth/me", &[("authorization", &bearer(&t2))], None).await.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn expired_is_401_and_use_slides_the_expiry() {
    let e = setup("expiry");
    let token = app_login(&e.app, "dev-1", "Pixel").await.body["token"].as_str().unwrap().to_string();
    let key = crate::auth::app_token_key(&token);
    let auth = bearer(&token);

    // used within the day: no write
    e.st.db.execute("UPDATE sessions SET last_seen_at=?, expires_at=? WHERE token=?", p![now() - 3600.0, now() + 100.0, key]).unwrap();
    assert_eq!(call(&e.app, "GET", "/api/auth/me", &[("authorization", &auth)], None).await.status, StatusCode::OK);
    let row = e.st.db.one("SELECT * FROM sessions WHERE token=?", p![key]).unwrap().unwrap();
    assert!(row.f("expires_at") < now() + 200.0);

    // last seen two days ago: this request moves expires_at a full lifetime ahead
    e.st.db.execute("UPDATE sessions SET last_seen_at=? WHERE token=?", p![now() - 2.0 * 86400.0, key]).unwrap();
    assert_eq!(call(&e.app, "GET", "/api/auth/me", &[("authorization", &auth)], None).await.status, StatusCode::OK);
    let row = e.st.db.one("SELECT * FROM sessions WHERE token=?", p![key]).unwrap().unwrap();
    assert!((row.f("expires_at") - now() - 365.0 * 86400.0).abs() < 60.0);
    assert!(now() - row.f("last_seen_at") < 60.0);

    // expired: 401
    e.st.db.execute("UPDATE sessions SET expires_at=? WHERE token=?", p![now() - 1.0, key]).unwrap();
    assert_eq!(call(&e.app, "GET", "/api/auth/me", &[("authorization", &auth)], None).await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(call(&e.app, "POST", "/api/auth/2fa/setup", &[("authorization", &auth)], Some(json!({}))).await.status, StatusCode::UNAUTHORIZED);

    // disabled account: 401
    let fresh = app_login(&e.app, "dev-1", "Pixel").await.body["token"].as_str().unwrap().to_string();
    e.st.db.execute("UPDATE users SET disabled=1 WHERE username='listener'", p![]).unwrap();
    assert_eq!(call(&e.app, "GET", "/api/auth/me", &[("authorization", &bearer(&fresh))], None).await.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn info_advertises_app_tokens_and_api_level() {
    let e = setup("info");
    let r = call(&e.app, "GET", "/api/auth/info", &[], None).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.body["api"], 2);
    assert!(r.body["features"].as_array().unwrap().iter().any(|f| f == "app_tokens"));
    assert!(r.body["features"].as_array().unwrap().iter().any(|f| f == "engine_token"));
}

// ---- POST /api/auth/app-token: a signed-in session mints a bearer token for a device ----------

async fn app_token(app: &Router, headers: &[(&str, &str)], body: Value) -> Resp {
    call(app, "POST", "/api/auth/app-token", headers, Some(body)).await
}

#[tokio::test]
async fn app_token_from_a_cookie_session_works_as_bearer_and_is_listed_as_an_app_session() {
    let e = setup("apptoken");
    let cookie = web_login(&e.app).await;
    let r = app_token(&e.app, &[("cookie", &cookie), ("x-requested-with", "encore")], json!({ "device_id": "cadenza-desktop-abc", "device_name": "Cadenza on studio" })).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(r.body["device_id"], "cadenza-desktop-abc");
    let token = r.body["token"].as_str().unwrap().to_string();
    assert!(token.len() >= 32);
    assert!(r.headers.get("set-cookie").is_none(), "no cookie is set for an app token");
    // only a digest is stored
    assert!(e.st.db.one("SELECT token FROM sessions WHERE kind='app' AND device_id='cadenza-desktop-abc'", p![]).unwrap().unwrap().s("token").starts_with("sha256:"));

    // the token signs in with Authorization: Bearer
    let me = call(&e.app, "GET", "/api/auth/me", &[("authorization", &bearer(&token))], None).await;
    assert_eq!(me.status, StatusCode::OK);
    assert_eq!(me.body["username"], "listener");

    // listed under Account → sessions as kind app with the device name
    let r = call(&e.app, "GET", "/api/auth/sessions", &[("cookie", &cookie)], None).await;
    assert_eq!(r.status, StatusCode::OK);
    let rows = r.body.as_array().unwrap();
    assert_eq!(rows.len(), 2);
    let app_row = rows.iter().find(|s| s["kind"] == "app").expect("app session row");
    assert_eq!(app_row["device_name"], "Cadenza on studio");
    assert_eq!(app_row["current"], false);
    assert!(app_row["last_seen_at"].as_f64().unwrap() > 0.0);
    let cur = rows.iter().find(|s| s["current"] == true).unwrap();
    assert_eq!(cur["kind"], "web");

    // logging the browser out does not revoke the app token
    let r = call(&e.app, "POST", "/api/auth/logout", &[("cookie", &cookie), ("x-requested-with", "encore")], None).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(call(&e.app, "GET", "/api/auth/me", &[("authorization", &bearer(&token))], None).await.status, StatusCode::OK);
}

#[tokio::test]
async fn app_token_reissue_replaces_the_previous_one_for_the_same_device() {
    let e = setup("apptoken-reissue");
    let cookie = web_login(&e.app).await;
    let h = [("cookie", cookie.as_str()), ("x-requested-with", "encore")];
    let t1 = app_token(&e.app, &h, json!({ "device_id": "cadenza-desktop-abc", "device_name": "Cadenza on studio" })).await.body["token"].as_str().unwrap().to_string();
    let t2 = app_token(&e.app, &h, json!({ "device_id": "cadenza-desktop-abc", "device_name": "Cadenza on studio" })).await.body["token"].as_str().unwrap().to_string();
    assert_ne!(t1, t2);
    let n = |dev: &str| e.st.db.one("SELECT COUNT(*) n FROM sessions WHERE kind='app' AND device_id=?", p![dev]).unwrap().unwrap().i("n");
    assert_eq!(n("cadenza-desktop-abc"), 1);
    assert_eq!(call(&e.app, "GET", "/api/auth/me", &[("authorization", &bearer(&t1))], None).await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(call(&e.app, "GET", "/api/auth/me", &[("authorization", &bearer(&t2))], None).await.status, StatusCode::OK);
    // another device id is another row, and the web session is untouched
    app_token(&e.app, &h, json!({ "device_id": "cadenza-desktop-xyz" })).await;
    assert_eq!(n("cadenza-desktop-xyz"), 1);
    assert_eq!(e.st.db.one("SELECT COUNT(*) n FROM sessions WHERE kind='app'", p![]).unwrap().unwrap().i("n"), 2);
    assert_eq!(e.st.db.one("SELECT COUNT(*) n FROM sessions WHERE kind='web'", p![]).unwrap().unwrap().i("n"), 1);
    // a bearer session may ask too (no CSRF header needed)
    let r = app_token(&e.app, &[("authorization", &bearer(&t2))], json!({ "device_id": "cadenza-desktop-abc", "device_name": "Cadenza on studio" })).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(n("cadenza-desktop-abc"), 1);
}

#[tokio::test]
async fn app_token_rejects_bad_ids_and_anonymous_callers() {
    let e = setup("apptoken-bad");
    let cookie = web_login(&e.app).await;
    let h = [("cookie", cookie.as_str()), ("x-requested-with", "encore")];
    assert_eq!(app_token(&e.app, &h, json!({ "device_id": "" })).await.status, StatusCode::BAD_REQUEST);
    assert_eq!(app_token(&e.app, &h, json!({ "device_id": "   " })).await.status, StatusCode::BAD_REQUEST);
    assert_eq!(app_token(&e.app, &h, json!({ "device_id": "x".repeat(65) })).await.status, StatusCode::BAD_REQUEST);
    assert_eq!(app_token(&e.app, &h, json!({ "device_id": "x".repeat(64) })).await.status, StatusCode::OK);
    assert_eq!(e.st.db.one("SELECT COUNT(*) n FROM sessions WHERE kind='app'", p![]).unwrap().unwrap().i("n"), 1);
    // no session at all
    assert_eq!(app_token(&e.app, &[("x-requested-with", "encore")], json!({ "device_id": "cadenza-desktop-abc" })).await.status, StatusCode::UNAUTHORIZED);
    // a cookie without the CSRF header is refused like every other POST
    assert_eq!(app_token(&e.app, &[("cookie", &cookie)], json!({ "device_id": "cadenza-desktop-abc" })).await.status, StatusCode::FORBIDDEN);
    assert_eq!(e.st.db.one("SELECT COUNT(*) n FROM sessions WHERE kind='app'", p![]).unwrap().unwrap().i("n"), 1);
}

#[tokio::test]
async fn tracks_by_ids_keep_the_requested_order() {
    let e = setup("ids");
    for (i, t) in ["One", "Two", "Three"].iter().enumerate() {
        e.st.db.execute("INSERT INTO tracks (path, album_id, title, artist, album, albumartist, track, duration, format) VALUES (?,?,?,?,?,?,?,?,?)",
            p![format!("/m/{t}.flac"), "alb", t, "Artist", "Album", "Artist", i as i64 + 1, 200.0, "flac"]).unwrap();
    }
    let token = app_login(&e.app, "dev-1", "Pixel").await.body["token"].as_str().unwrap().to_string();
    let auth = bearer(&token);
    let r = call(&e.app, "GET", "/api/library/tracks?ids=3,1,999,3,2", &[("authorization", &auth)], None).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let titles: Vec<&str> = r.body.as_array().unwrap().iter().map(|t| t["title"].as_str().unwrap()).collect();
    assert_eq!(titles, ["Three", "One", "Two"]);
    let first = &r.body[0];
    for k in ["id", "title", "artist", "album", "albumartist", "album_id", "track", "disc", "year", "duration", "format", "bitrate", "added_at"] {
        assert!(first.get(k).is_some(), "missing {k}");
    }
    assert!(first.get("path").is_none());

    let r = call(&e.app, "GET", "/api/library/tracks?ids=", &[("authorization", &auth)], None).await;
    assert_eq!(r.body, json!([]));
    let r = call(&e.app, "GET", "/api/library/tracks?ids=1,x", &[("authorization", &auth)], None).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let many = (1..=301).map(|i| i.to_string()).collect::<Vec<_>>().join(",");
    let r = call(&e.app, "GET", &format!("/api/library/tracks?ids={many}"), &[("authorization", &auth)], None).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let ok = (1..=300).map(|i| i.to_string()).collect::<Vec<_>>().join(",");
    let r = call(&e.app, "GET", &format!("/api/library/tracks?ids={ok}"), &[("authorization", &auth)], None).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.body.as_array().unwrap().len(), 3);
    // without ids the list is unchanged
    let r = call(&e.app, "GET", "/api/library/tracks?sort=title", &[("authorization", &auth)], None).await;
    assert_eq!(r.body.as_array().unwrap().len(), 3);
}
