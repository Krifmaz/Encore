//! Your folders (Downloads page): music folders on this computer that the browser may read, played here
//! without uploading anything – the desktop app's "Your folders" (clients/desktop/src/desktop/local.rs) for
//! the browser. Chrome, Edge and other Chromium browsers only: it needs the File System Access API
//! (`showDirectoryPicker`); other browsers say so on the page.
//!
//! - The folders' handles are kept in IndexedDB (`encore-local`, store `folders`), the one store that can
//!   hold them. After a restart the browser asks again before the page may read them ("Allow access").
//! - A scan walks the folders and reads each audio file's tags with lofty (the server's reader) from its
//!   first [`HEAD_BYTES`]; what it read is kept in the Cache API (`encore-local-v1`), so a file is read again
//!   only when its size or time changed.
//! - Only the tags go to the server (`POST /api/library/match-local`), which answers with the library song
//!   each file already is. Those songs play from the file wherever they appear (player/local.rs asks [`has`]
//!   and [`audio_url`]); the rest are listed on the page under ids below zero, which no library song has.

use crate::api;
use crate::dom;
use crate::json::J;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};

/// Sent on `window` whenever the folders, their songs or the server's answers change.
pub const CHANGED_EVENT: &str = "encore:folders-changed";
pub const UNSUPPORTED: &str = "This browser can't open folders on your computer. Chrome and Edge can, and so does the Encore desktop app.";
const DB: &str = "encore-local";
const STORE: &str = "folders";
const CACHE: &str = "encore-local-v1";
const INDEX_KEY: &str = "/local/index.json";
/// What a scan reads of each file for its tags (the whole file when it is smaller).
pub const HEAD_BYTES: f64 = 4.0 * 1024.0 * 1024.0;
/// Most songs one match call sends (the server takes up to 2000).
const MATCH_CHUNK: usize = 500;
pub const AUDIO: &[&str] = &["flac", "mp3", "m4a", "aac", "ogg", "oga", "opus", "wav", "aif", "aiff", "wv", "ape"];

/// One song in a folder.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Song {
    /// "<folder>/<path inside it>": the song's name here and its key on the server's answer.
    pub key: String,
    pub size: f64,
    pub modified: f64,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub albumartist: String,
    pub track: u32,
    pub disc: u32,
    /// Seconds; 0 when unknown (formats whose length needs the whole file leave it out).
    pub duration: f64,
    pub format: String,
    pub quality: String,
    pub isrc: String,
    pub mbid: String,
}

impl Song {
    /// An id below zero, the same for this file every time, that no library song has.
    pub fn local_id(&self) -> i64 { local_id(&self.key) }
}

/// FNV-1a of the key, kept within what JavaScript numbers hold exactly, below zero.
pub fn local_id(key: &str) -> i64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in key.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    -((h & 0x000f_ffff_ffff_ffff) as i64) - 1
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Saved {
    songs: Vec<Song>,
    /// file key → the library song's row
    matches: HashMap<String, Value>,
    /// the scan the answers belong to
    matched_at: u64,
    scan: u64,
}

thread_local! {
    /// (folder name, its directory handle)
    static FOLDERS: RefCell<Vec<(String, JsValue)>> = const { RefCell::new(Vec::new()) };
    static SAVED: RefCell<Saved> = RefCell::new(Saved::default());
    /// file key → its file handle (found by the last walk this session)
    static FILES: RefCell<HashMap<String, JsValue>> = RefCell::new(HashMap::new());
    /// song id (library or below zero) → file key
    static IDS: RefCell<HashMap<i64, String>> = RefCell::new(HashMap::new());
    static LOADED: Cell<bool> = const { Cell::new(false) };
    static SCANNING: Cell<bool> = const { Cell::new(false) };
    static MATCHING: Cell<bool> = const { Cell::new(false) };
    static SCANNED: Cell<usize> = const { Cell::new(0) };
}

fn changed() { dom::dispatch(CHANGED_EVENT, None); }

/// This browser can open folders.
pub fn supported() -> bool {
    web_sys::window().is_some_and(|w| w.is_secure_context() && js_sys::Reflect::has(&w, &"showDirectoryPicker".into()).unwrap_or(false))
}

pub fn folders() -> Vec<String> { FOLDERS.with(|f| f.borrow().iter().map(|(n, _)| n.clone()).collect()) }
pub fn songs() -> Vec<Song> { SAVED.with(|s| s.borrow().songs.clone()) }
pub fn matches() -> HashMap<String, Value> { SAVED.with(|s| s.borrow().matches.clone()) }
pub fn scanning() -> Option<usize> { SCANNING.with(|s| s.get()).then(|| SCANNED.with(|n| n.get())) }
pub fn matching() -> bool { MATCHING.with(|m| m.get()) }

/// A song (a library id, or a folder song's id below zero) plays from a file in a folder.
pub fn has(id: i64) -> bool {
    IDS.with(|m| m.borrow().get(&id).cloned()).is_some_and(|k| FILES.with(|f| f.borrow().contains_key(&k)))
}

/// The quality label of the file a song plays from.
pub fn quality(id: i64) -> Option<String> {
    let key = IDS.with(|m| m.borrow().get(&id).cloned())?;
    SAVED.with(|s| s.borrow().songs.iter().find(|x| x.key == key).map(|x| x.quality.clone()))
}

/// An object URL of the file a song plays from (the player revokes it when it moves on).
pub async fn audio_url(id: i64) -> Option<String> {
    let key = IDS.with(|m| m.borrow().get(&id).cloned())?;
    let handle = FILES.with(|f| f.borrow().get(&key).cloned())?;
    let file = file_of(&handle).await?;
    web_sys::Url::create_object_url_with_blob(&file).ok()
}

fn rebuild_ids() {
    let (songs, matches) = SAVED.with(|s| { let s = s.borrow(); (s.songs.clone(), s.matches.clone()) });
    IDS.with(|m| {
        let mut m = m.borrow_mut();
        m.clear();
        for s in &songs {
            let id = matches.get(&s.key).and_then(|r| r.g("id").as_i64()).unwrap_or_else(|| s.local_id());
            m.insert(id, s.key.clone());
        }
    });
}

/// A folder song as the player's queue wants it: the library song's row when the library has it, otherwise
/// the file's own tags under its id below zero.
pub fn row_of(s: &Song, matches: &HashMap<String, Value>) -> Value {
    if let Some(r) = matches.get(&s.key) { return r.clone(); }
    let mut t = json!({ "id": s.local_id(), "title": s.title, "artist": s.artist, "album": s.album, "track": s.track, "disc": s.disc, "local": true });
    if s.duration > 0.0 { t["duration"] = json!(s.duration); }
    if !s.format.is_empty() { t["format"] = json!(s.format); }
    t
}

/// What a match call sends for a song (only its tags).
pub fn match_body(s: &Song) -> Value {
    json!({
        "key": s.key, "title": s.title, "artist": s.artist,
        "duration": (s.duration > 0.0).then_some(s.duration),
        "isrc": (!s.isrc.is_empty()).then_some(&s.isrc),
        "mbid": (!s.mbid.is_empty()).then_some(&s.mbid),
    })
}

pub fn is_audio(name: &str) -> bool {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    AUDIO.contains(&ext.as_str())
}

pub fn codec_of(ext: &str) -> &'static str {
    match ext {
        "flac" => "flac", "mp3" => "mp3", "m4a" | "aac" => "aac", "ogg" | "oga" => "vorbis", "opus" => "opus",
        "wav" => "wav", "aif" | "aiff" => "aiff", "wv" => "wv", "ape" => "ape", _ => "",
    }
}

/// The length a file's first bytes tell reliably: FLAC's stream info and an MP4's header do; the others
/// need the whole file (or a header that may be missing), so they leave it out.
pub fn trusted_length(format: &str) -> bool { matches!(format, "flac" | "aac" | "alac" | "wav" | "aiff") }

/// "FLAC 24/96", "FLAC 16/44.1", "MP3 320", like the desktop app's labels.
pub fn quality_label(format: &str, bits: Option<u8>, rate: Option<u32>, kbps: Option<u32>) -> String {
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

/// The tags of a file from its first bytes (`whole`: they are the whole file).
pub fn read_tags(name: &str, bytes: Vec<u8>, whole: bool) -> Song {
    use lofty::file::{AudioFile, TaggedFileExt};
    use lofty::tag::{Accessor, ItemKey};
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    let stem = name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name).to_string();
    let mut s = Song { title: stem, format: codec_of(&ext).into(), ..Song::default() };
    let parsed = lofty::probe::Probe::new(std::io::Cursor::new(bytes)).guess_file_type().ok().and_then(|p| p.read().ok());
    match parsed {
        Some(f) => {
            let props = f.properties();
            if ext == "m4a" && props.bit_depth().is_some() { s.format = "alac".into(); }
            if whole || trusted_length(&s.format) { s.duration = props.duration().as_secs_f64(); }
            if let Some(tag) = f.primary_tag().or_else(|| f.first_tag()) {
                let txt = |v: Option<std::borrow::Cow<'_, str>>| v.map(|x| x.trim().to_string()).unwrap_or_default();
                let title = txt(tag.title());
                if !title.is_empty() { s.title = title; }
                s.artist = txt(tag.artist());
                s.album = txt(tag.album());
                s.track = tag.track().unwrap_or(0);
                s.disc = tag.disk().unwrap_or(0);
                let item = |k: ItemKey| tag.get_string(k).map(|x| x.trim().to_string()).unwrap_or_default();
                s.albumartist = item(ItemKey::AlbumArtist);
                s.isrc = item(ItemKey::Isrc);
                s.mbid = item(ItemKey::MusicBrainzRecordingId);
            }
            // an MP3's bitrate is in its first frame; the others' needs the whole file
            let kbps = if whole || trusted_length(&s.format) || s.format == "mp3" { props.audio_bitrate() } else { None };
            s.quality = quality_label(&s.format, props.bit_depth(), props.sample_rate(), kbps);
        }
        None => s.quality = s.format.to_ascii_uppercase(),
    }
    if s.artist.is_empty() { s.artist = s.albumartist.clone(); }
    s
}

// ---- IndexedDB: the folder handles -----------------------------------------------------------------

fn get(o: &JsValue, k: &str) -> JsValue { js_sys::Reflect::get(o, &k.into()).unwrap_or(JsValue::UNDEFINED) }

fn call0(o: &JsValue, method: &str) -> Result<JsValue, JsValue> {
    get(o, method).dyn_into::<js_sys::Function>()?.call0(o)
}

fn call1(o: &JsValue, method: &str, a: &JsValue) -> Result<JsValue, JsValue> {
    get(o, method).dyn_into::<js_sys::Function>()?.call1(o, a)
}

async fn wait(v: JsValue) -> Result<JsValue, JsValue> {
    wasm_bindgen_futures::JsFuture::from(v.dyn_into::<js_sys::Promise>()?).await
}

/// An IndexedDB request as a promise of its result.
fn request(req: JsValue) -> js_sys::Promise {
    js_sys::Promise::new(&mut |resolve, reject| {
        let r2 = req.clone();
        let ok = Closure::once_into_js(move |_: JsValue| { let _ = resolve.call1(&JsValue::NULL, &get(&r2, "result")); });
        let r3 = req.clone();
        let err = Closure::once_into_js(move |_: JsValue| { let _ = reject.call1(&JsValue::NULL, &get(&r3, "error")); });
        let _ = js_sys::Reflect::set(&req, &"onsuccess".into(), &ok);
        let _ = js_sys::Reflect::set(&req, &"onerror".into(), &err);
    })
}

async fn db() -> Result<JsValue, JsValue> {
    let idb = get(&dom::window(), "indexedDB");
    let req = idb.dyn_ref::<js_sys::Object>().ok_or("no IndexedDB")?;
    let open = js_sys::Reflect::get(req, &"open".into())?.dyn_into::<js_sys::Function>()?.call2(req, &DB.into(), &1.into())?;
    let o2 = open.clone();
    let upgrade = Closure::once_into_js(move |_: JsValue| {
        let d = get(&o2, "result");
        let _ = call1(&d, "createObjectStore", &STORE.into());
    });
    let _ = js_sys::Reflect::set(&open, &"onupgradeneeded".into(), &upgrade);
    wasm_bindgen_futures::JsFuture::from(request(open)).await
}

async fn store(mode: &str) -> Result<JsValue, JsValue> {
    let d = db().await?;
    let tx = get(&d, "transaction").dyn_into::<js_sys::Function>()?.call2(&d, &STORE.into(), &mode.into())?;
    call1(&tx, "objectStore", &STORE.into())
}

async fn idb_all() -> Result<Vec<(String, JsValue)>, JsValue> {
    let st = store("readonly").await?;
    let keys = wasm_bindgen_futures::JsFuture::from(request(call0(&st, "getAllKeys")?)).await?;
    let st = store("readonly").await?;
    let vals = wasm_bindgen_futures::JsFuture::from(request(call0(&st, "getAll")?)).await?;
    let (k, v): (js_sys::Array, js_sys::Array) = (keys.into(), vals.into());
    Ok(k.iter().zip(v.iter()).filter_map(|(k, v)| Some((k.as_string()?, v))).collect())
}

async fn idb_put(name: &str, handle: &JsValue) -> Result<(), JsValue> {
    let st = store("readwrite").await?;
    let req = get(&st, "put").dyn_into::<js_sys::Function>()?.call2(&st, handle, &name.into())?;
    wasm_bindgen_futures::JsFuture::from(request(req)).await.map(|_| ())
}

async fn idb_del(name: &str) -> Result<(), JsValue> {
    let st = store("readwrite").await?;
    let req = call1(&st, "delete", &name.into())?;
    wasm_bindgen_futures::JsFuture::from(request(req)).await.map(|_| ())
}

// ---- the Cache API: what the scans read ---------------------------------------------------------------

async fn cache() -> Option<web_sys::Cache> {
    let c: web_sys::CacheStorage = get(&dom::window(), "caches").dyn_into().ok()?;
    wasm_bindgen_futures::JsFuture::from(c.open(CACHE)).await.ok()?.dyn_into().ok()
}

async fn read_saved() -> Saved {
    let Some(c) = cache().await else { return Saved::default() };
    let Ok(r) = wasm_bindgen_futures::JsFuture::from(c.match_with_str(INDEX_KEY)).await else { return Saved::default() };
    let Ok(r) = r.dyn_into::<web_sys::Response>() else { return Saved::default() };
    let Ok(p) = r.text() else { return Saved::default() };
    wasm_bindgen_futures::JsFuture::from(p).await.ok().and_then(|t| t.as_string()).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

async fn write_saved() {
    let Some(c) = cache().await else { return };
    let text = SAVED.with(|s| serde_json::to_string(&*s.borrow()).unwrap_or_default());
    if let Ok(r) = web_sys::Response::new_with_opt_str(Some(&text)) {
        let _ = wasm_bindgen_futures::JsFuture::from(c.put_with_str(INDEX_KEY, &r)).await;
    }
}

// ---- the folders ----------------------------------------------------------------------------------------

/// Read the folders and what the last scan found (once per page load).
pub async fn load() {
    if LOADED.with(|l| l.replace(true)) || !supported() { return; }
    let list = idb_all().await.unwrap_or_default();
    FOLDERS.with(|f| *f.borrow_mut() = list);
    let saved = read_saved().await;
    SAVED.with(|s| *s.borrow_mut() = saved);
    rebuild_ids();
    changed();
}

/// The browser lets the page read this folder now (`ask`: ask the listener, which needs a click).
async fn allowed(handle: &JsValue, ask: bool) -> bool {
    let opts = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&opts, &"mode".into(), &"read".into());
    let method = if ask { "requestPermission" } else { "queryPermission" };
    match call1(handle, method, &opts) {
        Ok(p) => wait(p).await.ok().and_then(|v| v.as_string()).as_deref() == Some("granted"),
        Err(_) => false,
    }
}

/// Folders the browser wants asked again before the page may read them (after a restart).
pub async fn needs_permission() -> bool {
    let handles: Vec<JsValue> = FOLDERS.with(|f| f.borrow().iter().map(|(_, h)| h.clone()).collect());
    for h in &handles {
        if !allowed(h, false).await { return true; }
    }
    false
}

/// "Allow access": ask for every folder (from a click), then scan.
pub async fn ask_permission() {
    let handles: Vec<JsValue> = FOLDERS.with(|f| f.borrow().iter().map(|(_, h)| h.clone()).collect());
    for h in &handles { allowed(h, true).await; }
    scan().await;
}

/// "Add folder…": the browser's folder picker; the folder is kept and scanned. Err with words to show.
pub async fn add_folder() -> Result<(), String> {
    let opts = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&opts, &"mode".into(), &"read".into());
    let handle = match call1(&dom::window(), "showDirectoryPicker", &opts) {
        Ok(p) => match wait(p).await { Ok(h) => h, Err(_) => return Ok(()) },
        Err(_) => return Err(UNSUPPORTED.into()),
    };
    let mut name = get(&handle, "name").as_string().unwrap_or_else(|| "Music".into());
    let taken = folders();
    if taken.contains(&name) {
        let mut n = 2;
        while taken.contains(&format!("{name} ({n})")) { n += 1; }
        name = format!("{name} ({n})");
    }
    idb_put(&name, &handle).await.map_err(|e| crate::offline::store::js_error(&e))?;
    FOLDERS.with(|f| f.borrow_mut().push((name, handle)));
    scan().await;
    Ok(())
}

pub async fn remove_folder(name: &str) {
    let _ = idb_del(name).await;
    FOLDERS.with(|f| f.borrow_mut().retain(|(n, _)| n != name));
    scan().await;
}

async fn file_of(handle: &JsValue) -> Option<web_sys::File> {
    wait(call0(handle, "getFile").ok()?).await.ok()?.dyn_into().ok()
}

/// Every audio file under a directory handle: (key, file handle).
async fn walk(prefix: String, dir: JsValue, out: &mut Vec<(String, JsValue)>) {
    let mut stack = vec![(prefix, dir, 0usize)];
    while let Some((prefix, dir, depth)) = stack.pop() {
        if depth > 32 { continue; }
        let Ok(it) = call0(&dir, "values") else { continue };
        loop {
            let Ok(p) = call0(&it, "next") else { break };
            let Ok(step) = wait(p).await else { break };
            if get(&step, "done").as_bool().unwrap_or(true) { break; }
            let entry = get(&step, "value");
            let name = get(&entry, "name").as_string().unwrap_or_default();
            if name.starts_with('.') { continue; }
            let key = format!("{prefix}/{name}");
            match get(&entry, "kind").as_string().as_deref() {
                Some("directory") => stack.push((key, entry, depth + 1)),
                Some("file") if is_audio(&name) => out.push((key, entry)),
                _ => {}
            }
        }
    }
}

/// Scan every folder the page may read; a file's tags are read again only when its size or time changed.
/// Folders it may not read (permission not given again yet) keep what the last scan found.
pub async fn scan() {
    if SCANNING.with(|s| s.replace(true)) { return; }
    SCANNED.with(|n| n.set(0));
    changed();
    let before: HashMap<String, Song> = SAVED.with(|s| s.borrow().songs.iter().map(|x| (x.key.clone(), x.clone())).collect());
    let list: Vec<(String, JsValue)> = FOLDERS.with(|f| f.borrow().clone());
    let mut found: Vec<Song> = vec![];
    let mut files: HashMap<String, JsValue> = HashMap::new();
    for (name, handle) in list {
        if !allowed(&handle, false).await {
            found.extend(before.values().filter(|s| s.key.starts_with(&format!("{name}/"))).cloned());
            continue;
        }
        let mut entries = vec![];
        walk(name, handle, &mut entries).await;
        for (key, fh) in entries {
            SCANNED.with(|n| n.set(n.get() + 1));
            if SCANNED.with(|n| n.get()).is_multiple_of(50) { changed(); }
            let Some(file) = file_of(&fh).await else { continue };
            let (size, modified) = (file.size(), file.last_modified());
            let song = match before.get(&key) {
                Some(old) if old.size == size && old.modified == modified => old.clone(),
                _ => {
                    let whole = size <= HEAD_BYTES;
                    let blob = if whole { Ok(file.clone().into()) } else { file.slice_with_f64_and_f64(0.0, HEAD_BYTES) };
                    let bytes = match blob {
                        Ok(b) => match wasm_bindgen_futures::JsFuture::from(b.array_buffer()).await {
                            Ok(buf) => js_sys::Uint8Array::new(&buf).to_vec(),
                            Err(_) => vec![],
                        },
                        Err(_) => vec![],
                    };
                    let name = key.rsplit('/').next().unwrap_or(&key).to_string();
                    let mut s = read_tags(&name, bytes, whole);
                    s.key = key.clone();
                    s.size = size;
                    s.modified = modified;
                    s
                }
            };
            files.insert(key, fh);
            found.push(song);
        }
    }
    found.sort_by(|a, b| {
        let k = |s: &Song| (if s.albumartist.is_empty() { &s.artist } else { &s.albumartist }.to_lowercase(), s.album.to_lowercase(), s.disc, s.track, s.title.to_lowercase());
        k(a).cmp(&k(b))
    });
    found.dedup_by(|a, b| a.key == b.key);
    FILES.with(|f| *f.borrow_mut() = files);
    SAVED.with(|s| {
        let mut s = s.borrow_mut();
        if s.songs != found {
            let keep: std::collections::HashSet<String> = found.iter().map(|x| x.key.clone()).collect();
            s.matches.retain(|k, _| keep.contains(k));
            s.songs = found;
            s.scan += 1;
        }
    });
    rebuild_ids();
    write_saved().await;
    SCANNING.with(|s| s.set(false));
    changed();
    match_now().await;
}

/// Ask the server which songs its library has (only the tags go), when its answers are older than the scan.
pub async fn match_now() {
    let (songs, scan, at) = SAVED.with(|s| { let s = s.borrow(); (s.songs.clone(), s.scan, s.matched_at) });
    if songs.is_empty() || scan == at || MATCHING.with(|m| m.replace(true)) { return; }
    changed();
    let mut found: HashMap<String, Value> = HashMap::new();
    let mut ok = true;
    for part in songs.chunks(MATCH_CHUNK) {
        match api::post("/library/match-local", json!({ "songs": part.iter().map(match_body).collect::<Vec<_>>() })).await {
            Ok(v) => if let Some(m) = v.g("matches").as_object() { found.extend(m.iter().map(|(k, r)| (k.clone(), r.clone()))); },
            Err(e) => { dom::console_error(&format!("local folders: {}", e.message)); ok = false; break; }
        }
    }
    MATCHING.with(|m| m.set(false));
    if ok {
        SAVED.with(|s| { let mut s = s.borrow_mut(); s.matches = found; s.matched_at = scan; });
        rebuild_ids();
        write_saved().await;
    }
    changed();
}

/// After a page load the file handles are found again by a scan (cheap when nothing changed); until then
/// the songs are listed but play from the server.
pub async fn ensure_scanned() {
    load().await;
    if FILES.with(|f| f.borrow().is_empty()) && !folders().is_empty() && !needs_permission().await { scan().await; }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_ids_are_below_zero_and_stable() {
        let a = local_id("Music/A/01.flac");
        assert!(a < 0);
        assert_eq!(a, local_id("Music/A/01.flac"));
        assert_ne!(a, local_id("Music/A/02.flac"));
        assert!(a > -(1i64 << 53), "a JavaScript number holds it exactly");
    }

    #[test]
    fn a_folder_song_plays_as_the_library_song_when_the_library_has_it() {
        let s = Song { key: "M/x.flac".into(), title: "T".into(), artist: "A".into(), format: "flac".into(), duration: 100.0, ..Song::default() };
        let mut m = HashMap::new();
        let r = row_of(&s, &m);
        assert_eq!(r["id"], s.local_id());
        assert_eq!(r["local"], true);
        assert_eq!(r["format"], "flac");
        m.insert("M/x.flac".to_string(), json!({ "id": 9, "title": "Library" }));
        assert_eq!(row_of(&s, &m)["id"], 9);
    }

    #[test]
    fn only_audio_files_are_read_and_unsure_lengths_are_left_out() {
        assert!(is_audio("01 Song.FLAC"));
        assert!(!is_audio("cover.jpg"));
        assert!(!is_audio("README"));
        assert!(trusted_length("flac"));
        assert!(!trusted_length("mp3"));
        let s = Song { key: "k".into(), title: "T".into(), ..Song::default() };
        let b = match_body(&s);
        assert!(b["duration"].is_null() && b["isrc"].is_null() && b["mbid"].is_null());
    }

    #[test]
    fn a_file_without_tags_is_named_by_its_file() {
        let s = read_tags("Some Song.mp3", b"not audio".to_vec(), true);
        assert_eq!(s.title, "Some Song");
        assert_eq!(s.format, "mp3");
        assert_eq!(s.quality, "MP3");
        assert_eq!(quality_label("flac", Some(24), Some(96000), None), "FLAC 24/96");
        assert_eq!(quality_label("mp3", None, Some(44100), Some(320)), "MP3 320");
    }
}
