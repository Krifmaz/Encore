//! Port of web/js/player.js – see docs/archive/ui-rewrite/CONTRACT.md
//! Web player: streams the original file (bit-perfect for MP3/FLAC/AAC/Opus); formats a browser
//! can't decode (ALAC/APE/WavPack/AIFF/WMA) are converted to FLAC on the fly by the server. On Auto quality it
//! measures the connection and steps the stream down (256 → 96 kbps AAC/Opus, 64 at worst) and back up as the bandwidth
//! allows. A stream that breaks off mid-song (a dropped connection, a locked phone) is picked up again at the same spot.
//! Optional sound effects (EQ, crossfeed, balance, mono) run in this browser through Web Audio, and crossfade
//! blends one song into the next with the two <audio> elements.
//! Tap the bar for the full-screen player (lyrics + Up next). When the queue runs out it keeps going
//! with a shuffle of the whole library: a single song → straight into the shuffle, an album → after its last song.
//!
//! The player is split by concern: queue.rs (the queue, shuffle and repeat, the saved spot), load.rs (which
//! stream to ask for and starting it, seeking, play/pause, volume), recovery.rs (a stream that breaks off),
//! bandwidth.rs (Auto quality), transitions.rs (crossfade and overlap), fx.rs (Web Audio effects), bar.rs (the
//! bar at the bottom), full.rs (the full-screen player), lyrics.rs, media_session.rs (lock screen and the native
//! apps), link.rs (this device as a remote, and remote control of it) and signal.rs (the Signal path window).
//! The shared state, the <audio> elements and the small helpers every part uses live here, with `mount()`.

mod bandwidth;
mod bar;
mod full;
mod fx;
mod link;
mod load;
mod local;
mod lyrics;
mod media_session;
mod queue;
mod recovery;
mod signal;
mod transitions;

pub use bar::{bar_buttons, customize_bar_modal, set_bar_buttons, BAR_ACTIONS};
pub use full::set_full;
pub use fx::device_rate;
pub use link::{command, link_state, linked, set_link, snapshot, Link};
pub use load::{seek_to, set_volume, toggle};
pub(crate) use load::can_play;
pub use local::{offline_setting, prefer_local, set_prefer_local};
pub use media_session::{can_save_offline, save_offline};
pub use queue::{enqueue, next, play_next, play_queue, prev, preview, queue, restore, saved_spot, shuffle_library};
pub(crate) use crate::num::{khz, to_fixed1};

use bandwidth::*;
use bar::*;
use full::*;
use fx::*;
use link::*;
use load::*;
use local::*;
use lyrics::*;
use media_session::*;
use queue::*;
use recovery::*;
use signal::*;
use transitions::*;

use crate::api::{self, ApiError};
use crate::dom::{self, body, by_id, document, now_ms, qs_in, qsa_in, set_html, EventListener, Interval, Timeout};
use crate::json::{self, js_string, J};
use crate::num::{js_num, num, truthy_num};
use crate::ui::{art, esc, esck, fmt, icon, icon_attr, pop_menu, share_track, toast, MenuItem, ModalOpts};
use crate::{app, native, playback};
use gloo_events::EventListenerOptions;
use serde_json::{json, Map, Value};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{Element, Event, HtmlElement, HtmlMediaElement, KeyboardEvent, ScrollBehavior, ScrollToOptions};

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
    /// shuffle mode, and the queue's order from before shuffling (put back when it goes off)
    shuffle: bool,
    order: Option<Vec<Rc<Value>>>,
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
    /// you want music now (a song started, play pressed) – false only after a real pause (button, headset, lock screen)
    want: bool,
    /// where the song last was for sure
    at: f64,
    /// when we last gave the element a new source (its 'emptied'/'abort' are ours)
    src_at: f64,
    /// Auto's lowest rung (96 kbps; 64 after 96 stalled 3 times within 2 minutes), and those stalls at 96
    floor: i64,
    low_stalls: Vec<f64>,
    /// the song plays from its copy downloaded to this device (local.rs), not from the server
    local_copy: bool,
}
impl Default for State {
    fn default() -> Self {
        Self { queue: vec![], index: -1, transcoded: false, lossy: false, rung: 0, dsp: false, dsp_rate: 0.0, offset: 0.0, dragging: false, context: "list".into(), repeat: "off".into(), shuffle: false, order: None, full: false, pane: "now".into(), logged: false, announced: None,
            stalls: vec![], stall_at: 0.0, seek_at: 0.0, step_at: 0.0, switched_at: 0.0, last_preload_check: 0.0, slow_start: None,
            want: false, at: 0.0, src_at: 0.0, floor: playback::AUTO_FLOOR, low_stalls: vec![], local_copy: false }
    }
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
    /// `let prefs = playback.current()`
    static PREFS: RefCell<Value> = RefCell::new(playback::current());
    /// the volume slider: the ceiling of every fade
    static USER_VOL: Cell<f64> = const { Cell::new(0.9) };
    /// a restored spot sits paused and hasn't played yet: its position may not be applied yet, so it isn't saved over
    static HELD: Cell<bool> = const { Cell::new(false) };
    /// where the held song was restored to (the original file only seeks there once its metadata is in)
    static HELD_AT: Cell<f64> = const { Cell::new(0.0) };
}

fn user_vol() -> f64 { USER_VOL.with(|v| v.get()) }

/// `a === b` for two elements.
fn same_el(a: &HtmlMediaElement, b: &HtmlMediaElement) -> bool { let x: &JsValue = a.as_ref(); let y: &JsValue = b.as_ref(); x == y }

fn st<R>(f: impl FnOnce(&State) -> R) -> R { STATE.with(|s| f(&s.borrow())) }
fn st_mut<R>(f: impl FnOnce(&mut State) -> R) -> R { STATE.with(|s| f(&mut s.borrow_mut())) }
fn audio() -> Option<HtmlMediaElement> { AUDIO.with(|a| a.borrow().clone()) }
fn spare() -> Option<HtmlMediaElement> { SPARE.with(|a| a.borrow().clone()) }
fn plain() -> Option<HtmlMediaElement> { PLAIN.with(|a| a.borrow().clone()) }
fn is_plain(el: &HtmlMediaElement) -> bool { plain().is_some_and(|p| same_el(&p, el)) }

/// `state.queue[state.index] || null` (the queue entry itself, for identity checks).
fn cur() -> Option<Rc<Value>> { st(|s| if s.index < 0 { None } else { s.queue.get(s.index as usize).cloned() }) }

fn prefs() -> Value { PREFS.with(|p| p.borrow().clone()) }

/// `audio.play().catch(() => {})`.
fn play(a: &HtmlMediaElement) {
    if let Ok(p) = a.play() { dom::spawn(async move { let _ = wasm_bindgen_futures::JsFuture::from(p).await; }); }
}
/// `new URL(u, location.href).href`.
fn abs_url(u: &str) -> String {
    let base = dom::location().href().unwrap_or_default();
    web_sys::Url::new_with_base(u, &base).map(|x| x.href()).unwrap_or_default()
}

/// `root.querySelector(sel).on<ev> = f`.
fn wire(out: &mut Vec<EventListener>, root: &Element, sel: &str, ev: &str, f: impl FnMut(&Event) + 'static) {
    if let Some(el) = qs_in(root, sel) { out.push(dom::listener(&el, ev, f)); }
}

/// `t.album || t.title` (the label `art` makes the placeholder letter from).
fn album_or_title(t: &Value) -> String { if t.truthy("album") { t.s("album") } else { t.s("title") } }
/// `!!window.encoreRemote`.
fn has_encore_remote() -> bool { js_sys::Reflect::get(&dom::window(), &"encoreRemote".into()).map(|v| v.is_truthy()).unwrap_or(false) }
/// `window.encoreRemote?.open()`.
fn remote_open() { if has_encore_remote() { crate::remote::open(); } }
/// `window.encoreRemote?.unlink()`.
fn remote_unlink() { if has_encore_remote() { crate::remote::unlink(); } }

fn js_set(o: &JsValue, key: &str, v: &JsValue) { let _ = js_sys::Reflect::set(o, &key.into(), v); }

/// `document.createElement('audio').canPlayType(mime) !== ''`.
fn can_play_type(mime: &str) -> bool {
    dom::create("audio").dyn_into::<HtmlMediaElement>().map(|a| !a.can_play_type(mime).is_empty()).unwrap_or(true)
}

async fn js_await(p: JsValue) -> Option<JsValue> {
    let p = p.dyn_into::<js_sys::Promise>().ok()?;
    wasm_bindgen_futures::JsFuture::from(p).await.ok()
}
fn js_get(o: &JsValue, key: &str) -> JsValue { js_sys::Reflect::get(o, &key.into()).unwrap_or(JsValue::UNDEFINED) }
fn js_call0(o: &JsValue, name: &str) -> Option<JsValue> {
    js_get(o, name).dyn_into::<js_sys::Function>().ok()?.call0(o).ok()
}

// ---- listeners: the pages and the remote are told whenever the song changes (the native apps too, see media_session.rs) ----
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

pub fn current() -> Option<Value> { cur().map(|t| (*t).clone()) }

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
    crate::ui::cover_url(&t.s("album_id"), crate::ui::cover_q(t).trim_start_matches("?v="), size)
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
    // a stream that broke off mid-song also says 'ended': only a song that really reached its end moves on
    on_both(&a, &sp, "ended", Rc::new(|_| {
        if !really_ended() { if rec_deferred() { retry_now(); } return; }
        log_play(); next(true);
    }));
    on_both(&a, &sp, "play", Rc::new(|_| {
        HELD.with(|h| h.set(false));
        HELD_AT.with(|h| h.set(0.0));
        PLAYED_SINCE.with(|p| p.set(true));
        st_mut(|s| s.want = true);
        if let Some(t) = cur() { announce(&t); }
        resume_fx();
        if let Some(out) = FADE.with(|f| f.borrow().as_ref().map(|f| f.out.clone())) { play(&out); }
        render(); emit(); media_state();
    }));
    // a pause nobody here made (headphones unplugged, a call or another app took the sound) is a real pause too: it stays
    // paused – not the end of the song and not a stream being picked up again
    on_both(&a, &sp, "pause", Rc::new(|_| {
        if !audio().is_some_and(|a| a.ended()) && !rec_need() && !HELD.with(|h| h.get()) { st_mut(|s| s.want = false); }
        if let Some(out) = FADE.with(|f| f.borrow().as_ref().map(|f| f.out.clone())) { let _ = out.pause(); }
        render(); emit(); save_spot(true); media_state();
    }));
    on_both(&a, &sp, "playing", Rc::new(|_| { let w = STALL_WATCH.with(|w| w.borrow_mut().take()); drop(w); }));
    on_both(&a, &sp, "timeupdate", Rc::new(|_| {
        update_time(); log_play(); sync_lyrics(false); maybe_transition();
        if audio().is_some_and(|a| a.ready_state() >= 2 && !a.seeking()) && !rec_need() {
            let p = position();
            st_mut(|s| s.at = p);
            // playing on well after a reload: the failures in a row are over
            REC.with(|r| { let mut r = r.borrow_mut(); if r.fails > 0 && p > r.from + 10.0 { r.fails = 0; r.empty = 0; } });
        } else if rec_need() && cur().is_some_and(|t| rec_is(&t)) && audio().is_some_and(|a| a.ready_state() >= 2 && !a.seeking() && !a.paused()) {
            // broke off but still playing what it has: the spot to pick it up at moves along
            let d = duration();
            let p = position();
            let at = (if d > 0.0 { p.min(d - 1.0) } else { p }).max(0.0);
            REC.with(|r| r.borrow_mut().at = at);
            st_mut(|s| s.at = at);
        }
        if audio().is_some_and(|a| !a.paused()) { save_spot(false); }
        if now_ms() - POS_STATE_AT.with(|c| c.get()) > 5000.0 { media_state(); }
        preload_tick();
    }));
    on_both(&a, &sp, "progress", Rc::new(|_| preload_tick()));
    on_both(&a, &sp, "waiting", Rc::new(|_| { if rec_deferred() && rec_need() { retry_now(); return; } stalled(); watch_stall(); }));
    on_both(&a, &sp, "stalled", Rc::new(|_| watch_stall()));
    on_both(&a, &sp, "loadedmetadata", Rc::new(|_| update_time()));
    // the element dropped its source by itself (not one of our loads): pick the song up again
    let lost: Rc<dyn Fn(&Event)> = Rc::new(|_| {
        let has_src = audio().and_then(|a| a.get_attribute("src")).is_some_and(|s| !s.is_empty());
        if has_src && !HELD.with(|h| h.get()) && now_ms() - st(|s| s.src_at) > 1500.0 { retry(None); }
    });
    on_both(&a, &sp, "emptied", lost.clone());
    on_both(&a, &sp, "abort", lost);
    // the connection broke off (any stream) → the same spot again, the same stream
    // a reload near the end asked past the file's real end (nothing to play) → the song is over
    // the server's Cadenza is off, busy or failed → the same spot again without processing, and no more processing this session
    // the browser couldn't decode the original (e.g. ALAC in .m4a) → ask the server for FLAC
    // a server-made stream that failed some other way → the same spot again
    on_both(&a, &sp, "error", Rc::new(|_| {
        let Some(t) = cur() else { return };
        let has_src = audio().and_then(|a| a.get_attribute("src")).is_some_and(|s| !s.is_empty());
        if !has_src || t.truthy("preview") { return; }
        // a downloaded copy that won't play: the server's stream instead
        if st(|s| s.local_copy) { copy_failed(&t); return; }
        let code = audio().map(|a| js_get(&js_get(a.as_ref(), "error"), "code").as_f64().unwrap_or(0.0)).unwrap_or(0.0);
        if code == 2.0 { retry(None); return; }   // MEDIA_ERR_NETWORK
        if failed_past_end() { log_play(); next(true); return; }
        if st(|s| s.dsp) { dsp_fallback(); return; }
        // a restored song stays paused until you press play
        if !st(|s| s.transcoded) {
            load_opts(LoadOpts { transcode: true, offset: spot_position(), hold: HELD.with(|h| h.get()), ..Default::default() });
            return;
        }
        retry(None);
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
        // the lock screen's / headset's play and pause are yours: a pause there stays paused
        set_action("play", |_| user_play());
        set_action("pause", |_| user_pause());
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
    dom::on_forever(&document(), "visibilitychange", |_| wake());
    dom::on_window("pageshow", |_| wake());
    // the network is back while a broken-off song waits for its next try: try now
    dom::on_window("online", |_| { if rec_need() && st(|s| s.want) { retry_now(); } });
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
    local::start();
    dom::interval_forever(2000, adapt_tick);
    render();
}
