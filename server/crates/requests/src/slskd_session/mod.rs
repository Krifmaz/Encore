//! The Soulseek session: whether slskd is logged in to the Soulseek server, Encore's own reconnect
//! schedule, and the gate every search goes through.
//!
//! slskd (0.26) reconnects by itself: its ConnectionWatchdog tries again 0–1 s, 1 s, 3 s, 7 s … after a
//! failure (up to 5 minutes apart), starts over at once after every session that drops, and can't be told
//! otherwise. A Soulseek server that ignores logins from an address that knocks too often then never lets
//! it in. So Encore decides when slskd may reach the server, on its own schedule (30 s, 1 m, 2 m, 5 m,
//! 10 m, then every 15 m, each ±20 %):
//! - slskd reaches the server only through Encore ([`door`]): a relay on loopback, or Encore's SOCKS
//!   proxy into the VPN. The door lets one connection through per attempt of Encore's; slskd's own
//!   attempts fail on loopback in milliseconds and never reach the server.
//! - slskd.yml says `flags.no_connect: true`: slskd doesn't connect when it starts. Encore connects with
//!   PUT /api/v0/server (the door opened just before) and follows the attempt in slskd's log ("Logged in
//!   …", "Disconnected from the Soulseek server: …") and, as a backstop, in GET /api/v0/application and
//!   the door (the connection it let through has closed).
//! - When an attempt fails or a session drops, slskd's own loop goes on knocking at the shut door; Encore
//!   quiets it with DELETE /api/v0/server once it waits between two attempts ("Waiting about …"), which
//!   stops it without a word. (A DELETE while slskd is still connecting would start the loop again at
//!   once: slskd counts that cancel as an ordinary disconnect. Behind the door that is harmless, and
//!   Encore doesn't do it.)
//! - A new slskd process (changed settings, a new VPN port, a crash) ends the session without a reason
//!   of the server's: the next attempt goes as soon as the 30 s gap allows. The count, the last and the
//!   next attempt are kept in a small file beside slskd.yml, so a restart of Encore keeps them too.
//!
//! Searches: one at a time, each starting at least 3 s after the one before; never while the session is
//! down (the request worker parks its requests until it is back, people get "isn't connected" at once);
//! a "too many searches" answer means waiting 60 s ([`gate`]).
pub mod door;
mod gate;
mod schedule;
mod supervisor;

pub use door::{DoorSeen, Pass, DOOR_WINDOW};
pub use gate::{Turn, Who, PERSON_WAIT, RATE_LIMIT_PAUSE, RATE_LIMIT_TRIES, SPACING};
pub use schedule::{backoff, classify, describe, jittered, read_line, span, AttemptNow, Conn, Line, Reason, Refused, State, ATTEMPT_MAX, KICK_PAUSE, MIN_GAP, PRESS_GAP, STABLE};
pub use supervisor::supervisor;

use crate::Ctx;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tokio::sync::Notify;
use tokio::time::Instant;

/// How old the known session state may be before a search reads it again.
pub const FRESH: Duration = Duration::from_secs(10);
/// A request waits this long for Soulseek at most, then goes on down its sources.
pub const WORKER_WAIT_MAX: f64 = 3600.0;

/// The Soulseek session is down. The request worker parks its request; a person's search is refused at
/// once with this message ("Soulseek isn't connected – next attempt at 14:20").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotConnected(pub String);

impl std::fmt::Display for NotConnected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for NotConnected {}

/// A person's search could not run now: another search held the turn for 30 s, or the server is turning
/// searches away for longer than a person should wait. The message says which and until when.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Busy(pub String);

impl std::fmt::Display for Busy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Busy {}

/// The session's state and the search gate. The server keeps one (in `slskd_managed::Managed`); clones
/// share it.
#[derive(Clone, Default)]
pub struct Session(Arc<Inner>);

/// The session's shared state.
#[derive(Default)]
struct Inner {
    /// the connection and its schedule
    conn: Mutex<Conn>,
    /// wakes the supervisor (a log line, a search that found the session gone, Reconnect now)
    wake: Notify,
    /// what slskd's log said, and when, for the supervisor
    lines: Mutex<VecDeque<(Line, Instant)>>,
    /// the door to the Soulseek server
    door: Mutex<door::DoorState>,
    /// one search at a time
    gate: gate::Limiter,
    /// one read of the server state at a time (many parked requests may find it old together)
    refreshing: tokio::sync::Mutex<()>,
    /// one download handed to slskd at a time (it answers 429 to more than two at once)
    enqueuing: tokio::sync::Mutex<()>,
}

/// The time of day `unix` falls on, on this server's clock ("14:20").
pub fn clock(unix: f64) -> String {
    let t = unix as libc::time_t;
    // SAFETY: localtime_r reads `t` and writes only the struct handed to it, which lives on this stack.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let ok = unsafe { !libc::localtime_r(&t, &mut tm).is_null() };
    if ok {
        format!("{:02}:{:02}", tm.tm_hour, tm.tm_min)
    } else {
        let s = (unix.max(0.0) as i64).rem_euclid(86_400);
        format!("{:02}:{:02}", s / 3600, (s % 3600) / 60)
    }
}

/// The unix time a moment of the runtime's clock falls on.
pub fn unix_at(at: Instant) -> f64 {
    let now = Instant::now();
    let unix = encore_core::time::now();
    if at >= now { unix + at.duration_since(now).as_secs_f64() } else { unix - now.duration_since(at).as_secs_f64() }
}

impl Session {
    /// The connection state, locked.
    fn conn(&self) -> MutexGuard<'_, Conn> {
        self.0.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A copy of the session's state.
    pub fn snapshot(&self) -> Conn {
        self.conn().clone()
    }

    /// Wake the supervisor now.
    pub fn wake(&self) {
        self.0.wake.notify_one();
    }

    /// The turn to hand a download to slskd (POST /transfers/downloads), held while slskd takes it.
    pub async fn enqueue_turn(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.0.enqueuing.lock().await
    }

    /// A line of slskd's log: what it says about the session goes to the supervisor.
    pub fn on_line(&self, l: &str) {
        if let Some(line) = read_line(l) {
            self.0.lines.lock().unwrap_or_else(|e| e.into_inner()).push_back((line, Instant::now()));
            self.wake();
        }
    }

    /// The log lines heard since the supervisor last looked.
    fn take_lines(&self) -> Vec<(Line, Instant)> {
        self.0.lines.lock().unwrap_or_else(|e| e.into_inner()).drain(..).collect()
    }

    /// One state change, one line in the log.
    fn note(line: Option<String>) {
        if let Some(l) = line {
            tracing::info!("{l}");
        }
    }

    /// A search answer said the session is down (409, the "must be connected and logged in" error, slskd
    /// not answering): no more searches until the supervisor has looked, which it does at once. The state
    /// itself is the supervisor's to change: the answer may be about a session that is already over.
    pub fn lost(&self) {
        {
            let mut c = self.conn();
            c.checked = None;
            if c.state == State::LoggedIn {
                c.lost = true;
            }
        }
        self.wake();
    }

    /// Reconnect now (an admin's button): refused while logged in or connecting, and within 30 s of the
    /// last attempt (how long to wait).
    pub fn reconnect_now(&self) -> Result<(), Refused> {
        let r = self.conn().reconnect_now(Instant::now(), encore_core::time::now());
        if r.is_ok() {
            self.wake();
        }
        r
    }

    /// Download now on a request waiting for Soulseek ([`Conn::attempt_now`]): the planned attempt due now
    /// where Encore's own schedule holds the session (the supervisor is woken), else when it can be.
    pub fn attempt_now(&self) -> AttemptNow {
        let r = self.conn().attempt_now(Instant::now());
        if r == AttemptNow::Due {
            tracing::info!("soulseek: an attempt is due now (Download now)");
            self.wake();
        }
        r
    }

    /// What GET /api/v0/server (or the `server` part of /application) says: logged in or not. Takes a
    /// session found logged in; marks one found gone (the supervisor handles the rest).
    pub fn observe_server(&self, server: &Value, user: Option<&str>) -> bool {
        let logged_in = server["isLoggedIn"].as_bool().unwrap_or_else(|| server["state"].as_str().unwrap_or("").contains("LoggedIn"));
        let now = Instant::now();
        let mut c = self.conn();
        c.checked = Some(now);
        if logged_in {
            let user = user.map(String::from).or_else(|| c.username.clone()).unwrap_or_default();
            let line = c.logged_in(&user, now, encore_core::time::now());
            drop(c);
            Self::note(line);
        } else if c.state == State::LoggedIn {
            c.lost = true;
            drop(c);
            self.wake();
        }
        logged_in
    }

    /// Read the server state now when what is known is older than `FRESH` (only the states the gate may
    /// act on: logged in, or not connected with nothing planned; the others are the supervisor's).
    pub async fn refresh_if_old(&self, st: &impl Ctx) {
        let old = |c: &Conn| matches!(c.state, State::LoggedIn | State::Disconnected) && !c.lost
            && c.checked.is_none_or(|t| t.elapsed() >= FRESH);
        if !old(&self.conn()) {
            return;
        }
        let _one = self.0.refreshing.lock().await;
        if !old(&self.conn()) {
            return; // read meanwhile by another caller
        }
        match crate::slskd_managed::api_json(st, reqwest::Method::GET, "server", None).await {
            Ok(v) => {
                let user = st.cfg().str("soulseek_username");
                self.observe_server(&v, Some(user.trim()));
            }
            Err(e) => {
                // slskd doesn't answer (it is restarting, or busy): what was known no longer counts
                tracing::debug!("soulseek: couldn't read the server state: {e}");
                self.unreachable();
            }
        }
    }

    /// slskd's API didn't answer: a session believed logged in is in doubt (the gate refuses until the
    /// supervisor knows), and the next look comes at the usual pace, not at once.
    pub(crate) fn unreachable(&self) {
        let mut c = self.conn();
        c.checked = Some(Instant::now());
        if c.state == State::LoggedIn && !c.lost {
            c.lost = true;
            drop(c);
            self.wake();
        }
    }

    /// Logged in, as far as is known now (read again when old): Ok, else why not in words for a person.
    pub async fn check(&self, st: &impl Ctx) -> Result<(), NotConnected> {
        if let Some(why) = crate::sources::slskd::not_ready(st) {
            return Err(NotConnected(why));
        }
        self.refresh_if_old(st).await;
        let c = self.conn();
        if c.state == State::LoggedIn && !c.lost {
            return Ok(());
        }
        Err(NotConnected(format!("Soulseek isn't connected – {}", when_text(&c))))
    }

    /// Whether the request worker should search Soulseek now, wait for it, or go on to its next source.
    pub async fn for_worker(&self, st: &impl Ctx) -> ForWorker {
        if let Some(why) = crate::sources::slskd::not_ready(st) {
            return match st.requests().slskd.state() {
                // short and passing by themselves: the VPN comes back, a stopped engine is started again
                "waiting_vpn" => ForWorker::Wait("waiting for the VPN".into()),
                "crashed" => ForWorker::Wait("the Soulseek engine stopped and is started again shortly".into()),
                // off, not installed, can't start: as before
                _ => ForWorker::Skip(why),
            };
        }
        if st.requests().slskd.state() != "running" {
            return ForWorker::Wait("the Soulseek engine is starting".into());
        }
        self.refresh_if_old(st).await;
        let c = self.conn();
        match c.state {
            State::LoggedIn if !c.lost => ForWorker::Go,
            State::Stopped if c.reason.is_some() => ForWorker::Skip(format!("Soulseek stopped connecting – {}", describe(c.reason.unwrap(), c.message.as_deref()))),
            State::Stopped => ForWorker::Skip(c.message.clone().unwrap_or_else(|| "Soulseek is off".into())),
            _ => ForWorker::Wait(when_text(&c)),
        }
    }

    /// A search holds the turn right now.
    pub fn searching(&self) -> bool {
        self.0.gate.busy()
    }

    /// Searches waiting for their turn right now.
    pub fn searches_queued(&self) -> usize {
        self.0.gate.queued()
    }
}

/// The worker's answer from [`Session::for_worker`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForWorker {
    /// go ahead
    Go,
    /// wait (in words: "next attempt at 14:20", "connecting now")
    Wait(String),
    /// go on to the next source, for this reason
    Skip(String),
}

/// When the session should be back, in words: "next attempt at 14:20", "connecting now".
pub fn when_text(c: &Conn) -> String {
    match c.state {
        State::Connecting => "connecting now".into(),
        State::Waiting => match c.next_at {
            Some(t) => format!("next attempt at {}", clock(unix_at(t))),
            None => "connecting shortly".into(),
        },
        State::Stopped => match c.reason {
            Some(Reason::Rejected) => format!("the login was rejected: {}", rejected(c, " – fix it in Settings → Plugins → Soulseek")),
            Some(r) => format!("{} – press Reconnect now in Settings → Plugins → Soulseek", describe(r, c.message.as_deref())),
            None => c.message.clone().unwrap_or_else(|| "Soulseek is off".into()),
        },
        State::Disconnected if c.message.is_some() => c.message.clone().unwrap_or_default(),
        State::Disconnected => match c.next_at.filter(|t| t.saturating_duration_since(Instant::now()) > Duration::from_secs(5)) {
            Some(t) => format!("next attempt at {}", clock(unix_at(t))),
            None => "connecting shortly".into(),
        },
        _ => "connecting shortly".into(),
    }
}

/// Why the session isn't up and when it should be: "the login timed out (…) – next attempt at 14:20".
pub fn why_not(c: &Conn) -> String {
    match (c.state, c.reason) {
        (State::Waiting, Some(r)) => format!("{} – {}", describe(r, c.message.as_deref()), when_text(c)),
        _ => when_text(c),
    }
}

/// The card's sentence: "Logged in as alice since 14:02", "Disconnected – the login timed out (the
/// Soulseek server did not answer in 5 s). Next attempt at 14:20 (attempt 3)", "Rejected: invalid
/// username or password – fix them below".
pub fn sentence(c: &Conn) -> String {
    let next = |c: &Conn| match c.next_at {
        Some(t) => format!(" Next attempt at {} (attempt {}).", clock(unix_at(t)), c.attempts + 1),
        None => String::new(),
    };
    match (c.state, c.reason) {
        (State::LoggedIn, _) => {
            let mut s = format!("Logged in as {} since {}", c.username.as_deref().unwrap_or("?"), clock(c.since));
            if let Some(t) = c.rate_limited_until.filter(|t| *t > Instant::now()) {
                s += &format!(". Rate limited by the Soulseek server – next search at {}", clock(unix_at(t)));
            }
            s
        }
        (State::Connecting, _) => format!("Connecting to the Soulseek server (attempt {})…", c.attempts),
        (State::Waiting, Some(r)) => {
            let mut s = format!("Disconnected – {}.", describe(r, c.message.as_deref()));
            if r == Reason::Kicked {
                s += " Encore waits 15 minutes and tries once more; close the other client or use another username.";
            }
            s + &next(c)
        }
        (State::Stopped, Some(Reason::Rejected)) => format!("Rejected: {}", rejected(c, " – fix them below")),
        (State::Stopped, Some(Reason::Banned)) => format!("Stopped: {}. Encore does not connect again until you press Reconnect now.", describe(Reason::Banned, c.message.as_deref())),
        (State::Stopped, Some(r)) => format!("Stopped: {} again. Close the other client, then press Reconnect now.", describe(r, c.message.as_deref())),
        (State::Stopped, None) => c.message.clone().unwrap_or_else(|| "Soulseek is off".into()),
        (_, Some(r)) => format!("Disconnected – {}.{}", describe(r, c.message.as_deref()), next(c)),
        (_, None) => {
            let when = when_text(c);
            let n = if when.starts_with("next attempt at ") { format!(" (attempt {})", c.attempts + 1) } else { String::new() };
            format!("Not connected – {when}{n}")
        }
    }
}

/// A rejected login in words, with `fix` (where to put it right) when it was the username or password:
/// the server's other refusals (this engine's version, a username it doesn't allow) say what to do.
fn rejected(c: &Conn, fix: &str) -> String {
    let what = describe(Reason::Rejected, c.message.as_deref());
    if what == describe(Reason::Rejected, None) { what + fix } else { what }
}

/// Requests parked for the session, counted while it is coming back: once it stopped for good (a rejected
/// login, a ban, Soulseek off) they go on to their next source at the worker's next look instead.
pub fn parked_waiting(st: &impl Ctx, c: &Conn) -> i64 {
    if matches!(c.state, State::Waiting | State::Disconnected | State::Connecting) { parked(st) } else { 0 }
}

/// The card's sentence away from the card (Diagnostics, the phone, older apps): "fix them below" points
/// at the card's fields, so it names the card instead.
pub fn text_elsewhere(text: &str) -> String {
    text.replace(" – fix them below", " – fix them in Settings → Plugins → Soulseek")
}

/// Requests parked until Soulseek is back.
pub fn parked(st: &impl Ctx) -> i64 {
    st.db().one("SELECT COUNT(*) AS n FROM requests WHERE status IN ('waiting','queued') AND json_extract(detail, '$.soulseek_wait') IS NOT NULL
                 AND json_extract(detail, '$.in_line') IS NULL", encore_db::p![])
        .ok().flatten().map(|r| encore_db::RowExt::i(&r, "n")).unwrap_or(0)
}

/// GET /api/system/soulseek: {state: connecting | logged_in | disconnected | waiting | stopped, reason:
/// timeout | rejected | kicked | banned | rate_limited | network | null, message (slskd's or the server's
/// words), since, next_attempt_at, attempts, last_search_at, searches_waiting, search_running, username,
/// next_search_at, reconnect_in (seconds until Reconnect now is allowed), can_reconnect, text (the card's
/// sentence)}. Times are unix seconds.
pub fn status(st: &impl Ctx) -> Value {
    let s = st.requests().slskd.session();
    let c = s.snapshot();
    let now = Instant::now();
    let limited = c.rate_limited_until.filter(|t| *t > now);
    let reason = match (c.state, limited) {
        (State::LoggedIn, Some(_)) => Some(Reason::RateLimited),
        _ => c.reason,
    };
    let reconnect_in = c.last_attempt.map(|t| (t + MIN_GAP).saturating_duration_since(now).as_secs_f64().ceil() as u64).unwrap_or(0);
    let waiting = parked_waiting(st, &c) as usize + s.searches_queued();
    json!({
        "state": c.state.as_str(),
        "reason": reason.map(|r| r.as_str()),
        "message": c.message,
        "since": (c.since > 0.0).then_some(c.since),
        "next_attempt_at": c.next_at.filter(|_| matches!(c.state, State::Waiting | State::Disconnected)).map(unix_at),
        "attempts": c.attempts,
        "last_search_at": c.last_search_at,
        "searches_waiting": waiting,
        // a search holds the turn: a person's search waits for it (up to 30 s)
        "search_running": s.searching(),
        "username": c.username,
        "next_search_at": limited.map(unix_at),
        "reconnect_in": reconnect_in,
        "can_reconnect": !matches!(c.state, State::LoggedIn | State::Connecting) && st.requests().slskd.state() == "running",
        // connections the door let through to the Soulseek server, and turned away (slskd's own attempts)
        "server_connections": s.door_seen().passed,
        "turned_away": s.door_seen().refused,
        "text": sentence(&c),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sentences_for_the_card() {
        let now = Instant::now();
        let mut c = Conn::default();
        c.engine_up(now, 0.0);
        c.attempt_begins(now, 0.0);
        assert_eq!(sentence(&c), "Connecting to the Soulseek server (attempt 1)…");
        c.dropped(Reason::Timeout, "The wait timed out after 5000 milliseconds", now, 0.0, 0.0);
        let s = sentence(&c);
        assert!(s.starts_with("Disconnected – the login timed out (the Soulseek server did not answer in 5 s). Next attempt at "), "{s}");
        assert!(s.ends_with("(attempt 2)."), "{s}");
        c.dropped(Reason::Rejected, "The server rejected login attempt: INVALIDPASS", now, 0.0, 0.0);
        assert_eq!(sentence(&c), "Rejected: invalid username or password – fix them below");
        assert!(when_text(&c).contains("fix it in Settings"));
        // away from the card (Diagnostics, the phone): the card is named instead of "below"
        assert_eq!(text_elsewhere(&sentence(&c)), "Rejected: invalid username or password – fix them in Settings → Plugins → Soulseek");
        assert_eq!((supervisor::searches(1), supervisor::searches(37)), ("1 search".to_string(), "37 searches".to_string()));
        // the server's other refusals say what to do, without pointing at the username and password
        let mut v = c.clone();
        v.message = Some("The server rejected login attempt: INVALIDVERSION".into());
        assert_eq!(sentence(&v), "Rejected: the Soulseek server refused this engine version – update the Soulseek plugin under Settings → Plugins");
        assert_eq!(when_text(&v), "the login was rejected: the Soulseek server refused this engine version – update the Soulseek plugin under Settings → Plugins");
        v.message = Some("The server rejected login attempt: INVALIDUSERNAME".into());
        assert_eq!(sentence(&v), "Rejected: the Soulseek server does not allow this username – choose another");
        c.reconnect_now(now + MIN_GAP, 0.0).unwrap();
        let s = sentence(&c);
        assert!(s.starts_with("Not connected – next attempt at ") && s.ends_with(" (attempt 1)"), "{s}");
        c.attempt_begins(now + MIN_GAP, 0.0);
        c.logged_in("alice", now + MIN_GAP, 1_760_000_000.0);
        assert_eq!(sentence(&c), format!("Logged in as alice since {}", clock(1_760_000_000.0)));
        assert_eq!(clock(3600.0 * 14.0 + 1200.0).len(), 5);
    }

    /// A stale "not connected" answer (a search posted under a session that has since been replaced):
    /// the session stays as it is – no new "logged in" line, the 10-minute clock keeps running – until the
    /// supervisor has looked; the gate refuses meanwhile.
    #[tokio::test(start_paused = true)]
    async fn a_stale_not_connected_answer_changes_nothing_until_the_supervisor_looks() {
        let s = Session::default();
        let t0 = Instant::now();
        {
            let mut c = s.conn();
            c.engine_up(t0, 0.0);
            c.attempt_begins(t0, 0.0);
            c.logged_in("alice", t0, 100.0);
        }
        tokio::time::sleep(Duration::from_secs(300)).await;
        s.lost();
        let c = s.snapshot();
        assert_eq!((c.state, c.since, c.logged_in_at), (State::LoggedIn, 100.0, Some(t0)), "state, since and the stable clock unchanged");
        assert!(c.lost && c.checked.is_none(), "the gate refuses and the supervisor looks at once");
        // the supervisor finds slskd logged in: no new line, nothing reset
        assert!(s.observe_server(&json!({ "state": "Connected, LoggedIn", "isLoggedIn": true }), Some("alice")));
        let c = s.snapshot();
        assert_eq!((c.state, c.lost, c.since, c.logged_in_at), (State::LoggedIn, false, 100.0, Some(t0)));
    }
}
