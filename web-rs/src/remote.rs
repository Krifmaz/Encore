//! Port of web/js/remote.js – see docs/ui-rewrite/CONTRACT.md
//!
//! Use any signed-in device as a remote for the others: every open app checks in every couple of
//! seconds with what it's playing and runs the commands other devices sent it. With the server's event stream
//! open, commands and the other devices' state arrive the moment they change instead of at the next check-in,
//! and this device checks in right away whenever its own playback changes – so a remote feels instant.

use crate::dom;
use crate::json::J;
use crate::ui::{self, art, esc, fmt, icon, toast, ModalOpts};
use crate::{api, app, player};
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};

const KEY: &str = "encore.device";
const NAME_KEY: &str = "encore.device.name";
// Linked: this device is locked as a remote for another one until you unlink.
const CHAIN: &str = "encore.link";
// The Cadenza engine built into the desktop app: it signs in with its own token and shows up as a device of
// its own, which this window links to on its own – unless you unlinked it (encore.engine.off = "1").
const ENGINE_OFF: &str = "encore.engine.off";
pub const ENGINE_LABEL: &str = "This computer (Cadenza)";

thread_local! {
    static STARTED: Cell<bool> = const { Cell::new(false) };
    static MISSED: Cell<u32> = const { Cell::new(0) };
    /// `const ME = deviceId()` – computed once, on first use.
    static ME: String = device_id();
    /// the other devices, as the server last told us (pushed, or fetched)
    static DEVICES: RefCell<Vec<Value>> = const { RefCell::new(Vec::new()) };
    /// the open Devices window repaints through this
    static ON_DEVICES: RefCell<Option<Rc<dyn Fn()>>> = const { RefCell::new(None) };
    /// commands already run (each one can arrive pushed and again on the next check-in)
    static SEEN: RefCell<Vec<i64>> = const { RefCell::new(Vec::new()) };
    /// the server's event stream
    static ES: RefCell<Option<web_sys::EventSource>> = const { RefCell::new(None) };
    static BEAT_TIMER: RefCell<Option<dom::Timeout>> = const { RefCell::new(None) };
    static ON_CHANGE: RefCell<Option<player::OnChange>> = const { RefCell::new(None) };
    /// `{ id, name }` of this computer's engine (desktop app only), known before it is online
    static ENGINE: RefCell<Option<(String, String)>> = const { RefCell::new(None) };
    /// the pick-up bar's click handlers and its player subscription (dropping them removes them)
    static SPOT: RefCell<Option<(Vec<dom::EventListener>, player::OnChange)>> = const { RefCell::new(None) };
}

fn me() -> String { ME.with(|m| m.clone()) }

/// `n.toString(36)`.
fn to36(n: f64) -> String { js_sys::Number::from(n).to_string_with_radix(36).map(String::from).unwrap_or_default() }
/// `Math.random().toString(36).slice(2)`.
fn rand36() -> String {
    let s = js_sys::JsString::from(to36(js_sys::Math::random()));
    String::from(s.slice(2, s.length()))
}

fn device_id() -> String {
    let fallback = || format!("tab-{}", rand36());
    let Some(store) = dom::local_storage() else { return fallback() };
    match store.get_item(KEY) {
        Ok(Some(id)) if !id.is_empty() => id,
        Ok(_) => {
            let id = format!("{}{}", rand36(), to36(dom::now_ms()));
            if store.set_item(KEY, &id).is_err() { return fallback(); }
            id
        }
        Err(_) => fallback(),
    }
}

/// `window.EncoreNative?.platform`.
fn native_platform() -> Option<String> {
    let n = js_sys::Reflect::get(&dom::window(), &"EncoreNative".into()).ok()?;
    if n.is_undefined() || n.is_null() { return None; }
    js_sys::Reflect::get(&n, &"platform".into()).ok()?.as_string()
}

/// A readable name: "Android app", "Chrome on Windows", "Safari on iPhone" … (or the one you gave it)
fn device_name() -> String {
    if let Some(own) = dom::ls_get(NAME_KEY).filter(|s| !s.is_empty()) { return own; }
    let ua = dom::window().navigator().user_agent().unwrap_or_default();
    if ua.contains("EncoreAndroid") || native_platform().as_deref() == Some("android") { return "Android app".into(); }
    if ua.contains("EncoreDesktop") || ua.contains("Electron") || native_platform().as_deref() == Some("desktop") {
        return format!("Desktop app ({})", if ua.contains("Mac") { "Mac" } else if ua.contains("Windows") { "Windows" } else { "Linux" });
    }
    let os = if ua.contains("iPhone") { "iPhone" } else if ua.contains("iPad") { "iPad" } else if ua.contains("Android") { "Android" }
        else if ua.contains("Mac") { "Mac" } else if ua.contains("Windows") { "Windows" } else if ua.contains("Linux") { "Linux" } else { "device" };
    let br = if ua.contains("Edg/") { "Edge" } else if ua.contains("Firefox/") { "Firefox" } else if ua.contains("Chrome/") { "Chrome" }
        else if ua.contains("Safari/") { "Safari" } else { "Browser" };
    format!("{br} on {os}")
}

/// `JSON.parse(localStorage.getItem(CHAIN) || 'null')` – `Some` only when that is truthy.
fn saved_link() -> Option<Value> {
    let raw = dom::ls_get(CHAIN).filter(|s| !s.is_empty()).unwrap_or_else(|| "null".into());
    serde_json::from_str::<Value>(&raw).ok().filter(crate::json::truthy)
}

/// `api.post(`/remote/${id}/command`, { ...body, from: deviceName() })`.
async fn send_to(id: String, body: Value) -> api::ApiResult {
    let mut body = if body.is_object() { body } else { json!({}) };
    body["from"] = Value::String(device_name());
    api::post(&format!("/remote/{}/command", String::from(js_sys::encode_uri_component(&id))), body).await
}

/// `(c) => sendTo(id, c).catch(e => toast(e.message, 'err'))` – the link's `send`.
fn link_sender(id: String) -> Rc<dyn Fn(Value)> {
    Rc::new(move |c| {
        let id = id.clone();
        dom::spawn(async move { if let Err(e) = send_to(id, c).await { toast(&e.message, "err"); } });
    })
}

/// This computer's Cadenza engine as a device `(id, name)` – only in the desktop app, None elsewhere.
pub fn engine_device() -> Option<(String, String)> { ENGINE.with(|e| e.borrow().clone()) }
pub fn is_engine(id: &str) -> bool { ENGINE.with(|e| e.borrow().as_ref().is_some_and(|(eid, _)| eid == id)) }
/// The name a device is shown under: the built-in engine is "This computer (Cadenza)".
pub fn shown_name(d: &Value) -> String { if is_engine(&d.s("id")) { ENGINE_LABEL.to_string() } else { d.s("name") } }
/// You unlinked this computer's engine: it is not linked to again on its own.
pub fn engine_off() -> bool { dom::ls_get(ENGINE_OFF).as_deref() == Some("1") }

pub fn link_to(d: Value) { link_to_opts(d, false); }
fn link_to_opts(d: Value, silent: bool) {
    let name = shown_name(&d);
    hide_spot();
    let id = d.s("id");
    if is_engine(&id) { dom::ls_remove(ENGINE_OFF); }
    dom::ls_set(CHAIN, &json!({ "id": d.g("id"), "name": name }).to_string());
    player::set_link(Some(player::Link { id: id.clone(), name: name.clone(), state: d.o("state"), online: true, send: link_sender(id) }));
    if !silent { toast(&format!("Linked – this device now controls {name}"), "ok"); }
    dom::spawn(beat());
}

pub fn unlink() {
    if player::linked().is_some_and(|l| is_engine(&l.id)) { dom::ls_set(ENGINE_OFF, "1"); }
    dom::ls_remove(CHAIN);
    player::set_link(None);
    toast("Unlinked – music plays on this device again", "ok");
    dom::spawn(beat());
}

/// Desktop app: sign the built-in engine in. The app knows the engine's id and name before it runs; the
/// server hands out a token for them, and the app starts the engine with it. Idempotent – safe on every start.
async fn engine_sign_in() {
    if !app::native_truthy("engine") || !app::native_has("engineStatus") || !app::native_has("engineLogin") { return; }
    // any failure: Settings → Playback → Cadenza engine says what is wrong
    let Some(st) = app::native_call_async("engineStatus", None).await else { return };
    if !st.truthy("device_id") { return; }
    let name = if st.truthy("name") { st.s("name") } else { "Cadenza".to_string() };
    ENGINE.with(|e| *e.borrow_mut() = Some((st.s("device_id"), name.clone())));
    // already running with a token and no error: keep it – a new token would restart the engine's link
    if st.truthy("running") && !st.truthy("error") { return; }
    let Ok(r) = api::post("/auth/app-token", json!({ "device_id": st.s("device_id"), "device_name": name })).await else { return };
    let origin = dom::location().origin().unwrap_or_default();
    let r2 = app::native_call_async("engineLogin", Some(&json!({ "token": r.g("token"), "server": origin }).to_string())).await;
    // the app answers with the id it signed in under – that is the device to look for
    if let Some(r2) = r2.filter(|r2| r2.truthy("device_id")) {
        let name = if r2.truthy("name") { r2.s("name") } else { name };
        ENGINE.with(|e| *e.borrow_mut() = Some((r2.s("device_id"), name)));
    }
}

/// Run a command another device sent us – once, however many ways it arrived.
fn run_command(c: Value) {
    if !crate::json::truthy(&c) { return; }
    if !c.is_nullish("seq") {
        let seq = c.i("seq");
        let dup = SEEN.with(|s| {
            let mut s = s.borrow_mut();
            if s.contains(&seq) { return true; }
            s.push(seq);
            if s.len() > 200 { s.drain(0..100); }
            false
        });
        if dup { return; }
    }
    if c.s("action") == "play_queue" && c.truthy("from") { toast(&format!("Music sent here from {}", c.s("from")), "ok"); }
    player::command(c);
}

/// The other devices changed (pushed by the server, or fetched).
fn got_devices(list: Vec<Value>) {
    let my = me();
    DEVICES.with(|d| *d.borrow_mut() = list.into_iter().filter(|d| d.s("id") != my).collect());
    // this computer's engine came online: link to it on its own (quietly), unless you unlinked it
    if let Some((eid, _)) = engine_device() {
        if player::linked().is_none() && !engine_off() {
            let d = DEVICES.with(|d| d.borrow().iter().find(|x| x.s("id") == eid).cloned());
            if let Some(d) = d { link_to_opts(d, true); }
        }
    }
    if let Some(link) = player::linked() {
        let d = DEVICES.with(|d| d.borrow().iter().find(|x| x.s("id") == link.id).cloned());
        MISSED.with(|m| m.set(if d.is_some() { 0 } else { m.get() + 1 }));
        if let Some(d) = d { player::link_state(d.g("state").clone(), true); }
        else if MISSED.with(|m| m.get()) > 1 { player::link_state(link.state.clone(), false); }
    }
    let f = ON_DEVICES.with(|o| o.borrow().clone());
    if let Some(f) = f { f(); }
}

async fn beat() {
    let link = player::linked();
    // any failure: offline – try again next time
    let state = match &link { Some(l) => json!({ "playing": false, "remote_of": l.id }), None => player::snapshot() };
    let Ok(r) = api::post("/remote/heartbeat", json!({ "id": me(), "name": device_name(), "state": state })).await else { return };
    for c in r.a("commands") { run_command(c); }
    // with the event stream open the server pushes the others' state; linked, we still ask so a device that went quiet shows as offline
    if link.is_some() || !streaming() {
        if let Ok(list) = api::get("/remote/devices").await { got_devices(list.items()); }
    }
}
/// Check in right away (debounced): something about this device's playback just changed.
fn beat_soon() {
    let t = dom::Timeout::new(150, || dom::spawn(beat()));
    BEAT_TIMER.with(|b| *b.borrow_mut() = Some(t));   // dropping the old one cancels it
}

fn streaming() -> bool { ES.with(|e| e.borrow().as_ref().is_some_and(|es| es.ready_state() == 1)) }
/// Open the server's event stream: our commands as they are sent, every other device's state as it changes.
fn connect() {
    if ES.with(|e| e.borrow().is_some()) || js_sys::Reflect::get(&js_sys::global(), &"EventSource".into()).map(|v| v.is_undefined()).unwrap_or(true) { return; }
    let Ok(es) = web_sys::EventSource::new(&format!("/api/remote/events?device={}", String::from(js_sys::encode_uri_component(&me())))) else { return };
    let data = |e: &web_sys::Event| -> Option<Value> {
        let m = e.dyn_ref::<web_sys::MessageEvent>()?;
        let s = m.data().as_string()?;
        serde_json::from_str::<Value>(&s).ok()
    };
    dom::on_forever(&es, "command", move |e| { if let Some(v) = data(e) { run_command(v.g("command").clone()); } });
    dom::on_forever(&es, "devices", move |e| { if let Some(v) = data(e) { got_devices(v.a("devices")); } });
    dom::on_forever(&es, "error", |_| {
        // the browser reconnects on its own; when it gives up (signed out, server gone) try again later – the heartbeat covers the gap
        let closed = ES.with(|e| e.borrow().as_ref().is_some_and(|es| es.ready_state() == 2));
        if closed {
            ES.with(|e| *e.borrow_mut() = None);
            dom::Timeout::new(30000, connect).forget();
        }
    });
    ES.with(|e| *e.borrow_mut() = Some(es));
}

pub fn start() {
    // the JS computed ME at module load; create the device id as early as we can
    let _ = me();
    if STARTED.with(|s| s.get()) || !app::has_feature("remote") { return; }
    STARTED.with(|s| s.set(true));
    expose_global();
    if let Some(l) = saved_link() {
        let id = l.s("id");
        player::set_link(Some(player::Link { id: id.clone(), name: l.s("name"), state: json!({}), online: true, send: link_sender(id) }));
    }
    dom::spawn(engine_sign_in());
    connect();
    ON_CHANGE.with(|o| *o.borrow_mut() = Some(player::on_change(|_| beat_soon())));
    run_loop();
}

/// `window.encoreRemote = { open, unlink, engine }` – the player bar and the native apps call these.
fn expose_global() {
    let obj = js_sys::Object::new();
    let open_fn = Closure::<dyn Fn()>::new(open);
    let unlink_fn = Closure::<dyn Fn()>::new(unlink);
    let engine_fn = Closure::<dyn Fn() -> JsValue>::new(|| match engine_device() {
        Some((id, name)) => serde_wasm_bindgen::to_value(&json!({ "id": id, "name": name })).unwrap_or(JsValue::NULL),
        None => JsValue::NULL,
    });
    let _ = js_sys::Reflect::set(&obj, &"open".into(), open_fn.as_ref());
    let _ = js_sys::Reflect::set(&obj, &"unlink".into(), unlink_fn.as_ref());
    let _ = js_sys::Reflect::set(&obj, &"engine".into(), engine_fn.as_ref());
    open_fn.forget();
    unlink_fn.forget();
    engine_fn.forget();
    let _ = js_sys::Reflect::set(&dom::window(), &"encoreRemote".into(), &obj);
}

/// `loop()`: beat, then again after 2 s (6 s while the tab is hidden and nothing plays or is linked).
fn run_loop() {
    let busy = player::linked().is_some() || player::snapshot().truthy("playing");
    dom::spawn(async move {
        beat().await;
        let hidden = dom::document().hidden();
        dom::sleep(if hidden && !busy { 6000 } else { 2000 }).await;
        run_loop();
    });
}

// ---- pick up where you left off ------------------------------------------------------------------
// The server keeps your last spot (song, position, queue, and the device that played it). When the app opens
// with nothing playing, this device's own spot comes back paused – or, when another device played since,
// a bar offers to continue that here.
const DISMISSED: &str = "encore.spot.dismissed";
const WEEK: f64 = 7.0 * 24.0 * 3600.0;

/// At start (after the player is mounted): put back where you left off, when Settings → Playback allows it.
pub fn resume() { dom::spawn(resume_async()); }
async fn resume_async() {
    if !crate::playback::current().truthy("resume") || player::current().is_some() || player::linked().is_some() { return; }
    let local = player::saved_spot();
    let spot = if app::has_feature("remote") { api::get("/remote/spot").await.unwrap_or(Value::Null) } else { Value::Null };
    if player::current().is_some() || player::linked().is_some() { return; }
    let at = spot.f("updated_at");
    let other = crate::json::truthy(&spot) && !spot.is_nullish("track_id") && spot.truthy("title") && spot.s("device_id") != me() && dom::now_ms() / 1000.0 - at < WEEK;
    // another device played since this one saved its spot: offer that one (unless you closed this very spot before)
    if other && !local.as_ref().is_some_and(|l| l.f("saved_at") >= at) {
        let gone = dom::ls_get(DISMISSED);
        if gone.as_deref() != Some(spot.s("updated_at").as_str()) { show_spot(spot); return; }
    }
    player::restore(local);
}

fn hide_spot() {
    if let Some(b) = dom::by_id("spot-bar") { dom::remove(&b); }
    let old = SPOT.with(|x| x.borrow_mut().take());
    drop(old);
}

/// "Continue <song> from 2:31 · was playing on <device>" with Play here and ×.
fn show_spot(s: Value) {
    hide_spot();
    let bar = dom::create("div");
    bar.set_id("spot-bar");
    let _ = bar.set_attribute("role", "status");
    let device = if s.truthy("device_name") { s.s("device_name") } else { "another device".to_string() };
    dom::set_html(&bar, &format!(r#"<span>Continue <b>{}</b> from {} · was playing on {}</span>
    <button class="btn sm primary" id="spot-play">Play here</button>
    <button class="btn ghost icon sm" id="spot-close" aria-label="Close" title="Close">{}</button>"#,
        esc(&s.s("title")), fmt::dur(s.f("position")), esc(&device), icon("x")));
    let _ = dom::body().append_child(&bar);
    let mut ls = vec![];
    if let Some(b) = dom::qs_in(&bar, "#spot-play") {
        let s2 = s.clone();
        ls.push(dom::listener(&b, "click", move |_| dom::spawn(play_spot(s2.clone()))));
    }
    if let Some(b) = dom::qs_in(&bar, "#spot-close") {
        let when = s.s("updated_at");
        ls.push(dom::listener(&b, "click", move |_| {
            dom::ls_set(DISMISSED, &when);
            // after this handler returns: hiding drops the listener that is running now
            dom::spawn(async {
                hide_spot();
                // back to this device's own spot, as if the bar had never been offered
                if player::current().is_none() && player::linked().is_none() { player::restore(player::saved_spot()); }
            });
        }));
    }
    // something else started here, or this device became a remote: the offer is gone
    let off = player::on_change(|t| { if t.is_some() || player::linked().is_some() { hide_spot(); } });
    SPOT.with(|x| *x.borrow_mut() = Some((ls, off)));
}

/// Play here: the other device's queue, at its song and position, on this device – and that device pauses.
async fn play_spot(s: Value) {
    let btn = dom::by_id("spot-play");
    if let Some(b) = &btn { dom::set_disabled(b, true); }
    let tid = s.i("track_id");
    let mut ids: Vec<i64> = s.a("queue").iter().filter_map(|v| v.as_i64()).take(300).collect();
    if !ids.contains(&tid) { ids = vec![tid]; }
    let path = format!("/library/tracks?ids={}", ids.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(","));
    let got = match api::get(&path).await {
        Ok(r) => r.items(),
        Err(e) => {
            if let Some(b) = &btn { dom::set_disabled(b, false); }
            toast(&e.message, "err");
            return;
        }
    };
    // back in the queue's own order, repeats included (the server sends each song once)
    let by_id: std::collections::HashMap<i64, Value> = got.into_iter().filter(|t| t.g("id").is_number()).map(|t| (t.i("id"), t)).collect();
    let idx = if s.is_nullish("index") { s.f("idx") } else { s.f("index") };
    let mut tracks: Vec<Value> = vec![];
    let mut index = None;
    for (i, id) in ids.iter().enumerate() {
        let Some(t) = by_id.get(id) else { continue };
        if i as f64 == idx && *id == tid { index = Some(tracks.len()); }
        tracks.push(t.clone());
    }
    let index = index.or_else(|| tracks.iter().position(|t| t.i("id") == tid));
    hide_spot();
    let Some(index) = index else { toast("That song is no longer in your library", "err"); return };
    let mut at = s.f("position");
    // still playing there: it has moved on since its last check-in
    if s.truthy("online") && s.truthy("playing") && (s.is_nullish("device_playing") || s.truthy("device_playing")) {
        at += (dom::now_ms() / 1000.0 - s.f("updated_at")).max(0.0);
        let id = s.s("device_id");
        dom::spawn(async move { let _ = send_to(id, json!({ "action": "pause" })).await; });
    }
    let dur = tracks[index].f("duration");
    if dur != 0.0 && at > dur - 2.0 { at = 0.0; }
    player::command(json!({ "action": "play_queue", "tracks": tracks, "index": index, "position": at }));
}

/// One device's card in the Devices window (the body of the JS `devices.map`).
fn device_card(d: &Value, devices: &[Value], l: Option<&player::Link>, me: &str) -> String {
    let s = d.o("state");
    let name = shown_name(d);
    let is_linked = l.map(|l| l.id == d.s("id")).unwrap_or(false);
    let remote_of_me = s.s("remote_of").as_str() == me;
    let now = if s.truthy("remote_of") {
        let who = if s.s("remote_of").as_str() == me { "this device".to_string() } else {
            devices.iter().find(|x| x.s("id") == s.s("remote_of")).map(|x| x.s("name")).filter(|n| !n.is_empty()).unwrap_or_else(|| "another device".into())
        };
        format!(r#"<div class="small muted">🔗 Remote for {}</div>"#, esc(&who))
    } else if s.truthy("title") {
        let label = s.g("signal").s("label");
        format!(r#"<div class="row" style="gap:12px">
          <div class="thumb" style="width:52px;height:52px">{}</div>
          <div class="grow"><div class="ellipsis" style="font-weight:650">{}</div><div class="small muted ellipsis">{}{}</div>
            <div class="small faint">{} · {} / {}{}</div></div></div>"#,
            art(&s.s("cover"), &if s.truthy("album") { s.s("album") } else { s.s("title") }, ""),
            esc(&s.s("title")), esc(&s.s("artist")), if s.truthy("album") { format!(" · {}", esc(&s.s("album"))) } else { String::new() },
            if s.truthy("playing") { "Playing" } else { "Paused" }, fmt::dur(s.f("position")), fmt::dur(s.f("duration")),
            if label.is_empty() { String::new() } else { format!(" · {}", esc(&label)) })
    } else {
        r#"<div class="small muted">Nothing playing</div>"#.to_string()
    };
    let link_btn = if remote_of_me || s.truthy("remote_of") { String::new() }
        else if is_linked { format!(r#"<button class="btn sm primary" data-a="unlink">{}Linked · Unlink</button>"#, icon("link")) }
        else { format!(r#"<button class="btn sm" data-a="link" title="Lock this device as a remote for {}: everything you play here plays there">{}Link as remote</button>"#, esc(&name), icon("link")) };
    let controls = if s.truthy("remote_of") { String::new() } else {
        let transport = if s.truthy("title") {
            format!(r#"<button class="btn sm ghost icon" data-a="prev" aria-label="Previous">{}</button>
            <button class="btn sm" data-a="toggle">{}</button>
            <button class="btn sm ghost icon" data-a="next" aria-label="Next">{}</button>
            <input type="range" min="0" max="1" step="0.05" value="{}" data-vol title="Volume" style="width:110px;accent-color:var(--accent)">
            {}"#,
                icon("prev"),
                if s.truthy("playing") { format!("{}Pause", icon("pause")) } else { format!("{}Play", icon("play")) },
                icon("next"),
                if s.is_nullish("volume") { "0.9".to_string() } else { s.s("volume") },
                if is_linked { "" } else { r#"<button class="btn sm" data-a="here" title="Continue this song on this device">Play here</button>"# })
        } else { String::new() };
        let send_btn = if player::current().is_some() && l.is_none() {
            format!(r#"<button class="btn sm primary" data-a="send" title="Move what you're playing to {}">Send my music there</button>"#, esc(&name))
        } else { String::new() };
        format!(r#"<div class="row wrap" style="gap:8px">
          {transport}
          {send_btn}
        </div>"#)
    };
    format!(r#"<div class="card stack {}" data-id="{}" style="gap:10px">
        <div class="row between"><b>{}{}</b>
          {link_btn}</div>
        {now}
        {controls}</div>"#,
        if is_linked { "linked" } else { "" }, esc(&d.s("id")),
        ui::icon_attr("devices", r#"width="16" style="vertical-align:-3px;margin-right:6px""#), esc(&name))
}

/// One Roon zone in the Devices window (the JS `zoneCard`).
fn zone_card(z: &Value) -> String {
    let np = z.g("now_playing");
    let vol = z.a("outputs").into_iter().find(|o| o.g("volume").is_object()).map(|o| o.o("volume"));
    let v = vol.map(|vol| { let (min, max) = (vol.f("min"), vol.f("max")); let span = if max - min == 0.0 { 1.0 } else { max - min }; ((vol.f("value") - min) / span).clamp(0.0, 1.0) });
    let state = z.s("state");
    let now = if np.is_object() {
        let img = if np.truthy("image_key") { format!("/api/roon/image/{}?s=104", String::from(js_sys::encode_uri_component(&np.s("image_key")))) } else { String::new() };
        format!(r#"<div class="row" style="gap:12px">
          <div class="thumb" style="width:52px;height:52px">{}</div>
          <div class="grow"><div class="ellipsis" style="font-weight:650">{}</div><div class="small muted ellipsis">{}{}</div>
            <div class="small faint">{}{}</div></div></div>"#,
            art(&img, &np.s("line1"), ""), esc(&np.s("line1")), esc(&np.s("line2")), if np.truthy("line3") { format!(" · {}", esc(&np.s("line3"))) } else { String::new() },
            match state.as_str() { "playing" => "Playing", "paused" => "Paused", "loading" => "Loading", _ => "Stopped" },
            if np.truthy("length") { format!(" · {} / {}", fmt::dur(np.f("seek_position")), fmt::dur(np.f("length"))) } else { String::new() })
    } else { r#"<div class="small muted">Nothing playing</div>"#.to_string() };
    format!(r#"<div class="card stack" data-zone="{}" style="gap:10px">
        <div class="row between"><b>{}{} <span class="badge" style="margin-left:6px">Roon</span></b>
          <button class="btn sm" data-z="search" title="Search Roon's library and play the first match here">Play in Roon…</button></div>
        {now}
        <div class="row wrap" style="gap:8px">
          <button class="btn sm ghost icon" data-z="prev" aria-label="Previous" {}>{}</button>
          <button class="btn sm" data-z="toggle">{}</button>
          <button class="btn sm ghost icon" data-z="next" aria-label="Next" {}>{}</button>
          {}
        </div></div>"#,
        esc(&z.s("zone_id")), ui::icon_attr("devices", r#"width="16" style="vertical-align:-3px;margin-right:6px""#), esc(&z.s("name")),
        if z.truthy("is_previous_allowed") { "" } else { "disabled" }, icon("prev"),
        if state == "playing" { format!("{}Pause", icon("pause")) } else { format!("{}Play", icon("play")) },
        if z.truthy("is_next_allowed") { "" } else { "disabled" }, icon("next"),
        v.map(|v| format!(r#"<input type="range" min="0" max="1" step="0.02" value="{}" data-zvol title="Volume" style="width:110px;accent-color:var(--accent)">"#, v)).unwrap_or_default())
}

/// The Devices window: what every other device is playing, with controls, and Link / Unlink.
pub fn open() {
    let m = ui::modal(ModalOpts::new("Your devices", r#"<div id="dev-me" class="stack" style="margin-bottom:12px"></div><div id="dev-list" class="stack"><div class="skeleton" style="height:90px"></div></div>"#));
    let (Some(list), Some(me_box)) = (m.qs("#dev-list"), m.qs("#dev-me")) else { return };
    let timer: Rc<RefCell<Option<dom::Interval>>> = Rc::new(RefCell::new(None));
    // Roon zones (when Roon is enabled): shown before the Encore devices, refreshed with them
    let roon_on = app::has_feature("roon");
    let zones: Rc<RefCell<Vec<Value>>> = Rc::new(RefCell::new(Vec::new()));

    // paint(): the list from DEVICES (what the server last pushed or we last fetched)
    let paint: Rc<dyn Fn()> = {
        let (list, timer, zones) = (list.clone(), timer.clone(), zones.clone());
        Rc::new(move || {
            if !list.is_connected() { timer.borrow_mut().take(); ON_DEVICES.with(|o| *o.borrow_mut() = None); return; }
            let my = me();
            let devs = DEVICES.with(|d| d.borrow().clone());
            let zs = zones.borrow();
            if devs.is_empty() && zs.is_empty() {
                dom::set_html(&list, &format!(r#"<p class="muted" style="margin:0">No other devices right now. Open Encore on your phone, computer or the apps (signed in as you) and they show up here.{}</p>"#,
                    if roon_on { " Roon zones appear once Encore is connected to your Roon Core (Settings → Apps & sources → Roon)." } else { "" }));
                return;
            }
            let l = player::linked();
            dom::set_html(&list, &format!("{}{}", zs.iter().map(zone_card).collect::<String>(), devs.iter().map(|d| device_card(d, &devs, l.as_ref(), &my)).collect::<String>()));
        })
    };
    let refresh_zones: Rc<dyn Fn()> = {
        let (list, zones, paint) = (list.clone(), zones.clone(), paint.clone());
        Rc::new(move || {
            if !roon_on || !list.is_connected() { return; }
            let (zones, paint) = (zones.clone(), paint.clone());
            dom::spawn(async move {
                // Roon away: keep what we had
                if let Ok(z) = api::get("/roon/zones").await {
                    let items = z.items();
                    if serde_json::to_string(&items).ok() != serde_json::to_string(&*zones.borrow()).ok() { *zones.borrow_mut() = items; paint(); }
                }
            });
        })
    };
    let refresh: Rc<dyn Fn()> = {
        let (list, timer, paint, refresh_zones) = (list.clone(), timer.clone(), paint.clone(), refresh_zones.clone());
        Rc::new(move || {
            let (list, timer, paint, refresh_zones) = (list.clone(), timer.clone(), paint.clone(), refresh_zones.clone());
            dom::spawn(async move {
                if !list.is_connected() { timer.borrow_mut().take(); ON_DEVICES.with(|o| *o.borrow_mut() = None); return; }
                match api::get("/remote/devices").await {
                    Ok(v) => got_devices(v.items()),
                    Err(_) => { DEVICES.with(|d| d.borrow_mut().clear()); paint(); }
                }
                refresh_zones();
            });
        })
    };
    {
        let (zones, refresh_zones) = (zones.clone(), refresh_zones.clone());
        dom::on(&list, "click", move |e| {
            let Some(b) = dom::closest(e, "[data-z]") else { return };
            let id = dom::closest_of(&b, "[data-zone]").map(|c| dom::data(&c, "zone")).unwrap_or_default();
            let name = zones.borrow().iter().find(|x| x.s("zone_id") == id).map(|z| z.s("name"));
            let what = dom::data(&b, "z");
            let refresh_zones = refresh_zones.clone();
            dom::spawn(async move {
                let r = if what == "search" {
                    let Some(q) = dom::prompt(&format!("Play in Roon ({}): artist, album or song", name.clone().unwrap_or_else(|| "zone".into())), "") else { return };
                    if q.trim().is_empty() { return; }
                    match api::post(&format!("/roon/zones/{}/play", String::from(js_sys::encode_uri_component(&id))), json!({ "query": q.trim() })).await {
                        Ok(r) => { toast(&format!("Playing {} in {}", r.s("playing"), name.unwrap_or_else(|| "Roon".into())), "ok"); Ok(()) }
                        Err(e) => Err(e),
                    }
                } else {
                    api::post(&format!("/roon/zones/{}/command", String::from(js_sys::encode_uri_component(&id))), json!({ "action": what })).await.map(|_| ())
                };
                match r {
                    Ok(()) => dom::timeout(500, move || refresh_zones()),
                    Err(e) => toast(&e.message, "err"),
                }
            });
        });
        dom::on(&list, "change", move |e| {
            let Some(v) = dom::closest(e, "[data-zvol]") else { return };
            let id = dom::closest_of(&v, "[data-zone]").map(|c| dom::data(&c, "zone")).unwrap_or_default();
            let n: f64 = dom::value(&v).trim().parse().unwrap_or(0.0);
            dom::spawn(async move {
                if let Err(e) = api::post(&format!("/roon/zones/{}/command", String::from(js_sys::encode_uri_component(&id))), json!({ "action": "volume", "value": n })).await { toast(&e.message, "err"); }
            });
        });
    }

    let send: Rc<dyn Fn(String, Value)> = {
        let refresh = refresh.clone();
        Rc::new(move |id, body| {
            let refresh = refresh.clone();
            dom::spawn(async move {
                match send_to(id, body).await {
                    Ok(_) => { if !streaming() { dom::timeout(400, move || refresh()); } }
                    Err(e) => toast(&e.message, "err"),
                }
            });
        })
    };

    // paintMe calls itself from its own buttons
    type PaintMe = Rc<RefCell<Option<Rc<dyn Fn()>>>>;
    let paint_me_cell: PaintMe = Rc::new(RefCell::new(None));
    let paint_me: Rc<dyn Fn()> = {
        let (me_box, paint, cell) = (me_box.clone(), paint.clone(), paint_me_cell.clone());
        Rc::new(move || {
            let l = player::linked();
            dom::set_html(&me_box, &format!(r##"<div class="row wrap between" style="gap:8px"><div class="small muted">This device: <b style="color:var(--text)">{}</b> <a href="#" id="dev-rename" class="small">rename</a></div>
      {}</div>
      {}"##,
                esc(&device_name()),
                l.as_ref().map(|l| format!(r#"<button class="btn sm" id="dev-unlink">{}Unlink from {}</button>"#, icon("link"), esc(&l.name))).unwrap_or_default(),
                l.as_ref().map(|l| format!(r#"<div class="small muted">🔗 Linked: everything you play here goes to <b>{}</b>. Unlink to listen on this device.</div>"#, esc(&l.name))).unwrap_or_default()));
            let again = { let cell = cell.clone(); move || { let f = cell.borrow().clone(); if let Some(f) = f { f() } } };
            if let Some(b) = dom::qs_in(&me_box, "#dev-unlink") {
                let (again, paint) = (again.clone(), paint.clone());
                dom::on(&b, "click", move |_| { unlink(); again(); paint(); });
            }
            if let Some(a) = dom::qs_in(&me_box, "#dev-rename") {
                dom::on(&a, "click", move |e| {
                    dom::prevent(e);
                    let Some(n) = dom::prompt("Name this device (shown on your other devices)", &device_name()) else { return };
                    let t = js_sys::JsString::from(n.as_str()).trim();
                    if t.length() > 0 { dom::ls_set(NAME_KEY, &String::from(t.slice(0, 60))); } else { dom::ls_remove(NAME_KEY); }
                    again();
                    dom::spawn(beat());
                });
            }
        })
    };
    *paint_me_cell.borrow_mut() = Some(paint_me.clone());

    {
        let (send, paint_me, paint, m) = (send.clone(), paint_me.clone(), paint.clone(), m.clone());
        dom::on(&list, "click", move |e| {
            let Some(b) = dom::closest(e, "[data-a]") else { return };
            let id = dom::closest_of(&b, "[data-id]").map(|c| dom::data(&c, "id")).unwrap_or_default();
            let d = DEVICES.with(|d| d.borrow().iter().find(|d| d.s("id") == id).cloned());
            let Some(d) = d else { return };
            let a = dom::data(&b, "a");
            match a.as_str() {
                "link" => { link_to(d); paint_me(); paint(); }
                "unlink" => { unlink(); paint_me(); paint(); }
                "here" => {
                    let s = d.o("state");
                    let queue = s.a("queue");
                    if queue.is_empty() { return; }
                    if player::linked().is_some() { unlink(); }
                    player::play_queue(queue, s.i("index").max(0) as usize, "list");
                    let at = s.f("position");
                    dom::timeout(600, move || player::seek_to(at));
                    send(d.s("id"), json!({ "action": "pause" }));
                    m.close();
                }
                "send" => {
                    let snap = player::snapshot();
                    let queue = snap.a("queue");
                    if queue.is_empty() { toast("Only songs from your library can be sent", "err"); return; }
                    let index = queue.iter().position(|t| t.g("id") == snap.g("id")).map(|i| i as i64).unwrap_or(-1);
                    send(d.s("id"), json!({ "action": "play_queue", "tracks": queue, "index": index, "position": snap.g("position") }));
                    player::command(json!({ "action": "pause" }));
                }
                _ => send(d.s("id"), json!({ "action": a })),
            }
        });
    }
    {
        let send = send.clone();
        dom::on(&list, "change", move |e| {
            let Some(v) = dom::closest(e, "[data-vol]") else { return };
            let id = dom::closest_of(&v, "[data-id]").map(|c| dom::data(&c, "id")).unwrap_or_default();
            let n: f64 = dom::value(&v).trim().parse().unwrap_or(0.0);
            let value = if n.fract() == 0.0 { json!(n as i64) } else { json!(n) };
            send(id, json!({ "action": "volume", "value": value }));
        });
    }
    ON_DEVICES.with(|o| *o.borrow_mut() = Some(paint.clone()));
    paint_me();
    refresh();
    // pushed updates repaint the list; this is the fallback when the stream is down (Roon zones are always polled)
    let (r, rz) = (refresh.clone(), refresh_zones.clone());
    *timer.borrow_mut() = Some(dom::Interval::new(2000, move || { if !streaming() { r(); } else { rz(); } }));
}
