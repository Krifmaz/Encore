//! Collaborative playlists through the real router: anna owns the playlist, ben is invited by name, cara
//! joins with the invite link, dan is nobody's. Toggling, what members may and may not do, leaving,
//! removing, the link's life, and a disabled owner.
use super::invite_token_of;
use crate::state::AppState;
use crate::test_support::{send, state_in, Resp};
use axum::http::StatusCode;
use axum::Router;
use encore_db::{p, RowExt};
use serde_json::{json, Value};
use std::path::PathBuf;

const PW: &str = "collab-pass-1";

struct Env {
    st: AppState,
    app: Router,
    dir: PathBuf,
    anna: String,
    ben: String,
    cara: String,
    dan: String,
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
    let dir = std::env::temp_dir().join(format!("encore-collab-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let st = state_in(&dir);
    let app = crate::app(st.clone());
    let anna = login(&st, &app, "anna").await;
    let ben = login(&st, &app, "ben").await;
    let cara = login(&st, &app, "cara").await;
    let dan = login(&st, &app, "dan").await;
    for id in 1..=4 {
        st.db.execute("INSERT INTO tracks (id, path, title, artist, album, albumartist, album_id, format, duration, added_at) VALUES (?,?,?,?,?,?,?,?,?,?)",
            p![id, format!("/m/{id}.mp3"), format!("Song {id}"), "Band", "Record", "Band", "band-record", "mp3", 180.0, id as f64]).unwrap();
    }
    Env { st, app, dir, anna, ben, cara, dan }
}

impl Env {
    async fn req(&self, cookie: &str, method: &str, uri: &str, body: Option<Value>) -> Resp {
        send(&self.app, method, uri, cookie, body, &[]).await
    }
    async fn ok(&self, cookie: &str, method: &str, uri: &str, body: Option<Value>) -> Value {
        let r = self.req(cookie, method, uri, body).await;
        assert_eq!(r.status, StatusCode::OK, "{method} {uri}: {}", r.text());
        r.json()
    }
    async fn status(&self, cookie: &str, method: &str, uri: &str, body: Option<Value>) -> StatusCode {
        self.req(cookie, method, uri, body).await.status
    }
    /// anna's playlist with songs 1 and 2.
    async fn playlist(&self) -> i64 {
        let pid = self.ok(&self.anna, "POST", "/api/playlists", Some(json!({ "name": "Road trip" }))).await["id"].as_i64().unwrap();
        self.ok(&self.anna, "POST", &format!("/api/playlists/{pid}/tracks"), Some(json!({ "track_ids": [1, 2] }))).await;
        pid
    }
    async fn titles(&self, cookie: &str, pid: i64) -> Vec<String> {
        let d = self.ok(cookie, "GET", &format!("/api/playlists/{pid}"), None).await;
        d["items"].as_array().unwrap().iter().map(|i| i["title"].as_str().unwrap_or("").to_string()).collect()
    }
    async fn listed(&self, cookie: &str, pid: i64) -> Option<Value> {
        self.ok(cookie, "GET", "/api/playlists", None).await.as_array().unwrap().iter().find(|p| p["id"] == json!(pid)).cloned()
    }
}

#[tokio::test]
async fn invited_member_edits_songs_but_not_the_playlist() {
    let e = setup("invite").await;
    let pid = e.playlist().await;
    assert_eq!(e.listed(&e.anna, pid).await.unwrap()["collab"], Value::Null, "not collaborative yet");

    // inviting by name turns Collaborative on
    let m = e.ok(&e.anna, "POST", &format!("/api/playlists/{pid}/collaborators"), Some(json!({ "username": "Ben" }))).await;
    assert_eq!(m["collaborative"], json!(true));
    let ben_id = e.st.db.one("SELECT id FROM users WHERE username='ben'", p![]).unwrap().unwrap().i("id");
    assert_eq!(m["members"], json!([{ "user_id": ben_id, "username": "ben", "status": "invited" }]));
    assert_eq!(e.status(&e.anna, "POST", &format!("/api/playlists/{pid}/collaborators"), Some(json!({ "username": "nobody" }))).await, StatusCode::NOT_FOUND);
    assert_eq!(e.status(&e.anna, "POST", &format!("/api/playlists/{pid}/collaborators"), Some(json!({ "username": "anna" }))).await, StatusCode::BAD_REQUEST);

    // not a member until the invite is accepted
    assert_eq!(e.status(&e.ben, "GET", &format!("/api/playlists/{pid}"), None).await, StatusCode::NOT_FOUND);
    let inv = e.ok(&e.ben, "GET", "/api/collab/invites", None).await;
    assert_eq!(inv, json!([{ "playlist_id": pid, "name": "Road trip", "owner": "anna", "count": 2 }]));
    assert_eq!(e.status(&e.cara, "POST", &format!("/api/collab/invites/{pid}/accept"), None).await, StatusCode::NOT_FOUND, "cara wasn't invited");
    e.ok(&e.ben, "POST", &format!("/api/collab/invites/{pid}/accept"), None).await;
    assert_eq!(e.ok(&e.ben, "GET", "/api/collab/invites", None).await, json!([]));

    // ben has it among his playlists and changes its songs
    let row = e.listed(&e.ben, pid).await.expect("in ben's list");
    assert_eq!(row["collab"], json!({ "role": "collaborator", "owner": "anna", "members": 1 }));
    assert_eq!(row["read_only"], json!(false));
    assert!(row.get("invite_token").is_none());
    assert_eq!(e.listed(&e.anna, pid).await.unwrap()["collab"], json!({ "role": "owner", "owner": "anna", "members": 1 }));
    e.ok(&e.ben, "POST", &format!("/api/playlists/{pid}/tracks"), Some(json!({ "track_ids": [3] }))).await;
    let items = e.ok(&e.ben, "GET", &format!("/api/playlists/{pid}"), None).await["items"].clone();
    let first = items[0]["id"].as_i64().unwrap();
    e.ok(&e.ben, "POST", &format!("/api/playlists/{pid}/move"), Some(json!({ "item_id": first, "to": 2 }))).await;
    assert_eq!(e.titles(&e.anna, pid).await, ["Song 2", "Song 3", "Song 1"]);
    let second = items[1]["id"].as_i64().unwrap();
    e.ok(&e.ben, "DELETE", &format!("/api/playlists/{pid}/items/{second}"), None).await;
    assert_eq!(e.titles(&e.anna, pid).await, ["Song 3", "Song 1"]);

    // only the owner renames, shares, toggles and invites
    assert_eq!(e.status(&e.ben, "PATCH", &format!("/api/playlists/{pid}"), Some(json!({ "name": "Mine" }))).await, StatusCode::NOT_FOUND);
    assert_eq!(e.status(&e.ben, "POST", &format!("/api/playlists/{pid}/share"), None).await, StatusCode::NOT_FOUND);
    assert_eq!(e.status(&e.ben, "POST", &format!("/api/playlists/{pid}/collaborators"), Some(json!({ "username": "cara" }))).await, StatusCode::NOT_FOUND);
    assert_eq!(e.status(&e.ben, "POST", &format!("/api/playlists/{pid}/invite-link"), None).await, StatusCode::NOT_FOUND);

    // others see nothing
    assert_eq!(e.status(&e.dan, "GET", &format!("/api/playlists/{pid}"), None).await, StatusCode::NOT_FOUND);
    assert_eq!(e.status(&e.dan, "POST", &format!("/api/playlists/{pid}/tracks"), Some(json!({ "track_ids": [4] }))).await, StatusCode::NOT_FOUND);

    // cara declines an invite
    e.ok(&e.anna, "POST", &format!("/api/playlists/{pid}/collaborators"), Some(json!({ "username": "cara" }))).await;
    assert_eq!(e.ok(&e.cara, "GET", "/api/collab/invites", None).await.as_array().unwrap().len(), 1);
    e.ok(&e.cara, "POST", &format!("/api/collab/invites/{pid}/decline"), None).await;
    assert_eq!(e.ok(&e.cara, "GET", "/api/collab/invites", None).await, json!([]));

    // a member's Delete is Leave: the playlist stays anna's
    e.ok(&e.ben, "DELETE", &format!("/api/playlists/{pid}"), None).await;
    assert!(e.listed(&e.ben, pid).await.is_none());
    assert_eq!(e.titles(&e.anna, pid).await, ["Song 3", "Song 1"]);
}

#[tokio::test]
async fn invite_link_toggle_and_removal() {
    let e = setup("link").await;
    let pid = e.playlist().await;
    let link = e.ok(&e.anna, "POST", &format!("/api/playlists/{pid}/invite-link"), None).await;
    let token = link["token"].as_str().unwrap().to_string();
    assert_eq!(link["path"], json!(format!("/#/collab/{token}")));
    assert_eq!(e.ok(&e.anna, "POST", &format!("/api/playlists/{pid}/invite-link"), None).await["token"], json!(token), "the same link again");

    let pasted = format!("Join my playlist: https://music.example/#/collab/{token}");
    let c = e.ok(&e.cara, "POST", "/api/collab/join/check", Some(json!({ "link": pasted }))).await;
    assert_eq!((c["name"].clone(), c["owner"].clone(), c["count"].clone(), c["joined"].clone(), c["own"].clone()),
        (json!("Road trip"), json!("anna"), json!(2), json!(false), json!(false)));
    assert_eq!(e.status(&e.anna, "POST", "/api/collab/join", Some(json!({ "link": pasted }))).await, StatusCode::BAD_REQUEST);
    e.ok(&e.cara, "POST", "/api/collab/join", Some(json!({ "link": pasted }))).await;
    assert_eq!(e.titles(&e.cara, pid).await, ["Song 1", "Song 2"]);
    let m = e.ok(&e.cara, "GET", &format!("/api/playlists/{pid}/collaborators"), None).await;
    assert_eq!((m["role"].clone(), m["invite_link"].clone()), (json!("collaborator"), Value::Null), "a member doesn't see the link");

    // off: paused for members, the link stops; on again: as it was
    e.ok(&e.anna, "PATCH", &format!("/api/playlists/{pid}"), Some(json!({ "collaborative": false }))).await;
    assert_eq!(e.status(&e.cara, "GET", &format!("/api/playlists/{pid}"), None).await, StatusCode::NOT_FOUND);
    assert!(e.listed(&e.cara, pid).await.is_none());
    assert_eq!(e.status(&e.dan, "POST", "/api/collab/join", Some(json!({ "link": token }))).await, StatusCode::NOT_FOUND);
    e.ok(&e.anna, "PATCH", &format!("/api/playlists/{pid}"), Some(json!({ "collaborative": true }))).await;
    assert!(e.listed(&e.cara, pid).await.is_some());

    // a new link ends the old one; members stay
    e.ok(&e.anna, "DELETE", &format!("/api/playlists/{pid}/invite-link"), None).await;
    assert_eq!(e.status(&e.dan, "POST", "/api/collab/join", Some(json!({ "link": token }))).await, StatusCode::NOT_FOUND);
    assert!(e.listed(&e.cara, pid).await.is_some());

    // the owner removes a member
    let cara_id = e.st.db.one("SELECT id FROM users WHERE username='cara'", p![]).unwrap().unwrap().i("id");
    e.ok(&e.anna, "DELETE", &format!("/api/playlists/{pid}/collaborators/{cara_id}"), None).await;
    assert_eq!(e.status(&e.cara, "GET", &format!("/api/playlists/{pid}"), None).await, StatusCode::NOT_FOUND);
    // and someone else can't remove anyone
    assert_eq!(e.status(&e.dan, "DELETE", &format!("/api/playlists/{pid}/collaborators/{cara_id}"), None).await, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn liked_songs_disabled_owner_and_delete() {
    let e = setup("ends").await;
    let liked = e.ok(&e.anna, "GET", "/api/playlists", None).await[0]["id"].as_i64().unwrap();
    assert_eq!(e.status(&e.anna, "PATCH", &format!("/api/playlists/{liked}"), Some(json!({ "collaborative": true }))).await, StatusCode::BAD_REQUEST);
    assert_eq!(e.status(&e.anna, "POST", &format!("/api/playlists/{liked}/invite-link"), None).await, StatusCode::BAD_REQUEST);

    let pid = e.playlist().await;
    e.ok(&e.anna, "POST", &format!("/api/playlists/{pid}/collaborators"), Some(json!({ "username": "ben" }))).await;
    e.ok(&e.ben, "POST", &format!("/api/collab/invites/{pid}/accept"), None).await;
    // a disabled owner ends it for members
    e.st.db.execute("UPDATE users SET disabled=1 WHERE username='anna'", p![]).unwrap();
    assert_eq!(e.status(&e.ben, "GET", &format!("/api/playlists/{pid}"), None).await, StatusCode::NOT_FOUND);
    e.st.db.execute("UPDATE users SET disabled=0 WHERE username='anna'", p![]).unwrap();
    assert!(e.listed(&e.ben, pid).await.is_some());

    // deleting the playlist takes its members and invites with it
    e.ok(&e.anna, "DELETE", &format!("/api/playlists/{pid}"), None).await;
    assert_eq!(e.st.db.one("SELECT COUNT(*) n FROM playlist_collaborators", p![]).unwrap().unwrap().i("n"), 0);
    assert!(e.listed(&e.ben, pid).await.is_none());
}

#[tokio::test]
async fn a_new_playlist_can_start_collaborative_with_its_link() {
    let e = setup("new").await;
    let r = e.ok(&e.anna, "POST", "/api/playlists", Some(json!({ "name": "Party", "collaborative": true }))).await;
    let pid = r["id"].as_i64().unwrap();
    let token = r["invite_link"]["token"].as_str().unwrap().to_string();
    assert_eq!(r["invite_link"]["path"], json!(format!("/#/collab/{token}")));
    assert_eq!(e.listed(&e.anna, pid).await.unwrap()["collab"]["role"], json!("owner"));
    e.ok(&e.ben, "POST", "/api/collab/join", Some(json!({ "link": token }))).await;
    assert!(e.listed(&e.ben, pid).await.is_some());
    // without the box: an ordinary playlist, no link
    let r = e.ok(&e.anna, "POST", "/api/playlists", Some(json!({ "name": "Solo" }))).await;
    assert!(r.get("invite_link").is_none());
    assert_eq!(e.listed(&e.anna, r["id"].as_i64().unwrap()).await.unwrap()["collab"], Value::Null);
}

#[test]
fn invite_tokens_in_pasted_text() {
    assert_eq!(invite_token_of("https://x.example/#/collab/AbCdEfGh12_-").as_deref(), Some("AbCdEfGh12_-"));
    assert_eq!(invite_token_of("<https://x.example/collab/AbCdEfGh12>.").as_deref(), Some("AbCdEfGh12"));
    assert_eq!(invite_token_of("  AbCdEfGh12  ").as_deref(), Some("AbCdEfGh12"));
    assert_eq!(invite_token_of("https://x.example/s/AbCdEfGh12 and more"), None);
    assert_eq!(invite_token_of(""), None);
}
