//! A whole album through the import (tags, names, the library, a second import of the same album) with
//! the server's real state; the tagging itself is encore_library::tagger.
use encore_db::RowExt;
use encore_db::p;
use encore_library::tagger::{import_album_files, AlbumImport};
use encore_media::tags::read_tags;
use serde_json::json;

#[tokio::test]
async fn album_import_offline() {
    let dir = std::env::temp_dir().join(format!("encore-import-{}", std::process::id()));
    let (lib, dl) = (dir.join("lib"), dir.join("dl/Some Album"));
    std::fs::create_dir_all(&lib).unwrap();
    std::fs::create_dir_all(&dl).unwrap();
    // the library folder and an address where no MusicBrainz answers, as settings of this server only
    let st = crate::test_support::TestServer::in_dir(&dir)
        .with(serde_json::json!({ "library_path": lib.to_string_lossy(), "musicbrainz_url": "http://127.0.0.1:9" })).st;
    let mut files = vec![];
    for (i, (f, name)) in [(440, "first"), (660, "Second (album version)")].iter().enumerate() {
        let p = dl.join(format!("{:02} track.flac", i + 1));
        let ok = std::process::Command::new("ffmpeg").args(["-v", "error", "-y", "-f", "lavfi", "-i", &format!("sine=frequency={f}:duration=3")])
            .args(["-metadata", &format!("title={name}"), "-metadata", "artist=Tester", "-metadata", &format!("track={}", i + 1)]).arg(&p).status();
        if !ok.map(|s| s.success()).unwrap_or(false) {
            return; // no ffmpeg here
        }
        files.push(p);
    }
    let meta = json!({ "tracks": [{ "title": "First", "artist": "Tester", "track": 1, "disc": 1 }, { "title": "Second", "artist": "Tester", "track": 2, "disc": 1 }], "year": "2024", "upc": "123" });
    let new = import_album_files(&st, &files, AlbumImport { artist: "Tester", album: "Some Album", rgid: None, source_root: &dir.join("dl"), reject: true, audio: true, source_meta: Some(&meta), hint: None }).await.unwrap();
    assert_eq!(new.len(), 2);
    assert_eq!(new[0], lib.join("Tester/Some Album (2024)/01 - First.flac"));
    let t = read_tags(&new[1].to_string_lossy());
    assert_eq!(t.s("title"), "Second");
    assert_eq!(t.s("album"), "Some Album");
    assert!(!dl.exists(), "download folder cleaned up");
    let n = st.db.one("SELECT COUNT(*) n FROM tracks", p![]).unwrap().unwrap().i("n");
    assert_eq!(n, 2);
    // the same album again: same paths, no "(2)" copies
    std::fs::create_dir_all(&dl).unwrap();
    let again = dl.join("01 track.flac");
    std::fs::copy(&new[0], &again).unwrap();
    let new2 = import_album_files(&st, &[again], AlbumImport { artist: "Tester", album: "Some Album", rgid: None, source_root: &dir.join("dl"), reject: true, audio: true, source_meta: Some(&meta), hint: None }).await.unwrap();
    assert_eq!(new2[0], new[0]);
    let _ = std::fs::remove_dir_all(&dir);
}

/// With no track list (no MusicBrainz match, none from the source), files tagged as another album of the
/// artist are turned away as the wrong release and stay where they are; with the source's track list, the
/// same files are judged by their song titles instead and go in.
#[tokio::test]
async fn another_album_by_its_tags_is_turned_away() {
    let dir = std::env::temp_dir().join(format!("encore-import-other-{}", std::process::id()));
    let (lib, dl) = (dir.join("lib"), dir.join("dl/Some Album"));
    std::fs::create_dir_all(&lib).unwrap();
    std::fs::create_dir_all(&dl).unwrap();
    let st = crate::test_support::TestServer::in_dir(&dir)
        .with(serde_json::json!({ "library_path": lib.to_string_lossy(), "musicbrainz_url": "http://127.0.0.1:9" })).st;
    let mut files = vec![];
    for (i, (f, name)) in [(440, "First"), (660, "Second")].iter().enumerate() {
        let p = dl.join(format!("{:02} track.flac", i + 1));
        let ok = std::process::Command::new("ffmpeg").args(["-v", "error", "-y", "-f", "lavfi", "-i", &format!("sine=frequency={f}:duration=3")])
            .args(["-metadata", &format!("title={name}"), "-metadata", "artist=Tester", "-metadata", "album=Other Record", "-metadata", &format!("track={}", i + 1)]).arg(&p).status();
        if !ok.map(|s| s.success()).unwrap_or(false) {
            return; // no ffmpeg here
        }
        files.push(p);
    }
    let root = dir.join("dl");
    let what = |meta| AlbumImport { artist: "Tester", album: "Some Album", rgid: None, source_root: &root, reject: true, audio: true, source_meta: meta, hint: None };
    let e = import_album_files(&st, &files, what(None)).await.unwrap_err();
    assert_eq!(e.to_string(), "Wrong release: the download is the album “Other Record”, not “Some Album”");
    assert!(files.iter().all(|f| f.exists()), "nothing was moved");
    assert_eq!(st.db.one("SELECT COUNT(*) n FROM tracks", p![]).unwrap().unwrap().i("n"), 0);
    let meta = json!({ "tracks": [{ "title": "First", "artist": "Tester", "track": 1, "disc": 1 }, { "title": "Second", "artist": "Tester", "track": 2, "disc": 1 }] });
    let new = import_album_files(&st, &files, what(Some(&meta))).await.unwrap();
    assert_eq!(new.len(), 2);
    let _ = std::fs::remove_dir_all(&dir);
}
