//! One row per song: a request whose last attempt failed or was cancelled is reused when the song is asked
//! for again (reset like Retry, the attempt counted, the old message in its log) instead of a new row; a
//! manual pick for an existing request reuses its row too (no network).
use super::*;
use crate::auth;

fn state(name: &str) -> AppState {
    let dir = std::env::temp_dir().join(format!("encore-reuse-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let _env = crate::TEST_ENV.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var("ENCORE_CONFIG_DIR", &dir);
    let st = crate::build_state().unwrap();
    st.cfg.set_many(&json!({ "auto_approve": false }));
    st
}

fn user(st: &AppState, name: &str, role: &str) -> Row {
    let id = auth::create_user(st, name, "a-long-password-1", role, None, None).unwrap();
    st.db.one("SELECT * FROM users WHERE id=?", p![id]).unwrap().unwrap()
}

fn row(st: &AppState, id: i64) -> Value {
    get_request(st, id).unwrap()
}

fn song(artist: &str, title: &str) -> NewRequest {
    NewRequest { kind: "track".into(), artist: Some(artist.into()), title: Some(title.into()), length: Some(200.0), isrc: Some("USX11234567".into()), ..Default::default() }
}

fn rows_for(st: &AppState, title: &str) -> i64 {
    st.db.one("SELECT COUNT(*) n FROM requests WHERE lower(title)=lower(?)", p![title]).unwrap().unwrap().i("n")
}

#[test]
fn a_failed_or_cancelled_twin_is_reused() {
    let st = state("twin");
    let u = user(&st, "owner1", "admin");
    let first = create_request(&st, &u, &song("Band", "Song"), None, None).unwrap();
    assert_eq!(row(&st, first)["detail"]["attempts"], Value::Null, "a first request counts no attempts yet");
    // still queued: asking again is the usual 409 with the existing id
    let e = create_request(&st, &u, &song("Band", "Song"), None, None).err().expect("duplicate refused");
    assert_eq!(e.status.as_u16(), 409);
    assert_eq!(e.extra.as_ref().unwrap()["id"], first);
    // it failed: asking again brings the same row back, reset like Retry
    st.db.execute("UPDATE requests SET status='failed', message='All sources exhausted (slskd: nothing found)', source='slskd', progress=40 WHERE id=?", p![first]).unwrap();
    let again = create_request(&st, &u, &song("Band", "Song"), None, None).unwrap();
    assert_eq!(again, first);
    let r = row(&st, first);
    assert_eq!(r["status"], "queued");
    assert_eq!(r["message"], "Requested again");
    assert!(r["source"].is_null(), "{r}");
    assert_eq!(r["progress"].as_f64(), Some(0.0));
    assert_eq!(r["detail"]["attempts"], 2);
    assert_eq!(r["detail"]["log"], json!(["attempt 1 failed: All sources exhausted (slskd: nothing found)"]));
    assert_eq!(r["detail"]["isrc"], "USX11234567");
    assert_eq!(r["detail"]["length"], 200.0);
    assert_eq!(rows_for(&st, "Song"), 1);
    // cancelled, then asked for by someone else: the row moves to them, waits for approval, counts attempt 3
    let other = user(&st, "friend", "user");
    st.db.execute("UPDATE requests SET status='cancelled', message='Cancelled by owner1' WHERE id=?", p![first]).unwrap();
    assert_eq!(create_request(&st, &other, &song("band", "song"), None, None).unwrap(), first, "the match ignores case");
    let r = row(&st, first);
    assert_eq!(r["user_id"], other.i("id"));
    assert_eq!(r["status"], "pending");
    assert_eq!(r["message"], "Waiting for admin approval");
    assert_eq!(r["detail"]["attempts"], 3);
    assert_eq!(r["detail"]["log"][1], "attempt 2 cancelled: Cancelled by owner1");
    assert_eq!(rows_for(&st, "Song"), 1);
    // the list carries the count
    let d: Value = serde_json::from_str(&st.db.one("SELECT detail FROM requests WHERE id=?", p![first]).unwrap().unwrap().s("detail")).unwrap();
    assert_eq!(d["attempts"].as_u64().unwrap_or(1), 3);
    // denied is not reused: that is a new ask for approval, as before
    st.db.execute("UPDATE requests SET status='denied' WHERE id=?", p![first]).unwrap();
    let fresh = create_request(&st, &other, &song("Band", "Song"), None, None).unwrap();
    assert_ne!(fresh, first);
    assert_eq!(rows_for(&st, "Song"), 2);
}

#[test]
fn twins_are_matched_by_mbid_and_never_inside_a_discography() {
    let st = state("mbid");
    let u = user(&st, "owner2", "admin");
    let album = NewRequest { kind: "album".into(), mbid: Some("11111111-2222-3333-4444-555555555555".into()), artist: Some("Band".into()), title: Some("Album".into()), ..Default::default() };
    let id = create_request(&st, &u, &album, None, None).unwrap();
    st.db.execute("UPDATE requests SET status='failed', message='nope' WHERE id=?", p![id]).unwrap();
    // the same release group under another spelling of its title: the id decides
    let renamed = NewRequest { title: Some("Album (Deluxe)".into()), cover_url: Some("https://x/cover.jpg".into()), ..album.clone() };
    assert_eq!(create_request(&st, &u, &renamed, None, None).unwrap(), id);
    let r = row(&st, id);
    assert_eq!(r["title"], "Album (Deluxe)");
    assert_eq!(r["cover_url"], "https://x/cover.jpg");
    assert_eq!(r["detail"]["attempts"], 2);
    // a failed child of a discography request is left alone: the direct ask gets its own row
    let artist = create_request(&st, &u, &NewRequest { kind: "artist".into(), mbid: Some("aaaaaaaa-2222-3333-4444-555555555555".into()), artist: Some("Band".into()), ..Default::default() }, None, None).unwrap();
    let child = NewRequest { kind: "album".into(), mbid: Some("bbbbbbbb-2222-3333-4444-555555555555".into()), artist: Some("Band".into()), title: Some("Second".into()), ..Default::default() };
    let cid = create_request(&st, &u, &child, Some(artist), None).unwrap();
    st.db.execute("UPDATE requests SET status='failed', message='nope' WHERE id=?", p![cid]).unwrap();
    let direct = create_request(&st, &u, &child, None, None).unwrap();
    assert_ne!(direct, cid);
    assert!(row(&st, direct)["parent_id"].is_null());
    // and a new child never reuses a top-level twin
    st.db.execute("UPDATE requests SET status='failed' WHERE id=?", p![direct]).unwrap();
    let another_child = create_request(&st, &u, &child, Some(artist), None).unwrap();
    assert_ne!(another_child, direct);
    assert_eq!(row(&st, another_child)["parent_id"], artist);
}

#[test]
fn a_pick_for_an_existing_request_reuses_its_row() {
    let st = state("pick");
    let u = user(&st, "owner3", "admin");
    let id = create_request(&st, &u, &song("Band", "Pickme"), None, None).unwrap();
    st.db.execute("UPDATE requests SET status='failed', message='All sources exhausted' WHERE id=?", p![id]).unwrap();
    let (pid, status) = crate::routes::picker::create_pick_for(&st, &u, &song("Band", "Pickme"), "slskd", json!({ "username": "x", "files": [] }), "Band – Pickme", Some(id)).unwrap();
    assert_eq!((pid, status.as_str()), (id, "queued"));
    let r = row(&st, id);
    assert_eq!(r["message"], "Manual pick: Band – Pickme");
    assert_eq!(r["detail"]["pick"]["source"], "slskd");
    assert_eq!(r["detail"]["attempts"], 2);
    assert_eq!(r["detail"]["log"][0], "attempt 1 failed: All sources exhausted");
    // a pick from the search box for a song whose request failed lands on that row as well
    st.db.execute("UPDATE requests SET status='failed', message='Soulseek: gone' WHERE id=?", p![id]).unwrap();
    let (pid2, _) = crate::routes::picker::create_pick(&st, &u, &song("Band", "Pickme"), "ytmusic", json!({ "url": "https://music.youtube.com/watch?v=1" }), "Pickme").unwrap();
    assert_eq!(pid2, id);
    let r = row(&st, id);
    assert_eq!(r["detail"]["pick"]["source"], "ytmusic");
    assert_eq!(r["detail"]["attempts"], 3);
    assert_eq!(r["detail"]["log"], json!(["attempt 1 failed: All sources exhausted", "attempt 2 failed: Soulseek: gone"]));
    assert_eq!(rows_for(&st, "Pickme"), 1);
    // a pick for a running request takes the row over too (the pick replaces the search)
    st.db.execute("UPDATE requests SET status='searching', message='Trying slskd…', source='slskd' WHERE id=?", p![id]).unwrap();
    let (pid3, _) = crate::routes::picker::create_pick_for(&st, &u, &song("Band", "Pickme"), "slskd", json!({ "username": "y", "files": [] }), "Band – Pickme", Some(id)).unwrap();
    assert_eq!(pid3, id);
    let r = row(&st, id);
    assert_eq!(r["status"], "queued");
    assert_eq!(r["detail"]["attempts"], 4);
    assert_eq!(r["detail"]["log"][2], "attempt 3 searching: Trying slskd…");
}

#[test]
fn retry_detail_counts_attempts_and_keeps_a_bounded_log() {
    let d = detail_for_retry(&json!({ "attempts": 2, "log": ["slskd: nothing", "x".repeat(600)] }), "failed", "boom", json!({ "length": 1.5 }));
    assert_eq!(d["attempts"], 3);
    assert_eq!(d["length"], 1.5);
    assert_eq!(d["log"][0], "slskd: nothing");
    assert_eq!(d["log"][1].as_str().unwrap().chars().count(), KEPT_LOG_CHARS + 1, "long lines are cut");
    assert!(d["log"][1].as_str().unwrap().ends_with('…'));
    assert_eq!(d["log"][2], "attempt 2 failed: boom");
    // a first request has no counter yet: its retry is attempt 2; an empty message leaves no trailing colon
    let d = detail_for_retry(&json!({}), "cancelled", "", json!({}));
    assert_eq!(d["attempts"], 2);
    assert_eq!(d["log"], json!(["attempt 1 cancelled"]));
    // the kept log never grows past the limit
    let many: Vec<String> = (0..100).map(|i| format!("line {i}")).collect();
    let d = detail_for_retry(&json!({ "attempts": 50, "log": many }), "failed", "again", json!({}));
    assert_eq!(d["log"].as_array().unwrap().len(), KEPT_LOG_LINES);
    assert_eq!(d["log"][KEPT_LOG_LINES - 1], "attempt 50 failed: again");
    assert_eq!(d["log"][0], "line 61");
}
