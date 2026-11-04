//! The device identity: after sign-in the app asks the server for an app token once per
//! server (kept in the config) and starts cadenza's Encore link
//! (`cadenza::encore::start`) on the embedded engine. The link heartbeats, receives commands
//! from other devices (play these tracks, next, volume …) and drives the same engine, so this
//! computer is a device in Your devices without a second identity.
//!
//! The token is checked all along: when the server stops taking it (a password change, Sign
//! out everywhere, a revoke under Account → sessions) the app asks for a new one with its
//! session (`on_tick`), and the songs in the queue get it. A token request that fails because
//! the server is away is tried again later (30 s, 2 min, then every 10 min).
//!
//! Package F owns this file in stage 2; `start`, `update`, `stop` and `auth` keep their shapes.

use crate::api::{Api, ApiError};
use crate::config::Config;
use crate::engine;
use cadenza::config::EncoreConfig;
use cadenza::decode::Source;
use cadenza::encore::EncoreLink;
use iced::Task;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Whether a link runs right now (the player logs plays itself while there is none: the
/// link reports them when it is up).
static CONNECTED: AtomicBool = AtomicBool::new(false);

/// Why this computer is not in Your devices, for the pages to show (see [`problem`]).
static PROBLEM: Mutex<Option<String>> = Mutex::new(None);

/// What the link's own error says when the server refuses the token (cadenza's encore client).
const TOKEN_REFUSED: &str = "did not accept the token";

/// The waits before asking again for a token the server could not be reached for.
const RETRY: [Duration; 3] = [
    Duration::from_secs(30),
    Duration::from_secs(120),
    Duration::from_secs(600),
];

/// A refused token is replaced at most this often.
const REFRESH_EVERY: Duration = Duration::from_secs(60);

/// Whether the device link is running (and the server takes its token).
pub fn connected() -> bool {
    CONNECTED.load(Ordering::SeqCst)
}

/// Why this computer is not in Your devices right now (the token could not be had, the engine
/// did not start, the server refuses the token), in words to show; None while all is well.
pub fn problem() -> Option<String> {
    PROBLEM.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

fn set_problem(p: Option<String>) {
    *PROBLEM.lock().unwrap_or_else(|e| e.into_inner()) = p;
}

/// The running link and what it was started with.
#[derive(Default)]
pub struct State {
    pub server: String,
    pub device_id: String,
    pub link: Option<Arc<EncoreLink>>,
    /// Why there is no link (token refused, engine failed).
    pub error: Option<String>,
    /// A token request started by `on_tick` is on its way.
    asking: bool,
    /// When to ask again for a token the server could not be reached for, and how many times
    /// it failed so far.
    retry: Option<(Instant, usize)>,
    /// The refused token being replaced: the queue's songs carry it.
    replacing: Option<String>,
    /// When a refused token was last replaced.
    refreshed: Option<Instant>,
}

impl std::fmt::Debug for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Link({}, {})", self.device_id, if self.link.is_some() { "on" } else { "off" })
    }
}

#[derive(Clone, Debug)]
pub enum Message {
    /// The app token for `server` arrived (or not: a 401 means the session itself is gone).
    Token(String, Result<String, ApiError>),
}

/// FNV-1a, as cadenza's client uses for its device ids.
pub fn fnv1a(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
        (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// `desktop-<fnv1a(hostname|server)>`: stable per computer and server.
pub fn device_id(server: &str) -> String {
    let server = crate::config::normalize(server);
    format!("desktop-{:016x}", fnv1a(&format!("{}|{server}", engine::hostname())))
}

/// The credential a stream uses: the app token when the server has issued one, else the
/// session cookie.
pub fn auth(config: &Config, api: &Api) -> engine::tracks::Auth {
    match config.current().filter(|s| !s.token.is_empty()) {
        Some(s) => engine::tracks::bearer(&s.token),
        None => engine::tracks::cookie(&api.session_cookie().unwrap_or_default()),
    }
}

/// Get the token for the current server (from the config, or from the server) and connect.
pub fn start(api: Arc<Api>, config: &Config) -> Task<Message> {
    let server = config.server.clone();
    if server.is_empty() {
        return Task::none();
    }
    let id = device_id(&server);
    if let Some(s) = config.current().filter(|s| !s.token.is_empty() && s.device_id == id) {
        let token = s.token.clone();
        return Task::done(Message::Token(server, Ok(token)));
    }
    let name = engine::device_name();
    Task::perform(
        async move {
            let r = api.app_token(&id, &name).await;
            (server, r)
        },
        |(server, r)| Message::Token(server, r),
    )
}

/// Apply a message; `config` gets the token and is saved.
pub fn update(state: &mut State, msg: Message, config: &mut Config) -> Task<Message> {
    match msg {
        Message::Token(server, Ok(token)) => {
            state.asking = false;
            if server != config.server {
                return Task::none();
            }
            state.retry = None;
            let id = device_id(&server);
            if let Some(s) = config.server_mut(&server) {
                if s.token != token || s.device_id != id {
                    s.token = token.clone();
                    s.device_id = id;
                    config.save();
                }
            }
            connect(state, &server, &token);
            if let Some(old) = state.replacing.take().filter(|o| *o != token) {
                reauth_queue(&old, &token);
            }
            Task::none()
        }
        Message::Token(server, Err(e)) => {
            state.asking = false;
            log::warn!("no app token from {server}: {}", e.message);
            let words = format!("Encore did not give this computer its token: {}", e.message);
            state.error = Some(words.clone());
            set_problem(Some(words));
            // The server is away (restarting, the network down): try again later.
            if server == config.server && (e.status == 0 || e.status >= 500) {
                let n = state.retry.map_or(0, |(_, n)| n + 1);
                let wait = RETRY[n.min(RETRY.len() - 1)];
                log::info!("asking for the app token again in {} s", wait.as_secs());
                state.retry = Some((Instant::now() + wait, n));
            }
            Task::none()
        }
    }
}

/// The app's tick while signed in: a token the server refuses is replaced (the session asks for
/// a new one), and a token request that failed because the server was away is tried again.
pub fn on_tick(state: &mut State, api: Arc<Api>, config: &mut Config) -> Task<Message> {
    if state.asking || config.server.is_empty() {
        return Task::none();
    }
    let refused = refused(state);
    if refused {
        // the server takes nothing from this link: the player logs plays itself meanwhile
        CONNECTED.store(false, Ordering::SeqCst);
    }
    if refused && state.refreshed.is_none_or(|t| t.elapsed() >= REFRESH_EVERY) {
        log::info!("the server no longer takes this computer's app token: asking for a new one");
        set_problem(Some("Encore no longer accepts this computer's token: asking for a new one".into()));
        if let Some(l) = state.link.take() {
            l.stop();
            state.replacing = Some(l.token().to_string());
        }
        let server = config.server.clone();
        if let Some(s) = config.server_mut(&server) {
            s.token.clear();
            config.save();
        }
        state.refreshed = Some(Instant::now());
        state.asking = true;
        return start(api, config);
    }
    if let Some((at, _)) = state.retry {
        if state.link.is_none() && Instant::now() >= at {
            state.retry = None;
            state.asking = true;
            return start(api, config);
        }
    }
    Task::none()
}

/// Whether the running link reports that the server refuses its token.
fn refused(state: &State) -> bool {
    state
        .link
        .as_ref()
        .and_then(|l| l.last_error())
        .is_some_and(|e| e.contains(TOKEN_REFUSED))
}

/// The queue's server songs carry the refused token in their headers: give them the new one.
/// Playing or paused, the queue is handed over again at the same place (paused stays paused);
/// stopped, it is left as it is (handing it over would start the song) and the next song
/// started from a page carries the new token.
fn reauth_queue(old: &str, new: &str) {
    let Some(e) = engine::engine() else {
        return;
    };
    let (from, to) = (engine::tracks::bearer(old), engine::tracks::bearer(new));
    let st = e.status();
    let (queue, changed) = with_auth(&st.queue, &from, &to);
    let paused = match st.state {
        engine::PlayState::Playing => false,
        engine::PlayState::Paused => true,
        _ => return,
    };
    let Some(i) = st.index.filter(|_| changed) else {
        return;
    };
    // the order kept for Shuffle off names the same songs, so it gets the new token too
    if let Some(order) = crate::player::queue::order() {
        crate::player::queue::set_order(Some(with_auth(&order, &from, &to).0));
    }
    e.play_queue(queue, i, st.position);
    if paused {
        e.send(engine::Command::Pause);
    }
}

/// `tracks` with the `from` header replaced by `to`, and whether any had it.
fn with_auth(
    tracks: &[engine::Track],
    from: &engine::tracks::Auth,
    to: &engine::tracks::Auth,
) -> (Vec<engine::Track>, bool) {
    let mut changed = false;
    let out = tracks
        .iter()
        .map(|t| match &t.source {
            Source::Url { url, headers } if headers.contains(from) => {
                changed = true;
                let headers = headers
                    .iter()
                    .map(|h| if h == from { to.clone() } else { h.clone() })
                    .collect();
                engine::Track {
                    source: Source::Url {
                        url: url.clone(),
                        headers,
                    },
                    ..t.clone()
                }
            }
            _ => t.clone(),
        })
        .collect();
    (out, changed)
}

/// Start (or re-point) the link. Same server and token: nothing changes.
pub fn connect(state: &mut State, server: &str, token: &str) {
    let server = crate::config::normalize(server);
    let id = device_id(&server);
    if let Some(l) = state.link.as_ref() {
        if !l.stopped() && l.server() == server && l.token() == token && l.device_id() == id {
            return;
        }
        l.stop();
    }
    let eng = match engine::start() {
        Ok(e) => e,
        Err(e) => {
            log::warn!("the device link did not start: {e}");
            set_problem(Some(e.clone()));
            state.error = Some(e);
            CONNECTED.store(false, Ordering::SeqCst);
            return;
        }
    };
    let cfg = EncoreConfig {
        url: server.clone(),
        token: token.to_string(),
        device_id: id.clone(),
        device_name: engine::device_name(),
        embedded: false,
        ..Default::default()
    };
    let link = cadenza::encore::start(Arc::new(cfg), eng);
    state.server = server;
    state.device_id = id;
    state.error = None;
    state.link = Some(Arc::new(link));
    set_problem(None);
    CONNECTED.store(true, Ordering::SeqCst);
}

/// Stop the link (sign out, server change). The engine keeps running.
pub fn stop(state: &mut State) {
    if let Some(l) = state.link.take() {
        l.stop();
    }
    CONNECTED.store(false, Ordering::SeqCst);
    state.server.clear();
    state.error = None;
    state.asking = false;
    state.retry = None;
    state.replacing = None;
    set_problem(None);
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_ids_are_stable_and_per_server() {
        let a = device_id("http://a:8080/");
        assert_eq!(a, device_id("http://a:8080"));
        assert_ne!(a, device_id("http://b:8080"));
        assert!(a.starts_with("desktop-") && a.len() == 8 + 16);
        assert_eq!(fnv1a(""), 0xcbf2_9ce4_8422_2325);
    }

    #[test]
    fn a_new_token_replaces_the_old_one_in_server_songs_only() {
        let (old, new) = (engine::tracks::bearer("old"), engine::tracks::bearer("new"));
        let row = serde_json::json!({ "id": 7, "title": "Song" });
        let song = engine::tracks::from_encore("http://nas:8080", &old, &row).unwrap();
        let other = engine::tracks::from_encore("http://nas:8080", &engine::tracks::bearer("x"), &row).unwrap();
        let file = engine::test_sound::track(std::path::PathBuf::from("/tmp/a.wav"));
        let (out, changed) = with_auth(&[song, other.clone(), file.clone()], &old, &new);
        assert!(changed);
        match &out[0].source {
            Source::Url { headers, .. } => {
                assert!(headers.contains(&new) && !headers.contains(&old));
                assert!(headers.iter().any(|(k, _)| k == "X-Encore-Decoders"), "the other headers stay");
            }
            _ => panic!("still a stream"),
        }
        assert_eq!(out[0].title, "Song");
        assert_eq!(out[1], other);
        assert_eq!(out[2], file);
        assert!(!with_auth(&[other, file], &old, &new).1);
    }

    #[test]
    fn a_failed_token_request_is_tried_again_later_and_a_refusal_is_not() {
        let mut state = State::default();
        let mut config = Config::default();
        config.remember("http://nas:8080");
        let away = Message::Token("http://nas:8080".into(), Err(ApiError::new(0, "Not connected to the server")));
        let _ = update(&mut state, away.clone(), &mut config);
        let (at, n) = state.retry.expect("a retry");
        assert_eq!(n, 0);
        assert!(at > Instant::now() + Duration::from_secs(25));
        let _ = update(&mut state, away, &mut config);
        assert_eq!(state.retry.unwrap().1, 1);
        assert!(problem().is_some_and(|p| p.contains("Not connected")));
        let mut state = State::default();
        let refused = Message::Token("http://nas:8080".into(), Err(ApiError::new(401, "Not signed in")));
        let _ = update(&mut state, refused, &mut config);
        assert!(state.retry.is_none(), "a 401 is the session's matter, not the network's");
        stop(&mut state);
        assert!(problem().is_none());
    }
}
