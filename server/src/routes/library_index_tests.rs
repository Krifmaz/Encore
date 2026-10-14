//! GET /api/library/index through the real router: every sort's labels, and offsets that land on the
//! section's first item in the list endpoint's own order.
use crate::p;
use crate::state::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use serde_json::{json, Value};
use tower::ServiceExt;

const PW: &str = "indexer-pass-1";

struct Env {
    st: AppState,
    app: Router,
    dir: std::path::PathBuf,
    cookie: String,
    next: i64,
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn setup(name: &str) -> Env {
    let dir = std::env::temp_dir().join(format!("encore-index-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let st = {
        let _env = crate::TEST_ENV.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("ENCORE_CONFIG_DIR", &dir);
        crate::build_state().unwrap()
    };
    crate::auth::create_user(&st, "indexer", PW, "user", None, None).unwrap();
    let app = crate::app(st.clone());
    let req = Request::builder().method("POST").uri("/api/auth/login").header("x-requested-with", "encore").header("content-type", "application/json")
        .body(Body::from(json!({ "username": "indexer", "password": PW }).to_string())).unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let cookie = resp.headers().get("set-cookie").and_then(|v| v.to_str().ok()).unwrap_or("").split(';').next().unwrap().to_string();
    Env { st, app, dir, cookie, next: 1 }
}

impl Env {
    /// `n` songs of one album; each song `secs` long and `bytes` big, added at `added` (epoch s).
    #[allow(clippy::too_many_arguments)]
    fn album(&mut self, albumartist: &str, album: &str, year: &str, n: usize, secs: f64, bytes: i64, added: f64) {
        let aid = crate::library::album_key(albumartist, album, "");
        for i in 0..n {
            let id = self.next;
            self.next += 1;
            self.st.db.execute(
                "INSERT INTO tracks (id, path, title, artist, album, albumartist, album_id, format, bitrate, duration, size, year, disc, track, added_at)
                 VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
                p![id, format!("/m/{id}.flac"), format!("{album} {i}"), albumartist, album, albumartist, aid.clone(), "flac", 900, secs, bytes, year, 1, i as i64 + 1, added]).unwrap();
        }
        crate::artists::invalidate(&self.st.db);
    }
    async fn get(&self, uri: &str) -> Value {
        let mut b = Request::builder().method("GET").uri(uri).header("x-requested-with", "encore");
        b = b.header("cookie", &self.cookie);
        let resp = self.app.clone().oneshot(b.body(Body::empty()).unwrap()).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 10 << 20).await.unwrap();
        let v: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        assert_eq!(status, StatusCode::OK, "{uri}: {v:?}");
        v
    }
    async fn labels(&self, q: &str) -> Vec<(String, usize)> {
        self.get(&format!("/api/library/index?{q}")).await.as_array().unwrap().iter()
            .map(|s| (s["label"].as_str().unwrap().to_string(), s["offset"].as_u64().unwrap() as usize)).collect()
    }
    fn names(v: &[(String, usize)]) -> Vec<&str> {
        v.iter().map(|(l, _)| l.as_str()).collect()
    }
}

const OCT_2026: f64 = 1_791_936_000.0; // 2026-10-14
const JAN_2020: f64 = 1_579_000_000.0; // 2020-01-14

async fn library(name: &str) -> Env {
    let mut e = setup(name).await;
    e.album("ABBA", "Arrival", "1976", 10, 200.0, 30_000_000, JAN_2020); // 33 min, 300 MB
    e.album("Émilie Simon", "Végétal", "2006-03-01", 3, 240.0, 10_000_000, JAN_2020); // 12 min
    e.album("2Pac", "Me Against the World", "1995", 1, 100.0, 5_000_000, OCT_2026); // single
    e.album("The Beatles", "Abbey Road", "", 17, 170.0, 70_000_000, OCT_2026); // 48 min, 1.19 GB
    e.album("Zoe", "Zed", "2026", 4, 1300.0, 1_000_000, OCT_2026); // 86 min
    e
}

#[tokio::test]
async fn album_sections_match_the_list() {
    let e = library("album_sections_match_the_list").await;
    // every sort the clients offer, both directions: each section starts where the list has an item with that label
    for sort in ["name", "artist", "album", "released", "added", "duration", "size"] {
        for dir in ["asc", "desc"] {
            let secs = e.labels(&format!("kind=albums&sort={sort}&dir={dir}")).await;
            let list = e.get(&format!("/api/library/albums?sort={sort}&dir={dir}&limit=500")).await["items"].as_array().unwrap().clone();
            assert!(!secs.is_empty(), "{sort} {dir}");
            assert_eq!(secs[0].1, 0, "{sort} {dir}: the first section starts the list");
            let field = if sort == "album" { "album" } else { "albumartist" };
            let letters = super::letters(list.iter().map(|a| a[field].as_str().unwrap()));
            let mut last = 0;
            for (label, off) in &secs {
                assert!(*off < list.len(), "{sort} {dir}: {label} at {off}");
                assert!(*off >= last, "{sort} {dir}: offsets in order");
                last = *off;
                let it = &list[*off];
                let ok = match sort {
                    "name" | "artist" | "album" => letters[*off] == *label && (*off == 0 || letters[off - 1] != *label),
                    "released" => super::label(&it["year"], super::By::Year) == *label,
                    "added" => super::label(&it["added"], super::By::Month) == *label,
                    "duration" => super::label(&it["duration"], super::By::AlbumLength) == *label,
                    _ => super::label(&it["size"], super::By::Size) == *label,
                };
                assert!(ok, "{sort} {dir}: {label} at {off} is {it}");
            }
        }
    }
    // the labels themselves
    assert_eq!(Env::names(&e.labels("kind=albums&sort=artist&dir=asc").await), vec!["#", "A", "T", "Z"]);
    // Z–A: "Émilie Simon" sorts first (after "z" read backwards) and joins the Z section
    assert_eq!(e.labels("kind=albums&sort=artist&dir=desc").await.into_iter().map(|(l, o)| format!("{l}{o}")).collect::<Vec<_>>(), vec!["Z0", "T2", "A3", "#4"]);
    assert_eq!(Env::names(&e.labels("kind=albums&sort=album&dir=asc").await), vec!["A", "M", "V", "Z"]);
    assert_eq!(Env::names(&e.labels("kind=albums&sort=released&dir=desc").await), vec!["2026", "2006", "1995", "1976", "?"]);
    assert_eq!(Env::names(&e.labels("kind=albums&sort=added&dir=desc").await), vec!["Oct 2026", "Jan 2020"]);
    assert_eq!(Env::names(&e.labels("kind=albums&sort=duration&dir=asc").await), vec!["<20 min", "20–40 min", "40–60 min", "60+ min"]);
    assert_eq!(Env::names(&e.labels("kind=albums&sort=size&dir=desc").await), vec!["1 GB+", "250–500 MB", "<100 MB"]);
    // an unknown sort falls back like the list (name)
    assert_eq!(e.labels("kind=albums&sort=nope").await, e.labels("kind=albums&sort=name").await);
    // a search is ranked: no sections
    assert!(e.labels("kind=albums&q=abba").await.is_empty());
}

#[tokio::test]
async fn album_filters_artist_and_singles() {
    let e = library("album_filters_artist_and_singles").await;
    // the web's Albums / Singles tabs
    assert_eq!(Env::names(&e.labels("kind=albums&album_kind=albums&sort=artist&dir=asc").await), vec!["A", "T", "Z"]);
    assert_eq!(e.labels("kind=singles&sort=artist&dir=asc").await, vec![("#".to_string(), 0)]);
    // an artist's albums (the artist page grid)
    assert_eq!(e.labels("kind=albums&artist=Beatles&sort=released").await, vec![("?".to_string(), 0)]);
    assert_eq!(e.labels("kind=albums&artist=abba&sort=released").await, vec![("1976".to_string(), 0)]);
}

#[tokio::test]
async fn track_sections_match_the_list() {
    let e = library("track_sections_match_the_list").await;
    for sort in ["artist", "title", "album", "added", "released", "duration"] {
        for dir in ["asc", "desc"] {
            let secs = e.labels(&format!("kind=tracks&sort={sort}&dir={dir}")).await;
            let list = e.get(&format!("/api/library/tracks?sort={sort}&dir={dir}&limit=1000")).await.as_array().unwrap().clone();
            let field = if sort == "title" || sort == "album" { sort } else { "artist" };
            let letters = super::letters(list.iter().map(|t| t[field].as_str().unwrap()));
            assert_eq!(secs[0].1, 0);
            for (label, off) in &secs {
                // the list's own paging lands on it too
                let one = e.get(&format!("/api/library/tracks?sort={sort}&dir={dir}&limit=1&offset={off}")).await;
                let it = &one[0];
                assert_eq!(it["id"], list[*off]["id"], "{sort} {dir}");
                let got = match sort {
                    "artist" | "title" | "album" => letters[*off].clone(),
                    "added" => super::label(&it["added_at"], super::By::Month),
                    "released" => super::label(&it["year"], super::By::Year),
                    _ => super::label(&it["duration"], super::By::TrackLength),
                };
                assert_eq!(&got, label, "{sort} {dir} at {off}");
                // the item before belongs to another section
                if *off > 0 && matches!(sort, "artist" | "title" | "album") {
                    assert_ne!(&letters[off - 1], label, "{sort} {dir} at {off}");
                }
            }
        }
    }
    let by_artist = e.labels("kind=tracks").await; // the default sort: artist A–Z
    assert_eq!(Env::names(&by_artist), vec!["#", "A", "T", "Z"]);
    assert_eq!(by_artist.iter().map(|s| s.1).collect::<Vec<_>>(), vec![0, 1, 11, 28]); // É sorts after z: its songs stay under Z
    assert_eq!(Env::names(&e.labels("kind=tracks&sort=duration&dir=desc").await), vec!["20+ min", "2–5 min", "<2 min"]);
}

#[tokio::test]
async fn artist_sections() {
    let e = library("artist_sections").await;
    let names: Vec<String> = e.get("/api/library/artists?sort=name").await.as_array().unwrap().iter().map(|a| a["name"].as_str().unwrap().to_string()).collect();
    let secs = e.labels("kind=artists&sort=name").await;
    let letters = super::letters(names.iter().map(|n| n.as_str()));
    for (label, off) in &secs {
        assert_eq!(&letters[*off], label);
    }
    assert_eq!(Env::names(&secs), vec!["#", "A", "T", "Z"]);
    assert_eq!(Env::names(&e.labels("kind=artists&sort=tracks").await), vec!["10–24 songs", "<10 songs"]);
    assert_eq!(Env::names(&e.labels("kind=artists&sort=albums").await), vec!["1 album"]);
    assert_eq!(Env::names(&e.labels("kind=artists&sort=added").await), vec!["Oct 2026", "Jan 2020"]);
}
