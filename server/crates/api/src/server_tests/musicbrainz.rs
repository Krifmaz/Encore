//! MusicBrainz's line on a whole server (the owner on Oct 21: "why is it taking so long to import?"), against a
//! stand-in MusicBrainz and a stand-in Deezer on 127.0.0.1: an import's lookups go before background work and
//! after a page's, and its message says what it is at and what it waits for; the album lookup of a song ends
//! (the album Deezer names – never a best-of –, the song on its own at once while MusicBrainz fails, or after its
//! tries while it is busy) and says when it tries again; one album request serves the songs of an album however
//! it was found; a restart keeps what MusicBrainz already said; a 503 still backs off and tries again; one
//! look-up asked for by several pages at once is one call, and a look-up is answered from disk after a restart
//! (an old one at once for a page, then asked again in the background).
use crate::state::AppState;
use crate::test_support::{user, TestServer};
use encore_db::{now, p, RowExt};
use encore_http::mb::{self, Priority};
use encore_requests::worker;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// What the stand-in MusicBrainz was asked, in order.
type Hits = Arc<Mutex<Vec<String>>>;

/// How the stand-in MusicBrainz answers a recording, the ISRC lookup and the recording search: at once, 500
/// (it fails) or 503 (it is busy).
const UP: u8 = 0;
const DOWN: u8 = 1;
const BUSY: u8 = 2;

/// A stand-in MusicBrainz (`/ws/2/…`): artist and release-group lookups answer at once (the calls that fill
/// the line); a recording, the ISRC lookup and the recording search answer as `mode` says; `label/busy` says
/// 503 twice, then answers. → (its address, what it was asked, `mode`)
async fn musicbrainz() -> (String, Hits, Arc<AtomicU8>) {
    use axum::extract::{Path as P, State};
    use axum::http::StatusCode;
    use axum::routing::get;
    #[derive(Clone)]
    struct S {
        hits: Hits,
        mode: Arc<AtomicU8>,
        busy: Arc<AtomicUsize>,
    }
    fn hit(s: &S, what: String) {
        s.hits.lock().unwrap().push(what);
    }
    fn failing(s: &S) -> Option<StatusCode> {
        match s.mode.load(Ordering::SeqCst) {
            DOWN => Some(StatusCode::INTERNAL_SERVER_ERROR),
            BUSY => Some(StatusCode::SERVICE_UNAVAILABLE),
            _ => None,
        }
    }
    let s = S { hits: Hits::default(), mode: Arc::new(AtomicU8::new(UP)), busy: Arc::default() };
    let app = axum::Router::new()
        .route("/ws/2/artist/{id}", get(|State(s): State<S>, P(id): P<String>| async move { hit(&s, format!("artist/{id}")); axum::Json(json!({ "id": id })) }))
        .route("/ws/2/release-group/{id}", get(|State(s): State<S>, P(id): P<String>| async move { hit(&s, format!("release-group/{id}")); axum::Json(json!({ "id": id })) }))
        .route("/ws/2/recording/{id}", get(|State(s): State<S>, P(id): P<String>| async move {
            hit(&s, format!("recording/{id}"));
            if let Some(code) = failing(&s) {
                return Err(code);
            }
            Ok(axum::Json(json!({ "id": id, "title": "Song", "length": 3000, "isrcs": ["USX9P1234567"], "releases": [],
                                  "artist-credit": [{ "name": "Band", "artist": { "id": "band-1", "name": "Band" } }] })))
        }))
        .route("/ws/2/recording", get(|State(s): State<S>| async move {
            hit(&s, "search".into());
            match failing(&s) { Some(code) => Err(code), None => Ok(axum::Json(json!({ "recordings": [] }))) }
        }))
        .route("/ws/2/isrc/{isrc}", get(|State(s): State<S>, P(isrc): P<String>| async move {
            hit(&s, format!("isrc/{isrc}"));
            match failing(&s) { Some(code) => Err(code), None => Ok(axum::Json(json!({ "recordings": [] }))) }
        }))
        .route("/ws/2/label/busy", get(|State(s): State<S>| async move {
            hit(&s, "label/busy".into());
            if s.busy.fetch_add(1, Ordering::SeqCst) < 2 { Err(StatusCode::SERVICE_UNAVAILABLE) } else { Ok(axum::Json(json!({ "name": "Label" }))) }
        }))
        .with_state(s.clone());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move {
        let _ = axum::serve(l, app).await;
    });
    (url, s.hits, s.mode)
}

/// A stand-in Deezer: the track with ISRC USX9P1234567 is on the album "Discovery" (302127) by Band, the one
/// with ISRC USX9P7654321 on Band's "Greatest Hits" (1109731).
async fn deezer() -> String {
    use axum::extract::Path as P;
    use axum::routing::get;
    let app = axum::Router::new()
        .route("/track/{key}", get(|P(key): P<String>| async move {
            if key == "isrc:USX9P1234567" {
                axum::Json(json!({ "id": 3135556, "title": "Song", "album": { "id": 302127, "title": "Discovery" } }))
            } else if key == "isrc:USX9P7654321" {
                axum::Json(json!({ "id": 3135557, "title": "Other Song", "album": { "id": 1109731, "title": "Greatest Hits" } }))
            } else {
                axum::Json(json!({ "error": { "type": "DataException", "message": "no data", "code": 800 } }))
            }
        }))
        .route("/album/{id}", get(|P(id): P<i64>| async move {
            if id == 1109731 {
                return axum::Json(json!({ "id": id, "title": "Greatest Hits", "record_type": "album", "artist": { "id": 27, "name": "Band" },
                                          "release_date": "2010-11-15", "upc": "5099990000000" }));
            }
            axum::Json(json!({ "id": id, "title": "Discovery", "record_type": "album", "artist": { "id": 27, "name": "Band" },
                               "release_date": "2001-03-07", "upc": "724384960650", "cover_xl": "http://127.0.0.1:9/cover.jpg" }))
        }));
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move {
        let _ = axum::serve(l, app).await;
    });
    url
}

/// A server that asks the stand-ins, two MusicBrainz calls a second, with its own library and no download
/// source; no audio fingerprint or sound check on import, no lyrics.
async fn server(name: &str) -> (TestServer, Hits, Arc<AtomicU8>) {
    let (mb_url, hits, mode) = musicbrainz().await;
    let dz = deezer().await;
    let ts = TestServer::new(&format!("mb-{name}")).with_urls(encore_http::Urls { deezer: dz, ..Default::default() });
    let (lib, staging) = (ts.dir.join("music"), ts.dir.join("staging"));
    std::fs::create_dir_all(&lib).unwrap();
    ts.st.cfg.set_many(&json!({ "library_path": lib.to_string_lossy(), "staging_path": staging.to_string_lossy(), "musicbrainz_url": mb_url,
        "musicbrainz_rate": 2.0, "verify_on_import": false, "quality_check_on_import": false, "lyrics_on_import": false, "auto_approve": true,
        "album_upgrade": "album_ep", "slskd_enabled": false, "ytdlp_enabled": false, "ytdlp_last_resort": false, "streamrip_enabled": false }));
    (ts, hits, mode)
}

fn row(st: &AppState, id: i64) -> (String, String, Value) {
    let r = st.db.one("SELECT status, message, detail FROM requests WHERE id=?", p![id]).unwrap().unwrap();
    (r.s("status"), r.s("message"), serde_json::from_str(&r.s("detail")).unwrap_or_else(|_| json!({})))
}

/// Every message request `id` shows while `work` runs (read every 20 ms).
async fn messages<F: std::future::Future>(st: &AppState, id: i64, work: F) -> (F::Output, Vec<String>) {
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let stop = Arc::new(AtomicBool::new(false));
    let (s, sn, sp) = (st.clone(), seen.clone(), stop.clone());
    let watcher = tokio::spawn(async move {
        while !sp.load(Ordering::SeqCst) {
            let m = row(&s, id).1;
            {
                let mut sn = sn.lock().unwrap();
                if sn.last() != Some(&m) {
                    sn.push(m);
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    });
    let out = work.await;
    stop.store(true, Ordering::SeqCst);
    let _ = watcher.await;
    let seen = seen.lock().unwrap().clone();
    (out, seen)
}

/// `n` calls to MusicBrainz at priority `p` that wait in the line now (each its own path, so none is cached).
async fn fill_line(st: &AppState, p: Option<Priority>, prefix: &str, n: usize) -> Vec<tokio::task::JoinHandle<bool>> {
    let mut hs = vec![];
    for i in 0..n {
        let s = st.clone();
        let path = format!("{prefix}-{i}");
        let call = async move { s.mb.get(&s, &path, &[]).await.is_ok() };
        hs.push(match p {
            Some(p) => tokio::spawn(mb::with_priority(p, call)),
            None => tokio::spawn(call),
        });
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    hs
}

/// A short FLAC (false when there is no ffmpeg here).
fn flac(path: &Path) -> bool {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::process::Command::new("ffmpeg").args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=frequency=440:duration=3"])
        .args(["-metadata", "title=Song", "-metadata", "artist=Band"]).arg(path).status().map(|s| s.success()).unwrap_or(false)
}

/// SHE-DEVIL's case: a song from Soulseek whose import runs again from its kept files (as after an upgrade's
/// restart) while MusicBrainz's line is long. Its lookup goes after the pages someone is looking at and before
/// the background work that waited longer, it asks for the recording the album lookup found (no search), and
/// its message says what it is at and what it waits for – "Importing – waiting for MusicBrainz (…, 3 lookups
/// ahead)" – instead of "Importing the downloaded files…" for ten minutes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_import_says_what_it_waits_for_and_goes_before_background_work() {
    let (ts, hits, _) = server("import").await;
    let st = ts.st.clone();
    let dl = ts.dir.join("slskd/SHE-DEVIL");
    let f = dl.join("01 Song.flac");
    let Some(cover) = super::streamrip::cover_server(&ts.dir).await else { return };
    if !flac(&f) {
        return;
    }
    let admin = user(&st, "boss", "admin").i("id");
    let t = now();
    let d = json!({ "recording": "rec-1", "wait": { "reason": "storage_import", "since": t, "next_try_at": t },
                    "kept": { "files": [f], "root": dl, "source": "slskd_lossless", "how": "request", "own": false,
                              "origin": { "source": "Soulseek" }, "done": "Downloaded from Soulseek", "reason": "storage" } });
    let id = st.db.execute("INSERT INTO requests (user_id, kind, artist, title, cover_url, status, source, progress, message, detail, created_at, updated_at)
                            VALUES (?,?,?,?,?,?,?,?,?,?,?,?)",
        p![admin, "track", "Band", "Song", cover, "waiting", "slskd_lossless", 100, "Waiting to import the downloaded files", d.to_string(), t, t]).unwrap();
    // ten page lookups in the line (five seconds of it), then six of background work
    let pages = fill_line(&st, Some(Priority::Interactive), "artist/page", 10).await;
    let background = fill_line(&st, None, "release-group/bg", 6).await;
    let ((), seen) = messages(&st, id, worker::step(st.clone(), id)).await;
    for h in pages.into_iter().chain(background) {
        assert!(h.await.unwrap());
    }
    let (status, message, _) = row(&st, id);
    assert_eq!((status.as_str(), message.as_str()), ("completed", "Downloaded from Soulseek"), "{seen:?}");
    assert!(seen.iter().any(|m| m == "Importing – looking it up on MusicBrainz"), "{seen:?}");
    assert!(seen.iter().any(|m| m.starts_with("Importing – waiting for MusicBrainz (under a minute, ") && m.ends_with(" ahead)")), "{seen:?}");
    // (the steps after it – the cover, the move into the library – are too quick here to be seen every time)
    // the order MusicBrainz was asked in: every page, then the import, then the background work
    let hits = hits.lock().unwrap().clone();
    let at = |p: &str| hits.iter().position(|h| h == p).unwrap_or_else(|| panic!("{p} not asked: {hits:?}"));
    let import = at("recording/rec-1");
    assert!((0..10).all(|i| at(&format!("artist/page-{i}")) < import), "{hits:?}");
    assert!((0..6).all(|i| at(&format!("release-group/bg-{i}")) > import), "{hits:?}");
    // the recording the album lookup found was asked for alone: no search
    assert!(!hits.iter().any(|h| h == "search"), "{hits:?}");
    ts.remove();
}

/// While MusicBrainz can't say which album a song is on (it is down here), the album Deezer names for the song's
/// ISRC is fetched instead – the whole album, as the album lookup would have – and while the lookup waits in
/// MusicBrainz's line its message says so.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_album_lookup_takes_the_album_deezer_names_when_musicbrainz_cant_say() {
    let (ts, _, mode) = server("deezer-album").await;
    let st = ts.st.clone();
    mode.store(DOWN, Ordering::SeqCst);
    let admin = user(&st, "boss", "admin").i("id");
    let t = now();
    let id = st.db.execute("INSERT INTO requests (user_id, kind, artist, title, status, message, detail, created_at, updated_at) VALUES (?,?,?,?,?,?,?,?,?)",
        p![admin, "track", "Band", "Song", "queued", "", json!({ "isrc": "USX9P1234567" }).to_string(), t, t]).unwrap();
    // six imports' lookups ahead of it (three seconds)
    let imports = fill_line(&st, Some(Priority::Import), "artist/import", 6).await;
    let ((), seen) = messages(&st, id, mb::with_priority(Priority::Request, worker::step(st.clone(), id))).await;
    for h in imports {
        assert!(h.await.unwrap());
    }
    assert!(seen.iter().any(|m| m.starts_with("Looking up which album this song is on – waiting for MusicBrainz (under a minute, ")), "{seen:?}");
    let (status, message, d) = row(&st, id);
    assert_eq!((status.as_str(), message.as_str()), ("monitoring", "Getting the whole album “Discovery”"));
    assert_eq!(d["album_note"], "MusicBrainz didn't answer – Deezer says the song is on the album “Discovery”");
    let alb = st.db.one("SELECT * FROM requests WHERE id=?", p![d["album_request"].as_i64().unwrap()]).unwrap().unwrap();
    assert_eq!((alb.s("kind").as_str(), alb.s("artist").as_str(), alb.s("title").as_str(), alb.s("year").as_str(), alb.s("mbid").as_str()), ("album", "Band", "Discovery", "2001", ""));
    let ad: Value = serde_json::from_str(&alb.s("detail")).unwrap();
    assert_eq!(ad["ext"], json!({ "deezer": 302127, "upc": "724384960650" }));
    ts.remove();
}

fn track(st: &AppState, user: i64, artist: &str, title: &str, detail: Value) -> i64 {
    let t = now();
    st.db.execute("INSERT INTO requests (user_id, kind, artist, title, status, message, detail, created_at, updated_at) VALUES (?,?,?,?,?,?,?,?,?)",
        p![user, "track", artist, title, "queued", "", detail.to_string(), t, t]).unwrap()
}

fn log_of(d: &Value) -> Vec<String> {
    d["log"].as_array().map(|a| a.iter().filter_map(|l| l.as_str().map(String::from)).collect()).unwrap_or_default()
}

/// MusicBrainz is busy (503 after its back-off) and Deezer doesn't know the song: the lookup says why and when
/// it tries again ("… – MusicBrainz is busy. Next try at 14:20"), and after its third try the song goes on its
/// own, the request's details saying why. (It used to wait in the line without an end.)
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_album_lookup_says_when_it_tries_again_and_ends_after_its_tries() {
    let (ts, _, mode) = server("tries").await;
    let st = ts.st.clone();
    mode.store(BUSY, Ordering::SeqCst);
    let admin = user(&st, "boss", "admin").i("id");
    let first = track(&st, admin, "JustWarrenPeace", "Outside", json!({}));
    // its last try at the same time (an auto-download's third attempt): the song on its own, and with no source
    // here it fails
    let d = json!({ "auto": true, "attempts": 3, "album_lookup": { "tries": worker::ALBUM_TRIES - 1, "next_try_at": now() - 1.0, "why": "MusicBrainz is busy", "in_line": false } });
    let last = track(&st, admin, "SauceDad69", "TALK2ME", d);
    // (each busy answer backs off 1.5 + 3 + 4.5 + 6 s before the lookup hears MusicBrainz is busy)
    let s = st.clone();
    let first_try = tokio::spawn(tokio::time::timeout(Duration::from_secs(25), mb::with_priority(Priority::Request, worker::step(s, first))));
    mb::with_priority(Priority::Request, worker::step(st.clone(), last)).await;
    let (status, message, d) = row(&st, last);
    assert_eq!((status.as_str(), message.as_str()), ("failed", "All sources exhausted (no download sources enabled)"), "{d}");
    assert_eq!(d["no_album"], true);
    assert!(d["album_checked_at"].is_null() && d["album_lookup"].is_null(), "a lookup that gave up is not kept for the next attempt: {d}");
    let log = log_of(&d);
    assert_eq!(log[0], "album lookup: MusicBrainz is busy – after 3 tries the song is downloaded on its own");
    assert!(log.iter().any(|l| l.starts_with("auto-download: asks for it again after ") && l.ends_with("(the wait doubles after each failed attempt, at most a day)")), "{log:?}");

    // its first try: it waits for the next one (five minutes; the test doesn't)
    let t0 = std::time::Instant::now();
    loop {
        let (_, message, _) = row(&st, first);
        if message.contains("Next try") || t0.elapsed() > Duration::from_secs(20) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    first_try.abort();
    let (status, message, d) = row(&st, first);
    assert_eq!(status, "searching");
    assert!(message.starts_with("Looking up which album this song is on – MusicBrainz is busy. Next try at "), "{message}");
    assert_eq!(d["album_lookup"]["tries"], 1);
    assert_eq!(d["album_lookup"]["in_line"], false, "MusicBrainz said it is busy: the try waits its whole time");
    let at = d["album_lookup"]["next_try_at"].as_f64().unwrap();
    assert!((at - now() - worker::ALBUM_RETRY_SECS).abs() < 15.0, "{d}");
    assert!(d["no_album"].is_null());
    ts.remove();
}

/// MusicBrainz fails (a 500): the song goes on its own at once, as it did before the album lookup had tries –
/// not after ten minutes of them – and the album Deezer names for it is a best-of, which is not the album the
/// song is from.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failing_musicbrainz_sends_the_song_on_its_own_at_once() {
    let (ts, _, mode) = server("failing").await;
    let st = ts.st.clone();
    mode.store(DOWN, Ordering::SeqCst);
    let admin = user(&st, "boss", "admin").i("id");
    let id = track(&st, admin, "Band", "Other Song", json!({ "isrc": "USX9P7654321" }));
    let t0 = std::time::Instant::now();
    tokio::time::timeout(Duration::from_secs(20), mb::with_priority(Priority::Request, worker::step(st.clone(), id))).await.expect("no wait between tries");
    assert!(t0.elapsed() < Duration::from_secs(10), "{:?}", t0.elapsed());
    let (status, message, d) = row(&st, id);
    assert_eq!((status.as_str(), message.as_str()), ("failed", "All sources exhausted (no download sources enabled)"), "{d}");
    assert_eq!(d["no_album"], true);
    assert!(d["album_lookup"].is_null() && d["album_checked_at"].is_null() && d["album_request"].is_null(), "{d}");
    assert_eq!(log_of(&d)[0], "album lookup: MusicBrainz didn't answer – the song is downloaded on its own");
    let albums = st.db.one("SELECT COUNT(*) AS n FROM requests WHERE kind='album'", p![]).unwrap().unwrap().i("n");
    assert_eq!(albums, 0, "Band's Greatest Hits is not requested for the song");
    ts.remove();
}

/// The album request already there for the album is the one the song follows, however it was made: a
/// MusicBrainz release group of the same name (edition notes aside), or the same Deezer album under another
/// name. MusicBrainz's answer for one song and Deezer's for another song of the album used to make two album
/// requests that both downloaded.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_songs_of_one_album_follow_one_album_request() {
    let (ts, _, mode) = server("one-album").await;
    let st = ts.st.clone();
    mode.store(DOWN, Ordering::SeqCst);
    let admin = user(&st, "boss", "admin").i("id");
    let t = now();
    let album = |mbid: Option<&str>, title: &str, detail: Value| st.db.execute("INSERT INTO requests (user_id, kind, mbid, artist, title, album, status, message, detail, created_at, updated_at)
                                                                          VALUES (?,?,?,?,?,?,?,?,?,?,?)",
        p![admin, "album", mbid, "Band", title, title, "queued", "Full album for “Song”", detail.to_string(), t, t]).unwrap();
    let count = || st.db.one("SELECT COUNT(*) AS n FROM requests WHERE kind='album'", p![]).unwrap().unwrap().i("n");
    // MusicBrainz's "Discovery (Remastered)", requested for another song before MusicBrainz got busy
    let by_name = album(Some("rg-discovery"), "Discovery (Remastered)", json!({}));
    let song = track(&st, admin, "Band", "Song", json!({ "isrc": "USX9P1234567" }));
    mb::with_priority(Priority::Request, worker::step(st.clone(), song)).await;
    let (status, _, d) = row(&st, song);
    assert_eq!((status.as_str(), d["album_request"].as_i64()), ("monitoring", Some(by_name)), "{d}");
    assert_eq!(count(), 1);
    // the same Deezer album under another name
    st.db.execute("UPDATE requests SET status='cancelled' WHERE id=?", p![by_name]).unwrap();
    let by_id = album(None, "Discovery 2001", json!({ "ext": { "deezer": 302127 } }));
    let song = track(&st, admin, "Band", "Song", json!({ "isrc": "USX9P1234567" }));
    mb::with_priority(Priority::Request, worker::step(st.clone(), song)).await;
    let (_, _, d) = row(&st, song);
    assert_eq!(d["album_request"].as_i64(), Some(by_id), "{d}");
    assert_eq!(count(), 2);
    ts.remove();
}

/// A restart keeps what MusicBrainz already said about a request – which album its song is on (none), its
/// recording, the album's songs – so it doesn't ask again for every request at once after an upgrade; a
/// lossless upgrade stays one (it used to come back as a plain request, find the lossy copy in the library and
/// end "Already in library"), and so does an auto-download.
#[tokio::test]
async fn a_restart_keeps_what_musicbrainz_said() {
    let (ts, _, _) = server("recover").await;
    let st = ts.st.clone();
    let admin = user(&st, "boss", "admin").i("id");
    // whole seconds: serde_json reads some fractions back one ulp off (1792641925.3096 → …5.3095999)
    let t = now().floor();
    let kept = json!({ "no_album": true, "recording": "rec-1", "album_checked_at": t, "album_lookup": { "tries": 1 }, "album_note": "n",
                       "release": { "id": "rel-1", "tracks": [] }, "lossless_only": true, "auto": true, "isrc": "X", "attempts": 2 });
    let mut d = kept.clone();
    d["sources"] = json!(["ytdlp"]);
    d["slskd"] = json!({ "idx": 0 });
    let id = st.db.execute("INSERT INTO requests (user_id, kind, artist, title, status, source, message, detail, created_at, updated_at) VALUES (?,?,?,?,?,?,?,?,?,?)",
        p![admin, "album", "Band", "Discovery", "searching", "streamrip", "Looking for an exact lossless match…", d.to_string(), t, t]).unwrap();
    worker::recover(&st);
    let (status, message, d) = row(&st, id);
    assert_eq!((status.as_str(), message.as_str()), ("queued", "Restarted after server restart"));
    for (k, v) in kept.as_object().unwrap() {
        assert_eq!(&d[k], v, "{k}: {d}");
    }
    assert!(d["sources"].is_null() && d["slskd"].is_null(), "{d}");
    ts.remove();
}

/// The line keeps MusicBrainz's 503 back-off: a call told twice that it is busy waits 1.5 s and 3 s and gets its
/// answer on the third go.
#[tokio::test]
async fn a_busy_musicbrainz_is_asked_again_after_its_back_off() {
    let (ts, hits, _) = server("busy").await;
    let st = ts.st.clone();
    let t0 = std::time::Instant::now();
    let v = st.mb.get(&st, "label/busy", &[]).await.unwrap();
    assert_eq!(v["name"], "Label");
    assert!(t0.elapsed() >= Duration::from_millis(4500), "{:?}", t0.elapsed());
    assert_eq!(hits.lock().unwrap().iter().filter(|h| *h == "label/busy").count(), 3);
    // and the answer is kept: asked again, MusicBrainz isn't
    st.mb.get(&st, "label/busy", &[]).await.unwrap();
    assert_eq!(hits.lock().unwrap().iter().filter(|h| *h == "label/busy").count(), 3);
    ts.remove();
}

/// The same look-up asked for four times at once (the Android artist page and its sections) is one call: the
/// others wait for its answer instead of taking turns of their own.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_lookup_asked_at_once_is_one_call() {
    let (ts, hits, _) = server("one-flight").await;
    let st = ts.st.clone();
    let calls: Vec<_> = (0..4).map(|_| {
        let st = st.clone();
        tokio::spawn(mb::with_priority(Priority::Interactive, async move { st.mb.get(&st, "artist/flight", &[]).await.map(|v| v["id"].clone()) }))
    }).collect();
    for c in calls {
        assert_eq!(c.await.unwrap().unwrap(), json!("flight"));
    }
    assert_eq!(hits.lock().unwrap().iter().filter(|h| *h == "artist/flight").count(), 1);
    ts.remove();
}

/// After a restart a look-up is answered from disk: under an hour old as it is; older, a page gets it at once and
/// it is asked again in the background (work nobody waits for asks MusicBrainz instead). Searches aren't kept.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_restart_answers_lookups_from_disk() {
    let (ts, hits, _) = server("disk").await;
    let st = ts.st.clone();
    let n = |what: &str| hits.lock().unwrap().iter().filter(|h| *h == what).count();
    let page = |mbv: mb::Mb, path: &'static str| {
        let st = st.clone();
        mb::with_priority(Priority::Interactive, async move { mbv.get(&st, path, &[]).await.unwrap() })
    };
    page(st.mb.clone(), "artist/kept").await;
    st.mb.get(&st, "recording", &[("query", "x".to_string())]).await.unwrap();
    assert_eq!((n("artist/kept"), n("search")), (1, 1));
    // the answer is written off the request's thread: wait until it is there
    let file = ts.dir.join("musicbrainz-cache.db");
    for _ in 0..100 {
        if mb::Mb::with_store(&file).stored() == Some(1) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    // a restart: a new line and an empty memory on the same file
    let fresh = mb::Mb::with_store(&file);
    assert_eq!(fresh.stored(), Some(1), "only the look-up is on disk, not the search");
    page(fresh.clone(), "artist/kept").await;
    assert_eq!(n("artist/kept"), 1, "under an hour old: from disk");
    // two hours old: a page has it at once, and the refresh asks MusicBrainz again
    rusqlite::Connection::open(&file).unwrap().execute("UPDATE answers SET fetched_at = fetched_at - 7200", []).unwrap();
    let old = mb::Mb::with_store(&file);
    let refresh = { let (old, st) = (old.clone(), st.clone()); tokio::spawn(async move { old.refresh_stale(&st).await }) };
    assert_eq!(page(old.clone(), "artist/kept").await["id"], "kept");
    for _ in 0..100 {
        if n("artist/kept") == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(n("artist/kept"), 2, "asked again in the background");
    refresh.abort();
    // the refreshed answer reaches the file (written off the thread), then is made old again
    let age = || rusqlite::Connection::open(&file).unwrap().query_row("SELECT ? - fetched_at FROM answers", [now()], |r| r.get::<_, f64>(0)).unwrap();
    for _ in 0..100 {
        if age() < 60.0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    // background work doesn't take an old answer: it asks
    rusqlite::Connection::open(&file).unwrap().execute("UPDATE answers SET fetched_at = fetched_at - 7200", []).unwrap();
    let bg = mb::Mb::with_store(&file);
    mb::with_priority(Priority::Background, bg.get(&st, "artist/kept", &[])).await.unwrap();
    assert_eq!(n("artist/kept"), 3);
    ts.remove();
}

/// A MusicBrainz nobody can reach: a port on 127.0.0.1 nothing listens on, so every call is refused at once
/// (reqwest's "error sending request", as on the owner's TV on Oct 25).
const NOWHERE: &str = "http://127.0.0.1:1";

/// The owner on Oct 25: an artist page on the TV was a full-screen "MusicBrainz error: error sending request for
/// url (https://musicbrainz.org/ws/2/artist/…)". While MusicBrainz can't be reached the page is the library's
/// (its albums and songs, `mb_unavailable` set), and an artist the library doesn't have is an error a person
/// can read: never a URL.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unreachable_musicbrainz_still_shows_the_library_artist_page() {
    let ts = TestServer::new("mb-unreachable");
    let st = ts.st.clone();
    st.cfg.set_many(&json!({ "musicbrainz_url": NOWHERE, "musicbrainz_rate": 50.0 }));
    let mbid = "d72ae77d-ca32-498d-870e-8204165b8ee4";
    for (i, title) in ["One", "Two"].iter().enumerate() {
        st.db.execute("INSERT INTO tracks (path, album_id, album, albumartist, artist, title, track, year, mb_albumartistid, mb_artistid) VALUES (?,?,?,?,?,?,?,?,?,?)",
            p![format!("/m/band/{i}.flac"), "band-album", "Album", "Band", "Band", *title, i as i64 + 1, "2020", mbid, mbid]).unwrap();
    }
    user(&st, "tv-box", "user");
    let app = ts.app();
    let (code, _, cookie) = crate::test_support::call(&app, "POST", "/api/auth/login", "", Some(json!({ "username": "tv-box", "password": "a-long-password-1" }))).await;
    assert_eq!(code, axum::http::StatusCode::OK);
    let cookie = cookie.split(';').next().unwrap_or("").to_string();
    let t0 = std::time::Instant::now();
    let (code, page, _) = crate::test_support::call(&app, "GET", &format!("/api/discover/artist/{mbid}"), &cookie, None).await;
    assert_eq!(code, axum::http::StatusCode::OK, "{page}");
    assert_eq!(page["name"], "Band");
    assert_eq!(page["mb_unavailable"], true);
    assert_eq!(page["library_albums"].as_array().map(|a| a.len()), Some(1), "{page}");
    assert_eq!(page["library_tracks"], 2);
    assert_eq!(page["release_groups"], json!([]));
    assert!(!page["mb_message"].as_str().unwrap().contains("http"), "{page}");
    // three tries, 1 s and 2 s apart
    assert!(t0.elapsed() >= Duration::from_secs(3), "tried again: {:?}", t0.elapsed());
    // nothing in the library: an error, in words
    let (code, e, _) = crate::test_support::call(&app, "GET", "/api/discover/artist/00000000-0000-0000-0000-000000000000", &cookie, None).await;
    assert_eq!(code, axum::http::StatusCode::BAD_GATEWAY, "{e}");
    let text = e.to_string();
    assert!(!text.contains("http") && !text.contains("ws/2"), "{text}");
    assert!(text.contains("Couldn't reach MusicBrainz"), "{text}");
    ts.remove();
}

/// While MusicBrainz can't be reached, a look-up kept on disk is the answer however old it is (background work
/// too, which otherwise asks again past an hour), instead of an error.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unreachable_musicbrainz_falls_back_to_the_answer_on_disk() {
    let ts = TestServer::new("mb-unreachable-disk");
    let st = ts.st.clone();
    st.cfg.set_many(&json!({ "musicbrainz_url": NOWHERE, "musicbrainz_rate": 50.0 }));
    let file = ts.dir.join("mb-old.db");
    // a day-old answer for the artist, as the key `Mb::get` uses (fmt=json among the parameters)
    encore_http::mb_store::Store::open(&file).unwrap().put("artist/old?fmt=json", &json!({ "id": "old" }), now() - 86400.0);
    let m = mb::Mb::with_store(&file);
    let v = mb::with_priority(Priority::Background, m.get(&st, "artist/old", &[])).await.unwrap();
    assert_eq!(v["id"], "old");
    // a search has nothing on disk: the transport error, which a person is shown in words
    let e = m.get(&st, "artist", &[("query", "x".to_string())]).await.unwrap_err();
    assert!(mb::user_message(&e).starts_with("Couldn't reach MusicBrainz"), "{}", mb::user_message(&e));
    ts.remove();
}
