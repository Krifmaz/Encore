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

use crate::Ctx;
use encore_db::now;
use jar::Jar;
use reqwest::header::HeaderMap;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
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
            Kind::Extension => "Everyday browser (extension)",
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
    /// that browser's full User-Agent (extensions from 0.26 on report it): what Encore presents when it
    /// refreshes a handed-over session
    pub user_agent: Option<String>,
    /// cookies the last refresh by Encore renewed (Some(0): Google answered but sent none back, so the
    /// sign-in was only checked; None: never refreshed, never rotated on purpose, or the rotation call
    /// itself failed – `message` says)
    pub rotated: Option<usize>,
}

/// `<config>/youtube-browser/state.json` (the folder keeps its old name: installs from before keep their state)
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AuthState {
    /// the server-owned session (`managed` before 0.25.3, when it came from Encore's own browser)
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
    pub fn get_mut(&mut self, k: Kind) -> &mut SourceState {
        match k {
            Kind::Handoff => &mut self.handoff,
            Kind::Extension => &mut self.extension,
            Kind::Pasted => &mut self.pasted,
        }
    }
}

/// The sign-in's locks, kept in the server's state. Clones share them.
#[derive(Clone, Default)]
pub struct Locks {
    /// the state file: read, change and write as one step
    state: Arc<Mutex<()>>,
    /// a rotation of the hand-off or pasted cookies in progress: one at a time (two at once would each
    /// supersede the other's cookies), and a hand-off arriving meanwhile waits for it rather than being
    /// overwritten by the older session's renewed cookies
    rotating: Arc<tokio::sync::Mutex<()>>,
}

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

pub fn load(data: &Path) -> AuthState {
    std::fs::read_to_string(state_path(data)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

/// Change the state file (read, change, write under one lock).
pub fn update<R>(st: &impl Ctx, f: impl FnOnce(&mut AuthState) -> R) -> R {
    let _g = st.requests().youtube.state.lock().unwrap_or_else(|e| e.into_inner());
    let data = st.data_dir();
    let mut s = load(data);
    let r = f(&mut s);
    if let Ok(t) = serde_json::to_string_pretty(&s) {
        if let Err(e) = write_private(&state_path(data), &t) {
            tracing::warn!("saving the YouTube sign-in state failed: {e}");
        }
    }
    r
}

/// The everyday browser's cookies (the extension's uploads).
pub fn ext_path(data: &Path) -> PathBuf {
    home(data).join("extension-cookies.txt")
}

/// The server-owned session (the file the old managed browser exported to: what it held is server-only too).
pub fn handoff_path(data: &Path) -> PathBuf {
    home(data).join("cookies.txt")
}

/// The cookies of one source (empty when it has none).
pub fn jar_of(st: &impl Ctx, k: Kind) -> Jar {
    match k {
        Kind::Handoff => std::fs::read_to_string(handoff_path(st.data_dir())).map(|t| Jar::parse_netscape(&t)).unwrap_or_default(),
        Kind::Extension => std::fs::read_to_string(ext_path(st.data_dir())).map(|t| Jar::parse_netscape(&t)).unwrap_or_default(),
        Kind::Pasted => Jar::parse_netscape(&st.cfg().str("ytdlp_cookies")),
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

fn candidates(st: &impl Ctx, s: &AuthState) -> Vec<Candidate> {
    Kind::ALL.iter().map(|k| Candidate { kind: *k, jar: jar_of(st, *k), state: s.get(*k).clone() }).collect()
}

/// The source in use now.
pub fn active(st: &impl Ctx) -> Option<Kind> {
    let s = load(st.data_dir());
    choose(&candidates(st, &s), now() as i64)
}

/// Netscape cookies for yt-dlp from the source in use (None: no cookies at all).
pub fn cookies_for_ytdlp(st: &impl Ctx) -> Option<String> {
    let s = load(st.data_dir());
    let c = candidates(st, &s);
    let k = choose(&c, now() as i64)?;
    let jar = &c.iter().find(|x| x.kind == k)?.jar;
    // the pasted text as it is (it may hold cookies of other sites the owner wanted yt-dlp to have)
    Some(if k == Kind::Pasted { st.cfg().str("ytdlp_cookies") } else { jar.to_netscape() })
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
pub async fn account_of(st: &impl Ctx, jar: &Jar) -> Option<String> {
    let cookie = jar.header_for("music.youtube.com", now() as i64);
    if cookie.is_empty() {
        return None;
    }
    let data = json!({ "mode": "browser", "headers": { "cookie": cookie, "x-goog-authuser": "0" } });
    let v = encore_http::ytmusic::authed(st, &data, "account/account_menu", json!({})).await.ok()?;
    account_name(&v)
}

/// The values that identify one Google session in a jar (they stay the same between uploads of one
/// session and differ for another account or a new sign-in).
fn session_of(jar: &Jar) -> [Option<String>; 3] {
    ["SAPISID", "__Secure-3PSID", "SID"].map(|n| jar.get("www.youtube.com", n).map(|c| c.value.clone()))
}

/// The account name behind a source's cookies, looked up in the background: a hand-off or an upload is
/// answered at once, without waiting for YouTube. The name is written only while the source still holds the
/// session that was looked up (a newer hand-off meanwhile keeps its own, still unknown, account).
fn spawn_account_lookup(st: &impl Ctx, k: Kind, jar: Jar) {
    let st = st.clone();
    tokio::spawn(async move {
        let Some(account) = account_of(&st, &jar).await else { return };
        if session_of(&jar_of(&st, k)) == session_of(&jar) {
            update(&st, |s| s.get_mut(k).account = Some(account));
        }
    });
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

/// Is a jar signed in to YouTube Music? (the page's ytcfg says LOGGED_IN). `ua`: the browser the session
/// came from (rotate::user_agent).
pub async fn verify(st: &impl Ctx, jar: &Jar, ua: &str) -> anyhow::Result<bool> {
    let cookie = jar.header_for("music.youtube.com", now() as i64);
    if cookie.is_empty() {
        return Ok(false);
    }
    let html = st.http().get("https://music.youtube.com/").header("Cookie", cookie).header("Accept-Language", "en").header("User-Agent", ua)
        .timeout(Duration::from_secs(30)).send().await?.text().await?;
    Ok(html.contains("\"LOGGED_IN\":true"))
}

// ---- refreshing ---------------------------------------------------------------------------------

/// What a refresh found.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Refresh {
    pub signed_in: bool,
    /// cookies Google renewed (None: nothing was rotated on purpose – the extension's cookies, or pasted
    /// cookies that are only checked – or the rotation call itself failed; the status's message says)
    pub rotated: Option<usize>,
}

/// Record what a refresh of a source found. `rotated` Some(0) is a refresh that renewed nothing: the
/// sign-in was checked, but "last refresh" (`updated_at`) stays where it was, so the status never calls a
/// bare check a refresh. `message`: what to tell the admin (None when all is well); with a message and
/// `rotated` None (the rotation call failed, the sign-in holds) "last refresh" stays as well.
pub fn note_refresh(src: &mut SourceState, t: f64, ok: bool, rotated: Option<usize>, message: Option<String>, account: Option<String>) {
    src.checked_at = Some(t);
    src.ok = Some(ok);
    src.rotated = rotated;
    src.message = message;
    if ok {
        if src.message.is_none() && rotated.is_none_or(|n| n > 0) {
            src.updated_at = Some(t);
        }
        if account.is_some() {
            src.account = account;
        }
    }
}

/// The plain note for a source whose last refresh renewed nothing (None otherwise, and None when the
/// source has a message of its own: that one says what happened).
fn refresh_note(src: &SourceState) -> Option<String> {
    (src.rotated == Some(0) && src.ok == Some(true) && src.message.is_none()).then(|| "the last refresh renewed no cookies (Google sent none back) – only the sign-in was checked".to_string())
}

/// What a rotation renewed, for `note_refresh` and the answer: Some(n) when Google answered (0: without
/// cookies); None when nothing was rotated on purpose (`rotated` false) or the call itself failed – then
/// nothing is known about the cookies and the message says why.
fn renewed(outcome: &rotate::Outcome, rotated: bool) -> Option<usize> {
    match outcome { rotate::Outcome::Rotated(n) if rotated => Some(*n), _ => None }
}

/// Light refresh (rotate.rs) of the server-owned session, presented as the browser it was handed over
/// from; the renewed cookies are saved back. Answers whether it is still signed in and what was renewed.
pub async fn refresh_handoff(st: &impl Ctx) -> anyhow::Result<Refresh> {
    let _g = st.requests().youtube.rotating.lock().await;
    let data = st.data_dir().to_path_buf();
    let mut j = jar_of(st, Kind::Handoff);
    let t = now();
    if j.is_empty() {
        anyhow::bail!("No handed-over sign-in – press Hand over a YouTube sign-in in the Encore extension");
    }
    let src = load(&data).handoff;
    let ua = rotate::user_agent(src.user_agent.as_deref(), src.agent.as_deref());
    let outcome = rotate::run(st.http(), &mut j, t as i64, &ua).await;
    if matches!(outcome, rotate::Outcome::Rotated(n) if n > 0) {
        write_private(&handoff_path(&data), &j.to_netscape())?;
    }
    let ok = match &outcome {
        rotate::Outcome::SignedOut => Some(false),
        _ => verify(st, &j, &ua).await.ok(),
    };
    let Some(ok) = ok else {
        update(st, |s| {
            s.handoff.checked_at = Some(t);
            s.handoff.message = Some("Couldn't reach YouTube to refresh the sign-in – trying again in 30 min".into());
        });
        anyhow::bail!("Couldn't reach YouTube to refresh the sign-in");
    };
    let rotated = renewed(&outcome, true);
    let message = match (&outcome, ok) {
        (_, false) => Some("Signed out – do the hand-off again from the Encore extension".to_string()),
        // the rotation call itself failed (Google unreachable or an error answer): tried again in 30 min
        (rotate::Outcome::Failed(e), true) => Some(format!("Couldn't renew the cookies ({e}) – still signed in, trying again in 30 min")),
        _ => None,
    };
    let account = if ok { account_of(st, &j).await } else { None };
    update(st, |s| note_refresh(&mut s.handoff, t, ok, rotated, message, account));
    if ok {
        st.db().log("info", &match &outcome {
            rotate::Outcome::Rotated(n) if *n > 0 => format!("YouTube sign-in refreshed by Encore (handed-over session, {n} cookies renewed)"),
            rotate::Outcome::Failed(e) => format!("YouTube sign-in checked by Encore (handed-over session): still signed in, but the cookies could not be renewed: {e}"),
            _ => "YouTube sign-in checked by Encore (handed-over session): still signed in, but Google renewed no cookies".to_string(),
        });
        signed_in_again(st);
    } else {
        st.db().log("warning", "The handed-over YouTube sign-in was signed out");
        signed_out(st, Kind::Handoff).await;
    }
    Ok(Refresh { signed_in: ok, rotated })
}

/// Light refresh of the pasted cookies; the renewed jar is saved back to the setting. With `rotate` false
/// (cookies set by YTDLP_COOKIES, which Encore can't save back, or the light refresh switched off) they are
/// only checked: rotating a session that can't be stored would leave the stored one superseded.
pub async fn refresh_pasted(st: &impl Ctx, rotate: bool) -> anyhow::Result<Refresh> {
    let _g = st.requests().youtube.rotating.lock().await;
    let raw = st.cfg().str("ytdlp_cookies");
    let mut j = Jar::parse_netscape(&raw);
    if !j.has_auth(now() as i64) {
        anyhow::bail!("The pasted cookies have no YouTube sign-in");
    }
    let t = now();
    let rotate = rotate && !st.cfg().env_locked("ytdlp_cookies");
    // pasted cookies come with no browser: the default one
    let ua = rotate::user_agent(None, None);
    let outcome = if rotate { rotate::run(st.http(), &mut j, t as i64, &ua).await } else { rotate::Outcome::Rotated(0) };
    // Google renewed them: saved whatever the check below says (the old values are superseded now)
    if matches!(outcome, rotate::Outcome::Rotated(n) if n > 0) {
        st.cfg().set_many(&json!({ "ytdlp_cookies": j.to_netscape() }));
    }
    // None: YouTube couldn't be reached – nothing is known, nothing changes and nobody is told
    let ok = match &outcome {
        rotate::Outcome::SignedOut => Some(false),
        _ => verify(st, &j, &ua).await.ok(),
    };
    let Some(ok) = ok else {
        update(st, |s| {
            s.pasted.checked_at = Some(t);
            s.pasted.message = Some("Couldn't reach YouTube to check the cookies – trying again later".into());
        });
        anyhow::bail!("Couldn't reach YouTube to check the cookies");
    };
    let msg = match (&outcome, ok) {
        (rotate::Outcome::Failed(e), true) if rotate => Some(format!("Couldn't renew the cookies ({e}) – still signed in, trying again later")),
        (_, true) => None,
        (rotate::Outcome::SignedOut, _) => Some("Google signed these cookies out – paste fresh ones or do the hand-off from the Encore extension".to_string()),
        (rotate::Outcome::Failed(e), _) => Some(format!("Stale: {e}")),
        _ => Some("Stale: YouTube no longer accepts these cookies".to_string()),
    };
    // only checked (rotation off): nothing was meant to be renewed; a failed call renewed nothing either
    let rotated = renewed(&outcome, rotate);
    let account = if ok { account_of(st, &j).await } else { None };
    update(st, |s| note_refresh(&mut s.pasted, t, ok, rotated, msg, account));
    if ok {
        signed_in_again(st);
    } else {
        signed_out(st, Kind::Pasted).await;
    }
    Ok(Refresh { signed_in: ok, rotated })
}

/// Check the extension's cookies still sign in (never rotated by Encore).
pub async fn check_extension(st: &impl Ctx) -> anyhow::Result<bool> {
    let j = jar_of(st, Kind::Extension);
    let src = load(st.data_dir()).extension;
    let ok = verify(st, &j, &rotate::user_agent(src.user_agent.as_deref(), src.agent.as_deref())).await?;
    let account = if ok { account_of(st, &j).await } else { None };
    update(st, |s| {
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
async fn signed_out(st: &impl Ctx, k: Kind) {
    if active(st).is_some_and(|a| a != k && load(st.data_dir()).get(a).ok != Some(false)) {
        return; // another source still works
    }
    let first = update(st, |s| !std::mem::replace(&mut s.signed_out_notified, true));
    if first {
        let msg = signed_out_message(k);
        st.db().log("warning", &msg);
        crate::notify::send(st, "YouTube signed out", &msg, None).await;
    }
}

fn signed_in_again(st: &impl Ctx) {
    update(st, |s| s.signed_out_notified = false);
}

/// yt-dlp was asked to sign in / to prove it is not a bot: refresh the cookies Encore owns (the hand-off
/// session, or pasted cookies with the light refresh on) once. true → refreshed and signed in, try again.
pub async fn after_ytdlp_auth_error(st: &impl Ctx) -> bool {
    let s = load(st.data_dir());
    let act = active(st);
    let src = match act {
        Some(Kind::Handoff) => &s.handoff,
        Some(Kind::Pasted) if st.cfg().bool("youtube_light_refresh") && !st.cfg().env_locked("ytdlp_cookies") => &s.pasted,
        _ => return false,
    };
    // at most once per 10 minutes, however many downloads fail together
    if src.checked_at.is_some_and(|c| now() - c < 600.0) {
        return false;
    }
    let r = if act == Some(Kind::Handoff) { refresh_handoff(st).await } else { refresh_pasted(st, true).await };
    matches!(r, Ok(Refresh { signed_in: true, .. }))
}

/// Background loop: refresh of the hand-off session and of pasted cookies (4 h).
pub async fn supervisor(st: impl Ctx) {
    tokio::time::sleep(Duration::from_secs(90)).await;
    loop {
        tick(&st).await;
        tokio::time::sleep(TICK).await;
    }
}

async fn tick(st: &impl Ctx) {
    let t = now();
    let s = load(st.data_dir());
    let failed = s.handoff.message.is_some() && s.handoff.ok != Some(false);
    if handoff_due(t, !jar_of(st, Kind::Handoff).is_empty(), s.handoff.checked_at, failed) {
        let _ = refresh_handoff(st).await;
    }
    let s = load(st.data_dir());
    if light_due(t, s.pasted.checked_at, active(st), st.cfg().bool("youtube_light_refresh") && !st.cfg().env_locked("ytdlp_cookies")) {
        let _ = refresh_pasted(st, true).await;
    }
}

// ---- status -------------------------------------------------------------------------------------

/// Settings → YouTube. Never holds a cookie value.
pub fn status(st: &impl Ctx) -> Value {
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
            "account": st8.account, "message": st8.message, "agent": st8.agent, "rotated": st8.rotated,
        }))
    }).collect();
    let light_on = st.cfg().bool("youtube_light_refresh");
    let next = next_refresh(now(), act, &s, light_on && !st.cfg().env_locked("ytdlp_cookies"));
    let current = act.map(|k| s.get(k).clone()).unwrap_or_default();
    json!({
        "source": act.map(|k| k.key()), "source_label": act.map(|k| k.label()), "account": current.account,
        "last_refresh": current.updated_at, "last_check": current.checked_at, "next_refresh": next,
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
fn diagnostics(st: &impl Ctx, act: Option<Kind>, s: &AuthState) -> String {
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
            if let Some(n) = refresh_note(src) {
                parts.push(n);
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

// ---- sign-out and the extension's token --------------------------------------------------------

/// Forget a sign-in: "handoff", "extension" or "all" (both; the pasted cookies have their own clear).
pub async fn sign_out(st: &impl Ctx, which: &str) {
    if which == "handoff" || which == "all" {
        let _g = st.requests().youtube.rotating.lock().await;
        let _ = std::fs::remove_file(handoff_path(st.data_dir()));
        update(st, |s| s.handoff = SourceState::default());
    }
    if which == "extension" || which == "all" {
        let _ = std::fs::remove_file(ext_path(st.data_dir()));
        update(st, |s| {
            s.extension = SourceState::default();
            s.ext_token = None;
            s.ext_token_at = None;
        });
    }
}

/// A new extension token (shown once; the old one stops working). It can only upload cookies.
pub fn new_token(st: &impl Ctx) -> String {
    let token = format!("eyt_{}", encore_core::ids::random_hex(24));
    let h = token_hash(&token);
    update(st, |s| {
        s.ext_token = Some(h);
        s.ext_token_at = Some(now());
    });
    token
}

/// How the extension token is kept: its SHA-256 (the token itself is shown once and never stored).
pub fn token_hash(t: &str) -> String {
    encore_core::ids::sha256_hex(t.trim())
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

/// The browser's full `navigator.userAgent` (extensions from 0.26 on send it as `user_agent`).
fn user_agent_of(body: &Value) -> Option<String> {
    body["user_agent"].as_str().map(|a| a.chars().filter(|c| !c.is_control()).take(400).collect::<String>()).filter(|a| !a.is_empty())
}

/// A hand-off: the private window's cookies become the server's own session.
pub async fn accept_handoff(st: &impl Ctx, body: &Value) -> anyhow::Result<Value> {
    let j = cookie_list(body)?;
    let t = now();
    if !j.has_auth(t as i64) {
        return Ok(json!({ "ok": false, "message": "That window is not signed in to YouTube yet – finish signing in, then send again" }));
    }
    {
        let _g = st.requests().youtube.rotating.lock().await;
        write_private(&handoff_path(st.data_dir()), &j.to_netscape())?;
        update(st, |s| {
            s.handoff = SourceState { updated_at: Some(t), checked_at: Some(t), ok: Some(true), account: None, message: None, agent: agent_of(body),
                user_agent: user_agent_of(body), rotated: None };
        });
    }
    signed_in_again(st);
    // the account name needs YouTube: looked up after the answer, so the extension's window closes at once
    spawn_account_lookup(st, Kind::Handoff, j.clone());
    st.db().log("info", &format!("YouTube sign-in handed over from the Encore extension{} – Encore keeps it alive by itself now",
        agent_of(body).map(|a| format!(" ({a})")).unwrap_or_default()));
    Ok(json!({ "ok": true, "handoff": true, "cookies": j.0.len(),
        "message": "Encore has its own YouTube sign-in now and keeps it alive by itself. The private window can close." }))
}

pub async fn accept_extension(st: &impl Ctx, body: &Value) -> anyhow::Result<Value> {
    let j = cookie_list(body)?;
    let t = now();
    if !j.has_auth(t as i64) {
        update(st, |s| {
            s.extension.checked_at = Some(t);
            s.extension.ok = Some(false);
            s.extension.message = Some("The browser is not signed in to YouTube".into());
            s.extension.agent = agent_of(body);
            s.extension.user_agent = user_agent_of(body);
        });
        return Ok(json!({ "ok": false, "message": "This browser is not signed in to YouTube – sign in at music.youtube.com" }));
    }
    // another Google account in that browser: its session cookies differ (they stay the same between
    // uploads of one session), so the account name is looked up again
    let before = session_of(&jar_of(st, Kind::Extension));
    write_private(&ext_path(st.data_dir()), &j.to_netscape())?;
    let switched = before != session_of(&j);
    let changed_account = load(st.data_dir()).extension.account.is_none() || switched;
    update(st, |s| {
        s.extension.updated_at = Some(t);
        s.extension.checked_at = Some(t);
        s.extension.ok = Some(true);
        s.extension.message = None;
        s.extension.agent = agent_of(body);
        s.extension.user_agent = user_agent_of(body);
        // a switched account never keeps the old name (the new one arrives from the lookup below)
        if switched {
            s.extension.account = None;
        }
    });
    signed_in_again(st);
    if changed_account {
        spawn_account_lookup(st, Kind::Extension, j.clone());
    }
    Ok(json!({ "ok": true, "cookies": j.0.len(), "message": format!("{} cookies received", j.0.len()) }))
}
