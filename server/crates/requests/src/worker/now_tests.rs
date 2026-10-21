//! Download now's pure parts: the choice of a user with a free upload slot, and Download all now's sentence.
//! What a request offers in every state (`offer`, `row_fields`) needs a whole server: crates/api,
//! server_tests/download_now.rs.
use super::*;
use crate::test_ctx::TestCtx;

/// One Soulseek answer: `user`'s folder `dir` with `n` files of `ext` at `br` kbps (0: lossless).
fn answer(user: &str, free: bool, dir: &str, n: usize, ext: &str, br: i64, speed: f64) -> Value {
    let files: Vec<Value> = (1..=n).map(|i| json!({ "filename": format!("Music\\{dir}\\{i:02} Light {i}.{ext}"), "size": 20_000_000, "bitRate": if br > 0 { json!(br) } else { Value::Null } })).collect();
    json!({ "username": user, "hasFreeUploadSlot": free, "uploadSpeed": speed, "queueLength": 0, "files": files })
}

fn album() -> Want {
    Want { album: true, artist: "Duumu".into(), name: "Illuminate EP".into(), tracks: Some(4), length: None, lossless_only: false }
}

fn current(user: &str, format: &str) -> Value {
    json!({ "username": user, "directory": "Music\\Duumu - Illuminate EP", "files": [], "format": format, "bitrate": 0 })
}

fn users(c: &[Value]) -> Vec<&str> {
    c.iter().map(|c| c["username"].as_str().unwrap()).collect()
}

#[test]
fn free_slot_choice_keeps_the_workers_rules() {
    let st = TestCtx::new("free-slot");
    st.cfg.set_many(&json!({ "slskd_preferred_format": "flac", "slskd_min_bitrate": 192, "slskd_max_candidates": 4 }));
    let dir = "Duumu - Illuminate EP";
    let answers = [
        answer("bob", true, dir, 4, "flac", 0, 1e6),
        // under the minimum bit rate
        answer("carol", true, dir, 4, "mp3", 128, 9e6),
        // no free slot, a better score (faster)
        answer("dave", false, dir, 4, "flac", 0, 9e7),
        // another album
        answer("erin", true, "Duumu - Other Things", 4, "flac", 0, 1e6),
        // tried already
        answer("gina", true, dir, 4, "flac", 0, 1e6),
        // the current user
        answer("alice", true, dir, 4, "flac", 0, 1e6),
    ];
    let tried = vec![format!("gina|Music/{dir}")]; // (the ranking names folders with "/")
    let d = json!({});
    let got = free_slot_choice(&st, &answers, &album(), &current("alice", "flac"), &tried, |_| false, &d);
    assert_eq!(users(&got), ["bob"]);
    assert_eq!(got[0]["files"].as_array().unwrap().len(), 4);
    // a blocked folder never comes back
    let got = free_slot_choice(&st, &answers, &album(), &current("alice", "flac"), &tried, |c| c["username"] == "bob", &d);
    assert!(got.is_empty(), "{got:?}");
    // a lossless download never switches to MP3 320; an any-format one does when no free FLAC is there
    let mp3 = [answer("frank", true, dir, 4, "mp3", 320, 1e6), answer("dave", false, dir, 4, "flac", 0, 1e6)];
    assert!(free_slot_choice(&st, &mp3, &album(), &current("alice", "flac"), &[], |_| false, &d).is_empty());
    assert_eq!(users(&free_slot_choice(&st, &mp3, &album(), &current("alice", "mp3"), &[], |_| false, &d)), ["frank"]);
    // a lossless step (or a request for a lossless copy) only takes lossless files
    let mut w = album();
    w.lossless_only = true;
    assert!(free_slot_choice(&st, &mp3, &w, &current("alice", "mp3"), &[], |_| false, &d).is_empty());
    // the cut to slskd_max_candidates never drops a free slot: the answers without one are out before it
    st.cfg.set_many(&json!({ "slskd_max_candidates": 2 }));
    let mut crowd: Vec<Value> = (0..6).map(|i| answer(&format!("busy{i}"), false, dir, 4, "flac", 0, 5e7)).collect();
    crowd.push(answer("bob", true, dir, 4, "flac", 0, 1.0));
    assert_eq!(users(&free_slot_choice(&st, &crowd, &album(), &current("alice", "flac"), &[], |_| false, &d)), ["bob"]);
    // nor do tried or blocked folders take the places before the cut: four of them rank above bob (faster)
    st.cfg.set_many(&json!({ "slskd_max_candidates": 4 }));
    let mut ahead: Vec<Value> = ["gina", "hana", "ivan", "kate"].iter().map(|u| answer(u, true, dir, 4, "flac", 0, 9e6)).collect();
    ahead.push(answer("bob", true, dir, 4, "flac", 0, 1.0));
    let tried: Vec<String> = ["gina", "hana", "ivan"].iter().map(|u| format!("{u}|Music/{dir}")).collect();
    assert_eq!(users(&free_slot_choice(&st, &ahead, &album(), &current("alice", "flac"), &tried, |c| c["username"] == "kate", &d)), ["bob"]);
    let song = Want { album: false, artist: "Duumu".into(), name: "Light 1".into(), tracks: None, length: None, lossless_only: false };
    assert_eq!(users(&free_slot_choice(&st, &ahead, &song, &current("alice", "flac"), &tried, |c| c["username"] == "kate", &d)), ["bob"]);
    // users a replacement avoids go last
    let two = [answer("bob", true, dir, 4, "flac", 0, 9e6), answer("hana", true, dir, 4, "flac", 0, 1e6)];
    let avoid = json!({ "avoid": { "users": ["bob"] } });
    assert_eq!(users(&free_slot_choice(&st, &two, &album(), &current("alice", "flac"), &[], |_| false, &avoid)), ["hana", "bob"]);
    // a song: the worker's song ranking (title, no unwanted version)
    let song = Want { album: false, artist: "Duumu".into(), name: "Light 1".into(), tracks: None, length: None, lossless_only: false };
    let got = free_slot_choice(&st, &answers, &song, &current("alice", "flac"), &[], |_| false, &d);
    assert!(!got.is_empty() && got.iter().all(|c| c["username"] != "alice" && c["username"] != "dave"), "{got:?}");
}

#[test]
fn download_all_now_says_what_it_did() {
    let lines = [json!({ "source": "streamrip", "label": "Deezer", "limit": 1, "running": 1, "waiting": 6 }),
                 json!({ "source": "ytdlp", "label": "YouTube", "limit": 2, "running": 2, "waiting": 2 })];
    let notes = vec!["Soulseek asked Encore to wait – next attempt at 14:20".to_string()];
    assert_eq!(all_text(false, 8, 3, &lines, &notes),
        "Woke 3 waiting requests; 8 stay in line: Deezer 1 download at a time (6 in line), YouTube 2 downloads at a time (2 in line). Soulseek asked Encore to wait – next attempt at 14:20");
    assert_eq!(all_text(true, 5, 2, &lines, &[]),
        "Moved 5 of your requests to the front of their lines and woke 2 waiting requests; 8 stay in line: Deezer 1 download at a time (6 in line), YouTube 2 downloads at a time (2 in line)");
    assert_eq!(all_text(false, 0, 1, &[], &[]), "Woke 1 waiting request");
    assert_eq!(all_text(false, 0, 0, &[], &[]), "Nothing is waiting – every request is downloading or done");
    assert_eq!(all_text(false, 0, 0, &[], &notes), "Soulseek asked Encore to wait – next attempt at 14:20");
}

#[test]
fn the_words_of_a_place_in_a_queue() {
    assert_eq!(remote_text("alice", Some(12)), "12th in alice's upload queue");
    assert_eq!(remote_text("alice", None), "in alice's upload queue");
    assert_eq!(stays("alice", Some(12)), "it stays 12th in alice's queue");
    assert_eq!(stays("alice", None), "it stays in alice's queue");
    assert_eq!(lower_first("Next in line for Deezer"), "next in line for Deezer");
}
