//! Torrents and Usenet on a whole server (encore_requests::worker with the torznab source), against a
//! stand-in for Prowlarr and qBittorrent: an album request reaches the indexers and qBittorrent gets the
//! release from Encore; a client that doesn't answer for a while, a torrent it doesn't list yet, a queued
//! torrent and a finished download in a folder Encore can't see no longer throw the download away.
use crate::state::AppState;
use crate::test_support::{user, TestServer};
use axum::extract::{Multipart, Query};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::Router;
use encore_db::{now, p, RowExt};
use encore_requests::worker;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

const HASH: &str = "c12fe1c06bba254a9dc9f519b335aa7c1367a88a";

/// What the stand-in was asked and what it answers.
#[derive(Default)]
struct Fake {
    /// Prowlarr searches: the query words
    searches: Vec<String>,
    /// qBittorrent calls: (path, fields)
    calls: Vec<(String, Value)>,
    /// qBittorrent answers HTTP 500 to everything (restarting, or behind a VPN that reconnects)
    down: bool,
    /// what torrents/info lists
    listed: Option<Value>,
}

type Shared = Arc<Mutex<Fake>>;

fn release(base: &str, title: &str, mb: f64, seeds: i64) -> Value {
    // Prowlarr hands its own proxy link, also for a magnet
    json!({ "guid": title, "title": title, "size": mb * 1e6, "seeders": seeds, "indexer": "Tracker", "protocol": "torrent",
            "magnetUrl": format!("{base}/1/download?apikey=k&link={}", title.len()) })
}

async fn fields(mut m: Multipart) -> Value {
    let mut v = json!({});
    while let Ok(Some(f)) = m.next_field().await {
        let k = f.name().unwrap_or("").to_string();
        v[k] = json!(f.text().await.unwrap_or_default());
    }
    v
}

/// Prowlarr and qBittorrent on one port.
async fn fake(f: Shared) -> String {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", l.local_addr().unwrap());
    let (f1, f2, f3, f4, b1) = (f.clone(), f.clone(), f.clone(), f.clone(), base.clone());
    let app = Router::new()
        .route("/api/v1/search", get(move |Query(q): Query<HashMap<String, String>>| {
            let (f, base) = (f1.clone(), b1.clone());
            async move {
                let words = q.get("query").cloned().unwrap_or_default();
                f.lock().unwrap().searches.push(words.clone());
                let list = if words == "Travis Scott Rodeo" {
                    vec![release(&base, "Travis Scott - Rodeo (2015) [FLAC]", 700.0, 40), release(&base, "Travis Scott - Days Before Rodeo (2014) [FLAC]", 600.0, 90),
                         release(&base, "Travis Scott - Rodeo [MP3 320]", 150.0, 30)]
                } else {
                    vec![]
                };
                axum::Json(json!(list))
            }
        }))
        // Prowlarr's own link: only with its API key
        .route("/1/download", get(|Query(q): Query<HashMap<String, String>>| async move {
            if q.get("apikey").map(String::as_str) != Some("k") {
                return (StatusCode::UNAUTHORIZED, "Unauthorized").into_response();
            }
            (StatusCode::MOVED_PERMANENTLY, [("Location", format!("magnet:?xt=urn:btih:{HASH}&dn=Rodeo"))]).into_response()
        }))
        .route("/api/v2/auth/login", post(move || {
            let f = f2.clone();
            async move {
                if f.lock().unwrap().down {
                    return (StatusCode::INTERNAL_SERVER_ERROR, "restarting").into_response();
                }
                ([("Set-Cookie", "SID=s; path=/")], "Ok.").into_response()
            }
        }))
        .route("/api/v2/torrents/info", get(move |Query(q): Query<HashMap<String, String>>| {
            let f = f3.clone();
            async move {
                let mut g = f.lock().unwrap();
                g.calls.push(("torrents/info".into(), json!(q)));
                if g.down {
                    return (StatusCode::INTERNAL_SERVER_ERROR, "restarting").into_response();
                }
                axum::Json(g.listed.clone().map(|t| json!([t])).unwrap_or(json!([]))).into_response()
            }
        }))
        .route("/api/v2/torrents/{op}", post(move |axum::extract::Path(op): axum::extract::Path<String>, m: Multipart| {
            let f = f4.clone();
            async move {
                let v = fields(m).await;
                let mut g = f.lock().unwrap();
                g.calls.push((format!("torrents/{op}"), v));
                if g.down {
                    return (StatusCode::INTERNAL_SERVER_ERROR, "restarting").into_response();
                }
                "Ok.".into_response()
            }
        }));
    tokio::spawn(async move {
        let _ = axum::serve(l, app).await;
    });
    base
}

struct Env {
    ts: TestServer,
    f: Shared,
    admin: i64,
}

impl Env {
    fn st(&self) -> &AppState {
        &self.ts.st
    }
    fn calls(&self, path: &str) -> Vec<Value> {
        self.f.lock().unwrap().calls.iter().filter(|c| c.0 == path).map(|c| c.1.clone()).collect()
    }
}

/// A server whose only source is the indexers, with qBittorrent; no MusicBrainz answers.
async fn env(name: &str) -> Env {
    let ts = TestServer::new(&format!("torrents-{name}")).with_sources();
    let f: Shared = Default::default();
    let url = fake(f.clone()).await;
    let dl = ts.dir.join("torrents");
    std::fs::create_dir_all(&dl).unwrap();
    ts.st.cfg.set_many(&json!({ "library_path": ts.dir.join("music").to_string_lossy(), "staging_path": ts.dir.join("staging").to_string_lossy(),
        "musicbrainz_url": "http://127.0.0.1:9", "source_order": "torznab", "ytdlp_last_resort": false, "auto_approve": true,
        "torznab_enabled": true, "prowlarr_url": format!("{url}/ \n"), "prowlarr_api_key": "k", "torrent_client": "qbittorrent", "qbit_url": url,
        "qbit_download_path": dl.to_string_lossy(), "torznab_min_seeders": 1 }));
    let admin = user(&ts.st, &format!("admin-{name}"), "admin").i("id");
    Env { ts, f, admin }
}

fn row(st: &AppState, id: i64) -> (String, String, Value) {
    let r = st.db.one("SELECT status, message, detail FROM requests WHERE id=?", p![id]).unwrap().unwrap();
    (r.s("status"), r.s("message"), serde_json::from_str(&r.s("detail")).unwrap_or(json!({})))
}

/// An album downloading through qBittorrent (added `started` seconds ago, no progress since
/// `last_change` seconds ago); a pick, so no other source follows.
fn downloading(e: &Env, started: f64, last_change: f64) -> i64 {
    let t = now();
    let cand = json!({ "title": "Travis Scott - Rodeo (2015) [FLAC]", "indexer": "Tracker", "protocol": "torrent", "url": format!("magnet:?xt=urn:btih:{HASH}") });
    let d = json!({ "sources": [], "log": [], "pick": { "source": "torznab", "payload": cand },
        "torznab": { "candidates": [cand], "idx": 0, "id": format!("qbit:{HASH}"), "started": t - started, "last_change": t - last_change, "last_progress": 10 } });
    e.st().db.execute("INSERT INTO requests (user_id, kind, artist, title, album, status, source, progress, message, detail, created_at, updated_at)
                       VALUES (?,?,?,?,?,?,?,?,?,?,?,?)",
        p![e.admin, "album", "Travis Scott", "Rodeo", "Rodeo", "downloading", "torznab", 10, "qBittorrent: 1.0 MB/s", d.to_string(), t, t]).unwrap()
}

fn torrent(state: &str, progress: f64, content_path: &str) -> Value {
    json!({ "hash": HASH, "tags": "encore-1", "state": state, "progress": progress, "dlspeed": 1_000_000, "content_path": content_path })
}

/// The owner's "torrents and Usenet aren't working": an album request reaches the indexers (the search
/// is the album without its edition, the release named without it is taken, the one that only shares a
/// word ranks below it), qBittorrent gets the magnet from Encore instead of Prowlarr's link, and the
/// indexers' answer is kept for the picker opened for the request. One search: the album's name alone
/// isn't searched when "artist album" found releases.
#[tokio::test]
async fn an_album_reaches_the_indexers_and_qbittorrent_gets_the_release_from_encore() {
    let e = env("album").await;
    let st = e.st().clone();
    let t = now();
    let id = st.db.execute("INSERT INTO requests (user_id, kind, artist, title, album, year, status, detail, created_at, updated_at) VALUES (?,?,?,?,?,?,?,?,?,?)",
        p![e.admin, "album", "Travis Scott", "Rodeo (Expanded Edition)", "Rodeo (Expanded Edition)", "2015", "queued", "{}", t, t]).unwrap();
    worker::step(st.clone(), id).await;
    let (status, msg, d) = row(&st, id);
    assert_eq!((status.as_str(), msg.as_str()), ("downloading", "qBittorrent: Travis Scott - Rodeo (2015) [FLAC] (Tracker)"), "{d}");
    assert_eq!(e.f.lock().unwrap().searches, ["Travis Scott Rodeo"]);
    assert_eq!(d["torznab"]["id"], format!("qbit:{HASH}"));
    let titles: Vec<&str> = d["torznab"]["candidates"].as_array().unwrap().iter().map(|c| c["title"].as_str().unwrap()).collect();
    assert_eq!(titles[0], "Travis Scott - Rodeo (2015) [FLAC]", "{titles:?}");
    let add = e.calls("torrents/add");
    assert_eq!(add.len(), 1);
    assert_eq!(add[0]["urls"], format!("magnet:?xt=urn:btih:{HASH}&dn=Rodeo"), "{}", add[0]);
    assert_eq!((add[0]["tags"].as_str(), add[0]["addToTopOfQueue"].as_str()), (Some(format!("encore-{id}").as_str()), Some("true")));
    // kept for the picker, with the request
    let key = encore_requests::search_cache::torznab_key("Travis Scott Rodeo", &["3000"]);
    let kept = st.requests.search_cache.get(st.data_dir(), &key, now()).expect("kept");
    assert_eq!((kept.request_id, kept.results.len()), (Some(id), 3));
    // followed by its hash
    e.f.lock().unwrap().listed = Some(torrent("downloading", 0.25, ""));
    worker::step(st.clone(), id).await;
    let (status, msg, _) = row(&st, id);
    assert_eq!((status.as_str(), msg.as_str()), ("downloading", "qBittorrent: 1.0 MB/s"));
    assert!(e.calls("torrents/info").iter().any(|q| q["hashes"] == HASH));
    e.ts.remove();
}

/// qBittorrent not answering for a while (a restart, a VPN reconnecting) keeps the download: the request
/// says so and asks again, goes on where it was once qBittorrent is back, and gives up only after as long
/// as a stalled download (diagnosis item 13). It used to move on to the next source at the first failed
/// look, leaving the torrent in qBittorrent.
#[tokio::test]
async fn a_download_client_that_does_not_answer_keeps_the_download() {
    let e = env("down").await;
    let st = e.st().clone();
    let id = downloading(&e, 600.0, 60.0);
    e.f.lock().unwrap().down = true;
    worker::step(st.clone(), id).await;
    let (status, msg, d) = row(&st, id);
    assert_eq!(status, "downloading", "{msg}");
    assert!(msg.starts_with("qBittorrent not answering – asking again (qBittorrent login failed (HTTP 500"), "{msg}");
    assert!(d["torznab"]["err_since"].is_number() && d["torznab"]["id"] == format!("qbit:{HASH}"), "{d}");
    worker::step(st.clone(), id).await;
    assert_eq!(row(&st, id).0, "downloading");
    // back
    {
        let mut g = e.f.lock().unwrap();
        g.down = false;
        g.listed = Some(torrent("downloading", 0.4, ""));
    }
    worker::step(st.clone(), id).await;
    let (status, msg, d) = row(&st, id);
    assert_eq!((status.as_str(), msg.as_str()), ("downloading", "qBittorrent: 1.0 MB/s"));
    assert!(d["torznab"]["err_since"].is_null(), "{d}");
    assert!(d["log"].as_array().unwrap().iter().any(|l| l == "torznab: qBittorrent answers again"), "{d}");
    assert!(e.calls("torrents/delete").is_empty(), "nothing removed");
    // not answering for longer than a stall may last (120 minutes): given up
    e.f.lock().unwrap().down = true;
    worker::step(st.clone(), id).await;
    assert_eq!(row(&st, id).0, "downloading");
    st.db.execute("UPDATE requests SET detail=json_set(detail, '$.torznab.err_since', ?) WHERE id=?", p![now() - 3.0 * 3600.0, id]).unwrap();
    worker::step(st.clone(), id).await;
    let (status, msg, _) = row(&st, id);
    assert_eq!(status, "failed");
    assert!(msg.contains("torznab: qBittorrent didn't answer for 120 minutes"), "{msg}");
    e.ts.remove();
}

/// A torrent qBittorrent doesn't list yet, right after it was added, is waited for; one it still doesn't
/// list two minutes later is gone (diagnosis item 2: the first look, 0-5 s after the add, used to drop it).
#[tokio::test]
async fn a_torrent_qbittorrent_does_not_list_yet_is_waited_for() {
    let e = env("adding").await;
    let st = e.st().clone();
    let id = downloading(&e, 3.0, 3.0);
    worker::step(st.clone(), id).await;
    let (status, msg, _) = row(&st, id);
    assert_eq!((status.as_str(), msg.as_str()), ("downloading", "qBittorrent: adding the download…"));
    let old = downloading(&e, 300.0, 300.0);
    worker::step(st.clone(), old).await;
    let (status, msg, _) = row(&st, old);
    assert_eq!((status.as_str(), msg.as_str()), ("failed", "All sources exhausted (torznab: torrent disappeared from qBittorrent)"));
    e.ts.remove();
}

/// A torrent waiting in qBittorrent's own queue is not stalled: its wait doesn't count toward the stall
/// time (diagnosis item 15: two hours at "0.0 MB/s" per release, then the next one). A queue that never
/// moves is given up after six stall times.
#[tokio::test]
async fn a_queued_torrent_is_not_a_stalled_one() {
    let e = env("queued").await;
    let st = e.st().clone();
    let id = downloading(&e, 4.0 * 3600.0, 3.0 * 3600.0);
    e.f.lock().unwrap().listed = Some(torrent("queuedDL", 0.1, ""));
    worker::step(st.clone(), id).await;
    let (status, msg, d) = row(&st, id);
    assert_eq!((status.as_str(), msg.as_str()), ("downloading", "qBittorrent: queued – its limit of downloads at once is reached"));
    assert!(now() - d["torznab"]["last_change"].as_f64().unwrap() < 60.0, "{d}");
    assert!(d["torznab"]["queued_since"].is_number(), "{d}");
    // once it downloads, the stall time starts from there
    e.f.lock().unwrap().listed = Some(torrent("stalledDL", 0.1, ""));
    worker::step(st.clone(), id).await;
    let (status, _, d) = row(&st, id);
    assert_eq!(status, "downloading");
    assert!(d["torznab"]["queued_since"].is_null(), "{d}");
    // a queue that doesn't move for six stall times (12 hours with the default 120 minutes): the next
    // release is tried (here there is none)
    e.f.lock().unwrap().listed = Some(torrent("queuedDL", 0.1, ""));
    worker::step(st.clone(), id).await;
    assert_eq!(row(&st, id).0, "downloading");
    st.db.execute("UPDATE requests SET detail=json_set(detail, '$.torznab.queued_since', ?) WHERE id=?", p![now() - 13.0 * 3600.0, id]).unwrap();
    worker::step(st.clone(), id).await;
    let (status, msg, _) = row(&st, id);
    assert_eq!((status.as_str(), msg.as_str()), ("failed", "All sources exhausted (torznab: qBittorrent kept it queued for 13 hours)"));
    e.ts.remove();
}

/// A download qBittorrent finished in a folder Encore can't see stays in qBittorrent (it used to be
/// deleted as "the download had no audio files", and the next release went the same way), and the
/// request says where Encore looked (diagnosis item 23).
#[tokio::test]
async fn a_finished_download_encore_cannot_see_stays_in_the_client() {
    let e = env("unseen").await;
    let st = e.st().clone();
    let id = downloading(&e, 600.0, 60.0);
    e.f.lock().unwrap().listed = Some(torrent("stalledUP", 1.0, "/data/torrents/Rodeo"));
    worker::step(st.clone(), id).await;
    let (status, msg, _) = row(&st, id);
    let seen = e.ts.dir.join("torrents/Rodeo");
    assert_eq!(status, "failed");
    assert!(msg.contains(&format!("qBittorrent finished the download, but Encore can't see it at {}", seen.display())), "{msg}");
    assert!(msg.contains("correct the download folder on the qBittorrent card under Settings → Plugins"), "the client's own card has the field: {msg}");
    assert!(e.calls("torrents/delete").is_empty(), "left in qBittorrent");
    e.ts.remove();
}

/// A finished download Encore may not read (the client writes its files as another user) stays in
/// qBittorrent, and the request says so with what to check; one the disk can't read (EIO) waits for the
/// storage. Both used to read as "the download had no audio files", and the download was deleted with its
/// files (diagnosis item 23: errors while reading the folder were dropped).
#[tokio::test]
async fn a_finished_download_encore_cannot_read_stays_in_the_client() {
    use encore_core::fsx::fault;
    let e = env("unreadable").await;
    let st = e.st().clone();
    let folder = e.ts.dir.join("torrents/Rodeo");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("01 Pornography.flac"), b"x").unwrap();
    e.f.lock().unwrap().listed = Some(torrent("stalledUP", 1.0, &folder.to_string_lossy()));
    // another user's files
    let id = downloading(&e, 600.0, 60.0);
    fault::fail(&folder, "read", 13);
    worker::step(st.clone(), id).await;
    fault::clear(&folder);
    let (status, msg, _) = row(&st, id);
    assert_eq!(status, "failed");
    let want = format!("qBittorrent finished the download, but Encore can't read it: read {}: Permission denied (os error 13) – Encore may not read there (check the folder's owner, PUID/PGID)", folder.display());
    assert!(msg.contains(&want), "{msg}");
    assert!(e.calls("torrents/delete").is_empty(), "left in qBittorrent");
    // the disk's error
    let id = downloading(&e, 600.0, 60.0);
    fault::fail(&folder, "read", 5);
    worker::step(st.clone(), id).await;
    fault::clear(&folder);
    let (status, msg, d) = row(&st, id);
    assert_eq!((status.as_str(), d["wait"]["reason"].as_str()), ("waiting", Some("storage_download")), "{msg}");
    assert!(e.calls("torrents/delete").is_empty(), "left in qBittorrent");
    assert!(folder.join("01 Pornography.flac").exists());
    e.ts.remove();
}

/// A finished download whose folder can't be looked at because the storage fails (a mount that went away:
/// ENOTCONN on stat) is no "can't see it": the request waits for the storage with the download kept in
/// qBittorrent, and once the folder answers again that same download is looked at and imported – the
/// indexers are not searched again and nothing is added to qBittorrent a second time.
#[tokio::test]
async fn a_finished_download_behind_a_failing_mount_is_looked_at_again() {
    use encore_core::fsx::fault;
    let e = env("mount").await;
    let st = e.st().clone();
    let folder = e.ts.dir.join("torrents/Rodeo");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("01 Pornography.flac"), b"x").unwrap();
    e.f.lock().unwrap().listed = Some(torrent("stalledUP", 1.0, &folder.to_string_lossy()));
    // an automatic request (no pick): its sources go on after the indexers
    let id = downloading(&e, 600.0, 60.0);
    st.db.execute("UPDATE requests SET detail=json_remove(detail, '$.pick') WHERE id=?", p![id]).unwrap();
    fault::fail(&folder, "stat", 107);
    worker::step(st.clone(), id).await;
    fault::clear(&folder);
    let (status, msg, d) = row(&st, id);
    assert_eq!((status.as_str(), d["wait"]["reason"].as_str()), ("waiting", Some("storage_download")), "{msg}");
    assert_eq!(d["torznab"]["id"], format!("qbit:{HASH}"), "the download is still the request's: {d}");
    assert!(!d["sources"].as_array().unwrap().iter().any(|s| s == "torznab"), "the indexers are not searched again: {d}");
    assert!(e.calls("torrents/delete").is_empty(), "left in qBittorrent");
    // the mount is back: the same download is looked at again
    let infos = e.calls("torrents/info").len();
    st.db.execute("UPDATE requests SET detail=json_set(detail, '$.wait.next_try_at', 0) WHERE id=?", p![id]).unwrap();
    worker::step(st.clone(), id).await;
    let (status, msg, d) = row(&st, id);
    assert_ne!(status, "waiting", "{msg}");
    assert!(e.calls("torrents/info").len() > infos, "qBittorrent was asked about the download");
    assert!(e.f.lock().unwrap().searches.is_empty(), "no new search: {d}");
    assert!(e.calls("torrents/add").is_empty(), "nothing added again: {d}");
    e.ts.remove();
}

/// Keep anyway on a request that is still downloading: its torrent leaves qBittorrent, and its place in the
/// queue stays taken from that stop until the new state is written (a step started in between would find
/// the torrent gone and add the next release, which nothing would then look after); then it is free.
#[tokio::test]
async fn keep_anyway_holds_the_queue_place_while_it_takes_over() {
    let e = env("hold").await;
    let st = e.st().clone();
    let kept = e.ts.dir.join("staging/rejected-x/1");
    std::fs::create_dir_all(&kept).unwrap();
    std::fs::write(kept.join("01 Pornography.flac"), b"x").unwrap();
    e.f.lock().unwrap().listed = Some(torrent("downloading", 0.5, &e.ts.dir.join("torrents/Rodeo").to_string_lossy()));
    let id = downloading(&e, 600.0, 60.0);
    let entry = json!([{ "id": 1, "source": "slskd", "via": "Soulseek", "files": [kept.join("01 Pornography.flac")], "dir": kept, "text": "The audio sounds like X", "at": now() }]);
    st.db.execute("UPDATE requests SET detail=json_set(detail, '$.rejected', json(?)) WHERE id=?", p![entry.to_string(), id]).unwrap();
    // the hold itself: taken until it goes
    let hold = worker::cancel_holding(&st, id).await;
    assert!(st.requests.tasks.taken(id), "the queue starts no step while it is held");
    assert_eq!(e.calls("torrents/delete").len(), 1, "the torrent left qBittorrent");
    drop(hold);
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(!st.requests.tasks.taken(id), "free once the hold goes");
    // Keep anyway on the running request
    st.db.execute("UPDATE requests SET status='downloading' WHERE id=?", p![id]).unwrap();
    worker::rejected::keep_anyway(&st, id, None, "boss").await.unwrap();
    let (status, msg, d) = row(&st, id);
    assert_eq!((status.as_str(), msg.as_str()), ("waiting", "Waiting to import the kept download"), "{d}");
    assert_eq!(d["skip_check"]["why"], "keep");
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(!st.requests.tasks.taken(id), "its place is given back after the new state is written");
    e.ts.remove();
}

/// A release picked by hand: the picker's row carries Prowlarr's link without Prowlarr's API key (sealed),
/// and the download gets the key back – Encore fetches the link from Prowlarr and qBittorrent gets the
/// magnet (diagnosis item 20: the key travelled to everyone who may pick downloads).
#[tokio::test]
async fn a_picked_release_keeps_prowlarrs_key_out_of_the_picker() {
    let e = env("pick").await;
    let st = e.st().clone();
    let b: crate::routes::picker::SearchBody = serde_json::from_value(json!({ "source": "torznab", "kind": "album", "artist": "Travis Scott", "title": "Rodeo" })).unwrap();
    let a = crate::routes::picker::run_search(&st, &b).await.unwrap();
    let picked = a.items.iter().find(|x| x["title"] == "Travis Scott - Rodeo (2015) [FLAC]").expect("the release is offered").clone();
    let link = picked["payload"]["url"].as_str().unwrap();
    assert!(link.contains("/1/download?apikey=encore-seal-") && !link.contains("apikey=k&"), "{link}");
    let u = st.db.one("SELECT * FROM users WHERE id=?", p![e.admin]).unwrap().unwrap();
    let req = encore_requests::create::NewRequest { kind: "album".into(), artist: Some("Travis Scott".into()), title: Some("Rodeo".into()), ..Default::default() };
    let (id, _) = crate::routes::picker::create_pick(&st, &u, &req, "torznab", picked["payload"].clone(), "Travis Scott - Rodeo (2015) [FLAC]").unwrap();
    worker::step(st.clone(), id).await;
    let (status, msg, d) = row(&st, id);
    assert_eq!((status.as_str(), msg.as_str()), ("downloading", "qBittorrent: Travis Scott - Rodeo (2015) [FLAC] (Tracker)"), "{d}");
    let add = e.calls("torrents/add");
    assert_eq!(add.len(), 1);
    assert_eq!(add[0]["urls"], format!("magnet:?xt=urn:btih:{HASH}&dn=Rodeo"), "{}", add[0]);
    e.ts.remove();
}
