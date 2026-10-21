//! The enrichment fixtures: answers of the pages people open most, captured from the 0.28 server (when
//! two middlewares still rewrote every JSON body) on the P4 sample library, signed in as its admin, who has
//! translations on. `fixtures/enrich/*.json` are those answers byte for byte (pretty, keys sorted, the data
//! folder written `$WORK`); `fixtures/enrich/library.sql` is the sample library's songs, requests, plays,
//! translations and Liked Songs playlist. The test builds the same library, does what the capture did
//! (likes four songs, makes the playlist "P4 sample"), asks the same URLs and compares the canonical text
//! (serde_json, keys sorted, pretty) with the fixture, after putting these in both:
//! - `$WORK` for the test's folder,
//! - one placeholder for the values that carry a time (the keys compare.py skips: added_at, created_at …),
//! - one placeholder for `cover_v` (a hash of the cover file's path and mtime, so it differs per folder),
//! - floats to 15 significant digits: 0.28 parsed each body again with serde_json's fast float reader, which
//!   can land one unit in the last place away from the stored value; the new path writes the stored value.
//!
//! The covers' colours come from the captured answers (the cover pass found them on the real images).
use crate::state::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use tower::ServiceExt;

/// (fixture name, URL): the order and URLs of the capture (p4/tools/capture.py).
const CASES: &[(&str, &str)] = &[
    ("home_recent", "/api/library/recent?limit=12"),
    ("home_requests_active", "/api/requests?status=active&mine=true&limit=8"),
    ("home_made_for_you", "/api/discover/made-for-you"),
    ("library_albums", "/api/library/albums?q=&sort=&dir=&kind=albums&filter=&artist=&limit=120&offset=0"),
    ("library_album", "/api/library/albums/150c2e63e67a8f86"),
    ("library_album_japanese", "/api/library/albums/52b52b2323b5df9d"),
    ("library_artists", "/api/library/artists?q=&sort=&dir="),
    ("library_artist_albums", "/api/library/albums?q=&sort=&dir=&kind=albums&filter=&artist=Beta%20Band&limit=120&offset=0"),
    ("library_artist_tracks", "/api/library/artist-tracks?name=Beta%20Band"),
    ("discover_artist_library_tracks", "/api/discover/artist/none/library-tracks?name=Beta%20Band"),
    ("library_songs", "/api/library/tracks?q=&sort=&dir=&limit=200&offset=0"),
    ("playlists", "/api/playlists"),
    ("playlist_sample", "/api/playlists/2"),
    ("playlist_liked", "/api/playlists/1"),
    ("requests", "/api/requests?limit=300"),
    ("requests_mine", "/api/requests?mine=true&limit=100"),
    ("for_you", "/api/discover/for-you"),
    ("library_search", "/api/library/search?q=beta&limit=12"),
];

/// Keys whose values carry a time (compare.py's list).
const TIMES: &[&str] = &["added_at", "created_at", "updated_at", "last_login", "last_scan", "played_at", "time", "ms", "free", "used"];

const PASSWORD: &str = "adminpass1234";

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src/fixtures/enrich")
}

fn fixture(name: &str) -> String {
    std::fs::read_to_string(fixtures().join(format!("{name}.json"))).unwrap()
}

/// album → "#rrggbb" wherever a captured answer shows a cover colour.
fn captured_colors() -> BTreeMap<String, String> {
    fn walk(v: &Value, out: &mut BTreeMap<String, String>) {
        match v {
            Value::Object(o) => {
                let album = o.get("album_id").or_else(|| o.get("cover_album")).or_else(|| o.get("id").filter(|i| i.is_string())).and_then(|a| a.as_str());
                if let (Some(a), Some(c)) = (album, o.get("cover_color").and_then(|c| c.as_str())) {
                    out.insert(a.to_string(), c.to_string());
                }
                o.values().for_each(|x| walk(x, out));
            }
            Value::Array(a) => a.iter().for_each(|x| walk(x, out)),
            _ => {}
        }
    }
    let mut out = BTreeMap::new();
    for (name, _) in CASES {
        walk(&serde_json::from_str(&fixture(name)).unwrap(), &mut out);
    }
    out
}

pub(crate) struct Sample {
    pub st: AppState,
    pub app: Router,
    pub work: PathBuf,
    pub cookie: String,
}

impl Drop for Sample {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.work);
    }
}

pub(crate) async fn call(app: &Router, method: &str, uri: &str, cookie: &str, body: Option<Value>) -> (StatusCode, Vec<u8>) {
    let mut b = Request::builder().method(method).uri(uri).header("x-requested-with", "encore");
    if !cookie.is_empty() {
        b = b.header("cookie", cookie);
    }
    let req = match body {
        Some(v) => b.header("content-type", "application/json").body(Body::from(v.to_string())).unwrap(),
        None => b.body(Body::empty()).unwrap(),
    };
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    (status, axum::body::to_bytes(resp.into_body(), 32 << 20).await.unwrap().to_vec())
}

/// The P4 sample library in a folder of its own, its covers looked at, signed in as its admin, with the
/// capture's likes and playlist.
pub(crate) async fn sample(name: &str) -> Sample {
    let work = std::env::temp_dir().join(format!("encore-enrich-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(work.join("music")).unwrap();
    let work = work.canonicalize().unwrap();
    let w = work.to_string_lossy().to_string();
    let st = crate::test_support::TestServer::in_dir(&work.join("cfg")).with(json!({ "library_path": format!("{w}/music") })).st;
    // a new server makes its owner "admin" (id 1, a random password): the sample's admin, with its password
    // and its language settings
    st.db.execute("UPDATE users SET password_hash=?, prefs=? WHERE id=1 AND username='admin' AND role='owner'",
        encore_db::p![crate::auth::hash_password(PASSWORD), r#"{"translate_lang":"en","translate_mode":"under"}"#]).unwrap();
    let sql = std::fs::read_to_string(fixtures().join("library.sql")).unwrap().replace("$WORK", &w);
    st.db.execute_batch(&sql).unwrap();
    // every album folder in the library holds a cover.jpg, as in the sample data (the older rows under
    // music-live have none)
    let rows = st.db.query("SELECT path, album_id FROM tracks", encore_db::p![]).unwrap();
    let mut albums: Vec<String> = vec![];
    for r in &rows {
        let p = r["path"].as_str().unwrap();
        let dir = Path::new(p).parent().unwrap();
        std::fs::create_dir_all(dir).unwrap();
        if p.starts_with(&format!("{w}/music/")) && !dir.join("cover.jpg").exists() {
            std::fs::write(dir.join("cover.jpg"), b"cover").unwrap();
        }
        let a = r["album_id"].as_str().unwrap().to_string();
        if !albums.contains(&a) {
            albums.push(a);
        }
    }
    // the cover pass had looked at every album before the capture: versions known, colours found
    let colors = captured_colors();
    for a in &albums {
        let (_, v) = st.library.covers.get(&st.db, a).unwrap_or_else(|| panic!("no cover for {a}"));
        if let Some(c) = colors.get(a) {
            st.db.execute("INSERT OR REPLACE INTO cover_color (album_id, v, color) VALUES (?, ?, ?)", encore_db::p![a, v as i64, c]).unwrap();
        }
    }
    let app = crate::app(st.clone());
    let (status, _) = call(&app, "GET", "/api/health", "", None).await;
    assert_eq!(status, StatusCode::OK);
    let req = Request::builder().method("POST").uri("/api/auth/login").header("x-requested-with", "encore").header("content-type", "application/json")
        .body(Body::from(json!({ "username": "admin", "password": PASSWORD }).to_string())).unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let cookie = resp.headers().get("set-cookie").and_then(|v| v.to_str().ok()).unwrap().split(';').next().unwrap().to_string();
    // what capture.py does once per data copy
    for tid in [28, 13, 22, 31] {
        let (s, b) = call(&app, "POST", &format!("/api/likes/{tid}"), &cookie, None).await;
        assert_eq!(s, StatusCode::OK, "{}", String::from_utf8_lossy(&b));
    }
    let (s, b) = call(&app, "POST", "/api/playlists", &cookie, Some(json!({ "name": "P4 sample" }))).await;
    assert_eq!(s, StatusCode::OK, "{}", String::from_utf8_lossy(&b));
    let pid = serde_json::from_slice::<Value>(&b).unwrap()["id"].as_i64().unwrap();
    assert_eq!(pid, 2);
    let (s, b) = call(&app, "POST", "/api/playlists/2/tracks", &cookie, Some(json!({ "track_ids": [19, 25, 28, 32, 36] }))).await;
    assert_eq!(s, StatusCode::OK, "{}", String::from_utf8_lossy(&b));
    Sample { st, app, work, cookie }
}

/// The canonical text of an answer with the placeholders described at the top.
fn canonical(text: &str, work: &str) -> String {
    fn norm(v: &mut Value, key: Option<&str>, work: &str) {
        if let Some(k) = key {
            if TIMES.contains(&k) && !v.is_null() {
                *v = json!("<time>");
                return;
            }
            if k == "cover_v" {
                *v = json!("<cover_v>");
                return;
            }
        }
        match v {
            Value::Object(o) => o.iter_mut().for_each(|(k, x)| norm(x, Some(k), work)),
            Value::Array(a) => a.iter_mut().for_each(|x| norm(x, None, work)),
            Value::String(s) if !work.is_empty() && s.contains(work) => *s = s.replace(work, "$WORK"),
            Value::Number(n) if n.is_f64() => *v = json!(format!("{:.14e}", n.as_f64().unwrap())),
            _ => {}
        }
    }
    let mut v: Value = serde_json::from_str(text).unwrap_or_else(|e| panic!("not JSON ({e}): {text}"));
    norm(&mut v, None, work);
    serde_json::to_string_pretty(&v).unwrap() + "\n"
}

/// Every captured answer, through the server as it is now.
#[tokio::test]
async fn answers_match_the_captured_fixtures() {
    let s = sample("fixtures").await;
    let w = s.work.to_string_lossy().to_string();
    let mut bad = vec![];
    for (name, url) in CASES {
        let (status, body) = call(&s.app, "GET", url, &s.cookie, None).await;
        assert_eq!(status, StatusCode::OK, "{name}: {}", String::from_utf8_lossy(&body));
        let got = canonical(&String::from_utf8(body).unwrap(), &w);
        let want = canonical(&fixture(name), "");
        if got != want {
            let line = got.lines().zip(want.lines()).position(|(a, b)| a != b).unwrap_or(got.lines().count().min(want.lines().count()));
            let out = s.work.parent().unwrap().join(format!("encore-enrich-{name}.got.json"));
            std::fs::write(&out, &got).unwrap();
            bad.push(format!("{name}: line {}: got {:?}, want {:?} (whole answer in {})", line + 1, got.lines().nth(line), want.lines().nth(line), out.display()));
        }
    }
    assert!(bad.is_empty(), "{} answer(s) differ:\n{}", bad.len(), bad.join("\n"));
}

/// Every GET answer that can carry music is already enriched: running the pass on it again changes
/// nothing. A handler that forgot to call it would show here (no `cover_v`, no `artists`, no `album_t`):
/// the middlewares used to do it for every answer. The sample library gets a translation the admin
/// reads, a heartbeat with a song, a share link, a report, a suspect file, a mismatch, a torrent download,
/// a block and a bug report, so these answers carry something to enrich. Only answers that need no
/// network are here.
#[tokio::test]
async fn every_answer_is_already_enriched() {
    let s = sample("audit").await;
    let (st, app, cookie) = (&s.st, &s.app, s.cookie.as_str());
    let db = &st.db;
    encore_library::translate::store::set_manual(db, "ペルソナ4 サウンドトラック", "en", "album", Some("Persona 4 Soundtrack"), None).unwrap();
    db.execute("UPDATE tracks SET verify_status='mismatch', verify_detail='test' WHERE id=13", encore_db::p![]).unwrap();
    // loudness and audio properties (0.31): the rows' `gain`, `sample_rate`, `bit_depth` are stamped once too
    db.execute("UPDATE tracks SET rg_track_gain=-6.5, rg_track_peak=0.95, rg_album_gain=-6.0, loudness_lufs=-11.5, loudness_source='tags', sample_rate=44100, bit_depth=16
                WHERE id IN (13, 32)", encore_db::p![]).unwrap();
    db.execute("UPDATE requests SET source='torznab', status='downloading' WHERE id=4", encore_db::p![]).unwrap();
    db.execute("INSERT INTO reports (user_id, kind, track_id, album_id, reason, artist, title, album, status, created_at) VALUES
                (1, 'track', 32, '52b52b2323b5df9d', 'other', '平田志穂子', 'Reach Out To The Truth', 'ペルソナ4 サウンドトラック', 'done', 1)", encore_db::p![]).unwrap();
    db.execute("INSERT INTO source_block (kind, key, label, report_id, user_id, created_at) VALUES ('slskd', 'u|d', 'ペルソナ4 サウンドトラック', 1, 1, 1)", encore_db::p![]).unwrap();
    db.execute("INSERT INTO track_quality (track_id, status, issues, summary, checked_at) VALUES (32, 'suspect', '[]', 'cutoff', 1)", encore_db::p![]).unwrap();
    let state = json!({ "id": 32, "title": "Reach Out To The Truth", "artist": "平田志穂子", "album": "ペルソナ4 サウンドトラック", "album_id": "52b52b2323b5df9d",
                        "playing": true, "position": 1.5, "queue": [{ "id": 32, "title": "Reach Out To The Truth", "artist": "平田志穂子", "album_id": "52b52b2323b5df9d" }] });
    let (code, b) = call(app, "POST", "/api/remote/heartbeat", cookie, Some(json!({ "id": "phone", "name": "Phone", "state": state }))).await;
    assert_eq!(code, StatusCode::OK, "{}", String::from_utf8_lossy(&b));
    let (code, b) = call(app, "POST", "/api/library/albums/52b52b2323b5df9d/share", cookie, None).await;
    assert_eq!(code, StatusCode::OK, "{}", String::from_utf8_lossy(&b));
    let token = serde_json::from_slice::<Value>(&b).unwrap()["token"].as_str().unwrap().to_string();
    // the bug report tables are made on first use
    let (code, _) = call(app, "GET", "/api/bug-reports", cookie, None).await;
    assert_eq!(code, StatusCode::OK);
    db.execute("INSERT INTO bug_reports (user_id, username, kind, title, body, status, created_at, updated_at) VALUES (1, 'admin', 'bug', 'ペルソナ4 サウンドトラック', 'x', 'new', 1, 1)", encore_db::p![]).unwrap();
    // a group listen (0.31): its queue rows are enriched once per change, and the list's tiles carry the song
    let group = json!({ "device_id": "audit-desk", "device_name": "Audit", "visibility": "anyone", "everyone_controls": false,
                        "tracks": [32, 13], "index": 0, "position": 0.0, "playing": false });
    let (code, b) = call(app, "POST", "/api/group", cookie, Some(group)).await;
    assert_eq!(code, StatusCode::OK, "{}", String::from_utf8_lossy(&b));
    let urls = [
        "/api/library/recent?limit=12",
        "/api/library/albums?kind=albums&limit=120",
        "/api/library/albums?q=persona",
        "/api/library/albums?artist=%E5%B9%B3%E7%94%B0%E5%BF%97%E7%A9%82%E5%AD%90",
        "/api/library/albums/52b52b2323b5df9d",
        "/api/library/artists",
        "/api/library/artists?q=beta",
        "/api/library/tracks?limit=200",
        "/api/library/tracks?q=reach",
        "/api/library/tracks?ids=32,13,1",
        "/api/library/artist-tracks?name=%E5%B9%B3%E7%94%B0%E5%BF%97%E7%A9%82%E5%AD%90",
        "/api/library/artists/%E5%B9%B3%E7%94%B0%E5%BF%97%E7%A9%82%E5%AD%90/tracks",
        "/api/library/artists/_/liked?name=%E5%B9%B3%E7%94%B0%E5%BF%97%E7%A9%82%E5%AD%90",
        "/api/library/artists/_/one-offs?name=%E5%B9%B3%E7%94%B0%E5%BF%97%E7%A9%82%E5%AD%90",
        "/api/library/album-types",
        "/api/library/albums?type=one_off",
        "/api/library/search?q=reach",
        "/api/library/shuffle?limit=40",
        "/api/library/tracks/32",
        "/api/library/plays/recent",
        "/api/library/mismatches",
        "/api/library/storage",
        "/api/library/duplicates",
        &format!("/api/share/{token}"),
        "/api/playlists",
        "/api/playlists/1",
        "/api/playlists/2",
        "/api/requests?limit=300",
        "/api/remote/devices",
        "/api/remote/spot",
        "/api/stats/me",
        "/api/stats/leaderboard",
        "/api/system/downloads",
        "/api/contribute",
        "/api/reports",
        "/api/blocks",
        "/api/quality/suspect",
        "/api/bug-reports",
        "/api/bug-reports/1",
        "/api/discover/artist/none/library-tracks?name=%E5%B9%B3%E7%94%B0%E5%BF%97%E7%A9%82%E5%AD%90",
        "/api/discover/for-you",
        "/api/discover/made-for-you",
        "/api/group",
        "/api/group/current?device_id=audit-desk",
    ];
    let mut admin = db.one("SELECT * FROM users WHERE id=1", encore_db::p![]).unwrap().unwrap();
    admin.remove("password_hash");
    let mut stamped = 0;
    for url in urls {
        let (code, body) = call(app, "GET", url, cookie, None).await;
        assert_eq!(code, StatusCode::OK, "{url}: {}", String::from_utf8_lossy(&body));
        let got: Value = serde_json::from_slice(&body).unwrap_or_else(|e| panic!("{url}: {e}"));
        let mut again = got.clone();
        let st2 = st.clone();
        let who = admin.clone();
        let again = tokio::task::spawn_blocking(move || {
            super::enrich_now(&st2, Some(&who), &mut again);
            again
        }).await.unwrap();
        assert_eq!(again, got, "{url}: the answer was not enriched");
        let text = got.to_string();
        if text.contains("\"cover_v\"") || text.contains("\"album_t\"") || text.contains("\"artists\":[{") {
            stamped += 1;
        }
    }
    // most of them did carry something to enrich
    assert!(stamped >= 25, "only {stamped} answers carried covers, credits or translations");
    // and the translation the admin reads is there
    let (_, body) = call(app, "GET", "/api/library/albums/52b52b2323b5df9d", cookie, None).await;
    assert_eq!(serde_json::from_slice::<Value>(&body).unwrap()["album_t"], "Persona 4 Soundtrack");
}
