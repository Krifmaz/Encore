//! Songs in the listener's own folders on this computer ("On this device"): the folders are picked
//! on the page (`config.local_folders`), scanned on a thread of their own, and their tags read here
//! with lofty (the reader the server uses). Nothing is uploaded: only the tags go to the server, to
//! `POST /api/library/match-local`, which answers with the library song each file already is.
//!
//! - A file the library has is that library song: wherever the app plays the song, it plays the file
//!   ([`file_for`] in `tracks::from_encore`), and the play counts and scrobbles as the song's.
//! - The rest are listed on the page and play from the file like an offline song.
//!
//! The scan keeps what it read in `<data dir>/encore-desktop/local/index.json` and reads a file's
//! tags again only when its size or time changed. The server's answers are kept per server.

use crate::engine::Track;
use cadenza::decode::Source;
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::tag::{Accessor, ItemKey};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

/// The audio files a scan reads (lower case extensions).
pub const AUDIO: &[&str] = &[
    "flac", "mp3", "m4a", "aac", "alac", "ogg", "oga", "opus", "wav", "aif", "aiff", "wv", "ape",
    "dsf", "dff", "mpc", "wma",
];

/// Most songs one match call sends (the server takes up to 2000).
const MATCH_CHUNK: usize = 500;

/// One song in a local folder.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct LocalSong {
    /// The file, absolute.
    pub path: String,
    pub size: u64,
    /// The file's modified time (Unix seconds).
    pub mtime: u64,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub albumartist: String,
    pub track: u32,
    pub disc: u32,
    pub year: String,
    /// Seconds (0 = unknown).
    pub duration: f64,
    /// The codec, lower case (`flac`, `mp3` …).
    pub format: String,
    /// The label the player shows: `FLAC 24/96`, `MP3 320`.
    pub quality: String,
    pub isrc: String,
    /// The MusicBrainz recording id.
    pub mbid: String,
    /// The cover: a picture next to the file or the one in its tags, saved once per folder
    /// ("" = none).
    pub art: String,
}

impl LocalSong {
    /// "Artist · Album" (or what of it is known).
    pub fn sub(&self) -> String {
        match (self.artist.is_empty(), self.album.is_empty()) {
            (false, false) => format!("{} · {}", self.artist, self.album),
            (false, true) => self.artist.clone(),
            (true, false) => self.album.clone(),
            (true, true) => String::new(),
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Store {
    songs: Vec<LocalSong>,
    /// Per server ("http://host:port"): file path → the library song's row.
    matches: BTreeMap<String, BTreeMap<String, Value>>,
    /// The songs' revision each server's answers belong to.
    matched_at: BTreeMap<String, u64>,
    /// Grows with each scan that changed the songs.
    songs_rev: u64,
    /// `matches` the other way round, per server: song id → file (built when first asked).
    #[serde(skip)]
    ids: HashMap<String, HashMap<i64, String>>,
}

static STORE: Mutex<Option<Store>> = Mutex::new(None);
/// Grows with every change (the page reloads when it moved).
static REVISION: AtomicU64 = AtomicU64::new(0);
static SCANNING: AtomicBool = AtomicBool::new(false);
static MATCHING: AtomicBool = AtomicBool::new(false);
/// Files read so far by the running scan.
static SCANNED: AtomicU64 = AtomicU64::new(0);

fn dir() -> PathBuf {
    crate::config::data_dir().join("local")
}

fn index_path() -> PathBuf {
    dir().join("index.json")
}

fn with<T>(f: impl FnOnce(&mut Store) -> T) -> T {
    let mut g = STORE.lock().unwrap_or_else(|e| e.into_inner());
    let s = g.get_or_insert_with(|| {
        std::fs::read(index_path())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    });
    f(s)
}

fn persist(s: &Store) {
    let p = index_path();
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    match serde_json::to_vec(s) {
        Ok(b) => {
            let tmp = p.with_extension("json.part");
            if std::fs::write(&tmp, b).is_ok() {
                let _ = std::fs::rename(&tmp, &p);
            }
        }
        Err(e) => log::warn!("local folders: {e}"),
    }
}

pub fn revision() -> u64 {
    REVISION.load(Ordering::SeqCst)
}

/// The songs found, by artist, album, disc and track.
pub fn list() -> Vec<LocalSong> {
    with(|s| s.songs.clone())
}

/// This server's library rows for the files it has: path → row.
pub fn matches(base: &str) -> BTreeMap<String, Value> {
    with(|s| s.matches.get(base).cloned().unwrap_or_default())
}

/// The file of this library song on this computer, when a local folder has it.
pub fn file_for(base: &str, id: i64) -> Option<PathBuf> {
    with(|s| {
        if !s.ids.contains_key(base) {
            let m = s.matches.get(base)?;
            let ids = m
                .iter()
                .filter_map(|(path, row)| Some((crate::engine::tracks::id_of(row)?, path.clone())))
                .collect();
            s.ids.insert(base.to_string(), ids);
        }
        s.ids.get(base)?.get(&id).map(PathBuf::from)
    })
    .filter(|p| p.is_file())
}

/// A file a local folder holds that is a library song (its plays count as the song's).
pub fn is_library_file(path: &Path) -> bool {
    let key = path.to_string_lossy();
    with(|s| s.matches.values().any(|m| m.contains_key(key.as_ref())))
}

/// Files read by the scan now running (None when none runs).
pub fn scanning() -> Option<u64> {
    SCANNING
        .load(Ordering::SeqCst)
        .then(|| SCANNED.load(Ordering::SeqCst))
}

pub fn matching() -> bool {
    MATCHING.load(Ordering::SeqCst)
}

/// Scan these folders on a thread (nothing when a scan runs already). Songs in folders no longer
/// listed are dropped.
pub fn scan(folders: Vec<String>) {
    if SCANNING.swap(true, Ordering::SeqCst) {
        return;
    }
    SCANNED.store(0, Ordering::SeqCst);
    REVISION.fetch_add(1, Ordering::SeqCst);
    let spawned = std::thread::Builder::new()
        .name("encore-local-scan".into())
        .spawn(move || {
            let before: HashMap<String, LocalSong> =
                with(|s| s.songs.iter().map(|x| (x.path.clone(), x.clone())).collect());
            let songs = scan_now(&folders, &before);
            with(|s| {
                if s.songs != songs {
                    s.songs = songs;
                    s.songs_rev += 1;
                    // answers about files that are gone go with them
                    let keep: std::collections::HashSet<&str> =
                        s.songs.iter().map(|x| x.path.as_str()).collect();
                    for m in s.matches.values_mut() {
                        m.retain(|k, _| keep.contains(k.as_str()));
                    }
                    s.ids.clear();
                }
                persist(s);
            });
            SCANNING.store(false, Ordering::SeqCst);
            REVISION.fetch_add(1, Ordering::SeqCst);
        });
    if spawned.is_err() {
        SCANNING.store(false, Ordering::SeqCst);
    }
}

fn scan_now(folders: &[String], before: &HashMap<String, LocalSong>) -> Vec<LocalSong> {
    let mut files = Vec::new();
    for f in folders {
        walk(Path::new(f), &mut files, 0);
    }
    files.sort();
    files.dedup();
    let mut covers: HashMap<PathBuf, String> = HashMap::new();
    let mut out = Vec::with_capacity(files.len());
    for path in files {
        SCANNED.fetch_add(1, Ordering::SeqCst);
        let Ok(meta) = std::fs::metadata(&path) else { continue };
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let key = path.to_string_lossy().into_owned();
        let song = match before.get(&key) {
            Some(old) if old.size == meta.len() && old.mtime == mtime => old.clone(),
            _ => {
                let mut s = read_tags(&path);
                s.path = key;
                s.size = meta.len();
                s.mtime = mtime;
                s
            }
        };
        let mut song = song;
        if song.art.is_empty() || !Path::new(&song.art).is_file() {
            song.art = cover_for(&path, &mut covers);
        }
        out.push(song);
    }
    out.sort_by(|a, b| {
        let k = |s: &LocalSong| {
            let artist = if s.albumartist.is_empty() { &s.artist } else { &s.albumartist };
            (artist.to_lowercase(), s.album.to_lowercase(), s.disc, s.track, s.title.to_lowercase())
        };
        k(a).cmp(&k(b))
    });
    out
}

/// The audio files under `dir` (links are not followed, hidden folders skipped).
fn walk(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) {
    if depth > 32 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let Ok(ft) = e.file_type() else { continue };
        let p = e.path();
        let hidden = e.file_name().to_string_lossy().starts_with('.');
        if ft.is_dir() && !hidden {
            walk(&p, out, depth + 1);
        } else if ft.is_file() && !hidden && is_audio(&p) {
            out.push(p);
        }
    }
}

pub fn is_audio(p: &Path) -> bool {
    p.extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .is_some_and(|e| AUDIO.contains(&e.as_str()))
}

/// The tags of one file; the file name stands in for a missing title.
pub fn read_tags(path: &Path) -> LocalSong {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let mut s = LocalSong {
        format: codec_of(&ext).into(),
        ..LocalSong::default()
    };
    match lofty::read_from_path(path) {
        Ok(f) => {
            let props = f.properties();
            s.duration = props.duration().as_secs_f64();
            if let Some(tag) = f.primary_tag().or_else(|| f.first_tag()) {
                let txt = |v: Option<std::borrow::Cow<'_, str>>| {
                    v.map(|x| x.trim().to_string()).unwrap_or_default()
                };
                s.title = txt(tag.title());
                s.artist = txt(tag.artist());
                s.album = txt(tag.album());
                s.track = tag.track().unwrap_or(0);
                s.disc = tag.disk().unwrap_or(0);
                s.year = tag.date().map(|d| d.year.to_string()).unwrap_or_default();
                let item = |k: ItemKey| tag.get_string(k).map(|x| x.trim().to_string()).unwrap_or_default();
                s.albumartist = item(ItemKey::AlbumArtist);
                s.isrc = item(ItemKey::Isrc);
                s.mbid = item(ItemKey::MusicBrainzRecordingId);
            }
            if ext == "m4a" && props.bit_depth().is_some() {
                s.format = "alac".into();
            }
            s.quality = quality(&s.format, props.bit_depth(), props.sample_rate(), props.audio_bitrate());
        }
        Err(e) => {
            log::debug!("local folders: no tags in {}: {e}", path.display());
            s.quality = s.format.to_ascii_uppercase();
        }
    }
    if s.title.is_empty() {
        s.title = path
            .file_stem()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
    }
    if s.artist.is_empty() {
        s.artist = s.albumartist.clone();
    }
    s
}

fn codec_of(ext: &str) -> &'static str {
    match ext {
        "flac" => "flac",
        "mp3" => "mp3",
        "m4a" | "aac" => "aac",
        "alac" => "alac",
        "ogg" | "oga" => "vorbis",
        "opus" => "opus",
        "wav" => "wav",
        "aif" | "aiff" => "aiff",
        "wv" => "wv",
        "ape" => "ape",
        "dsf" => "dsf",
        "dff" => "dff",
        "mpc" => "mpc",
        "wma" => "wma",
        _ => "",
    }
}

/// The label the player shows: lossless with its depth and rate ("FLAC 24/96", "FLAC 16/44.1"),
/// lossy with its bitrate ("MP3 320").
pub fn quality(format: &str, bits: Option<u8>, rate: Option<u32>, kbps: Option<u32>) -> String {
    let f = format.to_ascii_uppercase();
    let lossless = matches!(f.as_str(), "FLAC" | "ALAC" | "WAV" | "AIFF" | "WV" | "APE");
    match (lossless, bits, rate, kbps) {
        (true, Some(b), Some(r), _) if b > 0 && r > 0 => {
            let khz = r as f64 / 1000.0;
            let khz = if khz.fract() == 0.0 { format!("{khz:.0}") } else { format!("{khz:.1}") };
            format!("{f} {b}/{khz}")
        }
        (false, _, _, Some(k)) if k > 0 && !f.is_empty() => format!("{f} {k}"),
        _ => f,
    }
}

/// A cover for the songs of this file's folder: `cover.jpg`, `folder.jpg` … next to it, or the
/// picture in the file's tags saved once under `<data dir>/local/art/`.
fn cover_for(file: &Path, seen: &mut HashMap<PathBuf, String>) -> String {
    let Some(folder) = file.parent() else { return String::new() };
    if let Some(c) = seen.get(folder).filter(|c| !c.is_empty()) {
        return c.clone();
    }
    let found = next_to(folder).or_else(|| embedded(file, folder)).unwrap_or_default();
    seen.insert(folder.to_path_buf(), found.clone());
    found
}

fn next_to(folder: &Path) -> Option<String> {
    for name in ["cover", "folder", "front", "album", "Cover", "Folder", "Front", "AlbumArt"] {
        for ext in ["jpg", "jpeg", "png", "webp"] {
            let p = folder.join(format!("{name}.{ext}"));
            if p.is_file() {
                return Some(p.to_string_lossy().into_owned());
            }
        }
    }
    None
}

fn embedded(file: &Path, folder: &Path) -> Option<String> {
    let f = lofty::read_from_path(file).ok()?;
    let tag = f.primary_tag().or_else(|| f.first_tag())?;
    let pic = tag.pictures().first()?;
    let ext = match pic.mime_type().map(|m| m.as_str()) {
        Some("image/png") => "png",
        Some("image/webp") => "webp",
        _ => "jpg",
    };
    use sha2::Digest;
    let name = format!("{:x}", sha2::Sha256::digest(folder.to_string_lossy().as_bytes()));
    let out = dir().join("art").join(format!("{}.{ext}", &name[..32]));
    std::fs::create_dir_all(out.parent()?).ok()?;
    std::fs::write(&out, pic.data()).ok()?;
    Some(out.to_string_lossy().into_owned())
}

/// The server's answers for this server are older than the songs.
pub fn needs_match(base: &str) -> bool {
    !base.is_empty()
        && !matching()
        && scanning().is_none()
        && with(|s| !s.songs.is_empty() && s.matched_at.get(base) != Some(&s.songs_rev))
}

/// Ask this server which of the songs its library has, and keep the answers.
pub async fn match_with(api: std::sync::Arc<crate::api::Api>) -> Result<usize, String> {
    if MATCHING.swap(true, Ordering::SeqCst) {
        return Ok(0);
    }
    let base = api.base();
    let (songs, rev) = with(|s| (s.songs.clone(), s.songs_rev));
    let mut found = BTreeMap::new();
    let mut result = Ok(());
    for part in songs.chunks(MATCH_CHUNK) {
        let body = json!({ "songs": part.iter().map(match_body).collect::<Vec<_>>() });
        match api.post("/library/match-local", body).await {
            Ok(v) => {
                if let Some(m) = v.get("matches").and_then(Value::as_object) {
                    found.extend(m.iter().map(|(k, r)| (k.clone(), r.clone())));
                }
            }
            Err(e) => {
                result = Err(e.message);
                break;
            }
        }
    }
    MATCHING.store(false, Ordering::SeqCst);
    result?;
    let n = found.len();
    with(|s| {
        s.matches.insert(base.clone(), found);
        s.ids.remove(&base);
        s.matched_at.insert(base, rev);
        persist(s);
    });
    REVISION.fetch_add(1, Ordering::SeqCst);
    Ok(n)
}

fn match_body(s: &LocalSong) -> Value {
    json!({
        "key": s.path,
        "title": s.title,
        "artist": s.artist,
        "duration": (s.duration > 0.0).then_some(s.duration),
        "isrc": (!s.isrc.is_empty()).then_some(&s.isrc),
        "mbid": (!s.mbid.is_empty()).then_some(&s.mbid),
    })
}

/// A local song as a queue entry: the library song when this server has it (its id, so plays,
/// likes and the Signal path are the song's), otherwise the file alone.
pub fn track(s: &LocalSong, row: Option<&Value>) -> Track {
    let cover = (!s.art.is_empty()).then(|| s.art.clone());
    let source = Source::Path { path: PathBuf::from(&s.path) };
    match row {
        Some(r) => {
            let mut v = r.clone();
            v["art"] = json!(s.art);
            v["format"] = json!(s.format);
            let mut t = crate::engine::tracks::from_file(PathBuf::from(&s.path), &v);
            t.format = Some(s.quality.clone()).filter(|q| !q.is_empty());
            t.cover = cover;
            t
        }
        None => Track {
            id: None,
            source,
            title: s.title.clone(),
            artist: s.artist.clone(),
            album: s.album.clone(),
            album_id: None,
            duration: (s.duration > 0.0).then_some(s.duration),
            format: Some(s.quality.clone()).filter(|q| !q.is_empty()),
            cover,
        },
    }
}

/// Play these songs from `index`, each as the library song when this server has it.
pub fn play(base: &str, songs: &[LocalSong], index: usize) -> Result<(), String> {
    let m = matches(base);
    let list: Vec<Track> = songs.iter().map(|s| track(s, m.get(&s.path))).collect();
    crate::engine::play(list, index, 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quality_labels_read_like_the_servers() {
        assert_eq!(quality("flac", Some(24), Some(96000), Some(3000)), "FLAC 24/96");
        assert_eq!(quality("flac", Some(16), Some(44100), None), "FLAC 16/44.1");
        assert_eq!(quality("mp3", None, Some(44100), Some(320)), "MP3 320");
        assert_eq!(quality("flac", None, None, None), "FLAC");
        assert_eq!(quality("", None, None, Some(128)), "");
    }

    #[test]
    fn only_audio_files_are_scanned() {
        assert!(is_audio(Path::new("/m/a/01 Song.FLAC")));
        assert!(is_audio(Path::new("x.opus")));
        assert!(!is_audio(Path::new("cover.jpg")));
        assert!(!is_audio(Path::new("notes")));
    }

    #[test]
    fn a_file_without_tags_is_named_by_its_file() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("Some Song.mp3");
        std::fs::write(&p, b"not audio").unwrap();
        let s = read_tags(&p);
        assert_eq!(s.title, "Some Song");
        assert_eq!(s.format, "mp3");
        assert_eq!(s.quality, "MP3");
    }

    #[test]
    fn the_walk_finds_audio_in_subfolders_and_skips_hidden_ones() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("A/B")).unwrap();
        std::fs::create_dir_all(d.path().join(".hidden")).unwrap();
        std::fs::write(d.path().join("A/B/1.flac"), b"").unwrap();
        std::fs::write(d.path().join("A/cover.jpg"), b"").unwrap();
        std::fs::write(d.path().join(".hidden/2.mp3"), b"").unwrap();
        std::fs::write(d.path().join("3.m4a"), b"").unwrap();
        let mut out = Vec::new();
        walk(d.path(), &mut out, 0);
        out.sort();
        assert_eq!(out, vec![d.path().join("3.m4a"), d.path().join("A/B/1.flac")]);
    }

    #[test]
    fn the_match_body_leaves_out_what_is_unknown() {
        let s = LocalSong {
            path: "/m/x.flac".into(),
            title: "T".into(),
            artist: "A".into(),
            ..LocalSong::default()
        };
        let b = match_body(&s);
        assert_eq!(b["key"], "/m/x.flac");
        assert!(b["duration"].is_null() && b["isrc"].is_null() && b["mbid"].is_null());
    }

    #[test]
    fn an_unmatched_song_plays_as_a_file_and_a_matched_one_as_the_library_song() {
        let s = LocalSong {
            path: "/m/x.flac".into(),
            title: "T".into(),
            artist: "A".into(),
            quality: "FLAC 16/44.1".into(),
            duration: 100.0,
            ..LocalSong::default()
        };
        let t = track(&s, None);
        assert!(t.id.is_none());
        assert!(matches!(&t.source, Source::Path { path } if path == Path::new("/m/x.flac")));
        let row = json!({ "id": 7, "title": "Library title", "artist": "A", "album": "Al", "album_id": "a1" });
        let t = track(&s, Some(&row));
        assert_eq!(t.id.as_deref(), Some("7"));
        assert_eq!(t.title, "Library title");
        assert_eq!(t.format.as_deref(), Some("FLAC 16/44.1"));
    }
}
