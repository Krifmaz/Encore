//! Port of web/js/player.js – see docs/ui-rewrite/CONTRACT.md
//! Web player: streams the original file (bit-perfect for MP3/FLAC/AAC/Opus); formats a browser
//! can't decode (ALAC/APE/WavPack/AIFF/WMA) are converted to FLAC on the fly by the server.
//! Tap the bar for the full-screen player (lyrics + Up next). When the queue runs out it keeps going
//! with a shuffle of the whole library: a single song → straight into the shuffle, an album → after its last song.

use crate::api::{self, ApiError};
use crate::app;
use crate::dom::{self, body, by_id, document, qs_in, qsa_in, set_html, EventListener, Interval, Timeout};
use crate::json::{self, js_string, J};
use crate::ui::{art, esc, esck, fmt, icon, icon_attr, toast};
use gloo_events::EventListenerOptions;
use serde_json::{json, Map, Value};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{Element, Event, HtmlElement, HtmlMediaElement, KeyboardEvent, ScrollBehavior, ScrollToOptions};

/// `link = { id, name, state, online, send(cmd) }`: this device is locked as a remote for another one.
#[derive(Clone)]
pub struct Link { pub id: String, pub name: String, pub state: Value, pub online: bool, pub send: Rc<dyn Fn(Value)> }

/// `const state = {...}`. The queue holds `Rc`s so `state.announced !== t` keeps its object identity.
struct State {
    queue: Vec<Rc<Value>>,
    index: i64,
    transcoded: bool,
    offset: f64,
    dragging: bool,
    context: String,
    repeat: String,
    full: bool,
    pane: String,
    logged: bool,
    announced: Option<Rc<Value>>,
}
impl Default for State {
    fn default() -> Self {
        Self { queue: vec![], index: -1, transcoded: false, offset: 0.0, dragging: false, context: "list".into(), repeat: "off".into(), full: false, pane: "now".into(), logged: false, announced: None }
    }
}

struct Line { t: f64, text: String }
/// `lyr = { id, lines, plain, loading, instrumental }`.
struct Lyr { id: Value, lines: Vec<Line>, plain: String, loading: bool, instrumental: bool }
impl Default for Lyr {
    fn default() -> Self { Self { id: Value::Null, lines: vec![], plain: String::new(), loading: false, instrumental: false } }
}

type Listener = Rc<dyn Fn(Option<Value>)>;

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
    static LISTENERS: RefCell<Vec<(u64, Listener)>> = const { RefCell::new(Vec::new()) };
    static LISTENER_N: Cell<u64> = const { Cell::new(0) };
    static AUDIO: RefCell<Option<HtmlMediaElement>> = const { RefCell::new(None) };
    /// second <audio> that pre-buffers the next song, so skipping is instant
    static SPARE: RefCell<Option<HtmlMediaElement>> = const { RefCell::new(None) };
    static LYR: RefCell<Lyr> = RefCell::new(Lyr::default());
    /// Songs heard this listening session (this tab): shuffles never repeat one until the whole library has played.
    static PLAYED: RefCell<Vec<i64>> = RefCell::new(load_played());
    static LINK: RefCell<Option<Link>> = const { RefCell::new(None) };
    /// true while running a command another device sent us (never forward those)
    static LOCAL: Cell<bool> = const { Cell::new(false) };
    static LINK_TICK: RefCell<Option<Interval>> = const { RefCell::new(None) };
    static TOPPING: Cell<bool> = const { Cell::new(false) };
    static LAST_LINE: Cell<i64> = const { Cell::new(-1) };
    /// the `onclick = …` handlers of the bar / the full-screen view: replaced on every render, like the elements
    static BAR: RefCell<Vec<EventListener>> = const { RefCell::new(Vec::new()) };
    static FULL: RefCell<Vec<EventListener>> = const { RefCell::new(Vec::new()) };
    static EXPOSED: Cell<bool> = const { Cell::new(false) };
}

fn st<R>(f: impl FnOnce(&State) -> R) -> R { STATE.with(|s| f(&s.borrow())) }
fn st_mut<R>(f: impl FnOnce(&mut State) -> R) -> R { STATE.with(|s| f(&mut s.borrow_mut())) }
fn audio() -> Option<HtmlMediaElement> { AUDIO.with(|a| a.borrow().clone()) }
fn spare() -> Option<HtmlMediaElement> { SPARE.with(|a| a.borrow().clone()) }
/// `state.queue[state.index] || null` (the queue entry itself, for identity checks).
fn cur() -> Option<Rc<Value>> { st(|s| if s.index < 0 { None } else { s.queue.get(s.index as usize).cloned() }) }
fn is_linked() -> bool { LINK.with(|l| l.borrow().is_some()) }
fn is_local() -> bool { LOCAL.with(|l| l.get()) }
/// `${n}` of a JS number ("0.9", "1", "52.3").
fn js_num(v: f64) -> String { v.to_string() }
/// `Number("…")` of an input value.
fn num(s: &str) -> f64 { s.trim().parse().unwrap_or(0.0) }
/// JS truthiness of a number.
fn truthy_num(x: f64) -> bool { x != 0.0 && !x.is_nan() }
fn session_storage() -> Option<web_sys::Storage> { dom::window().session_storage().ok().flatten() }
/// `audio.play().catch(() => {})`.
fn play(a: &HtmlMediaElement) {
    if let Ok(p) = a.play() { dom::spawn(async move { let _ = wasm_bindgen_futures::JsFuture::from(p).await; }); }
}
/// `new URL(u, location.href).href`.
fn abs_url(u: &str) -> String {
    let base = dom::location().href().unwrap_or_default();
    web_sys::Url::new_with_base(u, &base).map(|x| x.href()).unwrap_or_default()
}
/// `(x).toFixed(1)` – exact decimal expansion, ties (k.25 / k.75) rounded up like JS.
fn to_fixed1(x: f64) -> String {
    let q = x * 4.0;
    if x.is_finite() && q.fract() == 0.0 && (q as i64).rem_euclid(2) == 1 {
        let n = (x * 10.0).round() as i64;
        format!("{}.{}", n / 10, (n % 10).abs())
    } else {
        format!("{:.1}", x)
    }
}
fn set_bar(ls: Vec<EventListener>) { let old = BAR.with(|b| std::mem::replace(&mut *b.borrow_mut(), ls)); drop(old); }
fn set_full_listeners(ls: Vec<EventListener>) { let old = FULL.with(|b| std::mem::replace(&mut *b.borrow_mut(), ls)); drop(old); }
/// `root.querySelector(sel).on<ev> = f`.
fn wire(out: &mut Vec<EventListener>, root: &Element, sel: &str, ev: &str, f: impl FnMut(&Event) + 'static) {
    if let Some(el) = qs_in(root, sel) { out.push(dom::listener(&el, ev, f)); }
}
/// `seek.style.setProperty('--p', `${seek.value / 10}%`)`.
fn set_p(seek: &Element) { dom::set_style(seek, "--p", &format!("{}%", js_num(num(&dom::value(seek)) / 10.0))); }
/// `t.album || t.title` (the label `art` makes the placeholder letter from).
fn album_or_title(t: &Value) -> String { if t.truthy("album") { t.s("album") } else { t.s("title") } }
/// `!!window.encoreRemote`.
fn has_encore_remote() -> bool { js_sys::Reflect::get(&dom::window(), &"encoreRemote".into()).map(|v| v.is_truthy()).unwrap_or(false) }
/// `window.encoreRemote?.open()`.
fn remote_open() { if has_encore_remote() { crate::remote::open(); } }
/// `window.encoreRemote?.unlink()`.
fn remote_unlink() { if has_encore_remote() { crate::remote::unlink(); } }
/// `'mediaSession' in navigator`.
fn has_media_session() -> bool { js_sys::Reflect::has(&dom::window().navigator(), &"mediaSession".into()).unwrap_or(false) }
/// `navigator.mediaSession` – untyped, because web-sys gates the Media Session API behind `web_sys_unstable_apis`.
fn media_session() -> Option<JsValue> {
    if !has_media_session() { return None; }
    js_sys::Reflect::get(&dom::window().navigator(), &"mediaSession".into()).ok().filter(|v| !v.is_undefined() && !v.is_null())
}
fn js_set(o: &JsValue, key: &str, v: &JsValue) { let _ = js_sys::Reflect::set(o, &key.into(), v); }
/// `navigator.mediaSession.metadata = new MediaMetadata({ title, artist, album, artwork })`.
fn set_media_metadata(t: &Value) {
    let Some(ms) = media_session() else { return };
    let init = js_sys::Object::new();
    js_set(&init, "title", &t.s("title").into());
    js_set(&init, "artist", &t.s("artist").into());
    js_set(&init, "album", &t.s("album").into());
    let artwork = js_sys::Array::new();
    let cov = cover(t);
    if !cov.is_empty() {
        let img = js_sys::Object::new();
        js_set(&img, "src", &cov.into());
        js_set(&img, "sizes", &"512x512".into());
        artwork.push(&img);
    }
    js_set(&init, "artwork", &artwork);
    let Ok(ctor) = js_sys::Reflect::get(&js_sys::global(), &"MediaMetadata".into()).and_then(|c| c.dyn_into::<js_sys::Function>()) else { return };
    if let Ok(md) = js_sys::Reflect::construct(&ctor, &js_sys::Array::of1(&init)) { js_set(&ms, "metadata", &md); }
}

// ---- played this session ------------------------------------------------------------------------

fn load_played() -> Vec<i64> {
    let raw = session_storage().and_then(|s| s.get_item("encore.played").ok().flatten()).unwrap_or_default();
    let raw = if raw.is_empty() { "[]".to_string() } else { raw };
    serde_json::from_str::<Value>(&raw).map(|v| v.items().iter().filter_map(|x| x.as_i64()).collect()).unwrap_or_default()
}
fn mark_played(t: &Value) {
    if !t.g("id").is_number() { return; }
    let id = t.i("id");
    let list = PLAYED.with(|p| {
        let mut p = p.borrow_mut();
        if p.contains(&id) { return None; }
        p.push(id);
        Some(p.clone())
    });
    if let (Some(list), Some(s)) = (list, session_storage()) {
        let _ = s.set_item("encore.played", &serde_json::to_string(&list).unwrap_or_else(|_| "[]".into()));
    }
}
/// Random library songs not yet heard (or queued) this session; when everything has played, start a fresh round.
async fn fresh_shuffle(limit: usize) -> Result<Vec<Value>, ApiError> {
    let queued: Vec<i64> = st(|s| s.queue.iter().filter(|t| t.g("id").is_number()).map(|t| t.i("id")).collect());
    let mut exclude: Vec<i64> = PLAYED.with(|p| p.borrow().clone());
    for id in queued { if !exclude.contains(&id) { exclude.push(id); } }
    let mut more = api::post("/library/shuffle", json!({ "limit": limit, "exclude": exclude })).await?.items();
    if more.is_empty() {
        PLAYED.with(|p| p.borrow_mut().clear());
        if let Some(s) = session_storage() { let _ = s.remove_item("encore.played"); }
        let cur_id: Vec<i64> = cur().filter(|t| t.g("id").is_number()).map(|t| vec![t.i("id")]).unwrap_or_default();
        more = api::post("/library/shuffle", json!({ "limit": limit, "exclude": cur_id })).await?.items();
    }
    Ok(more)
}
/// The Shuffle button: play the whole library in random order, no song twice this session.
pub async fn shuffle_library() -> usize {
    match fresh_shuffle(50).await {
        Ok(first) => {
            let n = first.len();
            if n > 0 { play_queue(first, 0, "library"); }
            n
        }
        // the JS let the error escape the caller's async handler (nothing shown but a console error);
        // the contract only carries a count, so answer "not empty" to keep the caller's toast away
        Err(e) => { dom::console_error(&e.message); 1 }
    }
}

// ---- listeners / native apps ---------------------------------------------------------------------

/// `onChange(fn)` – dropping the handle unsubscribes.
pub struct OnChange(u64);
impl Drop for OnChange {
    fn drop(&mut self) { let id = self.0; LISTENERS.with(|l| l.borrow_mut().retain(|(i, _)| *i != id)); }
}
pub fn on_change(f: impl Fn(Option<Value>) + 'static) -> OnChange {
    let id = LISTENER_N.with(|n| { n.set(n.get() + 1); n.get() });
    LISTENERS.with(|l| l.borrow_mut().push((id, Rc::new(f))));
    OnChange(id)
}
fn emit() {
    let fns: Vec<Listener> = LISTENERS.with(|l| l.borrow().iter().map(|(_, f)| f.clone()).collect());
    let c = current();
    for f in fns { f(c.clone()); }
    native();
}

/// Android / desktop apps: tell them what's playing (notification + lock screen + tray), and let them control us.
fn native() {
    if !app::native_has("nowPlaying") { return; }
    let Some(a) = audio() else { return };
    let payload = match cur() {
        Some(t) => {
            let cov = cover(&t);
            let art_url = if cov.is_empty() { String::new() } else { abs_url(&cov) };
            json!({ "title": t.s("title"), "artist": t.s("artist"), "album": t.s("album"), "playing": !a.paused(), "art": art_url })
        }
        None => json!({ "stopped": true }),
    };
    app::native_call_with("nowPlaying", Some(&payload.to_string()));
}
/// `window.encorePlayer = { toggle, next, prev, open }`.
fn expose_global() {
    if EXPOSED.with(|e| e.replace(true)) { return; }
    let obj = js_sys::Object::new();
    let put = |name: &str, f: Box<dyn Fn()>| {
        let c = Closure::<dyn Fn()>::new(move || f());
        let _ = js_sys::Reflect::set(&obj, &name.into(), c.as_ref());
        c.forget();
    };
    put("toggle", Box::new(toggle));
    put("next", Box::new(|| next(false)));
    put("prev", Box::new(prev));
    put("open", Box::new(|| set_full(true)));
    let _ = js_sys::Reflect::set(&dom::window(), &"encorePlayer".into(), &obj);
}
pub fn current() -> Option<Value> { cur().map(|t| (*t).clone()) }

// ---- linked: this device is locked as a remote for another one (Encore 2) ----
// Everything you play or press here goes to that device; nothing plays locally until you unlink.
fn via_link(c: Value) -> bool {
    if is_local() { return false; }
    let send = LINK.with(|l| l.borrow().as_ref().map(|l| l.send.clone()));
    let Some(send) = send else { return false };
    send(c);
    true
}
pub fn linked() -> Option<Link> { LINK.with(|l| l.borrow().clone()) }
pub fn set_link(l: Option<Link>) {
    let was = is_linked();
    let now = l.is_some();
    let old = LINK.with(|x| x.replace(l));
    drop(old);
    if now && !was {
        if let Some(a) = audio() { if !a.paused() { let _ = a.pause(); } }
    }
    let old_tick = LINK_TICK.with(|t| t.borrow_mut().take());
    drop(old_tick);
    // between check-ins, move the remote's seek bar along on our own
    if now {
        let tick = Interval::new(1000, || {
            let playing = LINK.with(|l| l.borrow().as_ref().is_some_and(|l| l.state.truthy("playing")));
            if playing {
                LINK.with(|l| {
                    if let Some(l) = l.borrow_mut().as_mut() {
                        let p = l.state.f("position") + 1.0;
                        if let Some(o) = l.state.as_object_mut() { o.insert("position".into(), json!(p)); }
                    }
                });
                update_linked_time();
            }
        });
        LINK_TICK.with(|t| *t.borrow_mut() = Some(tick));
    }
    if st(|s| s.full) { set_full(false); }
    render();
}
/// New state from the linked device (every check-in).
pub fn link_state(s: Value, online: bool) {
    let changed = LINK.with(|l| {
        let mut guard = l.borrow_mut();
        let l = guard.as_mut()?;
        let before = (l.state.g("id").clone(), l.state.g("playing").clone(), l.online);
        l.state = if json::truthy(&s) { s } else { json!({}) };
        l.online = online;
        Some(before != (l.state.g("id").clone(), l.state.g("playing").clone(), l.online))
    });
    match changed {
        None => {}
        Some(true) => render(),
        Some(false) => update_linked_time(),
    }
}

// ---- remote control (Encore 2): what this device is playing, and commands from your other devices ----
const SLIM_KEYS: [&str; 7] = ["id", "title", "artist", "album", "album_id", "format", "duration"];
fn slim(t: &Value) -> Value {
    let mut m = Map::new();
    for k in SLIM_KEYS { if t.has(k) { m.insert(k.into(), t.g(k).clone()); } }
    Value::Object(m)
}
pub fn snapshot() -> Value {
    let (Some(t), Some(a)) = (cur(), audio()) else { return json!({ "playing": false }) };
    let mut m = match slim(&t) { Value::Object(m) => m, _ => Map::new() };
    // `cover: cover(t)` – an undefined cover is left out, like JSON.stringify does
    let cov = cover_v(&t);
    if t.truthy("preview") || !t.truthy("album_id") { if t.has("cover") { m.insert("cover".into(), cov); } } else { m.insert("cover".into(), cov); }
    m.insert("playing".into(), json!(!a.paused()));
    m.insert("position".into(), json!(position()));
    m.insert("duration".into(), json!(duration()));
    m.insert("volume".into(), json!(a.volume()));
    m.insert("index".into(), json!(st(|s| s.index)));
    let q: Vec<Value> = st(|s| s.queue.iter().filter(|x| x.g("id").is_number()).take(300).map(|x| slim(x)).collect());
    m.insert("queue".into(), Value::Array(q));
    Value::Object(m)
}
pub fn set_volume(v: f64) {
    if via_link(json!({ "action": "volume", "value": v })) {
        LINK.with(|l| if let Some(l) = l.borrow_mut().as_mut() { if let Some(o) = l.state.as_object_mut() { o.insert("volume".into(), json!(v)); } });
        return;
    }
    let (Some(a), Some(sp)) = (audio(), spare()) else { return };
    let vol = 0f64.max(1f64.min(v));
    a.set_volume(vol);
    sp.set_volume(vol);
    dom::ls_set("encore.volume", &js_num(a.volume()));
    if let Some(el) = by_id("vol") { dom::set_value(&el, &js_num(a.volume())); }
}
pub fn command(c: Value) {
    LOCAL.with(|l| l.set(true));
    run_command(&c);
    LOCAL.with(|l| l.set(false));
}
fn run_command(c: &Value) {
    match c.s("action").as_str() {
        "toggle" => toggle(),
        "play" => { if cur().is_some() { if let Some(a) = audio() { play(&a); } } }
        "pause" | "stop" => { if let Some(a) = audio() { let _ = a.pause(); } }
        "next" => next(false),
        "prev" => prev(),
        "seek" => seek_to(c.f("value")),
        "volume" => set_volume(c.f("value")),
        "enqueue" => { if c.g("tracks").is_array() { enqueue(c.a("tracks")); } }
        "play_next" => { if c.g("tracks").is_array() { play_next(c.a("tracks")); } }
        "play_queue" => {
            if !c.g("tracks").is_array() || c.a("tracks").is_empty() { return; }
            play_queue(c.a("tracks"), c.i("index").max(0) as usize, "list");
            if c.f("position") > 1.0 {
                let at = c.f("position");
                if let Some(a) = audio() { EventListener::once(&a, "loadedmetadata", move |_| seek_to(at)).forget(); }
            }
        }
        _ => {}
    }
}
pub fn queue() -> (Vec<Value>, usize) {
    st(|s| (s.queue.iter().map(|t| (**t).clone()).collect(), s.index.max(0) as usize))
}

/// `cover(t)` as a JSON value (undefined → Null).
fn cover_v(t: &Value) -> Value {
    if t.truthy("preview") { t.g("cover").clone() }
    else if t.truthy("album_id") { Value::String(format!("/api/library/albums/{}/cover", t.s("album_id"))) }
    else { t.g("cover").clone() }
}
/// `cover(t)` as used in strings ("" when falsy).
fn cover(t: &Value) -> String { let v = cover_v(t); if json::truthy(&v) { js_string(&v) } else { String::new() } }

/// context: 'album' | 'single' | 'list' – decides what happens when the queue ends.
fn sendable(tracks: &[Value]) -> Vec<Value> { tracks.iter().filter(|t| json::truthy(t) && t.g("id").is_number()).map(slim).collect() }
pub fn play_queue(tracks: Vec<Value>, start: usize, context: &str) {
    if is_linked() && !is_local() && context != "preview" {
        let list: Vec<Value> = tracks.into_iter().filter(json::truthy).collect();
        let at = (start as i64).min(list.len() as i64 - 1).max(0);
        let first_id = list.get(at as usize).map(|f| f.g("id").clone());
        let out = sendable(&list);
        if out.is_empty() { toast("Only songs from your library can be played on another device", "err"); return; }
        let index = first_id.and_then(|fid| out.iter().position(|t| *t.g("id") == fid)).map(|i| i as i64).unwrap_or(-1).max(0);
        via_link(json!({ "action": "play_queue", "tracks": out, "index": index }));
        return;
    }
    st_mut(|s| {
        s.queue = tracks.into_iter().filter(json::truthy).map(Rc::new).collect();
        s.index = (start as i64).min(s.queue.len() as i64 - 1).max(0);
        s.context = context.to_string();
    });
    load(false, 0.0);
}

pub fn enqueue(tracks: Vec<Value>) {
    if is_linked() && !is_local() {
        let out = sendable(&tracks);
        if !out.is_empty() { via_link(json!({ "action": "enqueue", "tracks": out })); }
        return;
    }
    st_mut(|s| s.queue.extend(tracks.into_iter().map(Rc::new)));
    if st(|s| s.index < 0) { st_mut(|s| s.index = 0); load(false, 0.0); } else { render(); emit(); preload_next(); }
}

pub fn play_next(tracks: Vec<Value>) {
    if is_linked() && !is_local() {
        let out = sendable(&tracks);
        if !out.is_empty() { via_link(json!({ "action": "play_next", "tracks": out })); }
        return;
    }
    if st(|s| s.index < 0) { play_queue(tracks, 0, "list"); return; }
    st_mut(|s| {
        let at = ((s.index + 1) as usize).min(s.queue.len());
        s.queue.splice(at..at, tracks.into_iter().map(Rc::new));
    });
    render(); emit(); preload_next();
}

/// 30-second preview of a song that isn't in the library (Deezer/iTunes preview URL).
pub fn preview(t: Value) {
    let mut p = if t.is_object() { t.clone() } else { json!({}) };
    if let Some(o) = p.as_object_mut() {
        o.insert("preview".into(), json!(true));
        o.insert("id".into(), Value::String(format!("pv-{}", t.s("preview_url"))));
    }
    play_queue(vec![p], 0, "preview");
}

// formats browsers can't decode → the server converts to FLAC; the player then seeks via the server
const CONVERT: [&str; 10] = ["ape", "wv", "aiff", "aif", "wma", "alac", "dsf", "dff", "tta", "mpc"];

fn stream_url(t: &Value, transcode: bool, offset: f64) -> String {
    if t.truthy("preview") { return t.s("preview_url"); }
    let q = if transcode {
        format!("?transcode=1{}", if truthy_num(offset) { format!("&t={}", to_fixed1(offset)) } else { String::new() })
    } else { String::new() };
    format!("/api/library/tracks/{}/stream{q}", t.s("id"))
}
fn convert(t: &Value) -> bool { !t.truthy("preview") && CONVERT.contains(&t.s("format").to_lowercase().as_str()) }

/// Android app: keep songs on the phone so they play with no server.
pub fn can_save_offline() -> bool { app::native_has("saveOffline") }
pub fn save_offline(tracks: Vec<Value>) {
    for t in &tracks {
        if !t.g("id").is_number() { continue; }
        let conv = convert(t);
        let id = t.s("id");
        let ext = if conv { "flac".to_string() } else if t.truthy("format") { t.s("format") } else { "audio".to_string() };
        let url = abs_url(&format!("/api/library/tracks/{id}/stream{}", if conv { "?transcode=true" } else { "" }));
        let art_url = if t.truthy("album_id") { abs_url(&format!("/api/library/albums/{}/cover?s=600", t.s("album_id"))) } else { String::new() };
        app::native_call_with("saveOffline", Some(&json!({
            "id": id, "title": t.s("title"), "artist": t.s("artist"), "album": t.s("album"), "ext": ext,
            "url": url,
            "art": art_url,
        }).to_string()));
    }
}

fn load(transcode: bool, offset: f64) {
    let Some(t) = cur() else { return };
    mark_played(&t);
    let transcode = transcode || convert(&t);
    let transcoded = !t.truthy("preview") && transcode;
    st_mut(|s| {
        s.transcoded = transcoded;
        s.offset = offset;   // converted streams restart at the seek point, so their clock starts at `offset`
        s.dragging = false;
    });
    if !truthy_num(offset) {
        st_mut(|s| s.logged = false);
        // "now playing" on Last.fm / ListenBrainz (Encore 2)
        if t.g("id").is_number() && !st(|s| s.announced.as_ref().is_some_and(|a| Rc::ptr_eq(a, &t))) {
            st_mut(|s| s.announced = Some(t.clone()));
            let id = t.g("id").clone();
            dom::spawn(async move { let _ = api::post("/library/now-playing", json!({ "track_id": id })).await; });
        }
    }
    let url = stream_url(&t, transcoded, offset);
    let (Some(mut a), Some(sp)) = (audio(), spare()) else { return };
    if !truthy_num(offset) && dom::data_opt(&sp, "url").as_deref() == Some(url.as_str()) {
        // already buffered in the background: swap players and start right away
        let old = a;
        a = sp;
        AUDIO.with(|x| *x.borrow_mut() = Some(a.clone()));
        SPARE.with(|x| *x.borrow_mut() = Some(old.clone()));
        let _ = old.pause();
        let _ = old.remove_attribute("src");
        let _ = old.remove_attribute("data-url");
        a.set_volume(old.volume());
        a.set_current_time(0.0);
    } else {
        a.set_src(&url);
    }
    dom::set_data(&a, "url", &url);
    play(&a);
    if has_media_session() { set_media_metadata(&t); }
    dom::add_class(&body(), "has-player");
    render();
    emit();
    if st(|s| s.full) { load_lyrics(); }
    top_up();
    Timeout::new(1500, preload_next).forget();      // let the current song get going first
}

/// Start buffering the song that plays next.
fn preload_next() {
    let Some(sp) = spare() else { return };
    let n = st(|s| if s.index + 1 < 0 { None } else { s.queue.get((s.index + 1) as usize).cloned() });
    let Some(n) = n else { return };
    let url = stream_url(&n, convert(&n), 0.0);
    if dom::data_opt(&sp, "url").as_deref() == Some(url.as_str()) { return; }
    dom::set_data(&sp, "url", &url);
    sp.set_src(&url);
    sp.load();
}

/// Near the end of the queue: fetch the library shuffle ahead of time, so the next song is ready too.
fn top_up() {
    if TOPPING.with(|t| t.get()) || st(|s| s.context == "preview" || s.index < s.queue.len() as i64 - 2) { return; }
    let enabled = dom::ls_get("encore.autoplay").as_deref() != Some("off");
    if !enabled { return; }
    TOPPING.with(|t| t.set(true));
    dom::spawn(async move {
        if let Ok(more) = fresh_shuffle(50).await {
            if !more.is_empty() {
                st_mut(|s| s.queue.extend(more.into_iter().map(Rc::new)));
                render(); emit();
                preload_next();
            }
        }
        // offline: next() tries again
        TOPPING.with(|t| t.set(false));
    });
}

fn duration() -> f64 {
    let t = cur();
    let ad = audio().map(|a| a.duration()).unwrap_or(f64::NAN);
    let ad_ok = truthy_num(ad) && ad.is_finite();
    if t.as_ref().is_some_and(|t| t.truthy("preview")) { return if ad_ok { ad } else { 30.0 }; }
    let td = t.map(|t| t.f("duration")).unwrap_or(0.0);
    if st(|s| s.transcoded) { return td; }
    if ad_ok { ad } else { td }
}
fn position() -> f64 { st(|s| s.offset) + audio().map(|a| a.current_time()).unwrap_or(0.0) }

pub fn seek_to(sec: f64) {
    if via_link(json!({ "action": "seek", "value": sec.max(0.0) })) {
        LINK.with(|l| if let Some(l) = l.borrow_mut().as_mut() { if let Some(o) = l.state.as_object_mut() { o.insert("position".into(), json!(sec)); } });
        update_linked_time();
        return;
    }
    let dur = duration();
    if cur().is_none() || !truthy_num(dur) { return; }
    let sec = sec.min(dur - 0.25).max(0.0);
    if st(|s| s.transcoded) { load(true, sec); }      // ask the server to start the conversion at that point
    else if let Some(a) = audio() { a.set_current_time(sec); }
    update_time();
}

pub fn toggle() {
    if via_link(json!({ "action": "toggle" })) {
        LINK.with(|l| if let Some(l) = l.borrow_mut().as_mut() {
            let p = !l.state.truthy("playing");
            if let Some(o) = l.state.as_object_mut() { o.insert("playing".into(), json!(p)); }
        });
        render();
        return;
    }
    if cur().is_some() {
        if let Some(a) = audio() { if a.paused() { play(&a); } else { let _ = a.pause(); } }
    }
}

pub fn next(auto: bool) {
    if !auto && via_link(json!({ "action": "next" })) { return; }
    if auto && st(|s| s.repeat == "one") { seek_to(0.0); if let Some(a) = audio() { play(&a); } return; }
    if st(|s| s.index < s.queue.len() as i64 - 1) { st_mut(|s| s.index += 1); load(false, 0.0); return; }
    if st(|s| s.repeat == "all" && !s.queue.is_empty()) { st_mut(|s| s.index = 0); load(false, 0.0); return; }
    if st(|s| s.context == "preview") { return; }
    dom::spawn(async move {
        if continue_with_library().await { st_mut(|s| s.index += 1); load(false, 0.0); }
    });
}

/// Queue ran out: keep the music going with a shuffle of the whole library.
async fn continue_with_library() -> bool {
    let enabled = dom::ls_get("encore.autoplay").as_deref() != Some("off");
    if !enabled { return false; }
    match fresh_shuffle(50).await {
        Ok(more) => {
            if more.is_empty() { return false; }
            st_mut(|s| { s.queue.extend(more.into_iter().map(Rc::new)); s.context = "library".into(); });
            true
        }
        Err(_) => false,
    }
}

pub fn prev() {
    if via_link(json!({ "action": "prev" })) { return; }
    if position() > 3.0 || st(|s| s.index == 0) { seek_to(0.0); return; }
    st_mut(|s| s.index -= 1);
    load(false, 0.0);
}

fn jump(i: i64) { st_mut(|s| s.index = i); load(false, 0.0); }
fn remove_at(i: i64) {
    if st(|s| i == s.index) { return; }
    st_mut(|s| {
        if i >= 0 && (i as usize) < s.queue.len() { s.queue.remove(i as usize); }
        if i < s.index { s.index -= 1; }
    });
    render(); emit(); preload_next();
}
fn move_to(from: i64, to: i64) {
    if st(|s| from == s.index || to <= s.index || to >= s.queue.len() as i64) { return; }
    st_mut(|s| {
        if from >= 0 && (from as usize) < s.queue.len() {
            let t = s.queue.remove(from as usize);
            s.queue.insert(to as usize, t);
        }
    });
    render(); emit(); preload_next();
}
fn shuffle_up_next() {
    st_mut(|s| {
        let start = (s.index + 1).max(0) as usize;
        if start < s.queue.len() {
            // `.sort(() => Math.random() - 0.5)` – a random order of what's left (Fisher–Yates with Math.random)
            let rest = &mut s.queue[start..];
            for i in (1..rest.len()).rev() {
                let j = (js_sys::Math::random() * (i + 1) as f64).floor() as usize;
                rest.swap(i, j);
            }
        }
    });
    render(); emit(); preload_next();
}

fn log_play() {
    let Some(t) = cur() else { return };
    if t.truthy("preview") || st(|s| s.logged) || !t.g("id").is_number() { return; }
    let p = position();
    let d = duration();
    // counts (and scrobbles) like Last.fm: half the song or 4 minutes, whichever comes first; very short songs at the end
    let hit = if truthy_num(d) { if d < 30.0 { p >= d - 1.0 } else { p >= (d / 2.0).min(240.0) } } else { p >= 30.0 };
    if hit {
        st_mut(|s| s.logged = true);
        let id = t.g("id").clone();
        dom::spawn(async move { let _ = api::post("/library/plays", json!({ "track_id": id, "seconds": p })).await; });
    }
}

/// `e.target === audio` – only the element currently playing counts.
fn is_current_audio(e: &Event) -> bool {
    let Some(t) = e.target() else { return false };
    AUDIO.with(|a| a.borrow().as_ref().is_some_and(|a| { let av: &JsValue = a.as_ref(); let tv: &JsValue = t.as_ref(); av == tv }))
}
/// `const on = (type, fn) => [audio, spare].forEach(a => a.addEventListener(type, e => { if (e.target === audio) fn(e); }))`.
fn on_both(a: &HtmlMediaElement, sp: &HtmlMediaElement, ty: &str, f: Rc<dyn Fn(&Event)>) {
    for x in [a, sp] {
        let f = f.clone();
        dom::on_forever(x, ty, move |e| { if is_current_audio(e) { f(e); } });
    }
}
/// `navigator.mediaSession.setActionHandler(action, f)` – called through the JS function so an unknown
/// action (older browsers) is caught like the JS `try { … } catch {}`.
fn set_action(action: &str, f: impl Fn(JsValue) + 'static) {
    let Some(ms) = media_session() else { return };
    let Ok(func) = js_sys::Reflect::get(&ms, &"setActionHandler".into()).and_then(|f| f.dyn_into::<js_sys::Function>()) else { return };
    let c = Closure::<dyn Fn(JsValue)>::new(f);
    let _ = func.call2(&ms, &action.into(), c.as_ref());
    c.forget();
}
/// `d.seekTime` / `d.seekOffset` of a MediaSessionActionDetails.
fn detail_num(d: &JsValue, key: &str) -> Option<f64> { js_sys::Reflect::get(d, &key.into()).ok().and_then(|v| v.as_f64()) }

pub fn mount() {
    expose_global();
    if by_id("player").is_some() { return; }
    let a: HtmlMediaElement = match by_id("audio").and_then(|e| e.dyn_into::<HtmlMediaElement>().ok()) {
        Some(a) => a,
        None => { let e = dom::create("audio"); e.set_id("audio"); e.dyn_into::<HtmlMediaElement>().expect("audio") }
    };
    if !a.is_connected() { let _ = body().append_child(&a); }
    let sp: HtmlMediaElement = dom::create("audio").dyn_into::<HtmlMediaElement>().expect("audio");
    sp.set_id("audio-next");
    sp.set_preload("auto");
    let _ = body().append_child(&sp);
    a.set_preload("auto");
    let el = dom::create("div");
    el.set_class_name("player");
    el.set_id("player");
    let _ = body().append_child(&el);
    let full = dom::create("div");
    full.set_class_name("np-full hidden");
    full.set_id("np-full");
    let _ = body().append_child(&full);
    let vol = js_sys::parse_float(&dom::ls_get("encore.volume").unwrap_or_else(|| "0.9".into()));
    // `audio.volume = spare.volume = vol` (the setter rejects anything outside 0..1)
    if vol.is_finite() && (0.0..=1.0).contains(&vol) { a.set_volume(vol); sp.set_volume(vol); }
    AUDIO.with(|x| *x.borrow_mut() = Some(a.clone()));
    SPARE.with(|x| *x.borrow_mut() = Some(sp.clone()));
    // both elements get the listeners; only the one currently playing counts
    on_both(&a, &sp, "ended", Rc::new(|_| { log_play(); next(true); }));
    on_both(&a, &sp, "play", Rc::new(|_| { render(); native(); }));
    on_both(&a, &sp, "pause", Rc::new(|_| { render(); native(); }));
    on_both(&a, &sp, "timeupdate", Rc::new(|_| { update_time(); log_play(); sync_lyrics(false); }));
    on_both(&a, &sp, "loadedmetadata", Rc::new(|_| update_time()));
    // the browser couldn't decode the original (e.g. ALAC in .m4a) → ask the server for FLAC
    on_both(&a, &sp, "error", Rc::new(|_| {
        if let Some(t) = cur() {
            let has_src = audio().and_then(|a| a.get_attribute("src")).is_some_and(|s| !s.is_empty());
            if !st(|s| s.transcoded) && !t.truthy("preview") && has_src { load(true, position()); }
        }
    }));
    dom::on_forever(&sp, "error", |_| { if let Some(s) = spare() { let _ = s.remove_attribute("data-url"); } });
    if has_media_session() {
        set_action("play", |_| { if let Some(a) = audio() { play(&a); } });
        set_action("pause", |_| { if let Some(a) = audio() { let _ = a.pause(); } });
        set_action("nexttrack", |_| next(false));
        set_action("previoustrack", |_| prev());
        // older browsers
        set_action("seekto", |d| seek_to(detail_num(&d, "seekTime").unwrap_or(0.0)));
        set_action("seekforward", |d| seek_to(position() + detail_num(&d, "seekOffset").filter(|v| truthy_num(*v)).unwrap_or(10.0)));
        set_action("seekbackward", |d| seek_to(position() - detail_num(&d, "seekOffset").filter(|v| truthy_num(*v)).unwrap_or(10.0)));
    }
    let keydown = EventListener::new_with_options(&document(), "keydown", EventListenerOptions::enable_prevent_default(), |e: &Event| {
        let ae = document().active_element();
        if dom::key(e) == "Escape" && st(|s| s.full) { set_full(false); return; }
        let linked = is_linked();
        let (tag, id) = ae.map(|a| (a.tag_name(), a.id())).unwrap_or_default();
        let is_field = ["INPUT", "TEXTAREA", "SELECT", "BUTTON"].iter().any(|k| tag.contains(k));
        if (cur().is_none() && !linked) || (is_field && !id.contains("seek")) { return; }
        let code = e.dyn_ref::<KeyboardEvent>().map(|k| k.code()).unwrap_or_default();
        if code == "Space" { e.prevent_default(); toggle(); }
        let at = if linked { LINK.with(|l| l.borrow().as_ref().map(|l| l.state.f("position")).unwrap_or(0.0)) } else { position() };
        if code == "ArrowRight" { e.prevent_default(); seek_to(at + 10.0); }
        if code == "ArrowLeft" { e.prevent_default(); seek_to(at - 10.0); }
    });
    dom::keep_forever(keydown);
    dom::on_window("hashchange", |_| { if st(|s| s.full) { set_full(false); } });
    render();
}

pub fn set_full(on: bool) {
    if on && is_linked() { remote_open(); return; }
    st_mut(|s| s.full = on);
    let Some(f) = by_id("np-full") else { return };
    dom::toggle_class(&f, "hidden", !on);
    dom::toggle_class(&body(), "np-open", on);
    if on { render_full(); load_lyrics(); }
}

fn update_time() {
    if is_linked() { return; }
    let dur = duration();
    let dragging = st(|s| s.dragging);
    for (sid, cid, did) in [("seek", "t-cur", "t-dur"), ("fseek", "f-cur", "f-dur")] {
        let Some(seek) = by_id(sid) else { continue };
        if !dragging {
            dom::set_value(&seek, &js_num(if truthy_num(dur) { (position() / dur) * 1000.0 } else { 0.0 }));
            if let Some(c) = by_id(cid) { dom::set_text(&c, &fmt::dur(position())); }
        }
        set_p(&seek);
        if let Some(d) = by_id(did) { dom::set_text(&d, &fmt::dur(dur)); }
    }
}

fn quality(t: &Value) -> String {
    if t.truthy("preview") { return "30 s preview".into(); }
    let f = t.s("format").to_uppercase();
    if st(|s| s.transcoded) { format!("{f} → FLAC") }
    else { format!("{f}{}", if t.truthy("bitrate") { format!(" {}k", t.s("bitrate")) } else { String::new() }) }
}

fn bind_seek(out: &mut Vec<EventListener>, seek: &Element, cur_id: &str) {
    out.push(dom::listener(seek, "pointerdown", |_| st_mut(|s| s.dragging = true)));
    out.push(dom::listener(seek, "touchstart", |_| st_mut(|s| s.dragging = true)));   // passive, like the JS
    let (sk, cid) = (seek.clone(), cur_id.to_string());
    out.push(dom::listener(seek, "input", move |_| {
        st_mut(|s| s.dragging = true);
        if let Some(c) = by_id(&cid) { dom::set_text(&c, &fmt::dur((num(&dom::value(&sk)) / 1000.0) * duration())); }
        set_p(&sk);
    }));
    let sk = seek.clone();
    let commit: Rc<dyn Fn()> = Rc::new(move || {
        if st(|s| s.dragging) { st_mut(|s| s.dragging = false); seek_to((num(&dom::value(&sk)) / 1000.0) * duration()); }
    });
    for ev in ["change", "pointerup", "touchend"] {
        let c = commit.clone();
        out.push(dom::listener(seek, ev, move |_| c()));
    }
}

fn update_linked_time() {
    let Some(s) = LINK.with(|l| l.borrow().as_ref().map(|l| l.state.clone())) else { return };
    let Some(seek) = by_id("seek") else { return };
    if st(|x| x.dragging) { return; }
    let dur = s.f("duration");
    let pos = s.f("position").min(if truthy_num(dur) { dur } else { f64::INFINITY });
    dom::set_value(&seek, &js_num(if truthy_num(dur) { (pos / dur) * 1000.0 } else { 0.0 }));
    set_p(&seek);
    if let Some(c) = by_id("t-cur") { dom::set_text(&c, &fmt::dur(pos)); }
    if let Some(d) = by_id("t-dur") { dom::set_text(&d, &fmt::dur(dur)); }
}

fn render_linked(el: &Element) {
    let Some(l) = linked() else { return };
    let s = l.state.clone();
    let name = esc(&l.name);
    let where_ = format!(r#"<span class="linked-to" title="This device is a remote for {name}">{}{}Playing on {name}</span>"#, icon("link"), if l.online { "" } else { "Offline · " });
    dom::add_class(&body(), "has-player");
    let has_title = s.truthy("title");
    let thumb = if has_title { art(&s.s("cover"), &album_or_title(&s), "") } else { format!(r#"<div class="ph">{}</div>"#, icon_attr("devices", r#"width="20""#)) };
    let title = if has_title { esck(&s, "title") } else { "Nothing playing".to_string() };
    let sub = if has_title {
        format!("{}{}", esc(&s.s("artist")), if s.truthy("album") { format!(" · {}", esck(&s, "album")) } else { String::new() })
    } else { "Play anything – it starts there".to_string() };
    let vol = if s.is_nullish("volume") { "0.9".to_string() } else { s.s("volume") };
    set_html(el, &format!(r##"
    <div class="now" id="p-open" role="button" tabindex="0" title="Your devices">
      <div class="thumb">{thumb}</div>
      <div class="grow">
        <div class="ellipsis" style="font-weight:650">{title}</div>
        <div class="ellipsis small muted">{sub}</div>
        <div class="ellipsis small">{where_}</div>
      </div>
    </div>
    <div class="ctrls">
      <div class="row">
        <button class="btn ghost icon" id="p-prev" aria-label="Previous">{prev}</button>
        <button class="play-btn" id="p-toggle" aria-label="Play/Pause">{pp}</button>
        <button class="btn ghost icon" id="p-next" aria-label="Next">{next}</button>
      </div>
      <div class="seek"><span id="t-cur">0:00</span><input type="range" id="seek" min="0" max="1000" value="0"><span id="t-dur">0:00</span></div>
    </div>
    <div class="right">
      <button class="btn sm" id="p-unlink" title="Stop being a remote and play on this device again">{link}Unlink</button>
      <span class="faint vol-ic">{volume}</span>
      <input type="range" id="vol" min="0" max="1" step="0.05" value="{vol}">
    </div>"##,
        prev = icon("prev"), pp = if s.truthy("playing") { icon("pause") } else { icon("play") }, next = icon("next"),
        link = icon("link"), volume = icon_attr("volume", r#"width="18""#)));
    let mut ls = Vec::new();
    wire(&mut ls, el, "#p-toggle", "click", |_| toggle());
    wire(&mut ls, el, "#p-next", "click", |_| next(false));
    wire(&mut ls, el, "#p-prev", "click", |_| prev());
    wire(&mut ls, el, "#p-open", "click", |_| remote_open());
    wire(&mut ls, el, "#p-unlink", "click", |_| remote_unlink());
    wire(&mut ls, el, "#vol", "change", |e| { if let Some(v) = dom::target_el(e) { set_volume(num(&dom::value(&v))); } });
    if let Some(seek) = qs_in(el, "#seek") {
        ls.push(dom::listener(&seek, "pointerdown", |_| st_mut(|s| s.dragging = true)));
        let s_dur = s.f("duration");
        let sk = seek.clone();
        ls.push(dom::listener(&seek, "input", move |_| {
            st_mut(|s| s.dragging = true);
            if let Some(c) = by_id("t-cur") { dom::set_text(&c, &fmt::dur((num(&dom::value(&sk)) / 1000.0) * s_dur)); }
            set_p(&sk);
        }));
        let sk = seek.clone();
        ls.push(dom::listener(&seek, "change", move |_| {
            st_mut(|s| s.dragging = false);
            let d = LINK.with(|l| l.borrow().as_ref().map(|l| l.state.f("duration")).unwrap_or(0.0));
            seek_to((num(&dom::value(&sk)) / 1000.0) * d);
        }));
    }
    set_bar(ls);
    update_linked_time();
}

fn render() {
    let Some(el) = by_id("player") else { return };
    if is_linked() { render_linked(&el); return; }
    let t = cur();
    dom::add_class(&body(), "has-player");
    let Some(t) = t else {
        let devices = if has_encore_remote() {
            format!(r#"<div class="right"><button class="btn ghost icon" id="p-devices" aria-label="Other devices" title="Play on or control your other devices">{}</button></div>"#, icon("devices"))
        } else { String::new() };
        set_html(&el, &format!(r##"<div class="now"><div class="thumb"><div class="ph">{music}</div></div>
      <div class="grow"><div style="font-weight:650" class="faint">Nothing playing</div>
      <div class="small muted ellipsis">Open an album in <a href="#/library" style="color:var(--accent)">Library</a> and press ▶ Play</div></div></div>
      {devices}"##, music = icon_attr("music", r#"width="20""#)));
        let mut ls = Vec::new();
        wire(&mut ls, &el, "#p-devices", "click", |_| remote_open());
        set_bar(ls);
        if st(|s| s.full) { set_full(false); }
        return;
    };
    let Some(a) = audio() else { return };
    let transcoded = st(|s| s.transcoded);
    let badge_title = if transcoded { "Converted to FLAC for your browser (lossless)" } else if t.truthy("preview") { "A 30-second preview – not in your library" } else { "Original file, no conversion" };
    let devices = if has_encore_remote() {
        format!(r#"<button class="btn ghost icon" id="p-devices" aria-label="Other devices" title="Play on or control your other devices">{}</button>"#, icon("devices"))
    } else { String::new() };
    set_html(&el, &format!(r##"
    <div class="now" id="p-open" role="button" tabindex="0" aria-label="Open the full-screen player" title="Open player">
      <div class="thumb">{thumb}</div>
      <div class="grow">
        <div class="ellipsis" style="font-weight:650">{title}</div>
        <div class="ellipsis small muted">{artist}{album}</div>
      </div>
    </div>
    <div class="ctrls">
      <div class="row">
        <button class="btn ghost icon" id="p-prev" aria-label="Previous">{prev}</button>
        <button class="play-btn" id="p-toggle" aria-label="Play/Pause">{pp}</button>
        <button class="btn ghost icon" id="p-next" aria-label="Next">{next}</button>
      </div>
      <div class="seek"><span id="t-cur">0:00</span><input type="range" id="seek" min="0" max="1000" value="0"><span id="t-dur">0:00</span></div>
    </div>
    <div class="right">
      <span class="badge" title="{badge_title}">{quality}</span>
      <button class="btn ghost icon" id="p-queue" aria-label="Up next" title="Up next">{list}</button>
      {devices}
      <span class="faint vol-ic">{volume}</span>
      <input type="range" id="vol" min="0" max="1" step="0.01" value="{vol}">
      <button class="btn ghost icon" id="p-close" aria-label="Close player">{x}</button>
    </div>"##,
        thumb = art(&cover(&t), &album_or_title(&t), ""),
        title = esck(&t, "title"),
        artist = esck(&t, "artist"),
        album = if t.truthy("album") { format!(" · {}", esck(&t, "album")) } else { String::new() },
        prev = icon("prev"), pp = if a.paused() { icon("play") } else { icon("pause") }, next = icon("next"),
        quality = esc(&quality(&t)), list = icon("list"),
        volume = icon_attr("volume", r#"width="18""#), vol = js_num(a.volume()), x = icon("x")));
    let mut ls = Vec::new();
    wire(&mut ls, &el, "#p-toggle", "click", |_| toggle());
    wire(&mut ls, &el, "#p-next", "click", |_| next(false));
    wire(&mut ls, &el, "#p-prev", "click", |_| prev());
    wire(&mut ls, &el, "#p-open", "click", |_| { st_mut(|s| s.pane = "now".into()); set_full(true); });
    wire(&mut ls, &el, "#p-open", "keydown", |e| { if dom::key(e) == "Enter" { set_full(true); } });
    wire(&mut ls, &el, "#p-queue", "click", |_| { st_mut(|s| s.pane = "queue".into()); set_full(true); });
    wire(&mut ls, &el, "#p-devices", "click", |_| remote_open());
    if let Some(seek) = qs_in(&el, "#seek") { bind_seek(&mut ls, &seek, "t-cur"); }
    wire(&mut ls, &el, "#vol", "input", |e| {
        if let (Some(v), Some(a), Some(sp)) = (dom::target_el(e), audio(), spare()) {
            let vol = num(&dom::value(&v));
            a.set_volume(vol);
            sp.set_volume(vol);
            dom::ls_set("encore.volume", &js_num(a.volume()));
        }
    });
    wire(&mut ls, &el, "#p-close", "click", |_| {
        if let (Some(a), Some(sp)) = (audio(), spare()) {
            let _ = a.pause();
            let _ = a.remove_attribute("src");
            let _ = sp.remove_attribute("src");
            let _ = sp.remove_attribute("data-url");
        }
        st_mut(|s| { s.queue.clear(); s.index = -1; });
        render(); emit();
    });
    set_bar(ls);
    update_time();
    if st(|s| s.full) { render_full(); }
}

fn render_full() {
    let Some(f) = by_id("np-full") else { return };
    let Some(t) = cur() else { return };
    let Some(a) = audio() else { return };
    let (index, pane, repeat, up_next) = st(|s| (s.index, s.pane.clone(), s.repeat.clone(), s.queue.iter().skip((s.index + 1).max(0) as usize).cloned().collect::<Vec<_>>()));
    let auto = dom::ls_get("encore.autoplay").as_deref() != Some("off");
    dom::set_style(&f, "--np-bg", &format!(r#"url("{}")"#, cover(&t)));
    let tab = |p: &str, label: &str| format!(r#"<button data-pane="{p}" class="{}">{label}</button>"#, if pane == p { "active" } else { "" });
    let artist = if t.truthy("preview") { esck(&t, "artist") } else {
        format!(r##"<a href="#/artist-name/{}">{}</a>"##, String::from(js_sys::encode_uri_component(&t.s("artist"))), esck(&t, "artist"))
    };
    let album = if t.truthy("album") {
        format!(" · {}", if t.truthy("album_id") { format!(r##"<a href="#/library/album/{}">{}</a>"##, esck(&t, "album_id"), esck(&t, "album")) } else { esck(&t, "album") })
    } else { String::new() };
    let list = if !up_next.is_empty() {
        up_next.iter().take(200).enumerate().map(|(k, q)| {
            let i = index + 1 + k as i64;
            format!(r#"<div class="np-item" data-i="{i}">
            <div class="thumb">{}</div>
            <div class="grow" style="min-width:0"><div class="ellipsis" style="font-weight:600">{}</div><div class="small muted ellipsis">{}</div></div>
            <button class="btn ghost icon sm" data-up="{i}" aria-label="Move up" {}>↑</button>
            <button class="btn ghost icon sm" data-rm="{i}" aria-label="Remove">{}</button></div>"#,
                art(&cover(q), &album_or_title(q), ""), esck(q, "title"), esck(q, "artist"), if i == index + 1 { "disabled" } else { "" }, icon("x"))
        }).collect::<String>()
    } else {
        format!(r#"<div class="muted small" style="padding:16px 4px">{}</div>"#, if auto { "Nothing queued – a shuffle of your library plays next." } else { "Nothing queued." })
    };
    set_html(&f, &format!(r##"
    <div class="np-backdrop"></div>
    <div class="np-top">
      <button class="btn ghost icon" id="f-close" aria-label="Close">{x}</button>
      <div class="tabs np-tabs">{tab_now}{tab_lyrics}{tab_queue}</div>
      <span></span>
    </div>
    <div class="np-body pane-{pane}">
      <section class="np-main">
        <div class="np-art">{big_art}</div>
        <div class="np-meta">
          <h2 class="ellipsis">{title}</h2>
          <div class="muted ellipsis">{artist}{album}</div>
          <span class="badge mt">{quality}</span>
        </div>
        <div class="seek np-seek"><span id="f-cur">0:00</span><input type="range" id="fseek" min="0" max="1000" value="0"><span id="f-dur">0:00</span></div>
        <div class="np-ctrls">
          <button class="btn ghost icon" id="f-shuffle" title="Shuffle up next" aria-label="Shuffle up next">{shuffle}</button>
          <button class="btn ghost icon" id="f-prev" aria-label="Previous">{prev}</button>
          <button class="play-btn lg" id="f-toggle" aria-label="Play/Pause">{pp}</button>
          <button class="btn ghost icon" id="f-next" aria-label="Next">{next}</button>
          <button class="btn ghost icon {repeat_on}" id="f-repeat" title="Repeat: {repeat}" aria-label="Repeat">{refresh}{repeat_sup}</button>
        </div>
        <div class="row np-vol"><span class="faint">{volume}</span><input type="range" id="fvol" min="0" max="1" step="0.01" value="{vol}"></div>
      </section>
      <section class="np-lyrics" id="np-lyrics">{lyrics}</section>
      <section class="np-queue">
        <div class="row between"><h3>Up next</h3>
          <label class="row small muted" style="gap:6px" title="When the queue ends, keep playing a shuffle of the whole library"><input type="checkbox" id="f-auto" {checked}> Then shuffle library</label></div>
        <div class="np-list">{list}</div>
      </section>
    </div>"##,
        x = icon("x"), tab_now = tab("now", "Now playing"), tab_lyrics = tab("lyrics", "Lyrics"), tab_queue = tab("queue", "Up next"),
        big_art = art(&cover(&t), &album_or_title(&t), "big"), title = esck(&t, "title"), quality = esc(&quality(&t)),
        shuffle = icon("shuffle"), prev = icon("prev"), pp = if a.paused() { icon("play") } else { icon("pause") }, next = icon("next"),
        repeat_on = if repeat != "off" { "on" } else { "" }, refresh = icon("refresh"), repeat_sup = if repeat == "one" { "<sup>1</sup>" } else { "" },
        volume = icon_attr("volume", r#"width="18""#), vol = js_num(a.volume()), lyrics = lyrics_html(),
        checked = if auto { "checked" } else { "" }));
    let mut ls = Vec::new();
    wire(&mut ls, &f, "#f-close", "click", |_| set_full(false));
    wire(&mut ls, &f, ".np-tabs", "click", |e| {
        if let Some(b) = dom::closest(e, "[data-pane]") {
            let p = dom::data(&b, "pane");
            st_mut(|s| s.pane = p.clone());
            render_full();
            if p == "lyrics" { load_lyrics(); }
        }
    });
    wire(&mut ls, &f, "#f-toggle", "click", |_| toggle());
    wire(&mut ls, &f, "#f-next", "click", |_| next(false));
    wire(&mut ls, &f, "#f-prev", "click", |_| prev());
    wire(&mut ls, &f, "#f-shuffle", "click", |_| shuffle_up_next());
    wire(&mut ls, &f, "#f-repeat", "click", |_| {
        st_mut(|s| s.repeat = match s.repeat.as_str() { "off" => "all", "all" => "one", _ => "off" }.into());
        render_full();
    });
    wire(&mut ls, &f, "#f-auto", "change", |e| {
        if let Some(c) = dom::target_el(e) { dom::ls_set("encore.autoplay", if dom::checked(&c) { "on" } else { "off" }); }
        render_full();
    });
    wire(&mut ls, &f, "#fvol", "input", |e| {
        if let (Some(v), Some(a), Some(sp)) = (dom::target_el(e), audio(), spare()) {
            let vol = num(&dom::value(&v));
            a.set_volume(vol);
            sp.set_volume(vol);
            if let Some(el) = by_id("vol") { dom::set_value(&el, &js_num(a.volume())); }
        }
    });
    if let Some(seek) = qs_in(&f, "#fseek") { bind_seek(&mut ls, &seek, "f-cur"); }
    wire(&mut ls, &f, ".np-list", "click", |e| {
        if let Some(rm) = dom::closest(e, "[data-rm]") { remove_at(num(&dom::data(&rm, "rm")) as i64); return; }
        if let Some(up) = dom::closest(e, "[data-up]") { let i = num(&dom::data(&up, "up")) as i64; move_to(i, i - 1); return; }
        if let Some(it) = dom::closest(e, "[data-i]") { jump(num(&dom::data(&it, "i")) as i64); }
    });
    wire(&mut ls, &f, "#np-lyrics", "click", |e| { if let Some(l) = dom::closest(e, "[data-t]") { seek_to(num(&dom::data(&l, "t"))); } });
    set_full_listeners(ls);
    update_time();
    sync_lyrics(true);
}

// ---- lyrics ----------------------------------------------------------------------------------

/// One `[mm:ss.xx]` stamp: the regex `\[(\d+):(\d+(?:\.\d+)?)\]` applied to the bracket's content.
fn parse_stamp(s: &str) -> Option<f64> {
    let (m, rest) = s.split_once(':')?;
    let digits = |x: &str| !x.is_empty() && x.bytes().all(|c| c.is_ascii_digit());
    if !digits(m) { return None; }
    match rest.split_once('.') {
        Some((sec, frac)) => { if !digits(sec) || !digits(frac) { return None; } }
        None => { if !digits(rest) { return None; } }
    }
    let minutes: f64 = m.parse().ok()?;
    let seconds: f64 = rest.parse().ok()?;
    Some(minutes * 60.0 + seconds)
}
/// `[...line.matchAll(/\[(\d+):(\d+(?:\.\d+)?)\]/g)]`.
fn lrc_stamps(line: &str) -> Vec<f64> {
    let b = line.as_bytes();
    let mut out = vec![];
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'[' {
            if let Some(j) = b[i + 1..].iter().position(|&c| c == b']') {
                if let Some(t) = parse_stamp(&line[i + 1..i + 1 + j]) { out.push(t); i = i + 1 + j + 1; continue; }
            }
        }
        i += 1;
    }
    out
}
/// `line.replace(/\[[^\]]*\]/g, '')`.
fn strip_brackets(line: &str) -> String {
    let mut out = String::new();
    let mut rest = line;
    loop {
        let Some(i) = rest.find('[') else { out.push_str(rest); break };
        match rest[i + 1..].find(']') {
            Some(j) => { out.push_str(&rest[..i]); rest = &rest[i + 1 + j + 1..]; }
            None => { out.push_str(rest); break }
        }
    }
    out
}
fn parse_lrc(text: &str) -> Vec<Line> {
    let mut out = Vec::new();
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let stamps = lrc_stamps(line);
        let words = strip_brackets(line).trim().to_string();
        for t in stamps { out.push(Line { t, text: words.clone() }); }
    }
    out.sort_by(|a, b| a.t.partial_cmp(&b.t).unwrap_or(std::cmp::Ordering::Equal));
    out
}

fn lyrics_html() -> String {
    let Some(t) = cur() else { return String::new() };
    if t.truthy("preview") { return r#"<div class="muted np-empty">Lyrics are available for songs in your library.</div>"#.into(); }
    LYR.with(|l| {
        let l = l.borrow();
        if l.loading || l.id != *t.g("id") { return r#"<div class="muted np-empty"><span class="spinner"></span> Looking for lyrics…</div>"#.to_string(); }
        if !l.lines.is_empty() {
            return l.lines.iter().enumerate().map(|(i, ln)| {
                let text = esc(&ln.text);
                format!(r#"<p data-t="{}" data-li="{i}">{}</p>"#, js_num(ln.t), if text.is_empty() { "♪" } else { &text })
            }).collect::<String>();
        }
        if !l.plain.is_empty() { return format!(r#"<div class="np-plain">{}</div>"#, esc(&l.plain)); }
        format!(r#"<div class="muted np-empty">{}</div>"#, if l.instrumental { "Instrumental" } else { "No lyrics found for this song." })
    })
}

fn load_lyrics() {
    let t = cur();
    let skip = match &t { None => true, Some(t) => t.truthy("preview") || LYR.with(|l| l.borrow().id == *t.g("id")) };
    if skip {
        if let Some(b) = by_id("np-lyrics") { set_html(&b, &lyrics_html()); }
        return;
    }
    let Some(t) = t else { return };
    let id = t.g("id").clone();
    LYR.with(|l| *l.borrow_mut() = Lyr { id: id.clone(), lines: vec![], plain: String::new(), loading: true, instrumental: false });
    if let Some(b) = by_id("np-lyrics") { set_html(&b, &lyrics_html()); }
    dom::spawn(async move {
        match api::get(&format!("/library/tracks/{}/lyrics", js_string(&id))).await {
            Ok(r) => {
                if LYR.with(|l| l.borrow().id != id) { return; }
                let synced = r.truthy("synced");
                LYR.with(|l| *l.borrow_mut() = Lyr {
                    id: id.clone(),
                    lines: if synced { parse_lrc(&r.s("lyrics")) } else { vec![] },
                    plain: if synced { String::new() } else { r.s("lyrics") },
                    loading: false,
                    instrumental: r.truthy("instrumental"),
                });
            }
            Err(_) => { LYR.with(|l| *l.borrow_mut() = Lyr { id: id.clone(), lines: vec![], plain: String::new(), loading: false, instrumental: false }); }
        }
        if let Some(b) = by_id("np-lyrics") { set_html(&b, &lyrics_html()); sync_lyrics(true); }
    });
}

fn sync_lyrics(force: bool) {
    if !st(|s| s.full) { return; }
    let cur_id = cur().map(|t| t.g("id").clone());
    let ok = LYR.with(|l| { let l = l.borrow(); !l.lines.is_empty() && cur_id.as_ref() == Some(&l.id) });
    if !ok { return; }
    let p = position() + 0.3;
    let i = LYR.with(|l| {
        let l = l.borrow();
        let mut i: i64 = -1;
        while ((i + 1) as usize) < l.lines.len() && l.lines[(i + 1) as usize].t <= p { i += 1; }
        i
    });
    if i == LAST_LINE.with(|x| x.get()) && !force { return; }
    LAST_LINE.with(|x| x.set(i));
    let Some(b) = by_id("np-lyrics") else { return };
    for n in qsa_in(&b, "p.on") { dom::remove_class(&n, "on"); }
    if let Some(c) = qs_in(&b, &format!(r#"p[data-li="{i}"]"#)) {
        dom::add_class(&c, "on");
        let (top, h) = c.dyn_ref::<HtmlElement>().map(|h| (h.offset_top() as f64, h.offset_height() as f64)).unwrap_or((0.0, 0.0));
        let opts = ScrollToOptions::new();
        opts.set_top(top - b.client_height() as f64 / 2.0 + h / 2.0);
        opts.set_behavior(if force { ScrollBehavior::Auto } else { ScrollBehavior::Smooth });
        b.scroll_to_with_scroll_to_options(&opts);
    }
}
