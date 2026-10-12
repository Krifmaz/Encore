//! Port of web/js/remote.js – see docs/ui-rewrite/CONTRACT.md
//!
//! Use any signed-in device as a remote for the others (Encore 2): every open app checks in every couple of
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
use wasm_bindgen::JsCast;

const KEY: &str = "encore.device";
const NAME_KEY: &str = "encore.device.name";
// Linked: this device is locked as a remote for another one until you unlink.
const CHAIN: &str = "encore.link";

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
    if ua.contains("EncoreAndroid") || app::native_has("saveOffline") { return "Android app".into(); }
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

pub fn link_to(d: Value) {
    dom::ls_set(CHAIN, &json!({ "id": d.g("id"), "name": d.g("name") }).to_string());
    let id = d.s("id");
    player::set_link(Some(player::Link { id: id.clone(), name: d.s("name"), state: d.o("state"), online: true, send: link_sender(id) }));
    toast(&format!("Linked – this device now controls {}", d.s("name")), "ok");
    dom::spawn(beat());
}

pub fn unlink() {
    dom::ls_remove(CHAIN);
    player::set_link(None);
    toast("Unlinked – music plays on this device again", "ok");
    dom::spawn(beat());
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
    connect();
    ON_CHANGE.with(|o| *o.borrow_mut() = Some(player::on_change(|_| beat_soon())));
    run_loop();
}

/// `window.encoreRemote = { open, unlink }` – the player bar and the native apps call these.
fn expose_global() {
    let obj = js_sys::Object::new();
    let open_fn = Closure::<dyn Fn()>::new(open);
    let unlink_fn = Closure::<dyn Fn()>::new(unlink);
    let _ = js_sys::Reflect::set(&obj, &"open".into(), open_fn.as_ref());
    let _ = js_sys::Reflect::set(&obj, &"unlink".into(), unlink_fn.as_ref());
    open_fn.forget();
    unlink_fn.forget();
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

/// One device's card in the Devices window (the body of the JS `devices.map`).
fn device_card(d: &Value, devices: &[Value], l: Option<&player::Link>, me: &str) -> String {
    let s = d.o("state");
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
        else { format!(r#"<button class="btn sm" data-a="link" title="Lock this device as a remote for {}: everything you play here plays there">{}Link as remote</button>"#, esc(&d.s("name")), icon("link")) };
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
            format!(r#"<button class="btn sm primary" data-a="send" title="Move what you're playing to {}">Send my music there</button>"#, esc(&d.s("name")))
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
        ui::icon_attr("devices", r#"width="16" style="vertical-align:-3px;margin-right:6px""#), esc(&d.s("name")))
}

/// The Devices window: what every other device is playing, with controls, and Link / Unlink.
pub fn open() {
    let m = ui::modal(ModalOpts::new("Your devices", r#"<div id="dev-me" class="stack" style="margin-bottom:12px"></div><div id="dev-list" class="stack"><div class="skeleton" style="height:90px"></div></div>"#));
    let (Some(list), Some(me_box)) = (m.qs("#dev-list"), m.qs("#dev-me")) else { return };
    let timer: Rc<RefCell<Option<dom::Interval>>> = Rc::new(RefCell::new(None));

    // paint(): the list from DEVICES (what the server last pushed or we last fetched)
    let paint: Rc<dyn Fn()> = {
        let (list, timer) = (list.clone(), timer.clone());
        Rc::new(move || {
            if !list.is_connected() { timer.borrow_mut().take(); ON_DEVICES.with(|o| *o.borrow_mut() = None); return; }
            let my = me();
            let devs = DEVICES.with(|d| d.borrow().clone());
            if devs.is_empty() {
                dom::set_html(&list, r#"<p class="muted" style="margin:0">No other devices right now. Open Encore on your phone, computer or the apps (signed in as you) and they show up here.</p>"#);
                return;
            }
            let l = player::linked();
            dom::set_html(&list, &devs.iter().map(|d| device_card(d, &devs, l.as_ref(), &my)).collect::<String>());
        })
    };
    let refresh: Rc<dyn Fn()> = {
        let (list, timer, paint) = (list.clone(), timer.clone(), paint.clone());
        Rc::new(move || {
            let (list, timer, paint) = (list.clone(), timer.clone(), paint.clone());
            dom::spawn(async move {
                if !list.is_connected() { timer.borrow_mut().take(); ON_DEVICES.with(|o| *o.borrow_mut() = None); return; }
                match api::get("/remote/devices").await {
                    Ok(v) => got_devices(v.items()),
                    Err(_) => { DEVICES.with(|d| d.borrow_mut().clear()); paint(); }
                }
            });
        })
    };

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
    // pushed updates repaint the list; this is the fallback when the stream is down
    let r = refresh.clone();
    *timer.borrow_mut() = Some(dom::Interval::new(2000, move || { if !streaming() { r(); } }));
}
