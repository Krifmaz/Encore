//! Encore's own updates: the release answer from a local stand-in for GitHub's releases API, the socket check
//! in plain words (no socket in tests; a mock Docker on a unix socket for the rest), the restart planning
//! (never exits), and "Update now"'s pull and helper start against the mock Docker.
use super::*;
use crate::selfupdate::tests::{fixture, serve_mock, sock_dir, HELPER_ID};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;
use tower::ServiceExt;

#[test]
fn a_release_is_kept_with_its_notes_cut_at_4000_characters() {
    let body = format!("## What changed\r\n\r\n{}", "ü".repeat(5000));
    let r = parse_release(&json!({ "tag_name": "v0.29.0", "body": body, "html_url": "https://github.com/Krifmaz/Encore/releases/tag/v0.29.0",
                                    "published_at": "2026-11-02T10:00:00Z" }), 1_800_000_000.0).unwrap();
    assert_eq!((r["tag"].as_str(), r["latest"].as_str()), (Some("v0.29.0"), Some("0.29.0")));
    assert_eq!(r["notes"].as_str().unwrap().chars().count(), 4000);
    assert!(r["notes"].as_str().unwrap().starts_with("## What changed\n\n"), "CRLF becomes LF");
    assert_eq!(r["notes_cut"], json!(true));
    assert_eq!((r["published_at"].as_str(), r["checked_at"].as_i64()), (Some("2026-11-02T10:00:00Z"), Some(1_800_000_000)));
    assert!(parse_release(&json!({ "message": "Not Found" }), 0.0).is_none());
    let r = parse_release(&json!({ "tag_name": "0.29.1" }), 0.0).unwrap();
    assert_eq!((r["notes"].as_str(), r["notes_cut"].as_bool()), (Some(""), Some(false)));
    assert_eq!(r["url"], json!("https://github.com/Krifmaz/Encore/releases/tag/0.29.1"));
}

#[test]
fn newer_is_a_version_comparison_and_a_dev_build_is_older_than_any_release() {
    assert!(is_newer("0.29.0", "0.28.3", false));
    assert!(is_newer("0.28.10", "0.28.9", false), "numbers, not text");
    assert!(!is_newer("0.28.0", "0.28.0", false));
    assert!(!is_newer("0.27.1", "0.28.0", false));
    assert!(is_newer("0.26.1", "2.0.0", true), "a dev build");
    assert!(!is_newer("", "0.28.0", true));
    let su = SelfUpdate { reason: Some("The Docker socket is not mounted".into()), ..Default::default() };
    let rel = parse_release(&json!({ "tag_name": "v0.29.0", "body": "notes", "published_at": "2026-11-02T10:00:00Z" }), 1_800_000_000.0).unwrap();
    let a = answer(Some(&rel), &su, "0.28.0", false);
    assert_eq!(a["current"], json!("0.28.0"));
    assert_eq!((a["latest"].as_str(), a["is_newer"].as_bool()), (Some("0.29.0"), Some(true)));
    assert_eq!((a["restart"].as_str(), a["self_update"].as_str()), (Some("ready"), Some("unavailable")));
    assert_eq!(a["self_update_reason"], json!("The Docker socket is not mounted"));
    assert_eq!(a["image"], json!("ghcr.io/krifmaz/encore:0.29.0"));
    assert_eq!((a["notes"].as_str(), a["published_at"].as_str(), a["checked_at"].as_i64()), (Some("notes"), Some("2026-11-02T10:00:00Z"), Some(1_800_000_000)));
    // nothing known yet
    let a = answer(None, &SelfUpdate { ready: true, ..Default::default() }, "0.28.0", false);
    assert_eq!((a["latest"].clone(), a["is_newer"].clone(), a["image"].clone()), (Value::Null, json!(false), Value::Null));
    assert_eq!((a["self_update"].as_str(), a["self_update_reason"].clone()), (Some("ready"), Value::Null));
}

#[test]
fn image_tags_are_checked() {
    assert_eq!(check_tag("0.29.0").as_deref(), Ok("0.29.0"));
    assert_eq!(check_tag("v0.29.0").as_deref(), Ok("0.29.0"), "the images are tagged without the v");
    assert_eq!(check_tag("latest").as_deref(), Ok("latest"));
    assert_eq!(check_tag("0.28.0").as_deref(), Ok("0.28.0"));
    for bad in ["", "0.29.0;rm", "../x", "a b", "x:y", &"a".repeat(65), "ghcr.io/other/image"] {
        assert_eq!(check_tag(bad), Err("Not a valid image tag".to_string()), "{bad}");
    }
    assert!(check_tag("0.27.0").unwrap_err().contains("can't update itself"), "older images have no helper");
}

#[test]
fn the_container_id_is_found_in_mountinfo() {
    let id = "784f60a526476812415c46b2613ebae100e5df0b6fc23a4b255d19ccf653c7a5";
    let text = format!("1236 1218 0:322 / / rw,relatime master:500 - overlay overlay rw,lowerdir=/var/lib/docker/overlay2/l/ABC\n\
        1250 1236 254:1 /var/lib/docker/containers/{id}/resolv.conf /etc/resolv.conf rw,relatime - ext4 /dev/vda1 rw\n\
        1251 1236 254:1 /var/lib/docker/containers/{id}/hostname /etc/hostname rw,relatime - ext4 /dev/vda1 rw\n");
    assert_eq!(container_id_from_mountinfo(&text).as_deref(), Some(id));
    assert_eq!(container_id_from_mountinfo("22 1 254:1 / / rw - ext4 /dev/vda1 rw\n"), None);
    assert_eq!(container_id_from_mountinfo("1 1 0:1 /containers/short/hostname /etc/hostname rw - ext4 x rw"), None);
}

/// Each reason in plain words, in the order they are checked.
#[tokio::test]
async fn self_update_says_why_not() {
    let dir = sock_dir("probe");
    let missing = dir.join("none.sock");
    assert_eq!(probe_with(&missing, "", "abc", "").await.reason.as_deref(), Some("The Docker socket is not mounted"));
    assert_eq!(probe_with(&missing, "off", "abc", "").await.reason.as_deref(), Some("Switched off in the settings (self_update = off)"));
    std::fs::write(dir.join("file.sock"), "").unwrap();
    let r = probe_with(&dir.join("file.sock"), "", "abc", "").await;
    assert!(!r.ready && r.reason.as_deref().unwrap().starts_with("The Docker socket is not mounted ("), "{r:?}");
    // a socket nobody answers on
    let dead = dir.join("dead.sock");
    drop(std::os::unix::net::UnixListener::bind(&dead).unwrap());
    let r = probe_with(&dead, "", "abc", "").await;
    assert!(r.reason.as_deref().unwrap().starts_with("The Docker socket refused the request: "), "{r:?}");

    let (_d, m) = serve_mock(&dir).await;
    let sock = dir.join("docker.sock");
    let ins = fixture();
    let id = ins["Id"].as_str().unwrap();
    // the hostname is the id's first 12 characters
    let r = probe_with(&sock, "", &id[..12], "").await;
    assert_eq!(r, SelfUpdate { ready: true, reason: None, id: Some(id.into()), name: Some("encore".into()), image: Some("ghcr.io/krifmaz/encore:latest".into()) });
    // a hostname of the owner's own: the id from mountinfo
    let mi = format!("1251 1236 254:1 /var/lib/docker/containers/{id}/hostname /etc/hostname rw - ext4 /dev/vda1 rw\n");
    assert!(probe_with(&sock, "", "music", &mi).await.ready);
    // a Docker that doesn't know this machine
    assert_eq!(probe_with(&sock, "", "laptop", "").await.reason.as_deref(), Some("Not running in a container"));
    // switched off wins even with the socket there
    assert!(!probe_with(&sock, "OFF", &id[..12], "").await.ready);
    // a container started with --rm can't be swapped
    m.fail.lock().unwrap().insert("rm");
    let r = probe_with(&sock, "", &id[..12], "").await;
    assert_eq!(r.reason.as_deref(), Some("This container is removed when it stops (started with --rm), so it can't be replaced"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// "Restart server" plans the restart and does not exit: the server loop (`serve`) does that.
#[tokio::test]
async fn a_restart_is_scheduled_once() {
    let r: &'static Restart = Box::leak(Box::default());
    assert!(!r.is_requested());
    assert!(r.schedule(Duration::from_millis(30)));
    assert!(!r.schedule(Duration::from_millis(30)), "one restart at a time");
    assert!(!r.is_requested(), "not before the delay: the 202 answer gets out first");
    tokio::time::timeout(Duration::from_secs(2), r.requested()).await.expect("the restart comes after the delay");
    assert!(r.is_requested());
    // asked for after the fact: answers at once
    tokio::time::timeout(Duration::from_millis(100), r.requested()).await.unwrap();
}

/// "Update now" up to the helper: the pull (Docker's words when it fails), then the helper's container from
/// the new image with the socket, started.
#[tokio::test]
async fn update_now_pulls_and_starts_the_helper() {
    let dir = sock_dir("start");
    let (d, m) = serve_mock(&dir).await;
    let id = fixture()["Id"].as_str().unwrap().to_string();
    let name = start_update(&d, &id, "0.28.0").await.unwrap();
    assert!(name.starts_with("encore-update-"), "{name}");
    let calls = m.calls();
    assert_eq!(calls[0], "POST /images/create?fromImage=ghcr.io%2Fkrifmaz%2Fencore&tag=0.28.0");
    assert_eq!(calls[1], format!("GET /containers/{id}/json"));
    assert!(calls[2].starts_with("POST /containers/create?name=encore-update-"), "{calls:?}");
    assert_eq!(calls[3], format!("POST /containers/{HELPER_ID}/start"));
    let body = m.body("/containers/create?name=encore-update-").unwrap();
    assert_eq!(body["Cmd"], json!(["self-update-helper", "--target", id, "--image", "ghcr.io/krifmaz/encore:0.28.0"]));
    assert_eq!(body["HostConfig"]["AutoRemove"], json!(true));

    let e = start_update(&d, &id, "9.9.9").await.unwrap_err();
    assert_eq!(e, "Could not pull ghcr.io/krifmaz/encore:9.9.9: manifest unknown");
    let e = start_update(&d, &id, "denied").await.unwrap_err();
    assert!(e.starts_with("Could not pull ghcr.io/krifmaz/encore:denied: pull access denied"), "{e}");
    assert_eq!(m.calls().iter().filter(|c| c.starts_with("POST /containers/create")).count(), 1, "no helper after a failed pull");
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- through the router -------------------------------------------------------------------------------

const PW: &str = "update-pass-12";

async fn call(app: &axum::Router, method: &str, uri: &str, cookie: &str, csrf: bool, body: Option<Value>) -> (StatusCode, Value, String) {
    let mut b = Request::builder().method(method).uri(uri);
    if csrf {
        b = b.header("x-requested-with", "encore");
    }
    if !cookie.is_empty() {
        b = b.header("cookie", cookie);
    }
    let req = match body {
        Some(j) => b.header("content-type", "application/json").body(Body::from(j.to_string())).unwrap(),
        None => b.body(Body::empty()).unwrap(),
    };
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let set_cookie = resp.headers().get("set-cookie").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let bytes = axum::body::to_bytes(resp.into_body(), 10 << 20).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null), set_cookie)
}

async fn login(app: &axum::Router, name: &str) -> String {
    let r = call(app, "POST", "/api/auth/login", "", true, Some(json!({ "username": name, "password": PW }))).await;
    assert_eq!(r.0, StatusCode::OK, "{:?}", r.1);
    r.2.split(';').next().unwrap().to_string()
}

#[derive(Clone, Default)]
struct Hits {
    full: Arc<AtomicUsize>,
    conditional: Arc<AtomicUsize>,
}

/// GitHub's releases API as far as the check uses it: the latest release with an ETag, 304 for a repeat.
async fn github_latest(axum::extract::State(h): axum::extract::State<Hits>, headers: axum::http::HeaderMap) -> axum::response::Response {
    use axum::response::IntoResponse;
    if headers.get("if-none-match").and_then(|v| v.to_str().ok()) == Some("\"rel-1\"") {
        h.conditional.fetch_add(1, Ordering::SeqCst);
        return (StatusCode::NOT_MODIFIED, [("etag", "\"rel-1\"")]).into_response();
    }
    h.full.fetch_add(1, Ordering::SeqCst);
    let body = json!({ "tag_name": "v99.0.0", "name": "Encore 99.0.0", "html_url": "https://github.com/Krifmaz/Encore/releases/tag/v99.0.0",
                       "published_at": "2026-11-02T10:00:00Z", "body": format!("## What changed\n\n{}", "x".repeat(6000)) });
    (StatusCode::OK, [("etag", "\"rel-1\""), ("content-type", "application/json")], body.to_string()).into_response()
}

/// GET /api/system/update for admins only, from GitHub's answer, kept for an hour (force asks again, through
/// the ETag); self-update unavailable without a socket, with the reason; the diagnostics' machine block and
/// the scheduled task see the kept answer; POST update refuses without a socket; restart needs the CSRF header.
#[tokio::test]
async fn the_update_endpoints() {
    let hits = Hits::default();
    let gh = axum::Router::new().route("/repos/Krifmaz/Encore/releases/latest", axum::routing::get(github_latest)).with_state(hits.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, gh).await.unwrap() });
    *TEST_API.lock().unwrap() = Some(base);
    let dir = std::env::temp_dir().join(format!("encore-update-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    *TEST_SOCKET.lock().unwrap() = Some(dir.join("no-docker.sock"));
    let st = {
        let _env = crate::TEST_ENV.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("ENCORE_CONFIG_DIR", &dir);
        crate::build_state().unwrap()
    };
    crate::auth::create_user(&st, "boss", PW, "admin", None, None).unwrap();
    crate::auth::create_user(&st, "listener", PW, "user", None, None).unwrap();
    let app = crate::app(st.clone());
    let admin = login(&app, "boss").await;
    let user = login(&app, "listener").await;
    let n = |h: &Arc<AtomicUsize>| h.load(Ordering::SeqCst);

    // nothing kept yet: the diagnostics block says no update
    assert_eq!(crate::routes::system::server_block(&st)["encore_update_available"], json!(false));
    let (s, _, _) = call(&app, "GET", "/api/system/update", &user, false, None).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "admins only");
    let (s, v, _) = call(&app, "GET", "/api/system/update", &admin, false, None).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["current"], json!(crate::VERSION));
    assert_eq!((v["latest"].as_str(), v["is_newer"].as_bool()), (Some("99.0.0"), Some(true)));
    assert_eq!(v["notes"].as_str().unwrap().chars().count(), 4000);
    assert_eq!(v["url"], json!("https://github.com/Krifmaz/Encore/releases/tag/v99.0.0"));
    assert_eq!(v["published_at"], json!("2026-11-02T10:00:00Z"));
    assert!(v["checked_at"].as_i64().unwrap() > 1_700_000_000);
    assert_eq!((v["restart"].as_str(), v["self_update"].as_str()), (Some("ready"), Some("unavailable")));
    assert_eq!(v["self_update_reason"], json!("The Docker socket is not mounted"));
    assert_eq!(v["image"], json!("ghcr.io/krifmaz/encore:99.0.0"));
    assert_eq!(v["error"], Value::Null);
    assert_eq!((n(&hits.full), n(&hits.conditional)), (1, 0));
    // within the hour: the kept answer, no call
    let (_, v, _) = call(&app, "GET", "/api/system/update", &admin, false, None).await;
    assert_eq!(v["latest"], json!("99.0.0"));
    assert_eq!((n(&hits.full), n(&hits.conditional)), (1, 0));
    // Check now: asks again, GitHub says nothing changed
    let (_, v, _) = call(&app, "GET", "/api/system/update?force=1", &admin, false, None).await;
    assert_eq!(v["latest"], json!("99.0.0"));
    assert_eq!((n(&hits.full), n(&hits.conditional)), (1, 1));
    // the Settings banner reads the kept answer from the diagnostics' machine block
    let m = crate::routes::system::server_block(&st);
    assert_eq!((m["encore_update_available"].clone(), m["encore_latest"].clone()), (json!(true), json!("99.0.0")));
    // checks switched off: an old answer is shown without asking GitHub, Check now still asks
    st.db.execute("UPDATE sync_state SET value=json_set(value, '$.checked_at', 1) WHERE scope='encore:update'", crate::p![]).unwrap();
    st.cfg.set_many(&json!({ "check_updates": false }));
    let (_, v, _) = call(&app, "GET", "/api/system/update", &admin, false, None).await;
    assert_eq!((v["latest"].as_str(), v["check_updates"].as_bool(), v["checked_at"].as_i64()), (Some("99.0.0"), Some(false), Some(1)));
    assert_eq!((n(&hits.full), n(&hits.conditional)), (1, 1));
    let (_, v, _) = call(&app, "GET", "/api/system/update?force=1", &admin, false, None).await;
    assert!(v["checked_at"].as_i64().unwrap() > 1);
    assert_eq!((n(&hits.full), n(&hits.conditional)), (1, 2));
    // the scheduled task: once a day, on with check_updates
    let task = crate::scheduler::listing(&st).into_iter().find(|t| t["key"] == "update").unwrap();
    assert_eq!((task["enabled"].as_bool(), task["minutes"].as_i64()), (Some(false), Some(1440)));
    st.cfg.set_many(&json!({ "check_updates": true }));
    assert_eq!(crate::scheduler::listing(&st).into_iter().find(|t| t["key"] == "update").unwrap()["enabled"], json!(true));
    assert_eq!(scheduled(&st).await.unwrap(), "Encore 99.0.0 is available");

    // Update now: no socket here
    let (s, v, _) = call(&app, "POST", "/api/system/update", &admin, true, Some(json!({ "tag": "0.29.0" }))).await;
    assert_eq!((s, v["detail"].as_str()), (StatusCode::CONFLICT, Some("The Docker socket is not mounted")));
    let (s, _, _) = call(&app, "POST", "/api/system/update", &user, true, None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _, _) = call(&app, "POST", "/api/system/update", &admin, false, None).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "CSRF header");

    // Restart: admins, with the CSRF header; the answer comes before the restart
    let (s, _, _) = call(&app, "POST", "/api/system/restart", &admin, false, None).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "CSRF header");
    let (s, _, _) = call(&app, "POST", "/api/system/restart", &user, true, None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert!(!RESTART.is_requested());
    let (s, v, _) = call(&app, "POST", "/api/system/restart", &admin, true, None).await;
    assert_eq!((s, v), (StatusCode::ACCEPTED, json!({ "ok": true, "message": "Restarting" })));
    tokio::time::timeout(Duration::from_secs(3), RESTART.requested()).await.expect("the restart is asked for after half a second");
    let (s, _, _) = call(&app, "POST", "/api/system/restart", &admin, true, None).await;
    assert_eq!(s, StatusCode::ACCEPTED, "asking twice is fine");

    *TEST_API.lock().unwrap() = None;
    *TEST_SOCKET.lock().unwrap() = None;
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_restart_finds_every_process_below_the_server() {
    assert_eq!(parent_of("4242 (yt-dlp) S 100 4242 1 0 -1"), Some(100));
    assert_eq!(parent_of("77 (odd ) name (x)) R 4242 77 1"), Some(4242), "a name with spaces and brackets");
    assert_eq!(parent_of("garbage"), None);
    // 1 tini · 7 the entrypoint · 100 Encore · 200 yt-dlp → 201 ffmpeg → 202 · 300 a render · 400 someone else's
    let procs = [(1, 0), (7, 1), (100, 7), (200, 100), (201, 200), (202, 201), (300, 100), (400, 1), (401, 400)];
    let mut below = descendants(100, &procs);
    below.sort();
    assert_eq!(below, vec![200, 201, 202, 300], "never the entrypoint, tini or a stranger");
    assert!(descendants(400, &[(401, 400), (400, 401)]).contains(&401), "a loop in the list ends");
    assert!(descendants(100, &[]).is_empty());
    // this test process: whatever /proc lists, the walk ends and never names this process or its parent
    let me = std::process::id() as i32;
    let found = descendants(me, &processes());
    assert!(!found.contains(&me));
}
