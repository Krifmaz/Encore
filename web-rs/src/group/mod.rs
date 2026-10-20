//! Group listen (0.31, p6 item 8): several people on this server hear the same queue at the same time, each on
//! their own device. This module is the web app's side of it for accounts: the group this browser is in (its
//! state, from the server's answers and its event stream), the server's clock, the live group listens on the
//! server, and every action – start, join by code or from the list, leave, end, the host's settings and the
//! playback commands. The player follows the state (player/group.rs); ui.rs draws the dialogs, the Home row and
//! the devices panel's section; guest.rs is the page for people without an account.
//!
//! The endpoints and JSON shapes are docs/contracts/client-shapes-0.31-0.33.md §2.2, §2.3 and §3.6–3.19; the words
//! are its §5.7, the same on the phone and the desktop app.

pub mod guest;
pub mod rules;
pub mod ui;

use crate::api::{self, ApiError};
use crate::dom::{self, EventListener};
use crate::json::J;
use crate::ui::toast;
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use wasm_bindgen::JsCast;

/// This browser's group, kept for a reload of the page (the group comes back "Paused here").
const KEEP_ID: &str = "encore.group.id";
/// A group link opened while signed out: asked again once signed in (sessionStorage).
pub(crate) const PENDING: &str = "encore.group.join";
/// The clock is measured again this often while in a group (client-shapes §5.6).
const CLOCK_EVERY_MS: u32 = 600_000;
/// A server without group listens (before 0.31, or turned off) is asked again after this long.
const NO_GROUPS_ASK_MS: f64 = 600_000.0;

type Listener = Rc<dyn Fn()>;

thread_local! {
    static STATE: RefCell<Option<Value>> = const { RefCell::new(None) };
    /// this member paused here only ("Paused here · Catch up"); the group plays on
    static LOCAL_PAUSED: Cell<bool> = const { Cell::new(false) };
    /// the server's clock minus `performance.now()`, ms (None: not measured, the wall clock stands in)
    static OFFSET: Cell<Option<f64>> = const { Cell::new(None) };
    /// the server process the group was last seen on: another one after a restart
    static BOOT: RefCell<String> = const { RefCell::new(String::new()) };
    /// the group's event stream and its listeners
    static ES: RefCell<Option<(String, web_sys::EventSource, Vec<EventListener>)>> = const { RefCell::new(None) };
    static CLOCK_TIMER: RefCell<Option<dom::Interval>> = const { RefCell::new(None) };
    static RETRY: RefCell<Option<dom::Timeout>> = const { RefCell::new(None) };
    static BACKOFF: Cell<u32> = const { Cell::new(1000) };
    static LISTENERS: RefCell<Vec<(u64, Listener)>> = const { RefCell::new(Vec::new()) };
    static LISTENER_N: Cell<u64> = const { Cell::new(0) };
    /// the live group listens this account may see (GET /api/group)
    static LIVE: RefCell<Vec<Value>> = const { RefCell::new(Vec::new()) };
    /// the server has group listens (None: not asked yet)
    static AVAILABLE: Cell<Option<bool>> = const { Cell::new(None) };
    static NO_GROUPS_AT: Cell<f64> = const { Cell::new(0.0) };
    /// a start or join is on its way (the dialogs' buttons wait)
    static BUSY: Cell<bool> = const { Cell::new(false) };
    static STARTED: Cell<bool> = const { Cell::new(false) };
}

// ---- what the rest of the app reads ----------------------------------------------------------------------------

/// The group this browser is in, as the server last told it.
pub fn current() -> Option<Value> { STATE.with(|s| s.borrow().clone()) }
pub fn in_group() -> bool { STATE.with(|s| s.borrow().is_some()) }
/// This device may play, pause, seek and change the queue (the host, or "Everyone can control").
pub fn can_control() -> bool { STATE.with(|s| s.borrow().as_ref().is_some_and(|s| s.g("you").truthy("can_control"))) }
pub fn local_paused() -> bool { LOCAL_PAUSED.with(|p| p.get()) }
pub fn busy() -> bool { BUSY.with(|b| b.get()) }
/// The server has group listens (false: turned off, or a server before 0.31; None: not asked yet).
pub fn available() -> Option<bool> { AVAILABLE.with(|a| a.get()) }
/// The live group listens this account may see ("Listening together now", the devices panel).
pub fn live() -> Vec<Value> { LIVE.with(|l| l.borrow().clone()) }

/// "Only Alex controls playback" while in a group.
pub fn only_host() -> String { current().map(|s| rules::only_host(&s)).unwrap_or_default() }

/// `performance.now()`: a clock no change of the computer's time moves.
fn mono_ms() -> f64 { dom::window().performance().map(|p| p.now()).unwrap_or_else(dom::now_ms) }

/// The server's clock now (Unix ms).
pub fn server_now() -> f64 { OFFSET.with(|o| o.get()).map(|o| mono_ms() + o).unwrap_or_else(dom::now_ms) }

/// This device's id and name, as the remote knows them (`/api/remote/*`).
fn device() -> (String, String) { crate::remote::device() }

/// A body for a group route: `{device_id, device_name, …extra}`.
fn body(extra: Value) -> Value {
    let (id, name) = device();
    let mut b = json!({ "device_id": id, "device_name": name });
    if let (Some(o), Some(e)) = (b.as_object_mut(), extra.as_object()) { for (k, v) in e { o.insert(k.clone(), v.clone()); } }
    b
}

fn enc(s: &str) -> String { String::from(js_sys::encode_uri_component(s)) }

// ---- listeners: the dialogs, the Home row, the devices panel and the player follow every change -----------------

/// `onChange(fn)` for the group: dropping the handle unsubscribes.
pub struct OnGroup(u64);
impl Drop for OnGroup {
    fn drop(&mut self) { let id = self.0; LISTENERS.with(|l| l.borrow_mut().retain(|(i, _)| *i != id)); }
}
pub fn on_change(f: impl Fn() + 'static) -> OnGroup {
    let id = LISTENER_N.with(|n| { n.set(n.get() + 1); n.get() });
    LISTENERS.with(|l| l.borrow_mut().push((id, Rc::new(f))));
    OnGroup(id)
}
fn notify() {
    crate::player::group_changed();
    let fns: Vec<Listener> = LISTENERS.with(|l| l.borrow().iter().map(|(_, f)| f.clone()).collect());
    for f in fns { f(); }
}

// ---- start of the app ------------------------------------------------------------------------------------------

/// Signed in: back into the group this browser was in (paused here: the listener chooses to hear it), a group link
/// opened before signing in, and whether the server has group listens at all.
pub fn start() {
    if STARTED.with(|s| s.replace(true)) { return; }
    dom::spawn(async {
        refresh_list().await;
        if let Some(id) = dom::ls_get(KEEP_ID).filter(|s| !s.is_empty()) {
            let (dev, _) = device();
            match api::get(&format!("/group/current?device_id={}", enc(&dev))).await {
                Ok(s) if s.s("id") == id => {
                    clock().await;
                    LOCAL_PAUSED.with(|p| p.set(true));
                    adopt(s);
                }
                Ok(_) => dom::ls_remove(KEEP_ID),
                Err(e) if matches!(e.status, 403 | 404) => dom::ls_remove(KEEP_ID),
                Err(_) => {}
            }
        }
        let pending = dom::window().session_storage().ok().flatten().and_then(|s| { let v = s.get_item(PENDING).ok().flatten(); let _ = s.remove_item(PENDING); v });
        if let Some(code) = pending.filter(|c| !c.is_empty()) { ui::open_invite(&code); }
    });
}

// ---- the server's list ------------------------------------------------------------------------------------------

/// GET /api/group now: the list and whether the server has group listens. Quiet on any error; a server without
/// them is asked again only every 10 minutes.
pub async fn refresh_list() {
    if crate::app::user().is_none() { return; }
    if available() == Some(false) && dom::now_ms() - NO_GROUPS_AT.with(|n| n.get()) < NO_GROUPS_ASK_MS { return; }
    match api::get("/group").await {
        Ok(v) => {
            let list = if v.is_array() { v.items() } else { v.a("groups") };
            LIVE.with(|l| *l.borrow_mut() = list);
            AVAILABLE.with(|a| a.set(Some(true)));
        }
        Err(e) if matches!(e.status, 403..=405) => {
            LIVE.with(|l| l.borrow_mut().clear());
            AVAILABLE.with(|a| a.set(Some(false)));
            NO_GROUPS_AT.with(|n| n.set(dom::now_ms()));
        }
        Err(_) => return,
    }
    let fns: Vec<Listener> = LISTENERS.with(|l| l.borrow().iter().map(|(_, f)| f.clone()).collect());
    for f in fns { f(); }
}

// ---- start and join -----------------------------------------------------------------------------------------------

/// Start a group listen: what plays here (library songs only, at most 1000 around the one playing) becomes its queue.
pub async fn start_group(visibility: &str, everyone_controls: bool) -> Result<(), String> {
    if BUSY.with(|b| b.replace(true)) { return Err(String::new()); }
    let (queue, index) = crate::player::queue();
    let snap = crate::player::snapshot();
    let mut ids: Vec<i64> = vec![];
    let mut at = 0usize;
    for (i, t) in queue.iter().enumerate() {
        if !t.g("id").is_number() || t.truthy("preview") { continue; }
        if i < index { at += 1; }
        ids.push(t.i("id"));
    }
    let (win, at) = rules::window(&ids, at, 1000);
    let playing = snap.truthy("playing") && !win.is_empty();
    let position = if win.is_empty() { 0.0 } else { snap.f("position") };
    clock().await;
    let r = api::post("/group", body(json!({ "visibility": visibility, "everyone_controls": everyone_controls, "tracks": win, "index": at, "position": position, "playing": playing }))).await;
    BUSY.with(|b| b.set(false));
    match r {
        Ok(s) if s.truthy("id") => {
            leave_quietly_if_other(&s);
            crate::remote::unlink_quiet();
            LOCAL_PAUSED.with(|p| p.set(false));
            adopt(s);
            Ok(())
        }
        Ok(_) => Err("Couldn't start the group listen".into()),
        Err(e) => Err(if e.message.is_empty() { "Couldn't start the group listen".into() } else { e.message }),
    }
}

/// Look up who hosts the group behind a code (GET /api/group/code/{code}).
pub async fn lookup(code: &str) -> Result<Value, ApiError> { api::get(&format!("/group/code/{}", enc(code))).await }

/// Join by code (the answer to "Join <host>'s group listen?").
pub async fn join_code(code: &str) -> Result<(), String> { join(api::post("/group/join", body(json!({ "code": code })))).await }

/// Join an "anyone" group from the server's list.
pub async fn join_id(id: &str) -> Result<(), String> {
    if current().is_some_and(|s| s.s("id") == id) { return Ok(()); }
    join(api::post(&format!("/group/{}/join", enc(id)), body(json!({})))).await
}

async fn join(call: impl std::future::Future<Output = api::ApiResult>) -> Result<(), String> {
    if BUSY.with(|b| b.replace(true)) { return Err(String::new()); }
    clock().await;
    let r = call.await;
    BUSY.with(|b| b.set(false));
    match r {
        Ok(s) if s.truthy("id") => {
            leave_quietly_if_other(&s);
            crate::remote::unlink_quiet();
            LOCAL_PAUSED.with(|p| p.set(false));
            toast(&if s.g("you").truthy("host") { "You joined your group listen".to_string() } else { format!("You joined {}'s group listen", rules::host_name(&s)) }, "ok");
            adopt(s);
            Ok(())
        }
        Ok(_) => Err("Couldn't join".into()),
        Err(e) => Err(match e.status {
            404 if e.message.is_empty() || e.message.starts_with("HTTP") => "No group listen with that code".into(),
            _ if e.message.is_empty() => "Couldn't join".into(),
            _ => e.message,
        }),
    }
}

/// Starting or joining another group took this device out of the one it was in (client-shapes §2.2): nothing to
/// tell the server, only the old stream to close.
fn leave_quietly_if_other(s: &Value) {
    if current().is_some_and(|old| old.s("id") != s.s("id")) { close_events(); STATE.with(|x| *x.borrow_mut() = None); }
}

// ---- leave, end, settings -------------------------------------------------------------------------------------------

/// Leave (the host leaving hands the group to the one who has been in it longest).
pub fn leave() {
    let Some(s) = current() else { return };
    stop(Some("You left the group listen".into()));
    let id = s.s("id");
    dom::spawn(async move { let _ = api::post(&format!("/group/{}/leave", enc(&id)), body(json!({}))).await; });
}

/// "End for everyone" (the host).
pub fn end() {
    let Some(s) = current() else { return };
    let id = s.s("id");
    dom::spawn(async move {
        match api::post(&format!("/group/{}/end", enc(&id)), body(json!({}))).await {
            // the event stream's `ended` may have come first and said it already
            Ok(_) => { if current().is_some_and(|s| s.s("id") == id) { stop(Some("You ended the group listen".into())); } }
            Err(e) => toast(if e.message.is_empty() { "Couldn't end the group listen" } else { &e.message }, "err"),
        }
    });
}

/// The host's settings: any of everyone_controls, visibility, guests_allowed, max_members.
pub fn patch(fields: Value) {
    let Some(s) = current() else { return };
    let id = s.s("id");
    dom::spawn(async move {
        match api::patch(&format!("/group/{}", enc(&id)), body(fields)).await {
            Ok(s) => apply(s),
            Err(e) => {
                toast(if e.message.is_empty() { "Couldn't change the group listen" } else { &e.message }, "err");
                // the dialog is drawn again as the group is (the switch the host moved goes back)
                if let Some(el) = dom::by_id("g-sheet") { let _ = el.remove_attribute("data-drawn"); }
                notify();
            }
        }
    });
}

/// "Remove from the group" (guests only; the host or an admin).
pub fn remove_member(member: &str, name: &str) {
    let Some(s) = current() else { return };
    let (id, member, name) = (s.s("id"), member.to_string(), name.to_string());
    dom::spawn(async move {
        let (dev, _) = device();
        match api::del(&format!("/group/{}/members/{}?device_id={}", enc(&id), enc(&member), enc(&dev))).await {
            Ok(s) if s.truthy("id") => apply(s),
            Ok(_) => {}
            Err(e) => toast(&if e.message.is_empty() { format!("Couldn't remove {name}") } else { e.message }, "err"),
        }
    });
}

// ---- playback -------------------------------------------------------------------------------------------------------

/// POST /api/group/{id}/command {op, …}: a refusal shows the server's reason and the player follows the group again.
pub fn command(op: &str, fields: Value) {
    let Some(s) = current() else { return };
    let id = s.s("id");
    let mut b = body(fields);
    b["op"] = json!(op);
    dom::spawn(async move {
        match api::post(&format!("/group/{}/command", enc(&id)), b).await {
            Ok(s) if s.truthy("id") => apply(s),
            Ok(_) => {}
            Err(e) => {
                toast(if e.message.is_empty() { "Couldn't change the group listen" } else { &e.message }, "err");
                crate::player::group_resync();
            }
        }
    });
}

/// This member paused here only (or plays again). The group plays on.
pub fn set_local_paused(on: bool) {
    if LOCAL_PAUSED.with(|p| p.replace(on)) == on { return; }
    notify();
}

/// "Catch up": play again where the group is now.
pub fn catch_up() {
    LOCAL_PAUSED.with(|p| p.set(false));
    crate::player::group_resync();
    notify();
}

// ---- state ---------------------------------------------------------------------------------------------------------

/// A state of the group this browser is in (an answer or the event stream), when it is not older than the one held.
fn apply(s: Value) {
    let Some(old) = current() else { return };
    // an answer that comes after leaving (or about another group) changes nothing: only start and join adopt a group
    if old.s("id") != s.s("id") || s.i("rev") < old.i("rev") { return; }
    for t in rules::comings(&old, &s) { toast(&t, ""); }
    if s.truthy("boot") { BOOT.with(|b| *b.borrow_mut() = s.s("boot")); }
    STATE.with(|x| *x.borrow_mut() = Some(s));
    notify();
}

/// Now in `s`: remembered for a reload, its event stream opens and the clock is kept.
fn adopt(s: Value) {
    if s.truthy("boot") { BOOT.with(|b| *b.borrow_mut() = s.s("boot")); }
    let id = s.s("id");
    dom::ls_set(KEEP_ID, &id);
    STATE.with(|x| *x.borrow_mut() = Some(s));
    open_events(&id);
    keep_clock();
    notify();
}

/// Out of the group (left, ended, removed); `message` as a toast.
fn stop(message: Option<String>) {
    close_events();
    CLOCK_TIMER.with(|c| c.borrow_mut().take());
    RETRY.with(|r| r.borrow_mut().take());
    let was = STATE.with(|x| x.borrow_mut().take()).is_some();
    LOCAL_PAUSED.with(|p| p.set(false));
    dom::ls_remove(KEEP_ID);
    if let Some(m) = message { toast(&m, ""); }
    if was { notify(); }
    dom::spawn(refresh_list());
}

// ---- the server's clock ----------------------------------------------------------------------------------------------

/// Three round trips to GET /api/time; the one with the smallest round trip sets the offset (the guest page too).
pub(crate) async fn clock() {
    let mut samples = vec![];
    for _ in 0..3 {
        let t0 = mono_ms();
        match api::get("/time").await {
            Ok(v) if v.f("now_ms") > 0.0 => {
                samples.push((t0, v.f("now_ms"), mono_ms()));
                if v.truthy("boot") && BOOT.with(|b| b.borrow().is_empty()) { BOOT.with(|b| *b.borrow_mut() = v.s("boot")); }
            }
            // a server without /api/time: its clock is taken as this device's
            Ok(_) | Err(ApiError { status: 400..=499, .. }) => break,
            Err(_) => {}
        }
    }
    if let Some(o) = rules::clock_offset(&samples) { OFFSET.with(|x| x.set(Some(o))); }
}

/// The computer's clock may move under a running group: measured again every 10 minutes while in one.
fn keep_clock() {
    let t = dom::Interval::new(CLOCK_EVERY_MS, || { if in_group() { dom::spawn(clock()); } });
    CLOCK_TIMER.with(|c| *c.borrow_mut() = Some(t));
}

// ---- the event stream --------------------------------------------------------------------------------------------------

fn close_events() {
    if let Some((_, es, ls)) = ES.with(|e| e.borrow_mut().take()) { es.close(); drop(ls); }
}

fn open_events(id: &str) {
    if ES.with(|e| e.borrow().as_ref().is_some_and(|(i, es, _)| i == id && es.ready_state() != 2)) { return; }
    close_events();
    let (dev, _) = device();
    let Ok(es) = web_sys::EventSource::new(&format!("/api/group/{}/events?device_id={}", enc(id), enc(&dev))) else { return };
    let data = |e: &web_sys::Event| -> Option<Value> {
        let m = e.dyn_ref::<web_sys::MessageEvent>()?;
        serde_json::from_str::<Value>(&m.data().as_string()?).ok()
    };
    let mut ls = vec![];
    let gid = id.to_string();
    ls.push(dom::listener(&es, "state", move |e| {
        BACKOFF.with(|b| b.set(1000));
        if let Some(v) = data(e) { let v = if v.g("state").is_object() { v.g("state").clone() } else { v }; if v.s("id") == gid { apply(v); } }
    }));
    let gid = id.to_string();
    ls.push(dom::listener(&es, "ended", move |e| {
        if current().is_none_or(|s| s.s("id") != gid) { return; }
        let reason = data(e).map(|v| v.s("reason")).unwrap_or_default();
        let s = current().unwrap_or(Value::Null);
        stop(Some(rules::ended_text(&reason, &rules::host_name(&s), s.g("you").truthy("host"))));
    }));
    let gid = id.to_string();
    ls.push(dom::listener(&es, "error", move |e| {
        // the browser tries again by itself after a dropped connection; a stream it gave up on (the server answered
        // 404: this device was dropped, or the group is gone) is looked into here
        let closed = e.target().and_then(|t| t.dyn_into::<web_sys::EventSource>().ok()).is_some_and(|es| es.ready_state() == 2);
        if closed && current().is_some_and(|s| s.s("id") == gid) {
            let gid = gid.clone();
            let wait = BACKOFF.with(|b| { let w = b.get(); b.set((w * 2).min(15000)); w });
            let t = dom::Timeout::new(wait, move || dom::spawn(gone(gid)));
            RETRY.with(|r| *r.borrow_mut() = Some(t));
        }
    }));
    ES.with(|e| *e.borrow_mut() = Some((id.to_string(), es, ls)));
}

/// The event stream was refused: this device was dropped (offline over 2 minutes) or the group is gone. Back in
/// with the code when it is still there (client-shapes §3.17); else out: ended (after a server restart, said so),
/// or the join's own reason when the group is there but this device can't get back in (full, too many tries, off).
async fn gone(id: String) {
    let Some(s) = current().filter(|s| s.s("id") == id) else { return };
    let (dev, _) = device();
    match api::get(&format!("/group/current?device_id={}", enc(&dev))).await {
        Ok(v) if v.s("id") == id => { apply(v); open_events(&id); return; }
        Ok(_) => {}
        Err(e) if e.status == 403 => { stop(Some(rules::rejoin_refused(403, &e.message))); return; }
        // the server can't be asked now: the stream is tried again, and with it the join
        Err(_) => { retry_later(id); return; }
    }
    let back = match api::post("/group/join", body(json!({ "code": s.s("code") }))).await {
        Ok(v) => Some(v),
        Err(e) if e.status == 404 => None,
        Err(e) if e.status == 0 || e.status >= 500 => { retry_later(id); return; }
        Err(e) => { stop(Some(rules::rejoin_refused(e.status, &e.message))); return; }
    };
    if let Some(v) = back {
        if v.s("id") == id {
            STATE.with(|x| *x.borrow_mut() = None);
            adopt(v);
            return;
        }
        // the code belongs to another group now (this one ended and the code was given out again): not that one
        let other = v.s("id");
        let _ = api::post(&format!("/group/{}/leave", enc(&other)), body(json!({}))).await;
    }
    let now_boot = api::get("/time").await.map(|t| t.s("boot")).unwrap_or_default();
    let last = BOOT.with(|b| b.borrow().clone());
    let restarted = !now_boot.is_empty() && !last.is_empty() && now_boot != last;
    stop(Some(rules::ended_text(if restarted { "restart" } else { "" }, &rules::host_name(&s), false)));
}

fn retry_later(id: String) {
    let wait = BACKOFF.with(|b| { let w = b.get(); b.set((w * 2).min(15000)); w });
    let t = dom::Timeout::new(wait, move || { close_events(); if current().is_some_and(|s| s.s("id") == id) { open_events(&id); } });
    RETRY.with(|r| *r.borrow_mut() = Some(t));
}
