//! Sign-ins with another service that come back through a browser: Spotify, Tidal and Last.fm (Settings →
//! Connections, each person's own accounts) and MusicBrainz (Settings → Contribute, the server's account).
//!
//! The start call (signed in: the web app, the phone app or the desktop app) makes a single-use state, 32 random
//! bytes kept with the user, the service, the device that started it and the PKCE verifier, valid for 10 minutes.
//! The service sends the browser back to the callback with it. The callback takes the state (it is gone after
//! that, whatever happens next) and links the account to the state's user, never to whoever the browser is
//! signed in as, so a sign-in started in the phone app works in a browser that is not signed in to Encore.
//!
//! A browser signed in as the state's own user (the web app's sign-in, in the same browser) is linked at once.
//! Any other browser, signed in to nobody or to someone else, first gets a page that asks: "Link the Spotify
//! account <name> to the Encore user <user>?". For Spotify, Tidal and MusicBrainz that page stops account-linking
//! CSRF: someone who starts a sign-in on their own Encore account and gets another person to open the service's
//! sign-in page in their browser would otherwise get that person's service account linked to their own Encore
//! user. Those services send the browser back only to the address registered with them, so the code reaches the
//! victim's browser and the page. The page's Link button posts a second single-use secret that only the page
//! holds; the page may not be framed. Last.fm is different: its web sign-in takes the callback address (`cb`,
//! with the state in it) from the sign-in link as given, and its token can be exchanged by whoever holds the
//! API secret, which is this server. An attacker can point `cb` at their own site, read the token the victim's
//! approval sends there and bring it to the callback in their own browser, signed in as themselves: no page is
//! shown. Nothing on this server can tell that apart, so for Last.fm only the approval page on last.fm protects
//! the person, and the Last.fm connection says "Only approve Encore on Last.fm when you started the sign-in
//! yourself."
//!
//! Where the states live: in memory ([`store`], the server state's `web.sign_ins`), each for [`STATE_SECS`]. The database has no table made for
//! them (the settings table holds the server's settings and goes into backups and exports), and the PKCE verifier
//! and the tokens waiting on the confirmation page are better never written to disk. A restart drops the
//! sign-ins in progress; the person starts again.
use crate::auth;
use crate::state::AppState;
use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::post;
use axum::{Form, Router};
use encore_core::text::html_escape as e;
use encore_db::{p, Row, RowExt};
use encore_sync::connections;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long a sign-in's state (and its confirmation page) stays good.
pub const STATE_SECS: u64 = 600;
/// Sign-ins one user may have going at once (the oldest goes first).
const PER_USER: usize = 10;
/// Sign-ins kept at most (the oldest goes first).
const MOST: usize = 2000;

/// What the error pages and the web app say when the state is unknown, used or too old.
pub const EXPIRED: &str = "This sign-in expired or was already used. Start it again in Encore";

/// A sign-in in progress.
#[derive(Clone, Debug)]
pub struct Pending {
    /// the Encore user signing in
    pub user_id: i64,
    /// "spotify", "tidal", "lastfm" or "musicbrainz"
    pub service: String,
    /// what started it, in the Sessions list's words for a user agent ("Encore app on Android", "Firefox on Linux")
    pub device: String,
    /// the PKCE code verifier (Spotify, Tidal), else ""
    pub verifier: String,
    /// when it started
    pub started: Instant,
    /// the account the service signed in, once the callback came back to a browser that has to confirm it
    pub linked: Option<Linked>,
}

/// An account a service signed in, not linked yet.
#[derive(Clone, Debug)]
pub struct Linked {
    /// the account's name ("alex" on Spotify)
    pub account: String,
    /// what the connection keeps (tokens): `connections.data`, or the MusicBrainz token answer
    pub data: Value,
}

/// The sign-ins in progress: states by `state:<value>`, confirmation pages by `confirm:<value>`, so neither can
/// stand in for the other.
#[derive(Default)]
pub struct SignIns(Mutex<HashMap<String, Pending>>);

/// The server's sign-ins in progress.
pub fn store(st: &AppState) -> &SignIns {
    &st.web.sign_ins
}

/// A new state: 32 random bytes as 64 hex digits.
pub fn new_state() -> String {
    encore_core::ids::random_hex(32)
}

/// Whether a sign-in is still within its time.
fn alive(p: &Pending) -> bool {
    p.started.elapsed() < Duration::from_secs(STATE_SECS)
}

impl SignIns {
    /// Keep a sign-in under `key`, dropping the expired ones and the oldest past the limits.
    fn put(&self, key: String, p: Pending) {
        let mut m = self.0.lock().unwrap_or_else(|e| e.into_inner());
        m.retain(|_, x| alive(x));
        let oldest = |m: &HashMap<String, Pending>, uid: Option<i64>| {
            m.iter().filter(|(_, x)| uid.is_none_or(|u| x.user_id == u)).min_by_key(|(_, x)| x.started).map(|(k, _)| k.clone())
        };
        if m.values().filter(|x| x.user_id == p.user_id).count() >= PER_USER {
            if let Some(k) = oldest(&m, Some(p.user_id)) {
                m.remove(&k);
            }
        }
        if m.len() >= MOST {
            if let Some(k) = oldest(&m, None) {
                m.remove(&k);
            }
        }
        m.insert(key, p);
    }

    /// Take the sign-in under `key`, when it is still alive.
    fn take(&self, key: &str) -> Option<Pending> {
        let mut m = self.0.lock().unwrap_or_else(|e| e.into_inner());
        m.remove(key).filter(alive)
    }

    /// A new state for `user_id`'s sign-in with `service`: [`new_state`], kept.
    pub fn start(&self, user_id: i64, service: &str, device: &str, verifier: &str) -> String {
        let state = new_state();
        self.keep(&state, user_id, service, device, verifier);
        state
    }

    /// Keeps `state` for `user_id`'s sign-in with `service`, started now.
    pub fn keep(&self, state: &str, user_id: i64, service: &str, device: &str, verifier: &str) {
        self.put(format!("state:{state}"), Pending {
            user_id, service: service.to_string(), device: device.to_string(), verifier: verifier.to_string(), started: Instant::now(), linked: None,
        });
    }

    /// The sign-in of `state`, taken: a second call with it finds nothing. None when unknown or older than
    /// [`STATE_SECS`].
    pub fn take_state(&self, state: &str) -> Option<Pending> {
        if state.is_empty() {
            return None;
        }
        self.take(&format!("state:{state}"))
    }

    /// Keeps `p` with the account the service signed in until the confirmation page answers: the page's
    /// secret. The sign-in keeps its start time, so the page is good for what is left of the 10 minutes.
    pub fn hold(&self, mut p: Pending, linked: Linked) -> String {
        p.linked = Some(linked);
        let key = encore_core::ids::random_hex(32);
        self.put(format!("confirm:{key}"), p);
        key
    }

    /// The sign-in a confirmation page holds, taken.
    pub fn take_confirm(&self, key: &str) -> Option<Pending> {
        if key.is_empty() {
            return None;
        }
        self.take(&format!("confirm:{key}")).filter(|p| p.linked.is_some())
    }

    /// The sign-in of `state` without taking it (tests).
    #[cfg(test)]
    pub fn peek(&self, state: &str) -> Option<Pending> {
        self.0.lock().unwrap().get(&format!("state:{state}")).cloned()
    }

    /// How many sign-ins `user_id` has going (tests).
    #[cfg(test)]
    pub fn count_for(&self, user_id: i64) -> usize {
        self.0.lock().unwrap().values().filter(|p| p.user_id == user_id && alive(p)).count()
    }

    /// Makes the sign-in of `state` (or a confirmation page's) `secs` older (tests of the expiry).
    #[cfg(test)]
    pub fn age(&self, key: &str, secs: u64) {
        let mut m = self.0.lock().unwrap();
        for k in [format!("state:{key}"), format!("confirm:{key}")] {
            if let Some(p) = m.get_mut(&k) {
                p.started = p.started.checked_sub(Duration::from_secs(secs)).unwrap_or(p.started);
            }
        }
    }
}

/// "Spotify", "Last.fm" … for the pages.
pub fn service_label(service: &str) -> &str {
    match service {
        "spotify" => "Spotify",
        "tidal" => "Tidal",
        "lastfm" => "Last.fm",
        "musicbrainz" => "MusicBrainz",
        s => s,
    }
}

/// The device a request comes from, in the few words the Sessions list makes of a user agent ("Encore app on
/// Android", "Firefox on Linux"): the session's user agent, else the request's. Never the device name an app
/// sent ("Pixel 9 Pro"): that is free text the starter chooses, and the confirmation page shows this to
/// someone else, beside its warning.
pub fn device_of(st: &AppState, headers: &HeaderMap) -> String {
    let ua = headers.get(header::USER_AGENT).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let s = auth::session_key(headers).and_then(|k| st.db.one("SELECT user_agent FROM sessions WHERE token=?", p![k]).ok().flatten());
    match s {
        Some(s) if !s.s("user_agent").is_empty() => crate::routes::auth::device_name(&s.s("user_agent")),
        _ => crate::routes::auth::device_name(&ua),
    }
}

/// The start call of a sign-in: a new state for `uid` and `service` (with a PKCE verifier for Spotify and
/// Tidal), handed to `url` to build the service's sign-in address. Nothing is kept when `url` fails.
pub fn start(st: &AppState, uid: i64, service: &str, headers: &HeaderMap, url: impl FnOnce(&str, &str) -> anyhow::Result<String>) -> anyhow::Result<String> {
    let verifier = if matches!(service, "spotify" | "tidal") { connections::pkce_verifier() } else { String::new() };
    let state = new_state();
    // the address first: a missing Client ID or Public URL leaves no state behind
    let address = url(&state, &verifier)?;
    store(st).keep(&state, uid, service, &device_of(st, headers), &verifier);
    Ok(address)
}

/// The test builds' stand-in for the services: a code `test-account:<name>` signs in the account `<name>`
/// without asking the service (only in `cargo test`; a real build always asks the service).
#[cfg(test)]
pub const TEST_CODE: &str = "test-account:";

/// The callback's code (Last.fm: token) exchanged with the service for the account's name and tokens.
async fn exchange(st: &AppState, p: &Pending, code: &str) -> Result<Linked, String> {
    #[cfg(test)]
    if let Some(account) = code.strip_prefix(TEST_CODE) {
        return Ok(Linked { account: account.to_string(), data: serde_json::json!({ "access_token": "test-access", "refresh_token": "test-refresh", "expires_in": 3600, "sk": "test-sk", "user": account }) });
    }
    if code.is_empty() {
        return Err(format!("{} sent no sign-in code. Try again", service_label(&p.service)));
    }
    let r = match p.service.as_str() {
        "spotify" | "tidal" => connections::oauth_exchange(st, &p.service, code, &p.verifier).await,
        "lastfm" => connections::lastfm_session(st, code).await,
        "musicbrainz" => crate::contribute::mb_exchange(st, code).await,
        _ => Err(anyhow::anyhow!("Unknown service")),
    };
    r.map(|(account, data)| Linked { account, data }).map_err(|e| e.to_string())
}

/// Links `l` to the user who started `p`: the user's connection, or for MusicBrainz the server's account (the
/// user must still be an admin). Returns the user's name.
fn link(st: &AppState, p: &Pending, l: &Linked) -> Result<String, String> {
    let u = st.db.one("SELECT * FROM users WHERE id=? AND disabled=0", p![p.user_id]).ok().flatten()
        .ok_or_else(|| "The Encore user who started this sign-in no longer exists or is disabled".to_string())?;
    match p.service.as_str() {
        "musicbrainz" => crate::contribute::mb_link(st, &u, &l.account, &l.data)?,
        s => {
            connections::save(st, p.user_id, s, &l.account, &l.data).map_err(|e| e.to_string())?;
        }
    }
    Ok(u.s("username"))
}

/// The callback of every service, after the service-specific checks. `browser`: who the browser is signed in
/// as. `error`: the service's `error=` (the person said no). `code`: the code (Last.fm: the token). `web`: a
/// browser that is signed in to Encore goes back to the web app's page with the outcome; any other browser gets
/// a page of its own.
pub async fn callback(st: &AppState, browser: Option<Row>, service: &str, state: &str, error: &str, code: &str,
                      web: impl Fn(Result<(), String>) -> Response) -> Response {
    // single use: taken before anything else, whatever comes of it
    let pending = store(st).take_state(state).filter(|p| p.service == service);
    let fail = |msg: String| if browser.is_some() { web(Err(msg)) } else { error_page(&msg) };
    if !error.is_empty() {
        return fail(if error == "access_denied" { "You did not allow Encore. Try again and approve the access".to_string() } else { error.replace('_', " ") });
    }
    let Some(p) = pending else { return fail(EXPIRED.to_string()) };
    let linked = match exchange(st, &p, code).await {
        Ok(l) => l,
        Err(e) => return fail(e),
    };
    if browser.as_ref().is_some_and(|u| u.i("id") == p.user_id) {
        return web(link(st, &p, &linked).map(|_| ()));
    }
    let owner = st.db.one("SELECT username FROM users WHERE id=?", p![p.user_id]).ok().flatten().map(|u| u.s("username")).unwrap_or_default();
    let key = store(st).hold(p.clone(), linked.clone());
    confirm_page(&p, &linked, &owner, browser.as_ref().map(|u| u.s("username")), &key)
}

/// The sign-in confirmation route.
pub fn router() -> Router<AppState> {
    Router::new().route("/api/sign-in/confirm", post(confirm))
}

/// The confirmation page's form.
#[derive(Deserialize, Default)]
#[serde(default)]
struct ConfirmForm {
    /// the confirmation key
    confirm: String,
    /// "link" to link the account, anything else cancels
    answer: String,
}

/// The confirmation page's answer: Link or Cancel.
async fn confirm(State(st): State<AppState>, Form(f): Form<ConfirmForm>) -> Response {
    let Some(p) = store(&st).take_confirm(&f.confirm) else { return error_page(EXPIRED) };
    let Some(l) = p.linked.clone() else { return error_page(EXPIRED) };
    let svc = service_label(&p.service).to_string();
    if f.answer != "link" {
        return page(StatusCode::OK, "Nothing was linked", &format!("<h1>Nothing was linked</h1><p>The {} account was not linked. You can close this page.</p>", e(&svc)));
    }
    match link(&st, &p, &l) {
        Ok(user) => {
            let what = if p.service == "musicbrainz" { "this Encore server".to_string() } else { format!("the Encore user “{}”", e(&user)) };
            page(StatusCode::OK, &format!("{svc} account linked"), &format!("<h1>{} account linked</h1><p>The {} account “{}” is now linked to {what}. You can close this page and go back to Encore.</p>",
                e(&svc), e(&svc), e(&l.account)))
        }
        Err(msg) => error_page(&msg),
    }
}

/// "Link the Spotify account “alex” to the Encore user “sam”?", who started it and where, and Link / Cancel.
fn confirm_page(p: &Pending, l: &Linked, owner: &str, browser_user: Option<String>, key: &str) -> Response {
    let svc = e(service_label(&p.service));
    let device: String = p.device.chars().take(40).collect();
    let (acct, owner_e, device) = (e(&l.account), e(owner), e(&device));
    let question = if p.service == "musicbrainz" {
        format!("Link the MusicBrainz account “{acct}” to this Encore server?")
    } else {
        format!("Link the {svc} account “{acct}” to the Encore user “{owner_e}”?")
    };
    let mut body = format!("<h1>{question}</h1><p>The Encore user “{owner_e}” started this sign-in on “{device}”.</p>");
    if p.service == "musicbrainz" {
        body.push_str("<p>Encore then sends ISRCs to MusicBrainz as this account.</p>");
    }
    if let Some(b) = browser_user {
        body.push_str(&format!("<p>This browser is signed in to Encore as “{}”, not as “{owner_e}”.</p>", e(&b)));
    }
    body.push_str(&format!(r#"<p class="warn">Only link it if you started this sign-in yourself. If someone else sent you the sign-in link, choose Cancel.</p>
<form method="post" action="/api/sign-in/confirm"><input type="hidden" name="confirm" value="{key}">
<button type="submit" name="answer" value="link">Link</button> <button type="submit" name="answer" value="cancel" class="plain">Cancel</button></form>"#, key = e(key)));
    page(StatusCode::OK, &format!("Link {svc} account"), &body)
}

/// The page that says the sign-in failed, with why.
fn error_page(msg: &str) -> Response {
    let msg: String = msg.chars().take(300).collect();
    page(StatusCode::BAD_REQUEST, "The sign-in did not work", &format!("<h1>The sign-in did not work</h1><p>{}</p><p>You can close this page and start the sign-in again in Encore.</p>", e(&msg)))
}

/// A small page of its own (no web app around it), never framed, cached or sent on as a referrer.
fn page(status: StatusCode, title: &str, body: &str) -> Response {
    let html = format!(r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>{title}</title><meta name="referrer" content="no-referrer">
<style>body{{font-family:system-ui,sans-serif;background:#111;color:#eee;display:grid;place-items:center;min-height:100vh;margin:0;padding:16px;box-sizing:border-box}}main{{max-width:520px}}h1{{font-size:1.4em}}p{{color:#bbb}}p.warn{{color:#e8c37a}}button{{font:inherit;padding:10px 18px;border-radius:8px;border:0;background:#ba478f;color:#fff;cursor:pointer;margin:6px 8px 0 0}}button.plain{{background:#333}}</style></head>
<body><main>{body}</main></body></html>"#, title = e(title));
    let mut r = (status, Html(html)).into_response();
    let h = r.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert("x-frame-options", HeaderValue::from_static("DENY"));
    h.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    h.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("default-src 'none'; style-src 'unsafe-inline'; form-action 'self'; frame-ancestors 'none'; base-uri 'none'"));
    r
}

#[cfg(test)]
#[path = "signin_tests.rs"]
mod signin_tests;
