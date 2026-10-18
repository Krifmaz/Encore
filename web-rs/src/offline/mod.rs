//! Songs saved in this browser, to play when the server can't be reached (0.28). store.rs keeps them in the
//! Cache API (`encore-offline-v1`) with their index, download.rs saves them, outbox.rs keeps the plays and
//! likes made meanwhile until the server is back. This file holds the session grace: the signed-in user
//! is remembered on this device, so the app can open in offline mode for 30 days after it last reached
//! the server.
//!
//! mode.rs is offline mode (the banner, the switched-off shell, the retries); the Downloads page and the
//! "Download for offline" menu entries are in pages/downloads.rs. The native apps keep their own offline
//! screens.

pub mod download;
pub mod mode;
pub mod outbox;
pub mod store;

pub use download::{download, download_again, Progress, Summary};
pub use store::{
    audio_url, available, cover_url, estimate, has, index, lyrics, persist, persisted, record, refresh, remove, remove_all, Record,
    CHANGED_EVENT,
};

use crate::dom;
use crate::json::J;
use serde_json::{json, Value};

/// How long after the last contact with the server the downloaded songs still play.
pub const GRACE_DAYS: i64 = 30;
const ME_KEY: &str = "encore.offline.me";
/// localStorage: the liked track ids as this device last knew them (player/local.rs keeps them for the hearts
/// in offline mode); forgotten with the user.
pub const LIKES_KEY: &str = "encore.offline.likes";
const DAY: f64 = 86_400.0;

/// What is kept of the `/auth/me` answer: all of it but the email address, which offline mode never shows.
pub fn me_to_keep(me: &Value) -> Value {
    let mut u = me.clone();
    if let Some(o) = u.as_object_mut() { o.remove("email"); }
    u
}

/// The stored document: `{user, cached_at}` (unix seconds).
pub fn me_doc(me: &Value, now: f64) -> Value { json!({ "user": me_to_keep(me), "cached_at": now.floor() }) }

/// The user and when it was remembered, from the stored document.
pub fn parse_me(text: &str) -> Option<(Value, f64)> {
    let v: Value = serde_json::from_str(text).ok()?;
    let user = v.g("user");
    if !user.is_object() { return None; }
    let at = v.num("cached_at").filter(|t| t.is_finite() && *t > 0.0)?;
    Some((user.clone(), at))
}

/// Whole days of grace left (rounded up): GRACE_DAYS right after a contact, 0 once it has ended. A clock
/// that went back counts as no time passed.
pub fn grace_left(cached_at: f64, now: f64) -> i64 {
    let passed = (now - cached_at).max(0.0);
    let left = GRACE_DAYS as f64 * DAY - passed;
    if left <= 0.0 { 0 } else { (left / DAY).ceil() as i64 }
}

/// Remember the signed-in user (after every successful `/auth/me`): localStorage `encore.offline.me`.
pub fn remember_me(me: &Value) {
    if !me.is_object() || me.is_nullish("id") { return; }
    dom::ls_set(ME_KEY, &me_doc(me, dom::now_s()).to_string());
}

/// Forget it (signing out), and the liked songs kept for that user.
pub fn forget_me() {
    dom::ls_remove(ME_KEY);
    dom::ls_remove(LIKES_KEY);
}

fn stored() -> Option<(Value, f64)> { parse_me(&dom::ls_get(ME_KEY)?) }

/// The remembered user while the grace lasts (None after GRACE_DAYS, or when nobody was remembered).
pub fn cached_me() -> Option<Value> {
    let (user, at) = stored()?;
    (grace_left(at, dom::now_s()) > 0).then_some(user)
}

/// The remembered user even after the grace ended: offline mode still opens, says to sign in again and
/// keeps the play buttons off.
pub fn last_user() -> Option<Value> { stored().map(|(u, _)| u) }

/// Days of grace left: None when nobody was remembered, Some(0) once it has ended.
pub fn grace_left_days() -> Option<i64> {
    let (_, at) = stored()?;
    Some(grace_left(at, dom::now_s()))
}

// ---- a sign-out the server has not confirmed yet ----------------------------------------------------------

/// localStorage: "1" while a sign-out has not reached the server. The browser still holds the session cookie
/// (HttpOnly: only the server can end it), so the next start that reaches the server ends the session before
/// anything else (mode.rs `settle_sign_out`), and until then the app opens neither signed in nor in offline
/// mode. A sign-in on the sign-in page replaces the cookie and clears it.
pub const SIGNOUT_PENDING_KEY: &str = "encore.signout_pending";

pub fn signout_pending() -> bool { dom::ls_get(SIGNOUT_PENDING_KEY).as_deref() == Some("1") }
pub fn set_signout_pending(on: bool) {
    if on { dom::ls_set(SIGNOUT_PENDING_KEY, "1") } else { dom::ls_remove(SIGNOUT_PENDING_KEY) }
}

/// `POST /auth/logout` answered `status` (0: no answer): whether the session may still be valid. Only an
/// answer that it ended, or that there was none (401), settles it.
pub fn session_may_live(status: u16) -> bool { !((200..300).contains(&status) || status == 401) }

/// What boot does about a pending sign-out, from the status of the `/auth/me` answer (200: signed in).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AtBoot {
    /// no sign-out pending: as usual
    AsUsual,
    /// the session still works: end it now, then open signed out
    SignOut,
    /// the server says there is no session: nothing left to end
    Gone,
    /// the server can't be reached (or failed): open signed out, without offline mode, and try again next time
    Wait,
}

pub fn at_boot(pending: bool, me_status: u16) -> AtBoot {
    match (pending, me_status) {
        (false, _) => AtBoot::AsUsual,
        (true, 200..=299) => AtBoot::SignOut,
        (true, 401) => AtBoot::Gone,
        (true, _) => AtBoot::Wait,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_grace_lasts_thirty_days_from_the_last_contact() {
        let t0 = 1_760_000_000.0;
        assert_eq!(grace_left(t0, t0), 30);
        assert_eq!(grace_left(t0, t0 + 3600.0), 30, "rounded up");
        assert_eq!(grace_left(t0, t0 + DAY), 29);
        assert_eq!(grace_left(t0, t0 + 29.5 * DAY), 1);
        assert_eq!(grace_left(t0, t0 + 30.0 * DAY - 1.0), 1);
        assert_eq!(grace_left(t0, t0 + 30.0 * DAY), 0, "ended");
        assert_eq!(grace_left(t0, t0 + 400.0 * DAY), 0);
        assert_eq!(grace_left(t0, t0 - 5.0 * DAY), 30, "a clock that went back");
    }

    #[test]
    fn a_pending_sign_out_is_settled_only_by_the_server() {
        assert_eq!(at_boot(false, 200), AtBoot::AsUsual);
        assert_eq!(at_boot(false, 0), AtBoot::AsUsual);
        assert_eq!(at_boot(true, 200), AtBoot::SignOut, "the session still works: end it first");
        assert_eq!(at_boot(true, 401), AtBoot::Gone);
        assert_eq!(at_boot(true, 0), AtBoot::Wait);
        assert_eq!(at_boot(true, 502), AtBoot::Wait);
        assert_eq!(at_boot(true, 500), AtBoot::Wait);
        assert!(!session_may_live(200));
        assert!(!session_may_live(401), "no session to end");
        assert!(session_may_live(0), "no answer: the cookie is still valid");
        assert!(session_may_live(503));
        assert!(session_may_live(500));
        assert!(session_may_live(403));
    }

    #[test]
    fn the_remembered_user_round_trips_without_the_email() {
        let me = json!({ "id": 3, "username": "ana", "email": "ana@example.org", "is_admin": false, "prefs": { "translate_mode": "" } });
        let doc = me_doc(&me, 1_760_000_000.7);
        assert_eq!(doc["cached_at"], json!(1_760_000_000.0));
        let (user, at) = parse_me(&doc.to_string()).unwrap();
        assert_eq!(at, 1_760_000_000.0);
        assert_eq!(user["username"], "ana");
        assert!(user.get("email").is_none(), "{user}");
        assert_eq!(parse_me("not json"), None);
        assert_eq!(parse_me(r#"{"user":null,"cached_at":5}"#), None);
        assert_eq!(parse_me(r#"{"user":{"id":1}}"#), None, "no time, no grace");
    }
}
