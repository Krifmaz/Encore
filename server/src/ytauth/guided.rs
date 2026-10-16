//! Guided sign-in: Encore's own form in front of Google's sign-in pages. The managed browser opens
//! Google's sign-in, Encore reads the page (one script over the DOM, `DETECT_JS`), turns it into a
//! `Step` the web UI shows as a plain form (e-mail, password, a code, a list of ways to sign in …), and
//! types what the owner enters into Google's own fields (focus + Input.insertText, then Enter).
//!
//! Security keys and passkeys can't work in a browser on a server: on such a page Encore presses
//! Google's "Try another way" by itself and offers the list Google shows, the authenticator app first.
//! Anything Encore doesn't recognise (a captcha, "Verify it's you" …) shows Google's own words and a
//! picture, with the live view as the way through.
//!
//! What the owner types (password, codes) is only ever held for the one step that sends it: never
//! logged, never stored, never part of an answer.
use super::browser::{self, Browser};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::Duration;

/// Where a guided sign-in ends: YouTube Music, signed in.
pub const HOME: &str = "https://music.youtube.com";
/// How long one step waits for Google's page to change.
pub const STEP_WAIT: Duration = Duration::from_secs(20);

/// Reads the sign-in page: what is on it, and marks the elements Encore may fill or press with
/// data-encore / data-encore-choice / data-encore-button (cleared and set again on every read).
pub const DETECT_JS: &str = r#"(() => {
  const vis = (e) => {
    if (!e || !e.isConnected) return false;
    if (e.closest('[aria-hidden="true"],[hidden]')) return false;
    if (e.tagName === 'INPUT' && (e.type === 'hidden' || e.disabled)) return false;
    const cs = getComputedStyle(e);
    if (cs.display === 'none' || cs.visibility === 'hidden') return false;
    const r = e.getBoundingClientRect();
    return r.width > 0 && r.height > 0;
  };
  const txt = (e) => ((e && (e.innerText || e.textContent)) || '').replace(/\s+/g, ' ').trim();
  const all = (sel) => Array.from(document.querySelectorAll(sel)).filter(vis);
  const first = (sels) => { for (const s of sels) { const e = all(s)[0]; if (e) return e; } return null; };
  for (const e of document.querySelectorAll('[data-encore],[data-encore-choice],[data-encore-button]')) {
    e.removeAttribute('data-encore'); e.removeAttribute('data-encore-choice'); e.removeAttribute('data-encore-button');
  }
  const tag = (e, v) => { if (e) e.setAttribute('data-encore', v); return !!e; };
  const email = first(['input[type=email]', 'input[name=identifier]']);
  const password = first(['input[name=Passwd]', 'input[type=password]']);
  const code = first(['input[name=totpPin]', 'input[autocomplete=one-time-code]', 'input[name=Pin]', 'input[name=idvPin]', 'input[name=backupCodePin]', 'input[type=tel]']);
  const captcha = !!first(['#captchaimg', 'img[src*="Captcha"]', 'iframe[src*="recaptcha"]', 'iframe[title*="reCAPTCHA"]', 'input[name=ca]']);
  const clickable = 'button,[role=button],[role=link],a';
  const another = all(clickable).find(e => /^try another way$/i.test(txt(e))) || null;
  const next = all('button,[role=button]').find(e => /^next$/i.test(txt(e))) || first(['#identifierNext button', '#passwordNext button', '#totpNext button']);
  let choiceEls = all('[data-challengetype],[data-action="selectchallenge"]').filter(e => e.getAttribute('data-challengeunavailable') !== 'true');
  choiceEls = choiceEls.filter(e => !choiceEls.some(o => o !== e && o.contains(e)));
  let chooser = false;
  if (!choiceEls.length) {
    const accounts = all('[data-identifier]').filter((e, _, a) => !a.some(o => o !== e && o.contains(e)));
    if (accounts.length) {
      chooser = true;
      const other = all('[role=link],[role=button],button,li,a').filter(e => /^use another account$/i.test(txt(e)));
      choiceEls = accounts.concat(other.filter(e => !other.some(o => o !== e && e.contains(o))).slice(0, 1));
    }
  }
  const choices = choiceEls.map((e, i) => { e.setAttribute('data-encore-choice', String(i)); return txt(e).slice(0, 160); });
  const choiceTypes = choiceEls.map(e => e.getAttribute('data-challengetype') || '');
  const seen = new Set();
  const buttons = [];
  for (const e of all('button,[role=button]')) {
    const t = txt(e);
    if (!t || t.length > 40 || e === another || seen.has(t) || choiceEls.includes(e) || buttons.length >= 6) continue;
    seen.add(t);
    e.setAttribute('data-encore-button', String(buttons.length));
    buttons.push(t);
  }
  tag(email, 'email'); tag(password, 'password'); tag(code, 'code'); tag(another, 'another'); tag(next, 'next');
  const errEl = all('[aria-live=assertive],[role=alert],[jsname="B34EJ"],.o6cuMc,.Ekjuhf').find(e => txt(e) && txt(e).length <= 300);
  const heading = txt(first(['#headingText', 'h1']));
  const subtext = txt(first(['#headingSubtext']));
  const label = code ? (txt(code.labels && code.labels[0]) || code.getAttribute('aria-label') || code.placeholder || '') : '';
  const main = first(['[role=main]', 'main', 'form']) || document.body;
  const page = (document.body ? txt(document.body) : '').toLowerCase();
  let loggedIn = false;
  try { loggedIn = !!(window.ytcfg && ytcfg.get && ytcfg.get('LOGGED_IN')); } catch (e) { loggedIn = false; }
  return {
    url: location.href, ready: document.readyState === 'complete', logged_in: loggedIn,
    heading, subtext, body: txt(main).slice(0, 600),
    email: !!email, password: !!password,
    code: code ? { name: code.name || '', autocomplete: code.getAttribute('autocomplete') || '', kind: code.type || '', label } : null,
    captcha, another: !!another, next: !!next, chooser, choices, choice_types: choiceTypes, buttons,
    error: errEl ? txt(errEl) : '',
    passkey: /security key|passkey|use your phone or tablet to|insert your/.test(page),
    tap_yes: /tap yes|check your phone|check your tablet|check your device|google sent a notification|open the .{1,40} app on your/.test(page),
  };
})()"#;

/// The code field of a page.
#[derive(Deserialize, Default, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct CodeInput {
    pub name: String,
    pub autocomplete: String,
    pub kind: String,
    pub label: String,
}

/// What `DETECT_JS` found on the page.
#[derive(Deserialize, Default, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Snapshot {
    pub url: String,
    pub ready: bool,
    pub logged_in: bool,
    pub heading: String,
    pub subtext: String,
    pub body: String,
    pub email: bool,
    pub password: bool,
    pub code: Option<CodeInput>,
    pub captcha: bool,
    pub another: bool,
    pub next: bool,
    pub chooser: bool,
    pub choices: Vec<String>,
    pub choice_types: Vec<String>,
    pub buttons: Vec<String>,
    pub error: String,
    pub passkey: bool,
    pub tap_yes: bool,
}

impl Snapshot {
    /// What tells one state of the page from the next (a new page, a new field, an error appearing).
    pub fn signature(&self) -> String {
        format!("{}|{}|{}|{}{}{}{}{}|{}|{}", self.url, self.heading, self.error, self.email as u8, self.password as u8, self.code.is_some() as u8,
            self.captcha as u8, self.another as u8, self.choices.join("/"), self.buttons.join("/"))
    }
}

/// One way to go on, shown as a button.
#[derive(Serialize, Clone, Debug, Default, PartialEq)]
pub struct Choice {
    pub id: String,
    pub label: String,
    /// the one to offer first (an authenticator app)
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub default: bool,
}

/// What the form shows next. `kind`: email | password | code | choice | wait | unknown | done | error.
#[derive(Serialize, Clone, Debug, Default, PartialEq)]
pub struct Step {
    pub kind: String,
    pub title: String,
    pub text: String,
    pub error: String,
    pub choices: Vec<Choice>,
    /// for a code: "totp" (authenticator app), "sms", "backup" or ""
    pub code_hint: String,
    /// a small JPEG of Google's page (base64), for pages Encore can't fill in
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screenshot: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
}

impl Step {
    pub fn new(kind: &str, title: &str) -> Step {
        Step { kind: kind.into(), title: title.into(), ..Default::default() }
    }
    pub fn error(msg: &str) -> Step {
        Step { kind: "error".into(), title: "The sign-in stopped".into(), error: msg.into(), ..Default::default() }
    }
}

/// What a page asks for.
#[derive(Debug, Clone, PartialEq)]
pub enum Detected {
    Step(Step),
    /// a security key or passkey page: press "Try another way"
    TryAnother,
    /// on YouTube: the sign-in is through (if YouTube says so)
    Home,
}

fn or(a: &str, b: &str) -> String {
    if a.trim().is_empty() { b.to_string() } else { a.trim().to_string() }
}

fn another_choice() -> Choice {
    Choice { id: "another".into(), label: "Try another way".into(), default: false }
}

/// Is this YouTube (where Google sends the browser once the sign-in is through)?
pub fn is_home(url: &str, home: &str) -> bool {
    url.starts_with(home) || (home == HOME && url.starts_with("https://www.youtube.com"))
}

/// Turn what is on the page into the next step.
pub fn classify(s: &Snapshot, home: &str) -> Detected {
    if is_home(&s.url, home) {
        return Detected::Home;
    }
    let lower = format!("{} {} {}", s.heading, s.subtext, s.body).to_lowercase();
    let step = |kind: &str, title: &str| Step { kind: kind.into(), title: or(&s.heading, title), error: s.error.clone(), ..Default::default() };
    if s.captcha {
        let mut st = step("unknown", "Google wants to check this is you");
        st.text = or(&s.subtext, &s.body);
        return Detected::Step(st);
    }
    if s.password {
        let mut st = step("password", "Enter your password");
        st.text = s.subtext.clone();
        return Detected::Step(st);
    }
    if s.email {
        let mut st = step("email", "Sign in");
        st.text = s.subtext.clone();
        return Detected::Step(st);
    }
    if let Some(c) = &s.code {
        let mut st = step("code", "Enter the code");
        let words = format!("{lower} {}", c.label.to_lowercase());
        st.code_hint = if c.name == "totpPin" || words.contains("authenticator") {
            "totp"
        } else if c.name == "backupCodePin" || words.contains("backup code") {
            "backup"
        } else if c.name == "idvPin" || words.contains("text message") || words.contains("sms") || words.contains("sent a text") {
            "sms"
        } else {
            ""
        }.into();
        st.text = or(&s.subtext, &c.label);
        if s.another {
            st.choices.push(another_choice());
        }
        return Detected::Step(st);
    }
    if !s.choices.is_empty() {
        let mut st = step("choice", if s.chooser { "Choose an account" } else { "Choose how to sign in" });
        st.text = s.subtext.clone();
        let auth = s.choices.iter().position(|c| c.to_lowercase().contains("authenticator"))
            .or_else(|| s.choice_types.iter().position(|t| t == "6"));
        st.choices = s.choices.iter().enumerate().map(|(i, l)| Choice { id: format!("c{i}"), label: l.clone(), default: !s.chooser && auth == Some(i) }).collect();
        // the authenticator app first
        st.choices.sort_by_key(|c| !c.default);
        return Detected::Step(st);
    }
    if s.passkey && s.another {
        return Detected::TryAnother;
    }
    if s.tap_yes {
        let mut st = step("wait", "Google is waiting for your phone");
        st.text = or(&s.subtext, &s.body);
        if s.another {
            st.choices.push(another_choice());
        }
        return Detected::Step(st);
    }
    Detected::Step(unknown(s))
}

/// A page Encore has no form for: Google's words, its buttons, and (added later) a picture.
pub fn unknown(s: &Snapshot) -> Step {
    let mut st = Step { kind: "unknown".into(), title: or(&s.heading, "Google needs something else"), error: s.error.clone(), ..Default::default() };
    st.text = or(&s.subtext, &s.body);
    st.choices = s.buttons.iter().enumerate().map(|(i, l)| Choice { id: format!("b{i}"), label: l.clone(), default: false }).collect();
    if s.another {
        st.choices.push(another_choice());
    }
    st
}

// ---- driving the page ---------------------------------------------------------------------------

pub async fn detect(b: &Browser) -> Snapshot {
    try_detect(b).await.unwrap_or_default()
}

/// Read the page; Err when the script could not run (no page, a navigation, a dead browser …).
pub async fn try_detect(b: &Browser) -> anyhow::Result<Snapshot> {
    let v = b.eval(DETECT_JS).await?;
    Ok(serde_json::from_value(v).unwrap_or_default())
}

/// An error that means the browser itself is gone or stuck (not just Google's page changing under a
/// click or a script).
pub fn browser_failed(e: &anyhow::Error) -> bool {
    let m = e.to_string();
    m.contains("the browser connection is closed") || m.contains("the browser closed") || m.contains("no answer from the browser")
}

/// A small picture of the page (half size).
pub async fn screenshot(b: &Browser) -> Option<String> {
    let r = b.page("Page.captureScreenshot", json!({ "format": "jpeg", "quality": 55, "clip": { "x": 0, "y": 0, "width": 1280, "height": 860, "scale": 0.5 } })).await.ok()?;
    r["data"].as_str().map(String::from)
}

/// Click a marked element with a real mouse click at its middle.
async fn click(b: &Browser, sel: &str) -> anyhow::Result<()> {
    let expr = format!("(() => {{ const e = document.querySelector({}); if (!e) return null; e.scrollIntoView({{ block: 'center' }}); const r = e.getBoundingClientRect(); return [r.left + r.width / 2, r.top + r.height / 2]; }})()", json!(sel));
    let v = b.eval(&expr).await?;
    let (Some(x), Some(y)) = (v[0].as_f64(), v[1].as_f64()) else { anyhow::bail!("The button is no longer on Google's page") };
    b.click_at(x, y).await
}

/// Type into a marked field the way a person does: focus it, select what is there, insert the text
/// (Google ignores a value set by script), press Enter.
async fn fill(b: &Browser, sel: &str, value: &str) -> anyhow::Result<()> {
    let expr = format!("(() => {{ const e = document.querySelector({}); if (!e) return false; e.scrollIntoView({{ block: 'center' }}); e.focus(); if (e.select) e.select(); return document.activeElement === e; }})()", json!(sel));
    if b.eval(&expr).await?.as_bool() != Some(true) {
        // a click puts the caret there too
        click(b, sel).await?;
        b.eval(&format!("(() => {{ const e = document.querySelector({}); if (e && e.select) e.select(); }})()", json!(sel))).await?;
    }
    // the text goes as a DevTools parameter only, never into a script
    b.page("Input.insertText", json!({ "text": value })).await?;
    for (method, params) in browser::input_commands(&json!({ "type": "key", "key": "Enter", "code": "Enter" })) {
        b.page(method, params).await?;
    }
    Ok(())
}

/// Wait for the page to change from `before` (None: take what is there), then work out the step.
/// Security key / passkey pages are passed with "Try another way"; YouTube means done (if signed in).
pub async fn advance(b: &Browser, before: Option<String>, home: &str) -> anyhow::Result<Step> {
    advance_with(b, before, home, false).await
}

async fn advance_with(b: &Browser, mut before: Option<String>, home: &str, filled: bool) -> anyhow::Result<Step> {
    let mut deadline = tokio::time::Instant::now() + STEP_WAIT;
    let started = tokio::time::Instant::now();
    let (mut nudged, mut passed, mut last_unknown) = (!filled, 0, None::<String>);
    loop {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let s = match try_detect(b).await {
            Ok(s) => s,
            // the browser is gone or stuck: no use waiting
            Err(e) if browser_failed(&e) => return Err(e),
            // a navigation (or a dialog) in the way: read again, but not for ever
            Err(e) => {
                if tokio::time::Instant::now() >= deadline {
                    anyhow::bail!("Google's page could not be read ({e})");
                }
                continue;
            }
        };
        let sig = s.signature();
        let timed_out = tokio::time::Instant::now() >= deadline;
        if before.as_deref() == Some(sig.as_str()) && !timed_out {
            // Enter did nothing after a few seconds: press Google's Next
            if !nudged && s.next && started.elapsed() > Duration::from_secs(4) {
                nudged = true;
                let _ = click(b, "[data-encore=next]").await;
            }
            continue;
        }
        if s.url.is_empty() || s.url == "about:blank" {
            if timed_out {
                return Ok(Step::error("Google's page did not load. Start again."));
            }
            continue;
        }
        if !s.ready && !timed_out {
            continue;
        }
        if before.as_deref() != Some(sig.as_str()) {
            // let the page settle
            tokio::time::sleep(Duration::from_millis(600)).await;
        }
        let s = match try_detect(b).await {
            Ok(s2) if !s2.url.is_empty() => s2,
            Err(e) if browser_failed(&e) => return Err(e),
            // gone again in between (a navigation): read once more
            _ if !timed_out => continue,
            _ => s,
        };
        let sig = s.signature();
        let found = match classify(&s, home) {
            // pressed twice and still a security key: show the page as it is
            Detected::TryAnother if passed >= 2 => Detected::Step(unknown(&s)),
            d => d,
        };
        match found {
            Detected::TryAnother => {
                passed += 1;
                match click(b, "[data-encore=another]").await {
                    Ok(()) => {
                        before = Some(sig);
                        deadline = tokio::time::Instant::now() + STEP_WAIT;
                    }
                    Err(e) if browser_failed(&e) => return Err(e),
                    // Google re-drew the page under the click: read it again (at most twice more)
                    Err(_) => {}
                }
            }
            Detected::Home => {
                if home == HOME && !s.url.starts_with(HOME) {
                    b.navigate(&format!("{HOME}/")).await?;
                    b.wait_load(Duration::from_secs(20)).await;
                }
                // else the page is YouTube and loaded already (`ready`): no load event will come
                for _ in 0..15 {
                    if b.logged_in().await {
                        return Ok(Step::new("done", "Signed in"));
                    }
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                return Ok(Step::error("Google sent the browser to YouTube, but YouTube says it is not signed in. Try again."));
            }
            Detected::Step(mut st) => {
                // an unknown page must hold still for a moment (Google's pages fill in after loading)
                if st.kind == "unknown" && last_unknown.as_deref() != Some(sig.as_str()) && !timed_out {
                    last_unknown = Some(sig);
                    continue;
                }
                if timed_out && before.as_deref() == Some(sig.as_str()) && st.error.is_empty() {
                    st.error = "Nothing changed on Google's page. Check what you entered and try again.".into();
                }
                if st.kind == "unknown" {
                    st.screenshot = screenshot(b).await;
                }
                return Ok(st);
            }
        }
    }
}

/// Open the sign-in page and read the first step.
pub async fn open(b: &Browser, url: &str, home: &str) -> anyhow::Result<Step> {
    b.navigate(url).await?;
    b.wait_load(STEP_WAIT).await;
    advance(b, None, home).await
}

/// Check that what the owner sends fits the step it answers (before anything goes to Google).
pub fn check_submit(cur: &Step, kind: &str, value: &str) -> Result<(), &'static str> {
    match kind {
        "check" => Ok(()),
        "email" | "password" | "code" => {
            if cur.kind != kind {
                return Err("Google's page has moved on – look at the form again");
            }
            let v = value.trim();
            if v.is_empty() {
                return Err("Enter something first");
            }
            let max = match kind { "email" => 320, "password" => 1024, _ => 64 };
            if value.chars().count() > max {
                return Err("That is too long");
            }
            if kind == "code" && cur.code_hint == "totp" && !v.chars().all(|c| c.is_ascii_digit() || c == ' ') {
                return Err("The code is digits only");
            }
            Ok(())
        }
        "choice" => {
            if cur.choices.iter().any(|c| c.id == value) {
                Ok(())
            } else {
                Err("That choice is not on Google's page any more")
            }
        }
        _ => Err("Unknown step"),
    }
}

/// Send one step to Google and read the next. `value` (a password, a code) is used here and dropped.
pub async fn submit(b: &Browser, cur: &Step, kind: &str, value: &str, home: &str) -> anyhow::Result<Step> {
    if let Err(e) = check_submit(cur, kind, value) {
        let mut st = cur.clone();
        st.error = e.into();
        return Ok(st);
    }
    // read the page again: the marks are fresh, and it is still the page the form shows
    let s = detect(b).await;
    let now = classify(&s, home);
    if kind != "check" {
        let same = matches!(&now, Detected::Step(st) if st.kind == cur.kind);
        if !same {
            return advance(b, None, home).await;
        }
    }
    let before = Some(s.signature());
    match kind {
        "check" => return advance(b, None, home).await,
        "email" => fill(b, "[data-encore=email]", value.trim()).await?,
        "password" => fill(b, "[data-encore=password]", value).await?,
        "code" => {
            let v: String = if cur.code_hint == "totp" { value.chars().filter(|c| !c.is_whitespace()).collect() } else { value.trim().to_string() };
            fill(b, "[data-encore=code]", &v).await?
        }
        _ => {
            let sel = if value == "another" {
                "[data-encore=another]".to_string()
            } else if let Some(i) = value.strip_prefix('c') {
                format!("[data-encore-choice=\"{}\"]", i.parse::<u32>().unwrap_or(0))
            } else {
                format!("[data-encore-button=\"{}\"]", value.trim_start_matches('b').parse::<u32>().unwrap_or(0))
            };
            click(b, &sel).await?;
            return advance_with(b, before, home, false).await;
        }
    }
    advance_with(b, before, home, true).await
}

/// The body of POST /api/youtube/guided/step. No Debug: the value may be a password.
#[derive(Deserialize)]
pub struct Submit {
    pub kind: String,
    #[serde(default)]
    pub value: String,
}

impl std::fmt::Debug for Submit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Submit").field("kind", &self.kind).field("value", &"<hidden>").finish()
    }
}

// ---- the one guided session ---------------------------------------------------------------------

/// Closed after this long without a step (or the live view open on it).
pub const IDLE: Duration = Duration::from_secs(600);

struct Session {
    id: u64,
    browser: std::sync::Arc<Browser>,
    _lock: tokio::sync::MutexGuard<'static, ()>,
    last: tokio::time::Instant,
    step: Step,
    /// the live view showing this browser (the id of the last one to open: a second one takes over)
    live: Option<u64>,
}

static SESSION: tokio::sync::Mutex<Option<Session>> = tokio::sync::Mutex::const_new(None);
static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Closes a session whose start was abandoned (the request dropped while the browser was opening).
struct StartGuard(Option<u64>);

impl Drop for StartGuard {
    fn drop(&mut self) {
        if let Some(id) = self.0.take() {
            tokio::spawn(async move {
                let mut slot = SESSION.lock().await;
                if slot.as_ref().is_some_and(|s| s.id == id && s.step.kind.is_empty() && s.live.is_none()) {
                    if let Some(s) = slot.take() {
                        s.browser.close().await;
                    }
                }
            });
        }
    }
}

/// Close the guided session, if there is one.
pub async fn cancel() -> bool {
    let s = SESSION.lock().await.take();
    match s {
        Some(s) => {
            s.browser.close().await;
            true
        }
        None => false,
    }
}

/// Save the cookies of a signed-in browser. None: the browser has no YouTube sign-in cookies.
pub async fn save_signin(st: &crate::state::AppState, b: &Browser) -> anyhow::Result<Option<Option<String>>> {
    let jar = b.cookies().await?;
    if !jar.has_auth(crate::db::now() as i64) {
        return Ok(None);
    }
    browser::write_cookies(st.data_dir(), &jar)?;
    let account = super::account_of(st, &jar).await;
    let t = crate::db::now();
    super::update(st.data_dir(), |s| {
        s.managed.updated_at = Some(t);
        s.managed.checked_at = Some(t);
        s.managed.ok = Some(true);
        s.managed.message = None;
        s.managed.account = account.clone();
    });
    Ok(Some(account))
}

/// A step that says "done": save the cookies, close the session and say who is signed in.
async fn finish(st: &crate::state::AppState, slot: &mut Option<Session>) -> Step {
    let Some(s) = slot.take() else { return Step::error("The sign-in window is closed") };
    let res = save_signin(st, &s.browser).await;
    s.browser.close().await;
    drop(s);
    match res {
        Ok(Some(account)) => {
            super::signed_in_again(st);
            st.db.log("info", &format!("Encore's browser is signed in to YouTube{}", account.as_deref().map(|a| format!(" as {a}")).unwrap_or_default()));
            let mut d = Step::new("done", "Signed in");
            d.text = match &account {
                Some(a) => format!("Signed in as {a}"),
                None => "Signed in to YouTube".into(),
            };
            d.account = account;
            d
        }
        Ok(None) => Step::error("YouTube opened, but Google gave the browser no sign-in cookies. Try again."),
        Err(e) => Step::error(&format!("Signed in, but the cookies could not be saved: {e}")),
    }
}

/// After a step: done → finish; an error from the browser → close; else keep the step.
async fn settle(st: &crate::state::AppState, slot: &mut Option<Session>, r: anyhow::Result<Step>) -> Step {
    match r {
        Ok(step) if step.kind == "done" => finish(st, slot).await,
        Ok(step) => {
            if let Some(s) = slot.as_mut() {
                s.step = step.clone();
                s.last = tokio::time::Instant::now();
            }
            step
        }
        Err(e) => {
            if let Some(s) = slot.take() {
                s.browser.close().await;
            }
            Step::error(&e.to_string())
        }
    }
}

/// Start (or start again): launch the browser, check for a sign-in from before, else open Google's
/// sign-in and read the first step.
pub async fn start(st: &crate::state::AppState) -> Result<Step, (u16, String)> {
    let mut slot = SESSION.lock().await;
    if let Some(old) = slot.take() {
        old.browser.close().await;
    }
    let Ok(lock) = browser::LOCK.try_lock() else {
        return Err((409, "The browser is busy (a refresh or the live view) – try again in a minute".into()));
    };
    let b = match Browser::launch(st.data_dir()).await {
        Ok(b) => std::sync::Arc::new(b),
        Err(e) => return Err((400, e.to_string())),
    };
    let id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    *slot = Some(Session { id, browser: b.clone(), _lock: lock, last: tokio::time::Instant::now(), step: Step::default(), live: None });
    // from now on the session is closed even if this request goes away half way
    tokio::spawn(reap(id));
    let mut guard = StartGuard(Some(id));
    // already signed in (a profile from before)?
    let r: anyhow::Result<Step> = async {
        b.navigate(&format!("{HOME}/")).await?;
        b.wait_load(STEP_WAIT).await;
        tokio::time::sleep(Duration::from_secs(2)).await;
        if b.logged_in().await {
            return Ok(Step::new("done", "Signed in"));
        }
        open(&b, browser::SIGN_IN_URL, HOME).await
    }.await;
    let step = settle(st, &mut slot, r).await;
    guard.0 = None;
    drop(slot);
    Ok(step)
}

/// One step from the form.
pub async fn step(st: &crate::state::AppState, kind: &str, value: &str) -> Result<Step, (u16, String)> {
    // a live view just closed lets go of the browser a moment later: wait for it a little
    let wait_live = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut slot = loop {
        let slot = SESSION.lock().await;
        let Some(s) = slot.as_ref() else { return Err((409, "The sign-in window is closed – start again".into())) };
        if s.live.is_none() {
            break slot;
        }
        if tokio::time::Instant::now() >= wait_live {
            return Err((409, "The live view is open – close it to go on here".into()));
        }
        drop(slot);
        tokio::time::sleep(Duration::from_millis(200)).await;
    };
    let Some(s) = slot.as_ref() else { return Err((409, "The sign-in window is closed – start again".into())) };
    let (b, cur) = (s.browser.clone(), s.step.clone());
    let r = match submit(&b, &cur, kind, value, HOME).await {
        // Google's page changed under a click or a script: show the page as it is now
        Err(e) if !browser_failed(&e) => advance(&b, None, HOME).await.map(|mut s| {
            if s.kind != "done" && s.error.is_empty() {
                s.error = "Google's page changed – look at it again and try once more".into();
            }
            s
        }),
        r => r,
    };
    Ok(settle(st, &mut slot, r).await)
}

/// The live view takes over the guided session's browser: its id (for `detach`) and the browser
/// (None: there is no guided session). A second live view takes over from the first.
pub async fn attach() -> Option<(u64, std::sync::Arc<Browser>)> {
    let mut slot = SESSION.lock().await;
    let s = slot.as_mut()?;
    let id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    s.live = Some(id);
    s.last = tokio::time::Instant::now();
    Some((id, s.browser.clone()))
}

/// The live view `id` let go of it: signed in there (close the session) or back to the form (unless
/// a newer live view has it now).
pub async fn detach(id: u64, signed_in: bool) {
    let mut slot = SESSION.lock().await;
    if signed_in {
        if let Some(s) = slot.take() {
            s.browser.close().await;
        }
    } else if let Some(s) = slot.as_mut() {
        if s.live != Some(id) {
            return;
        }
        s.live = None;
        s.last = tokio::time::Instant::now();
        let _ = s.browser.page("Page.stopScreencast", json!({})).await;
    }
}

/// Close the session after 10 minutes without a step.
async fn reap(id: u64) {
    loop {
        tokio::time::sleep(Duration::from_secs(20)).await;
        let mut slot = SESSION.lock().await;
        match slot.as_ref() {
            Some(s) if s.id == id => {
                if s.live.is_none() && s.last.elapsed() >= IDLE {
                    if let Some(s) = slot.take() {
                        s.browser.close().await;
                    }
                    return;
                }
            }
            _ => return,
        }
    }
}
