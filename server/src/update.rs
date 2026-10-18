//! Encore's own updates (Settings → Diagnostics → Updates):
//!   GET  /api/system/update[?force=1]  (admin) → {current, latest, is_newer, notes, url, published_at, checked_at,
//!        restart: "ready", self_update: "ready" | "unavailable", self_update_reason, image, …}
//!   POST /api/system/update {tag?}     (admin) → 202: the new image is pulled and the helper container started
//!   POST /api/system/restart           (admin) → 202: the server stops gracefully and exits with code 75
//! The newest release comes from GitHub (`crate::tools::github_get`, ETags and the token setting) and is kept
//! in the database, so the Diagnostics page and its banner show it without waiting; the scheduler looks once a
//! day (`check_updates`). "Update now" needs the Docker socket mounted into the container; selfupdate.rs does
//! the container swap. "Restart server" ends the process with exit code 75: the image's entrypoint starts it
//! again at once, and a service unit can do the same (`RestartForceExitStatus=75`).
use crate::auth::Admin;
use crate::db::{self, RowExt};
use crate::docker::{self, Docker};
use crate::error::{err, ApiResult};
use crate::p;
use crate::selfupdate::{self, IMAGE};
use crate::state::AppState;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::LazyLock;
use std::time::Duration;

pub const REPO: &str = "Krifmaz/Encore";
/// The exit code that asks for a restart (docker/entrypoint.sh starts the server again).
pub const EXIT_RESTART: i32 = 75;
/// A build without ENCORE_VERSION (cargo build on a desk) is not a release: any release counts as newer.
pub const DEV: bool = option_env!("ENCORE_VERSION").is_none();
/// The oldest image that has the self-update helper in it.
const FIRST_SELF_UPDATING: &str = "0.28.0";
const FRESH_SECS: f64 = 3600.0;
const NOTES_MAX: usize = 4000;
/// Where the last answer from GitHub is kept (sync_state, connection 0).
const SCOPE: &str = "encore:update";
const DOCKER_CACHE: &str = "update:docker";
/// Requests still open this long after "Restart server" are cut off.
const GRACE: Duration = Duration::from_secs(10);
/// A pull of a new image (a few hundred MB) on a slow line.
const PULL_TIMEOUT: Duration = Duration::from_secs(20 * 60);

// ---- the newest release -----------------------------------------------------------------------------

fn api_base() -> String {
    #[cfg(test)]
    if let Some(b) = TEST_API.lock().unwrap().clone() {
        return b;
    }
    "https://api.github.com".into()
}

#[cfg(test)]
pub(crate) static TEST_API: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
#[cfg(test)]
pub(crate) static TEST_SOCKET: std::sync::Mutex<Option<std::path::PathBuf>> = std::sync::Mutex::new(None);

fn socket_path() -> std::path::PathBuf {
    #[cfg(test)]
    if let Some(p) = TEST_SOCKET.lock().unwrap().clone() {
        return p;
    }
    docker::SOCKET.into()
}

/// GitHub's release JSON → what is kept: {tag, latest, notes, notes_cut, url, published_at, checked_at}.
pub fn parse_release(v: &Value, now: f64) -> Option<Value> {
    let tag = v["tag_name"].as_str().map(str::trim).filter(|t| !t.is_empty())?;
    let body = v["body"].as_str().unwrap_or("").replace("\r\n", "\n");
    let cut = body.chars().count() > NOTES_MAX;
    let notes: String = body.chars().take(NOTES_MAX).collect();
    Some(json!({
        "tag": tag,
        "latest": tag.trim_start_matches(['v', 'V']),
        "notes": notes.trim_end(),
        "notes_cut": cut,
        "url": v["html_url"].as_str().map(String::from).unwrap_or_else(|| format!("https://github.com/{REPO}/releases/tag/{tag}")),
        "published_at": v["published_at"],
        "checked_at": now as i64,
    }))
}

/// Whether `latest` is newer than the running version (a dev build: any release is).
pub fn is_newer(latest: &str, current: &str, dev: bool) -> bool {
    if latest.trim().is_empty() {
        return false;
    }
    dev || crate::tools::newer(latest, current)
}

/// The last answer from GitHub kept in the database.
pub fn stored(st: &AppState) -> Option<Value> {
    let row = st.db.one("SELECT value FROM sync_state WHERE connection_id=0 AND scope=?", p![SCOPE]).ok().flatten()?;
    serde_json::from_str::<Value>(row.get("value")?.as_str()?).ok().filter(|v| v["latest"].is_string())
}

fn store(st: &AppState, release: &Value) {
    let _ = st.db.execute("INSERT OR REPLACE INTO sync_state (connection_id, scope, value) VALUES (0,?,?)", p![SCOPE, release.to_string()]);
}

/// Ask GitHub (the ETag makes an unchanged answer free) and keep the answer.
pub async fn fetch(st: &AppState) -> anyhow::Result<Value> {
    let (code, v) = crate::tools::github_get(st, &format!("{}/repos/{REPO}/releases/latest", api_base())).await?;
    match code {
        200 => {
            let r = parse_release(&v, db::now()).ok_or_else(|| anyhow::anyhow!("GitHub's answer has no release tag"))?;
            store(st, &r);
            Ok(r)
        }
        // GitHub answers 404 for a private repository too, without a token or with one that can't read it
        404 => anyhow::bail!("GitHub shows no Encore release (none yet, or the repository is private: set the GitHub token with read access to it)"),
        _ => anyhow::bail!("GitHub answered {code} for the latest Encore release: {}", v["message"].as_str().unwrap_or("")),
    }
}

/// The kept answer when it is less than an hour old (or when checks are off and nobody pressed Check now),
/// else a new one. An error keeps the old answer next to it.
async fn release(st: &AppState, force: bool) -> (Option<Value>, Option<String>) {
    let kept = stored(st);
    let fresh = kept.as_ref().and_then(|r| r["checked_at"].as_f64()).is_some_and(|t| db::now() - t < FRESH_SECS);
    if !force && (fresh || !st.cfg.bool("check_updates")) {
        return (kept, None);
    }
    match fetch(st).await {
        Ok(r) => (Some(r), None),
        Err(e) => (kept, Some(e.to_string())),
    }
}

/// The scheduled task "Check for Encore updates".
pub async fn scheduled(st: &AppState) -> anyhow::Result<String> {
    let r = fetch(st).await?;
    let latest = r["latest"].as_str().unwrap_or("");
    Ok(if is_newer(latest, crate::VERSION, DEV) { format!("Encore {latest} is available") } else { format!("Encore {} is the newest version", crate::VERSION) })
}

/// For the diagnostics answer's `machine` block (no network): whether the kept answer names a newer release.
pub fn machine_fields(st: &AppState) -> (bool, Option<String>) {
    match stored(st) {
        Some(r) => {
            let latest = r["latest"].as_str().unwrap_or("").to_string();
            let newer = is_newer(&latest, crate::VERSION, DEV);
            (newer, newer.then_some(latest))
        }
        None => (false, None),
    }
}

// ---- can this container replace itself -----------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SelfUpdate {
    pub ready: bool,
    pub reason: Option<String>,
    /// this container's full id, its name and the image it was created from (when ready)
    pub id: Option<String>,
    pub name: Option<String>,
    pub image: Option<String>,
}

impl SelfUpdate {
    fn no(reason: impl Into<String>) -> SelfUpdate {
        SelfUpdate { reason: Some(reason.into()), ..Default::default() }
    }
    fn json(&self) -> Value {
        json!({ "ready": self.ready, "reason": self.reason, "id": self.id, "name": self.name, "image": self.image })
    }
    fn from_json(v: &Value) -> SelfUpdate {
        let s = |k: &str| v[k].as_str().map(String::from);
        SelfUpdate { ready: v["ready"] == true, reason: s("reason"), id: s("id"), name: s("name"), image: s("image") }
    }
}

/// The 64-hex container id Docker mounts this container's /etc/hostname (and resolv.conf, hosts) from:
/// `… /var/lib/docker/containers/<id>/hostname /etc/hostname …` in /proc/self/mountinfo.
pub fn container_id_from_mountinfo(text: &str) -> Option<String> {
    for line in text.lines() {
        for part in line.split_whitespace() {
            if let Some(rest) = part.split("/containers/").nth(1) {
                let id = rest.split('/').next().unwrap_or("");
                if id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return Some(id.to_string());
                }
            }
        }
    }
    None
}

fn hostname() -> String {
    ["/proc/sys/kernel/hostname", "/etc/hostname"].iter().filter_map(|p| std::fs::read_to_string(p).ok())
        .map(|h| h.trim().to_string()).find(|h| !h.is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok()).unwrap_or_default()
}

fn writable(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) else { return false };
    // SAFETY: a valid NUL-terminated path; access() only reads it
    unsafe { libc::access(c.as_ptr(), libc::W_OK) == 0 }
}

/// Whether "Update now" can work here, and in plain words why not: the `self_update` setting, the socket
/// (there, a socket, writable, answering `_ping`), and this process in a container Docker knows (the id from
/// mountinfo, or the hostname, which is the id's first 12 characters unless the owner set one).
pub async fn probe_with(sock: &Path, setting: &str, hostname: &str, mountinfo: &str) -> SelfUpdate {
    if matches!(setting.trim().to_ascii_lowercase().as_str(), "off" | "false" | "no" | "0") {
        return SelfUpdate::no("Switched off in the settings (self_update = off)");
    }
    let meta = match std::fs::metadata(sock) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return SelfUpdate::no("The Docker socket is not mounted"),
        Err(e) => return SelfUpdate::no(format!("The Docker socket can't be read: {e}")),
    };
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    if !meta.file_type().is_socket() {
        return SelfUpdate::no(format!("The Docker socket is not mounted ({} is not a socket)", sock.display()));
    }
    if !writable(sock) {
        // SAFETY: plain getters without arguments
        let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
        return SelfUpdate::no(format!("The Docker socket is not writable by Encore (it runs as uid {uid}, gid {gid}; the socket's group is {}): set PGID={} or PUID=0",
            meta.gid(), meta.gid()));
    }
    let d = Docker::new(sock);
    match d.get("/_ping").await {
        Ok((200, _)) => {}
        Ok((code, v)) => return SelfUpdate::no(format!("The Docker socket refused the request: Docker answered {code}: {}", docker::message(&v))),
        Err(e) => return SelfUpdate::no(format!("The Docker socket refused the request: {e}")),
    }
    let mut candidates: Vec<String> = container_id_from_mountinfo(mountinfo).into_iter().collect();
    if !hostname.is_empty() && !candidates.iter().any(|c| c == hostname) {
        candidates.push(hostname.to_string());
    }
    for c in candidates {
        match d.inspect(&c).await {
            Ok(Some(v)) => {
                let id = v["Id"].as_str().unwrap_or("");
                if id.starts_with(&c) || v["Config"]["Hostname"].as_str() == Some(hostname) {
                    if let Some(why) = selfupdate::cannot_replace(&v) {
                        return SelfUpdate::no(why);
                    }
                    let s = |x: &Value| x.as_str().map(String::from);
                    return SelfUpdate { ready: true, reason: None, id: s(&v["Id"]), name: v["Name"].as_str().map(|n| n.trim_start_matches('/').to_string()),
                        image: s(&v["Config"]["Image"]) };
                }
            }
            Ok(None) => {}
            Err(e) => return SelfUpdate::no(format!("The Docker socket refused the request: {e}")),
        }
    }
    SelfUpdate::no("Not running in a container")
}

/// `probe_with` for this process, kept for an hour.
pub async fn self_update(st: &AppState, force: bool) -> SelfUpdate {
    if !force {
        if let Some(v) = st.cache_get(DOCKER_CACHE) {
            return SelfUpdate::from_json(&v);
        }
    }
    let mountinfo = std::fs::read_to_string("/proc/self/mountinfo").unwrap_or_default();
    let s = probe_with(&socket_path(), &st.cfg.str("self_update"), &hostname(), &mountinfo).await;
    st.cache_put(DOCKER_CACHE, s.json(), 3600);
    s
}

// ---- the answer -------------------------------------------------------------------------------------------

/// GET /api/system/update's answer from the kept release and the socket check.
pub fn answer(release: Option<&Value>, su: &SelfUpdate, current: &str, dev: bool) -> Value {
    let r = release.cloned().unwrap_or(Value::Null);
    let latest = r["latest"].as_str().map(String::from);
    json!({
        "current": current,
        "dev": dev,
        "latest": latest,
        "tag": r["tag"],
        "is_newer": latest.as_deref().is_some_and(|l| is_newer(l, current, dev)),
        "notes": r["notes"].as_str().unwrap_or(""),
        "notes_cut": r["notes_cut"] == true,
        "url": r["url"].as_str().map(String::from).unwrap_or_else(|| format!("https://github.com/{REPO}/releases")),
        "published_at": r["published_at"],
        "checked_at": r["checked_at"],
        "restart": "ready",
        "self_update": if su.ready { "ready" } else { "unavailable" },
        "self_update_reason": su.reason,
        "image": latest.as_ref().map(|l| format!("{IMAGE}:{l}")),
        "container_image": su.image,
    })
}

pub async fn status(st: &AppState, force: bool) -> Value {
    let ((release, error), su) = tokio::join!(release(st, force), self_update(st, force));
    let mut out = answer(release.as_ref(), &su, crate::VERSION, DEV);
    out["error"] = json!(error);
    out["check_updates"] = json!(st.cfg.bool("check_updates"));
    out
}

#[derive(Deserialize)]
pub struct UpdateQ {
    force: Option<String>,
}

pub async fn get_update(State(st): State<AppState>, Admin(_): Admin, Query(q): Query<UpdateQ>) -> ApiResult<Json<Value>> {
    let force = matches!(q.force.as_deref(), Some("1" | "true" | "yes"));
    Ok(Json(status(&st, force).await))
}

// ---- Update now -------------------------------------------------------------------------------------------

/// The tag to pull: `latest` or a version (a leading v is dropped: the images are tagged 0.28.0), letters,
/// digits, dots, dashes and underscores only; versions before the first self-updating image are refused
/// (their binary has no helper, so the swap could never finish).
pub fn check_tag(tag: &str) -> Result<String, String> {
    let t = tag.trim();
    let valid = !t.is_empty() && t.len() <= 64 && t.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b));
    if !valid {
        return Err("Not a valid image tag".into());
    }
    let t = if t.len() > 1 && t.starts_with(['v', 'V']) && t.as_bytes()[1].is_ascii_digit() { &t[1..] } else { t };
    if t.as_bytes()[0].is_ascii_digit() && crate::tools::compare(t, FIRST_SELF_UPDATING) == std::cmp::Ordering::Less {
        return Err(format!("Encore {t} can't update itself; pick {FIRST_SELF_UPDATING} or newer"));
    }
    Ok(t.to_string())
}

/// Pull `IMAGE:tag`, then create and start the helper container from it. Ok(the helper's name).
pub async fn start_update(d: &Docker, self_id: &str, tag: &str) -> Result<String, String> {
    let image = format!("{IMAGE}:{tag}");
    d.pull(IMAGE, tag, PULL_TIMEOUT).await.map_err(|e| format!("Could not pull {image}: {e}"))?;
    let me = d.inspect(self_id).await.map_err(|e| format!("The Docker socket refused the request: {e}"))?
        .ok_or_else(|| "Not running in a container".to_string())?;
    let body = selfupdate::plan_helper_body(&me, &image).ok_or_else(|| "The Docker socket is not mounted from the host (no bind for /var/run/docker.sock)".to_string())?;
    let name = format!("{}-update-{}", me["Name"].as_str().unwrap_or("encore").trim_start_matches('/'), db::now() as i64);
    let (code, v) = d.post(&format!("/containers/create?name={}", urlencoding::encode(&name)), Some(&body)).await
        .map_err(|e| format!("Could not start the update helper: {e}"))?;
    if code != 201 {
        return Err(format!("Could not start the update helper: Docker answered {code}: {}", docker::message(&v)));
    }
    let id = v["Id"].as_str().unwrap_or(&name).to_string();
    let (code, v) = d.post(&format!("/containers/{id}/start"), None).await.map_err(|e| format!("Could not start the update helper: {e}"))?;
    if !(200..300).contains(&code) {
        let _ = d.delete(&format!("/containers/{id}?force=true")).await;
        return Err(format!("Could not start the update helper: Docker answered {code}: {}", docker::message(&v)));
    }
    Ok(name)
}

static UPDATING: AtomicBool = AtomicBool::new(false);

#[derive(Deserialize, Default)]
pub struct UpdateBody {
    tag: Option<String>,
}

pub async fn post_update(State(st): State<AppState>, Admin(u): Admin, body: Option<Json<UpdateBody>>) -> ApiResult<(StatusCode, Json<Value>)> {
    let su = self_update(&st, true).await;
    if !su.ready {
        return Err(err(409, su.reason.unwrap_or_else(|| "Update now is not available here".into())));
    }
    let tag = match body.and_then(|b| b.0.tag).filter(|t| !t.trim().is_empty()) {
        Some(t) => t,
        None => {
            let (r, e) = release(&st, false).await;
            r.and_then(|r| r["latest"].as_str().map(String::from))
                .ok_or_else(|| err(409, e.unwrap_or_else(|| "No Encore release to update to".into())))?
        }
    };
    let tag = check_tag(&tag).map_err(|e| err(400, e))?;
    if UPDATING.swap(true, Ordering::SeqCst) {
        return Err(err(409, "An update is already on its way"));
    }
    st.db.log("warning", &format!("{} started an update to {IMAGE}:{tag}", u.s("username")));
    // The pull can take minutes: it runs in a task of its own, so a request cut off on the way (a reverse
    // proxy's read timeout, a closed tab) neither stops the update half-way nor leaves it marked as running.
    let (st2, id, tag2) = (st.clone(), su.id.clone().unwrap_or_default(), tag.clone());
    let res = tokio::spawn(async move {
        let res = start_update(&Docker::new(socket_path()), &id, &tag2).await;
        UPDATING.store(false, Ordering::SeqCst);
        match &res {
            Ok(helper) => tracing::info!("update to {IMAGE}:{tag2}: helper container {helper} started"),
            Err(e) => st2.db.log("error", &format!("Update to {IMAGE}:{tag2} failed: {e}")),
        }
        res
    })
    .await
    .unwrap_or_else(|e| Err(format!("The update stopped: {e}")));
    match res {
        Ok(helper) => Ok((StatusCode::ACCEPTED, Json(json!({ "ok": true, "message": format!("Updating to {tag}; the server restarts in a moment"), "image": format!("{IMAGE}:{tag}"), "helper": helper })))),
        Err(e) => Err(err(502, e)),
    }
}

// ---- Restart server ---------------------------------------------------------------------------------------

/// A restart asked for: `schedule` sets it off after a delay (so the 202 answer gets out first), `requested`
/// waits for it.
pub struct Restart {
    pending: AtomicBool,
    tx: tokio::sync::watch::Sender<bool>,
}

impl Default for Restart {
    fn default() -> Self {
        Restart { pending: AtomicBool::new(false), tx: tokio::sync::watch::channel(false).0 }
    }
}

impl Restart {
    /// False when a restart is already on its way.
    pub fn schedule(&'static self, delay: Duration) -> bool {
        if self.pending.swap(true, Ordering::SeqCst) {
            return false;
        }
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            self.tx.send_replace(true);
        });
        true
    }

    pub fn is_requested(&self) -> bool {
        *self.tx.borrow()
    }

    pub async fn requested(&self) {
        let mut rx = self.tx.subscribe();
        let _ = rx.wait_for(|r| *r).await;
    }
}

pub static RESTART: LazyLock<Restart> = LazyLock::new(Restart::default);

/// "Restart server": 202 at once, the restart half a second later.
pub fn restart_after(delay: Duration) -> bool {
    RESTART.schedule(delay)
}

pub async fn post_restart(State(st): State<AppState>, Admin(u): Admin) -> ApiResult<(StatusCode, Json<Value>)> {
    if restart_after(Duration::from_millis(500)) {
        st.db.log("warning", &format!("{} restarted the server", u.s("username")));
        tracing::info!("restart asked for by {}", u.s("username"));
    }
    Ok((StatusCode::ACCEPTED, Json(json!({ "ok": true, "message": "Restarting" }))))
}

/// The web server until a restart is asked for: then no new connections, open requests get up to 10 s,
/// the database's WAL is written back, slskd and the translator are stopped, the other children are killed,
/// and the process exits with 75.
pub async fn serve(listener: tokio::net::TcpListener, app: axum::Router, st: AppState) -> anyhow::Result<()> {
    let server = axum::serve(listener, app.into_make_service_with_connect_info::<std::net::SocketAddr>()).with_graceful_shutdown(RESTART.requested());
    let server = std::future::IntoFuture::into_future(server);
    tokio::pin!(server);
    tokio::select! {
        r = &mut server => r?,
        _ = async { RESTART.requested().await; tokio::time::sleep(GRACE).await } => tracing::info!("restart: requests still open after 10 s are cut off"),
    }
    if !RESTART.is_requested() {
        return Ok(());
    }
    finish(&st).await;
    tracing::info!("restart: exiting with code {EXIT_RESTART}");
    std::process::exit(EXIT_RESTART);
}

/// Write the WAL back into the database and stop the child programs.
async fn finish(st: &AppState) {
    if let Err(e) = st.db.query("PRAGMA wal_checkpoint(TRUNCATE)", p![]) {
        tracing::warn!("restart: WAL checkpoint failed: {e}");
    }
    if let Some(pid) = crate::slskd_managed::status()["pid"].as_u64() {
        stop_pid(pid as i32, Duration::from_secs(10)).await;
    }
    crate::translate::llama::stop().await;
    stop_children();
}

/// Whatever else Encore started and is still running (yt-dlp, streamrip, ffmpeg, a Cadenza render, a tool
/// update) is killed right before the exit, as `docker stop` did before: exit() runs no destructors, so
/// `kill_on_drop` never fires, and a child left behind would run on beside the restarted server, which may
/// start the same job again.
fn stop_children() {
    let me = std::process::id() as i32;
    for pid in descendants(me, &processes()) {
        if pid > 1 {
            // SAFETY: sending a signal to a process id is harmless for this process
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }
    }
}

/// `(pid, parent pid)` of every process in /proc (none where there is no /proc).
fn processes() -> Vec<(i32, i32)> {
    let Ok(dir) = std::fs::read_dir("/proc") else { return vec![] };
    dir.flatten()
        .filter_map(|e| {
            let pid: i32 = e.file_name().to_str()?.parse().ok()?;
            let stat = std::fs::read_to_string(e.path().join("stat")).ok()?;
            Some((pid, parent_of(&stat)?))
        })
        .collect()
}

/// The parent pid in a /proc/<pid>/stat line: `pid (name) state ppid …`, where the name may hold spaces and
/// brackets of its own.
pub(crate) fn parent_of(stat: &str) -> Option<i32> {
    stat.rsplit_once(')')?.1.split_whitespace().nth(1)?.parse().ok()
}

/// Every process below `root` (its children, theirs, …) in a `(pid, parent pid)` list.
pub(crate) fn descendants(root: i32, procs: &[(i32, i32)]) -> Vec<i32> {
    let mut out: Vec<i32> = vec![];
    let mut todo = vec![root];
    while let Some(parent) = todo.pop() {
        for &(pid, ppid) in procs {
            if ppid == parent && pid != root && !out.contains(&pid) {
                out.push(pid);
                todo.push(pid);
            }
        }
    }
    out
}

/// Gone, or a zombie waiting for its parent (this process, about to end).
fn gone(pid: i32) -> bool {
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(s) => s.rsplit_once(')').map(|(_, rest)| rest.trim_start().starts_with('Z')).unwrap_or(true),
        Err(_) => true,
    }
}

/// SIGTERM, then SIGKILL after `wait` (how slskd is stopped everywhere else).
async fn stop_pid(pid: i32, wait: Duration) {
    if pid <= 1 {
        return;
    }
    // SAFETY: sending a signal to a process id is harmless for this process
    unsafe { libc::kill(pid, libc::SIGTERM) };
    let deadline = tokio::time::Instant::now() + wait;
    while !gone(pid) && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    if !gone(pid) {
        unsafe { libc::kill(pid, libc::SIGKILL) };
    }
}

#[cfg(test)]
#[path = "update_tests.rs"]
mod tests;
