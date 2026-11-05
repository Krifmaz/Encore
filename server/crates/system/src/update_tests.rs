//! Encore's own updates: the release answer from a local stand-in for GitHub's releases API, the
//! `self_update` setting, Encore's tag rule and the restart planning (never exits). The socket check, the
//! pull, the helper and the swap are the kit's, tested there.
use super::*;

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
fn the_hotfix_channel_takes_the_highest_of_releases_and_hotfixes() {
    assert_eq!(Channel::parse("hotfix"), Channel::Hotfix);
    assert_eq!(Channel::parse(" Hotfix "), Channel::Hotfix);
    for s in ["", "stable", "nightly"] {
        assert_eq!(Channel::parse(s), Channel::Stable, "{s:?}");
    }
    let list = json!([
        { "tag_name": "hotfix-0.35.2.18", "prerelease": true, "draft": true },
        { "tag_name": "hotfix-0.35.2.17", "prerelease": true },
        { "tag_name": "v0.36.0-beta", "prerelease": true },
        { "tag_name": "v0.35.2" },
        { "tag_name": "hotfix-0.35.1.9", "prerelease": true },
    ]);
    let r = pick_newest(&list, Channel::Hotfix, 0.0).unwrap();
    assert_eq!((r["tag"].as_str(), r["latest"].as_str(), r["hotfix"].as_bool()), (Some("hotfix-0.35.2.17"), Some("0.35.2.17"), Some(true)),
               "the draft and the other prerelease don't count");
    // a full release after the hotfixes is newer than all of them
    let list = json!([{ "tag_name": "v0.35.3" }, { "tag_name": "hotfix-0.35.2.17", "prerelease": true }]);
    assert_eq!(pick_newest(&list, Channel::Hotfix, 0.0).unwrap()["latest"], json!("0.35.3"));
    assert!(is_newer("0.35.3", "0.35.2.17", false));
    assert!(is_newer("0.35.2.17", "0.35.2", false));
    assert!(!is_newer("0.35.2", "0.35.2.17", false), "the stable channel does not go back from a hotfix");
    assert!(pick_newest(&json!([]), Channel::Hotfix, 0.0).is_none());
    assert_eq!(check_tag("0.35.2.17").as_deref(), Ok("0.35.2.17"));
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

/// Switched off in the settings wins over everything the socket would say.
#[tokio::test]
async fn self_update_can_be_switched_off() {
    let missing = std::path::Path::new("/nonexistent/docker.sock");
    for off in ["off", "OFF", "false", "no", "0"] {
        assert_eq!(probe_with(missing, off, "abc", "").await.reason.as_deref(), Some("Switched off in the settings (self_update = off)"), "{off}");
    }
    assert_eq!(probe_with(missing, "", "abc", "").await.reason.as_deref(), Some("The Docker socket is not mounted"));
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

/// The stable channel takes the highest full release from the list, not GitHub's "Latest": the hotfix built while
/// v0.42.6 was on its way was numbered 0.42.5.191 and marked Latest, and servers on it never saw 0.42.6.
#[test]
fn the_highest_release_wins_over_githubs_latest() {
    // GitHub's list as it stood on 2026-10-29 (newest first; the hotfix is a normal release, the "Latest")
    let list = json!([
        { "tag_name": "v0.42.6" },
        { "tag_name": "v0.42.5" },
        { "tag_name": "hotfix-0.42.5.191" },
        { "tag_name": "hotfix-0.42.3.183" },
    ]);
    for channel in [Channel::Stable, Channel::Hotfix] {
        let r = pick_newest(&list, channel, 0.0).unwrap();
        assert_eq!(r["latest"], json!("0.42.6"), "{channel:?}");
        assert!(is_newer(r["latest"].as_str().unwrap(), "0.42.5.191", false), "0.42.6 is offered to a server on 0.42.5.191");
    }
    // a hotfix after the newest full release (a normal release, as CI publishes them) reaches both channels
    let list = json!([{ "tag_name": "hotfix-0.42.6.195" }, { "tag_name": "v0.42.6" }]);
    assert_eq!(pick_newest(&list, Channel::Stable, 0.0).unwrap()["latest"], json!("0.42.6.195"));
    assert_eq!(pick_newest(&list, Channel::Hotfix, 0.0).unwrap()["latest"], json!("0.42.6.195"));
    // a hotfix prerelease only reaches the hotfix channel; a beta and a draft reach neither
    let list = json!([
        { "tag_name": "hotfix-0.42.6.196", "prerelease": true },
        { "tag_name": "v0.43.0-beta", "prerelease": true },
        { "tag_name": "v0.43.0", "draft": true },
        { "tag_name": "v0.42.6" },
    ]);
    assert_eq!(pick_newest(&list, Channel::Stable, 0.0).unwrap()["latest"], json!("0.42.6"));
    assert_eq!(pick_newest(&list, Channel::Hotfix, 0.0).unwrap()["latest"], json!("0.42.6.196"));
}
