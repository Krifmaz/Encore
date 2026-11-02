//! The video downloads to a device (routes::video_links) on a whole server, with a stand-in for yt-dlp: a
//! video is handed over once and then gone from the server; a failed or cancelled one leaves nothing behind;
//! only the account that started a job sees it; the permission and the per-account cap hold.
use super::*;
use crate::test_support::{call, send, user, TestServer};
use encore_db::p;
use std::sync::atomic::{AtomicBool, Ordering};

/// Signs `name` in → its session cookie.
async fn login(app: &Router, name: &str) -> String {
    let (code, _, cookie) = call(app, "POST", "/api/auth/login", "", Some(json!({ "username": name, "password": "a-long-password-1" }))).await;
    assert_eq!(code, 200, "login {name}");
    cookie.split(';').next().unwrap_or("").to_string()
}

/// A stand-in download: writes `body` as "<title>.mp4" in its folder after reporting 50 %; with a title of
/// "fail" it fails instead; while `hold` is set it waits (a download still running).
fn fake(body: &'static [u8], hold: Arc<AtomicBool>) -> Fetch {
    Arc::new(move |_st, url: String, dir: PathBuf, progress: Progress| {
        let hold = hold.clone();
        Box::pin(async move {
            progress(50.0);
            while hold.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            if url.contains("fail") {
                std::fs::create_dir_all(&dir)?;
                std::fs::write(dir.join("half.mp4.part"), b"x")?;
                anyhow::bail!("ERROR: Video unavailable");
            }
            std::fs::create_dir_all(&dir)?;
            let file = dir.join("My clip [abc].mp4");
            std::fs::write(&file, body)?;
            Ok(ytdlp::Video { file, title: Some("My clip: part 1/2".into()) })
        })
    })
}

/// Polls job `id` until it leaves queued/downloading → its last answer.
async fn settle(app: &Router, cookie: &str, id: &str) -> Value {
    for _ in 0..400 {
        let (code, v, _) = call(app, "GET", &format!("/api/video-links/{id}"), cookie, None).await;
        assert_eq!(code, 200, "{v}");
        if !matches!(v["state"].as_str(), Some("queued" | "downloading")) {
            return v;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("job {id} never settled");
}

/// A server with two members, and the stand-in in place of yt-dlp.
fn server(name: &str, hold: Arc<AtomicBool>) -> (TestServer, Router) {
    let ts = TestServer::new(&format!("video-links-{name}"));
    user(&ts.st, "ann", "user");
    user(&ts.st, "bob", "user");
    ts.st.web.video_links.fake(fake(b"MP4DATA-0123456789", hold));
    let app = ts.app();
    (ts, app)
}

#[tokio::test]
async fn a_video_is_handed_over_once_then_deleted() {
    let (ts, app) = server("handover", Arc::new(AtomicBool::new(false)));
    let ann = login(&app, "ann").await;
    let (code, v, _) = call(&app, "POST", "/api/video-links", &ann, Some(json!({ "url": "https://www.youtube.com/watch?v=abc" }))).await;
    assert_eq!(code, 200, "{v}");
    let id = v["id"].as_str().unwrap().to_string();
    let v = settle(&app, &ann, &id).await;
    assert_eq!(v["state"], "ready");
    assert_eq!(v["size"], 18);
    // the title made safe for every file system
    assert_eq!(v["name"], "My clip_ part 1_2.mp4");
    let dir = root(&ts.st).join(&id);
    assert!(dir.exists(), "kept until fetched");

    // someone else never sees it, let alone fetches it
    let bob = login(&app, "bob").await;
    assert_eq!(call(&app, "GET", &format!("/api/video-links/{id}"), &bob, None).await.0, 404);
    assert_eq!(send(&app, "GET", &format!("/api/video-links/{id}/file"), &bob, None, &[]).await.status, 404);

    let r = send(&app, "GET", &format!("/api/video-links/{id}/file"), &ann, None, &[]).await;
    assert_eq!(r.status, 200);
    assert_eq!(r.body, b"MP4DATA-0123456789");
    assert_eq!(r.h("content-type"), "video/mp4");
    assert_eq!(r.h("content-length"), "18");
    assert!(r.h("content-disposition").starts_with("attachment;"), "{}", r.h("content-disposition"));
    // sent whole: gone from the server
    assert!(!dir.exists());
    assert_eq!(call(&app, "GET", &format!("/api/video-links/{id}"), &ann, None).await.0, 404);
    assert_eq!(send(&app, "GET", &format!("/api/video-links/{id}/file"), &ann, None, &[]).await.status, 404);
    // never in the library
    assert_eq!(ts.st.db.one("SELECT COUNT(*) n FROM tracks", p![]).unwrap().unwrap().i("n"), 0);
    ts.remove();
}

#[tokio::test]
async fn a_failed_download_leaves_nothing_but_its_error() {
    let (ts, app) = server("failed", Arc::new(AtomicBool::new(false)));
    let ann = login(&app, "ann").await;
    let (_, v, _) = call(&app, "POST", "/api/video-links", &ann, Some(json!({ "url": "https://example.com/fail" }))).await;
    let id = v["id"].as_str().unwrap().to_string();
    let v = settle(&app, &ann, &id).await;
    assert_eq!(v["state"], "failed");
    assert_eq!(v["error"], "ERROR: Video unavailable");
    assert!(!root(&ts.st).join(&id).exists());
    assert_eq!(send(&app, "GET", &format!("/api/video-links/{id}/file"), &ann, None, &[]).await.status, 409);
    ts.remove();
}

#[tokio::test]
async fn cancelling_stops_the_download_and_deletes_it() {
    let hold = Arc::new(AtomicBool::new(true));
    let (ts, app) = server("cancel", hold.clone());
    let ann = login(&app, "ann").await;
    let (_, v, _) = call(&app, "POST", "/api/video-links", &ann, Some(json!({ "url": "https://vimeo.com/1" }))).await;
    let id = v["id"].as_str().unwrap().to_string();
    // running: progress shows
    let mut seen = false;
    for _ in 0..200 {
        let (_, v, _) = call(&app, "GET", &format!("/api/video-links/{id}"), &ann, None).await;
        if v["state"] == "downloading" && v["progress"] == 50.0 {
            seen = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(seen, "progress never showed");
    // only the owner may cancel
    let bob = login(&app, "bob").await;
    assert_eq!(call(&app, "DELETE", &format!("/api/video-links/{id}"), &bob, None).await.0, 404);
    assert_eq!(call(&app, "DELETE", &format!("/api/video-links/{id}"), &ann, None).await.0, 200);
    hold.store(false, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(50)).await;
    // the aborted task never wrote its file
    assert!(!root(&ts.st).join(&id).exists());
    assert_eq!(call(&app, "GET", &format!("/api/video-links/{id}"), &ann, None).await.0, 404);
    ts.remove();
}

#[tokio::test]
async fn the_permission_the_cap_and_bad_links_are_refused() {
    let hold = Arc::new(AtomicBool::new(true));
    let (ts, app) = server("refused", hold.clone());
    let ann = login(&app, "ann").await;
    assert_eq!(call(&app, "POST", "/api/video-links", &ann, Some(json!({ "url": "not a link" }))).await.0, 400);
    for i in 0..PER_USER {
        assert_eq!(call(&app, "POST", "/api/video-links", &ann, Some(json!({ "url": format!("https://vimeo.com/{i}") }))).await.0, 200);
    }
    assert_eq!(call(&app, "POST", "/api/video-links", &ann, Some(json!({ "url": "https://vimeo.com/x" }))).await.0, 429);
    // bob has a cap of his own, until an admin takes the permission away
    let bob = login(&app, "bob").await;
    let bob_id = ts.st.db.one("SELECT id FROM users WHERE username='bob'", p![]).unwrap().unwrap().i("id");
    ts.st.db.execute("UPDATE users SET perms=? WHERE id=?", p![json!({ PERM: false }).to_string(), bob_id]).unwrap();
    assert_eq!(call(&app, "POST", "/api/video-links", &bob, Some(json!({ "url": "https://vimeo.com/y" }))).await.0, 403);
    hold.store(false, Ordering::SeqCst);
    ts.remove();
}

#[test]
fn download_names_are_safe_everywhere() {
    assert_eq!(download_name(Some("a/b\\c:d*e?f\"g<h>i|j")), "a_b_c_d_e_f_g_h_i_j.mp4");
    assert_eq!(download_name(None), "video.mp4");
    assert_eq!(download_name(Some("  ... ")), "video.mp4");
    assert_eq!(download_name(Some("Ending dots...")), "Ending dots.mp4");
    let long = "é".repeat(400);
    assert_eq!(download_name(Some(&long)).chars().count(), 154);
}
