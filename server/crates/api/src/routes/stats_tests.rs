//! The leaderboard's boards and the rank on Home through the real router: the totals are reused between
//! calls, but a like, a new playlist or a play shows at the next call, as it did when they were counted on
//! every call.
use crate::state::AppState;
use crate::test_support::TempState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use serde_json::{json, Value};
use tower::ServiceExt;

const PW: &str = "listener-pass-1";

async fn login(app: &Router, name: &str) -> String {
    let req = Request::builder().method("POST").uri("/api/auth/login").header("x-requested-with", "encore").header("content-type", "application/json")
        .body(Body::from(json!({ "username": name, "password": PW }).to_string())).unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    resp.headers().get("set-cookie").and_then(|v| v.to_str().ok()).unwrap_or("").split(';').next().unwrap().to_string()
}

async fn call(app: &Router, cookie: &str, method: &str, uri: &str, body: Option<Value>) -> Value {
    let mut b = Request::builder().method(method).uri(uri).header("x-requested-with", "encore").header("cookie", cookie);
    if body.is_some() {
        b = b.header("content-type", "application/json");
    }
    let req = b.body(body.map(|v| Body::from(v.to_string())).unwrap_or_else(Body::empty)).unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "{method} {uri}");
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

/// One board's rows as (username, value, me).
fn board(v: &Value, key: &str) -> Vec<(String, i64, bool)> {
    v["boards"][key]["rows"].as_array().unwrap().iter()
        .map(|r| (r["username"].as_str().unwrap_or("").to_string(), r["value"].as_i64().unwrap_or(-1), r["me"].as_bool().unwrap_or(false))).collect()
}

fn song(st: &AppState, id: i64) {
    st.db.execute("INSERT INTO tracks (id, path, title, artist, album, album_id, format, duration, added_at) VALUES (?,?,?,?,?,?,?,?,?)",
        encore_db::p![id, format!("/music/{id}.flac"), format!("Song {id}"), "Tester", "Tones", "tester|tones", "flac", 200.0, 1.0]).unwrap();
}

#[tokio::test]
async fn the_boards_and_the_rank_follow_every_write() {
    // removed again when the test ends, also when it fails
    let st = TempState::new("stats-fresh");
    for name in ["ada", "bob"] {
        crate::auth::create_user(&st, name, PW, "user", None, None).unwrap();
    }
    for id in 1..=3 {
        song(&st, id);
    }
    let app = crate::app((*st).clone());
    let (ada, bob) = (login(&app, "ada").await, login(&app, "bob").await);

    // nothing yet; the second call is answered from the cache
    for _ in 0..2 {
        let lb = call(&app, &ada, "GET", "/api/stats/leaderboard", None).await;
        for key in ["requests", "plays", "listened", "likes", "playlists"] {
            assert!(board(&lb, key).is_empty(), "{key}: {lb}");
        }
    }

    // a like and a new playlist show at the next call
    call(&app, &ada, "POST", "/api/likes/1", None).await;
    call(&app, &ada, "POST", "/api/playlists", Some(json!({ "name": "Fresh" }))).await;
    let lb = call(&app, &ada, "GET", "/api/stats/leaderboard", None).await;
    assert_eq!(board(&lb, "likes"), vec![("ada".to_string(), 1, true)]);
    assert_eq!(board(&lb, "playlists"), vec![("ada".to_string(), 1, true)]);
    // another user's like too; "me" is per caller
    call(&app, &bob, "POST", "/api/likes/2", None).await;
    call(&app, &bob, "POST", "/api/likes/3", None).await;
    let lb = call(&app, &ada, "GET", "/api/stats/leaderboard", None).await;
    assert_eq!(board(&lb, "likes"), vec![("bob".to_string(), 2, false), ("ada".to_string(), 1, true)]);
    // an unlike as well
    call(&app, &bob, "DELETE", "/api/likes/3", None).await;
    let lb = call(&app, &bob, "GET", "/api/stats/leaderboard", None).await;
    let likes = board(&lb, "likes");
    assert_eq!(likes.len(), 2);
    assert!(likes.contains(&("bob".to_string(), 1, true)), "{likes:?}");

    // the rank on Home: requests + plays, moved by the next play at once
    call(&app, &ada, "POST", "/api/library/plays", Some(json!({ "track_id": 1, "seconds": 60 }))).await;
    assert_eq!(call(&app, &ada, "GET", "/api/stats/me", None).await["me"]["rank"], json!(1));
    // (the server's first account, the owner, is on the boards too, with nothing yet)
    assert_ne!(call(&app, &bob, "GET", "/api/stats/me", None).await["me"]["rank"], json!(1));
    for _ in 0..2 {
        call(&app, &bob, "POST", "/api/library/plays", Some(json!({ "track_id": 2, "seconds": 60 }))).await;
    }
    assert_eq!(call(&app, &ada, "GET", "/api/stats/me", None).await["me"]["rank"], json!(2));
    assert_eq!(call(&app, &bob, "GET", "/api/stats/me", None).await["me"]["rank"], json!(1));
    let lb = call(&app, &ada, "GET", "/api/stats/leaderboard", None).await;
    assert_eq!(board(&lb, "plays"), vec![("bob".to_string(), 2, false), ("ada".to_string(), 1, true)]);

    // a disabled account leaves the boards at once
    st.db.execute("UPDATE users SET disabled=1 WHERE username='bob'", encore_db::p![]).unwrap();
    let lb = call(&app, &ada, "GET", "/api/stats/leaderboard", None).await;
    assert_eq!(board(&lb, "plays"), vec![("ada".to_string(), 1, true)]);

    // your own numbers on Home are kept between calls too, and the next play shows at once
    let me = call(&app, &ada, "GET", "/api/stats/me", None).await;
    assert_eq!((me["me"]["plays"].as_i64(), me["me"]["listened"].as_f64(), me["me"]["requests"].as_i64()), (Some(1), Some(60.0), Some(0)), "{me}");
    assert_eq!(call(&app, &ada, "GET", "/api/stats/me", None).await["me"], me["me"]);
    call(&app, &ada, "POST", "/api/library/plays", Some(json!({ "track_id": 3, "seconds": 30 }))).await;
    let me = call(&app, &ada, "GET", "/api/stats/me", None).await;
    assert_eq!((me["me"]["plays"].as_i64(), me["me"]["listened"].as_f64()), (Some(2), Some(90.0)), "{me}");
    assert_eq!(me["library"]["tracks"], json!(3), "{me}");
}

/// The library's fun numbers: whole numbers with thousands separators, one decimal only below ten.
#[test]
fn library_numbers_read_as_numbers() {
    assert_eq!(super::times_through(157_680.04), "157,680");
    assert_eq!(super::times_through(12.6), "13");
    assert_eq!(super::times_through(2.44), "2.4");
    assert_eq!(super::thousands(1_234_567), "1,234,567");
    assert_eq!(super::thousands(0), "0");
}
