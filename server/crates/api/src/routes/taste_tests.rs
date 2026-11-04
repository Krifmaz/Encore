//! /api/taste through the real router: smart shuffle and the plain shuffle never offer a disliked song or artist,
//! a dislike and a like undo each other, skips are saved (not the first-second ones), the mixes, genres, "More
//! like this" and the profile answer from the listener's plays and likes.
use crate::state::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use encore_db::{now, p, RowExt};
use serde_json::{json, Value};
use std::path::PathBuf;
use tower::ServiceExt;

/// The test user's password.
const PW: &str = "listener-pass-1";

/// A server with one signed-in listener and a small library.
struct Env {
    /// the server's state
    st: AppState,
    /// the router
    app: Router,
    /// the temporary folder (removed on drop)
    dir: PathBuf,
    /// the listener's session cookie
    cookie: String,
    /// the listener's id
    uid: i64,
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A server whose library has 6 artists × 6 songs: ids 10k+1..10k+6 for artist k (1..=6); artists 1–3 are rock,
/// 4–6 jazz.
async fn setup(name: &str) -> Env {
    let dir = std::env::temp_dir().join(format!("encore-taste-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let st = crate::test_support::state_in(&dir);
    let uid = crate::auth::create_user(&st, "listener", PW, "user", None, None).unwrap();
    let app = crate::app(st.clone());
    let req = Request::builder().method("POST").uri("/api/auth/login").header("x-requested-with", "encore").header("content-type", "application/json")
        .body(Body::from(json!({ "username": "listener", "password": PW }).to_string())).unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let cookie = resp.headers().get("set-cookie").and_then(|v| v.to_str().ok()).unwrap_or("").split(';').next().unwrap().to_string();
    for k in 1..=6 {
        let genre = if k <= 3 { "Rock" } else { "Jazz" };
        for j in 1..=6 {
            let id = k * 10 + j;
            st.db.execute("INSERT INTO tracks (id, path, title, artist, album, album_id, genre, year, format, duration, added_at) VALUES (?,?,?,?,?,?,?,?,?,?,?)",
                p![id, format!("/music/{id}.flac"), format!("Song {id}"), format!("Artist {k}"), format!("Album {k}"), format!("a{k}"), genre, "1999", "flac", 200.0, 1.0]).unwrap();
        }
    }
    Env { st, app, dir, cookie, uid }
}

impl Env {
    /// One call: method, path and an optional JSON body → status and JSON answer.
    async fn call(&self, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        let req = Request::builder().method(method).uri(uri).header("x-requested-with", "encore").header("content-type", "application/json")
            .header("cookie", &self.cookie).body(body.map(|b| Body::from(b.to_string())).unwrap_or_else(Body::empty)).unwrap();
        let resp = self.app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 8 << 20).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    /// The ids of a song list answer.
    fn ids(v: &Value) -> Vec<i64> {
        v.as_array().unwrap().iter().map(|t| t["id"].as_i64().unwrap()).collect()
    }
}

#[tokio::test]
async fn dislikes_never_come_back() {
    let e = setup("dislikes").await;
    assert_eq!(e.call("PUT", "/api/taste/dislikes/tracks/11", None).await.0, StatusCode::OK);
    assert_eq!(e.call("PUT", "/api/taste/dislikes/artists/artist%202", None).await.0, StatusCode::OK);
    let (s, d) = e.call("GET", "/api/taste/dislikes", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(d["tracks"][0]["id"], json!(11));
    assert_eq!(d["tracks"][0]["name"], json!("Artist 1 – Song 11"));
    assert_eq!(d["artists"][0]["name"], json!("artist 2"));
    for _ in 0..5 {
        let (s, v) = e.call("POST", "/api/taste/shuffle", Some(json!({ "limit": 200 }))).await;
        assert_eq!(s, StatusCode::OK);
        let ids = Env::ids(&v);
        assert_eq!(ids.len(), 36 - 1 - 6);
        assert!(!ids.contains(&11) && !ids.iter().any(|i| (21..=26).contains(i)), "{ids:?}");
        // the plain shuffle leaves them out too
        let (_, v) = e.call("POST", "/api/library/shuffle", Some(json!({ "limit": 200 }))).await;
        let ids = Env::ids(&v);
        assert!(!ids.contains(&11) && !ids.iter().any(|i| (21..=26).contains(i)), "{ids:?}");
    }
    // a shuffled list loses its disliked songs too
    let (_, v) = e.call("POST", "/api/taste/shuffle", Some(json!({ "ids": [11, 12, 13, 21] }))).await;
    let mut ids: Vec<i64> = v.as_array().unwrap().iter().map(|x| x.as_i64().unwrap()).collect();
    ids.sort();
    assert_eq!(ids, vec![12, 13]);
    // a genre shuffle of several genres
    let (s, v) = e.call("POST", "/api/taste/shuffle", Some(json!({ "genres": ["Jazz", "polka"], "limit": 100 }))).await;
    assert_eq!(s, StatusCode::OK);
    let ids = Env::ids(&v);
    assert_eq!(ids.len(), 18);
    assert!(ids.iter().all(|i| *i > 40));
    assert_eq!(e.call("POST", "/api/taste/shuffle", Some(json!({ "genres": ["polka"] }))).await.0, StatusCode::NOT_FOUND);
    // taken back: they come up again
    e.call("DELETE", "/api/taste/dislikes/artists/Artist%202", None).await;
    e.call("DELETE", "/api/taste/dislikes/tracks/11", None).await;
    let (_, v) = e.call("POST", "/api/taste/shuffle", Some(json!({ "limit": 200 }))).await;
    assert_eq!(Env::ids(&v).len(), 36);
}

#[tokio::test]
async fn a_dislike_and_a_like_undo_each_other() {
    let e = setup("opposites").await;
    e.call("POST", "/api/likes/15", None).await;
    e.call("PUT", "/api/taste/dislikes/tracks/15", None).await;
    let (_, likes) = e.call("GET", "/api/likes", None).await;
    assert_eq!(likes, json!([]));
    e.call("POST", "/api/likes/15", None).await;
    let (_, d) = e.call("GET", "/api/taste/dislikes", None).await;
    assert_eq!(d["tracks"], json!([]));
    assert_eq!(e.call("PUT", "/api/taste/dislikes/tracks/999", None).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn skips_are_saved() {
    let e = setup("skips").await;
    let (_, v) = e.call("POST", "/api/taste/skip", Some(json!({ "track_id": 31, "seconds": 0.5 }))).await;
    assert_eq!(v["counted"], json!(false));
    let (_, v) = e.call("POST", "/api/taste/skip", Some(json!({ "track_id": 31, "seconds": 12 }))).await;
    assert_eq!(v["counted"], json!(true));
    let (_, v) = e.call("POST", "/api/taste/skip", Some(json!({ "track_id": 9999, "seconds": 12 }))).await;
    assert_eq!(v["counted"], json!(false));
    let rows = e.st.db.query("SELECT track_id, seconds FROM skips WHERE user_id=?", p![e.uid]).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!((rows[0].i("track_id"), rows[0].f("seconds")), (31, 12.0));
}

#[tokio::test]
async fn mixes_follow_plays_and_likes() {
    let e = setup("mixes").await;
    let (_, v) = e.call("GET", "/api/taste/mixes", None).await;
    assert_eq!(v["cold"], json!(true));
    // a listener of the jazz artists (plays a month and a half ago) who likes two jazz songs
    let t = now() - 45.0 * 86400.0;
    for k in 4..=6 {
        for j in 1..=6 {
            e.st.db.execute("INSERT INTO plays (user_id, track_id, seconds, played_at) VALUES (?,?,?,?)", p![e.uid, k * 10 + j, 200.0, t + (k * 10 + j) as f64 * 60.0]).unwrap();
        }
    }
    e.call("POST", "/api/likes/41", None).await;
    e.call("POST", "/api/likes/51", None).await;
    let (s, v) = e.call("GET", "/api/taste/mixes?limit=10", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["cold"], json!(false));
    let mixes = v["mixes"].as_array().unwrap();
    let ids: Vec<&str> = mixes.iter().map(|m| m["id"].as_str().unwrap()).collect();
    assert!(ids.contains(&"you") && ids.contains(&"genre:jazz"), "{ids:?}");
    let you = mixes.iter().find(|m| m["id"] == "you").unwrap();
    assert_eq!(you["tracks"].as_array().unwrap().len(), 10);
    assert!(!you["covers"].as_array().unwrap().is_empty());
    // the jazz mix is jazz only
    let (_, v) = e.call("POST", "/api/taste/mix", Some(json!({ "id": "genre:jazz", "limit": 50 }))).await;
    assert!(Env::ids(&v).iter().all(|i| *i > 40), "{v}");
    assert_eq!(e.call("POST", "/api/taste/mix", Some(json!({ "id": "genre:polka" }))).await.0, StatusCode::NOT_FOUND);
    // the genres list puts jazz first, the profile names the jazz artists
    let (_, g) = e.call("GET", "/api/taste/genres", None).await;
    assert_eq!(g[0]["genre"], json!("jazz"));
    assert_eq!(g[0]["songs"], json!(18));
    let (_, p) = e.call("GET", "/api/taste/profile", None).await;
    assert!(p["artists"].as_array().unwrap().iter().all(|a| ["Artist 4", "Artist 5", "Artist 6"].contains(&a["name"].as_str().unwrap())), "{p}");
    assert_eq!(p["decades"][0]["name"], json!("1990s"));
    // more like a jazz song: jazz first
    let (s, v) = e.call("GET", "/api/taste/like-this/41?limit=8", None).await;
    assert_eq!(s, StatusCode::OK);
    let ids = Env::ids(&v);
    assert!(!ids.contains(&41));
    assert!(ids.iter().filter(|i| **i > 40).count() >= 6, "{ids:?}");
    assert_eq!(e.call("GET", "/api/taste/like-this/999", None).await.0, StatusCode::NOT_FOUND);
}
