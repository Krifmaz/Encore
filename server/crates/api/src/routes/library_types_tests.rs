//! Library types, bulk delete to Trash, One-offs and who added it (0.36, docs/contracts/library-types.md §9.3)
//! through the real router: the album lists by type, the Type counts, the album page's type and history, Change
//! type, the Trash (dry run, real run, restore), a song's "added" line, the One-offs shelf, who may fix their
//! songs after Clear finished, and no MusicBrainz call on any of these.
use crate::test_support::{call, send, user, Resp, TestServer};
use axum::Router;
use encore_db::{now, p, RowExt};
use serde_json::{json, Value};
use encore_http::Ctx as _;
use std::path::{Path, PathBuf};

struct Env {
    ts: TestServer,
    app: Router,
    lib: PathBuf,
    admin: i64,
    admin_name: String,
    cookie: String,
    member: i64,
    member_cookie: String,
    next: i64,
}

async fn login(app: &Router, name: &str) -> String {
    let (code, _, cookie) = call(app, "POST", "/api/auth/login", "", Some(json!({ "username": name, "password": "a-long-password-1" }))).await;
    assert_eq!(code, axum::http::StatusCode::OK);
    cookie.split(';').next().unwrap_or("").to_string()
}

/// A server with a library folder, an admin and a member signed in; MusicBrainz at a closed port.
async fn env_at(name: &str, lib: Option<PathBuf>) -> Env {
    let ts = TestServer::new(&format!("libtypes-{name}"));
    let lib = lib.unwrap_or_else(|| ts.dir.join("music"));
    std::fs::create_dir_all(&lib).unwrap();
    let lib = std::fs::canonicalize(&lib).unwrap();
    let staging = ts.dir.join("staging");
    std::fs::create_dir_all(&staging).unwrap();
    ts.st.cfg.set_many(&json!({ "library_path": lib.to_string_lossy(), "staging_path": staging.to_string_lossy(), "musicbrainz_url": "http://127.0.0.1:9" }));
    let admin_name = format!("boss-{name}");
    let admin = user(&ts.st, &admin_name, "admin").i("id");
    let member = user(&ts.st, &format!("fan-{name}"), "user").i("id");
    let app = ts.app();
    let cookie = login(&app, &admin_name).await;
    let member_cookie = login(&app, &format!("fan-{name}")).await;
    Env { ts, app, lib, admin, admin_name, cookie, member, member_cookie, next: 1 }
}

async fn env(name: &str) -> Env {
    env_at(name, None).await
}

impl Env {
    fn st(&self) -> &crate::state::AppState {
        &self.ts.st
    }
    async fn get(&self, uri: &str) -> Resp {
        send(&self.app, "GET", uri, &self.cookie, None, &[]).await
    }
    async fn as_member(&self, method: &str, uri: &str, body: Option<Value>) -> Resp {
        send(&self.app, method, uri, &self.member_cookie, body, &[]).await
    }
    async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> Resp {
        send(&self.app, method, uri, &self.cookie, body, &[]).await
    }
    async fn trash(&self, body: Value) -> Resp {
        self.send("POST", "/api/library/albums/trash", Some(body)).await
    }
    async fn added(&self, id: i64) -> Value {
        self.get(&format!("/api/library/tracks/{id}/added")).await.json()
    }
    /// One song: a file at `lib/<artist>/<album>/<title>.flac` (a few bytes; its row says `size`) and its row, as a
    /// scan of that file would make it (the folder names are its artist and album). → its id
    fn song_in(&mut self, base: &Path, artist: &str, album: &str, title: &str, track: i64, size: i64) -> i64 {
        let dir = base.join(artist).join(album);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{title}.flac"));
        std::fs::write(&path, b"x").unwrap();
        let id = self.next;
        self.next += 1;
        self.st().db.execute(
            "INSERT INTO tracks (id, path, album_id, title, artist, album, albumartist, track, disc, size, duration, format, added_at, added_via)
             VALUES (?,?,?,?,?,?,?,?,1,?,180,'flac',?,'scan')",
            p![id, path.to_string_lossy(), encore_library::album_key(artist, album, ""), title, artist, album, artist, track, size, now()]).unwrap();
        id
    }
    fn song(&mut self, artist: &str, album: &str, title: &str, track: i64, size: i64) -> i64 {
        let lib = self.lib.clone();
        self.song_in(&lib, artist, album, title, track, size)
    }
    /// An album of `n` songs ("Song 1" …). → its id
    fn album(&mut self, artist: &str, album: &str, n: i64) -> String {
        for i in 1..=n {
            self.song(artist, album, &format!("Song {i}"), i, 1000);
        }
        encore_library::album_key(artist, album, "")
    }
    fn done(self) {
        self.ts.remove();
    }
}

fn ids(v: &Value) -> Vec<String> {
    v["items"].as_array().unwrap().iter().map(|i| i["id"].as_str().unwrap().to_string()).collect()
}

/// The lists by type: every item carries its filter key; `kind=albums` is format Album, `kind=singles` Singles &
/// EPs, no kind leaves the One-offs out and a search finds them; `type=` alone decides; `ids_only` gives the
/// whole filtered list; the side index with `type=` lands on the same albums.
#[tokio::test]
async fn album_lists_by_type() {
    let mut e = env("lists").await;
    let studio = e.album("Studio Artist", "Rodeo", 10);
    let live = e.album("Live Band", "Live at Wembley", 10);
    e.song("Various Artists", "Now 99", "Hit", 7, 1000);
    let one_off = encore_library::album_key("Various Artists", "Now 99", "");
    e.song("Solo", "Song", "Song", 1, 1000);
    let single = encore_library::album_key("Solo", "Song", "");
    let ep = e.album("Quad", "Four - EP", 4);
    let all = e.get("/api/library/albums?limit=500").await.json();
    assert_eq!(all["total"], 4, "{all}");
    assert!(!ids(&all).contains(&one_off), "no kind: no One-offs");
    let type_of = |id: &str| all["items"].as_array().unwrap().iter().find(|i| i["id"] == id).map(|i| i["type"].clone()).unwrap();
    assert_eq!((type_of(&studio), type_of(&live), type_of(&single), type_of(&ep)), (json!("album"), json!("live"), json!("singles"), json!("singles")));
    let mut got = ids(&e.get("/api/library/albums?kind=albums&limit=500").await.json());
    got.sort();
    let mut want = vec![studio.clone(), live.clone()];
    want.sort();
    assert_eq!(got, want, "kind=albums: format Album of any kind");
    let mut got = ids(&e.get("/api/library/albums?kind=singles").await.json());
    got.sort();
    let mut want = vec![single.clone(), ep.clone()];
    want.sort();
    assert_eq!(got, want);
    assert_eq!(ids(&e.get("/api/library/albums?q=now").await.json()), vec![one_off.clone()], "a search finds One-offs");
    assert_eq!(ids(&e.get("/api/library/albums?type=live&kind=singles").await.json()), vec![live.clone()], "type alone decides");
    assert_eq!(ids(&e.get("/api/library/albums?type=one_off").await.json()), vec![one_off.clone()]);
    assert_eq!(e.get("/api/library/albums?type=singles").await.json()["total"], 2);
    assert_eq!(ids(&e.get("/api/library/albums?type=ep").await.json()), vec![ep.clone()]);
    let bad = e.get("/api/library/albums?type=bogus").await;
    assert_eq!(bad.status, 400);
    assert_eq!(bad.json()["detail"], "Unknown album type");
    let only = e.get("/api/library/albums?ids_only=1&limit=1").await.json();
    assert_eq!((only["total"].clone(), only["ids"].as_array().unwrap().len(), only["capped"].clone()), (json!(4), 4, json!(false)));
    assert_eq!(e.get("/api/library/albums?ids_only=1&type=one_off").await.json()["ids"], json!([one_off]));
    // the side index with a type: offsets on the same list
    let idx = e.get("/api/library/index?kind=albums&type=singles&sort=artist&dir=asc").await.json();
    let list = e.get("/api/library/albums?type=singles&sort=artist&dir=asc").await.json();
    assert_eq!(idx, json!([{ "label": "Q", "offset": 0 }, { "label": "S", "offset": 1 }]), "{idx}");
    assert_eq!(list["items"][1]["albumartist"], "Solo");
    assert_eq!(e.get("/api/library/index?kind=albums&type=bogus").await.status, 400);
    e.done();
}

/// GET /api/library/album-types: every key in its order with its label and count (0 included), over every album.
#[tokio::test]
async fn album_type_counts() {
    let mut e = env("counts").await;
    e.album("Studio Artist", "Rodeo", 10);
    e.album("Live Band", "Live at Wembley", 10);
    e.song("Various Artists", "Now 99", "Hit", 7, 1000);
    e.song("Solo", "Song", "Song", 1, 1000);
    let v = e.get("/api/library/album-types").await.json();
    assert_eq!(v["total"], 4);
    let keys: Vec<&str> = v["types"].as_array().unwrap().iter().map(|t| t["key"].as_str().unwrap()).collect();
    assert_eq!(keys, ["album", "live", "compilation", "remix", "soundtrack", "demo", "dj_mix", "mixtape", "karaoke", "singles", "one_off", "other"]);
    let count = |k: &str| v["types"].as_array().unwrap().iter().find(|t| t["key"] == k).unwrap()["count"].as_i64().unwrap();
    assert_eq!((count("album"), count("live"), count("singles"), count("one_off"), count("remix")), (1, 1, 1, 1, 0));
    assert_eq!(v["types"].as_array().unwrap().iter().map(|t| t["count"].as_i64().unwrap()).sum::<i64>(), 4);
    assert_eq!(v["types"][9]["label"], "Singles & EPs");
    assert_eq!(v["types"][10]["label"], "One-offs");
    assert_eq!(v["types"][0]["label"], "Studio albums");
    // an artist's
    let a = e.get("/api/library/album-types?artist=Live%20Band").await.json();
    assert_eq!((a["total"].clone(), a["types"][1]["count"].clone()), (json!(1), json!(1)));
    e.done();
}

/// The album page: its type, the history line's groups (two requesters and an upload: three, most songs
/// first), each song's adder – and "requested by" still there after Clear finished deleted the requests.
#[tokio::test]
async fn album_page_type_history_and_who_added() {
    let mut e = env("history").await;
    let alex = user(e.st(), "alex", "user").i("id");
    let sam = user(e.st(), "sam", "user").i("id");
    let album = e.album("Mufasa Cast", "Mufasa", 5);
    let db = &e.ts.st.db;
    let ra = db.execute("INSERT INTO requests (user_id, kind, status, created_at, updated_at) VALUES (?, 'album', 'completed', 1000, 1000)", p![alex]).unwrap();
    let rs = db.execute("INSERT INTO requests (user_id, kind, status, created_at, updated_at) VALUES (?, 'track', 'completed', 2000, 2000)", p![sam]).unwrap();
    db.execute("UPDATE tracks SET request_id=?, added_by=?, added_via='request', requested_at=1000 WHERE id IN (1, 2)", p![ra, alex]).unwrap();
    db.execute("UPDATE tracks SET request_id=?, added_by=?, added_via='link', requested_at=2000 WHERE id=3", p![rs, sam]).unwrap();
    db.execute("UPDATE tracks SET added_by=?, added_via='upload' WHERE id=4", p![e.admin]).unwrap();
    db.execute("UPDATE tracks SET added_by=9999, added_via='import' WHERE id=5", p![]).unwrap();
    let v = e.get(&format!("/api/library/albums/{album}")).await.json();
    assert_eq!(v["type"]["key"], "album");
    assert_eq!(v["type"]["label"], "Album");
    assert_eq!(v["type"]["chosen"], Value::Null);
    let h = v["history"].as_array().unwrap();
    assert_eq!(h.len(), 4, "{h:?}");
    assert_eq!((h[0]["via"].clone(), h[0]["by"].clone(), h[0]["songs"].clone(), h[0]["requested_at"].clone()), (json!("request"), json!("alex"), json!(2), json!(1000.0)));
    assert_eq!((h[1]["via"].clone(), h[1]["by"].clone(), h[1]["requested_at"].clone()), (json!("link"), json!("sam"), json!(2000.0)));
    let rest: Vec<(Value, Value, Value)> = h[2..].iter().map(|g| (g["via"].clone(), g["by"].clone(), g["by_deleted"].clone())).collect();
    assert!(rest.contains(&(json!("upload"), json!(e.admin_name), json!(false))), "{rest:?}");
    assert!(rest.contains(&(json!("import"), Value::Null, json!(true))), "{rest:?}");
    assert!(v["added_at"].as_f64().is_some());
    let t0 = &v["tracks"][0];
    assert_eq!((t0["added_via"].clone(), t0["added_by_name"].clone(), t0["requested_by"].clone(), t0["requested_at"].clone()), (json!("request"), json!("alex"), json!("alex"), json!(1000.0)));
    assert!(t0.get("releasetype").is_none(), "the type tags stay on the server");
    assert_eq!(v["tracks"][4]["added_by_deleted"], true);
    // Clear finished deletes the request rows: the names stay (the owner's Mufasa case)
    let r = e.send("POST", "/api/requests/clear", Some(json!({ "statuses": ["completed"] }))).await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(e.st().db.one("SELECT COUNT(*) n FROM requests", p![]).unwrap().unwrap().i("n"), 0);
    let v = e.get(&format!("/api/library/albums/{album}")).await.json();
    assert_eq!((v["tracks"][0]["requested_by"].clone(), v["tracks"][0]["requested_at"].clone()), (json!("alex"), json!(1000.0)));
    assert_eq!(v["tracks"][2]["requested_by"], "sam");
    assert_eq!(v["history"][0]["by"], "alex");
    e.done();
}

/// PUT …/type: admins only, a known format or null, an album the library has; the choice shows in the lists and
/// the page, null is automatic again, and a full rescan keeps it.
#[tokio::test]
async fn change_type() {
    let mut e = env("change").await;
    e.song("Various Artists", "Now 99", "Hit", 7, 1000);
    let id = encore_library::album_key("Various Artists", "Now 99", "");
    let uri = format!("/api/library/albums/{id}/type");
    assert_eq!(e.as_member("PUT", &uri, Some(json!({ "format": "album" }))).await.status, 403);
    let bad = e.send("PUT", &uri, Some(json!({ "format": "lp" }))).await;
    assert_eq!((bad.status.as_u16(), bad.json()["detail"].clone()), (400, json!("Pick Album, EP, Single or One-off")));
    assert_eq!(e.send("PUT", "/api/library/albums/nothing/type", Some(json!({ "format": "album" }))).await.status, 404);
    let r = e.send("PUT", &uri, Some(json!({ "format": "album" }))).await;
    assert_eq!(r.status, 200, "{}", r.text());
    let t = r.json();
    assert_eq!((t["format"].clone(), t["kind"].clone(), t["key"].clone(), t["chosen"].clone(), t["why_text"].clone()),
               (json!("album"), json!("compilation"), json!("compilation"), json!("album"), json!("You chose this")));
    assert_eq!(t["auto"]["label"], "Compilation · One-off");
    assert_eq!(t["auto"]["why_text"], "Only 1 of its 7 songs are here");
    assert_eq!(ids(&e.get("/api/library/albums?kind=albums").await.json()), vec![id.clone()], "in the Albums grid now");
    assert_eq!(e.get(&format!("/api/library/albums/{id}")).await.json()["type"]["label"], "Compilation");
    // a full rescan rewrites the rows, never the choice
    let lib = e.lib.to_string_lossy().to_string();
    e.st().library.scan_sync(&e.st().db, &lib, true);
    assert_eq!(e.get(&format!("/api/library/albums/{id}")).await.json()["type"]["chosen"], "album");
    let r = e.send("PUT", &uri, Some(json!({ "format": null }))).await.json();
    assert_eq!((r["chosen"].clone(), r["format"].clone()), (Value::Null, json!("one_off")));
    assert!(ids(&e.get("/api/library/albums").await.json()).is_empty());
    e.done();
}

/// POST /api/library/albums/trash: admins only, 1–100 albums a real run (5000 a dry run); the dry run counts
/// and touches nothing; a real run moves an album's own folder as one item (its cover with it), a shared
/// folder's songs one by one with their lyrics, songs in an extra library folder one by one, and leaves albums
/// outside every root; rows go, playlist entries stay as missing; Trash lists the items; Restore brings the
/// album back with its id, its type choice and its "Requested by"; DELETE …/albums/{id} lands in the Trash too.
#[tokio::test]
async fn albums_to_the_trash_and_back() {
    let mut e = env("trash").await;
    let alex = user(e.st(), "alex", "user").i("id");
    // an album with a folder of its own and a cover
    let x1 = e.song("Ex Artist", "Exclusive", "One", 1, 524_288);
    e.song("Ex Artist", "Exclusive", "Two", 2, 524_288);
    let ex = encore_library::album_key("Ex Artist", "Exclusive", "");
    std::fs::write(e.lib.join("Ex Artist/Exclusive/cover.jpg"), b"jpg").unwrap();
    e.st().db.execute("UPDATE tracks SET added_by=?, added_via='request', requested_at=1000 WHERE album_id=?", p![alex, ex]).unwrap();
    // two albums sharing a folder (only one goes); its song has lyrics beside it
    let y1 = e.song("Shared", "Mixed", "Yes", 1, 524_288);
    let shared_dir = e.lib.join("Shared/Mixed");
    std::fs::write(shared_dir.join("Yes.lrc"), b"[00:01]la").unwrap();
    let yid = "shared-y".to_string();
    e.st().db.execute("UPDATE tracks SET album_id=?, album='Y' WHERE id=?", p![yid, y1]).unwrap();
    let z1 = e.song("Shared", "Mixed", "Zed", 2, 1000);
    // an album kept where it was downloaded: its folder is an extra library folder (in Soulseek's folder)
    let slsk = e.ts.dir.join("slskd");
    std::fs::create_dir_all(slsk.join("Kept/Here")).unwrap();
    let slsk = std::fs::canonicalize(&slsk).unwrap();
    let kept = slsk.join("Kept/Here");
    e.st().cfg.set_many(&json!({ "slskd_download_path": slsk.to_string_lossy(), "library_extra_roots": kept.to_string_lossy() }));
    let k1 = e.song_in(&slsk, "Kept", "Here", "K", 1, 1000);
    let kid = encore_library::album_key("Kept", "Here", "");
    // an album whose files are outside every folder Encore manages
    e.st().db.execute("INSERT INTO tracks (path, album_id, album, title, size, added_via) VALUES ('/nowhere-encore/a.flac', 'outside', 'Far', 'A', 5, 'scan')", p![]).unwrap();
    // a liked song and a type choice
    let pid = e.st().db.execute("INSERT INTO playlists (user_id, name, kind, source, sync_targets, created_at, updated_at) VALUES (?, 'Mix', 'playlist', 'manual', '[]', 1, 1)", p![e.admin]).unwrap();
    let item = e.st().db.execute("INSERT INTO playlist_items (playlist_id, position, title, track_id, added_at) VALUES (?, 0, 'One', ?, 1)", p![pid, x1]).unwrap();
    assert_eq!(e.send("PUT", &format!("/api/library/albums/{ex}/type"), Some(json!({ "format": "ep" }))).await.status, 200);

    assert_eq!(e.as_member("POST", "/api/library/albums/trash", Some(json!({ "ids": [ex], "dry_run": true }))).await.status, 403);
    let r = e.trash(json!({ "ids": [], "dry_run": true })).await;
    assert_eq!((r.status.as_u16(), r.json()["detail"].clone()), (400, json!("Pick at least one album")));
    let many: Vec<String> = (0..101).map(|i| format!("a{i}")).collect();
    let r = e.trash(json!({ "ids": many, "dry_run": false })).await;
    assert_eq!((r.status.as_u16(), r.json()["detail"].clone()), (400, json!("At most 100 albums at a time")));
    let lots: Vec<String> = (0..5001).map(|i| format!("a{i}")).collect();
    assert_eq!(e.trash(json!({ "ids": lots, "dry_run": true })).await.status, 400);
    // the dry run counts and touches nothing
    let d = e.trash(json!({ "ids": [ex, yid], "dry_run": true })).await.json();
    assert_eq!((d["albums"].clone(), d["songs"].clone(), d["bytes"].clone(), d["text"].clone()), (json!(2), json!(3), json!(1_572_864), json!("Delete 2 albums (1.5 MB)?")));
    assert_eq!((d["skipped"].clone(), d["skipped_text"].clone(), d["missing"].clone()), (json!([]), json!(""), json!([])));
    assert_eq!(e.trash(json!({ "ids": [ex], "dry_run": true })).await.json()["text"], "Delete 1 album (1.0 MB)?");
    let d = e.trash(json!({ "ids": [ex, "outside", "unknown"], "dry_run": true })).await.json();
    assert_eq!(d["missing"], json!(["unknown"]));
    assert_eq!(d["skipped_text"], "1 of them can't be moved to the Trash and stays: Its files are outside the folders Encore manages");
    assert_eq!(e.st().db.one("SELECT COUNT(*) n FROM tracks", p![]).unwrap().unwrap().i("n"), 6);
    assert!(e.lib.join("Ex Artist/Exclusive/One.flac").exists());
    // the real run
    let r = e.trash(json!({ "ids": [ex, yid, kid, "outside"], "dry_run": false })).await;
    assert_eq!(r.status, 200, "{}", r.text());
    let v = r.json();
    assert_eq!((v["albums"].clone(), v["songs"].clone(), v["text"].clone()), (json!(3), json!(4), json!("Moved 3 albums to the Trash")), "{v}");
    assert_eq!(v["failed"], json!([{ "id": "outside", "album": "Far", "reason": "Its files are outside the folders Encore manages" }]));
    assert_eq!(v["items"].as_array().unwrap().len(), 3, "the exclusive folder once, the shared song, the kept song: {v}");
    assert!(!e.lib.join("Ex Artist/Exclusive").exists(), "the album's own folder went whole");
    assert!(!shared_dir.join("Yes.flac").exists() && !shared_dir.join("Yes.lrc").exists(), "the song and its lyrics went");
    assert!(shared_dir.join("Zed.flac").exists(), "the other album's song stays");
    assert!(!kept.join("K.flac").exists() && kept.is_dir(), "the song went, the extra library folder stays");
    let left: Vec<i64> = e.st().db.query("SELECT id FROM tracks ORDER BY id", p![]).unwrap().iter().map(|r| r.i("id")).collect();
    assert!(left.contains(&z1) && !left.contains(&x1) && !left.contains(&k1), "{left:?}");
    assert!(e.st().db.one("SELECT track_id FROM playlist_items WHERE id=?", p![item]).unwrap().unwrap().get("track_id").unwrap().is_null());
    // the Trash lists them, by whom
    let list = e.get("/api/files/trash").await.json();
    let items = list.as_array().unwrap();
    assert_eq!(items.len(), 3);
    assert!(items.iter().all(|i| i["by"] == json!(e.admin_name)));
    let folder = items.iter().find(|i| i["kind"] == "dir").unwrap();
    assert_eq!(folder["name"], "Exclusive");
    let item_dir = encore_files::trash::dir_of(e.st()).join(folder["id"].as_str().unwrap());
    assert!(item_dir.join("Exclusive/cover.jpg").exists(), "the cover went with the folder");
    let lyric_item = items.iter().find(|i| i["name"] == "Yes.flac").unwrap();
    assert!(encore_files::trash::dir_of(e.st()).join(lyric_item["id"].as_str().unwrap()).join("Yes.lrc").exists(), "the lyrics are in the song's item");
    // Restore: the album comes back with its id, its choice and who asked for it; the lyrics come back too
    let r = e.send("POST", "/api/files/restore", Some(json!({ "items": [folder["id"], lyric_item["id"]] }))).await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json()["restored"], 2, "{}", r.text());
    assert!(shared_dir.join("Yes.lrc").exists(), "the lyrics are back beside the song");
    let page = e.get(&format!("/api/library/albums/{ex}")).await.json();
    assert_eq!(page["tracks"].as_array().map(|a| a.len()), Some(2), "{page}");
    assert_eq!(page["type"]["chosen"], "ep");
    assert_eq!(page["tracks"][0]["requested_by"], "alex");
    assert_eq!(page["tracks"][0]["id"], x1, "the old id");
    assert_eq!(e.st().db.one("SELECT track_id FROM playlist_items WHERE id=?", p![item]).unwrap().unwrap().i("track_id"), x1);
    // DELETE …/albums/{id}: to the Trash as well
    let r = e.send("DELETE", &format!("/api/library/albums/{ex}"), None).await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!((r.json()["ok"].clone(), r.json()["text"].clone()), (json!(true), json!("Moved 1 album to the Trash")));
    assert_eq!(e.get("/api/files/trash").await.json().as_array().unwrap().len(), 2);
    assert_eq!(e.send("DELETE", "/api/library/albums/nothing", None).await.status, 404);
    assert!(e.st().db.query("SELECT message FROM activity WHERE message LIKE '%moved 3 albums to the Trash (4 songs, %'", p![]).unwrap().len() == 1);
    e.done();
}

/// An album with a song whose file is gone already goes all the same (its other files to the Trash, the gone
/// song's row with them), whole folder or song by song; an album none of whose files is there stays as it was
/// and says why, and a DELETE of it answers 409 with that reason.
#[tokio::test]
async fn an_album_missing_a_file_goes_and_one_missing_all_stays() {
    let mut e = env("gaps").await;
    // its own folder, one of three files gone
    let gap = e.album("Gap Artist", "Gap", 3);
    std::fs::remove_file(e.lib.join("Gap Artist/Gap/Song 2.flac")).unwrap();
    // sharing a folder with another album (so song by song), one of two files gone
    let s1 = e.song("Shared", "Both", "Here", 1, 1000);
    let s2 = e.song("Shared", "Both", "Gone", 2, 1000);
    e.st().db.execute("UPDATE tracks SET album_id='half', album='Half' WHERE id IN (?, ?)", p![s1, s2]).unwrap();
    std::fs::remove_file(e.lib.join("Shared/Both/Gone.flac")).unwrap();
    let other = e.song("Shared", "Both", "Other", 3, 1000);
    // every file gone (a disk that isn't mounted looks the same)
    let none = e.album("None Artist", "None", 2);
    for i in 1..=2 {
        std::fs::remove_file(e.lib.join(format!("None Artist/None/Song {i}.flac"))).unwrap();
    }
    let r = e.trash(json!({ "ids": [gap, "half", none], "dry_run": false })).await;
    assert_eq!(r.status, 200, "{}", r.text());
    let v = r.json();
    assert_eq!((v["albums"].clone(), v["text"].clone()), (json!(2), json!("Moved 2 albums to the Trash")), "{v}");
    assert_eq!(v["failed"], json!([{ "id": none, "album": "None", "reason": encore_files::albums::FILE_MISSING }]));
    assert_eq!(v["items"].as_array().unwrap().len(), 2, "Gap's folder once, Half's one song there: {v}");
    assert!(!e.lib.join("Gap Artist/Gap").exists(), "the folder went whole");
    assert!(!e.lib.join("Shared/Both/Here.flac").exists() && e.lib.join("Shared/Both/Other.flac").exists());
    let left = |album: &str| e.st().db.one("SELECT COUNT(*) n FROM tracks WHERE album_id=?", p![album]).unwrap().unwrap().i("n");
    assert_eq!((left(&gap), left("half"), left(&none)), (0, 0, 2), "the gone songs' rows left with their albums; None's stay");
    assert!(e.st().db.one("SELECT 1 x FROM tracks WHERE id=?", p![other]).unwrap().is_some());
    let r = e.send("DELETE", &format!("/api/library/albums/{none}"), None).await;
    assert_eq!((r.status.as_u16(), r.json()["detail"].clone()), (409, json!(encore_files::albums::FILE_MISSING)));
    assert_eq!(left(&none), 2);
    e.done();
}

/// A Trash on another disk needs room for a copy: the dry run says so, a real run is refused with 507.
#[tokio::test]
async fn the_trash_needs_room_on_another_disk() {
    let shm = PathBuf::from("/dev/shm");
    let probe = std::env::temp_dir();
    use std::os::unix::fs::MetadataExt;
    let (Ok(a), Ok(b)) = (std::fs::metadata(&shm), std::fs::metadata(&probe)) else { return };
    if a.dev() == b.dev() {
        eprintln!("/dev/shm is on the temp folder's disk here: skipped");
        return;
    }
    let lib = shm.join(format!("encore-libtypes-room-{}", std::process::id()));
    let mut e = env_at("room", Some(lib.clone())).await;
    e.song("Big", "Huge", "One", 1, 10 << 40);
    let id = encore_library::album_key("Big", "Huge", "");
    let d = e.send("POST", "/api/library/albums/trash", Some(json!({ "ids": [id], "dry_run": true }))).await.json();
    assert_eq!((d["other_disk"].clone(), d["room"].clone()), (json!(true), json!(false)), "{d}");
    assert!(d["room_text"].as_str().unwrap().starts_with("There isn't room in the Trash for 10.0 TB ("), "{d}");
    assert!(d["room_text"].as_str().unwrap().ends_with(" free on its disk). Empty the Trash first, or delete fewer albums."));
    let r = e.send("POST", "/api/library/albums/trash", Some(json!({ "ids": [id], "dry_run": false }))).await;
    assert_eq!(r.status, 507);
    assert!(lib.join("Big/Huge/One.flac").exists());
    e.done();
    let _ = std::fs::remove_dir_all(&lib);
}

/// GET /api/library/tracks/{id}/added: every way a song comes in, a deleted user, a cleared request, a surround
/// song, an unknown id.
#[tokio::test]
async fn who_added_a_song() {
    let mut e = env("added").await;
    let alex = user(e.st(), "alex", "user").i("id");
    let rows = [("request", Some(alex), Some(1000.0)), ("link", Some(alex), None), ("soulseek", Some(9999), Some(5.0)), ("request", None, None),
                ("upload", Some(e.admin), None), ("import", Some(9999), None), ("scan", None, None)];
    let mut ids = vec![];
    for (i, (via, by, at)) in rows.iter().enumerate() {
        let id = e.song("A", "B", &format!("S{i}"), i as i64 + 1, 1);
        e.st().db.execute("UPDATE tracks SET added_via=?, added_by=?, requested_at=? WHERE id=?", p![via, by, at, id]).unwrap();
        ids.push(id);
    }
    let v = e.added(ids[0]).await;
    assert_eq!((v["via"].clone(), v["by"].clone(), v["by_id"].clone(), v["by_deleted"].clone(), v["requested_at"].clone()), (json!("request"), json!("alex"), json!(alex), json!(false), json!(1000.0)));
    assert!(v["added_at"].as_f64().is_some());
    assert_eq!(e.added(ids[1]).await["requested_at"], Value::Null);
    let v = e.added(ids[2]).await;
    assert_eq!((v["via"].clone(), v["by"].clone(), v["by_deleted"].clone()), (json!("soulseek"), Value::Null, json!(true)));
    let v = e.added(ids[3]).await;
    assert_eq!((v["by"].clone(), v["by_id"].clone(), v["by_deleted"].clone()), (Value::Null, Value::Null, json!(false)), "a cleared request");
    assert_eq!(e.added(ids[4]).await["by"], json!(e.admin_name));
    assert_eq!(e.added(ids[5]).await["by_deleted"], true);
    assert_eq!(e.added(ids[6]).await["via"], "scan");
    // a member may ask too
    assert_eq!(e.as_member("GET", &format!("/api/library/tracks/{}/added", ids[0]), None).await.json()["by"], "alex");
    let sid = e.st().db.execute("INSERT INTO surround_tracks (path, title, added_via, added_at) VALUES ('/m/s.m4a', 'S', 'scan', 1)", p![]).unwrap();
    assert_eq!(e.added(sid).await["via"], "scan");
    let r = e.get("/api/library/tracks/424242/added").await;
    assert_eq!((r.status.as_u16(), r.json()["detail"].clone()), (404, json!("Track not found")));
    e.done();
}

/// GET /api/library/artists/{name}/one-offs: only the artist's songs on One-off albums, newest first, with
/// their release label; `mbid=` finds songs credited to it; paging.
#[tokio::test]
async fn one_offs_shelf() {
    let mut e = env("oneoffs").await;
    e.album("Solo Star", "Own Album", 10);
    let a = e.song("Various Artists", "Now 99", "Shine", 7, 1);
    let b = e.song("Various Artists", "Now 100", "Glow", 3, 1);
    e.song("Various Artists", "Now 100", "Other Artist Song", 4, 1);
    e.st().db.execute("UPDATE tracks SET artist='Solo Star', mb_artistid='11111111-2222-3333-4444-555555555555', added_at=added_at + id WHERE id IN (?, ?)", p![a, b]).unwrap();
    let v = e.get("/api/library/artists/Solo%20Star/one-offs").await.json();
    assert_eq!(v["total"], 2, "{v}");
    let titles: Vec<&str> = v["tracks"].as_array().unwrap().iter().map(|t| t["title"].as_str().unwrap()).collect();
    assert_eq!(titles, ["Glow", "Shine"], "newest first");
    assert_eq!(v["tracks"][0]["release_label"], "Various artists");
    let page = e.get("/api/library/artists/_/one-offs?name=Solo%20Star&limit=1&offset=1").await.json();
    assert_eq!((page["total"].clone(), page["offset"].clone(), page["tracks"].as_array().unwrap().len()), (json!(2), json!(1), 1));
    let by_id = e.get("/api/library/artists/Someone%20Else/one-offs?mbid=11111111-2222-3333-4444-555555555555").await.json();
    assert_eq!(by_id["total"], 2);
    assert_eq!(e.get("/api/library/artists/Nobody/one-offs").await.json()["total"], 0);
    e.done();
}

/// A member with "fix" may still confirm or remove a song they requested after Clear finished deleted the
/// request; another member may not.
#[tokio::test]
async fn owners_keep_fixing_their_songs() {
    let mut e = env("owns").await;
    let id = e.song("A", "B", "Mine", 1, 1);
    let other = e.song("A", "B", "Theirs", 2, 1);
    e.st().db.execute("UPDATE tracks SET added_by=?, added_via='request' WHERE id=?", p![e.member, id]).unwrap();
    e.st().db.execute("UPDATE tracks SET added_by=?, added_via='request' WHERE id=?", p![e.admin, other]).unwrap();
    let r = e.as_member("POST", "/api/library/verify-manual", Some(json!({ "track_ids": [id] }))).await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(e.st().db.one("SELECT verify_status FROM tracks WHERE id=?", p![id]).unwrap().unwrap().s("verify_status"), "manual");
    assert_eq!(e.as_member("POST", "/api/library/verify-manual", Some(json!({ "track_ids": [other] }))).await.status, 403);
    assert_eq!(e.as_member("DELETE", &format!("/api/library/tracks/{other}"), None).await.status, 403);
    // an upload is not a request: no "fix" right from it
    e.st().db.execute("UPDATE tracks SET added_via='upload' WHERE id=?", p![id]).unwrap();
    assert_eq!(e.as_member("POST", "/api/library/verify-manual", Some(json!({ "track_ids": [id] }))).await.status, 403);
    e.st().db.execute("UPDATE tracks SET added_via='soulseek' WHERE id=?", p![id]).unwrap();
    assert_eq!(e.as_member("DELETE", &format!("/api/library/tracks/{id}"), None).await.status, 200);
    e.done();
}

/// None of the library-type answers asks MusicBrainz (its address here is a closed port): the lists, the album
/// page, the counts, Change type, the One-offs, the Trash's dry run and a song's "added" line.
#[tokio::test]
async fn no_musicbrainz_on_requests() {
    let mut e = env("nomb").await;
    let id = e.album("A", "B", 3);
    e.st().db.execute("UPDATE tracks SET mb_releasegroupid='0b0f2b0e-0000-0000-0000-000000000000', mb_trackid='0b0f2b0e-0000-0000-0000-000000000001'", p![]).unwrap();
    let turns = |st: &crate::state::AppState| -> i64 {
        st.mb().line_stats().as_object().unwrap().values().map(|c| c["turns"].as_i64().unwrap_or(0)).sum()
    };
    let before = turns(e.st());
    for (m, uri, body) in [
        ("GET", "/api/library/albums".to_string(), None), ("GET", "/api/library/albums?type=one_off".into(), None), ("GET", format!("/api/library/albums/{id}"), None),
        ("GET", "/api/library/album-types".into(), None), ("PUT", format!("/api/library/albums/{id}/type"), Some(json!({ "format": "single" }))),
        ("GET", "/api/library/artists/A/one-offs".into(), None), ("POST", "/api/library/albums/trash".into(), Some(json!({ "ids": [id], "dry_run": true }))),
        ("GET", "/api/library/tracks/1/added".into(), None),
    ] {
        let r = e.send(m, &uri, body).await;
        assert_eq!(r.status, 200, "{uri}: {}", r.text());
    }
    assert_eq!(turns(e.st()), before, "no call to MusicBrainz");
    e.done();
}

/// Settings → Tasks lists "Sort albums by type" with its words, and its settings are in the schema.
#[tokio::test]
async fn the_types_task_is_listed() {
    let e = env("task").await;
    let tasks = crate::scheduler::listing(e.st());
    let t = tasks.iter().find(|t| t["key"] == "types").expect("the types task");
    assert_eq!(t["name"], "Sort albums by type");
    assert!(t["desc"].as_str().unwrap().starts_with("Work out which albums are live, compilations, remixes, soundtracks, singles or one-offs"));
    assert_eq!((t["enabled"].clone(), t["minutes"].clone()), (json!(true), json!(360)));
    e.done();
}
