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

/// A replacement that takes over the reported song's id keeps who first asked for the song (contract
/// library-types §9.2 #16): the new download's own adder (another user's request, or none) doesn't replace
/// "Requested by alex".
#[tokio::test]
async fn the_replacement_keeps_who_asked_for_the_song() {
    let (st, dir) = state("adder");
    let admin = user(&st, "boss", "admin");
    let alex = user(&st, "alex", "user");
    for (title, via, new_adder, new_via) in [("Antidote", "request", Some(admin.i("id")), "request"), ("Maria", "link", None, "scan")] {
        let (tid, _f) = song(&st, &dir, &format!("Travis Scott/Singles/Travis Scott - {title}.flac"), title, 0, &format!("single-{title}"));
        st.db.execute("UPDATE tracks SET added_by=?, added_via=?, requested_at=1234 WHERE id=?", p![alex.i("id"), via, tid]).unwrap();
        let rep = file_report(&st, &admin, &ReportBody { kind: "track".into(), track_id: Some(tid), reason: "other".into(), note: Some("hiss".into()), ..Default::default() }).unwrap();
        let req = encore_requests::worker::get(&st, rep["request_id"].as_i64().unwrap()).unwrap();
        // the replacement arrives under a new row stamped by its own import
        let (nid, nf) = song(&st, &dir, &format!("Travis Scott/Singles/Travis Scott - {title} (2).flac"), title, 0, &format!("single-{title}"));
        st.db.execute("UPDATE tracks SET added_at=?, added_by=?, added_via=?, requested_at=? WHERE id=?", p![now(), new_adder, new_via, now(), nid]).unwrap();
        finish(&st, &req.d, std::slice::from_ref(&nf), req.row.f("created_at"));
        let t = st.db.one("SELECT * FROM tracks WHERE id=?", p![tid]).unwrap().unwrap();
        assert_eq!(t.s("path"), nf.to_string_lossy(), "{title}");
        assert_eq!((t.i("added_by"), t.s("added_via"), t.f("requested_at")), (alex.i("id"), via.to_string(), 1234.0), "{title}");
        assert!(st.db.one("SELECT 1 FROM tracks WHERE id=?", p![nid]).unwrap().is_none());
        assert_eq!(report_json(&st, rep["id"].as_i64().unwrap()).unwrap()["status"], "replaced");
    }
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

/// A stand-in LRCLIB for "Travis Scott – 90210": its exact match has the wrong words; a search also
/// offers the right words time-synced (version 2) and plain (version 3). → its address
async fn lrclib() -> String {
    use axum::routing::get;
    let app = axum::Router::new()
        .route("/get", get(|| async { axum::Json(json!({ "id": 1, "duration": 200.0, "syncedLyrics": "[00:01.00] wrong words\n[00:02.00] here" })) }))
        .route("/search", get(|| async {
            axum::Json(json!([
                { "id": 1, "duration": 200.0, "syncedLyrics": "[00:01.00] wrong words\n[00:02.00] here" },
                { "id": 2, "duration": 201.0, "syncedLyrics": "[00:05.00] right words" },
                { "id": 3, "duration": 200.0, "plainLyrics": "Right words" },
            ]))
        }));
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move {
        let _ = axum::serve(l, app).await;
    });
    url
}

/// Wrong lyrics / Lyrics out of sync never touch the audio: no block, no trash, no new download; the
/// lyrics file is replaced by another LRCLIB version (time-synced first) and the rejected one stays away.
#[tokio::test]
async fn a_lyrics_report_keeps_the_song_and_finds_other_lyrics() {
    let dir = std::env::temp_dir().join(format!("encore-report-lyrics-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("music")).unwrap();
    let st = crate::test_support::TestServer::in_dir(&dir).with_urls(encore_http::Urls { lrclib: lrclib().await, ..Default::default() }).st;
    st.cfg.set_many(&json!({ "library_path": dir.join("music").to_string_lossy(), "quality_check_on_import": false, "lyrics_save_files": true, "lyrics_embed": false }));
    let listener = user(&st, "listener", "user");
    let (tid, f) = song(&st, &dir, "Travis Scott/Singles/Travis Scott - 90210.flac", "90210", 0, "single-90210");
    st.db.execute("INSERT INTO plays (user_id, track_id, seconds, played_at) VALUES (?,?,?,?)", p![listener.i("id"), tid, 200.0, now()]).unwrap();
    // the wrong lyrics are saved next to the song (the exact match gave them)
    std::fs::write(f.with_extension("lrc"), "[00:01.00] Wrong words\r\n[00:02.00] here\r\n").unwrap();
    let audio = std::fs::read(&f).unwrap();
    let body = ReportBody { kind: "track".into(), track_id: Some(tid), reason: "wrong_lyrics".into(), ..Default::default() };

    // lyrics are reported on one song, and only through file()
    assert_eq!(file(&st, &listener, &ReportBody { kind: "album".into(), album_id: Some("single-90210".into()), ..body.clone() }).await.unwrap_err().status.as_u16(), 400);
    assert_eq!(file_report(&st, &listener, &body).unwrap_err().status.as_u16(), 400);

    let rep = file(&st, &listener, &body).await.unwrap();
    assert_eq!(rep["status"], "lyrics_found", "{rep}");
    assert_eq!(rep["reason_label"], "Wrong lyrics");
    // the other words, time-synced, are in the answer and saved in place of the wrong ones
    assert_eq!(rep["lyrics"]["lyrics"], "[00:05.00] right words");
    assert_eq!(rep["lyrics"]["synced"], true);
    assert_eq!(std::fs::read_to_string(f.with_extension("lrc")).unwrap(), "[00:05.00] right words");
    // the song is untouched: same file, same row, nothing blocked, parked or requested
    assert_eq!(std::fs::read(&f).unwrap(), audio);
    assert!(st.db.one("SELECT 1 FROM tracks WHERE id=?", p![tid]).unwrap().is_some());
    assert!(st.db.one("SELECT 1 FROM source_block", p![]).unwrap().is_none());
    assert!(st.db.one("SELECT 1 FROM track_parked", p![]).unwrap().is_none());
    assert!(st.db.one("SELECT 1 FROM requests", p![]).unwrap().is_none());
    assert!(!trash_dir(&st).exists() || std::fs::read_dir(trash_dir(&st)).unwrap().next().is_none());
    // a later lookup (the file gone again) still never picks the wrong words
    std::fs::remove_file(f.with_extension("lrc")).unwrap();
    let t = st.db.one("SELECT * FROM tracks WHERE id=?", p![tid]).unwrap().unwrap();
    assert_eq!(encore_library::lyrics::fetch_for_track(&st, &t, false).await.unwrap()["lyrics"], "[00:05.00] right words");

    // the new timing is off too: the same words without timing are shown instead
    let sync = file(&st, &listener, &ReportBody { reason: "lyrics_sync".into(), ..body.clone() }).await.unwrap();
    assert_eq!(sync["status"], "lyrics_found");
    assert_eq!(sync["lyrics"]["lyrics"], "Right words");
    assert_eq!(sync["lyrics"]["synced"], false);
    assert!(!f.with_extension("lrc").exists() && f.with_extension("txt").exists());

    // and those are wrong as well: nothing is left, so no lyrics rather than wrong ones
    let none = file(&st, &listener, &body).await.unwrap();
    assert_eq!(none["status"], "lyrics_none");
    assert!(none["lyrics"]["lyrics"].is_null());
    assert!(!f.with_extension("txt").exists());
    assert_eq!(std::fs::read(&f).unwrap(), audio);

    // dismissing a lyrics report lets the lyrics it rejected come back
    let n = |st: &AppState| st.db.one("SELECT COUNT(*) n FROM lyrics_reject WHERE track_id=?", p![tid]).unwrap().unwrap().i("n");
    assert_eq!(n(&st), 3);
    dismiss_report(&st, none["id"].as_i64().unwrap()).await.unwrap();
    assert_eq!(n(&st), 2);
    let _ = std::fs::remove_dir_all(&dir);
}
