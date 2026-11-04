//! Play anything owned, live request labels: status events on every change, owned songs of an album.
use encore_db::p;
use crate::state::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use serde_json::{json, Value};
use tower::ServiceExt;

const PW: &str = "labels-pass-1";

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
    let dir = std::env::temp_dir().join(format!("encore-labels-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let st = crate::test_support::state_in(&dir);
    crate::auth::create_user(&st, "labeller", PW, "user", None, None).unwrap();
    let app = crate::app(st.clone());
    let (status, _, cookie) = call(&app, "POST", "/api/auth/login", "", Some(json!({ "username": "labeller", "password": PW }))).await;
    assert_eq!(status, StatusCode::OK);
    Env { st, app, dir, cookie }
}

async fn call(app: &Router, method: &str, uri: &str, cookie: &str, body: Option<Value>) -> (StatusCode, Value, String) {
    let mut b = Request::builder().method(method).uri(uri).header("x-requested-with", "encore");
    if !cookie.is_empty() {
        b = b.header("cookie", cookie);
    }
    let body = match body {
        Some(v) => {
            b = b.header("content-type", "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    let resp = app.clone().oneshot(b.body(body).unwrap()).await.unwrap();
    let status = resp.status();
    let set = resp.headers().get("set-cookie").and_then(|v| v.to_str().ok()).unwrap_or("").split(';').next().unwrap_or("").to_string();
    let bytes = axum::body::to_bytes(resp.into_body(), 10 << 20).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null), set)
}

/// The next event about request `id` (other tests share the channel).
async fn next_for(rx: &mut tokio::sync::broadcast::Receiver<(i64, Value)>, id: i64) -> Value {
    let wait = async {
        loop {
            let (_, v) = rx.recv().await.unwrap();
            if v["request_id"] == id {
                return v;
            }
        }
    };
    tokio::time::timeout(std::time::Duration::from_secs(5), wait).await.expect("no event for the request")
}

#[tokio::test]
async fn status_events_on_every_change() {
    let e = setup("events").await;
    let mut rx = e.st.requests.events.subscribe();
    let (s, r, _) = call(&e.app, "POST", "/api/requests", &e.cookie,
        Some(json!({ "kind": "album", "mbid": "rg-rodeo-1", "artist": "Travis Scott", "title": "Rodeo", "album": "Rodeo" }))).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    let id = r["id"].as_i64().unwrap();
    let ev = next_for(&mut rx, id).await;
    assert!(["queued", "pending"].contains(&ev["status"].as_str().unwrap()), "{ev}");
    let keys: Vec<&str> = ev["keys"].as_array().unwrap().iter().filter_map(|k| k.as_str()).collect();
    assert!(keys.contains(&"rg:rg-rodeo-1") && keys.contains(&"a:travisscott|rodeo"), "{keys:?}");

    // the worker's steps: searching → downloading → importing → in library
    for (status, label) in [("searching", "Searching"), ("downloading", "Downloading"), ("importing", "Importing"), ("completed", "In library")] {
        e.st.db.execute("UPDATE requests SET status=? WHERE id=?", p![status, id]).unwrap();
        encore_requests::events::emit(&e.st, id);
        let ev = next_for(&mut rx, id).await;
        assert_eq!((ev["status"].as_str().unwrap(), ev["label"].as_str().unwrap()), (status, label));
    }
    e.st.db.execute("UPDATE requests SET status='failed', message='All sources exhausted (slskd: no results)' WHERE id=?", p![id]).unwrap();
    encore_requests::events::emit(&e.st, id);
    assert_eq!(next_for(&mut rx, id).await["label"], "Not found");

    // retry and cancel through the API emit too
    let (s, _, _) = call(&e.app, "POST", &format!("/api/requests/{id}/cancel"), &e.cookie, None).await;
    assert_eq!(s, StatusCode::OK);
    // failed can't be cancelled; a new song request can
    let (_, r2, _) = call(&e.app, "POST", "/api/requests", &e.cookie,
        Some(json!({ "kind": "track", "artist": "Travis Scott", "title": "90210", "isrc": "USSM11504270" }))).await;
    let id2 = r2["id"].as_i64().unwrap();
    assert_eq!(next_for(&mut rx, id2).await["kind"], "track");
    let (s, _, _) = call(&e.app, "POST", &format!("/api/requests/{id2}/cancel"), &e.cookie, None).await;
    assert_eq!(s, StatusCode::OK);
    let ev = next_for(&mut rx, id2).await;
    assert_eq!(ev["status"], "cancelled");
    assert!(ev["keys"].as_array().unwrap().contains(&json!("t:travisscott|90210")), "{ev}");
}

#[tokio::test]
async fn album_plays_when_only_some_songs_are_owned() {
    let e = setup("owned").await;
    // two of the four songs, under another album name (a playlist rip), one only matched by ISRC
    e.st.db.execute("INSERT INTO tracks (id, path, title, artist, album, albumartist, album_id, format, duration, isrc, added_at)
                     VALUES (1, '/m/1.flac', '90210', 'Travis Scott', 'Best of 2015', 'Various Artists', 'alb1', 'flac', 339.0, NULL, 1),
                            (2, '/m/2.flac', 'Antidote (Radio Edit)', 'Travis Scott', 'Best of 2015', 'Various Artists', 'alb1', 'flac', 262.0, 'USSM11505412', 2)", p![]).unwrap();
    let mut tracks = vec![
        json!({ "recording_id": "rec-pornography", "title": "Pornography", "artist": "Travis Scott", "length": 225.0, "isrc": null }),
        json!({ "recording_id": "rec-90210", "title": "90210", "artist": "Travis Scott", "length": 339.4, "isrc": null }),
        json!({ "recording_id": "rec-antidote", "title": "Antidote", "artist": "Travis Scott", "length": 262.0, "isrc": "USSM1-1505412" }),
        json!({ "recording_id": "rec-3500", "title": "3500", "artist": "Travis Scott", "length": 461.0, "isrc": null }),
    ];
    // a song request on the album shows its label
    e.st.db.execute("INSERT INTO requests (user_id, kind, mbid, artist, title, status, detail, created_at, updated_at) VALUES (1, 'track', 'rec-3500', 'Travis Scott', '3500', 'downloading', '{}', 1, 1)", p![]).unwrap();
    crate::routes::discover::annotate_album_tracks(&e.st, &mut tracks);
    let owned: Vec<bool> = tracks.iter().map(|t| t["in_library"] == true).collect();
    assert_eq!(owned, vec![false, true, true, false]);
    assert_eq!(tracks[1]["library"]["id"], 1);
    assert_eq!(tracks[2]["library"]["id"], 2);
    assert_eq!(tracks[3]["request_status"], "downloading");
    assert_eq!(tracks[3]["request_label"], "Downloading");
    assert!(tracks[0]["request_status"].is_null());

    // ext (Deezer / Apple) songs: same ISRC, else title + artist within 3 s
    assert!(encore_library::catalog::library_track_for(&e.st.db, "USSM11505412", "Someone", "Other", None).is_some());
    assert!(encore_library::catalog::library_track_for(&e.st.db, "", "travis scott", "90210", Some(340.0)).is_some());
    assert!(encore_library::catalog::library_track_for(&e.st.db, "", "Travis Scott", "90210", Some(200.0)).is_none());
}

#[tokio::test]
async fn request_event_stream_opens() {
    let e = setup("sse").await;
    let req = Request::builder().method("GET").uri("/api/requests/events").header("x-requested-with", "encore").header("cookie", &e.cookie).body(Body::empty()).unwrap();
    let resp = e.app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(resp.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").starts_with("text/event-stream"));
    let (s, _, _) = call(&e.app, "GET", "/api/requests/events", "", None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
}
