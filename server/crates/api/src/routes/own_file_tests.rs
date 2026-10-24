//! Bring your own music on a whole server over temporary folders: one song's file replaced through the API
//! (the id, plays, likes, playlist spots and lyrics stay; the old file waits in the trash; the new one is
//! tagged as the song; the Signal path's info says whose file it is), who may do it, files that aren't audio
//! turned away, and a batch upload matched to the songs by its tags and applied. Real audio from ffmpeg: the
//! tests skip themselves where it isn't installed.
use crate::test_support::{call, send, user, Resp, TestServer};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use encore_db::{now, p, RowExt};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use tower::ServiceExt;

/// A server, its music folder and two signed-in accounts (an admin, and a listener without the permission).
struct Env {
    ts: TestServer,
    app: Router,
    lib: PathBuf,
    admin: String,
    listener: String,
    listener_id: i64,
}

impl Env {
    fn st(&self) -> &crate::state::AppState {
        &self.ts.st
    }
}

/// Signs `name` in → its session cookie.
async fn login(app: &Router, name: &str) -> String {
    let (code, _, cookie) = call(app, "POST", "/api/auth/login", "", Some(json!({ "username": name, "password": "a-long-password-1" }))).await;
    assert_eq!(code, StatusCode::OK);
    cookie.split(';').next().unwrap_or("").to_string()
}

/// The server for test `name`.
async fn env(name: &str) -> Env {
    let ts = TestServer::new(&format!("own-{name}"));
    let lib = ts.dir.join("music");
    std::fs::create_dir_all(&lib).unwrap();
    ts.st.cfg.set_many(&json!({ "library_path": lib.to_string_lossy(), "musicbrainz_url": "http://127.0.0.1:9", "quality_check_on_import": false }));
    user(&ts.st, &format!("boss-{name}"), "admin");
    let listener_id = user(&ts.st, &format!("fan-{name}"), "user").i("id");
    let app = ts.app();
    let admin = login(&app, &format!("boss-{name}")).await;
    let listener = login(&app, &format!("fan-{name}")).await;
    Env { lib: std::fs::canonicalize(&lib).unwrap(), ts, app, admin, listener, listener_id }
}

/// A one-second sine at `path` in the format its extension says, with these tags (false without ffmpeg).
fn audio(path: &Path, freq: u32, secs: u32, tags: &[(&str, &str)]) -> bool {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut c = std::process::Command::new("ffmpeg");
    c.args(["-v", "error", "-y", "-f", "lavfi", "-i", &format!("sine=frequency={freq}:duration={secs}")]);
    for (k, v) in tags {
        c.args(["-metadata", &format!("{k}={v}")]);
    }
    c.arg(path).status().map(|s| s.success()).unwrap_or(false)
}

/// A multipart upload to `uri` as `cookie`, one `file` part per (filename, bytes).
async fn upload(e: &Env, cookie: &str, uri: &str, parts: &[(&str, Vec<u8>)]) -> Resp {
    let b = "encoreOwnFileBoundary42";
    let mut body = Vec::new();
    for (name, data) in parts {
        body.extend(format!("--{b}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\nContent-Type: application/octet-stream\r\n\r\n").as_bytes());
        body.extend(data);
        body.extend(b"\r\n");
    }
    body.extend(format!("--{b}--\r\n").as_bytes());
    let req = Request::builder().method("POST").uri(uri).header("x-requested-with", "encore").header("cookie", cookie)
        .header("content-type", format!("multipart/form-data; boundary={b}")).body(Body::from(body)).unwrap();
    let resp = e.app.clone().oneshot(req).await.unwrap();
    let (status, headers) = (resp.status(), resp.headers().clone());
    let body = axum::body::to_bytes(resp.into_body(), 64 << 20).await.unwrap().to_vec();
    Resp { status, headers, body }
}

/// A song of the library: an MP3 at `rel` with full tags, read in. → (its id, its file), None without ffmpeg
fn song(e: &Env, rel: &str, title: &str, track: &str) -> Option<(i64, PathBuf)> {
    let f = e.lib.join(rel);
    let tags = [("title", title), ("artist", "Gnarls Barkley"), ("album_artist", "Gnarls Barkley"), ("album", "St. Elsewhere"),
                ("track", track), ("date", "2006"), ("genre", "Soul")];
    if !audio(&f, 440, 2, &tags) {
        return None;
    }
    encore_library::index_file(&e.st().db, &f.to_string_lossy()).unwrap();
    let id = e.st().db.one("SELECT id FROM tracks WHERE path=?", p![f.to_string_lossy()]).unwrap().unwrap().i("id");
    Some((id, f))
}

/// The bytes of a new file made by ffmpeg in the test's folder (an upload from someone's computer).
fn upload_bytes(e: &Env, name: &str, secs: u32, tags: &[(&str, &str)]) -> Option<Vec<u8>> {
    let f = e.ts.dir.join("mine").join(name);
    audio(&f, 660, secs, tags).then(|| std::fs::read(&f).unwrap())
}

#[tokio::test]
async fn a_song_takes_your_own_file_and_keeps_everything_else() {
    let e = env("one").await;
    let Some((id, old)) = song(&e, "Gnarls Barkley/St. Elsewhere/02 Crazy.mp3", "Crazy", "2") else { return e.ts.remove() };
    // lyrics beside it, a play, a like and a playlist spot
    std::fs::write(old.with_extension("lrc"), "[00:01.00] I remember when").unwrap();
    let db = &e.st().db;
    db.execute("INSERT INTO plays (user_id, track_id, seconds, played_at) VALUES (?,?,?,?)", p![e.listener_id, id, 120.0, now()]).unwrap();
    let pl = db.execute("INSERT INTO playlists (user_id, name, kind, created_at, updated_at) VALUES (?,?,?,?,?)", p![e.listener_id, "Liked", "liked", now(), now()]).unwrap();
    db.execute("INSERT INTO playlist_items (playlist_id, position, title, track_id, added_at) VALUES (?,?,?,?,?)", p![pl, 0, "Crazy", id, now()]).unwrap();
    // the upload: a FLAC with other tags (someone's own rip, named whatever)
    let Some(bytes) = upload_bytes(&e, "my rip.flac", 2, &[("title", "crazy (my rip)"), ("artist", "gnarls"), ("REPLAYGAIN_TRACK_GAIN", "-3.00 dB")]) else { return e.ts.remove() };
    let uri = format!("/api/library/tracks/{id}/own-file");

    // a listener without the permission can't; one with it can
    assert_eq!(upload(&e, &e.listener, &uri, &[("my rip.flac", bytes.clone())]).await.status, StatusCode::FORBIDDEN);
    // something that isn't audio is turned away, and the song is untouched
    let r = upload(&e, &e.admin, &uri, &[("notes.flac", b"these are not sounds".to_vec())]).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.text());
    let r = upload(&e, &e.admin, &uri, &[("cover.jpg", b"\xff\xd8\xffimage".to_vec())]).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.text());
    assert!(old.is_file());
    // no song there
    assert_eq!(upload(&e, &e.admin, "/api/library/tracks/999999/own-file", &[("my rip.flac", bytes.clone())]).await.status, StatusCode::NOT_FOUND);

    let r = upload(&e, &e.admin, &uri, &[("my rip.flac", bytes)]).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let v = r.json();
    assert_eq!((v["before"]["format"].as_str(), v["after"]["format"].as_str()), (Some("mp3"), Some("flac")));
    // the same song: same id, the new file under the old name with its own extension
    let row = db.one("SELECT * FROM tracks WHERE id=?", p![id]).unwrap().expect("the song keeps its id");
    let new = old.with_extension("flac");
    assert_eq!(row.s("path"), new.to_string_lossy());
    assert_eq!((row.s("format").as_str(), row.s("title").as_str(), row.s("artist").as_str(), row.s("album").as_str(), row.i("track")),
               ("flac", "Crazy", "Gnarls Barkley", "St. Elsewhere", 2));
    assert_eq!((row.s("source").as_str(), row.s("source_detail").as_str()), ("Your own file", "uploaded by boss-one"));
    assert_eq!(db.query("SELECT id FROM tracks", p![]).unwrap().len(), 1, "no second row");
    // the file itself carries the song's tags, not the upload's, and keeps its own loudness tag
    let tags = encore_media::tags::read_tags(&new.to_string_lossy());
    assert_eq!((tags.s("title").as_str(), tags.s("artist").as_str(), tags.s("album").as_str(), tags.i("track")), ("Crazy", "Gnarls Barkley", "St. Elsewhere", 2));
    assert_eq!(encore_media::tags::read_facts(&new.to_string_lossy()).gain.track_gain, Some(-3.0));
    // plays, likes, playlists and lyrics stay with it
    assert_eq!(db.one("SELECT COUNT(*) n FROM plays WHERE track_id=?", p![id]).unwrap().unwrap().i("n"), 1);
    assert_eq!(db.one("SELECT COUNT(*) n FROM playlist_items WHERE track_id=?", p![id]).unwrap().unwrap().i("n"), 1);
    assert!(new.with_extension("lrc").is_file());
    // the old file waits in the trash (deleted after 7 days by the report flow's clean-up)
    assert!(!old.exists());
    let trashed = PathBuf::from(v["trash_path"].as_str().unwrap());
    assert!(trashed.is_file() && trashed.starts_with(e.st().data_dir().join("trash")), "{trashed:?}");
    assert_eq!(encore_requests::report::purge_trash(e.st()), 0, "kept for now");
    // the Signal path's info says whose file it is
    let r = send(&e.app, "GET", &format!("/api/library/tracks/{id}/info"), &e.listener, None, &[]).await;
    assert_eq!(r.json()["own_file"]["by"], "boss-one");
    assert_eq!(r.json()["own_file"]["old_format"], "mp3");
    // nothing staged is left behind
    assert_eq!(std::fs::read_dir(e.st().data_dir().join("own-files")).map(|d| d.count()).unwrap_or(0), 0);

    // an admin can give a listener the permission; then they may too
    db.execute("UPDATE users SET perms=? WHERE id=?", p![json!({ "own_files": true }).to_string(), e.listener_id]).unwrap();
    let Some(again) = upload_bytes(&e, "again.mp3", 2, &[]) else { return e.ts.remove() };
    let r = upload(&e, &e.listener, &uri, &[("again.mp3", again)]).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(db.one("SELECT path FROM tracks WHERE id=?", p![id]).unwrap().unwrap().s("path"), old.to_string_lossy(), "back to .mp3");
    let r = send(&e.app, "GET", &format!("/api/library/tracks/{id}/info"), &e.listener, None, &[]).await;
    assert_eq!(r.json()["own_file"]["by"], "fan-one");
    e.ts.remove();
}

#[tokio::test]
async fn a_batch_upload_is_matched_by_tags_and_applied() {
    let e = env("batch").await;
    let Some((crazy, _)) = song(&e, "Gnarls Barkley/St. Elsewhere/02 Crazy.mp3", "Crazy", "2") else { return e.ts.remove() };
    let Some((smiley, _)) = song(&e, "Gnarls Barkley/St. Elsewhere/03 Smiley Faces.mp3", "Smiley Faces", "3") else { return e.ts.remove() };
    let tagged = |title: &'static str| vec![("title", title), ("artist", "Gnarls Barkley"), ("album", "St. Elsewhere")];
    let (Some(a), Some(b), Some(c)) = (upload_bytes(&e, "a.flac", 2, &tagged("Crazy")), upload_bytes(&e, "b.flac", 2, &tagged("Smiley Faces")),
        upload_bytes(&e, "c.flac", 2, &[("title", "Crazy (Live)"), ("artist", "Gnarls Barkley")])) else { return e.ts.remove() };
    let r = upload(&e, &e.admin, "/api/library/own-files", &[("a.flac", a), ("b.flac", b), ("c.flac", c), ("readme.txt", b"hi".to_vec())]).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let v = r.json();
    let files = v["files"].as_array().unwrap();
    assert_eq!(files.len(), 3);
    assert_eq!(v["failed"][0]["name"], "readme.txt");
    let by_name = |n: &str| files.iter().find(|f| f["name"] == n).unwrap().clone();
    assert_eq!(by_name("a.flac")["match"]["track"]["id"].as_i64(), Some(crazy));
    assert_eq!(by_name("b.flac")["match"]["track"]["id"].as_i64(), Some(smiley));
    // a live recording is never the studio song
    assert!(by_name("c.flac")["match"].is_null(), "{}", by_name("c.flac"));

    // someone else can't use these uploads
    db_perm(&e);
    let items = json!({ "items": [{ "token": by_name("a.flac")["token"], "track_id": crazy }] });
    let r = send(&e.app, "POST", "/api/library/own-files/apply", &e.listener, Some(items), &[]).await;
    assert_eq!(r.json()["failed"][0]["reason"], "That upload is someone else's");
    // one song can't take two files
    let twice = json!({ "items": [{ "token": by_name("a.flac")["token"], "track_id": crazy }, { "token": by_name("b.flac")["token"], "track_id": crazy }] });
    assert_eq!(send(&e.app, "POST", "/api/library/own-files/apply", &e.admin, Some(twice), &[]).await.status, StatusCode::BAD_REQUEST);

    let items = json!({ "items": [{ "token": by_name("a.flac")["token"], "track_id": crazy }, { "token": by_name("b.flac")["token"], "track_id": smiley }] });
    let r = send(&e.app, "POST", "/api/library/own-files/apply", &e.admin, Some(items), &[]).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(r.json()["replaced"].as_array().unwrap().len(), 2, "{}", r.text());
    for id in [crazy, smiley] {
        assert_eq!(e.st().db.one("SELECT format FROM tracks WHERE id=?", p![id]).unwrap().unwrap().s("format"), "flac");
    }
    // a used upload is gone; the unused one is dropped on request
    let r = send(&e.app, "DELETE", &format!("/api/library/own-files/{}", by_name("c.flac")["token"].as_str().unwrap()), &e.admin, None, &[]).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(std::fs::read_dir(e.st().data_dir().join("own-files")).unwrap().count(), 0);
    e.ts.remove();
}

/// Gives the listener of `e` the permission.
fn db_perm(e: &Env) {
    e.st().db.execute("UPDATE users SET perms=? WHERE id=?", p![json!({ "own_files": true }).to_string(), e.listener_id]).unwrap();
}

#[test]
fn values_of_a_song_for_its_new_file() {
    let mut row = encore_db::Row::new();
    for (k, v) in [("title", json!("Crazy")), ("artist", json!("Gnarls Barkley")), ("track", json!(2)), ("disc", json!(0)), ("year", json!("2006")),
                   ("artist_credit", json!(r#"[{"name":"CeeLo Green"},{"name":"Danger Mouse"}]"#)), ("compilation", json!(0)), ("isrc", json!(""))] {
        row.insert(k.into(), v);
    }
    let v: Value = Value::Object(encore_library::own_file::tag_values(&row));
    assert_eq!(v["title"], "Crazy");
    assert_eq!(v["tracknumber"], 2);
    assert_eq!(v["date"], "2006");
    assert_eq!(v["artists"], json!(["CeeLo Green", "Danger Mouse"]));
    // nothing empty is written over what the old file said
    assert!(v.get("discnumber").is_none() && v.get("isrc").is_none() && v.get("compilation").is_none());
}
