//! The YouTube sign-in (encore_requests::ytauth) through the router: the extension's cookie upload and the
//! hand-off, with YouTube Music's account lookup standing in locally.
use axum::routing::post;
use axum::{Json, Router};
use encore_db::now;
use encore_media::Ctx as _;
use encore_requests::ytauth::*;
use serde_json::{json, Value};
use std::path::Path;
use std::time::Duration;

/// A stand-in for music.youtube.com's web API, the only network `account_of` needs: every call is answered
/// with one account after `delay` ms. → its address, for the server's `urls.ytmusic`. Nothing in these tests
/// reaches the network.
async fn mock_ytmusic(account: &'static str, delay: u64) -> String {
    let app = Router::new().route("/account/account_menu", post(move || async move {
        tokio::time::sleep(Duration::from_millis(delay)).await;
        Json(json!({ "actions": [{ "openPopupAction": { "popup": { "multiPageMenuRenderer": { "header": { "activeAccountHeaderRenderer": {
            "accountName": { "runs": [{ "text": account }] } } } } } } }] }))
    }));
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move {
        let _ = axum::serve(l, app).await;
    });
    url
}

/// A server on `dir` whose YouTube Music is the stand-in at `ytmusic`.
fn state_with_ytmusic(dir: &Path, ytmusic: String) -> crate::state::AppState {
    crate::test_support::TestServer::in_dir(dir).with_urls(encore_http::Urls { ytmusic, ..Default::default() }).st
}

/// Wait (up to 5 s) until the state file says what `f` wants.
async fn eventually(data: &Path, f: impl Fn(&AuthState) -> bool) -> bool {
    for _ in 0..100 {
        if f(&load(data)) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

#[tokio::test]
async fn extension_upload_route_needs_its_token() {
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;
    let dir = std::env::temp_dir().join(format!("encore-ytauth-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let st = state_with_ytmusic(&dir, mock_ytmusic("Kris", 0).await);
    let app = crate::app(st.clone());
    let ua = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36 Brave";
    let body = json!({ "cookies": [
        { "domain": ".youtube.com", "hostOnly": false, "name": "SAPISID", "value": "a", "path": "/", "secure": true, "httpOnly": false, "session": false, "expirationDate": 4_000_000_000u64 },
    ], "agent": "Chrome 141", "user_agent": ua });
    let post = |tok: Option<&str>| {
        let mut b = Request::builder().method("POST").uri("/api/youtube/extension-cookies").header("content-type", "application/json");
        if let Some(t) = tok {
            b = b.header("authorization", format!("Bearer {t}"));
        }
        b.body(Body::from(body.to_string())).unwrap()
    };
    let r = app.clone().oneshot(post(Some("eyt_guess"))).await.unwrap();
    assert_eq!(r.status(), 401);
    assert_eq!(r.headers().get("access-control-allow-origin").unwrap(), "*");
    // a token made the way the route makes it
    let token = "eyt_testtoken";
    update(&st, |s| s.ext_token = Some(token_hash(token)));
    let r = app.clone().oneshot(post(Some(token))).await.unwrap();
    assert_eq!(r.status(), 200);
    let saved = std::fs::read_to_string(ext_path(st.data_dir())).unwrap();
    assert!(saved.contains("SAPISID\ta"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(ext_path(st.data_dir())).unwrap().permissions().mode() & 0o777, 0o600);
    }
    let s = load(st.data_dir());
    assert_eq!(s.extension.agent.as_deref(), Some("Chrome 141"));
    assert_eq!(s.extension.user_agent.as_deref(), Some(ua), "the browser's User-Agent is kept for the sign-in check");
    assert_eq!(rotate::user_agent(s.extension.user_agent.as_deref(), s.extension.agent.as_deref()), ua);
    // the extension's cookies are now what yt-dlp gets
    assert_eq!(active(&st), Some(Kind::Extension));
    assert!(cookies_for_ytdlp(&st).unwrap().contains("SAPISID\ta"));
    // the account name arrives from the lookup after the answer
    assert!(eventually(st.data_dir(), |s| s.extension.account.as_deref() == Some("Kris")).await, "account looked up in the background");
    // the status route is for admins only
    let r = app.clone().oneshot(Request::builder().uri("/api/youtube/auth").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(r.status(), 401);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn handoff_becomes_the_server_owned_session_and_stays_out_of_logs() {
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;
    let dir = std::env::temp_dir().join(format!("encore-ytauth-ho-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // YouTube Music stands in locally and takes its time: the hand-off answer must not wait for it
    const SLOW_MS: u64 = 1500;
    let st = state_with_ytmusic(&dir, mock_ytmusic("Kris (@kris)", SLOW_MS).await);
    let app = crate::app(st.clone());
    let ff = "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:143.0) Gecko/20100101 Firefox/143.0";
    let token = "eyt_handofftoken";
    update(&st, |s| s.ext_token = Some(token_hash(token)));
    let post = |handoff: bool, value: &str| {
        let body = json!({ "handoff": handoff, "agent": "Firefox 143", "user_agent": ff, "cookies": [
            { "domain": ".youtube.com", "hostOnly": false, "name": "SAPISID", "value": value, "path": "/", "secure": true, "httpOnly": false, "session": false, "expirationDate": 4_000_000_000u64 },
            { "domain": ".youtube.com", "hostOnly": false, "name": "__Secure-3PSID", "value": format!("{value}-3p"), "path": "/", "secure": true, "httpOnly": true, "session": false, "expirationDate": 4_000_000_000u64 },
        ] });
        Request::builder().method("POST").uri("/api/youtube/extension-cookies").header("content-type", "application/json")
            .header("authorization", format!("Bearer {token}")).body(Body::from(body.to_string())).unwrap()
    };
    // the everyday browser first, then a hand-off: the hand-off wins
    let r = app.clone().oneshot(post(false, "everydaySECRET")).await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(active(&st), Some(Kind::Extension));
    let started = std::time::Instant::now();
    let r = app.clone().oneshot(post(true, "handoffSECRET")).await.unwrap();
    assert_eq!(r.status(), 200);
    assert!(started.elapsed() < Duration::from_millis(SLOW_MS), "the hand-off is answered without waiting for YouTube ({:?})", started.elapsed());
    let bytes = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(!text.contains("SECRET"), "no cookie values in the answer: {text}");
    let v: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["handoff"], true);
    assert_eq!(v["ok"], true);
    assert_eq!(active(&st), Some(Kind::Handoff));
    assert!(cookies_for_ytdlp(&st).unwrap().contains("handoffSECRET"));
    let s = load(st.data_dir());
    assert_eq!(s.handoff.ok, Some(true));
    assert_eq!(s.handoff.agent.as_deref(), Some("Firefox 143"));
    assert_eq!(s.handoff.user_agent.as_deref(), Some(ff), "the refresh will present the browser the hand-off came from");
    assert_eq!(rotate::user_agent(s.handoff.user_agent.as_deref(), s.handoff.agent.as_deref()), ff);
    assert!(s.extension.updated_at.is_some(), "the everyday browser's cookies stay as the next choice");
    // the server refreshes the hand-off itself; the extension's never
    assert!(next_refresh(now(), active(&st), &s, false).is_some());
    let shown = status(&st).to_string();
    assert_eq!(status_source(&shown), "handoff");
    assert!(!shown.contains("SECRET"), "no cookie values in the status");
    assert!(shown.contains("Handed-over sign-in") && shown.contains("Everyday browser (extension)"), "{shown}");
    // the account name follows once YouTube answered
    assert!(eventually(st.data_dir(), |s| s.handoff.account.as_deref() == Some("Kris (@kris)")).await, "account looked up in the background");
    assert_eq!(status(&st)["account"], "Kris (@kris)");
    // a refresh that renewed nothing is reported as a check, and "last refresh" stays
    let handed_at = load(st.data_dir()).handoff.updated_at;
    update(&st, |s| note_refresh(&mut s.handoff, now() + 1.0, true, Some(0), None, None));
    let v = status(&st);
    assert_eq!(v["last_refresh"], json!(handed_at));
    assert_eq!(v["sources"]["handoff"]["rotated"], 0);
    assert!(v["diagnostics"].as_str().unwrap().contains("renewed no cookies"), "{}", v["diagnostics"]);
    let logs = st.db().query("SELECT message FROM activity", vec![]).unwrap();
    assert!(logs.iter().any(|r| r["message"].as_str().unwrap_or("").contains("handed over")));
    assert!(!logs.iter().any(|r| r["message"].as_str().unwrap_or("").contains("SECRET")), "no cookie values in the log");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(handoff_path(st.data_dir())).unwrap().permissions().mode() & 0o777, 0o600);
    }
    // a window that isn't signed in yet: nothing replaced
    let empty = Request::builder().method("POST").uri("/api/youtube/extension-cookies").header("content-type", "application/json")
        .header("authorization", format!("Bearer {token}")).body(Body::from(json!({ "handoff": true, "cookies": [] }).to_string())).unwrap();
    let r = app.clone().oneshot(empty).await.unwrap();
    let v: Value = serde_json::from_slice(&axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap()).unwrap();
    assert_eq!(v["ok"], false);
    assert!(cookies_for_ytdlp(&st).unwrap().contains("handoffSECRET"));
    assert_eq!(load(st.data_dir()).handoff.account.as_deref(), Some("Kris (@kris)"), "a failed hand-off changes nothing");
    let _ = std::fs::remove_dir_all(&dir);
}

fn status_source(status: &str) -> String {
    serde_json::from_str::<Value>(status).unwrap()["source"].as_str().unwrap_or_default().to_string()
}
