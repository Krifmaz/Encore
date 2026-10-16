//! YouTube sign-in for yt-dlp, three ways, freshest first:
//!
//! * **managed**: a headless browser Encore runs (browser.rs). Signed in once through Encore's guided
//!   form (guided.rs: Encore fills Google's sign-in pages) or the live view, it
//!   opens YouTube Music every 48 h (and when the cookies near their end, or yt-dlp is asked to sign in)
//!   so Google rotates the cookies the normal way, and exports them.
//! * **extension**: the Encore browser extension (extension/encore-youtube-cookies) posts the cookies of
//!   the owner's own browser. Encore must never rotate these itself: that would sign the browser out.
//! * **pasted**: a cookies.txt pasted under Settings (`ytdlp_cookies`). With nothing else, Encore does a
//!   light refresh every 4 h (rotate.rs) and saves the renewed cookies back.
//!
//! Precedence managed > extension > pasted among the sources that are signed in; with none signed in,
//! the freshest one is still handed to yt-dlp.
pub mod browser;
pub mod guided;
pub mod jar;
pub mod rotate;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod guided_tests;

use crate::auth::Admin;
use crate::db::now;
use crate::error::{err, ApiResult};
use crate::state::AppState;
use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Json, Router};
use jar::Jar;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

/// Managed browser: open YouTube Music this often.
pub const MANAGED_EVERY: f64 = 48.0 * 3600.0;
/// …or sooner, when the sign-in cookies expire within this.
pub const EXPIRY_MARGIN: f64 = 3.0 * 86400.0;
/// Light refresh of pasted cookies.
pub const LIGHT_EVERY: f64 = 4.0 * 3600.0;
/// How often the supervisor looks.
const TICK: Duration = Duration::from_secs(600);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Managed,
    Extension,
    Pasted,
}

impl Kind {
    pub const ALL: [Kind; 3] = [Kind::Managed, Kind::Extension, Kind::Pasted];
    pub fn key(self) -> &'static str {
        match self {
            Kind::Managed => "managed",
            Kind::Extension => "extension",
            Kind::Pasted => "pasted",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Kind::Managed => "Encore's browser",
            Kind::Extension => "Browser extension",
            Kind::Pasted => "Pasted cookies",
        }
    }
}

/// What Encore knows about one source.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SourceState {
    /// cookies last received / exported / rotated
    pub updated_at: Option<f64>,
    /// last check of the sign-in
    pub checked_at: Option<f64>,
    /// the last check's verdict (None: not checked)
    pub ok: Option<bool>,
    pub account: Option<String>,
    pub message: Option<String>,
    /// the extension's browser ("Chrome 141", "Firefox 143")
    pub agent: Option<String>,
}

/// `<config>/youtube-browser/state.json`
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AuthState {
    pub managed: SourceState,
    pub extension: SourceState,
    pub pasted: SourceState,
    /// sha256 of the extension's token
    pub ext_token: Option<String>,
    pub ext_token_at: Option<f64>,
    /// admins were told the sign-in ended (once, until it works again)
    pub signed_out_notified: bool,
}

impl AuthState {
    pub fn get(&self, k: Kind) -> &SourceState {
        match k {
            Kind::Managed => &self.managed,
            Kind::Extension => &self.extension,
            Kind::Pasted => &self.pasted,
        }
    }
}

static STATE_LOCK: Mutex<()> = Mutex::new(());

fn state_path(data: &Path) -> PathBuf {
    browser::home(data).join("state.json")
}

pub fn load(data: &Path) -> AuthState {
    std::fs::read_to_string(state_path(data)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

/// Change the state file (read, change, write under one lock).
pub fn update<R>(data: &Path, f: impl FnOnce(&mut AuthState) -> R) -> R {
    let _g = STATE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut s = load(data);
    let r = f(&mut s);
    if let Ok(t) = serde_json::to_string_pretty(&s) {
        if let Err(e) = browser::write_private(&state_path(data), &t) {
            tracing::warn!("saving the YouTube sign-in state failed: {e}");
        }
    }
    r
}

fn ext_path(data: &Path) -> PathBuf {
    browser::home(data).join("extension-cookies.txt")
}

fn managed_path(data: &Path) -> PathBuf {
    browser::home(data).join("cookies.txt")
}

/// The cookies of one source (empty when it has none).
pub fn jar_of(st: &AppState, k: Kind) -> Jar {
    match k {
        Kind::Managed => std::fs::read_to_string(managed_path(st.data_dir())).map(|t| Jar::parse_netscape(&t)).unwrap_or_default(),
        Kind::Extension => std::fs::read_to_string(ext_path(st.data_dir())).map(|t| Jar::parse_netscape(&t)).unwrap_or_default(),
        Kind::Pasted => Jar::parse_netscape(&st.cfg.str("ytdlp_cookies")),
    }
}

/// One source as the chooser sees it.
#[derive(Clone, Debug)]
pub struct Candidate {
    pub kind: Kind,
    pub jar: Jar,
    pub state: SourceState,
}

/// Which source yt-dlp gets: the first signed-in one in precedence order (signed in = sign-in cookies
/// present and unexpired, and the last check did not find it signed out); with none, the freshest that
/// has any cookies.
pub fn choose(cands: &[Candidate], now: i64) -> Option<Kind> {
    for k in Kind::ALL {
        if let Some(c) = cands.iter().find(|c| c.kind == k) {
            if c.jar.has_auth(now) && c.state.ok != Some(false) {
                return Some(k);
            }
        }
    }
    cands.iter().filter(|c| !c.jar.is_empty())
        .max_by(|a, b| a.state.updated_at.unwrap_or(0.0).total_cmp(&b.state.updated_at.unwrap_or(0.0)))
        .map(|c| c.kind)
}

fn candidates(st: &AppState, s: &AuthState) -> Vec<Candidate> {
    Kind::ALL.iter().map(|k| Candidate { kind: *k, jar: jar_of(st, *k), state: s.get(*k).clone() }).collect()
}

/// The source in use now.
pub fn active(st: &AppState) -> Option<Kind> {
    let s = load(st.data_dir());
    choose(&candidates(st, &s), now() as i64)
}

/// Netscape cookies for yt-dlp from the source in use (None: no cookies at all).
pub fn cookies_for_ytdlp(st: &AppState) -> Option<String> {
    let s = load(st.data_dir());
    let c = candidates(st, &s);
    let k = choose(&c, now() as i64)?;
    let jar = &c.iter().find(|x| x.kind == k)?.jar;
    // the pasted text as it is (it may hold cookies of other sites the owner wanted yt-dlp to have)
    Some(if k == Kind::Pasted { st.cfg.str("ytdlp_cookies") } else { jar.to_netscape() })
}

// ---- when to refresh ----------------------------------------------------------------------------

fn newest(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.max(y)),
        (x, y) => x.or(y),
    }
}

/// Managed browser: due after 48 h, or when the sign-in cookies expire within 3 days. `last` is the
/// last refresh (None: never – then only once there is a profile to refresh).
pub fn managed_due(now: f64, last: Option<f64>, expiry: Option<i64>) -> bool {
    let Some(last) = last else { return false };
    now - last >= MANAGED_EVERY || expiry.is_some_and(|e| (e as f64) - now < EXPIRY_MARGIN)
}

/// Light refresh: pasted cookies only (no extension, no managed browser in use), every 4 h.
pub fn light_due(now: f64, last: Option<f64>, active: Option<Kind>, enabled: bool) -> bool {
    enabled && active == Some(Kind::Pasted) && last.is_none_or(|l| now - l >= LIGHT_EVERY)
}

fn managed_ready(st: &AppState, s: &AuthState) -> bool {
    browser::installed(st.data_dir()).is_some() && s.managed.updated_at.is_some()
}

/// The sign-in account behind a jar (YouTube Music's account menu).
pub async fn account_of(st: &AppState, jar: &Jar) -> Option<String> {
    let cookie = jar.header_for("music.youtube.com", now() as i64);
    if cookie.is_empty() {
        return None;
    }
    let data = json!({ "mode": "browser", "headers": { "cookie": cookie, "x-goog-authuser": "0" } });
    let v = crate::sources::ytmusic::authed(st, &data, "account/account_menu", json!({})).await.ok()?;
    account_name(&v)
}

/// "Name (handle or e-mail)" from an account_menu answer.
pub fn account_name(v: &Value) -> Option<String> {
    fn find<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
        match v {
            Value::Object(m) => m.get(key).or_else(|| m.values().find_map(|x| find(x, key))),
            Value::Array(a) => a.iter().find_map(|x| find(x, key)),
            _ => None,
        }
    }
    let h = find(v, "activeAccountHeaderRenderer")?;
    let text = |k: &str| h[k]["runs"].as_array().and_then(|r| r.first()).and_then(|r| r["text"].as_str()).or_else(|| h[k]["simpleText"].as_str()).map(String::from);
    let name = text("accountName")?;
    Some(match text("channelHandle").or_else(|| text("email")) {
        Some(x) if !x.is_empty() && x != name => format!("{name} ({x})"),
        _ => name,
    })
}

/// Is a jar signed in to YouTube Music? (the page's ytcfg says LOGGED_IN)
pub async fn verify(st: &AppState, jar: &Jar) -> anyhow::Result<bool> {
    let cookie = jar.header_for("music.youtube.com", now() as i64);
    if cookie.is_empty() {
        return Ok(false);
    }
    let html = st.http.get("https://music.youtube.com/").header("Cookie", cookie).header("Accept-Language", "en").header("User-Agent", rotate::UA)
        .timeout(Duration::from_secs(30)).send().await?.text().await?;
    Ok(html.contains("\"LOGGED_IN\":true"))
}

// ---- refreshing ---------------------------------------------------------------------------------

/// Refresh the managed browser's cookies now.
pub async fn refresh_managed(st: &AppState) -> anyhow::Result<bool> {
    let data = st.data_dir().to_path_buf();
    let r = browser::refresh(&data).await;
    let t = now();
    match r {
        Ok(r) => {
            let account = if r.signed_in { account_of(st, &r.jar).await } else { None };
            update(&data, |s| {
                s.managed.checked_at = Some(t);
                s.managed.ok = Some(r.signed_in);
                if r.signed_in {
                    s.managed.updated_at = Some(t);
                    s.managed.message = None;
                    if account.is_some() {
                        s.managed.account = account.clone();
                    }
                } else {
                    s.managed.message = Some("Signed out – sign in to YouTube again".into());
                }
            });
            if r.signed_in {
                st.db.log("info", "YouTube sign-in refreshed by Encore's browser");
            } else {
                signed_out(st, Kind::Managed).await;
            }
            Ok(r.signed_in)
        }
        Err(e) => {
            update(&data, |s| {
                s.managed.checked_at = Some(t);
                s.managed.message = Some(format!("Refresh failed: {e}"));
            });
            st.db.log("warning", &format!("YouTube browser refresh failed: {e}"));
            Err(e)
        }
    }
}

/// Light refresh of the pasted cookies; the renewed jar is saved back to the setting. With `rotate` false
/// (cookies set by YTDLP_COOKIES, which Encore can't save back, or the light refresh switched off) they are
/// only checked: rotating a session that can't be stored would leave the stored one superseded.
pub async fn refresh_pasted(st: &AppState, rotate: bool) -> anyhow::Result<bool> {
    let raw = st.cfg.str("ytdlp_cookies");
    let mut j = Jar::parse_netscape(&raw);
    if !j.has_auth(now() as i64) {
        anyhow::bail!("The pasted cookies have no YouTube sign-in");
    }
    let t = now();
    let rotate = rotate && !st.cfg.env_locked("ytdlp_cookies");
    let outcome = if rotate { rotate::run(&st.http, &mut j, t as i64).await } else { rotate::Outcome::Rotated(0) };
    // Google renewed them: saved whatever the check below says (the old values are superseded now)
    if matches!(outcome, rotate::Outcome::Rotated(n) if n > 0) {
        st.cfg.set_many(&json!({ "ytdlp_cookies": j.to_netscape() }));
    }
    // None: YouTube couldn't be reached – nothing is known, nothing changes and nobody is told
    let ok = match &outcome {
        rotate::Outcome::SignedOut => Some(false),
        _ => verify(st, &j).await.ok(),
    };
    let Some(ok) = ok else {
        update(st.data_dir(), |s| {
            s.pasted.checked_at = Some(t);
            s.pasted.message = Some("Couldn't reach YouTube to check the cookies – trying again later".into());
        });
        anyhow::bail!("Couldn't reach YouTube to check the cookies");
    };
    let msg = match (&outcome, ok) {
        (_, true) => None,
        (rotate::Outcome::SignedOut, _) => Some("Google signed these cookies out – paste fresh ones or use another way to sign in".to_string()),
        (rotate::Outcome::Failed(e), _) => Some(format!("Stale: {e}")),
        _ => Some("Stale: YouTube no longer accepts these cookies".to_string()),
    };
    let account = if ok { account_of(st, &j).await } else { None };
    update(st.data_dir(), |s| {
        s.pasted.checked_at = Some(t);
        s.pasted.ok = Some(ok);
        s.pasted.message = msg;
        if ok {
            s.pasted.updated_at = Some(t);
            if account.is_some() {
                s.pasted.account = account;
            }
        }
    });
    if !ok {
        signed_out(st, Kind::Pasted).await;
    }
    Ok(ok)
}

/// Check the extension's cookies still sign in (never rotated by Encore).
pub async fn check_extension(st: &AppState) -> anyhow::Result<bool> {
    let j = jar_of(st, Kind::Extension);
    let ok = verify(st, &j).await?;
    let account = if ok { account_of(st, &j).await } else { None };
    update(st.data_dir(), |s| {
        s.extension.checked_at = Some(now());
        s.extension.ok = Some(ok);
        s.extension.message = (!ok).then(|| "The browser's cookies no longer sign in – open YouTube in that browser".to_string());
        if account.is_some() {
            s.extension.account = account;
        }
    });
    Ok(ok)
}

/// Tell the admins once that YouTube signed out (and nothing else is signed in).
async fn signed_out(st: &AppState, k: Kind) {
    if active(st).is_some_and(|a| a != k && load(st.data_dir()).get(a).ok != Some(false)) {
        return; // another source still works
    }
    let first = update(st.data_dir(), |s| !std::mem::replace(&mut s.signed_out_notified, true));
    if first {
        let msg = format!("{} is no longer signed in to YouTube. Downloads from YouTube Music still work, at lower quality and with more bot checks, until you sign in again under Settings → YouTube sign-in.", k.label());
        st.db.log("warning", &msg);
        crate::notify::send(st, "YouTube signed out", &msg, None).await;
    }
}

fn signed_in_again(st: &AppState) {
    update(st.data_dir(), |s| s.signed_out_notified = false);
}

/// yt-dlp was asked to sign in / to prove it is not a bot: refresh the managed browser once.
/// true → cookies were refreshed, try again.
pub async fn after_ytdlp_auth_error(st: &AppState) -> bool {
    let s = load(st.data_dir());
    if !managed_ready(st, &s) || active(st) != Some(Kind::Managed) {
        return false;
    }
    // at most once per 10 minutes, however many downloads fail together
    if s.managed.checked_at.is_some_and(|c| now() - c < 600.0) {
        return false;
    }
    matches!(refresh_managed(st).await, Ok(true))
}

/// Background loop: managed refresh (48 h / expiry), light refresh of pasted cookies (4 h).
pub async fn supervisor(st: AppState) {
    tokio::time::sleep(Duration::from_secs(90)).await;
    loop {
        tick(&st).await;
        tokio::time::sleep(TICK).await;
    }
}

async fn tick(st: &AppState) {
    let t = now();
    let s = load(st.data_dir());
    if managed_ready(st, &s) {
        let expiry = jar_of(st, Kind::Managed).auth_expiry(t as i64);
        // a refresh that failed (not a sign-out: the browser didn't start, no network) is tried again after 2 h
        let failed = s.managed.message.is_some() && s.managed.ok != Some(false) && s.managed.checked_at.is_some_and(|c| t - c >= 7200.0);
        if failed || managed_due(t, newest(s.managed.checked_at, s.managed.updated_at), expiry) {
            if let Ok(true) = refresh_managed(st).await {
                signed_in_again(st);
            }
        }
    }
    let s = load(st.data_dir());
    let act = active(st);
    if light_due(t, s.pasted.checked_at, act, st.cfg.bool("youtube_light_refresh") && !st.cfg.env_locked("ytdlp_cookies")) {
        if let Ok(true) = refresh_pasted(st, true).await {
            signed_in_again(st);
        }
    }
}

// ---- status -------------------------------------------------------------------------------------

/// Settings → YouTube sign-in.
pub fn status(st: &AppState) -> Value {
    let data = st.data_dir();
    let s = load(data);
    let t = now() as i64;
    let cands = candidates(st, &s);
    let act = choose(&cands, t);
    let installed = browser::installed(data);
    let sources: serde_json::Map<String, Value> = cands.iter().map(|c| {
        let st8 = &c.state;
        (c.kind.key().to_string(), json!({
            "label": c.kind.label(), "has_cookies": !c.jar.is_empty(), "signed_in": c.jar.has_auth(t) && st8.ok != Some(false),
            "expires": c.jar.auth_expiry(t), "updated_at": st8.updated_at, "checked_at": st8.checked_at, "ok": st8.ok,
            "account": st8.account, "message": st8.message, "agent": st8.agent,
        }))
    }).collect();
    let light_on = st.cfg.bool("youtube_light_refresh");
    let next_check = match act {
        Some(Kind::Managed) => {
            let last = newest(s.managed.checked_at, s.managed.updated_at).unwrap_or(now());
            let exp = jar_of(st, Kind::Managed).auth_expiry(t).map(|e| e as f64 - EXPIRY_MARGIN);
            Some(exp.map_or(last + MANAGED_EVERY, |e| e.min(last + MANAGED_EVERY)).max(now()))
        }
        Some(Kind::Pasted) if light_on && !st.cfg.env_locked("ytdlp_cookies") => Some(s.pasted.checked_at.map_or(now(), |c| c + LIGHT_EVERY).max(now())),
        _ => None,
    };
    let current = act.map(|k| s.get(k).clone()).unwrap_or_default();
    let diag = diagnostics(st, act, &s, installed.is_some());
    json!({
        "source": act.map(|k| k.key()), "source_label": act.map(|k| k.label()), "account": current.account,
        "last_refresh": current.updated_at, "next_check": next_check, "sources": sources,
        "browser": { "installed": installed.as_ref().map(|(_, v)| v.clone()), "signed_in_once": s.managed.updated_at.is_some() },
        "extension": { "token_at": s.ext_token_at, "connected": s.extension.updated_at.is_some() },
        "light_refresh": light_on, "diagnostics": diag,
    })
}

fn ago(secs: f64) -> String {
    let s = (now() - secs).max(0.0);
    if s < 90.0 { "just now".into() } else if s < 5400.0 { format!("{} min ago", (s / 60.0).round()) } else if s < 129600.0 { format!("{} h ago", (s / 3600.0).round()) } else { format!("{} days ago", (s / 86400.0).round()) }
}

/// One plain line about what is going on.
fn diagnostics(st: &AppState, act: Option<Kind>, s: &AuthState, has_browser: bool) -> String {
    let mut parts = vec![];
    match act {
        None => parts.push("No YouTube cookies – yt-dlp downloads signed out (128 kbps, more bot checks)".to_string()),
        Some(k) => {
            let src = s.get(k);
            let jar = jar_of(st, k);
            let t = now() as i64;
            parts.push(format!("{} in use", k.label()));
            match jar.auth_expiry(t) {
                Some(e) => parts.push(format!("sign-in cookies valid for {} more days", ((e - t) as f64 / 86400.0).floor().max(0.0))),
                None if jar.has_auth(t) => parts.push("session cookies only".into()),
                None => parts.push("no sign-in cookies".into()),
            }
            if let Some(c) = src.checked_at {
                parts.push(format!("checked {}{}", ago(c), match src.ok { Some(true) => ": signed in", Some(false) => ": signed out", None => "" }));
            }
            if let Some(m) = &src.message {
                parts.push(m.clone());
            }
            if k == Kind::Extension {
                parts.push("Encore does not rotate these cookies (that would sign your browser out)".into());
            }
        }
    }
    if !has_browser && act != Some(Kind::Managed) {
        parts.push("Install the browser under Download tools to let Encore sign in by itself".into());
    }
    parts.join(" · ")
}

// ---- routes -------------------------------------------------------------------------------------

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/youtube/auth", get(get_status))
        .route("/api/youtube/refresh", post(refresh_now))
        .route("/api/youtube/signout", post(sign_out))
        .route("/api/youtube/extension-token", post(new_token))
        .route("/api/youtube/extension-cookies", post(extension_cookies).options(preflight))
        .route("/api/youtube/browser/live", get(live))
        .route("/api/youtube/guided/start", post(guided_start))
        .route("/api/youtube/guided/step", post(guided_step))
        .route("/api/youtube/guided/cancel", post(guided_cancel))
}

// ---- guided sign-in (guided.rs) -----------------------------------------------------------------

/// Start the guided sign-in: answers the first step ({kind, title, text, error, choices, code_hint,
/// screenshot?, account?}).
async fn guided_start(State(st): State<AppState>, Admin(u): Admin) -> ApiResult<Json<Value>> {
    use crate::db::RowExt;
    st.db.log("info", &format!("{} started the YouTube sign-in", u.s("username")));
    let step = guided::start(&st).await.map_err(|(c, m)| err(c, m))?;
    Ok(Json(json!(step)))
}

/// One step: {kind: "email"|"password"|"code"|"choice"|"check", value}. The value is never logged or kept.
async fn guided_step(State(st): State<AppState>, Admin(_): Admin, Json(body): Json<guided::Submit>) -> ApiResult<Json<Value>> {
    let step = guided::step(&st, &body.kind, &body.value).await.map_err(|(c, m)| err(c, m))?;
    drop(body);
    Ok(Json(json!(step)))
}

async fn guided_cancel(State(_st): State<AppState>, Admin(_): Admin) -> ApiResult<Json<Value>> {
    let closed = guided::cancel().await;
    Ok(Json(json!({ "ok": true, "closed": closed })))
}

async fn get_status(State(st): State<AppState>, Admin(_): Admin) -> ApiResult<Json<Value>> {
    Ok(Json(tokio::task::spawn_blocking(move || status(&st)).await.map_err(|e| err(500, e.to_string()))?))
}

/// "Refresh now": the source in use (the managed browser opens YouTube; pasted cookies are rotated;
/// the extension's are only checked).
async fn refresh_now(State(st): State<AppState>, Admin(u): Admin) -> ApiResult<Json<Value>> {
    use crate::db::RowExt;
    let s = load(st.data_dir());
    let act = active(&st);
    let res = if managed_ready(&st, &s) && (act == Some(Kind::Managed) || act.is_none() || s.managed.ok == Some(false)) {
        refresh_managed(&st).await
    } else {
        match act {
            Some(Kind::Extension) => check_extension(&st).await,
            // rotated only with the light refresh on (and never cookies from the environment)
            Some(Kind::Pasted) => refresh_pasted(&st, st.cfg.bool("youtube_light_refresh")).await,
            Some(Kind::Managed) => refresh_managed(&st).await,
            None => return Err(err(400, "No YouTube sign-in to refresh – sign in with Encore's browser, connect the extension or paste cookies")),
        }
    };
    let ok = res.map_err(|e| err(400, e.to_string()))?;
    if ok {
        signed_in_again(&st);
    }
    st.db.log("info", &format!("{} refreshed the YouTube sign-in ({})", u.s("username"), if ok { "signed in" } else { "signed out" }));
    let st2 = st.clone();
    let mut v = tokio::task::spawn_blocking(move || status(&st2)).await.map_err(|e| err(500, e.to_string()))?;
    v["refreshed"] = json!(ok);
    Ok(Json(v))
}

#[derive(Deserialize, Default)]
struct SignOutBody {
    /// "managed" | "extension" | "all" (default: managed and extension; pasted cookies have their own clear)
    source: Option<String>,
}

async fn sign_out(State(st): State<AppState>, Admin(u): Admin, body: Option<Json<SignOutBody>>) -> ApiResult<Json<Value>> {
    use crate::db::RowExt;
    let which = body.and_then(|b| b.0.source).unwrap_or_else(|| "all".into());
    let data = st.data_dir().to_path_buf();
    if which == "managed" || which == "all" {
        guided::cancel().await;
        let _g = tokio::time::timeout(Duration::from_secs(60), browser::LOCK.lock()).await.map_err(|_| err(409, "The browser is busy – try again in a minute"))?;
        let _ = std::fs::remove_dir_all(browser::profile_dir(&data));
        let _ = std::fs::remove_file(managed_path(&data));
        update(&data, |s| s.managed = SourceState::default());
    }
    if which == "extension" || which == "all" {
        let _ = std::fs::remove_file(ext_path(&data));
        update(&data, |s| {
            s.extension = SourceState::default();
            s.ext_token = None;
            s.ext_token_at = None;
        });
    }
    st.db.log("info", &format!("{} signed Encore out of YouTube ({which})", u.s("username")));
    let st2 = st.clone();
    Ok(Json(tokio::task::spawn_blocking(move || status(&st2)).await.map_err(|e| err(500, e.to_string()))?))
}

fn token_hash(t: &str) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(t.trim().as_bytes()))
}

/// A new extension token (shown once; the old one stops working). It can only upload cookies.
async fn new_token(State(st): State<AppState>, Admin(u): Admin) -> ApiResult<Json<Value>> {
    use crate::db::RowExt;
    let token = format!("eyt_{}", crate::auth::random_hex(24));
    let h = token_hash(&token);
    update(st.data_dir(), |s| {
        s.ext_token = Some(h);
        s.ext_token_at = Some(now());
    });
    st.db.log("info", &format!("{} made a new YouTube extension token", u.s("username")));
    Ok(Json(json!({ "token": token })))
}

/// The extension's token from `Authorization: Bearer …` (or `X-Encore-Token`), checked in constant time.
pub fn token_ok(headers: &HeaderMap, stored: Option<&str>) -> bool {
    use subtle::ConstantTimeEq;
    let Some(stored) = stored else { return false };
    let given = headers.get("authorization").and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer ")).map(String::from)
        .or_else(|| headers.get("x-encore-token").and_then(|v| v.to_str().ok()).map(String::from));
    let Some(given) = given.filter(|g| !g.trim().is_empty()) else { return false };
    token_hash(&given).as_bytes().ct_eq(stored.as_bytes()).into()
}

async fn preflight() -> Response {
    let mut r = Response::new(axum::body::Body::empty());
    cors(r.headers_mut());
    r
}

fn cors(h: &mut HeaderMap) {
    use axum::http::HeaderValue;
    h.insert("access-control-allow-origin", HeaderValue::from_static("*"));
    h.insert("access-control-allow-methods", HeaderValue::from_static("POST, OPTIONS"));
    h.insert("access-control-allow-headers", HeaderValue::from_static("authorization, content-type, x-encore-token"));
}

/// The browser extension's upload: `{cookies: [chrome.cookies.Cookie…], agent?}` with the token.
async fn extension_cookies(State(st): State<AppState>, headers: HeaderMap, Json(body): Json<Value>) -> Response {
    use axum::response::IntoResponse;
    let s = load(st.data_dir());
    let mut resp = if !token_ok(&headers, s.ext_token.as_deref()) {
        err(401, "Unknown token – make a new one under Settings → YouTube sign-in → Connect browser extension").into_response()
    } else {
        match accept_extension(&st, &body).await {
            Ok(v) => Json(v).into_response(),
            Err(e) => err(400, e.to_string()).into_response(),
        }
    };
    cors(resp.headers_mut());
    resp
}

async fn accept_extension(st: &AppState, body: &Value) -> anyhow::Result<Value> {
    let list = body["cookies"].as_array().cloned().unwrap_or_default();
    if list.len() > 2000 {
        anyhow::bail!("too many cookies");
    }
    let j = Jar::from_extension(&list);
    let t = now();
    if !j.has_auth(t as i64) {
        update(st.data_dir(), |s| {
            s.extension.checked_at = Some(t);
            s.extension.ok = Some(false);
            s.extension.message = Some("The browser is not signed in to YouTube".into());
            s.extension.agent = body["agent"].as_str().map(|a| a.chars().take(80).collect());
        });
        return Ok(json!({ "ok": false, "message": "This browser is not signed in to YouTube – sign in at music.youtube.com" }));
    }
    // another Google account in that browser: its session cookies differ (they stay the same between
    // uploads of one session), so the account name is looked up again
    let session = |jar: &Jar| ["SAPISID", "__Secure-3PSID", "SID"].map(|n| jar.get("www.youtube.com", n).map(|c| c.value.clone()));
    let before = session(&jar_of(st, Kind::Extension));
    browser::write_private(&ext_path(st.data_dir()), &j.to_netscape())?;
    let switched = before != session(&j);
    let changed_account = load(st.data_dir()).extension.account.is_none() || switched;
    let account = if changed_account { account_of(st, &j).await } else { None };
    update(st.data_dir(), |s| {
        s.extension.updated_at = Some(t);
        s.extension.checked_at = Some(t);
        s.extension.ok = Some(true);
        s.extension.message = None;
        s.extension.agent = body["agent"].as_str().map(|a| a.chars().take(80).collect());
        // a switched account never keeps the old name, even when the new one couldn't be read
        if account.is_some() || switched {
            s.extension.account = account;
        }
    });
    signed_in_again(st);
    Ok(json!({ "ok": true, "cookies": j.0.len(), "message": format!("{} cookies received", j.0.len()) }))
}

// ---- live view ----------------------------------------------------------------------------------

/// Admin-only WebSocket: the managed browser's screen as JPEG frames, mouse and keys back. With a
/// guided sign-in open (guided.rs) it shows that browser as it is, and closing it hands it back to the form.
/// Server → client: {type:"frame", data, width, height} · {type:"url", url} · {type:"status", message}
/// · {type:"done", account} · {type:"error", message}. Client → server: {type:"mouse"|"key"|"text"|"nav", …}
/// (see browser::input_commands) and {type:"check"} (look for a finished sign-in now).
async fn live(ws: WebSocketUpgrade, State(st): State<AppState>, headers: HeaderMap, Admin(u): Admin) -> Response {
    use crate::db::RowExt;
    use axum::response::IntoResponse;
    // a GET skips the CSRF header check and the session cookie is SameSite=Lax: only Encore's own pages
    // may open the browser (not another site on a sibling subdomain)
    if !same_origin(&headers, &st.cfg.str("base_url")) {
        return err(403, "The sign-in browser can only be opened from Encore itself").into_response();
    }
    st.db.log("info", &format!("{} opened the YouTube sign-in browser", u.s("username")));
    ws.max_message_size(1 << 20).on_upgrade(move |sock| live_session(st, sock))
}

/// The request's Origin is this server (its Host, the proxy's X-Forwarded-Host or the Base URL). No Origin
/// at all is not a browser page, so no session cookie rides along unasked.
pub fn same_origin(headers: &HeaderMap, base_url: &str) -> bool {
    let h = |k: &str| headers.get(k).and_then(|v| v.to_str().ok()).map(|v| v.trim().to_lowercase());
    let Some(origin) = h("origin") else { return true };
    let Some(o) = reqwest::Url::parse(&origin).ok().and_then(|u| u.host_str().map(|host| match u.port() {
        Some(p) => format!("{host}:{p}"),
        None => host.to_string(),
    })) else { return false };
    let base = reqwest::Url::parse(base_url.trim()).ok().and_then(|u| u.host_str().map(|host| match u.port() {
        Some(p) => format!("{host}:{p}"),
        None => host.to_string(),
    }));
    let mut ok: Vec<String> = [h("host"), h("x-forwarded-host"), base].into_iter().flatten()
        .flat_map(|v| v.split(',').map(|x| x.trim().to_string()).collect::<Vec<_>>()).filter(|v| !v.is_empty()).collect();
    // a Host with the scheme's default port written out
    for v in ok.clone() {
        if let Some(bare) = v.strip_suffix(":443").or(v.strip_suffix(":80")) {
            ok.push(bare.to_string());
        }
    }
    ok.contains(&o)
}

async fn send(sock: &mut WebSocket, v: Value) -> bool {
    sock.send(WsMessage::Text(v.to_string().into())).await.is_ok()
}

async fn live_session(st: AppState, mut sock: WebSocket) {
    let data = st.data_dir().to_path_buf();
    // the guided sign-in's browser, as it is (its page stays where the form left it)
    if let Some((id, b)) = guided::attach().await {
        // the page tells Close to go back to the guided form
        send(&mut sock, json!({ "type": "guided" })).await;
        let res = drive(&st, &b, &mut sock, false).await;
        let signed_in = matches!(res, Ok(Some(_)));
        guided::detach(id, signed_in).await;
        match res {
            Ok(Some(account)) => {
                signed_in_again(&st);
                st.db.log("info", &format!("Encore's browser is signed in to YouTube{}", account.as_deref().map(|a| format!(" as {a}")).unwrap_or_default()));
                send(&mut sock, json!({ "type": "done", "account": account })).await;
            }
            Ok(None) => {}
            Err(e) => {
                send(&mut sock, json!({ "type": "error", "message": e.to_string() })).await;
            }
        }
        let _ = sock.send(WsMessage::Close(None)).await;
        return;
    }
    let Ok(_g) = browser::LOCK.try_lock() else {
        send(&mut sock, json!({ "type": "error", "message": "The browser is busy refreshing – try again in a minute" })).await;
        return;
    };
    send(&mut sock, json!({ "type": "status", "message": "Starting the browser…" })).await;
    let b = match browser::Browser::launch(&data).await {
        Ok(b) => b,
        Err(e) => {
            send(&mut sock, json!({ "type": "error", "message": e.to_string() })).await;
            return;
        }
    };
    let res = drive(&st, &b, &mut sock, true).await;
    b.close().await;
    match res {
        Ok(Some(account)) => {
            signed_in_again(&st);
            st.db.log("info", &format!("Encore's browser is signed in to YouTube{}", account.as_deref().map(|a| format!(" as {a}")).unwrap_or_default()));
            send(&mut sock, json!({ "type": "done", "account": account })).await;
        }
        Ok(None) => {}
        Err(e) => {
            send(&mut sock, json!({ "type": "error", "message": e.to_string() })).await;
        }
    }
    let _ = sock.send(WsMessage::Close(None)).await;
}

/// Run the live view until the sign-in is done (Some(account)), the viewer leaves (None) or 15 minutes pass.
/// `fresh`: a browser of its own (open YouTube Music / Google's sign-in first); else the guided
/// sign-in's browser, left on its page.
async fn drive(st: &AppState, b: &browser::Browser, sock: &mut WebSocket, fresh: bool) -> anyhow::Result<Option<Option<String>>> {
    let mut ev = b.cdp.subscribe();
    b.page("Page.startScreencast", json!({ "format": "jpeg", "quality": 70, "maxWidth": 1280, "maxHeight": 860, "everyNthFrame": 1 })).await?;
    if fresh {
        // already signed in (a profile from before)? Then YouTube Music; else Google's sign-in page
        b.navigate("https://music.youtube.com/").await?;
        b.wait_load(Duration::from_secs(20)).await;
        tokio::time::sleep(Duration::from_secs(2)).await;
        if !b.logged_in().await {
            b.navigate(browser::SIGN_IN_URL).await?;
        }
    } else {
        send(sock, json!({ "type": "url", "url": b.url().await })).await;
        // a first frame now (the screencast sends frames only when the page paints)
        if let Ok(r) = b.page("Page.captureScreenshot", json!({ "format": "jpeg", "quality": 70 })).await {
            send(sock, json!({ "type": "frame", "data": r["data"], "width": 1280, "height": 860 })).await;
        }
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(900);
    let mut check_at: Option<tokio::time::Instant> = Some(tokio::time::Instant::now() + Duration::from_secs(1));
    loop {
        let wake = check_at.unwrap_or(deadline).min(deadline);
        tokio::select! {
            e = ev.recv() => {
                let Ok(e) = e else { continue };
                match e["method"].as_str().unwrap_or("") {
                    "Page.screencastFrame" => {
                        let p = &e["params"];
                        let _ = b.page("Page.screencastFrameAck", json!({ "sessionId": p["sessionId"] })).await;
                        let (w, h) = (p["metadata"]["deviceWidth"].as_f64().unwrap_or(1280.0), p["metadata"]["deviceHeight"].as_f64().unwrap_or(860.0));
                        if !send(sock, json!({ "type": "frame", "data": p["data"], "width": w, "height": h })).await {
                            return Ok(None);
                        }
                    }
                    "Page.frameNavigated" => {
                        let f = &e["params"]["frame"];
                        if f["parentId"].is_null() {
                            let url = f["url"].as_str().unwrap_or("").to_string();
                            send(sock, json!({ "type": "url", "url": url })).await;
                            if url.starts_with("https://music.youtube.com") || url.starts_with("https://www.youtube.com") {
                                check_at = Some(tokio::time::Instant::now() + Duration::from_secs(3));
                            }
                        }
                    }
                    _ => {}
                }
            }
            m = sock.recv() => {
                let Some(Ok(m)) = m else { return Ok(None) };
                let WsMessage::Text(t) = m else {
                    if matches!(m, WsMessage::Close(_)) { return Ok(None) }
                    continue;
                };
                let Ok(v) = serde_json::from_str::<Value>(t.as_str()) else { continue };
                if v["type"] == "check" {
                    check_at = Some(tokio::time::Instant::now());
                    continue;
                }
                for (method, params) in browser::input_commands(&v) {
                    let _ = b.page(method, params).await;
                }
            }
            _ = tokio::time::sleep_until(wake) => {
                if tokio::time::Instant::now() >= deadline {
                    anyhow::bail!("The sign-in window closed after 15 minutes – open it again to finish");
                }
                check_at = None;
                if b.url().await.starts_with("https://music.youtube.com") && b.logged_in().await {
                    send(sock, json!({ "type": "status", "message": "Signed in – saving the cookies…" })).await;
                    if let Some(account) = guided::save_signin(st, b).await? {
                        return Ok(Some(account));
                    }
                }
            }
        }
    }
}
