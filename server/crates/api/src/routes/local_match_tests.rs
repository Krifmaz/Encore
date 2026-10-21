//! POST /api/library/match-local through the real router: a device's songs found by MusicBrainz id, ISRC
//! and title, a remix never standing in for the original, and the keys sent back as they came.
use crate::state::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use encore_db::p;
use serde_json::{json, Value};
use tower::ServiceExt;

const PW: &str = "local-match-pass-1";

struct Env {
    st: AppState,
    app: Router,
    dir: std::path::PathBuf,
    cookie: String,
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn setup(name: &str) -> Env {
    let dir = std::env::temp_dir().join(format!("encore-local-match-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let st = crate::test_support::state_in(&dir);
    crate::auth::create_user(&st, "local", PW, "user", None, None).unwrap();
    let app = crate::app(st.clone());
    let req = Request::builder().method("POST").uri("/api/auth/login").header("x-requested-with", "encore").header("content-type", "application/json")
        .body(Body::from(json!({ "username": "local", "password": PW }).to_string())).unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let cookie = resp.headers().get("set-cookie").and_then(|v| v.to_str().ok()).unwrap_or("").split(';').next().unwrap().to_string();
    Env { st, app, dir, cookie }
}

impl Env {
    fn song(&self, id: i64, title: &str, artist: &str, secs: f64, isrc: &str, mbid: &str) {
        let aid = encore_library::album_key(artist, "Album", "");
        self.st.db.execute(
            "INSERT INTO tracks (id, path, title, artist, album, albumartist, album_id, format, bitrate, duration, size, year, disc, track, added_at, isrc, mb_trackid)
             VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            p![id, format!("/m/{id}.flac"), title, artist, "Album", artist, aid, "flac", 900, secs, 1000, "2020", 1, id, 0.0, isrc, mbid]).unwrap();
    }
    async fn post(&self, body: Value) -> (StatusCode, Value) {
        let req = Request::builder().method("POST").uri("/api/library/match-local").header("x-requested-with", "encore")
            .header("content-type", "application/json").header("cookie", &self.cookie).body(Body::from(body.to_string())).unwrap();
        let resp = self.app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 10 << 20).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }
}

#[tokio::test]
async fn device_songs_are_found_once_and_versions_kept_apart() {
    let e = setup("versions").await;
    e.song(1, "Hold On", "Alpha", 200.0, "", "mb-hold-on");
    e.song(2, "Runaway", "Beta", 240.0, "USABC1234567", "");
    e.song(3, "Midnight", "Gamma", 180.0, "", "");
    let (status, v) = e.post(json!({ "songs": [
        // the MusicBrainz recording wins over a differently spelled title
        { "key": "a.flac", "title": "Hold On (2011 Remaster)", "artist": "alpha", "mbid": "mb-hold-on" },
        { "key": "b.mp3", "title": "Something else", "artist": "Beta", "isrc": "us-abc-12-34567" },
        { "key": "c.m4a", "title": "Midnight", "artist": "Gamma feat. Delta", "duration": 181.0 },
        // a remix is another song
        { "key": "d.flac", "title": "Midnight (Club Remix)", "artist": "Gamma", "duration": 181.0 },
        { "key": "e.flac", "title": "Not in the library", "artist": "Nobody" },
        { "key": "", "title": "No key", "artist": "Alpha" },
    ] })).await;
    assert_eq!(status, StatusCode::OK, "{v:?}");
    let m = v["matches"].as_object().unwrap();
    assert_eq!(m["a.flac"]["id"], 1);
    assert_eq!(m["b.mp3"]["id"], 2);
    assert_eq!(m["c.m4a"]["id"], 3);
    assert_eq!(m["c.m4a"]["title"], "Midnight");
    assert!(!m.contains_key("d.flac"), "{m:?}");
    assert!(!m.contains_key("e.flac"));
    assert_eq!(m.len(), 3);
}

#[tokio::test]
async fn too_many_songs_at_once_are_refused() {
    let e = setup("limit").await;
    let songs: Vec<Value> = (0..=super::MAX_SONGS).map(|i| json!({ "key": i.to_string(), "title": "x", "artist": "y" })).collect();
    let (status, _) = e.post(json!({ "songs": songs })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
