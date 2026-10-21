//! Download missing: which rows are requested again, which count as in progress, what the report says,
//! and the 409 that links a row to the request someone else already made (no network).
use super::*;
use crate::test_support::user;
use encore_sync::playlists::MissingFailure;
use encore_requests::create::{create_request, NewRequest};

fn state(name: &str) -> crate::test_support::TempState {
    crate::test_support::TempState::new(&format!("missing-{name}"))
}

fn playlist(st: &AppState, u: &Row) -> i64 {
    st.db.execute("INSERT INTO playlists (user_id, name, kind, source, sync_targets, created_at, updated_at) VALUES (?,?,?,?,?,?,?)",
        p![u.i("id"), "Mix", "playlist", "manual", "[]", now(), now()]).unwrap()
}

/// A song that is not in the library, linked to `request_id` when given (the id may be of a request that no longer exists).
fn item(st: &AppState, pid: i64, pos: i64, title: &str, artist: &str, request_id: Option<i64>) -> i64 {
    st.db.execute("INSERT INTO playlist_items (playlist_id, position, title, artist, request_id, added_at) VALUES (?,?,?,?,?,?)",
        p![pid, pos, title, artist, request_id, now()]).unwrap()
}

/// A request row in a given state.
fn request(st: &AppState, u: &Row, artist: &str, title: &str, status: &str) -> i64 {
    st.db.execute("INSERT INTO requests (user_id, kind, artist, title, status, message, detail, created_at, updated_at) VALUES (?,?,?,?,?,?,?,?,?)",
        p![u.i("id"), "track", artist, title, status, "", "{}", now(), now()]).unwrap()
}

/// The request a playlist row follows: (request id, its status; "" when the request is gone).
fn linked(st: &AppState, iid: i64) -> (i64, String) {
    let it = st.db.one("SELECT request_id FROM playlist_items WHERE id=?", p![iid]).unwrap().unwrap();
    let rid = it.i("request_id");
    let status = st.db.one("SELECT status FROM requests WHERE id=?", p![rid]).unwrap().map(|r| r.s("status")).unwrap_or_default();
    (rid, status)
}

#[tokio::test]
async fn gone_failed_cancelled_and_denied_requests_are_asked_again() {
    let st = state("rules");
    let u = user(&st, "owner1", "admin");
    let pid = playlist(&st, &u);
    // never requested
    let fresh = item(&st, pid, 0, "Fresh", "Band", None);
    // its request row was deleted
    let gone = item(&st, pid, 1, "Gone", "Band", Some(999_999));
    // ended without the song
    let mut again = vec![];
    for (i, s) in ["failed", "cancelled", "denied"].iter().enumerate() {
        let rid = request(&st, &u, "Band", &format!("Over {s}"), s);
        again.push((item(&st, pid, 2 + i as i64, &format!("Over {s}"), "Band", Some(rid)), rid));
    }
    // waiting, running or done: left alone
    let mut waiting = vec![];
    for (i, s) in ["pending", "queued", "completed"].iter().enumerate() {
        let rid = request(&st, &u, "Band", &format!("Still {s}"), s);
        waiting.push((item(&st, pid, 5 + i as i64, &format!("Still {s}"), "Band", Some(rid)), rid));
    }
    // in the library already
    st.db.execute("INSERT INTO playlist_items (playlist_id, position, title, artist, track_id, added_at) VALUES (?,?,?,?,?,?)", p![pid, 8, "Owned", "Band", 1, now()]).unwrap();
    // nothing to search for: no title
    let nameless = item(&st, pid, 9, "", "Band", None);

    let rep = encore_requests::missing::request_missing(&*st, &u, pid, false, None).unwrap();
    assert_eq!((rep.requested, rep.in_progress), (5, 3), "{rep:?}");
    assert_eq!(rep.failed, vec![MissingFailure { title: "Band".into(), reason: "Track needs a MusicBrainz id or artist + title".into() }]);

    assert_eq!(linked(&st, fresh).1, "queued");
    let (rid, status) = linked(&st, gone);
    assert!(rid != 999_999 && status == "queued", "the gone request is replaced: #{rid} {status}");
    for (iid, old) in &again {
        let (rid, status) = linked(&st, *iid);
        assert!(rid > 0 && status == "queued", "item {iid} should follow a live request, not #{old}: #{rid} {status}");
    }
    for (iid, old) in &waiting {
        assert_eq!(linked(&st, *iid).0, *old, "item {iid} keeps its request");
    }
    assert_eq!(linked(&st, nameless).0, 0);

    // a second click asks for nothing twice: everything is on its way now
    let rep = encore_requests::missing::request_missing(&*st, &u, pid, false, None).unwrap();
    assert_eq!((rep.requested, rep.in_progress, rep.failed.len()), (0, 8, 1), "{rep:?}");
}

#[tokio::test]
async fn an_already_requested_song_links_to_that_request() {
    let st = state("dup");
    let u = user(&st, "owner2", "admin");
    let other = user(&st, "friend", "user");
    let song = NewRequest { kind: "track".into(), artist: Some("Band".into()), title: Some("Shared".into()), ..Default::default() };
    // the friend asked first
    let theirs = create_request(&*st, &other, &song, None, None).unwrap();
    // asking again is a 409 that names the request
    let e = create_request(&*st, &u, &song, None, None).expect_err("a duplicate is refused");
    assert_eq!(e.status.as_u16(), 409);
    assert!(e.detail.starts_with("Already requested (#"), "{}", e.detail);
    assert_eq!(e.extra.as_ref().and_then(|x| x["id"].as_i64()), Some(theirs));

    let pid = playlist(&st, &u);
    let iid = item(&st, pid, 0, "Shared", "Band", None);
    let rep = encore_requests::missing::request_missing(&*st, &u, pid, false, None).unwrap();
    assert_eq!((rep.requested, rep.in_progress, rep.failed.len()), (0, 1, 0), "{rep:?}");
    assert_eq!(linked(&st, iid).0, theirs, "the row follows the friend's request");

    // the route reports the same numbers, and the rows carry the request's status
    let Json(v) = request_missing(State(st.clone()), User(u.clone()), Path(pid), None).await.unwrap();
    assert_eq!(v, json!({ "requested": 0, "in_progress": 1, "failed": [] }));
    let theirs_status = st.db.one("SELECT status FROM requests WHERE id=?", p![theirs]).unwrap().unwrap().s("status");
    let Json(d) = detail(State(st.clone()), User(u), Path(pid)).await.unwrap();
    assert_eq!(d["items"][0]["request_id"], json!(theirs));
    assert_eq!(d["items"][0]["request_status"], json!(theirs_status));
}

/// The auto-download asks again for a song whose request failed only after a wait that doubles with each
/// failed attempt (15 minutes, 30, an hour … at most a day) – it used to ask at every sync, every 15 minutes, for
/// days. "Download missing" (a person) asks at once.
#[tokio::test]
async fn the_auto_download_waits_longer_after_each_failed_attempt() {
    use encore_requests::missing::auto_retry_after;
    assert_eq!((auto_retry_after(1), auto_retry_after(2), auto_retry_after(3), auto_retry_after(8), auto_retry_after(40)), (900.0, 1800.0, 3600.0, 86_400.0, 86_400.0));
    let st = state("backoff");
    let u = user(&st, "owner1", "admin");
    st.db.execute("UPDATE users SET auto_download=1 WHERE id=?", p![u.i("id")]).unwrap();
    let pid = playlist(&st, &u);
    let rid = request(&st, &u, "Band", "Nowhere", "failed");
    st.db.execute("UPDATE requests SET detail=? WHERE id=?", p![json!({ "attempts": 3, "auto": true }).to_string(), rid]).unwrap();
    let iid = item(&st, pid, 0, "Nowhere", "Band", Some(rid));
    // failed just now, its third attempt: an hour's wait
    let rep = encore_requests::missing::request_missing(&*st, &u, pid, true, None).unwrap();
    assert_eq!(rep.requested, 0, "{rep:?}");
    assert_eq!(linked(&st, iid), (rid, "failed".to_string()));
    // 59 minutes later still not
    st.db.execute("UPDATE requests SET updated_at=? WHERE id=?", p![now() - 59.0 * 60.0, rid]).unwrap();
    assert_eq!(encore_requests::missing::request_missing(&*st, &u, pid, true, None).unwrap().requested, 0);
    // a person's Download missing asks at once
    let rep = encore_requests::missing::request_missing(&*st, &u, pid, false, None).unwrap();
    assert_eq!(rep.requested, 1, "{rep:?}");
    assert_eq!(linked(&st, iid), (rid, "queued".to_string()));
    // that was its fourth attempt: when it fails, the auto-download waits two hours, then asks
    st.db.execute("UPDATE requests SET status='failed', updated_at=? WHERE id=?", p![now() - 119.0 * 60.0, rid]).unwrap();
    assert_eq!(encore_requests::missing::request_missing(&*st, &u, pid, true, None).unwrap().requested, 0);
    st.db.execute("UPDATE requests SET updated_at=? WHERE id=?", p![now() - 121.0 * 60.0, rid]).unwrap();
    let rep = encore_requests::missing::request_missing(&*st, &u, pid, true, None).unwrap();
    assert_eq!(rep.requested, 1, "{rep:?}");
    assert_eq!(linked(&st, iid), (rid, "queued".to_string()));
}
