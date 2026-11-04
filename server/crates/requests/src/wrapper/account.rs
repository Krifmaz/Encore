//! wrapper-v2's HTTP control API, the part Encore uses: `GET /health`, `GET /me`, `POST /login`,
//! `POST /login/2fa` and `DELETE /login`. The answers are JSON; `/me` and the sign-in answers carry the
//! account's tokens (`music_user_token`, `dev_token`), which never leave this module: [`Account`] keeps only
//! what a page may show.
//!
//! A sign-in answers 200 when it is done, 202 when Apple wants the code it sent to the owner's devices (then
//! `POST /login/2fa` with the code), 401 when Apple refused it. wrapper-v2 waits up to 30 s for Apple on
//! `/login` and 60 s on `/login/2fa` before it answers.
use serde_json::{json, Value};
use std::time::Duration;

/// The wrapper-v2 API version gamdl 3.9 accepts (`version` in `/me`); gamdl refuses any other.
pub const API_VERSION: &str = "0.0.2";

/// The longest a sign-in step may take: wrapper-v2's own wait for Apple, and some room.
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(90);
/// `/health` and `/me` answer at once from a running wrapper.
const QUICK: Duration = Duration::from_secs(10);

/// Where a sign-in stands, as wrapper-v2 names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// not signed in
    LoggedOut,
    /// Apple is being asked
    InProgress,
    /// Apple sent a code to the owner's devices; `POST /login/2fa` with it
    Awaiting2fa,
    /// signed in: downloads work
    Authenticated,
    /// the last sign-in failed (`error` says why)
    Failed,
    /// a state this version of Encore does not know
    Unknown,
}

impl State {
    /// From wrapper-v2's word (`auth.state`).
    pub fn parse(s: &str) -> State {
        match s {
            "logged_out" => State::LoggedOut,
            "in_progress" => State::InProgress,
            "awaiting_2fa" => State::Awaiting2fa,
            "authenticated" => State::Authenticated,
            "failed" => State::Failed,
            _ => State::Unknown,
        }
    }

    /// wrapper-v2's word for it, as the pages get it.
    pub fn word(self) -> &'static str {
        match self {
            State::LoggedOut => "logged_out",
            State::InProgress => "in_progress",
            State::Awaiting2fa => "awaiting_2fa",
            State::Authenticated => "authenticated",
            State::Failed => "failed",
            State::Unknown => "unknown",
        }
    }
}

/// The account as a page may see it: never a token.
#[derive(Clone, Debug, PartialEq)]
pub struct Account {
    /// the sign-in's state
    pub state: State,
    /// the Apple ID signed in (or being signed in)
    pub apple_id: Option<String>,
    /// the account's country (`us`), once signed in
    pub storefront: Option<String>,
    /// why the last sign-in failed, in Apple's or wrapper-v2's words
    pub error: Option<String>,
    /// the API version the wrapper speaks (`/me`'s `version`; [`API_VERSION`] for the one gamdl needs)
    pub version: Option<String>,
    /// whether the wrapper's Apple libraries loaded (`/me`'s `runtime.loader_ok`): false when the image was
    /// built from the wrong Apple Music version or the container lacks its privileges
    pub loader_ok: Option<bool>,
}

impl Account {
    /// The account part of a wrapper answer: `/me` (`{version, runtime, auth: {...}}`) or a sign-in answer
    /// (the `auth` object itself).
    pub fn from_answer(v: &Value) -> Account {
        let auth = if v["auth"].is_object() { &v["auth"] } else { v };
        let s = |x: &Value| x.as_str().map(str::trim).filter(|t| !t.is_empty()).map(String::from);
        Account {
            state: State::parse(auth["state"].as_str().unwrap_or("")),
            apple_id: s(&auth["apple_id"]).or_else(|| s(&auth["username"])),
            storefront: s(&auth["storefront"]),
            // the supervisor's own failures put their words in `detail` beside an `error` code
            error: s(&v["detail"]).or_else(|| s(&auth["error"])).or_else(|| s(&v["error"])),
            version: s(&v["version"]),
            loader_ok: v["runtime"]["loader_ok"].as_bool(),
        }
    }

    /// As the pages get it.
    pub fn json(&self) -> Value {
        json!({
            "state": self.state.word(),
            "signed_in": self.state == State::Authenticated,
            "apple_id": self.apple_id,
            "storefront": self.storefront,
            "error": self.error,
            "version": self.version,
            "loader_ok": self.loader_ok,
        })
    }

    /// One line about it, for the card's status.
    pub fn line(&self) -> String {
        let who = self.apple_id.as_deref().map(|a| format!(" as {a}")).unwrap_or_default();
        match self.state {
            State::Authenticated => {
                let sf = self.storefront.as_deref().map(|s| format!(" · storefront {s}")).unwrap_or_default();
                format!("Signed in{who}{sf}")
            }
            State::Awaiting2fa => format!("Waiting for the code Apple sent to your devices{who}"),
            State::InProgress => format!("Signing in{who}…"),
            State::Failed => format!("Sign-in failed{who}: {}", self.error.as_deref().unwrap_or("Apple gave no reason")),
            State::LoggedOut => "Runs, not signed in: sign in with your Apple ID".to_string(),
            State::Unknown => "Runs; its sign-in state is unknown".to_string(),
        }
    }
}

/// A base URL without its trailing slash.
fn base(url: &str) -> String {
    url.trim().trim_end_matches('/').to_string()
}

/// A wrapper that does not answer, in words.
fn unreachable(url: &str, e: &reqwest::Error) -> String {
    if e.is_timeout() { format!("The wrapper at {url} did not answer in time") } else { format!("The wrapper at {url} can't be reached: {e}") }
}

/// `GET /health`: the wrapper runs (its version, from the supervisor).
pub async fn health(http: &reqwest::Client, url: &str) -> Result<String, String> {
    let url = base(url);
    let r = http.get(format!("{url}/health")).timeout(QUICK).send().await.map_err(|e| unreachable(&url, &e))?;
    if !r.status().is_success() {
        return Err(format!("The wrapper at {url} answered {} to /health", r.status()));
    }
    let v: Value = r.json().await.unwrap_or(Value::Null);
    Ok(v["version"].as_str().unwrap_or("?").to_string())
}

/// `GET /me`: who is signed in.
pub async fn me(http: &reqwest::Client, url: &str) -> Result<Account, String> {
    let url = base(url);
    let r = http.get(format!("{url}/me")).timeout(QUICK).send().await.map_err(|e| unreachable(&url, &e))?;
    let status = r.status();
    let v: Value = r.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        // the supervisor answers 502 while its Apple worker restarts
        let detail = v["detail"].as_str().or(v["error"].as_str()).unwrap_or("no details");
        return Err(format!("The wrapper at {url} answered {status}: {detail}"));
    }
    Ok(Account::from_answer(&v))
}

/// What a sign-in step came to.
#[derive(Debug, PartialEq)]
pub enum SignIn {
    /// signed in
    Done(Account),
    /// Apple wants the code it sent: [`code`] next
    NeedsCode(Account),
    /// refused (a wrong password or code, an account Apple would not sign in), in words
    Refused(String),
}

/// Read a sign-in answer: 200 done, 202 a code is wanted, anything else refused with the wrapper's words.
fn sign_in_answer(status: u16, v: &Value) -> SignIn {
    let acct = Account::from_answer(v);
    match status {
        200 if acct.state == State::Awaiting2fa => SignIn::NeedsCode(acct),
        200 => SignIn::Done(acct),
        202 => SignIn::NeedsCode(acct),
        _ => {
            let why = acct.error.unwrap_or_else(|| format!("the wrapper answered {status}"));
            SignIn::Refused(if status == 401 { format!("Apple refused the sign-in: {why}") } else { format!("The sign-in failed: {why}") })
        }
    }
}

/// `POST /login` with the Apple ID and its password. They go to the wrapper only; Encore keeps neither.
pub async fn login(http: &reqwest::Client, url: &str, apple_id: &str, password: &str) -> Result<SignIn, String> {
    let url = base(url);
    let r = http.post(format!("{url}/login")).json(&json!({ "username": apple_id.trim(), "password": password }))
        .timeout(SIGN_IN_TIMEOUT).send().await.map_err(|e| unreachable(&url, &e))?;
    let status = r.status().as_u16();
    let v: Value = r.json().await.unwrap_or(Value::Null);
    Ok(sign_in_answer(status, &v))
}

/// `POST /login/2fa` with the code Apple sent to the owner's devices.
pub async fn code(http: &reqwest::Client, url: &str, code: &str) -> Result<SignIn, String> {
    let url = base(url);
    // the six digits, whatever was typed around them ("123 456", "123-456")
    let digits: String = code.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() != 6 {
        return Ok(SignIn::Refused("The code is the six digits Apple shows on your iPhone, iPad or Mac".into()));
    }
    let r = http.post(format!("{url}/login/2fa")).json(&json!({ "code": digits })).timeout(SIGN_IN_TIMEOUT).send().await.map_err(|e| unreachable(&url, &e))?;
    let status = r.status().as_u16();
    let v: Value = r.json().await.unwrap_or(Value::Null);
    Ok(sign_in_answer(status, &v))
}

/// `DELETE /login`: stops a sign-in in progress or forgets the tokens the wrapper holds. The session it saved
/// on disk stays until the container's volume goes (wrapper-v2 restores it at its next start).
pub async fn logout(http: &reqwest::Client, url: &str) -> Result<(), String> {
    let url = base(url);
    let r = http.delete(format!("{url}/login")).timeout(QUICK).send().await.map_err(|e| unreachable(&url, &e))?;
    if r.status().is_success() { Ok(()) } else { Err(format!("The wrapper at {url} answered {} to the sign-out", r.status())) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_account_never_carries_a_token() {
        let me = json!({
            "version": "0.0.2",
            "runtime": { "loader_ok": true, "initialized": true },
            "auth": { "state": "authenticated", "apple_id": "me@example.com", "storefront": "us", "music_user_token": "SECRET", "dev_token": "SECRET2" }
        });
        let a = Account::from_answer(&me);
        assert_eq!(a.state, State::Authenticated);
        assert_eq!(a.apple_id.as_deref(), Some("me@example.com"));
        assert_eq!(a.storefront.as_deref(), Some("us"));
        assert_eq!(a.version.as_deref(), Some(API_VERSION));
        assert_eq!(a.loader_ok, Some(true));
        let shown = a.json().to_string();
        assert!(!shown.contains("SECRET"), "{shown}");
        assert_eq!(a.line(), "Signed in as me@example.com · storefront us");
    }

    #[test]
    fn sign_in_answers_read_as_done_code_or_refused() {
        let ok = json!({ "state": "authenticated", "apple_id": "a@b.c" });
        assert!(matches!(sign_in_answer(200, &ok), SignIn::Done(a) if a.apple_id.as_deref() == Some("a@b.c")));
        let code = json!({ "state": "awaiting_2fa", "apple_id": "a@b.c" });
        assert!(matches!(sign_in_answer(202, &code), SignIn::NeedsCode(_)));
        // a 200 that still waits for the code is not done
        assert!(matches!(sign_in_answer(200, &code), SignIn::NeedsCode(_)));
        let bad = json!({ "state": "failed", "error": "invalid credentials" });
        assert_eq!(sign_in_answer(401, &bad), SignIn::Refused("Apple refused the sign-in: invalid credentials".into()));
        let worker = json!({ "error": "worker_unavailable", "detail": "ipc timeout" });
        assert_eq!(sign_in_answer(502, &worker), SignIn::Refused("The sign-in failed: ipc timeout".into()));
    }

    #[test]
    fn states_round_trip() {
        for w in ["logged_out", "in_progress", "awaiting_2fa", "authenticated", "failed"] {
            assert_eq!(State::parse(w).word(), w);
        }
        assert_eq!(State::parse("??"), State::Unknown);
    }
}
