//! Encore's own updates: the release answer from a local stand-in for GitHub's releases API, the socket check
//! in plain words (no socket in tests; a mock Docker on a unix socket for the rest), the restart planning
//! (never exits), and "Update now"'s pull and helper start against the mock Docker.
use super::*;
use crate::selfupdate::tests::{fixture, serve_mock, sock_dir, HELPER_ID};

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
    let r = Restart::default();
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
