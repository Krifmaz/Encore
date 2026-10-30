//! Storage problems (encore_requests::worker::storage) on a whole server, with the fault seam of
//! encore_core::fsx making the library's disk fail: a download whose import hits EIO waits with its files
//! kept, counts no attempt and tries no other source, and imports once the disk is back; the health check
//! pauses imports and the API says so; Check again, Import again and Retry; a restart in the middle; a
//! cancelled or deleted request removes only Encore's own staging folder.
use crate::state::AppState;
use crate::test_support::{call, user, TestServer};
use axum::http::StatusCode;
use encore_core::fsx::fault;
use encore_db::{now, p, RowExt};
use encore_requests::worker::{self, storage};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

struct Env {
    ts: TestServer,
    lib: PathBuf,
    staging: PathBuf,
    /// Soulseek's download folder (a download client's folder: Encore never clears it)
    slsk: PathBuf,
    admin: i64,
}

impl Env {
    fn st(&self) -> &AppState {
        &self.ts.st
    }
    fn done(self) {
        fault::clear(&self.lib);
        fault::clear(&self.staging);
        self.ts.remove();
    }
}

/// A server whose library, staging folder and Soulseek folder are its own; no MusicBrainz answers.
fn env(name: &str) -> Env {
    let ts = TestServer::new(&format!("storage-{name}")).with_sources();
    let (lib, staging, slsk) = (ts.dir.join("music"), ts.dir.join("staging"), ts.dir.join("slskd"));
    for d in [&lib, &staging, &slsk] {
        std::fs::create_dir_all(d).unwrap();
    }
    ts.st.cfg.set_many(&json!({ "library_path": lib.to_string_lossy(), "staging_path": staging.to_string_lossy(), "slskd_download_path": slsk.to_string_lossy(),
                               "musicbrainz_url": "http://127.0.0.1:9", "source_order": "slskd,ytdlp", "ytdlp_enabled": true, "auto_approve": true }));
    let admin = user(&ts.st, &format!("admin-{name}"), "admin").i("id");
    Env { ts, lib, staging, slsk, admin }
}

/// A short FLAC with tags (false when there is no ffmpeg here).
fn flac(path: &Path, artist: &str, album: &str, title: &str, track: u32) -> bool {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let freq = 300 + track * 110;
    std::process::Command::new("ffmpeg").args(["-v", "error", "-y", "-f", "lavfi", "-i", &format!("sine=frequency={freq}:duration=3")])
        .args(["-metadata", &format!("title={title}"), "-metadata", &format!("artist={artist}"), "-metadata", &format!("album_artist={artist}"),
               "-metadata", &format!("album={album}"), "-metadata", &format!("track={track}")])
        .arg(path).status().map(|s| s.success()).unwrap_or(false)
}

/// An album downloaded from Soulseek user u1, its two files finished in Soulseek's folder: the request is at
/// the look that imports them (yt-dlp would be its next source). → (id, the files)
fn soulseek_album(e: &Env, artist: &str, album: &str, attempts: u64) -> Option<(i64, Vec<PathBuf>)> {
    let mut files = vec![];
    let mut cand_files = vec![];
    for (i, title) in ["First", "Second"].iter().enumerate() {
        let name = format!("{:02} {title}.flac", i + 1);
        let p = e.slsk.join(album).join(&name);
        if !flac(&p, artist, album, title, i as u32 + 1) {
            return None;
        }
        files.push(p);
        cand_files.push(json!({ "filename": format!("@@u1\\Music\\{album}\\{name}"), "size": 1000 }));
    }
    let cand = json!({ "username": "u1", "directory": format!("@@u1\\Music\\{album}"), "format": "flac", "bitrate": 0, "files": cand_files });
    let t = now();
    let d = json!({ "sources": ["ytdlp"], "attempts": attempts, "log": [], "slskd": { "candidates": [cand], "idx": 0, "started": t, "last_change": t, "last_bytes": 0 } });
    let id = e.st().db.execute("INSERT INTO requests (user_id, kind, artist, title, album, status, source, progress, message, detail, created_at, updated_at)
                               VALUES (?,?,?,?,?,?,?,?,?,?,?,?)",
        p![e.admin, "album", artist, album, album, "downloading", "slskd", 50, "Downloading from u1", d.to_string(), t, t]).unwrap();
    Some((id, files))
}

fn row(st: &AppState, id: i64) -> (String, String, String, Value) {
    let r = st.db.one("SELECT status, message, source, detail FROM requests WHERE id=?", p![id]).unwrap().unwrap();
    (r.s("status"), r.s("message"), r.s("source"), serde_json::from_str(&r.s("detail")).unwrap_or_else(|_| json!({})))
}

/// The next try of a waiting request is due now (its 10 minutes are up).
fn due(st: &AppState, id: i64) {
    st.db.execute("UPDATE requests SET detail=json_set(detail, '$.wait.next_try_at', ?) WHERE id=?", p![now() - 1.0, id]).unwrap();
}

/// The worker's 10-minute look, waited for (the worker's loop starts it and goes on).
async fn tick(st: &AppState) {
    if let Some(h) = storage::tick(st) {
        h.await.unwrap();
    }
}

fn activity(st: &AppState, like: &str) -> Vec<String> {
    st.db.query("SELECT message FROM activity WHERE message LIKE ? ORDER BY id", p![like]).unwrap().iter().map(|r| r.s("message")).collect()
}

fn audio_under(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = walkdir::WalkDir::new(dir).into_iter().flatten().filter(|e| e.file_type().is_file())
        .map(|e| e.into_path()).filter(|p| p.extension().is_some_and(|x| x == "flac")).collect();
    v.sort();
    v
}

async fn login(app: &axum::Router, name: &str) -> String {
    let (code, _, cookie) = call(app, "POST", "/api/auth/login", "", Some(json!({ "username": name, "password": "a-long-password-1" }))).await;
    assert_eq!(code, StatusCode::OK);
    cookie.split(';').next().unwrap_or("").to_string()
}

/// The owner's items 2 and 4: EIO on the move into the library (mkdir of the album's folder, the root itself
/// fine – a mergerfs pool with one bad disk). The request waits ("Waiting: library storage error – import
/// failed: mkdir …: Input/output error (os error 5). Next try at …"), its attempts unchanged, yt-dlp not
/// tried, the files still in Soulseek's folder; nothing changes until its next try; the API shows it; once
/// the disk is back the import alone runs and completes the request as a normal import does.
#[tokio::test]
async fn eio_on_the_move_into_the_library_waits_with_the_files_kept() {
    let e = env("eio");
    let st = e.st().clone();
    let Some((id, files)) = soulseek_album(&e, "Duumu", "Illuminate EP", 3) else { return e.done() };
    let album_dir = e.lib.join("Duumu/Illuminate EP");
    fault::fail(e.lib.join("Duumu"), "mkdir", 5);
    worker::step(st.clone(), id).await;
    let (status, message, source, d) = row(&st, id);
    assert_eq!(status, "waiting", "{message}");
    assert!(message.starts_with(&format!("Waiting: library storage error – import failed: mkdir {}: Input/output error (os error 5). Next try at ", album_dir.display())), "{message}");
    assert_eq!(d["wait"]["reason"], "storage_import");
    assert!(d["wait"]["next_try_at"].as_f64().unwrap() > now() + 590.0, "{d}");
    assert_eq!((d["wait"]["error"]["step"].as_str(), d["wait"]["error"]["op"].as_str(), d["wait"]["error"]["errno"].as_i64()), (Some("import"), Some("mkdir"), Some(5)));
    assert_eq!(d["attempts"], 3, "a storage wait counts no attempt");
    assert_eq!(source, "slskd");
    assert_eq!(d["sources"], json!(["ytdlp"]), "no other source tried");
    assert!(files.iter().all(|f| f.is_file()), "the files stay in Soulseek's folder");
    assert!(audio_under(&e.lib).is_empty() && !album_dir.exists());
    assert_eq!(d["kept"]["own"], false);
    let line = activity(&st, "%waits for the storage%");
    assert_eq!(line.len(), 1);
    assert!(line[0].contains(&format!("Request #{id} (Duumu – Illuminate EP) waits for the storage: import failed: mkdir {}: Input/output error (os error 5) – storage problem, not the source", album_dir.display())), "{line:?}");
    assert_eq!(activity(&st, "storage: % not writable%").len(), 1, "imports paused, said once");

    // the requests list and the storage state as the clients read them
    let app = e.ts.app();
    let cookie = login(&app, "admin-eio").await;
    let (code, list, _) = call(&app, "GET", "/api/requests?status=active", &cookie, None).await;
    assert_eq!(code, StatusCode::OK);
    let r = list.as_array().unwrap().iter().find(|r| r["id"] == id).expect("Active includes waiting").clone();
    assert_eq!((r["status"].as_str(), r["wait_reason"].as_str(), r["has_files"].as_bool()), (Some("waiting"), Some("storage_import"), Some(true)), "{r}");
    assert!(r["next_try_at"].as_f64().is_some(), "{r}");
    assert_eq!(r["kept_path"], e.slsk.to_string_lossy().as_ref());
    assert_eq!((r["error"]["op"].as_str(), r["error"]["errno"].as_i64(), r["error"]["storage"].as_bool()), (Some("mkdir"), Some(5), Some(true)), "{r}");
    assert_eq!(r["error"]["path"], album_dir.to_string_lossy().as_ref());
    assert_eq!(r["attempts"], 3);
    let (_, s, _) = call(&app, "GET", "/api/requests/storage", &cookie, None).await;
    assert_eq!((s["ok"].as_bool(), s["op"].as_str(), s["errno"].as_i64(), s["waiting"].as_i64()), (Some(false), Some("mkdir"), Some(5), Some(1)), "{s}");
    assert_eq!(s["path"], album_dir.to_string_lossy().as_ref());
    assert!(s["text"].as_str().unwrap().starts_with(&format!("Imports are paused: {} is not writable – mkdir {}: Input/output error (os error 5). Checked ", album_dir.display(), album_dir.display())), "{s}");
    user(&st, "plain-eio", "user");
    let plain = login(&app, "plain-eio").await;
    let (_, s, _) = call(&app, "GET", "/api/requests/storage", &plain, None).await;
    assert_eq!(s, json!({ "ok": false, "since": s["since"], "text": storage::PLAIN_BANNER, "downloads": { "ok": true } }));

    // nothing happens before its next try
    let at = st.db.one("SELECT updated_at FROM requests WHERE id=?", p![id]).unwrap().unwrap().f("updated_at");
    worker::step(st.clone(), id).await;
    assert_eq!(st.db.one("SELECT updated_at FROM requests WHERE id=?", p![id]).unwrap().unwrap().f("updated_at"), at);
    // 10 minutes later, the disk still bad: the import alone is tried again, and waits again
    due(&st, id);
    worker::step(st.clone(), id).await;
    let (status, _, _, d) = row(&st, id);
    assert_eq!(status, "waiting");
    assert_eq!(d["attempts"], 3);
    assert!(files.iter().all(|f| f.is_file()));
    assert!(!album_dir.exists(), "the health check's probe of the album's folder leaves nothing behind");

    // the disk is back: the next try imports and completes it like any import
    fault::clear(e.lib.join("Duumu"));
    due(&st, id);
    worker::step(st.clone(), id).await;
    let (status, message, _, d) = row(&st, id);
    assert_eq!(status, "completed", "{message} {d}");
    assert_eq!(message, "Downloaded from Soulseek (FLAC)");
    assert_eq!(d["attempts"], 3);
    assert!(d["kept"].is_null() && d["wait"].is_null(), "{d}");
    let imported = audio_under(&album_dir);
    assert_eq!(imported.len(), 2, "{imported:?}");
    assert!(files.iter().all(|f| !f.exists()), "moved, not copied");
    assert_eq!(st.db.one("SELECT COUNT(*) n FROM tracks WHERE request_id=?", p![id]).unwrap().unwrap().i("n"), 2, "indexed with their origin");
    assert_eq!(activity(&st, "storage: % writable again; % resumed").len(), 1);
    assert_eq!(activity(&st, "Request completed: Duumu – Illuminate EP%").len(), 1);
    let (_, s, _) = call(&app, "GET", "/api/requests/storage", &cookie, None).await;
    assert_eq!((s["ok"].as_bool(), s["text"].clone()), (Some(true), Value::Null), "{s}");
    e.done();
}

/// A disk that fails part way through an album (the second file's copy finds the disk full): nothing of the
/// album stays in the library half done, both files stay in the download folder, and the next try imports
/// the whole album.
#[tokio::test]
async fn a_failure_part_way_through_an_album_leaves_nothing_half_done() {
    let e = env("partial");
    let st = e.st().clone();
    let Some((id, files)) = soulseek_album(&e, "Duumu", "Illuminate EP", 1) else { return e.done() };
    let second = e.lib.join("Duumu/Illuminate EP/02 - Second.flac.encore-part");
    for op in ["link", "copy"] {
        fault::fail(&second, op, 28);
    }
    worker::step(st.clone(), id).await;
    let (status, message, _, d) = row(&st, id);
    assert_eq!(status, "waiting", "{message}");
    assert!(message.contains("No space left on device (os error 28)"), "{message}");
    assert!(message.starts_with("Waiting: library storage error – import failed: copy "), "the library's disk, not the download folder: {message}");
    assert_eq!(d["wait"]["error"]["op"], "copy", "{d}");
    let album = e.lib.join("Duumu/Illuminate EP");
    assert_eq!(d["wait"]["dir"], album.to_string_lossy().as_ref(), "{d}");
    assert_eq!(storage::status(&st, true)["path"], album.to_string_lossy().as_ref(), "the banner names the library's folder");
    assert!(audio_under(&e.lib).is_empty(), "the first file went again: {:?}", audio_under(&e.lib));
    assert!(walkdir::WalkDir::new(&e.lib).into_iter().flatten().all(|x| !x.file_name().to_string_lossy().ends_with(".encore-part")));
    assert!(files.iter().all(|f| f.is_file()), "both files are still in the download folder");
    fault::clear(&second);
    due(&st, id);
    worker::step(st.clone(), id).await;
    let (status, message, ..) = row(&st, id);
    assert_eq!(status, "completed", "{message}");
    assert_eq!(audio_under(&e.lib.join("Duumu/Illuminate EP")).len(), 2);
    assert!(files.iter().all(|f| !f.exists()));
    e.done();
}

/// The owner's item 3: the health check before an import (a probe folder made and removed in the library)
/// fails – imports are paused, every request reaching its import waits, the API reports it; Check again
/// while it still fails changes nothing, and once it passes the waiting imports resume and complete.
#[tokio::test]
async fn a_failing_health_check_pauses_imports_until_check_again_passes() {
    let e = env("health");
    let st = e.st().clone();
    let Some((a, _)) = soulseek_album(&e, "Band A", "First Album", 1) else { return e.done() };
    let Some((b, _)) = soulseek_album(&e, "Band B", "Second Album", 1) else { return e.done() };
    fault::fail(&e.lib, "mkdir", 30);
    worker::step(st.clone(), a).await;
    worker::step(st.clone(), b).await;
    for id in [a, b] {
        let (status, message, _, d) = row(&st, id);
        assert_eq!(status, "waiting", "{message}");
        assert!(message.starts_with(&format!("Waiting: library storage error – import failed: mkdir {}/.encore-probe-", e.lib.display())), "{message}");
        assert!(message.contains("Read-only file system (os error 30). Next try at "), "{message}");
        assert_eq!(d["attempts"], 1);
    }
    assert!(audio_under(&e.lib).is_empty());
    let app = e.ts.app();
    let cookie = login(&app, "admin-health").await;
    let (_, s, _) = call(&app, "GET", "/api/requests/storage", &cookie, None).await;
    assert_eq!((s["ok"].as_bool(), s["path"].as_str(), s["op"].as_str(), s["waiting"].as_i64()), (Some(false), Some(e.lib.to_str().unwrap()), Some("mkdir"), Some(2)), "{s}");
    assert!(s["next_check_at"].as_f64().unwrap() > s["checked_at"].as_f64().unwrap() + 590.0, "{s}");
    let paused = activity(&st, "storage: % not writable%");
    assert_eq!(paused.len(), 1, "{paused:?}");
    assert!(paused[0].starts_with(&format!("storage: {} not writable – mkdir {}/.encore-probe-", e.lib.display(), e.lib.display())) && paused[0].ends_with("Read-only file system (os error 30); imports paused"), "{paused:?}");
    // Check again: admins only, and with the CSRF header
    user(&st, "plain-health", "user");
    let plain = login(&app, "plain-health").await;
    assert_eq!(call(&app, "POST", "/api/requests/storage/check", &plain, None).await.0, StatusCode::FORBIDDEN);
    let (code, s, _) = call(&app, "POST", "/api/requests/storage/check", &cookie, None).await;
    assert_eq!((code, s["ok"].as_bool()), (StatusCode::OK, Some(false)), "{s}");
    // the folder is writable again: Check again resumes both at once
    fault::clear(&e.lib);
    let (_, s, _) = call(&app, "POST", "/api/requests/storage/check", &cookie, None).await;
    assert_eq!(s["ok"], true, "{s}");
    let resumed = activity(&st, "storage: % writable again%");
    assert_eq!(resumed, vec![format!("storage: {} writable again; 2 imports resumed", e.lib.display())]);
    for id in [a, b] {
        let (_, _, _, d) = row(&st, id);
        assert!(d["wait"]["next_try_at"].as_f64().unwrap() <= now(), "{d}");
        worker::step(st.clone(), id).await;
        let (status, message, _, d) = row(&st, id);
        assert_eq!(status, "completed", "{message}");
        assert_eq!(d["attempts"], 1);
    }
    assert_eq!(audio_under(&e.lib).len(), 4);
    e.done();
}

/// The health check every 10 minutes, and its state across a restart: a server that starts while imports
/// are paused still says so, and its next look at the folders resumes them.
#[tokio::test]
async fn the_paused_state_survives_a_restart_and_is_checked_again() {
    let e = env("restart-health");
    let st = e.st().clone();
    let Some((id, _)) = soulseek_album(&e, "Band", "Album", 1) else { return e.done() };
    fault::fail(&e.lib, "mkdir", 5);
    worker::step(st.clone(), id).await;
    assert_eq!(row(&st, id).0, "waiting");
    // the server restarts (its state comes from the database): still paused
    let ts2 = TestServer::in_dir(&e.ts.dir);
    let st2 = ts2.st.clone();
    assert_eq!(storage::status(&st2, true)["ok"], false);
    // its 10 minutes are not up: the worker's look changes nothing yet
    fault::clear(&e.lib);
    tick(&st2).await;
    assert_eq!(storage::status(&st2, true)["ok"], false);
    // they are up (the state row says so): the next look passes and resumes the import
    let v: Value = serde_json::from_str(&st2.db.one("SELECT value FROM settings WHERE key='storage_health'", p![]).unwrap().unwrap().s("value")).unwrap();
    let mut v = v;
    v["next_check_at"] = json!(now() - 1.0);
    st2.db.execute("UPDATE settings SET value=? WHERE key='storage_health'", p![v.to_string()]).unwrap();
    let ts3 = TestServer::in_dir(&e.ts.dir);
    let st3 = ts3.st.clone();
    tick(&st3).await;
    assert_eq!(storage::status(&st3, true)["ok"], true);
    assert!(row(&st3, id).3["wait"]["next_try_at"].as_f64().unwrap() <= now());
    worker::step(st3.clone(), id).await;
    assert_eq!(row(&st3, id).0, "completed");
    e.done();
}

/// A restart in the middle of an import: the request finds its kept files again and imports them (nothing
/// downloaded again), and a request that failed before this version with its download still in its
/// staging folder ("C U Next Tuesday") is rescued the same way.
#[tokio::test]
async fn a_restart_in_the_middle_imports_the_kept_files() {
    let e = env("restart");
    let st = e.st().clone();
    // mid-import when the server stopped (the kept files are recorded before an import starts)
    let Some((id, files)) = soulseek_album(&e, "Band", "Album", 2) else { return e.done() };
    let kept = json!({ "files": files, "root": e.slsk, "source": "slskd", "how": "request", "own": false, "origin": { "source": "Soulseek" },
                       "done": "Downloaded from Soulseek (FLAC)", "reason": "storage" });
    st.db.execute("UPDATE requests SET status='importing', detail=json_set(detail, '$.kept', json(?)) WHERE id=?", p![kept.to_string(), id]).unwrap();
    // failed before this version, its song left in its staging folder
    let t = now();
    let old = st.db.execute("INSERT INTO requests (user_id, kind, artist, title, status, source, message, detail, created_at, updated_at) VALUES (?,?,?,?,?,?,?,?,?,?)",
        p![e.admin, "track", "Pat", "C U Next Tuesday", "failed", "ytdlp", "All sources exhausted (ytdlp: error: Read-only file system (os error 30))", r#"{"attempts": 2}"#, t, t]).unwrap();
    let song = e.staging.join(format!("request-{old}/Pat - C U Next Tuesday.flac"));
    if !flac(&song, "Pat", "", "C U Next Tuesday", 1) {
        return e.done();
    }
    // a yt-dlp song whose import was running when the server stopped (recorded by a version that left it
    // `downloading`): its kept file is imported, not downloaded again
    let song2 = e.staging.join("request-song/Pat - Other Song.flac");
    if !flac(&song2, "Pat", "", "Other Song", 1) {
        return e.done();
    }
    let kept2 = json!({ "files": [song2], "root": song2.parent().unwrap(), "source": "ytdlp", "how": "request", "own": true, "origin": { "source": "YouTube" },
                        "done": "Downloaded from YouTube", "reason": "storage" });
    let yt = st.db.execute("INSERT INTO requests (user_id, kind, artist, title, status, source, message, detail, created_at, updated_at) VALUES (?,?,?,?,?,?,?,?,?,?)",
        p![e.admin, "track", "Pat", "Other Song", "downloading", "ytdlp", "Downloading", json!({ "attempts": 1, "kept": kept2 }).to_string(), t, t]).unwrap();
    // a failed request with nothing left is left alone
    let gone = st.db.execute("INSERT INTO requests (user_id, kind, artist, title, status, message, detail, created_at, updated_at) VALUES (?,?,?,?,?,?,?,?,?)",
        p![e.admin, "track", "Pat", "Other", "failed", "All sources exhausted", "{}", t, t]).unwrap();
    worker::recover(&st);
    for rid in [id, old] {
        let (status, message, _, d) = row(&st, rid);
        assert_eq!((status.as_str(), d["wait"]["reason"].as_str()), ("waiting", Some("storage_import")), "{message} {d}");
        assert!(d["wait"]["next_try_at"].as_f64().unwrap() <= now());
    }
    assert_eq!(row(&st, gone).0, "failed");
    assert_eq!(activity(&st, "%its downloaded files are still there%").len(), 2);
    worker::step(st.clone(), id).await;
    let (status, message, _, d) = row(&st, id);
    assert_eq!(status, "completed", "{message}");
    assert_eq!(d["attempts"], 2);
    worker::step(st.clone(), old).await;
    let (status, message, _, d) = row(&st, old);
    assert_eq!(status, "completed", "{message} {d}");
    assert_eq!(d["attempts"], 2, "the rescue counts no attempt");
    assert!(!e.staging.join(format!("request-{old}")).exists(), "Encore's own staging folder goes once the files are in");
    let (status, message, _, d) = row(&st, yt);
    assert_eq!(status, "queued", "{message}");
    assert!(d["kept"].is_object(), "the restart keeps what is kept: {d}");
    worker::step(st.clone(), yt).await;
    let (status, message, _, d) = row(&st, yt);
    assert_eq!(status, "completed", "{message} {d}");
    assert_eq!(d["attempts"], 1);
    assert_eq!(audio_under(&e.lib).len(), 4);
    e.done();
}

/// A file step that fails for a reason other than the storage's (a permission): the request fails at once
/// with the step, the path and what to do, keeps its files and tries no other source; Retry and Import again
/// import them – nothing downloaded again, no attempt counted.
#[tokio::test]
async fn a_permission_error_fails_with_the_files_kept_and_retry_imports_them() {
    let e = env("perm");
    let st = e.st().clone();
    let Some((id, files)) = soulseek_album(&e, "Band", "Album", 1) else { return e.done() };
    fault::fail(e.lib.join("Band"), "mkdir", 13);
    worker::step(st.clone(), id).await;
    let (status, message, _, d) = row(&st, id);
    assert_eq!(status, "failed");
    assert_eq!(message, format!("import failed: mkdir {}: Permission denied (os error 13) – Encore may not write there (check the folder's owner, PUID/PGID)", e.lib.join("Band/Album").display()));
    assert_eq!(d["sources"], json!(["ytdlp"]), "no other source tried");
    assert_eq!(d["kept"]["reason"], "error");
    assert!(files.iter().all(|f| f.is_file()));
    let app = e.ts.app();
    let cookie = login(&app, "admin-perm").await;
    let (_, list, _) = call(&app, "GET", "/api/requests", &cookie, None).await;
    let r = list.as_array().unwrap().iter().find(|r| r["id"] == id).unwrap().clone();
    assert_eq!((r["has_files"].as_bool(), r["error"]["errno"].as_i64(), r["wait_reason"].clone()), (Some(true), Some(13), Value::Null), "{r}");
    // a start-up leaves it alone (Import again is someone's choice)
    worker::recover(&st);
    assert_eq!(row(&st, id).0, "failed");
    // Retry: the import again, not a download
    fault::clear(e.lib.join("Band"));
    let (code, body, _) = call(&app, "POST", &format!("/api/requests/{id}/retry"), &cookie, None).await;
    assert_eq!(code, StatusCode::OK, "{body}");
    let (status, message, source, d) = row(&st, id);
    assert_eq!((status.as_str(), message.as_str(), source.as_str()), ("waiting", "Waiting to import the downloaded files", "slskd"));
    assert_eq!(d["attempts"], 1);
    worker::step(st.clone(), id).await;
    let (status, message, _, d) = row(&st, id);
    assert_eq!(status, "completed", "{message}");
    assert_eq!(d["attempts"], 1);
    // Import again: the request's owner or an admin, and only while files are kept
    let (code, body, _) = call(&app, "POST", &format!("/api/requests/{id}/import-again"), &cookie, None).await;
    assert_eq!(code, StatusCode::BAD_REQUEST, "{body}");
    let Some((other, _)) = soulseek_album(&e, "Band", "Other Album", 1) else { return e.done() };
    fault::fail(e.lib.join("Band/Other Album"), "mkdir", 13);
    worker::step(st.clone(), other).await;
    assert_eq!(row(&st, other).0, "failed");
    fault::clear(e.lib.join("Band/Other Album"));
    user(&st, "stranger-perm", "user");
    let stranger = login(&app, "stranger-perm").await;
    assert_eq!(call(&app, "POST", &format!("/api/requests/{other}/import-again"), &stranger, None).await.0, StatusCode::FORBIDDEN);
    let (code, body, _) = call(&app, "POST", &format!("/api/requests/{other}/import-again"), &cookie, None).await;
    assert_eq!((code, body["status"].as_str()), (StatusCode::OK, Some("waiting")), "{body}");
    worker::step(st.clone(), other).await;
    assert_eq!(row(&st, other).0, "completed");
    e.done();
}

/// A link whose file can't go into the library is a failure that says why (never "Downloaded 0 file(s)",
/// shown as Available); its file is kept for Import again.
#[tokio::test]
async fn a_link_that_imported_nothing_fails_with_the_reason() {
    let e = env("link");
    let st = e.st().clone();
    let t = now();
    let id = st.db.execute("INSERT INTO requests (user_id, kind, url, status, source, message, detail, created_at, updated_at) VALUES (?,?,?,?,?,?,?,?,?)",
        p![e.admin, "url", "https://www.youtube.com/watch?v=x", "failed", "ytdlp", "Downloaded 0 file(s)", "{}", t, t]).unwrap();
    let file = e.staging.join(format!("request-{id}/Some Artist - Some Song.flac"));
    if !flac(&file, "Some Artist", "", "Some Song", 1) {
        return e.done();
    }
    fault::fail(e.lib.join("Some Artist"), "mkdir", 13);
    worker::recover(&st);
    worker::step(st.clone(), id).await;
    let (status, message, _, d) = row(&st, id);
    assert_eq!(status, "failed");
    assert!(message.starts_with(&format!("Downloaded, but nothing could be imported: import failed: mkdir {}", e.lib.join("Some Artist").display())), "{message}");
    assert!(file.is_file(), "kept for Import again");
    assert_eq!(d["kept"]["how"], "url");
    fault::clear(e.lib.join("Some Artist"));
    storage::import_again(&st, id).unwrap();
    worker::step(st.clone(), id).await;
    let (status, message, ..) = row(&st, id);
    assert_eq!((status.as_str(), message.as_str()), ("completed", "Downloaded 1 file(s)"));
    e.done();
}

/// The Oct 17 case: the staging folder is read-only before any file exists. The source step waits for the
/// storage (`storage_download`) instead of failing every request, counts no attempt, and runs again once the
/// folder is writable.
#[tokio::test]
async fn a_read_only_staging_folder_waits_and_runs_the_step_again() {
    let e = env("erofs");
    let st = e.st().clone();
    let t = now();
    let id = st.db.execute("INSERT INTO requests (user_id, kind, url, status, message, detail, created_at, updated_at) VALUES (?,?,?,?,?,?,?,?)",
        p![e.admin, "url", "https://www.youtube.com/watch?v=y", "queued", "", r#"{"attempts": 4}"#, t, t]).unwrap();
    fault::fail(&e.staging, "mkdir", 30);
    worker::step(st.clone(), id).await;
    let (status, message, _, d) = row(&st, id);
    assert_eq!((status.as_str(), d["wait"]["reason"].as_str()), ("waiting", Some("storage_download")), "{message}");
    assert!(message.starts_with(&format!("Waiting: storage error – staging failed: mkdir {}: Read-only file system (os error 30). Next try at ", e.staging.join(format!("request-{id}")).display())), "{message}");
    assert_eq!(d["attempts"], 4);
    // still read-only at its next try: it waits again (the health check sees it)
    due(&st, id);
    worker::step(st.clone(), id).await;
    assert_eq!(row(&st, id).0, "waiting");
    // writable: the step runs again (here yt-dlp itself isn't installed, so it then fails for that reason)
    fault::clear(&e.staging);
    due(&st, id);
    worker::step(st.clone(), id).await;
    let (status, message, _, d) = row(&st, id);
    assert_ne!(status, "waiting", "{message}");
    assert!(!message.contains("Read-only"), "{message}");
    assert_eq!(d["attempts"], 4);
    e.done();
}

/// A cancelled or deleted request removes Encore's own staging folder only: a download client's folder
/// (Soulseek's here) keeps its files.
#[tokio::test]
async fn cancel_and_delete_remove_only_encores_own_staging() {
    let e = env("cancel");
    let st = e.st().clone();
    let Some((id, files)) = soulseek_album(&e, "Band", "Album", 1) else { return e.done() };
    fault::fail(e.lib.join("Band"), "mkdir", 5);
    worker::step(st.clone(), id).await;
    assert_eq!(row(&st, id).0, "waiting");
    let own = e.staging.join(format!("request-{id}"));
    std::fs::create_dir_all(own.join("streamrip")).unwrap();
    std::fs::write(own.join("streamrip/leftover.flac"), b"x").unwrap();
    let app = e.ts.app();
    let cookie = login(&app, "admin-cancel").await;
    let (code, _, _) = call(&app, "POST", &format!("/api/requests/{id}/cancel"), &cookie, None).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(row(&st, id).0, "cancelled");
    assert!(!own.exists(), "Encore's staging folder of the request is gone");
    assert!(files.iter().all(|f| f.is_file()), "Soulseek's folder is never cleared");
    // the folder still fails, but no request waits for it any more: Check again lifts the pause (it doesn't
    // hold up every other import for good)
    let (_, s, _) = call(&app, "POST", "/api/requests/storage/check", &cookie, None).await;
    assert_eq!(s["ok"], true, "{s}");
    // deleted from the list (without its files): the same
    let Some((id2, files2)) = soulseek_album(&e, "Band", "Second", 1) else { return e.done() };
    worker::step(st.clone(), id2).await;
    assert_eq!(row(&st, id2).0, "waiting");
    let own2 = e.staging.join(format!("request-{id2}"));
    std::fs::create_dir_all(&own2).unwrap();
    std::fs::write(own2.join("x.flac"), b"x").unwrap();
    let (code, _, _) = call(&app, "DELETE", &format!("/api/requests/{id2}"), &cookie, None).await;
    assert_eq!(code, StatusCode::OK);
    assert!(!own2.exists());
    assert!(files2.iter().all(|f| f.is_file()));
    e.done();
}

/// Kept files that can't be looked at (a network or FUSE mount that went away: ENOTCONN) are not files that
/// are gone: the request keeps waiting with them, no other source is tried, Import again stays offered, and
/// they import once the mount is back.
#[tokio::test]
async fn kept_files_that_cannot_be_looked_at_are_not_gone() {
    let e = env("enotconn");
    let st = e.st().clone();
    let Some((id, files)) = soulseek_album(&e, "Band", "Album", 2) else { return e.done() };
    fault::fail(e.lib.join("Band"), "mkdir", 5);
    worker::step(st.clone(), id).await;
    assert_eq!(row(&st, id).0, "waiting");
    fault::clear(e.lib.join("Band"));
    // Soulseek's folder went away
    fault::fail(&e.slsk, "stat", 107);
    fault::fail(&e.slsk, "read", 107);
    due(&st, id);
    worker::step(st.clone(), id).await;
    let (status, message, source, d) = row(&st, id);
    assert_eq!(status, "waiting", "{message}");
    assert_eq!(d["kept"]["files"].as_array().map(|a| a.len()), Some(2), "{d}");
    assert_eq!((source.as_str(), d["sources"].clone(), d["attempts"].clone()), ("slskd", json!(["ytdlp"]), json!(2)), "no other source, no attempt");
    assert_eq!((d["wait"]["error"]["op"].as_str(), d["wait"]["error"]["errno"].as_i64()), (Some("stat"), Some(107)), "{d}");
    assert!(message.contains("Transport endpoint is not connected (os error 107)"), "{message}");
    let app = e.ts.app();
    let cookie = login(&app, "admin-enotconn").await;
    let (_, list, _) = call(&app, "GET", "/api/requests", &cookie, None).await;
    let r = list.as_array().unwrap().iter().find(|r| r["id"] == id).unwrap().clone();
    assert_eq!(r["has_files"], true, "Import again stays offered: {r}");
    // the mount is back: the next try imports them
    fault::clear(&e.slsk);
    due(&st, id);
    worker::step(st.clone(), id).await;
    let (status, message, ..) = row(&st, id);
    assert_eq!(status, "completed", "{message}");
    assert!(files.iter().all(|f| !f.exists()));
    e.done();
}

/// A disk that fails on a write the probe can't see (the copy into the library; making folders works): a
/// waiting import's own passing check doesn't lift the pause or wake the other waiting imports (no retry
/// storm), each is tried on its own 10-minute turn, and the pause is said once.
#[tokio::test]
async fn a_passing_check_before_an_import_does_not_wake_every_waiting_import() {
    let e = env("storm");
    let st = e.st().clone();
    let Some((a, _)) = soulseek_album(&e, "Band A", "First", 1) else { return e.done() };
    let Some((b, _)) = soulseek_album(&e, "Band B", "Second", 1) else { return e.done() };
    for op in ["link", "copy"] {
        fault::fail(&e.lib, op, 5);
    }
    worker::step(st.clone(), a).await;
    worker::step(st.clone(), b).await;
    assert_eq!((row(&st, a).0.as_str(), row(&st, b).0.as_str()), ("waiting", "waiting"));
    let b_next = row(&st, b).3["wait"]["next_try_at"].as_f64().unwrap();
    due(&st, a);
    worker::step(st.clone(), a).await;
    let (status, message, _, d) = row(&st, a);
    assert_eq!(status, "waiting", "{message}");
    assert!(d["wait"]["next_try_at"].as_f64().unwrap() > now() + 590.0, "{d}");
    assert_eq!(row(&st, b).3["wait"]["next_try_at"].as_f64().unwrap(), b_next, "b is not woken by a's check");
    assert!(activity(&st, "storage: % writable again%").is_empty(), "a check that can't see the problem doesn't lift the pause");
    assert_eq!(activity(&st, "storage: % not writable%").len(), 1, "the pause is said once");
    assert_eq!(storage::status(&st, true)["ok"], false);
    e.done();
}

/// A permission problem is not a storage problem: with the artist's folder there but not writable for
/// Encore (EACCES on the probe in it), imports are not paused – the import fails at once with the folder's
/// owner hint, its files kept for Import again.
#[tokio::test]
async fn a_folder_encore_may_not_write_to_fails_the_import_without_pausing_imports() {
    let e = env("eacces");
    let st = e.st().clone();
    let Some((id, files)) = soulseek_album(&e, "Band", "Album", 1) else { return e.done() };
    std::fs::create_dir_all(e.lib.join("Band")).unwrap();
    fault::fail(e.lib.join("Band"), "mkdir", 13);
    worker::step(st.clone(), id).await;
    let (status, message, _, d) = row(&st, id);
    assert_eq!(status, "failed", "{message}");
    assert!(message.ends_with("Permission denied (os error 13) – Encore may not write there (check the folder's owner, PUID/PGID)"), "{message}");
    assert_eq!(d["kept"]["reason"], "error");
    assert!(files.iter().all(|f| f.is_file()));
    assert_eq!(storage::status(&st, true)["ok"], true, "imports are not paused");
    assert!(activity(&st, "storage: % not writable%").is_empty());
    let app = e.ts.app();
    let cookie = login(&app, "admin-eacces").await;
    let (_, list, _) = call(&app, "GET", "/api/requests", &cookie, None).await;
    let r = list.as_array().unwrap().iter().find(|r| r["id"] == id).unwrap().clone();
    assert_eq!((r["has_files"].as_bool(), r["wait_reason"].clone()), (Some(true), Value::Null), "{r}");
    fault::clear(e.lib.join("Band"));
    e.done();
}

/// Other users than admins see no server folders in their own rows, as in the storage banner: the wait in
/// plain words, no kept folder and no output, the error as {storage, text}, the folders left out of the log.
#[tokio::test]
async fn members_see_no_server_folders_in_their_rows() {
    let e = env("member");
    let st = e.st().clone();
    let Some((id, _)) = soulseek_album(&e, "Duumu", "Illuminate EP", 1) else { return e.done() };
    let member = user(&st, "member-rows", "user").i("id");
    let output = json!({ "source": "streamrip", "url": "u", "text": format!("wrote {}/x", e.staging.display()), "ok": true });
    st.db.execute("UPDATE requests SET user_id=?, detail=json_set(detail, '$.output', json(?)) WHERE id=?", p![member, output.to_string(), id]).unwrap();
    fault::fail(e.lib.join("Duumu"), "mkdir", 5);
    worker::step(st.clone(), id).await;
    assert_eq!(row(&st, id).0, "waiting");
    let app = e.ts.app();
    let cookie = login(&app, "member-rows").await;
    let (_, list, _) = call(&app, "GET", "/api/requests", &cookie, None).await;
    let r = list.as_array().unwrap().iter().find(|r| r["id"] == id).unwrap().clone();
    let all = r.to_string();
    for dir in [&e.lib, &e.staging, &e.slsk] {
        assert!(!all.contains(dir.to_str().unwrap()), "{dir:?} in {r}");
    }
    assert!(r["message"].as_str().unwrap().starts_with("Waiting: a storage problem on the server – your download is kept and will be added when it is fixed. Next try at "), "{r}");
    assert_eq!(r["error"], json!({ "storage": true, "text": "A storage problem on the server – not the source" }));
    assert_eq!((r["kept_path"].clone(), r["output"].clone(), r["has_files"].clone()), (Value::Null, Value::Null, json!(true)), "{r}");
    assert!(r["log"].to_string().contains("mkdir …/Duumu/Illuminate EP"), "{r}");
    // an admin reads it all
    let admin = login(&app, "admin-member").await;
    let (_, list, _) = call(&app, "GET", "/api/requests", &admin, None).await;
    let r = list.as_array().unwrap().iter().find(|r| r["id"] == id).unwrap().clone();
    assert_eq!(r["kept_path"], e.slsk.to_string_lossy().as_ref());
    assert!(r["message"].as_str().unwrap().starts_with("Waiting: library storage error – import failed: mkdir "), "{r}");
    fault::clear(e.lib.join("Duumu"));
    e.done();
}

/// Retry on a playlist: a song whose downloaded files are kept imports them (nothing downloaded again); the
/// others start over with their attempt counted.
#[tokio::test]
async fn retry_on_a_playlist_imports_the_songs_whose_files_are_kept() {
    let e = env("playlist");
    let st = e.st().clone();
    let t = now();
    let pl = st.db.execute("INSERT INTO requests (user_id, kind, title, status, message, detail, created_at, updated_at) VALUES (?,?,?,?,?,?,?,?)",
        p![e.admin, "playlist", "Mix", "completed", "", "{}", t, t]).unwrap();
    let child = |title: &str| st.db.execute("INSERT INTO requests (user_id, kind, artist, title, status, source, message, detail, parent_id, created_at, updated_at) VALUES (?,?,?,?,?,?,?,?,?,?,?)",
        p![e.admin, "track", "Pat", title, "failed", "ytdlp", "import failed", r#"{"attempts": 1}"#, pl, t, t]).unwrap();
    let (kept, plain) = (child("Song"), child("Other"));
    let file = e.staging.join(format!("request-{kept}/Pat - Song.flac"));
    if !flac(&file, "Pat", "", "Song", 1) {
        return e.done();
    }
    let k = json!({ "files": [file], "root": file.parent().unwrap(), "source": "ytdlp", "how": "request", "own": true, "origin": { "source": "YouTube" },
                    "done": "Downloaded from YouTube", "reason": "error" });
    st.db.execute("UPDATE requests SET detail=json_set(detail, '$.kept', json(?)) WHERE id=?", p![k.to_string(), kept]).unwrap();
    let app = e.ts.app();
    let cookie = login(&app, "admin-playlist").await;
    let (code, body, _) = call(&app, "POST", &format!("/api/requests/{pl}/retry"), &cookie, None).await;
    assert_eq!(code, StatusCode::OK, "{body}");
    let (status, message, _, d) = row(&st, kept);
    assert_eq!((status.as_str(), message.as_str()), ("waiting", "Waiting to import the downloaded files"), "{d}");
    assert_eq!(d["attempts"], 1);
    assert!(file.is_file());
    let (status, _, _, d) = row(&st, plain);
    assert_eq!((status.as_str(), d["attempts"].as_i64()), ("queued", Some(2)), "{d}");
    worker::step(st.clone(), kept).await;
    assert_eq!(row(&st, kept).0, "completed");
    e.done();
}

/// Remove artist stops the artist's waiting requests too (a waiting import would bring the artist back once
/// the disk is fixed); only Encore's own staging folder goes.
#[tokio::test]
async fn remove_artist_cancels_its_waiting_requests() {
    let e = env("remove-artist");
    let st = e.st().clone();
    let Some((id, files)) = soulseek_album(&e, "Band", "Album", 1) else { return e.done() };
    fault::fail(e.lib.join("Band"), "mkdir", 5);
    worker::step(st.clone(), id).await;
    assert_eq!(row(&st, id).0, "waiting");
    let own = e.staging.join(format!("request-{id}"));
    std::fs::create_dir_all(&own).unwrap();
    std::fs::write(own.join("leftover.flac"), b"x").unwrap();
    let app = e.ts.app();
    let cookie = login(&app, "admin-remove-artist").await;
    let (code, body, _) = call(&app, "DELETE", "/api/library/artists?name=Band", &cookie, None).await;
    assert_eq!(code, StatusCode::OK, "{body}");
    let (status, message, ..) = row(&st, id);
    assert_eq!((status.as_str(), message.as_str()), ("cancelled", "Artist removed"));
    assert!(!own.exists());
    assert!(files.iter().all(|f| f.is_file()), "Soulseek's folder is never cleared");
    fault::clear(e.lib.join("Band"));
    e.done();
}

/// Picking another download for a request whose downloaded files wait to be imported would start it over
/// and throw them away: the pick is refused with what to do instead.
#[tokio::test]
async fn a_pick_for_a_request_with_kept_files_is_refused() {
    let e = env("repick");
    let st = e.st().clone();
    let Some((id, files)) = soulseek_album(&e, "Band", "Album", 1) else { return e.done() };
    fault::fail(e.lib.join("Band"), "mkdir", 5);
    worker::step(st.clone(), id).await;
    assert_eq!(row(&st, id).0, "waiting");
    let app = e.ts.app();
    let cookie = login(&app, "admin-repick").await;
    let pick = json!({ "source": "link", "payload": { "url": "https://www.youtube.com/watch?v=x" }, "request_id": id });
    let (code, body, _) = call(&app, "POST", "/api/picker/pick", &cookie, Some(pick)).await;
    assert_eq!(code, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["detail"], "This request has downloaded files waiting to be imported – Import again or cancel it first", "{body}");
    assert_eq!(row(&st, id).0, "waiting");
    assert!(files.iter().all(|f| f.is_file()));
    fault::clear(e.lib.join("Band"));
    e.done();
}

/// A link of which only some files could go into the library (a folder Encore may not write to): the
/// request is done for those, and the files that could not be imported are kept – not deleted – for Import
/// again, which imports them once the folder is fixed.
#[tokio::test]
async fn a_link_that_imported_part_keeps_the_rest_for_import_again() {
    let e = env("link-part");
    let st = e.st().clone();
    let t = now();
    let id = st.db.execute("INSERT INTO requests (user_id, kind, url, status, source, message, detail, created_at, updated_at) VALUES (?,?,?,?,?,?,?,?,?)",
        p![e.admin, "url", "https://www.youtube.com/playlist?list=x", "importing", "ytdlp", "Tagging and importing…", "{}", t, t]).unwrap();
    let dir = e.staging.join(format!("request-{id}"));
    let (one, two) = (dir.join("Artist One - First.flac"), dir.join("Artist Two - Second.flac"));
    if !flac(&one, "Artist One", "", "First", 1) || !flac(&two, "Artist Two", "", "Second", 2) {
        return e.done();
    }
    fault::fail(e.lib.join("Artist Two"), "mkdir", 13);
    worker::recover(&st);
    worker::step(st.clone(), id).await;
    let (status, message, _, d) = row(&st, id);
    assert_eq!(status, "completed", "{message}");
    assert!(message.starts_with("Downloaded 1 of 2 file(s) – the rest could not be imported: import failed: mkdir "), "{message}");
    assert!(!one.exists() && two.is_file(), "only what was imported left the folder");
    assert_eq!(d["kept"]["reason"], "error");
    let app = e.ts.app();
    let cookie = login(&app, "admin-link-part").await;
    let (_, list, _) = call(&app, "GET", "/api/requests", &cookie, None).await;
    let r = list.as_array().unwrap().iter().find(|r| r["id"] == id).unwrap().clone();
    assert_eq!(r["has_files"], true, "{r}");
    fault::clear(e.lib.join("Artist Two"));
    let (code, body, _) = call(&app, "POST", &format!("/api/requests/{id}/import-again"), &cookie, None).await;
    assert_eq!(code, StatusCode::OK, "{body}");
    worker::step(st.clone(), id).await;
    let (status, message, _, d) = row(&st, id);
    assert_eq!((status.as_str(), message.as_str()), ("completed", "Downloaded 2 file(s)"), "{d}");
    assert!(!dir.exists(), "the staging folder goes once everything is in");
    assert_eq!(audio_under(&e.lib).len(), 2);
    e.done();
}
