//! Page-load benchmark for the pages that wait on MusicBrainz: the artist page (with the sections the apps
//! load next to it), an album page, an album opened right after an artist page, search while typing, and
//! an artist page after a restart. MusicBrainz is a local stand-in that answers like the real one does:
//! every answer takes `MB_LATENCY` and the server keeps to its default of one call a second, so the
//! numbers are what a listener waits for on a cold cache (the local database is not what is slow here:
//! `cargo bench --bench perf` times that).
//!
//!     cargo bench -p encore --bench pages        (in server/; `--profile dev` builds in a minute)
//!
//! Prints a Markdown table: each scenario's time in ms and how many calls MusicBrainz got (docs/perf/pages.md has
//! the numbers before and after the Oct 23 pass).
use axum::body::Body;
use axum::extract::{Path as P, Query, State};
use axum::http::{Request, StatusCode};
use axum::routing::get;
use axum::Router;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tower::ServiceExt;

/// The bench owner's username.
const OWNER: &str = "benchowner";
/// The bench owner's password.
const PW: &str = "bench-password-1";
/// How long the stand-in MusicBrainz takes for each answer (musicbrainz.org from Europe or the US: 200–400 ms).
const MB_LATENCY: Duration = Duration::from_millis(250);
/// The artist every scenario opens: 250 release groups (three pages of MusicBrainz's 100), as a long-running
/// band has.
const ARTIST: &str = "a74b1b7f-71a5-4011-9441-d0b5e4122711";
/// Release groups of the artist (and releases: one per release group).
const GROUPS: usize = 250;

/// The calls the stand-in MusicBrainz got, shared with the scenarios that count them.
type Hits = Arc<AtomicUsize>;

/// A release group of [`ARTIST`] by number.
fn rg_id(k: usize) -> String {
    format!("00000000-0000-0000-0001-{k:012}")
}

/// A release of [`ARTIST`] by number (one per release group).
fn rel_id(k: usize) -> String {
    format!("00000000-0000-0000-0002-{k:012}")
}

/// The stand-in MusicBrainz: an artist with [`GROUPS`] release groups and as many releases, each release with
/// ten songs, and searches that find a few of each. Every answer waits [`MB_LATENCY`] first.
async fn mock_mb(hits: Hits) -> String {
    /// `/ws/2/artist/{id}`: the artist with its tags and links.
    async fn artist(State(h): State<Hits>, P(id): P<String>) -> axum::Json<Value> {
        h.fetch_add(1, Ordering::Relaxed);
        tokio::time::sleep(MB_LATENCY).await;
        axum::Json(json!({ "id": id, "name": "Radiohead", "sort-name": "Radiohead", "type": "Group", "country": "GB",
            "life-span": { "begin": "1985" }, "tags": [{ "name": "rock", "count": 3 }],
            "relations": [{ "type": "official homepage", "url": { "resource": "https://example.org" } }] }))
    }
    /// `/ws/2/artist?query=`: an artist search.
    async fn artist_search(State(h): State<Hits>) -> axum::Json<Value> {
        h.fetch_add(1, Ordering::Relaxed);
        tokio::time::sleep(MB_LATENCY).await;
        axum::Json(json!({ "artists": [{ "id": ARTIST, "name": "Radiohead", "sort-name": "Radiohead", "score": 100, "type": "Group" }] }))
    }
    /// `/ws/2/release-group`: the artist's release groups a page at a time, or a search.
    async fn groups(State(h): State<Hits>, Query(q): Query<HashMap<String, String>>) -> axum::Json<Value> {
        h.fetch_add(1, Ordering::Relaxed);
        tokio::time::sleep(MB_LATENCY).await;
        let offset: usize = q.get("offset").and_then(|o| o.parse().ok()).unwrap_or(0);
        let n = if q.contains_key("query") { 5 } else { GROUPS.saturating_sub(offset).min(100) };
        let rgs: Vec<Value> = (offset..offset + n).map(|k| json!({ "id": rg_id(k), "title": format!("Record {k}"),
            "primary-type": if k % 4 == 0 { "Single" } else { "Album" }, "secondary-types": [], "first-release-date": format!("{}-01-01", 1985 + k % 40),
            "artist-credit": [{ "name": "Radiohead", "artist": { "id": ARTIST, "name": "Radiohead" } }] })).collect();
        axum::Json(json!({ "release-groups": rgs, "release-group-count": if q.contains_key("query") { n } else { GROUPS } }))
    }
    /// `/ws/2/release-group/{id}`: one release group with its releases.
    async fn group(State(h): State<Hits>, P(id): P<String>) -> axum::Json<Value> {
        h.fetch_add(1, Ordering::Relaxed);
        tokio::time::sleep(MB_LATENCY).await;
        axum::Json(json!({ "id": id, "title": "Record 1", "primary-type": "Album", "first-release-date": "1997-05-21",
            "artist-credit": [{ "name": "Radiohead", "artist": { "id": ARTIST, "name": "Radiohead" } }], "releases": [{ "id": rel_id(1) }] }))
    }
    /// `/ws/2/release`: the releases of a release group or of the artist (barcodes), a page at a time.
    async fn releases(State(h): State<Hits>, Query(q): Query<HashMap<String, String>>) -> axum::Json<Value> {
        h.fetch_add(1, Ordering::Relaxed);
        tokio::time::sleep(MB_LATENCY).await;
        if q.contains_key("release-group") {
            return axum::Json(json!({ "releases": [{ "id": rel_id(1), "status": "Official", "country": "GB", "date": "1997-05-21",
                "media": [{ "format": "CD", "track-count": 10 }] }], "release-count": 1 }));
        }
        let offset: usize = q.get("offset").and_then(|o| o.parse().ok()).unwrap_or(0);
        let rels: Vec<Value> = (offset..(offset + 100).min(GROUPS)).map(|k| json!({ "id": rel_id(k), "barcode": format!("{:012}", 1000 + k),
            "release-group": { "id": rg_id(k) } })).collect();
        axum::Json(json!({ "releases": rels, "release-count": GROUPS }))
    }
    /// `/ws/2/release/{id}`: a release with its ten songs.
    async fn release(State(h): State<Hits>, P(id): P<String>) -> axum::Json<Value> {
        h.fetch_add(1, Ordering::Relaxed);
        tokio::time::sleep(MB_LATENCY).await;
        let tracks: Vec<Value> = (1..=10).map(|t| json!({ "position": t, "title": format!("Song {t}"), "length": 200_000,
            "recording": { "id": format!("00000000-0000-0000-0003-{t:012}"), "title": format!("Song {t}"),
                "artist-credit": [{ "name": "Radiohead", "artist": { "id": ARTIST, "name": "Radiohead" } }] } })).collect();
        axum::Json(json!({ "id": id, "title": "Record 1", "date": "1997-05-21", "country": "GB", "status": "Official",
            "artist-credit": [{ "name": "Radiohead", "artist": { "id": ARTIST, "name": "Radiohead" } }],
            "release-group": { "id": rg_id(1), "primary-type": "Album", "first-release-date": "1997-05-21" },
            "media": [{ "position": 1, "format": "CD", "tracks": tracks }] }))
    }
    /// Anything else (recording searches, ISRC look-ups): an empty answer, after the same wait.
    async fn other(State(h): State<Hits>) -> axum::Json<Value> {
        h.fetch_add(1, Ordering::Relaxed);
        tokio::time::sleep(MB_LATENCY).await;
        axum::Json(json!({ "recordings": [], "count": 0 }))
    }
    let app = Router::new()
        .route("/ws/2/artist/{id}", get(artist))
        .route("/ws/2/artist", get(artist_search))
        .route("/ws/2/release-group", get(groups))
        .route("/ws/2/release-group/{id}", get(group))
        .route("/ws/2/release", get(releases))
        .route("/ws/2/release/{id}", get(release))
        .fallback(other)
        .with_state(hits);
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    format!("http://{addr}")
}

/// A server on `dir`, signed in as its owner.
#[derive(Clone)]
struct Server {
    /// the server's router, called in process
    app: Router,
    /// the owner's session cookie
    cookie: String,
}

/// A server on `dir` (its database made on the first start) that asks `mb` at the default one call a second.
async fn start(dir: &Path, mb: &str) -> Server {
    let st = encore_api::test_support::state_in(dir);
    st.db.execute("UPDATE users SET username=?, password_hash=? WHERE role='owner'", encore_db::p![OWNER, encore_api::auth::hash_password(PW)]).unwrap();
    st.cfg.set_many(&json!({ "musicbrainz_url": mb, "musicbrainz_rate": 1.0 }));
    // what main() starts next to the router (the background work a page may hand on)
    encore_api::warm_up(&st);
    let app = encore_api::app(st.clone());
    let req = Request::builder().method("POST").uri("/api/auth/login").header("x-requested-with", "encore").header("content-type", "application/json")
        .body(Body::from(json!({ "username": OWNER, "password": PW }).to_string())).unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let cookie = resp.headers().get("set-cookie").and_then(|v| v.to_str().ok()).unwrap_or("").split(';').next().unwrap().to_string();
    // the sign-in's own look-ups are not part of any page
    tokio::time::sleep(Duration::from_millis(1100)).await;
    Server { app, cookie }
}

impl Server {
    /// One GET; returns (ms, status).
    async fn get(&self, uri: &str) -> (f64, u16) {
        let req = Request::builder().method("GET").uri(uri).header("x-requested-with", "encore").header("cookie", &self.cookie).body(Body::empty()).unwrap();
        let t = Instant::now();
        let resp = self.app.clone().oneshot(req).await.unwrap();
        let status = resp.status().as_u16();
        let _ = axum::body::to_bytes(resp.into_body(), 64 << 20).await;
        (t.elapsed().as_secs_f64() * 1000.0, status)
    }
}

/// The sections an artist page loads next to (web) or with (Android) the page itself.
fn sections() -> Vec<String> {
    vec![
        format!("/api/discover/artist/{ARTIST}/catalog"),
        format!("/api/discover/artist/{ARTIST}/extras"),
        format!("/api/discover/artist/{ARTIST}/similar"),
        format!("/api/discover/artist/{ARTIST}/library-tracks?name=Radiohead"),
    ]
}

/// One scenario's row: its name, the times it reports and the MusicBrainz calls it made.
struct Row {
    /// what was timed
    name: String,
    /// what the listener waits for, in ms
    ms: f64,
    /// MusicBrainz calls during the scenario
    calls: usize,
}

fn main() {
    tokio::runtime::Builder::new_multi_thread().worker_threads(4).enable_all().build().unwrap().block_on(bench());
}

/// Time every page scenario against the stand-in MusicBrainz and print the results.
async fn bench() {
    let base = std::env::temp_dir().join(format!("encore-pages-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let hits: Hits = Arc::default();
    let mb = mock_mb(hits.clone()).await;
    let mut rows: Vec<Row> = vec![];
    let artist_uri = format!("/api/discover/artist/{ARTIST}");
    let album_uri = format!("/api/discover/album/{}", rg_id(1));
    // a fresh server (nothing cached, nothing on disk) for every scenario
    let mut n = 0;
    let mut fresh = || {
        n += 1;
        base.join(format!("s{n}"))
    };

    // 1. the web app: the artist page, then its four sections at once
    {
        let s = start(&fresh(), &mb).await;
        let h0 = hits.load(Ordering::Relaxed);
        let t = Instant::now();
        let (page, _) = s.get(&artist_uri).await;
        futures_util::future::join_all(sections().iter().map(|u| s.get(u))).await;
        let all = t.elapsed().as_secs_f64() * 1000.0;
        let calls = hits.load(Ordering::Relaxed) - h0;
        rows.push(Row { name: "web: artist page".into(), ms: page, calls: 0 });
        rows.push(Row { name: "web: artist page and every section".into(), ms: all, calls });
    }
    // 2. Android: the page and its four sections at once
    {
        let s = start(&fresh(), &mb).await;
        let h0 = hits.load(Ordering::Relaxed);
        let t = Instant::now();
        let page = s.get(&artist_uri);
        let rest = futures_util::future::join_all(sections().into_iter().map(|u| { let s = s.clone(); async move { s.get(&u).await } }));
        let ((page, _), _) = futures_util::future::join(page, rest).await;
        let all = t.elapsed().as_secs_f64() * 1000.0;
        let calls = hits.load(Ordering::Relaxed) - h0;
        rows.push(Row { name: "Android: artist page".into(), ms: page, calls: 0 });
        rows.push(Row { name: "Android: artist page and every section".into(), ms: all, calls });
    }
    // 3. an album page, cold
    {
        let s = start(&fresh(), &mb).await;
        let h0 = hits.load(Ordering::Relaxed);
        let (ms, _) = s.get(&album_uri).await;
        rows.push(Row { name: "album page".into(), ms, calls: hits.load(Ordering::Relaxed) - h0 });
    }
    // 4. an album opened a second after the artist page showed (its sections still loading)
    {
        let s = start(&fresh(), &mb).await;
        let h0 = hits.load(Ordering::Relaxed);
        s.get(&artist_uri).await;
        let bg: Vec<_> = sections().into_iter().map(|u| { let s = s.clone(); tokio::spawn(async move { s.get(&u).await }) }).collect();
        tokio::time::sleep(Duration::from_secs(1)).await;
        let (ms, _) = s.get(&album_uri).await;
        for b in bg {
            let _ = b.await;
        }
        rows.push(Row { name: "album opened 1 s after the artist page".into(), ms, calls: hits.load(Ordering::Relaxed) - h0 });
    }
    // 5. search while typing: "rad", "radio", "radiohead" 0.7 s apart (the web app's 650 ms pause), timed
    //    from the last one. Each earlier search either runs on (an app that doesn't cancel it) or is
    //    dropped when the next one starts (an app that does: the browser's abort closes the connection,
    //    and the server drops the call with it).
    for cancel in [false, true] {
        let s = start(&fresh(), &mb).await;
        let h0 = hits.load(Ordering::Relaxed);
        let mut older: Vec<tokio::task::JoinHandle<(f64, u16)>> = vec![];
        for q in ["rad", "radio"] {
            let s2 = s.clone();
            let uri = format!("/api/discover/search?q={q}&type=all");
            older.push(tokio::spawn(async move { s2.get(&uri).await }));
            tokio::time::sleep(Duration::from_millis(700)).await;
            if cancel {
                older.last().unwrap().abort();
            }
        }
        let (ms, _) = s.get("/api/discover/search?q=radiohead&type=all").await;
        for o in older {
            let _ = o.await;
        }
        let name = if cancel { "search while typing, earlier searches cancelled" } else { "search while typing, earlier searches run on" };
        rows.push(Row { name: name.into(), ms, calls: hits.load(Ordering::Relaxed) - h0 });
    }
    // 6. the artist page again after a restart (a new server on the same folder)
    {
        let d = fresh();
        let s = start(&d, &mb).await;
        s.get(&artist_uri).await;
        drop(s);
        let s = start(&d, &mb).await;
        let h0 = hits.load(Ordering::Relaxed);
        let (ms, _) = s.get(&artist_uri).await;
        rows.push(Row { name: "artist page again after a restart".into(), ms, calls: hits.load(Ordering::Relaxed) - h0 });
    }
    println!("\nMusicBrainz: {} ms an answer, 1 call a second\n\n| scenario | ms | MusicBrainz calls |\n|---|---|---|", MB_LATENCY.as_millis());
    for r in rows {
        let calls = if r.calls == 0 { "".to_string() } else { r.calls.to_string() };
        println!("| {} | {:.0} | {} |", r.name, r.ms, calls);
    }
    let _ = std::fs::remove_dir_all(&base);
}
