//! Reloading into a new version of the web app by itself. After the server updates (Settings → Diagnostics →
//! Update now, or a new Docker image) an open tab still runs the old build; this module notices and reloads it,
//! without cutting off the music.
//!
//! How a new build is noticed, two ways that meet in [`announce`]:
//! - `/api/health` names the build the server serves (`web`: the hash in `encore-web-<hash>.js`, see
//!   `web_build` in server/crates/api/src/lib.rs). This page knows its own from the script it loaded
//!   ([`build_in`]). The page asks every minute, when the tab comes back into view and when the browser is
//!   online again. A server that does not answer is being restarted (or is down): the page asks every 3 s,
//!   after two minutes every 15 s, until it answers, and then compares. A server that names no build is
//!   compared by `version` against the first one this page saw.
//! - The service worker (sw.rs) finds a new worker script and the new worker waits ([`worker_ready`]).
//!   When this page already runs the build the server serves, the worker takes over quietly, with no reload.
//!   With a worker in charge, a new build from `/api/health` first asks the browser for the new worker and
//!   gives it a while to install, so that the reload lands in the new shell and not the old one.
//!
//! When to reload, decided by [`Gate`]: at once when nothing holds the page ([`waiting_for`]: the music,
//! an upload, songs being saved for offline, an open dialog). Otherwise a toast says "A new version of Encore
//! is ready", why it waits, and offers Reload; the page reloads by itself once nothing has held it for 5 s
//! (so the short gap between two songs does not count as stopped). The browser would not start the music
//! again in the new page without a tap, so a reload never happens while a song plays.
//!
//! A reload that did not bring the new build (a proxy or CDN in front still serves the old one) is not
//! repeated in a loop: the build the page reloaded for is kept in sessionStorage, and when the new page
//! still differs from it, only the toast shows.
use crate::dom;
use crate::json::J;
use serde_json::Value;
use std::cell::{Cell, RefCell};

/// The toast's text and its button.
pub const READY_TEXT: &str = "A new version of Encore is ready";
pub const RELOAD_LABEL: &str = "Reload";
/// How often the page asks `/api/health` while all is well.
const POLL_MS: u32 = 60 * 1000;
/// How often it asks while the server does not answer (a restart takes seconds) …
const DOWN_POLL_MS: u32 = 3 * 1000;
/// … and after this long without an answer (the server is down, not restarting): every 15 s.
const DOWN_SLOW_AFTER_MS: f64 = 2.0 * 60.0 * 1000.0;
const DOWN_SLOW_POLL_MS: u32 = 15 * 1000;
/// Coming back into view asks at most this often.
const FOCUS_GAP_MS: f64 = 15.0 * 1000.0;
/// The first question after the page loads (the boot has its own calls to make first).
const FIRST_CHECK_MS: u32 = 10 * 1000;
/// With a service worker in charge, how long a new build waits for its new worker before reloading anyway.
const WORKER_WAIT_MS: f64 = 20.0 * 1000.0;
/// Nothing may have held the page for this long before it reloads by itself.
pub const IDLE_FOR_MS: f64 = 5.0 * 1000.0;
/// How often a waiting reload looks again whether the page is free.
const TICK_MS: u32 = 1000;
/// sessionStorage: the build this tab last reloaded for (the loop guard).
const TRIED_KEY: &str = "encore.update.tried";

/// What one answer of `/api/health` says about the build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Seen {
    /// the server serves the build this page runs
    Same,
    /// the server serves another build; its id (the hash, or the version when it names no build)
    New(String),
    /// no answer: the server is restarting or down
    Down,
}

/// The comparison of this page's build with the server's, apart from the network.
#[derive(Debug, Default)]
pub struct Watch {
    /// this page's build (the hash of its script), when it could tell
    mine: Option<String>,
    /// the first version the server named (the fallback when a side has no build id)
    version: Option<String>,
}

impl Watch {
    /// A watch for a page running build `mine`.
    pub fn new(mine: Option<String>) -> Self { Watch { mine, version: None } }

    /// One answer of `/api/health` (None: none came).
    pub fn seen(&mut self, health: Option<&Value>) -> Seen {
        let Some(h) = health else { return Seen::Down };
        let web = h.s("web");
        if let (Some(mine), false) = (&self.mine, web.is_empty()) {
            return if *mine == web { Seen::Same } else { Seen::New(web) };
        }
        // one side has no build id: the server's version against the first one this page saw
        let v = h.s("version");
        if v.is_empty() { return Seen::Same; }
        match &self.version {
            None => { self.version = Some(v); Seen::Same }
            Some(first) if *first != v => Seen::New(v),
            Some(_) => Seen::Same,
        }
    }
}

/// What the page does next about a ready update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Act {
    Nothing,
    /// reload into the new version now (sw::apply_update)
    Reload,
    /// show the toast (once): something holds the page, or the last reload did not bring the new build
    ShowToast,
}

/// When to reload once a new version is ready.
#[derive(Debug, Default)]
pub struct Gate {
    /// a new version is ready
    ready: bool,
    /// only Reload in the toast reloads (the last automatic reload did not bring the new build)
    manual: bool,
    /// the toast is on screen
    toast: bool,
    /// a reload is under way (one only)
    reloading: bool,
    /// since when nothing has held the page (ms), while a reload waits
    idle_since: Option<f64>,
}

impl Gate {
    /// A new version is ready at `now`. `held`: something holds the page right now ([`waiting_for`]);
    /// `tried`: this tab already reloaded once for this very build, and it did not come.
    pub fn ready(&mut self, held: bool, tried: bool, now: f64) -> Act {
        if self.ready || self.reloading { return Act::Nothing; }
        self.ready = true;
        self.manual = tried;
        if !held && !tried {
            self.reloading = true;
            return Act::Reload;
        }
        if !held { self.idle_since = Some(now); }
        self.toast = true;
        Act::ShowToast
    }

    /// The tick while a reload waits: reload once nothing has held the page for [`IDLE_FOR_MS`].
    pub fn tick(&mut self, held: bool, now: f64) -> Act {
        if !self.ready || self.reloading || self.manual { return Act::Nothing; }
        if held { self.idle_since = None; return Act::Nothing; }
        let since = *self.idle_since.get_or_insert(now);
        if now - since < IDLE_FOR_MS { return Act::Nothing; }
        self.reloading = true;
        Act::Reload
    }

    /// Reload pressed in the toast.
    pub fn pressed(&mut self) -> Act {
        if self.reloading { return Act::Nothing; }
        self.reloading = true;
        Act::Reload
    }

    /// Whether a new version is ready (the tick runs, the toast may show).
    pub fn is_ready(&self) -> bool { self.ready }
    /// Whether only Reload in the toast reloads.
    pub fn is_manual(&self) -> bool { self.manual }
}

/// What holds the page from reloading, as the toast's second line says it; None when nothing does. The
/// first one counts: music, then an upload, then saving for offline, then an open dialog.
pub fn waiting_for(playing: bool, uploading: bool, downloading: bool, dialog: bool) -> Option<&'static str> {
    if playing { Some("It reloads when the music stops") }
    else if uploading { Some("It reloads when the upload is done") }
    else if downloading { Some("It reloads when the songs are saved") }
    else if dialog { Some("It reloads when this dialog is closed") }
    else { None }
}

/// The hash of the web app's script named in `html` (`encore-web-<hash>.js`, `…_bg.wasm`): the build this
/// page runs. Trunk drops leading zeros, so 12 hex digits or more count (the server's rule, `web_build_of`).
pub fn build_in(html: &str) -> Option<String> {
    let mut rest = html;
    while let Some(i) = rest.find("encore-web-") {
        rest = &rest[i + "encore-web-".len()..];
        let hash: String = rest.chars().take_while(char::is_ascii_hexdigit).collect();
        let after = rest[hash.len()..].chars().next();
        if hash.len() >= 12 && matches!(after, Some('.') | Some('_')) { return Some(hash); }
    }
    None
}

thread_local! {
    static WATCH: RefCell<Watch> = RefCell::new(Watch::default());
    static GATE: RefCell<Gate> = RefCell::new(Gate::default());
    /// a check (or the wait for a restarting server) is under way
    static CHECKING: Cell<bool> = const { Cell::new(false) };
    /// when the page last asked because it came into view
    static LAST_FOCUS: Cell<f64> = const { Cell::new(0.0) };
    /// the id of the build that is ready (the loop guard keeps it)
    static TARGET: RefCell<Option<String>> = const { RefCell::new(None) };
}

fn gate<T>(f: impl FnOnce(&mut Gate) -> T) -> T { GATE.with(|g| f(&mut g.borrow_mut())) }

/// What holds this page right now ([`waiting_for`] with the live answers).
fn held_by() -> Option<&'static str> {
    waiting_for(crate::player::playing(), crate::pages::files::uploading(), crate::offline::download::downloading(), dom::qs(".modal-bg").is_some())
}

fn session_storage() -> Option<web_sys::Storage> { dom::window().session_storage().ok().flatten() }

/// `GET /api/health` without the app's connection events (the banner would flash); None when it does not
/// answer within 3 s or not with ok. Settings → Diagnostics uses it too, while it waits for an update.
pub async fn health() -> Option<Value> {
    let ctl = web_sys::AbortController::new().ok()?;
    let signal = ctl.signal();
    let _abort = dom::Timeout::new(3000, move || ctl.abort());
    let req = gloo_net::http::Request::get("/api/health")
        .cache(web_sys::RequestCache::NoStore)
        .abort_signal(Some(&signal))
        .build().ok()?;
    let r = req.send().await.ok()?;
    if !r.ok() { return None; }
    let v: Value = serde_json::from_str(&r.text().await.ok()?).ok()?;
    v.truthy("ok").then_some(v)
}

/// Start watching (app.rs, once, at boot): the build this page runs, the timer and the listeners.
pub fn start() {
    let mine = dom::document().head().and_then(|h| build_in(&h.inner_html()));
    WATCH.with(|w| *w.borrow_mut() = Watch::new(mine));
    dom::keep_forever(dom::Timeout::new(FIRST_CHECK_MS, || dom::spawn(check())));
    dom::interval_forever(POLL_MS, || dom::spawn(check()));
    dom::on_forever(&dom::document(), "visibilitychange", |_| {
        if dom::document().hidden() { return; }
        let now = dom::now_ms();
        if now - LAST_FOCUS.with(Cell::get) < FOCUS_GAP_MS { return; }
        LAST_FOCUS.with(|l| l.set(now));
        dom::spawn(check());
    });
    dom::on_window("online", |_| dom::spawn(check()));
}

/// Ask the server once; when it does not answer, keep asking until it does (a restart), then compare.
async fn check() {
    if gate(|g| g.is_ready()) || CHECKING.with(|c| c.replace(true)) { return; }
    let mut seen = WATCH.with(|w| w.borrow_mut().seen(None));
    let start = dom::now_ms();
    let mut first = true;
    while seen == Seen::Down {
        if !first {
            let waited = dom::now_ms() - start;
            dom::sleep(if waited < DOWN_SLOW_AFTER_MS { DOWN_POLL_MS } else { DOWN_SLOW_POLL_MS }).await;
        }
        first = false;
        let h = health().await;
        seen = WATCH.with(|w| w.borrow_mut().seen(h.as_ref()));
    }
    if let Seen::New(id) = seen { new_build(id).await; }
    CHECKING.with(|c| c.set(false));
}

/// The server serves build `id`: with a service worker in charge, have its new worker installed first.
async fn new_build(id: String) {
    if crate::sw::controlled() && !crate::sw::waiting() {
        crate::sw::check_now().await;
        let start = dom::now_ms();
        while !crate::sw::waiting() && dom::now_ms() - start < WORKER_WAIT_MS { dom::sleep(500).await; }
    }
    announce(id);
}

/// sw.rs: a new worker waits. A reload when the server serves another build than this page's; a quiet
/// takeover when this page already runs it.
pub fn worker_ready() {
    dom::spawn(async {
        let h = health().await;
        match WATCH.with(|w| w.borrow_mut().seen(h.as_ref())) {
            Seen::Same => crate::sw::activate_quietly(),
            Seen::New(id) => announce(id),
            // offline: the worker has the new shell all the same
            Seen::Down => announce(String::new()),
        }
    });
}

/// A new version (build `id`) is ready: reload now, or show the toast and wait for the page to be free.
fn announce(id: String) {
    let tried = !id.is_empty() && session_storage().and_then(|s| s.get_item(TRIED_KEY).ok().flatten()).as_deref() == Some(id.as_str());
    TARGET.with(|t| *t.borrow_mut() = Some(id));
    let held = held_by();
    match gate(|g| g.ready(held.is_some(), tried, dom::now_ms())) {
        Act::Reload => reload(),
        Act::ShowToast => {
            show_toast(held);
            dom::interval_forever(TICK_MS, || {
                let held = held_by();
                if !gate(|g| g.is_manual()) { set_reason(held); }
                if gate(|g| g.tick(held.is_some(), dom::now_ms())) == Act::Reload { reload(); }
            });
        }
        Act::Nothing => {}
    }
}

/// Reload into the new version, remembering which build it was for (the loop guard).
fn reload() {
    if let (Some(s), Some(id)) = (session_storage(), TARGET.with(|t| t.borrow().clone()).filter(|i| !i.is_empty())) {
        let _ = s.set_item(TRIED_KEY, &id);
    }
    crate::sw::apply_update();
}

/// The persistent toast (it stays until the page reloads): the news, why it waits, and Reload.
fn show_toast(held: Option<&str>) {
    let Some(host) = dom::by_id("toasts") else { return };
    if dom::by_id("sw-update").is_some() { return; }
    let el = dom::create("div");
    el.set_id("sw-update");
    el.set_class_name("toast");
    let _ = el.set_attribute("role", "status");
    let _ = el.set_attribute("style", "display:flex;align-items:center;gap:12px");
    dom::set_html(&el, &format!(r#"<span class="grow">{}<span class="small muted" data-update-why style="display:block"></span></span><button class="btn sm primary" type="button">{}</button>"#, READY_TEXT, RELOAD_LABEL));
    if let Some(btn) = dom::qs_in(&el, "button") {
        let b = btn.clone();
        dom::on_forever(&btn, "click", move |_| {
            dom::set_disabled(&b, true);
            if gate(|g| g.pressed()) == Act::Reload { reload(); }
        });
    }
    let _ = host.append_child(&el);
    if !gate(|g| g.is_manual()) { set_reason(held); }
}

/// The toast's second line: what the reload waits for (empty once nothing holds the page).
fn set_reason(held: Option<&str>) {
    let Some(el) = dom::qs("#sw-update [data-update-why]") else { return };
    let text = held.unwrap_or("");
    if dom::text_of(&el) != text { dom::set_text(&el, text); }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_build_is_read_from_the_pages_script() {
        let head = r#"<link rel="modulepreload" href="/encore-web-f92ab5b84b305a59.js" crossorigin="anonymous">
            <link rel="preload" href="/encore-web-f92ab5b84b305a59_bg.wasm" as="fetch" type="application/wasm">"#;
        assert_eq!(build_in(head).as_deref(), Some("f92ab5b84b305a59"));
        // the inline module script alone, and a hash that lost its leading zero (15 digits)
        assert_eq!(build_in("import init from '/encore-web-59d563296e62da3.js';").as_deref(), Some("59d563296e62da3"));
        // the wasm name alone
        assert_eq!(build_in(r#"href="/encore-web-f92ab5b84b305a59_bg.wasm""#).as_deref(), Some("f92ab5b84b305a59"));
        // a mention that is not a build, then the real one
        assert_eq!(build_in("encore-web-app encore-web-12ab.js /encore-web-0123456789abcdef.js").as_deref(), Some("0123456789abcdef"));
        for none in ["", "<title>Encore</title>", "encore-web-", "encore-web-f92ab5b8.js", "encore-web-f92ab5b84b305a59x.js", "encore-web-f92ab5b84b305a59"] {
            assert_eq!(build_in(none), None, "{none}");
        }
    }

    #[test]
    fn another_build_on_the_server_is_new() {
        let mut w = Watch::new(Some("aaaaaaaaaaaa1111".into()));
        assert_eq!(w.seen(Some(&json!({ "ok": true, "version": "0.35.2", "web": "aaaaaaaaaaaa1111" }))), Seen::Same);
        assert_eq!(w.seen(None), Seen::Down);
        assert_eq!(w.seen(Some(&json!({ "ok": true, "version": "0.35.2.18", "web": "bbbbbbbbbbbb2222" }))), Seen::New("bbbbbbbbbbbb2222".into()));
        // the build decides, not the version: a new server version with the same web build is the same page
        let mut w = Watch::new(Some("aaaaaaaaaaaa1111".into()));
        assert_eq!(w.seen(Some(&json!({ "ok": true, "version": "0.36.0", "web": "aaaaaaaaaaaa1111" }))), Seen::Same);
    }

    #[test]
    fn without_a_build_id_the_version_decides() {
        // the server names no build (an older one, or one without the web app next to it)
        let mut w = Watch::new(Some("aaaaaaaaaaaa1111".into()));
        assert_eq!(w.seen(Some(&json!({ "ok": true, "version": "0.35.2" }))), Seen::Same, "the first version seen is this page's");
        assert_eq!(w.seen(Some(&json!({ "ok": true, "version": "0.35.2", "web": null }))), Seen::Same);
        assert_eq!(w.seen(None), Seen::Down);
        assert_eq!(w.seen(Some(&json!({ "ok": true, "version": "0.35.3" }))), Seen::New("0.35.3".into()));
        // this page could not tell its own build
        let mut w = Watch::new(None);
        assert_eq!(w.seen(Some(&json!({ "ok": true, "version": "1.0.0", "web": "bbbbbbbbbbbb2222" }))), Seen::Same);
        assert_eq!(w.seen(Some(&json!({ "ok": true, "version": "1.0.1", "web": "cccccccccccc3333" }))), Seen::New("1.0.1".into()));
        // no version either: nothing to compare
        let mut w = Watch::new(None);
        assert_eq!(w.seen(Some(&json!({ "ok": true }))), Seen::Same);
    }

    #[test]
    fn nothing_playing_reloads_at_once() {
        let mut g = Gate::default();
        assert!(!g.is_ready());
        assert_eq!(g.ready(false, false, 0.0), Act::Reload);
        // one reload only
        assert_eq!(g.ready(false, false, 1.0), Act::Nothing);
        assert_eq!(g.tick(false, 10_000.0), Act::Nothing);
        assert_eq!(g.pressed(), Act::Nothing);
    }

    #[test]
    fn music_waits_until_it_has_stopped_for_a_while() {
        let mut g = Gate::default();
        assert_eq!(g.ready(true, false, 0.0), Act::ShowToast);
        // a second announcement (the worker and /api/health both noticed) does not stack toasts
        assert_eq!(g.ready(true, false, 1.0), Act::Nothing);
        assert_eq!(g.tick(true, 1_000.0), Act::Nothing);
        // the gap between two songs: stopped for a moment, then playing again
        assert_eq!(g.tick(false, 2_000.0), Act::Nothing);
        assert_eq!(g.tick(false, 2_000.0 + IDLE_FOR_MS - 1.0), Act::Nothing);
        assert_eq!(g.tick(true, 2_000.0 + IDLE_FOR_MS - 0.5), Act::Nothing);
        assert_eq!(g.tick(false, 10_000.0), Act::Nothing, "the quiet time starts again");
        // paused for good
        assert_eq!(g.tick(false, 10_000.0 + IDLE_FOR_MS), Act::Reload);
        assert_eq!(g.tick(false, 20_000.0), Act::Nothing);
    }

    #[test]
    fn reload_in_the_toast_reloads_even_while_playing() {
        let mut g = Gate::default();
        assert_eq!(g.ready(true, false, 0.0), Act::ShowToast);
        assert_eq!(g.pressed(), Act::Reload);
        assert_eq!(g.pressed(), Act::Nothing);
        assert_eq!(g.tick(false, 60_000.0), Act::Nothing);
    }

    #[test]
    fn a_reload_that_did_not_bring_the_build_is_not_repeated() {
        // this tab already reloaded for this build: the toast only, and no automatic reload
        let mut g = Gate::default();
        assert_eq!(g.ready(false, true, 0.0), Act::ShowToast);
        assert!(g.is_manual());
        assert_eq!(g.tick(false, 60_000.0), Act::Nothing);
        assert_eq!(g.pressed(), Act::Reload);
    }

    #[test]
    fn what_holds_the_page_comes_in_order() {
        assert_eq!(waiting_for(false, false, false, false), None);
        assert_eq!(waiting_for(true, true, true, true), Some("It reloads when the music stops"));
        assert_eq!(waiting_for(false, true, true, true), Some("It reloads when the upload is done"));
        assert_eq!(waiting_for(false, false, true, true), Some("It reloads when the songs are saved"));
        assert_eq!(waiting_for(false, false, false, true), Some("It reloads when this dialog is closed"));
    }

    #[test]
    fn the_server_names_its_build_in_health() {
        // the field this module reads is the one the server sends
        let lib = include_str!("../../server/crates/api/src/lib.rs");
        assert!(lib.contains(r#""web": web"#), "/api/health sends `web`");
        assert!(lib.contains(r#"strip_prefix("encore-web-")"#), "the server reads the same script name");
    }
}
