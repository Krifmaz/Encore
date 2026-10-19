//! Offline mode (0.28): the server can't be reached when the page opens, but this browser has downloaded songs
//! and remembers who was signed in. The app then opens with the remembered user, every part that needs the
//! server switched off (class `offline-disabled`), a banner at the top and the Downloads page; the songs saved
//! on this device play. Every 30 s (and when the browser says it is back online, or on Retry) `/auth/me` is
//! tried again; once it answers, the plays and likes made meanwhile are sent and the page reloads into the
//! normal app.
//!
//! The session grace (mod.rs): 30 days after the last contact with the server the downloads still play; after
//! that offline mode still opens, but says to sign in again and keeps the play buttons off.
//!
//! app.rs calls [`settle_sign_out`] with the `/auth/me` answer, [`try_enter`] when it fails, [`shell_ready`]
//! once the shell is drawn, [`route_path`] on every navigation, [`signed_in`] after a normal sign-in and
//! [`sign_out`] from the Sign out button. The player asks [`active`] and [`can_play`].

use super::{outbox, AtBoot};
use crate::api::{ApiError, ApiResult};
use crate::ui::{esc, icon, toast};
use crate::{api, app, dom};
use serde_json::Value;
use std::cell::Cell;
use std::future::Future;

/// How often offline mode tries to reach the server again.
pub const RETRY_MS: u32 = 30_000;
/// How long `/auth/me` gets when offline mode could open (and on each retry): a server whose address drops
/// packets would otherwise keep the page waiting for the connect timeout, a minute or more.
pub const WAIT_MS: u32 = 8_000;
/// While the server answers, the remembered user (the 30 days of grace) is refreshed at most this often.
const REMEMBER_EVERY_MS: f64 = 3_600_000.0;
/// The one page offline mode shows.
pub const PAGE: &str = "/downloads";
pub const BANNER: &str = "Offline – the server can't be reached. Downloaded songs play.";
pub const BANNER_ENDED: &str = "Offline – sign in again when the server is back to keep playing downloaded songs";
pub const DISABLED_TITLE: &str = "Not available offline";
pub const STILL_OFFLINE: &str = "The server still can't be reached";

thread_local! {
    static ACTIVE: Cell<bool> = const { Cell::new(false) };
    /// offline mode with the grace over: nothing plays
    static ENDED: Cell<bool> = const { Cell::new(false) };
    /// a retry is on its way
    static TRYING: Cell<bool> = const { Cell::new(false) };
    /// when the remembered user was last refreshed (ms), and whether the server's answers are watched for it
    static REMEMBERED: Cell<f64> = const { Cell::new(0.0) };
    static WATCHING: Cell<bool> = const { Cell::new(false) };
}

/// The app runs in offline mode (the server could not be reached when the page opened).
pub fn active() -> bool { ACTIVE.with(|a| a.get()) }
/// Offline mode after the 30 days of grace: the user has to sign in again before anything plays.
pub fn grace_ended() -> bool { active() && ENDED.with(|e| e.get()) }
/// Downloaded songs may play now (always true online).
pub fn can_play() -> bool { !grace_ended() }

/// An answer that means the server could not be reached: no answer at all (status 0, which is also what
/// api.rs makes of a proxy's own error page, such as Traefik's 404 or Cloudflare's 530), or a proxy in front
/// of it saying it is down (502–504).
pub fn unreachable(status: u16) -> bool { status == 0 || (502..=504).contains(&status) }

/// Whether `/auth/me` failing with `status` opens offline mode, and with which user: the one remembered within
/// the grace (`cached`), else the last one remembered (`last`, the grace has ended → true). None = the app
/// behaves as before (the sign-in page with its "Can't reach" state).
pub fn decide(status: u16, cached: Option<Value>, last: Option<Value>, saved: usize) -> Option<(Value, bool)> {
    if !unreachable(status) || saved == 0 { return None; }
    match (cached, last) {
        (Some(u), _) => Some((u, false)),
        (None, Some(u)) => Some((u, true)),
        _ => None,
    }
}

/// boot(), before asking the server: offline mode could open if it can't be reached (downloads here and a
/// remembered user). Then the first calls get [`WAIT_MS`] instead of the connect timeout.
pub async fn could_open() -> bool {
    super::available() && super::last_user().is_some() && !super::index().await.is_empty()
}

/// boot(): `/auth/me` failed with `status`. Enters offline mode when it applies – the user is set from the
/// remembered one and true comes back; app.rs then draws the shell and calls [`shell_ready`].
pub async fn try_enter(status: u16) -> bool {
    if !unreachable(status) || !super::available() { return false; }
    let saved = super::index().await.len();
    let Some((user, ended)) = decide(status, super::cached_me(), super::last_user(), saved) else { return false };
    ACTIVE.with(|a| a.set(true));
    ENDED.with(|e| e.set(ended));
    app::set_user(Some(user));
    // read the outbox now, so "Back online" can say what was sent
    let _ = outbox::pending();
    true
}

/// After a normal sign-in (every successful `/auth/me`): remember the user for offline mode, read the saved
/// songs so the player knows them from the start, and send the plays and likes still waiting.
pub fn signed_in(me: &Value) {
    super::remember_me(me);
    REMEMBERED.with(|r| r.set(dom::now_ms()));
    // the grace counts from the last contact with the server, not from the last page load: a tab or an app
    // window that stays open for weeks keeps it fresh (api.rs sends encore:server-up on every answer of the
    // server). Only a user still remembered is refreshed: signing out forgets them for good.
    if !WATCHING.with(|w| w.replace(true)) {
        dom::on_window("encore:server-up", |_| {
            let now = dom::now_ms();
            if active() || now - REMEMBERED.with(|r| r.get()) < REMEMBER_EVERY_MS || super::last_user().is_none() { return; }
            if let Some(u) = app::user() {
                REMEMBERED.with(|r| r.set(now));
                super::remember_me(&u);
            }
        });
    }
    if !super::available() { return; }
    dom::spawn(async {
        super::index().await;
        if let Err(e) = outbox::flush().await { dom::console_error(&format!("offline outbox: {e}")); }
        // what downloads stopped in the middle left behind (a tab closed, an app killed)
        super::download::sweep_leftovers().await;
    });
}

/// End this browser's session on the server: the plays and likes still waiting go out first, under it
/// (outbox.rs sends only its owner's). The status of `POST /auth/logout` (0: no answer, or none within
/// [`WAIT_MS`]: Sign out does not wait for the connect timeout, the sign-out just stays pending).
async fn end_session(send_outbox: bool) -> u16 {
    if send_outbox && super::available() {
        if let Err(e) = outbox::flush().await { dom::console_error(&format!("offline outbox: {e}")); }
    }
    match api::post_empty_within("/auth/logout", WAIT_MS).await { Ok(_) => 200, Err(e) => e.status }
}

/// The Sign out button. When the server does not confirm it (it can't be reached, say), the browser still
/// holds the session cookie: the sign-out stays pending (offline/mod.rs `SIGNOUT_PENDING_KEY`) and the next
/// start that reaches the server ends the session before anything opens. Offline mode forgets the user
/// either way (the downloads stay for the next sign-in).
pub async fn sign_out() {
    // in offline mode the server is most likely away: one wait for it is enough
    let status = end_session(!active()).await;
    super::set_signout_pending(super::session_may_live(status));
    super::forget_me();
}

/// boot(), with the `/auth/me` answer: a sign-out that could not reach the server earlier is finished first.
/// What comes back is the answer boot goes on with – while a sign-out is pending, "not signed in", so the app
/// opens neither as the user who signed out nor in offline mode.
pub async fn settle_sign_out(me: ApiResult) -> ApiResult {
    if !super::signout_pending() { return me; }
    let (me, still) = settle(true, me, || end_session(true)).await;
    super::set_signout_pending(still);
    // another tab that reached the server meanwhile may have remembered the user again
    super::forget_me();
    me
}

/// [`settle_sign_out`] without the browser: `end` ends the session and gives the status of its
/// `POST /auth/logout`. The answer to go on with, and whether the sign-out is still pending.
pub async fn settle<F, Fut>(pending: bool, me: ApiResult, end: F) -> (ApiResult, bool)
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = u16>,
{
    let status = match &me { Ok(_) => 200, Err(e) => e.status };
    let signed_out = || Err(ApiError { status: 401, message: "Signed out".into(), data: Value::Null });
    match super::at_boot(pending, status) {
        AtBoot::AsUsual => (me, false),
        AtBoot::SignOut => {
            let s = end().await;
            (signed_out(), super::session_may_live(s))
        }
        AtBoot::Gone => (signed_out(), false),
        AtBoot::Wait => (signed_out(), true),
    }
}

/// The path to render for `path`: in offline mode everything but the Downloads page becomes the Downloads
/// page (the address is replaced, so Back doesn't land on a page that can't open).
pub fn route_path(path: String) -> String {
    if !active() || allowed(&path) { return path; }
    let _ = dom::window().history().and_then(|h| h.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&format!("#{PAGE}"))));
    PAGE.to_string()
}
fn allowed(path: &str) -> bool { path == PAGE }

/// The shell is on screen in offline mode: the banner, the entries that need the server switched off, and the
/// retries.
pub fn shell_ready() {
    if !active() { return; }
    dom::add_class(&dom::body(), "offline-mode");
    // the failed calls at boot already put up "The Encore server isn't answering"
    if let Some(b) = dom::by_id("conn-banner") { b.remove(); }
    banner();
    disable_shell();
    dom::interval_forever(RETRY_MS, || dom::spawn(retry(false)));
    dom::on_window("online", |_| dom::spawn(retry(false)));
    // a tap on a switched-off entry does nothing (the route guard would land on Downloads anyway)
    if let Some(sb) = dom::by_id("sidebar") {
        dom::on_forever(&sb, "click", |e| {
            if dom::closest(e, ".offline-disabled").is_some() { e.prevent_default(); e.stop_propagation(); }
        });
    }
}

fn banner() {
    if dom::by_id("offline-banner").is_some() { return; }
    let b = dom::create("div");
    b.set_id("offline-banner");
    b.set_class_name("offline-banner");
    let _ = b.set_attribute("role", "status");
    let text = if grace_ended() { BANNER_ENDED } else { BANNER };
    dom::set_html(&b, &format!(r#"{}<span>{}</span><button class="btn sm" type="button" id="offline-retry">Retry</button>"#, icon("offline"), esc(text)));
    if let Some(btn) = dom::qs_in(&b, "#offline-retry") {
        dom::on_forever(&btn, "click", |_| dom::spawn(retry(true)));
    }
    let body = dom::body();
    let _ = body.insert_before(&b, body.first_child().as_ref());
}

/// Everything in the sidebar but Downloads (and signing out: without the server it is finished at the next
/// start that reaches it, see [`sign_out`]) is switched off.
fn disable_shell() {
    for el in dom::qsa("#sidebar a[href], #sidebar .sidebar-foot a") {
        if dom::data(&el, "nav") == PAGE { continue; }
        dom::add_class(&el, "offline-disabled");
        let _ = el.set_attribute("title", DISABLED_TITLE);
        let _ = el.set_attribute("aria-disabled", "true");
        let _ = el.set_attribute("tabindex", "-1");
    }
}

/// "3 plays and 1 like", "1 play", "2 likes" ("" for none).
pub fn counts_text(plays: usize, likes: usize) -> String {
    let one = |n: usize, word: &str| format!("{n} {word}{}", if n == 1 { "" } else { "s" });
    match (plays, likes) {
        (0, 0) => String::new(),
        (p, 0) => one(p, "play"),
        (0, l) => one(l, "like"),
        (p, l) => format!("{} and {}", one(p, "play"), one(l, "like")),
    }
}

/// The toast once the server answers again: "Back online – 3 plays sent".
pub fn back_online_text(plays: usize, likes: usize) -> String {
    let sent = counts_text(plays, likes);
    if sent.is_empty() { "Back online".into() } else { format!("Back online – {sent} sent") }
}

fn retry_button(busy: bool) {
    if let Some(b) = dom::by_id("offline-retry") {
        dom::set_disabled(&b, busy);
        dom::set_text(&b, if busy { "Trying…" } else { "Retry" });
    }
}

/// Try the server again; when it answers, send what waits in the outbox and reload into the normal app.
async fn retry(pressed: bool) {
    if !active() || TRYING.with(|t| t.replace(true)) { return; }
    retry_button(true);
    match api::get_within("/auth/me", WAIT_MS).await {
        Ok(me) => {
            super::remember_me(&me);
            let before = outbox::pending_counts();
            let sent = outbox::flush().await;
            let after = outbox::pending_counts();
            let (plays, likes) = (before.0.saturating_sub(after.0), before.1.saturating_sub(after.1));
            if let Err(e) = &sent { dom::console_error(&format!("offline outbox: {e}")); }
            toast(&back_online_text(plays, likes), "ok");
            dom::sleep(1500).await;
            dom::reload();
        }
        // the server answers but this session is gone: the normal app shows the sign-in page. Any other answer
        // (an error of the server itself) keeps offline mode: a reload would only land on the sign-in page.
        Err(e) if e.status == 401 || e.status == 403 => dom::reload(),
        Err(_) => {
            TRYING.with(|t| t.set(false));
            retry_button(false);
            if pressed { toast(STILL_OFFLINE, "warn"); }
        }
    }
}

/// Switch a button off the offline way (for pages that draw their own buttons).
pub fn mark_disabled(el: &web_sys::Element) {
    dom::add_class(el, "offline-disabled");
    let _ = el.set_attribute("title", DISABLED_TITLE);
    dom::set_disabled(el, true);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn offline_mode_needs_no_answer_a_remembered_user_and_downloads() {
        let ana = || Some(json!({ "id": 1, "username": "ana" }));
        assert_eq!(decide(0, ana(), ana(), 3), Some((json!({ "id": 1, "username": "ana" }), false)));
        assert_eq!(decide(502, ana(), ana(), 3).map(|x| x.1), Some(false), "a proxy saying the server is down");
        assert_eq!(decide(0, None, ana(), 3).map(|x| x.1), Some(true), "the grace has ended: sign in again");
        assert_eq!(decide(0, None, None, 3), None, "nobody remembered");
        assert_eq!(decide(0, ana(), ana(), 0), None, "nothing downloaded");
        assert_eq!(decide(401, ana(), ana(), 3), None, "the server answered: signed out");
        assert_eq!(decide(500, ana(), ana(), 3), None);
    }

    #[test]
    fn a_pending_sign_out_ends_the_session_before_boot_goes_on() {
        use futures::executor::block_on;
        let me = || -> ApiResult { Ok(json!({ "id": 1, "username": "ana", "role": "admin" })) };
        let down = || -> ApiResult { Err(ApiError { status: 0, message: "Not connected to the server".into(), data: Value::Null }) };
        // pending, and the session still works: the logout call, then signed out
        let calls = Cell::new(0);
        let (r, still) = block_on(settle(true, me(), || { calls.set(calls.get() + 1); async { 200 } }));
        assert_eq!(calls.get(), 1, "POST /auth/logout first");
        assert_eq!(r.map_err(|e| e.status).err(), Some(401), "then the sign-in page, not ana's library");
        assert!(!still, "the server confirmed it");
        // the logout call failed: signed out all the same, and it stays pending
        let (r, still) = block_on(settle(true, me(), || async { 0 }));
        assert!(r.is_err() && still);
        let (r, still) = block_on(settle(true, me(), || async { 500 }));
        assert!(r.is_err() && still);
        // the server can't be reached: no call, signed out (no offline mode), still pending
        let calls = Cell::new(0);
        let (r, still) = block_on(settle(true, down(), || { calls.set(calls.get() + 1); async { 200 } }));
        assert_eq!(calls.get(), 0);
        assert_eq!(r.map_err(|e| e.status).err(), Some(401));
        assert!(still);
        // the session is already gone
        let gone = Err(ApiError { status: 401, message: "Not signed in".into(), data: Value::Null });
        let (r, still) = block_on(settle(true, gone, || async { 200 }));
        assert!(r.is_err() && !still);
        // nothing pending: the answer as it was, no call
        let calls = Cell::new(0);
        let (r, still) = block_on(settle(false, me(), || { calls.set(calls.get() + 1); async { 200 } }));
        assert_eq!((r.ok().map(|u| u["id"].clone()), still, calls.get()), (Some(json!(1)), false, 0));
        let (r, _) = block_on(settle(false, down(), || async { 200 }));
        assert_eq!(r.map_err(|e| e.status).err(), Some(0), "offline mode can still open");
    }

    #[test]
    fn the_counts_read_as_words() {
        assert_eq!(counts_text(0, 0), "");
        assert_eq!(counts_text(1, 0), "1 play");
        assert_eq!(counts_text(3, 1), "3 plays and 1 like");
        assert_eq!(counts_text(0, 2), "2 likes");
        assert_eq!(back_online_text(0, 0), "Back online");
        assert_eq!(back_online_text(3, 0), "Back online – 3 plays sent");
        assert_eq!(back_online_text(3, 1), "Back online – 3 plays and 1 like sent");
    }
}
