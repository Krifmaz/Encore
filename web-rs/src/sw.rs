//! The service worker from the page's side (static/sw.js is the worker itself, hooks/sw.sh fills it in at
//! build time): registers /sw.js, notices a new version and shows "A new version of Encore is ready" with a
//! Reload button, asks the server for a new worker when the tab comes back into view (at most every 10
//! minutes, and once an hour while it stays in view), and remembers which saved shell the worker serves.
//!
//! The order of events, as the browser runs them:
//! 1. `register("/sw.js")`; the browser fetches the worker script and, when it differs from the active one,
//!    fires `updatefound` with the new worker in `registration.installing`.
//! 2. The new worker saves the new shell and reaches `installed`. It does not take over by itself (sw.js no
//!    longer calls `skipWaiting()` when it installs), so it waits. Reaching `installed` while this page has a
//!    controller means "an update": the toast shows. Without a controller it is the very first worker of this
//!    browser, which activates at once: nothing to say.
//! 3. Reload posts `{type: 'skip-waiting'}` to the waiting worker; it activates, drops the old shell and claims
//!    the page; `controllerchange` fires and the page reloads into the new version. Other tabs see the same
//!    `controllerchange` but did not ask, so they stay as they are (their own Reload then reloads at once,
//!    because no worker waits any more).
//!
//! `Flow` holds the decisions as plain Rust, apart from the browser objects, so the host tests drive it with a
//! fake worker.
use crate::dom;
use serde::Serialize;
use serde_json::{json, Value};
use std::cell::RefCell;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{ServiceWorker, ServiceWorkerContainer, ServiceWorkerRegistration, ServiceWorkerState};

/// The toast's text and its button.
pub const READY_TEXT: &str = "A new version of Encore is ready";
pub const RELOAD_LABEL: &str = "Reload";
/// `visibilitychange` → visible asks for an update at most this often.
pub const CHECK_EVERY_MS: f64 = 10.0 * 60.0 * 1000.0;
/// While the tab stays in view, an update check this often (the same throttle applies).
const TIMER_MS: u32 = 60 * 60 * 1000;
/// After Reload, the page reloads on `controllerchange`; this long without one, it reloads anyway.
const RELOAD_FALLBACK_MS: u32 = 4000;

/// What the page does after an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Nothing,
    /// show the persistent "A new version of Encore is ready" toast
    ShowToast,
    /// `{type: 'skip-waiting'}` went to the waiting worker; the reload follows on `controllerchange`
    Posted,
    /// `location.reload()` now
    Reload,
}

/// The worker the page posts to: the waiting `ServiceWorker` in the browser, a fake in the tests.
pub trait Worker {
    /// `worker.postMessage(msg)`; false when the browser refused it.
    fn post(&self, msg: &Value) -> bool;
}

impl Worker for ServiceWorker {
    fn post(&self, msg: &Value) -> bool {
        // plain JS objects (serde_wasm_bindgen's default would make Maps, which the worker can't read as d.type)
        let Ok(js) = msg.serialize(&serde_wasm_bindgen::Serializer::json_compatible()) else { return false };
        self.post_message(&js).is_ok()
    }
}

/// The message that lets a waiting worker take over.
pub fn skip_waiting_message() -> Value { json!({ "type": "skip-waiting" }) }
/// The question the worker answers with `{type: 'encore:version', cache}`.
pub fn version_question() -> Value { json!({ "type": "version?" }) }

/// The page's side of an update.
#[derive(Debug, Default)]
pub struct Flow {
    /// the toast is on screen (it shows once per page load)
    toast: bool,
    /// Reload was pressed in this page
    asked: bool,
    /// a reload is under way (one only)
    reloading: bool,
    /// when the page last asked for an update (ms since the epoch)
    last_check: Option<f64>,
    /// the cache the controlling worker serves the shell from (`encore-shell-<hash>`)
    cache: Option<String>,
}

impl Flow {
    /// The worker was registered at `now`: the browser has just looked for a new one itself.
    pub fn registered(&mut self, now: f64) { self.last_check = Some(now); }

    /// A worker of this registration reached `state` (`installed`, `activated`, …).
    pub fn worker_state(&mut self, state: &str, has_controller: bool) -> Step {
        if state == "installed" && has_controller && !self.toast {
            self.toast = true;
            return Step::ShowToast;
        }
        Step::Nothing
    }

    /// Reload pressed in the toast; `waiting` is `registration.waiting` at that moment.
    pub fn reload_pressed<W: Worker>(&mut self, waiting: Option<&W>) -> Step {
        if self.reloading { return Step::Nothing; }
        self.asked = true;
        if let Some(w) = waiting {
            if w.post(&skip_waiting_message()) { return Step::Posted; }
        }
        // nothing waits any more (another tab switched already) or the message did not go out
        self.reloading = true;
        Step::Reload
    }

    /// `controllerchange`: a reload only when this page asked for the new version. The very first worker
    /// claiming the page also fires it, and so does another tab's Reload.
    pub fn controller_changed(&mut self) -> Step {
        if self.asked && !self.reloading {
            self.reloading = true;
            return Step::Reload;
        }
        Step::Nothing
    }

    /// The fallback timer after Reload: reload when `controllerchange` has not done it.
    pub fn reload_timeout(&mut self) -> Step {
        if self.reloading { return Step::Nothing; }
        self.reloading = true;
        Step::Reload
    }

    /// The tab came into view (or the hourly timer fired) at `now`: whether to call `registration.update()`.
    pub fn should_check(&mut self, now: f64) -> bool {
        if self.last_check.is_some_and(|t| now - t < CHECK_EVERY_MS) { return false; }
        self.last_check = Some(now);
        true
    }

    /// A message from the worker; `{type: 'encore:version', cache}` is remembered and returned.
    pub fn message(&mut self, data: &Value) -> Option<String> {
        if data.get("type").and_then(Value::as_str) != Some("encore:version") { return None; }
        let cache = data.get("cache").and_then(Value::as_str).filter(|c| c.starts_with("encore-shell-"))?.to_string();
        self.cache = Some(cache.clone());
        Some(cache)
    }

    pub fn cache(&self) -> Option<String> { self.cache.clone() }
    pub fn toast_shown(&self) -> bool { self.toast }
}

thread_local! {
    static FLOW: RefCell<Flow> = RefCell::new(Flow::default());
    static REG: RefCell<Option<ServiceWorkerRegistration>> = const { RefCell::new(None) };
}

fn flow<T>(f: impl FnOnce(&mut Flow) -> T) -> T { FLOW.with(|x| f(&mut x.borrow_mut())) }

/// The shell cache the worker serves this page from (`encore-shell-<hash>`), once it has said so.
pub fn shell_cache() -> Option<String> { flow(|f| f.cache()) }

/// Whether a service worker controls this page (the shell opens without the server).
pub fn controlled() -> bool { container().is_some_and(|c| c.controller().is_some()) }

/// `navigator.serviceWorker`, when this browser has it here (not in a private window of some browsers, not
/// on plain http other than localhost).
fn container() -> Option<ServiceWorkerContainer> {
    let nav = dom::window().navigator();
    let v = js_sys::Reflect::get(&nav, &JsValue::from_str("serviceWorker")).ok()?;
    if v.is_undefined() || v.is_null() { return None; }
    v.dyn_into::<ServiceWorkerContainer>().ok()
}

fn state_name(s: ServiceWorkerState) -> &'static str {
    match s {
        ServiceWorkerState::Parsed => "parsed",
        ServiceWorkerState::Installing => "installing",
        ServiceWorkerState::Installed => "installed",
        ServiceWorkerState::Activating => "activating",
        ServiceWorkerState::Activated => "activated",
        ServiceWorkerState::Redundant => "redundant",
        _ => "",
    }
}

fn apply(step: Step) {
    match step {
        Step::ShowToast => show_toast(),
        Step::Reload => dom::reload(),
        Step::Posted => dom::keep_forever(dom::Timeout::new(RELOAD_FALLBACK_MS, || apply(flow(|f| f.reload_timeout())))),
        Step::Nothing => {}
    }
}

/// The persistent toast (it stays until the page reloads).
fn show_toast() {
    let Some(host) = dom::by_id("toasts") else { return };
    if dom::by_id("sw-update").is_some() { return; }
    let el = dom::create("div");
    el.set_id("sw-update");
    el.set_class_name("toast");
    let _ = el.set_attribute("role", "status");
    let _ = el.set_attribute("style", "display:flex;align-items:center;gap:12px");
    dom::set_html(&el, &format!(r#"<span class="grow">{}</span><button class="btn sm primary" type="button">{}</button>"#, READY_TEXT, RELOAD_LABEL));
    if let Some(btn) = dom::qs_in(&el, "button") {
        let b = btn.clone();
        dom::on_forever(&btn, "click", move |_| {
            dom::set_disabled(&b, true);
            let waiting = REG.with(|r| r.borrow().as_ref().and_then(|r| r.waiting()));
            apply(flow(|f| f.reload_pressed(waiting.as_ref())));
        });
    }
    let _ = host.append_child(&el);
}

/// Follow a new worker through its states.
fn watch(w: ServiceWorker) {
    let w2 = w.clone();
    dom::on_forever(&w, "statechange", move |_| {
        let has_controller = container().is_some_and(|c| c.controller().is_some());
        apply(flow(|f| f.worker_state(state_name(w2.state()), has_controller)));
    });
}

/// `registration.update()` when the throttle allows it (offline it fails quietly).
fn check_for_update() {
    if !flow(|f| f.should_check(dom::now_ms())) { return; }
    let Some(reg) = REG.with(|r| r.borrow().clone()) else { return };
    if let Ok(p) = reg.update() {
        dom::spawn(async move { let _ = wasm_bindgen_futures::JsFuture::from(p).await; });
    }
}

fn ask_version(c: &ServiceWorkerContainer) {
    if let Some(ctrl) = c.controller() { let _ = ctrl.post(&version_question()); }
}

/// Register /sw.js and follow its updates (app.rs calls this on https and on localhost).
pub fn register() {
    let Some(c) = container() else { return };
    dom::on_forever(&c, "controllerchange", |_| {
        apply(flow(|f| f.controller_changed()));
        if let Some(c) = container() { ask_version(&c); }
    });
    dom::on_forever(&c, "message", |e| {
        let Some(me) = e.dyn_ref::<web_sys::MessageEvent>() else { return };
        let data: Value = serde_wasm_bindgen::from_value(me.data()).unwrap_or(Value::Null);
        flow(|f| f.message(&data));
    });
    dom::on_forever(&dom::document(), "visibilitychange", |_| {
        if !dom::document().hidden() { check_for_update(); }
    });
    dom::interval_forever(TIMER_MS, || { if !dom::document().hidden() { check_for_update(); } });
    dom::spawn(async move {
        let Ok(reg) = wasm_bindgen_futures::JsFuture::from(c.register("/sw.js")).await else { return };
        let Ok(reg) = reg.dyn_into::<ServiceWorkerRegistration>() else { return };
        flow(|f| f.registered(dom::now_ms()));
        REG.with(|r| *r.borrow_mut() = Some(reg.clone()));
        // a worker that installed during an earlier visit and still waits
        if reg.waiting().is_some() {
            apply(flow(|f| f.worker_state("installed", c.controller().is_some())));
        }
        if let Some(w) = reg.installing() { watch(w); }
        let reg2 = reg.clone();
        dom::on_forever(&reg, "updatefound", move |_| { if let Some(w) = reg2.installing() { watch(w); } });
        ask_version(&c);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// A waiting worker that records what the page posted to it.
    #[derive(Default)]
    struct Fake { sent: RefCell<Vec<Value>>, refuse: bool }
    impl Worker for Fake {
        fn post(&self, msg: &Value) -> bool {
            if self.refuse { return false; }
            self.sent.borrow_mut().push(msg.clone());
            true
        }
    }

    #[test]
    fn a_new_worker_shows_the_toast_once_and_only_as_an_update() {
        let mut f = Flow::default();
        // the very first worker of this browser: no controller yet, nothing to say
        assert_eq!(f.worker_state("installing", false), Step::Nothing);
        assert_eq!(f.worker_state("installed", false), Step::Nothing);
        assert!(!f.toast_shown());
        // a new version next to a controlling one
        assert_eq!(f.worker_state("installing", true), Step::Nothing);
        assert_eq!(f.worker_state("installed", true), Step::ShowToast);
        assert!(f.toast_shown());
        // a second statechange or a second new worker does not stack toasts
        assert_eq!(f.worker_state("installed", true), Step::Nothing);
        assert_eq!(f.worker_state("activated", true), Step::Nothing);
    }

    #[test]
    fn reload_posts_skip_waiting_then_reloads_on_controllerchange() {
        let mut f = Flow::default();
        f.worker_state("installed", true);
        let w = Fake::default();
        assert_eq!(f.reload_pressed(Some(&w)), Step::Posted);
        assert_eq!(*w.sent.borrow(), vec![json!({ "type": "skip-waiting" })]);
        assert_eq!(f.controller_changed(), Step::Reload);
        // one reload only: a second controllerchange or the fallback timer do nothing
        assert_eq!(f.controller_changed(), Step::Nothing);
        assert_eq!(f.reload_timeout(), Step::Nothing);
        assert_eq!(f.reload_pressed(Some(&w)), Step::Nothing);
    }

    #[test]
    fn the_fallback_timer_reloads_when_controllerchange_never_comes() {
        let mut f = Flow::default();
        let w = Fake::default();
        assert_eq!(f.reload_pressed(Some(&w)), Step::Posted);
        assert_eq!(f.reload_timeout(), Step::Reload);
        assert_eq!(f.controller_changed(), Step::Nothing);
    }

    #[test]
    fn controllerchange_without_reload_pressed_keeps_the_page() {
        // the first worker claiming the page, or another tab pressing Reload
        let mut f = Flow::default();
        assert_eq!(f.controller_changed(), Step::Nothing);
        f.worker_state("installed", true);
        assert_eq!(f.controller_changed(), Step::Nothing);
        // this tab's Reload afterwards: nothing waits any more, so it reloads at once
        assert_eq!(f.reload_pressed::<Fake>(None), Step::Reload);
    }

    #[test]
    fn a_refused_message_reloads_at_once() {
        let mut f = Flow::default();
        let w = Fake { refuse: true, ..Fake::default() };
        assert_eq!(f.reload_pressed(Some(&w)), Step::Reload);
        assert!(w.sent.borrow().is_empty());
    }

    #[test]
    fn update_checks_are_at_least_ten_minutes_apart() {
        let mut f = Flow::default();
        let t0 = 1_700_000_000_000.0;
        // never checked: yes
        assert!(f.should_check(t0));
        assert!(!f.should_check(t0 + 60_000.0));
        assert!(!f.should_check(t0 + CHECK_EVERY_MS - 1.0));
        assert!(f.should_check(t0 + CHECK_EVERY_MS));
        // registering counts as a check (the browser looked for a new worker itself)
        let mut g = Flow::default();
        g.registered(t0);
        assert!(!g.should_check(t0 + 5.0 * 60_000.0));
        assert!(g.should_check(t0 + 11.0 * 60_000.0));
    }

    #[test]
    fn the_version_answer_is_remembered() {
        let mut f = Flow::default();
        assert_eq!(f.message(&json!({ "type": "other", "cache": "encore-shell-abc" })), None);
        assert_eq!(f.message(&json!({ "type": "encore:version" })), None);
        assert_eq!(f.message(&json!({ "type": "encore:version", "cache": "encore-offline-v1" })), None);
        assert_eq!(f.message(&Value::Null), None);
        assert_eq!(f.cache(), None);
        assert_eq!(f.message(&json!({ "type": "encore:version", "cache": "encore-shell-e56412d8e3fb9a38" })).as_deref(), Some("encore-shell-e56412d8e3fb9a38"));
        assert_eq!(f.cache().as_deref(), Some("encore-shell-e56412d8e3fb9a38"));
    }

    #[test]
    fn the_messages_match_the_worker() {
        // static/sw.js reads d.type === 'skip-waiting' and d.type === 'version?', and answers 'encore:version'
        let sw = include_str!("../static/sw.js");
        assert_eq!(skip_waiting_message()["type"], "skip-waiting");
        assert_eq!(version_question()["type"], "version?");
        assert!(sw.contains("d.type === 'skip-waiting'"));
        assert!(sw.contains("d.type === 'version?'"));
        assert!(sw.contains("type: 'encore:version', cache: CACHE"));
        // install waits: the page decides when the new worker takes over
        let install = sw.split("addEventListener('install'").nth(1).unwrap().split("addEventListener('activate'").next().unwrap();
        let code: String = install.lines().filter(|l| !l.trim_start().starts_with("//")).collect();
        assert!(!code.contains("skipWaiting"), "install must not call skipWaiting()");
        // activate drops old shells only, never the saved songs
        assert!(sw.contains("k.startsWith(SHELL_PREFIX) && k !== CACHE"));
        assert!(sw.contains("const SHELL_PREFIX = 'encore-shell-'"));
        // what the worker never touches
        for p in ["p.startsWith('/api/')", "p === '/sw.js'", "url.origin !== self.location.origin"] {
            assert!(sw.contains(p), "{p}");
        }
        assert!(sw.contains("const CACHE = '__CACHE__';") && sw.contains("const SHELL = [__SHELL__];"), "the hook's placeholders");
    }
}
