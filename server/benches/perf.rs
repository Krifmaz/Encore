//! Speed benchmark: a synthetic library of 60k songs / 6k albums / 2.5k artists, and every endpoint the
//! apps call while pages load, timed cold (a fresh server on the same database file) and warm (p50/p95).
//!
//!     cargo bench --bench perf          (in server/; BENCH_COLD=main times only the main pages cold,
//!                                         BENCH_TRANSLATE=1 turns translations on for the listener)
//!
//! Prints a Markdown table (docs/perf/ has the numbers per release: 0.26-baseline.md before the 0.26 speed
//! pass, 0.26.md after it). MusicBrainz is a local mock.
use encore_api::state::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use serde_json::{json, Value};
use std::path::Path;
use std::time::Instant;
use tower::ServiceExt;

/// The bench owner's username.
const OWNER: &str = "benchowner";
/// The bench owner's password.
const PW: &str = "bench-password-1";
/// Songs in the synthetic library.
pub const TRACKS: usize = 60_000;
/// Albums in the synthetic library.
pub const ALBUMS: usize = 6_000;
/// Artists in the synthetic library.
pub const ARTISTS: usize = 2_500;

/// A made-up artist name for number `i` ("The Echo Velvet 0042").
fn artist_name(i: usize) -> String {
    const FIRST: &[&str] = &["The", "Los", "DJ", "Little", "Big", "Saint", "Lady", "MC", "Young", "Old"];
    const WORD: &[&str] = &["Echo", "Velvet", "Mirror", "Atlas", "Copper", "Sun", "River", "Neon", "Ghost", "Ivory", "Paper", "Static", "Honey", "Signal"];
    format!("{} {} {} {i:04}", FIRST[i % FIRST.len()], WORD[(i / 10) % WORD.len()], WORD[(i * 7) % WORD.len()])
}

/// Fills `dir/encore.db` (the schema must exist): songs, plays, requests, playlists, Liked Songs.
pub fn fill(dir: &Path, owner_id: i64) {
    let mut c = rusqlite::Connection::open(dir.join("encore.db")).unwrap();
    let tx = c.transaction().unwrap();
    let music = dir.join("music");
    {
        let mut ins = tx.prepare(
            "INSERT INTO tracks (id, path, album_id, mtime, size, title, artist, album, albumartist, track, disc, year, genre, duration, bitrate, format,
                                 mb_trackid, mb_albumid, mb_releasegroupid, mb_artistid, mb_albumartistid, added_at)
             VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)").unwrap();
        let mut id = 1i64;
        for a in 0..ALBUMS {
            let ai = a % ARTISTS;
            let aa = artist_name(ai);
            let album = format!("Album {} of {}", a / ARTISTS + 1, aa);
            let aid = encore_library::album_key(&aa, &album, "");
            let year = format!("{}", 1965 + (a * 13) % 60);
            let tagged = a % 3 != 0;
            let (rg, rel, ambid) = (format!("rg-{a:08}"), format!("rel-{a:08}"), format!("00000000-0000-0000-0000-{ai:012}"));
            let folder = music.join(format!("{aa}/{album}"));
            for t in 0..(TRACKS / ALBUMS) {
                // every 7th song is a collaboration
                let artist = if t % 7 == 3 { format!("{aa} feat. {}", artist_name((ai + 17) % ARTISTS)) } else { aa.clone() };
                ins.execute(rusqlite::params![
                    id, folder.join(format!("{:02} Song {t}.flac", t + 1)).to_string_lossy(), aid, 1_700_000_000.0 + a as f64, 30_000_000 + (id % 1000) * 1000,
                    format!("Song {t} of {album}"), artist, album, aa, t as i64 + 1, 1, year, ["Rock", "Pop", "Jazz", "Electronic"][a % 4],
                    180.0 + (id % 120) as f64, 900, if a % 5 == 0 { "mp3" } else { "flac" },
                    tagged.then(|| format!("rec-{id:08}")), tagged.then(|| rel.clone()), tagged.then(|| rg.clone()), tagged.then(|| ambid.clone()), tagged.then(|| ambid.clone()),
                    1_750_000_000.0 + (a / 50) as f64 * 3600.0,
                ]).unwrap();
                id += 1;
            }
            // a real cover file for the first albums (cover requests)
            if a < 20 {
                std::fs::create_dir_all(&folder).unwrap();
                let img = image::RgbImage::from_fn(600, 600, |x, y| image::Rgb([(x % 256) as u8, (y % 256) as u8, 90]));
                img.save(folder.join("cover.jpg")).unwrap();
            }
        }
        // plays: 20k over a year
        let mut p = tx.prepare("INSERT INTO plays (user_id, track_id, artist, title, album, seconds, played_at) SELECT ?, id, artist, title, album, 200, ? FROM tracks WHERE id=?").unwrap();
        for i in 0..20_000i64 {
            let tid = (i * 7919) % TRACKS as i64 + 1;
            p.execute(rusqlite::params![owner_id, 1_760_000_000.0 + i as f64 * 1500.0, tid]).unwrap();
        }
        // requests
        let mut r = tx.prepare("INSERT INTO requests (user_id, kind, mbid, artist, title, status, source, created_at, updated_at) VALUES (?,?,?,?,?,?,?,?,?)").unwrap();
        for i in 0..600usize {
            let st = ["completed", "completed", "failed", "searching", "pending_approval", "downloading"][i % 6];
            r.execute(rusqlite::params![owner_id, if i % 2 == 0 { "album" } else { "track" }, format!("rg-{:08}", i * 3), artist_name(i % ARTISTS), format!("Requested {i}"), st, "slskd", 1_760_000_000.0 + i as f64, 1_760_000_000.0 + i as f64]).unwrap();
        }
        // playlists: 30 × 50 songs, Liked Songs with 2.4k
        let mut pl = tx.prepare("INSERT INTO playlists (user_id, name, kind, source, sync_targets, created_at, updated_at) VALUES (?,?,?,?,'[]',?,?)").unwrap();
        let mut it = tx.prepare("INSERT INTO playlist_items (playlist_id, position, title, artist, album, duration, source, track_id, match_reason, added_at)
                                 SELECT ?, ?, title, artist, album, duration, 'library', id, 'added from library', ? FROM tracks WHERE id=?").unwrap();
        pl.execute(rusqlite::params![owner_id, "Liked Songs", "liked", "manual", 1.0, 1.0]).unwrap();
        let liked = tx.last_insert_rowid();
        for i in 0..2400i64 {
            it.execute(rusqlite::params![liked, i, 1_760_000_000.0 + i as f64, (i * 23) % TRACKS as i64 + 1]).unwrap();
        }
        for n in 0..30i64 {
            pl.execute(rusqlite::params![owner_id, format!("Playlist {n}"), "playlist", "manual", 2.0, 2.0 + n as f64]).unwrap();
            let pid = tx.last_insert_rowid();
            for i in 0..50i64 {
                it.execute(rusqlite::params![pid, i, 1.0, (n * 1000 + i * 37) % TRACKS as i64 + 1]).unwrap();
            }
        }
    }
    tx.commit().unwrap();
}

/// A local MusicBrainz: artists with 40 release groups, release groups with one release.
async fn mock_mb() -> String {
    use axum::extract::{Path as P, Query};
    use axum::routing::get;
    use std::collections::HashMap;
    async fn artist(P(id): P<String>) -> axum::Json<Value> {
        let i: usize = id.rsplit('-').next().and_then(|n| n.parse().ok()).unwrap_or(0);
        axum::Json(json!({ "id": id, "name": artist_name(i), "sort-name": artist_name(i), "type": "Group", "country": "GB",
            "life-span": { "begin": "1990" }, "tags": [{ "name": "rock", "count": 3 }], "relations": [{ "type": "official homepage", "url": { "resource": "https://example.org" } }] }))
    }
    async fn groups(Query(q): Query<HashMap<String, String>>) -> axum::Json<Value> {
        let a = q.get("artist").cloned().unwrap_or_default();
        let i: usize = a.rsplit('-').next().and_then(|n| n.parse().ok()).unwrap_or(0);
        let rgs: Vec<Value> = (0..40).map(|k| json!({ "id": if k < 3 { format!("rg-{:08}", i + k * ARTISTS) } else { format!("rg-x{i}-{k}") },
            "title": format!("Record {k}"), "primary-type": if k % 4 == 0 { "Single" } else { "Album" }, "secondary-types": [], "first-release-date": format!("{}-01-01", 1970 + k) })).collect();
        axum::Json(json!({ "release-groups": rgs, "release-group-count": 40 }))
    }
    let app = Router::new()
        .route("/ws/2/artist/{id}", get(artist))
        .route("/ws/2/release-group", get(groups))
        .fallback(|| async { axum::Json(json!({})) });
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    format!("http://{addr}")
}

/// A server under test.
struct Server {
    /// its state
    st: AppState,
    /// its router
    app: Router,
    /// the owner's session cookie
    cookie: String,
}

/// A server on the database in `dir`, MusicBrainz at `mb`, signed in as the owner.
async fn start(dir: &Path, mb: &str) -> Server {
    let st = encore_api::test_support::state_in(dir);
    // the owner the server made at start, with the bench's name and password
    st.db.execute("UPDATE users SET username=?, password_hash=? WHERE role='owner'", encore_db::p![OWNER, encore_api::auth::hash_password(PW)]).unwrap();
    st.cfg.set_many(&json!({ "musicbrainz_url": mb, "musicbrainz_rate": 1000.0, "library_path": dir.join("music").to_string_lossy() }));
    let app = encore_api::app(st.clone());
    let req = Request::builder().method("POST").uri("/api/auth/login").header("x-requested-with", "encore").header("content-type", "application/json")
        .body(Body::from(json!({ "username": OWNER, "password": PW }).to_string())).unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let cookie = resp.headers().get("set-cookie").and_then(|v| v.to_str().ok()).unwrap_or("").split(';').next().unwrap().to_string();
    Server { st, app, cookie }
}

impl Server {
    /// One call with any method; returns (ms, status).
    async fn call(&self, method: &str, uri: &str) -> (f64, u16) {
        let req = Request::builder().method(method).uri(uri).header("x-requested-with", "encore").header("cookie", &self.cookie).body(Body::empty()).unwrap();
        let t = Instant::now();
        let resp = self.app.clone().oneshot(req).await.unwrap();
        let status = resp.status().as_u16();
        let _ = axum::body::to_bytes(resp.into_body(), 64 << 20).await;
        (t.elapsed().as_secs_f64() * 1000.0, status)
    }

    /// One GET; returns (ms, status, body bytes).
    async fn get(&self, uri: &str) -> (f64, u16, usize) {
        let req = Request::builder().method("GET").uri(uri).header("x-requested-with", "encore").header("cookie", &self.cookie)
            .header("accept", "image/webp,application/json").body(Body::empty()).unwrap();
        let t = Instant::now();
        let resp = self.app.clone().oneshot(req).await.unwrap();
        let status = resp.status().as_u16();
        let bytes = axum::body::to_bytes(resp.into_body(), 64 << 20).await.unwrap();
        (t.elapsed().as_secs_f64() * 1000.0, status, bytes.len())
    }
}

/// Waits for the cover thumbnail pass a first request may have started (so one measurement doesn't
/// pay for the last one's background work).
async fn quiesce(st: &AppState) {
    let t = Instant::now();
    while st.library.covers.progress().2 && t.elapsed().as_secs() < 120 {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

/// The `p` percentile (0–1) of `v`.
fn pct(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((v.len() as f64 - 1.0) * p).round() as usize]
}

/// Copy the database files from `from` to a fresh folder `to`.
fn copy_dir(from: &Path, to: &Path) {
    let _ = std::fs::remove_dir_all(to);
    std::fs::create_dir_all(to).unwrap();
    for f in ["encore.db", "encore.db-wal", "encore.db-shm"] {
        if from.join(f).exists() {
            std::fs::copy(from.join(f), to.join(f)).unwrap();
        }
    }
    // the music folder (covers) is shared: link it
    #[cfg(unix)]
    std::os::unix::fs::symlink(from.join("music"), to.join("music")).unwrap();
}

fn main() {
    tokio::runtime::Builder::new_multi_thread().worker_threads(4).enable_all().build().unwrap().block_on(perf_bench());
}

/// Build the library, then time every endpoint cold and warm and print the table.
async fn perf_bench() {
    let base = std::env::temp_dir().join(format!("encore-perf-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let seed = base.join("seed");
    std::fs::create_dir_all(&seed).unwrap();
    let mb = mock_mb().await;
    let t = Instant::now();
    {
        let s = start(&seed, &mb).await;
        let uid = s.st.db.one("SELECT id FROM users WHERE username=?", encore_db::p![OWNER]).unwrap().unwrap();
        let uid = encore_db::RowExt::i(&uid, "id");
        fill(&seed, uid);
        // translations on for the listener (the worst case: every answer is decorated)
        if std::env::var("BENCH_TRANSLATE").is_ok() {
            s.st.db.execute("UPDATE users SET prefs=? WHERE id=?", encore_db::p![json!({ "translate_mode": "under", "translate_lang": "en" }).to_string(), uid]).unwrap();
        }
        s.st.library.artists.invalidate();
    }
    eprintln!("library built in {:.1} s", t.elapsed().as_secs_f64());

    // ids the pages use
    let s0 = start(&seed, &mb).await;
    let first_album = encore_library::album_key(&artist_name(0), &format!("Album 1 of {}", artist_name(0)), "");
    let liked = encore_db::RowExt::i(&s0.st.db.one("SELECT id FROM playlists WHERE kind='liked'", encore_db::p![]).unwrap().unwrap(), "id");
    let pl = liked + 1;
    let a5 = artist_name(5);
    let ambid = format!("00000000-0000-0000-0000-{:012}", 5);
    // warm the cover file once so "cover (cache hit)" is a hit
    let cover = format!("/api/library/albums/{first_album}/cover?s=400");
    s0.get(&cover).await;
    drop(s0);
    let enc = |s: &str| urlencoding::encode(s).into_owned();
    let mut eps: Vec<(String, String)> = vec![
        ("home: /api/library/recent".into(), "/api/library/recent?limit=12".into()),
        ("home: /api/stats/me".into(), "/api/stats/me".into()),
        ("home: /api/requests (active, mine)".into(), "/api/requests?status=active&mine=true&limit=8".into()),
        ("home: /api/discover/made-for-you".into(), "/api/discover/made-for-you".into()),
        ("home: /api/discover/new-releases".into(), "/api/discover/new-releases?limit=12".into()),
        ("/api/auth/me".into(), "/api/auth/me".into()),
        ("/api/likes".into(), "/api/likes".into()),
        ("/api/library/stats".into(), "/api/library/stats".into()),
        // the side bar's storage meter (every start of the web app and every 5 minutes)
        ("/api/library/storage".into(), "/api/library/storage".into()),
    ];
    for sort in ["name", "artist", "album", "year", "added", "duration", "size"] {
        eps.push((format!("albums sort={sort}"), format!("/api/library/albums?sort={sort}&limit=120")));
    }
    eps.push(("albums q=echo".into(), "/api/library/albums?q=echo&limit=120".into()));
    eps.push(("albums artist=…".into(), format!("/api/library/albums?artist={}", enc(&a5))));
    for sort in ["name", "albums", "tracks", "added"] {
        eps.push((format!("artists sort={sort}"), format!("/api/library/artists?sort={sort}")));
    }
    eps.push(("artists q=neon".into(), "/api/library/artists?q=neon".into()));
    for sort in ["artist", "title", "album", "added", "released", "duration"] {
        eps.push((format!("tracks sort={sort}"), format!("/api/library/tracks?sort={sort}&limit=200")));
    }
    eps.push(("tracks q=song 3".into(), "/api/library/tracks?q=song%203&limit=200".into()));
    eps.push(("/api/library/search q=velvet".into(), "/api/library/search?q=velvet".into()));
    eps.push(("/api/library/search q=echo mirror song".into(), "/api/library/search?q=echo%20mirror%20song".into()));
    for (kind, sort) in [("albums", "name"), ("albums", "added"), ("artists", "name"), ("tracks", "artist"), ("tracks", "title")] {
        eps.push((format!("index {kind} sort={sort}"), format!("/api/library/index?kind={kind}&sort={sort}")));
    }
    eps.extend([
        ("album page".to_string(), format!("/api/library/albums/{first_album}")),
        ("artist page /api/discover/artist/…".into(), format!("/api/discover/artist/{ambid}")),
        ("artist page library-tracks".into(), format!("/api/discover/artist/{ambid}/library-tracks?name={}", enc(&a5))),
        ("library artist-tracks".into(), format!("/api/library/artist-tracks?name={}", enc(&a5))),
        ("/api/stats/leaderboard".into(), "/api/stats/leaderboard".into()),
        ("/api/requests".into(), "/api/requests".into()),
        ("/api/playlists".into(), "/api/playlists".into()),
        ("playlist detail (50)".into(), format!("/api/playlists/{pl}")),
        ("Liked Songs detail (2.4k)".into(), format!("/api/playlists/{liked}")),
        ("/api/remote/spot".into(), "/api/remote/spot".into()),
        ("/api/remote/devices".into(), "/api/remote/devices".into()),
        ("cover 400 (cache hit)".into(), cover.clone()),
        ("/api/system/tools".into(), "/api/system/tools".into()),
        ("/api/youtube/auth".into(), "/api/youtube/auth".into()),
        ("/api/admin/settings".into(), "/api/admin/settings".into()),
        ("/api/admin/tasks".into(), "/api/admin/tasks".into()),
        ("/api/translate/status".into(), "/api/translate/status".into()),
    ]);

    // BENCH_AFTER_DELETE=1: every page warm, then an artist removed, then each page once (what the next
    // page load after a delete costs)
    if std::env::var("BENCH_AFTER_DELETE").is_ok() {
        after_delete(&base, &seed, &mb, &eps).await;
        let _ = std::fs::remove_dir_all(&base);
        return;
    }
    // cold: a fresh server (empty in-memory caches) on a copy of the database, one call.
    // BENCH_COLD=main measures only the calls the main pages make (each cold server's cover pass is
    // waited for, which took a minute per server before 0.25.3)
    let main_only = std::env::var("BENCH_COLD").is_ok_and(|v| v == "main");
    let is_main = |name: &str| name.starts_with("home:") || ["albums sort=name", "artists sort=name", "tracks sort=artist", "/api/library/search q=velvet", "index albums sort=name",
        "album page", "artist page /api/discover/artist/…", "/api/playlists", "Liked Songs detail (2.4k)", "/api/likes", "/api/auth/me"].contains(&name);
    let mut cold = vec![];
    for (i, (name, uri)) in eps.iter().enumerate() {
        if main_only && !is_main(name) {
            cold.push((f64::NAN, 0));
            continue;
        }
        let d = base.join(format!("cold-{i}"));
        copy_dir(&seed, &d);
        let s = start(&d, &mb).await;
        let (ms, status, _) = s.get(uri).await;
        cold.push((ms, status));
        quiesce(&s.st).await;
        drop(s);
        let _ = std::fs::remove_dir_all(&d);
    }
    // warm: one server, 3 calls to settle, then 25 timed
    let s = start(&seed, &mb).await;
    for (_, uri) in &eps {
        s.get(uri).await;
    }
    quiesce(&s.st).await;
    let mut rows = vec![];
    for (i, (name, uri)) in eps.iter().enumerate() {
        for _ in 0..3 {
            s.get(uri).await;
        }
        let mut v = vec![];
        let (mut size, mut code) = (0, 200);
        for _ in 0..25 {
            let (ms, c, n) = s.get(uri).await;
            v.push(ms);
            size = n;
            code = c;
        }
        let name = if code == 200 { name.clone() } else { format!("{name} (HTTP {code})") };
        let (p50, p95) = (pct(&mut v.clone(), 0.5), pct(&mut v, 0.95));
        let c = if cold[i].0.is_nan() { "–".to_string() } else { format!("{:.1}", cold[i].0) };
        rows.push(format!("| {name} | {c} | {:.1} | {:.1} | {} |", p50, p95, size / 1024));
    }
    // a home page load: the six calls at once, as the browser makes them
    let home: Vec<&str> = eps.iter().take(5).map(|(_, u)| u.as_str()).chain(["/api/auth/me"]).collect();
    let mut v = vec![];
    for _ in 0..15 {
        let t = Instant::now();
        futures_util::future::join_all(home.iter().map(|u| s.get(u))).await;
        v.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    rows.push(format!("| **home page, 6 calls in parallel** | | {:.1} | {:.1} | |", pct(&mut v.clone(), 0.5), pct(&mut v, 0.95)));
    // the same while a background job keeps the database busy (scan-like full reads and writes)
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let bg = {
        let (db, stop) = (s.st.db.clone(), stop.clone());
        std::thread::spawn(move || {
            let mut n = 0;
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                let _ = db.query("SELECT path, mtime, size FROM tracks", encore_db::p![]);
                let _ = db.execute("UPDATE tracks SET bitrate=bitrate WHERE id=?", encore_db::p![n % 1000 + 1]);
                n += 1;
            }
        })
    };
    let mut v = vec![];
    for _ in 0..15 {
        let t = Instant::now();
        futures_util::future::join_all(home.iter().map(|u| s.get(u))).await;
        v.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    bg.join().unwrap();
    rows.push(format!("| **home page while a background job reads the whole library** | | {:.1} | {:.1} | |", pct(&mut v.clone(), 0.5), pct(&mut v, 0.95)));
    // paging through the songs list: ten pages nobody opened yet, one after the other (since 0.26 a page is a
    // slice of one cached id list; the 0.25.3 WIP ran the sorted page query below for every new page)
    let t = Instant::now();
    for i in 0..10 {
        s.get(&format!("/api/library/tracks?sort=title&limit=200&offset={}", 2000 + i * 200)).await;
    }
    rows.push(format!("| **songs list: 10 pages nobody opened yet, one after the other (total)** | | {:.1} | | |", t.elapsed().as_secs_f64() * 1000.0));
    let t = Instant::now();
    for i in 0..10 {
        let _ = s.st.db.query(&format!("SELECT {} FROM tracks ORDER BY lower(title) ASC, id LIMIT 200 OFFSET {}", encore_library::lists::TRACK_COLS, 4000 + i * 200), encore_db::p![]);
    }
    rows.push(format!("| **the same 10 pages as sorted page queries (what each new page cost before 0.26, database only)** | | {:.1} | | |", t.elapsed().as_secs_f64() * 1000.0));
    println!("\n| endpoint | cold ms | warm p50 ms | warm p95 ms | KiB |\n|---|---|---|---|---|");
    for r in rows {
        println!("{r}");
    }
    drop(s);
    let _ = std::fs::remove_dir_all(&base);
}

/// Each page once after an artist is removed, with every page warm before: three rounds (three artists).
async fn after_delete(base: &Path, seed: &Path, mb: &str, eps: &[(String, String)]) {
    let d = base.join("after-delete");
    copy_dir(seed, &d);
    let s = start(&d, mb).await;
    let mut rows: Vec<(String, Vec<f64>)> = eps.iter().map(|(n, _)| (n.clone(), vec![])).collect();
    let mut deletes = vec![];
    let mut totals = vec![];
    for round in 0..3 {
        for (_, uri) in eps {
            s.get(uri).await;
        }
        quiesce(&s.st).await;
        let (ms, status) = s.call("DELETE", &format!("/api/library/artists?name={}", urlencoding::encode(&artist_name(100 + round)))).await;
        assert_eq!(status, 200);
        deletes.push(ms);
        let t = Instant::now();
        for (i, (_, uri)) in eps.iter().enumerate() {
            rows[i].1.push(s.get(uri).await.0);
        }
        totals.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    println!("\n| after an artist is removed | round 1 ms | round 2 ms | round 3 ms |\n|---|---|---|---|");
    println!("| **DELETE /api/library/artists** | {:.1} | {:.1} | {:.1} |", deletes[0], deletes[1], deletes[2]);
    for (name, v) in rows {
        println!("| {name} | {:.1} | {:.1} | {:.1} |", v[0], v[1], v[2]);
    }
    println!("| **every page once (total)** | {:.1} | {:.1} | {:.1} |", totals[0], totals[1], totals[2]);
}
