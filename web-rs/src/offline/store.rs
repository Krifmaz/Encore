//! Where the saved songs live: one Cache API cache, `encore-offline-v1`, the same in Chrome, Brave, Firefox and
//! Safari (iOS too). Its entries, keyed by same-origin paths nothing ever fetches:
//!
//! - `/offline/index.json` – the list of saved songs, a JSON array of [`Record`]s (the shape of
//!   docs/contracts/offline-manifest.md, the same the desktop app keeps in its own `index.json`);
//! - `/offline/outbox.json` – plays and likes waiting for the server (outbox.rs);
//! - `/offline/audio/<track id>` – the audio, with `content-type` and `x-encore-bytes` headers;
//! - `/offline/cover/<album id>` – the album cover at 600 px, shared by the album's songs;
//! - `/offline/lyrics/<track id>` – the lyrics answer of the server, as JSON text.
//!
//! The two JSON documents are read, changed and written back under one lock ([`lock`]): the page's own, plus
//! the browser's Web Locks lock `encore-offline` shared by every tab of the site (the installed app window and
//! a browser tab, two tabs) where the browser has it. So two downloads finishing at the same time, a removal
//! and the outbox never overwrite each other's changes, in one tab or across tabs. The index is also kept in
//! memory after the first read, so [`has`] and [`record`] answer at once.
//!
//! A download writes the audio last, right before its record ([`add_record`]); what a tab closed in between
//! still leaves behind goes at the next [`sweep`].

use crate::dom;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::thread::LocalKey;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{Blob, Cache, CacheStorage, Response, StorageEstimate, StorageManager};

pub const CACHE_NAME: &str = "encore-offline-v1";
pub const INDEX_KEY: &str = "/offline/index.json";
pub const OUTBOX_KEY: &str = "/offline/outbox.json";
/// Sent on `window` whenever the saved songs or the outbox change, so a page that shows them can redraw.
pub const CHANGED_EVENT: &str = "encore:offline-changed";

/// What a download says when this browser has no Cache API here (a plain-http address, a private window).
pub const NO_STORE: &str = "This browser can't keep downloads here: they need an HTTPS or localhost address, and some browsers allow none in a private window";
/// A full disk or a browser limit, in words a toast can show.
pub const QUOTA_FULL: &str = "There is no room left on this device for downloads";

pub fn audio_key(id: i64) -> String { format!("/offline/audio/{id}") }
pub fn cover_key(album_id: &str) -> String { format!("/offline/cover/{}", clean_id(album_id)) }
pub fn lyrics_key(id: i64) -> String { format!("/offline/lyrics/{id}") }

/// Letters, digits, `-` and `_` only: an album id becomes part of a cache key and a URL.
pub fn clean_id(s: &str) -> String { s.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect() }

// ---- the record ----------------------------------------------------------------------------------

/// One saved song (docs/contracts/offline-manifest.md). Reading is lenient: numbers may come as strings, a
/// missing field takes its empty value, and the desktop app's older shape (`id` as a string, `file`, `art`)
/// reads too – `art` becomes `cover`, and `format` / `quality` come from `ext` when they are missing.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Record {
    /// The server's track id.
    pub id: i64,
    pub title: String,
    pub artist: String,
    pub album: String,
    /// "" when the song has no album id.
    pub album_id: String,
    /// The saved file's extension (`flac`, `mp3`, `aac` …).
    pub ext: String,
    /// The saved audio's size.
    pub bytes: u64,
    /// What the player shows: "FLAC 16/44.1", "MP3 320k", "AAC 256k (converted)".
    pub quality: String,
    /// The album cover was saved.
    pub cover: bool,
    /// Lyrics were saved.
    pub lyrics: bool,
    /// When it was saved (unix seconds).
    pub saved_at: i64,
    /// The server it came from (`location.origin` on the web).
    pub source_server: String,
    /// Seconds (0 = unknown).
    pub duration: f64,
    /// Track and disc number (0 = unknown).
    pub track: i64,
    pub disc: i64,
    /// The saved codec: `flac`, `mp3`, `aac`, `opus`, `vorbis` …
    pub format: String,
    /// The server's `artists` list for the song, when it sent one.
    pub artists: Vec<Value>,
}

impl Record {
    /// A copy the server converted (the browser can't play the original): "Download again in original quality".
    pub fn converted(&self) -> bool { is_converted(&self.quality) }

    /// A record from a JSON object (None without a usable track id).
    pub fn from_value(v: &Value) -> Option<Record> {
        let o = v.as_object()?;
        let get = |k: &str| o.get(k).unwrap_or(&Value::Null);
        let id = int(get("id")).filter(|id| *id > 0)?;
        let mut format = text(get("format")).to_ascii_lowercase();
        // older indexes: the extension of `file`, else the format, else "audio"
        let mut ext = text(get("ext")).to_ascii_lowercase();
        if ext.is_empty() { ext = file_ext(&text(get("file"))); }
        if ext.is_empty() { ext = clean_id(&format); }
        if ext.is_empty() { ext = "audio".into(); }
        if format.is_empty() { format = ext.clone(); }
        let mut quality = text(get("quality"));
        if quality.is_empty() && ext != "audio" { quality = ext.to_ascii_uppercase(); }
        let cover = match o.get("cover") {
            Some(c) => flag(c),
            // the desktop's older shape: `art` is the cover's file name ("" = none)
            None => flag(get("art")),
        };
        Some(Record {
            id,
            title: text(get("title")),
            artist: text(get("artist")),
            album: text(get("album")),
            album_id: text(get("album_id")),
            ext,
            bytes: number(get("bytes")).filter(|b| *b > 0.0).map(|b| b as u64).unwrap_or(0),
            quality,
            cover,
            lyrics: flag(get("lyrics")),
            saved_at: number(get("saved_at")).map(|s| s as i64).unwrap_or(0),
            source_server: text(get("source_server")),
            duration: number(get("duration")).filter(|d| d.is_finite() && *d > 0.0).unwrap_or(0.0),
            track: int(get("track")).unwrap_or(0),
            disc: int(get("disc")).unwrap_or(0),
            format,
            artists: get("artists").as_array().cloned().unwrap_or_default(),
        })
    }
}

impl<'de> Deserialize<'de> for Record {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Value::deserialize(d)?;
        Record::from_value(&v).ok_or_else(|| serde::de::Error::custom("a saved song needs a numeric track id"))
    }
}

/// A string, or a number written as one (null → "").
fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.trim().to_string(),
        Value::Number(_) => crate::json::js_string(v),
        _ => String::new(),
    }
}
fn number(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
    .filter(|x: &f64| x.is_finite())
}
fn int(v: &Value) -> Option<i64> { number(v).filter(|x| x.fract() == 0.0).map(|x| x as i64) }
fn flag(v: &Value) -> bool { crate::json::truthy(v) }
/// The extension of a file name, a path or an asset URL ("" when it has none).
fn file_ext(file: &str) -> String {
    let name = file.split(['?', '#']).next().unwrap_or("").rsplit(['/', '\\']).next().unwrap_or("");
    match name.rsplit_once('.') {
        Some((stem, e)) if !stem.is_empty() && !e.is_empty() && e.len() <= 5 && e.chars().all(|c| c.is_ascii_alphanumeric()) => e.to_ascii_lowercase(),
        _ => String::new(),
    }
}

/// `quality` ends in "(converted)": the server made this copy, the original was not saved.
pub fn is_converted(quality: &str) -> bool { quality.trim_end().ends_with("(converted)") }

/// The index document as saved: an array of records; an object with a `songs` / `tracks` list reads too.
/// Entries without a track id are dropped; a track id that appears twice keeps its last entry.
pub fn parse_index(text: &str) -> Vec<Record> {
    let Ok(v) = serde_json::from_str::<Value>(text) else { return vec![] };
    let list = match &v {
        Value::Array(a) => a.clone(),
        Value::Object(o) => ["songs", "tracks", "records"].iter().find_map(|k| o.get(*k).and_then(|x| x.as_array()).cloned()).unwrap_or_default(),
        _ => vec![],
    };
    let mut out: Vec<Record> = Vec::with_capacity(list.len());
    for r in list.iter().filter_map(Record::from_value) {
        match out.iter().position(|x| x.id == r.id) {
            Some(i) => out[i] = r,
            None => out.push(r),
        }
    }
    out
}
pub fn index_json(v: &[Record]) -> String { serde_json::to_string(v).unwrap_or_else(|_| "[]".into()) }

/// Put `r` in the list: in place of an older record of the same song, else at the end.
pub fn upsert(v: &mut Vec<Record>, r: Record) {
    match v.iter().position(|x| x.id == r.id) {
        Some(i) => v[i] = r,
        None => v.push(r),
    }
}

/// The album covers no remaining record needs any more.
pub fn unused_covers(removed: &[Record], kept: &[Record]) -> Vec<String> {
    let used: HashSet<&str> = kept.iter().filter(|r| r.cover).map(|r| r.album_id.as_str()).collect();
    let mut out: Vec<String> = vec![];
    for r in removed.iter().filter(|r| r.cover && !r.album_id.is_empty()) {
        if !used.contains(r.album_id.as_str()) && !out.contains(&r.album_id) { out.push(r.album_id.clone()); }
    }
    out
}

/// A browser error in plain words; a full store becomes [`QUOTA_FULL`].
pub fn error_words(name: &str, message: &str) -> String {
    let m = message.to_ascii_lowercase();
    if name == "QuotaExceededError" || m.contains("quota") || m.contains("no space") || m.contains("disk is full") { return QUOTA_FULL.into(); }
    if name == "AbortError" { return "The download was stopped".into(); }
    if !message.trim().is_empty() { return message.trim().to_string(); }
    if !name.is_empty() { return name.to_string(); }
    "Something went wrong".into()
}
pub(crate) fn js_error(e: &JsValue) -> String {
    let get = |k: &str| js_sys::Reflect::get(e, &k.into()).ok().and_then(|v| v.as_string()).unwrap_or_default();
    match e.as_string() {
        Some(s) => error_words("", &s),
        None => error_words(&get("name"), &get("message")),
    }
}

// ---- the lock ------------------------------------------------------------------------------------

/// A lock for async code on one thread: a busy flag plus the queue of those waiting, served in order. The
/// one who lets go hands the lock straight to the next in line, so nobody can slip in between.
pub struct Lock {
    busy: Cell<bool>,
    waiting: RefCell<VecDeque<futures::channel::oneshot::Sender<()>>>,
}
impl Lock {
    pub const fn new() -> Self { Lock { busy: Cell::new(false), waiting: RefCell::new(VecDeque::new()) } }
    /// Someone holds it.
    pub fn held(&self) -> bool { self.busy.get() }
}
impl Default for Lock { fn default() -> Self { Self::new() } }

/// Holding the lock; dropping it lets the next in line in.
pub struct Guard(&'static LocalKey<Lock>);
impl Drop for Guard { fn drop(&mut self) { release(self.0); } }

fn release(key: &'static LocalKey<Lock>) {
    key.with(|l| loop {
        let next = l.waiting.borrow_mut().pop_front();
        match next {
            // a waiter that gave up (its future was dropped) can't take it: try the next one
            Some(tx) => if tx.send(()).is_ok() { return },
            None => { l.busy.set(false); return; }
        }
    });
}

/// A place in the queue. Dropped after the lock was handed over but before it was taken, it passes it on.
struct Waiting {
    key: &'static LocalKey<Lock>,
    rx: Option<futures::channel::oneshot::Receiver<()>>,
}
impl Drop for Waiting {
    fn drop(&mut self) {
        if let Some(mut rx) = self.rx.take() {
            if let Ok(Some(())) = rx.try_recv() { release(self.key); }
        }
    }
}

/// Wait for the lock behind `key`.
pub async fn acquire(key: &'static LocalKey<Lock>) -> Guard {
    loop {
        let rx = key.with(|l| {
            if !l.busy.get() {
                l.busy.set(true);
                return None;
            }
            let (tx, rx) = futures::channel::oneshot::channel();
            l.waiting.borrow_mut().push_back(tx);
            Some(rx)
        });
        let Some(rx) = rx else { return Guard(key) };
        let mut w = Waiting { key, rx: Some(rx) };
        let got = match w.rx.as_mut() { Some(rx) => rx.await.is_ok(), None => false };
        w.rx = None;
        if got { return Guard(key); }
    }
}

// ---- the lock across tabs ------------------------------------------------------------------------

/// The Web Locks lock the index and the outbox are changed under, in every tab of this site.
pub const WEB_LOCK: &str = "encore-offline";
/// Waiting longer than this for another tab gives up on its lock and goes on without it: a tab the browser
/// froze while it held the lock must not stop this one for good.
pub const WEB_LOCK_WAIT_MS: u32 = 10_000;

/// Holding a Web Locks lock (`navigator.locks`); dropping it lets the other tabs in.
pub struct WebLock(Option<js_sys::Function>);
impl Drop for WebLock {
    fn drop(&mut self) {
        if let Some(release) = self.0.take() { let _ = release.call0(&JsValue::NULL); }
    }
}

/// Take the lock `name` that every tab of this site shares (`navigator.locks.request`), waiting at most
/// `wait_ms`. None where the browser has no Web Locks (before Chrome 69, Firefox 96, Safari 15.4) or after
/// waiting that long: the caller goes on with its in-page lock only.
pub async fn web_lock(name: &str, wait_ms: u32) -> Option<WebLock> {
    let nav = web_sys::window()?.navigator();
    let locks = js_sys::Reflect::get(&nav, &"locks".into()).ok().filter(|l| l.is_object())?;
    let request: js_sys::Function = js_sys::Reflect::get(&locks, &"request".into()).ok()?.dyn_into().ok()?;
    let (tx, rx) = futures::channel::oneshot::channel::<js_sys::Function>();
    // the browser calls this once the lock is ours and holds it until the promise it returns settles: the
    // WebLock keeps that promise's resolve function
    let granted = wasm_bindgen::closure::Closure::once_into_js(move |_lock: JsValue| -> JsValue {
        let mut resolve: Option<js_sys::Function> = None;
        let held = js_sys::Promise::new(&mut |res, _rej| { resolve = Some(res); });
        if let Some(r) = resolve {
            // nobody waits for it any more (gave up): let go at once
            if let Err(r) = tx.send(r) { let _ = r.call0(&JsValue::NULL); }
        }
        held.into()
    });
    if request.call2(&locks, &name.into(), &granted).is_err() { return None; }
    let got = Box::pin(rx);
    let late = Box::pin(gloo_timers::future::TimeoutFuture::new(wait_ms));
    match futures::future::select(got, late).await {
        futures::future::Either::Left((Ok(release), _)) => Some(WebLock(Some(release))),
        // the receiver goes with this future: a lock granted later is let go at once
        _ => None,
    }
}

/// Holding the lock of the index and the outbox: this page's, and the one across tabs where there is one.
pub struct StoreGuard {
    _tabs: Option<WebLock>,
    _page: Guard,
}

thread_local! {
    /// The lock for the index and the outbox documents.
    static STORE_LOCK: Lock = const { Lock::new() };
    /// The index as last read or written (None until the first read).
    static INDEX: RefCell<Option<Vec<Record>>> = const { RefCell::new(None) };
    static LOADING: Cell<bool> = const { Cell::new(false) };
    /// `caches.open` failed (Firefox's private windows): no downloads in this page.
    static BROKEN: Cell<bool> = const { Cell::new(false) };
    /// album id → the object URL of its saved cover (made once, revoked when the cover goes)
    static COVER_URLS: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());
}

/// The lock of the index and the outbox: everything that reads, changes and writes one of them holds it. The
/// page's lock comes first, so one request per page at most waits for the other tabs.
pub async fn lock() -> StoreGuard {
    let page = acquire(&STORE_LOCK).await;
    let tabs = web_lock(WEB_LOCK, WEB_LOCK_WAIT_MS).await;
    StoreGuard { _tabs: tabs, _page: page }
}

// ---- the Cache API -------------------------------------------------------------------------------

pub(crate) async fn wait(p: js_sys::Promise) -> Result<JsValue, JsValue> { wasm_bindgen_futures::JsFuture::from(p).await }

fn caches() -> Option<CacheStorage> {
    let w = web_sys::window()?;
    if !w.is_secure_context() { return None; }
    let c = js_sys::Reflect::get(&w, &"caches".into()).ok()?;
    if !c.is_object() { return None; }
    Some(c.unchecked_into())
}

/// This browser can keep downloads here: `window.caches` exists (a secure context) and opening it has not failed.
pub fn available() -> bool { !BROKEN.with(|b| b.get()) && caches().is_some() }

pub(crate) async fn open() -> Result<Cache, String> {
    let c = caches().ok_or_else(|| NO_STORE.to_string())?;
    match wait(c.open(CACHE_NAME)).await {
        Ok(v) => Ok(v.unchecked_into()),
        Err(e) => {
            BROKEN.with(|b| b.set(true));
            dom::console_error(&format!("offline store: {}", js_error(&e)));
            Err(NO_STORE.into())
        }
    }
}

/// The entry under `key` (Ok(None) when there is none).
pub(crate) async fn get(cache: &Cache, key: &str) -> Result<Option<Response>, String> {
    let r = wait(cache.match_with_str(key)).await.map_err(|e| js_error(&e))?;
    if r.is_undefined() || r.is_null() { return Ok(None); }
    Ok(Some(r.unchecked_into()))
}
pub(crate) async fn exists(cache: &Cache, key: &str) -> bool { matches!(get(cache, key).await, Ok(Some(_))) }

pub(crate) async fn read_text(cache: &Cache, key: &str) -> Option<String> {
    let r = get(cache, key).await.ok()??;
    wait(r.text().ok()?).await.ok()?.as_string()
}
pub(crate) async fn read_blob(cache: &Cache, key: &str) -> Result<Option<Blob>, String> {
    let Some(r) = get(cache, key).await? else { return Ok(None) };
    let p = r.blob().map_err(|e| js_error(&e))?;
    let b = wait(p).await.map_err(|e| js_error(&e))?;
    Ok(Some(b.unchecked_into()))
}

/// When an entry was written (unix seconds): the sweep leaves young files alone.
const WRITTEN_AT: &str = "x-encore-at";
/// A file without a record younger than this may belong to a download still running in another tab.
pub const SWEEP_AFTER_S: f64 = 3600.0;

fn response(body: ResponseBody<'_>, ctype: &str, bytes: Option<f64>) -> Result<Response, String> {
    let headers = web_sys::Headers::new().map_err(|e| js_error(&e))?;
    let _ = headers.set("content-type", ctype);
    let _ = headers.set(WRITTEN_AT, &(dom::now_s().floor() as i64).to_string());
    if let Some(n) = bytes { let _ = headers.set("x-encore-bytes", &crate::num::js_num(n)); }
    let init = web_sys::ResponseInit::new();
    init.set_status(200);
    init.set_headers(&headers);
    match body {
        ResponseBody::Text(t) => Response::new_with_opt_str_and_init(Some(t), &init),
        ResponseBody::Blob(b) => Response::new_with_opt_blob_and_init(Some(b), &init),
    }
    .map_err(|e| js_error(&e))
}
enum ResponseBody<'a> { Text(&'a str), Blob(&'a Blob) }

pub(crate) async fn put(cache: &Cache, key: &str, r: &Response) -> Result<(), String> {
    wait(cache.put_with_str(key, r)).await.map(|_| ()).map_err(|e| js_error(&e))
}
pub(crate) async fn write_text(cache: &Cache, key: &str, text: &str, ctype: &str) -> Result<(), String> {
    put(cache, key, &response(ResponseBody::Text(text), ctype, None)?).await
}
/// A synthetic answer with `content-type` and `x-encore-bytes`, made from a downloaded body.
pub(crate) async fn write_blob(cache: &Cache, key: &str, blob: &Blob, ctype: &str) -> Result<(), String> {
    put(cache, key, &response(ResponseBody::Blob(blob), ctype, Some(blob.size()))?).await
}
pub(crate) async fn delete(cache: &Cache, key: &str) { let _ = wait(cache.delete_with_str(key)).await; }

pub(crate) async fn read_index_from(cache: &Cache) -> Vec<Record> { read_text(cache, INDEX_KEY).await.map(|t| parse_index(&t)).unwrap_or_default() }
pub(crate) async fn write_index_to(cache: &Cache, v: &[Record]) -> Result<(), String> { write_text(cache, INDEX_KEY, &index_json(v), "application/json").await }

pub(crate) fn changed() { dom::dispatch(CHANGED_EVENT, None); }
fn set_index(v: Vec<Record>) { INDEX.with(|i| *i.borrow_mut() = Some(v)); }

// ---- the public API ------------------------------------------------------------------------------

/// The saved songs, oldest first (read once, then kept in memory; [`refresh`] reads again).
pub async fn index() -> Vec<Record> {
    if let Some(v) = INDEX.with(|i| i.borrow().clone()) { return v; }
    refresh().await
}

/// Read the index again (another tab may have changed it).
pub async fn refresh() -> Vec<Record> {
    let v = {
        let _g = lock().await;
        match open().await {
            Ok(c) => read_index_from(&c).await,
            Err(_) => vec![],
        }
    };
    set_index(v.clone());
    v
}

/// The first look at the in-memory index starts reading it, for code that can't wait (the player).
fn with_index<R>(f: impl FnOnce(&[Record]) -> R) -> R {
    if INDEX.with(|i| i.borrow().is_none()) && available() && !LOADING.with(|l| l.replace(true)) {
        dom::spawn(async { refresh().await; changed(); });
    }
    INDEX.with(|i| f(i.borrow().as_deref().unwrap_or(&[])))
}

/// This song is saved on this device (from the in-memory index).
pub fn has(id: i64) -> bool { with_index(|v| v.iter().any(|r| r.id == id)) }
pub fn record(id: i64) -> Option<Record> { with_index(|v| v.iter().find(|r| r.id == id).cloned()) }

/// An object URL of the saved audio, for the <audio> element (it seeks in it like in a file). The player
/// revokes it (`Url::revoke_object_url`) when it moves on. A record whose audio the browser has thrown away
/// is dropped from the index.
pub async fn audio_url(id: i64) -> Option<String> {
    let cache = open().await.ok()?;
    match read_blob(&cache, &audio_key(id)).await {
        Ok(Some(b)) => web_sys::Url::create_object_url_with_blob(&b).ok(),
        Ok(None) => {
            if has(id) { let _ = remove(&[id]).await; }
            None
        }
        Err(e) => { dom::console_error(&format!("offline audio {id}: {e}")); None }
    }
}

/// An object URL of the song's saved album cover (one per album, kept for the page's life).
pub async fn cover_url(id: i64) -> Option<String> {
    let r = match record(id) { Some(r) => r, None => { index().await; record(id)? } };
    if !r.cover || r.album_id.is_empty() { return None; }
    if let Some(u) = COVER_URLS.with(|m| m.borrow().get(&r.album_id).cloned()) { return Some(u); }
    let cache = open().await.ok()?;
    let b = read_blob(&cache, &cover_key(&r.album_id)).await.ok()??;
    let url = web_sys::Url::create_object_url_with_blob(&b).ok()?;
    // two calls at once: keep the first URL, drop ours
    let kept = COVER_URLS.with(|m| m.borrow_mut().entry(r.album_id.clone()).or_insert_with(|| url.clone()).clone());
    if kept != url { let _ = web_sys::Url::revoke_object_url(&url); }
    Some(kept)
}

/// The saved lyrics: the server's `/lyrics` answer as JSON text.
pub async fn lyrics(id: i64) -> Option<String> {
    let cache = open().await.ok()?;
    read_text(&cache, &lyrics_key(id)).await
}

fn forget_cover_urls(album_ids: Option<&[String]>) {
    COVER_URLS.with(|m| {
        let mut m = m.borrow_mut();
        let gone: Vec<String> = match album_ids { Some(a) => a.to_vec(), None => m.keys().cloned().collect() };
        for a in gone {
            if let Some(u) = m.remove(&a) { let _ = web_sys::Url::revoke_object_url(&u); }
        }
    });
}

/// Delete these songs: their audio and lyrics, and the covers no other saved song uses.
pub async fn remove(ids: &[i64]) -> Result<(), String> {
    if ids.is_empty() { return Ok(()); }
    let _g = lock().await;
    let cache = open().await?;
    let all = read_index_from(&cache).await;
    let (gone, kept): (Vec<Record>, Vec<Record>) = all.into_iter().partition(|r| ids.contains(&r.id));
    let covers = unused_covers(&gone, &kept);
    // the files first: on a full device the index can only be written once they have made room (a record whose
    // audio is gone drops out by itself, see audio_url)
    for id in ids {
        delete(&cache, &audio_key(*id)).await;
        delete(&cache, &lyrics_key(*id)).await;
    }
    for a in &covers { delete(&cache, &cover_key(a)).await; }
    forget_cover_urls(Some(&covers));
    let written = write_index_to(&cache, &kept).await;
    set_index(kept);
    changed();
    written
}

/// Delete every saved song (the outbox stays: its plays and likes still go to the server).
pub async fn remove_all() -> Result<(), String> {
    let _g = lock().await;
    let cache = open().await?;
    // the list goes by deleting it, which needs no room on a full device (writing an empty one does)
    wait(cache.delete_with_str(INDEX_KEY)).await.map_err(|e| js_error(&e))?;
    set_index(vec![]);
    let keys = wait(cache.keys()).await.map_err(|e| js_error(&e))?;
    for req in js_sys::Array::from(&keys).iter() {
        let Some(req) = req.dyn_ref::<web_sys::Request>() else { continue };
        let url = req.url();
        if url.ends_with(OUTBOX_KEY) || url.ends_with(INDEX_KEY) { continue; }
        let _ = wait(cache.delete_with_request(req)).await;
    }
    forget_cover_urls(None);
    changed();
    Ok(())
}

fn storage() -> Option<StorageManager> {
    let n = web_sys::window()?.navigator();
    let s = js_sys::Reflect::get(&n, &"storage".into()).ok()?;
    if !s.is_object() { return None; }
    Some(s.unchecked_into())
}

/// `navigator.storage.estimate()`: (bytes used, bytes this site may use).
pub async fn estimate() -> Option<(f64, f64)> {
    let e: StorageEstimate = wait(storage()?.estimate().ok()?).await.ok()?.unchecked_into();
    Some((e.get_usage().unwrap_or(0.0), e.get_quota()?))
}
/// `navigator.storage.persisted()`: the browser won't clear the downloads when space runs low.
pub async fn persisted() -> Option<bool> { wait(storage()?.persisted().ok()?).await.ok()?.as_bool() }
/// `navigator.storage.persist()`: ask for that (false when the browser says no).
pub async fn persist() -> bool {
    let Some(s) = storage() else { return false };
    let Ok(p) = s.persist() else { return false };
    wait(p).await.ok().and_then(|v| v.as_bool()).unwrap_or(false)
}

/// Add a downloaded song to the index, under the lock – only while its audio is still there (a removal
/// while it was downloading wins).
pub(crate) async fn add_record(cache: &Cache, rec: Record) -> Result<(), String> {
    let _g = lock().await;
    if !exists(cache, &audio_key(rec.id)).await { return Err("It was removed while it was downloading".into()); }
    let mut v = read_index_from(cache).await;
    upsert(&mut v, rec);
    write_index_to(cache, &v).await?;
    set_index(v);
    changed();
    Ok(())
}

/// What a [`sweep`] deletes, from the paths of the cache's entries: the audio and lyrics of songs that have no
/// record and are not being downloaded here (`busy`), and the covers no record uses.
pub fn leftovers(paths: &[String], index: &[Record], busy: &HashSet<i64>) -> Vec<String> {
    let ids: HashSet<i64> = index.iter().map(|r| r.id).collect();
    let covers: HashSet<&str> = index.iter().filter(|r| r.cover).map(|r| r.album_id.as_str()).collect();
    paths.iter().filter(|p| {
        if let Some(id) = p.strip_prefix("/offline/audio/").or_else(|| p.strip_prefix("/offline/lyrics/")) {
            return id.parse::<i64>().map_or(true, |id| !ids.contains(&id) && !busy.contains(&id));
        }
        p.strip_prefix("/offline/cover/").is_some_and(|a| !covers.contains(a))
    }).cloned().collect()
}

/// Delete what downloads left behind without a record (a tab closed or an app killed in the middle of one),
/// which would take room and never show in the list. Entries written in the last hour stay: another tab may
/// still be saving them. `busy`: the songs this page is downloading. Gives the number deleted.
pub async fn sweep(busy: &HashSet<i64>) -> usize {
    let _g = lock().await;
    let Ok(cache) = open().await else { return 0 };
    let index = read_index_from(&cache).await;
    let Ok(keys) = wait(cache.keys()).await else { return 0 };
    let paths: Vec<String> = js_sys::Array::from(&keys).iter()
        .filter_map(|r| r.dyn_ref::<web_sys::Request>().and_then(|r| web_sys::Url::new(&r.url()).ok()).map(|u| u.pathname()))
        .collect();
    let now = dom::now_s();
    let mut n = 0;
    for p in leftovers(&paths, &index, busy) {
        let at = match get(&cache, &p).await {
            Ok(Some(r)) => r.headers().get(WRITTEN_AT).ok().flatten().and_then(|t| t.parse::<f64>().ok()),
            _ => None,
        };
        if at.is_some_and(|t| now - t < SWEEP_AFTER_S) { continue; }
        delete(&cache, &p).await;
        n += 1;
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rec(id: i64, album_id: &str, cover: bool) -> Record {
        Record { id, album_id: album_id.into(), cover, title: format!("Song {id}"), ext: "flac".into(), format: "flac".into(), quality: "FLAC 16/44.1".into(), ..Record::default() }
    }

    #[test]
    fn a_record_round_trips_in_the_contract_shape() {
        let r = Record {
            id: 42, title: "Song".into(), artist: "Band".into(), album: "Album".into(), album_id: "0123456789abcdef".into(),
            ext: "flac".into(), bytes: 31_457_280, quality: "FLAC 16/44.1".into(), cover: true, lyrics: true,
            saved_at: 1_760_000_000, source_server: "https://music.example.org".into(), duration: 245.5, track: 3, disc: 1,
            format: "flac".into(), artists: vec![json!({ "name": "Band", "id": 7 })],
        };
        let v = serde_json::to_value(&r).unwrap();
        for k in ["id", "title", "artist", "album", "album_id", "ext", "bytes", "quality", "cover", "lyrics", "saved_at", "source_server",
            "duration", "track", "disc", "format", "artists"] {
            assert!(v.get(k).is_some(), "{k} missing in {v}");
        }
        assert_eq!(v["id"], json!(42), "the id is a number");
        assert_eq!(serde_json::from_value::<Record>(v).unwrap(), r);
        assert!(!r.converted());
    }

    #[test]
    fn the_desktop_apps_older_shape_still_reads() {
        let old = r#"[{"id":"7","title":"Old","artist":"A","album":"B","ext":"FLAC","file":"7-abcd1234.flac","art":"cover-x-abcd1234.jpg"},
                      {"id":"8","title":"No art","ext":"mp3","file":"8-abcd1234.mp3","art":""},
                      {"id":"9","title":"Desktop 0.27","ext":"flac","file":"9.flac","art":"","album_id":"ab12","bytes":1234,
                       "quality":"FLAC","saved_at":1760000000.25,"source_server":"http://nas:8080","duration":200.0}]"#;
        let v = parse_index(old);
        assert_eq!(v.len(), 3);
        assert_eq!((v[0].id, v[0].title.as_str(), v[0].cover, v[0].ext.as_str(), v[0].format.as_str(), v[0].quality.as_str()), (7, "Old", true, "flac", "flac", "FLAC"));
        assert!(!v[1].cover, "an empty art is no cover");
        assert_eq!(v[1].quality, "MP3");
        // the classic app had no ext on some records: the file's extension, else the format, else "audio"
        let classic = parse_index(r#"[{"id":"1","file":"asset://localhost/C%3A/Music/1-x.M4A","art":""},{"id":"2","file":"","format":"opus"},{"id":"3"}]"#);
        assert_eq!(classic.iter().map(|r| r.ext.as_str()).collect::<Vec<_>>(), vec!["m4a", "opus", "audio"]);
        assert_eq!(classic.iter().map(|r| r.format.as_str()).collect::<Vec<_>>(), vec!["m4a", "opus", "audio"]);
        assert_eq!(classic[0].quality, "M4A");
        assert_eq!((v[2].bytes, v[2].saved_at, v[2].album_id.as_str(), v[2].duration), (1234, 1_760_000_000, "ab12", 200.0));
    }

    #[test]
    fn reading_is_lenient_and_drops_what_has_no_id() {
        let v = parse_index(r#"[{"id":5,"title":1999,"bytes":"2048","cover":1,"track":"4","album_id":77},
                                {"title":"no id"},{"id":"x"},{"id":-1},{"id":2.5},"junk",
                                {"id":5,"title":"Newer"}]"#);
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(v[0].title, "Newer", "the last entry of an id wins");
        let r = Record::from_value(&json!({"id":5,"title":1999,"bytes":"2048","cover":1,"track":"4","album_id":77})).unwrap();
        assert_eq!((r.title.as_str(), r.bytes, r.cover, r.track, r.album_id.as_str()), ("1999", 2048, true, 4, "77"));
        assert_eq!(parse_index("{\"songs\":[{\"id\":3}]}")[0].id, 3);
        assert!(parse_index("").is_empty());
        assert!(parse_index("null").is_empty());
        assert!(serde_json::from_value::<Record>(json!({"title": "x"})).is_err());
        assert_eq!(parse_index(&index_json(&[rec(1, "a", true), rec(2, "", false)])), vec![rec(1, "a", true), rec(2, "", false)]);
    }

    #[test]
    fn converted_copies_are_marked_by_their_quality() {
        assert!(is_converted("AAC 256k (converted)"));
        assert!(is_converted("FLAC (converted) "));
        assert!(!is_converted("FLAC 16/44.1"));
        assert!(!is_converted(""));
    }

    #[test]
    fn upsert_replaces_the_same_song() {
        let mut v = vec![rec(1, "a", true), rec(2, "a", true)];
        upsert(&mut v, Record { title: "again".into(), ..rec(1, "a", true) });
        upsert(&mut v, rec(3, "b", false));
        assert_eq!(v.iter().map(|r| (r.id, r.title.as_str())).collect::<Vec<_>>(), vec![(1, "again"), (2, "Song 2"), (3, "Song 3")]);
    }

    #[test]
    fn a_cover_goes_only_when_no_saved_song_needs_it() {
        let removed = vec![rec(1, "a", true), rec(2, "b", true), rec(3, "b", true), rec(4, "", true), rec(5, "c", false)];
        let kept = vec![rec(6, "a", true), rec(7, "c", true)];
        assert_eq!(unused_covers(&removed, &kept), vec!["b".to_string()]);
        assert_eq!(unused_covers(&removed, &[]), vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn the_sweep_takes_only_what_no_record_needs() {
        let paths: Vec<String> = ["/offline/index.json", "/offline/outbox.json", "/offline/audio/1", "/offline/lyrics/1", "/offline/audio/2",
            "/offline/lyrics/3", "/offline/audio/4", "/offline/cover/a", "/offline/cover/b", "/offline/cover/c", "/offline/audio/x"]
            .iter().map(|s| s.to_string()).collect();
        // song 1 is saved with album a's cover; song 4 is downloading right now; c's song has no cover saved
        let index = vec![rec(1, "a", true), rec(5, "c", false)];
        let busy: HashSet<i64> = [4].into_iter().collect();
        assert_eq!(leftovers(&paths, &index, &busy),
            vec!["/offline/audio/2", "/offline/lyrics/3", "/offline/cover/b", "/offline/cover/c", "/offline/audio/x"]);
        assert!(leftovers(&paths[..2], &[], &HashSet::new()).is_empty(), "the two documents always stay");
    }

    #[test]
    fn keys_and_errors() {
        assert_eq!(audio_key(12), "/offline/audio/12");
        assert_eq!(lyrics_key(12), "/offline/lyrics/12");
        assert_eq!(cover_key("../ab/c?d"), "/offline/cover/abcd");
        assert_eq!(error_words("QuotaExceededError", "The quota has been exceeded."), QUOTA_FULL);
        assert_eq!(error_words("UnknownError", "Quota exceeded"), QUOTA_FULL);
        assert_eq!(error_words("TypeError", "Failed to fetch"), "Failed to fetch");
        assert_eq!(error_words("AbortError", ""), "The download was stopped");
        assert_eq!(error_words("", ""), "Something went wrong");
    }

    // ---- the lock, driven by a single-threaded executor like the browser's ----

    use futures::task::LocalSpawnExt;
    use std::future::Future;
    use std::rc::Rc;

    thread_local! {
        static TEST_LOCK: Lock = const { Lock::new() };
        static OTHER_LOCK: Lock = const { Lock::new() };
    }

    /// Give the other tasks a turn.
    async fn yield_now() {
        let mut first = true;
        futures::future::poll_fn(move |cx| {
            if first { first = false; cx.waker().wake_by_ref(); std::task::Poll::Pending } else { std::task::Poll::Ready(()) }
        })
        .await
    }

    #[test]
    fn the_lock_lets_one_in_at_a_time_in_order() {
        let mut pool = futures::executor::LocalPool::new();
        let log = Rc::new(RefCell::new(Vec::<String>::new()));
        for i in 0..4 {
            let log = log.clone();
            pool.spawner().spawn_local(async move {
                let _g = acquire(&TEST_LOCK).await;
                log.borrow_mut().push(format!("in {i}"));
                yield_now().await;
                yield_now().await;
                log.borrow_mut().push(format!("out {i}"));
            }).unwrap();
        }
        pool.run();
        let want: Vec<String> = (0..4).flat_map(|i| [format!("in {i}"), format!("out {i}")]).collect();
        assert_eq!(*log.borrow(), want);
        assert!(!TEST_LOCK.with(|l| l.held()), "free again");
    }

    #[test]
    fn a_waiter_that_gives_up_does_not_keep_the_lock() {
        let mut pool = futures::executor::LocalPool::new();
        let first = pool.run_until(acquire(&OTHER_LOCK));
        // a second one queues, is polled once, and is dropped after the lock was handed to it
        let mut second = Box::pin(acquire(&OTHER_LOCK));
        let waker = futures::task::noop_waker();
        let mut cx = std::task::Context::from_waker(&waker);
        assert!(second.as_mut().poll(&mut cx).is_pending());
        drop(first);
        drop(second);
        assert!(!OTHER_LOCK.with(|l| l.held()), "passed on, not lost");
        let again = pool.run_until(acquire(&OTHER_LOCK));
        drop(again);
        assert!(!OTHER_LOCK.with(|l| l.held()));
    }
}
