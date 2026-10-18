//! POST /api/library/plays through the real router: the single form `{track_id, seconds}` as it always was,
//! the list an offline client sends when the server is back (`{plays: [...]}` or the bare array) with each
//! play's own time, `played_at` clamped to the last 400 days, unknown songs and plays sent twice skipped, and
//! at most 500 plays in one call.
use crate::db::{now, RowExt};
use crate::p;
use crate::state::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use serde_json::{json, Value};
use std::path::PathBuf;
use tower::ServiceExt;

const PW: &str = "listener-pass-1";
const DAY: f64 = 86400.0;

struct Env {
    st: AppState,
    app: Router,
    dir: PathBuf,
    cookie: String,
    uid: i64,
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn setup(name: &str) -> Env {
    let dir = std::env::temp_dir().join(format!("encore-plays-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let st = {
        let _env = crate::TEST_ENV.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("ENCORE_CONFIG_DIR", &dir);
        crate::build_state().unwrap()
    };
    let uid = crate::auth::create_user(&st, "listener", PW, "user", None, None).unwrap();
    let app = crate::app(st.clone());
    let req = Request::builder().method("POST").uri("/api/auth/login").header("x-requested-with", "encore").header("content-type", "application/json")
        .body(Body::from(json!({ "username": "listener", "password": PW }).to_string())).unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let cookie = resp.headers().get("set-cookie").and_then(|v| v.to_str().ok()).unwrap_or("").split(';').next().unwrap().to_string();
    for id in 1..=3 {
        st.db.execute("INSERT INTO tracks (id, path, title, artist, album, album_id, format, duration, added_at) VALUES (?,?,?,?,?,?,?,?,?)",
            p![id, format!("/music/{id}.flac"), format!("Song {id}"), "Tester", "Tones", "tester|tones", "flac", 200.0, 1.0]).unwrap();
    }
    Env { st, app, dir, cookie, uid }
}

impl Env {
    async fn post(&self, body: Value) -> (StatusCode, Value) {
        let req = Request::builder().method("POST").uri("/api/library/plays").header("x-requested-with", "encore").header("content-type", "application/json")
            .header("cookie", &self.cookie).body(Body::from(body.to_string())).unwrap();
        let resp = self.app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let body = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
    }
    /// This user's plays, oldest first: (track, seconds, played_at, title).
    fn plays(&self) -> Vec<(i64, f64, f64, String)> {
        self.st.db.query("SELECT track_id, seconds, played_at, title FROM plays WHERE user_id=? ORDER BY played_at, id", p![self.uid]).unwrap()
            .iter().map(|r| (r.i("track_id"), r.f("seconds"), r.f("played_at"), r.s("title"))).collect()
    }
}

#[tokio::test]
async fn the_single_form_is_unchanged() {
    let e = setup("single").await;
    let before = now();
    let (s, b) = e.post(json!({ "track_id": 2, "seconds": 185.5 })).await;
    assert_eq!((s, b), (StatusCode::OK, json!({ "ok": true })));
    let rows = e.plays();
    assert_eq!(rows.len(), 1);
    let (tid, secs, at, title) = &rows[0];
    assert_eq!((*tid, *secs, title.as_str()), (2, 185.5, "Song 2"));
    assert!(*at >= before - 1.0 && *at <= now() + 1.0, "saved now: {at}");
    // seconds are clamped to 0..2 h, and may be left out
    assert_eq!(e.post(json!({ "track_id": 1, "seconds": 99_999 })).await.0, StatusCode::OK);
    assert_eq!(e.post(json!({ "track_id": 1 })).await.0, StatusCode::OK);
    let secs: Vec<f64> = e.plays().iter().filter(|r| r.0 == 1).map(|r| r.1).collect();
    assert_eq!(secs, vec![7200.0, 0.0]);
    // an unknown song is still a 404 in the single form, and nothing is saved
    let (s, b) = e.post(json!({ "track_id": 999, "seconds": 10 })).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_eq!(b["detail"], "Track not found");
    assert_eq!(e.plays().len(), 3);
    // a body that is no play at all
    assert_eq!(e.post(json!({ "seconds": 10 })).await.0, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn the_list_form_saves_each_play_at_its_own_time() {
    let e = setup("list").await;
    let t = now().floor();
    let (s, b) = e.post(json!({ "plays": [
        { "track_id": 1, "seconds": 180, "played_at": t - 3.0 * DAY },
        { "track_id": 2, "seconds": 200.5, "played_at": t - 2.0 * DAY + 0.25 },
        { "track_id": 3, "seconds": 60 },
    ] })).await;
    assert_eq!((s, b), (StatusCode::OK, json!({ "ok": true, "saved": 3, "skipped": 0 })));
    let rows = e.plays();
    assert_eq!(rows.len(), 3);
    assert_eq!((rows[0].0, rows[0].1, rows[0].2, rows[0].3.as_str()), (1, 180.0, t - 3.0 * DAY, "Song 1"));
    assert_eq!((rows[1].0, rows[1].1, rows[1].2), (2, 200.5, t - 2.0 * DAY + 0.25), "fractions of a second are kept");
    assert_eq!(rows[2].0, 3);
    assert!((rows[2].2 - now()).abs() < 5.0, "no played_at: now");
    // the bare array is the same list
    let (s, b) = e.post(json!([{ "track_id": 2, "seconds": 30, "played_at": t - DAY }])).await;
    assert_eq!((s, b), (StatusCode::OK, json!({ "ok": true, "saved": 1, "skipped": 0 })));
    assert_eq!(e.plays()[2].2, t - DAY);
    // an empty list is fine
    let (s, b) = e.post(json!({ "plays": [] })).await;
    assert_eq!((s, b), (StatusCode::OK, json!({ "ok": true, "saved": 0, "skipped": 0 })));
    // the times show where they belong: the song played a day ago is the newest of the dated ones
    let req = Request::builder().uri("/api/library/plays/recent").header("x-requested-with", "encore").header("cookie", &e.cookie).body(Body::empty()).unwrap();
    let resp = e.app.clone().oneshot(req).await.unwrap();
    let recent: Value = serde_json::from_slice(&axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap()).unwrap();
    let ids: Vec<i64> = recent.as_array().unwrap().iter().map(|r| r["id"].as_i64().unwrap()).collect();
    assert_eq!(ids, vec![3, 2, 1], "{recent}");
    // a list that is not a list, and a play without a song
    assert_eq!(e.post(json!({ "plays": { "track_id": 1 } })).await.0, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(e.post(json!({ "plays": [{ "track_id": 1 }, { "seconds": 3 }] })).await.0, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(e.plays().len(), 4, "a bad list saves nothing");
}

#[tokio::test]
async fn played_at_is_clamped_to_the_last_400_days() {
    let e = setup("clamp").await;
    let (s, b) = e.post(json!({ "plays": [
        { "track_id": 1, "seconds": 100, "played_at": now() - 900.0 * DAY },
        { "track_id": 2, "seconds": 100, "played_at": now() + 30.0 * DAY },
        { "track_id": 3, "seconds": 100, "played_at": -5 },
    ] })).await;
    assert_eq!((s, b), (StatusCode::OK, json!({ "ok": true, "saved": 3, "skipped": 0 })));
    let at = |tid: i64| e.plays().iter().find(|r| r.0 == tid).unwrap().2;
    let n = now();
    assert!((at(1) - (n - 400.0 * DAY)).abs() < 5.0, "too old: 400 days ago, {}", (n - at(1)) / DAY);
    assert!((at(2) - n).abs() < 5.0, "in the future: now");
    assert!((at(3) - (n - 400.0 * DAY)).abs() < 5.0, "before 1970: 400 days ago");
    // the edge itself is kept as sent
    let edge = (now() - 399.0 * DAY).floor();
    e.post(json!({ "plays": [{ "track_id": 1, "seconds": 1, "played_at": edge }] })).await;
    assert!(e.plays().iter().any(|r| r.2 == edge));
}

#[tokio::test]
async fn unknown_songs_and_plays_sent_twice_are_skipped() {
    let e = setup("skipped").await;
    let t = now().floor() - 3600.0;
    let list = json!({ "plays": [
        { "track_id": 1, "seconds": 100, "played_at": t },
        { "track_id": 404, "seconds": 100, "played_at": t + 1.0 },
        { "track_id": 2, "seconds": 100, "played_at": t + 2.0 },
        { "track_id": -1, "seconds": 100 },
    ] });
    let (s, b) = e.post(list.clone()).await;
    assert_eq!((s, b), (StatusCode::OK, json!({ "ok": true, "saved": 2, "skipped": 2 })));
    assert_eq!(e.plays().iter().map(|r| r.0).collect::<Vec<_>>(), vec![1, 2]);
    // the same list again (the answer was lost on the way): nothing new
    let (s, b) = e.post(list).await;
    assert_eq!((s, b), (StatusCode::OK, json!({ "ok": true, "saved": 0, "skipped": 4 })));
    assert_eq!(e.plays().len(), 2);
    // the same play twice in one list counts once; the same song at another time is another play
    let (_, b) = e.post(json!([
        { "track_id": 3, "seconds": 50, "played_at": t + 10.0 },
        { "track_id": 3, "seconds": 50, "played_at": t + 10.0 },
        { "track_id": 1, "seconds": 100, "played_at": t + 20.0 },
    ])).await;
    assert_eq!(b, json!({ "ok": true, "saved": 2, "skipped": 1 }));
    assert_eq!(e.plays().len(), 4);
    // plays without a time are never taken for a repeat: each is a new play now
    for _ in 0..2 {
        e.post(json!([{ "track_id": 3, "seconds": 5 }])).await;
    }
    assert_eq!(e.plays().iter().filter(|r| r.0 == 3).count(), 3);
    // another listener's play at the same time is theirs, not a repeat of this one
    let other = crate::auth::create_user(&e.st, "other", PW, "user", None, None).unwrap();
    e.st.db.execute("DELETE FROM plays WHERE user_id=?", p![e.uid]).unwrap();
    e.st.db.execute("INSERT INTO plays (user_id, track_id, seconds, played_at) VALUES (?,?,?,?)", p![other, 1, 100.0, t]).unwrap();
    let (_, b) = e.post(json!([{ "track_id": 1, "seconds": 100, "played_at": t }])).await;
    assert_eq!(b, json!({ "ok": true, "saved": 1, "skipped": 0 }));
}

#[tokio::test]
async fn at_most_500_plays_in_one_call() {
    let e = setup("cap").await;
    let t = now().floor() - DAY;
    let list = |n: usize| (0..n).map(|i| json!({ "track_id": 1 + (i % 3) as i64, "seconds": 30, "played_at": t + i as f64 })).collect::<Vec<_>>();
    let (s, b) = e.post(json!({ "plays": list(501) })).await;
    assert_eq!(s, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(b["detail"], "At most 500 plays in one call");
    assert_eq!(e.post(json!(list(501))).await.0, StatusCode::PAYLOAD_TOO_LARGE, "the bare array too");
    assert!(e.plays().is_empty(), "nothing saved");
    let (s, b) = e.post(json!({ "plays": list(500) })).await;
    assert_eq!((s, b), (StatusCode::OK, json!({ "ok": true, "saved": 500, "skipped": 0 })));
    let rows = e.plays();
    assert_eq!(rows.len(), 500);
    assert_eq!((rows[0].2, rows[499].2), (t, t + 499.0));
    assert_eq!(rows.iter().filter(|r| r.0 == 2).count(), 167);
}
