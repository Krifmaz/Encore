//! Someone else's shared playlist in my playlists, through the real router: anna shares, ben adds it (from a
//! pasted link or the share page) and sees it live and read-only, cara is a third account. Stop sharing, a
//! deleted playlist and a disabled or deleted owner end the entry; Remove and Save a copy; the owner's count.
//! No ffmpeg is needed (the songs are .mp3 files served as they are).
use super::share_token_of;
use crate::state::AppState;
use crate::test_support::{send, state_in, Resp};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use encore_db::{p, RowExt};
use serde_json::{json, Value};
use std::path::PathBuf;
use tower::ServiceExt;

const PW: &str = "linker-pass-1";

struct Env {
    st: AppState,
    app: Router,
    dir: PathBuf,
    music: PathBuf,
    anna: String,
    ben: String,
    cara: String,
    next: i64,
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn login(st: &AppState, app: &Router, name: &str) -> String {
    crate::auth::create_user(st, name, PW, "user", None, None).unwrap();
    let r = send(app, "POST", "/api/auth/login", "", Some(json!({ "username": name, "password": PW })), &[]).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    r.h("set-cookie").split(';').next().unwrap().to_string()
}

async fn setup(name: &str) -> Env {
    let dir = std::env::temp_dir().join(format!("encore-links-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let st = state_in(&dir);
    let app = crate::app(st.clone());
    let anna = login(&st, &app, "anna").await;
    let ben = login(&st, &app, "ben").await;
    let cara = login(&st, &app, "cara").await;
    let music = dir.join("music");
    std::fs::create_dir_all(&music).unwrap();
    Env { st, app, dir, music, anna, ben, cara, next: 1 }
}

impl Env {
    /// A library song whose file exists (an .mp3 is served as it is, so the bytes needn't be real audio).
    fn add(&mut self, title: &str, album: &str) -> i64 {
        let id = self.next;
        self.next += 1;
        let aid = encore_library::album_key("Band", album, "");
        let folder = self.music.join(&aid);
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join(format!("{id}.mp3"));
        std::fs::write(&path, b"ID3 not really audio").unwrap();
        self.st.db.execute(
            "INSERT INTO tracks (id, path, title, artist, album, albumartist, album_id, format, bitrate, duration, size, year, disc, track, added_at)
             VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            p![id, path.to_string_lossy(), title, "Band", album, "Band", aid, "mp3", 320, 180.0, 20, "2001", 1, id, id as f64]).unwrap();
        self.st.library.artists.invalidate();
        id
    }
    async fn req(&self, cookie: &str, method: &str, uri: &str, body: Option<Value>) -> Resp {
        send(&self.app, method, uri, cookie, body, &[]).await
    }
    async fn get(&self, cookie: &str, uri: &str) -> Value {
        let r = self.req(cookie, "GET", uri, None).await;
        assert_eq!(r.status, StatusCode::OK, "{uri}: {}", r.text());
        r.json()
    }
    /// A playlist of `cookie`'s with these library songs, in this order.
    async fn playlist(&self, cookie: &str, name: &str, ids: &[i64]) -> i64 {
        let r = self.req(cookie, "POST", "/api/playlists", Some(json!({ "name": name }))).await;
        assert_eq!(r.status, StatusCode::OK, "{}", r.text());
        let pid = r.json()["id"].as_i64().unwrap();
        if !ids.is_empty() {
            let r = self.req(cookie, "POST", &format!("/api/playlists/{pid}/tracks"), Some(json!({ "track_ids": ids }))).await;
            assert_eq!(r.json()["added"], ids.len(), "{}", r.text());
        }
        pid
    }
    /// anna shares her playlist: (token, url).
    async fn share(&self, pid: i64) -> (String, String) {
        let r = self.req(&self.anna, "POST", &format!("/api/playlists/{pid}/share"), None).await;
        assert_eq!(r.status, StatusCode::OK, "{}", r.text());
        (r.json()["token"].as_str().unwrap().to_string(), r.json()["url"].as_str().unwrap().to_string())
    }
    /// `cookie` adds a pasted link: the answer.
    async fn add_link(&self, cookie: &str, link: &str) -> Resp {
        self.req(cookie, "POST", "/api/playlists/shared", Some(json!({ "link": link }))).await
    }
    /// The row of the list of `cookie`'s with this id.
    async fn row(&self, cookie: &str, id: i64) -> Value {
        self.get(cookie, "/api/playlists").await.as_array().unwrap().iter().find(|p| p["id"] == id).cloned().unwrap_or_else(|| panic!("no playlist {id}"))
    }
    /// The song ids of a playlist's detail, in order.
    async fn songs(&self, cookie: &str, id: i64) -> Vec<i64> {
        self.get(cookie, &format!("/api/playlists/{id}")).await["items"].as_array().unwrap().iter().map(|i| i["track_id"].as_i64().unwrap()).collect()
    }
    /// anna's item ids of her playlist, in order.
    async fn items(&self, pid: i64) -> Vec<i64> {
        self.get(&self.anna, &format!("/api/playlists/{pid}")).await["items"].as_array().unwrap().iter().map(|i| i["id"].as_i64().unwrap()).collect()
    }
    fn linked_rows(&self, user: &str) -> i64 {
        self.st.db.one("SELECT COUNT(*) n FROM playlists p JOIN users u ON u.id=p.user_id WHERE u.username=? AND p.kind='linked'", p![user]).unwrap().unwrap()["n"].as_i64().unwrap()
    }
    /// anna's share.added_by of a playlist, from her list and her detail (they must agree).
    async fn added_by(&self, pid: i64) -> i64 {
        let list = self.row(&self.anna, pid).await["share"]["added_by"].as_i64().unwrap();
        let detail = self.get(&self.anna, &format!("/api/playlists/{pid}")).await["share"]["added_by"].as_i64().unwrap();
        assert_eq!(list, detail, "the list and the detail count the same");
        list
    }
}

fn detail_of(r: &Resp) -> (StatusCode, String) {
    (r.status, r.json()["detail"].as_str().unwrap_or("").to_string())
}

#[test]
fn the_share_link_parser_takes_every_form_the_server_makes() {
    let t = "AbC12_xY-9876543";
    let forms = [
        format!("https://music.example/s/{t}"),
        format!("https://example.org/encore/s/{t}"),
        format!("/s/{t}"),
        format!("http://192.168.1.5:8080/s/{t}"),
        format!("Road trip\nhttps://music.example/s/{t}"),
        format!("/api/share/{t}"),
        format!("https://music.example/api/share/{t}/cover?s=600"),
        format!("https://music.example/api/share/{t}/stream?track=3"),
        format!("https://music.example/api/share/{t}/video.mp4"),
        format!("https://music.example/s/{t}/"),
        format!("https://music.example/s/{t}?x=1"),
        format!("https://music.example/s/{t}#frag"),
        format!("Listen to this: https://music.example/s/{t}."),
        format!("https://music.example/s/{t}, it's good"),
        format!("<https://music.example/s/{t}>"),
        format!("\"https://music.example/s/{t}\""),
        format!("'https://music.example/s/{t}'"),
        format!("music.example/s/{t}"),
        format!("  {t}\n"),
        t.to_string(),
    ];
    for text in &forms {
        assert_eq!(share_token_of(text).as_deref(), Some(t), "{text:?}");
    }
    // another site's /s/ link gives a token too (the server then answers 404, and the phone falls back)
    assert_eq!(share_token_of("https://link.deezer.com/s/30ABCdef").as_deref(), Some("30ABCdef"));
    let long = "a".repeat(65);
    for text in ["", "   ", "https://open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M", "/s/", "https://music.example/s/", "hello", &format!("/s/{long}"), &long] {
        assert_eq!(share_token_of(text), None, "{text:?}");
    }
}

#[tokio::test]
async fn adding_a_shared_playlist_makes_a_live_read_only_entry() {
    let mut e = setup("live").await;
    let (a1, a2, a3) = (e.add("One", "First"), e.add("Two", "First"), e.add("Three", "Second"));
    let pid = e.playlist(&e.anna, "Road trip", &[a1, a2, a3]).await;
    let (tok, url) = e.share(pid).await;
    let r = e.add_link(&e.ben, &url).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let j = r.json();
    assert_eq!((j["name"].as_str(), j["added"].as_bool()), (Some("Road trip"), Some(true)));
    let entry = j["id"].as_i64().unwrap();
    assert_ne!(entry, pid);

    // the list row
    let list = e.req(&e.ben, "GET", "/api/playlists", None).await;
    assert!(!list.text().contains("owner_id"), "{}", list.text());
    let row = e.row(&e.ben, entry).await;
    assert_eq!(row["kind"], "linked");
    assert_eq!(row["read_only"], true);
    assert_eq!(row["linked"], json!({ "owner": "anna", "active": true, "liked": false }));
    assert_eq!((row["total"].as_i64(), row["matched"].as_i64()), (Some(3), Some(3)));
    assert!(row["source_url"].is_null() && row["share"].is_null(), "{row}");
    assert_eq!((row["follow"].clone(), row["sync_targets"].clone(), row["remote_ids"].clone()), (json!(0), json!([]), json!("{}")));
    assert_eq!(row["cover_url"], format!("/api/share/{tok}/cover"));
    assert_eq!(row["name"], "Road trip");

    // the detail: anna's songs in her order, numbered from 0, nothing of her imports or requests
    let d = e.req(&e.ben, "GET", &format!("/api/playlists/{entry}"), None).await;
    assert_eq!(d.status, StatusCode::OK, "{}", d.text());
    assert!(!d.text().contains("owner_id"));
    let items = d.json()["items"].as_array().unwrap().clone();
    assert_eq!(items.iter().map(|i| i["track_id"].as_i64().unwrap()).collect::<Vec<_>>(), vec![a1, a2, a3]);
    assert_eq!(items.iter().map(|i| i["position"].as_i64().unwrap()).collect::<Vec<_>>(), vec![0, 1, 2]);
    assert!(items.iter().all(|i| i.get("request_status").is_none() && i.get("source").is_none()), "{items:?}");
    assert_eq!(d.json()["remote_ids"], json!({}));

    // anna changes her playlist: ben's entry follows each step
    let it = e.items(pid).await;
    let r = e.req(&e.anna, "POST", &format!("/api/playlists/{pid}/move"), Some(json!({ "item_id": it[2], "to": 0 }))).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(e.songs(&e.ben, entry).await, vec![a3, a1, a2]);
    let r = e.req(&e.anna, "DELETE", &format!("/api/playlists/{pid}/items/{}", it[0]), None).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(e.songs(&e.ben, entry).await, vec![a3, a2]);
    let a4 = e.add("Four", "Third");
    let r = e.req(&e.anna, "POST", &format!("/api/playlists/{pid}/tracks"), Some(json!({ "track_ids": [a4] }))).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(e.songs(&e.ben, entry).await, vec![a3, a2, a4]);
    let before = e.st.db.one("SELECT updated_at FROM playlists WHERE id=?", p![entry]).unwrap().unwrap()["updated_at"].clone();
    let r = e.req(&e.anna, "PATCH", &format!("/api/playlists/{pid}"), Some(json!({ "name": "Road trip 2" }))).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(e.row(&e.ben, entry).await["name"], "Road trip 2");
    // written back without moving the entry in the list
    let stored = e.st.db.one("SELECT name, updated_at FROM playlists WHERE id=?", p![entry]).unwrap().unwrap();
    assert_eq!((stored["name"].clone(), stored["updated_at"].clone()), (json!("Road trip 2"), before));

    // the owner sees how many added it
    assert_eq!(e.added_by(pid).await, 1);
    // the follower plays the songs as library songs
    let r = e.req(&e.ben, "GET", &format!("/api/library/tracks/{a3}/stream"), None).await;
    assert_eq!(r.status, StatusCode::OK);
}

#[tokio::test]
async fn adding_twice_does_nothing_new_and_the_owner_cannot_add_their_own() {
    let mut e = setup("twice").await;
    let a1 = e.add("One", "First");
    let pid = e.playlist(&e.anna, "Road trip", &[a1]).await;
    let (tok, url) = e.share(pid).await;
    let first = e.add_link(&e.ben, &url).await.json();
    let again = e.add_link(&e.ben, &format!("Road trip\n{url}")).await;
    assert_eq!(again.status, StatusCode::OK);
    assert_eq!((again.json()["id"].clone(), again.json()["added"].clone()), (first["id"].clone(), json!(false)));
    let page = e.req(&e.ben, "POST", &format!("/api/share/{tok}/add"), None).await;
    assert_eq!((page.status, page.json()["id"].clone(), page.json()["added"].clone()), (StatusCode::OK, first["id"].clone(), json!(false)));
    assert_eq!(e.linked_rows("ben"), 1);
    assert_eq!(e.get(&e.ben, "/api/playlists").await.as_array().unwrap().iter().filter(|p| p["kind"] == "linked").count(), 1);

    // two adds at once leave one entry
    let body = json!({ "link": url });
    let page_add = format!("/api/share/{tok}/add");
    let (x, y) = tokio::join!(
        send(&e.app, "POST", "/api/playlists/shared", &e.cara, Some(body.clone()), &[]),
        send(&e.app, "POST", &page_add, &e.cara, None, &[]),
    );
    assert_eq!((x.status, y.status), (StatusCode::OK, StatusCode::OK), "{} {}", x.text(), y.text());
    assert_eq!(x.json()["id"], y.json()["id"]);
    assert_eq!([x.json()["added"].as_bool().unwrap(), y.json()["added"].as_bool().unwrap()].iter().filter(|a| **a).count(), 1);
    assert_eq!(e.linked_rows("cara"), 1);

    // the owner can't add their own
    for r in [e.add_link(&e.anna, &url).await, e.req(&e.anna, "POST", &format!("/api/share/{tok}/add"), None).await] {
        assert_eq!(detail_of(&r), (StatusCode::BAD_REQUEST, "This is your own playlist".into()));
    }
    assert_eq!(e.linked_rows("anna"), 0);
    assert_eq!(e.added_by(pid).await, 2);
    // the activity log (which an admin owner reads) says that it was added, not by whom
    let logged: Vec<String> = e.st.db.query("SELECT message FROM activity WHERE message LIKE '%to their playlists' ORDER BY id", p![]).unwrap()
        .iter().map(|r| r.s("message")).collect();
    assert_eq!(logged, vec!["Someone added anna's playlist 'Road trip' to their playlists"; 2]);
}

#[tokio::test]
async fn links_that_are_not_playlists_are_refused() {
    let mut e = setup("refused").await;
    let a1 = e.add("One", "First");
    let pid = e.playlist(&e.anna, "Road trip", &[a1]).await;
    let (_, url) = e.share(pid).await;
    for link in ["https://open.spotify.com/playlist/x", "", "hello"] {
        assert_eq!(detail_of(&e.add_link(&e.ben, link).await), (StatusCode::BAD_REQUEST, "That isn't a share link".into()), "{link}");
    }
    // song and album links, through both routes
    let aid = e.st.db.one("SELECT album_id FROM tracks WHERE id=?", p![a1]).unwrap().unwrap()["album_id"].as_str().unwrap().to_string();
    let album = e.req(&e.anna, "POST", &format!("/api/library/albums/{aid}/share"), None).await.json();
    let song = e.req(&e.anna, "POST", &format!("/api/library/tracks/{a1}/share"), None).await.json();
    for s in [album, song] {
        let url = s["path"].as_str().unwrap();
        let tok = s["token"].as_str().unwrap();
        assert_eq!(detail_of(&e.add_link(&e.ben, url).await), (StatusCode::BAD_REQUEST, "Only playlists can be added".into()));
        let r = e.req(&e.ben, "POST", &format!("/api/share/{tok}/add"), None).await;
        assert_eq!(detail_of(&r), (StatusCode::BAD_REQUEST, "Only playlists can be added".into()));
    }
    assert_eq!(detail_of(&e.add_link(&e.ben, "/s/nosuchtoken12").await), (StatusCode::NOT_FOUND, "This link doesn't work any more".into()));
    assert_eq!(e.req(&e.ben, "POST", "/api/share/nosuchtoken12/add", None).await.status, StatusCode::NOT_FOUND);
    // signed out; a cookie without the CSRF header; GET is not a route
    assert_eq!(e.add_link("", &url).await.status, StatusCode::UNAUTHORIZED);
    let raw = Request::builder().method("POST").uri("/api/playlists/shared").header("cookie", &e.ben).header("content-type", "application/json")
        .body(Body::from(json!({ "link": url }).to_string())).unwrap();
    assert_eq!(e.app.clone().oneshot(raw).await.unwrap().status(), StatusCode::FORBIDDEN);
    assert_eq!(e.req(&e.ben, "GET", "/api/playlists/shared", None).await.status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(e.linked_rows("ben"), 0);
}

#[tokio::test]
async fn an_added_playlist_cannot_be_changed() {
    let mut e = setup("readonly").await;
    let (a1, a2) = (e.add("One", "First"), e.add("Two", "First"));
    let pid = e.playlist(&e.anna, "Road trip", &[a1, a2]).await;
    let (_, url) = e.share(pid).await;
    let entry = e.add_link(&e.ben, &url).await.json()["id"].as_i64().unwrap();
    let mine = e.playlist(&e.ben, "Mine", &[a1]).await;
    let item = e.items(pid).await[0];
    let before_row = e.st.db.one("SELECT name, sync_targets, follow FROM playlists WHERE id=?", p![entry]).unwrap().unwrap();
    let before_items = e.items(pid).await;
    let only = (StatusCode::FORBIDDEN, "Only anna can change this playlist".to_string());
    let at = format!("/api/playlists/{entry}");
    let refused: Vec<(&str, String, Option<Value>)> = vec![
        ("PATCH", at.clone(), Some(json!({ "name": "Mine now" }))),
        ("PATCH", at.clone(), Some(json!({ "description": "x" }))),
        ("PATCH", at.clone(), Some(json!({ "follow": true }))),
        ("PATCH", at.clone(), Some(json!({ "sync_targets": ["jellyfin"] }))),
        ("POST", format!("{at}/tracks"), Some(json!({ "track_ids": [a2] }))),
        ("POST", format!("{at}/move"), Some(json!({ "item_id": item, "to": 1 }))),
        ("DELETE", format!("{at}/items/{item}"), None),
        ("POST", format!("{at}/dedupe"), None),
        ("POST", format!("{at}/rematch"), None),
        ("POST", format!("{at}/request-missing"), Some(json!({}))),
        ("POST", format!("{at}/refresh"), None),
        ("POST", format!("{at}/sync"), None),
        ("POST", format!("{at}/share"), None),
        ("DELETE", format!("{at}/share"), None),
        // refused before the link is fetched (there is no network here)
        ("POST", "/api/playlists/import".to_string(), Some(json!({ "url": "https://open.spotify.com/playlist/x", "into": entry }))),
    ];
    for (method, uri, body) in refused {
        let r = e.req(&e.ben, method, &uri, body).await;
        assert_eq!(detail_of(&r), only, "{method} {uri}");
    }
    let count = |e: &Env| e.st.db.one("SELECT COUNT(*) n FROM playlists p JOIN users u ON u.id=p.user_id WHERE u.username='ben'", p![]).unwrap().unwrap()["n"].as_i64().unwrap();
    let n = count(&e);
    for delete_sources in [false, true] {
        let r = e.req(&e.ben, "POST", "/api/playlists/merge", Some(json!({ "ids": [mine, entry], "name": "M", "delete_sources": delete_sources }))).await;
        assert_eq!(detail_of(&r), (StatusCode::FORBIDDEN, "“Road trip” is anna's playlist. Save a copy to merge it.".into()));
    }
    assert_eq!(count(&e), n, "nothing was made or deleted");
    // a linked entry is never shared, even through a shares row of its own
    e.st.db.execute("INSERT INTO shares (token, track_id, user_id, created_at, kind, ref) VALUES ('linkedrowtoken1', 0, (SELECT id FROM users WHERE username='ben'), 1, 'playlist', ?)",
        p![entry.to_string()]).unwrap();
    assert_eq!(e.req("", "GET", "/api/share/linkedrowtoken1", None).await.status, StatusCode::NOT_FOUND);
    // another user's id stays "not found"
    assert_eq!(detail_of(&e.req(&e.cara, "PATCH", &at, Some(json!({ "name": "x" }))).await), (StatusCode::NOT_FOUND, "Playlist not found".into()));

    // nothing changed
    assert_eq!(e.items(pid).await, before_items);
    assert_eq!(e.get(&e.anna, &format!("/api/playlists/{pid}")).await["name"], "Road trip");
    let after_row = e.st.db.one("SELECT name, sync_targets, follow FROM playlists WHERE id=?", p![entry]).unwrap().unwrap();
    assert_eq!(after_row, before_row);
}

#[tokio::test]
async fn stop_sharing_ends_it_and_sharing_again_does_not_bring_it_back() {
    let mut e = setup("stop").await;
    let (a1, a2) = (e.add("One", "First"), e.add("Two", "Second"));
    let pid = e.playlist(&e.anna, "Road trip", &[a1, a2]).await;
    let (tok, url) = e.share(pid).await;
    let entry = e.add_link(&e.ben, &url).await.json()["id"].as_i64().unwrap();
    let r = e.req(&e.anna, "DELETE", &format!("/api/playlists/{pid}/share"), None).await;
    assert_eq!(r.status, StatusCode::OK);

    let row = e.row(&e.ben, entry).await;
    assert_eq!(row["linked"], json!({ "owner": "anna", "active": false, "liked": false }));
    assert_eq!((row["total"].as_i64(), row["cover_url"].clone(), row["name"].as_str()), (Some(0), Value::Null, Some("Road trip")));
    assert_eq!(e.get(&e.ben, &format!("/api/playlists/{entry}")).await["items"], json!([]));
    let r = e.req(&e.ben, "POST", &format!("/api/playlists/{entry}/copy"), None).await;
    assert_eq!(detail_of(&r), (StatusCode::NOT_FOUND, "This link doesn't work any more".into()));

    // sharing again makes a new link: the entry stays ended, and nobody has added the new one
    let (tok2, url2) = e.share(pid).await;
    assert_ne!(tok2, tok);
    assert_eq!(e.row(&e.ben, entry).await["linked"]["active"], false);
    assert_eq!(e.added_by(pid).await, 0);
    // adding the new link takes the ended entry over
    let r = e.add_link(&e.ben, &url2).await.json();
    assert_eq!((r["id"].as_i64(), r["added"].as_bool()), (Some(entry), Some(true)));
    let row = e.row(&e.ben, entry).await;
    assert_eq!((row["linked"]["active"].clone(), row["total"].as_i64()), (json!(true), Some(2)));
    assert_eq!(row["cover_url"], format!("/api/share/{tok2}/cover"));
    assert_eq!(e.linked_rows("ben"), 1);
    assert_eq!(e.added_by(pid).await, 1);
}

#[tokio::test]
async fn a_deleted_playlist_or_a_disabled_or_deleted_owner_ends_it() {
    let mut e = setup("ends").await;
    let a1 = e.add("One", "First");
    // a deleted playlist
    let gone = e.playlist(&e.anna, "Gone", &[a1]).await;
    let (_, url) = e.share(gone).await;
    let entry = e.add_link(&e.ben, &url).await.json()["id"].as_i64().unwrap();
    assert_eq!(e.req(&e.anna, "DELETE", &format!("/api/playlists/{gone}"), None).await.status, StatusCode::OK);
    let row = e.row(&e.ben, entry).await;
    assert_eq!((row["name"].as_str(), row["linked"].clone()), (Some("Gone"), json!({ "owner": "anna", "active": false, "liked": false })));

    // a disabled owner, then enabled again
    let kept = e.playlist(&e.anna, "Kept", &[a1]).await;
    let (_, url) = e.share(kept).await;
    let entry = e.add_link(&e.ben, &url).await.json()["id"].as_i64().unwrap();
    e.st.db.execute("UPDATE users SET disabled=1 WHERE username='anna'", p![]).unwrap();
    assert_eq!(e.row(&e.ben, entry).await["linked"]["active"], false);
    assert_eq!(e.get(&e.ben, &format!("/api/playlists/{entry}")).await["items"], json!([]));
    e.st.db.execute("UPDATE users SET disabled=0 WHERE username='anna'", p![]).unwrap();
    let row = e.row(&e.ben, entry).await;
    assert_eq!((row["linked"]["active"].clone(), row["total"].as_i64()), (json!(true), Some(1)));

    // a deleted owner: ended, with the name the entry knew
    e.st.db.execute("DELETE FROM users WHERE username='anna'", p![]).unwrap();
    let row = e.row(&e.ben, entry).await;
    assert_eq!((row["name"].as_str(), row["linked"].clone()), (Some("Kept"), json!({ "owner": "anna", "active": false, "liked": false })));
    let r = e.req(&e.ben, "POST", &format!("/api/playlists/{entry}/copy"), None).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn liked_songs_are_added_as_liked_songs_of_the_owner() {
    let mut e = setup("liked").await;
    let (a1, a2, a3) = (e.add("One", "First"), e.add("Two", "First"), e.add("Three", "Second"));
    for id in [a1, a2, a3] {
        assert_eq!(e.req(&e.anna, "POST", &format!("/api/likes/{id}"), None).await.status, StatusCode::OK);
    }
    let liked = e.get(&e.anna, "/api/playlists").await.as_array().unwrap().iter().find(|p| p["kind"] == "liked").unwrap()["id"].as_i64().unwrap();
    let (_, url) = e.share(liked).await;
    let r = e.add_link(&e.ben, &url).await.json();
    assert_eq!(r["name"], "Liked Songs · anna");
    let entry = r["id"].as_i64().unwrap();
    let row = e.row(&e.ben, entry).await;
    assert_eq!((row["name"].as_str(), row["linked"]["liked"].as_bool()), (Some("Liked Songs · anna"), Some(true)));
    assert_eq!(e.songs(&e.ben, entry).await, vec![a3, a2, a1], "newest first");
    assert_eq!(e.req(&e.anna, "DELETE", &format!("/api/likes/{a2}"), None).await.status, StatusCode::OK);
    assert_eq!(e.songs(&e.ben, entry).await, vec![a3, a1]);
    // ben's own Liked Songs is untouched and still first
    let list = e.get(&e.ben, "/api/playlists").await;
    assert_eq!((list[0]["kind"].as_str(), list[0]["total"].as_i64()), (Some("liked"), Some(0)));
    assert_eq!(e.get(&e.ben, "/api/likes").await, json!([]));
}

#[tokio::test]
async fn remove_from_my_playlists_and_save_a_copy() {
    let mut e = setup("remove").await;
    let (a1, a2, a3) = (e.add("One", "First"), e.add("Two", "First"), e.add("Three", "Second"));
    let pid = e.playlist(&e.anna, "Road trip", &[a1, a2, a3]).await;
    let (_, url) = e.share(pid).await;
    let entry = e.add_link(&e.ben, &url).await.json()["id"].as_i64().unwrap();
    let r = e.req(&e.ben, "DELETE", &format!("/api/playlists/{entry}"), None).await;
    assert_eq!((r.status, r.json()), (StatusCode::OK, json!({ "ok": true })));
    assert!(e.st.db.one("SELECT 1 FROM playlists WHERE id=?", p![entry]).unwrap().is_none());
    assert_eq!(e.songs(&e.anna, pid).await, vec![a1, a2, a3]);
    assert_eq!(e.added_by(pid).await, 0);
    assert_eq!(e.req("", "GET", &url.replace("/s/", "/api/share/"), None).await.status, StatusCode::OK, "the link still works");

    // added again, then saved as a copy of ben's own
    let entry = e.add_link(&e.ben, &url).await.json()["id"].as_i64().unwrap();
    let r = e.req(&e.ben, "POST", &format!("/api/playlists/{entry}/copy"), None).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!((r.json()["name"].as_str(), r.json()["added"].as_i64()), (Some("Road trip"), Some(3)));
    let copy = r.json()["id"].as_i64().unwrap();
    let row = e.st.db.one("SELECT p.kind, p.description, u.username FROM playlists p JOIN users u ON u.id=p.user_id WHERE p.id=?", p![copy]).unwrap().unwrap();
    assert_eq!(Value::Object(row), json!({ "kind": "playlist", "description": "Shared by anna", "username": "ben" }));
    assert_eq!(e.songs(&e.ben, copy).await, vec![a1, a2, a3]);
    // the copy is ben's to change
    assert_eq!(e.req(&e.ben, "PATCH", &format!("/api/playlists/{copy}"), Some(json!({ "name": "Mine" }))).await.status, StatusCode::OK);

    let mine = e.playlist(&e.ben, "Own", &[a1]).await;
    let r = e.req(&e.ben, "POST", &format!("/api/playlists/{mine}/copy"), None).await;
    assert_eq!(detail_of(&r), (StatusCode::BAD_REQUEST, "Only playlists added from a link can be copied here".into()));
    let r = e.req(&e.cara, "POST", &format!("/api/playlists/{entry}/copy"), None).await;
    assert_eq!(detail_of(&r), (StatusCode::NOT_FOUND, "Playlist not found".into()));
}

#[tokio::test]
async fn the_share_page_knows_whether_you_added_it() {
    let mut e = setup("viewer").await;
    let a1 = e.add("One", "First");
    let pid = e.playlist(&e.anna, "Road trip", &[a1]).await;
    let (tok, url) = e.share(pid).await;
    let at = format!("/api/share/{tok}");
    assert_eq!(e.get(&e.ben, &at).await["viewer"], json!({ "signed_in": true, "own": false, "added": false, "playlist_id": null }));
    let entry = e.add_link(&e.ben, &url).await.json()["id"].as_i64().unwrap();
    assert_eq!(e.get(&e.ben, &at).await["viewer"], json!({ "signed_in": true, "own": false, "added": true, "playlist_id": entry }));
    assert_eq!(e.get(&e.anna, &at).await["viewer"], json!({ "signed_in": true, "own": true }));
    assert_eq!(e.get("", &at).await["viewer"], json!({ "signed_in": false, "own": false }));
}

#[tokio::test]
async fn check_says_what_a_link_would_add() {
    let mut e = setup("check").await;
    let (a1, a2, a3) = (e.add("One", "First"), e.add("Two", "First"), e.add("Three", "Second"));
    let pid = e.playlist(&e.anna, "Road trip", &[a1, a2, a3]).await;
    let (_, url) = e.share(pid).await;
    let check = |cookie: String, link: String| {
        let app = e.app.clone();
        async move { send(&app, "POST", "/api/playlists/shared/check", &cookie, Some(json!({ "link": link })), &[]).await }
    };
    let r = check(e.ben.clone(), format!("Road trip\n{url}")).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(r.json(), json!({ "name": "Road trip", "title": "Road trip", "owner": "anna", "liked": false, "count": 3, "own": false, "added": false, "id": null }));
    assert_eq!(e.linked_rows("ben"), 0, "checking adds nothing");
    let entry = e.add_link(&e.ben, &url).await.json()["id"].as_i64().unwrap();
    let r = check(e.ben.clone(), url.clone()).await.json();
    assert_eq!((r["added"].clone(), r["id"].as_i64()), (json!(true), Some(entry)));
    let r = check(e.anna.clone(), url.clone()).await.json();
    assert_eq!((r["own"].clone(), r["added"].clone(), r["id"].as_i64()), (json!(true), json!(false), Some(pid)));
    // the errors of adding
    assert_eq!(detail_of(&check(e.ben.clone(), "https://open.spotify.com/playlist/x".into()).await), (StatusCode::BAD_REQUEST, "That isn't a share link".into()));
    let song = e.req(&e.anna, "POST", &format!("/api/library/tracks/{a1}/share"), None).await.json();
    assert_eq!(detail_of(&check(e.ben.clone(), song["url"].as_str().unwrap().into()).await), (StatusCode::BAD_REQUEST, "Only playlists can be added".into()));
    assert_eq!(detail_of(&check(e.ben.clone(), "https://link.deezer.com/s/30ABCdef".into()).await), (StatusCode::NOT_FOUND, "This link doesn't work any more".into()));
    assert_eq!(check(String::new(), url.clone()).await.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn added_playlists_are_not_counted_as_made() {
    let mut e = setup("stats").await;
    let a1 = e.add("One", "First");
    let pid = e.playlist(&e.anna, "Road trip", &[a1]).await;
    let (_, url) = e.share(pid).await;
    let builders = |board: &Value| -> Vec<(String, i64)> {
        board["boards"]["playlists"]["rows"].as_array().unwrap().iter().map(|r| (r["username"].as_str().unwrap().to_string(), r["value"].as_i64().unwrap())).collect()
    };
    e.add_link(&e.ben, &url).await;
    let board = e.get(&e.ben, "/api/stats/leaderboard").await;
    assert_eq!(builders(&board), vec![("anna".to_string(), 1)]);
    e.playlist(&e.ben, "Mine", &[]).await;
    let board = e.get(&e.ben, "/api/stats/leaderboard").await;
    let mut got = builders(&board);
    got.sort();
    assert_eq!(got, vec![("anna".to_string(), 1), ("ben".to_string(), 1)]);
}
