//! The requests of one person (`user`) through the real router: the list (`GET /api/requests?user=`), the
//! people who have requests (`GET /api/requests/users`, admins), and the bulk actions (Retry failed, Download
//! all now, Clear finished, Clear waiting) kept to that person. A member only ever sees and changes their own,
//! whatever `user` says.
use super::*;
use crate::test_support::{call, TestServer};
use axum::http::StatusCode;

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

fn status(st: &AppState, id: i64) -> String {
    st.db.one("SELECT status FROM requests WHERE id=?", p![id]).unwrap().map(|r| r.s("status")).unwrap_or_else(|| "gone".into())
}

/// The owners of the rows a list call answers with.
async fn owners(app: &Router, uri: &str, cookie: &str) -> Vec<i64> {
    let (code, v, _) = call(app, "GET", uri, cookie, None).await;
    assert_eq!(code, StatusCode::OK, "{v}");
    let mut o: Vec<i64> = v.as_array().unwrap().iter().map(|r| r["user_id"].as_i64().unwrap()).collect();
    o.sort();
    o.dedup();
    o
}

#[test]
fn whose_requests() {
    let row = |id: i64, role: &str| {
        let mut r = Row::new();
        r.insert("id".into(), json!(id));
        r.insert("role".into(), json!(role));
        r
    };
    let (admin, member) = (row(1, "admin"), row(2, "user"));
    assert_eq!(whose(&admin, false, None), None, "everyone's");
    assert_eq!(whose(&admin, false, Some(7)), Some(7), "one person's");
    assert_eq!(whose(&admin, true, Some(7)), Some(1), "Only mine wins");
    assert_eq!(whose(&member, false, None), Some(2));
    assert_eq!(whose(&member, false, Some(7)), Some(2), "a member only ever their own");
    assert_eq!(user_id(Some("12")), Some(12));
    assert_eq!(user_id(Some("")), None);
    assert_eq!(user_id(Some("caz")), None);
    assert_eq!(user_id(None), None);
}

#[tokio::test]
async fn an_admin_looks_at_one_persons_requests_and_a_member_only_at_their_own() {
    let ts = TestServer::new("requests-user");
    let st = ts.st.clone();
    let app = ts.app();
    let (sam, sam_c) = account(&st, "sam", "user");
    let (caz, _) = account(&st, "Caz", "user");
    let (boss, boss_c) = account(&st, "boss", "admin");
    let (_, nobody) = account(&st, "nobody", "user"); // no requests: not in the list of people
    let _ = nobody;
    let mut rows = vec![];
    for (owner, statuses) in [(sam, vec!["failed", "completed", "queued"]), (caz, vec!["failed", "failed", "completed", "queued", "waiting"]), (boss, vec!["completed"])] {
        for s in statuses {
            rows.push((insert(&st, owner, &format!("{s} of {owner}"), s), owner, s));
        }
    }

    // the people who have requests, by name, with their counts (admins only)
    let (code, v, _) = call(&app, "GET", "/api/requests/users", &boss_c, None).await;
    assert_eq!(code, StatusCode::OK, "{v}");
    assert_eq!(v, json!([{ "id": boss, "username": "boss", "count": 1 }, { "id": caz, "username": "Caz", "count": 5 }, { "id": sam, "username": "sam", "count": 3 }]));
    assert_eq!(call(&app, "GET", "/api/requests/users", &sam_c, None).await.0, StatusCode::FORBIDDEN);
    assert_eq!(call(&app, "GET", "/api/requests/users", "", None).await.0, StatusCode::UNAUTHORIZED);

    // the list: everyone's, one person's, Only mine; a user id that isn't one reads as everyone's
    assert_eq!(owners(&app, "/api/requests", &boss_c).await, vec![sam, caz, boss]);
    assert_eq!(owners(&app, &format!("/api/requests?user={caz}"), &boss_c).await, vec![caz]);
    assert_eq!(owners(&app, &format!("/api/requests?user={caz}&status=failed"), &boss_c).await, vec![caz]);
    assert_eq!(owners(&app, "/api/requests?user=", &boss_c).await, vec![sam, caz, boss]);
    assert_eq!(owners(&app, "/api/requests?mine=true", &boss_c).await, vec![boss]);
    // a member: their own, whoever they ask for
    assert_eq!(owners(&app, &format!("/api/requests?user={caz}"), &sam_c).await, vec![sam]);
    assert_eq!(owners(&app, "/api/requests", &sam_c).await, vec![sam]);

    // Clear waiting and Clear finished, kept to Caz
    let (_, v, _) = call(&app, "POST", "/api/requests/clear-waiting", &boss_c, Some(json!({ "user": caz, "dry_run": true }))).await;
    assert_eq!(v, json!({ "count": 2, "text": "2 waiting requests" }));
    // (a member asking for Caz's counts only their own)
    let (_, v, _) = call(&app, "POST", "/api/requests/clear-waiting", &sam_c, Some(json!({ "user": caz, "dry_run": true }))).await;
    assert_eq!(v["count"], 1);
    let (_, v, _) = call(&app, "POST", "/api/requests/clear", &boss_c, Some(json!({ "user": caz, "statuses": ["completed"] }))).await;
    assert_eq!(v["removed"], 1, "{v}");
    for (id, owner, s) in &rows {
        if *s == "completed" {
            assert_eq!(status(&st, *id), if *owner == caz { "gone" } else { "completed" }, "{s} of {owner}");
        }
    }

    // Download all now, kept to Caz: only Caz's queued request is woken; the words don't say "your"
    let (code, v, _) = call(&app, "POST", "/api/requests/download-now", &boss_c, Some(json!({ "user": caz }))).await;
    assert_eq!(code, StatusCode::OK, "{v}");
    assert_eq!((v["woken"].as_u64(), v["text"].as_str()), (Some(1), Some("Woke 1 waiting request")), "{v}");
    let (_, v, _) = call(&app, "POST", "/api/requests/download-now", &boss_c, Some(json!({}))).await;
    assert_eq!(v["woken"], 2, "everyone's: Caz's and Sam's queued ones");

    // Retry failed, kept to Caz: Caz's two failed ones go back in the queue, Sam's stays failed
    let (_, v, _) = call(&app, "POST", "/api/requests/retry-failed", &boss_c, Some(json!({ "user": caz }))).await;
    assert_eq!(v["retried"], 2, "{v}");
    for (id, owner, s) in &rows {
        if *s == "failed" {
            assert_eq!(status(&st, *id), if *owner == caz { "queued" } else { "failed" }, "{s} of {owner}");
        }
    }
    // the counts follow
    let (_, v, _) = call(&app, "GET", "/api/requests/users", &boss_c, None).await;
    assert_eq!(v[1], json!({ "id": caz, "username": "Caz", "count": 4 }));
    ts.remove();
}
