//! Songs saved on this computer, to play without the server: the audio as the server streams it
//! to Cadenza (the original file when the engine decodes it) and the album cover, in
//! `<data dir>/encore-desktop/offline/`, listed in `index.json` in the record shape of the classic
//! app (clients/app/src/offline.rs: id, title, artist, album, ext, file, art) plus album_id,
//! bytes, quality, saved_at, source_server and duration. `file` and `art` are file names in that
//! folder. They play through the engine like any song (`Source::Path`).
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

/// One saved song.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Song {
    /// The library id on its server.
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
    /// `FLAC`, `MP3 320` (what the file is).
    pub quality: String,
    /// When it was saved (Unix seconds).
    pub saved_at: f64,
    /// The server it came from ("http://host:port").
    pub source_server: String,
    /// Seconds (0 = unknown).
    pub duration: f64,
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
    std::fs::read(p)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
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
    let server = job.base.clone();
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
    let quality = if original {
        fmt::quality(&job.row)
    } else {
        ext.to_ascii_uppercase()
    };
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
        saved_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0),
        source_server: server,
        duration: fmt::f(&job.row, "duration"),
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
            ..Song::default()
        };
        let before = revision();
        write_index(&p, std::slice::from_ref(&s)).unwrap();
        assert!(revision() > before);
        assert_eq!(read_index(&p), vec![s.clone()]);
        let raw: Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
        for k in [
            "id",
            "title",
            "artist",
            "album",
            "ext",
            "file",
            "art",
            "album_id",
            "bytes",
            "quality",
            "saved_at",
            "source_server",
        ] {
            assert!(raw[0].get(k).is_some(), "{k}");
        }
        // a record of the classic app (fewer fields) still reads
        std::fs::write(
            &p,
            br#"[{"id":"7","title":"Old","artist":"A","album":"B","file":"asset://x","art":""}]"#,
        )
        .unwrap();
        assert_eq!(read_index(&p)[0].title, "Old");
        assert_eq!(total_bytes(&[s.clone(), s]), 2468);
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
