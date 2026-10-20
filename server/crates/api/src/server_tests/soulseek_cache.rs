//! Soulseek results kept for the picker (wf/req-slsk-cache.md), against the stand-in slskd of the tests
//! above, which answers each search text with `answers` once the search is over (as slskd 0.26.1 does).
//! The picker is asked through its HTTP route, as the apps ask it.
//!
//! Covered: a second picker search answers from what is kept, without a new search; Search again
//! (`fresh`) searches through the gate (3 s after the one before, one at a time) and replaces what is
//! kept; two pickers asking at once share one search; a picker closed while Soulseek searches still keeps
//! the answer; the worker's own searches fill what the picker reads (also the album search that fell back
//! to the album's name alone, for a picker opened for that request); while Soulseek isn't connected kept
//! rows still answer (without Search again) and nothing kept is a 409 as before; a busy Soulseek is a 429
//! that leaves what is kept alone; the Soulseek page's finished search is kept for the picker; a kept row
//! whose user went offline fails its pick with words saying so.
use super::*;

/// One user's answer: audio files in one folder (and a cover, which is not kept).
fn shared(user: &str, dir: &str, names: &[&str]) -> Value {
    let mut files: Vec<Value> = names.iter().map(|n| json!({ "filename": format!("{dir}\\{n}"), "size": 30_000_000, "bitRate": 0, "bitDepth": 16,
        "sampleRate": 44100, "length": 200, "extension": "flac", "code": 1, "isLocked": false })).collect();
    files.push(json!({ "filename": format!("{dir}\\cover.jpg"), "size": 1000 }));
    json!({ "username": user, "hasFreeUploadSlot": true, "uploadSpeed": 2_000_000, "queueLength": 0, "fileCount": files.len(), "lockedFileCount": 0,
        "token": 1, "files": files })
}

fn answers(f: &Shared, text: &str, v: Value) {
    f.lock().unwrap().answers.insert(text.to_string(), v);
}

fn posts(f: &Shared) -> Vec<(Instant, String)> {
    f.lock().unwrap().posts.clone()
}

async fn picker(app: &Router, cookie: &str, body: Value) -> (StatusCode, Value) {
    let (code, body, _) = call(app, "POST", "/api/picker/search", cookie, Some(body)).await;
    (code, body)
}

fn illuminate(extra: Value) -> Value {
    let mut b = json!({ "source": "slskd", "kind": "album", "artist": "Duumu", "title": "Illuminate EP", "query": "Duumu Illuminate EP", "track_count": 2 });
    for (k, v) in extra.as_object().unwrap() {
        b[k] = v.clone();
    }
    b
}

fn users(answer: &Value) -> Vec<String> {
    answer["items"].as_array().unwrap().iter().map(|r| r["payload"]["username"].as_str().unwrap().to_string()).collect()
}

/// The first picker search is live; the next one answers from what is kept (no search), also for the same
/// words typed in another order; Search again searches through the gate and replaces what is kept; two
/// pickers asking at once share one search.
#[tokio::test(start_paused = true)]
async fn the_picker_answers_from_what_is_kept_and_searches_again_through_the_gate() {
    let (ts, f) = setup("kept");
    let st = ts.st.clone();
    logged_in_already(&st, &f);
    f.lock().unwrap().search_secs = 2;
    answers(&f, "Duumu Illuminate EP", json!([shared("alice", "Music\\Duumu\\Illuminate EP", &["01 Lights.flac", "02 Shade.flac"])]));
    let app = ts.app();
    let cookie = admin_cookie(&app).await;

    let (code, live) = picker(&app, &cookie, illuminate(json!({}))).await;
    assert_eq!(code, StatusCode::OK, "{live}");
    assert_eq!((live["from"].as_str(), live["cached_at"].is_null(), live["found"].is_null()), (Some("live"), true, true), "{live}");
    assert_eq!((live["can_search"].as_bool(), live["notice"].is_null()), (Some(true), true));
    assert_eq!(users(&live), ["alice"]);
    assert_eq!(live["items"][0]["tracks"], 2, "the cover is not a track");
    assert_eq!(posts(&f).len(), 1);

    let (code, kept) = picker(&app, &cookie, illuminate(json!({}))).await;
    assert_eq!(code, StatusCode::OK, "{kept}");
    assert_eq!(kept["from"], "cache", "{kept}");
    let at = kept["cached_at"].as_i64().expect("when it was found");
    assert!((at as f64 - encore_core::time::now()).abs() < 5.0, "{at}");
    assert!(kept["found"].as_str().unwrap().starts_with("Found at "), "{kept}");
    assert_eq!((kept["query"].as_str(), kept["can_search"].as_bool(), kept["notice"].is_null()), (Some("Duumu Illuminate EP"), Some(true), true));
    assert_eq!(kept["items"], live["items"], "the same rows");
    // the same words in another order, typed: Soulseek's answer would be the same
    let (_, typed) = picker(&app, &cookie, illuminate(json!({ "query": "illuminate EP duumu" }))).await;
    assert_eq!(typed["from"], "cache", "{typed}");
    assert_eq!(posts(&f).len(), 1, "no new search");

    // Search again: a new search, 3 s after the one before at least, and its answer is what is kept now
    answers(&f, "Duumu Illuminate EP", json!([shared("bob", "Music\\Duumu - Illuminate EP (2024)", &["01.flac", "02.flac"])]));
    let (code, again) = picker(&app, &cookie, illuminate(json!({ "fresh": true }))).await;
    assert_eq!(code, StatusCode::OK, "{again}");
    assert_eq!((again["from"].as_str(), users(&again)), (Some("live"), vec!["bob".to_string()]), "{again}");
    let p = posts(&f);
    assert_eq!(p.len(), 2);
    assert!(p[1].0.duration_since(p[0].0) >= Duration::from_secs(3), "through the gate's 3 s");
    let (_, kept) = picker(&app, &cookie, illuminate(json!({}))).await;
    assert_eq!((kept["from"].as_str(), users(&kept)), (Some("cache"), vec!["bob".to_string()]), "{kept}");
    // `fresh` as the number 1 also searches again
    let (_, one) = picker(&app, &cookie, illuminate(json!({ "fresh": 1 }))).await;
    assert_eq!(one["from"], "live");
    assert_eq!(posts(&f).len(), 3);

    // two pickers asking the same words at once: one search
    let (a, b) = tokio::join!(picker(&app, &cookie, illuminate(json!({ "fresh": true }))), picker(&app, &cookie, illuminate(json!({ "fresh": true }))));
    assert_eq!((a.0, b.0), (StatusCode::OK, StatusCode::OK));
    assert_eq!((a.1["from"].as_str(), b.1["from"].as_str()), (Some("live"), Some("live")));
    assert_eq!(posts(&f).len(), 4, "one search for both");
    assert_eq!(f.lock().unwrap().most, 1, "never two searches at once");
    ts.remove();
}

/// A picker closed while Soulseek searches (the app went away): the search runs to its end, is not
/// stopped, and the next picker answers from it at once.
#[tokio::test(start_paused = true)]
async fn a_search_whose_picker_closed_is_kept() {
    let (ts, f) = setup("closed");
    let st = ts.st.clone();
    logged_in_already(&st, &f);
    f.lock().unwrap().search_secs = 4;
    answers(&f, "Duumu Illuminate EP", json!([shared("alice", "Music\\Duumu\\Illuminate EP", &["01.flac", "02.flac"])]));
    let app = ts.app();
    let cookie = admin_cookie(&app).await;
    let gone = tokio::time::timeout(Duration::from_secs(2), picker(&app, &cookie, illuminate(json!({})))).await;
    assert!(gone.is_err(), "the picker closed before the answer");
    assert_eq!(posts(&f).len(), 1);
    tokio::time::sleep(Duration::from_secs(8)).await;
    assert!(f.lock().unwrap().stopped.is_empty(), "the search was not stopped");
    let (code, kept) = picker(&app, &cookie, illuminate(json!({}))).await;
    assert_eq!((code, kept["from"].as_str(), users(&kept)), (StatusCode::OK, Some("cache"), vec!["alice".to_string()]), "{kept}");
    assert_eq!(posts(&f).len(), 1);
    ts.remove();
}

/// The worker's own searches are kept: a song's "Artist Title", and an album's search by its name alone
/// after "artist album" found nothing – the picker opened for that request answers from it, with the
/// words it was found with.
#[tokio::test(start_paused = true)]
async fn the_workers_searches_fill_what_the_picker_reads() {
    let (ts, f) = setup("worker");
    let st = ts.st.clone();
    logged_in_already(&st, &f);
    f.lock().unwrap().search_secs = 2;
    let app = ts.app();
    let cookie = admin_cookie(&app).await;

    // a song (the brackets are left out of the search, as the worker does)
    answers(&f, "Machine Time Moonlight Shadow", json!([shared("carol", "Music\\Machine Time", &["Moonlight Shadow (nightcore).flac"])]));
    let id = st.db.execute("INSERT INTO requests (user_id, kind, artist, title, status, message, detail, created_at, updated_at) VALUES (1,'track','Machine Time','Moonlight Shadow (nightcore)','queued','','{}',?,?)",
        p![encore_db::now(), encore_db::now()]).unwrap();
    encore_requests::worker::step(st.clone(), id).await;
    assert_eq!(posts(&f).iter().map(|p| p.1.as_str()).collect::<Vec<_>>(), ["Machine Time Moonlight Shadow"]);
    // kept with the request it was made for
    let key = encore_requests::search_cache::slskd_key("Machine Time Moonlight Shadow");
    assert_eq!(st.requests.search_cache.get(st.data_dir(), &key, encore_db::now()).map(|k| k.request_id), Some(Some(id)));
    let song = json!({ "source": "slskd", "kind": "track", "artist": "Machine Time", "title": "Moonlight Shadow (nightcore)",
        "query": "Machine Time Moonlight Shadow (nightcore)", "request_id": id });
    let (code, kept) = picker(&app, &cookie, song).await;
    assert_eq!((code, kept["from"].as_str()), (StatusCode::OK, Some("cache")), "{kept}");
    assert_eq!(users(&kept), ["carol"]);
    // the box's words come back: the kept search is the one they make (the brackets left out), so the apps
    // add no "for …" to "Found at …"
    assert_eq!(kept["query"], "Machine Time Moonlight Shadow (nightcore)");
    assert_eq!(posts(&f).len(), 1, "no new search");

    // an album: "Duumu Illuminate EP" finds nothing, "Illuminate EP" finds it
    answers(&f, "Illuminate EP", json!([shared("dave", "Music\\Duumu\\Illuminate EP", &["01.flac", "02.flac"])]));
    let id = st.db.execute("INSERT INTO requests (user_id, kind, artist, title, status, message, detail, created_at, updated_at) VALUES (1,'album','Duumu','Illuminate EP','queued','','{}',?,?)",
        p![encore_db::now(), encore_db::now()]).unwrap();
    encore_requests::worker::step(st.clone(), id).await;
    let texts: Vec<String> = posts(&f).into_iter().map(|p| p.1).collect();
    assert_eq!(texts[1..], ["Duumu Illuminate EP", "Illuminate EP"]);
    let (code, kept) = picker(&app, &cookie, illuminate(json!({ "request_id": id }))).await;
    assert_eq!((code, kept["from"].as_str(), kept["query"].as_str()), (StatusCode::OK, Some("cache"), Some("Illuminate EP")), "{kept}");
    assert_eq!(users(&kept), ["dave"]);
    // without the request (or for words typed by hand) the empty answer of "Duumu Illuminate EP" is what is kept
    let (_, plain) = picker(&app, &cookie, illuminate(json!({}))).await;
    assert_eq!((plain["from"].as_str(), plain["items"].as_array().map(Vec::len)), (Some("cache"), Some(0)), "{plain}");
    // someone who may not see the request: it is ignored, not an error
    crate::test_support::user(&st, "guest", "member");
    st.db.execute("UPDATE users SET perms=? WHERE username='guest'", p![json!({ "pick": true }).to_string()]).unwrap();
    let (code, body, guest) = call(&app, "POST", "/api/auth/login", "", Some(json!({ "username": "guest", "password": PW }))).await;
    assert_eq!(code, StatusCode::OK, "{body}");
    let guest = guest.split(';').next().unwrap().to_string();
    let (code, theirs) = picker(&app, &guest, illuminate(json!({ "request_id": id }))).await;
    assert_eq!((code, theirs["items"].as_array().map(Vec::len)), (StatusCode::OK, Some(0)), "{theirs}");
    assert_eq!(posts(&f).len(), 3, "the picker searched nothing");
    ts.remove();
}

/// Soulseek not connected: kept rows still answer (200, no Search again, the reason instead); Search
/// again and words with nothing kept get the 409 as before, and nothing is searched.
#[tokio::test(start_paused = true)]
async fn kept_rows_answer_while_soulseek_is_not_connected() {
    let (ts, f) = setup("down");
    let st = ts.st.clone();
    f.lock().unwrap().login = Login::Works;
    f.lock().unwrap().search_secs = 2;
    answers(&f, "Duumu Illuminate EP", json!([shared("alice", "Music\\Duumu\\Illuminate EP", &["01.flac", "02.flac"])]));
    let sup = tokio::spawn(session::supervisor(st.clone()));
    until("logged in", Duration::from_secs(5), || state(&st) == S::LoggedIn).await;
    let app = ts.app();
    let cookie = admin_cookie(&app).await;
    let (code, live) = picker(&app, &cookie, illuminate(json!({}))).await;
    assert_eq!((code, live["from"].as_str()), (StatusCode::OK, Some("live")), "{live}");

    // the session ends behind slskd's back; the next search finds out, and Encore waits to reconnect
    let keep = {
        let mut f = f.lock().unwrap();
        f.client = Client::Disconnected;
        f.login = Login::TimesOut;
        f.pass.take()
    };
    assert!(slskd::search(&st, "a").await.is_err());
    until("waiting", Duration::from_secs(5), || state(&st) == S::Waiting).await;
    let n = posts(&f).len();
    let (code, kept) = picker(&app, &cookie, illuminate(json!({}))).await;
    assert_eq!(code, StatusCode::OK, "{kept}");
    assert_eq!((kept["from"].as_str(), kept["can_search"].as_bool()), (Some("cache"), Some(false)), "{kept}");
    let notice = kept["notice"].as_str().unwrap();
    assert!(notice.starts_with("Soulseek isn't connected – "), "{notice}");
    assert_eq!(users(&kept), ["alice"]);
    // Search again can't run: 409 with the state, nothing searched, what is kept stays
    let (code, again) = picker(&app, &cookie, illuminate(json!({ "fresh": true }))).await;
    assert_eq!((code, again["state"].as_str()), (StatusCode::CONFLICT, Some("not_connected")), "{again}");
    // nothing kept for these words: 409 as before
    let (code, other) = picker(&app, &cookie, illuminate(json!({ "query": "something else entirely" }))).await;
    assert_eq!((code, other["state"].as_str()), (StatusCode::CONFLICT, Some("not_connected")), "{other}");
    assert_eq!(posts(&f).len(), n, "nothing searched");
    let (_, still) = picker(&app, &cookie, illuminate(json!({}))).await;
    assert_eq!(still["items"], kept["items"]);
    drop(keep);
    sup.abort();
    ts.remove();
}

/// Soulseek busy (the server paused searches): Search again is a 429 at once, and what is kept is left
/// as it was.
#[tokio::test(start_paused = true)]
async fn a_busy_soulseek_leaves_what_is_kept_alone() {
    let (ts, f) = setup("busy");
    let st = ts.st.clone();
    logged_in_already(&st, &f);
    f.lock().unwrap().search_secs = 2;
    answers(&f, "Duumu Illuminate EP", json!([shared("alice", "Music\\Duumu\\Illuminate EP", &["01.flac", "02.flac"])]));
    let app = ts.app();
    let cookie = admin_cookie(&app).await;
    let (_, live) = picker(&app, &cookie, illuminate(json!({}))).await;
    assert_eq!(live["from"], "live");
    // a request's search is turned away: searches pause for 60 s
    f.lock().unwrap().too_many = 1;
    let st2 = st.clone();
    let worker = tokio::spawn(async move { slskd::search(&st2, "other words").await });
    until("turned away", Duration::from_secs(10), || !f.lock().unwrap().refused.is_empty()).await;
    let t = Instant::now();
    let (code, busy) = picker(&app, &cookie, illuminate(json!({ "fresh": true }))).await;
    assert_eq!(code, StatusCode::TOO_MANY_REQUESTS, "{busy}");
    assert!(busy["detail"].as_str().unwrap().starts_with("Rate limited by the Soulseek server – next search at "), "{busy}");
    assert!(t.elapsed() < Duration::from_secs(1), "at once");
    let (_, kept) = picker(&app, &cookie, illuminate(json!({}))).await;
    assert_eq!((kept["from"].as_str(), users(&kept)), (Some("cache"), vec!["alice".to_string()]), "{kept}");
    worker.await.unwrap().unwrap();
    ts.remove();
}

/// A search on the Soulseek page that is over is kept, and the picker for the same words answers from it.
#[tokio::test(start_paused = true)]
async fn the_soulseek_pages_search_is_kept_for_the_picker() {
    let (ts, f) = setup("page");
    let st = ts.st.clone();
    logged_in_already(&st, &f);
    f.lock().unwrap().search_secs = 2;
    answers(&f, "duumu illuminate", json!([shared("erin", "Music\\Duumu\\Illuminate EP", &["01.flac", "02.flac"])]));
    let app = ts.app();
    let cookie = admin_cookie(&app).await;
    let (code, started, _) = call(&app, "POST", "/api/soulseek/search", &cookie, Some(json!({ "q": "duumu illuminate" }))).await;
    assert_eq!(code, StatusCode::OK, "{started}");
    let id = started["id"].as_str().unwrap().to_string();
    let mut done = false;
    for _ in 0..20 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let (code, poll, _) = call(&app, "GET", &format!("/api/soulseek/search/{id}"), &cookie, None).await;
        assert_eq!(code, StatusCode::OK, "{poll}");
        if poll["done"] == true {
            assert_eq!(poll["results"][0]["username"], "erin");
            done = true;
            break;
        }
    }
    assert!(done);
    let (code, kept) = picker(&app, &cookie, illuminate(json!({ "query": "Illuminate Duumu" }))).await;
    assert_eq!((code, kept["from"].as_str(), users(&kept)), (StatusCode::OK, Some("cache"), vec!["erin".to_string()]), "{kept}");
    assert_eq!(posts(&f).len(), 1, "the page's search only");
    ts.remove();
}

/// A kept row whose user has gone offline since it was found (design 3.8): picking it fails at once with
/// the user's name and what to do next, not "All sources exhausted (… 500 …)", and nothing is downloaded.
#[tokio::test(start_paused = true)]
async fn picking_a_kept_row_whose_user_went_offline_says_so() {
    let (ts, f) = setup("offline");
    let st = ts.st.clone();
    logged_in_already(&st, &f);
    f.lock().unwrap().search_secs = 2;
    answers(&f, "Duumu Illuminate EP", json!([shared("alice", "Music\\Duumu\\Illuminate EP", &["01 Lights.flac", "02 Shade.flac"])]));
    let app = ts.app();
    let cookie = admin_cookie(&app).await;
    picker(&app, &cookie, illuminate(json!({}))).await;
    let (_, kept) = picker(&app, &cookie, illuminate(json!({}))).await;
    assert_eq!(kept["from"], "cache", "{kept}");
    // alice leaves Soulseek after the search
    f.lock().unwrap().offline.push("alice".into());
    let found = &kept["items"][0];
    let id = request(&st, "Illuminate EP", "queued", json!({ "pick": { "source": "slskd", "payload": found["payload"], "title": found["title"] } }));
    encore_requests::worker::step(st.clone(), id).await;
    let (status, message, ..) = row(&st, id);
    assert_eq!((status.as_str(), message.as_str()), ("failed", "alice is offline on Soulseek – open Pick and choose another result, or Search again"));
    assert!(f.lock().unwrap().enqueues.is_empty());
    ts.remove();
}

/// A result picked by hand stays picked across a restart of the server (wf/req-slsk-cache.md side
/// finding 3): the request starts again with its pick, not as an automatic request through every source.
#[tokio::test(start_paused = true)]
async fn a_picked_result_survives_a_server_restart() {
    let (ts, _f) = setup("restart");
    let st = ts.st.clone();
    let pick = json!({ "source": "slskd", "payload": { "username": "alice", "directory": "Music\\Duumu\\Illuminate EP", "files": [] }, "title": "Illuminate EP" });
    let id = request(&st, "Song", "searching", json!({ "pick": pick, "attempts": 1, "sources": [] }));
    encore_requests::worker::recover(&st);
    let (status, message, _, d, _) = row(&st, id);
    assert_eq!((status.as_str(), message.as_str()), ("queued", "Restarted after server restart"));
    assert_eq!(d["pick"], pick, "{d}");
    assert_eq!(d["attempts"], 1);
    ts.remove();
}
