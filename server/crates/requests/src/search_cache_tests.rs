//! The kept searches on a file in a temporary folder: keys, keeping and finding, a restart, expiry, the
//! lookup for a request, the size limits, what is kept of an answer, a damaged or unopenable file, one
//! live search per key, and the line's words. No network; `now` is passed in.
use super::*;
use serde_json::json;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Duration;

static SEQ: AtomicUsize = AtomicUsize::new(0);

/// A new, empty folder (removed when dropped).
struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Dir {
        let d = std::env::temp_dir().join(format!("encore-search-cache-{name}-{}-{}", std::process::id(), SEQ.fetch_add(1, Ordering::SeqCst)));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Dir(d)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const NOW: f64 = 1_760_875_320.0;

fn answer(user: &str, files: &[&str]) -> Value {
    json!({ "username": user, "hasFreeUploadSlot": true, "uploadSpeed": 1_200_000, "queueLength": 0, "token": 7, "fileCount": files.len(),
        "files": files.iter().map(|f| json!({ "filename": f, "size": 1000, "bitRate": 320, "length": 200, "code": 1, "isLocked": false })).collect::<Vec<_>>() })
}

fn rows(d: &Path) -> i64 {
    let c = Connection::open(path(d)).unwrap();
    c.query_row("SELECT COUNT(*) FROM search_cache", [], |r| r.get(0)).unwrap()
}

#[test]
fn soulseek_keys_ignore_case_punctuation_word_order_and_repeats() {
    let k = slskd_key("Duumu Illuminate EP");
    assert_eq!(k, "slskd:duumu ep illuminate");
    for same in ["illuminate ep duumu", "  DUUMU  –  Illuminate, EP ", "Duumu Duumu Illuminate EP"] {
        assert_eq!(slskd_key(same), k, "{same}");
    }
    assert_ne!(slskd_key("Duumu Illuminate"), k);
    // an apostrophe is part of a word, as Soulseek reads it
    assert_eq!(slskd_key("Don't Stop"), "slskd:don't stop");
}

#[test]
fn indexer_keys_keep_word_order_and_the_categories() {
    assert_eq!(torznab_key("Travis Scott Rodeo", &["3000"]), "torznab:3000:travis scott rodeo");
    assert_ne!(torznab_key("Rodeo Travis Scott", &["3000"]), torznab_key("Travis Scott Rodeo", &["3000"]));
    assert_eq!(torznab_key("Travis Scott 90210", &["3040", "3000", "3010"]), "torznab:3000,3010,3040:travis scott 90210");
    assert_ne!(torznab_key("Travis Scott 90210", &["3000"]), torznab_key("Travis Scott 90210", &crate::sources::torznab::SONG_CATS));
    // the text as the indexers get it: Ke$ha is searched as Kesha
    assert_eq!(torznab_key("Ke$ha Cannibal", &["3000"]), torznab_key("Kesha Cannibal", &["3000"]));
}

#[test]
fn the_default_text_drops_brackets_as_the_worker_does() {
    use crate::sources::slskd::query_text;
    assert_eq!(query_text(&["Ke$ha", "Cannibal (Deluxe Edition)"]), "Kesha Cannibal");
    assert_eq!(query_text(&["Duumu", "Illuminate EP"]), "Duumu Illuminate EP");
    assert_eq!(query_text(&["", "Song [Live]"]), "Song");
    assert_eq!(worker_texts("album", "Duumu", "Illuminate EP", ""), ["Duumu Illuminate EP", "Illuminate EP"]);
    assert_eq!(worker_texts("album", "Duumu", "ignored", "Illuminate EP (Deluxe)"), ["Duumu Illuminate EP", "Illuminate EP"]);
    assert_eq!(worker_texts("track", "Machine Time", "Moonlight Shadow (nightcore)", "x"), ["Machine Time Moonlight Shadow"]);
}

#[test]
fn kept_answers_come_back_and_survive_a_restart() {
    let d = Dir::new("put");
    let c = Cache::default();
    let res = slim_soulseek(&[answer("alice", &["Music\\Duumu\\Illuminate EP\\01 Track.flac"])]);
    assert!(c.put(&d.0, "slskd", &slskd_key("Duumu Illuminate EP"), "Duumu Illuminate EP", Some(12), &res, NOW));
    let f = c.get(&d.0, "slskd:duumu ep illuminate", NOW + 60.0).expect("kept");
    assert_eq!((f.query.as_str(), f.searched_at, f.request_id), ("Duumu Illuminate EP", NOW, Some(12)));
    assert_eq!(f.results, res);
    assert!(c.get(&d.0, "slskd:other", NOW).is_none());
    // a restart of Encore: a new handle on the same folder still has it
    drop(c);
    let c = Cache::default();
    assert_eq!(c.get(&d.0, "slskd:duumu ep illuminate", NOW + 120.0).unwrap().results, res);
    // a newer search of the same words takes its place
    let res2 = slim_soulseek(&[answer("bob", &["a\\b.mp3"])]);
    assert!(c.put(&d.0, "slskd", "slskd:duumu ep illuminate", "illuminate ep duumu", None, &res2, NOW + 300.0));
    let f = c.get(&d.0, "slskd:duumu ep illuminate", NOW + 301.0).unwrap();
    assert_eq!((f.results, f.query.as_str(), f.request_id), (res2, "illuminate ep duumu", None));
    assert_eq!(rows(&d.0), 1);
}

#[test]
fn answers_expire_after_a_day_and_nothing_found_after_an_hour() {
    let d = Dir::new("expiry");
    let c = Cache::default();
    let res = slim_soulseek(&[answer("alice", &["x\\1.flac"])]);
    c.put(&d.0, "slskd", "slskd:found", "found", None, &res, NOW);
    c.put(&d.0, "slskd", "slskd:empty", "empty", None, &[], NOW);
    assert!(c.get(&d.0, "slskd:empty", NOW + 59.0 * 60.0).is_some(), "an empty answer is kept for an hour");
    assert!(c.get(&d.0, "slskd:empty", NOW + 61.0 * 60.0).is_none());
    assert!(c.get(&d.0, "slskd:found", NOW + 23.0 * 3600.0).is_some(), "results are kept for a day");
    assert!(c.get(&d.0, "slskd:found", NOW + 25.0 * 3600.0).is_none());
    assert_eq!(rows(&d.0), 2, "not deleted by reading");
    // the next write deletes what has expired
    c.put(&d.0, "slskd", "slskd:new", "new", None, &res, NOW + 2.0 * 3600.0);
    assert_eq!(rows(&d.0), 2, "the empty answer went");
    c.put(&d.0, "slskd", "slskd:newer", "newer", None, &res, NOW + 25.0 * 3600.0);
    assert_eq!(rows(&d.0), 2, "the old answer went");
    assert!(c.get(&d.0, "slskd:new", NOW + 25.0 * 3600.0).is_some());
}

#[test]
fn a_request_finds_its_newest_answer_with_results() {
    let d = Dir::new("request");
    let c = Cache::default();
    let created = NOW - 7200.0;
    let res = |u: &str| slim_soulseek(&[answer(u, &["x\\1.flac"])]);
    // the worker's album searches: "artist album" found nothing, "album" found something
    c.put(&d.0, "slskd", &slskd_key("Duumu Illuminate EP"), "Duumu Illuminate EP", None, &[], NOW - 600.0);
    c.put(&d.0, "slskd", &slskd_key("Illuminate EP"), "Illuminate EP", None, &res("worker"), NOW - 590.0);
    let keys: Vec<String> = worker_texts("album", "Duumu", "Illuminate EP", "").iter().map(|t| slskd_key(t)).collect();
    let f = c.get_for_request(&d.0, "slskd", Some(5), &keys, created, NOW).expect("found");
    assert_eq!((f.query.as_str(), f.results[0]["username"].as_str()), ("Illuminate EP", Some("worker")));
    // a picker search made for the request with other words, later: the newest wins
    c.put(&d.0, "slskd", &slskd_key("illuminate duumu flac"), "illuminate duumu flac", Some(5), &res("picker"), NOW - 60.0);
    let f = c.get_for_request(&d.0, "slskd", Some(5), &keys, created, NOW).unwrap();
    assert_eq!(f.query, "illuminate duumu flac");
    // an empty answer for the request is skipped, however new
    c.put(&d.0, "slskd", &slskd_key("illuminate typo"), "illuminate typo", Some(5), &[], NOW - 30.0);
    assert_eq!(c.get_for_request(&d.0, "slskd", Some(5), &keys, created, NOW).unwrap().query, "illuminate duumu flac");
    // an answer kept for request 5 before request 5 was made belongs to an earlier request of that id
    // (a database restored from a backup)
    assert_eq!(c.get_for_request(&d.0, "slskd", Some(5), &keys, NOW - 59.0, NOW).unwrap().query, "Illuminate EP", "only the worker's texts count");
    assert!(c.get_for_request(&d.0, "slskd", Some(5), &[], NOW - 59.0, NOW).is_none());
    // another kind, or too old: nothing
    assert!(c.get_for_request(&d.0, "torznab", Some(5), &keys, created, NOW).is_none());
    assert!(c.get_for_request(&d.0, "slskd", Some(5), &keys, created, NOW + KEEP).is_none());
    // without a request: the keys alone
    assert_eq!(c.get_for_request(&d.0, "slskd", None, &keys, 0.0, NOW).unwrap().query, "Illuminate EP");
}

#[test]
fn the_oldest_go_first_over_the_size_limit_and_a_big_answer_is_not_kept() {
    // the limits' values
    assert_eq!((MAX_BYTES, MAX_ENTRY), (32 * 1024 * 1024, 2 * 1024 * 1024));
    assert_eq!((KEEP, KEEP_EMPTY), (86_400.0, 3_600.0));
    let d = Dir::new("limits");
    let c = Cache::default();
    // answers that don't compress (random names), all of about the same size
    let noisy = |n: usize| -> Vec<Value> {
        let names: Vec<String> = (0..n).map(|_| format!("{}\\{}.flac", hex::encode(rand::random::<[u8; 32]>()), hex::encode(rand::random::<[u8; 32]>()))).collect();
        slim_soulseek(&[answer("u", &names.iter().map(String::as_str).collect::<Vec<_>>())])
    };
    assert!(c.put(&d.0, "slskd", "slskd:size", "q", None, &noisy(40), NOW - 100.0));
    let one: i64 = Connection::open(path(&d.0)).unwrap().query_row("SELECT bytes FROM search_cache", [], |r| r.get(0)).unwrap();
    assert!(one > 2000, "{one}");
    c.clear(&d.0);
    // room for three and a half of them, two of them at most in one answer
    c.set_limits(one * 7 / 2, one as usize * 2);
    for i in 0..5 {
        assert!(c.put(&d.0, "slskd", &format!("slskd:{i}"), "q", None, &noisy(40), NOW + i as f64));
    }
    let kept: Vec<bool> = (0..5).map(|i| c.get(&d.0, &format!("slskd:{i}"), NOW + 10.0).is_some()).collect();
    assert_eq!(kept, [false, false, true, true, true], "the oldest went");
    let total: i64 = Connection::open(path(&d.0)).unwrap().query_row("SELECT SUM(bytes) FROM search_cache", [], |r| r.get(0)).unwrap();
    assert!(total <= one * 7 / 2, "{total}");
    // too big to keep: not kept, and what was kept under its key goes
    assert!(!c.put(&d.0, "slskd", "slskd:4", "q", None, &noisy(120), NOW + 20.0));
    assert!(c.get(&d.0, "slskd:4", NOW + 21.0).is_none());
    assert!(c.get(&d.0, "slskd:3", NOW + 21.0).is_some());
}

#[test]
fn only_what_the_readers_use_is_kept() {
    let res = vec![
        answer("alice", &["Music\\A\\01.flac", "Music\\A\\cover.jpg", "Music\\A\\info.nfo"]),
        answer("bob", &["Music\\B\\cover.jpg"]),
        json!({ "username": "carol", "files": [] }),
        json!({ "username": "dave" }),
    ];
    let slim = slim_soulseek(&res);
    assert_eq!(slim, vec![json!({ "username": "alice", "hasFreeUploadSlot": true, "uploadSpeed": 1_200_000, "queueLength": 0,
        "files": [{ "filename": "Music\\A\\01.flac", "size": 1000, "bitRate": 320, "length": 200 }] })]);
    let rel = json!({ "title": "A - B [FLAC]", "guid": "g", "size": 5, "seeders": 3, "age": 2, "indexer": "T", "protocol": "torrent",
        "magnetUrl": "magnet:?x", "downloadUrl": "http://d", "infoUrl": "http://i", "categories": [3000] });
    let kept = slim_releases(&[rel]);
    assert_eq!(kept[0].as_object().unwrap().len(), 9);
    assert!(kept[0].get("infoUrl").is_none() && kept[0]["magnetUrl"] == "magnet:?x");
}

#[test]
fn a_damaged_or_unopenable_file_means_no_cache_and_no_error() {
    // damaged: set aside, and a new file is started
    let d = Dir::new("damaged");
    std::fs::create_dir_all(d.0.join("cache")).unwrap();
    std::fs::write(path(&d.0), b"this is not a database, it is a damaged file of some size".repeat(100)).unwrap();
    let c = Cache::default();
    assert!(c.get(&d.0, "slskd:x", NOW).is_none());
    assert!(c.put(&d.0, "slskd", "slskd:x", "x", None, &[], NOW), "a new file works");
    assert!(c.get(&d.0, "slskd:x", NOW).is_some());
    assert!(path(&d.0).with_extension("db.damaged").is_file(), "the damaged file was set aside");
    // can't be opened (the cache folder is a file): off, quietly
    let d = Dir::new("unopenable");
    std::fs::write(d.0.join("cache"), b"a file where the folder should be").unwrap();
    let c = Cache::default();
    assert!(!c.put(&d.0, "slskd", "slskd:x", "x", None, &[], NOW));
    assert!(c.get(&d.0, "slskd:x", NOW).is_none());
    assert!(c.get_for_request(&d.0, "slskd", Some(1), &["slskd:x".into()], 0.0, NOW).is_none());
    assert!(c.is_off());
}

#[test]
fn clear_forgets_everything() {
    let d = Dir::new("clear");
    let c = Cache::default();
    c.put(&d.0, "slskd", "slskd:x", "x", Some(1), &[], NOW);
    c.clear(&d.0);
    assert!(!path(&d.0).exists());
    assert!(c.get(&d.0, "slskd:x", NOW).is_none());
    assert!(c.put(&d.0, "slskd", "slskd:y", "y", None, &[], NOW), "made again on the next use");
}

#[tokio::test(start_paused = true)]
async fn one_live_search_per_key_and_it_ends_without_its_callers() {
    let c = Cache::default();
    let runs = Arc::new(AtomicUsize::new(0));
    let work = |runs: Arc<AtomicUsize>| async move {
        runs.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_secs(10)).await;
        Ok(Found { key: "k".into(), query: "q".into(), searched_at: NOW, request_id: None, results: vec![json!({ "username": "a" })] })
    };
    let (a, b) = tokio::join!(c.single("k", work(runs.clone())), c.single("k", work(runs.clone())));
    assert_eq!(runs.load(Ordering::SeqCst), 1, "one search for both");
    assert_eq!(a.unwrap().results, b.unwrap().results);
    assert!(!c.running("k"));
    // the caller goes away (a closed picker): the search goes on to the end
    let done = Arc::new(AtomicBool::new(false));
    let d2 = done.clone();
    let gone = tokio::time::timeout(Duration::from_secs(2), c.single("k2", async move {
        tokio::time::sleep(Duration::from_secs(10)).await;
        d2.store(true, Ordering::SeqCst);
        Err(Failure::Busy("busy".into()))
    })).await;
    assert!(gone.is_err());
    assert!(c.running("k2"));
    tokio::time::sleep(Duration::from_secs(9)).await;
    assert!(done.load(Ordering::SeqCst) && !c.running("k2"), "it ended, and the next call searches again");
    // a failure comes back as the typed error
    let e = c.single("k3", async { Err(Failure::NotConnected("Soulseek isn't connected – connecting shortly".into())) }).await.unwrap_err().into_error();
    assert!(e.is::<crate::slskd_session::NotConnected>(), "{e}");
    assert!(Failure::Busy("b".into()).into_error().is::<crate::slskd_session::Busy>());
    assert_eq!(Failure::of(&anyhow::anyhow!("x")), Failure::Other("x".into()));
}

#[test]
fn the_line_says_when_on_the_servers_clock() {
    let at = NOW;
    let time = crate::slskd_session::clock(at);
    assert_eq!(found_words(at, at + 60.0), format!("Found at {time}"));
    assert_eq!(found_words(at, at + 86_400.0), format!("Found yesterday at {time}"));
    let two = found_words(at, at + 2.0 * 86_400.0);
    assert!(two.starts_with("Found at ") && two.ends_with(&time) && two.contains('/'), "{two}");
}
