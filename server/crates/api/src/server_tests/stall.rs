//! An import that hasn't moved (encore_requests::worker::stall) on a whole server, with a stand-in stuck
//! import – a task that never ends in the request's place, as an import hung on a dead mount would be: the
//! row offers Import again only once nothing moved for 15 minutes (or its task is gone), Import again stops the
//! stuck task and imports the kept files at once in its place, and the watchdog starts a search or an import
//! whose task is gone again without a server restart.
use crate::state::AppState;
use crate::test_support::{call, user, TestServer};
use axum::http::StatusCode;
use encore_db::{now, p, RowExt};
use encore_requests::worker::{self, stall};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::Duration;

struct Env {
    ts: TestServer,
    lib: PathBuf,
    staging: PathBuf,
    owner: i64,
}

impl Env {
    fn st(&self) -> &AppState {
        &self.ts.st
    }
}

/// A server whose library and staging folder are its own; no MusicBrainz answers. The requests are sam's.
fn env(name: &str) -> Env {
    let ts = TestServer::new(&format!("stall-{name}")).with_sources();
    let (lib, staging) = (ts.dir.join("music"), ts.dir.join("staging"));
    for d in [&lib, &staging] {
        std::fs::create_dir_all(d).unwrap();
    }
    ts.st.cfg.set_many(&json!({ "library_path": lib.to_string_lossy(), "staging_path": staging.to_string_lossy(),
                               "musicbrainz_url": "http://127.0.0.1:9", "source_order": "ytdlp", "auto_approve": true }));
    let owner = user(&ts.st, &format!("sam-{name}"), "user").i("id");
    Env { ts, lib, staging, owner }
}

async fn login(app: &axum::Router, name: &str) -> String {
    let (code, _, cookie) = call(app, "POST", "/api/auth/login", "", Some(json!({ "username": name, "password": "a-long-password-1" }))).await;
    assert_eq!(code, StatusCode::OK);
    cookie.split(';').next().unwrap_or("").to_string()
}

fn row(st: &AppState, id: i64) -> (String, String, Value) {
    let r = st.db.one("SELECT status, message, detail FROM requests WHERE id=?", p![id]).unwrap().unwrap();
    (r.s("status"), r.s("message"), serde_json::from_str(&r.s("detail")).unwrap_or(json!({})))
}

/// A request of `owner` in `status`, its files kept for the import (`kept`: in its staging folder, Encore's own).
fn request(st: &AppState, owner: i64, status: &str, source: &str, kept: Option<(&Path, Vec<PathBuf>)>) -> i64 {
    let t = now();
    let mut d = json!({ "log": [], "attempts": 1 });
    if let Some((root, files)) = kept {
        d["kept"] = json!({ "files": files, "root": root, "source": source, "how": "request", "own": true, "origin": { "source": "YouTube" },
                            "done": "Downloaded from YouTube", "reason": "storage" });
    }
    st.db.execute("INSERT INTO requests (user_id, kind, artist, title, album, status, source, progress, message, detail, created_at, updated_at)
                   VALUES (?,?,?,?,?,?,?,?,?,?,?,?)",
        p![owner, "album", "Band", "Album", "Album", status, source, 100, "Importing – checking the audio", d.to_string(), t, t]).unwrap()
}

/// The stand-in stuck import: a task that never ends, in the request's place in the queue.
async fn stuck(st: &AppState, id: i64) -> tokio::task::JoinHandle<()> {
    let hold = worker::hold_idle(st, id, Duration::from_secs(1)).await.unwrap();
    worker::take_turn(st, id, hold, std::future::pending::<()>())
}

/// The row as the requests list shows it.
async fn listed(app: &axum::Router, cookie: &str, id: i64) -> Value {
    let (code, list, _) = call(app, "GET", "/api/requests", cookie, None).await;
    assert_eq!(code, StatusCode::OK, "{list}");
    list.as_array().unwrap().iter().find(|r| r["id"] == id).unwrap().clone()
}

/// Until the request's status is none of `busy` (at most 30 s).
async fn settles(st: &AppState, id: i64, busy: &[&str]) -> (String, String, Value) {
    let t0 = std::time::Instant::now();
    loop {
        let r = row(st, id);
        if !busy.contains(&r.0.as_str()) {
            return r;
        }
        assert!(t0.elapsed() < Duration::from_secs(30), "still {}: {}", r.0, r.1);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Fifteen minutes and a second ago, the row's last write.
fn quiet_for_15_minutes(st: &AppState, id: i64) {
    st.db.execute("UPDATE requests SET updated_at=? WHERE id=?", p![now() - stall::STALL_SECS - 1.0, id]).unwrap();
}

/// A stuck import whose kept files went meanwhile: no Import again while it moves, then Import again (its
/// owner or an admin, nobody else) stops the stuck task and runs the import from the kept files at once – no
/// queue runs here, so the request's new state can only come from that – which finds the files gone and says
/// so.
#[tokio::test]
async fn import_again_stops_a_stuck_import_and_imports_at_once() {
    let e = env("gone");
    let st = e.st().clone();
    let app = e.ts.app();
    let root = e.staging.join("request-x");
    let id = request(&st, e.owner, "importing", "ytdlp", Some((&root, vec![root.join("01 First.flac")])));
    let task = stuck(&st, id).await;
    let sam = login(&app, "sam-gone").await;

    // it moves (its row was written just now): no Import again
    let r = listed(&app, &sam, id).await;
    assert_eq!(r["import_stalled"], false, "{r}");
    assert!(r["import_moved_at"].as_f64().unwrap() > now() - 60.0, "{r}");
    let (code, v, _) = call(&app, "POST", &format!("/api/requests/{id}/import-again"), &sam, None).await;
    assert_eq!((code, v["detail"].as_str()), (StatusCode::CONFLICT, Some("It is being imported now")), "{v}");
    assert!(!task.is_finished());

    // nothing moved for 15 minutes: Import again is offered
    quiet_for_15_minutes(&st, id);
    let r = listed(&app, &sam, id).await;
    assert_eq!(r["import_stalled"], true, "{r}");
    user(&st, "stranger-gone", "user");
    let stranger = login(&app, "stranger-gone").await;
    assert_eq!(call(&app, "POST", &format!("/api/requests/{id}/import-again"), &stranger, None).await.0, StatusCode::FORBIDDEN);
    assert!(!task.is_finished());

    let (code, v, _) = call(&app, "POST", &format!("/api/requests/{id}/import-again"), &sam, None).await;
    assert_eq!(code, StatusCode::OK, "{v}");
    assert!(task.await.unwrap_err().is_cancelled(), "the stuck task was stopped");
    let (status, message, d) = settles(&st, id, &["importing", "waiting"]).await;
    assert_eq!(status, "failed", "{message}");
    assert_eq!(message, format!("Import failed: the downloaded files are no longer in {} – request it again to download it again", root.display()));
    let log: Vec<String> = d["log"].as_array().unwrap().iter().filter_map(|l| l.as_str().map(String::from)).collect();
    assert!(log.iter().any(|l| l == "Import again by sam-gone: the import hadn't moved for 15 minutes – it was stopped and started again"), "{log:?}");
    let r = listed(&app, &sam, id).await;
    assert_eq!((r["import_stalled"].clone(), r["import_moved_at"].clone()), (json!(false), Value::Null));
    e.ts.remove();
}

/// An import whose task is gone is stalled at once; one with nothing to import (no kept files, not a download
/// client's) is never offered Import again – the watchdog starts it over instead.
#[tokio::test]
async fn an_import_without_its_task_and_one_without_files() {
    let e = env("task-gone");
    let st = e.st().clone();
    let app = e.ts.app();
    let root = e.staging.join("request-y");
    let kept = request(&st, e.owner, "importing", "ytdlp", Some((&root, vec![root.join("01 First.flac")])));
    let bare = request(&st, e.owner, "importing", "ytdlp", None);
    let sam = login(&app, "sam-task-gone").await;
    assert_eq!(listed(&app, &sam, kept).await["import_stalled"], true, "no task works on it");
    assert_eq!(listed(&app, &sam, bare).await["import_stalled"], false, "nothing to import");
    let (code, v, _) = call(&app, "POST", &format!("/api/requests/{bare}/import-again"), &sam, None).await;
    assert_eq!((code, v["detail"].as_str()), (StatusCode::BAD_REQUEST, Some("There are no downloaded files to import")), "{v}");
    e.ts.remove();
}

/// With ffmpeg here: the kept files of a stuck import are really imported again, and the request completes.
#[tokio::test]
async fn import_again_on_a_stuck_import_completes_it() {
    if !encore_media::ffmpeg::available() {
        return;
    }
    let e = env("real");
    let st = e.st().clone();
    let app = e.ts.app();
    let root = e.staging.join("request-z");
    let mut files = vec![];
    for (i, title) in ["First", "Second"].iter().enumerate() {
        let f = root.join(format!("{:02} {title}.flac", i + 1));
        std::fs::create_dir_all(&root).unwrap();
        let ok = std::process::Command::new("ffmpeg").args(["-v", "error", "-y", "-f", "lavfi", "-i", &format!("sine=frequency={}:duration=3", 300 + i * 110)])
            .args(["-metadata", &format!("title={title}"), "-metadata", "artist=Band", "-metadata", "album_artist=Band", "-metadata", "album=Album",
                   "-metadata", &format!("track={}", i + 1)]).arg(&f).status().map(|s| s.success()).unwrap_or(false);
        if !ok {
            return e.ts.remove();
        }
        files.push(f);
    }
    let id = request(&st, e.owner, "importing", "ytdlp", Some((&root, files.clone())));
    let task = stuck(&st, id).await;
    quiet_for_15_minutes(&st, id);
    let sam = login(&app, "sam-real").await;
    let (code, v, _) = call(&app, "POST", &format!("/api/requests/{id}/import-again"), &sam, None).await;
    assert_eq!(code, StatusCode::OK, "{v}");
    assert!(task.await.unwrap_err().is_cancelled());
    let (status, message, d) = settles(&st, id, &["importing", "waiting"]).await;
    assert_eq!(status, "completed", "{message}");
    assert!(message.starts_with("Downloaded from YouTube"), "{message}");
    assert!(d["kept"].is_null(), "{d}");
    let imported = walkdir::WalkDir::new(&e.lib).into_iter().flatten().filter(|f| f.path().extension().is_some_and(|x| x == "flac")).count();
    assert_eq!(imported, 2);
    e.ts.remove();
}

/// The watchdog: a search and imports whose task is gone start again at its look – an import from its kept
/// files, a Soulseek download's import by looking at the download again, a search from the start with what it
/// asked for kept – while one whose task runs (the stand-in), one whose place is held and the others are left
/// alone. A second look finds nothing.
#[tokio::test]
async fn the_watchdog_starts_again_what_lost_its_task() {
    let e = env("watchdog");
    let st = e.st().clone();
    let root = e.staging.join("request-w");
    let kept = request(&st, e.owner, "importing", "ytdlp", Some((&root, vec![root.join("01 First.flac")])));
    let soulseek = request(&st, e.owner, "importing", "slskd", None);
    let search = request(&st, e.owner, "searching", "ytdlp", None);
    st.db.execute("UPDATE requests SET detail=? WHERE id=?", p![json!({ "isrc": "GBAYE0000001", "sources": ["ytdlp"], "log": ["a line"], "attempts": 2 }).to_string(), search]).unwrap();
    let running = request(&st, e.owner, "importing", "ytdlp", Some((&root, vec![])));
    let task = stuck(&st, running).await;
    let held = request(&st, e.owner, "searching", "ytdlp", None);
    let hold = worker::hold_idle(&st, held, Duration::from_secs(1)).await.unwrap();
    let done = request(&st, e.owner, "completed", "ytdlp", None);

    assert_eq!(worker::watchdog(&st), 3);
    let (status, message, d) = row(&st, kept);
    assert_eq!((status.as_str(), message.as_str()), ("waiting", "Waiting to import the downloaded files"));
    assert_eq!((d["wait"]["reason"].as_str(), d["kept"]["root"].as_str()), (Some("storage_import"), root.to_str()));
    assert!(d["wait"]["next_try_at"].as_f64().unwrap() <= now(), "due now");
    assert_eq!(row(&st, soulseek).0, "downloading");
    assert_eq!(row(&st, soulseek).1, "Importing again");
    let (status, message, d) = row(&st, search);
    assert_eq!((status.as_str(), message.as_str()), ("queued", "Started again – the search had stopped"));
    assert_eq!(d, json!({ "isrc": "GBAYE0000001", "log": ["a line"], "attempts": 2 }), "what was asked for stays; its sources are picked again");
    for (id, s) in [(running, "importing"), (held, "searching"), (done, "completed")] {
        assert_eq!(row(&st, id).0, s);
    }
    assert!(!task.is_finished(), "a running import is left alone");
    let lines: Vec<String> = st.db.query("SELECT message FROM activity WHERE message LIKE '%had stopped without saying where it was%' ORDER BY id", p![]).unwrap()
        .iter().map(|r| r.s("message")).collect();
    assert_eq!(lines.len(), 3, "{lines:?}");
    assert!(lines.contains(&format!("Request #{search} (Band – Album): its search had stopped without saying where it was – started again")), "{lines:?}");
    // a second look: nothing more (the held place keeps the other search; the running import runs)
    assert_eq!(worker::watchdog(&st), 0);
    drop(hold);
    task.abort();
    e.ts.remove();
}
