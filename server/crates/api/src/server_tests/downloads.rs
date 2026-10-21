//! The download folders' probe (encore_requests::worker::downloads) on a whole server: a request whose
//! source saves in a folder that can't be written waits ("Waiting for the downloads folder") instead of
//! failing – no attempt counted, no other source tried, the indexers not even asked – and goes on by itself
//! once the probe passes; admins read the banner (`downloads` in GET /api/requests/storage), everyone else
//! plain words without the server's folders; Check again probes at once. The probe is a stand-in
//! ([`downloads::fake_probe`]) for the pause and resume, and the real one through the fault seam for the
//! errnos a test can't make without privileges (EROFS, ENOSPC).
use crate::state::AppState;
use crate::test_support::{call, user, TestServer};
use axum::extract::Query;
use axum::http::StatusCode;
use axum::routing::get;
use axum::Router;
use encore_core::fsx::{fault, Step, StepError};
use encore_db::{now, p, RowExt};
use encore_requests::worker::{self, downloads};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Folders the stand-in probe finds failing, with their errno.
type Failing = Arc<Mutex<HashMap<PathBuf, i32>>>;

struct Env {
    ts: TestServer,
    staging: PathBuf,
    /// the torrent client's folder as Encore sees it
    torrents: PathBuf,
    admin: i64,
    member: i64,
    /// what the stand-in Prowlarr was asked
    searches: Arc<Mutex<Vec<String>>>,
    failing: Failing,
}

impl Env {
    fn st(&self) -> &AppState {
        &self.ts.st
    }
    /// The stand-in probe from now on: a folder in `failing` fails with its errno.
    fn fake(&self) {
        let failing = self.failing.clone();
        downloads::fake_probe(self.st(), Some(Arc::new(move |dir: &Path| match failing.lock().unwrap().get(dir) {
            Some(n) => Err(StepError::new(Step::Download, "write", &dir.join(".encore-probe-1"), None, std::io::Error::from_raw_os_error(*n))),
            None => Ok(()),
        })));
    }
    fn fail(&self, dir: &Path, errno: i32) {
        self.failing.lock().unwrap().insert(dir.to_path_buf(), errno);
    }
    fn fix(&self) {
        self.failing.lock().unwrap().clear();
    }
    fn searches(&self) -> usize {
        self.searches.lock().unwrap().len()
    }
    fn done(self) {
        fault::clear(&self.staging);
        fault::clear(&self.torrents);
        self.ts.remove();
    }
}

/// A server with its own staging folder and torrent folder; torrents through qBittorrent, found on a
/// stand-in Prowlarr that finds nothing (its searches are kept); no MusicBrainz, no yt-dlp.
async fn env(name: &str) -> Env {
    let ts = TestServer::new(&format!("downloads-{name}")).with_sources();
    let (staging, torrents, lib) = (ts.dir.join("dl/staging"), ts.dir.join("dl/torrents"), ts.dir.join("music"));
    for d in [&staging, &torrents, &lib] {
        std::fs::create_dir_all(d).unwrap();
    }
    let searches: Arc<Mutex<Vec<String>>> = Default::default();
    let asked = searches.clone();
    let app = Router::new().route("/api/v1/search", get(move |Query(q): Query<HashMap<String, String>>| {
        let asked = asked.clone();
        async move {
            asked.lock().unwrap().push(q.get("query").cloned().unwrap_or_default());
            axum::Json(json!([]))
        }
    }));
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move {
        let _ = axum::serve(l, app).await;
    });
    ts.st.cfg.set_many(&json!({ "library_path": lib.to_string_lossy(), "staging_path": staging.to_string_lossy(), "musicbrainz_url": "http://127.0.0.1:9",
        "source_order": "torznab", "ytdlp_enabled": false, "ytdlp_last_resort": false, "streamrip_enabled": false, "auto_approve": true,
        "torznab_enabled": true, "prowlarr_url": url, "prowlarr_api_key": "k", "torrent_client": "qbittorrent", "qbit_url": url,
        "qbit_download_path": torrents.to_string_lossy() }));
    let admin = user(&ts.st, &format!("admin-{name}"), "admin").i("id");
    let member = user(&ts.st, &format!("member-{name}"), "user").i("id");
    Env { ts, staging, torrents, admin, member, searches, failing: Default::default() }
}

/// A queued album request of `user` (attempts 3: a wait must not change it).
fn album(e: &Env, user: i64, title: &str) -> i64 {
    let t = now();
    e.st().db.execute("INSERT INTO requests (user_id, kind, artist, title, album, status, message, detail, created_at, updated_at) VALUES (?,?,?,?,?,?,?,?,?,?)",
        p![user, "album", "Band", title, title, "queued", "", r#"{"attempts": 3}"#, t, t]).unwrap()
}

fn row(st: &AppState, id: i64) -> (String, String, String, Value, f64) {
    let r = st.db.one("SELECT status, message, source, detail, updated_at FROM requests WHERE id=?", p![id]).unwrap().unwrap();
    (r.s("status"), r.s("message"), r.s("source"), serde_json::from_str(&r.s("detail")).unwrap_or(json!({})), r.f("updated_at"))
}

fn activity(st: &AppState, like: &str) -> Vec<String> {
    st.db.query("SELECT message FROM activity WHERE message LIKE ? ORDER BY id", p![like]).unwrap().iter().map(|r| r.s("message")).collect()
}

async fn login(app: &Router, name: &str) -> String {
    let (code, _, cookie) = call(app, "POST", "/api/auth/login", "", Some(json!({ "username": name, "password": "a-long-password-1" }))).await;
    assert_eq!(code, StatusCode::OK);
    cookie.split(';').next().unwrap_or("").to_string()
}

/// The backlog's item 17 on the torrent side: the torrent folder is missing (a mount that went away). The
/// request waits with its source first again, its attempts unchanged and the indexers not asked; nothing is
/// written again while the folder still fails; admins read the banner and every folder with its result,
/// members plain words; one log line each way; once the probe passes the request goes on to the indexers.
#[tokio::test]
async fn a_missing_torrent_folder_holds_the_request_until_the_probe_passes() {
    let e = env("missing").await;
    let st = e.st().clone();
    e.fake();
    e.fail(&e.torrents, 2);
    downloads::probe_all(&st);
    let id = album(&e, e.member, "Album");
    worker::step(st.clone(), id).await;
    let (status, message, source, d, at) = row(&st, id);
    assert_eq!(status, "waiting", "{message}");
    assert_eq!(message, format!("Waiting for the downloads folder – {} is missing", e.torrents.display()));
    assert_eq!((source.as_str(), d["sources"].clone(), d["attempts"].clone()), ("torznab", json!(["torznab"]), json!(3)), "{d}");
    assert_eq!((d["wait"]["reason"].as_str(), d["wait"]["source"].as_str()), (Some("waiting_disk"), Some("torznab")));
    assert_eq!((d["wait"]["dir"].as_str(), d["wait"]["error"]["errno"].as_i64()), (e.torrents.to_str(), Some(2)), "{d}");
    assert_eq!(e.searches(), 0, "the indexers are not asked while the folder can't be written");
    // the next looks change nothing while the folder still fails (a pass in between included)
    downloads::probe_all(&st);
    worker::step(st.clone(), id).await;
    worker::step(st.clone(), id).await;
    let (status, _, _, d2, at2) = row(&st, id);
    assert_eq!((status.as_str(), at2), ("waiting", at), "nothing written again");
    assert_eq!(d2["log"].as_array().map(|l| l.len()), Some(1), "one line in the request's log: {d2}");
    // one log line when the folder started failing (not one per pass or per request)
    let lines = activity(&st, "downloads folder:%");
    assert_eq!(lines, vec![format!("downloads folder: {} is missing – requests that save there wait", e.torrents.display())]);

    // the banner and the folders for an admin
    let app = e.ts.app();
    let admin = login(&app, "admin-missing").await;
    let (code, s, _) = call(&app, "GET", "/api/requests/storage", &admin, None).await;
    assert_eq!(code, StatusCode::OK);
    let dl = &s["downloads"];
    assert_eq!((dl["ok"].clone(), dl["waiting"].clone()), (json!(false), json!(1)), "{s}");
    assert_eq!(dl["text"], format!("The downloads folder {} is missing – check the mount", e.torrents.display()));
    assert!(dl["checked_at"].as_f64().is_some() && dl["next_check_at"].as_f64() == dl["checked_at"].as_f64().map(|t| t + 60.0), "{dl}");
    let roots = dl["roots"].as_array().unwrap();
    let staging = roots.iter().find(|r| r["key"] == "staging_path").unwrap();
    assert_eq!((staging["state"].as_str(), staging["ok"].as_bool(), staging["own"].as_bool()), (Some("ok"), Some(true), Some(true)), "{staging}");
    let torrents = roots.iter().find(|r| r["key"] == "qbit_download_path").unwrap();
    assert_eq!((torrents["state"].as_str(), torrents["ok"].as_bool(), torrents["own"].as_bool(), torrents["result"].as_str()),
        (Some("missing"), Some(false), Some(false), Some("Missing")), "{torrents}");
    assert_eq!((torrents["label"].as_str(), torrents["sources"].clone()), (Some("Torrent downloads (qBittorrent)"), json!(["torznab"])));
    assert!(torrents["since"].as_f64().is_some() && torrents["checked_at"].as_f64().is_some(), "{torrents}");
    assert_eq!(s["ok"], true, "imports are not paused for it");
    // a member: no folders, no banner, the plain words on the request
    let member = login(&app, "member-missing").await;
    let (_, s, _) = call(&app, "GET", "/api/requests/storage", &member, None).await;
    assert_eq!(s["downloads"], json!({ "ok": false }));
    let (_, list, _) = call(&app, "GET", "/api/requests", &member, None).await;
    let r = list.as_array().unwrap().iter().find(|r| r["id"] == id).unwrap().clone();
    assert_eq!((r["status"].as_str(), r["wait_reason"].as_str()), (Some("waiting"), Some("waiting_disk")), "{r}");
    assert_eq!(r["message"], "Waiting for the downloads folder – a storage problem on the server; the download starts when it is fixed");
    assert_eq!(r["error"], json!({ "storage": true, "text": "A storage problem on the server – not the source" }));
    assert!(!r.to_string().contains(e.torrents.to_str().unwrap()), "no server folder for a member: {r}");

    // the folder is back: one log line, and the request goes on to the indexers by itself
    e.fix();
    downloads::probe_all(&st);
    assert_eq!(activity(&st, "downloads folder:%").last().unwrap(), &format!("downloads folder: {} can be written again; 1 waiting request goes on", e.torrents.display()));
    worker::step(st.clone(), id).await;
    let (status, message, _, d, _) = row(&st, id);
    assert!(e.searches() > 0, "the indexers were asked");
    assert_eq!(status, "failed", "{message}");
    assert!(message.ends_with("torznab: no matching release on your indexers)"), "{message}");
    assert_eq!(d["attempts"], 3);
    assert!(d["wait"].is_null(), "{d}");
    e.done();
}

/// The staging folder read-only (the Oct 17 disk): a hand pick and a Qobuz link wait for it too, while a
/// request whose source saves elsewhere (the torrent client's folder) goes on; once the folder is back
/// the pick and the link start again. A pick or a link whose plugin can't run fails at once with what it
/// needs instead of waiting for the folder first.
#[tokio::test]
async fn picks_and_links_wait_for_the_staging_folder_and_other_sources_go_on() {
    let e = env("staging").await;
    let st = e.st().clone();
    e.fake();
    e.fail(&e.staging, 30);
    downloads::probe_all(&st);
    let t = now();
    let insert_pick = |title: &str| {
        let pick = json!({ "pick": { "source": "deezer", "payload": { "url": "https://www.deezer.com/album/1" } }, "attempts": 1 });
        st.db.execute("INSERT INTO requests (user_id, kind, artist, title, album, status, message, detail, created_at, updated_at) VALUES (?,?,?,?,?,?,?,?,?,?)",
            p![e.admin, "album", "Band", title, title, "queued", "", pick.to_string(), t, t]).unwrap()
    };
    let insert_link = || st.db.execute("INSERT INTO requests (user_id, kind, url, status, message, detail, created_at, updated_at) VALUES (?,?,?,?,?,?,?,?)",
        p![e.admin, "url", "https://www.qobuz.com/album/x/1", "queued", "", "{}", t, t]).unwrap();
    // streamrip can't run here yet: the pick and the link say what they need at once, without waiting
    let (pick_off, link_off) = (insert_pick("Off"), insert_link());
    let other = album(&e, e.admin, "Elsewhere");
    for id in [pick_off, link_off, other] {
        worker::step(st.clone(), id).await;
    }
    for id in [pick_off, link_off] {
        let (status, message, ..) = row(&st, id);
        assert_eq!(status, "failed", "{message}");
        assert!(message.starts_with("Qobuz/Tidal/Deezer links need the streamrip plugin switched on"), "{message}");
    }
    let (status, message, ..) = row(&st, other);
    assert_eq!(status, "failed", "the torrent side is fine: {message}");
    assert!(e.searches() > 0);
    // streamrip on (a stand-in that notes its calls and fails): the pick and the link wait for the folder
    let fake = e.ts.dir.join("fake-rip");
    std::fs::create_dir_all(&fake).unwrap();
    let rip = fake.join("rip");
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(&rip, "#!/bin/sh\necho \"$*\" >> \"$(dirname \"$0\")/calls.log\"\nexit 1\n").unwrap();
        std::fs::set_permissions(&rip, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    st.cfg.set_many(&json!({ "streamrip_enabled": true, "streamrip_bin": rip.to_string_lossy() }));
    let (picked, link) = (insert_pick("Picked"), insert_link());
    for id in [picked, link] {
        worker::step(st.clone(), id).await;
    }
    let read_only = format!("Waiting for the downloads folder – {} is read-only", e.staging.display());
    let (status, message, source, d, _) = row(&st, picked);
    assert_eq!((status.as_str(), message.as_str(), source.as_str()), ("waiting", read_only.as_str(), "streamrip"), "{d}");
    assert_eq!(d["wait"]["source"], "streamrip");
    let (status, message, source, d, _) = row(&st, link);
    assert_eq!((status.as_str(), message.as_str(), source.as_str()), ("waiting", read_only.as_str(), ""), "{d}");
    assert_eq!(d["wait"]["source"], "url");
    assert!(!fake.join("calls.log").exists(), "streamrip was not run while the folder failed");
    // the folder is back: both start again (the stand-in streamrip is run, and fails)
    e.fix();
    downloads::probe_all(&st);
    for id in [picked, link] {
        worker::step(st.clone(), id).await;
        let (status, message, ..) = row(&st, id);
        assert_ne!(status, "waiting", "{message}");
    }
    assert!(fake.join("calls.log").exists(), "streamrip was run once the folder could be written");
    e.done();
}

/// After a restart nothing moves before the first pass: a request that waited goes on only once a probe
/// says the folder can be written, and a client's folder Encore may only read holds nothing up.
#[tokio::test]
async fn a_waiting_request_waits_for_the_first_pass_and_a_readable_client_folder_holds_nothing_up() {
    let e = env("restart").await;
    let st = e.st().clone();
    let t = now();
    let d = json!({ "attempts": 2, "sources": ["torznab"], "log": [],
                    "wait": { "reason": "waiting_disk", "since": t - 600.0, "next_try_at": null, "source": "torznab", "dir": e.torrents.to_string_lossy() } });
    let id = st.db.execute("INSERT INTO requests (user_id, kind, artist, title, album, status, source, message, detail, created_at, updated_at) VALUES (?,?,?,?,?,?,?,?,?,?,?)",
        p![e.admin, "album", "Band", "Album", "Album", "waiting", "torznab", "Waiting for the downloads folder – x is missing", d.to_string(), t, t]).unwrap();
    worker::recover(&st);
    worker::step(st.clone(), id).await;
    assert_eq!(row(&st, id).0, "waiting", "no pass yet");
    // Encore may not write in the client's folder, but reads it: fine. The first pass starts at the
    // worker's first look, the next one a minute later
    e.fake();
    e.fail(&e.torrents, 13);
    downloads::tick(&st, true).expect("the first pass starts at once").await.unwrap();
    assert!(downloads::tick(&st, true).is_none() && downloads::tick(&st, false).is_none(), "a minute between passes, ten while nothing downloads");
    let s = downloads::status(&st, true);
    assert_eq!((s["ok"].clone(), s["text"].clone()), (json!(true), Value::Null), "{s}");
    let torrents = s["roots"].as_array().unwrap().iter().find(|r| r["key"] == "qbit_download_path").unwrap().clone();
    assert_eq!((torrents["state"].as_str(), torrents["ok"].as_bool()), (Some("denied"), Some(true)), "{torrents}");
    assert!(torrents["note"].as_str().unwrap().contains("Encore only reads"), "{torrents}");
    worker::step(st.clone(), id).await;
    let (status, message, _, d, _) = row(&st, id);
    assert_eq!(status, "failed", "{message}");
    assert!(e.searches() > 0);
    assert_eq!(d["attempts"], 2);
    // Encore's own folder must be writable: the same errno there holds the downloads up
    e.fix();
    e.fail(&e.staging, 13);
    downloads::probe_all(&st);
    // SAFETY: both calls only read the process's credentials and cannot fail.
    let (uid, gid) = unsafe { (libc::geteuid(), libc::getegid()) };
    assert_eq!(downloads::status(&st, true)["text"], format!("The downloads folder {s} isn't writable by Encore (uid {uid}, gid {gid}) – chown {uid}:{gid} {s} on the host, or set PUID/PGID to its owner",
        s = e.staging.display()));
    e.done();
}

/// The real probe, through the fault seam: Check again (POST /api/requests/storage/check) probes every
/// folder at once; a read-only staging folder and a full torrent disk each get their sentence, both folders
/// read-only one sentence for the folder they share.
#[tokio::test]
async fn check_again_probes_now_and_tells_the_errnos_apart() {
    let e = env("check").await;
    let st = e.st().clone();
    let app = e.ts.app();
    let admin = login(&app, "admin-check").await;
    let check = || async { call(&app, "POST", "/api/requests/storage/check", &admin, None).await.1["downloads"].clone() };
    assert_eq!(check().await["ok"], true);
    fault::fail(&e.staging, "write", 30);
    fault::fail(&e.torrents, "fsync", 28);
    let dl = check().await;
    assert_eq!(dl["text"], format!("The downloads folder {} is read-only – check the disk (dmesg, fsck) or the mount. The downloads folder {} is full – free some space on its disk",
        e.staging.display(), e.torrents.display()), "{dl}");
    let state = |key: &str| dl["roots"].as_array().unwrap().iter().find(|r| r["key"] == key).map(|r| (r["state"].clone(), r["error"]["op"].clone())).unwrap();
    assert_eq!(state("staging_path"), (json!("read_only"), json!("write")));
    assert_eq!(state("qbit_download_path"), (json!("full"), json!("fsync")));
    assert!(std::fs::read_dir(&e.torrents).unwrap().next().is_none(), "the probe's file is gone again");
    // the whole downloads disk read-only: one sentence for the folder both lie in
    fault::clear(&e.torrents);
    fault::fail(&e.torrents, "write", 30);
    let dl = check().await;
    assert_eq!(dl["text"], format!("The downloads folder {} is read-only – check the disk (dmesg, fsck) or the mount", e.ts.dir.join("dl").display()), "{dl}");
    fault::clear(&e.staging);
    fault::clear(&e.torrents);
    let dl = check().await;
    assert_eq!((dl["ok"].clone(), dl["text"].clone()), (json!(true), Value::Null), "{dl}");
    assert!(downloads::tick(&st, true).is_none(), "Check again just probed: the background pass waits its minute");
    e.done();
}

/// The indexers' releases go to the torrent or the Usenet client. SABnzbd is never told a folder, so its
/// folder setting left at a path that isn't in Encore's container (SABnzbd's own path is mounted the same way)
/// holds nothing up; a request isn't held while one client's folder can be written; a release for the client
/// whose folder fails is not handed over – the request waits for that folder, and goes on once it is back.
#[tokio::test]
async fn a_release_waits_for_its_own_clients_folder() {
    let e = env("clients").await;
    let st = e.st().clone();
    let usenet = e.ts.dir.join("dl/usenet-not-here");
    let url = st.cfg.str("prowlarr_url");
    st.cfg.set_many(&json!({ "nzb_client": "sabnzbd", "sab_url": url, "sab_api_key": "k", "sab_download_path": usenet.to_string_lossy() }));
    // the real probe: SABnzbd's folder isn't there, which is fine
    let dl = call(&e.ts.app(), "POST", "/api/requests/storage/check", &login(&e.ts.app(), "admin-clients").await, None).await.1["downloads"].clone();
    assert_eq!((dl["ok"].clone(), dl["text"].clone()), (json!(true), Value::Null), "{dl}");
    let sab = dl["roots"].as_array().unwrap().iter().find(|r| r["key"] == "sab_download_path").unwrap().clone();
    assert_eq!((sab["state"].as_str(), sab["ok"].as_bool()), (Some("missing"), Some(true)), "{sab}");
    assert!(sab["note"].as_str().unwrap().starts_with("Not in Encore's container"), "{sab}");
    assert!(!usenet.exists(), "a client's folder is not made");

    // the torrent folder fails: an indexer request still starts (Usenet can take its releases)
    e.fake();
    e.fail(&e.torrents, 2);
    downloads::probe_all(&st);
    let asked = album(&e, e.admin, "Searched");
    worker::step(st.clone(), asked).await;
    assert!(e.searches() > 0, "the indexers were asked");
    assert_ne!(row(&st, asked).0, "waiting");

    // a torrent picked by hand is not handed to qBittorrent while its folder fails: it waits for that folder
    let t = now();
    let pick = json!({ "pick": { "source": "torznab", "payload": { "url": "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567", "title": "Band - Picked (2020) FLAC",
                       "protocol": "torrent", "indexer": "x" } }, "attempts": 1 });
    let picked = st.db.execute("INSERT INTO requests (user_id, kind, artist, title, album, status, message, detail, created_at, updated_at) VALUES (?,?,?,?,?,?,?,?,?,?)",
        p![e.admin, "album", "Band", "Picked", "Picked", "queued", "", pick.to_string(), t, t]).unwrap();
    worker::step(st.clone(), picked).await;
    let (status, message, source, d, at) = row(&st, picked);
    assert_eq!((status.as_str(), source.as_str()), ("waiting", "torznab"), "{message} {d}");
    assert_eq!(message, format!("Waiting for the downloads folder – {} is missing", e.torrents.display()));
    assert_eq!((d["wait"]["proto"].as_str(), d["wait"]["dir"].as_str(), d["attempts"].as_i64()), (Some("torrent"), e.torrents.to_str(), Some(1)), "{d}");
    // it stays while the torrent folder fails, though an indexer request as a whole could go on
    worker::step(st.clone(), picked).await;
    assert_eq!((row(&st, picked).0, row(&st, picked).4), ("waiting".to_string(), at), "nothing written again");
    // the folder is back: the pick is handed over (the stand-in has no qBittorrent, which it then says)
    e.fix();
    downloads::probe_all(&st);
    worker::step(st.clone(), picked).await;
    let (status, message, _, d, _) = row(&st, picked);
    assert_ne!(status, "waiting", "{message}");
    assert!(d["wait"].is_null(), "{d}");
    e.done();
}
