//! Port of web/js/player.js – see docs/ui-rewrite/CONTRACT.md
//! Web player: streams the original file (bit-perfect for MP3/FLAC/AAC/Opus); formats a browser
//! can't decode (ALAC/APE/WavPack/AIFF/WMA) are converted to FLAC on the fly by the server. On Auto quality it
//! measures the connection and steps the stream down (256 → 32 kbps AAC/Opus) and back up as the bandwidth allows.
//! Optional sound effects (EQ, crossfeed, balance, mono) run in this browser through Web Audio, and crossfade
//! blends one song into the next with the two <audio> elements.
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
    /// kbps of the lossy stream playing (0 = not lossy: the original, FLAC or Cadenza)
    rung: i64,
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
    /// when Auto last stepped down because of a stall, and when the rung last changed mid-song
    step_at: f64,
    switched_at: f64,
    last_preload_check: f64,
    slow_start: Option<Timeout>,
}
impl Default for State {
    fn default() -> Self {
        Self { queue: vec![], index: -1, transcoded: false, lossy: false, rung: 0, dsp: false, dsp_rate: 0.0, offset: 0.0, dragging: false, context: "list".into(), repeat: "off".into(), full: false, pane: "now".into(), logged: false, announced: None,
            stalls: vec![], stall_at: 0.0, seek_at: 0.0, step_at: 0.0, switched_at: 0.0, last_preload_check: 0.0, slow_start: None }
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
    /// a third <audio>, never wrapped in Web Audio: once the effects graph exists, 30-second previews (third-party
    /// URLs, often without CORS) play on it – through the graph they would fail or be silent
    static PLAIN: RefCell<Option<HtmlMediaElement>> = const { RefCell::new(None) };
    /// the wrapped element set aside while PLAIN plays
    static PARKED: RefCell<Option<HtmlMediaElement>> = const { RefCell::new(None) };
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
    /// the volume slider: the ceiling of every fade
    static USER_VOL: Cell<f64> = const { Cell::new(0.9) };
    /// when the "Connection is slow" toast last showed (at most once a minute)
    static SLOW_TOAST_AT: Cell<f64> = const { Cell::new(0.0) };
    /// liked track ids, fetched the first time a heart is drawn
    static LIKED: RefCell<Option<HashSet<i64>>> = const { RefCell::new(None) };
    /// AudioContext: reads the device's output sample rate, and runs the sound effects when they are on
    static CTX: RefCell<Option<web_sys::AudioContext>> = const { RefCell::new(None) };
    /// track id → /info (codec, sample rate, bit depth …)
    static INFO: RefCell<HashMap<i64, Value>> = RefCell::new(HashMap::new());
    /// what this browser says it can decode, by file format
    static PLAYABLE: RefCell<HashMap<String, bool>> = RefCell::new(HashMap::new());
    /// the lighter streams' codec: AAC where the browser decodes it, Opus elsewhere (Chromium builds, Firefox on Linux)
    static LOSSY: String = detect_lossy();
    /// GET /api/playback/dsp: is server-side Cadenza processing on, and may this account use it
    static DSP_INFO: RefCell<Option<Value>> = const { RefCell::new(None) };
    /// a processed stream failed this session: play originals until the page reloads
    static DSP_FAILED: Cell<bool> = const { Cell::new(false) };
    static DSP_TOASTED: Cell<bool> = const { Cell::new(false) };
    /// track id → what the server's pre-render cache says: 'cached' | 'rendering' | 'queued' | 'busy'
    static DSP_CACHE: RefCell<HashMap<i64, String>> = RefCell::new(HashMap::new());
    /// track id → when we last asked (prepare)
    static DSP_ASKED: RefCell<HashMap<i64, f64>> = RefCell::new(HashMap::new());
    /// the bandwidth estimate (Auto quality)
    static EST: RefCell<Est> = RefCell::new(Est::new());
    /// the crossfade in progress
    static FADE: RefCell<Option<Fade>> = const { RefCell::new(None) };
    /// the sound-effects graph (Web Audio)
    static FX: RefCell<Fx> = RefCell::new(Fx::default());
    /// a restored spot sits paused and hasn't played yet: its position may not be applied yet, so it isn't saved over
    static HELD: Cell<bool> = const { Cell::new(false) };
    /// where the held song was restored to (the original file only seeks there once its metadata is in)
    static HELD_AT: Cell<f64> = const { Cell::new(0.0) };
    /// music actually played since the last save: only then is saved_at moved on
    static PLAYED_SINCE: Cell<bool> = const { Cell::new(false) };
    /// when saveSpot last wrote (every 5 s while playing)
    static SPOT_SAVED_AT: Cell<f64> = const { Cell::new(0.0) };
}

fn fetch_dsp() {
    dom::spawn(async {
        let i = api::get("/playback/dsp").await.ok();
        DSP_INFO.with(|d| *d.borrow_mut() = i);
    });
}
fn dsp_info() -> Value { DSP_INFO.with(|d| d.borrow().clone()).unwrap_or(Value::Null) }
fn dsp_key(p: &Value) -> Value { json!([p.g("dsp"), p.g("dsp_filter"), p.g("dsp_dither"), p.g("dsp_rate")]) }
/// New settings: a different Cadenza setup forgets what the render cache said; the effects follow at once.
fn new_prefs(p: Value) {
    if dsp_key(&p) != dsp_key(&prefs()) {
        DSP_CACHE.with(|m| m.borrow_mut().clear());
        DSP_ASKED.with(|m| m.borrow_mut().clear());
    }
    PREFS.with(|x| *x.borrow_mut() = p);
    if playback::fx_active(&prefs()) { ensure_fx(); }
    apply_fx();
}
fn dsp_state(id: i64) -> String { DSP_CACHE.with(|m| m.borrow().get(&id).cloned()).unwrap_or_default() }
fn dsp_asked(id: i64) -> f64 { DSP_ASKED.with(|m| m.borrow().get(&id).copied()).unwrap_or(0.0) }
fn user_vol() -> f64 { USER_VOL.with(|v| v.get()) }
fn fading() -> bool { FADE.with(|f| f.borrow().is_some()) }
/// `a === b` for two elements.
fn same_el(a: &HtmlMediaElement, b: &HtmlMediaElement) -> bool { let x: &JsValue = a.as_ref(); let y: &JsValue = b.as_ref(); x == y }

fn st<R>(f: impl FnOnce(&State) -> R) -> R { STATE.with(|s| f(&s.borrow())) }
fn st_mut<R>(f: impl FnOnce(&mut State) -> R) -> R { STATE.with(|s| f(&mut s.borrow_mut())) }
fn audio() -> Option<HtmlMediaElement> { AUDIO.with(|a| a.borrow().clone()) }
fn spare() -> Option<HtmlMediaElement> { SPARE.with(|a| a.borrow().clone()) }
fn plain() -> Option<HtmlMediaElement> { PLAIN.with(|a| a.borrow().clone()) }
fn is_plain(el: &HtmlMediaElement) -> bool { plain().is_some_and(|p| same_el(&p, el)) }
fn fx_built() -> bool { FX.with(|f| f.borrow().built) }
/// `state.queue[state.index] || null` (the queue entry itself, for identity checks).
fn cur() -> Option<Rc<Value>> { st(|s| if s.index < 0 { None } else { s.queue.get(s.index as usize).cloned() }) }
fn is_linked() -> bool { LINK.with(|l| l.borrow().is_some()) }
fn is_local() -> bool { LOCAL.with(|l| l.get()) }
fn prefs() -> Value { PREFS.with(|p| p.borrow().clone()) }
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
pub(crate) fn to_fixed1(x: f64) -> String {
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
    let cov = cover_at(t, 600);
    if !cov.is_empty() {
        let img = js_sys::Object::new();
        js_set(&img, "src", &cov.into());
        js_set(&img, "sizes", &"600x600".into());
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
            let cov = cover_at(&t, 600);
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

// ---- linked: this device is locked as a remote for another one ----
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

// ---- remote control: what this device is playing, and commands from your other devices ----
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
    m.insert("position".into(), json!(spot_position()));
    m.insert("duration".into(), json!(duration()));
    m.insert("volume".into(), json!(user_vol()));
    let rate = CTX.with(|c| c.borrow().as_ref().map(|c| c.sample_rate() as f64));
    m.insert("signal".into(), json!({ "label": quality(&t), "transport": transport().map(|(k, _)| k), "output_rate": rate }));
    let (q, index) = lib_window();
    m.insert("index".into(), json!(index));
    m.insert("queue".into(), Value::Array(q));
    Value::Object(m)
}
fn local_volume(v: f64) {
    let (Some(a), Some(sp)) = (audio(), spare()) else { return };
    let vol = if v.is_finite() { 0f64.max(1f64.min(v)) } else { 0.9 };
    USER_VOL.with(|u| u.set(vol));
    if fading() { step_fade(); } else { set_fade_gain(&a, 1.0); set_fade_gain(&sp, 1.0); }
    dom::ls_set("encore.volume", &js_num(vol));
    for id in ["vol", "fvol"] {
        if let Some(el) = by_id(id) { if num(&dom::value(&el)) != vol { dom::set_value(&el, &js_num(vol)); } }
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
    else if t.truthy("album_id") { Value::String(format!("/api/library/albums/{}/cover{}", t.s("album_id"), crate::ui::cover_q(t))) }
    else { t.g("cover").clone() }
}
/// `cover(t)` as used in strings ("" when falsy).
fn cover(t: &Value) -> String { let v = cover_v(t); if json::truthy(&v) { js_string(&v) } else { String::new() } }
/// `coverAt(t, s)`: a library cover at a thumbnail size (other covers as they are).
fn cover_at(t: &Value, size: u32) -> String {
    let c = cover(t);
    if t.truthy("preview") || !t.truthy("album_id") || !c.starts_with("/api/library/albums/") { return c; }
    crate::ui::cover_url(&t.s("album_id"), &crate::ui::cover_q(t).trim_start_matches("?v=").to_string(), size)
}
/// `preloadNextCover()`: the next song's cover, so it shows at once when the song changes.
fn preload_next_cover() {
    let (q, i, full) = st(|s| (s.queue.get((s.index + 1).max(0) as usize).map(|t| (**t).clone()), s.index, s.full));
    let Some(n) = q.filter(|_| i >= 0) else { return };
    for size in if full { vec![200, 1000] } else { vec![200] } {
        let c = cover_at(&n, size);
        if c.is_empty() { continue; }
        if let Ok(img) = web_sys::HtmlImageElement::new() { img.set_decoding("async"); img.set_src(&c); }
    }
}

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
/// Upsample this library song on the server? Only when switched on and allowed; Data saver and Auto's lighter streams win.
fn want_dsp(t: &Value, lossy: bool) -> bool {
    prefs().truthy("dsp") && dsp_info().truthy("allowed") && !t.truthy("preview") && t.g("id").is_number() && !lossy && !DSP_FAILED.with(|f| f.get())
}
/// The server keeps finished Cadenza renders (pre-render cache): seeking and the next song need no new job.
fn dsp_cache_on() -> bool { dsp_info().g("cache").f("max_mb") > 0.0 }
/// transcode: FLAC made by the server · rung: kbps of the lossy stream (0 = not lossy) · dsp: upsampled by the server's Cadenza · offset: where a server-made stream starts
fn stream_url(t: &Value, transcode: bool, rung: i64, dsp: bool, offset: f64) -> String {
    if t.truthy("preview") { return t.s("preview_url"); }
    let mut q: Vec<String> = vec![];
    if rung > 0 { q.push(format!("q={}", lossy_codec())); q.push(format!("kbps={rung}")); }
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
/// kbit/s of a Cadenza stream at `rate` (24-bit stereo FLAC, roughly 65 % of the raw PCM).
fn dsp_kbps(rate: f64) -> f64 { playback::js_round(rate * 48.0 * 0.65 / 1000.0) }
/// kbit/s the song needs as the "original" rung: the Cadenza stream's when the server upsamples it, else the file's.
fn orig_kbps(t: Option<&Value>) -> f64 {
    let Some(t) = t else { return 0.0 };
    if t.truthy("preview") { return 0.0; }
    if want_dsp(t, false) { return dsp_kbps(dsp_out_rate(t)); }
    file_kbps(Some(t))
}
/// kbit/s of the file itself (0 = unknown, counted as 1411).
fn file_kbps(t: Option<&Value>) -> f64 {
    let Some(t) = t else { return 0.0 };
    if t.truthy("preview") { return 0.0; }
    if t.f("bitrate") > 0.0 { return t.f("bitrate"); }
    let i = INFO.with(|m| m.borrow().get(&t.i("id")).map(|i| i.f("bitrate"))).unwrap_or(0.0);
    if t.g("id").is_number() && i > 0.0 { return i; }
    if t.f("size") > 0.0 && t.f("duration") > 0.0 { playback::js_round(t.f("size") * 8.0 / 1000.0 / t.f("duration")) } else { 0.0 }
}
fn est_kbps() -> Option<f64> { EST.with(|e| e.borrow().kbps) }
/// Which rung a song starts on: Original never converts, Data saver is fixed at 96 kbps, Auto follows the estimated bandwidth.
fn rung_for(t: &Value) -> i64 {
    let q = prefs().s("quality");
    if t.truthy("preview") || q == "original" { return 0; }
    if q == "saver" { return playback::SAVER_KBPS; }
    playback::choose_rung(est_kbps(), orig_kbps(Some(t)))
}
/// The URL a song starts with (what the spare element pre-loads, and what load() asks for at a song start).
fn start_url(t: &Value) -> String {
    let rung = rung_for(t);
    stream_url(t, convert(t) && rung == 0, rung, want_dsp(t, rung > 0), 0.0)
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
        let art_url = if t.truthy("album_id") { abs_url(&cover_at(t, 600)) } else { String::new() };
        app::native_call_with("saveOffline", Some(&json!({
            "id": id, "title": t.s("title"), "artist": t.s("artist"), "album": t.s("album"), "ext": ext,
            "url": url,
            "art": art_url,
        }).to_string()));
    }
}

fn load(transcode: bool, offset: f64, rung: Option<i64>, dsp: Option<bool>) {
    load_opts(LoadOpts { transcode, offset, rung, dsp, ..Default::default() });
}
/// rung: kbps of the lossy stream (None = what the song should start on) · dsp: None = whatever the settings say
/// resume: the same song continues (a quality switch or a seek) – no new play / now-playing · fade: a song transition into it
/// hold: load it paused (a restored spot) – it starts when you press play
/// from: where the new song starts in a transition (Smart overlap skips its silent lead-in)
#[derive(Default)]
struct LoadOpts { transcode: bool, offset: f64, rung: Option<i64>, dsp: Option<bool>, resume: bool, fade: bool, hold: bool, from: f64 }
/// Start the current song (or restart it at `offset`).
fn load_opts(o: LoadOpts) {
    let Some(t) = cur() else { return };
    HELD.with(|h| h.set(o.hold));
    HELD_AT.with(|h| h.set(if o.hold { o.offset } else { 0.0 }));
    if o.hold { PLAYED_SINCE.with(|p| p.set(false)); } else if !o.resume { PLAYED_SINCE.with(|p| p.set(true)); }
    if !o.fade { end_fade(); }
    let offset = o.offset;
    let start = !truthy_num(offset) && !o.resume;
    mark_played(&t);
    let transcode = o.transcode || convert(&t);
    let prev_rung = st(|s| s.rung);
    let rung = if t.truthy("preview") { 0 } else { o.rung.unwrap_or_else(|| rung_for(&t)) };
    st_mut(|s| s.rung = rung);
    if start && !t.truthy("preview") && prefs().s("quality") == "auto" { note_rung_change(prev_rung, rung); }
    let is_lossy = rung > 0;
    let is_dsp = o.dsp.unwrap_or(true) && want_dsp(&t, is_lossy);
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
    let transcoded = !t.truthy("preview") && (transcode || is_lossy || is_dsp);
    // server-made streams restart at the seek point (their clock starts at `offset`); the original file and a
    // pre-rendered Cadenza file are whole files the browser seeks in itself
    let server_offset = if transcoded && !(is_dsp && dsp_state(t.i("id")) == "cached") { offset } else { 0.0 };
    st_mut(|s| {
        s.transcoded = transcoded;
        s.offset = server_offset;
        s.dragging = false;
    });
    if start {
        st_mut(|s| { s.logged = false; s.stalls.clear(); s.step_at = 0.0; });
        if !o.hold { announce(&t); }
    }
    let url = stream_url(&t, transcoded && !is_lossy && !is_dsp, rung, is_dsp, server_offset);
    let (Some(mut a), Some(sp)) = (audio(), spare()) else { return };
    let use_plain = t.truthy("preview") && fx_built();
    let swap = !truthy_num(offset) && !use_plain && dom::data_opt(&sp, "url").as_deref() == Some(url.as_str());
    // a transition needs the pre-buffered element; without it the old song stops and this one starts plainly
    if o.fade && !swap { end_fade(); }
    if swap {
        // already buffered in the background: swap players and start right away
        let old = a;
        a = sp;
        AUDIO.with(|x| *x.borrow_mut() = Some(a.clone()));
        // the preview element never pre-loads: the parked wrapped one takes the spare's place
        let next_spare = if is_plain(&old) { PARKED.with(|x| x.borrow_mut().take()).unwrap_or_else(|| old.clone()) } else { old.clone() };
        SPARE.with(|x| *x.borrow_mut() = Some(next_spare));
        if !o.fade { let _ = old.pause(); let _ = old.remove_attribute("src"); }
        let _ = old.remove_attribute("data-url");
        a.set_current_time(if o.fade && o.from > 0.0 && seekable_to(&a, o.from) { o.from } else { 0.0 });
        let g = if o.fade { FADE.with(|f| f.borrow().as_ref().map(|f| playback::transition_gains(&f.mode, 0.0, 0.0).1)).unwrap_or(1.0) } else { 1.0 };
        set_fade_gain(&a, g);
    } else {
        if use_plain != is_plain(&a) {
            // a preview with the effects graph built plays on the never-wrapped element; anything else on a wrapped one
            let old = a.clone();
            let _ = old.pause(); let _ = old.remove_attribute("src"); let _ = old.remove_attribute("data-url");
            let next = if use_plain { PARKED.with(|x| *x.borrow_mut() = Some(old.clone())); plain() } else { PARKED.with(|x| x.borrow_mut().take()) };
            if let Some(n) = next {
                a = n;
                AUDIO.with(|x| *x.borrow_mut() = Some(a.clone()));
                set_fade_gain(&a, 1.0);
            }
        }
        // without the pre-render cache a pre-buffered processed stream holds one of the server's Cadenza slots: free it first
        if is_dsp && !dsp_cache_on() && dom::data_opt(&sp, "url").is_some_and(|u| u.contains("dsp=1")) { cancel_preload(); }
        a.set_src(&url);
    }
    dom::set_data(&a, "url", &url);
    if truthy_num(offset) && !truthy_num(server_offset) {
        let el = a.clone();
        if el.ready_state() >= 1 { el.set_current_time(offset); }
        else { EventListener::once(&a, "loadedmetadata", move |_| el.set_current_time(offset)).forget(); }
    }
    if playback::fx_active(&prefs()) { ensure_fx(); }
    route();
    resume_fx();
    if !o.hold { play(&a); }
    st_mut(|s| s.slow_start = None);
    if start && !o.hold && !t.truthy("preview") && prefs().s("quality") == "auto" {
        // can't even get going: six seconds in and nothing has played yet → one rung down
        let t2 = t.clone();
        let timer = Timeout::new(6000, move || {
            let same = cur().is_some_and(|c| Rc::ptr_eq(&c, &t2));
            let stuck = audio().is_some_and(|a| !a.paused() && a.current_time() < 0.5 && a.ready_state() < 3);
            if same && stuck { step_down_now(); }
        });
        st_mut(|s| s.slow_start = Some(timer));
    }
    if start && is_dsp && dsp_cache_on() { prepare_ahead(&t); }
    if has_media_session() { set_media_metadata(&t); }
    preload_next_cover();
    dom::add_class(&body(), "has-player");
    render();
    emit();
    if !o.hold { save_spot(true); }
    if st(|s| s.full) { load_lyrics(); }
    top_up();
}
/// "now playing" on Last.fm / ListenBrainz – once per song start.
fn announce(t: &Rc<Value>) {
    if t.g("id").is_number() && !st(|s| s.announced.as_ref().is_some_and(|a| Rc::ptr_eq(a, t))) {
        st_mut(|s| s.announced = Some(t.clone()));
        let id = t.g("id").clone();
        dom::spawn(async move { let _ = api::post("/library/now-playing", json!({ "track_id": id })).await; });
    }
}

// ---- remember where you left off: this device's queue and spot, put back paused when the app opens --------
const RESUME: &str = "encore.resume";
fn is_lib(x: &Value) -> bool { json::truthy(x) && !x.truthy("preview") && x.g("id").is_number() }
/// Save the queue and the spot (every 5 s while playing; at once on pause, a new song and when the page goes away).
fn save_spot(force: bool) {
    if is_linked() || HELD.with(|h| h.get()) || audio().is_none() { return; }
    let Some(t) = cur() else { return };
    if !is_lib(&t) { return; }
    let now = now_ms();
    if !force && now - SPOT_SAVED_AT.with(|c| c.get()) < 5000.0 { return; }
    SPOT_SAVED_AT.with(|c| c.set(now));
    let (queue, index) = lib_window();
    let (context, repeat) = st(|s| (s.context.clone(), s.repeat.clone()));
    // saved_at is when music last played here: a pause or a page closed while paused keeps the old one, so a
    // song played on another device since still wins when the app opens again
    let saved_at = if PLAYED_SINCE.with(|p| p.get()) { now / 1000.0 } else {
        saved_spot().map(|s| s.f("saved_at")).filter(|x| x.is_finite() && *x != 0.0).unwrap_or(now / 1000.0)
    };
    if audio().is_some_and(|a| a.paused()) { PLAYED_SINCE.with(|p| p.set(false)); }
    dom::ls_set(RESUME, &json!({ "queue": queue, "index": index, "position": position(), "shuffle": context == "library",
        "repeat": repeat, "context": context, "saved_at": saved_at }).to_string());
}
/// The library songs of the queue – at most 300, around the current song – and where the current song is in them.
fn lib_window() -> (Vec<Value>, i64) {
    let (mut queue, mut index) = st(|s| {
        let q: Vec<Value> = s.queue.iter().filter(|x| is_lib(x)).map(|x| slim(x)).collect();
        let i = s.queue.iter().take(s.index.max(0) as usize).filter(|x| is_lib(x)).count() as i64;
        (q, i)
    });
    if queue.len() > 300 {
        let from = (index - 50).min(queue.len() as i64 - 300).max(0) as usize;
        queue = queue[from..from + 300].to_vec();
        index -= from as i64;
    }
    (queue, index)
}
/// Where the song is – for a held (restored) song whose file hasn't loaded yet, where it was restored to.
fn spot_position() -> f64 {
    let at = HELD_AT.with(|h| h.get());
    if HELD.with(|h| h.get()) && at != 0.0 && audio().is_some_and(|a| a.ready_state() < 1) { at } else { position() }
}
/// What saveSpot saved on this device: { queue, index, position, shuffle, repeat, context, saved_at } – None when nothing is saved.
pub fn saved_spot() -> Option<Value> {
    let s = serde_json::from_str::<Value>(&dom::ls_get(RESUME).unwrap_or_default()).ok()?;
    if s.g("queue").as_array().is_some_and(|q| q.iter().any(is_lib)) { Some(s) } else { None }
}
/// Put a saved spot back: the queue, paused at the song and position you left off (one tap plays).
pub fn restore(s: Option<Value>) -> bool {
    let Some(s) = s else { return false };
    if is_linked() || cur().is_some() || audio().is_none() || !s.g("queue").is_array() { return false; }
    let q: Vec<Value> = s.a("queue").into_iter().filter(is_lib).collect();
    if q.is_empty() { return false; }
    let ctx = s.s("context");
    let context = if ["album", "single", "list", "library"].contains(&ctx.as_str()) { ctx } else if s.truthy("shuffle") { "library".to_string() } else { "list".to_string() };
    let repeat = s.s("repeat");
    let index = s.f("index");
    st_mut(|x| {
        let n = q.len() as i64;
        x.queue = q.into_iter().map(Rc::new).collect();
        x.index = (if index.is_finite() { index as i64 } else { 0 }).min(n - 1).max(0);
        x.context = context;
        if ["off", "all", "one"].contains(&repeat.as_str()) { x.repeat = repeat; }
    });
    let dur = cur().map(|t| t.f("duration")).unwrap_or(0.0);
    let at = s.f("position");
    let offset = if at > 1.0 && !(dur > 0.0 && at > dur - 2.0) { at } else { 0.0 };
    load_opts(LoadOpts { offset, hold: true, ..Default::default() });
    true
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
fn maybe_preload(force: bool) { maybe_preload_need(force, false); }
/// need: the transition window is open – fetch the next song even with pre-load off.
fn maybe_preload_need(force: bool, need: bool) {
    let Some(sp) = spare() else { return };
    if (!prefs().truthy("preload") && !need) || is_linked() || fading() { return; }
    let n = st(|s| if s.index + 1 < 0 { None } else { s.queue.get((s.index + 1) as usize).cloned() });
    let Some(n) = n else { return };
    if n.truthy("preview") && fx_built() { return; }   // a preview plays on the never-wrapped element, not the spare
    let url = start_url(&n);
    if dom::data_opt(&sp, "url").as_deref() == Some(url.as_str()) { return; }
    let t = cur();
    let remaining = duration() - position();
    let quiet = now_ms() - st(|s| s.stall_at) > 15000.0;
    if url.contains("dsp=1") && dsp_cache_on() && st(|s| s.dsp) {
        // a processed next song: ask the server to render it ahead, and buffer it only once that render exists
        // (cached or in progress) – then the pre-load takes no extra processing slot
        let id = n.i("id");
        let s = dsp_state(id);
        if s != "cached" && s != "rendering" {
            if now_ms() - dsp_asked(id) > 15000.0 {
                let n2 = n.clone();
                dom::spawn(async move { prepare_dsp(&n2).await; maybe_preload(force); });
            }
            return;
        }
        if !force && !(fully_buffered() || (quiet && buffered_ahead() > 45.0) || remaining < 40.0) { return; }
    } else if !force {
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
    if dom::data_opt(&sp, "url").is_none() || fading() { return; }
    let _ = sp.remove_attribute("src");
    let _ = sp.remove_attribute("data-url");
    sp.load();
}
/// The browser ran out of buffer mid-song. Give it the bandwidth back, and on Auto quality step the stream down.
fn stalled() {
    let now = now_ms();
    let paused = audio().map(|a| a.paused()).unwrap_or(true);
    if now - st(|s| s.seek_at) < 3000.0 || position() < 1.0 || paused { return; }   // a seek, a switch or the start, not a slow link
    st_mut(|s| { s.stall_at = now; s.stalls.push(now); });
    if !fully_buffered() { cancel_preload(); }
    if prefs().s("quality") != "auto" || cur().is_some_and(|t| t.truthy("preview")) { return; }
    // stalling again within 90 s of stepping down: the new rung isn't small enough either
    let step_at = st(|s| s.step_at);
    if truthy_num(step_at) && now - step_at < 90000.0 { step_down_now(); return; }
    let recent = st(|s| s.stalls.iter().filter(|x| now - **x < 90000.0).count());
    if recent >= 2 { step_down_now(); return; }
    // one long stall counts too: still waiting four seconds later
    Timeout::new(4000, move || {
        let waiting = audio().is_some_and(|a| !a.paused() && a.ready_state() < 3);
        if waiting && st(|s| s.stall_at == now) { step_down_now(); }
    }).forget();
}
/// Auto: a stall forces one rung down right away, and the estimate drops to 60 % of what the rung needed.
fn step_down_now() {
    let Some(t) = cur() else { return };
    if t.truthy("preview") || prefs().s("quality") != "auto" { return; }
    st_mut(|s| s.slow_start = None);
    let orig = orig_kbps(Some(&t));
    let rung = st(|s| s.rung);
    let lower = playback::step_down(rung, orig);
    if lower == rung { return; }   // already at 32 kbps
    EST.with(|e| {
        let mut e = e.borrow_mut();
        e.kbps = Some(0.6 * playback::needed(rung, orig));
        e.at = now_ms();
        e.fresh = false;
        e.up_since = 0.0;
    });
    st_mut(|s| { s.stalls.clear(); s.step_at = now_ms(); });
    switch_rung(lower);
}
/// Reload the song at the current position on another rung.
fn switch_rung(rung: i64) {
    if cur().is_none() { return; }
    let at = position();
    note_rung_change(st(|s| s.rung), rung);
    let now = now_ms();
    st_mut(|s| { s.switched_at = now; s.seek_at = now; });
    load_opts(LoadOpts { rung: Some(rung), offset: at, resume: true, transcode: rung > 0, ..Default::default() });
}
fn note_rung_change(was: i64, now: i64) {
    if was == now { return; }
    if now > 0 && (was == 0 || now < was) {
        if now_ms() - SLOW_TOAST_AT.with(|c| c.get()) > 60000.0 {
            SLOW_TOAST_AT.with(|c| c.set(now_ms()));
            toast(&format!("Connection is slow – lowering quality to {now} kbps to keep playing"), "");
        }
    } else if now == 0 { toast("Connection recovered – back to the original file", "ok"); }
}

// ---- the bandwidth estimate (Auto quality) -----------------------------------------------------
// An exponentially weighted average of measured throughput in kbit/s: from how fast the playing song's buffer
// grows, from a small probe when nothing is downloading, and – until the first sample – the browser's own guess.
struct Est { kbps: Option<f64>, at: f64, fresh: bool, up_since: f64, el: Option<HtmlMediaElement>, url: String, end: f64, wall: f64, probing: bool, probe_at: f64 }
impl Est {
    fn new() -> Self {
        let mut e = Est { kbps: None, at: 0.0, fresh: false, up_since: 0.0, el: None, url: String::new(), end: 0.0, wall: 0.0, probing: false, probe_at: 0.0 };
        // `navigator.connection?.downlink` (Mbit/s) – untyped, the Network Information API isn't in every browser
        let d = js_sys::Reflect::get(&dom::window().navigator(), &"connection".into()).ok()
            .filter(|c| c.is_object())
            .and_then(|c| js_sys::Reflect::get(&c, &"downlink".into()).ok())
            .and_then(|v| v.as_f64());
        if let Some(d) = d { if d > 0.0 { e.kbps = Some(d * 1000.0); e.at = now_ms(); } }
        e
    }
}
fn perf_now() -> f64 { dom::window().performance().map(|p| p.now()).unwrap_or_else(now_ms) }
fn add_sample(kbps: f64) {
    if !(kbps >= 0.0) || !kbps.is_finite() { return; }
    EST.with(|e| {
        let mut e = e.borrow_mut();
        e.kbps = Some(playback::ewma(e.kbps, kbps));
        e.at = now_ms();
        e.fresh = true;
    });
}
/// kbit/s of the stream the element is playing (None when the server, not the network, sets the pace).
fn stream_kbps() -> Option<f64> {
    let (rung, dsp) = st(|s| (s.rung, s.dsp));
    if rung > 0 { return Some(rung as f64); }
    if dsp { return None; }   // Cadenza renders at its own speed: its download rate says nothing about the connection
    let o = file_kbps(cur().as_deref());
    Some(if truthy_num(o) { o } else { 1411.0 })
}
/// Every 2 s: how far the buffer grew × the stream's bitrate ÷ the time it took. Returns whether a download is in progress.
fn sample_progress() -> bool {
    let Some(a) = audio() else { return false };
    let now = perf_now();
    let b = a.buffered();
    let end = if b.length() > 0 { b.end(b.length() - 1).unwrap_or(0.0) } else { 0.0 };
    let d = a.duration();
    let downloading = a.network_state() == 2 && !(d.is_finite() && d > 0.0 && end >= d - 0.5);
    let url = dom::data(&a, "url");
    let kbps = stream_kbps();
    let same = EST.with(|e| { let e = e.borrow(); e.el.as_ref().is_some_and(|x| same_el(x, &a)) && e.url == url });
    // only while the element is still downloading: a finished download would read as a slow link
    let kbps = kbps.filter(|k| truthy_num(*k));
    let Some(kbps) = kbps.filter(|_| same && downloading) else {
        EST.with(|e| { let mut e = e.borrow_mut(); e.el = Some(a.clone()); e.url = url; e.end = end; e.wall = now; });
        return downloading;
    };
    let (secs, grown) = EST.with(|e| {
        let mut e = e.borrow_mut();
        let r = ((now - e.wall) / 1000.0, end - e.end);
        e.end = end; e.wall = now;
        r
    });
    if secs < 0.25 || grown < 0.0 { return true; }
    if grown == 0.0 && buffered_ahead() > 30.0 { return true; }   // the browser holding back with a full buffer, not a slow link
    add_sample(grown * kbps / secs);
    true
}
/// Probe sizes: 128 KB, and when that arrives in under 0.25 s (too fast to time), 1 MB, then 4 MB.
const PROBE_SIZES: [f64; 3] = [131072.0, 1048576.0, 4194304.0];
/// Nothing downloading and the estimate is older than 20 s: time a piece of the current or next song.
fn probe() {
    let now = now_ms();
    let busy = EST.with(|e| { let e = e.borrow(); e.probing || now - e.at < 20000.0 || now - e.probe_at < 20000.0 });
    if busy { return; }
    let cands = st(|s| {
        let c = if s.index < 0 { None } else { s.queue.get(s.index as usize).cloned() };
        let n = if s.index + 1 < 0 { None } else { s.queue.get((s.index + 1) as usize).cloned() };
        [c, n]
    });
    let Some(t) = cands.into_iter().flatten().find(|x| !x.truthy("preview") && x.g("id").is_number() && !convert(x)) else { return };
    EST.with(|e| { let mut e = e.borrow_mut(); e.probing = true; e.probe_at = now; });
    let id = t.s("id");
    dom::spawn(async move {
        for (i, &size) in PROBE_SIZES.iter().enumerate() {
            let Some((bytes, secs)) = probe_once(&id, size).await else { break };
            if let Some(kbps) = playback::rate_of(bytes, secs) { add_sample(kbps); break; }
            if bytes < size || i == PROBE_SIZES.len() - 1 {
                // still under 0.25 s (the whole song, or 4 MB): "at least this fast" – it may lift the estimate, never lower it
                let at_least = bytes * 8.0 / 1000.0 / 0.25;
                if est_kbps().is_none_or(|k| k < at_least) { add_sample(at_least); }
                break;
            }
        }
        EST.with(|e| e.borrow_mut().probing = false);
    });
}
async fn js_await(p: JsValue) -> Option<JsValue> {
    let p = p.dyn_into::<js_sys::Promise>().ok()?;
    wasm_bindgen_futures::JsFuture::from(p).await.ok()
}
fn js_get(o: &JsValue, key: &str) -> JsValue { js_sys::Reflect::get(o, &key.into()).unwrap_or(JsValue::UNDEFINED) }
fn js_call0(o: &JsValue, name: &str) -> Option<JsValue> {
    js_get(o, name).dyn_into::<js_sys::Function>().ok()?.call0(o).ok()
}
/// `fetch(stream, { headers: { Range: 'bytes=0-<size - 1>' }, credentials: 'same-origin', cache: 'no-store' })`, timed:
/// (bytes, seconds); None when offline or refused.
async fn probe_once(id: &str, size: f64) -> Option<(f64, f64)> {
    let init = web_sys::RequestInit::new();
    init.set_credentials(web_sys::RequestCredentials::SameOrigin);
    init.set_cache(web_sys::RequestCache::NoStore);
    let headers = web_sys::Headers::new().ok()?;
    headers.set("Range", &format!("bytes=0-{}", js_num(size - 1.0))).ok()?;
    init.set_headers(&headers);
    let t0 = perf_now();
    let r = js_await(dom::window().fetch_with_str_and_init(&format!("/api/library/tracks/{id}/stream"), &init).into()).await?;
    let r: web_sys::Response = r.dyn_into().ok()?;
    let body = js_get(r.as_ref(), "body");
    if r.status() != 206 && r.status() != 200 {
        if body.is_object() { if let Some(p) = js_call0(&body, "cancel") { let _ = js_await(p).await; } }
        return None;
    }
    if !body.is_object() { return None; }
    let reader = js_call0(&body, "getReader")?;
    let mut bytes = 0f64;
    while bytes < size {
        let Some(chunk) = js_call0(&reader, "read") else { break };
        let Some(res) = js_await(chunk).await else { break };
        if js_get(&res, "done").is_truthy() { break; }
        bytes += js_get(&js_get(&res, "value"), "byteLength").as_f64().unwrap_or(0.0);
    }
    if let Some(p) = js_call0(&reader, "cancel") { dom::spawn(async move { let _ = js_await(p).await; }); }
    Some((bytes.min(size), (perf_now() - t0) / 1000.0))
}
fn adapt_tick() {
    if is_linked() || audio().is_none() { return; }
    let Some(t) = cur() else { return };
    if t.truthy("preview") { return; }
    let downloading = sample_progress();
    if audio().is_some_and(|a| a.paused()) { EST.with(|e| e.borrow_mut().up_since = 0.0); return; }
    if !downloading { probe(); }
    if prefs().s("quality") == "auto" { decide(); }
}
/// Auto: follow the estimate – down at once (when the buffer is short), up when it has held 1.5× the higher rung's need for 15 s.
fn decide() {
    let t = cur();
    let orig = orig_kbps(t.as_deref());
    let l = playback::ladder(orig);
    let rung = st(|s| s.rung);
    let ci = l.iter().position(|&r| r == rung);
    let (fresh, est) = EST.with(|e| { let mut e = e.borrow_mut(); let f = e.fresh; e.fresh = false; (f, e.kbps) });
    let (Some(est), Some(ci)) = (est, ci) else { return };
    let now = now_ms();
    let target = playback::choose_rung(Some(est), orig);
    let ti = l.iter().position(|&r| r == target).unwrap_or(0);
    if ti > ci {
        EST.with(|e| e.borrow_mut().up_since = 0.0);
        if fresh && now - st(|s| s.switched_at) > 10000.0 && !fully_buffered() && buffered_ahead() < 20.0 { switch_rung(l[ti]); }
        return;
    }
    let up = l.iter().copied().find(|&r| playback::needed(r, orig) * 1.5 <= est);
    let Some(up) = up.filter(|u| l.iter().position(|r| r == u).unwrap_or(0) < ci) else { EST.with(|e| e.borrow_mut().up_since = 0.0); return };
    let since = EST.with(|e| e.borrow().up_since);
    if !truthy_num(since) { EST.with(|e| e.borrow_mut().up_since = now); return; }
    // safe to reload: 10 s in hand, or a server-made stream the browser has finished fetching (it shows little of it as buffered)
    let safe = buffered_ahead() > 10.0 || (st(|s| s.transcoded) && audio().is_some_and(|a| a.network_state() != 2));
    if now - since >= 15000.0 && safe { EST.with(|e| e.borrow_mut().up_since = 0.0); switch_rung(up); }
}

// ---- server-side Cadenza: render ahead -----------------------------------------------------------
/// Ask the server to render a song ahead (or say how far it is): 'cached' | 'rendering' | 'queued' | 'busy'.
async fn prepare_dsp(t: &Value) -> Option<String> {
    if !t.g("id").is_number() { return None; }
    let id = t.i("id");
    DSP_ASKED.with(|m| m.borrow_mut().insert(id, now_ms()));
    let p = prefs();
    match api::post("/playback/dsp/prepare", json!({ "track_id": t.g("id"), "rate": playback::jnum(dsp_request_rate()), "filter": p.g("dsp_filter"), "dither": p.g("dsp_dither") })).await {
        Ok(r) => {
            let s = r.g("state").as_str().unwrap_or("").to_string();
            DSP_CACHE.with(|m| m.borrow_mut().insert(id, s.clone()));
            Some(s)
        }
        Err(_) => None,
    }
}
/// The current song started as a processed stream: learn whether it is pre-rendered, and have the next one rendered now.
fn prepare_ahead(t: &Value) {
    let t2 = t.clone();
    dom::spawn(async move { prepare_dsp(&t2).await; });
    let n = st(|s| if s.index + 1 < 0 { None } else { s.queue.get((s.index + 1) as usize).cloned() });
    if let Some(n) = n {
        if want_dsp(&n, rung_for(&n) > 0) {
            dom::spawn(async move { prepare_dsp(&n).await; maybe_preload(false); });
        }
    }
}
fn dsp_note(s: &str) -> &'static str { match s { "cached" => " (pre-rendered)", "rendering" | "queued" => " (rendering ahead)", _ => "" } }

// ---- song transitions ----------------------------------------------------------------------------
// Off | Crossfade (equal-power, both fade) | Overlap (the next song starts at full volume early, only the old one
// fades out) | Smart overlap (the silent ends of both songs are skipped: the next song starts at its first audible
// moment, overlapping the old one up to its last audible moment). The next song plays on the spare element; when
// the window opens and it isn't buffered (pre-load off, a slow server stream), it starts loading then and the
// transition begins as soon as it can play, over what is left of the window.
// The gains follow the new song's own clock and are set from its timeupdate events as well as a timer: in a
// hidden or minimised window (or a phone with the screen off) timers are throttled or stopped, and the fade must
// not stall there – that is what left the next song silent before ("it just makes the audio quiet").
struct Fade { out: HtmlMediaElement, mode: String, len: f64, from: f64, out_end: f64, timer: Option<Interval> }
thread_local! {
    /// track id → (start_s, end_s) once known (None: asked, unknown)
    static EDGES: RefCell<HashMap<i64, Option<(f64, f64)>>> = RefCell::new(HashMap::new());
}
/// Where a song's sound starts and stops (Smart overlap) – None until the server has answered.
fn track_edges(t: &Value) -> Option<(f64, f64)> {
    if !is_lib(t) { return None; }
    let id = t.i("id");
    if let Some(e) = EDGES.with(|m| m.borrow().get(&id).cloned()) { return e; }
    EDGES.with(|m| m.borrow_mut().insert(id, None));
    dom::spawn(async move {
        if let Ok(e) = api::get(&format!("/library/tracks/{id}/edges")).await {
            let (s, end) = (e.f("start_s"), e.f("end_s"));
            if end > 0.0 { EDGES.with(|m| m.borrow_mut().insert(id, Some((s.max(0.0), end)))); }
        }
    });
    None
}
/// The URL load_opts() asks for with these options (offset 0) – the same decisions load_opts() makes.
fn planned_url(t: &Value, transcode: bool, rung: i64, dsp: bool) -> String {
    let lossy = rung > 0;
    let d = dsp && want_dsp(t, lossy);
    let transcoded = convert(t) || transcode || lossy || d;
    stream_url(t, transcoded && !lossy && !d, rung, d, 0.0)
}
/// The load options (transcode, rung, dsp) that make load_opts() use exactly what the spare element holds for song
/// `n` (any rung) – None when it holds something else.
fn spare_for(n: &Value) -> Option<(bool, i64, bool)> {
    let sp = spare()?;
    let u = dom::data_opt(&sp, "url")?;
    if n.truthy("preview") || !n.g("id").is_number() || !u.starts_with(&format!("/api/library/tracks/{}/stream", n.i("id"))) { return None; }
    let q = web_sys::UrlSearchParams::new_with_str(u.split_once('?').map(|x| x.1).unwrap_or("")).ok()?;
    if q.has("t") { return None; }
    let rung = q.get("kbps").and_then(|k| k.parse::<i64>().ok()).unwrap_or(0);
    let (dsp, transcode) = (q.get("dsp").as_deref() == Some("1"), q.get("transcode").as_deref() == Some("1"));
    (planned_url(n, transcode, rung, dsp) == u).then_some((transcode, rung, dsp))
}
fn seekable_to(el: &HtmlMediaElement, at: f64) -> bool {
    let s = el.seekable();
    s.length() > 0 && s.end(s.length() - 1).unwrap_or(0.0) >= at
}
fn maybe_transition() {
    let p = prefs();
    let mode = p.s("transition");
    if mode.is_empty() || mode == "off" || fading() || is_linked() || HELD.with(|h| h.get()) || st(|s| s.repeat == "one" || s.dragging) || audio().is_none_or(|a| a.paused()) { return; }
    let (t, n) = st(|s| (
        if s.index < 0 { None } else { s.queue.get(s.index as usize).cloned() },
        if s.index + 1 < 0 { None } else { s.queue.get((s.index + 1) as usize).cloned() },
    ));
    let (Some(t), Some(n)) = (t, n) else { return };
    if t.truthy("preview") || (n.truthy("preview") && fx_built()) { return; }
    if p.truthy("crossfade_album_gapless") && !t.is_nullish("album_id") && t.g("album_id") == n.g("album_id") { return; }
    let cf = p.f("crossfade");
    let secs = if cf > 0.0 { cf } else { playback::TRANSITION_DEFAULT_S }.max(1.0);
    let d = duration();
    if !truthy_num(d) || d < 2.0 * secs { return; }
    let smart = if mode == "smart" { Some((track_edges(&t), track_edges(&n))) } else { None };
    // the old song's last audible moment (Smart overlap), else its end
    let end = match smart { Some((Some((_, e)), _)) if e > 2.0 * secs && e < d => e, _ => d };
    let remaining = end - position();
    if remaining > secs || remaining < 1.0 { return; }
    let Some(sp) = spare() else { return };
    let o = spare_for(&n);
    if o.is_none() || sp.ready_state() < 3 {
        // not buffered yet: fetch it now (even with pre-load off) and start as soon as it can play
        if o.is_none() { maybe_preload_need(true, true); }
        return;
    }
    let Some((transcode, rung, dsp)) = o else { return };
    let Some(out) = audio() else { return };
    log_play();
    let from = match smart { Some((_, Some((s, _)))) if s > 0.05 && s < 30.0 => s, _ => 0.0 };
    let out_end = out.current_time() + remaining;
    FADE.with(|f| *f.borrow_mut() = Some(Fade { out, mode, len: remaining, from, out_end, timer: None }));
    st_mut(|s| s.index += 1);
    load_opts(LoadOpts { fade: true, transcode, rung: Some(rung), dsp: Some(dsp), from, ..Default::default() });
    if !fading() { return; }   // load_opts() could not use the spare after all: a plain start
    let started = audio().map(|a| a.current_time()).unwrap_or(0.0);
    let timer = Interval::new(50, step_fade);
    FADE.with(|f| if let Some(f) = f.borrow_mut().as_mut() { f.from = started; f.timer = Some(timer); });
    step_fade();
}
fn step_fade() {
    let Some((out, mode, len, from, out_end)) = FADE.with(|f| f.borrow().as_ref().map(|f| (f.out.clone(), f.mode.clone(), f.len, f.from, f.out_end))) else { return };
    let Some(a) = audio() else { return };
    let x = ((a.current_time() - from).max(0.0) / len).min(1.0);
    let x = if x.is_nan() { 0.0 } else { x };
    let (g_out, g_in) = playback::transition_gains(&mode, x, x * len);
    set_fade_gain_smooth(&out, g_out, true);
    set_fade_gain_smooth(&a, g_in, true);
    if x >= 1.0 || out.ended() || out.current_time() >= out_end { end_fade(); }
}
/// Finish (or cut short – next/prev/seek) a transition: the old song stops, the new one plays at full volume.
fn end_fade() {
    let Some(f) = FADE.with(|f| f.borrow_mut().take()) else { return };
    // the interval may be the caller: let it go once this turn is over
    if let Some(timer) = f.timer { dom::spawn(async move { drop(timer); }); }
    let out = f.out;
    let _ = out.pause();
    let _ = out.remove_attribute("src");
    let _ = out.remove_attribute("data-url");
    out.load();
    set_fade_gain(&out, 1.0);
    if let Some(a) = audio() { set_fade_gain(&a, 1.0); }
    route();
    maybe_preload(false);
}
/// `p.cancelAndHoldAtTime(t)` where the browser has it (not in web-sys), else `cancelScheduledValues(t)`.
fn cancel_and_hold(p: &web_sys::AudioParam, t: f64) {
    let f = js_sys::Reflect::get(p, &JsValue::from_str("cancelAndHoldAtTime")).ok().and_then(|f| f.dyn_into::<js_sys::Function>().ok());
    match f {
        Some(f) => { let _ = f.call1(p, &JsValue::from_f64(t)); }
        None => { let _ = p.cancel_scheduled_values(t); }
    }
}
/// The fade level of one element: a Web Audio gain when it is routed through the effects graph, else its volume (under the slider).
fn set_fade_gain(el: &HtmlMediaElement, g: f64) { set_fade_gain_smooth(el, g, false); }
/// smooth: glide there on the audio clock (the steps come a few times a second, more slowly in a hidden window).
fn set_fade_gain_smooth(el: &HtmlMediaElement, g: f64, smooth: bool) {
    let gain = FX.with(|fx| fx.borrow().sources.iter().find(|s| same_el(&s.el, el)).and_then(|s| s.gain.clone()));
    match (gain, audio_ctx()) {
        (Some(n), Some(ctx)) => {
            let p = n.gain();
            let now = ctx.current_time();
            cancel_and_hold(&p, now);
            let set = if smooth { p.set_target_at_time(g as f32, now, 0.05) } else { p.set_value_at_time(g as f32, now) };
            if set.is_err() { p.set_value(g as f32); }
            el.set_volume(user_vol());
        }
        (Some(n), None) => { n.gain().set_value(g as f32); el.set_volume(user_vol()); }
        _ => el.set_volume((user_vol() * g).clamp(0.0, 1.0)),
    }
}

// ---- sound effects on this device (Web Audio) ----------------------------------------------------
// Built the first time an effect is switched on; until then the <audio> elements play straight out, untouched.
// An element wrapped in a MediaElementSource can only be heard through the graph, so once built the graph stays
// and turns into a pass-through when the effects are switched off.
/// `out` feeds the graph: the fade gain, or the source itself when no gain could be made
struct FxSource { el: HtmlMediaElement, out: web_sys::AudioNode, gain: Option<web_sys::GainNode>, on: bool }
#[derive(Default)]
struct Fx {
    built: bool,
    /// half built (a node could not be made): what is wrapped plays through a plain pass-through
    broken: bool,
    sources: Vec<FxSource>,
    input: Option<web_sys::GainNode>,
    key: String,
    eq: Vec<web_sys::BiquadFilterNode>,
    cross: Vec<web_sys::GainNode>,
    trim: Option<web_sys::GainNode>,
    pan: Option<web_sys::StereoPannerNode>,
    nodes: Vec<web_sys::AudioNode>,
}
fn db_gain(v: f64) -> f64 { 10f64.powf(v / 20.0) }
fn xfeed_db(x: &str) -> f64 { match x { "light" => -9.0, "medium" => -6.0, "strong" => -4.5, _ => -6.0 } }
fn biquad_type(t: &str) -> web_sys::BiquadFilterType {
    use web_sys::BiquadFilterType as B;
    match t { "lowshelf" => B::Lowshelf, "highshelf" => B::Highshelf, "lowpass" => B::Lowpass, "highpass" => B::Highpass, "notch" => B::Notch, _ => B::Peaking }
}
fn audio_ctx() -> Option<web_sys::AudioContext> { CTX.with(|c| c.borrow().clone()) }
fn ensure_fx() -> bool {
    if fx_built() { return true; }
    // a preview playing now stays out of the graph (it is built at the next library song)
    if cur().is_some_and(|t| t.truthy("preview")) { return false; }
    let (Some(a), Some(sp)) = (audio(), spare()) else { return false };
    output_rate();
    let Some(ctx) = audio_ctx() else { return false };
    let built = (|| -> Result<(), JsValue> {
        let input = ctx.create_gain()?;
        FX.with(|f| f.borrow_mut().input = Some(input));
        for el in [a, sp] {
            el.set_cross_origin(Some("anonymous"));
            let src = ctx.create_media_element_source(&el)?;
            // kept at once: an element can be wrapped only once, and from now on is heard only through the graph
            let i = FX.with(|f| { let mut f = f.borrow_mut(); f.sources.push(FxSource { el, out: src.clone().into(), gain: None, on: false }); f.sources.len() - 1 });
            let gain = ctx.create_gain()?;
            src.connect_with_audio_node(&gain)?;
            FX.with(|f| { let mut f = f.borrow_mut(); f.sources[i].out = gain.clone().into(); f.sources[i].gain = Some(gain); });
        }
        Ok(())
    })();
    let ok = built.is_ok();
    let wrapped = FX.with(|f| {
        let mut f = f.borrow_mut();
        if f.sources.is_empty() { f.input = None; return false; }   // nothing wrapped: the elements still play straight out
        f.built = true;
        f.broken = !ok;
        f.key = String::new();
        true
    });
    if !wrapped { return false; }
    if ok { apply_fx(); }
    else if let Some(i) = FX.with(|f| f.borrow().input.clone()) { let _ = i.connect_with_audio_node(&ctx.destination()); }   // half built: a pass-through
    route();
    resume_fx();
    true
}
/// Resume the graph's context on play: suspended by the autoplay policy, or 'interrupted' (Safari after a call or Siri).
fn resume_fx() {
    if !fx_built() { return; }
    if let Some(ctx) = audio_ctx() {
        if ctx.state() != web_sys::AudioContextState::Running {
            if let Ok(p) = ctx.resume() { dom::spawn(async move { let _ = wasm_bindgen_futures::JsFuture::from(p).await; }); }
        }
    }
}
/// Only the element playing (and the one fading out) feeds the graph.
fn route() {
    let a = audio();
    let out = FADE.with(|f| f.borrow().as_ref().map(|f| f.out.clone()));
    FX.with(|f| {
        let mut f = f.borrow_mut();
        if !f.built { return; }
        let input: web_sys::AudioNode = match (f.input.clone(), audio_ctx()) {
            (Some(i), _) => i.into(),
            (None, Some(c)) => c.destination().into(),
            _ => return,
        };
        for s in f.sources.iter_mut() {
            let want = a.as_ref().is_some_and(|a| same_el(&s.el, a)) || out.as_ref().is_some_and(|o| same_el(&s.el, o));
            if want && !s.on { let _ = s.out.connect_with_audio_node(&input); s.on = true; }
            else if !want && s.on { let _ = s.out.disconnect(); s.on = false; }
        }
    });
}
/// Bring the graph in line with the settings: rewire only when its shape changes, otherwise just set the values.
fn apply_fx() {
    if FX.with(|f| { let f = f.borrow(); !f.built || f.broken }) { return; }
    let Some(ctx) = audio_ctx() else { return };
    let p = prefs();
    let on = playback::fx_active(&p);
    let bands = if on { playback::eq_bands(&p) } else { vec![] };
    let mono = on && p.truthy("mono");
    let xfeed = if on && !mono { p.s("crossfeed") } else { "off".to_string() };
    let key = json!([on, bands.iter().map(|b| b.g("type").clone()).collect::<Vec<_>>(), xfeed != "off", mono]).to_string();
    FX.with(|f| {
        let mut fx = f.borrow_mut();
        let Some(input) = fx.input.clone() else { return };
        if key != fx.key {
            fx.key = key;
            let _ = input.disconnect();
            for n in fx.nodes.drain(..) { let _ = n.disconnect(); }
            fx.eq.clear(); fx.cross.clear(); fx.trim = None; fx.pan = None;
            let wired = (|fx: &mut Fx| -> Result<(), JsValue> {
                let mut last: web_sys::AudioNode = input.clone().into();
                let add = |fx: &mut Fx, last: &mut web_sys::AudioNode, n: web_sys::AudioNode| -> Result<(), JsValue> {
                    fx.nodes.push(n.clone());
                    last.connect_with_audio_node(&n)?;
                    *last = n;
                    Ok(())
                };
                if on {
                    for b in &bands {
                        let f = ctx.create_biquad_filter()?;
                        f.set_type(biquad_type(&b.s("type")));
                        add(fx, &mut last, f.clone().into())?;
                        fx.eq.push(f);
                    }
                    if xfeed != "off" {
                        // each channel gets a low-passed, slightly delayed, quieter copy of the other one
                        let pre = ctx.create_gain()?;
                        pre.set_channel_count(2);
                        pre.set_channel_count_mode(web_sys::ChannelCountMode::Explicit);
                        pre.set_channel_interpretation(web_sys::ChannelInterpretation::Speakers);
                        add(fx, &mut last, pre.clone().into())?;
                        let split = ctx.create_channel_splitter_with_number_of_outputs(2)?;
                        let merge = ctx.create_channel_merger_with_number_of_inputs(2)?;
                        fx.nodes.push(split.clone().into());
                        fx.nodes.push(merge.clone().into());
                        pre.connect_with_audio_node(&split)?;
                        split.connect_with_audio_node_and_output_and_input(&merge, 0, 0)?;
                        split.connect_with_audio_node_and_output_and_input(&merge, 1, 1)?;
                        for (from, to) in [(0u32, 1u32), (1, 0)] {
                            let lp = ctx.create_biquad_filter()?;
                            lp.set_type(web_sys::BiquadFilterType::Lowpass);
                            lp.frequency().set_value(700.0);
                            lp.q().set_value(0.5);
                            let dl = ctx.create_delay_with_max_delay_time(0.01)?;
                            dl.delay_time().set_value(0.0003);
                            let g = ctx.create_gain()?;
                            fx.nodes.push(lp.clone().into());
                            fx.nodes.push(dl.clone().into());
                            fx.nodes.push(g.clone().into());
                            split.connect_with_audio_node_and_output(&lp, from)?;
                            lp.connect_with_audio_node(&dl)?;
                            dl.connect_with_audio_node(&g)?;
                            g.connect_with_audio_node_and_output_and_input(&merge, 0, to)?;
                            fx.cross.push(g);
                        }
                        last = merge.into();
                        let trim = ctx.create_gain()?;
                        add(fx, &mut last, trim.clone().into())?;
                        fx.trim = Some(trim);
                    }
                    if mono {
                        let m = ctx.create_gain()?;
                        m.set_channel_count(1);
                        m.set_channel_count_mode(web_sys::ChannelCountMode::Explicit);
                        m.set_channel_interpretation(web_sys::ChannelInterpretation::Speakers);
                        add(fx, &mut last, m.into())?;
                        // back to two equal channels, so the panner sees stereo and stays at unity in the middle (a mono input would
                        // take the panner's mono law: −3 dB)
                        let up = ctx.create_gain()?;
                        up.set_channel_count(2);
                        up.set_channel_count_mode(web_sys::ChannelCountMode::Explicit);
                        up.set_channel_interpretation(web_sys::ChannelInterpretation::Speakers);
                        add(fx, &mut last, up.into())?;
                    }
                    let pan = ctx.create_stereo_panner()?;
                    add(fx, &mut last, pan.clone().into())?;
                    fx.pan = Some(pan);
                }
                last.connect_with_audio_node(&ctx.destination())?;
                Ok(())
            })(&mut fx);
            if wired.is_err() { dom::console_error("Sound effects: could not build the audio graph"); }
        }
        input.gain().set_value(if on { db_gain(p.f("eq_preamp")) as f32 } else { 1.0 });
        for (f, b) in fx.eq.iter().zip(bands.iter()) {
            f.set_type(biquad_type(&b.s("type")));
            f.frequency().set_value(b.f("freq") as f32);
            f.gain().set_value(b.f("gain") as f32);
            f.q().set_value(b.f("q") as f32);
        }
        if !fx.cross.is_empty() {
            let g = db_gain(xfeed_db(&xfeed));
            for n in &fx.cross { n.gain().set_value(g as f32); }
            if let Some(t) = &fx.trim { t.gain().set_value((1.0 / (1.0 + g)) as f32); }
        }
        if let Some(pan) = &fx.pan { pan.pan().set_value((p.f("balance") / 100.0) as f32); }
    });
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
    if HELD.with(|h| h.get()) { HELD_AT.with(|h| h.set(sec)); }
    end_fade();
    st_mut(|s| s.seek_at = now_ms());
    let Some(t) = cur() else { return };
    let id = t.i("id");
    if st(|s| s.dsp) && dsp_cache_on() && dsp_state(id) != "cached" && now_ms() - dsp_asked(id) > 3000.0 {
        // maybe the render has finished since: then the browser seeks in the pre-rendered file itself
        dom::spawn(async move {
            prepare_dsp(&t).await;
            if cur().is_some_and(|c| Rc::ptr_eq(&c, &t)) { seek_now(sec); }
        });
    } else { seek_now(sec); }
    update_time();
    emit();
}
/// Whole files (the original, a pre-rendered Cadenza file) seek in the browser; server-made streams restart at the point.
fn seek_now(sec: f64) {
    let Some(a) = audio() else { return };
    let cached = st(|s| s.dsp) && cur().is_some_and(|t| dsp_state(t.i("id")) == "cached");
    let mut local = !st(|s| s.transcoded);
    if cached && st(|s| s.offset) == 0.0 {
        let s = a.seekable();
        local = s.length() > 0 && s.end(s.length() - 1).map(|e| e >= sec - 0.5).unwrap_or(false);
    }
    st_mut(|s| s.seek_at = now_ms());
    if local { a.set_current_time(sec); }
    else {
        let (rung, dsp) = st(|s| (s.rung, s.dsp));
        load_opts(LoadOpts { transcode: true, rung: Some(rung), dsp: Some(dsp), offset: sec, resume: true, ..Default::default() });
    }
    update_time();
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
        resume_fx();
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
    let rung = st(|s| s.rung);
    if rung > 0 {
        let codec = lossy_codec();
        let why = if prefs().s("quality") == "saver" { "(Data saver is on)" } else { "(adaptive – the connection could not keep up)" };
        return Some((format!("{codec}-{rung}"), format!("{} {rung} kbps, lossy {why}", codec.to_uppercase())));
    }
    if st(|s| s.dsp) {
        let r = st(|s| s.dsp_rate);
        return Some((format!("cadenza-{}", js_num(r)), format!("Upsampled on the server by Cadenza to 24-bit / {} kHz (lossless FLAC){}", khz(r), dsp_note(&dsp_state(t.i("id"))))));
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
/// kbit/s → "0.25" / "4.2" / "12" (Mbit/s)
fn mbit(kbps: f64) -> String {
    if kbps < 1000.0 { format!("{:.2}", kbps / 1000.0) } else if kbps < 10000.0 { to_fixed1(kbps / 1000.0) } else { js_num(playback::js_round(kbps / 1000.0)) }
}
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
    if linked().is_none() && st(|s| s.dsp) && dsp_cache_on() && cur().is_some_and(|c| *c == t) { prepare_dsp(&t).await; }   // pre-rendered yet?
    let mut rows: Vec<(String, String, String)> = vec![];
    rows.push(("Source".into(), source_line(&t, info.as_ref()), if info.as_ref().is_some_and(|i| i.g("lossless") == &json!(false)) { "A lossy file: what the file lost is gone before Encore touches it".into() } else { String::new() }));
    let linked = linked();
    let pure;
    let mut untouched = false;
    let cadenza = linked.is_none() && transport().is_some_and(|(k, _)| k.starts_with("cadenza"));
    // a Cadenza device reports every stage of its own pipeline; a browser device just plays
    let stages: Vec<Value> = linked.as_ref().map(|l| l.state.g("signal").a("stages")).unwrap_or_default();
    let local_engine = linked.as_ref().is_some_and(|l| crate::remote::is_engine(&l.id));
    if let Some(l) = &linked {
        let sig = l.state.o("signal");
        let eng = l.state.o("engine");
        let tr = sig.s("transport");
        let what = if tr == "original" { "The original file, byte for byte".to_string() }
            else if tr.is_empty() { "Not reported by that device".to_string() }
            else if tr == "flac" { "Converted to FLAC by the server (lossless)".to_string() }
            else if let Some(rate) = tr.strip_prefix("cadenza-") { let n = num(rate); format!("Upsampled on the server by Cadenza ({} kHz)", khz(if truthy_num(n) { n } else { 0.0 })) }
            else if tr.starts_with("aac-") || tr.starts_with("opus-") { let (c, k) = tr.split_once('-').unwrap_or((&tr, "")); format!("{} {k} kbps (lossy)", c.to_uppercase()) }
            else { tr.clone() };
        rows.push((format!("Encore → {}", esc(&l.name)), what, String::new()));
        if !stages.is_empty() {
            for stg in &stages { rows.push((esc(&stg.s("name")), stg.s("detail"), String::new())); }
            rows.push(("Output".into(), format!("{}{}{}", eng.s("output"), if eng.truthy("exclusive") { " · exclusive" } else { "" }, if sig.truthy("bitperfect") { " · bit-perfect" } else { "" }), String::new()));
        } else {
            rows.push((esc(&l.name), if sig.truthy("output_rate") { format!("Plays through its browser audio at {} kHz", khz(sig.f("output_rate"))) } else { "Playing there".into() }, String::new()));
        }
        pure = false;
    } else {
        let tr = transport();
        let net = match est_kbps() { Some(k) if k > 0.0 => format!(" · connection ≈ {} Mbit/s", mbit(k)), _ => String::new() };
        rows.push(("Encore → this device".into(), format!("{}{net}", tr.as_ref().map(|(_, x)| x.clone()).unwrap_or_default()), String::new()));
        let out = output_rate();
        let src = info.as_ref().map(|i| i.f("sample_rate")).unwrap_or(0.0);
        let resampled = truthy_num(out) && truthy_num(src) && out != src;
        rows.push(("This device".into(),
            if truthy_num(out) { format!("Browser audio → the system mixer at {} kHz", khz(out)) } else { "Browser audio → the system mixer".into() },
            if resampled { format!("Your device resamples {} → {} kHz. A browser can't hand bits straight to the DAC; for true bit-perfect output play through a Cadenza device or an exclusive-mode player.", khz(src), khz(out)) }
            else if truthy_num(out) && truthy_num(src) { "Same rate as the file – no resampling in the browser. Whether the operating system mixes it with other sounds is up to your audio settings.".into() }
            else { String::new() }));
        let effects = playback::effects_summary(&prefs(), true);
        if !effects.is_empty() { rows.push(("Effects on this device".into(), effects, String::new())); }
        let tr = transport();
        untouched = tr.as_ref().is_some_and(|(k, _)| k == "original") && !info.as_ref().is_some_and(|i| i.g("lossless") == &json!(false));
        pure = untouched && !playback::fx_active(&prefs());
    }
    let Some(body_el) = m.qs(".modal-body") else { return };
    let rows_html: String = rows.iter().map(|(k, v, note)| format!(r#"<div class="stack" style="gap:2px"><div class="small muted">{k}</div><div style="font-weight:600">{}</div>{}</div>"#,
        esc(v), if note.is_empty() { String::new() } else { format!(r#"<div class="small muted">{}</div>"#, esc(note)) })).collect();
    let (head, text) = if let Some(l) = &linked {
        if !stages.is_empty() {
            (format!("Played by Cadenza on {}", if local_engine { "this computer".to_string() } else { esc(&l.name) }), "Every stage above is what the engine reports. Bit-perfect means nothing touched the samples.")
        } else {
            ("Playing on another device".to_string(), "The rows above are what that device reports. Open Encore there for its own signal path.")
        }
    } else if cadenza {
        ("Processed by Cadenza on the server".to_string(), "Encore upsampled the file on the server with Cadenza and sent lossless 24-bit FLAC. Turn it off under Settings → Playback → Server processing to get the untouched file.")
    } else if pure {
        ("Pure playback from Encore".to_string(), "Encore sent the file exactly as it is stored. Everything after that – volume, mixing, resampling – is your browser and operating system.")
    } else if untouched {
        ("Not the untouched file".to_string(), "Encore sent the file untouched; this device adds the effects you chose under Settings → Playback → Sound.")
    } else {
        ("Not the untouched file".to_string(), "Encore converted this stream (see above). Settings → Playback → Quality → Original keeps the file untouched even when the connection is slow.")
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
    let pl = plain();
    for x in [Some(a), Some(sp), pl.as_ref()].into_iter().flatten() {
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
    let pl: HtmlMediaElement = dom::create("audio").dyn_into::<HtmlMediaElement>().expect("audio");
    pl.set_id("audio-preview");
    pl.set_preload("auto");
    let _ = body().append_child(&pl);
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
    let vol = if vol.is_finite() { vol.clamp(0.0, 1.0) } else { 0.9 };
    USER_VOL.with(|u| u.set(vol));
    a.set_volume(vol);
    sp.set_volume(vol);
    pl.set_volume(vol);
    AUDIO.with(|x| *x.borrow_mut() = Some(a.clone()));
    SPARE.with(|x| *x.borrow_mut() = Some(sp.clone()));
    PLAIN.with(|x| *x.borrow_mut() = Some(pl.clone()));
    // the first bandwidth guess (navigator.connection.downlink) is taken now, at load, as in the JS UI
    EST.with(|_| ());
    // every element gets the listeners; only the one currently playing counts
    on_both(&a, &sp, "ended", Rc::new(|_| { log_play(); next(true); }));
    on_both(&a, &sp, "play", Rc::new(|_| {
        HELD.with(|h| h.set(false));
        HELD_AT.with(|h| h.set(0.0));
        PLAYED_SINCE.with(|p| p.set(true));
        if let Some(t) = cur() { announce(&t); }
        resume_fx();
        if let Some(out) = FADE.with(|f| f.borrow().as_ref().map(|f| f.out.clone())) { play(&out); }
        render(); emit();
    }));
    on_both(&a, &sp, "pause", Rc::new(|_| {
        if let Some(out) = FADE.with(|f| f.borrow().as_ref().map(|f| f.out.clone())) { let _ = out.pause(); }
        render(); emit(); save_spot(true);
    }));
    on_both(&a, &sp, "timeupdate", Rc::new(|_| {
        update_time(); log_play(); sync_lyrics(false); maybe_transition();
        if audio().is_some_and(|a| !a.paused()) { save_spot(false); }
        preload_tick();
    }));
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
        // a restored song stays paused until you press play
        if !st(|s| s.transcoded) && !t.truthy("preview") {
            load_opts(LoadOpts { transcode: true, offset: spot_position(), hold: HELD.with(|h| h.get()), ..Default::default() });
        }
    }));
    dom::on_forever(&sp, "error", |_| { if let Some(s) = spare() { let _ = s.remove_attribute("data-url"); } });
    // a transition moves with either song's clock (timers stall in hidden windows), and ends when the old song does
    for x in [Some(&a), Some(&sp), Some(&pl)].into_iter().flatten() {
        dom::on_forever(x, "timeupdate", |_| { if fading() { step_fade(); } });
        dom::on_forever(x, "ended", |e: &Event| {
            let out = FADE.with(|f| f.borrow().as_ref().map(|f| f.out.clone()));
            let target = e.target().and_then(|t| t.dyn_into::<HtmlMediaElement>().ok());
            if let (Some(out), Some(t)) = (out, target) { if same_el(&out, &t) { end_fade(); } }
        });
    }
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
        // Space is play/pause anywhere except while typing – even when a player button has focus
        // (otherwise the browser "presses" the focused button again, e.g. Previous, which restarts the song)
        let typing = ae.as_ref().map(|a| {
            let tag = a.tag_name();
            let ty = a.get_attribute("type").unwrap_or_default().to_lowercase();
            let editable = a.dyn_ref::<web_sys::HtmlElement>().map(|h| h.is_content_editable()).unwrap_or(false);
            (tag == "INPUT" && !["range", "checkbox", "radio", "button", "submit"].contains(&ty.as_str())) || tag == "TEXTAREA" || tag == "SELECT" || editable
        }).unwrap_or(false);
        let space = e.dyn_ref::<KeyboardEvent>().map(|k| k.code() == "Space").unwrap_or(false);
        if space && !typing && dom::qs(".modal-bg").is_none() {
            e.prevent_default();
            if cur().is_some() || linked { toggle(); }
            return;
        }
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
    // the button press itself happens on keyup – swallow it too
    let keyup = EventListener::new_with_options(&document(), "keyup", EventListenerOptions::enable_prevent_default(), |e: &Event| {
        let on_button = document().active_element().map(|a| a.tag_name() == "BUTTON").unwrap_or(false);
        let space = e.dyn_ref::<KeyboardEvent>().map(|k| k.code() == "Space").unwrap_or(false);
        if space && on_button && dom::qs(".modal-bg").is_none() { e.prevent_default(); }
    });
    dom::keep_forever(keyup);
    dom::on_window("hashchange", |_| { if st(|s| s.full) { set_full(false); } });
    dom::on_window("pagehide", |_| save_spot(true));
    dom::on_window("encore:likes", |_| { LIKED.with(|l| *l.borrow_mut() = None); dom::spawn(async { likes().await; paint_likes(); }); });
    dom::on_window("encore:playback-prefs", |_| {
        new_prefs(playback::current());
        fetch_dsp();
        if !prefs().truthy("preload") { cancel_preload(); } else { maybe_preload(true); }
        if st(|s| s.full) { render_full(); }
    });
    dom::on_window("storage", |e| {
        let key = e.dyn_ref::<web_sys::StorageEvent>().and_then(|s| s.key());
        if key.as_deref() == Some("encore.playback") { new_prefs(playback::current()); }
    });
    fetch_dsp();
    dom::interval_forever(2000, adapt_tick);
    render();
}

/// The server's Cadenza is off, busy or failed: the same spot again without processing, and no more processing this session.
fn dsp_fallback() {
    DSP_FAILED.with(|f| f.set(true));
    let at = spot_position();
    if !DSP_TOASTED.with(|t| t.replace(true)) { toast("Server processing isn't available right now – playing the original", ""); }
    // a restored song stays paused until you press play
    load_opts(LoadOpts { transcode: at > 0.5, offset: if at > 0.5 { at } else { 0.0 }, dsp: Some(false), hold: HELD.with(|h| h.get()), ..Default::default() });
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
    let rung = st(|s| s.rung);
    if rung > 0 { return format!("{f} → {} {rung}k", lossy_codec().to_uppercase()); }
    if st(|s| s.dsp) { return format!("{f} → Cadenza {} kHz", khz(st(|s| s.dsp_rate))); }
    if st(|s| s.transcoded) { format!("{f} → FLAC") }
    else { format!("{f}{}", if t.truthy("bitrate") { format!(" {}k", t.s("bitrate")) } else { String::new() }) }
}
fn quality_title(t: &Value) -> String {
    if t.truthy("remote") { return format!("What {} is playing", { let n = link_name(); if n.is_empty() { "that device".to_string() } else { n } }); }
    if t.truthy("preview") { return "A 30-second preview – not in your library".into(); }
    let rung = st(|s| s.rung);
    if rung > 0 { return if prefs().s("quality") == "saver" { "Data saver: a small 96 kbps stream".into() } else { format!("Adaptive quality: {rung} kbps while the connection can't keep up with the original") }; }
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
    let badge = match t { Some(t) if !t.truthy("preview") => format!(r#"<button class="badge hide-sm" id="p-signal" type="button" title="{} – click for the signal path">{}</button>"#, esc(&quality_title(t)), esc(&quality(t))), _ => String::new() };
    let like = if can_like(t) { like_button("p-like", if t.is_some_and(is_liked) { "on" } else { "" }) } else { String::new() };
    let devices = if has_encore_remote() { format!(r#"<button class="btn ghost icon" id="p-devices" aria-label="Other devices" title="Play on or control your other devices">{}</button>"#, icon("devices")) } else { String::new() };
    let vol = if is_linked() { let s = link_state_value(); if s.is_nullish("volume") { "0.9".to_string() } else { s.s("volume") } } else { js_num(user_vol()) };
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
    wire(ls, el, "#p-signal", "click", |_| dom::spawn(signal_path()));
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
            format!(r#"<button class="btn ghost icon" id="p-devices" aria-label="Other devices" title="Play on or control your other devices">{}</button>"#, icon("devices"))
        } else { String::new() };
        set_html(&el, &format!(r##"<div class="now"><div class="thumb"><div class="ph">{music}</div></div>
      <div class="grow"><div style="font-weight:650" class="faint">Nothing playing</div>
      <div class="small muted ellipsis">Open an album in <a href="#/library" style="color:var(--accent)">Library</a> and press ▶ Play</div></div></div>
      <div class="ctrls idle">
        <div class="row">
          <button class="btn ghost icon" id="p-prev" aria-label="Previous" disabled>{prev}</button>
          <button class="play-btn" id="p-toggle" aria-label="Play/Pause" disabled>{play}</button>
          <button class="btn ghost icon" id="p-next" aria-label="Next" disabled>{next}</button>
        </div>
        <div class="seek"><span>0:00</span><input type="range" min="0" max="1000" value="0" disabled aria-label="Seek"><span>0:00</span></div>
      </div>
      <div class="right">
        {devices}
        <span class="faint vol-ic">{volume}</span>
        <input type="range" id="vol" min="0" max="1" step="0.01" value="{vol}">
      </div>"##, music = icon_attr("music", r#"width="20""#), prev = icon("prev"), play = icon("play"), next = icon("next"),
            volume = icon_attr("volume", r#"width="18""#), vol = js_num(user_vol())));
        let mut ls = Vec::new();
        wire(&mut ls, &el, "#p-devices", "click", |_| remote_open());
        wire(&mut ls, &el, "#vol", "input", |e| { if let Some(v) = dom::target_el(e) { local_volume(num(&dom::value(&v))); } });
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
        thumb = crate::ui::art_sized(&cover(&t), &album_or_title(&t), "", 200),
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
        end_fade();
        if let (Some(a), Some(sp)) = (audio(), spare()) {
            let _ = a.pause();
            let _ = a.remove_attribute("src");
            let _ = sp.remove_attribute("src");
            let _ = sp.remove_attribute("data-url");
        }
        st_mut(|s| { s.queue.clear(); s.index = -1; });
        HELD.with(|h| h.set(false));
        HELD_AT.with(|h| h.set(0.0));
        dom::ls_remove(RESUME);
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
    let volume = if link.is_some() { if s.is_nullish("volume") { "0.9".to_string() } else { s.s("volume") } } else { js_num(user_vol()) };
    let autoplay = prefs().truthy("autoplay");
    dom::set_style(&f, "--np-bg", &format!(r#"url("{}")"#, cover_at(&t, 400)));
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
            {btns}</div>"#, crate::ui::art_sized(&cover(x), &album_or_title(x), "", 200), esck(x, "title"), esck(x, "artist"))
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
