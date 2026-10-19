//! The loop that keeps the session: it reads slskd's state and log, connects on Encore's schedule through
//! the door, and quiets slskd's own reconnect loop (see the module's notes).
use super::{classify, Line, Reason, Session, State, ATTEMPT_MAX, DOOR_WINDOW};
use crate::Ctx;
use serde_json::Value;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::time::Duration;
use tokio::time::Instant;

/// How often "N searches waiting for the connection" may be logged.
const REPORT_EVERY: Duration = Duration::from_secs(60);
/// At most one DELETE /server this often (it quiets slskd's own loop).
const QUIET_GAP: Duration = Duration::from_secs(2);
/// After changed login settings or slskd saying it shuts down, attempts wait this long at most for the
/// new slskd process.
const HOLD_MAX: Duration = Duration::from_secs(60);
/// How long after the connection through the door closed the attempt's reason may still come in the log.
const REASON_GRACE: Duration = Duration::from_secs(1);

/// What the loop keeps between its rounds.
#[derive(Default)]
struct Round {
    /// the login settings last seen (a change starts the schedule over)
    login: Option<u64>,
    /// the last "searches waiting" line
    reported: Option<Instant>,
    /// the last DELETE /server
    quieted: Option<Instant>,
    /// look at slskd's own loop again then (to quiet it once it waits between two attempts)
    quiet_at: Option<Instant>,
    /// the slskd process the last round saw
    gen: Option<u64>,
    /// attempts wait for a new slskd process (the one running has the old login, or is shutting down):
    /// (the process that has to go, until when at most)
    hold: Option<(u64, Instant)>,
    /// what the state file holds
    saved: Option<String>,
}

/// The login settings: username, password and server.
fn login_key(st: &impl Ctx) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (st.cfg().str("soulseek_username").trim(), st.cfg().str("soulseek_password"), std::env::var("SOULSEEK_SERVER").unwrap_or_default()).hash(&mut h);
    h.finish()
}

/// The schedule kept across restarts of Encore, beside slskd.yml.
fn state_file(st: &impl Ctx) -> PathBuf {
    st.data_dir().join("slskd").join("encore-session.json")
}

/// Runs as long as the server: keeps the Soulseek session on Encore's schedule.
pub async fn supervisor(st: impl Ctx) {
    let s = st.requests().slskd.session().clone();
    let mut round = Round::default();
    s.restore(&st, &mut round);
    loop {
        s.round(&st, &mut round).await;
        let now = Instant::now();
        let managed = &st.requests().slskd;
        let mut wait = sleep_for(round.hold.is_some(), managed.state(), managed.ready(), &s.conn(), now);
        for t in [round.quiet_at, round.hold.map(|h| h.1)].into_iter().flatten() {
            wait = wait.min(t.saturating_duration_since(now).max(Duration::from_millis(100)));
        }
        tokio::select! {
            _ = s.0.wake.notified() => {}
            _ = tokio::time::sleep(wait) => {}
        }
    }
}

/// How long the loop sleeps (something that changes the picture wakes it sooner): 10 s while attempts wait
/// for a new slskd process (a planned attempt doesn't count; the process starting wakes the loop), 1 s
/// while slskd finishes its start (its last startup line wakes the loop), else until the session's next
/// look or attempt – 10 s while Soulseek is off or its engine isn't running.
fn sleep_for(held: bool, engine: &str, ready: bool, c: &super::Conn, now: Instant) -> Duration {
    if held {
        Duration::from_secs(10)
    } else if engine == "running" && !ready {
        Duration::from_secs(1)
    } else {
        c.next_wakeup(now)
    }
}

/// "1 search", "37 searches".
pub(super) fn searches(n: usize) -> String {
    if n == 1 { "1 search".into() } else { format!("{n} searches") }
}

/// A random jitter between -1 and 1.
fn jitter() -> f64 {
    rand::random::<f64>() * 2.0 - 1.0
}

fn logged_in(server: &Value) -> bool {
    server["isLoggedIn"].as_bool().unwrap_or_else(|| server["state"].as_str().unwrap_or("").contains("LoggedIn"))
}

/// slskd's client is between two states (connecting, connected and about to log in, logging in,
/// disconnecting): a DELETE now would cancel a connection under way.
fn busy(server: &Value) -> bool {
    let b = |k: &str| server[k].as_bool().unwrap_or(false);
    let s = server["state"].as_str().unwrap_or("");
    b("isTransitioning") || b("isConnecting") || b("isLoggingIn") || b("isConnected")
        || s.contains("Connecting") || s.contains("LoggingIn") || s.contains("Disconnecting") || (s.contains("Connected") && !s.contains("Disconnected"))
}

impl Session {
    /// Take up the schedule a previous run of Encore left (same login settings only).
    fn restore(&self, st: &impl Ctx, round: &mut Round) {
        if !crate::slskd_managed::configured(st) {
            return;
        }
        let Ok(text) = std::fs::read_to_string(state_file(st)) else { return };
        let Ok(v) = serde_json::from_str::<Value>(&text) else { return };
        self.conn().restore(&v, Instant::now(), encore_core::time::now(), login_key(st));
        round.login = Some(login_key(st));
    }

    /// Keep the schedule for the next run of Encore (written when it changed).
    fn save(&self, st: &impl Ctx, round: &mut Round, key: u64) {
        let (now, unix) = (Instant::now(), encore_core::time::now());
        let c = self.conn().clone();
        // what is worth a write: the times as the runtime keeps them, not as unix seconds that move
        let what = format!("{:?}{:?}{:?}{:?}{:?}{}", c.attempts, c.last_attempt, c.next_at.filter(|_| c.state == State::Waiting), c.state == State::Stopped, c.reason, c.kicked_retry);
        if round.saved.as_deref() == Some(what.as_str()) {
            return;
        }
        round.saved = Some(what);
        let path = state_file(st);
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(e) = std::fs::write(&path, c.saved(now, unix, key).to_string()) {
            tracing::debug!("soulseek: couldn't keep the reconnect schedule in {}: {e}", path.display());
        }
    }

    /// One round: the engine and the settings, slskd's log, the server state, the attempt under way, the
    /// next attempt, and the waiting searches.
    async fn round(&self, st: &impl Ctx, round: &mut Round) {
        let now = Instant::now();
        let unix = encore_core::time::now();
        let managed = &st.requests().slskd;
        let gen = managed.generation();
        if !crate::slskd_managed::configured(st) {
            self.take_lines();
            self.shut_door();
            round.gen = Some(gen);
            // switched off on purpose: nothing to log
            let _ = self.conn().engine_down(false, "Soulseek is off or has no username and password", now, unix);
            return;
        }
        let key = login_key(st);
        let engine = managed.state();
        if round.login.is_some_and(|k| k != key) {
            self.conn().settings_changed(now, unix);
            self.shut_door();
            // slskd is restarted with the new login: an attempt waits for that process, or the old one
            // would answer with the old password
            if engine == "running" {
                round.hold = Some((round.gen.unwrap_or(gen), now + HOLD_MAX));
            }
        }
        round.login = Some(key);
        if engine != "running" {
            self.take_lines();
            self.shut_door();
            round.gen = Some(gen);
            round.hold = None; // the process that runs next is a new one
            let why = crate::sources::slskd::not_ready(st).unwrap_or_else(|| "the Soulseek engine is starting".into());
            Self::note(self.conn().engine_down(engine == "starting", &why, now, unix));
            self.save(st, round, key);
            return;
        }
        if round.gen.is_some_and(|g| g != gen) {
            // a new slskd process, not seen starting (a quick restart between two rounds)
            self.shut_door();
            Self::note(self.conn().engine_restarted(now, unix));
        }
        round.gen = Some(gen);
        if round.hold.is_some_and(|(g, until)| g != gen || now >= until) {
            round.hold = None;
        }
        self.conn().engine_up(now, unix);

        for (line, at) in self.take_lines() {
            self.on_event(st, line, at, round).await;
        }
        if round.quiet_at.is_some_and(|t| t <= Instant::now()) {
            round.quiet_at = None;
            self.conn().checked = None; // look whether slskd's loop now waits between two attempts
        }
        // while held, the process that answers is on its way out: what it says doesn't count
        if round.hold.is_none() && self.conn().poll_due(Instant::now()) {
            match crate::slskd_managed::api_json(st, reqwest::Method::GET, "application", None).await {
                Ok(app) => self.on_application(st, &app, round).await,
                Err(e) => {
                    tracing::debug!("soulseek: couldn't read slskd's state: {e}");
                    self.unreachable();
                }
            }
        }
        self.deadlines(st, round).await;
        if round.hold.is_none() && managed.ready() && self.conn().due(Instant::now()) {
            self.attempt(st, round).await;
        }
        self.report(st, round);
        self.save(st, round, key);
    }

    /// The door let a connection through for the attempt that started then (and when).
    fn passed_since(&self, started: Instant) -> Option<Instant> {
        self.door_seen().passed_at.filter(|p| *p >= started)
    }

    async fn on_event(&self, st: &impl Ctx, line: Line, at: Instant, round: &mut Round) {
        match line {
            Line::LoggedIn(user) => {
                // believed as it is only for the attempt under way, and never from before a DELETE of
                // Encore's; otherwise slskd's state says whether it holds
                let ours = {
                    let c = self.conn();
                    c.state == State::Connecting && c.attempt_started.is_some_and(|t| at >= t)
                } && round.quieted.is_none_or(|q| at > q);
                if ours {
                    let l = self.conn().logged_in(&user, Instant::now(), encore_core::time::now());
                    Self::note(l);
                } else {
                    self.conn().checked = None;
                }
            }
            Line::Dropped(reason, msg) => {
                {
                    // slskd says "invalid username or password" first, then the server's own words
                    // ("The server rejected login attempt: INVALIDUSERNAME"): those are kept
                    let mut c = self.conn();
                    if c.state == State::Stopped && c.reason == Some(Reason::Rejected) && reason == Reason::Rejected && msg.to_lowercase().contains("rejected login attempt") {
                        c.message = Some(msg);
                        return;
                    }
                }
                let counts = {
                    let c = self.conn();
                    match c.state {
                        // the connection Encore let through failed (not one of slskd's own attempts, turned
                        // away at the door before or after it)
                        State::Connecting => c.attempt_started.and_then(|t| self.passed_since(t)).is_some_and(|p| at >= p),
                        State::LoggedIn => c.lost || c.logged_in_at.is_some_and(|t| at >= t),
                        _ => c.lost,
                    }
                };
                if counts {
                    self.conn().last_line = Some((reason, msg.clone()));
                    self.fail(st, reason, &msg, false, round).await;
                } else {
                    tracing::debug!("soulseek: slskd's own attempt was turned away: {msg}");
                }
            }
            Line::Waiting => self.quiet(st, round).await,
            Line::EngineStopping => {
                self.shut_door();
                let line = self.conn().engine_down(true, "the Soulseek engine is restarting", Instant::now(), encore_core::time::now());
                Self::note(line);
                round.hold = Some((round.gen.unwrap_or_default(), Instant::now() + HOLD_MAX));
            }
            // the echo of a cancel (Encore's PUT or DELETE), and slskd's own attempts: nothing to do
            Line::Cancelled | Line::Attempting => {}
        }
    }

    /// What GET /api/v0/application says: the server state and slskd's watchdog.
    async fn on_application(&self, st: &impl Ctx, app: &Value, round: &mut Round) {
        let server = &app["server"];
        let user = app["user"]["username"].as_str().map(String::from);
        if logged_in(server) {
            self.observe_server(server, user.as_deref());
            return;
        }
        let (state, started, lost, last) = {
            let mut c = self.conn();
            c.checked = Some(Instant::now());
            (c.state, c.attempt_started, c.lost, c.last_line.clone())
        };
        match state {
            State::Connecting => {
                // the connection Encore let through has closed without a login: the attempt is over (its
                // reason, when slskd's log gave one, came with the line already)
                let seen = self.door_seen();
                let through = started.and_then(|t| self.passed_since(t)).is_some();
                if through && seen.live == 0 && seen.closed_at.is_some_and(|t| t.elapsed() >= REASON_GRACE) {
                    let (reason, msg) = last.unwrap_or((Reason::Network, "the connection attempt failed".into()));
                    self.fail(st, reason, &msg, false, round).await;
                }
            }
            _ if state == State::LoggedIn || lost => {
                let (reason, msg) = last.unwrap_or((Reason::Network, "the connection to the Soulseek server was lost".into()));
                self.fail(st, reason, &msg, false, round).await;
            }
            _ => {
                let wd = &app["connectionWatchdog"];
                let on = wd["isEnabled"].as_bool().unwrap_or(false) || wd["isAttemptingConnection"].as_bool().unwrap_or(false);
                if on && !busy(server) {
                    self.quiet(st, round).await;
                } else if on {
                    round.quiet_at = Some(Instant::now() + QUIET_GAP);
                }
            }
        }
    }

    /// An attempt slskd never made (the door saw no connection), or one that neither logged in nor failed
    /// in time, is called off.
    async fn deadlines(&self, st: &impl Ctx, round: &mut Round) {
        let started = {
            let c = self.conn();
            c.attempt_started.filter(|_| c.state == State::Connecting)
        };
        let Some(started) = started else { return };
        if self.passed_since(started).is_none() && started.elapsed() >= DOOR_WINDOW {
            let msg = format!("slskd did not connect to the Soulseek server within {} s", DOOR_WINDOW.as_secs());
            self.fail(st, Reason::Network, &msg, true, round).await;
        } else if started.elapsed() >= ATTEMPT_MAX {
            let msg = format!("no answer from the Soulseek server within {} s", ATTEMPT_MAX.as_secs());
            self.fail(st, Reason::Timeout, &msg, true, round).await;
        }
    }

    /// Stop slskd's own reconnect loop (DELETE /server): only while Encore has no attempt or session
    /// going, and at most once every 2 s. Behind the shut door the loop can't reach the server; this
    /// only keeps it (and its log) quiet.
    async fn quiet(&self, st: &impl Ctx, round: &mut Round) {
        if !matches!(self.conn().state, State::Disconnected | State::Waiting | State::Stopped) {
            return;
        }
        let now = Instant::now();
        if let Some(t) = round.quieted.map(|t| t + QUIET_GAP).filter(|t| *t > now) {
            round.quiet_at = Some(t);
            return;
        }
        round.quiet_at = None;
        round.quieted = Some(now);
        tracing::debug!("soulseek: stopping slskd's own reconnect loop");
        disconnect(st).await;
    }

    /// The attempt failed or the session ended: shut the door and plan the next attempt. `end`: Encore
    /// calls the attempt off itself, so slskd's connection is ended too.
    async fn fail(&self, st: &impl Ctx, reason: Reason, msg: &str, end: bool, round: &mut Round) {
        self.shut_door();
        if end {
            round.quieted = Some(Instant::now());
            disconnect(st).await;
        }
        let (line, stopped) = {
            let mut c = self.conn();
            let line = c.dropped(reason, msg, Instant::now(), encore_core::time::now(), jitter());
            (line, c.state == State::Stopped)
        };
        if stopped {
            st.db().log("warning", &line.replacen("soulseek: ", "Soulseek ", 1));
        }
        Self::note(Some(line));
    }

    /// Connect once: open the door (on the direct route the relay reaches the server first), then
    /// PUT /server; the next rounds watch how it goes. Not begun while slskd's API doesn't answer (the
    /// relay would reach the server for nothing).
    async fn attempt(&self, st: &impl Ctx, round: &mut Round) {
        if let Err(e) = crate::slskd_managed::api_json(st, reqwest::Method::GET, "application", None).await {
            tracing::debug!("soulseek: slskd doesn't answer yet, the attempt waits: {e}");
            self.unreachable();
            return;
        }
        let line = self.conn().attempt_begins(Instant::now(), encore_core::time::now());
        Self::note(Some(line));
        if let Err(e) = st.requests().slskd.open_door().await {
            let msg = e.to_string();
            self.fail(st, classify(&msg), &msg, false, round).await;
            return;
        }
        // slskd's part starts now (the relay may have taken up to 30 s to reach the server): the door's
        // window and the attempt's deadline count from here; the 30 s gap from the attempt's start
        self.conn().attempt_started = Some(Instant::now());
        let mut put = crate::slskd_managed::api_json(st, reqwest::Method::PUT, "server", None).await;
        if put.as_ref().is_err_and(|e| e.is::<crate::slskd_managed::Unreachable>()) {
            // once more, while the door is open: slskd's API was seen to refuse a connection for a moment
            tokio::time::sleep(Duration::from_secs(1)).await;
            put = crate::slskd_managed::api_json(st, reqwest::Method::PUT, "server", None).await;
        }
        if let Err(e) = put {
            self.fail(st, Reason::Network, &format!("slskd did not take the connect request: {e}"), false, round).await;
        }
    }

    /// "soulseek: 37 searches waiting for the connection", at most once a minute.
    fn report(&self, st: &impl Ctx, round: &mut Round) {
        let c = self.conn().clone();
        if c.state == State::LoggedIn || round.reported.is_some_and(|t| t.elapsed() < REPORT_EVERY) {
            return;
        }
        let n = super::parked_waiting(st, &c) as usize + self.searches_queued();
        if n > 0 {
            round.reported = Some(Instant::now());
            tracing::info!("soulseek: {} waiting for the connection", searches(n));
        }
    }
}

/// DELETE /server: slskd's watchdog stops (and a connection under way ends, "by the user" once it is
/// connected; while it is still connecting slskd starts its loop again, which the door keeps harmless).
async fn disconnect(st: &impl Ctx) {
    let body = serde_json::json!("Encore reconnects on its own schedule");
    if let Err(e) = crate::slskd_managed::api_json(st, reqwest::Method::DELETE, "server", Some(&body)).await {
        tracing::debug!("soulseek: couldn't stop slskd's reconnect loop: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::slskd_session::Conn;

    /// Soulseek off, not installed or its engine starting: the loop looks every 10 s (a wake-up comes when
    /// the engine is up), never every second.
    #[test]
    fn the_loop_rests_while_soulseek_is_off_or_its_engine_is_down() {
        let now = Instant::now();
        let mut c = Conn::default();
        c.engine_down(false, "Soulseek is off or has no username and password", now, 0.0);
        for engine in ["off", "not_installed", "waiting_vpn", "crashed"] {
            assert_eq!(sleep_for(false, engine, false, &c, now), Duration::from_secs(10), "{engine}");
        }
        c.engine_down(true, "the Soulseek engine is starting", now, 0.0);
        assert_eq!(sleep_for(false, "starting", false, &c, now), Duration::from_secs(10));
        assert_eq!(sleep_for(false, "running", false, &c, now), Duration::from_secs(1), "finishing its start");
        assert_eq!(sleep_for(true, "running", true, &c, now), Duration::from_secs(10), "held for a new process");
    }
}
