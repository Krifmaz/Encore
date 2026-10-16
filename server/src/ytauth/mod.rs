//! YouTube sign-in for yt-dlp, three ways:
//!
//! * **handoff** (server-owned): the Encore browser extension opens a new private window, the owner signs
//!   in to Google there, and the extension sends that window's cookies with `handoff: true`, then closes
//!   the window without signing out (a private window's cookies are thrown away, so no browser ever uses
//!   that session again). From then on the session belongs to the server alone: Encore refreshes it itself
//!   every 4 h (rotate.rs: RotateCookies + a signed-in check on music.youtube.com) and when yt-dlp is
//!   asked to sign in or to prove it is not a bot.
//! * **extension**: the extension syncs the cookies of the owner's everyday browser. Encore must never
//!   rotate these itself: that would sign the browser out.
//! * **pasted**: a cookies.txt pasted under Settings (`ytdlp_cookies`), light-refreshed every 4 h while
//!   the switch is on, the renewed cookies saved back.
//!
//! Precedence handoff > extension > pasted among the sources that are signed in; with none signed in,
//! the freshest one is still handed to yt-dlp. Cookie values never go into logs or answers.
pub mod jar;
pub mod rotate;
#[cfg(test)]
mod tests;

use crate::auth::Admin;
use crate::db::now;
use crate::error::{err, ApiResult};
use crate::state::AppState;
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

/// Server-owned (hand-off) and pasted cookies: refresh this often.
pub const LIGHT_EVERY: f64 = 4.0 * 3600.0;
/// A refresh that failed (YouTube not reachable – not a sign-out) is tried again after this.
pub const RETRY_AFTER: f64 = 1800.0;
/// How often the supervisor looks.
const TICK: Duration = Duration::from_secs(600);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// cookies handed over by the extension from a private window: the server's own session
    #[serde(alias = "managed")]
    Handoff,
    Extension,
    Pasted,
}

impl Kind {
    pub const ALL: [Kind; 3] = [Kind::Handoff, Kind::Extension, Kind::Pasted];
    pub fn key(self) -> &'static str {
        match self {
            Kind::Handoff => "handoff",
            Kind::Extension => "extension",
            Kind::Pasted => "pasted",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Kind::Handoff => "Handed-over sign-in",
            Kind::Extension => "Browser extension (everyday browser)",
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

/// `<config>/youtube-browser/state.json` (the folder keeps its old name: installs from before keep their state)
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AuthState {
    /// the server-owned session (`managed` before 0.25.3, when it came from Encore's own browser)
    #[serde(alias = "managed")]
    pub handoff: SourceState,
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
            Kind::Handoff => &self.handoff,
            Kind::Extension => &self.extension,
            Kind::Pasted => &self.pasted,
        }
    }
}

static STATE_LOCK: Mutex<()> = Mutex::new(());

/// `<config>/youtube-browser`: the state and the cookies of the hand-off and the extension.
pub fn home(data: &Path) -> PathBuf {
    data.join("youtube-browser")
}

fn state_path(data: &Path) -> PathBuf {
    home(data).join("state.json")
}

/// Write a file only Encore can read (cookies, the state), atomically.
pub fn write_private(path: &Path, text: &str) -> anyhow::Result<()> {
    use std::io::Write;
    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p)?;
    }
    let tmp = path.with_extension("tmp");
    let _ = std::fs::remove_file(&tmp);
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    o.mode(0o600);
    let mut f = o.open(&tmp)?;
    f.write_all(text.as_bytes())?;
    f.sync_all()?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// 0.25.3 removed Encore's own sign-in browser (Google refuses sign-ins in it): the program a previous
/// install downloaded (`<config>/tools/browser`) and its profile (`<config>/youtube-browser/profile`) are
/// deleted once. Answers what was removed (empty: nothing was there).
pub fn remove_old_browser(data: &Path) -> Vec<PathBuf> {
    let mut gone = vec![];
    for d in [data.join("tools").join("browser"), home(data).join("profile")] {
        if d.exists() && std::fs::remove_dir_all(&d).is_ok() {
            gone.push(d);
        }
    }
    gone
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
        if let Err(e) = write_private(&state_path(data), &t) {
            tracing::warn!("saving the YouTube sign-in state failed: {e}");
        }
    }
    r
}

fn ext_path(data: &Path) -> PathBuf {
    home(data).join("extension-cookies.txt")
}

/// The server-owned session (the file the old managed browser exported to: what it held is server-only too).
fn handoff_path(data: &Path) -> PathBuf {
    home(data).join("cookies.txt")
}

/// The cookies of one source (empty when it has none).
pub fn jar_of(st: &AppState, k: Kind) -> Jar {
    match k {
        Kind::Handoff => std::fs::read_to_string(handoff_path(st.data_dir())).map(|t| Jar::parse_netscape(&t)).unwrap_or_default(),
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

/// Server-owned (hand-off) cookies: refreshed every 4 h while there are any, and 30 min after a refresh
/// that could not reach YouTube. `last`: the last check; `failed`: that check could not reach YouTube.
pub fn handoff_due(now: f64, has_cookies: bool, last: Option<f64>, failed: bool) -> bool {
    has_cookies && last.is_none_or(|l| now - l >= LIGHT_EVERY || (failed && now - l >= RETRY_AFTER))
}

/// Light refresh of pasted cookies: only while they are the source in use (never the extension's,
/// never while the hand-off session is in use), every 4 h, with the switch on.
pub fn light_due(now: f64, last: Option<f64>, active: Option<Kind>, enabled: bool) -> bool {
    enabled && active == Some(Kind::Pasted) && last.is_none_or(|l| now - l >= LIGHT_EVERY)
}

/// When the source in use is refreshed next (None: never by Encore – the extension's cookies, or the
/// light refresh switched off).
pub fn next_refresh(now: f64, act: Option<Kind>, s: &AuthState, pasted_rotates: bool) -> Option<f64> {
    let after = |src: &SourceState| {
        let failed = src.message.is_some() && src.ok != Some(false);
        Some(src.checked_at.map_or(now, |c| c + if failed { RETRY_AFTER } else { LIGHT_EVERY }).max(now))
    };
    match act {
        Some(Kind::Handoff) => after(&s.handoff),
        Some(Kind::Pasted) if pasted_rotates => after(&s.pasted),
        _ => None,
    }
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

/// One rotation at a time (two at once would each supersede the other's cookies), and a hand-off arriving
/// meanwhile waits for it rather than being overwritten by the older session's renewed cookies.
static ROTATING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Light refresh (rotate.rs) of the server-owned session; the renewed cookies are saved back. Answers
/// whether it is still signed in.
pub async fn refresh_handoff(st: &AppState) -> anyhow::Result<bool> {
    let _g = ROTATING.lock().await;
    let data = st.data_dir().to_path_buf();
    let mut j = jar_of(st, Kind::Handoff);
    let t = now();
    if j.is_empty() {
        anyhow::bail!("No handed-over sign-in – press Hand over a YouTube sign-in in the Encore extension");
    }
    let outcome = rotate::run(&st.http, &mut j, t as i64).await;
    if matches!(outcome, rotate::Outcome::Rotated(n) if n > 0) {
        write_private(&handoff_path(&data), &j.to_netscape())?;
    }
    let ok = match &outcome {
        rotate::Outcome::SignedOut => Some(false),
        _ => verify(st, &j).await.ok(),
    };
    let Some(ok) = ok else {
        update(&data, |s| {
            s.handoff.checked_at = Some(t);
            s.handoff.message = Some("Couldn't reach YouTube to refresh the sign-in – trying again in 30 min".into());
        });
        anyhow::bail!("Couldn't reach YouTube to refresh the sign-in");
    };
    let account = if ok { account_of(st, &j).await } else { None };
    update(&data, |s| {
        s.handoff.checked_at = Some(t);
        s.handoff.ok = Some(ok);
        s.handoff.message = (!ok).then(|| "Signed out – do the hand-off again from the Encore extension".to_string());
        if ok {
            s.handoff.updated_at = Some(t);
            if account.is_some() {
                s.handoff.account = account;
            }
        }
    });
    if ok {
        st.db.log("info", "YouTube sign-in refreshed by Encore (handed-over session)");
        signed_in_again(st);
    } else {
        st.db.log("warning", "The handed-over YouTube sign-in was signed out");
        signed_out(st, Kind::Handoff).await;
    }
    Ok(ok)
}

/// Light refresh of the pasted cookies; the renewed jar is saved back to the setting. With `rotate` false
/// (cookies set by YTDLP_COOKIES, which Encore can't save back, or the light refresh switched off) they are
/// only checked: rotating a session that can't be stored would leave the stored one superseded.
pub async fn refresh_pasted(st: &AppState, rotate: bool) -> anyhow::Result<bool> {
    let _g = ROTATING.lock().await;
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
        (rotate::Outcome::SignedOut, _) => Some("Google signed these cookies out – paste fresh ones or do the hand-off from the Encore extension".to_string()),
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
    if ok {
        signed_in_again(st);
    } else {
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
    if ok {
        signed_in_again(st);
    }
    Ok(ok)
}

/// What the admins are told when YouTube signed Encore out.
pub fn signed_out_message(k: Kind) -> String {
    let what = match k {
        Kind::Handoff => "The YouTube sign-in handed over from the Encore extension has ended",
        Kind::Extension => "The browser the Encore extension syncs is no longer signed in to YouTube",
        Kind::Pasted => "The pasted YouTube cookies no longer sign in",
    };
    format!("{what}. Do the hand-off again from the Encore extension (Hand over a YouTube sign-in). Until then downloads from YouTube Music still work, at lower quality and with more bot checks.")
}

/// Tell the admins once that YouTube signed out (and nothing else is signed in).
async fn signed_out(st: &AppState, k: Kind) {
    if active(st).is_some_and(|a| a != k && load(st.data_dir()).get(a).ok != Some(false)) {
        return; // another source still works
    }
    let first = update(st.data_dir(), |s| !std::mem::replace(&mut s.signed_out_notified, true));
    if first {
        let msg = signed_out_message(k);
        st.db.log("warning", &msg);
        crate::notify::send(st, "YouTube signed out", &msg, None).await;
    }
}

fn signed_in_again(st: &AppState) {
    update(st.data_dir(), |s| s.signed_out_notified = false);
}

/// yt-dlp was asked to sign in / to prove it is not a bot: refresh the cookies Encore owns (the hand-off
/// session, or pasted cookies with the light refresh on) once. true → refreshed and signed in, try again.
pub async fn after_ytdlp_auth_error(st: &AppState) -> bool {
    let s = load(st.data_dir());
    let act = active(st);
    let src = match act {
        Some(Kind::Handoff) => &s.handoff,
        Some(Kind::Pasted) if st.cfg.bool("youtube_light_refresh") && !st.cfg.env_locked("ytdlp_cookies") => &s.pasted,
        _ => return false,
    };
    // at most once per 10 minutes, however many downloads fail together
    if src.checked_at.is_some_and(|c| now() - c < 600.0) {
        return false;
    }
    let r = if act == Some(Kind::Handoff) { refresh_handoff(st).await } else { refresh_pasted(st, true).await };
    matches!(r, Ok(true))
}

/// Background loop: refresh of the hand-off session and of pasted cookies (4 h).
pub async fn supervisor(st: AppState) {
    let gone = remove_old_browser(st.data_dir());
    if !gone.is_empty() {
        st.db.log("info", "Removed Encore's old YouTube sign-in browser and its profile (sign-in now goes through the Encore browser extension's hand-off)");
    }
    tokio::time::sleep(Duration::from_secs(90)).await;
    loop {
        tick(&st).await;
        tokio::time::sleep(TICK).await;
    }
}

async fn tick(st: &AppState) {
    let t = now();
    let s = load(st.data_dir());
    let failed = s.handoff.message.is_some() && s.handoff.ok != Some(false);
    if handoff_due(t, !jar_of(st, Kind::Handoff).is_empty(), s.handoff.checked_at, failed) {
        let _ = refresh_handoff(st).await;
    }
    let s = load(st.data_dir());
    if light_due(t, s.pasted.checked_at, active(st), st.cfg.bool("youtube_light_refresh") && !st.cfg.env_locked("ytdlp_cookies")) {
        let _ = refresh_pasted(st, true).await;
    }
}

// ---- status -------------------------------------------------------------------------------------

/// Settings → YouTube. Never holds a cookie value.
pub fn status(st: &AppState) -> Value {
    let data = st.data_dir();
    let s = load(data);
    let t = now() as i64;
    let cands = candidates(st, &s);
    let act = choose(&cands, t);
    let sources: serde_json::Map<String, Value> = cands.iter().map(|c| {
        let st8 = &c.state;
        (c.kind.key().to_string(), json!({
            "label": c.kind.label(), "has_cookies": !c.jar.is_empty(), "signed_in": c.jar.has_auth(t) && st8.ok != Some(false),
            "expires": c.jar.auth_expiry(t), "updated_at": st8.updated_at, "checked_at": st8.checked_at, "ok": st8.ok,
            "account": st8.account, "message": st8.message, "agent": st8.agent,
        }))
    }).collect();
    let light_on = st.cfg.bool("youtube_light_refresh");
    let next = next_refresh(now(), act, &s, light_on && !st.cfg.env_locked("ytdlp_cookies"));
    let current = act.map(|k| s.get(k).clone()).unwrap_or_default();
    json!({
        "source": act.map(|k| k.key()), "source_label": act.map(|k| k.label()), "account": current.account,
        "last_refresh": current.updated_at, "last_check": current.checked_at, "next_refresh": next, "next_check": next,
        "sources": sources,
        "extension": { "token_at": s.ext_token_at, "connected": s.extension.updated_at.is_some() || s.handoff.updated_at.is_some(), "handoff_at": s.handoff.updated_at },
        "light_refresh": light_on, "diagnostics": diagnostics(st, act, &s),
    })
}

fn ago(secs: f64) -> String {
    let s = (now() - secs).max(0.0);
    if s < 90.0 { "just now".into() } else if s < 5400.0 { format!("{} min ago", (s / 60.0).round()) } else if s < 129600.0 { format!("{} h ago", (s / 3600.0).round()) } else { format!("{} days ago", (s / 86400.0).round()) }
}

/// One plain line about what is going on.
fn diagnostics(st: &AppState, act: Option<Kind>, s: &AuthState) -> String {
    let mut parts = vec![];
    match act {
        None => parts.push("No YouTube cookies – yt-dlp downloads signed out (128 kbps, more bot checks). Hand over a sign-in from the Encore extension".to_string()),
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
            match k {
                Kind::Handoff => parts.push("Encore keeps this sign-in alive by itself (every 4 h)".into()),
                Kind::Extension => parts.push("Encore does not rotate these cookies (that would sign your browser out) – the hand-off is the better way".into()),
                Kind::Pasted => {}
            }
        }
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
}

async fn get_status(State(st): State<AppState>, Admin(_): Admin) -> ApiResult<Json<Value>> {
    Ok(Json(tokio::task::spawn_blocking(move || status(&st)).await.map_err(|e| err(500, e.to_string()))?))
}

/// "Refresh now": the source in use (the hand-off session and pasted cookies are rotated; the
/// extension's are only checked).
async fn refresh_now(State(st): State<AppState>, Admin(u): Admin) -> ApiResult<Json<Value>> {
    use crate::db::RowExt;
    let act = active(&st);
    let res = match act {
        Some(Kind::Handoff) => refresh_handoff(&st).await,
        Some(Kind::Extension) => check_extension(&st).await,
        // rotated only with the light refresh on (and never cookies from the environment)
        Some(Kind::Pasted) => refresh_pasted(&st, st.cfg.bool("youtube_light_refresh")).await,
        None => return Err(err(400, "No YouTube sign-in to refresh – hand one over from the Encore extension or paste cookies")),
    };
    let ok = res.map_err(|e| err(400, e.to_string()))?;
    st.db.log("info", &format!("{} refreshed the YouTube sign-in ({})", u.s("username"), if ok { "signed in" } else { "signed out" }));
    let st2 = st.clone();
    let mut v = tokio::task::spawn_blocking(move || status(&st2)).await.map_err(|e| err(500, e.to_string()))?;
    v["refreshed"] = json!(ok);
    Ok(Json(v))
}

#[derive(Deserialize, Default)]
struct SignOutBody {
    /// "handoff" | "extension" | "all" (default: all but the pasted cookies, which have their own clear)
    source: Option<String>,
}

async fn sign_out(State(st): State<AppState>, Admin(u): Admin, body: Option<Json<SignOutBody>>) -> ApiResult<Json<Value>> {
    use crate::db::RowExt;
    let which = body.and_then(|b| b.0.source).unwrap_or_else(|| "all".into());
    let which = if which == "managed" { "handoff".to_string() } else { which };
    if !["handoff", "extension", "all"].contains(&which.as_str()) {
        return Err(err(400, "Unknown sign-in"));
    }
    let data = st.data_dir().to_path_buf();
    if which == "handoff" || which == "all" {
        let _g = ROTATING.lock().await;
        let _ = std::fs::remove_file(handoff_path(&data));
        update(&data, |s| s.handoff = SourceState::default());
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

/// The browser extension's upload: `{cookies: [chrome.cookies.Cookie…], agent?, handoff?}` with the token.
/// `handoff: true`: the cookies of a private window signed in for Encore only – they become the
/// server-owned session. Otherwise: the everyday browser's cookies (never rotated by Encore).
async fn extension_cookies(State(st): State<AppState>, headers: HeaderMap, Json(body): Json<Value>) -> Response {
    use axum::response::IntoResponse;
    let s = load(st.data_dir());
    let mut resp = if !token_ok(&headers, s.ext_token.as_deref()) {
        err(401, "Unknown token – make a new one under Settings → YouTube → Connect the browser extension").into_response()
    } else {
        let r = if body["handoff"].as_bool() == Some(true) { accept_handoff(&st, &body).await } else { accept_extension(&st, &body).await };
        match r {
            Ok(v) => Json(v).into_response(),
            Err(e) => err(400, e.to_string()).into_response(),
        }
    };
    cors(resp.headers_mut());
    resp
}

fn cookie_list(body: &Value) -> anyhow::Result<Jar> {
    let list = body["cookies"].as_array().cloned().unwrap_or_default();
    if list.len() > 2000 {
        anyhow::bail!("too many cookies");
    }
    Ok(Jar::from_extension(&list))
}

fn agent_of(body: &Value) -> Option<String> {
    body["agent"].as_str().map(|a| a.chars().filter(|c| !c.is_control()).take(80).collect())
}

/// A hand-off: the private window's cookies become the server's own session.
async fn accept_handoff(st: &AppState, body: &Value) -> anyhow::Result<Value> {
    let j = cookie_list(body)?;
    let t = now();
    if !j.has_auth(t as i64) {
        return Ok(json!({ "ok": false, "message": "That window is not signed in to YouTube yet – finish signing in, then send again" }));
    }
    let account = account_of(st, &j).await;
    {
        let _g = ROTATING.lock().await;
        write_private(&handoff_path(st.data_dir()), &j.to_netscape())?;
        update(st.data_dir(), |s| {
            s.handoff = SourceState { updated_at: Some(t), checked_at: Some(t), ok: Some(true), account: account.clone(), message: None, agent: agent_of(body) };
        });
    }
    signed_in_again(st);
    st.db.log("info", &format!("YouTube sign-in handed over from the Encore extension{}{} – Encore keeps it alive by itself now",
        account.as_deref().map(|a| format!(" ({a})")).unwrap_or_default(),
        agent_of(body).map(|a| format!(", {a}")).unwrap_or_default()));
    Ok(json!({ "ok": true, "handoff": true, "cookies": j.0.len(), "account": account,
        "message": "Encore has its own YouTube sign-in now and keeps it alive by itself. The private window can close." }))
}

async fn accept_extension(st: &AppState, body: &Value) -> anyhow::Result<Value> {
    let j = cookie_list(body)?;
    let t = now();
    if !j.has_auth(t as i64) {
        update(st.data_dir(), |s| {
            s.extension.checked_at = Some(t);
            s.extension.ok = Some(false);
            s.extension.message = Some("The browser is not signed in to YouTube".into());
            s.extension.agent = agent_of(body);
        });
        return Ok(json!({ "ok": false, "message": "This browser is not signed in to YouTube – sign in at music.youtube.com" }));
    }
    // another Google account in that browser: its session cookies differ (they stay the same between
    // uploads of one session), so the account name is looked up again
    let session = |jar: &Jar| ["SAPISID", "__Secure-3PSID", "SID"].map(|n| jar.get("www.youtube.com", n).map(|c| c.value.clone()));
    let before = session(&jar_of(st, Kind::Extension));
    write_private(&ext_path(st.data_dir()), &j.to_netscape())?;
    let switched = before != session(&j);
    let changed_account = load(st.data_dir()).extension.account.is_none() || switched;
    let account = if changed_account { account_of(st, &j).await } else { None };
    update(st.data_dir(), |s| {
        s.extension.updated_at = Some(t);
        s.extension.checked_at = Some(t);
        s.extension.ok = Some(true);
        s.extension.message = None;
        s.extension.agent = agent_of(body);
        // a switched account never keeps the old name, even when the new one couldn't be read
        if account.is_some() || switched {
            s.extension.account = account;
        }
    });
    signed_in_again(st);
    Ok(json!({ "ok": true, "cookies": j.0.len(), "message": format!("{} cookies received", j.0.len()) }))
}
