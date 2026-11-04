//! A YouTube import that names the uploader, not the artist: it still finds the library's song (by title
//! and length), and an item can be linked to a song (or unlinked) by hand, which the re-check keeps.
use super::*;
use crate::test_support::{user, TempState};
use encore_library::matcher::TITLE_ONLY_REASON;

/// A library song.
fn song(st: &AppState, id: i64, title: &str, artist: &str, dur: f64) {
    st.db.execute("INSERT INTO tracks (id, path, title, artist, album, album_id, duration) VALUES (?,?,?,?,?,?,?)",
        p![id, format!("/m/{id}.flac"), title, artist, "Album", "a1", dur]).unwrap();
}

/// The playlist's items: (title, artist, track id, match reason, source title).
fn rows(st: &AppState, pid: i64) -> Vec<(String, String, i64, String, String)> {
    playlists::items(st, pid).iter().map(|r| (r.s("title"), r.s("artist"), r.i("track_id"), r.s("match_reason"), r.s("source_title"))).collect()
}

/// An imported video whose uploader is a lyrics channel finds the library's song by title and length and
/// takes its artist; one whose title names the artist matches as usual; a song the library lacks stays
/// unmatched instead of being taken for another artist's song.
#[test]
fn uploader_named_imports_find_the_library_song() {
    let st = TempState::new("ytmatch-import");
    let u = user(&st, "owner1", "admin");
    song(&st, 1, "Levels", "Avicii", 200.0);
    song(&st, 2, "One More Time", "Daft Punk", 320.0);
    let tracks = vec![
        json!({ "title": "Levels", "artist": "7clouds", "duration": 201.0, "artist_guessed": 1, "source_title": "Levels (Lyrics)" }),
        json!({ "title": "One More Time", "artist": "Daft Punk", "duration": 321.0, "artist_guessed": 0, "source_title": "Daft Punk - One More Time [HD]" }),
        json!({ "title": "Unknown Song", "artist": "Some Fan", "duration": 180.0, "artist_guessed": 1 }),
    ];
    let res = playlists::create(&*st, u.i("id"), "YT", &tracks, "ytdlp", None, "", None, false).unwrap();
    let pid = res["id"].as_i64().unwrap();
    let r = rows(&st, pid);
    assert_eq!((r[0].1.as_str(), r[0].2, r[0].3.as_str(), r[0].4.as_str()), ("Avicii", 1, TITLE_ONLY_REASON, "Levels (Lyrics)"));
    assert_eq!((r[1].2, r[1].3.as_str()), (2, "metadata match"));
    assert_eq!(r[2].2, 0);
    // the same video imported twice is one item
    let (added, dupes) = playlists::add_tracks(&*st, pid, &tracks[..1], "ytdlp").unwrap();
    assert_eq!((added, dupes), (0, 1));
}

/// Linking by hand takes the song's name and keeps what the item was called; the re-check of the library
/// keeps a hand-made link and a hand-made unlink; linking a second item to the same song removes it as a
/// duplicate.
#[test]
fn matching_by_hand_sticks() {
    let st = TempState::new("ytmatch-hand");
    let u = user(&st, "owner1", "admin");
    song(&st, 1, "Levels", "Avicii", 200.0);
    song(&st, 2, "Strobe", "deadmau5", 600.0);
    let tracks = vec![json!({ "title": "lvls edit", "artist": "fan", "duration": 150.0, "artist_guessed": 1, "source_title": "lvls edit!!" }),
                      json!({ "title": "Strobe", "artist": "deadmau5", "duration": 600.0 }),
                      json!({ "title": "levels again", "artist": "other fan", "duration": 150.0 })];
    let pid = playlists::create(&*st, u.i("id"), "YT", &tracks, "ytdlp", None, "", None, false).unwrap()["id"].as_i64().unwrap();
    let ids: Vec<i64> = playlists::items(&*st, pid).iter().map(|r| r.i("id")).collect();
    assert_eq!(rows(&st, pid)[0].2, 0, "nothing like it in the library");

    assert_eq!(playlists::set_match(&*st, pid, ids[0], Some(1)).unwrap(), (true, 0));
    let r = rows(&st, pid);
    assert_eq!((r[0].0.as_str(), r[0].1.as_str(), r[0].2, r[0].3.as_str(), r[0].4.as_str()), ("Levels", "Avicii", 1, playlists::MATCHED_BY_HAND, "lvls edit!!"));
    // the matching was wrong about Strobe (say): unlinked by hand
    assert_eq!(playlists::set_match(&*st, pid, ids[1], None).unwrap(), (true, 0));
    playlists::rematch(&*st, Some(pid), false);
    let r = rows(&st, pid);
    assert_eq!((r[0].2, r[1].2), (1, 0), "the re-check keeps both choices");
    // a third item linked to Levels too is the same song: it goes
    assert_eq!(playlists::set_match(&*st, pid, ids[2], Some(1)).unwrap(), (true, 1));
    assert_eq!(rows(&st, pid).len(), 2);
    // an item of another playlist, a song that isn't there
    assert!(!playlists::set_match(&*st, pid + 1, ids[0], Some(1)).unwrap().0);
    assert!(playlists::set_match(&*st, pid, ids[0], Some(99)).is_err());
}
