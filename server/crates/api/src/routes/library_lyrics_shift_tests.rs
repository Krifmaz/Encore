//! The lyrics view's timing nudge through the real router: `PUT /api/library/tracks/{id}/lyrics/shift` saves
//! how much later the lines are shown, every later `GET …/lyrics` carries it as `shift_ms` (with the .lrc's own
//! `[offset:]` tag counted in), other lyrics start without a nudge, and plain lyrics can't be nudged.
use encore_db::{p, RowExt};
use crate::state::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use serde_json::{json, Value};
use std::path::PathBuf;
use tower::ServiceExt;

/// The listener's password.
const PW: &str = "listener-pass-1";

/// A server with one signed-in listener and one song whose .lrc is in a temp folder.
struct Env {
    /// the server's state (its database)
    st: AppState,
    /// the router requests go through
    app: Router,
    /// the temp folder, removed when the test ends
    dir: PathBuf,
    /// the listener's session cookie
    cookie: String,
    /// the song's .lrc file
    lrc: PathBuf,
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// The server, the listener signed in, and song 1 with time-synced lyrics saved next to it.
async fn setup(name: &str) -> Env {
    let dir = std::env::temp_dir().join(format!("encore-lyrics-shift-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let st = crate::test_support::state_in(&dir);
    crate::auth::create_user(&st, "listener", PW, "user", None, None).unwrap();
    let app = crate::app(st.clone());
    let req = Request::builder().method("POST").uri("/api/auth/login").header("x-requested-with", "encore").header("content-type", "application/json")
        .body(Body::from(json!({ "username": "listener", "password": PW }).to_string())).unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let cookie = resp.headers().get("set-cookie").and_then(|v| v.to_str().ok()).unwrap_or("").split(';').next().unwrap().to_string();
    let song = dir.join("1.flac");
    std::fs::write(&song, "audio").unwrap();
    let lrc = song.with_extension("lrc");
    std::fs::write(&lrc, "[00:01.00] one\n[00:02.00] two\n").unwrap();
    st.db.execute("INSERT INTO tracks (id, path, title, artist, album, album_id, format, duration, added_at) VALUES (?,?,?,?,?,?,?,?,?)",
        p![1, song.to_string_lossy(), "Song", "Tester", "Tones", "tester|tones", "flac", 200.0, 1.0]).unwrap();
    Env { st, app, dir, cookie, lrc }
}

impl Env {
    /// One request as the listener → (status, JSON body or null).
    async fn call(&self, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        let req = Request::builder().method(method).uri(uri).header("x-requested-with", "encore").header("content-type", "application/json")
            .header("cookie", &self.cookie).body(body.map(|b| Body::from(b.to_string())).unwrap_or_else(Body::empty)).unwrap();
        let resp = self.app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    /// `shift_ms` of song 1's lyrics as `GET …/lyrics?fetch=false` answers (no LRCLIB in tests).
    async fn shift(&self) -> Value {
        self.call("GET", "/api/library/tracks/1/lyrics?fetch=false", None).await.1["shift_ms"].clone()
    }

    /// Nudge song 1's lyrics to `ms` → (status, answer).
    async fn nudge(&self, ms: i64) -> (StatusCode, Value) {
        self.call("PUT", "/api/library/tracks/1/lyrics/shift", Some(json!({ "shift_ms": ms }))).await
    }
}

/// A nudge is saved, shown in every answer, clamped, and 0 takes it away.
#[tokio::test]
async fn a_nudge_is_saved_and_shown() {
    let e = setup("saved").await;
    assert_eq!(e.shift().await, 0);
    let (s, b) = e.nudge(-750).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["shift_ms"], -750);
    assert_eq!(b["lyrics"], "[00:01.00] one\n[00:02.00] two\n");
    assert_eq!(e.shift().await, -750);
    assert_eq!(e.nudge(99_999).await.1["shift_ms"], encore_library::lyrics::SHIFT_MAX_MS);
    assert_eq!(e.nudge(0).await.1["shift_ms"], 0);
    assert!(e.st.db.one("SELECT 1 FROM lyrics_shift", p![]).unwrap().is_none());
}

/// The nudge belongs to the lyrics it was made on: new lyrics start at 0; the .lrc's own `[offset:]` tag counts
/// in the total and stays in the file.
#[tokio::test]
async fn other_lyrics_start_fresh_and_offset_tags_count() {
    let e = setup("fresh").await;
    assert_eq!(e.nudge(500).await.0, StatusCode::OK);
    std::fs::write(&e.lrc, "[offset:+1000]\n[00:03.00] other\n").unwrap();
    // [offset:+1000] shows the lines a second sooner
    assert_eq!(e.shift().await, -1000);
    // the view sends the total it shows; the tag's part isn't saved again
    assert_eq!(e.nudge(-500).await.1["shift_ms"], -500);
    assert_eq!(e.st.db.one("SELECT shift_ms FROM lyrics_shift", p![]).unwrap().unwrap().i("shift_ms"), 500);
    assert!(std::fs::read_to_string(&e.lrc).unwrap().starts_with("[offset:+1000]"));
}

/// Plain lyrics have no timing to nudge.
#[tokio::test]
async fn plain_lyrics_cannot_be_nudged() {
    let e = setup("plain").await;
    std::fs::remove_file(&e.lrc).unwrap();
    std::fs::write(e.lrc.with_extension("txt"), "one\ntwo").unwrap();
    assert_eq!(e.shift().await, 0);
    assert_eq!(e.nudge(500).await.0, StatusCode::CONFLICT);
}
