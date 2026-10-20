//! The file manager's routes on a whole server over temporary folders (library, staging, Soulseek's folder,
//! the config folder's Trash): confinement (absolute, `..`, links out, `%2e%2e` decoded, NUL, Unicode),
//! admin only and CSRF, a folder upload, the conflict rules, moves within and across roots (and disks) with
//! the library's rows following, a rename keeping a liked song and a playlist entry, the Trash round trip,
//! delete only from Trash, Empty trash, tags, and import in place and into the library.
use crate::test_support::{call, send, user, Resp, TestServer};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use encore_db::{now, p, RowExt};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use tower::ServiceExt;

struct Env {
    ts: TestServer,
    app: Router,
    lib: PathBuf,
    staging: PathBuf,
    slsk: PathBuf,
    /// a folder outside every root
    outside: PathBuf,
    cookie: String,
    admin: i64,
}

impl Env {
    fn st(&self) -> &crate::state::AppState {
        &self.ts.st
    }
    async fn get(&self, uri: &str) -> Resp {
        send(&self.app, "GET", uri, &self.cookie, None, &[]).await
    }
    async fn post(&self, uri: &str, body: Value) -> Resp {
        send(&self.app, "POST", uri, &self.cookie, Some(body), &[]).await
    }
    fn done(self) {
        self.ts.remove();
    }
    /// The row of the song at this path ((id, path) or None).
    fn row(&self, path: &Path) -> Option<i64> {
        self.st().db.one("SELECT id FROM tracks WHERE path=?", p![path.to_string_lossy()]).unwrap().map(|r| r.i("id"))
    }
    fn path_of(&self, id: i64) -> Option<String> {
        self.st().db.one("SELECT path FROM tracks WHERE id=?", p![id]).unwrap().map(|r| r.s("path"))
    }
}

async fn login(app: &Router, name: &str) -> String {
    let (code, _, cookie) = call(app, "POST", "/api/auth/login", "", Some(json!({ "username": name, "password": "a-long-password-1" }))).await;
    assert_eq!(code, StatusCode::OK);
    cookie.split(';').next().unwrap_or("").to_string()
}

async fn env(name: &str) -> Env {
    let ts = TestServer::new(&format!("files-{name}"));
    let (lib, staging, slsk, outside) = (ts.dir.join("music"), ts.dir.join("staging"), ts.dir.join("slskd"), ts.dir.join("outside"));
    for d in [&lib, &staging, &slsk, &outside] {
        std::fs::create_dir_all(d).unwrap();
    }
    ts.st.cfg.set_many(&json!({ "library_path": lib.to_string_lossy(), "staging_path": staging.to_string_lossy(), "slskd_download_path": slsk.to_string_lossy(),
        "musicbrainz_url": "http://127.0.0.1:9", "verify_on_import": false, "upgrade_check_audio": false, "embed_cover": false, "save_cover": false }));
    let admin = user(&ts.st, &format!("boss-{name}"), "admin").i("id");
    let app = ts.app();
    let cookie = login(&app, &format!("boss-{name}")).await;
    // the roots as the server resolves them (a temp folder can be behind a link)
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap();
    Env { lib: canon(&lib), staging: canon(&staging), slsk: canon(&slsk), outside: canon(&outside), ts, app, cookie, admin }
}

/// A short FLAC with tags (false when there is no ffmpeg here).
fn flac(path: &Path, artist: &str, album: &str, title: &str, track: u32) -> bool {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::process::Command::new("ffmpeg").args(["-v", "error", "-y", "-f", "lavfi", "-i", &format!("sine=frequency={}:duration=1", 300 + track * 110)])
        .args(["-metadata", &format!("title={title}"), "-metadata", &format!("artist={artist}"), "-metadata", &format!("album_artist={artist}"),
               "-metadata", &format!("album={album}"), "-metadata", &format!("track={track}"), "-metadata", "genre=Pop"])
        .arg(path).status().map(|s| s.success()).unwrap_or(false)
}

/// An album of two songs at `dir`, read into the library when it is in it. → the files, or None without ffmpeg
fn album(e: &Env, dir: &Path, artist: &str, title: &str) -> Option<Vec<PathBuf>> {
    let files = vec![dir.join("01 First.flac"), dir.join("02 Second.flac")];
    for (i, f) in files.iter().enumerate() {
        if !flac(f, artist, title, ["First", "Second"][i], i as u32 + 1) {
            return None;
        }
    }
    if dir.starts_with(&e.lib) {
        let done = encore_library::index_files(&e.st().db, &files.iter().map(|f| f.to_string_lossy().into_owned()).collect::<Vec<_>>());
        assert!(done.failed.is_empty(), "{:?}", done.failed);
    }
    Some(files)
}

/// A multipart upload, one `file` part per (filename, bytes).
async fn upload(e: &Env, uri: &str, parts: &[(&str, Vec<u8>)]) -> Resp {
    let b = "encoreTestBoundary7MA4YWxk";
    let mut body = Vec::new();
    for (name, data) in parts {
        body.extend(format!("--{b}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\nContent-Type: application/octet-stream\r\n\r\n").as_bytes());
        body.extend(data);
        body.extend(b"\r\n");
    }
    body.extend(format!("--{b}--\r\n").as_bytes());
    let req = Request::builder().method("POST").uri(uri).header("x-requested-with", "encore").header("cookie", &e.cookie)
        .header("content-type", format!("multipart/form-data; boundary={b}")).body(Body::from(body)).unwrap();
    let resp = e.app.clone().oneshot(req).await.unwrap();
    let (status, headers) = (resp.status(), resp.headers().clone());
    let body = axum::body::to_bytes(resp.into_body(), 64 << 20).await.unwrap().to_vec();
    Resp { status, headers, body }
}

fn liked(e: &Env, track: i64) -> i64 {
    let pid = e.st().db.execute("INSERT INTO playlists (user_id, name, kind, source, sync_targets, created_at, updated_at) VALUES (?, 'Liked Songs', 'liked', 'manual', '[]', ?, ?)",
        p![e.admin, now(), now()]).unwrap();
    e.st().db.execute("INSERT INTO playlist_items (playlist_id, position, title, track_id, added_at) VALUES (?,?,?,?,?)", p![pid, 0, "First", track, now()]).unwrap()
}

fn playlist_item(e: &Env, track: i64) -> i64 {
    let pid = e.st().db.execute("INSERT INTO playlists (user_id, name, kind, source, sync_targets, created_at, updated_at) VALUES (?, 'Mix', 'playlist', 'manual', '[]', ?, ?)",
        p![e.admin, now(), now()]).unwrap();
    e.st().db.execute("INSERT INTO playlist_items (playlist_id, position, title, track_id, added_at) VALUES (?,?,?,?,?)", p![pid, 0, "Second", track, now()]).unwrap()
}

fn item_track(e: &Env, item: i64) -> Option<i64> {
    e.st().db.one("SELECT track_id FROM playlist_items WHERE id=?", p![item]).unwrap().unwrap().get("track_id").and_then(|v| v.as_i64())
}

/// The song plays: its file route answers with the bytes.
async fn plays(e: &Env, id: i64) -> bool {
    e.get(&format!("/api/library/tracks/{id}/file")).await.status == StatusCode::OK
}

fn activity(e: &Env) -> Vec<String> {
    e.st().db.query("SELECT message FROM activity WHERE message LIKE 'File manager:%' ORDER BY id", p![]).unwrap().iter().map(|r| r.s("message")).collect()
}

#[tokio::test]
async fn roots_list_the_folders_with_trash_last_for_admins_only() {
    let e = env("roots").await;
    let r = e.get("/api/files/roots").await;
    assert_eq!(r.status, StatusCode::OK);
    let roots = r.json();
    let ids: Vec<&str> = roots.as_array().unwrap().iter().map(|x| x["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["library", "staging", "slskd", "trash"], "the download clients' folders are there only when they exist: {roots}");
    assert_eq!(roots[0]["name"], "Library");
    assert_eq!(roots[0]["writable"], true);
    assert!(roots[0]["free_bytes"].as_u64().unwrap() > 0);
    assert_eq!(roots[3]["path"], e.ts.dir.join("trash").to_string_lossy().as_ref());
    // a torrent folder that is there on the disk joins as torrent-1
    let tor = e.ts.dir.join("torrents");
    std::fs::create_dir_all(&tor).unwrap();
    e.st().cfg.set_many(&json!({ "torrent_client": "transmission", "transmission_download_path": tor.to_string_lossy() }));
    let roots = e.get("/api/files/roots").await.json();
    let t = roots.as_array().unwrap().iter().find(|x| x["id"] == "torrent-1").unwrap();
    assert_eq!(t["name"], "Torrents (Transmission)");
    // not for members, not without signing in, not without the CSRF header
    user(e.st(), "member1", "user");
    let member = login(&e.app, "member1").await;
    for (m, uri, body) in [("GET", "/api/files/roots", None), ("GET", "/api/files/list?root=library", None), ("GET", "/api/files/trash", None),
                           ("POST", "/api/files/mkdir", Some(json!({ "root": "library", "path": "", "name": "x" }))),
                           ("POST", "/api/files/empty-trash", Some(json!({}))), ("DELETE", "/api/files/trash/1-1", None),
                           ("PUT", "/api/files/tags", Some(json!({ "root": "library", "path": "a.flac", "tags": {} })))] {
        assert_eq!(send(&e.app, m, uri, &member, body.clone(), &[]).await.status, StatusCode::FORBIDDEN, "{m} {uri}");
        assert_eq!(send(&e.app, m, uri, "", body, &[]).await.status, StatusCode::UNAUTHORIZED, "{m} {uri}");
    }
    let req = Request::builder().method("POST").uri("/api/files/mkdir").header("cookie", &e.cookie).header("content-type", "application/json")
        .body(Body::from(json!({ "root": "library", "path": "", "name": "csrf" }).to_string())).unwrap();
    let resp = e.app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN, "a write without the CSRF header");
    assert!(!e.lib.join("csrf").exists());
    e.done();
}

#[tokio::test]
async fn paths_outside_the_roots_are_refused() {
    let e = env("confine").await;
    std::fs::create_dir_all(e.lib.join("A")).unwrap();
    std::fs::write(e.outside.join("secret.txt"), b"s").unwrap();
    std::os::unix::fs::symlink(&e.outside, e.lib.join("escape")).unwrap();
    std::os::unix::fs::symlink(e.lib.join("A"), e.lib.join("inner")).unwrap();
    std::os::unix::fs::symlink(e.staging.as_path(), e.lib.join("to-staging")).unwrap();
    std::os::unix::fs::symlink(e.outside.join("secret.txt"), e.lib.join("secret-link.txt")).unwrap();
    // absolute, `..`, `%2e%2e` (decoded once by the query parser), NUL, a Unicode look-alike of `..`
    for (q, status) in [("/etc", 403), ("..", 403), ("A/../..", 403), ("%2e%2e", 403), ("%2e%2e%2fstaging", 403), ("A%2f%2e%2e%2f%2e%2e", 403),
                        ("%ef%bc%8e%ef%bc%8e", 403), ("A%00B", 400)] {
        let r = e.get(&format!("/api/files/list?root=library&path={q}")).await;
        assert_eq!(r.status.as_u16(), status, "{q}: {}", r.text());
    }
    // a link out of the roots is listed as a link but never browsed
    let r = e.get("/api/files/list?root=library").await;
    let entries = r.json()["entries"].as_array().unwrap().clone();
    let esc = entries.iter().find(|x| x["name"] == "escape").unwrap();
    assert_eq!((esc["kind"].as_str(), esc["target_kind"].as_str(), esc["target"].as_str()), (Some("link"), Some("dir"), Some(e.outside.to_string_lossy().as_ref())));
    let r = e.get("/api/files/list?root=library&path=escape").await;
    assert_eq!((r.status, r.json()["detail"].as_str()), (StatusCode::FORBIDDEN, Some("That link points outside the folders Encore may use")));
    // a link into a root is browsed (and says so)
    let r = e.get("/api/files/list?root=library&path=to-staging").await;
    assert_eq!((r.status, r.json()["via_link"].as_bool()), (StatusCode::OK, Some(true)));
    // nothing is written through a link
    let through = "That folder is reached through a link, so Encore doesn't change anything in it";
    for path in ["escape", "inner", "inner/x", "to-staging"] {
        let r = e.post("/api/files/mkdir", json!({ "root": "library", "path": path, "name": "new" })).await;
        assert_eq!((r.status, r.json()["detail"].as_str()), (StatusCode::FORBIDDEN, Some(through)), "{path}");
        let r = upload(&e, &format!("/api/files/upload?root=library&path={path}"), &[("x.txt", b"x".to_vec())]).await;
        assert_eq!(r.status, StatusCode::FORBIDDEN, "{path}");
    }
    let r = e.post("/api/files/move", json!({ "root": "library", "from": ["A"], "to_root": "library", "to_path": "escape" })).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let r = e.post("/api/files/move", json!({ "root": "library", "from": ["inner/x"], "to_root": "staging", "to_path": "" })).await;
    assert_eq!(r.json()["failed"][0]["reason"], through, "{}", r.text());
    let r = send(&e.app, "PUT", "/api/files/tags", &e.cookie, Some(json!({ "root": "library", "path": "secret-link.txt", "tags": { "title": "x" } })), &[]).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    // a client's path in a JSON body is never decoded: "%2e%2e" is a (missing) name there
    let r = e.post("/api/files/rename", json!({ "root": "library", "path": "%2e%2e", "name": "x" })).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    for (path, name, status) in [("../outside", "x", 403), ("/etc/passwd", "x", 403), ("A", "../x", 400), ("A", "a/b", 400), ("A", "..", 400), ("A", "a\u{202e}b", 400),
                                 ("A", "a\u{0}b", 400), ("", "x", 400)] {
        let r = e.post("/api/files/rename", json!({ "root": "library", "path": path, "name": name })).await;
        assert_eq!(r.status.as_u16(), status, "{path} → {name}: {}", r.text());
    }
    assert_eq!(e.get("/api/files/list?root=elsewhere").await.status, StatusCode::NOT_FOUND);
    // the link itself can be renamed and trashed; what it points at is never touched
    let r = e.post("/api/files/rename", json!({ "root": "library", "path": "escape", "name": "escape2" })).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let r = e.post("/api/files/trash", json!({ "root": "library", "paths": ["escape2", "secret-link.txt"] })).await;
    assert_eq!(r.json()["trashed"], 2, "{}", r.text());
    assert!(std::fs::symlink_metadata(e.lib.join("escape2")).is_err());
    assert_eq!(std::fs::read(e.outside.join("secret.txt")).unwrap(), b"s", "the target is untouched");
    assert_eq!(std::fs::read_dir(&e.outside).unwrap().count(), 1, "nothing was made outside");
    e.done();
}

#[tokio::test]
async fn a_folder_upload_lands_with_its_folders_and_the_conflict_rules() {
    let e = env("upload").await;
    let dir = e.ts.dir.join("src");
    let (a, b) = (dir.join("a.flac"), dir.join("b.flac"));
    if !flac(&a, "Gnarls Barkley", "St. Elsewhere", "Crazy", 2) || !flac(&b, "Gnarls Barkley", "St. Elsewhere", "Smiley Faces", 3) {
        return e.done();
    }
    let (ab, bb) = (std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap());
    let r = upload(&e, "/api/files/upload?root=library&path=", &[("Gnarls Barkley/St. Elsewhere/02 Crazy.flac", ab.clone()),
        ("Gnarls Barkley/St. Elsewhere/03 Smiley Faces.flac", bb.clone()), ("Gnarls Barkley/St. Elsewhere/cover.jpg", b"\xff\xd8jpeg".to_vec()),
        ("../evil.flac", b"x".to_vec()), ("Gnarls Barkley/../../evil2.flac", b"x".to_vec())]).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let j = r.json();
    assert_eq!(j["uploaded"].as_array().unwrap().len(), 3, "{j}");
    assert_eq!(j["uploaded"][0], json!({ "name": "Gnarls Barkley/St. Elsewhere/02 Crazy.flac", "path": "Gnarls Barkley/St. Elsewhere/02 Crazy.flac", "size": ab.len() }));
    assert_eq!(j["skipped"].as_array().unwrap().len(), 2, "the parts that name `..` are refused: {j}");
    assert!(!e.ts.dir.join("evil.flac").exists() && !e.lib.join("evil.flac").exists() && !e.ts.dir.join("evil2.flac").exists());
    let album = e.lib.join("Gnarls Barkley/St. Elsewhere");
    assert_eq!(std::fs::read(album.join("02 Crazy.flac")).unwrap(), ab);
    assert!(std::fs::read_dir(&album).unwrap().flatten().all(|x| !x.file_name().to_string_lossy().ends_with(".part")), "no part file is left");
    // the library read the songs in; the listing says what it knows
    let crazy = e.row(&album.join("02 Crazy.flac")).expect("the uploaded song is in the library");
    let r = e.get("/api/files/list?root=library&path=Gnarls%20Barkley/St.%20Elsewhere&sort=name").await.json();
    assert_eq!(r["total"], 3);
    let entries = r["entries"].as_array().unwrap();
    assert_eq!(entries[0]["name"], "02 Crazy.flac");
    assert_eq!(entries[0]["audio"]["track_id"], crazy);
    assert_eq!((entries[0]["audio"]["title"].as_str(), entries[0]["audio"]["format"].as_str()), (Some("Crazy"), Some("flac")));
    assert_eq!((entries[2]["name"].as_str(), entries[2]["image"].as_bool(), entries[2].get("audio")), (Some("cover.jpg"), Some(true), None));
    // the same name again: "(2)" by default, skipped with skip, the old one to Trash with replace
    let up = "/api/files/upload?root=library&path=Gnarls%20Barkley";
    let r = upload(&e, up, &[("St. Elsewhere/02 Crazy.flac", bb.clone())]).await.json();
    assert_eq!(r["renamed"], json!([{ "from": "St. Elsewhere/02 Crazy.flac", "to": "St. Elsewhere/02 Crazy (2).flac" }]), "{r}");
    assert_eq!(r["uploaded"][0]["path"], "Gnarls Barkley/St. Elsewhere/02 Crazy (2).flac");
    let r = upload(&e, &format!("{up}&conflict=skip"), &[("St. Elsewhere/02 Crazy.flac", bb.clone()), ("St. Elsewhere/04 Next.txt", b"next".to_vec())]).await.json();
    assert_eq!(r["skipped"], json!([{ "name": "St. Elsewhere/02 Crazy.flac", "reason": "A file with that name is already there" }]));
    assert_eq!(r["uploaded"][0]["path"], "Gnarls Barkley/St. Elsewhere/04 Next.txt", "the part after a skipped one is read: {r}");
    assert_eq!(std::fs::read(album.join("04 Next.txt")).unwrap(), b"next");
    assert_eq!(std::fs::read(album.join("02 Crazy.flac")).unwrap(), ab, "skip leaves the file alone");
    let r = upload(&e, &format!("{up}&conflict=replace"), &[("St. Elsewhere/02 Crazy.flac", bb.clone()), ("St. Elsewhere", b"x".to_vec())]).await.json();
    assert_eq!(r["uploaded"][0]["path"], "Gnarls Barkley/St. Elsewhere/02 Crazy.flac", "{r}");
    assert_eq!(r["skipped"][0]["reason"], "A folder with that name is already there", "a folder is never replaced");
    assert_eq!(std::fs::read(album.join("02 Crazy.flac")).unwrap(), bb);
    let trash = e.get("/api/files/trash").await.json();
    assert_eq!((trash[0]["root"].as_str(), trash[0]["path"].as_str(), trash[0]["name"].as_str()), (Some("library"), Some("Gnarls Barkley/St. Elsewhere/02 Crazy.flac"), Some("02 Crazy.flac")));
    assert!(e.path_of(crazy).is_none(), "the replaced song left the library as a trashed one does");
    assert!(e.row(&album.join("02 Crazy.flac")).is_some(), "the new file was read in");
    // an upload that stops part way leaves nothing behind: no file, no part file
    let body = "--cut\r\nContent-Disposition: form-data; name=\"file\"; filename=\"half.flac\"\r\n\r\nthe first half of a song";
    let req = Request::builder().method("POST").uri("/api/files/upload?root=staging").header("x-requested-with", "encore").header("cookie", &e.cookie)
        .header("content-type", "multipart/form-data; boundary=cut").body(Body::from(body)).unwrap();
    let resp = e.app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_eq!(std::fs::read_dir(&e.staging).unwrap().count(), 0, "no half.flac and no half.flac.part");
    // a bad conflict word, an upload into Trash
    assert_eq!(upload(&e, "/api/files/upload?root=library&conflict=overwrite", &[("x", vec![])]).await.status, StatusCode::BAD_REQUEST);
    assert_eq!(upload(&e, "/api/files/upload?root=trash", &[("x", vec![])]).await.status, StatusCode::FORBIDDEN);
    assert!(activity(&e).iter().any(|l| l.contains("uploaded library:Gnarls Barkley/St. Elsewhere/02 Crazy.flac")), "{:?}", activity(&e));
    e.done();
}

#[tokio::test]
async fn moves_within_and_across_roots_and_disks_keep_the_rows() {
    let e = env("move").await;
    let Some(files) = album(&e, &e.lib.join("Artist/Album"), "Artist", "Album") else { return e.done() };
    let (one, two) = (e.row(&files[0]).unwrap(), e.row(&files[1]).unwrap());
    let like = liked(&e, one);
    let item = playlist_item(&e, two);
    std::fs::create_dir_all(e.lib.join("Other/Album")).unwrap();
    // a name that is taken fails alone; the other items still move
    std::fs::write(e.lib.join("Artist/notes.txt"), b"n").unwrap();
    let r = e.post("/api/files/move", json!({ "root": "library", "from": ["Artist/Album", "Artist/notes.txt"], "to_root": "library", "to_path": "Other" })).await.json();
    assert_eq!(r["moved"], 1, "{r}");
    assert_eq!(r["failed"], json!([{ "path": "Artist/Album", "reason": "A file with that name is already there" }]));
    std::fs::remove_dir(e.lib.join("Other/Album")).unwrap();
    let v0 = e.st().db.library_version();
    let r = e.post("/api/files/move", json!({ "root": "library", "from": ["Artist/Album"], "to_root": "library", "to_path": "Other" })).await.json();
    assert_eq!(r, json!({ "moved": 1, "failed": [] }));
    assert_eq!(e.st().db.library_version(), v0 + 1, "one transaction, one library version move");
    assert_eq!(e.path_of(one).unwrap(), e.lib.join("Other/Album/01 First.flac").to_string_lossy());
    assert_eq!((item_track(&e, like), item_track(&e, item)), (Some(one), Some(two)), "the like and the playlist entry keep their songs");
    assert!(plays(&e, one).await && plays(&e, two).await);
    // a folder can't go into itself
    let r = e.post("/api/files/move", json!({ "root": "library", "from": ["Other"], "to_root": "library", "to_path": "Other/Album" })).await.json();
    assert_eq!(r["failed"][0]["reason"], "A folder can't be moved into itself");
    // across disks (the rename fails with EXDEV): copied, renamed into place, the original removed; the rows follow
    encore_core::fsx::fault::fail(e.lib.join("Other"), "rename", libc::EXDEV);
    let r = e.post("/api/files/move", json!({ "root": "library", "from": ["Other/Album"], "to_root": "library", "to_path": "" })).await.json();
    encore_core::fsx::fault::clear(e.lib.join("Other"));
    assert_eq!(r, json!({ "moved": 1, "failed": [] }));
    assert!(!e.lib.join("Other/Album").exists() && e.lib.join("Album/02 Second.flac").is_file());
    assert!(std::fs::read_dir(&e.lib).unwrap().flatten().all(|x| !x.file_name().to_string_lossy().contains("encore-part")));
    assert_eq!(e.path_of(two).unwrap(), e.lib.join("Album/02 Second.flac").to_string_lossy());
    assert!(plays(&e, two).await);
    // out of the library to staging: the rows leave as a delete takes them (the like is kept as missing)
    let r = e.post("/api/files/move", json!({ "root": "library", "from": ["Album"], "to_root": "staging", "to_path": "" })).await.json();
    assert_eq!(r["moved"], 1);
    assert!(e.path_of(one).is_none() && e.staging.join("Album/01 First.flac").is_file());
    assert_eq!(item_track(&e, like), None);
    // and back from staging: read in again
    let r = e.post("/api/files/move", json!({ "root": "staging", "from": ["Album"], "to_root": "library", "to_path": "Artist" })).await.json();
    assert_eq!(r["moved"], 1);
    assert!(e.row(&e.lib.join("Artist/Album/01 First.flac")).is_some());
    let lines = activity(&e);
    assert!(lines.iter().any(|l| l.ends_with("moved library:Album to staging:Album")), "{lines:?}");
    e.done();
}

#[tokio::test]
async fn a_rename_keeps_a_liked_song_and_a_playlist_entry() {
    let e = env("rename").await;
    let Some(files) = album(&e, &e.lib.join("Artist/Album"), "Artist", "Album") else { return e.done() };
    let (one, two) = (e.row(&files[0]).unwrap(), e.row(&files[1]).unwrap());
    let r = e.post(&format!("/api/likes/{one}"), json!({})).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let item = playlist_item(&e, two);
    let r = e.post("/api/files/rename", json!({ "root": "library", "path": "Artist/Album", "name": "Album (2006)" })).await;
    assert_eq!(r.json(), json!({ "path": "Artist/Album (2006)" }));
    let r = e.post("/api/files/rename", json!({ "root": "library", "path": "Artist/Album (2006)/01 First.flac", "name": "01 - First.flac" })).await;
    assert_eq!(r.json(), json!({ "path": "Artist/Album (2006)/01 - First.flac" }));
    assert_eq!(e.path_of(one).unwrap(), e.lib.join("Artist/Album (2006)/01 - First.flac").to_string_lossy());
    assert_eq!(e.get("/api/likes").await.json(), json!([one]), "the like still points at the song");
    assert_eq!(item_track(&e, item), Some(two));
    assert!(plays(&e, one).await && plays(&e, two).await);
    // a name that is taken: 409, nothing replaced
    let r = e.post("/api/files/rename", json!({ "root": "library", "path": "Artist/Album (2006)/01 - First.flac", "name": "02 Second.flac" })).await;
    assert_eq!((r.status, r.json()["detail"].as_str()), (StatusCode::CONFLICT, Some("A file with that name is already there")));
    assert!(e.lib.join("Artist/Album (2006)/01 - First.flac").is_file());
    // a root itself can't be renamed
    assert_eq!(e.post("/api/files/rename", json!({ "root": "library", "path": "", "name": "x" })).await.status, StatusCode::BAD_REQUEST);
    assert!(activity(&e).iter().any(|l| l.contains("renamed library:Artist/Album to library:Artist/Album (2006)")));
    e.done();
}

#[tokio::test]
async fn trash_and_restore_give_back_the_files_and_the_rows() {
    let e = env("trash").await;
    let Some(files) = album(&e, &e.lib.join("Artist/Album"), "Artist", "Album") else { return e.done() };
    let (one, two) = (e.row(&files[0]).unwrap(), e.row(&files[1]).unwrap());
    let like = liked(&e, one);
    let item = playlist_item(&e, two);
    let r = e.post("/api/files/trash", json!({ "root": "library", "paths": ["Artist/Album", "Artist/missing.flac"] })).await.json();
    assert_eq!(r["trashed"], 1, "{r}");
    assert_eq!(r["failed"], json!([{ "path": "Artist/missing.flac", "reason": "That file or folder isn't there" }]));
    let id = r["items"][0].as_str().unwrap().to_string();
    let held = e.ts.dir.join("trash").join(&id);
    assert!(held.join("Album/01 First.flac").is_file() && !e.lib.join("Artist/Album").exists());
    let origin: Value = serde_json::from_str(&std::fs::read_to_string(held.join(".origin.json")).unwrap()).unwrap();
    assert_eq!((origin["root"].as_str(), origin["path"].as_str(), origin["by"].as_str()), (Some("library"), Some("Artist/Album"), Some("boss-trash")));
    assert!(origin["at"].as_i64().unwrap() > 0);
    // the songs left the library; the like and the playlist entry wait as missing
    assert!(e.path_of(one).is_none() && e.path_of(two).is_none());
    assert_eq!((item_track(&e, like), item_track(&e, item)), (None, None));
    let list = e.get("/api/files/trash").await.json();
    assert_eq!(list.as_array().unwrap().len(), 1);
    let it = &list[0];
    assert_eq!((it["id"].as_str(), it["name"].as_str(), it["root"].as_str(), it["path"].as_str(), it["kind"].as_str(), it["by"].as_str()),
               (Some(id.as_str()), Some("Album"), Some("library"), Some("Artist/Album"), Some("dir"), Some("boss-trash")));
    assert!(it["size"].as_u64().unwrap() > 0);
    // restore: back where it was, the same songs (ids), the like and the entry pointing at them again
    let r = e.post("/api/files/restore", json!({ "items": [id] })).await.json();
    assert_eq!(r, json!({ "restored": 1, "failed": [] }));
    assert!(e.lib.join("Artist/Album/01 First.flac").is_file() && !held.exists());
    assert_eq!(e.path_of(one).unwrap(), e.lib.join("Artist/Album/01 First.flac").to_string_lossy());
    assert_eq!((item_track(&e, like), item_track(&e, item)), (Some(one), Some(two)));
    assert!(plays(&e, one).await);
    // a name taken meanwhile gets " (2)"; missing folders are made again
    let r = e.post("/api/files/trash", json!({ "root": "library", "paths": ["Artist/Album/02 Second.flac"] })).await.json();
    let id2 = r["items"][0].as_str().unwrap().to_string();
    std::fs::write(e.lib.join("Artist/Album/02 Second.flac"), b"another").unwrap();
    let r = e.post("/api/files/restore", json!({ "items": [id2] })).await.json();
    assert_eq!(r["restored"], 1);
    assert!(e.lib.join("Artist/Album/02 Second (2).flac").is_file());
    std::fs::create_dir_all(e.staging.join("deep/er")).unwrap();
    std::fs::write(e.staging.join("deep/er/x.txt"), b"x").unwrap();
    let id3 = e.post("/api/files/trash", json!({ "root": "staging", "paths": ["deep/er/x.txt"] })).await.json()["items"][0].as_str().unwrap().to_string();
    std::fs::remove_dir_all(e.staging.join("deep")).unwrap();
    assert_eq!(e.post("/api/files/restore", json!({ "items": [id3] })).await.json()["restored"], 1);
    assert_eq!(std::fs::read(e.staging.join("deep/er/x.txt")).unwrap(), b"x");
    // a root that is no longer configured: the item stays in the trash
    let id4 = e.post("/api/files/trash", json!({ "root": "staging", "paths": ["deep"] })).await.json()["items"][0].as_str().unwrap().to_string();
    e.st().cfg.set_many(&json!({ "staging_path": "" }));
    let r = e.post("/api/files/restore", json!({ "items": [id4.clone(), "1-999"] })).await.json();
    assert_eq!(r["restored"], 0);
    assert_eq!(r["failed"][0], json!({ "id": id4, "reason": "Its folder is no longer one Encore may use" }));
    assert_eq!(r["failed"][1]["reason"], "That isn't in the trash");
    assert!(e.ts.dir.join("trash").join(&id4).is_dir());
    let lines = activity(&e);
    assert!(lines.iter().any(|l| l.contains("moved to trash library:Artist/Album")) && lines.iter().any(|l| l.contains("restored from trash library:Artist/Album")), "{lines:?}");
    e.done();
}

#[tokio::test]
async fn only_trash_deletes_for_good_and_empty_trash_spares_parked_reports() {
    let e = env("delete").await;
    for n in ["a.txt", "b.txt", "c.txt"] {
        std::fs::write(e.staging.join(n), n).unwrap();
    }
    let r = e.post("/api/files/trash", json!({ "root": "staging", "paths": ["a.txt", "b.txt", "c.txt"] })).await.json();
    let ids: Vec<String> = r["items"].as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_string()).collect();
    assert_eq!(ids.len(), 3);
    // the report flow's parked file in the same folder is no file manager item
    let parked = e.ts.dir.join("trash").join("1790000000-42-song.flac");
    std::fs::write(&parked, b"p").unwrap();
    let r = send(&e.app, "DELETE", &format!("/api/files/trash/{}", ids[0]), &e.cookie, None, &[]).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert!(!e.ts.dir.join("trash").join(&ids[0]).exists());
    for bad in ["1790000000-42-song.flac", "..", "%2e%2e", "1-1", "abc"] {
        let r = send(&e.app, "DELETE", &format!("/api/files/trash/{bad}"), &e.cookie, None, &[]).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{bad}");
    }
    assert!(parked.is_file());
    // nothing else deletes: a file in a root can only go to Trash
    assert_eq!(send(&e.app, "DELETE", "/api/files/list?root=staging&path=a.txt", &e.cookie, None, &[]).await.status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(e.get("/api/files/trash").await.json().as_array().unwrap().len(), 2, "newest first, the deleted one gone");
    let r = e.post("/api/files/empty-trash", json!({})).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert_eq!(e.get("/api/files/trash").await.json(), json!([]));
    assert!(parked.is_file(), "a parked report file stays");
    let lines = activity(&e);
    assert!(lines.iter().any(|l| l.contains("deleted forever staging:a.txt")) && lines.iter().any(|l| l.contains("emptied the trash (2 items")), "{lines:?}");
    e.done();
}

#[tokio::test]
async fn tags_are_read_written_and_cleared_and_the_row_follows() {
    let e = env("tags").await;
    let Some(files) = album(&e, &e.lib.join("Artist/Album"), "Artist", "Album") else { return e.done() };
    let one = e.row(&files[0]).unwrap();
    let r = e.get("/api/files/tags?root=library&path=Artist/Album/01%20First.flac").await.json();
    assert_eq!(r["tags"], json!({ "title": "First", "artist": "Artist", "album": "Album", "albumartist": "Artist", "tracknumber": "1", "discnumber": "", "date": "", "genre": "Pop" }));
    let v0 = e.st().db.library_version();
    let r = send(&e.app, "PUT", "/api/files/tags", &e.cookie, Some(json!({ "root": "library", "path": "Artist/Album/01 First.flac",
        "tags": { "title": "First Light", "genre": "", "date": "2006" } })), &[]).await;
    assert_eq!((r.status, r.json()), (StatusCode::OK, json!({ "ok": true })), "{}", r.text());
    let t = e.get("/api/files/tags?root=library&path=Artist/Album/01%20First.flac").await.json()["tags"].clone();
    assert_eq!((t["title"].as_str(), t["genre"].as_str(), t["date"].as_str(), t["artist"].as_str()), (Some("First Light"), Some(""), Some("2006"), Some("Artist")),
               "\"\" clears a tag, the others stay: {t}");
    let row = e.st().db.one("SELECT title, genre, year FROM tracks WHERE id=?", p![one]).unwrap().unwrap();
    assert_eq!((row.s("title"), row.s("genre"), row.s("year")), ("First Light".into(), "".into(), "2006".into()), "the row was read again from the file");
    assert_eq!(e.st().db.library_version(), v0 + 1);
    let r = send(&e.app, "PUT", "/api/files/tags", &e.cookie, Some(json!({ "root": "library", "path": "Artist/Album/01 First.flac", "tags": { "comment": "x" } })), &[]).await;
    assert_eq!((r.status, r.json()["detail"].as_str()), (StatusCode::BAD_REQUEST, Some("Unknown tag: comment")));
    let r = send(&e.app, "PUT", "/api/files/tags", &e.cookie, Some(json!({ "root": "library", "path": "Artist/Album/01 First.flac", "tags": { "tracknumber": "one" } })), &[]).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    // a file outside the library is tagged too (staging); a folder is not a file
    if flac(&e.staging.join("x.flac"), "A", "B", "C", 1) {
        let r = send(&e.app, "PUT", "/api/files/tags", &e.cookie, Some(json!({ "root": "staging", "path": "x.flac", "tags": { "artist": "Someone" } })), &[]).await;
        assert_eq!(r.status, StatusCode::OK);
        assert_eq!(encore_media::tags::read_tags(&e.staging.join("x.flac").to_string_lossy()).s("artist"), "Someone");
    }
    assert_eq!(e.get("/api/files/tags?root=library&path=Artist").await.status, StatusCode::BAD_REQUEST);
    e.done();
}

#[tokio::test]
async fn import_in_place_keeps_the_files_where_they_are() {
    let e = env("inplace").await;
    let Some(files) = album(&e, &e.slsk.join("Kept/Some Album"), "Kept Artist", "Some Album") else { return e.done() };
    let before: Vec<(Vec<u8>, std::time::SystemTime)> = files.iter().map(|f| (std::fs::read(f).unwrap(), std::fs::metadata(f).unwrap().modified().unwrap())).collect();
    let r = e.post("/api/files/import/preview", json!({ "root": "slskd", "path": "Kept", "mode": "in_place" })).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let a = &r.json()["albums"][0];
    assert_eq!((a["folder"].as_str(), a["artist"].as_str(), a["album"].as_str(), a["files"].as_i64()), (Some("Kept/Some Album"), Some("Kept Artist"), Some("Some Album"), Some(2)));
    assert_eq!((a["release"].clone(), a["rows"].clone()), (Value::Null, json!([])));
    let r = e.post("/api/files/import/apply", json!({ "root": "slskd", "path": "Kept", "mode": "in_place", "albums": [{ "folder": "Kept/Some Album", "release_id": null }] })).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let j = r.json();
    assert_eq!((j["imported"].as_i64(), j["message"].as_str()), (Some(2), Some("Imported 2 songs")));
    assert_eq!(j["albums"].as_array().unwrap().len(), 1);
    // nothing moved, renamed or retagged; the songs are in the library where they are
    for (f, (bytes, mtime)) in files.iter().zip(&before) {
        assert_eq!((&std::fs::read(f).unwrap(), &std::fs::metadata(f).unwrap().modified().unwrap()), (bytes, mtime));
        assert!(e.row(f).is_some(), "{}", f.display());
    }
    assert!(std::fs::read_dir(&e.lib).unwrap().next().is_none(), "nothing went into the library folder");
    let kept = e.slsk.join("Kept");
    assert_eq!(e.st().cfg.str("library_extra_roots"), kept.to_string_lossy());
    // a folder inside it adds nothing; a folder holding it replaces it
    let r = e.post("/api/files/import/apply", json!({ "root": "slskd", "path": "Kept/Some Album", "mode": "in_place", "albums": [{ "folder": "Kept/Some Album" }] })).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(e.st().cfg.str("library_extra_roots"), kept.to_string_lossy());
    // the file manager keeps the rows and the setting in step when the folder is renamed
    let one = e.row(&files[0]).unwrap();
    let r = e.post("/api/files/rename", json!({ "root": "slskd", "path": "Kept", "name": "Kept2" })).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(e.st().cfg.str("library_extra_roots"), e.slsk.join("Kept2").to_string_lossy());
    assert_eq!(e.path_of(one).unwrap(), e.slsk.join("Kept2/Some Album/01 First.flac").to_string_lossy());
    assert!(plays(&e, one).await);
    // the listing of a kept folder knows its songs
    let r = e.get("/api/files/list?root=slskd&path=Kept2/Some%20Album").await.json();
    assert_eq!(r["entries"][0]["audio"]["track_id"], one);
    // Organize leaves the kept songs where and as they are
    let kept_file = e.slsk.join("Kept2/Some Album/01 First.flac");
    let bytes = std::fs::read(&kept_file).unwrap();
    encore_library::organize::organize_library(e.st().clone(), true).await;
    assert!(e.st().db.one("SELECT 1 x FROM activity WHERE message LIKE 'Organized the library: 0 moved, 0 re-tagged'", p![]).unwrap().is_some(), "organize ran");
    assert_eq!(std::fs::read(&kept_file).unwrap(), bytes, "not moved, renamed or retagged");
    assert_eq!(e.path_of(one).unwrap(), kept_file.to_string_lossy());
    assert!(std::fs::read_dir(&e.lib).unwrap().next().is_none());
    e.done();
}

#[tokio::test]
async fn import_into_the_library_with_the_files_own_tags() {
    let e = env("import").await;
    let Some(files) = album(&e, &e.staging.join("In/Duumu - Illuminate EP"), "Duumu", "Illuminate EP") else { return e.done() };
    std::fs::write(e.staging.join("In/Duumu - Illuminate EP/info.nfo"), b"n").unwrap();
    // no MusicBrainz here: no match, the rows say where its own tags put each file
    let r = e.post("/api/files/import/preview", json!({ "root": "staging", "path": "In", "mode": "move" })).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let a = &r.json()["albums"][0];
    assert_eq!((a["folder"].as_str(), a["release"].clone(), a["confidence"].as_f64()), (Some("In/Duumu - Illuminate EP"), Value::Null, Some(0.0)));
    assert_eq!(a["rows"][0]["path"], "In/Duumu - Illuminate EP/01 First.flac");
    assert_eq!(a["rows"][0]["new_path"], "Duumu/Illuminate EP/01 - First.flac", "{a}");
    let r = e.post("/api/files/import/apply", json!({ "root": "staging", "path": "In", "mode": "move", "albums": [{ "folder": "In/Duumu - Illuminate EP", "release_id": null },
        { "folder": "../outside", "release_id": null }] })).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let j = r.json();
    assert_eq!(j["imported"], 2, "{j}");
    assert_eq!(j["failed"][0]["folder"], "../outside");
    let dest = e.lib.join("Duumu/Illuminate EP/01 - First.flac");
    assert!(dest.is_file() && !files[0].exists(), "moved into the library");
    assert!(e.row(&dest).is_some());
    assert!(!e.staging.join("In/Duumu - Illuminate EP").exists(), "the emptied download folder is cleared away");
    assert!(!e.staging.join("In").exists() && e.staging.is_dir(), "so is the folder imported, emptied; the root stays");
    // an album folder imported by itself does not stay behind empty, and the folder holding it stays
    let Some(_) = album(&e, &e.staging.join("Keep/Solo Album"), "Solo", "Solo Album") else { return e.done() };
    std::fs::write(e.staging.join("Keep/notes.txt"), b"mine").unwrap();
    let r = e.post("/api/files/import/apply", json!({ "root": "staging", "path": "Keep/Solo Album", "mode": "move", "albums": [{ "folder": "Keep/Solo Album" }] })).await;
    assert_eq!((r.status, r.json()["imported"].as_i64()), (StatusCode::OK, Some(2)), "{}", r.text());
    assert!(!e.staging.join("Keep/Solo Album").exists(), "the album folder went with its songs");
    assert_eq!(std::fs::read(e.staging.join("Keep/notes.txt")).unwrap(), b"mine", "the folder above is left as it is");
    // a folder outside the import's folder, a folder through a link
    let r = e.post("/api/files/import/preview", json!({ "root": "staging", "path": "In", "mode": "teleport" })).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    std::os::unix::fs::symlink(&e.outside, e.staging.join("linked")).unwrap();
    let r = e.post("/api/files/import/preview", json!({ "root": "staging", "path": "linked", "mode": "in_place" })).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    e.done();
}

#[tokio::test]
async fn listing_sorts_folders_first_and_pages() {
    let e = env("list").await;
    for i in 0..7 {
        std::fs::write(e.staging.join(format!("f{i}.txt")), "x".repeat(i)).unwrap();
    }
    std::fs::create_dir_all(e.staging.join("zz folder")).unwrap();
    let r = e.get("/api/files/list?root=staging&sort=size&dir=desc&offset=1&limit=3").await.json();
    assert_eq!(r["total"], 8);
    assert_eq!(r["offset"], 1);
    let names: Vec<&str> = r["entries"].as_array().unwrap().iter().map(|x| x["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["f6.txt", "f5.txt", "f4.txt"], "the folder is first in every sort, then largest first");
    let r = e.get("/api/files/list?root=staging").await.json();
    assert_eq!((r["root"].as_str(), r["path"].as_str(), r["entries"][0]["kind"].as_str()), (Some("staging"), Some(""), Some("dir")));
    assert_eq!(e.get("/api/files/list?root=staging&sort=colour").await.status, StatusCode::BAD_REQUEST);
    assert_eq!(e.get("/api/files/list?root=staging&path=f1.txt").await.status, StatusCode::BAD_REQUEST);
    assert_eq!(e.get("/api/files/list?root=staging&path=nothing").await.status, StatusCode::NOT_FOUND);
    // mkdir: made, then taken
    let r = e.post("/api/files/mkdir", json!({ "root": "staging", "path": "zz folder", "name": "New" })).await.json();
    assert_eq!(r, json!({ "path": "zz folder/New" }));
    let r = e.post("/api/files/mkdir", json!({ "root": "staging", "path": "zz folder", "name": "New" })).await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    e.done();
}

/// The kept folders of an import in place: the scheduler's scan and Rescan walk them with the library folder,
/// a kept folder that isn't there keeps its songs, they are storage roots, and neither Edit details with
/// renaming, a release's tags (the match review's retag) nor the automatic re-tag moves their songs.
#[tokio::test]
async fn kept_folders_are_scanned_and_their_songs_never_moved_by_tagging() {
    let mut e = env("keptscan").await;
    // no service answers (the covers and catalogues tagging looks up), so nothing leaves this machine
    e.ts.st.http = reqwest::Client::builder().proxy(reqwest::Proxy::all("http://127.0.0.1:9").unwrap()).build().unwrap();
    let Some(lib_files) = album(&e, &e.lib.join("Lib Artist/Lib Album"), "Lib Artist", "Lib Album") else { return e.done() };
    let Some(files) = album(&e, &e.slsk.join("Kept/Some Album"), "Kept Artist", "Some Album") else { return e.done() };
    let r = e.post("/api/files/import/apply", json!({ "root": "slskd", "path": "Kept", "mode": "in_place", "albums": [{ "folder": "Kept/Some Album", "release_id": null }] })).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let kept = e.slsk.join("Kept");
    assert_eq!(e.st().cfg.str("library_extra_roots"), kept.to_string_lossy());
    // a storage error in a kept folder reads as one
    encore_requests::worker::storage::register_roots(e.st());
    assert!(encore_core::fsx::roots::covers(&files[0]));
    let scan_task = || async { encore_jobs::scheduler::Tasks::run_task(e.st(), "scan").await.unwrap() };
    // a song put into the kept folder by hand is found by the scheduler's scan …
    let third = kept.join("Some Album/03 Third.flac");
    assert!(flac(&third, "Kept Artist", "Some Album", "Third", 3));
    assert_eq!(scan_task().await, "Scan complete: 1 updated, 0 removed");
    assert!(e.row(&third).is_some());
    // … and one taken out of it is gone after Rescan, the others stay
    std::fs::remove_file(&third).unwrap();
    let r = e.post("/api/library/scan", json!({})).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let mut waited = 0;
    while e.st().library.scan.lock().unwrap().message != "Scan complete: 0 updated, 1 removed" {
        waited += 1;
        assert!(waited < 400, "Rescan: {}", e.st().library.scan.lock().unwrap().message);
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert!(e.row(&third).is_none());
    assert!(lib_files.iter().chain(&files).all(|f| e.row(f).is_some()));
    // the kept folder's disk is away: it isn't scanned, its songs stay
    let away = e.slsk.join("Kept-away");
    std::fs::rename(&kept, &away).unwrap();
    assert_eq!(scan_task().await, format!("Scan complete: 0 updated, 0 removed (not there, so not scanned: {})", kept.display()));
    assert!(files.iter().all(|f| e.row(f).is_some()));
    std::fs::rename(&away, &kept).unwrap();
    // Edit details with renaming: the kept song gets its new title where it is, the library song is renamed
    let (one, lib_one) = (e.row(&files[0]).unwrap(), e.row(&lib_files[0]).unwrap());
    let r = e.post("/api/library/tags", json!({ "track_ids": [one, lib_one], "values": { "title": "Renamed" }, "rename": true, "lock": false })).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let title = |f: &Path| encore_media::tags::read_tags(&f.to_string_lossy()).s("title");
    assert_eq!(e.path_of(one).unwrap(), files[0].to_string_lossy());
    assert_eq!(title(&files[0]), "Renamed");
    let renamed = PathBuf::from(e.path_of(lib_one).unwrap());
    assert!(renamed != lib_files[0] && renamed.starts_with(&e.lib) && renamed.is_file() && !lib_files[0].exists(), "{}", renamed.display());
    // a release's tags with renaming on (retag_album and the re-tag run this): tagged, and only the library
    // song moves
    let row = |f: &Path, to: PathBuf| json!({ "path": f, "values": { "title": "From Release" }, "new_path": to });
    let pv = json!({ "release": { "title": "Some Album", "artist": "Kept Artist", "provider": "Deezer" }, "rows": [
        row(&files[1], e.lib.join("Kept Artist/Some Album/02 From Release.flac")), row(&lib_files[1], e.lib.join("Lib Artist/Lib Album/02 From Release.flac"))] });
    let new = encore_library::tagger::apply_preview(e.st(), &pv, true, None, None).await.unwrap();
    assert_eq!(new, vec![files[1].clone(), e.lib.join("Lib Artist/Lib Album/02 From Release.flac")]);
    assert!(new.iter().all(|f| f.is_file() && title(f) == "From Release"));
    assert!(!e.lib.join("Kept Artist").exists());
    encore_library::tagger::relink(e.st(), &[files[1].clone(), lib_files[1].clone()], &new);
    // the automatic re-tag (albums made from Deezer or Apple data) tries the library album only
    let req = e.st().db.execute("INSERT INTO requests (kind, status, detail, created_at, updated_at) VALUES ('album', 'done', '{\"ext\": true}', ?, ?)", p![now(), now()]).unwrap();
    e.st().db.execute("UPDATE tracks SET request_id=?", p![req]).unwrap();
    assert_eq!(encore_library::tagger::retag_external(e.st()).await, 0);
    let tried: Vec<String> = e.st().db.query("SELECT message FROM activity WHERE message LIKE 'Re-tag of %'", p![]).unwrap().iter().map(|r| r.s("message")).collect();
    assert!(tried.len() == 1 && tried[0].starts_with("Re-tag of Lib Album failed"), "{tried:?}");
    assert!(files.iter().all(|f| f.is_file() && e.row(f).is_some()));
    e.done();
}

/// Review fixes: a song kept in an extra library folder is never deleted by an import's dedupe (the file
/// manager's import or the request worker's), the file manager's import sends what it makes redundant (a
/// lower-quality library copy, the leftovers of an emptied folder) to Trash, and the like of the old copy
/// goes to the new song.
#[tokio::test]
async fn imports_never_delete_kept_songs_and_send_what_they_drop_to_trash() {
    let e = env("dedupe").await;
    e.st().cfg.set_many(&json!({ "dedupe_on_import": true }));
    let kept = e.slsk.join("Mount/Lossy Album/01 Song.mp3");
    let old = e.lib.join("Dup Artist/Old Single/01 Song.mp3");
    let better = e.staging.join("Better/01 Song.flac");
    if !flac(&kept, "Dup Artist", "Lossy Album", "Song", 1) || !flac(&old, "Dup Artist", "Old Single", "Song", 1) || !flac(&better, "Dup Artist", "Better", "Song", 1) {
        return e.done();
    }
    encore_library::index_files(&e.st().db, &[old.to_string_lossy().into_owned()]);
    let old_id = e.row(&old).unwrap();
    let like = liked(&e, old_id);
    std::fs::write(e.staging.join("Better/info.nfo"), b"rip log").unwrap();
    let r = e.post("/api/files/import/apply", json!({ "root": "slskd", "path": "Mount/Lossy Album", "mode": "in_place", "albums": [{ "folder": "Mount/Lossy Album" }] })).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let kept_id = e.row(&kept).unwrap();
    let r = e.post("/api/files/import/apply", json!({ "root": "staging", "path": "Better", "mode": "move", "albums": [{ "folder": "Better" }] })).await;
    assert_eq!((r.status, r.json()["imported"].as_i64()), (StatusCode::OK, Some(1)), "{}", r.text());
    let new = e.lib.join("Dup Artist/Better/01 - Song.flac");
    let new_id = e.row(&new).expect("the better copy is in the library");
    // the kept song is untouched: its file and its row
    assert!(kept.is_file() && e.row(&kept) == Some(kept_id), "a song in an extra library folder is never dropped");
    // the library's lower-quality copy went to Trash (not away for good), its like to the new song
    assert!(!old.exists() && e.path_of(old_id).is_none());
    assert_eq!(item_track(&e, like), Some(new_id));
    let trash = e.get("/api/files/trash").await.json();
    let items: Vec<(String, String)> = trash.as_array().unwrap().iter().map(|x| (x["root"].as_str().unwrap().to_string(), x["path"].as_str().unwrap().to_string())).collect();
    assert!(items.contains(&("library".into(), "Dup Artist/Old Single/01 Song.mp3".into())), "{items:?}");
    // the emptied download folder's leftovers are a Trash item too
    assert!(items.contains(&("staging".into(), "Better".into())) && !e.staging.join("Better").exists(), "{items:?}");
    let held = trash.as_array().unwrap().iter().find(|x| x["path"] == "Better").unwrap()["id"].as_str().unwrap().to_string();
    assert_eq!(std::fs::read(e.ts.dir.join("trash").join(&held).join("Better/info.nfo")).unwrap(), b"rip log");
    // the request worker's dedupe after an album import leaves the kept song alone as well
    let again = e.lib.join("Dup Artist/Third/01 Song.flac");
    assert!(flac(&again, "Dup Artist", "Third", "Song", 1));
    encore_library::index_files(&e.st().db, &[again.to_string_lossy().into_owned()]);
    encore_library::tagger::drop_superseded(e.st(), std::slice::from_ref(&again)).await;
    assert!(kept.is_file() && e.row(&kept) == Some(kept_id));
    e.done();
}

/// Review fix: Import (move) of songs that are in the library already keeps their rows: the same ids, the
/// like, no second row for the new path.
#[tokio::test]
async fn a_move_import_of_library_songs_keeps_their_rows() {
    let e = env("relink").await;
    let Some(files) = album(&e, &e.lib.join("Alpha Artist/First Album"), "Alpha Artist", "First Album") else { return e.done() };
    let one = e.row(&files[0]).unwrap();
    let like = liked(&e, one);
    let r = e.post("/api/files/import/apply", json!({ "root": "library", "path": "Alpha Artist/First Album", "mode": "move", "albums": [{ "folder": "Alpha Artist/First Album" }] })).await;
    assert_eq!((r.status, r.json()["imported"].as_i64()), (StatusCode::OK, Some(2)), "{}", r.text());
    let moved = e.lib.join("Alpha Artist/First Album/01 - First.flac");
    assert!(moved.is_file() && !files[0].exists());
    assert_eq!(e.path_of(one).unwrap(), moved.to_string_lossy(), "the same row follows the file");
    assert_eq!(e.st().db.one("SELECT COUNT(*) n FROM tracks", p![]).unwrap().unwrap().i("n"), 2, "no second row");
    assert_eq!(item_track(&e, like), Some(one));
    assert!(plays(&e, one).await);
    e.done();
}

/// Review fixes: renaming or moving a folder that holds a kept folder moves its songs' rows with it; such a
/// folder can't go to Trash; Staging and a folder holding the library can't be kept in place, and Encore's
/// clean-up of a request's staging folder leaves a kept folder in it alone.
#[tokio::test]
async fn kept_folders_keep_their_rows_and_are_never_cleared_away() {
    let e = env("keptparent").await;
    let Some(files) = album(&e, &e.slsk.join("Parent/Kept Album"), "Kept Artist", "Kept Album") else { return e.done() };
    let r = e.post("/api/files/import/apply", json!({ "root": "slskd", "path": "Parent/Kept Album", "mode": "in_place", "albums": [{ "folder": "Parent/Kept Album" }] })).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let one = e.row(&files[0]).unwrap();
    let like = liked(&e, one);
    let r = e.post("/api/files/rename", json!({ "root": "slskd", "path": "Parent", "name": "Parent2" })).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(e.st().cfg.str("library_extra_roots"), e.slsk.join("Parent2/Kept Album").to_string_lossy());
    assert_eq!(e.path_of(one).unwrap(), e.slsk.join("Parent2/Kept Album/01 First.flac").to_string_lossy(), "the row follows the renamed folder above");
    assert!(plays(&e, one).await);
    // moved into another root: the rows follow too
    let r = e.post("/api/files/move", json!({ "root": "slskd", "from": ["Parent2"], "to_root": "staging", "to_path": "" })).await.json();
    assert_eq!(r["moved"], 1, "{r}");
    assert_eq!(e.path_of(one).unwrap(), e.staging.join("Parent2/Kept Album/01 First.flac").to_string_lossy());
    assert_eq!(item_track(&e, like), Some(one));
    // the folder holding it can't go to Trash (the kept folder by itself can)
    let r = e.post("/api/files/trash", json!({ "root": "staging", "paths": ["Parent2"] })).await.json();
    assert_eq!(r["trashed"], 0);
    assert!(r["failed"][0]["reason"].as_str().unwrap().contains("holds an extra library folder"), "{r}");
    assert!(e.row(&e.staging.join("Parent2/Kept Album/01 First.flac")).is_some());
    // a request's staging folder holding a kept folder is not removed with the request
    std::fs::create_dir_all(e.staging.join("request-77")).unwrap();
    std::fs::rename(e.staging.join("Parent2"), e.staging.join("request-77/Parent2")).unwrap();
    e.st().cfg.set_many(&json!({ "library_extra_roots": e.staging.join("request-77/Parent2/Kept Album").to_string_lossy() }));
    assert!(!encore_requests::worker::storage::remove_staging(e.st(), 77));
    assert!(e.staging.join("request-77/Parent2/Kept Album/01 First.flac").is_file());
    // Staging, and a folder that holds the library, can't be kept where they are
    std::fs::create_dir_all(e.staging.join("Loose")).unwrap();
    assert!(flac(&e.staging.join("Loose/01 x.flac"), "A", "B", "C", 1));
    let r = e.post("/api/files/import/apply", json!({ "root": "staging", "path": "Loose", "mode": "in_place", "albums": [{ "folder": "Loose" }] })).await;
    assert_eq!(r.status, StatusCode::CONFLICT, "{}", r.text());
    assert!(r.json()["detail"].as_str().unwrap().starts_with("Files in Staging can't be kept where they are"));
    let lib_parent = e.lib.parent().unwrap().to_path_buf();
    assert!(encore_files::roots::keep_refusal(&e.st().cfg, e.st().data_dir(), &lib_parent).is_some_and(|w| w.contains("holds the library folder")));
    e.done();
}

/// Review fixes: each upload has a part file of its own: a file named like a part (a torrent client's
/// unfinished `<name>.part`) is never touched, two uploads of one name don't mix their bytes, and a part
/// whose upload went away is removed.
#[tokio::test]
async fn uploads_of_one_name_keep_their_own_part_files() {
    use encore_files::upload::{self, Begin, Conflict, Outcome};
    use std::io::Write;
    let e = env("parts").await;
    std::fs::write(e.staging.join("song.flac.part"), b"a torrent's unfinished file").unwrap();
    let r = upload(&e, "/api/files/upload?root=staging&path=", &[("song.flac", b"uploaded".to_vec())]).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(std::fs::read(e.staging.join("song.flac.part")).unwrap(), b"a torrent's unfinished file");
    assert_eq!(std::fs::read(e.staging.join("song.flac")).unwrap(), b"uploaded");
    // two uploads of race.flac at once
    let roots = encore_files::roots::all(&e.st().cfg, e.st().data_dir(), &e.slsk);
    let base = encore_files::Target::new(encore_files::roots::find(&roots, "staging").unwrap(), "").unwrap();
    let begin = || match upload::begin(&base, "race.flac", Conflict::Rename).unwrap() {
        Begin::Write(i) => *i,
        Begin::Skip(_, why) => panic!("{why}"),
    };
    let (mut a, mut b) = (begin(), begin());
    a.file.write_all(&[b'A'; 4000]).unwrap();
    b.file.write_all(&[b'B'; 2000]).unwrap();
    let who = encore_files::Who { id: e.admin, name: "boss-parts".into() };
    let mut change = encore_files::rows::Change::default();
    let oa = upload::finish(e.st(), &who, a, 4000, Conflict::Rename, &mut change);
    assert!(matches!(&oa, Outcome::Uploaded { path, .. } if path == "race.flac"));
    assert_eq!(std::fs::read(e.staging.join("race.flac")).unwrap(), vec![b'A'; 4000], "A's bytes, all of them");
    let ob = upload::finish(e.st(), &who, b, 2000, Conflict::Rename, &mut change);
    assert!(matches!(&ob, Outcome::Uploaded { path, .. } if path == "race (2).flac"));
    assert_eq!(std::fs::read(e.staging.join("race (2).flac")).unwrap(), vec![b'B'; 2000]);
    // an upload dropped part way (its request went away) leaves no part file
    let c = begin();
    drop(c);
    let left: Vec<String> = std::fs::read_dir(&e.staging).unwrap().flatten().map(|x| x.file_name().to_string_lossy().into_owned()).collect();
    assert!(left.iter().all(|n| !n.ends_with(".encore-upload")), "{left:?}");
    e.done();
}

/// Review fixes: Delete forever removes the note last (an item that can't be removed whole stays listed),
/// a note that names a path instead of one name is neither listed nor restored, and a root that holds
/// Encore's config folder can't reach into it.
#[tokio::test]
async fn trash_notes_are_checked_and_the_config_folder_is_fenced_off() {
    use std::os::unix::ffi::OsStrExt;
    let e = env("trashnote").await;
    std::fs::create_dir_all(e.staging.join("Box")).unwrap();
    std::fs::write(e.staging.join("Box/a.txt"), b"a").unwrap();
    let id = e.post("/api/files/trash", json!({ "root": "staging", "paths": ["Box"] })).await.json()["items"][0].as_str().unwrap().to_string();
    let held = e.ts.dir.join("trash").join(&id);
    std::fs::write(held.join("Box").join(std::ffi::OsStr::from_bytes(b"bad\xffname")), b"x").unwrap();
    let r = send(&e.app, "DELETE", &format!("/api/files/trash/{id}"), &e.cookie, None, &[]).await;
    assert_eq!(r.status, StatusCode::CONFLICT, "{}", r.text());
    assert!(held.join(".origin.json").is_file());
    assert_eq!(e.get("/api/files/trash").await.json().as_array().unwrap().len(), 1, "still listed with its note");
    std::fs::remove_file(held.join("Box").join(std::ffi::OsStr::from_bytes(b"bad\xffname"))).unwrap();
    assert_eq!(send(&e.app, "DELETE", &format!("/api/files/trash/{id}"), &e.cookie, None, &[]).await.status, StatusCode::NO_CONTENT);
    assert!(!held.exists());
    // a note that names a path out of its folder
    let forged = e.ts.dir.join("trash/1700000000-5");
    std::fs::create_dir_all(&forged).unwrap();
    std::fs::write(forged.join(".origin.json"), json!({ "root": "staging", "path": "x.db", "at": 1, "by": "x", "name": "x.db", "stored": "../../encore.db" }).to_string()).unwrap();
    assert_eq!(e.get("/api/files/trash").await.json(), json!([]));
    let r = e.post("/api/files/restore", json!({ "items": ["1700000000-5"] })).await.json();
    assert_eq!(r["restored"], 0, "{r}");
    assert!(!e.staging.join("x.db").exists());
    // a download folder set to Encore's config folder: nothing in it is listed or changed through it
    e.st().cfg.set_many(&json!({ "qbit_download_path": e.ts.dir.to_string_lossy() }));
    let roots = e.get("/api/files/roots").await.json();
    let tor = roots.as_array().unwrap().iter().find(|r| r["path"] == e.ts.dir.to_string_lossy().as_ref()).map(|r| r["id"].as_str().unwrap().to_string());
    let tor = tor.expect("the download folder is a root");
    assert_eq!(e.get(&format!("/api/files/list?root={tor}")).await.status, StatusCode::FORBIDDEN);
    let r = e.post("/api/files/rename", json!({ "root": tor, "path": "trash", "name": "gone" })).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "{}", r.text());
    assert!(e.ts.dir.join("trash").is_dir());
    e.done();
}
