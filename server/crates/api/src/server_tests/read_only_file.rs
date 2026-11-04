//! A library file that can't be written while its folder can: a mergerfs pool with one disk set read-only
//! (`/mnt/media7=RW:/mnt/media2=RO`) shows every file of that disk, lets new files land on the other disks,
//! and answers EROFS to any change of the old ones. Tagging such a file writes it anew next to itself
//! (encore_library::tagger::tag_file), an import that replaces it puts the new file under a name of its own,
//! and neither reads as a storage problem: the error is pinned on the file (encore_core::fsx::narrow) and
//! names the folder that was tested. The fault seam stands in for the read-only disk.
use crate::test_support::TestServer;
use encore_core::fsx::{self, fault, Step, StepError};
use encore_db::{Row, RowExt};
use serde_json::json;
use std::path::Path;

/// A short FLAC with a title (false when there is no ffmpeg here).
fn flac(path: &Path, title: &str) -> bool {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::process::Command::new("ffmpeg").args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=frequency=440:duration=2", "-metadata"])
        .arg(format!("title={title}")).arg(path).status().map(|s| s.success()).unwrap_or(false)
}

/// The tags to write: a new title.
fn title(t: &str) -> Row {
    let mut r = Row::new();
    r.insert("title".into(), json!(t));
    r
}

/// A server whose library is its own folder (registered as Encore's, so EROFS there reads as storage).
fn server(name: &str) -> (TestServer, std::path::PathBuf) {
    let ts = TestServer::new(&format!("rofile-{name}"));
    let lib = ts.dir.join("music");
    std::fs::create_dir_all(&lib).unwrap();
    ts.st.cfg.set_many(&json!({ "library_path": lib.to_string_lossy() }));
    fsx::roots::add([&lib]);
    (ts, lib)
}

/// Files left behind by a rewrite (`.encore-rewrite-*`) in `dir`.
fn leftovers(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir).unwrap().filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().to_string()).filter(|n| n.starts_with(".encore-")).collect()
}

/// The pool takes the new copy over the old name: the file is tagged at its own path, a warning says the
/// old copy stays on the read-only disk, and nothing temporary is left.
#[test]
fn a_read_only_file_is_tagged_anew_at_its_own_path() {
    let (ts, lib) = server("same");
    let song = lib.join("Coldplay/Singles/Coldplay - ♾.flac");
    if !flac(&song, "old") {
        return ts.remove();
    }
    fault::fail(&song, "write", 30);
    let got = encore_library::tagger::tag_file(&ts.st, &song, &title("♾️"), None).unwrap();
    fault::clear(&song);
    assert_eq!(got, song);
    assert_eq!(encore_media::tags::read_tags(&song.to_string_lossy()).s("title"), "♾️");
    assert!(leftovers(song.parent().unwrap()).is_empty());
    let logged = ts.st.db.query("SELECT message FROM activity WHERE level='warning'", encore_db::p![]).unwrap();
    assert!(logged.iter().any(|r| r.s("message").contains("written anew on a writable disk")), "{logged:?}");
    ts.remove();
}

/// The pool won't let the new copy take the old name: the tagged copy keeps a name of its own (the title's
/// emoji in words), the old file stays, and nothing temporary is left.
#[test]
fn a_read_only_file_that_cant_be_replaced_gets_a_name_of_its_own() {
    let (ts, lib) = server("alt");
    let song = lib.join("Coldplay/Singles/Coldplay - ♾.flac");
    if !flac(&song, "old") {
        return ts.remove();
    }
    fault::fail(&song, "write", 30);
    fault::fail(&song, "rename", 30);
    let got = encore_library::tagger::tag_file(&ts.st, &song, &title("♾️"), None).unwrap();
    fault::clear(&song);
    assert_eq!(got, lib.join("Coldplay/Singles/Coldplay - Infinity.flac"));
    assert_eq!(encore_media::tags::read_tags(&got.to_string_lossy()).s("title"), "♾️");
    assert_eq!(encore_media::tags::read_tags(&song.to_string_lossy()).s("title"), "old", "the old file stays as it was");
    assert!(leftovers(song.parent().unwrap()).is_empty());
    ts.remove();
}

/// An import that replaces a file the pool won't let go of puts the new file next to it under a name of
/// its own, instead of failing the import as a storage problem.
#[tokio::test]
async fn an_import_over_a_read_only_file_keeps_both() {
    let (ts, lib) = server("place");
    let old = lib.join("A/Song.flac");
    let new = ts.dir.join("staging/Song.flac");
    if !flac(&old, "Song") || !flac(&new, "Song") {
        return ts.remove();
    }
    fault::fail(&old, "rename", 30);
    let placed = encore_library::tagger::place_all(&ts.st, &[(new.clone(), old.clone())], false).await.unwrap();
    fault::clear(&old);
    assert_eq!(placed, [lib.join("A/Song (2).flac")]);
    assert!(old.is_file() && placed[0].is_file() && !new.exists());
    assert!(leftovers(&lib.join("A")).is_empty());
    ts.remove();
}

/// The error of a file that can't be written while its folder can names that folder and isn't a storage
/// problem; one whose folder fails its probe too stays a storage problem.
#[test]
fn narrow_pins_the_error_on_the_file_only_when_the_folder_is_fine() {
    let (ts, lib) = server("narrow");
    let dir = lib.join("Coldplay/Singles");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("Coldplay - ♾.opus");
    let erofs = |p: &Path| StepError::new(Step::Import, "write", p, None, std::io::Error::from_raw_os_error(30));
    assert!(erofs(&file).is_storage(), "before the look, EROFS in the library is storage");
    let e = fsx::narrow(erofs(&file));
    assert!(!e.is_storage());
    assert_eq!(e.only_file.as_ref().unwrap().dir, dir);
    assert_eq!(e.to_string(), format!("import failed: write {}: Read-only file system (os error 30) – {} can be written, this file can't: it is on a \
        read-only disk of the pool, or the disk refuses its name – not the source, and not the whole storage", file.display(), dir.display()));
    assert_eq!(e.json()["only_file"]["dir"], json!(dir.to_string_lossy()));
    // the whole folder is read-only: still the storage's problem
    fault::fail(&dir, "*", 30);
    let e = fsx::narrow(erofs(&file));
    fault::clear(&dir);
    assert!(e.is_storage() && e.only_file.is_none(), "{e}");
    // another errno (a permission) is not looked at again
    let eacces = fsx::narrow(StepError::new(Step::Import, "write", &file, None, std::io::Error::from_raw_os_error(13)));
    assert!(eacces.only_file.is_none());
    ts.remove();
}

/// The `file_names` setting: emoji titles in words make distinct, stable names; `keep` leaves them.
#[test]
fn file_names_setting_writes_emoji_as_words() {
    let (ts, lib) = server("names");
    let mut v = Row::new();
    for (k, x) in [("artist", "Coldplay"), ("albumartist", "Coldplay"), ("album", "Music of the Spheres"), ("year", "2021"), ("title", "♾️"), ("track", "11")] {
        v.insert(k.into(), json!(x));
    }
    let path = |st: &crate::state::AppState, v: &Row| encore_library::tagger::render_path(st, v, ".flac", false);
    assert_eq!(path(&ts.st, &v), lib.join("Coldplay/Music of the Spheres (2021)/11 - ♾️.flac"));
    ts.st.cfg.set_many(&json!({ "file_names": "words" }));
    assert_eq!(path(&ts.st, &v), lib.join("Coldplay/Music of the Spheres (2021)/11 - Infinity.flac"));
    v.insert("title".into(), json!("❤️"));
    assert_eq!(path(&ts.st, &v), lib.join("Coldplay/Music of the Spheres (2021)/11 - Red Heart.flac"));
    ts.remove();
}

/// A new file whose name its disk refuses (EILSEQ: an emoji on exFAT) goes in with the name in words.
#[tokio::test]
async fn a_refused_name_is_written_in_words() {
    let (ts, lib) = server("refused");
    let src = ts.dir.join("staging/x.flac");
    if !flac(&src, "♾️") {
        return ts.remove();
    }
    let dest = lib.join("Coldplay/Singles/Coldplay - ♾.flac");
    let part = lib.join("Coldplay/Singles/Coldplay - ♾.flac.encore-part");
    fault::fail(&part, "*", 84);
    let placed = encore_library::tagger::place_all(&ts.st, &[(src.clone(), dest.clone())], false).await.unwrap();
    fault::clear(&part);
    assert_eq!(placed, [lib.join("Coldplay/Singles/Coldplay - Infinity.flac")]);
    assert!(placed[0].is_file() && !dest.exists() && !src.exists());
    assert!(leftovers(&lib.join("Coldplay/Singles")).is_empty());
    ts.remove();
}
