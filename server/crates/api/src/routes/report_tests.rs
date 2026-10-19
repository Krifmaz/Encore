//! Report a problem: the source is blocked, the file parked in the trash, a replacement requested with
//! that source left out, and the replacement takes over the track id (mocked sources, no network).
use super::*;
use crate::test_support::user;
use encore_db::now;
use std::path::{Path as FsPath, PathBuf};

fn state(name: &str) -> (AppState, PathBuf) {
    let dir = std::env::temp_dir().join(format!("encore-report-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("music")).unwrap();
    let st = crate::test_support::state_in(&dir);
    st.cfg.set_many(&json!({ "library_path": dir.join("music").to_string_lossy(), "quality_check_on_import": false }));
    (st, dir)
}

/// A song in the library: a file on disk and its row.
fn song(st: &AppState, dir: &FsPath, rel: &str, title: &str, track: i64, album_id: &str) -> (i64, PathBuf) {
    let f = dir.join("music").join(rel);
    std::fs::create_dir_all(f.parent().unwrap()).unwrap();
    std::fs::write(&f, format!("audio of {title}")).unwrap();
    let id = st.db.execute("INSERT INTO tracks (path, album_id, title, artist, album, albumartist, track, disc, duration, format, mb_releasegroupid, added_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?)",
        p![f.to_string_lossy(), album_id, title, "Travis Scott", "Rodeo", "Travis Scott", track, 1, 200.0, "flac", "rg-rodeo", now() - 1000.0]).unwrap();
    (id, f)
}

#[tokio::test]
async fn report_blocks_parks_and_the_replacement_keeps_the_id() {
    let (st, dir) = state("flow");
    let listener = user(&st, "listener", "user");
    let (tid, f) = song(&st, &dir, "Travis Scott/Singles/Travis Scott - 90210.flac", "90210", 0, "single-90210");
    std::fs::write(f.with_extension("lrc"), "[00:01.00] la").unwrap();
    // it came from one Soulseek user's file
    record_origin(&st, std::slice::from_ref(&f), &json!({ "source": "Soulseek", "block": { "kind": "slskd", "key": "baduser|@@music\\90210.flac|123", "label": "baduser: @@music/90210.flac" } }));
    // it is liked (in a playlist) and was played
    let pl = st.db.execute("INSERT INTO playlists (user_id, name, kind, created_at, updated_at) VALUES (?,?,?,?,?)", p![listener.i("id"), "Liked", "liked", now(), now()]).unwrap();
    st.db.execute("INSERT INTO playlist_items (playlist_id, position, title, track_id, added_at) VALUES (?,?,?,?,?)", p![pl, 0, "90210", tid, now()]).unwrap();
    st.db.execute("INSERT INTO plays (user_id, track_id, seconds, played_at) VALUES (?,?,?,?)", p![listener.i("id"), tid, 200.0, now()]).unwrap();

    // someone who never played it can't report it
    let other = user(&st, "other", "user");
    let body = ReportBody { kind: "track".into(), track_id: Some(tid), reason: "wrong_version".into(), note: Some("the intro is missing".into()), ..Default::default() };
    assert_eq!(file_report(&st, &other, &body).unwrap_err().status.as_u16(), 403);
    assert_eq!(file_report(&st, &listener, &ReportBody { reason: "nonsense".into(), ..body.clone() }).unwrap_err().status.as_u16(), 400);

    let rep = file_report(&st, &listener, &body).unwrap();
    assert_eq!(rep["status"], "replacing");
    assert_eq!(rep["reason_label"], "Wrong version (live, remix, edit, missing parts)");
    // 1. the exact file is blocked
    assert!(blocked(&st, "slskd", "baduser|@@music\\90210.flac|123"));
    let bad = json!({ "username": "baduser", "directory": "@@music", "files": [{ "filename": "@@music\\90210.flac", "size": 123 }] });
    let same_user_other_file = json!({ "username": "baduser", "directory": "@@other", "files": [{ "filename": "@@other\\90210.flac", "size": 999 }] });
    let good = json!({ "username": "gooduser", "directory": "@@x", "files": [{ "filename": "@@x\\90210.flac", "size": 456 }] });
    assert!(slskd_blocked(&st, &bad) && !slskd_blocked(&st, &same_user_other_file) && !slskd_blocked(&st, &good));
    // 2. the file (and its lyrics) went to the trash, the row waits for the replacement
    assert!(!f.exists() && !f.with_extension("lrc").exists());
    let trash: Vec<_> = std::fs::read_dir(trash_dir(&st)).unwrap().flatten().collect();
    assert_eq!(trash.len(), 2, "{trash:?}");
    assert!(st.db.one("SELECT 1 FROM tracks WHERE id=?", p![tid]).unwrap().is_none());
    assert!(st.db.one("SELECT 1 FROM track_parked WHERE id=?", p![tid]).unwrap().is_some());
    // 3. the same song is requested again, that source last, other users first – no approval needed
    let req = encore_requests::worker::get(&st, rep["request_id"].as_i64().unwrap()).unwrap();
    assert_eq!(req.row.s("status"), "queued");
    assert_eq!((req.row.s("kind"), req.row.s("title")), ("track".to_string(), "90210".to_string()));
    assert_eq!(req.d["replace"]["tracks"], json!([tid]));
    assert_eq!(req.d["avoid"], json!({ "users": ["baduser"], "source": "slskd" }));
    assert_eq!(req.d["album_mode"], "song");
    let mut order: Vec<String> = ["slskd", "slskd_lossless", "streamrip", "ytdlp"].iter().map(|s| s.to_string()).collect();
    avoid_source(&req.d, &mut order);
    assert_eq!(order, ["streamrip", "ytdlp", "slskd", "slskd_lossless"]);
    // the mocked Soulseek results: the reported file is gone, the reporting user's other copy goes last
    let mut cands = vec![bad, same_user_other_file, good];
    cands.retain(|c| !slskd_blocked(&st, c));
    avoid_users(&req.d, &mut cands);
    assert_eq!(cands.iter().map(|c| c["username"].as_str().unwrap()).collect::<Vec<_>>(), ["gooduser", "baduser"]);
    // reporting it twice while the replacement is on its way
    assert_eq!(file_report(&st, &listener, &body).unwrap_err().status.as_u16(), 404);

    // the replacement arrives under a new row; it takes over the old id
    let (nid, nf) = song(&st, &dir, "Travis Scott/Singles/Travis Scott - 90210 (2).flac", "90210", 0, "single-90210");
    st.db.execute("UPDATE tracks SET added_at=? WHERE id=?", p![now(), nid]).unwrap();
    assert_ne!(nid, tid);
    finish(&st, &req.d, std::slice::from_ref(&nf), req.row.f("created_at"));
    let t = st.db.one("SELECT * FROM tracks WHERE id=?", p![tid]).unwrap().unwrap();
    assert_eq!(t.s("path"), nf.to_string_lossy());
    assert!(st.db.one("SELECT 1 FROM tracks WHERE id=?", p![nid]).unwrap().is_none());
    assert_eq!(st.db.one("SELECT track_id FROM playlist_items WHERE playlist_id=?", p![pl]).unwrap().unwrap().i("track_id"), tid);
    assert_eq!(st.db.one("SELECT track_id FROM plays WHERE user_id=?", p![listener.i("id")]).unwrap().unwrap().i("track_id"), tid);
    assert!(st.db.one("SELECT 1 FROM track_parked", p![]).unwrap().is_none());
    let rep = report_json(&st, rep["id"].as_i64().unwrap()).unwrap();
    assert_eq!(rep["status"], "replaced");
    // the trash keeps the reported file for 7 days
    assert_eq!(purge_trash(&st), 0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_failed_replacement_puts_the_album_back() {
    let (st, dir) = state("album");
    let admin = user(&st, "boss", "admin");
    let (a, fa) = song(&st, &dir, "Travis Scott/Rodeo (2015)/01 - Pornography.flac", "Pornography", 1, "rodeo");
    let (b, fb) = song(&st, &dir, "Travis Scott/Rodeo (2015)/02 - Oh My Dis Side.flac", "Oh My Dis Side", 2, "rodeo");
    for (id, rel) in [(a, "01"), (b, "02")] {
        st.db.execute("INSERT INTO track_origin (track_id, kind, key, label) VALUES (?,?,?,?)", p![id, "torrent", "0123abcd", format!("Rodeo [FLAC] {rel}")]).unwrap();
    }
    let rep = file_report(&st, &admin, &ReportBody { kind: "album".into(), album_id: Some("rodeo".into()), reason: "fake_lossless".into(), ..Default::default() }).unwrap();
    assert!(blocked(&st, "torrent", "0123abcd"));
    assert!(release_blocked(&st, &json!({ "protocol": "torrent", "url": "magnet:?xt=urn:btih:0123ABCD&dn=Rodeo", "title": "Rodeo" })));
    assert!(!release_blocked(&st, &json!({ "protocol": "torrent", "url": "magnet:?xt=urn:btih:ffff", "title": "Rodeo" })));
    let req = encore_requests::worker::get(&st, rep["request_id"].as_i64().unwrap()).unwrap();
    assert_eq!((req.row.s("kind"), req.row.s("mbid"), req.row.s("title")), ("album".to_string(), "rg-rodeo".to_string(), "Rodeo".to_string()));
    assert_eq!(req.d["avoid"]["source"], "torznab");
    assert!(!fa.exists() && !fb.exists());
    // nothing better anywhere: the songs come back where they were, same ids; the block stays
    replacement_failed(&st, &req.d, "All sources exhausted");
    assert!(fa.exists() && fb.exists());
    assert_eq!(st.db.one("SELECT path FROM tracks WHERE id=?", p![a]).unwrap().unwrap().s("path"), fa.to_string_lossy());
    assert_eq!(st.db.one("SELECT path FROM tracks WHERE id=?", p![b]).unwrap().unwrap().s("path"), fb.to_string_lossy());
    assert_eq!(report_json(&st, rep["id"].as_i64().unwrap()).unwrap()["status"], "no_replacement");
    assert!(blocked(&st, "torrent", "0123abcd"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_cancelled_replacement_is_noticed() {
    let (st, dir) = state("cancel");
    let admin = user(&st, "boss", "admin");
    let (tid, f) = song(&st, &dir, "Travis Scott/Singles/Travis Scott - Antidote.flac", "Antidote", 0, "single-antidote");
    let rep = file_report(&st, &admin, &ReportBody { kind: "track".into(), track_id: Some(tid), reason: "other".into(), note: Some("hiss".into()), ..Default::default() }).unwrap();
    st.db.execute("UPDATE requests SET status='cancelled' WHERE id=?", p![rep["request_id"]]).unwrap();
    reconcile(&st);
    assert!(f.exists());
    assert!(st.db.one("SELECT 1 FROM tracks WHERE id=?", p![tid]).unwrap().is_some());
    assert_eq!(report_json(&st, rep["id"].as_i64().unwrap()).unwrap()["status"], "no_replacement");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn block_keys() {
    assert_eq!(block_of_url("https://music.youtube.com/watch?v=abc123&si=x").unwrap().1, "abc123");
    assert_eq!(block_of_url("https://youtu.be/xyz").unwrap().1, "xyz");
    assert_eq!(block_of_url("https://www.youtube.com/playlist?list=PL1").unwrap().1, "list:PL1");
    assert_eq!(block_of_url("https://www.deezer.com/track/3135556").unwrap(), ("streamrip".to_string(), "deezer:3135556".to_string(), "Deezer track 3135556".to_string()));
    assert_eq!(block_of_url("https://tidal.com/browse/album/77").unwrap().1, "tidal:77");
    assert!(block_of_url("https://example.com/x").is_none());
    assert_eq!(block_of_release(&json!({ "protocol": "usenet", "guid": "nzb-1", "url": "http://indexer/get?id=1", "title": "X" })).unwrap(), ("usenet".to_string(), "nzb-1".to_string(), "X".to_string()));
    let song = json!({ "username": "u", "directory": "d", "files": [{ "filename": "d\\a.flac", "size": 5 }] });
    assert_eq!(block_of_slskd(&song, false).unwrap().1, "u|d\\a.flac|5");
    assert_eq!(block_of_slskd(&song, true).unwrap().1, "u|d");
}
