//! Download now (encore_requests::worker::now) on a whole server: the streamrip line with a stand-in `rip`
//! that holds its run (one at a time, a pressed request next), the waits tried now (the staging folder through
//! the fault seam, a downloads folder through the stand-in probe, Soulseek on the stand-in slskd of
//! soulseek.rs with its supervisor), a Soulseek download queued at its user switched to one with a free upload
//! slot (a stand-in slskd), a torrent with no progress given up for the next source (a stand-in qBittorrent),
//! what every state offers and answers, and who may press it (Download all now included). None of them needs
//! ffmpeg.
use crate::state::AppState;
use crate::test_support::{call, user, TestServer};
use axum::extract::{Multipart, Path as UrlPath, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use encore_core::fsx::fault;
use encore_core::fsx::{Step, StepError};
use encore_db::{now, p, RowExt};
use encore_requests::slskd_managed::Target;
use encore_requests::slskd_session::{self as session, Reason, State as S};
use encore_requests::worker::{self, downloads, now as dn};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

async fn login(app: &Router, name: &str) -> String {
    let (code, body, cookie) = call(app, "POST", "/api/auth/login", "", Some(json!({ "username": name, "password": "a-long-password-1" }))).await;
    assert_eq!(code, StatusCode::OK, "{body}");
    cookie.split(';').next().unwrap_or("").to_string()
}

/// A request row of `owner`.
#[allow(clippy::too_many_arguments)]
fn insert(st: &AppState, owner: i64, kind: &str, title: &str, status: &str, source: &str, message: &str, detail: Value) -> i64 {
    let t = now();
    st.db.execute("INSERT INTO requests (user_id, kind, artist, title, album, status, source, message, detail, created_at, updated_at) VALUES (?,?,?,?,?,?,?,?,?,?,?)",
        p![owner, kind, "Duumu", title, title, status, if source.is_empty() { Value::Null } else { json!(source) }, message, detail.to_string(), t, t]).unwrap()
}

fn row(st: &AppState, id: i64) -> (String, String, Value) {
    let r = st.db.one("SELECT status, message, detail FROM requests WHERE id=?", p![id]).unwrap().unwrap();
    (r.s("status"), r.s("message"), serde_json::from_str(&r.s("detail")).unwrap_or(json!({})))
}

fn log_text(d: &Value) -> String {
    d["log"].as_array().map(|a| a.iter().filter_map(|l| l.as_str()).collect::<Vec<_>>().join("\n")).unwrap_or_default()
}

/// GET /api/requests as the user of `cookie`: the row of request `id`.
async fn listed(app: &Router, cookie: &str, id: i64) -> Value {
    let (code, v, _) = call(app, "GET", "/api/requests?limit=300", cookie, None).await;
    assert_eq!(code, StatusCode::OK, "{v}");
    v.as_array().unwrap().iter().find(|r| r["id"] == id).cloned().unwrap_or_else(|| panic!("request {id} not listed: {v}"))
}

async fn press(app: &Router, cookie: &str, id: i64) -> (StatusCode, Value) {
    let (code, v, _) = call(app, "POST", &format!("/api/requests/{id}/download-now"), cookie, None).await;
    (code, v)
}

/// Wait (the runtime's time) until `cond` holds; fails after `max`.
async fn until(what: &str, max: Duration, cond: impl Fn() -> bool) {
    let t = tokio::time::Instant::now();
    while !cond() {
        assert!(t.elapsed() < max, "waited {max:?} for: {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

// ---- the streamrip line --------------------------------------------------------------------------------

/// Four picked Deezer albums with streamrip one at a time on Deezer: one runs, the others wait in line in the order
/// they came ("Next/2nd/3rd in line for Deezer", status queued); the owner presses the last one, which then
/// rips next. The stand-in never sees two runs at once, and a run is never interrupted.
#[tokio::test]
async fn the_streamrip_line_honours_download_now_and_its_limit() {
    let ts = TestServer::new("dn-rip").with_sources();
    let st = ts.st.clone();
    let fake = ts.dir.join("fake");
    let rip = super::streamrip::fake_rip(&fake);
    std::fs::write(fake.join("mode"), "hold").unwrap();
    let (staging, lib) = (ts.dir.join("staging"), ts.dir.join("lib"));
    std::fs::create_dir_all(&lib).unwrap();
    st.cfg.set_many(&json!({ "streamrip_enabled": true, "streamrip_bin": rip.to_string_lossy(), "streamrip_deezer_arl": "arl-cookie",
        "staging_path": staging.to_string_lossy(), "library_path": lib.to_string_lossy(), "musicbrainz_url": "http://127.0.0.1:9",
        "autotag_on_import": false, "quality_check_on_import": false, "lyrics_on_import": false, "auto_approve": true,
        "source_order": "deezer", "ytdlp_enabled": false, "ytdlp_last_resort": false, "slskd_enabled": false }));
    let owner = user(&st, "rio", "user").i("id");
    let app = ts.app();
    let cookie = login(&app, "rio").await;
    let ids: Vec<i64> = (1..=4).map(|n| insert(&st, owner, "album", &format!("Album {n}"), "queued", "", "",
        json!({ "pick": { "source": "deezer", "payload": { "url": format!("https://www.deezer.com/album/{n}") }, "title": format!("Album {n}") } }))).collect();
    let runs = || std::fs::read_to_string(fake.join("runs.log")).unwrap_or_default().lines().map(String::from).collect::<Vec<_>>();
    let line = st.requests.lines.streamrip("deezer");
    let mut tasks = vec![];
    for (i, id) in ids.iter().enumerate() {
        tasks.push(tokio::spawn(worker::step(st.clone(), *id)));
        if i == 0 {
            until("the first rip runs", Duration::from_secs(30), || runs().len() == 1).await;
        } else {
            until("in line", Duration::from_secs(30), || line.waiting() == i).await;
        }
    }
    for (id, want) in ids[1..].iter().zip(["Next in line for Deezer", "2nd in line for Deezer", "3rd in line for Deezer"]) {
        let r = listed(&app, &cookie, *id).await;
        assert_eq!((r["status"].as_str(), r["message"].as_str()), (Some("queued"), Some(want)), "{r}");
        assert_eq!((r["can_download_now"].as_bool(), r["download_now"]["act"].as_str(), r["download_now"]["title"].as_str()),
            (Some(true), Some("line"), Some("Go first in line for Deezer")), "{r}");
        assert_eq!((r["line"]["source"].as_str(), r["line"]["limit_text"].as_str(), r["line"]["waiting"].as_u64(), r["line"]["running"].as_u64()),
            (Some("streamrip"), Some("1 download at a time"), Some(3), Some(1)), "{r}");
    }
    // the one that runs has no button: a press never touches a running slot
    let r = listed(&app, &cookie, ids[0]).await;
    assert_eq!(r["can_download_now"], false, "{r}");
    let (code, v) = press(&app, &cookie, ids[0]).await;
    assert_eq!((code, v["detail"].as_str()), (StatusCode::CONFLICT, Some("It is downloading now")), "{v}");
    // the owner presses the last one: next in line
    let (code, v) = press(&app, &cookie, ids[3]).await;
    assert_eq!(code, StatusCode::OK, "{v}");
    assert_eq!((v["ok"].as_bool(), v["action"].as_str(), v["text"].as_str(), v["status"].as_str()),
        (Some(true), Some("line"), Some("Next in line for Deezer (1 download at a time)"), Some("queued")), "{v}");
    assert_eq!((v["line"]["position"].as_u64(), v["line"]["first"].as_bool()), (Some(1), Some(true)), "{v}");
    let r = listed(&app, &cookie, ids[1]).await;
    assert_eq!(r["message"], "2nd in line for Deezer", "the others moved down one: {r}");
    assert_eq!(listed(&app, &cookie, ids[3]).await["pressed"]["by"], "rio");
    // a second press behind it: behind 1 request asked for earlier
    let (_, v) = press(&app, &cookie, ids[2]).await;
    assert_eq!(v["text"], "2nd in line for Deezer (1 download at a time), behind 1 request asked for earlier", "{v}");
    // the running rip ends: the pressed one rips next, and says so in its log
    std::fs::write(fake.join("release"), "").unwrap();
    until("the next rip runs", Duration::from_secs(30), || runs().len() == 2).await;
    assert!(runs()[1].ends_with("album/4"), "{:?}", runs());
    until("its log line", Duration::from_secs(10), || log_text(&row(&st, ids[3]).2).contains("started first in line (Download now by rio)")).await;
    // then the one pressed second, then the last
    std::fs::write(fake.join("release"), "").unwrap();
    until("the third rip runs", Duration::from_secs(30), || runs().len() == 3).await;
    assert!(runs()[2].ends_with("album/3"), "{:?}", runs());
    std::fs::write(fake.join("release-all"), "").unwrap();
    for t in tasks {
        tokio::time::timeout(Duration::from_secs(60), t).await.expect("every step ends").unwrap();
    }
    assert_eq!(runs().len(), 4);
    assert!(runs()[3].ends_with("album/2"), "{:?}", runs());
    assert!(!fake.join("overlap.log").exists(), "two rips at once: {}", std::fs::read_to_string(fake.join("overlap.log")).unwrap_or_default());
    assert_eq!((line.running(), line.waiting()), (0, 0));
    ts.remove();
}

/// A request set to cancelled while it waits in a line (Remove artist writes its rows before it stops their
/// tasks) stays cancelled when its turn comes: nothing rips for it, and the slot is given back.
#[tokio::test]
async fn a_waiter_cancelled_meanwhile_stays_cancelled() {
    let ts = TestServer::new("dn-rip-cancel").with_sources();
    let st = ts.st.clone();
    let fake = ts.dir.join("fake");
    let rip = super::streamrip::fake_rip(&fake);
    std::fs::write(fake.join("mode"), "hold").unwrap();
    let (staging, lib) = (ts.dir.join("staging"), ts.dir.join("lib"));
    std::fs::create_dir_all(&lib).unwrap();
    st.cfg.set_many(&json!({ "streamrip_enabled": true, "streamrip_bin": rip.to_string_lossy(), "streamrip_deezer_arl": "arl-cookie",
        "staging_path": staging.to_string_lossy(), "library_path": lib.to_string_lossy(), "musicbrainz_url": "http://127.0.0.1:9",
        "autotag_on_import": false, "quality_check_on_import": false, "lyrics_on_import": false, "auto_approve": true,
        "source_order": "deezer", "ytdlp_enabled": false, "ytdlp_last_resort": false, "slskd_enabled": false }));
    let owner = user(&st, "rio-c", "user").i("id");
    let ids: Vec<i64> = (1..=2).map(|n| insert(&st, owner, "album", &format!("Album {n}"), "queued", "", "",
        json!({ "pick": { "source": "deezer", "payload": { "url": format!("https://www.deezer.com/album/{n}") }, "title": format!("Album {n}") } }))).collect();
    let runs = || std::fs::read_to_string(fake.join("runs.log")).unwrap_or_default().lines().count();
    let line = st.requests.lines.streamrip("deezer");
    let first = tokio::spawn(worker::step(st.clone(), ids[0]));
    until("the first rip runs", Duration::from_secs(30), || runs() == 1).await;
    let second = tokio::spawn(worker::step(st.clone(), ids[1]));
    until("in line", Duration::from_secs(30), || line.waiting() == 1).await;
    assert_eq!(row(&st, ids[1]).0, "queued");
    // removed with its artist: the row is written first, its task still waits
    st.db.execute("UPDATE requests SET status='cancelled', message='Artist removed' WHERE id=?", p![ids[1]]).unwrap();
    std::fs::write(fake.join("release-all"), "").unwrap();
    tokio::time::timeout(Duration::from_secs(60), first).await.expect("the first step ends").unwrap();
    tokio::time::timeout(Duration::from_secs(60), second).await.expect("the second step ends").unwrap();
    let (status, message, _) = row(&st, ids[1]);
    assert_eq!((status.as_str(), message.as_str()), ("cancelled", "Artist removed"));
    assert_eq!(runs(), 1, "nothing ripped for the removed one");
    assert_eq!((line.running(), line.waiting()), (0, 0), "the slot is given back");
    ts.remove();
}

// ---- waits with a timer --------------------------------------------------------------------------------

/// A download waiting for its staging folder: pressed while the folder still fails, it answers with the next
/// try (members in plain words, admins with the folder); once the folder can be written, the press starts it.
/// A request waiting for a downloads folder: the same with the stand-in probe.
#[tokio::test]
async fn waits_try_now() {
    let ts = TestServer::new("dn-waits").with_sources();
    let st = ts.st.clone();
    let (staging, lib) = (ts.dir.join("staging"), ts.dir.join("lib"));
    std::fs::create_dir_all(&staging).unwrap();
    std::fs::create_dir_all(&lib).unwrap();
    st.cfg.set_many(&json!({ "staging_path": staging.to_string_lossy(), "library_path": lib.to_string_lossy(), "musicbrainz_url": "http://127.0.0.1:9",
        "source_order": "ytdlp", "ytdlp_enabled": false, "ytdlp_last_resort": false, "slskd_enabled": false, "streamrip_enabled": false, "auto_approve": true }));
    user(&st, "boss-w", "admin");
    let mia = user(&st, "mia-w", "user").i("id");
    let app = ts.app();
    let (admin, member) = (login(&app, "boss-w").await, login(&app, "mia-w").await);
    // the staging folder: EIO
    let dir = staging.join("request-x");
    let t = now();
    let err = json!({ "step": "stage", "op": "mkdir", "path": dir.to_string_lossy(), "to": null, "errno": 5, "error": "Input/output error (os error 5)", "storage": true,
                      "text": format!("mkdir {}: Input/output error (os error 5)", dir.display()) });
    let id = insert(&st, mia, "album", "Album", "waiting", "ytdlp", &format!("Waiting: storage error – mkdir {}: Input/output error (os error 5). Next try at 14:31", dir.display()),
        json!({ "sources": [], "log": [], "attempts": 2, "wait": { "reason": "storage_download", "since": t, "next_try_at": t + 600.0, "error": err, "dir": dir.to_string_lossy() } }));
    let r = listed(&app, &member, id).await;
    assert_eq!((r["can_download_now"].as_bool(), r["download_now"]["act"].as_str(), r["download_now"]["title"].as_str()),
        (Some(true), Some("wake"), Some("Check the storage now and start the download if it can be written")), "{r}");
    fault::fail(&staging, "*", 5);
    let (code, v) = press(&app, &member, id).await;
    assert_eq!(code, StatusCode::OK, "{v}");
    assert_eq!((v["ok"].as_bool(), v["action"].as_str(), v["status"].as_str()), (Some(false), Some("later"), Some("waiting")), "{v}");
    assert!(v["at"].as_f64().unwrap() > now() + 500.0, "{v}");
    let text = v["text"].as_str().unwrap();
    assert!(text.starts_with("Waiting: a storage problem on the server – the download starts again when it is fixed. Next try at "), "{text}");
    assert!(!text.contains(&staging.to_string_lossy().to_string()), "no server folder for members: {text}");
    let (_, v) = press(&app, &admin, id).await;
    let text = v["text"].as_str().unwrap();
    assert!(text.starts_with("Waiting: storage error – ") && text.contains(&staging.to_string_lossy().to_string()), "{text}");
    assert_eq!(row(&st, id).2["attempts"], 2, "no attempt counted");
    fault::clear(&staging);
    let (code, v) = press(&app, &member, id).await;
    assert_eq!((code, v["ok"].as_bool(), v["action"].as_str(), v["text"].as_str()),
        (StatusCode::OK, Some(true), Some("now"), Some("The storage can be written again – downloading now")), "{v}");
    assert_ne!(row(&st, id).0, "waiting", "it went on");
    let activity: Vec<String> = st.db.query("SELECT message FROM activity WHERE message LIKE ? ORDER BY id", p![format!("%asked to download request #{id}%")]).unwrap()
        .iter().map(|r| r.s("message")).collect();
    assert_eq!(activity.len(), 3, "one line per press: {activity:?}");
    assert!(activity[0].starts_with(&format!("mia-w asked to download request #{id} (Duumu – Album) now: the storage still can't be written")), "{activity:?}");

    // a downloads folder the probe found read-only
    let failing = Arc::new(Mutex::new(true));
    let (f2, s2) = (failing.clone(), staging.clone());
    downloads::fake_probe(&st, Some(Arc::new(move |d: &Path| if *f2.lock().unwrap() && d == s2 {
        Err(StepError::new(Step::Download, "write", &d.join(".encore-probe-1"), None, std::io::Error::from_raw_os_error(30)))
    } else {
        Ok(())
    })));
    downloads::probe_all(&st);
    let id = insert(&st, mia, "album", "Disk", "waiting", "ytdlp", &format!("Waiting for the downloads folder – {} is read-only", staging.display()),
        json!({ "sources": [], "log": [], "wait": { "reason": "waiting_disk", "since": now(), "next_try_at": null, "source": "ytdlp", "dir": staging.to_string_lossy() } }));
    let r = listed(&app, &member, id).await;
    assert_eq!(r["download_now"]["title"], "Check the downloads folder now and start the download if it can be written", "{r}");
    let (_, v) = press(&app, &member, id).await;
    assert_eq!((v["ok"].as_bool(), v["action"].as_str(), v["at"].clone(), v["text"].as_str()),
        (Some(false), Some("later"), Value::Null, Some("The downloads folder still can't be written – a storage problem on the server; the download starts when it is fixed")), "{v}");
    let (_, v) = press(&app, &admin, id).await;
    assert_eq!(v["text"], format!("The downloads folder still can't be written – {} is read-only", staging.display()), "{v}");
    // the folder can be written again moments later: the press probes again (the last pass found it failing,
    // however young it is) and starts it
    *failing.lock().unwrap() = false;
    let (_, v) = press(&app, &member, id).await;
    assert_eq!((v["ok"].as_bool(), v["text"].as_str()), (Some(true), Some("The downloads folder can be written again – downloading now")), "{v}");
    until("it went on", Duration::from_secs(10), || row(&st, id).0 != "waiting").await;
    ts.remove();
}

/// Soulseek on the stand-in slskd of soulseek.rs: a lost connection (Encore's own schedule holds it) → the
/// press makes the planned attempt due now and exactly one connection reaches the server; a second press
/// within 2 minutes answers when; a login the server doesn't answer → the press answers the session's next
/// attempt and no connection reaches the server for it.
#[tokio::test(start_paused = true)]
async fn soulseek_tries_now_only_on_encores_own_schedule() {
    let (ts, f) = super::soulseek::setup("dn-soulseek");
    let st = ts.st.clone();
    f.lock().unwrap().login = super::soulseek::Login::Works;
    let sup = tokio::spawn(session::supervisor(st.clone()));
    super::soulseek::until("logged in", Duration::from_secs(60), || super::soulseek::state(&st) == S::LoggedIn).await;
    let t = now();
    let id = insert(&st, 1, "track", "Song", "waiting", "slskd", "Waiting for Soulseek – connecting shortly",
        json!({ "sources": ["slskd"], "attempts": 1, "soulseek_wait": { "since": t }, "wait": { "reason": "soulseek", "since": t, "next_try_at": null } }));
    let title = |st: &AppState| dn::row_fields(st, &worker::get(st, id).unwrap())["download_now"]["title"].as_str().unwrap_or("").to_string();
    assert_eq!(title(&st), "Search Soulseek now");
    // the session drops (a lost connection), long after the last attempt
    tokio::time::sleep(Duration::from_secs(60)).await;
    super::soulseek::server_closes(&f, "Remote connection closed");
    super::soulseek::until("waiting", Duration::from_secs(10), || super::soulseek::state(&st) == S::Waiting).await;
    assert_eq!(st.requests.slskd.session().snapshot().reason, Some(Reason::Network));
    assert_eq!(title(&st), "Connect to Soulseek now and search");
    let reached = f.lock().unwrap().reached.len();
    let v = dn::download_now(&st, id, "boss", true).await.unwrap();
    assert_eq!((v["ok"].as_bool(), v["action"].as_str(), v["text"].as_str()), (Some(true), Some("now"), Some("Connecting to Soulseek now – it searches once logged in")), "{v}");
    super::soulseek::until("logged in again", Duration::from_secs(10), || super::soulseek::state(&st) == S::LoggedIn).await;
    assert_eq!(f.lock().unwrap().reached.len(), reached + 1, "exactly one connection reached the server");
    // dropped again at once: a second press within 2 minutes answers when, and nothing more reaches the server
    tokio::time::sleep(Duration::from_secs(5)).await;
    super::soulseek::server_closes(&f, "Remote connection closed");
    super::soulseek::until("waiting", Duration::from_secs(10), || super::soulseek::state(&st) == S::Waiting).await;
    let v = dn::download_now(&st, id, "boss", true).await.unwrap();
    assert_eq!((v["ok"].as_bool(), v["action"].as_str()), (Some(false), Some("later")), "{v}");
    assert!(v["text"].as_str().unwrap().starts_with("Soulseek was tried moments ago – next attempt at "), "{v}");
    assert!(v["at"].as_f64().is_some(), "{v}");
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(f.lock().unwrap().reached.len(), reached + 1);
    // the server stops answering logins: the press answers the session's own next attempt and changes nothing
    f.lock().unwrap().login = super::soulseek::Login::TimesOut;
    super::soulseek::until("the login timed out", Duration::from_secs(300), || {
        let c = st.requests.slskd.session().snapshot();
        c.state == S::Waiting && c.reason == Some(Reason::Timeout)
    }).await;
    let planned = st.requests.slskd.session().snapshot().next_at.unwrap();
    let reached = f.lock().unwrap().reached.len();
    assert!(title(&st).starts_with("Soulseek asked Encore to wait – next attempt at "), "{}", title(&st));
    let v = dn::download_now(&st, id, "boss", true).await.unwrap();
    assert_eq!((v["ok"].as_bool(), v["action"].as_str()), (Some(false), Some("later")), "{v}");
    assert!(v["text"].as_str().unwrap().starts_with("The Soulseek server isn't answering logins – Encore tries again at ")
        && v["text"].as_str().unwrap().ends_with(" so it isn't shut out"), "{v}");
    assert!((v["at"].as_f64().unwrap() - session::unix_at(planned)).abs() < 2.0, "{v}");
    assert_eq!(st.requests.slskd.session().snapshot().next_at, Some(planned), "nothing planned changes");
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(f.lock().unwrap().reached.len(), reached, "no connection reached the server for the press");
    sup.abort();
    ts.remove();
}

// ---- a Soulseek download queued at its user ------------------------------------------------------------

/// The stand-in slskd: alice's transfers queued remotely, users' answers about their slots, what was asked.
#[derive(Default)]
struct Sl {
    /// alice's place in her upload queue (placeInQueue), None: slskd doesn't say
    place: Option<u64>,
    /// `users/{u}/info` answers: whether the user has a free slot
    free: HashMap<String, bool>,
    /// users whose client never answers
    slow: Vec<String>,
    /// users slskd can't take a download from (offline)
    offline: Vec<String>,
    /// downloads taken from users, by user: the file names
    taken: HashMap<String, Vec<String>>,
    /// what was asked, in order: "POST bob", "DELETE alice/alice-0", "POSITION alice"
    calls: Vec<String>,
    /// alice's upload starts when this is asked ("INFO bob": bob is asked about his slots, "POST bob": bob's
    /// files are handed to slskd)
    alice_starts_on: Option<String>,
    /// alice's upload runs: her files are "InProgress", bytes arrive
    alice_started: bool,
}

type Sh = Arc<Mutex<Sl>>;

fn files(dir: &str) -> Vec<String> {
    (1..=4).map(|i| format!("Music\\{dir}\\{i:02} Light {i}.flac")).collect()
}

fn fake_slskd(sh: Sh) -> Router {
    Router::new()
        .route("/api/v0/server", get(|| async { Json(json!({ "state": "Connected, LoggedIn", "isLoggedIn": true })) }))
        .route("/api/v0/transfers/downloads/{user}", get(|State(sh): State<Sh>, UrlPath(user): UrlPath<String>| async move {
            let s = sh.lock().unwrap();
            let list: Vec<Value> = if user == "alice" && s.alice_started {
                files("Duumu - Illuminate EP").iter().enumerate().map(|(i, n)| json!({ "id": format!("alice-{i}"), "filename": n, "state": "InProgress",
                    "size": 20_000_000, "bytesTransferred": 1_000_000 })).collect()
            } else if user == "alice" {
                files("Duumu - Illuminate EP").iter().enumerate().map(|(i, n)| json!({ "id": format!("alice-{i}"), "filename": n, "state": "Queued, Remotely",
                    "size": 20_000_000, "bytesTransferred": 0, "placeInQueue": s.place.map(|p| p + i as u64) })).collect()
            } else {
                s.taken.get(&user).cloned().unwrap_or_default().iter().enumerate().map(|(i, n)| json!({ "id": format!("{user}-{i}"), "filename": n,
                    "state": "Queued, Remotely", "size": 20_000_000, "bytesTransferred": 0 })).collect()
            };
            Json(json!({ "username": user, "directories": [{ "directory": "Music\\Duumu - Illuminate EP", "files": list }] }))
        }).post(|State(sh): State<Sh>, UrlPath(user): UrlPath<String>, Json(body): Json<Value>| async move {
            let mut s = sh.lock().unwrap();
            if s.offline.contains(&user) {
                return (StatusCode::INTERNAL_SERVER_ERROR, format!("User {user} appears to be offline"));
            }
            s.calls.push(format!("POST {user}"));
            if s.alice_starts_on.as_deref() == Some(format!("POST {user}").as_str()) {
                s.alice_started = true;
            }
            let names = body.as_array().unwrap().iter().map(|f| f["filename"].as_str().unwrap().to_string()).collect();
            s.taken.insert(user, names);
            (StatusCode::CREATED, "{}".to_string())
        }))
        .route("/api/v0/transfers/downloads/{user}/{id}", axum::routing::delete(|State(sh): State<Sh>, UrlPath((user, id)): UrlPath<(String, String)>| async move {
            sh.lock().unwrap().calls.push(format!("DELETE {user}/{id}"));
            StatusCode::NO_CONTENT
        }))
        .route("/api/v0/transfers/downloads/{user}/{id}/position", get(|State(sh): State<Sh>, UrlPath((user, _id)): UrlPath<(String, String)>| async move {
            sh.lock().unwrap().calls.push(format!("POSITION {user}"));
            Json(json!(7))
        }))
        .route("/api/v0/users/{user}/info", get(|State(sh): State<Sh>, UrlPath(user): UrlPath<String>| async move {
            let (slow, free) = {
                let mut s = sh.lock().unwrap();
                if s.alice_starts_on.as_deref() == Some(format!("INFO {user}").as_str()) {
                    s.alice_started = true;
                }
                (s.slow.contains(&user), s.free.get(&user).copied().unwrap_or(false))
            };
            if slow {
                tokio::time::sleep(Duration::from_secs(600)).await;
            }
            Json(json!({ "hasFreeUploadSlot": free, "queueLength": 0, "uploadSlots": 1 }))
        }))
        .with_state(sh)
}

/// One Soulseek answer: `user`'s folder `dir`, four FLACs (or MP3s at `br`).
fn answer(user: &str, free: bool, dir: &str, ext: &str, br: Option<i64>) -> Value {
    let f: Vec<Value> = files(dir).iter().map(|n| json!({ "filename": n.replace(".flac", &format!(".{ext}")), "size": 20_000_000, "bitRate": br })).collect();
    json!({ "username": user, "hasFreeUploadSlot": free, "uploadSpeed": 1_000_000, "queueLength": 3, "files": f })
}

struct Slsk {
    ts: TestServer,
    sh: Sh,
}

fn slsk_env(name: &str) -> Slsk {
    let ts = TestServer::new(&format!("dn-{name}")).with_sources()
        .with(json!({ "slskd_enabled": true, "soulseek_username": "me", "soulseek_password": "a-password", "slskd_preferred_format": "flac",
                      "slskd_min_bitrate": 192, "slskd_max_candidates": 4, "auto_approve": true, "musicbrainz_url": "http://127.0.0.1:9" }));
    let tools = ts.st.data_dir().join("tools").join("slskd");
    std::fs::create_dir_all(tools.join("0.26.1")).unwrap();
    std::fs::write(tools.join("0.26.1").join("slskd"), "#!/bin/sh\n").unwrap();
    std::fs::write(tools.join("current"), "0.26.1\n").unwrap();
    user(&ts.st, "carol", "admin");
    let sh: Sh = Default::default();
    sh.lock().unwrap().place = Some(12);
    ts.st.requests.slskd.set_state_for_tests("running");
    ts.st.requests.slskd.set_target(Target::App(fake_slskd(sh.clone())));
    Slsk { ts, sh }
}

/// An album (`title`) downloading from alice, every file queued in her upload queue (12th, as the last look
/// found); the request's Soulseek answers kept as its own search keeps them.
async fn queued_at_alice(e: &Slsk, title: &str, answers: &[Value]) -> i64 {
    let st = &e.ts.st;
    let t = now();
    let alice = json!({ "username": "alice", "directory": "Music/Duumu - Illuminate EP", "format": "flac", "bitrate": 0, "score": 60,
        "files": files("Duumu - Illuminate EP").iter().map(|n| json!({ "filename": n, "size": 20_000_000 })).collect::<Vec<_>>() });
    let tracks: Vec<Value> = (1..=4).map(|i| json!({ "title": format!("Light {i}") })).collect();
    let id = insert(st, 1, "album", title, "downloading", "slskd", "Queued by alice – 0/4 files",
        json!({ "sources": [], "log": [], "release": { "id": null, "tracks": tracks },
                "slskd": { "candidates": [alice], "idx": 0, "started": t - 600.0, "last_change": t - 600.0, "last_bytes": 0,
                           "remote": { "place": 12, "since": t - 300.0, "asked_at": null } } }));
    if !answers.is_empty() {
        assert!(encore_requests::search_cache::keep_slskd(st, &format!("Duumu {title}"), Some(id), answers).await);
    }
    id
}

/// alice's transfers sit in her upload queue: the look reads her place and offers the switch; pressing hands
/// bob's files (a free slot, FLAC, the album) to slskd before alice's are cancelled. Without anyone better
/// nothing changes; a user who doesn't answer is skipped; a download slskd can't take keeps alice's place.
#[tokio::test(start_paused = true)]
async fn a_soulseek_queue_switches_to_a_user_with_a_free_slot() {
    let e = slsk_env("switch");
    let st = e.ts.st.clone();
    let good = "Duumu - Illuminate EP";
    let answers = [answer("bob", true, good, "flac", None), answer("dave", false, good, "flac", None), answer("carol", true, good, "mp3", Some(128)),
                   answer("erin", true, "Duumu - Other Things", "flac", None), answer("alice", true, good, "flac", None)];
    {
        let mut s = e.sh.lock().unwrap();
        for u in ["bob", "dave", "carol", "erin", "hana"] {
            s.free.insert(u.into(), u != "dave");
        }
    }
    let id = queued_at_alice(&e, "Illuminate EP", &answers).await;
    // the poll reads her place
    worker::step(st.clone(), id).await;
    let (status, message, d) = row(&st, id);
    assert_eq!((status.as_str(), message.as_str()), ("downloading", "Queued by alice – 12th in their queue, 0/4 files"), "{d}");
    assert_eq!(d["slskd"]["remote"]["place"], 12, "{d}");
    let f = dn::row_fields(&st, &worker::get(&st, id).unwrap());
    assert_eq!(f["remote_queue"]["text"], "12th in alice's upload queue", "{f:?}");
    assert_eq!((f["download_now"]["act"].as_str(), f["download_now"]["title"].as_str()),
        (Some("switch"), Some("Switch to a user with a free upload slot – it is 12th in alice's queue")), "{f:?}");
    // pressed: bob, enqueued before alice's transfers go
    let v = dn::download_now(&st, id, "carol", true).await.unwrap();
    assert_eq!((v["ok"].as_bool(), v["action"].as_str(), v["text"].as_str(), v["status"].as_str()),
        (Some(true), Some("switched"), Some("Switched to bob, who has a free upload slot (FLAC, 4 files)"), Some("downloading")), "{v}");
    let calls = e.sh.lock().unwrap().calls.clone();
    assert_eq!(calls[0], "POST bob", "{calls:?}");
    assert_eq!(calls.iter().filter(|c| c.starts_with("DELETE alice/")).count(), 4, "{calls:?}");
    let (_, message, d) = row(&st, id);
    assert_eq!(message, "Downloading 4 file(s) from bob (FLAC)");
    assert_eq!((d["slskd"]["idx"].as_i64(), d["slskd"]["candidates"][1]["username"].as_str()), (Some(1), Some("bob")), "{d}");
    assert!(d["slskd"]["remote"].is_null() && d["slskd"]["last_bytes"] == 0, "{d}");
    assert_eq!(d["slskd_tried"], json!(["alice|Music/Duumu - Illuminate EP"]));
    assert!(log_text(&d).contains("slskd: switched from alice (12th in their queue) to bob (free upload slot) – Download now by carol"), "{d}");

    // only answers without a free slot, the wrong album, too low a bit rate, or blocked: nothing changes
    e.sh.lock().unwrap().calls.clear();
    let blocked = answer("gina", true, good, "flac", None);
    st.db.execute("INSERT INTO source_block (kind, key, label, created_at) VALUES ('slskd', ?, 'x', ?)", p![format!("gina|Music/{good}"), now()]).unwrap();
    let id = queued_at_alice(&e, "Illuminate EP", &[answers[1].clone(), answers[2].clone(), answers[3].clone(), blocked]).await;
    let v = dn::download_now(&st, id, "carol", true).await.unwrap();
    assert_eq!((v["ok"].as_bool(), v["action"].as_str(), v["text"].as_str()),
        (Some(false), Some("kept"), Some("No one with a free upload slot has it – it stays 12th in alice's queue")), "{v}");
    assert!(e.sh.lock().unwrap().calls.iter().all(|c| !c.starts_with("POST") && !c.starts_with("DELETE")), "{:?}", e.sh.lock().unwrap().calls);
    // nothing kept for it (nor for its words)
    let id = queued_at_alice(&e, "Nothing Kept", &[]).await;
    let v = dn::download_now(&st, id, "carol", true).await.unwrap();
    assert_eq!(v["text"], "No other Soulseek results are kept for it – it stays 12th in alice's queue", "{v}");
    // bob doesn't answer: the next free user takes it
    e.sh.lock().unwrap().slow.push("bob".into());
    let id = queued_at_alice(&e, "Illuminate EP", &[answer("bob", true, good, "flac", None), answer("hana", true, good, "flac", None)]).await;
    let v = dn::download_now(&st, id, "carol", true).await.unwrap();
    assert_eq!(v["text"], "Switched to hana, who has a free upload slot (FLAC, 4 files)", "{v}");
    // slskd can't take bob's files (bob went offline): alice keeps her place
    {
        let mut s = e.sh.lock().unwrap();
        s.slow.clear();
        s.offline.push("bob".into());
        s.calls.clear();
    }
    let id = queued_at_alice(&e, "Illuminate EP", &[answer("bob", true, good, "flac", None)]).await;
    let v = dn::download_now(&st, id, "carol", true).await.unwrap();
    assert_eq!((v["action"].as_str(), v["text"].as_str()),
        (Some("kept"), Some("bob couldn't be asked for the files (bob is offline on Soulseek) – it stays 12th in alice's queue")), "{v}");
    assert!(e.sh.lock().unwrap().calls.iter().all(|c| !c.starts_with("DELETE")), "alice untouched");
    let (_, message, d) = row(&st, id);
    assert_eq!((message.as_str(), d["slskd"]["idx"].as_i64()), ("Queued by alice – 0/4 files", Some(0)));
    // a download that moved meanwhile: no switch
    st.db.execute("UPDATE requests SET detail=json_set(detail, '$.slskd.last_bytes', 1000) WHERE id=?", p![id]).unwrap();
    assert_eq!(dn::download_now(&st, id, "carol", true).await.unwrap_err(), (409, "It is downloading now".to_string()));

    // alice's upload starts while the users are asked: nothing is handed over, nothing cancelled
    {
        let mut s = e.sh.lock().unwrap();
        s.offline.clear();
        s.calls.clear();
        s.alice_starts_on = Some("INFO bob".into());
    }
    let id = queued_at_alice(&e, "Illuminate EP", &[answer("bob", true, good, "flac", None)]).await;
    assert_eq!(dn::download_now(&st, id, "carol", true).await.unwrap_err(), (409, "It started downloading meanwhile".to_string()));
    assert!(e.sh.lock().unwrap().calls.is_empty(), "no POST, no DELETE: {:?}", e.sh.lock().unwrap().calls);
    // … or while bob's files are handed over: bob's go again, alice's stay
    {
        let mut s = e.sh.lock().unwrap();
        s.alice_started = false;
        s.alice_starts_on = Some("POST bob".into());
    }
    let id = queued_at_alice(&e, "Illuminate EP", &[answer("bob", true, good, "flac", None)]).await;
    assert_eq!(dn::download_now(&st, id, "carol", true).await.unwrap_err(), (409, "It started downloading meanwhile".to_string()));
    let calls = e.sh.lock().unwrap().calls.clone();
    assert_eq!(calls.first().map(String::as_str), Some("POST bob"), "{calls:?}");
    assert_eq!(calls.iter().filter(|c| c.starts_with("DELETE bob/")).count(), 4, "{calls:?}");
    assert!(!calls.iter().any(|c| c.starts_with("DELETE alice/")), "alice untouched: {calls:?}");
    let (_, message, d) = row(&st, id);
    assert_eq!((message.as_str(), d["slskd"]["idx"].as_i64(), d["slskd_tried"].clone()), ("Queued by alice – 0/4 files", Some(0), Value::Null), "{d}");
    {
        let mut s = e.sh.lock().unwrap();
        s.alice_started = false;
        s.alice_starts_on = None;
        s.calls.clear();
    }

    // bob is also further down the worker's own list: after the switch he is next, and only once
    let id = queued_at_alice(&e, "Illuminate EP", &answers).await;
    let later = |u: &str| json!({ "username": u, "directory": format!("Music/{good}"), "format": "flac", "bitrate": 0, "score": 50,
        "files": files(good).iter().map(|n| json!({ "filename": n, "size": 20_000_000 })).collect::<Vec<_>>() });
    let first = row(&st, id).2["slskd"]["candidates"][0].clone();
    st.db.execute("UPDATE requests SET detail=json_set(detail, '$.slskd.candidates', json(?)) WHERE id=?",
        p![json!([first, later("zoe"), later("bob")]).to_string(), id]).unwrap();
    let v = dn::download_now(&st, id, "carol", true).await.unwrap();
    assert_eq!(v["action"], "switched", "{v}");
    let d = row(&st, id).2;
    let names: Vec<&str> = d["slskd"]["candidates"].as_array().unwrap().iter().map(|c| c["username"].as_str().unwrap()).collect();
    assert_eq!(names, ["alice", "bob", "zoe"], "{d}");

    // files someone chose – a pick in the picker (with or without the audio check), Download anyway – are
    // never swapped for another user's: no button, and a press changes nothing
    let pick = json!({ "source": "slskd", "payload": { "username": "alice" } });
    let picked = "It was picked by hand – it waits 12th in alice's queue; pick another file in the picker to change it";
    for (extra, words) in [
        (json!({ "pick": pick }), picked),
        (json!({ "pick": pick, "skip_check": { "by": "carol", "why": "pick", "at": 1.0 } }), picked),
        (json!({ "pick": pick, "skip_check": { "by": "carol", "why": "download", "at": 1.0 } }), "Download anyway asked for exactly these files – it waits 12th in alice's queue"),
    ] {
        e.sh.lock().unwrap().calls.clear();
        let id = queued_at_alice(&e, "Illuminate EP", &answers).await;
        st.db.execute("UPDATE requests SET detail=json_patch(detail, ?) WHERE id=?", p![extra.to_string(), id]).unwrap();
        let f = dn::row_fields(&st, &worker::get(&st, id).unwrap());
        assert_eq!((f["can_download_now"].clone(), f["download_now"].clone(), f["remote_queue"]["text"].as_str()),
            (json!(false), Value::Null, Some("12th in alice's upload queue")), "{f:?}");
        assert_eq!(dn::download_now(&st, id, "carol", true).await.unwrap_err(), (409, words.to_string()));
        assert!(e.sh.lock().unwrap().calls.is_empty(), "no POST, no DELETE: {:?}", e.sh.lock().unwrap().calls);
        assert_eq!(row(&st, id).2["slskd"]["idx"], 0);
    }
    e.ts.remove();
}

// ---- a torrent with no progress ------------------------------------------------------------------------

const HASH: &str = "c12fe1c06bba254a9dc9f519b335aa7c1367a88a";

/// qBittorrent: its calls (path, fields) and the torrent it lists (`state`, `progress` 0–1).
async fn fake_qbit(calls: Arc<Mutex<Vec<(String, Value)>>>, state: &'static str, progress: f64) -> String {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", l.local_addr().unwrap());
    let (c1, c2) = (calls.clone(), calls.clone());
    let app = Router::new()
        .route("/api/v2/auth/login", post(|| async { ([("Set-Cookie", "SID=s; path=/")], "Ok.") }))
        .route("/api/v2/torrents/info", get(move || {
            let c = c1.clone();
            async move {
                c.lock().unwrap().push(("torrents/info".into(), json!({})));
                Json(json!([{ "hash": HASH, "tags": "encore-1", "state": state, "progress": progress, "dlspeed": 0, "content_path": "" }]))
            }
        }))
        .route("/api/v2/torrents/{op}", post(move |UrlPath(op): UrlPath<String>, mut m: Multipart| {
            let c = c2.clone();
            async move {
                let mut v = json!({});
                while let Ok(Some(f)) = m.next_field().await {
                    let k = f.name().unwrap_or("").to_string();
                    v[k] = json!(f.text().await.unwrap_or_default());
                }
                c.lock().unwrap().push((format!("torrents/{op}"), v));
                "Ok."
            }
        }));
    tokio::spawn(async move {
        let _ = axum::serve(l, app).await;
    });
    base
}

fn torrent(st: &AppState, owner: i64, started: f64, last_change: f64, sources: Value, pick: bool, queued: bool) -> i64 {
    let t = now();
    let cand = json!({ "title": "Duumu - Illuminate EP (2024) [FLAC]", "indexer": "Tracker", "protocol": "torrent", "url": format!("magnet:?xt=urn:btih:{HASH}") });
    let mut d = json!({ "sources": sources, "log": [],
        "torznab": { "candidates": [cand], "idx": 0, "id": format!("qbit:{HASH}"), "started": t - started, "last_change": t - last_change, "last_progress": 0,
                     "queued_since": if queued { json!(t - 60.0) } else { Value::Null } } });
    if pick {
        d["pick"] = json!({ "source": "torznab", "payload": cand });
    }
    insert(st, owner, "album", "Illuminate EP", "downloading", "torznab", "qBittorrent: stalled", d)
}

/// No progress for six minutes and another source to try: the client drops the torrent with its data and the
/// next source runs. A hand pick has no next source; a torrent in the client's own queue, one that moved
/// three minutes ago and a finished one the client still checks (or moves) are left alone.
#[tokio::test]
async fn a_torrent_with_no_progress_goes_to_the_next_source() {
    let ts = TestServer::new("dn-torrent").with_sources();
    let st = ts.st.clone();
    let calls: Arc<Mutex<Vec<(String, Value)>>> = Default::default();
    let url = fake_qbit(calls.clone(), "stalledDL", 0.0).await;
    let dl = ts.dir.join("torrents");
    std::fs::create_dir_all(&dl).unwrap();
    st.cfg.set_many(&json!({ "library_path": ts.dir.join("music").to_string_lossy(), "staging_path": ts.dir.join("staging").to_string_lossy(),
        "musicbrainz_url": "http://127.0.0.1:9", "source_order": "torznab,slskd", "ytdlp_last_resort": false, "auto_approve": true, "slskd_enabled": false,
        "torznab_enabled": true, "prowlarr_url": url.clone(), "prowlarr_api_key": "k", "torrent_client": "qbittorrent", "qbit_url": url, "qbit_download_path": dl.to_string_lossy() }));
    user(&st, "boss-t", "admin");
    let app = ts.app();
    let admin = login(&app, "boss-t").await;
    let id = torrent(&st, 1, 900.0, 360.0, json!(["slskd"]), false, false);
    let r = listed(&app, &admin, id).await;
    assert_eq!((r["can_download_now"].as_bool(), r["download_now"]["act"].as_str(), r["download_now"]["title"].as_str()),
        (Some(true), Some("next_source"), Some("Stop this download (no progress for 6 minutes) and try the next source: Soulseek")), "{r}");
    assert!(r["stalled_since"].as_f64().is_some_and(|s| now() - s >= 359.0), "{r}");
    let (code, v) = press(&app, &admin, id).await;
    assert_eq!((code, v["ok"].as_bool(), v["action"].as_str(), v["text"].as_str()),
        (StatusCode::OK, Some(true), Some("next_source"), Some("Trying the next source: Soulseek")), "{v}");
    until("the next source ran", Duration::from_secs(20), || row(&st, id).0 == "failed").await;
    let deleted: Vec<Value> = calls.lock().unwrap().iter().filter(|c| c.0 == "torrents/delete").map(|c| c.1.clone()).collect();
    assert_eq!(deleted.len(), 1, "{deleted:?}");
    assert_eq!((deleted[0]["hashes"].as_str(), deleted[0]["deleteFiles"].as_str()), (Some(HASH), Some("true")), "{deleted:?}");
    let (_, message, d) = row(&st, id);
    let log = log_text(&d);
    assert!(log.contains("torznab: no progress in qBittorrent for 6 minutes – Download now by boss-t"), "{log}");
    assert!(log.contains("slskd: "), "Soulseek was tried next: {log}");
    assert!(message.starts_with("All sources exhausted"), "{message}");
    assert!(d["torznab"]["id"].is_null() && d["torznab"]["idx"] == 1, "the search's other releases are skipped: {d}");
    // a hand pick: no other source
    let pick = torrent(&st, 1, 900.0, 360.0, json!([]), true, false);
    assert_eq!(listed(&app, &admin, pick).await["can_download_now"], false);
    assert_eq!(press(&app, &admin, pick).await.1["detail"], "No other source to try – the download keeps waiting for seeds");
    // in the client's own queue
    let q = torrent(&st, 1, 900.0, 360.0, json!(["slskd"]), false, true);
    assert_eq!(press(&app, &admin, q).await.1["detail"], "It waits in qBittorrent's own queue – Encore doesn't change the client's limit");
    // progress three minutes ago
    let moving = torrent(&st, 1, 900.0, 180.0, json!(["slskd"]), false, false);
    let r = listed(&app, &admin, moving).await;
    assert_eq!((r["can_download_now"].as_bool(), r["stalled_since"].clone()), (Some(false), Value::Null), "{r}");
    assert_eq!(press(&app, &admin, moving).await.1["detail"], "It is downloading now");
    // downloaded, and qBittorrent checks the files (its last progress six minutes ago): busy with them, not
    // stalled – a press would throw a finished download away
    let url = fake_qbit(calls.clone(), "checkingUP", 1.0).await;
    st.cfg.set_many(&json!({ "qbit_url": url }));
    let busy = torrent(&st, 1, 900.0, 360.0, json!(["slskd"]), false, false);
    st.db.execute("UPDATE requests SET detail=json_set(detail, '$.torznab.last_progress', 100) WHERE id=?", p![busy]).unwrap();
    assert_eq!(listed(&app, &admin, busy).await["can_download_now"], true, "before the client says so");
    worker::step(st.clone(), busy).await;
    let (status, _, d) = row(&st, busy);
    assert!(status == "downloading" && d["torznab"]["busy"].is_number(), "{status} {d}");
    let r = listed(&app, &admin, busy).await;
    assert_eq!((r["can_download_now"].as_bool(), r["stalled_since"].clone()), (Some(false), Value::Null), "{r}");
    assert_eq!(press(&app, &admin, busy).await.1["detail"], "qBittorrent is busy with the downloaded files – it is imported once they are ready");
    assert_eq!(calls.lock().unwrap().iter().filter(|c| c.0 == "torrents/delete").count(), 1, "nothing else dropped");
    ts.remove();
}

// ---- every state ---------------------------------------------------------------------------------------

/// What every state offers (`can_download_now`, `download_now`) and how a press is answered.
#[tokio::test]
async fn every_state_offers_what_it_can() {
    let ts = TestServer::new("dn-states").with_sources();
    let st = ts.st.clone();
    st.cfg.set_many(&json!({ "staging_path": ts.dir.join("staging").to_string_lossy(), "library_path": ts.dir.join("lib").to_string_lossy(),
        "musicbrainz_url": "http://127.0.0.1:9", "slskd_enabled": false, "auto_approve": false }));
    user(&st, "boss-s", "admin");
    let app = ts.app();
    let admin = login(&app, "boss-s").await;
    let t = now();
    let cand = json!({ "username": "alice", "directory": "Music/A", "files": [{ "filename": "Music\\A\\01.flac", "size": 1 }], "format": "flac", "bitrate": 0 });
    let album = insert(&st, 1, "album", "Illuminate EP", "searching", "slskd", "Searching Soulseek…", json!({}));
    let none: [(&str, &str, &str, Value, &str); 10] = [
        ("searching", "streamrip", "Looking for an exact lossless match…", json!({}), "It is searching now"),
        ("downloading", "slskd", "Downloading from alice – 1/4 files", json!({ "slskd": { "candidates": [cand], "idx": 0, "last_bytes": 5000 } }), "It is downloading now"),
        ("downloading", "slskd", "Queued by alice – 0/4 files", json!({ "slskd": { "candidates": [cand], "idx": 0, "last_bytes": 0 } }), "It is downloading now"),
        ("downloading", "slskd", "Waiting for Soulseek to take the download from alice (it is busy with other downloads)",
            json!({ "slskd": { "candidates": [cand], "idx": 0, "last_bytes": 0, "pending": { "since": t }, "remote": { "place": 3 } } }), "It is downloading now"),
        ("importing", "streamrip", "Verifying, tagging and importing…", json!({}), "It is being imported now"),
        ("monitoring", "album", "Getting the whole album “Illuminate EP”", json!({ "album_request": album }), "Its album “Illuminate EP” brings it – use Download now on the album"),
        ("waiting", "streamrip", "Waiting to import the downloaded files", json!({ "wait": { "reason": "storage_import", "since": t, "next_try_at": t + 600.0 },
            "kept": { "files": [] } }), "Its files are downloaded – Import again tries the import now"),
        // waiting for its source's plugin, which Encore still installs after the upgrade (plugins, 0.32)
        ("waiting", "ytdlp", "Waiting for the yt-dlp plugin to finish installing", json!({ "sources": ["slskd"],
            "install_wait": { "source": "ytdlp", "since": t, "step": "source" }, "wait": { "reason": "plugin_install", "since": t } }),
            "It waits for the yt-dlp plugin to finish installing – it starts as soon as the plugin is installed"),
        ("pending", "", "", json!({}), "It waits for an admin's approval"),
        ("completed", "", "Downloaded", json!({}), "Only a request that waits can be downloaded now"),
    ];
    for (status, source, message, d, words) in none {
        let id = insert(&st, 1, "track", "Song", status, source, message, d);
        let r = listed(&app, &admin, id).await;
        assert_eq!((r["can_download_now"].as_bool(), r["download_now"].clone(), r["line"].clone()), (Some(false), Value::Null, Value::Null), "{status} {message}: {r}");
        let (code, v) = press(&app, &admin, id).await;
        assert_eq!((code, v["detail"].as_str()), (StatusCode::CONFLICT, Some(words)), "{status} {message}");
    }
    for status in ["failed", "cancelled", "denied"] {
        let id = insert(&st, 1, "track", "Song", status, "", "", json!({}));
        assert_eq!(press(&app, &admin, id).await.1["detail"], "Only a request that waits can be downloaded now");
    }
    // queued and in no line: no button (its step starts within 5 s), and a press wakes the queue
    let q = insert(&st, 1, "track", "Song", "queued", "", "", json!({}));
    assert_eq!(listed(&app, &admin, q).await["can_download_now"], false);
    let (code, v) = press(&app, &admin, q).await;
    assert_eq!((code, v["ok"].as_bool(), v["action"].as_str(), v["text"].as_str()), (StatusCode::OK, Some(true), Some("now"), Some("Starting now")), "{v}");
    assert_eq!(listed(&app, &admin, q).await["pressed"]["by"], "boss-s", "kept for the lines it enters");
    // waiting for Soulseek while Soulseek is off: the next source now
    let w = insert(&st, 1, "track", "Song", "waiting", "slskd", "Waiting for Soulseek – connecting shortly",
        json!({ "sources": ["slskd"], "soulseek_wait": { "since": t }, "wait": { "reason": "soulseek", "since": t } }));
    let r = listed(&app, &admin, w).await;
    assert_eq!((r["can_download_now"].as_bool(), r["download_now"]["act"].as_str(), r["download_now"]["title"].as_str()),
        (Some(true), Some("wake"), Some("Try the next source now")), "{r}");
    let (_, v) = press(&app, &admin, w).await;
    assert_eq!((v["ok"].as_bool(), v["text"].as_str()), (Some(true), Some("Soulseek is stopped (Soulseek is off) – trying the next source")), "{v}");
    until("it went on", Duration::from_secs(10), || row(&st, w).0 != "waiting").await;
    // switched on, but the engine can't start or isn't installed: the reason the card shows, not "off"
    st.cfg.set_many(&json!({ "slskd_enabled": true, "soulseek_username": "me", "soulseek_password": "a-password" }));
    for (state, why) in [("blocked", "Soulseek can't start – see Settings → Plugins → Soulseek"), ("not_installed", "Soulseek is not installed")] {
        st.requests.slskd.set_state_for_tests(state);
        let w = insert(&st, 1, "track", "Song", "waiting", "slskd", "Waiting for Soulseek – connecting shortly",
            json!({ "sources": ["slskd"], "soulseek_wait": { "since": t }, "wait": { "reason": "soulseek", "since": t } }));
        assert_eq!(listed(&app, &admin, w).await["download_now"]["title"], "Try the next source now");
        let (_, v) = press(&app, &admin, w).await;
        assert_eq!(v["text"], format!("Soulseek is stopped ({why}) – trying the next source"), "{state}: {v}");
        until("it went on", Duration::from_secs(10), || row(&st, w).0 != "waiting").await;
    }
    // a request that doesn't exist
    assert_eq!(press(&app, &admin, 99_999).await.0, StatusCode::NOT_FOUND);
    ts.remove();
}

// ---- who may press it; Download all now ----------------------------------------------------------------

/// The owner and admins press it; another member gets 403. Download all now is the admins': with six in the
/// streamrip line, two storage waits and a Soulseek wait it presses the line in the order it stands (with
/// `mine` the admin's first), wakes the waits, leaves pending requests alone and never runs more than one rip.
#[tokio::test]
async fn who_may_press() {
    let ts = TestServer::new("dn-who").with_sources();
    let st = ts.st.clone();
    let (staging, lib) = (ts.dir.join("staging"), ts.dir.join("lib"));
    std::fs::create_dir_all(&staging).unwrap();
    std::fs::create_dir_all(&lib).unwrap();
    st.cfg.set_many(&json!({ "staging_path": staging.to_string_lossy(), "library_path": lib.to_string_lossy(), "musicbrainz_url": "http://127.0.0.1:9",
        "slskd_enabled": false, "auto_approve": false, "streamrip_enabled": false, "ytdlp_enabled": false, "ytdlp_last_resort": false }));
    let boss = user(&st, "boss-p", "admin").i("id");
    let mia = user(&st, "mia-p", "user").i("id");
    user(&st, "max-p", "user");
    let app = ts.app();
    let (admin, mia_c, max_c) = (login(&app, "boss-p").await, login(&app, "mia-p").await, login(&app, "max-p").await);
    let q = insert(&st, mia, "track", "Song", "queued", "", "", json!({}));
    let (code, v) = press(&app, &max_c, q).await;
    assert_eq!((code, v["detail"].as_str()), (StatusCode::FORBIDDEN, Some("Not your request")));
    assert_eq!(press(&app, &mia_c, q).await.0, StatusCode::OK);
    assert_eq!(press(&app, &admin, q).await.0, StatusCode::OK, "admins press anyone's");
    let (code, v, _) = call(&app, "POST", "/api/requests/download-now", &mia_c, Some(json!({}))).await;
    assert_eq!((code, v["detail"].as_str()), (StatusCode::FORBIDDEN, Some("Admin only")));
    st.db.execute("UPDATE requests SET status='completed' WHERE id=?", p![q]).unwrap();

    // six in the streamrip line behind a running one (mia's and the admin's in turn), held as the worker holds them
    let lines = st.requests.lines.clone();
    let running = insert(&st, mia, "album", "Running", "downloading", "streamrip", "streamrip: …", json!({}));
    let slot = lines.streamrip("deezer").acquire(running, "Deezer", &lines.pressed, || {}).await;
    let mut waiting = vec![];
    let mut tasks = vec![];
    for n in 0..6 {
        let owner = if n % 2 == 0 { mia } else { boss };
        let id = insert(&st, owner, "album", &format!("In line {n}"), "queued", "streamrip", "In line for Deezer", json!({}));
        let l = lines.clone();
        tasks.push(tokio::spawn(async move {
            let _s = l.streamrip("deezer").acquire(id, "Deezer", &l.pressed, || {}).await;
            std::future::pending::<()>().await;
        }));
        until("in line", Duration::from_secs(5), || lines.streamrip("deezer").waiting() == n + 1).await;
        waiting.push(id);
    }
    // two downloads waiting for the staging folder (writable again), one for Soulseek (off), one pending
    let t = now();
    let storage: Vec<i64> = (0..2).map(|n| insert(&st, mia, "album", &format!("Storage {n}"), "waiting", "ytdlp", "Waiting: storage error – …",
        json!({ "sources": [], "wait": { "reason": "storage_download", "since": t, "next_try_at": t + 600.0, "dir": staging.join("request-x").to_string_lossy() } }))).collect();
    let sk = insert(&st, boss, "track", "Song", "waiting", "slskd", "Waiting for Soulseek – connecting shortly",
        json!({ "sources": ["slskd"], "soulseek_wait": { "since": t }, "wait": { "reason": "soulseek", "since": t } }));
    let pending = insert(&st, mia, "album", "Pending", "pending", "", "", json!({}));
    let order = || lines.streamrip("deezer").waiters(&lines.pressed).into_iter().map(|w| w.0).collect::<Vec<_>>();
    assert_eq!(order(), waiting);
    // mine: the admin's first, in their order
    let (code, v, _) = call(&app, "POST", "/api/requests/download-now", &admin, Some(json!({ "mine": true }))).await;
    assert_eq!(code, StatusCode::OK, "{v}");
    assert_eq!((v["pressed"].as_u64(), v["woken"].as_u64()), (Some(3), Some(1)), "{v}");
    assert!(v["text"].as_str().unwrap().starts_with("Moved 3 of your requests to the front of their lines and woke 1 waiting request; 6 stay in line: Deezer 1 download at a time (6 in line)"), "{v}");
    assert_eq!(order(), [waiting[1], waiting[3], waiting[5], waiting[0], waiting[2], waiting[4]]);
    // everything: nobody overtakes anybody
    let (code, v, _) = call(&app, "POST", "/api/requests/download-now", &admin, Some(json!({}))).await;
    assert_eq!(code, StatusCode::OK, "{v}");
    assert_eq!((v["ok"].as_bool(), v["pressed"].as_u64(), v["woken"].as_u64(), v["later"].as_u64()), (Some(true), Some(6), Some(3), Some(0)), "{v}");
    assert_eq!(v["lines"], json!([{ "source": "streamrip", "label": "Deezer", "limit": 1, "running": 1, "waiting": 6 }]), "{v}");
    assert_eq!(v["text"], "Woke 3 waiting requests; 6 stay in line: Deezer 1 download at a time (6 in line)", "{v}");
    assert_eq!(order(), [waiting[1], waiting[3], waiting[5], waiting[0], waiting[2], waiting[4]], "the order stays");
    assert_eq!((lines.streamrip("deezer").running(), lines.streamrip("deezer").waiting()), (1, 6), "never more than its slot");
    for id in &storage {
        assert!(row(&st, *id).2["wait"]["next_try_at"].as_f64().unwrap() <= now() + 1.0, "the storage waits are due now");
    }
    let _ = sk;
    assert_eq!(row(&st, pending).0, "pending", "pending requests untouched");
    // the list says where each one stands
    let r = listed(&app, &admin, waiting[0]).await;
    assert_eq!((r["message"].as_str(), r["line"]["position"].as_u64(), r["line"]["first"].as_bool()), (Some("4th in line for Deezer"), Some(4), Some(true)), "{r}");
    drop(slot);
    for t in tasks {
        t.abort();
    }
    ts.remove();
}
