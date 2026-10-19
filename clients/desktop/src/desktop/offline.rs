//! Songs saved on this computer, to play without the server: the audio as the server streams it
//! to Cadenza (the original file when the engine decodes it) and the album cover, in
//! `<data dir>/encore-desktop/offline/`, listed in `index.json` as the one offline record every
//! Encore client writes (docs/contracts/offline-manifest.md: id, title, artist, album, album_id,
//! ext, bytes, quality, cover, lyrics, saved_at, source_server, duration, track, disc, format,
//! artists) plus the file-based fields `file` and `art`, file names in that folder. The index of
//! the classic app and of 0.27 is upgraded when it is read. The songs play through the engine
//! like any song (`Source::Path`).
//!
//! "Save for offline" on a song, an album or a playlist calls [`save`]: the songs are fetched one
//! after another on a thread of their own; the Offline music page shows the progress and the app's
//! tick turns the outcome into a toast ("Saved for offline: <title>", "Couldn't save: <why>").

use crate::engine::tracks::{self, Auth};
use crate::engine::Track;
use crate::fmt;
use crate::pages::Ctx;
use crate::widgets::ToastKind;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Mutex;
use std::time::Duration;

/// One saved song (docs/contracts/offline-manifest.md).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Song {
    /// The library id on its server, as digits; written as a number.
    #[serde(serialize_with = "id_as_number", deserialize_with = "id_from_any")]
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    /// The audio file's extension (`flac`, `mp3` …).
    pub ext: String,
    /// The audio file's name in the offline folder.
    pub file: String,
    /// The cover's file name in the offline folder ("" = none).
    pub art: String,
    pub album_id: String,
    /// The audio file's size.
    pub bytes: u64,
    /// The label the player shows: `FLAC 16/44.1`, `MP3 320k`, `FLAC (converted)` for a copy the
    /// server converted.
    pub quality: String,
    /// The cover was saved (`art` names a file that exists).
    pub cover: bool,
    /// The lyrics were saved (this app does not save them yet).
    pub lyrics: bool,
    /// When it was saved (Unix seconds; written as a whole number).
    #[serde(serialize_with = "whole_seconds")]
    pub saved_at: f64,
    /// The server it came from ("http://host:port").
    pub source_server: String,
    /// Seconds (0 = unknown).
    pub duration: f64,
    /// The track and disc numbers (0 = unknown).
    pub track: u32,
    pub disc: u32,
    /// The codec of the saved bytes, lower case (`flac`, `mp3`, `aac` …).
    pub format: String,
    /// The song's `artists` as the server sent them (empty: use `artist`).
    pub artists: Vec<Value>,
}

impl Song {
    /// A copy the server converted, not the original file: "Download again in original quality"
    /// applies to it. Only the tests call it so far (the offline page has no such button yet).
    #[allow(dead_code)]
    pub fn converted(&self) -> bool {
        self.quality.trim_end().ends_with("(converted)")
    }
}

fn id_as_number<S: serde::Serializer>(id: &str, s: S) -> Result<S::Ok, S::Error> {
    match id.parse::<u64>() {
        Ok(n) => s.serialize_u64(n),
        Err(_) => s.serialize_str(id),
    }
}

fn id_from_any<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Ok(id_text(&Value::deserialize(d)?).unwrap_or_default())
}

fn whole_seconds<S: serde::Serializer>(t: &f64, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_i64(if t.is_finite() && *t > 0.0 { t.floor() as i64 } else { 0 })
}

/// A track id as digits: a whole number, or a string of digits. Anything else is no id.
fn id_text(v: &Value) -> Option<String> {
    match v {
        Value::Number(n) => n
            .as_u64()
            .or_else(|| n.as_f64().filter(|f| *f >= 0.0 && f.fract() == 0.0).map(|f| f as u64))
            .map(|n| n.to_string()),
        Value::String(s) => {
            let t = s.trim();
            if t.is_empty() || !t.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            t.parse::<u64>().ok().map(|n| n.to_string())
        }
        _ => None,
    }
}

/// `<data dir>/encore-desktop/offline`.
pub fn dir() -> PathBuf {
    crate::config::data_dir().join("offline")
}

fn index_path() -> PathBuf {
    dir().join("index.json")
}

/// Index writes, one at a time (the save thread and the page's removals).
static INDEX: Mutex<()> = Mutex::new(());
/// Grows with every change of the index (the page reloads when it moved).
static REVISION: AtomicU64 = AtomicU64::new(0);

/// The saved songs, oldest first.
pub fn list() -> Vec<Song> {
    read_index(&index_path())
}

fn read_index(p: &Path) -> Vec<Song> {
    let Some(v) = std::fs::read(p)
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
    else {
        return Vec::new();
    };
    parse_index(v, p.parent().unwrap_or(Path::new("")))
}

/// The songs of an index document, read leniently (docs/contracts/offline-manifest.md): the plain
/// array, or an object holding it under `songs`, `tracks` or `records`; a record without a usable
/// id is skipped, one that appears twice keeps its last entry, and the older shapes are upgraded
/// ([`upgrade`]). `folder` holds the files.
pub fn parse_index(v: Value, folder: &Path) -> Vec<Song> {
    let list = match v {
        Value::Array(a) => a,
        Value::Object(mut o) => ["songs", "tracks", "records"]
            .iter()
            .find_map(|k| match o.remove(*k) {
                Some(Value::Array(a)) => Some(a),
                _ => None,
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    let mut out: Vec<Song> = Vec::new();
    for r in &list {
        let Some(s) = upgrade(r, folder) else { continue };
        out.retain(|o| !same(o, &s));
        out.push(s);
    }
    out
}

/// A file name in the offline folder from what an older app wrote: an asset URL
/// (`asset://…`, `https://asset.localhost/…`) or a path becomes its last segment, decoded.
fn file_name(s: &str) -> String {
    let s = s.trim();
    let s = s.split(['?', '#']).next().unwrap_or("");
    let last = s.rsplit(['/', '\\']).next().unwrap_or("");
    let name = percent_encoding::percent_decode_str(last).decode_utf8_lossy().into_owned();
    if name == "." || name == ".." || name.contains(['/', '\\']) {
        String::new()
    } else {
        name
    }
}

/// One record as any Encore client wrote it, in this release's shape: numbers written as strings
/// read as numbers, `file` and `art` cut to file names, and the fields older apps did not write
/// take their defaults (`ext` from the file, `quality` from `ext`, `format` from `ext`, `cover`
/// from the cover file, `bytes` and `saved_at` from the audio file). None without a usable id.
fn upgrade(v: &Value, folder: &Path) -> Option<Song> {
    let o = v.as_object()?;
    let id = id_text(o.get("id")?)?;
    let text = |k: &str| fmt::s(v, k).trim().to_string();
    let num = |k: &str| match o.get(k) {
        Some(Value::Number(n)) => n.as_f64().filter(|f| f.is_finite()).unwrap_or(0.0),
        Some(Value::String(s)) => s.trim().parse::<f64>().ok().filter(|f| f.is_finite()).unwrap_or(0.0),
        _ => 0.0,
    };
    let file = file_name(&text("file"));
    let art = file_name(&text("art"));
    let audio = (!file.is_empty()).then(|| folder.join(&file));
    let meta = audio.as_ref().and_then(|p| std::fs::metadata(p).ok());
    let ext = {
        let e = clean(&text("ext").to_ascii_lowercase());
        let from_file = Path::new(&file)
            .extension()
            .map(|x| clean(&x.to_string_lossy().to_ascii_lowercase()))
            .unwrap_or_default();
        let from_format = clean(&text("format").to_ascii_lowercase());
        [e, from_file, from_format]
            .into_iter()
            .find(|x| !x.is_empty())
            .unwrap_or_else(|| "audio".into())
    };
    let known_ext = ext != "audio";
    let quality = match o.get("quality") {
        Some(Value::String(q)) => q.trim().to_string(),
        _ if known_ext => ext.to_ascii_uppercase(),
        _ => String::new(),
    };
    let format = match text("format").to_ascii_lowercase() {
        f if !f.is_empty() => f,
        _ if known_ext => ext.clone(),
        _ => String::new(),
    };
    let bytes = match num("bytes") {
        b if b > 0.0 => b as u64,
        _ => meta.as_ref().map(|m| m.len()).unwrap_or(0),
    };
    let saved_at = match num("saved_at") {
        t if t > 0.0 => t,
        _ => meta
            .as_ref()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as f64)
            .unwrap_or(0.0),
    };
    let cover = !art.is_empty() && folder.join(&art).is_file();
    Some(Song {
        id,
        title: text("title"),
        artist: text("artist"),
        album: text("album"),
        ext,
        file,
        art,
        album_id: clean(&text("album_id")),
        bytes,
        quality,
        cover,
        lyrics: o.get("lyrics").and_then(Value::as_bool).unwrap_or(false),
        saved_at,
        source_server: text("source_server").trim_end_matches('/').to_string(),
        duration: num("duration").max(0.0),
        track: num("track").max(0.0) as u32,
        disc: num("disc").max(0.0) as u32,
        format,
        artists: o
            .get("artists")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
    })
}

fn write_index(p: &Path, songs: &[Song]) -> Result<(), String> {
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    let tmp = p.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(songs).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, p).map_err(|e| e.to_string())?;
    REVISION.fetch_add(1, Ordering::SeqCst);
    Ok(())
}

/// A number that changes whenever the saved songs change.
pub fn revision() -> u64 {
    REVISION.load(Ordering::SeqCst)
}

/// How many songs are saved.
pub fn count() -> usize {
    list().len()
}

/// The space the songs take.
pub fn total_bytes(songs: &[Song]) -> u64 {
    songs.iter().map(|s| s.bytes).sum()
}

/// A short tag of the server, so the same id from two servers gets two files.
pub fn server_tag(server: &str) -> String {
    format!("{:08x}", crate::link::fnv1a(server) as u32)
}


/// The audio file of a saved song.
pub fn audio_path(song: &Song) -> PathBuf {
    dir().join(&song.file)
}

/// The cover file of a saved song, when it has one.
pub fn art_path(song: &Song) -> Option<PathBuf> {
    (!song.art.is_empty())
        .then(|| dir().join(&song.art))
        .filter(|p| p.is_file())
}

fn same(a: &Song, b: &Song) -> bool {
    a.id == b.id && a.source_server == b.source_server
}

/// Delete a saved song (its audio, and the cover when no other song uses it).
pub fn remove(song: &Song) -> Result<(), String> {
    let _g = INDEX.lock().unwrap_or_else(|e| e.into_inner());
    let mut songs = list();
    songs.retain(|s| !same(s, song));
    write_index(&index_path(), &songs)?;
    delete_files(song, &songs);
    Ok(())
}

/// Delete every saved song.
pub fn remove_all() -> Result<(), String> {
    let _g = INDEX.lock().unwrap_or_else(|e| e.into_inner());
    let songs = list();
    write_index(&index_path(), &[])?;
    for s in &songs {
        delete_files(s, &[]);
    }
    Ok(())
}

fn delete_files(song: &Song, rest: &[Song]) {
    if !song.file.is_empty() {
        let _ = std::fs::remove_file(audio_path(song));
    }
    if !song.art.is_empty() && !rest.iter().any(|s| s.art == song.art) {
        let _ = std::fs::remove_file(dir().join(&song.art));
    }
}

/// A saved song as an engine queue entry (it plays from disk).
pub fn track(song: &Song) -> Track {
    let art = art_path(song).map(|p| p.to_string_lossy().into_owned());
    tracks::from_file(
        audio_path(song),
        &json!({
            "id": song.id,
            "title": song.title,
            "artist": song.artist,
            "album": song.album,
            "album_id": (!song.album_id.is_empty()).then_some(&song.album_id),
            "duration": (song.duration > 0.0).then_some(song.duration),
            "format": (!song.quality.is_empty()).then_some(&song.quality),
            "art": art,
        }),
    )
}

/// Play saved songs from `index` through the engine.
pub fn play(songs: &[Song], index: usize) -> Result<(), String> {
    let list: Vec<Track> = songs.iter().map(track).collect();
    crate::engine::play(list, index, 0.0)
}

// ---- saving -------------------------------------------------------------------------------------

struct Job {
    base: String,
    auth: Auth,
    row: Value,
}

/// The saves of the current batch.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Progress {
    pub total: usize,
    pub done: usize,
    pub saved: usize,
    pub failed: usize,
    /// The song being fetched.
    pub current: String,
    last_title: String,
    first_error: String,
}

static QUEUE: Mutex<Option<Sender<Job>>> = Mutex::new(None);
static PROGRESS: Mutex<Progress> = Mutex::new(Progress {
    total: 0,
    done: 0,
    saved: 0,
    failed: 0,
    current: String::new(),
    last_title: String::new(),
    first_error: String::new(),
});
static NOTICES: Mutex<Vec<(String, ToastKind)>> = Mutex::new(Vec::new());

/// The batch being saved, while one is.
pub fn progress() -> Option<Progress> {
    let p = PROGRESS.lock().unwrap_or_else(|e| e.into_inner()).clone();
    (p.total > p.done).then_some(p)
}

/// The outcomes to show as toasts (taken once).
pub fn take_notices() -> Vec<(String, ToastKind)> {
    std::mem::take(&mut *NOTICES.lock().unwrap_or_else(|e| e.into_inner()))
}

/// Save these library rows (as the server sends them) on this computer, with the toast the web
/// app shows ("Saving to this device…"). Returns how many songs were queued.
pub fn save(ctx: &mut Ctx, rows: Vec<Value>) -> usize {
    let auth = crate::link::auth(ctx.config, &ctx.api);
    let n = queue(ctx.base(), auth, rows);
    match n {
        0 => ctx.toast_err("Nothing to save"),
        1 => ctx.toast_ok("Saving to this device…"),
        n => ctx.toast_ok(format!("Saving {n} song(s) to this device…")),
    }
    n
}

/// Queue rows for saving (rows without a library id are skipped).
pub fn queue(base: String, auth: Auth, rows: Vec<Value>) -> usize {
    let rows: Vec<Value> = rows
        .into_iter()
        .filter(|r| tracks::id_of(r).is_some())
        .collect();
    if rows.is_empty() || base.is_empty() {
        return 0;
    }
    let mut q = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
    if q.is_none() {
        let (tx, rx) = channel();
        if std::thread::Builder::new()
            .name("encore-offline".into())
            .spawn(move || worker(rx))
            .is_err()
        {
            return 0;
        }
        *q = Some(tx);
    }
    let n = rows.len();
    PROGRESS.lock().unwrap_or_else(|e| e.into_inner()).total += n;
    if let Some(tx) = q.as_ref() {
        for row in rows {
            let _ = tx.send(Job {
                base: base.clone(),
                auth: auth.clone(),
                row,
            });
        }
    }
    n
}

enum Saved {
    New,
    Already,
}

fn worker(rx: Receiver<Job>) {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            log::warn!("offline saves cannot run: {e}");
            return;
        }
    };
    let client = match reqwest::Client::builder()
        .user_agent(crate::api::user_agent())
        .connect_timeout(Duration::from_secs(10))
        .read_timeout(Duration::from_secs(60))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            log::warn!("offline saves cannot run: {e}");
            return;
        }
    };
    clean_parts(&dir());
    while let Ok(job) = rx.recv() {
        let title = fmt::s(&job.row, "title");
        PROGRESS.lock().unwrap_or_else(|e| e.into_inner()).current = title.clone();
        let r = rt.block_on(save_one(&client, &job));
        if let Err(e) = &r {
            log::warn!("offline save of {title:?} failed: {e}");
        }
        let mut p = PROGRESS.lock().unwrap_or_else(|e| e.into_inner());
        p.done += 1;
        match r {
            Ok(_) => {
                p.saved += 1;
                p.last_title = title;
            }
            Err(e) => {
                p.failed += 1;
                if p.first_error.is_empty() {
                    p.first_error = e;
                }
            }
        }
        if p.done >= p.total {
            NOTICES
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .extend(notices(&p));
            *p = Progress::default();
        }
    }
}

/// The toasts at the end of a batch.
fn notices(p: &Progress) -> Vec<(String, ToastKind)> {
    let mut out = Vec::new();
    match p.saved {
        0 => {}
        1 => out.push((
            format!("Saved for offline: {}", p.last_title),
            ToastKind::Ok,
        )),
        n => out.push((format!("Saved for offline: {n} songs"), ToastKind::Ok)),
    }
    match p.failed {
        0 => {}
        1 => out.push((format!("Couldn't save: {}", p.first_error), ToastKind::Err)),
        n => out.push((
            format!("Couldn't save {n} songs: {}", p.first_error),
            ToastKind::Err,
        )),
    }
    out
}

/// Half-done downloads of an earlier run.
fn clean_parts(d: &Path) {
    if let Ok(rd) = std::fs::read_dir(d) {
        for e in rd.flatten() {
            if e.path().extension().is_some_and(|x| x == "part") {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

/// Letters, digits, `-` and `_` only (ids become file names).
fn clean(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect()
}

/// The audio file's extension: from the stream's type (`x-encore-stream` says whether it is the
/// original file or FLAC made on the fly), else the song's format, else "audio".
pub fn ext_for(label: Option<&str>, content_type: Option<&str>, format: &str) -> String {
    let by_type = content_type
        .map(|c| {
            c.split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase()
        })
        .and_then(|c| {
            Some(match c.as_str() {
                "audio/flac" | "audio/x-flac" => "flac",
                "audio/mpeg" | "audio/mp3" => "mp3",
                "audio/ogg" | "application/ogg" => "ogg",
                "audio/opus" => "opus",
                "audio/aac" | "audio/x-aac" => "aac",
                "audio/mp4" | "audio/x-m4a" | "audio/m4a" => "m4a",
                "audio/wav" | "audio/x-wav" | "audio/wave" => "wav",
                "audio/aiff" | "audio/x-aiff" => "aiff",
                "audio/x-dsf" | "audio/dsf" => "dsf",
                "audio/x-dff" | "audio/dff" => "dff",
                _ => return None,
            })
        });
    let original = matches!(label.map(str::trim), None | Some("") | Some("original"));
    if !original {
        return by_type.unwrap_or("flac").to_string();
    }
    if let Some(t) = by_type {
        return t.to_string();
    }
    let f = clean(&format.to_ascii_lowercase()).replace(['-', '_'], "");
    if f.is_empty() {
        "audio".into()
    } else {
        f
    }
}

/// A whole number field of a library row (0 when missing or not a number).
fn whole(row: &Value, key: &str) -> u32 {
    row.get(key)
        .and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok())))
        .map(|n| n.min(u32::MAX as u64) as u32)
        .unwrap_or(0)
}

/// A codec as the player writes it: upper case, except `Opus`, `Vorbis` and `WavPack`.
pub fn codec_name(codec: &str) -> String {
    match codec.to_ascii_lowercase().as_str() {
        "opus" => "Opus".into(),
        "vorbis" | "ogg vorbis" | "ogg" | "oga" => "Vorbis".into(),
        "wavpack" | "wv" => "WavPack".into(),
        "m4a" | "mp4" => "AAC".into(),
        c => c.to_uppercase(),
    }
}

/// A sample rate in kHz with one decimal at most and no trailing `.0`: 44100 → "44.1", 96000 → "96".
fn khz(hz: f64) -> String {
    let k = (hz / 100.0).round() / 10.0;
    if k.fract() == 0.0 {
        format!("{}", k as i64)
    } else {
        format!("{k}")
    }
}

/// The label of an original file (docs/contracts/offline-manifest.md): "FLAC 16/44.1",
/// "FLAC 44.1 kHz", "FLAC 24-bit", "MP3 320k", "Opus".
pub fn quality_label(codec: &str, lossless: bool, bits: i64, rate_hz: f64, kbps: f64) -> String {
    let name = codec_name(codec);
    let lossless = lossless
        || matches!(
            codec.to_ascii_lowercase().as_str(),
            "flac" | "alac" | "wav" | "aiff" | "aif" | "ape" | "wavpack" | "wv" | "tta"
        );
    if lossless {
        return match (bits > 0, rate_hz > 0.0) {
            (true, true) => format!("{name} {bits}/{}", khz(rate_hz)),
            (false, true) => format!("{name} {} kHz", khz(rate_hz)),
            (true, false) => format!("{name} {bits}-bit"),
            _ => name,
        };
    }
    if kbps > 0.0 {
        format!("{name} {}k", kbps.round() as i64)
    } else {
        name
    }
}

/// The codec of the saved bytes and the quality label. `original`: the server sent the file as it
/// is (else it converted it, and the label ends in " (converted)"); `info`: the `/info` answer
/// (Null when it was not asked).
pub fn kept(original: bool, ext: &str, row: &Value, info: &Value) -> (String, String) {
    let by_ext = |e: &str| match e {
        "m4a" | "mp4" => "aac".to_string(),
        "ogg" | "oga" => "vorbis".to_string(),
        "audio" => String::new(),
        e => e.to_string(),
    };
    if !original {
        let codec = by_ext(ext);
        let codec = if codec.is_empty() { "flac".to_string() } else { codec };
        return (codec.clone(), format!("{} (converted)", codec_name(&codec)));
    }
    let codec = match fmt::s(info, "codec").trim().to_ascii_lowercase().as_str() {
        "" => match fmt::s(row, "format").trim().to_ascii_lowercase().as_str() {
            "" => by_ext(ext),
            f => by_ext(f),
        },
        "ogg vorbis" => "vorbis".to_string(),
        c => c.to_string(),
    };
    if codec.is_empty() {
        return (String::new(), String::new());
    }
    let kbps = match fmt::f(info, "bitrate") {
        k if k > 0.0 => k,
        _ => fmt::f(row, "bitrate"),
    };
    let lossless = info.get("lossless").and_then(Value::as_bool).unwrap_or(false);
    let bits = info.get("bit_depth").and_then(Value::as_i64).unwrap_or(0);
    let quality = quality_label(&codec, lossless, bits, fmt::f(info, "sample_rate"), kbps);
    (codec, quality)
}

/// What a failed answer means, in plain words.
fn status_words(status: u16) -> String {
    match status {
        401 | 403 => "not signed in to the server".into(),
        404 => "the song is not on the server any more".into(),
        s => format!("the server answered {s}"),
    }
}

fn album_id_of(row: &Value) -> String {
    row.get("album_id")
        .and_then(|a| {
            a.as_str()
                .map(str::to_string)
                .or_else(|| a.as_i64().map(|n| n.to_string()))
        })
        .map(|a| clean(&a))
        .unwrap_or_default()
}

async fn get(
    client: &reqwest::Client,
    url: &str,
    auth: &Auth,
    decoders: bool,
) -> Result<reqwest::Response, String> {
    let mut req = client.get(url).header(auth.0.as_str(), auth.1.as_str());
    if decoders {
        req = req.header("X-Encore-Decoders", tracks::DECODERS);
    }
    let res = req.send().await.map_err(|e| {
        if e.is_connect() || e.is_timeout() {
            "the server can't be reached".to_string()
        } else {
            e.to_string()
        }
    })?;
    if !res.status().is_success() {
        return Err(status_words(res.status().as_u16()));
    }
    Ok(res)
}

async fn save_one(client: &reqwest::Client, job: &Job) -> Result<Saved, String> {
    use tokio::io::AsyncWriteExt;
    let id = tracks::id_of(&job.row).ok_or("nothing to save")?;
    let server = job.base.trim_end_matches('/').to_string();
    let tag = server_tag(&server);
    if let Some(s) = list()
        .into_iter()
        .find(|s| s.source_server == server && s.id == id.to_string())
    {
        if audio_path(&s).is_file() {
            return Ok(Saved::Already);
        }
    }
    let d = dir();
    std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;

    let mut res = get(client, &tracks::stream_url(&job.base, id), &job.auth, true).await?;
    let header = |k: &str| {
        res.headers()
            .get(k)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    };
    let label = header("x-encore-stream");
    let ctype = header("content-type");
    let ext = ext_for(
        label.as_deref(),
        ctype.as_deref(),
        &fmt::s(&job.row, "format"),
    );
    let original = matches!(
        label.as_deref().map(str::trim),
        None | Some("") | Some("original")
    );
    let stem = format!("{id}-{tag}");
    let part = d.join(format!("{stem}.part"));
    let file_name = format!("{stem}.{ext}");
    let mut out = tokio::fs::File::create(&part)
        .await
        .map_err(|e| e.to_string())?;
    let mut bytes = 0u64;
    loop {
        match res.chunk().await {
            Ok(Some(chunk)) => {
                out.write_all(&chunk).await.map_err(|e| e.to_string())?;
                bytes += chunk.len() as u64;
            }
            Ok(None) => break,
            Err(e) => {
                drop(out);
                let _ = std::fs::remove_file(&part);
                return Err(format!("the download stopped ({e})"));
            }
        }
    }
    out.flush().await.map_err(|e| e.to_string())?;
    drop(out);
    if bytes == 0 {
        let _ = std::fs::remove_file(&part);
        return Err("the server sent an empty file".into());
    }
    std::fs::rename(&part, d.join(&file_name)).map_err(|e| e.to_string())?;

    let album_id = album_id_of(&job.row);
    let art = if album_id.is_empty() {
        String::new()
    } else {
        save_cover(client, job, &album_id, &tag)
            .await
            .unwrap_or_default()
    };
    // bit depth, sample rate and bitrate for the label come from /info (best effort)
    let info = if original {
        let url = format!("{}/api/library/tracks/{id}/info", job.base);
        match get(client, &url, &job.auth, true).await {
            Ok(r) => r.json::<Value>().await.unwrap_or(Value::Null),
            Err(_) => Value::Null,
        }
    } else {
        Value::Null
    };
    let (format, quality) = kept(original, &ext, &job.row, &info);
    let cover = !art.is_empty() && d.join(&art).is_file();
    let song = Song {
        id: id.to_string(),
        title: fmt::s(&job.row, "title"),
        artist: {
            let a = fmt::s(&job.row, "artist");
            if a.is_empty() {
                fmt::s(&job.row, "albumartist")
            } else {
                a
            }
        },
        album: fmt::s(&job.row, "album"),
        ext,
        file: file_name.clone(),
        art,
        album_id,
        bytes,
        quality,
        cover,
        lyrics: false,
        saved_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as f64)
            .unwrap_or(0.0),
        source_server: server.trim_end_matches('/').to_string(),
        duration: {
            let d = fmt::f(&job.row, "duration");
            if d > 0.0 {
                d
            } else {
                fmt::f(&info, "duration")
            }
        },
        track: whole(&job.row, "track"),
        disc: whole(&job.row, "disc"),
        format,
        artists: job
            .row
            .get("artists")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
    };
    let _g = INDEX.lock().unwrap_or_else(|e| e.into_inner());
    let mut songs = list();
    // A file of an earlier save with another extension goes.
    for old in songs
        .iter()
        .filter(|s| same(s, &song) && s.file != file_name)
    {
        let _ = std::fs::remove_file(audio_path(old));
    }
    songs.retain(|s| !same(s, &song));
    songs.push(song);
    write_index(&index_path(), &songs)?;
    Ok(Saved::New)
}

/// The album's cover at 600 px, once per album and server; returns its file name.
async fn save_cover(
    client: &reqwest::Client,
    job: &Job,
    album_id: &str,
    tag: &str,
) -> Option<String> {
    let d = dir();
    let stem = format!("cover-{album_id}-{tag}");
    for ext in ["jpg", "png", "webp", "gif"] {
        let name = format!("{stem}.{ext}");
        if d.join(&name).is_file() {
            return Some(name);
        }
    }
    let url = format!("{}/api/library/albums/{album_id}/cover?s=600", job.base);
    let res = get(client, &url, &job.auth, false).await.ok()?;
    let body = res.bytes().await.ok()?;
    let ext = super::media::image_ext(&body)?;
    let name = format!("{stem}.{ext}");
    std::fs::write(d.join(&name), &body).ok()?;
    Some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_type_follows_the_stream() {
        assert_eq!(
            ext_for(Some("original"), Some("audio/flac"), "flac"),
            "flac"
        );
        assert_eq!(
            ext_for(Some("original"), Some("application/octet-stream"), "FLAC"),
            "flac"
        );
        assert_eq!(ext_for(None, None, "mp3"), "mp3");
        assert_eq!(ext_for(Some("original"), Some("audio/mp4"), "alac"), "m4a");
        assert_eq!(
            ext_for(Some("flac"), Some("audio/flac"), "ape"),
            "flac",
            "made on the fly"
        );
        assert_eq!(ext_for(Some("flac-fallback"), None, "wv"), "flac");
        assert_eq!(ext_for(Some("original"), None, ""), "audio");
        assert_eq!(ext_for(None, Some("audio/mpeg; charset=x"), ""), "mp3");
        assert_eq!(ext_for(None, None, "../etc"), "etc", "never a path");
    }

    #[test]
    fn the_index_keeps_the_classic_shape_and_round_trips() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("index.json");
        assert!(read_index(&p).is_empty());
        let s = Song {
            id: "12".into(),
            title: "Song".into(),
            ext: "flac".into(),
            file: "12-abcd.flac".into(),
            source_server: "http://nas:8080".into(),
            bytes: 1234,
            quality: "FLAC 16/44.1".into(),
            format: "flac".into(),
            saved_at: 1_792_310_400.75,
            track: 3,
            disc: 1,
            artists: vec![json!("Band")],
            ..Song::default()
        };
        let before = revision();
        write_index(&p, std::slice::from_ref(&s)).unwrap();
        assert!(revision() > before);
        let back = read_index(&p);
        assert_eq!(back[0].saved_at, 1_792_310_400.0, "written as a whole number");
        assert_eq!(back, vec![Song { saved_at: 1_792_310_400.0, ..s.clone() }]);
        let raw: Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
        // the contract's fields (docs/contracts/offline-manifest.md) and the file-based ones
        for k in [
            "id",
            "title",
            "artist",
            "album",
            "album_id",
            "ext",
            "bytes",
            "quality",
            "cover",
            "lyrics",
            "saved_at",
            "source_server",
            "duration",
            "track",
            "disc",
            "format",
            "artists",
            "file",
            "art",
        ] {
            assert!(raw[0].get(k).is_some(), "{k}");
        }
        assert_eq!(raw[0]["id"], json!(12), "the id is a number");
        assert_eq!(raw[0]["saved_at"], json!(1_792_310_400_i64));
        // a record of the classic app (fewer fields) still reads
        std::fs::write(
            &p,
            br#"[{"id":"7","title":"Old","artist":"A","album":"B","file":"asset://x","art":""}]"#,
        )
        .unwrap();
        let old = read_index(&p);
        assert_eq!(old[0].title, "Old");
        assert_eq!(old[0].id, "7");
        assert_eq!(old[0].file, "x");
        assert_eq!(total_bytes(&[s.clone(), s]), 2468);
    }

    #[test]
    fn the_0_27_index_is_upgraded() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("12-abcd.flac"), b"0123456789").unwrap();
        std::fs::write(d.path().join("cover-a1-abcd.jpg"), b"jpg").unwrap();
        // desktop 0.27: id a string, saved_at with a fraction, quality "FLAC", no cover/format/…
        let v: Value = serde_json::from_str(
            r#"[{"id":"12","title":"Song","artist":"Band","album":"LP","ext":"flac","file":"12-abcd.flac",
                 "art":"cover-a1-abcd.jpg","album_id":"a1","bytes":10,"quality":"FLAC",
                 "saved_at":1792310400.5,"source_server":"http://nas:8080/","duration":200.5}]"#,
        )
        .unwrap();
        let s = &parse_index(v, d.path())[0];
        assert_eq!(s.id, "12");
        assert_eq!(s.quality, "FLAC", "kept as it is");
        assert_eq!(s.format, "flac");
        assert!(s.cover, "the cover file exists");
        assert!(!s.lyrics);
        assert_eq!(s.saved_at, 1_792_310_400.5, "a fraction reads");
        assert_eq!(s.source_server, "http://nas:8080");
        assert!(!s.converted());
        // the classic app: asset URLs, no ext, no bytes, no time
        let v = json!([{ "id": "12", "title": "Song", "file": "https://asset.localhost/C%3A/x/12-abcd.flac", "art": "asset://localhost/gone.jpg" }]);
        let s = &parse_index(v, d.path())[0];
        assert_eq!(s.file, "12-abcd.flac");
        assert_eq!(s.ext, "flac");
        assert_eq!(s.quality, "FLAC");
        assert_eq!(s.bytes, 10, "the size of the file on disk");
        assert!(s.saved_at > 0.0, "the file's time");
        assert_eq!(s.art, "gone.jpg");
        assert!(!s.cover, "no such file");
    }

    #[test]
    fn the_index_reads_leniently() {
        let d = tempfile::tempdir().unwrap();
        let v = json!({ "songs": [
            { "id": 1, "title": "One", "bytes": "100", "track": "4", "duration": null, "album_id": 42 },
            { "id": "", "title": "no id" },
            { "id": "x7", "title": "not a number" },
            "not a record",
            { "id": 2, "title": "Two", "source_server": "http://a" },
            { "id": 2, "title": "Two again", "source_server": "http://a" },
            { "id": 2, "title": "Two elsewhere", "source_server": "http://b" },
        ]});
        let songs = parse_index(v, d.path());
        let titles: Vec<&str> = songs.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(titles, ["One", "Two again", "Two elsewhere"]);
        assert_eq!(songs[0].bytes, 100);
        assert_eq!(songs[0].track, 4);
        assert_eq!(songs[0].duration, 0.0);
        assert_eq!(songs[0].album_id, "42");
        assert_eq!(songs[0].ext, "audio");
        assert_eq!(songs[0].quality, "", "nothing known");
        assert!(parse_index(json!("nope"), d.path()).is_empty());
        assert_eq!(file_name("../../etc/passwd"), "passwd", "never a path");
        assert_eq!(file_name(".."), "");
    }

    #[test]
    fn the_quality_label_follows_the_contract() {
        let row = json!({ "format": "flac", "bitrate": 900 });
        let info = json!({ "codec": "flac", "lossless": true, "bit_depth": 16, "sample_rate": 44100 });
        assert_eq!(kept(true, "flac", &row, &info), ("flac".into(), "FLAC 16/44.1".into()));
        let info = json!({ "codec": "flac", "lossless": true, "bit_depth": 24, "sample_rate": 96000 });
        assert_eq!(kept(true, "flac", &row, &info).1, "FLAC 24/96");
        assert_eq!(kept(true, "flac", &row, &Value::Null).1, "FLAC", "no /info");
        let mp3 = json!({ "format": "mp3", "bitrate": 320 });
        assert_eq!(kept(true, "mp3", &mp3, &Value::Null), ("mp3".into(), "MP3 320k".into()));
        assert_eq!(kept(true, "opus", &json!({}), &json!({ "codec": "opus", "bitrate": 160 })).1, "Opus 160k");
        let (format, quality) = kept(false, "flac", &json!({ "format": "ape" }), &Value::Null);
        assert_eq!((format.as_str(), quality.as_str()), ("flac", "FLAC (converted)"));
        let s = Song { quality: quality.clone(), ..Song::default() };
        assert!(s.converted());
        assert_eq!(quality_label("flac", true, 0, 44100.0, 0.0), "FLAC 44.1 kHz");
        assert_eq!(quality_label("alac", false, 24, 0.0, 0.0), "ALAC 24-bit");
        assert_eq!(quality_label("aac", false, 0, 0.0, 0.0), "AAC");
    }

    #[test]
    fn batches_end_with_one_toast_each_way() {
        let p = Progress {
            total: 1,
            done: 1,
            saved: 1,
            last_title: "Song".into(),
            ..Progress::default()
        };
        assert_eq!(
            notices(&p),
            vec![("Saved for offline: Song".to_string(), ToastKind::Ok)]
        );
        let p = Progress {
            total: 5,
            done: 5,
            saved: 3,
            failed: 2,
            first_error: "the server can't be reached".into(),
            ..Progress::default()
        };
        let n = notices(&p);
        assert_eq!(n[0].0, "Saved for offline: 3 songs");
        assert_eq!(
            n[1],
            (
                "Couldn't save 2 songs: the server can't be reached".to_string(),
                ToastKind::Err
            )
        );
        assert_eq!(server_tag("http://a").len(), 8);
        assert_ne!(server_tag("http://a"), server_tag("http://b"));
        assert_eq!(album_id_of(&json!({ "album_id": "../x1" })), "x1");
        assert_eq!(album_id_of(&json!({ "album_id": 42 })), "42");
        assert_eq!(status_words(401), "not signed in to the server");
    }

    #[test]
    fn saved_songs_play_from_disk() {
        let s = Song {
            id: "12".into(),
            title: "Song".into(),
            artist: "Band".into(),
            file: "12-abcd.flac".into(),
            quality: "FLAC".into(),
            duration: 200.0,
            ..Song::default()
        };
        let t = track(&s);
        assert_eq!(t.id.as_deref(), Some("12"));
        assert_eq!(t.title, "Song");
        assert_eq!(t.duration, Some(200.0));
        assert!(
            matches!(&t.source, cadenza::decode::Source::Path { path } if path.ends_with("12-abcd.flac"))
        );
        assert_eq!(
            tracks::encore_id(&t),
            None,
            "a file is not streamed from the server"
        );
    }
}
