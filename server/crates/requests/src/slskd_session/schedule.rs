//! The pure parts of the Soulseek session: Encore's reconnect schedule, what slskd's log lines say about
//! the session, and the session's state with its changes. Nothing here does I/O or reads a clock: every
//! function is handed the time, so the tests walk through hours of reconnects at once.
use std::time::Duration;
use tokio::time::Instant;

/// Never two connection attempts closer than this (also the gap Reconnect now keeps).
pub const MIN_GAP: Duration = Duration::from_secs(30);
/// A session that stayed logged in this long starts the schedule over when it drops.
pub const STABLE: Duration = Duration::from_secs(600);
/// After "another client logged in with the same name": wait this long, then try once more.
pub const KICK_PAUSE: Duration = Duration::from_secs(900);
/// How far each wait may stray from the schedule (±20 %), so many servers don't knock in step.
pub const JITTER: f64 = 0.2;
/// An attempt that has neither logged in nor failed after this long is called off as timed out
/// (slskd's connect timeout is 30 s, its login wait 5 s; the rest is margin).
pub const ATTEMPT_MAX: Duration = Duration::from_secs(75);
/// The waits before the 2nd, 3rd … attempt of a run of failures: 30 s, 1 m, 2 m, 5 m, 10 m, then 15 m.
const SCHEDULE: [u64; 6] = [30, 60, 120, 300, 600, 900];
/// Download now makes the planned attempt due at most this often, so pressing can't drive the Soulseek
/// server harder than every 2 minutes.
pub const PRESS_GAP: Duration = Duration::from_secs(120);

/// What Download now could do about the session ([`Conn::attempt_now`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttemptNow {
    /// the planned attempt is due now (the schedule's count stays: a new failure backs off further)
    Due,
    /// already logged in
    LoggedIn,
    /// an attempt is under way (or the supervisor is looking at a session that may be gone)
    Connecting,
    /// Encore doesn't connect (a rejected login, a ban, kicked again, Soulseek off): the request goes on to
    /// its next source
    Stopped,
    /// not before then: the Soulseek server asked Encore to wait (it didn't answer the login, another
    /// client took the username), the last attempt was less than 30 s ago, or a press made one due less than
    /// 2 minutes ago
    Later(Instant),
}

/// The wait after the `n`-th failure in a row (n ≥ 1; 0 counts as 1).
pub fn backoff(n: u32) -> Duration {
    let i = (n.max(1) as usize - 1).min(SCHEDULE.len() - 1);
    Duration::from_secs(SCHEDULE[i])
}

/// `d` moved by `r` × 20 % (`r` between -1 and 1).
pub fn jittered(d: Duration, r: f64) -> Duration {
    d.mul_f64(1.0 + JITTER * r.clamp(-1.0, 1.0))
}

/// "2 m 10 s", "45 s", "15 m".
pub fn span(d: Duration) -> String {
    let s = d.as_secs_f64().round() as u64;
    match (s / 60, s % 60) {
        (0, s) => format!("{s} s"),
        (m, 0) => format!("{m} m"),
        (m, s) => format!("{m} m {s} s"),
    }
}

/// Where the session is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// An attempt is under way.
    Connecting,
    /// logged in to Soulseek
    LoggedIn,
    /// Not logged in and nothing planned yet (the engine has just come up, or the session was just seen
    /// to end): an attempt follows shortly.
    Disconnected,
    /// Not logged in for a reason worth trying again: the next attempt is planned.
    Waiting,
    /// Encore does not connect: Soulseek is off or its engine isn't running (no reason), or the server
    /// refused for good (rejected, banned, kicked again) until the settings change or Reconnect now.
    Stopped,
}

impl State {
    /// Its name in the API.
    pub fn as_str(self) -> &'static str {
        match self {
            State::Connecting => "connecting",
            State::LoggedIn => "logged_in",
            State::Disconnected => "disconnected",
            State::Waiting => "waiting",
            State::Stopped => "stopped",
        }
    }
}

/// Why the session isn't up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    /// The server did not answer the connect or the login in time.
    Timeout,
    /// The server refused the login (invalid username or password, or this engine's version).
    Rejected,
    /// Another client logged in with the same username.
    Kicked,
    /// The server says this account or address is banned.
    Banned,
    /// Searches are turned away for now (too many, too fast).
    RateLimited,
    /// The connection could not be made or was lost.
    Network,
}

impl Reason {
    /// The reason a stored name stands for.
    pub fn parse(s: &str) -> Option<Reason> {
        [Reason::Timeout, Reason::Rejected, Reason::Kicked, Reason::Banned, Reason::RateLimited, Reason::Network].into_iter().find(|r| r.as_str() == s)
    }

    /// Its name in the API.
    pub fn as_str(self) -> &'static str {
        match self {
            Reason::Timeout => "timeout",
            Reason::Rejected => "rejected",
            Reason::Kicked => "kicked",
            Reason::Banned => "banned",
            Reason::RateLimited => "rate_limited",
            Reason::Network => "network",
        }
    }
}

/// What one line of slskd's log says about the session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Line {
    /// "Logged in to the Soulseek server as X"
    LoggedIn(String),
    /// An attempt failed or the session ended, and why (slskd's or the server's words).
    Dropped(Reason, String),
    /// A connection under way was cancelled ("Operation cancelled"): the echo of Encore's own PUT or
    /// DELETE, never the server's verdict.
    Cancelled,
    /// slskd's own reconnect loop starts an attempt ("Attempting to connect to the Soulseek server…").
    Attempting,
    /// slskd's own reconnect loop waits before its next attempt ("Waiting about N seconds before
    /// attempting to reconnect"): a DELETE now stops it without a word.
    Waiting,
    /// slskd is shutting down (it is being restarted or stopped).
    EngineStopping,
}
/// The reason in a failure message of slskd or Soulseek.NET.
pub fn classify(msg: &str) -> Reason {
    let l = msg.to_lowercase();
    if l.contains("banned") || l.contains(" ban ") {
        Reason::Banned
    } else if l.contains("invalid username or password") || l.contains("invalidpass") || l.contains("invalidusername") || l.contains("invalidversion")
        || l.contains("rejected login") {
        Reason::Rejected
    } else if l.contains("another client") || l.contains("same username") || l.contains("kicked") || l.contains("relogged") {
        Reason::Kicked
    } else if l.contains("timed out") || l.contains("timeout") {
        Reason::Timeout
    } else {
        Reason::Network
    }
}

/// `msg` with surrounding blanks and a final full stop gone, at most 200 characters.
fn words(msg: &str) -> String {
    msg.trim().trim_end_matches('.').chars().take(200).collect()
}

/// A cancelled connection in .NET's words ("Operation cancelled", "The operation was canceled.").
fn cancelled(l: &str) -> bool {
    l.contains("operation cancelled") || l.contains("operation was canceled") || l.contains("task was canceled") || l.contains("operation was cancelled")
}

/// What a line of slskd's log says about the session (None for every other line, and for the session
/// ending because Encore asked for it: "disconnected by the user").
pub fn read_line(line: &str) -> Option<Line> {
    const LOGGED_IN: &str = "Logged in to the Soulseek server as ";
    const DISCONNECTED: &str = "Disconnected from the Soulseek server";
    const FAILED: &str = "Failed to reconnect: ";
    if let Some(i) = line.find(LOGGED_IN) {
        return Some(Line::LoggedIn(line[i + LOGGED_IN.len()..].trim().to_string()));
    }
    if line.contains("Application is shutting down") {
        return Some(Line::EngineStopping);
    }
    if let Some(i) = line.find(DISCONNECTED) {
        let rest = line[i + DISCONNECTED.len()..].trim_start_matches([':', ';', ' ']);
        let l = rest.to_lowercase();
        if l.starts_with("disconnected by the user") {
            return None;
        }
        if l.contains("shutting down") {
            return Some(Line::EngineStopping);
        }
        if cancelled(&l) {
            return Some(Line::Cancelled);
        }
        return Some(Line::Dropped(classify(rest), words(rest)));
    }
    if let Some(i) = line.find(FAILED) {
        let rest = &line[i + FAILED.len()..];
        if cancelled(&rest.to_lowercase()) {
            return Some(Line::Cancelled);
        }
        return Some(Line::Dropped(classify(rest), words(rest)));
    }
    if line.contains("Attempting to connect to the Soulseek server") {
        return Some(Line::Attempting);
    }
    if line.contains("Waiting about") && line.contains("before attempting to reconnect") {
        return Some(Line::Waiting);
    }
    None
}

/// The login wait in a timeout message ("The wait timed out after 5000 milliseconds" → 5 s).
pub fn waited(msg: &str) -> Option<Duration> {
    let l = msg.to_lowercase();
    let at = l.find("after ")? + 6;
    let n: String = l[at..].chars().take_while(|c| c.is_ascii_digit()).collect();
    let n: u64 = n.parse().ok()?;
    let unit = l[at + n.to_string().len()..].trim_start();
    if unit.starts_with("ms") || unit.starts_with("millisecond") {
        Some(Duration::from_millis(n))
    } else if unit.starts_with('s') {
        Some(Duration::from_secs(n))
    } else {
        None
    }
}

/// The session as Encore sees it, and its schedule. Changes return the one log line they are worth.
#[derive(Clone, Debug)]
pub struct Conn {
    /// where the session is
    pub state: State,
    /// why it isn't logged in, when known
    pub reason: Option<Reason>,
    /// slskd's or the server's words about the reason (or what keeps the engine from running)
    pub message: Option<String>,
    /// unix time of the last change of state
    pub since: f64,
    /// connection attempts made since the schedule last started over
    pub attempts: u32,
    /// when the next attempt may start
    pub next_at: Option<Instant>,
    /// when the last attempt started
    pub last_attempt: Option<Instant>,
    /// the attempt under way started then
    pub attempt_started: Option<Instant>,
    /// when it logged in
    pub logged_in_at: Option<Instant>,
    /// the Soulseek user it logged in as
    pub username: Option<String>,
    /// the one retry after "another client logged in" has been used
    pub kicked_retry: bool,
    /// the server state was last read then
    pub checked: Option<Instant>,
    /// a search answer said the session is gone; the supervisor looks at once
    pub lost: bool,
    /// the last failure slskd's log named, while the supervisor had not handled it yet
    pub last_line: Option<(Reason, String)>,
    /// searches wait until then (the server turned them away)
    pub rate_limited_until: Option<Instant>,
    /// unix time of the last search Encore started
    pub last_search_at: Option<f64>,
    /// the reason (and words) the planned attempt was for, kept while the engine is down: the attempt
    /// stays planned, and once the engine is back the card says why again, not "connecting shortly"
    pub held: Option<(Reason, Option<String>)>,
    /// when Download now last made an attempt due ([`PRESS_GAP`])
    pub last_press: Option<Instant>,
}

impl Default for Conn {
    fn default() -> Self {
        Conn {
            state: State::Disconnected, reason: None, message: None, since: 0.0, attempts: 0, next_at: None, last_attempt: None,
            attempt_started: None, logged_in_at: None, username: None, kicked_retry: false, checked: None, lost: false, last_line: None,
            rate_limited_until: None, last_search_at: None, held: None, last_press: None,
        }
    }
}

impl Conn {
    /// Change the state (and `since` when anything changed).
    fn set(&mut self, state: State, reason: Option<Reason>, message: Option<String>, unix: f64) {
        if self.state != state || self.reason != reason || self.message != message {
            self.since = unix;
        }
        self.state = state;
        self.reason = reason;
        self.message = message;
    }

    /// The earliest an attempt may start: now, but never within `MIN_GAP` of the last one.
    pub fn earliest(&self, now: Instant) -> Instant {
        self.last_attempt.map(|t| t + MIN_GAP).filter(|t| *t > now).unwrap_or(now)
    }

    /// Soulseek is off or its engine isn't running (`starting`: it is about to): no attempts. An attempt
    /// or a session under way ends with it (without a reason of the server's: no backoff); what was
    /// planned waits for the engine. Logs when Soulseek stops connecting, or a session ends.
    pub fn engine_down(&mut self, starting: bool, message: &str, now: Instant, unix: f64) -> Option<String> {
        if self.state == State::Stopped && matches!(self.reason, Some(Reason::Rejected | Reason::Banned | Reason::Kicked)) {
            return None; // stopped for a reason of the server's: that stays until the settings change
        }
        let was = self.state;
        if was == State::Waiting && self.next_at.is_some() {
            self.held = self.reason.map(|r| (r, self.message.clone()));
        }
        self.end_session(now);
        self.attempt_started = None;
        self.checked = None;
        self.lost = false;
        let state = if starting { State::Disconnected } else { State::Stopped };
        self.set(state, None, Some(message.to_string()), unix);
        if matches!(was, State::LoggedIn | State::Connecting) {
            return Some(format!("soulseek: disconnected – {message}"));
        }
        (was != state && !starting).then(|| format!("soulseek: not connecting – {message}"))
    }

    /// The engine runs: plan the first attempt if nothing is planned. An attempt planned before the engine
    /// went down keeps its time and its reason.
    pub fn engine_up(&mut self, now: Instant, unix: f64) {
        if self.reason.is_none() && matches!(self.state, State::Stopped | State::Disconnected) && self.attempt_started.is_none() {
            match self.held.take().filter(|_| self.next_at.is_some_and(|t| t > now)) {
                Some((r, m)) => self.set(State::Waiting, Some(r), m, unix),
                None if self.state == State::Stopped || self.message.is_some() => self.set(State::Disconnected, None, None, unix),
                None => {}
            }
            if self.next_at.is_none() {
                self.next_at = Some(self.earliest(now));
            }
        }
    }

    /// A new slskd process runs (it was restarted: changed settings, a new VPN port, a crash): a session
    /// or an attempt of the old one is over, without a reason of the server's – the next attempt goes
    /// as soon as the 30 s gap allows, and the schedule doesn't move on. A planned attempt stays planned.
    pub fn engine_restarted(&mut self, now: Instant, unix: f64) -> Option<String> {
        let line = self.engine_down(true, "the Soulseek engine restarted", now, unix);
        self.engine_up(now, unix);
        line.map(|l| match self.next_at {
            Some(t) => format!("{l}; next attempt in {}", span(t.saturating_duration_since(now))),
            None => l,
        })
    }

    /// An attempt is due: the engine runs, nothing is under way and its time has come.
    pub fn due(&self, now: Instant) -> bool {
        matches!(self.state, State::Disconnected | State::Waiting) && self.attempt_started.is_none()
            && self.next_at.is_some_and(|t| t <= now) && self.earliest(now) <= now
    }

    /// An attempt starts now → its log line.
    pub fn attempt_begins(&mut self, now: Instant, unix: f64) -> String {
        self.held = None;
        self.attempts += 1;
        self.last_attempt = Some(now);
        self.attempt_started = Some(now);
        self.next_at = None;
        self.last_line = None;
        self.set(State::Connecting, None, None, unix);
        format!("soulseek: connecting (attempt {})", self.attempts)
    }

    /// The session logged in as `user` → the log line, when it is news.
    pub fn logged_in(&mut self, user: &str, now: Instant, unix: f64) -> Option<String> {
        self.held = None;
        self.attempt_started = None;
        self.next_at = None;
        self.lost = false;
        self.last_line = None;
        self.checked = Some(now);
        if !user.is_empty() {
            self.username = Some(user.to_string());
        }
        if self.state == State::LoggedIn {
            return None;
        }
        self.logged_in_at = Some(now);
        self.set(State::LoggedIn, None, None, unix);
        Some(format!("soulseek: logged in as {}", self.username.as_deref().unwrap_or("?")))
    }

    /// A session that lasted `STABLE` starts the schedule over.
    fn end_session(&mut self, now: Instant) {
        if self.logged_in_at.is_some_and(|t| now.duration_since(t) >= STABLE) {
            self.attempts = 0;
            self.kicked_retry = false;
        }
        self.logged_in_at = None;
    }

    /// The attempt under way failed, or the session ended, for `reason`. `r` (-1 … 1) is the jitter.
    pub fn dropped(&mut self, reason: Reason, message: &str, now: Instant, unix: f64, r: f64) -> String {
        self.held = None;
        self.end_session(now);
        self.attempt_started = None;
        self.lost = false;
        self.last_line = None;
        let message = Some(message.to_string()).filter(|m| !m.trim().is_empty());
        let what = describe(reason, message.as_deref());
        match reason {
            Reason::Rejected | Reason::Banned => {
                self.next_at = None;
                self.set(State::Stopped, Some(reason), message, unix);
                format!("soulseek: disconnected – {what}; not connecting again until the settings change or Reconnect now")
            }
            Reason::Kicked if self.kicked_retry => {
                self.next_at = None;
                self.set(State::Stopped, Some(reason), message, unix);
                format!("soulseek: disconnected – {what} again; not connecting again until Reconnect now")
            }
            _ => {
                let wait = if reason == Reason::Kicked {
                    self.kicked_retry = true;
                    KICK_PAUSE
                } else {
                    jittered(backoff(self.attempts.max(1)), r)
                };
                let at = (now + wait).max(self.earliest(now));
                self.next_at = Some(at);
                self.set(State::Waiting, Some(reason), message, unix);
                format!("soulseek: disconnected – {what}; next attempt in {}", span(at.duration_since(now)))
            }
        }
    }

    /// Reconnect now: refused while logged in or while an attempt is under way, and within `MIN_GAP` of
    /// the last attempt; else the schedule starts over and an attempt is due at once.
    pub fn reconnect_now(&mut self, now: Instant, unix: f64) -> Result<(), Refused> {
        match self.state {
            State::LoggedIn if !self.lost => return Err(Refused::LoggedIn),
            State::Connecting => return Err(Refused::Connecting),
            _ => {}
        }
        if let Some(t) = self.last_attempt.map(|t| t + MIN_GAP).filter(|t| *t > now) {
            return Err(Refused::Wait(t.duration_since(now)));
        }
        self.start_over(now, unix);
        self.next_at = Some(now);
        Ok(())
    }

    /// Download now on a request waiting for Soulseek: the planned attempt is due now – only where Encore's
    /// own schedule holds it (a lost connection, nothing planned), never within `MIN_GAP` of the last attempt
    /// and at most once per `PRESS_GAP`, and without starting the schedule over (unlike Reconnect now: a new
    /// failure backs off further). Where the Soulseek server asked Encore to wait (a login it didn't answer –
    /// it shuts out addresses that knock too often –, another client with the username) nothing changes.
    pub fn attempt_now(&mut self, now: Instant) -> AttemptNow {
        match self.state {
            State::LoggedIn if !self.lost => return AttemptNow::LoggedIn,
            State::LoggedIn | State::Connecting => return AttemptNow::Connecting,
            State::Stopped => return AttemptNow::Stopped,
            State::Waiting if matches!(self.reason, Some(Reason::Timeout | Reason::Kicked)) => {
                return AttemptNow::Later(self.next_at.unwrap_or(self.earliest(now)).max(self.earliest(now)));
            }
            _ => {}
        }
        let earliest = self.earliest(now);
        let at = self.last_press.map(|p| p + PRESS_GAP).filter(|t| *t > earliest).unwrap_or(earliest);
        if at > now {
            // the planned attempt when it comes first, else the moment a press could make one due
            return AttemptNow::Later(self.next_at.map_or(at, |n| n.max(earliest).min(at)));
        }
        self.next_at = Some(now);
        self.last_press = Some(now);
        AttemptNow::Due
    }

    /// The login settings changed: whatever the server said about the old ones no longer counts, and an
    /// attempt under way (with the old ones) is given up.
    pub fn settings_changed(&mut self, now: Instant, unix: f64) {
        self.start_over(now, unix);
        if self.state == State::Disconnected {
            self.next_at = Some(self.earliest(now));
        }
    }

    /// A new run of attempts: the count and the kicked retry start again.
    fn start_over(&mut self, _now: Instant, unix: f64) {
        self.held = None;
        self.attempts = 0;
        self.kicked_retry = false;
        if self.state == State::Connecting {
            self.attempt_started = None;
            self.set(State::Disconnected, None, None, unix);
        }
        if matches!(self.state, State::Stopped | State::Waiting) && self.reason.is_some() {
            self.set(State::Disconnected, None, None, unix);
        }
    }

    /// How often the server state is read: every second during an attempt or after a search found the
    /// session gone, every minute while logged in, every 10 s otherwise.
    fn poll_every(&self) -> Duration {
        match self.state {
            _ if self.attempt_started.is_some() || self.lost => Duration::from_secs(1),
            State::LoggedIn => Duration::from_secs(60),
            _ => Duration::from_secs(10),
        }
    }

    /// How long the supervisor may sleep: until the next look at the server, the next attempt (only while
    /// one is planned) or the attempt's deadline. Something that changes the picture before then (a log
    /// line, a search that found the session gone, the engine starting or stopping) wakes it anyway.
    pub fn next_wakeup(&self, now: Instant) -> Duration {
        let every = self.poll_every();
        // not read yet (the engine isn't running, or a wake-up asked for a look): the round that wakes it
        // reads it; until then, the usual pace
        let mut at = self.checked.map(|t| t + every).unwrap_or(now + every);
        if let Some(t) = self.next_at.filter(|_| matches!(self.state, State::Disconnected | State::Waiting)) {
            at = at.min(t.max(self.earliest(now)));
        }
        if let Some(t) = self.attempt_started {
            at = at.min(t + super::DOOR_WINDOW).min(t + ATTEMPT_MAX);
        }
        at.saturating_duration_since(now).max(Duration::from_millis(100))
    }

    /// The server state is due to be read again.
    pub fn poll_due(&self, now: Instant) -> bool {
        let every = if self.attempt_started.is_some() || self.lost { Duration::ZERO } else { self.poll_every() };
        self.checked.is_none_or(|t| now.duration_since(t) >= every)
    }

    /// What survives a restart of Encore: the count, the last and the next attempt (unix times), and a
    /// stop of the server's with the login settings it was about.
    pub fn saved(&self, now: Instant, unix: f64, login: u64) -> serde_json::Value {
        let at = |t: Instant| if t >= now { unix + (t - now).as_secs_f64() } else { unix - (now - t).as_secs_f64() };
        let stop = (self.state == State::Stopped).then_some(self.reason).flatten();
        serde_json::json!({
            "attempts": self.attempts,
            "last_attempt": self.last_attempt.map(at),
            "next_at": self.next_at.filter(|_| self.state == State::Waiting).map(at),
            "reason": self.reason.filter(|_| matches!(self.state, State::Waiting | State::Stopped)).map(|r| r.as_str()),
            "message": self.message.as_ref().filter(|_| matches!(self.state, State::Waiting | State::Stopped) && self.reason.is_some()),
            "stopped": stop.is_some(),
            "kicked_retry": self.kicked_retry,
            "login": login.to_string(),
        })
    }

    /// Take up what a previous run of Encore saved (when the login settings are still the same): the next
    /// attempt keeps its time and its 30 s from the last one, and a stop of the server's stays.
    pub fn restore(&mut self, v: &serde_json::Value, now: Instant, unix: f64, login: u64) {
        if v["login"].as_str() != Some(login.to_string().as_str()) {
            return; // other login settings: the schedule starts over
        }
        self.attempts = v["attempts"].as_u64().unwrap_or(0) as u32;
        self.kicked_retry = v["kicked_retry"].as_bool().unwrap_or(false);
        if let Some(t) = v["last_attempt"].as_f64().filter(|t| unix - t < MIN_GAP.as_secs_f64()) {
            self.last_attempt = now.checked_sub(Duration::from_secs_f64((unix - t).max(0.0)));
        }
        let reason = v["reason"].as_str().and_then(Reason::parse);
        let message = v["message"].as_str().map(String::from);
        if v["stopped"].as_bool() == Some(true) && reason.is_some() {
            self.next_at = None;
            self.set(State::Stopped, reason, message, unix);
        } else if let Some(t) = v["next_at"].as_f64().filter(|t| *t > unix) {
            self.next_at = Some(now + Duration::from_secs_f64(t - unix));
            match reason {
                Some(r) => self.set(State::Waiting, Some(r), message, unix),
                None => self.set(State::Disconnected, None, None, unix),
            }
        }
    }
}

/// Why Reconnect now was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// already logged in
    LoggedIn,
    /// an attempt is under way
    Connecting,
    /// the last attempt was less than 30 s ago: wait this long
    Wait(Duration),
}

/// The reason in a few words: "the login timed out (the Soulseek server did not answer in 5 s)".
pub fn describe(reason: Reason, message: Option<&str>) -> String {
    let m = message.unwrap_or("");
    match reason {
        Reason::Timeout => match waited(m) {
            Some(d) => format!("the login timed out (the Soulseek server did not answer in {})", span(d)),
            None if m.is_empty() => "the connection timed out (the Soulseek server did not answer)".into(),
            None => format!("the connection timed out ({m})"),
        },
        Reason::Rejected => {
            let l = m.to_lowercase();
            if l.contains("invalidversion") {
                "the Soulseek server refused this engine version – update the Soulseek plugin under Settings → Plugins".into()
            } else if l.contains("invalidusername") {
                "the Soulseek server does not allow this username – choose another".into()
            } else {
                "invalid username or password".into()
            }
        }
        Reason::Kicked => "another Soulseek client logged in with this username".into(),
        Reason::Banned => if m.is_empty() { "banned by the Soulseek server".into() } else { format!("banned by the Soulseek server ({m})") },
        Reason::RateLimited => "rate limited by the Soulseek server".into(),
        Reason::Network => if m.is_empty() { "the connection to the Soulseek server was lost".into() } else { format!("connection problem ({m})") },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(d: Duration) -> f64 {
        d.as_secs_f64()
    }

    #[test]
    fn the_schedule_is_30s_1m_2m_5m_10m_then_15m() {
        let want = [30, 60, 120, 300, 600, 900, 900, 900];
        for (i, w) in want.iter().enumerate() {
            assert_eq!(backoff(i as u32 + 1), Duration::from_secs(*w), "after failure {}", i + 1);
        }
        assert_eq!(backoff(0), Duration::from_secs(30));
        assert_eq!(backoff(1000), Duration::from_secs(900), "capped at 15 minutes");
        // jitter: ±20 %, never more
        assert_eq!(jittered(Duration::from_secs(100), -1.0), Duration::from_secs(80));
        assert_eq!(jittered(Duration::from_secs(100), 1.0), Duration::from_secs(120));
        assert_eq!(jittered(Duration::from_secs(100), 7.0), Duration::from_secs(120), "out-of-range jitter is clamped");
        assert_eq!(span(Duration::from_secs(130)), "2 m 10 s");
        assert_eq!(span(Duration::from_secs(45)), "45 s");
        assert_eq!(span(Duration::from_secs(900)), "15 m");
    }

    /// Download now makes the planned attempt due only where Encore's own schedule holds the session (a lost
    /// connection): never within 30 s of the last attempt, at most once per 2 minutes, without resetting the
    /// count. Where the server asked Encore to wait (a login it didn't answer, a kick) nothing changes.
    #[test]
    fn attempt_now_only_where_encores_own_schedule_holds_it() {
        let s = |n: u64| Duration::from_secs(n);
        let t0 = Instant::now();
        let mut c = Conn::default();
        c.engine_up(t0, 0.0);
        c.attempt_begins(t0, 0.0);
        assert_eq!(c.attempt_now(t0), AttemptNow::Connecting);
        c.dropped(Reason::Network, "Remote connection closed", t0 + s(5), 0.0, 0.0);
        // within 30 s of the last attempt: later, at the earliest
        assert_eq!(c.attempt_now(t0 + s(10)), AttemptNow::Later(t0 + MIN_GAP));
        // a lost connection: due now, the count unchanged
        let n = c.attempts;
        let now = t0 + s(31);
        assert!(!c.due(now), "not due yet by the schedule");
        assert_eq!(c.attempt_now(now), AttemptNow::Due);
        assert!(c.due(now));
        assert_eq!(c.attempts, n, "the schedule's count stays");
        c.attempt_begins(now, 0.0);
        c.dropped(Reason::Network, "Remote connection closed", now + s(5), 0.0, 0.0);
        assert_eq!(c.next_at.unwrap().duration_since(now + s(5)), backoff(n + 1), "a new failure backs off further");
        // a 2nd press within 2 minutes: later, with the time
        let again = now + s(60);
        match c.attempt_now(again) {
            AttemptNow::Later(t) => assert!(t > again && t <= now + PRESS_GAP, "{:?}", t.duration_since(again)),
            other => panic!("{other:?}"),
        }
        assert!(!c.due(again), "nothing made due");
        // the login timed out (the server shuts out addresses that knock too often): later at the planned time
        let t = now + s(300);
        c.attempt_begins(t, 0.0);
        c.dropped(Reason::Timeout, "The wait timed out after 5000 milliseconds", t + s(5), 0.0, 0.0);
        let planned = c.next_at;
        assert_eq!(c.attempt_now(t + s(60)), AttemptNow::Later(planned.unwrap()));
        assert_eq!(c.next_at, planned, "nothing planned changes");
        // another client logged in with the username: later at the planned time (15 minutes)
        let t = planned.unwrap();
        c.attempt_begins(t, 0.0);
        c.dropped(Reason::Kicked, "Kicked from server: another client logged in", t + s(1), 0.0, 0.0);
        let planned = c.next_at;
        assert_eq!(c.attempt_now(t + s(200)), AttemptNow::Later(planned.unwrap()));
        assert_eq!(planned.unwrap().duration_since(t + s(1)), KICK_PAUSE);
        // logged in: nothing to do; stopped: the request goes on to its next source
        let t = planned.unwrap();
        c.attempt_begins(t, 0.0);
        c.logged_in("me", t, 0.0);
        assert_eq!(c.attempt_now(t), AttemptNow::LoggedIn);
        c.dropped(Reason::Banned, "banned", t + s(1), 0.0, 0.0);
        assert_eq!(c.attempt_now(t + s(600)), AttemptNow::Stopped);
    }

    /// A run of failures follows the schedule (within the jitter) and never puts two attempts closer than
    /// 30 s; a session that stayed logged in for 10 minutes starts it over, a shorter one does not.
    #[test]
    fn failures_follow_the_schedule_and_a_stable_session_resets_it() {
        let t0 = Instant::now();
        let mut c = Conn::default();
        c.engine_up(t0, 0.0);
        assert!(c.due(t0), "the first attempt goes at once");
        let mut now = t0;
        let mut starts = vec![];
        let want = [30.0, 60.0, 120.0, 300.0, 600.0, 900.0, 900.0];
        for (i, w) in want.iter().enumerate() {
            starts.push(now);
            c.attempt_begins(now, 0.0);
            assert_eq!(c.attempts, i as u32 + 1);
            assert!(!c.due(now), "only one attempt at a time");
            now += Duration::from_secs(5); // the attempt takes 5 s
            let r = if i % 2 == 0 { -1.0 } else { 1.0 };
            let line = c.dropped(Reason::Timeout, "The wait timed out after 5000 milliseconds", now, 0.0, r);
            assert!(line.starts_with("soulseek: disconnected – the login timed out (the Soulseek server did not answer in 5 s); next attempt in"), "{line}");
            let wait = secs(c.next_at.unwrap().duration_since(now));
            let (lo, hi) = (w * 0.8, w * 1.2);
            assert!(wait >= lo - 0.001 && wait <= hi + 0.001, "wait {wait} after failure {} not within {lo}–{hi}", i + 1);
            assert_eq!(c.state, State::Waiting);
            assert!(!c.due(now));
            now = c.next_at.unwrap();
            assert!(c.due(now));
        }
        for w in starts.windows(2) {
            assert!(w[1].duration_since(w[0]) >= MIN_GAP, "two attempts closer than 30 s");
        }

        // logged in for 9 minutes: the next drop continues the schedule
        c.attempt_begins(now, 0.0);
        let n = c.attempts;
        assert!(c.logged_in("me", now, 0.0).unwrap().contains("logged in as me"));
        now += Duration::from_secs(540);
        c.dropped(Reason::Network, "Remote connection closed", now, 0.0, 0.0);
        assert_eq!(c.attempts, n, "a short session doesn't reset the count");
        assert_eq!(c.next_at.unwrap().duration_since(now), Duration::from_secs(900));
        // logged in for 10 minutes: the schedule starts over (30 s ± 20 %)
        now = c.next_at.unwrap();
        c.attempt_begins(now, 0.0);
        c.logged_in("me", now, 0.0);
        now += STABLE;
        c.dropped(Reason::Network, "Remote connection closed", now, 0.0, -1.0);
        assert_eq!(c.attempts, 0);
        assert_eq!(c.next_at.unwrap().duration_since(now), Duration::from_secs(24), "30 s − 20 %, and the last attempt was long ago");
    }

    /// The first retry's jitter never brings two attempts within 30 s of each other.
    #[test]
    fn never_two_attempts_within_30_s() {
        let t0 = Instant::now();
        let mut c = Conn::default();
        c.engine_up(t0, 0.0);
        c.attempt_begins(t0, 0.0);
        c.dropped(Reason::Network, "refused", t0 + Duration::from_secs(1), 0.0, -1.0); // 30 s − 20 % = 24 s after a 1 s attempt
        assert_eq!(c.next_at.unwrap().duration_since(t0), MIN_GAP, "held back to 30 s after the last start");
        assert!(!c.due(t0 + Duration::from_secs(29)));
        assert!(c.due(t0 + MIN_GAP));
    }

    /// Reconnect now: refused within 30 s of the last attempt ("Wait N s"), else the schedule starts over;
    /// a rejected login stops the loop until then, and so does a second kick.
    #[test]
    fn reconnect_now_and_the_reasons_that_stop_the_loop() {
        let t0 = Instant::now();
        let mut c = Conn::default();
        c.engine_up(t0, 0.0);
        c.attempt_begins(t0, 0.0);
        assert_eq!(c.reconnect_now(t0 + Duration::from_secs(10), 0.0), Err(Refused::Connecting), "refused while an attempt is under way");
        let line = c.dropped(Reason::Rejected, "The server rejected login attempt: INVALIDPASS", t0 + Duration::from_secs(2), 0.0, 0.0);
        assert!(line.contains("invalid username or password") && line.contains("not connecting again"), "{line}");
        assert_eq!((c.state, c.reason), (State::Stopped, Some(Reason::Rejected)));
        assert!(!c.due(t0 + Duration::from_secs(3600)), "a rejected login is not tried again by itself");
        assert_eq!(c.reconnect_now(t0 + Duration::from_secs(10), 0.0), Err(Refused::Wait(Duration::from_secs(20))), "refused inside 30 s");
        assert_eq!(c.reconnect_now(t0 + Duration::from_secs(29), 0.0), Err(Refused::Wait(Duration::from_secs(1))));
        assert_eq!(c.reconnect_now(t0 + Duration::from_secs(30), 0.0), Ok(()));
        assert_eq!((c.state, c.reason, c.attempts), (State::Disconnected, None, 0));
        assert!(c.due(t0 + Duration::from_secs(30)));

        // kicked: wait 15 minutes, try once more; kicked again: stop
        let t1 = t0 + Duration::from_secs(30);
        c.attempt_begins(t1, 0.0);
        c.logged_in("me", t1, 0.0);
        assert_eq!(c.reconnect_now(t1 + MIN_GAP, 0.0), Err(Refused::LoggedIn), "nothing to reconnect");
        let line = c.dropped(Reason::Kicked, "another client logged in using the same username", t1 + Duration::from_secs(60), 0.0, 0.0);
        assert!(line.ends_with("next attempt in 15 m"), "{line}");
        assert_eq!(c.state, State::Waiting);
        let t2 = c.next_at.unwrap();
        c.attempt_begins(t2, 0.0);
        c.logged_in("me", t2, 0.0);
        c.dropped(Reason::Kicked, "another client logged in using the same username", t2 + Duration::from_secs(60), 0.0, 0.0);
        assert_eq!((c.state, c.reason), (State::Stopped, Some(Reason::Kicked)));
        // banned: stop; changed settings start over
        let mut b = Conn::default();
        b.engine_up(t0, 0.0);
        b.attempt_begins(t0, 0.0);
        b.dropped(Reason::Banned, "You are banned", t0, 0.0, 0.0);
        assert_eq!(b.state, State::Stopped);
        b.settings_changed(t0 + Duration::from_secs(5), 0.0);
        assert_eq!(b.state, State::Disconnected);
        assert!(!b.due(t0 + Duration::from_secs(5)), "still 30 s after the last attempt");
        assert!(b.due(t0 + MIN_GAP));
        // the engine going down doesn't undo a stop of the server's
        assert_eq!(c.engine_down(false, "Soulseek is off", t2, 0.0), None);
        assert_eq!(c.reason, Some(Reason::Kicked));
    }

    #[test]
    fn slskd_log_lines() {
        assert_eq!(read_line("[17:02:12 INF] Logged in to the Soulseek server as krifmaz"), Some(Line::LoggedIn("krifmaz".into())));
        assert_eq!(read_line("[17:02:11 ERR] Disconnected from the Soulseek server: The wait timed out after 5000 milliseconds"),
                   Some(Line::Dropped(Reason::Timeout, "The wait timed out after 5000 milliseconds".into())));
        assert_eq!(read_line("[17:02:11 ERR] Failed to reconnect: The server rejected login attempt: INVALIDPASS"),
                   Some(Line::Dropped(Reason::Rejected, "The server rejected login attempt: INVALIDPASS".into())));
        assert_eq!(read_line("[17:02:11 ERR] Disconnected from the Soulseek server: invalid username or password").map(|l| matches!(l, Line::Dropped(Reason::Rejected, _))), Some(true));
        assert_eq!(read_line("[17:02:11 ERR] Disconnected from the Soulseek server: another client logged in using the same username").map(|l| matches!(l, Line::Dropped(Reason::Kicked, _))), Some(true));
        assert_eq!(read_line("[17:02:11 ERR] Disconnected from the Soulseek server: Remote connection closed").map(|l| matches!(l, Line::Dropped(Reason::Network, _))), Some(true));
        assert_eq!(read_line("[17:02:11 INF] Disconnected from the Soulseek server: disconnected by the user"), None, "Encore's own disconnect");
        assert_eq!(read_line("[17:02:11 INF] Disconnected from the Soulseek server: the client is shutting down"), Some(Line::EngineStopping));
        assert_eq!(read_line("[17:02:11 WRN] Application is shutting down"), Some(Line::EngineStopping));
        // what slskd 0.26.1 printed when a connection under way was cancelled (checked against the real thing)
        assert_eq!(read_line("[13:11:28 ERR] Disconnected from the Soulseek server: Operation cancelled"), Some(Line::Cancelled));
        assert_eq!(read_line("[13:11:28 ERR] Failed to reconnect: The operation was canceled."), Some(Line::Cancelled));
        assert_eq!(read_line("[13:11:28 INF] Reconnection attempt cancelled"), None);
        // slskd's own attempts at a shut door (loopback)
        assert_eq!(read_line("[13:12:52 ERR] Failed to reconnect: Failed to connect: Failed to connect to 127.0.0.1:22421: Connection reset by peer").map(|l| matches!(l, Line::Dropped(Reason::Network, _))), Some(true));
        assert_eq!(read_line("[17:02:11 INF] Attempting to connect to the Soulseek server..."), Some(Line::Attempting));
        assert_eq!(read_line("[17:02:11 INF] Attempting to connect to the Soulseek server (#2)..."), Some(Line::Attempting));
        assert_eq!(read_line("[17:02:11 INF] Waiting about a second before attempting to reconnect"), Some(Line::Waiting));
        assert_eq!(read_line("[17:02:11 INF] Waiting about 4 seconds before attempting to reconnect"), Some(Line::Waiting));
        assert_eq!(read_line("[17:02:11 INF] Scanned 11% of shared directories."), None);
        assert_eq!(classify("You have been banned from the server"), Reason::Banned);
        assert_eq!(waited("The wait timed out after 30000 milliseconds"), Some(Duration::from_secs(30)));
        assert_eq!(waited("Connection timed out"), None);
        assert_eq!(describe(Reason::Rejected, Some("The server rejected login attempt: INVALIDUSERNAME")), "the Soulseek server does not allow this username – choose another");
    }

    /// The supervisor sleeps: with Soulseek off or its engine down it waits for a wake-up (or 10 s), a
    /// planned attempt only counts while one is planned, and a session logged in is read once a minute.
    #[test]
    fn the_supervisor_never_spins() {
        let t0 = Instant::now();
        let mut c = Conn::default();
        c.engine_down(false, "Soulseek is off", t0, 0.0);
        assert_eq!(c.next_wakeup(t0), Duration::from_secs(10), "off");
        c.engine_down(true, "the Soulseek engine is starting", t0, 0.0);
        assert_eq!(c.next_wakeup(t0), Duration::from_secs(10), "starting");
        c.engine_up(t0, 0.0);
        c.attempt_begins(t0, 0.0);
        c.logged_in("me", t0, 0.0);
        c.settings_changed(t0 + Duration::from_secs(1), 0.0);
        assert_eq!(c.state, State::LoggedIn, "the session goes on until slskd restarts with the new login");
        assert_eq!(c.next_wakeup(t0 + Duration::from_secs(1)), Duration::from_secs(59), "logged in: read once a minute");
        c.next_at = Some(t0); // left over from an earlier plan
        assert_eq!(c.next_wakeup(t0 + Duration::from_secs(1)), Duration::from_secs(59), "a plan doesn't count while logged in");
    }

    /// slskd restarted (or waited for the VPN) while an attempt was planned after a failure: the attempt
    /// keeps its time, and once the engine is back the card says why and when again – not "connecting
    /// shortly" while the next attempt is minutes away.
    #[test]
    fn a_planned_attempt_keeps_its_reason_while_the_engine_is_down() {
        let t0 = Instant::now();
        let mut c = Conn::default();
        c.engine_up(t0, 0.0);
        for i in 0..3 {
            let t = t0 + Duration::from_secs(1000 * i);
            c.attempt_begins(t, 0.0);
            c.dropped(Reason::Timeout, "The wait timed out after 5000 milliseconds", t, 0.0, 0.0);
        }
        let next = c.next_at.unwrap();
        let now = next - Duration::from_secs(100);
        assert_eq!(c.engine_restarted(now, 0.0), None, "nothing was under way");
        assert_eq!((c.state, c.reason, c.next_at), (State::Waiting, Some(Reason::Timeout), Some(next)));
        assert_eq!(c.message.as_deref(), Some("The wait timed out after 5000 milliseconds"));
        // waiting for the VPN, then back: the same
        c.engine_down(false, "Waiting for the VPN", now, 0.0);
        assert_eq!((c.state, c.reason), (State::Stopped, None));
        c.engine_down(false, "Waiting for the VPN", now + Duration::from_secs(10), 0.0);
        c.engine_up(now + Duration::from_secs(20), 0.0);
        assert_eq!((c.state, c.reason, c.next_at, c.attempts), (State::Waiting, Some(Reason::Timeout), Some(next), 3));
        // back only after the planned time: no reason left to give, the attempt goes at once
        c.engine_down(true, "the Soulseek engine is starting", now, 0.0);
        c.engine_up(next + Duration::from_secs(1), 0.0);
        assert_eq!((c.state, c.reason), (State::Disconnected, None));
        assert!(c.due(next + Duration::from_secs(1)));
    }

    /// A new slskd process ends the session without a reason of the server's: no backoff, the next
    /// attempt as soon as the 30 s gap allows; whether the engine was seen starting or not makes no
    /// difference. Changed login settings give up an attempt under way.
    #[test]
    fn an_engine_restart_is_not_a_failure() {
        let t0 = Instant::now();
        for seen_starting in [false, true] {
            let mut c = Conn::default();
            c.engine_up(t0, 0.0);
            for i in 0..4 {
                let t = t0 + Duration::from_secs(1000 * i);
                c.attempt_begins(t, 0.0);
                c.dropped(Reason::Timeout, "The wait timed out after 5000 milliseconds", t, 0.0, 0.0);
            }
            let t = c.next_at.unwrap();
            c.attempt_begins(t, 0.0);
            c.logged_in("me", t, 0.0);
            let now = t + Duration::from_secs(120);
            let line = if seen_starting {
                let l = c.engine_down(true, "the Soulseek engine is starting", now, 0.0);
                c.engine_up(now, 0.0);
                l
            } else {
                c.engine_restarted(now, 0.0)
            };
            assert!(line.unwrap().starts_with("soulseek: disconnected – the Soulseek engine"));
            assert_eq!((c.state, c.reason, c.attempts), (State::Disconnected, None, 5), "no reason, the count stays");
            assert!(c.due(now), "the next attempt at once (the last one was 2 minutes ago)");
        }
        let mut c = Conn::default();
        c.engine_up(t0, 0.0);
        c.attempt_begins(t0, 0.0);
        c.settings_changed(t0 + Duration::from_secs(1), 0.0);
        assert_eq!((c.state, c.attempt_started), (State::Disconnected, None), "the attempt with the old login is given up");
        assert!(c.due(t0 + MIN_GAP));
    }

    /// What a restart of Encore keeps: the 30 s gap from the last attempt, a planned attempt with its
    /// reason, a stop of the server's – for the same login settings only.
    #[test]
    fn the_schedule_survives_a_restart_of_encore() {
        let t0 = Instant::now() + Duration::from_secs(3600);
        let unix = 1_760_000_000.0;
        let mut c = Conn::default();
        c.engine_up(t0, unix);
        c.attempt_begins(t0, unix);
        c.dropped(Reason::Timeout, "The wait timed out after 5000 milliseconds", t0 + Duration::from_secs(5), unix + 5.0, 0.0);
        let saved = c.saved(t0 + Duration::from_secs(10), unix + 10.0, 7);
        // Encore starts again 10 s later
        let t1 = t0 + Duration::from_secs(1);
        let mut d = Conn::default();
        d.restore(&saved, t1, unix + 20.0, 7);
        d.engine_up(t1, unix + 20.0);
        assert_eq!((d.state, d.reason, d.attempts), (State::Waiting, Some(Reason::Timeout), 1));
        assert!(!d.due(t1 + Duration::from_secs(9)));
        let next = d.next_at.unwrap();
        assert_eq!(next.duration_since(t1).as_secs(), 15, "35 s after the failure, as planned");
        // a rejected login stays stopped; other login settings start over
        c.reconnect_now(t0 + MIN_GAP, unix + 30.0).unwrap();
        c.attempt_begins(t0 + MIN_GAP, unix + 30.0);
        c.dropped(Reason::Rejected, "The server rejected login attempt: INVALIDPASS", t0 + MIN_GAP, unix + 30.0, 0.0);
        let saved = c.saved(t0 + MIN_GAP, unix + 30.0, 7);
        let mut d = Conn::default();
        d.restore(&saved, t1, unix + 40.0, 7);
        d.engine_up(t1, unix + 40.0);
        assert_eq!((d.state, d.reason), (State::Stopped, Some(Reason::Rejected)));
        assert!(!d.due(t1 + Duration::from_secs(3600)));
        assert!(d.last_attempt.is_some(), "10 s after the last attempt: the gap still counts");
        let mut e = Conn::default();
        e.restore(&saved, t1, unix + 40.0, 8);
        e.engine_up(t1, unix + 40.0);
        assert_eq!((e.state, e.attempts), (State::Disconnected, 0));
        assert!(e.due(t1));
    }
}
