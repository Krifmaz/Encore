//! Clear waiting (`POST /api/requests/clear-waiting`) through the real router: what it cancels (pending,
//! queued, waiting, searching) and what it never touches (downloading, monitoring, importing, finished ones),
//! whose (the caller's own; everyone's for an admin unless `mine`), the count of a dry run, the words, the
//! request's place in the worker's queue (a search under way stops, nothing starts on the old state), and a
//! waiting import that holds its downloaded files (left alone: its own Cancel asks first).
use super::*;
use crate::test_support::{call, TestServer};
use axum::http::StatusCode;
use std::time::Duration;

/// An account without a password (signed in with a session made here; bcrypt would cost a second each).
fn account(st: &AppState, name: &str, role: &str) -> (i64, String) {
    let id = st.db.execute("INSERT INTO users (username, password_hash, role, created_at) VALUES (?, '-', ?, ?)", p![name, role, now()]).unwrap();
    let (t, _) = crate::auth::create_session(st, id, "test", "1.2.3.4").unwrap();
    (id, format!("encore_session={t}"))
}

fn insert(st: &AppState, owner: i64, title: &str, status: &str) -> i64 {
    let t = now();
    st.db.execute("INSERT INTO requests (user_id, kind, artist, title, status, message, detail, created_at, updated_at) VALUES (?,?,?,?,?,?,?,?,?)",
        p![owner, "track", "Duumu", title, status, "", "{}", t, t]).unwrap()
}

fn status(st: &AppState, id: i64) -> (String, String) {
    let r = st.db.one("SELECT status, message FROM requests WHERE id=?", p![id]).unwrap().unwrap();
    (r.s("status"), r.s("message"))
}

const ALL: [&str; 10] = ["pending", "queued", "waiting", "searching", "downloading", "monitoring", "importing", "completed", "failed", "cancelled"];

#[test]
fn the_words() {
    assert_eq!(waiting_text(612, true), "Cancelled 612 waiting requests");
    assert_eq!(waiting_text(1, true), "Cancelled 1 waiting request");
    assert_eq!(waiting_text(0, true), "Nothing is waiting");
    assert_eq!(waiting_text(612, false), "612 waiting requests");
    assert_eq!(waiting_text(0, false), "Nothing is waiting");
}

#[tokio::test]
async fn clear_waiting_cancels_only_what_waits_and_only_what_is_yours() {
    let ts = TestServer::new("clear-waiting");
    let st = ts.st.clone();
    let app = ts.app();
    let (sam, sam_c) = account(&st, "sam", "user");
    let (pat, _) = account(&st, "pat", "user");
    let (_, boss_c) = account(&st, "boss", "admin");
    let mut rows: Vec<(i64, i64, &str)> = vec![];
    for owner in [sam, pat] {
        for s in ALL {
            rows.push((insert(&st, owner, &format!("{s} of {owner}"), s), owner, s));
        }
    }
    let untouched = |rows: &[(i64, i64, &str)], which: &dyn Fn(i64, &str) -> bool| {
        for (id, owner, s) in rows {
            if !which(*owner, s) {
                assert_eq!(status(&st, *id).0, *s, "request {id} ({s} of {owner}) was left as it was");
            }
        }
    };
    // a search under way holds the request's place in the worker's queue: the clear stops it
    // downloaded, and its import waits for the library storage: it keeps its files, Clear waiting leaves it
    // alone (as one whose files were kept to import again)
    let kept = insert(&st, sam, "kept of sam", "waiting");
    st.db.execute("UPDATE requests SET progress=100, detail=? WHERE id=?",
        p![json!({ "kept": { "root": "/staging/1", "files": ["/staging/1/a.flac"] }, "wait": { "reason": "storage_import", "next_try_at": 1.0 } }).to_string(), kept]).unwrap();
    let kept_files = insert(&st, pat, "kept of pat", "waiting");
    st.db.execute("UPDATE requests SET detail=? WHERE id=?", p![json!({ "kept": { "files": ["/staging/2/b.flac"] } }).to_string(), kept_files]).unwrap();
    let searching_sam = rows.iter().find(|(_, o, s)| *o == sam && *s == "searching").unwrap().0;
    let hold = worker::hold_idle(&st, searching_sam, Duration::from_secs(1)).await.unwrap();
    let search = worker::take_turn(&st, searching_sam, hold, std::future::pending::<()>());
    assert!(st.requests.tasks.taken(searching_sam));

    // a member: the dry run counts only their own waiting ones, and changes nothing
    let (code, v, _) = call(&app, "POST", "/api/requests/clear-waiting", &sam_c, Some(json!({ "dry_run": true }))).await;
    assert_eq!(code, StatusCode::OK, "{v}");
    assert_eq!(v, json!({ "count": 4, "text": "4 waiting requests" }));
    untouched(&rows, &|_, _| false);
    // … and even asking for everyone's (mine false) a member clears only their own
    let (code, v, _) = call(&app, "POST", "/api/requests/clear-waiting", &sam_c, Some(json!({ "mine": false }))).await;
    assert_eq!(code, StatusCode::OK, "{v}");
    assert_eq!(v, json!({ "count": 4, "text": "Cancelled 4 waiting requests" }));
    for (id, owner, s) in &rows {
        if *owner == sam && WAITING.contains(s) {
            assert_eq!(status(&st, *id), ("cancelled".to_string(), "Cancelled by sam".to_string()));
        }
    }
    untouched(&rows, &|o, s| o == sam && WAITING.contains(&s));
    // the search's task was stopped, and its place is free again
    assert!(search.await.unwrap_err().is_cancelled());
    assert!(!st.requests.tasks.taken(searching_sam));
    // again: nothing is waiting
    let (_, v, _) = call(&app, "POST", "/api/requests/clear-waiting", &sam_c, Some(json!({}))).await;
    assert_eq!(v, json!({ "count": 0, "text": "Nothing is waiting" }));

    // an admin with Only mine: their own (none)
    let (_, v, _) = call(&app, "POST", "/api/requests/clear-waiting", &boss_c, Some(json!({ "mine": true, "dry_run": true }))).await;
    assert_eq!(v["count"], 0);
    // an admin: everyone's
    let (_, v, _) = call(&app, "POST", "/api/requests/clear-waiting", &boss_c, Some(json!({ "dry_run": true }))).await;
    assert_eq!(v, json!({ "count": 4, "text": "4 waiting requests" }));
    let (_, v, _) = call(&app, "POST", "/api/requests/clear-waiting", &boss_c, None).await;
    assert_eq!(v, json!({ "count": 4, "text": "Cancelled 4 waiting requests" }));
    for (id, owner, s) in &rows {
        if *owner == pat && WAITING.contains(s) {
            assert_eq!(status(&st, *id), ("cancelled".to_string(), "Cancelled by boss".to_string()));
        }
    }
    // downloads, monitoring and imports already running keep going; finished ones stay finished
    untouched(&rows, &|_, s| WAITING.contains(&s));
    // the waiting imports that hold their downloaded files were never counted nor cancelled
    for id in [kept, kept_files] {
        assert_eq!(status(&st, id).0, "waiting", "request {id} keeps its files");
    }
    // signed out: 401
    let (code, _, _) = call(&app, "POST", "/api/requests/clear-waiting", "", Some(json!({ "dry_run": true }))).await;
    assert_eq!(code, StatusCode::UNAUTHORIZED);
    ts.remove();
}
