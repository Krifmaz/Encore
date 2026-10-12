//! Port of web/js/player.js – see docs/ui-rewrite/CONTRACT.md
//! Web player: streams the original file (bit-perfect for MP3/FLAC/AAC/Opus); formats a browser
//! can't decode (ALAC/APE/WavPack/AIFF/WMA) are converted to FLAC on the fly by the server, and when the
//! connection can't keep up (Settings → Playback → Auto) the rest of the song comes as a smaller AAC stream.
//! Tap the bar for the full-screen player (lyrics + Up next). When the queue runs out it keeps going
//! with a shuffle of the whole library: a single song → straight into the shuffle, an album → after its last song.

use crate::api::{self, ApiError};
use crate::dom::{self, body, by_id, document, qs_in, qsa_in, set_html, EventListener, Interval, Timeout};
use crate::json::{self, js_string, J};
use crate::ui::{art, esc, esck, fmt, icon, icon_attr, pop_menu, share_track, toast, MenuItem, ModalOpts};
use crate::{app, playback};
use gloo_events::EventListenerOptions;
use serde_json::{json, Map, Value};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
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
    lossy: bool,
    /// upsampled by the server's Cadenza, and the rate it sends
    dsp: bool,
    dsp_rate: f64,
    offset: f64,
    dragging: bool,
    context: String,
    repeat: String,
    full: bool,
    pane: String,
    logged: bool,
    announced: Option<Rc<Value>>,
    stalls: Vec<f64>,
    stall_at: f64,
    seek_at: f64,
    switched: bool,
    last_preload_check: f64,
    slow_start: Option<Timeout>,
}
impl Default for State {
    fn default() -> Self {
        Self { queue: vec![], index: -1, transcoded: false, lossy: false, dsp: false, dsp_rate: 0.0, offset: 0.0, dragging: false, context: "list".into(), repeat: "off".into(), full: false, pane: "now".into(), logged: false, announced: None,
            stalls: vec![], stall_at: 0.0, seek_at: 0.0, switched: false, last_preload_check: 0.0, slow_start: None }
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
    /// `let prefs = playback.current()`
    static PREFS: RefCell<Value> = RefCell::new(playback::current());
    /// Auto quality: songs started before this time use the smaller stream
    static LOSSY_UNTIL: Cell<f64> = const { Cell::new(0.0) };
    /// liked track ids, fetched the first time a heart is drawn
    static LIKED: RefCell<Option<HashSet<i64>>> = const { RefCell::new(None) };
    /// AudioContext, only to read the device's output sample rate
    static CTX: RefCell<Option<web_sys::AudioContext>> = const { RefCell::new(None) };
    /// track id → /info (codec, sample rate, bit depth …)
    static INFO: RefCell<HashMap<i64, Value>> = RefCell::new(HashMap::new());
    /// what this browser says it can decode, by file format
    static PLAYABLE: RefCell<HashMap<String, bool>> = RefCell::new(HashMap::new());
    /// the smaller stream's codec: AAC where the browser decodes it, Opus elsewhere (Chromium builds, Firefox on Linux)
    static LOSSY: String = detect_lossy();
    /// GET /api/playback/dsp: is server-side Cadenza processing on, and may this account use it
    static DSP_INFO: RefCell<Option<Value>> = const { RefCell::new(None) };
    /// a processed stream failed this session: play originals until the page reloads
    static DSP_FAILED: Cell<bool> = const { Cell::new(false) };
    static DSP_TOASTED: Cell<bool> = const { Cell::new(false) };
}

fn fetch_dsp() {
    dom::spawn(async {
        let i = api::get("/playback/dsp").await.ok();
        DSP_INFO.with(|d| *d.borrow_mut() = i);
    });
}
fn dsp_info() -> Value { DSP_INFO.with(|d| d.borrow().clone()).unwrap_or(Value::Null) }

fn st<R>(f: impl FnOnce(&State) -> R) -> R { STATE.with(|s| f(&s.borrow())) }
fn st_mut<R>(f: impl FnOnce(&mut State) -> R) -> R { STATE.with(|s| f(&mut s.borrow_mut())) }
fn audio() -> Option<HtmlMediaElement> { AUDIO.with(|a| a.borrow().clone()) }
fn spare() -> Option<HtmlMediaElement> { SPARE.with(|a| a.borrow().clone()) }
/// `state.queue[state.index] || null` (the queue entry itself, for identity checks).
fn cur() -> Option<Rc<Value>> { st(|s| if s.index < 0 { None } else { s.queue.get(s.index as usize).cloned() }) }
fn is_linked() -> bool { LINK.with(|l| l.borrow().is_some()) }
fn is_local() -> bool { LOCAL.with(|l| l.get()) }
fn prefs() -> Value { PREFS.with(|p| p.borrow().clone()) }
fn kbps() -> i64 { prefs().i("kbps") }
fn lossy_codec() -> String { LOSSY.with(|l| l.clone()) }
/// `${n}` of a JS number ("0.9", "1", "52.3").
fn js_num(v: f64) -> String { v.to_string() }
/// `Number("…")` of an input value.
fn num(s: &str) -> f64 { s.trim().parse().unwrap_or(0.0) }
/// JS truthiness of a number.
fn truthy_num(x: f64) -> bool { x != 0.0 && !x.is_nan() }
fn session_storage() -> Option<web_sys::Storage> { dom::window().session_storage().ok().flatten() }
fn now_ms() -> f64 { js_sys::Date::now() }
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
/// `document.createElement('audio').canPlayType(mime) !== ''`.
fn can_play_type(mime: &str) -> bool {
    dom::create("audio").dyn_into::<HtmlMediaElement>().map(|a| !a.can_play_type(mime).is_empty()).unwrap_or(true)
}
fn detect_lossy() -> String {
    if can_play_type("audio/aac") { "aac".into() } else if can_play_type("audio/ogg; codecs=opus") { "opus".into() } else { "aac".into() }
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
fn link_state_value() -> Value { LINK.with(|l| l.borrow().as_ref().map(|l| l.state.clone()).unwrap_or(Value::Null)) }
fn link_name() -> String { LINK.with(|l| l.borrow().as_ref().map(|l| l.name.clone()).unwrap_or_default()) }
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
                sync_lyrics(false);
            }
        });
        LINK_TICK.with(|t| *t.borrow_mut() = Some(tick));
    }
    if now { cancel_preload(); }
    LYR.with(|l| *l.borrow_mut() = Lyr::default());
    render();
    if st(|s| s.full) { render_full(); load_lyrics(); }
}
/// `[id, playing, online, volume, queue.length, index]` – what a re-render of the bar depends on.
fn link_key(l: &Link) -> (Value, Value, bool, Value, usize, Value) {
    (l.state.g("id").clone(), l.state.g("playing").clone(), l.online, l.state.g("volume").clone(), l.state.a("queue").len(), l.state.g("index").clone())
}
/// New state from the linked device (every check-in).
pub fn link_state(s: Value, online: bool) {
    let changed = LINK.with(|l| {
        let mut guard = l.borrow_mut();
        let l = guard.as_mut()?;
        let before = link_key(l);
        l.state = if json::truthy(&s) { s } else { json!({}) };
        l.online = online;
        Some(before != link_key(l))
    });
    match changed {
        None => {}
        Some(true) => { render(); if st(|s| s.full) { render_full(); load_lyrics(); } }
        Some(false) => update_linked_time(),
    }
}
/// The linked device's song as a track object (what the full-screen view, likes and the ⋯ menu work on).
fn remote_track() -> Option<Value> {
    let s = link_state_value();
    if !s.truthy("title") { return None; }
    Some(json!({ "id": s.g("id"), "title": s.g("title"), "artist": s.g("artist"), "album": s.g("album"), "album_id": s.g("album_id"), "format": s.g("format"), "duration": s.g("duration"), "cover": s.g("cover"), "remote": true }))
}
/// The song in front of you: the linked device's when linked, else this device's.
fn now_track() -> Option<Value> { if is_linked() { remote_track() } else { current() } }
fn now_position() -> f64 { if is_linked() { link_state_value().f("position") } else { position() } }
fn now_playing() -> bool { if is_linked() { link_state_value().truthy("playing") } else { audio().is_some_and(|a| !a.paused()) } }

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
    let rate = CTX.with(|c| c.borrow().as_ref().map(|c| c.sample_rate() as f64));
    m.insert("signal".into(), json!({ "label": quality(&t), "transport": transport().map(|(k, _)| k), "output_rate": rate }));
    m.insert("index".into(), json!(st(|s| s.index)));
    let q: Vec<Value> = st(|s| s.queue.iter().filter(|x| x.g("id").is_number()).take(300).map(|x| slim(x)).collect());
    m.insert("queue".into(), Value::Array(q));
    Value::Object(m)
}
fn local_volume(v: f64) {
    let (Some(a), Some(sp)) = (audio(), spare()) else { return };
    let vol = 0f64.max(1f64.min(v));
    a.set_volume(vol);
    sp.set_volume(vol);
    dom::ls_set("encore.volume", &js_num(a.volume()));
    for id in ["vol", "fvol"] {
        if let Some(el) = by_id(id) { if num(&dom::value(&el)) != a.volume() { dom::set_value(&el, &js_num(a.volume())); } }
    }
    emit();
}
pub fn set_volume(v: f64) {
    if via_link(json!({ "action": "volume", "value": v })) {
        LINK.with(|l| if let Some(l) = l.borrow_mut().as_mut() { if let Some(o) = l.state.as_object_mut() { o.insert("volume".into(), json!(v)); } });
        return;
    }
    local_volume(v);
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
    load(false, 0.0, None, None);
}

pub fn enqueue(tracks: Vec<Value>) {
    if is_linked() && !is_local() {
        let out = sendable(&tracks);
        if !out.is_empty() { via_link(json!({ "action": "enqueue", "tracks": out })); }
        return;
    }
    st_mut(|s| s.queue.extend(tracks.into_iter().map(Rc::new)));
    if st(|s| s.index < 0) { st_mut(|s| s.index = 0); load(false, 0.0, None, None); } else { render(); emit(); preload_next(); }
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
// what this browser says it can decode, by file format (a Chromium build without AAC, Firefox on Linux, Safari without Ogg …)
fn mime_of(format: &str) -> Option<&'static str> {
    Some(match format {
        "mp3" => "audio/mpeg", "flac" => "audio/flac", "m4a" | "mp4" => r#"audio/mp4; codecs="mp4a.40.2""#, "aac" => "audio/aac",
        "ogg" | "oga" => "audio/ogg; codecs=vorbis", "opus" => "audio/ogg; codecs=opus", "wav" => "audio/wav", "webm" => "audio/webm",
        _ => return None,
    })
}
fn can_play(format: &str) -> bool {
    let f = format.to_lowercase();
    let Some(mime) = mime_of(&f) else { return false };
    PLAYABLE.with(|p| {
        let mut p = p.borrow_mut();
        if let Some(v) = p.get(&f) { return *v; }
        let v = can_play_type(mime);
        p.insert(f, v);
        v
    })
}

/// The rate asked of the server's Cadenza: this device's output rate, 0 (= the highest the server allows) or a fixed rate.
fn dsp_request_rate() -> f64 {
    let p = prefs();
    let r = p.g("dsp_rate");
    if r.as_str() == Some("device") { let o = output_rate().round(); return if truthy_num(o) { o } else { 0.0 }; }
    if r.as_str() == Some("auto") { return 0.0; }
    let n = p.f("dsp_rate");
    if truthy_num(n) { n } else { 0.0 }
}
/// The rate the server will send (its rule: at most max_rate, never below the file's; 0 → the highest multiple of the file's rate).
fn dsp_out_rate(t: &Value) -> f64 {
    let src = INFO.with(|m| m.borrow().get(&t.i("id")).map(|i| i.f("sample_rate"))).filter(|x| truthy_num(*x)).unwrap_or(44100.0);
    let max = { let m = dsp_info().f("max_rate"); if truthy_num(m) { m } else { 192000.0 } };
    let want = dsp_request_rate();
    let mut r = if want > 0.0 { want.min(max) } else { src };
    if want <= 0.0 { while r * 2.0 <= max { r *= 2.0; } }
    r.max(src)
}
/// Upsample this library song on the server? Only when switched on and allowed; Data saver / Auto's fallback win.
fn want_dsp(t: &Value, lossy: bool) -> bool {
    prefs().truthy("dsp") && dsp_info().truthy("allowed") && !t.truthy("preview") && t.g("id").is_number() && !lossy && !DSP_FAILED.with(|f| f.get())
}
/// transcode: FLAC made by the server · lossy: the smaller stream · dsp: upsampled by the server's Cadenza · offset: where a server-made stream starts
fn stream_url(t: &Value, transcode: bool, lossy: bool, dsp: bool, offset: f64) -> String {
    if t.truthy("preview") { return t.s("preview_url"); }
    let mut q: Vec<String> = vec![];
    if lossy { q.push(format!("q={}", lossy_codec())); q.push(format!("kbps={}", kbps())); }
    else if dsp {
        let p = prefs();
        q.push("dsp=1".into());
        q.push(format!("rate={}", js_num(dsp_request_rate())));
        if p.truthy("dsp_filter") { q.push(format!("filter={}", String::from(js_sys::encode_uri_component(&p.s("dsp_filter"))))); }
        q.push(format!("dither={}", String::from(js_sys::encode_uri_component(&p.s("dsp_dither")))));
    }
    else if transcode { q.push("transcode=1".into()); }
    if truthy_num(offset) { q.push(format!("t={}", to_fixed1(offset))); }
    format!("/api/library/tracks/{}/stream{}", t.s("id"), if q.is_empty() { String::new() } else { format!("?{}", q.join("&")) })
}
fn convert(t: &Value) -> bool { !t.truthy("preview") && (CONVERT.contains(&t.s("format").to_lowercase().as_str()) || !can_play(&t.s("format"))) }
/// Should this song start as the smaller stream? Data saver always; Auto for a while after the connection fell behind.
fn want_lossy(t: &Value) -> bool {
    let p = prefs();
    !t.truthy("preview") && (p.s("quality") == "saver" || (p.s("quality") == "auto" && now_ms() < LOSSY_UNTIL.with(|l| l.get())))
}

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

fn load(transcode: bool, offset: f64, lossy: Option<bool>, dsp: Option<bool>) {
    let Some(t) = cur() else { return };
    mark_played(&t);
    let transcode = transcode || convert(&t);
    let lossy = lossy.unwrap_or_else(|| want_lossy(&t));
    let is_lossy = !t.truthy("preview") && lossy;
    let is_dsp = dsp.unwrap_or(true) && want_dsp(&t, is_lossy);
    let dsp_rate = if is_dsp { dsp_out_rate(&t) } else { 0.0 };
    st_mut(|s| { s.lossy = is_lossy; s.dsp = is_dsp; s.dsp_rate = dsp_rate; });
    if is_dsp && !INFO.with(|m| m.borrow().contains_key(&t.i("id"))) {
        let t2 = t.clone();
        dom::spawn(async move {
            track_info(&t2).await;
            if cur().is_some_and(|c| Rc::ptr_eq(&c, &t2)) && st(|s| s.dsp) {
                let r = dsp_out_rate(&t2);
                st_mut(|s| s.dsp_rate = r);
                render(); emit();
            }
        });
    }
    let transcoded = !t.truthy("preview") && (transcode || lossy || is_dsp);
    st_mut(|s| {
        s.transcoded = transcoded;
        s.offset = offset;   // server-made streams restart at the seek point, so their clock starts at `offset`
        s.dragging = false;
    });
    if !truthy_num(offset) {
        st_mut(|s| { s.logged = false; s.stalls.clear(); s.switched = false; });
        // "now playing" on Last.fm / ListenBrainz (Encore 2)
        if t.g("id").is_number() && !st(|s| s.announced.as_ref().is_some_and(|a| Rc::ptr_eq(a, &t))) {
            st_mut(|s| s.announced = Some(t.clone()));
            let id = t.g("id").clone();
            dom::spawn(async move { let _ = api::post("/library/now-playing", json!({ "track_id": id })).await; });
        }
    }
    let url = stream_url(&t, transcoded && !is_lossy && !is_dsp, is_lossy, is_dsp, offset);
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
        // a pre-buffered processed stream holds one of the server's Cadenza slots: free it first
        if is_dsp && dom::data_opt(&sp, "url").is_some_and(|u| u.contains("dsp=1")) { cancel_preload(); }
        a.set_src(&url);
    }
    dom::set_data(&a, "url", &url);
    play(&a);
    st_mut(|s| s.slow_start = None);
    if !truthy_num(offset) && !is_lossy && !t.truthy("preview") && prefs().s("quality") == "auto" {
        // can't even get going: six seconds in and nothing has played yet → the smaller stream from the start
        let t2 = t.clone();
        let timer = Timeout::new(6000, move || {
            let same = cur().is_some_and(|c| Rc::ptr_eq(&c, &t2));
            let stuck = audio().is_some_and(|a| !a.paused() && a.current_time() < 0.5 && a.ready_state() < 3);
            if same && stuck { downgrade(); }
        });
        st_mut(|s| s.slow_start = Some(timer));
    }
    if has_media_session() { set_media_metadata(&t); }
    dom::add_class(&body(), "has-player");
    render();
    emit();
    if st(|s| s.full) { load_lyrics(); }
    top_up();
}

// ---- keeping the music going on a weak connection ---------------------------------------------
/// Seconds of this song already in the buffer past the play position.
fn buffered_ahead() -> f64 {
    let Some(a) = audio() else { return 0.0 };
    let b = a.buffered();
    let p = a.current_time();
    for i in 0..b.length() {
        let (Ok(s), Ok(e)) = (b.start(i), b.end(i)) else { continue };
        if s <= p + 0.5 && e >= p { return e - p; }
    }
    0.0
}
fn fully_buffered() -> bool {
    let Some(a) = audio() else { return false };
    let d = a.duration();
    if !truthy_num(d) || !d.is_finite() { return false; }
    let b = a.buffered();
    b.length() > 0 && b.end(b.length() - 1).unwrap_or(0.0) >= d - 1.0
}
/// Start buffering the song that plays next – once this one is safely ahead, never while it's short of bandwidth.
fn maybe_preload(force: bool) {
    let Some(sp) = spare() else { return };
    if !prefs().truthy("preload") || is_linked() { return; }
    let n = st(|s| if s.index + 1 < 0 { None } else { s.queue.get((s.index + 1) as usize).cloned() });
    let Some(n) = n else { return };
    let n_lossy = want_lossy(&n);
    let url = stream_url(&n, convert(&n), n_lossy, want_dsp(&n, n_lossy), 0.0);
    if dom::data_opt(&sp, "url").as_deref() == Some(url.as_str()) { return; }
    if !force {
        let t = cur();
        let remaining = duration() - position();
        let quiet = now_ms() - st(|s| s.stall_at) > 15000.0;
        let ok = if t.as_ref().is_some_and(|t| t.truthy("preview")) || st(|s| s.transcoded) { remaining < 40.0 }
            else { fully_buffered() || (quiet && buffered_ahead() > 45.0) || remaining < 30.0 };
        if !ok { return; }
    }
    dom::set_data(&sp, "url", &url);
    sp.set_src(&url);
    sp.load();
}
fn preload_next() { maybe_preload(true); }
fn cancel_preload() {
    let Some(sp) = spare() else { return };
    if dom::data_opt(&sp, "url").is_none() { return; }
    let _ = sp.remove_attribute("src");
    let _ = sp.remove_attribute("data-url");
    sp.load();
}
/// The browser ran out of buffer mid-song. Give it the bandwidth back, and on Auto quality drop to the smaller stream.
fn stalled() {
    let now = now_ms();
    let paused = audio().map(|a| a.paused()).unwrap_or(true);
    if now - st(|s| s.seek_at) < 3000.0 || position() < 1.0 || paused { return; }   // a seek or the start, not a slow link
    st_mut(|s| { s.stall_at = now; s.stalls.push(now); });
    if !fully_buffered() { cancel_preload(); }
    if prefs().s("quality") != "auto" || st(|s| s.lossy) || cur().is_some_and(|t| t.truthy("preview")) || st(|s| s.switched) { return; }
    let recent = st(|s| s.stalls.iter().filter(|x| now - **x < 90000.0).count());
    if recent >= 2 { downgrade(); return; }
    // one long stall counts too: still waiting four seconds later
    Timeout::new(4000, move || {
        let waiting = audio().is_some_and(|a| !a.paused() && a.ready_state() < 3);
        if waiting && st(|s| s.stall_at == now && !s.switched) { downgrade(); }
    }).forget();
}
fn downgrade() {
    let Some(t) = cur() else { return };
    if t.truthy("preview") || st(|s| s.lossy) { return; }
    st_mut(|s| s.switched = true);
    LOSSY_UNTIL.with(|l| l.set(now_ms() + 10.0 * 60000.0));
    load(true, position(), Some(true), None);
    toast(&format!("Connection can't keep up – playing a smaller {} kbps stream for a while (Settings → Playback)", kbps()), "");
    st_mut(|s| s.slow_start = None);
}

/// Near the end of the queue: fetch the library shuffle ahead of time, so the next song is ready too.
fn top_up() {
    if TOPPING.with(|t| t.get()) || st(|s| s.context == "preview" || s.index < s.queue.len() as i64 - 2) { return; }
    if !prefs().truthy("autoplay") { return; }
    TOPPING.with(|t| t.set(true));
    dom::spawn(async move {
        if let Ok(more) = fresh_shuffle(50).await {
            if !more.is_empty() {
                st_mut(|s| s.queue.extend(more.into_iter().map(Rc::new)));
                render(); emit();
                maybe_preload(false);
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
    st_mut(|s| s.seek_at = now_ms());
    if st(|s| s.transcoded) { let (lossy, dsp) = st(|s| (s.lossy, s.dsp)); load(true, sec, Some(lossy), Some(dsp)); }      // ask the server to start the stream at that point
    else if let Some(a) = audio() { a.set_current_time(sec); }
    update_time();
    emit();
}

pub fn toggle() {
    if via_link(json!({ "action": "toggle" })) {
        LINK.with(|l| if let Some(l) = l.borrow_mut().as_mut() {
            let p = !l.state.truthy("playing");
            if let Some(o) = l.state.as_object_mut() { o.insert("playing".into(), json!(p)); }
        });
        render();
        if st(|s| s.full) { render_full(); }
        return;
    }
    if cur().is_some() {
        if let Some(a) = audio() { if a.paused() { play(&a); } else { let _ = a.pause(); } }
    }
}

pub fn next(auto: bool) {
    if !auto && via_link(json!({ "action": "next" })) { return; }
    if auto && st(|s| s.repeat == "one") { seek_to(0.0); if let Some(a) = audio() { play(&a); } return; }
    if st(|s| s.index < s.queue.len() as i64 - 1) { st_mut(|s| s.index += 1); load(false, 0.0, None, None); return; }
    if st(|s| s.repeat == "all" && !s.queue.is_empty()) { st_mut(|s| s.index = 0); load(false, 0.0, None, None); return; }
    if st(|s| s.context == "preview") { return; }
    dom::spawn(async move {
        if continue_with_library().await { st_mut(|s| s.index += 1); load(false, 0.0, None, None); }
    });
}

/// Queue ran out: keep the music going with a shuffle of the whole library.
async fn continue_with_library() -> bool {
    if !prefs().truthy("autoplay") { return false; }
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
    load(false, 0.0, None, None);
}

fn jump(i: i64) { st_mut(|s| s.index = i); load(false, 0.0, None, None); }
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

// ---- likes, sharing and the ⋯ menu for the song in front of you -------------------------------------
async fn likes() -> HashSet<i64> {
    if LIKED.with(|l| l.borrow().is_none()) {
        let set: HashSet<i64> = match api::get("/likes").await { Ok(v) => v.items().iter().filter_map(|x| x.as_i64()).collect(), Err(_) => HashSet::new() };
        LIKED.with(|l| *l.borrow_mut() = Some(set));
    }
    LIKED.with(|l| l.borrow().clone().unwrap_or_default())
}
fn is_liked(t: &Value) -> bool { t.g("id").is_number() && LIKED.with(|l| l.borrow().as_ref().is_some_and(|s| s.contains(&t.i("id")))) }
fn can_like(t: Option<&Value>) -> bool { t.is_some_and(|t| t.g("id").is_number()) }
async fn toggle_like(t: Value) {
    if !t.g("id").is_number() { return; }
    let id = t.i("id");
    let s = likes().await;
    let r = if s.contains(&id) {
        api::del(&format!("/likes/{id}")).await.map(|_| { LIKED.with(|l| if let Some(s) = l.borrow_mut().as_mut() { s.remove(&id); }); toast("Removed from Liked Songs", "ok"); })
    } else {
        api::post_empty(&format!("/likes/{id}")).await.map(|_| { LIKED.with(|l| if let Some(s) = l.borrow_mut().as_mut() { s.insert(id); }); toast("Added to Liked Songs", "ok"); })
    };
    if let Err(e) = r { toast(&e.message, "err"); }
    dom::dispatch("encore:likes", None);
    paint_likes();
}
/// Update every heart on screen without redrawing the whole bar.
fn paint_likes() {
    let t = now_track();
    let on = t.as_ref().is_some_and(is_liked);
    for b in dom::qsa(".btn.like") {
        dom::toggle_class(&b, "on", on);
        let _ = b.set_attribute("title", if on { "Liked – tap to remove" } else { "Like" });
    }
}
fn like_button(id: &str, cls: &str) -> String {
    format!(r#"<button class="btn ghost icon like {cls}" id="{id}" aria-label="Like" title="Like">{}</button>"#, icon("heart"))
}
/// `if (liked === null && canLike(t)) likes().then(paintLikes)`.
fn warm_likes(t: Option<&Value>) {
    if LIKED.with(|l| l.borrow().is_none()) && can_like(t) { dom::spawn(async { likes().await; paint_likes(); }); }
}

/// "⋯" for the song in front of you (this device's or the linked device's).
fn more_menu(anchor: &Element) {
    let Some(t) = now_track() else { return };
    let lib = t.g("id").is_number();
    let linked = is_linked();
    let mut items: Vec<Option<MenuItem>> = vec![];
    if t.truthy("artist") && !t.truthy("preview") { items.push(MenuItem::link("Go to artist", &icon("user"), &format!("#/artist-name/{}", String::from(js_sys::encode_uri_component(&t.s("artist")))), false)); }
    if t.truthy("album_id") { items.push(MenuItem::link("View album", &icon("disc"), &format!("#/library/album/{}", t.s("album_id")), false)); }
    if lib { let t2 = t.clone(); items.push(MenuItem::new(if is_liked(&t) { "Remove from Liked Songs" } else { "Like" }, &icon("heart"), move || dom::spawn(toggle_like(t2.clone())))); }
    if lib { let id = t.i("id"); items.push(MenuItem::new("Add to playlist", &icon("plus"), move || crate::pages::playlists::add_to_playlist_modal(vec![id]))); }
    if lib { items.push(MenuItem::new("Lyrics", &icon("music"), || { st_mut(|s| s.pane = "lyrics".into()); set_full(true); })); }
    if lib { let t2 = t.clone(); items.push(MenuItem::new("Share link", &icon("external"), move || dom::spawn(share_track(t2.clone())))); }
    if lib { items.push(MenuItem::link("Download this song", &icon("download"), &format!("/api/library/tracks/{}/file", t.s("id")), true)); }
    if lib && !linked && can_save_offline() { let t2 = t.clone(); items.push(MenuItem::new("Save for offline", &icon("download"), move || { save_offline(vec![t2.clone()]); toast("Saving to this phone…", "ok"); })); }
    items.push(MenuItem::new("Signal path", &icon("activity"), || dom::spawn(signal_path())));
    if has_encore_remote() { items.push(MenuItem::new(if linked { "Your devices…" } else { "Play on another device…" }, &icon("devices"), remote_open)); }
    pop_menu(anchor, items);
}

// ---- signal path: what the file is, what Encore sends, what this device does with it -----------------
async fn track_info(t: &Value) -> Option<Value> {
    if !t.g("id").is_number() { return None; }
    let id = t.i("id");
    if let Some(i) = INFO.with(|m| m.borrow().get(&id).cloned()) { return Some(i); }
    match api::get(&format!("/library/tracks/{id}/info")).await {
        Ok(i) => { INFO.with(|m| m.borrow_mut().insert(id, i.clone())); Some(i) }
        Err(_) => None,
    }
}
/// What leaves the server for this device: `(kind, text)`.
fn transport() -> Option<(String, String)> {
    let t = cur()?;
    if t.truthy("preview") { return Some(("preview".into(), "A 30-second preview from the catalogue (lossy) – not your file".into())); }
    if st(|s| s.lossy) {
        let codec = lossy_codec();
        let why = if prefs().s("quality") == "saver" { "Data saver is on" } else { "the connection could not keep up with the original" };
        return Some((format!("{codec}-{}", kbps()), format!("{} {} kbps (lossy) – {why}", codec.to_uppercase(), kbps())));
    }
    if st(|s| s.dsp) {
        let r = st(|s| s.dsp_rate);
        return Some((format!("cadenza-{}", js_num(r)), format!("Upsampled on the server by Cadenza to 24-bit / {} kHz (lossless FLAC)", khz(r))));
    }
    if st(|s| s.transcoded) { return Some(("flac".into(), format!("Converted to FLAC by the server (lossless) – browsers can't play {}", t.s("format").to_uppercase()))); }
    Some(("original".into(), "The original file, byte for byte – nothing was converted or resampled on the way".into()))
}
/// This device's output sample rate in Hz (0 if unknown) – Settings → Playback shows it for "Match this device".
pub fn device_rate() -> f64 { output_rate() }
fn output_rate() -> f64 {
    CTX.with(|c| {
        let mut c = c.borrow_mut();
        if c.is_none() { *c = web_sys::AudioContext::new().ok(); }
        c.as_ref().map(|x| x.sample_rate() as f64).unwrap_or(0.0)
    })
}
/// `String(Math.round(hz / 100) / 10)`.
pub(crate) fn khz(hz: f64) -> String { js_num((hz / 100.0).round() / 10.0) }
fn source_line(t: &Value, info: Option<&Value>) -> String {
    let Some(info) = info else {
        return format!("{}{}", t.s("format").to_uppercase(), if t.truthy("bitrate") { format!(" · {} kbps", t.s("bitrate")) } else { String::new() });
    };
    let mut parts = vec![(if info.truthy("codec") { info.s("codec") } else { t.s("format") }).to_uppercase()];
    if info.truthy("lossless") && info.truthy("bit_depth") && info.truthy("sample_rate") { parts.push(format!("{}-bit / {} kHz", info.s("bit_depth"), khz(info.f("sample_rate")))); }
    else if info.truthy("sample_rate") { parts.push(format!("{} kHz", khz(info.f("sample_rate")))); }
    if info.truthy("channels") {
        let ch = info.i("channels");
        parts.push(if ch == 2 { "stereo".into() } else if ch == 1 { "mono".into() } else { format!("{ch} channels") });
    }
    if info.truthy("bitrate") { parts.push(format!("{} kbps", info.s("bitrate"))); }
    parts.join(" · ")
}
/// The Signal path window: three rows, and a plain verdict on whether what you hear is the file untouched.
async fn signal_path() {
    let Some(t) = now_track() else { return };
    let m = crate::ui::modal(ModalOpts::new("Signal path", r#"<div class="skeleton" style="height:120px"></div>"#));
    let info = track_info(&t).await;
    let mut rows: Vec<(String, String, String)> = vec![];
    rows.push(("Source".into(), source_line(&t, info.as_ref()), if info.as_ref().is_some_and(|i| i.g("lossless") == &json!(false)) { "A lossy file: what the file lost is gone before Encore touches it".into() } else { String::new() }));
    let linked = linked();
    let pure;
    let cadenza = linked.is_none() && transport().is_some_and(|(k, _)| k.starts_with("cadenza"));
    if let Some(l) = &linked {
        let sig = l.state.o("signal");
        let tr = sig.s("transport");
        let what = if tr == "original" { "The original file, byte for byte".to_string() }
            else if tr.is_empty() { "Not reported by that device".to_string() }
            else if tr == "flac" { "Converted to FLAC by the server (lossless)".to_string() }
            else if let Some(rate) = tr.strip_prefix("cadenza-") { let n = num(rate); format!("Upsampled on the server by Cadenza ({} kHz)", khz(if truthy_num(n) { n } else { 0.0 })) }
            else if tr.starts_with("aac-") || tr.starts_with("opus-") { let (c, k) = tr.split_once('-').unwrap_or((&tr, "")); format!("{} {k} kbps (lossy)", c.to_uppercase()) }
            else { tr.clone() };
        rows.push((format!("Encore → {}", esc(&l.name)), what, String::new()));
        rows.push((esc(&l.name), if sig.truthy("output_rate") { format!("Plays through its browser audio at {} kHz", khz(sig.f("output_rate"))) } else { "Playing there".into() }, String::new()));
        pure = false;
    } else {
        let tr = transport();
        rows.push(("Encore → this device".into(), tr.as_ref().map(|(_, x)| x.clone()).unwrap_or_default(), String::new()));
        let out = output_rate();
        let src = info.as_ref().map(|i| i.f("sample_rate")).unwrap_or(0.0);
        let resampled = truthy_num(out) && truthy_num(src) && out != src;
        rows.push(("This device".into(),
            if truthy_num(out) { format!("Browser audio → the system mixer at {} kHz", khz(out)) } else { "Browser audio → the system mixer".into() },
            if resampled { format!("Your device resamples {} → {} kHz. A browser can't hand bits straight to the DAC; for true bit-perfect output play through a Cadenza device or an exclusive-mode player.", khz(src), khz(out)) }
            else if truthy_num(out) && truthy_num(src) { "Same rate as the file – no resampling in the browser. Whether the operating system mixes it with other sounds is up to your audio settings.".into() }
            else { String::new() }));
        pure = tr.as_ref().is_some_and(|(k, _)| k == "original") && !info.as_ref().is_some_and(|i| i.g("lossless") == &json!(false));
    }
    let Some(body_el) = m.qs(".modal-body") else { return };
    let rows_html: String = rows.iter().map(|(k, v, note)| format!(r#"<div class="stack" style="gap:2px"><div class="small muted">{k}</div><div style="font-weight:600">{}</div>{}</div>"#,
        esc(v), if note.is_empty() { String::new() } else { format!(r#"<div class="small muted">{}</div>"#, esc(note)) })).collect();
    let (head, text) = if linked.is_some() {
        ("Playing on another device", "The rows above are what that device reports. Open Encore there for its own signal path.")
    } else if cadenza {
        ("Processed by Cadenza on the server", "Encore upsampled the file on the server with Cadenza and sent lossless 24-bit FLAC. Turn it off under Settings → Playback → Server processing to get the untouched file.")
    } else if pure {
        ("Pure playback from Encore", "Encore sent the file exactly as it is stored. Everything after that – volume, mixing, resampling – is your browser and operating system.")
    } else {
        ("Not the untouched file", "Encore converted this stream (see above). Settings → Playback → Quality → Original keeps the file untouched even when the connection is slow.")
    };
    set_html(&body_el, &format!(r#"
    <div class="stack" style="gap:12px">
      {rows_html}
      <div class="card" style="padding:12px"><b>{head}</b>
        <div class="small muted" style="margin-top:4px">{text}</div></div>
    </div>"#));
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
/// `maybePreload()` at most every two seconds, from timeupdate and progress.
fn preload_tick() {
    let now = now_ms();
    if now - st(|s| s.last_preload_check) > 2000.0 { st_mut(|s| s.last_preload_check = now); maybe_preload(false); }
}

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
    on_both(&a, &sp, "play", Rc::new(|_| { render(); emit(); }));
    on_both(&a, &sp, "pause", Rc::new(|_| { render(); emit(); }));
    on_both(&a, &sp, "timeupdate", Rc::new(|_| { update_time(); log_play(); sync_lyrics(false); preload_tick(); }));
    on_both(&a, &sp, "progress", Rc::new(|_| preload_tick()));
    on_both(&a, &sp, "waiting", Rc::new(|_| stalled()));
    on_both(&a, &sp, "loadedmetadata", Rc::new(|_| update_time()));
    // the server's Cadenza is off, busy or failed → the same spot again without processing, and no more processing this session
    // the browser couldn't decode the original (e.g. ALAC in .m4a) → ask the server for FLAC
    on_both(&a, &sp, "error", Rc::new(|_| {
        let Some(t) = cur() else { return };
        let has_src = audio().and_then(|a| a.get_attribute("src")).is_some_and(|s| !s.is_empty());
        if !has_src { return; }
        if st(|s| s.dsp) { dsp_fallback(); return; }
        if !st(|s| s.transcoded) && !t.truthy("preview") { load(true, position(), None, None); }
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
        if dom::key(e) == "Escape" && st(|s| s.full) { if dom::qs(".modal-bg, .menu-pop").is_none() { set_full(false); } return; }   // a window on top closes first
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
    dom::on_window("encore:likes", |_| { LIKED.with(|l| *l.borrow_mut() = None); dom::spawn(async { likes().await; paint_likes(); }); });
    dom::on_window("encore:playback-prefs", |_| {
        PREFS.with(|p| *p.borrow_mut() = playback::current());
        fetch_dsp();
        if !prefs().truthy("preload") { cancel_preload(); } else { maybe_preload(true); }
        if st(|s| s.full) { render_full(); }
    });
    dom::on_window("storage", |e| {
        let key = e.dyn_ref::<web_sys::StorageEvent>().and_then(|s| s.key());
        if key.as_deref() == Some("encore.playback") { PREFS.with(|p| *p.borrow_mut() = playback::current()); }
    });
    fetch_dsp();
    render();
}

/// The server's Cadenza is off, busy or failed: the same spot again without processing, and no more processing this session.
fn dsp_fallback() {
    DSP_FAILED.with(|f| f.set(true));
    let at = position();
    if !DSP_TOASTED.with(|t| t.replace(true)) { toast("Server processing isn't available right now – playing the original", ""); }
    load(at > 0.5, if at > 0.5 { at } else { 0.0 }, None, Some(false));
}

pub fn set_full(on: bool) {
    st_mut(|s| s.full = on);
    let Some(f) = by_id("np-full") else { return };
    dom::toggle_class(&f, "hidden", !on);
    dom::toggle_class(&body(), "np-open", on);
    if on { render_full(); load_lyrics(); }
}

fn update_time() {
    if is_linked() { update_linked_time(); return; }
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
    if t.truthy("remote") {
        let label = link_state_value().g("signal").s("label");
        return if label.is_empty() { t.s("format").to_uppercase() } else { label };
    }
    if t.truthy("preview") { return "30 s preview".into(); }
    let f = t.s("format").to_uppercase();
    if st(|s| s.lossy) { return format!("{f} → {} {}k", lossy_codec().to_uppercase(), kbps()); }
    if st(|s| s.dsp) { return format!("{f} → Cadenza {} kHz", khz(st(|s| s.dsp_rate))); }
    if st(|s| s.transcoded) { format!("{f} → FLAC") }
    else { format!("{f}{}", if t.truthy("bitrate") { format!(" {}k", t.s("bitrate")) } else { String::new() }) }
}
fn quality_title(t: &Value) -> String {
    if t.truthy("remote") { return format!("What {} is playing", { let n = link_name(); if n.is_empty() { "that device".to_string() } else { n } }); }
    if t.truthy("preview") { return "A 30-second preview – not in your library".into(); }
    if st(|s| s.lossy) { return "A smaller lossy stream, because the connection could not keep up (or Data saver is on)".into(); }
    if st(|s| s.dsp) { return "Upsampled on the server by Cadenza (lossless 24-bit FLAC)".into(); }
    if st(|s| s.transcoded) { "Converted to FLAC for your browser (lossless)".into() } else { "Original file, no conversion".into() }
}

/// `link ? (link.state?.duration || 0) : duration()`.
fn seek_duration() -> f64 { if is_linked() { link_state_value().f("duration") } else { duration() } }
fn bind_seek(out: &mut Vec<EventListener>, seek: &Element, cur_id: &str) {
    out.push(dom::listener(seek, "pointerdown", |_| st_mut(|s| s.dragging = true)));
    out.push(dom::listener(seek, "touchstart", |_| st_mut(|s| s.dragging = true)));   // passive, like the JS
    let (sk, cid) = (seek.clone(), cur_id.to_string());
    out.push(dom::listener(seek, "input", move |_| {
        st_mut(|s| s.dragging = true);
        if let Some(c) = by_id(&cid) { dom::set_text(&c, &fmt::dur((num(&dom::value(&sk)) / 1000.0) * seek_duration())); }
        set_p(&sk);
    }));
    let sk = seek.clone();
    let commit: Rc<dyn Fn()> = Rc::new(move || {
        if st(|s| s.dragging) { st_mut(|s| s.dragging = false); seek_to((num(&dom::value(&sk)) / 1000.0) * seek_duration()); }
    });
    for ev in ["change", "pointerup", "touchend"] {
        let c = commit.clone();
        out.push(dom::listener(seek, ev, move |_| c()));
    }
}

fn update_linked_time() {
    if !is_linked() || st(|x| x.dragging) { return; }
    let s = link_state_value();
    let dur = s.f("duration");
    let pos = s.f("position").min(if truthy_num(dur) { dur } else { f64::INFINITY });
    for (sid, cid, did) in [("seek", "t-cur", "t-dur"), ("fseek", "f-cur", "f-dur")] {
        let Some(seek) = by_id(sid) else { continue };
        dom::set_value(&seek, &js_num(if truthy_num(dur) { (pos / dur) * 1000.0 } else { 0.0 }));
        set_p(&seek);
        if let Some(c) = by_id(cid) { dom::set_text(&c, &fmt::dur(pos)); }
        if let Some(d) = by_id(did) { dom::set_text(&d, &fmt::dur(dur)); }
    }
}

/// The extra buttons on the right of the bar: like, ⋯, up next, devices, volume (and close when playing here).
fn right_html(t: Option<&Value>, closable: bool) -> String {
    let badge = match t { Some(t) if !t.truthy("preview") => format!(r#"<span class="badge hide-sm" title="{}">{}</span>"#, esc(&quality_title(t)), esc(&quality(t))), _ => String::new() };
    let like = if can_like(t) { like_button("p-like", if t.is_some_and(is_liked) { "on" } else { "" }) } else { String::new() };
    let devices = if has_encore_remote() { format!(r#"<button class="btn ghost icon" id="p-devices" aria-label="Other devices" title="Play on or control your other devices">{}</button>"#, icon("devices")) } else { String::new() };
    let vol = if is_linked() { let s = link_state_value(); if s.is_nullish("volume") { "0.9".to_string() } else { s.s("volume") } } else { js_num(audio().map(|a| a.volume()).unwrap_or(0.9)) };
    format!(r#"<div class="right">
      {badge}
      {like}
      <button class="btn ghost icon" id="p-more" aria-label="More" title="More">{more}</button>
      <button class="btn ghost icon" id="p-queue" aria-label="Up next" title="Up next">{list}</button>
      {devices}
      <span class="faint vol-ic">{volume}</span>
      <input type="range" id="vol" min="0" max="1" step="0.01" value="{vol}">
      {close}
    </div>"#,
        more = icon("more"), list = icon("list"), volume = icon_attr("volume", r#"width="18""#),
        close = if closable { format!(r#"<button class="btn ghost icon" id="p-close" aria-label="Close player">{}</button>"#, icon("x")) } else { String::new() })
}
fn bind_right(ls: &mut Vec<EventListener>, el: &Element) {
    wire(ls, el, "#p-like", "click", |_| { if let Some(t) = now_track() { dom::spawn(toggle_like(t)); } });
    wire(ls, el, "#p-more", "click", |e| { if let Some(b) = dom::closest(e, "#p-more") { more_menu(&b); } });
    wire(ls, el, "#p-queue", "click", |_| { st_mut(|s| s.pane = "queue".into()); set_full(true); });
    wire(ls, el, "#p-devices", "click", |_| remote_open());
    if is_linked() {
        wire(ls, el, "#vol", "change", |e| { if let Some(v) = dom::target_el(e) { set_volume(num(&dom::value(&v))); } });
    } else {
        wire(ls, el, "#vol", "input", |e| { if let Some(v) = dom::target_el(e) { local_volume(num(&dom::value(&v))); } });
    }
    warm_likes(now_track().as_ref());
}

fn render_linked(el: &Element) {
    let Some(l) = linked() else { return };
    let s = l.state.clone();
    let t = remote_track();
    let name = esc(&l.name);
    let where_ = format!(r#"<span class="linked-to" title="This device is a remote for {name}">{}{}Playing on {name}</span>"#, icon("link"), if l.online { "" } else { "Offline · " });
    dom::add_class(&body(), "has-player");
    let has_title = s.truthy("title");
    let thumb = if has_title { art(&s.s("cover"), &album_or_title(&s), "") } else { format!(r#"<div class="ph">{}</div>"#, icon_attr("devices", r#"width="20""#)) };
    let title = if has_title { esck(&s, "title") } else { "Nothing playing".to_string() };
    let sub = if has_title {
        format!("{}{}", esc(&s.s("artist")), if s.truthy("album") { format!(" · {}", esck(&s, "album")) } else { String::new() })
    } else { "Play anything – it starts there".to_string() };
    let right = right_html(t.as_ref(), false).replacen(r#"<div class="right">"#, &format!(r#"<div class="right"><button class="btn sm hide-sm" id="p-unlink" title="Stop being a remote and play on this device again">{}Unlink</button>"#, icon("link")), 1);
    set_html(el, &format!(r##"
    <div class="now" id="p-open" role="button" tabindex="0" aria-label="Open the full-screen player" title="Open player">
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
    {right}"##,
        prev = icon("prev"), pp = if s.truthy("playing") { icon("pause") } else { icon("play") }, next = icon("next")));
    let mut ls = Vec::new();
    wire(&mut ls, el, "#p-toggle", "click", |_| toggle());
    wire(&mut ls, el, "#p-next", "click", |_| next(false));
    wire(&mut ls, el, "#p-prev", "click", |_| prev());
    wire(&mut ls, el, "#p-open", "click", |_| { st_mut(|s| s.pane = "now".into()); set_full(true); });
    wire(&mut ls, el, "#p-open", "keydown", |e| { if dom::key(e) == "Enter" { set_full(true); } });
    wire(&mut ls, el, "#p-unlink", "click", |_| remote_unlink());
    bind_right(&mut ls, el);
    if let Some(seek) = qs_in(el, "#seek") { bind_seek(&mut ls, &seek, "t-cur"); }
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
    {right}"##,
        thumb = art(&cover(&t), &album_or_title(&t), ""),
        title = esck(&t, "title"),
        artist = esck(&t, "artist"),
        album = if t.truthy("album") { format!(" · {}", esck(&t, "album")) } else { String::new() },
        prev = icon("prev"), pp = if a.paused() { icon("play") } else { icon("pause") }, next = icon("next"),
        right = right_html(Some(&t), true)));
    let mut ls = Vec::new();
    wire(&mut ls, &el, "#p-toggle", "click", |_| toggle());
    wire(&mut ls, &el, "#p-next", "click", |_| next(false));
    wire(&mut ls, &el, "#p-prev", "click", |_| prev());
    wire(&mut ls, &el, "#p-open", "click", |_| { st_mut(|s| s.pane = "now".into()); set_full(true); });
    wire(&mut ls, &el, "#p-open", "keydown", |e| { if dom::key(e) == "Enter" { set_full(true); } });
    bind_right(&mut ls, &el);
    if let Some(seek) = qs_in(&el, "#seek") { bind_seek(&mut ls, &seek, "t-cur"); }
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
    let link = linked();
    let Some(t) = now_track() else {
        set_html(&f, &format!(r#"<div class="np-top"><button class="btn ghost icon" id="f-close" aria-label="Close">{}</button><span></span><span></span></div>
      <div class="empty">{}<h2>Nothing playing on {}</h2><p class="muted">Play anything – it starts there.</p></div>"#,
            icon("x"), icon("devices"), esc(&link.as_ref().map(|l| l.name.clone()).unwrap_or_else(|| "this device".into()))));
        let mut ls = Vec::new();
        wire(&mut ls, &f, "#f-close", "click", |_| set_full(false));
        set_full_listeners(ls);
        return;
    };
    let s = link.as_ref().map(|l| l.state.clone()).unwrap_or(json!({}));
    let (idx, pane, repeat, q): (i64, String, String, Vec<Value>) = if link.is_some() {
        st(|x| (if s.is_nullish("index") { -1 } else { s.i("index") }, x.pane.clone(), x.repeat.clone(), s.a("queue")))
    } else {
        st(|x| (x.index, x.pane.clone(), x.repeat.clone(), x.queue.iter().map(|t| (**t).clone()).collect()))
    };
    let up_next: Vec<Value> = q.iter().skip((idx + 1).max(0) as usize).cloned().collect();
    let playing = now_playing();
    let volume = if link.is_some() { if s.is_nullish("volume") { "0.9".to_string() } else { s.s("volume") } } else { js_num(audio().map(|a| a.volume()).unwrap_or(0.9)) };
    let autoplay = prefs().truthy("autoplay");
    dom::set_style(&f, "--np-bg", &format!(r#"url("{}")"#, cover(&t)));
    let tab = |p: &str, label: &str| format!(r#"<button data-pane="{p}" class="{}">{label}</button>"#, if pane == p { "active" } else { "" });
    let artist = if t.truthy("preview") { esck(&t, "artist") } else {
        format!(r##"<a href="#/artist-name/{}">{}</a>"##, String::from(js_sys::encode_uri_component(&t.s("artist"))), esck(&t, "artist"))
    };
    let album = if t.truthy("album") {
        format!(" · {}", if t.truthy("album_id") { format!(r##"<a href="#/library/album/{}">{}</a>"##, esck(&t, "album_id"), esck(&t, "album")) } else { esck(&t, "album") })
    } else { String::new() };
    let device_btn = if has_encore_remote() {
        format!(r#"<button class="btn sm ghost np-device" id="f-device" title="Choose where the music plays">{}<span class="hide-sm">{}</span>{}</button>"#,
            icon("devices"), match &link { Some(l) => format!("Playing on {}", esc(&l.name)), None => "This device".into() }, icon("chevron"))
    } else { "<span></span>".to_string() };
    let list = if !up_next.is_empty() {
        up_next.iter().take(200).enumerate().map(|(k, x)| {
            let i = idx + 1 + k as i64;
            let btns = if link.is_some() { String::new() } else {
                format!(r#"<button class="btn ghost icon sm" data-up="{i}" aria-label="Move up" {}>↑</button>
            <button class="btn ghost icon sm" data-rm="{i}" aria-label="Remove">{}</button>"#, if i == idx + 1 { "disabled" } else { "" }, icon("x"))
            };
            format!(r#"<div class="np-item" data-i="{i}">
            <div class="thumb">{}</div>
            <div class="grow" style="min-width:0"><div class="ellipsis" style="font-weight:600">{}</div><div class="small muted ellipsis">{}</div></div>
            {btns}</div>"#, art(&cover(x), &album_or_title(x), ""), esck(x, "title"), esck(x, "artist"))
        }).collect::<String>()
    } else {
        format!(r#"<div class="muted small" style="padding:16px 4px">{}</div>"#,
            if link.is_some() { "Nothing queued there." } else if autoplay { "Nothing queued – a shuffle of your library plays next." } else { "Nothing queued." })
    };
    let actions = format!(r#"<div class="np-actions">
          {}
          {}
          <button class="btn ghost icon" id="f-more" aria-label="More" title="More">{}</button>
        </div>"#,
        if can_like(Some(&t)) { like_button("f-like", if is_liked(&t) { "on" } else { "" }) } else { String::new() },
        if can_like(Some(&t)) { format!(r#"<button class="btn ghost icon" id="f-share" aria-label="Share" title="Share link">{}</button>"#, icon("external")) } else { String::new() },
        icon("more"));
    set_html(&f, &format!(r##"
    <div class="np-backdrop"></div>
    <div class="np-top">
      <button class="btn ghost icon" id="f-close" aria-label="Close">{x}</button>
      <div class="tabs np-tabs">{tab_now}{tab_lyrics}{tab_queue}</div>
      {device_btn}
    </div>
    <div class="np-body pane-{pane}">
      <section class="np-main">
        <div class="np-art">{big_art}</div>
        <div class="np-meta">
          <h2 class="ellipsis">{title}</h2>
          <div class="muted ellipsis">{artist}{album}</div>
          <button class="badge mt np-quality" id="f-signal" title="Signal path: what the file is, what Encore sends and what this device does with it">{quality}</button>
        </div>
        <div class="seek np-seek"><span id="f-cur">0:00</span><input type="range" id="fseek" min="0" max="1000" value="0"><span id="f-dur">0:00</span></div>
        <div class="np-ctrls">
          {shuffle}
          <button class="btn ghost icon" id="f-prev" aria-label="Previous">{prev}</button>
          <button class="play-btn lg" id="f-toggle" aria-label="Play/Pause">{pp}</button>
          <button class="btn ghost icon" id="f-next" aria-label="Next">{next}</button>
          {repeat}
        </div>
        <div class="row np-vol"><span class="faint">{volume_ic}</span><input type="range" id="fvol" min="0" max="1" step="0.01" value="{vol}"></div>
        {actions}
      </section>
      <section class="np-lyrics" id="np-lyrics">{lyrics}</section>
      <section class="np-queue">
        <div class="row between"><h3>Up next{on_name}</h3>
          {auto}</div>
        <div class="np-list">{list}</div>
      </section>
    </div>"##,
        x = icon("x"), tab_now = tab("now", "Now playing"), tab_lyrics = tab("lyrics", "Lyrics"), tab_queue = tab("queue", "Up next"),
        big_art = art(&cover(&t), &album_or_title(&t), "big"), title = esck(&t, "title"), quality = esc(&quality(&t)),
        shuffle = if link.is_some() { r#"<span class="np-gap"></span>"#.to_string() } else { format!(r#"<button class="btn ghost icon" id="f-shuffle" title="Shuffle up next" aria-label="Shuffle up next">{}</button>"#, icon("shuffle")) },
        prev = icon("prev"), pp = if playing { icon("pause") } else { icon("play") }, next = icon("next"),
        repeat = if link.is_some() { r#"<span class="np-gap"></span>"#.to_string() } else {
            format!(r#"<button class="btn ghost icon {}" id="f-repeat" title="Repeat: {repeat}" aria-label="Repeat">{}{}</button>"#, if repeat != "off" { "on" } else { "" }, icon("refresh"), if repeat == "one" { "<sup>1</sup>" } else { "" })
        },
        volume_ic = icon_attr("volume", r#"width="18""#), vol = volume, lyrics = lyrics_html(),
        on_name = link.as_ref().map(|l| format!(" on {}", esc(&l.name))).unwrap_or_default(),
        auto = if link.is_some() { String::new() } else { format!(r#"<label class="row small muted" style="gap:6px" title="When the queue ends, keep playing a shuffle of the whole library"><input type="checkbox" id="f-auto" {}> Then shuffle library</label>"#, if autoplay { "checked" } else { "" }) }));
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
    wire(&mut ls, &f, "#f-device", "click", |_| remote_open());
    wire(&mut ls, &f, "#f-signal", "click", |_| dom::spawn(signal_path()));
    wire(&mut ls, &f, "#f-toggle", "click", |_| toggle());
    wire(&mut ls, &f, "#f-next", "click", |_| next(false));
    wire(&mut ls, &f, "#f-prev", "click", |_| prev());
    wire(&mut ls, &f, "#f-shuffle", "click", |_| shuffle_up_next());
    wire(&mut ls, &f, "#f-repeat", "click", |_| {
        st_mut(|s| s.repeat = match s.repeat.as_str() { "off" => "all", "all" => "one", _ => "off" }.into());
        render_full();
    });
    wire(&mut ls, &f, "#f-auto", "change", |e| {
        let on = dom::target_el(e).map(|c| dom::checked(&c)).unwrap_or(false);
        let mut p = prefs();
        p["autoplay"] = json!(on);
        let server_prefs = app::has_feature("theme");
        dom::spawn(async move { playback::save(&p, server_prefs).await; });
    });
    if link.is_some() {
        wire(&mut ls, &f, "#fvol", "change", |e| { if let Some(v) = dom::target_el(e) { set_volume(num(&dom::value(&v))); } });
    } else {
        wire(&mut ls, &f, "#fvol", "input", |e| { if let Some(v) = dom::target_el(e) { local_volume(num(&dom::value(&v))); } });
    }
    { let t2 = t.clone(); wire(&mut ls, &f, "#f-like", "click", move |_| dom::spawn(toggle_like(t2.clone()))); }
    { let t2 = t.clone(); wire(&mut ls, &f, "#f-share", "click", move |_| dom::spawn(share_track(t2.clone()))); }
    wire(&mut ls, &f, "#f-more", "click", |e| { if let Some(b) = dom::closest(e, "#f-more") { more_menu(&b); } });
    if let Some(seek) = qs_in(&f, "#fseek") { bind_seek(&mut ls, &seek, "f-cur"); }
    let queue_for_link = s.a("queue");
    let is_link = link.is_some();
    wire(&mut ls, &f, ".np-list", "click", move |e| {
        if is_link {
            if let Some(it) = dom::closest(e, "[data-i]") {
                if !queue_for_link.is_empty() { via_link(json!({ "action": "play_queue", "tracks": queue_for_link, "index": num(&dom::data(&it, "i")) as i64 })); }
            }
            return;
        }
        if let Some(rm) = dom::closest(e, "[data-rm]") { remove_at(num(&dom::data(&rm, "rm")) as i64); return; }
        if let Some(up) = dom::closest(e, "[data-up]") { let i = num(&dom::data(&up, "up")) as i64; move_to(i, i - 1); return; }
        if let Some(it) = dom::closest(e, "[data-i]") { jump(num(&dom::data(&it, "i")) as i64); }
    });
    wire(&mut ls, &f, "#np-lyrics", "click", |e| { if let Some(l) = dom::closest(e, "[data-t]") { seek_to(num(&dom::data(&l, "t"))); } });
    set_full_listeners(ls);
    warm_likes(Some(&t));
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
    let Some(t) = now_track() else { return String::new() };
    if t.truthy("preview") { return r#"<div class="muted np-empty">Lyrics are available for songs in your library.</div>"#.into(); }
    if !t.g("id").is_number() { return r#"<div class="muted np-empty">No lyrics for this song.</div>"#.into(); }
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
    let t = now_track();
    let skip = match &t { None => true, Some(t) => t.truthy("preview") || !t.g("id").is_number() || LYR.with(|l| l.borrow().id == *t.g("id")) };
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
    let cur_id = now_track().map(|t| t.g("id").clone());
    let ok = LYR.with(|l| { let l = l.borrow(); !l.lines.is_empty() && cur_id.as_ref() == Some(&l.id) });
    if !ok { return; }
    let p = now_position() + 0.3;
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
