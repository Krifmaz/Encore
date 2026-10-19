//! Soulseek, run by Encore: slskd (github.com/slskd/slskd) as a headless child process. Encore
//! downloads it from slskd's GitHub releases (Download tools → Install/Update), writes its slskd.yml
//! from Encore's settings, starts it on 127.0.0.1 with an API key only Encore knows, restarts it when
//! it exits or the settings change, and keeps it inside the VPN when the VPN is on (a local SOCKS5
//! proxy for every outbound connection, the forwarded port relayed in). `sources::slskd` talks to it.
//!
//! Layout under the config folder: tools/slskd/<version>/slskd (+ `current` naming the version),
//! slskd/ (its --app-dir: slskd.yml, database, encore-run.log).
use crate::Ctx;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use encore_config::Config;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::{Child, Command};

/// slskd's HTTP API, on loopback only.
pub const WEB_PORT: u16 = 5031;
/// The setting that holds the API key Encore generated for itself (secret, made once).
pub const API_KEY_SETTING: &str = "slskd_managed_api_key";
/// slskd's default Soulseek server.
const SERVER: (&str, u16) = ("vps.slsknet.org", 2271);
const LOG_MAX: u64 = 5 * 1024 * 1024;
/// slskd's `soulseek.connection.timeout.connect`: how long it waits to reach the Soulseek server.
pub const CONNECT_TIMEOUT_MS: u64 = 30_000;
/// The empty folder slskd.yml names as the web UI's content (beside the binary; wwwroot is deleted).
const NO_WEB_UI: &str = "no-web-ui";
const RELEASES: &str = "https://api.github.com/repos/slskd/slskd/releases/latest";

pub const WAITING_FOR_VPN: &str = "Waiting for the VPN – Soulseek stays off until it is connected";
pub const NOT_INSTALLED: &str = "Soulseek is not installed – install it under Settings → Download tools (slskd – Soulseek engine)";

// ---- status ---------------------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
struct Status {
    /// off | not_installed | starting | running | crashed | waiting_vpn | blocked
    state: &'static str,
    version: Option<String>,
    pid: Option<u32>,
    error: Option<String>,
    listen_port: Option<u16>,
    vpn: bool,
    /// the share scan's progress (percent) while slskd builds its share list
    scan: Option<u8>,
    /// something to fix although slskd runs (downloads go to the fallback folder)
    warning: Option<String>,
    /// the folder slskd downloads into (the fallback when the configured one can't be written)
    downloads: Option<String>,
    /// why the engine isn't signed in to the Soulseek network, in plain words (from slskd's own log)
    server_error: Option<String>,
}

/// What the managed engine keeps while the server runs: its state, the last lines it printed, the last
/// downloads-folder probe and its supervisor's wake-ups. The server keeps one; clones share it.
#[derive(Clone)]
pub struct Managed(Arc<Inner>);

struct Inner {
    status: Mutex<Status>,
    /// the last lines slskd printed (also written to encore-run.log): shown on the Soulseek card to
    /// admins, so a sign-in problem can be read without opening the container
    log_tail: Mutex<VecDeque<String>>,
    wake: tokio::sync::Notify,
    /// an install is running
    installing: AtomicBool,
    /// set by Start now: the next check starts slskd at once instead of waiting out the backoff
    start_now: AtomicBool,
    /// the last downloads-folder probe, by what was probed
    probed: Mutex<Option<(ProbeKey, ProbeAnswer)>>,
    /// the Soulseek server's address the last time its name was looked up through the VPN
    server_ip: Mutex<Option<(String, std::net::IpAddr)>>,
    /// the session with the Soulseek server: its state, Encore's reconnect schedule and the search gate
    session: crate::slskd_session::Session,
    /// where slskd's API answers when not on its port on loopback (tests: a stand-in)
    target: Mutex<Option<Target>>,
    /// counts the slskd processes started (the session supervisor tells a restart by it)
    generation: AtomicU64,
    /// the process (its generation) that has finished starting: its API takes a connect request
    started: AtomicU64,
    /// when the engine last became "running"
    running_at: Mutex<Option<Instant>>,
    /// the direct route's relay to the Soulseek server, once started
    relay: Mutex<Option<Arc<Relay>>>,
}

impl Default for Managed {
    fn default() -> Self {
        Managed(Arc::new(Inner {
            status: Mutex::new(Status { state: "off", ..Default::default() }),
            log_tail: Default::default(),
            wake: Default::default(),
            installing: Default::default(),
            start_now: Default::default(),
            probed: Default::default(),
            server_ip: Default::default(),
            session: Default::default(),
            target: Default::default(),
            generation: Default::default(),
            started: Default::default(),
            running_at: Default::default(),
            relay: Default::default(),
        }))
    }
}

impl Managed {
    /// slskd's last 20 lines, oldest first.
    pub fn log_tail(&self) -> Vec<String> {
        self.0.log_tail.lock().unwrap().iter().cloned().collect()
    }

    /// {state: off|not_installed|starting|running|crashed|waiting_vpn|blocked, version, pid, error, listen_port,
    /// vpn, scanning, warning, download_dir}
    pub fn status(&self) -> Value {
        let s = self.0.status.lock().unwrap().clone();
        json!({ "state": s.state, "version": s.version, "pid": s.pid, "error": s.error, "listen_port": s.listen_port, "vpn": s.vpn, "scanning": s.scan,
            "warning": s.warning, "download_dir": s.downloads, "server_error": s.server_error })
    }

    /// The engine's state: off | not_installed | starting | running | crashed | waiting_vpn | blocked.
    pub fn state(&self) -> &'static str {
        self.0.status.lock().unwrap().state
    }

    /// The session with the Soulseek server (its state, Encore's reconnect schedule, the search gate).
    pub fn session(&self) -> &crate::slskd_session::Session {
        &self.0.session
    }

    /// Tests: slskd's API answers there instead of on its port on loopback.
    #[cfg(any(test, feature = "test-support"))]
    pub fn set_target(&self, t: Target) {
        *self.0.target.lock().unwrap() = Some(t);
    }

    /// Tests: the engine is in this state (as if the supervisor had brought it there).
    #[cfg(any(test, feature = "test-support"))]
    pub fn set_state_for_tests(&self, state: &'static str) {
        self.set_status(|s| s.state = state);
        if state == "running" {
            self.0.started.store(self.generation(), Ordering::SeqCst);
        }
        self.0.session.wake();
    }

    /// Tests: a new slskd process runs, started (as if the supervisor had restarted it).
    #[cfg(any(test, feature = "test-support"))]
    pub fn restarted_for_tests(&self) {
        let gen = self.0.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.0.started.store(gen, Ordering::SeqCst); // and has finished starting
        self.0.session.wake();
    }

    /// The slskd process that runs (or ran last): a new number for each start.
    pub fn generation(&self) -> u64 {
        self.0.generation.load(Ordering::SeqCst)
    }

    /// slskd runs and has finished starting, so a connect request (PUT /server) is acted on: its last
    /// startup line ("Not connecting to the Soulseek server; 'no-connect' option is enabled") has come, or
    /// it has been running for 20 s. (A PUT right after "Application started" was seen to do nothing.)
    pub fn ready(&self) -> bool {
        self.state() == "running"
            && (self.0.started.load(Ordering::SeqCst) == self.generation()
                || self.0.running_at.lock().unwrap().is_some_and(|t| t.elapsed() >= Duration::from_secs(20)))
    }

    fn now_running(&self) {
        *self.0.running_at.lock().unwrap() = Some(Instant::now());
        self.0.session.wake();
    }

    /// Encore's connection attempt: let one connection through to the Soulseek server. On the direct route
    /// the relay connects to the server first (30 s at most), so slskd's login wait (5 s, fixed in slskd)
    /// doesn't have to cover the connect as well; Err when the server can't be reached.
    pub async fn open_door(&self) -> anyhow::Result<()> {
        let direct = !self.0.status.lock().unwrap().vpn;
        let relay = self.0.relay.lock().unwrap().clone();
        if let Some(r) = relay.filter(|_| direct) {
            let up = r.upstream.lock().unwrap().clone();
            let s = dial(&up).await.map_err(|e| anyhow::anyhow!("can't reach {}:{}: {e}", up.0, up.1))?;
            let n = r.dialled.fetch_add(1, Ordering::SeqCst) + 1;
            *r.ready.lock().unwrap() = Some((n, s));
            // not taken while the door is open: closed again, the server mustn't wait on an idle connection
            tokio::spawn(async move {
                tokio::time::sleep(crate::slskd_session::DOOR_WINDOW).await;
                let mut ready = r.ready.lock().unwrap();
                if ready.as_ref().is_some_and(|(m, _)| *m == n) {
                    *ready = None;
                }
            });
        }
        self.0.session.open_door();
        Ok(())
    }

    /// The direct route's relay (started once): its port on loopback, and where it reaches the server.
    async fn relay_port(&self, upstream: (String, u16)) -> std::io::Result<u16> {
        if let Some(r) = self.0.relay.lock().unwrap().as_ref() {
            *r.upstream.lock().unwrap() = upstream;
            return Ok(r.port);
        }
        let l = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
        let port = l.local_addr()?.port();
        let r = Arc::new(Relay { port, upstream: Mutex::new(upstream), ready: Mutex::new(None), dialled: AtomicU64::new(0) });
        {
            let mut cur = self.0.relay.lock().unwrap();
            if let Some(won) = cur.as_ref() {
                return Ok(won.port); // another caller started it meanwhile; this listener just closes
            }
            *cur = Some(r.clone());
        }
        tokio::spawn(relay_loop(l, r, self.0.session.clone()));
        Ok(port)
    }

    fn set_status(&self, f: impl FnOnce(&mut Status)) {
        f(&mut self.0.status.lock().unwrap());
    }

    /// Look at the settings now instead of at the next check (after saving settings, installing, removing).
    pub fn poke(&self) {
        *self.0.probed.lock().unwrap() = None; // the settings may have changed: the downloads folder is probed again
        self.0.wake.notify_one();
        self.0.session.wake(); // and the login with them: the session's schedule starts over
    }

    /// Start now (after a crash or a blocked start): forget the backoff and try at once.
    pub fn start_now(&self) {
        self.0.start_now.store(true, Ordering::SeqCst);
        self.poke();
    }

    /// Where slskd's downloads land right now: the running plan's folder (the fallback when the configured
    /// one can't be written), else the configured one.
    pub fn download_dir(&self, cfg: &Config) -> PathBuf {
        match self.0.status.lock().unwrap().downloads.clone() {
            Some(d) => PathBuf::from(d),
            None => PathBuf::from(cfg.str("slskd_download_path")),
        }
    }

    /// `downloads_dir_for` for the settings, probed only when it matters. While slskd runs (or starts, or
    /// waits for the VPN) with the same folder settings the last answer is reused: the supervisor checks
    /// every few seconds, and writing a probe file each time would keep the downloads disk awake, poke
    /// slskd's own folder watcher and cost a round trip on a network mount. The folder is probed again when
    /// the setting changes, before a start (blocked, crashed) and after poke() or start_now().
    fn probed_downloads_dir(&self, cfg: &Config, data: &Path, configured: &Path) -> Result<(PathBuf, Option<String>), String> {
        let inputs = (configured.to_path_buf(), downloads_chosen(cfg), fallback_dir(data));
        let reuse = matches!(self.state(), "starting" | "running" | "waiting_vpn");
        if reuse {
            if let Some((_, r)) = self.0.probed.lock().unwrap().as_ref().filter(|(k, _)| *k == inputs) {
                return r.clone();
            }
        }
        let r = downloads_dir_for(&inputs.0, inputs.1, &inputs.2);
        *self.0.probed.lock().unwrap() = Some((inputs, r.clone()));
        r
    }

    /// What slskd's own output tells us: it is up once the application has started (its API can be slow
    /// to answer while a big share scan runs), and how far the share scan is.
    fn on_line(&self, l: &str) {
        {
            let mut t = self.0.log_tail.lock().unwrap();
            if t.len() >= 20 {
                t.pop_front();
            }
            t.push_back(l.chars().take(300).collect());
        }
        if l.contains("Application started") {
            let mut up = false;
            self.set_status(|s| {
                if s.state == "starting" {
                    s.state = "running";
                    s.error = None;
                    up = true;
                }
            });
            if up {
                self.now_running();
            }
        }
        if l.contains("Not connecting to the Soulseek server") || l.contains("Attempting to connect to the Soulseek server") {
            self.0.started.store(self.generation(), Ordering::SeqCst);
            self.0.session.wake();
        }
        if let Some(hint) = server_hint(l) {
            self.set_status(|s| s.server_error = hint);
        }
        self.0.session.on_line(l);
        if let Some(p) = scan_percent(l) {
            self.set_status(|s| s.scan = (p < 100).then_some(p));
        } else if l.contains("Share scan started") {
            self.set_status(|s| s.scan = Some(0));
        } else if l.to_lowercase().contains("scan completed") || l.to_lowercase().contains("scan failed") {
            self.set_status(|s| s.scan = None);
        }
    }
}

/// What a line of slskd's log means for the sign-in, in words a person can act on. `None` for lines
/// that say nothing about it; `Some(None)` for a successful sign-in (clears the problem).
pub fn server_hint(line: &str) -> Option<Option<String>> {
    let l = line.to_lowercase();
    let tail = || {
        let t = line.split("] ").last().unwrap_or(line).trim();
        t.chars().take(160).collect::<String>()
    };
    if l.contains("logged in to the soulseek server") || l.contains("login succeeded") || (l.contains("logged in") && l.contains(" as ")) {
        return Some(None);
    }
    if l.contains("invalidpass") {
        return Some(Some("wrong password for this username – enter the right one, or choose an unused username to create a new account".into()));
    }
    if l.contains("invalidusername") {
        return Some(Some("Soulseek doesn't allow this username – choose another".into()));
    }
    if l.contains("invalidversion") {
        return Some(Some("Soulseek refused this engine version – update it under Download tools".into()));
    }
    if l.contains("kicked") || l.contains("same username") || l.contains("another client") || l.contains("elsewhere") {
        return Some(Some("signed out because the same username signed in from another Soulseek client – close it, or use a different username here".into()));
    }
    if (l.contains("login") || l.contains("log in")) && (l.contains("reject") || l.contains("fail")) {
        return Some(Some(format!("Soulseek refused the sign-in: {}", tail())));
    }
    if (l.contains("soulseek server") || l.contains("server connection") || l.contains("slsknet.org"))
        && (l.contains("timed out") || l.contains("refused") || l.contains("unreachable") || l.contains("failed to connect") || l.contains("unable to connect") || l.contains("no such host") || l.contains("could not resolve"))
    {
        return Some(Some(format!("can't reach the Soulseek server (server.slsknet.org:2242) – check the VPN or firewall: {}", tail())));
    }
    if l.contains("disconnected from the soulseek server") || l.contains("soulseek client disconnected") {
        return Some(Some(format!("disconnected from the Soulseek server: {}", tail())));
    }
    None
}


// ---- the relay to the Soulseek server (direct route) ----------------------------------------------

/// On the direct route slskd.yml names this relay on loopback as the Soulseek server. It passes a
/// connection on to the real server only through the session's door (`slskd_session::door`): while one
/// of Encore's attempts is under way. slskd's own reconnect attempts are reset at once and never reach
/// the server. (On the VPN route Encore's SOCKS proxy keeps the same door.)
struct Relay {
    port: u16,
    /// the Soulseek server (host, port)
    upstream: Mutex<(String, u16)>,
    /// the connection to the server made for the attempt under way: (its number, the stream)
    ready: Mutex<Option<(u64, tokio::net::TcpStream)>>,
    dialled: AtomicU64,
}

/// A connection to the Soulseek server: 30 s to connect, keepalive on (the relay would not notice a
/// dead server connection otherwise, and neither would slskd behind it).
async fn dial(up: &(String, u16)) -> std::io::Result<tokio::net::TcpStream> {
    let s = tokio::time::timeout(Duration::from_millis(CONNECT_TIMEOUT_MS), tokio::net::TcpStream::connect((up.0.as_str(), up.1))).await
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, format!("no answer within {} s (timed out)", CONNECT_TIMEOUT_MS / 1000)))??;
    let _ = s.set_nodelay(true);
    let ka = socket2::TcpKeepalive::new().with_time(Duration::from_secs(60)).with_interval(Duration::from_secs(15));
    let _ = socket2::SockRef::from(&s).set_tcp_keepalive(&ka);
    Ok(s)
}

async fn relay_loop(l: tokio::net::TcpListener, r: Arc<Relay>, session: crate::slskd_session::Session) {
    loop {
        let s = match l.accept().await {
            Ok((s, _)) => s,
            Err(e) => {
                tracing::warn!("soulseek: the relay to the server couldn't accept: {e}");
                tokio::time::sleep(Duration::from_millis(500)).await;
                continue;
            }
        };
        let Some(pass) = session.door() else {
            // slskd's own attempt: reset at once, so it fails in a moment without reaching the server
            let _ = socket2::SockRef::from(&s).set_linger(Some(Duration::ZERO));
            drop(s);
            continue;
        };
        let ready = r.ready.lock().unwrap().take().map(|(_, u)| u);
        let up = r.upstream.lock().unwrap().clone();
        tokio::spawn(async move {
            let _pass = pass; // the connection counts as open until this task ends
            let mut s = s;
            let _ = s.set_nodelay(true);
            let mut u = match ready {
                Some(u) => u,
                None => match dial(&up).await {
                    Ok(u) => u,
                    Err(e) => {
                        tracing::debug!("soulseek: the relay couldn't reach {}:{}: {e}", up.0, up.1);
                        return;
                    }
                },
            };
            let _ = tokio::io::copy_bidirectional(&mut s, &mut u).await;
        });
    }
}

// ---- paths ----------------------------------------------------------------------------------------

fn tools_dir(data: &Path) -> PathBuf {
    data.join("tools").join("slskd")
}

fn app_dir(data: &Path) -> PathBuf {
    data.join("slskd")
}

/// (version, binary) from `current`, when that binary is there.
fn installed_in(data: &Path) -> Option<(String, PathBuf)> {
    let root = tools_dir(data);
    let ver = std::fs::read_to_string(root.join("current")).ok()?.trim().to_string();
    if ver.is_empty() || ver.contains('/') || ver.contains("..") {
        return None;
    }
    let bin = root.join(&ver).join("slskd");
    bin.is_file().then_some((ver, bin))
}

pub fn installed_version(st: &impl Ctx) -> Option<String> {
    installed_in(st.data_dir()).map(|(v, _)| v)
}

/// Soulseek is switched on with a login (whether or not slskd is installed yet).
pub fn configured(st: &impl Ctx) -> bool {
    st.cfg().bool("slskd_enabled") && !st.cfg().str("soulseek_username").trim().is_empty() && !st.cfg().str("soulseek_password").is_empty()
}

pub fn base_url() -> String {
    format!("http://127.0.0.1:{WEB_PORT}")
}

// ---- slskd's API ----------------------------------------------------------------------------------

/// Where slskd's API answers.
#[derive(Clone)]
pub enum Target {
    /// an address ("http://127.0.0.1:5031")
    Url(String),
    /// tests: a stand-in slskd answering in this process (no sockets, so tokio's paused time can run
    /// through the reconnect schedule and the search spacing without HTTP timeouts firing)
    #[cfg(any(test, feature = "test-support"))]
    App(axum::Router),
}

/// slskd's API didn't answer at all: it isn't listening (starting, restarting or stopped). Its words are
/// the ones people saw before ("Soulseek is starting – try again in a moment").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unreachable;

impl std::fmt::Display for Unreachable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Soulseek is starting – try again in a moment")
    }
}

impl std::error::Error for Unreachable {}

/// Tests: a stand-in slskd answers this status to play one that isn't listening ([`Unreachable`]).
#[cfg(any(test, feature = "test-support"))]
pub const STAND_IN_DOWN: u16 = 599;

/// One call to slskd's API with Encore's key: (status, body). `timeout` overrides the client's 30 s.
/// A connection refused is [`Unreachable`] (slskd isn't listening: starting, restarting or stopped).
pub async fn api(st: &impl Ctx, method: reqwest::Method, path: &str, body: Option<&Value>, query: &[(&str, &str)], timeout: Option<Duration>) -> anyhow::Result<(u16, String)> {
    let path = format!("/api/v0/{}", path.trim_start_matches('/'));
    let target = st.requests().slskd.0.target.lock().unwrap().clone();
    let base = match target {
        Some(Target::Url(u)) => u,
        #[cfg(any(test, feature = "test-support"))]
        Some(Target::App(app)) => return call_app(app, st, method, &path, body, query).await,
        None => base_url(),
    };
    let mut rb = st.http().request(method, format!("{base}{path}")).header("X-API-Key", api_key(st)).query(query);
    if let Some(b) = body {
        rb = rb.json(b);
    }
    if let Some(t) = timeout {
        rb = rb.timeout(t);
    }
    let r = rb.send().await.map_err(|e| {
        if e.is_connect() { anyhow::Error::new(Unreachable) } else { anyhow::anyhow!("Soulseek engine didn't answer: {e}") }
    })?;
    let status = r.status().as_u16();
    Ok((status, r.text().await.unwrap_or_default()))
}

#[cfg(any(test, feature = "test-support"))]
async fn call_app(app: axum::Router, st: &impl Ctx, method: reqwest::Method, path: &str, body: Option<&Value>, query: &[(&str, &str)]) -> anyhow::Result<(u16, String)> {
    use tower::ServiceExt;
    let q: Vec<String> = query.iter().map(|(k, v)| format!("{}={}", urlencoding::encode(k), urlencoding::encode(v))).collect();
    let uri = if q.is_empty() { path.to_string() } else { format!("{path}?{}", q.join("&")) };
    let mut req = axum::http::Request::builder().method(method).uri(uri).header("x-api-key", api_key(st));
    let bytes = match body {
        Some(b) => {
            req = req.header("content-type", "application/json");
            serde_json::to_vec(b)?
        }
        None => vec![],
    };
    let resp = app.oneshot(req.body(axum::body::Body::from(bytes))?).await?;
    let status = resp.status().as_u16();
    if status == STAND_IN_DOWN {
        return Err(Unreachable.into());
    }
    let text = axum::body::to_bytes(resp.into_body(), usize::MAX).await.map(|b| String::from_utf8_lossy(&b).to_string()).unwrap_or_default();
    Ok((status, text))
}

/// [`api`] as JSON (null for an empty answer); an error answer is an Err with its status and text.
pub async fn api_json(st: &impl Ctx, method: reqwest::Method, path: &str, body: Option<&Value>) -> anyhow::Result<Value> {
    let (status, text) = api(st, method, path, body, &[], Some(Duration::from_secs(10))).await?;
    if status >= 400 {
        anyhow::bail!("slskd answered {status}: {}", text.chars().take(200).collect::<String>());
    }
    Ok(serde_json::from_str(&text).unwrap_or(Value::Null))
}

// ---- the downloads folder --------------------------------------------------------------------------

/// Where downloads go when the configured folder can't be used: <config>/downloads/slskd (the config
/// folder is Encore's own, so it is always writable).
pub fn fallback_dir(data: &Path) -> PathBuf {
    data.join("downloads").join("slskd")
}

/// Encore's own uid and gid (what slskd runs as too), for the chown line.
fn ids() -> (u32, u32) {
    // SAFETY: both calls only read the process's credentials and cannot fail.
    unsafe { (libc::geteuid(), libc::getegid()) }
}

/// Can Encore write in `dir`? The folder is made when `create` is set (download folders; the library
/// folder has to be mounted already), then a probe file is written and removed. The Err is the reason.
pub fn probe_writable(dir: &Path, create: bool) -> Result<(), String> {
    if create {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    } else if !dir.is_dir() {
        return Err("the folder doesn't exist inside the container – check the volume mapping in your compose file".into());
    }
    let probe = dir.join(format!(".encore-write-test-{}", std::process::id()));
    std::fs::write(&probe, b"x").map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&probe);
    Ok(())
}

/// "/downloads/slskd isn't writable by Encore (uid 1000, gid 1000): Permission denied (os error 13). Fix:
/// chown 1000:1000 /downloads/slskd (on the host: the folder mounted there), or set PUID/PGID to its owner"
pub fn unwritable(dir: &Path, why: &str) -> String {
    let (uid, gid) = ids();
    format!("{d} isn't writable by Encore (uid {uid}, gid {gid}): {why}. Fix: chown {uid}:{gid} {d} (on the host: the folder mounted there), or set PUID/PGID to its owner", d = dir.display())
}

/// The folder slskd downloads into, probed before slskd starts (slskd itself would exit on it and be
/// started again for ever). The configured folder when Encore can write there. When it can't: a folder
/// the user chose (saved in Settings or set by SLSKD_DOWNLOAD_PATH) blocks the start with the exact fix
/// (Err); the default folder is replaced by the fallback, with a warning that says so (Ok with Some).
pub fn downloads_dir_for(configured: &Path, chosen: bool, fallback: &Path) -> Result<(PathBuf, Option<String>), String> {
    let Err(why) = probe_writable(configured, true) else { return Ok((configured.to_path_buf(), None)) };
    let problem = format!("Soulseek's downloads folder {}", unwritable(configured, &why));
    if chosen {
        return Err(problem);
    }
    match probe_writable(fallback, true) {
        Ok(()) => Ok((fallback.to_path_buf(), Some(format!("Downloads go to {} for now – {}", fallback.display(), unwritable(configured, &why))))),
        Err(e2) => Err(format!("{problem}. The fallback {} isn't writable either: {e2}", fallback.display())),
    }
}

/// The user chose the Soulseek downloads folder (saved in Settings or set by the environment) rather
/// than leaving the default.
fn downloads_chosen(cfg: &Config) -> bool {
    const KEY: &str = "slskd_download_path";
    cfg.env_locked(KEY) || cfg.def(KEY).map(|d| d.default != cfg.get(KEY)).unwrap_or(true)
}

/// Where slskd's downloads land right now: the running plan's folder (the fallback when the configured
/// one can't be written), else the configured one.
pub fn download_dir(st: &impl Ctx) -> PathBuf {
    st.requests().slskd.download_dir(st.cfg())
}

/// What a downloads-folder probe looked at: (configured folder, chosen, fallback folder).
type ProbeKey = (PathBuf, bool, PathBuf);
/// What it found: the folder to use and a note, or why none works.
type ProbeAnswer = Result<(PathBuf, Option<String>), String>;

/// The key slskd.yml gives Encore (48 random characters, generated once and kept in the settings).
pub fn api_key(st: &impl Ctx) -> String {
    let k = st.cfg().str(API_KEY_SETTING);
    if k.len() >= 16 {
        return k;
    }
    let k = encore_core::ids::random_alnum(48);
    st.cfg().set_many(&json!({ API_KEY_SETTING: k }));
    st.cfg().str(API_KEY_SETTING)
}

// ---- releases and install -------------------------------------------------------------------------

/// The token slskd's release assets use for this machine.
fn arch_token(arch: &str) -> Option<&'static str> {
    match arch {
        "x86_64" => Some("x64"),
        "aarch64" => Some("arm64"),
        _ => None,
    }
}

/// From a GitHub release: (version, URL of the glibc Linux .zip for `arch`) – e.g.
/// slskd-0.26.1-linux-x64.zip, never the musl or 32-bit arm builds.
pub fn pick_asset(release: &Value, arch: &str) -> Option<(String, String)> {
    let tok = arch_token(arch)?;
    let ver = release["tag_name"].as_str()?.trim().trim_start_matches('v').to_string();
    if ver.is_empty() {
        return None;
    }
    let url = release["assets"].as_array()?.iter().find_map(|a| {
        let name = a["name"].as_str()?.to_lowercase();
        let parts: Vec<&str> = name.trim_end_matches(".zip").split(['-', '_', '.']).collect();
        let ok = name.ends_with(".zip") && parts.contains(&"linux") && parts.contains(&tok) && !parts.contains(&"musl");
        ok.then(|| a["browser_download_url"].as_str().map(String::from)).flatten()
    })?;
    Some((ver, url))
}

/// The newest slskd release for this machine: (version, asset URL).
pub async fn latest(st: &impl Ctx) -> anyhow::Result<(String, String)> {
    let (code, v) = encore_http::github::get(st, RELEASES).await?;
    if code >= 400 {
        anyhow::bail!("GitHub returned {code} for the latest slskd release: {}", v["message"].as_str().unwrap_or(""));
    }
    pick_asset(&v, std::env::consts::ARCH)
        .ok_or_else(|| anyhow::anyhow!("The latest slskd release has no Linux build for {}", std::env::consts::ARCH))
}

/// Clears the installing flag when an install ends, however it ends.
struct InstallGuard(Managed);
impl Drop for InstallGuard {
    fn drop(&mut self) {
        self.0 .0.installing.store(false, Ordering::SeqCst);
    }
}

/// Download `url`, unpack it as `ver`, check that it runs, switch to it, restart slskd on it and
/// delete older versions.
pub async fn install(st: &impl Ctx, ver: &str, url: &str) -> anyhow::Result<()> {
    let ver = ver.trim().trim_start_matches('v').to_string();
    if ver.is_empty() || !ver.chars().all(|c| c.is_ascii_alphanumeric() || ".-+_".contains(c)) {
        anyhow::bail!("not a version: {ver:?}");
    }
    let managed = st.requests().slskd.clone();
    if managed.0.installing.swap(true, Ordering::SeqCst) {
        anyhow::bail!("slskd is already being installed");
    }
    let _guard = InstallGuard(managed);
    let root = tools_dir(st.data_dir());
    std::fs::create_dir_all(&root)?;
    let zip = root.join(format!(".{ver}.zip"));
    let res = async {
        download(st, url, &zip).await?;
        let (root2, zip2, ver2) = (root.clone(), zip.clone(), ver.clone());
        let bin = tokio::task::spawn_blocking(move || unpack(&root2, &ver2, &zip2)).await??;
        verify(&bin).await?;
        let (root2, ver2) = (root.clone(), ver.clone());
        tokio::task::spawn_blocking(move || switch_to(&root2, &ver2)).await??;
        anyhow::Ok(())
    }
    .await;
    let _ = std::fs::remove_file(&zip);
    let _ = std::fs::remove_dir_all(root.join(format!(".{ver}.new")));
    res?;
    st.db().log("info", &format!("Soulseek engine slskd {ver} installed"));
    st.requests().slskd.poke(); // the supervisor (re)starts it on the new binary
    Ok(())
}

async fn download(st: &impl Ctx, url: &str, to: &Path) -> anyhow::Result<()> {
    use futures_util::StreamExt;
    let r = st.http().get(url).timeout(Duration::from_secs(900)).send().await.map_err(|e| anyhow::anyhow!("download failed: {e}"))?;
    if !r.status().is_success() {
        anyhow::bail!("download failed: {} returned {}", url, r.status());
    }
    let mut f = tokio::fs::File::create(to).await?;
    let mut body = r.bytes_stream();
    let mut n: u64 = 0;
    while let Some(chunk) = body.next().await {
        let chunk = chunk.map_err(|e| anyhow::anyhow!("download failed: {e}"))?;
        n += chunk.len() as u64;
        if n > 1024 * 1024 * 1024 {
            anyhow::bail!("download failed: more than 1 GB");
        }
        tokio::io::AsyncWriteExt::write_all(&mut f, &chunk).await?;
    }
    tokio::io::AsyncWriteExt::flush(&mut f).await?;
    Ok(())
}

/// Unzip into tools/slskd/.<ver>.new, drop the web UI (wwwroot: Encore runs it headless) and make
/// the binary executable. Returns the binary's path.
fn unpack(root: &Path, ver: &str, zip: &Path) -> anyhow::Result<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let dest = root.join(format!(".{ver}.new"));
    let _ = std::fs::remove_dir_all(&dest);
    std::fs::create_dir_all(&dest)?;
    let mut a = zip::ZipArchive::new(std::fs::File::open(zip)?).map_err(|e| anyhow::anyhow!("not a zip file: {e}"))?;
    for i in 0..a.len() {
        let mut f = a.by_index(i)?;
        let Some(rel) = f.enclosed_name() else { continue }; // "../" and absolute names are skipped
        let out = dest.join(rel);
        if f.is_dir() {
            std::fs::create_dir_all(&out)?;
            continue;
        }
        if let Some(p) = out.parent() {
            std::fs::create_dir_all(p)?;
        }
        let mut w = std::fs::File::create(&out)?;
        std::io::copy(&mut f, &mut w)?;
        if let Some(mode) = f.unix_mode() {
            let _ = std::fs::set_permissions(&out, std::fs::Permissions::from_mode(mode & 0o755));
        }
    }
    let _ = std::fs::remove_dir_all(dest.join("wwwroot"));
    std::fs::create_dir_all(dest.join(NO_WEB_UI))?; // web.content_path must name a folder beside the binary
    let bin = dest.join("slskd");
    if !bin.is_file() {
        anyhow::bail!("the download has no slskd program in it");
    }
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755))?;
    Ok(bin)
}

fn child_env(cmd: &mut Command) {
    // the container's own SLSKD_* variables (old compose files) would configure slskd behind Encore's back
    for (k, _) in std::env::vars_os() {
        if k.to_string_lossy().to_uppercase().starts_with("SLSKD_") || k == "APP_DIR" {
            cmd.env_remove(k);
        }
    }
    cmd.env("DOTNET_SYSTEM_GLOBALIZATION_INVARIANT", "1").env("DOTNET_gcServer", "0").env("NO_COLOR", "1").env("DOTNET_CLI_TELEMETRY_OPTOUT", "1");
}

/// The new binary must run `--version` on this machine.
async fn verify(bin: &Path) -> anyhow::Result<String> {
    let mut cmd = Command::new(bin);
    cmd.arg("--version").stdin(Stdio::null()).kill_on_drop(true);
    child_env(&mut cmd);
    let o = tokio::time::timeout(Duration::from_secs(60), cmd.output()).await
        .map_err(|_| anyhow::anyhow!("slskd --version did not finish within 60 s"))?
        .map_err(|e| anyhow::anyhow!("slskd doesn't run on this machine: {e}"))?;
    let text = format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
    if !o.status.success() {
        anyhow::bail!("slskd --version failed ({}): {}", o.status, text.trim().chars().take(300).collect::<String>());
    }
    Ok(encore_core::version::extract_version(&text).unwrap_or_else(|| text.trim().to_string()))
}

/// Move .<ver>.new into place, point `current` at it and delete every other version.
fn switch_to(root: &Path, ver: &str) -> anyhow::Result<()> {
    let dest = root.join(ver);
    let _ = std::fs::remove_dir_all(&dest);
    std::fs::rename(root.join(format!(".{ver}.new")), &dest)?;
    let tmp = root.join(".current.tmp");
    std::fs::write(&tmp, format!("{ver}\n"))?;
    std::fs::rename(&tmp, root.join("current"))?;
    for e in std::fs::read_dir(root)?.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if e.path().is_dir() && name != ver && !name.starts_with('.') {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
    Ok(())
}

/// Stop slskd and delete it (the Soulseek data under config/slskd – its database and settings – stays).
pub fn reset(st: &impl Ctx) {
    let managed = &st.requests().slskd;
    if let Some(pid) = managed.0.status.lock().unwrap().pid {
        signal(pid, "KILL");
    }
    let _ = std::fs::remove_dir_all(tools_dir(st.data_dir()));
    managed.set_status(|s| {
        s.state = "not_installed";
        s.version = None;
        s.pid = None;
        s.error = None;
    });
    st.db().log("info", "Soulseek engine slskd removed");
    managed.poke();
}

// ---- slskd.yml ------------------------------------------------------------------------------------

/// Everything slskd.yml says, from Encore's settings.
#[derive(Clone, Debug, PartialEq)]
pub struct Yml {
    pub username: String,
    pub password: String,
    pub server: (String, u16),
    pub listen_ip: String,
    pub listen_port: u16,
    pub downloads: String,
    pub incomplete: String,
    pub shares: Vec<String>,
    pub upload_slots: i64,
    /// relative to the binary's folder (slskd insists): an empty folder, as there is no web UI
    pub content_path: String,
    pub api_key: String,
    /// (port, username, password) of the local SOCKS5 proxy into the VPN
    pub proxy: Option<(u16, String, String)>,
}

/// A YAML double-quoted scalar (JSON's escaping is valid YAML).
fn q(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

pub fn render_yml(y: &Yml) -> String {
    let mut o = String::from("# Written by Encore from its settings – edits here are overwritten.\n");
    o += "headless: true\nremote_configuration: false\nremote_file_management: false\ninstance_name: encore\n";
    // Encore connects to the Soulseek server itself, on its own schedule (crate::slskd_session)
    o += "flags:\n  no_logo: true\n  no_version_check: true\n  no_connect: true\n";
    o += "logger:\n  no_disk: true\n  no_color: true\n";
    o += &format!("directories:\n  downloads: {}\n  incomplete: {}\n", q(&y.downloads), q(&y.incomplete));
    o += "shares:\n";
    if y.shares.is_empty() {
        o += "  directories: []\n";
    } else {
        o += "  directories:\n";
        for s in &y.shares {
            o += &format!("    - {}\n", q(s));
        }
    }
    o += "  cache:\n    retention: 1440\n";
    o += &format!("transfers:\n  upload:\n    slots: {}\n", y.upload_slots.max(1));
    o += "soulseek:\n";
    o += &format!("  address: {}\n  port: {}\n", q(&y.server.0), y.server.1);
    o += &format!("  username: {}\n  password: {}\n", q(&y.username), q(&y.password));
    o += &format!("  listen_ip_address: {}\n  listen_port: {}\n", q(&y.listen_ip), y.listen_port);
    o += "  description: \"Encore\"\n";
    // 30 s to reach the server (slskd's default is 10 s; through a VPN that can be too short). The login's
    // own wait (Soulseek.NET's message timeout, 5 s) is not a setting of slskd: docs/backlog/slskd-upstream.md
    o += &format!("  connection:\n    timeout:\n      connect: {CONNECT_TIMEOUT_MS}\n");
    if let Some((port, user, pass)) = &y.proxy {
        o += &format!("    proxy:\n      enabled: true\n      address: \"127.0.0.1\"\n      port: {port}\n      username: {}\n      password: {}\n", q(user), q(pass));
    }
    o += &format!("web:\n  port: {WEB_PORT}\n  ip_address: \"127.0.0.1\"\n  url_base: \"/\"\n  content_path: {}\n  logging: false\n", q(&y.content_path));
    o += "  https:\n    disabled: true\n";
    o += &format!("  authentication:\n    disabled: false\n    username: \"encore\"\n    password: {}\n", q(&y.api_key));
    o += &format!("    api_keys:\n      encore:\n        key: {}\n        role: Administrator\n        cidr: \"127.0.0.1/32,::1/128\"\n", q(&y.api_key));
    o
}

/// The configured Soulseek server: SOULSEEK_SERVER ("host" or "host:port") or slskd's default.
fn server() -> (String, u16) {
    let Some(v) = std::env::var("SOULSEEK_SERVER").ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) else {
        return (SERVER.0.into(), SERVER.1);
    };
    match v.rsplit_once(':').and_then(|(h, p)| p.parse::<u16>().ok().map(|p| (h.to_string(), p))) {
        Some(x) => x,
        None => (v, SERVER.1),
    }
}

fn yml_from_settings(st: &impl Ctx) -> Yml {
    let downloads = st.cfg().str("slskd_download_path");
    let library = st.cfg().str("library_path");
    let share = st.cfg().bool("soulseek_share") && !library.trim().is_empty() && library.trim() != "/";
    Yml {
        username: st.cfg().str("soulseek_username").trim().to_string(),
        password: st.cfg().str("soulseek_password"),
        server: server(),
        listen_ip: "0.0.0.0".into(),
        listen_port: configured_port(st),
        incomplete: Path::new(&downloads).join(".incomplete").to_string_lossy().to_string(),
        downloads,
        shares: if share { vec![library.trim().to_string()] } else { vec![] },
        upload_slots: st.cfg().int("soulseek_upload_slots").clamp(1, 20),
        content_path: NO_WEB_UI.into(),
        api_key: api_key(st),
        proxy: None,
    }
}

fn configured_port(st: &impl Ctx) -> u16 {
    st.cfg().int("soulseek_port").clamp(1, 65535) as u16
}

// ---- routing --------------------------------------------------------------------------------------

/// How slskd reaches the network right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    /// The normal connection.
    Direct,
    /// Only through the VPN, listening on its forwarded port when it has one.
    Vpn(Option<u16>),
    /// The VPN is on but not connected and the kill switch is set: slskd stays stopped.
    Wait,
}

/// `vpn_on` is the setting; `usable` means the config parses; `active` a recent handshake.
pub fn route_for(vpn_on: bool, usable: bool, active: bool, kill_switch: bool, forwarded: Option<u16>) -> Route {
    if !vpn_on {
        Route::Direct
    } else if usable && active {
        Route::Vpn(forwarded.filter(|p| *p > 0))
    } else if kill_switch {
        Route::Wait
    } else {
        Route::Direct
    }
}

fn route(st: &impl Ctx) -> Route {
    let on = st.cfg().bool("vpn_enabled");
    if !on {
        return Route::Direct;
    }
    let usable = encore_vpn::enabled(st.cfg());
    route_for(on, usable, usable && st.vpn().active(), st.cfg().bool("vpn_kill_switch"), st.vpn().forwarded_port())
}

// ---- the process ----------------------------------------------------------------------------------

/// What should run.
#[derive(Clone, Debug)]
pub struct Plan {
    pub bin: PathBuf,
    pub app_dir: PathBuf,
    pub version: String,
    pub yml: String,
    /// folders slskd needs to exist before it starts
    pub dirs: Vec<PathBuf>,
    pub listen_port: u16,
    pub vpn: bool,
    /// where downloads land (the fallback when the configured folder can't be written)
    pub downloads: PathBuf,
    /// shown beside the state while slskd runs: the configured folder can't be written, so the fallback is used
    pub warning: Option<String>,
}

impl Plan {
    fn key(&self) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (&self.bin, &self.app_dir, &self.version, &self.yml).hash(&mut h);
        h.finish()
    }
}

#[derive(Debug)]
pub enum Want {
    Off,
    NotInstalled,
    WaitingVpn,
    /// can't start right now (shown as the error, retried at the next check)
    Blocked(String),
    Run(Plan),
}

/// encore-run.log: slskd's console output, rotated at 5 MB; the last lines are kept for errors.
struct RunLog {
    path: PathBuf,
    tail: Mutex<VecDeque<String>>,
}

impl RunLog {
    fn line(&self, l: &str) {
        {
            let mut t = self.tail.lock().unwrap();
            if t.len() >= 20 {
                t.pop_front();
            }
            t.push_back(l.to_string());
        }
        if std::fs::metadata(&self.path).map(|m| m.len() > LOG_MAX).unwrap_or(false) {
            let _ = std::fs::rename(&self.path, self.path.with_extension("log.1"));
        }
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&self.path) {
            let _ = writeln!(f, "{l}");
        }
    }

    /// The last line that reads like a problem, else the last line.
    fn last_error(&self) -> Option<String> {
        let t = self.tail.lock().unwrap();
        t.iter().rev().find(|l| l.contains("ERR") || l.contains("FTL") || l.to_lowercase().contains("error") || l.contains("Exception")).or(t.back())
            .map(|l| l.chars().take(300).collect())
    }
}

fn pipe<R: AsyncRead + Unpin + Send + 'static>(r: R, log: Arc<RunLog>, managed: Managed) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(r).lines();
        while let Ok(Some(l)) = lines.next_line().await {
            log.line(&l);
            managed.on_line(&l);
        }
    });
}

/// "Scanned 11% of shared directories. Found 7115 files so far." → 11
fn scan_percent(l: &str) -> Option<u8> {
    let rest = &l[l.find("Scanned ")? + 8..];
    let (n, tail) = rest.split_once('%')?;
    if !tail.trim_start().starts_with("of shared") {
        return None;
    }
    n.trim().parse::<u8>().ok()
}

fn signal(pid: u32, sig: &str) {
    let _ = std::process::Command::new("kill").arg(format!("-{sig}")).arg(pid.to_string()).stdout(Stdio::null()).stderr(Stdio::null()).status();
}

/// Starts, watches, restarts and stops one slskd process.
pub struct Runner {
    child: Option<Child>,
    key: u64,
    /// the key of the plan last started (or that failed to start): a different one is new settings,
    /// tried at once instead of after the old plan's backoff
    tried: u64,
    started: Instant,
    backoff: Duration,
    next_start: Option<Instant>,
    log: Option<Arc<RunLog>>,
    /// where the state it reaches is kept
    managed: Managed,
}

const BACKOFF_MIN: Duration = Duration::from_secs(5);
const BACKOFF_MAX: Duration = Duration::from_secs(300);

impl Runner {
    pub fn new(managed: Managed) -> Runner {
        Runner { child: None, key: 0, tried: 0, started: Instant::now(), backoff: BACKOFF_MIN, next_start: None, log: None, managed }
    }

    /// Stop gently (SIGTERM, then SIGKILL after 10 s).
    async fn stop(&mut self) {
        if let Some(mut c) = self.child.take() {
            if let Some(pid) = c.id() {
                signal(pid, "TERM");
            }
            if tokio::time::timeout(Duration::from_secs(10), c.wait()).await.is_err() {
                let _ = c.kill().await;
            }
            self.managed.0.session.wake();
        }
        self.key = 0;
    }

    /// Bring the process in line with `want` and update the status.
    pub async fn apply(&mut self, want: Want) {
        let start_now = self.managed.0.start_now.swap(false, Ordering::SeqCst);
        let plan = match want {
            Want::Off | Want::NotInstalled | Want::WaitingVpn | Want::Blocked(_) => {
                self.stop().await;
                self.next_start = None;
                self.backoff = BACKOFF_MIN;
                self.managed.set_status(|s| {
                    *s = Status { state: "off", ..Default::default() };
                    match want {
                        Want::NotInstalled => { s.state = "not_installed"; s.error = Some(NOT_INSTALLED.into()); }
                        Want::WaitingVpn => { s.state = "waiting_vpn"; s.error = Some(WAITING_FOR_VPN.into()); s.vpn = true; }
                        Want::Blocked(e) => { s.state = "blocked"; s.error = Some(e); }
                        _ => {}
                    }
                });
                return;
            }
            Want::Run(p) => p,
        };
        let key = plan.key();
        if let Some(c) = self.child.as_mut() {
            match c.try_wait() {
                Ok(None) if key == self.key => {
                    self.managed.set_status(|s| {
                        if s.state != "running" {
                            s.state = "starting";
                        }
                    });
                    return;
                }
                Ok(None) => {
                    tracing::info!("soulseek: restarting slskd for changed settings");
                    self.stop().await;
                    self.next_start = None;
                }
                res => {
                    let exit = match res { Ok(Some(e)) => e.to_string(), Err(e) => e.to_string(), Ok(None) => String::new() };
                    self.child = None;
                    self.key = 0;
                    if self.started.elapsed() > Duration::from_secs(120) {
                        self.backoff = BACKOFF_MIN; // it ran for a while: a fresh failure, not a crash loop
                    }
                    self.next_start = Some(Instant::now() + self.backoff);
                    let why = self.log.as_ref().and_then(|l| l.last_error()).map(|l| format!(" – {l}")).unwrap_or_default();
                    let msg = format!("slskd stopped ({exit}){why}; starting it again in {} s", self.backoff.as_secs());
                    tracing::warn!("soulseek: {msg}");
                    self.backoff = (self.backoff * 2).min(BACKOFF_MAX);
                    self.managed.set_status(|s| {
                        s.state = "crashed";
                        s.pid = None;
                        s.error = Some(msg);
                    });
                    if key == self.tried && !start_now {
                        return;
                    }
                    // the settings changed while it ran (or Start now was pressed): no waiting
                }
            }
        }
        if key != self.tried || start_now {
            // new settings (or Start now) are a fresh attempt: the old plan's backoff doesn't apply
            self.next_start = None;
            self.backoff = BACKOFF_MIN;
        }
        self.tried = key;
        if self.next_start.is_some_and(|t| Instant::now() < t) {
            return; // crashed: wait out the backoff
        }
        if let Err(e) = self.start(&plan) {
            let msg = format!("slskd didn't start: {e}");
            tracing::warn!("soulseek: {msg}");
            self.next_start = Some(Instant::now() + self.backoff);
            self.backoff = (self.backoff * 2).min(BACKOFF_MAX);
            self.managed.set_status(|s| {
                s.state = "crashed";
                s.pid = None;
                s.version = Some(plan.version.clone());
                s.error = Some(msg);
            });
            return;
        }
        self.key = key;
        self.next_start = None;
        let pid = self.child.as_ref().and_then(|c| c.id());
        self.managed.set_status(|s| {
            *s = Status { state: "starting", version: Some(plan.version.clone()), pid, error: None, listen_port: Some(plan.listen_port), vpn: plan.vpn, scan: None,
                warning: plan.warning.clone(), downloads: Some(plan.downloads.to_string_lossy().to_string()), server_error: None };
        });
        self.managed.0.log_tail.lock().unwrap().clear();
    }

    fn start(&mut self, plan: &Plan) -> anyhow::Result<()> {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(&plan.app_dir)?;
        for d in &plan.dirs {
            std::fs::create_dir_all(d).map_err(|e| anyhow::anyhow!("can't create {}: {e}", d.display()))?;
        }
        let yml = plan.app_dir.join("slskd.yml");
        std::fs::write(&yml, &plan.yml)?;
        let _ = std::fs::set_permissions(&yml, std::fs::Permissions::from_mode(0o600)); // the Soulseek password is in it
        let log = Arc::new(RunLog { path: plan.app_dir.join("encore-run.log"), tail: Mutex::new(VecDeque::new()) });
        log.line(&format!("---- Encore starts slskd {} ----", plan.version));
        let mut cmd = Command::new(&plan.bin);
        cmd.arg("--app-dir").arg(&plan.app_dir).current_dir(&plan.app_dir)
            .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
        child_env(&mut cmd);
        let mut child = cmd.spawn()?;
        if let Some(o) = child.stdout.take() {
            pipe(o, log.clone(), self.managed.clone());
        }
        if let Some(e) = child.stderr.take() {
            pipe(e, log.clone(), self.managed.clone());
        }
        if let Some(pid) = child.id() {
            let _ = std::fs::write(plan.app_dir.join("encore.pid"), pid.to_string());
        }
        tracing::info!("soulseek: started slskd {} (pid {:?}, port {}{})", plan.version, child.id(), plan.listen_port, if plan.vpn { ", through the VPN" } else { "" });
        self.managed.0.generation.fetch_add(1, Ordering::SeqCst);
        self.managed.0.session.wake();
        self.child = Some(child);
        self.started = Instant::now();
        self.log = Some(log);
        Ok(())
    }
}

/// An slskd left running by an Encore that was killed (its pid file names it): stop it, or the new
/// one can't take the port.
fn kill_stale(app: &Path) {
    let Some(pid) = std::fs::read_to_string(app.join("encore.pid")).ok().and_then(|s| s.trim().parse::<u32>().ok()) else { return };
    let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
    let cmdline = String::from_utf8_lossy(&cmdline);
    if cmdline.contains("slskd") && cmdline.contains(&*app.to_string_lossy()) {
        tracing::info!("soulseek: stopping the slskd (pid {pid}) a previous Encore left running");
        signal(pid, "KILL");
    }
    let _ = std::fs::remove_file(app.join("encore.pid"));
}

/// What the settings ask for now (starting the VPN plumbing it needs).
async fn want(st: &impl Ctx) -> Want {
    if !configured(st) {
        st.vpn().relay(None);
        return Want::Off;
    }
    let Some((version, bin)) = installed_in(st.data_dir()) else {
        st.vpn().relay(None);
        return Want::NotInstalled;
    };
    let mut y = yml_from_settings(st);
    // the downloads folder first: a folder Encore can't write is a configuration problem to show, not a
    // crash loop to run
    let configured = PathBuf::from(&y.downloads);
    let (downloads, warning) = match st.requests().slskd.probed_downloads_dir(st.cfg(), st.data_dir(), &configured) {
        Ok(x) => x,
        Err(e) => {
            st.vpn().relay(None);
            return Want::Blocked(e);
        }
    };
    if downloads != configured {
        y.downloads = downloads.to_string_lossy().to_string();
        y.incomplete = downloads.join(".incomplete").to_string_lossy().to_string();
    }
    let r = route(st);
    match r {
        Route::Wait => {
            st.vpn().relay(None);
            return Want::WaitingVpn;
        }
        Route::Direct => {
            st.vpn().relay(None);
            st.vpn().set_door(None);
            // slskd reaches the server through Encore's relay, which keeps the door (see Relay)
            match st.requests().slskd.relay_port(y.server.clone()).await {
                Ok(port) => y.server = ("127.0.0.1".into(), port),
                Err(e) => return Want::Blocked(format!("Couldn't start Encore's relay to the Soulseek server: {e}")),
            }
        }
        Route::Vpn(fwd) => {
            let proxy = match st.vpn().socks().await {
                Ok(p) => p,
                Err(e) => return Want::Blocked(format!("Couldn't start the VPN proxy for Soulseek: {e}")),
            };
            // slskd would look the server's name up itself, outside the VPN: hand it the address
            // (a failed lookup later on keeps the last answer: a DNS hiccup must not stop a running slskd)
            let last_ip = &st.requests().slskd.0.server_ip;
            let (host, port) = y.server.clone();
            let last = last_ip.lock().unwrap().clone().filter(|(h, _)| *h == host).map(|(_, ip)| ip);
            let ip = match (st.vpn().resolve(&host).await.map(|ips| ips.into_iter().find(|i| i.is_ipv4())), last) {
                (Ok(Some(ip)), _) => {
                    *last_ip.lock().unwrap() = Some((host.clone(), ip));
                    ip
                }
                (_, Some(ip)) => ip,
                (Ok(None), None) => return Want::Blocked(format!("{host} has no IPv4 address (looked up through the VPN)")),
                (Err(e), None) => return Want::Blocked(format!("Couldn't look up the Soulseek server {host} through the VPN: {e}")),
            };
            y.server = (ip.to_string(), port);
            // the proxy lets slskd reach the server only through the session's door (see Relay)
            if let std::net::IpAddr::V4(v4) = ip {
                let session = st.requests().slskd.session().clone();
                let door: encore_vpn::socks::Door = Arc::new(move || session.door().map(|p| Box::new(p) as encore_vpn::socks::Pass));
                st.vpn().set_door(Some((std::net::SocketAddrV4::new(v4, port), door)));
            }
            y.proxy = Some(proxy);
            // peers reach slskd only through the tunnel: the forwarded port is relayed to loopback
            y.listen_ip = "127.0.0.1".into();
            if let Some(p) = fwd {
                y.listen_port = p;
            }
            st.vpn().relay(fwd);
        }
    }
    let app = app_dir(st.data_dir());
    Want::Run(Plan {
        dirs: vec![PathBuf::from(&y.downloads), PathBuf::from(&y.incomplete), bin.parent().unwrap_or(Path::new("/")).join(&y.content_path)],
        listen_port: y.listen_port,
        vpn: matches!(r, Route::Vpn(_)),
        yml: render_yml(&y),
        bin,
        app_dir: app,
        version,
        downloads,
        warning,
    })
}

/// slskd's API answers (any answer: it is up).
async fn answers(st: &impl Ctx) -> bool {
    matches!(api(st, reqwest::Method::GET, "application", None, &[], Some(Duration::from_secs(10))).await, Ok((s, _)) if s > 0)
}

/// Stops slskd gently before the server restarts (SIGTERM, then SIGKILL after 10 s).
pub async fn stop(st: &impl Ctx) {
    if let Some(pid) = st.requests().slskd.status()["pid"].as_u64() {
        encore_system::update::stop_pid(pid as i32, Duration::from_secs(10)).await;
    }
}

/// Keeps slskd in step with the settings: installs nothing by itself unless tools_auto_slskd is on.
pub async fn supervisor(st: impl Ctx) {
    kill_stale(&app_dir(st.data_dir()));
    let managed = st.requests().slskd.clone();
    let mut run = Runner::new(managed.clone());
    let mut auto_tried: Option<Instant> = None;
    loop {
        let w = want(&st).await;
        if matches!(w, Want::NotInstalled) && st.cfg().bool("tools_auto_slskd") && !managed.0.installing.load(Ordering::SeqCst)
            && auto_tried.map(|t| t.elapsed() > Duration::from_secs(3600)).unwrap_or(true) {
            auto_tried = Some(Instant::now());
            let st2 = st.clone();
            tokio::spawn(async move {
                let res = async { let (v, u) = latest(&st2).await?; install(&st2, &v, &u).await }.await;
                if let Err(e) = res {
                    tracing::warn!("soulseek: automatic slskd install failed: {e}");
                }
            });
        }
        run.apply(w).await;
        if managed.state() == "starting" && run.child.is_some() && answers(&st).await {
            managed.set_status(|s| {
                s.state = "running";
                s.error = None;
            });
            managed.now_running();
        }
        let wait = if st.cfg().bool("vpn_enabled") || managed.state() == "starting" { 3 } else { 10 };
        tokio::select! {
            _ = managed.0.wake.notified() => {}
            _ = tokio::time::sleep(Duration::from_secs(wait)) => {}
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    /// slskd's own lines become words a person can act on; a sign-in clears them.
    #[test]
    fn sign_in_problems_are_said_in_plain_words() {
        let hint = |l: &str| server_hint(l).flatten();
        assert!(hint("[17:02:11 ERR] Login failed: The server rejected login attempt: INVALIDPASS").unwrap().contains("wrong password"));
        assert!(hint("[17:02:11 ERR] The server rejected login attempt: INVALIDUSERNAME").unwrap().contains("username"));
        assert!(hint("[17:02:11 WRN] Disconnected from the Soulseek server: Kicked from server by another client").unwrap().contains("another Soulseek client"));
        assert!(hint("[17:02:11 WRN] Soulseek client disconnected: Connection to the Soulseek server timed out").unwrap().contains("can't reach the Soulseek server"));
        assert!(hint("[17:02:11 WRN] Disconnected from the Soulseek server: the network went away").unwrap().starts_with("disconnected from the Soulseek server"));
        assert_eq!(server_hint("[17:02:12 INF] Logged in to the Soulseek server as krifmaz"), Some(None));
        assert_eq!(server_hint("[17:02:12 INF] Scanned 11% of shared directories. Found 7115 files so far."), None);
        // the status follows the lines, and the last 20 lines are kept for the card
        let m = Managed::default();
        m.set_status(|s| *s = Status { state: "running", ..Default::default() });
        m.on_line("[17:02:11 ERR] Login failed: The server rejected login attempt: INVALIDPASS");
        assert!(m.status()["server_error"].as_str().unwrap().contains("wrong password"));
        m.on_line("[17:02:12 INF] Logged in to the Soulseek server as krifmaz");
        assert!(m.status()["server_error"].is_null());
        assert!(m.log_tail().iter().any(|l| l.contains("INVALIDPASS")));
    }

    use std::os::unix::fs::PermissionsExt;

    use encore_core::tmp::dir as tmp;

    fn release() -> Value {
        let names = ["slskd-0.26.1-linux-arm.zip", "slskd-0.26.1-linux-musl-x64.zip", "slskd-0.26.1-linux-musl-arm64.zip", "slskd-0.26.1-osx-arm64.zip",
                     "slskd-0.26.1-win-x64.zip", "slskd-0.26.1-linux-arm64.zip", "slskd-0.26.1-linux-x64.zip"];
        json!({ "tag_name": "0.26.1", "assets": names.iter().map(|n| json!({ "name": n, "browser_download_url": format!("https://github.com/slskd/slskd/releases/download/0.26.1/{n}") })).collect::<Vec<_>>() })
    }

    #[test]
    fn picks_the_glibc_linux_zip_for_the_arch() {
        let (v, u) = pick_asset(&release(), "x86_64").unwrap();
        assert_eq!(v, "0.26.1");
        assert!(u.ends_with("/slskd-0.26.1-linux-x64.zip"), "{u}");
        let (_, u) = pick_asset(&release(), "aarch64").unwrap();
        assert!(u.ends_with("/slskd-0.26.1-linux-arm64.zip"), "{u}");
        assert_eq!(pick_asset(&release(), "riscv64"), None);
        let mut only_musl = release();
        only_musl["assets"] = json!([{ "name": "slskd-0.26.1-linux-musl-x64.zip", "browser_download_url": "x" }]);
        assert_eq!(pick_asset(&only_musl, "x86_64"), None, "never the musl build");
        let mut v = release();
        v["tag_name"] = json!("v0.27.0");
        assert_eq!(pick_asset(&v, "x86_64").unwrap().0, "0.27.0");
    }

    fn yml(proxy: bool, shares: bool) -> Yml {
        Yml {
            username: "me".into(), password: "pa\"ss".into(), server: ("vps.slsknet.org".into(), 2271), listen_ip: "0.0.0.0".into(), listen_port: 2234,
            downloads: "/downloads/slskd".into(), incomplete: "/downloads/slskd/.incomplete".into(),
            shares: if shares { vec!["/music".into()] } else { vec![] }, upload_slots: 2, content_path: "no-web-ui".into(),
            api_key: "k".repeat(48), proxy: proxy.then(|| (40000, "u".into(), "p".into())),
        }
    }

    #[test]
    fn writes_slskd_yml() {
        let y = render_yml(&yml(false, true));
        for want in ["headless: true", "  no_version_check: true", "  no_logo: true", "  downloads: \"/downloads/slskd\"", "  incomplete: \"/downloads/slskd/.incomplete\"",
                     "  directories:\n    - \"/music\"", "  upload:\n    slots: 2", "  username: \"me\"", "  password: \"pa\\\"ss\"", "  listen_ip_address: \"0.0.0.0\"",
                     "  listen_port: 2234", "  address: \"vps.slsknet.org\"\n  port: 2271", "web:\n  port: 5031\n  ip_address: \"127.0.0.1\"",
                     "  content_path: \"no-web-ui\"", "  https:\n    disabled: true", "role: Administrator", "cidr: \"127.0.0.1/32,::1/128\""] {
            assert!(y.contains(want), "missing {want:?} in\n{y}");
        }
        assert!(y.contains(&format!("key: \"{}\"", "k".repeat(48))));
        assert!(!y.contains("proxy"), "no proxy without the VPN");
        // Encore connects on its own schedule (slskd_session), with 30 s to reach the server; the other
        // timeouts stay slskd's
        assert!(y.contains("flags:\n  no_logo: true\n  no_version_check: true\n  no_connect: true\n"), "{y}");
        assert!(y.contains("  description: \"Encore\"\n  connection:\n    timeout:\n      connect: 30000\n"), "{y}");
        assert!(!y.contains("inactivity") && !y.contains("transfer:"), "{y}");
        let y = render_yml(&yml(false, false));
        assert!(y.contains("shares:\n  directories: []"), "{y}");
        let mut v = yml(true, true);
        v.listen_ip = "127.0.0.1".into();
        v.listen_port = 51234;
        v.server = ("208.76.170.59".into(), 2271);
        let y = render_yml(&v);
        assert!(y.contains("  connection:\n    timeout:\n      connect: 30000\n    proxy:\n      enabled: true\n      address: \"127.0.0.1\"\n      port: 40000\n      username: \"u\"\n      password: \"p\""), "{y}");
        assert_eq!(y.matches("connection:").count(), 1, "one connection block: {y}");
        assert!(y.contains("  listen_ip_address: \"127.0.0.1\"\n  listen_port: 51234"), "{y}");
        assert!(y.contains("  address: \"208.76.170.59\""), "the server by address, so slskd does no DNS lookup of its own");
    }

    #[test]
    fn routes() {
        assert_eq!(route_for(false, false, false, true, None), Route::Direct);
        assert_eq!(route_for(true, true, true, true, Some(51234)), Route::Vpn(Some(51234)));
        assert_eq!(route_for(true, true, true, false, None), Route::Vpn(None));
        assert_eq!(route_for(true, true, false, true, Some(1)), Route::Wait, "kill switch: off while the tunnel is down");
        assert_eq!(route_for(true, false, false, true, None), Route::Wait, "a broken config is not a way around the kill switch");
        assert_eq!(route_for(true, true, false, false, None), Route::Direct);
    }

    fn script(dir: &Path, body: &str) -> PathBuf {
        let p = dir.join("slskd");
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    #[tokio::test]
    async fn unpacks_verifies_and_switches_versions() {
        let root = tmp("install");
        let zip_path = root.join("a.zip");
        {
            let mut z = zip::ZipWriter::new(std::fs::File::create(&zip_path).unwrap());
            let o = zip::write::SimpleFileOptions::default().unix_permissions(0o644);
            z.start_file("slskd", o).unwrap();
            z.write_all(b"#!/bin/sh\necho '0.26.1+abc'\n").unwrap();
            z.add_directory("wwwroot/", o).unwrap();
            z.start_file("wwwroot/index.html", o).unwrap();
            z.write_all(b"<html>").unwrap();
            z.start_file("../evil", o).unwrap();
            z.write_all(b"x").unwrap();
            z.finish().unwrap();
        }
        std::fs::create_dir_all(root.join("0.25.0")).unwrap();
        let bin = unpack(&root, "0.26.1", &zip_path).unwrap();
        assert!(!root.join(".0.26.1.new").join("wwwroot").exists(), "headless: no web UI files");
        assert!(root.join(".0.26.1.new").join(NO_WEB_UI).is_dir(), "the empty content folder slskd.yml names");
        assert!(!root.join("evil").exists() && !root.parent().unwrap().join("evil").exists());
        assert_eq!(std::fs::metadata(&bin).unwrap().permissions().mode() & 0o777, 0o755);
        assert_eq!(verify(&bin).await.unwrap(), "0.26.1");
        switch_to(&root, "0.26.1").unwrap();
        assert_eq!(std::fs::read_to_string(root.join("current")).unwrap().trim(), "0.26.1");
        assert!(!root.join("0.25.0").exists(), "older versions are deleted");
        let data = root.parent().unwrap().to_path_buf();
        let _ = data; // installed_in reads <data>/tools/slskd; the layout itself:
        assert!(root.join("0.26.1").join("slskd").is_file());

        let bad = script(&root, "exit 4");
        assert!(verify(&bad).await.is_err(), "a binary that doesn't run is refused");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn installed_reads_current() {
        let data = tmp("current");
        assert_eq!(installed_in(&data), None);
        let dir = tools_dir(&data).join("0.26.1");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(tools_dir(&data).join("current"), "0.26.1\n").unwrap();
        assert_eq!(installed_in(&data), None, "no binary yet");
        script(&dir, "true");
        assert_eq!(installed_in(&data).unwrap().0, "0.26.1");
        std::fs::write(tools_dir(&data).join("current"), "../x\n").unwrap();
        assert_eq!(installed_in(&data), None);
        let _ = std::fs::remove_dir_all(&data);
    }

    fn plan(dir: &Path, bin: PathBuf, yml: &str) -> Plan {
        Plan { bin, app_dir: dir.join("app"), version: "0.26.1".into(), yml: yml.into(), dirs: vec![dir.join("dl")], listen_port: 2234, vpn: false, downloads: dir.join("dl"), warning: None }
    }

    fn state(r: &Runner) -> (String, Option<u32>, Option<String>) {
        let s = r.managed.status();
        (s["state"].as_str().unwrap().to_string(), s["pid"].as_u64().map(|p| p as u32), s["error"].as_str().map(String::from))
    }

    /// A fake slskd that keeps running: started once, left alone while nothing changes, restarted
    /// for new settings, stopped when Soulseek is switched off; and the other states.
    #[tokio::test]
    async fn supervisor_states_with_a_running_fake() {
        let dir = tmp("run");
        let bin = script(&dir, "echo started \"$@\"; exec sleep 30");
        let mut r = Runner::new(Managed::default());
        r.apply(plan(&dir, bin.clone(), "a: 1\n").into()).await;
        let (s, pid, _) = state(&r);
        assert_eq!(s, "starting");
        assert!(pid.is_some());
        assert_eq!(std::fs::read_to_string(dir.join("app").join("slskd.yml")).unwrap(), "a: 1\n");
        assert!(dir.join("dl").is_dir(), "folders slskd needs are made first");
        assert_eq!(std::fs::metadata(dir.join("app").join("slskd.yml")).unwrap().permissions().mode() & 0o777, 0o600);
        r.apply(plan(&dir, bin.clone(), "a: 1\n").into()).await;
        assert_eq!(state(&r).1, pid, "nothing changed: same process");
        r.apply(plan(&dir, bin.clone(), "a: 2\n").into()).await;
        let (s, pid2, _) = state(&r);
        assert_eq!(s, "starting");
        assert!(pid2.is_some() && pid2 != pid, "new settings: restarted");
        assert_eq!(std::fs::read_to_string(dir.join("app").join("slskd.yml")).unwrap(), "a: 2\n");
        tokio::time::sleep(Duration::from_millis(300)).await;
        let log = std::fs::read_to_string(dir.join("app").join("encore-run.log")).unwrap();
        assert!(log.contains(&format!("started --app-dir {}", dir.join("app").display())), "{log}");
        r.apply(Want::Off).await;
        assert_eq!(state(&r), ("off".into(), None, None));
        assert!(r.child.is_none());
        r.apply(Want::NotInstalled).await;
        assert_eq!(state(&r).0, "not_installed");
        r.apply(Want::WaitingVpn).await;
        assert_eq!(state(&r), ("waiting_vpn".into(), None, Some(WAITING_FOR_VPN.into())));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A fake slskd that exits at once: crashed with its last words, restarted only after the backoff.
    #[tokio::test]
    async fn supervisor_restarts_a_crashing_fake_with_backoff() {
        let dir = tmp("crash");
        let bin = script(&dir, "echo '[ERR] Invalid configuration: soulseek.username'; exit 3");
        let mut r = Runner::new(Managed::default());
        r.apply(plan(&dir, bin.clone(), "x\n").into()).await;
        assert_eq!(state(&r).0, "starting");
        tokio::time::sleep(Duration::from_millis(400)).await;
        r.apply(plan(&dir, bin.clone(), "x\n").into()).await;
        let (s, pid, err) = state(&r);
        assert_eq!(s, "crashed");
        assert_eq!(pid, None);
        let err = err.unwrap();
        assert!(err.contains("exit status: 3") && err.contains("Invalid configuration") && err.contains("again in 5 s"), "{err}");
        r.apply(plan(&dir, bin.clone(), "x\n").into()).await;
        assert_eq!(state(&r).0, "crashed", "waits out the backoff");
        assert!(r.child.is_none());
        r.next_start = Some(Instant::now()); // the backoff is over
        r.apply(plan(&dir, bin.clone(), "x\n").into()).await;
        assert_eq!(state(&r).0, "starting", "started again");
        tokio::time::sleep(Duration::from_millis(400)).await;
        r.apply(plan(&dir, bin.clone(), "x\n").into()).await;
        assert!(state(&r).2.unwrap().contains("again in 10 s"), "the backoff doubles");
        assert!(r.next_start.is_some(), "waiting 10 s");

        // a changed plan starts at once: new settings are a fresh attempt, not the old crash loop
        r.apply(plan(&dir, bin.clone(), "y\n").into()).await;
        assert_eq!(state(&r).0, "starting", "changed settings: started at once, no backoff");
        assert_eq!(std::fs::read_to_string(dir.join("app").join("slskd.yml")).unwrap(), "y\n");
        tokio::time::sleep(Duration::from_millis(400)).await;
        r.apply(plan(&dir, bin.clone(), "y\n").into()).await;
        let (s, _, err) = state(&r);
        assert_eq!(s, "crashed");
        assert!(err.unwrap().contains("again in 5 s"), "the backoff starts over for the new plan");
        r.apply(plan(&dir, bin.clone(), "y\n").into()).await;
        assert_eq!(state(&r).0, "crashed", "the same plan waits out its backoff");

        // Start now: the button on the Soulseek card forgets the backoff
        r.managed.start_now();
        r.apply(plan(&dir, bin, "y\n").into()).await;
        assert_eq!(state(&r).0, "starting", "Start now: started at once");
        r.apply(Want::Off).await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A folder Encore can't write: a path the user chose blocks the start with the exact chown line,
    /// the default path falls back to <config>/downloads/slskd with a warning; a blocked start is tried
    /// again at once when the folder is fixed.
    #[tokio::test]
    async fn unwritable_downloads_folder_blocks_with_the_chown_line() {
        let dir = tmp("unwritable");
        let (uid, gid) = ids();
        // root may write anywhere: for it, a file stands where the folder should be
        let bad = if uid == 0 {
            std::fs::write(dir.join("file"), "x").unwrap();
            dir.join("file").join("slskd")
        } else {
            let d = dir.join("ro");
            std::fs::create_dir_all(&d).unwrap();
            std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o500)).unwrap();
            d.join("slskd")
        };
        let fallback = dir.join("config").join("downloads").join("slskd");
        let chown = format!("chown {uid}:{gid} {}", bad.display());

        let good = dir.join("dl");
        assert_eq!(downloads_dir_for(&good, true, &fallback).unwrap(), (good.clone(), None));
        assert!(good.is_dir(), "a writable folder is made and used");
        assert!(!fallback.exists(), "no fallback while the configured folder works");

        let e = downloads_dir_for(&bad, true, &fallback).unwrap_err();
        assert!(e.contains(&chown), "the exact chown line\n{e}");
        assert!(e.contains("or set PUID/PGID"), "{e}");
        assert!(e.contains(&format!("isn't writable by Encore (uid {uid}, gid {gid})")), "{e}");
        assert!(e.starts_with("Soulseek's downloads folder"), "{e}");
        assert!(!fallback.exists(), "a chosen folder is never replaced");

        let (d, warn) = downloads_dir_for(&bad, false, &fallback).unwrap();
        assert_eq!(d, fallback, "the default folder falls back to <config>/downloads/slskd");
        assert!(fallback.is_dir());
        let warn = warn.unwrap();
        assert!(warn.starts_with(&format!("Downloads go to {} for now", fallback.display())), "{warn}");
        assert!(warn.contains(&chown) && warn.contains("or set PUID/PGID"), "{warn}");

        // not even the fallback: blocked, and both folders are named
        let bad_fallback = bad.join("fallback");
        let e = downloads_dir_for(&bad, false, &bad_fallback).unwrap_err();
        assert!(e.contains(&chown) && e.contains("isn't writable either"), "{e}");

        // the Runner shows Blocked as its own state, with the message, and starts at once when the plan arrives
        let mut r = Runner {
            backoff: Duration::from_secs(80), // left over from an earlier crash loop
            next_start: Some(Instant::now() + Duration::from_secs(80)),
            ..Runner::new(Managed::default())
        };
        r.apply(Want::Blocked(e.clone())).await;
        let (s, pid, err) = state(&r);
        assert_eq!((s.as_str(), pid), ("blocked", None));
        assert_eq!(err.as_deref(), Some(e.as_str()));
        assert!(r.next_start.is_none() && r.backoff == BACKOFF_MIN, "a blocked start is retried every check, without backoff");
        let bin = script(&dir, "exec sleep 30");
        let mut p = plan(&dir, bin, "a: 1\n");
        p.warning = Some(warn.clone());
        p.downloads = fallback.clone();
        r.apply(p.into()).await;
        assert_eq!(state(&r).0, "starting", "the folder was fixed: started at once");
        let st = r.managed.status();
        assert_eq!(st["warning"].as_str(), Some(warn.as_str()), "the fallback warning rides along with the status");
        assert_eq!(st["download_dir"].as_str(), Some(fallback.to_string_lossy().as_ref()));
        r.apply(Want::Off).await;
        assert!(r.managed.status()["warning"].is_null());
        if uid != 0 {
            let _ = std::fs::set_permissions(dir.join("ro"), std::fs::Permissions::from_mode(0o700));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The files of a finished download are looked for where slskd put them – the fallback folder while
    /// that is in use, not the setting – and the folder is probed only when it matters: the last answer is
    /// reused while slskd runs with the same folder settings, probed again after poke() and before a start.
    #[tokio::test]
    async fn finished_files_are_found_in_the_fallback_folder_and_the_probe_is_reused_while_running() {
        let dir = tmp("fallback-files");
        let data = dir.join("config");
        let cfg = encore_config::Config::load(encore_db::Db::open(&data).unwrap()).unwrap();
        let m = Managed::default();
        let configured = dir.join("configured");
        std::fs::create_dir_all(&configured).unwrap();
        cfg.set_many(&json!({ "slskd_download_path": configured.to_string_lossy() }));
        let fallback = fallback_dir(&data);
        std::fs::create_dir_all(fallback.join("Album")).unwrap();
        let file = fallback.join("Album").join("01 - Song.flac");
        std::fs::write(&file, b"flac").unwrap();
        let remote = "@@abcd\\Music\\Album\\01 - Song.flac";
        let local = |m: &Managed| crate::sources::slskd::local_path_in(&m.download_dir(&cfg), remote);
        assert_eq!(m.download_dir(&cfg), configured, "no plan: the setting");
        assert_eq!(local(&m), None, "nothing under the configured folder");
        m.set_status(|s| s.downloads = Some(fallback.to_string_lossy().to_string()));
        assert_eq!(m.download_dir(&cfg), fallback);
        assert_eq!(local(&m), Some(file.clone()), "found where slskd put it");
        m.set_status(|s| s.downloads = None);

        // the probe: reused while slskd runs, again after poke() and before a start
        m.set_status(|s| s.state = "running");
        assert_eq!(m.probed_downloads_dir(&cfg, &data, &configured), Ok((configured.clone(), None)));
        std::fs::remove_dir_all(&configured).unwrap();
        std::fs::write(&configured, "x").unwrap(); // a file where the folder was: nobody can write there now
        assert_eq!(m.probed_downloads_dir(&cfg, &data, &configured), Ok((configured.clone(), None)), "the last answer while slskd runs");
        m.poke();
        assert!(m.probed_downloads_dir(&cfg, &data, &configured).is_err(), "poke: probed again");
        std::fs::remove_file(&configured).unwrap();
        assert!(m.probed_downloads_dir(&cfg, &data, &configured).is_err(), "the last answer while slskd runs, whatever it was");
        m.set_status(|s| s.state = "crashed");
        assert_eq!(m.probed_downloads_dir(&cfg, &data, &configured), Ok((configured.clone(), None)), "before a start: probed again");
        assert!(configured.is_dir(), "the probe makes the folder");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The real thing: SLSKD_ZIP=<a downloaded slskd-…-linux-x64.zip> cargo test --release -- --ignored real_slskd
    /// unpacks it the way Install does, starts it on a generated slskd.yml and checks that the API
    /// answers Encore's key (and nothing else), headless.
    #[tokio::test]
    #[ignore]
    async fn real_slskd() {
        let zip = PathBuf::from(std::env::var("SLSKD_ZIP").expect("SLSKD_ZIP"));
        let dir = tmp("real");
        let root = dir.join("tools");
        std::fs::create_dir_all(&root).unwrap();
        let bin = unpack(&root, "0.0.0", &zip).unwrap();
        println!("--version: {}", verify(&bin).await.unwrap());
        switch_to(&root, "0.0.0").unwrap();
        let bin = root.join("0.0.0").join("slskd");
        let mut y = yml(false, false);
        let vpn = encore_vpn::Vpn::default();
        y.downloads = dir.join("dl").to_string_lossy().to_string();
        y.incomplete = dir.join("dl").join(".incomplete").to_string_lossy().to_string();
        y.shares = vec![dir.join("share").to_string_lossy().to_string()];
        std::fs::create_dir_all(dir.join("share")).unwrap();
        y.username = format!("encoretest{}", encore_core::ids::random_alnum(6));
        let p = Plan { bin, app_dir: dir.join("app"), version: "0.0.0".into(), yml: render_yml(&y), dirs: vec![dir.join("dl"), dir.join("dl").join(".incomplete")], listen_port: 2234, vpn: false, downloads: dir.join("dl"), warning: None };
        let mut r = Runner::new(Managed::default());
        r.apply(p.clone().into()).await;
        let http = reqwest::Client::new();
        let url = format!("{}/api/v0/application", base_url());
        let mut ok = None;
        for _ in 0..90 {
            tokio::time::sleep(Duration::from_secs(1)).await;
            if let Ok(resp) = http.get(&url).header("X-API-Key", &y.api_key).send().await {
                ok = Some((resp.status().as_u16(), resp.text().await.unwrap_or_default()));
                break;
            }
            r.apply(p.clone().into()).await;
            assert_ne!(state(&r).0, "crashed", "{:?}\n{}", state(&r), std::fs::read_to_string(dir.join("app").join("encore-run.log")).unwrap_or_default());
        }
        let log = || std::fs::read_to_string(dir.join("app").join("encore-run.log")).unwrap_or_default();
        let (code, body) = ok.unwrap_or_else(|| panic!("no answer in 90 s\n{}", log()));
        println!("application: {code} {}", body.chars().take(400).collect::<String>());
        assert_eq!(code, 200, "{body}\n{}", log());
        let anon = http.get(&url).send().await.unwrap().status().as_u16();
        assert_eq!(anon, 401, "no key, no API");
        let wrong = http.get(&url).header("X-API-Key", "x".repeat(48)).send().await.unwrap().status().as_u16();
        assert_eq!(wrong, 401);
        let ui = http.get(format!("{}/", base_url())).send().await.unwrap().status().as_u16();
        assert_eq!(ui, 404, "headless: no web UI");
        let opts = http.get(format!("{}/api/v0/options", base_url())).header("X-API-Key", &y.api_key).send().await.unwrap().text().await.unwrap();
        let o: Value = serde_json::from_str(&opts).unwrap_or(Value::Null);
        println!("options.soulseek: {}", o["soulseek"]);
        assert_eq!(o["transfers"]["upload"]["slots"], 2, "{opts}");
        assert_eq!(o["soulseek"]["listenPort"], 2234);
        assert_eq!(o["flags"]["noVersionCheck"], true);
        assert_eq!(o["headless"], true);

        // through the VPN proxy: with no tunnel up, the server connection must fail at the proxy
        // ("network unreachable"), never go out directly
        vpn.install(None);
        y.proxy = Some(vpn.socks().await.unwrap());
        y.listen_ip = "127.0.0.1".into();
        y.server = ("208.76.170.59".into(), 2271);
        let p2 = Plan { yml: render_yml(&y), ..p.clone() };
        r.apply(p2.clone().into()).await;
        let mut seen = false;
        for _ in 0..60 {
            tokio::time::sleep(Duration::from_secs(1)).await;
            r.apply(p2.clone().into()).await;
            if log().contains("Network unreachable") {
                seen = true;
                break;
            }
        }
        let opts = http.get(format!("{}/api/v0/options", base_url())).header("X-API-Key", &y.api_key).send().await.unwrap().text().await.unwrap();
        let o: Value = serde_json::from_str(&opts).unwrap_or(Value::Null);
        println!("options.soulseek (VPN): {}", o["soulseek"]);
        assert_eq!(o["soulseek"]["connection"]["proxy"]["enabled"], true, "{opts}");
        assert_eq!(o["soulseek"]["listenIpAddress"], "127.0.0.1");
        assert!(seen, "the server connection went to the proxy and was refused there\n{}", log());
        r.apply(Want::Off).await;
        println!("{}", log().lines().take(40).collect::<Vec<_>>().join("\n"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The relay of the direct route (real sockets): slskd's own attempts are reset at once and never reach
    /// the server; Encore's attempt connects to the server first, then lets exactly one connection
    /// through, which carries bytes both ways and counts as open until it closes.
    #[tokio::test]
    async fn the_relay_lets_only_encores_attempt_through() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let server = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let up = ("127.0.0.1".to_string(), server.local_addr().unwrap().port());
        let m = Managed::default();
        let port = m.relay_port(up.clone()).await.unwrap();
        assert_eq!(m.relay_port(up.clone()).await.unwrap(), port, "one relay");
        let session = m.session().clone();
        // the door is shut: reset at once, nothing reaches the server
        let mut c = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        let mut b = [0u8; 8];
        let r = tokio::time::timeout(Duration::from_secs(5), c.read(&mut b)).await.expect("turned away at once");
        assert!(matches!(r, Ok(0) | Err(_)), "{r:?}");
        assert_eq!(session.door_seen().refused, 1);
        assert!(tokio::time::timeout(Duration::from_millis(200), server.accept()).await.is_err(), "the server saw nothing");
        // Encore's attempt: the server is reached first, then slskd's connection is joined to it
        m.open_door().await.unwrap();
        let (mut srv, _) = tokio::time::timeout(Duration::from_secs(5), server.accept()).await.unwrap().unwrap();
        let mut c = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        c.write_all(b"login").await.unwrap();
        let mut got = [0u8; 5];
        srv.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"login");
        srv.write_all(b"ok").await.unwrap();
        let mut got = [0u8; 2];
        c.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"ok");
        assert_eq!((session.door_seen().passed, session.door_seen().live), (1, 1));
        // a second connection (slskd's loop after this attempt): reset
        let mut c2 = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        let r = tokio::time::timeout(Duration::from_secs(5), c2.read(&mut b)).await.expect("turned away at once");
        assert!(matches!(r, Ok(0) | Err(_)), "{r:?}");
        // the server ends the session: the connection through the door closes
        drop(srv);
        let r = tokio::time::timeout(Duration::from_secs(5), c.read(&mut b)).await.unwrap();
        assert!(matches!(r, Ok(0) | Err(_)));
        drop(c);
        for _ in 0..50 {
            if session.door_seen().live == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(session.door_seen().live, 0, "closed");
        // a server that can't be reached: the attempt fails before slskd is asked
        drop(server);
        m.relay_port(("127.0.0.1".into(), 1)).await.unwrap();
        let e = m.open_door().await.unwrap_err().to_string();
        assert!(e.starts_with("can't reach 127.0.0.1:1: "), "{e}");
        assert!(session.door().is_none(), "the door stays shut");
    }

    impl From<Plan> for Want {
        fn from(p: Plan) -> Want {
            Want::Run(p)
        }
    }
}
