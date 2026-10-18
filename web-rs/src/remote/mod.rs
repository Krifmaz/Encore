//! Port of web/js/remote.js – see docs/archive/ui-rewrite/CONTRACT.md
//!
//! Use any signed-in device as a remote for the others: every open app checks in every couple of
//! seconds with what it's playing and runs the commands other devices sent it. With the server's event stream
//! open, commands and the other devices' state arrive the moment they change instead of at the next check-in,
//! and this device checks in right away whenever its own playback changes – so a remote feels instant.
//!
//! Split by concern: link.rs (this device locked as a remote, and this computer's built-in engine), spot.rs
//! (pick up where you left off) and devices.rs (the Devices window). This file holds the device's identity,
//! the heartbeat and the event stream.

mod devices;
mod link;
mod spot;

pub use devices::open;
pub use link::{engine_device, engine_off, is_engine, link_to, shown_name, unlink, ENGINE_LABEL};
pub use spot::resume;

use link::*;
use spot::*;

use crate::dom;
use crate::json::J;
use crate::num::{num, rand36, to36};
use crate::ui::{self, art, esc, fmt, icon, toast, ModalOpts};
use crate::{api, app, native, player};
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};

const KEY: &str = "encore.device";
const NAME_KEY: &str = "encore.device.name";

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
}

fn me() -> String { ME.with(|m| m.clone()) }

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

/// A readable name: "Android app", "Chrome on Windows", "Safari on iPhone" … (or the one you gave it)
fn device_name() -> String {
    if let Some(own) = dom::ls_get(NAME_KEY).filter(|s| !s.is_empty()) { return own; }
    let ua = dom::window().navigator().user_agent().unwrap_or_default();
    if ua.contains("EncoreAndroid") || native::platform().as_deref() == Some("android") { return "Android app".into(); }
    if ua.contains("EncoreDesktop") || ua.contains("Electron") || native::platform().as_deref() == Some("desktop") {
        return format!("Desktop app ({})", if ua.contains("Mac") { "Mac" } else if ua.contains("Windows") { "Windows" } else { "Linux" });
    }
    let os = if ua.contains("iPhone") { "iPhone" } else if ua.contains("iPad") { "iPad" } else if ua.contains("Android") { "Android" }
        else if ua.contains("Mac") { "Mac" } else if ua.contains("Windows") { "Windows" } else if ua.contains("Linux") { "Linux" } else { "device" };
    let br = if ua.contains("Edg/") { "Edge" } else if ua.contains("Firefox/") { "Firefox" } else if ua.contains("Chrome/") { "Chrome" }
        else if ua.contains("Safari/") { "Safari" } else { "Browser" };
    format!("{br} on {os}")
}

/// `api.post(`/remote/${id}/command`, { ...body, from: deviceName() })`.
async fn send_to(id: String, body: Value) -> api::ApiResult {
    let mut body = if body.is_object() { body } else { json!({}) };
    body["from"] = Value::String(device_name());
    api::post(&format!("/remote/{}/command", String::from(js_sys::encode_uri_component(&id))), body).await
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
    // the built-in engine is no longer a device to link to (the server keeps it out of the list; 0.27 makes
    // it a Playback switch), so nothing links to it on its own any more
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
