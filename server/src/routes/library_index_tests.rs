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
    assert_eq!(Env::names(&e.labels("kind=albums&sort=duration&dir=asc").await), vec!["<20 min", "30–40 min", "40–50 min", "80+ min"]);
    assert_eq!(Env::names(&e.labels("kind=albums&sort=size&dir=desc").await), vec!["1–2 GB", "250–500 MB", "<50 MB"]);
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
    assert_eq!(Env::names(&e.labels("kind=tracks&sort=duration&dir=desc").await), vec!["20+ min", "4–5 min", "3–4 min", "2–3 min", "<2 min"]);
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
    assert_eq!(Env::names(&e.labels("kind=artists&sort=tracks").await), vec!["10–24 songs", "<5 songs"]);
    assert_eq!(Env::names(&e.labels("kind=artists&sort=albums").await), vec!["1 album"]);
    assert_eq!(Env::names(&e.labels("kind=artists&sort=added").await), vec!["Oct 2026", "Jan 2020"]);
    // every artist sort, both directions, lands on the list's own order
    for sort in ["name", "added", "albums", "tracks"] {
        for dir in ["asc", "desc"] {
            let list = e.get(&format!("/api/library/artists?sort={sort}&dir={dir}")).await.as_array().unwrap().clone();
            let secs = e.labels(&format!("kind=artists&sort={sort}&dir={dir}")).await;
            assert_eq!(secs[0].1, 0, "{sort} {dir}");
            let names: Vec<&str> = list.iter().map(|a| a["name"].as_str().unwrap()).collect();
            let letters = super::letters(names.into_iter());
            for (label, off) in &secs {
                let a = &list[*off];
                let got = match sort {
                    "name" => letters[*off].clone(),
                    "added" => super::label(&a["added"], super::By::Month),
                    "albums" => super::label(&a["albums"], super::By::AlbumCount),
                    _ => super::label(&a["tracks"], super::By::TrackCount),
                };
                assert_eq!(&got, label, "{sort} {dir} at {off}");
            }
        }
    }
    assert_eq!(Env::names(&e.labels("kind=artists&sort=name&dir=desc").await), vec!["Z", "T", "A", "#"]);
}

/// A library a first scan added at once: Date added (the phone's first sort) still gets sections to jump to,
/// split by minute, with offsets that land on the list's own pages.
#[tokio::test]
async fn added_in_one_scan_splits() {
    let mut e = setup("added_in_one_scan_splits").await;
    for i in 0..120 {
        e.album(&format!("Artist {i:03}"), &format!("Album {i}"), "2001", 3, 200.0, 1_000_000, JAN_2020 + (i * 20) as f64);
    }
    e.album("Late", "New", "2026", 3, 200.0, 1_000_000, OCT_2026);
    for (kind, list_uri, field) in [("albums", "/api/library/albums?sort=added&limit=500", "added"), ("tracks", "/api/library/tracks?sort=added&limit=1000", "added_at")] {
        let secs = e.get(&format!("/api/library/index?kind={kind}&sort=added")).await.as_array().unwrap().clone();
        let list = e.get(list_uri).await;
        let list = if kind == "albums" { list["items"].as_array().unwrap().clone() } else { list.as_array().unwrap().clone() };
        assert!(secs.len() > 10, "{kind}: {secs:?}");
        assert_eq!(secs[0]["label"], "Oct 2026", "{kind}");
        assert!(secs[0].get("short").is_none());
        assert!(secs[1]["label"].as_str().unwrap().starts_with("14 Jan 2020, "), "{kind}: {}", secs[1]);
        assert_eq!(secs[1]["short"].as_str().unwrap().len(), 5);
        let mut last = 0.0;
        for s in &secs {
            let off = s["offset"].as_u64().unwrap() as usize;
            let t = list[off][field].as_f64().unwrap();
            let label = s["label"].as_str().unwrap();
            if let Some(short) = s["short"].as_str() {
                assert!(label.ends_with(short), "{label} {short}");
                // the minute of the item the section starts at
                let secs_of_day = (t as i64).rem_euclid(86_400);
                assert_eq!(short, format!("{:02}:{:02}", secs_of_day / 3600, secs_of_day % 3600 / 60), "{kind} {label} at {off}");
                if off > 0 {
                    let p = list[off - 1][field].as_f64().unwrap();
                    assert_ne!((p / 60.0).floor(), (t / 60.0).floor(), "{kind}: the item before {label} is in the same minute");
                }
            }
            assert!(last == 0.0 || t <= last, "newest first");
            last = t;
        }
    }
}

/// The lists are cached (routes/library.rs `cached_list`), but never behind the library: a song added
/// or changed shows in the next answer, without waiting for the artist index's once-a-second check.
#[tokio::test]
async fn cached_lists_follow_every_change_at_once() {
    let mut e = library("cached_lists_follow_every_change_at_once").await;
    let albums = |v: &Value| v["total"].as_i64().unwrap();
    let before = e.get("/api/library/albums?sort=name").await;
    let sections = e.get("/api/library/index?kind=albums&sort=released&dir=desc").await;
    let recent = e.get("/api/library/recent?limit=50").await;
    // asked again: the same (cached) answers
    assert_eq!(e.get("/api/library/albums?sort=name").await, before);
    // a song of a new album, written the way the server writes (no invalidate, no waiting)
    let id = e.next;
    e.next += 1;
    e.st.db.execute("INSERT INTO tracks (id, path, title, artist, album, albumartist, album_id, format, duration, size, year, disc, track, added_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
        p![id, "/m/new.flac", "New", "Nina", "Fresh", "Nina", "freshalbum000001", "flac", 100.0, 1000, "1999", 1, 1, OCT_2026 + 10.0]).unwrap();
    assert_eq!(albums(&e.get("/api/library/albums?sort=name").await), albums(&before) + 1);
    assert_eq!(e.get("/api/library/recent?limit=50").await.as_array().unwrap().len(), recent.as_array().unwrap().len() + 1);
    assert_ne!(e.get("/api/library/index?kind=albums&sort=released&dir=desc").await, sections);
    // a changed year moves the album at once too
    e.st.db.execute("UPDATE tracks SET year='1888' WHERE id=?", p![id]).unwrap();
    let list = e.get("/api/library/albums?sort=year&dir=asc&limit=500").await;
    let first_dated = list["items"].as_array().unwrap().iter().find(|a| !a["year"].as_str().unwrap_or("").is_empty()).unwrap().clone();
    assert_eq!(first_dated["album"], "Fresh", "{list}");
    assert!(Env::names(&e.labels("kind=albums&sort=released&dir=asc").await).contains(&"1888"));
}

/// Cached id lists of the songs list (`track-ids…` keys of the list cache) for this test's database.
fn id_lists(e: &Env) -> usize {
    super::super::LISTS.lock().unwrap().keys().filter(|(d, k)| *d == e.st.db.data_dir && k.starts_with("track-ids\u{1f}")).count()
}

fn ids_of(v: &Value) -> Vec<i64> {
    v.as_array().unwrap().iter().map(|t| t["id"].as_i64().unwrap()).collect()
}

/// The songs list sorts the ids once per sort and keeps them; every page is a slice of that list, in
/// the list's order, with the same columns as before – and a new song shows up on the next load.
#[tokio::test]
async fn song_pages_are_slices_of_one_cached_id_list() {
    let mut e = library("song_pages").await;
    for (sort, dir) in [("artist", "asc"), ("title", "desc"), ("album", "asc"), ("added", "desc"), ("released", "desc"), ("duration", "asc")] {
        let all = e.get(&format!("/api/library/tracks?sort={sort}&dir={dir}&limit=1000")).await;
        let ids = ids_of(&all);
        assert_eq!(ids.len(), 35, "{sort}");
        let mut uniq = ids.clone();
        uniq.sort();
        uniq.dedup();
        assert_eq!(uniq.len(), ids.len(), "{sort}: no song twice");
        for (offset, limit) in [(0, 10), (10, 5), (30, 10), (35, 5)] {
            let page = e.get(&format!("/api/library/tracks?sort={sort}&dir={dir}&limit={limit}&offset={offset}")).await;
            let want: Vec<i64> = ids.iter().copied().skip(offset).take(limit).collect();
            assert_eq!(ids_of(&page), want, "{sort} {dir} offset {offset} limit {limit}");
        }
        let first = &all.as_array().unwrap()[0];
        for col in ["id", "title", "artist", "album", "albumartist", "album_id", "track", "disc", "year", "duration", "format", "bitrate", "added_at"] {
            assert!(first.get(col).is_some(), "{sort}: {col} missing from {first}");
        }
    }
    // the order is the sort's own (the same ORDER BY the page query used to run)
    let by_artist = e.get("/api/library/tracks?sort=artist&limit=1000").await;
    let direct: Vec<i64> = {
        use crate::db::RowExt;
        e.st.db.query("SELECT id FROM tracks ORDER BY lower(artist) ASC, lower(album), disc, track, id", p![]).unwrap().iter().map(|r| r.i("id")).collect()
    };
    assert_eq!(ids_of(&by_artist), direct);
    assert_eq!(by_artist[0]["artist"], "2Pac");
    let by_len = e.get("/api/library/tracks?sort=duration&dir=asc&limit=3").await;
    assert_eq!(by_len[0]["duration"], 100.0);
    // one cached id list per sort and direction, not one entry per page
    assert_eq!(id_lists(&e), 6);
    // a new song is on the next page load (the write moves the library version)
    e.album("Zz Top", "Zz", "2001", 1, 100.0, 1, OCT_2026);
    let all = e.get("/api/library/tracks?sort=artist&limit=1000").await;
    assert_eq!(all.as_array().unwrap().len(), 36);
    assert!(all.as_array().unwrap().iter().any(|t| t["artist"] == "Zz Top"));
    // the explicit-ids path keeps the asked order
    let picked = e.get(&format!("/api/library/tracks?ids={},{}", ids_of(&all)[5], ids_of(&all)[2])).await;
    assert_eq!(ids_of(&picked), vec![ids_of(&all)[5], ids_of(&all)[2]]);
}

/// Shuffle draws from the cached id list instead of sorting every song by RANDOM(): the asked number of
/// distinct songs, none of the excluded ones, and all that are left when fewer remain than asked for.
#[tokio::test]
async fn shuffle_draws_without_sorting_every_song() {
    let e = library("shuffle").await;
    let got = e.get("/api/library/shuffle?limit=10").await;
    let ids = ids_of(&got);
    assert_eq!(ids.len(), 10);
    let mut uniq = ids.clone();
    uniq.sort();
    uniq.dedup();
    assert_eq!(uniq.len(), 10, "no song twice");
    for col in ["id", "title", "artist", "album", "album_id", "format", "bitrate", "duration"] {
        assert!(got[0].get(col).is_some(), "{col} missing from {}", got[0]);
    }
    let all: Vec<i64> = {
        use crate::db::RowExt;
        e.st.db.query("SELECT id FROM tracks ORDER BY id", p![]).unwrap().iter().map(|r| r.i("id")).collect()
    };
    let exclude: Vec<String> = all[..30].iter().map(|i| i.to_string()).collect();
    let got = e.get(&format!("/api/library/shuffle?limit=10&exclude={}", exclude.join(","))).await;
    let mut ids = ids_of(&got);
    ids.sort();
    assert_eq!(ids, all[30..].to_vec(), "the five songs left, each once");
    // two draws of 20 out of 35 in the same order would be a very unlucky day
    let a = ids_of(&e.get("/api/library/shuffle?limit=20").await);
    let b = ids_of(&e.get("/api/library/shuffle?limit=20").await);
    assert!(a != b || a.is_empty());
    assert_eq!(id_lists(&e), 1, "one cached id list (any order) for shuffle");
}

/// The translate layer: nothing happens while no account has translations on; once one has, the list
/// answers to that listener carry the `_t` fields, and the layer gets the user from the handler's own
/// extractor (no user lookup of its own – the stamping still works with that path).
#[tokio::test]
async fn translations_are_stamped_through_the_handlers_user() {
    let e = library("translated").await;
    assert!(!crate::translate::anyone_on(&e.st.db));
    crate::translate::store::put(&e.st.db, "Végétal", "en", "album", Some("Vegetal"), None, crate::translate::store::Source::Official).unwrap();
    let v = e.get("/api/library/tracks?sort=album&limit=1000").await;
    let veg = v.as_array().unwrap().iter().find(|t| t["album"] == "Végétal").unwrap().clone();
    assert!(veg.get("album_t").is_none(), "nobody has translations on: {veg}");
    // the listener turns them on (a write to users: the layer notices without a restart)
    e.st.db.execute("UPDATE users SET prefs=? WHERE username='indexer'", p![r#"{"translate_mode":"under","translate_lang":"en"}"#]).unwrap();
    assert!(crate::translate::anyone_on(&e.st.db));
    let v = e.get("/api/library/tracks?sort=album&limit=1000").await;
    let veg = v.as_array().unwrap().iter().find(|t| t["album"] == "Végétal").unwrap().clone();
    assert_eq!(veg["album_t"], "Vegetal", "{veg}");
    assert_eq!(veg["artist"], "Émilie Simon");
    // off again: the fields go
    e.st.db.execute("UPDATE users SET prefs='{}' WHERE username='indexer'", p![]).unwrap();
    let v = e.get("/api/library/tracks?sort=album&limit=1000").await;
    assert!(v.as_array().unwrap().iter().all(|t| t.get("album_t").is_none()));
}
