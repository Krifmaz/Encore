//! Encore's own updates (Settings → Diagnostics → Updates):
//!   GET  /api/system/update[?force=1]  (admin) → {current, latest, is_newer, notes, url, published_at, checked_at,
//!        restart: "ready", self_update: "ready" | "unavailable", self_update_reason, image, …}
//!   POST /api/system/update {tag?}     (admin) → 202: the new image is pulled and the helper container started
//!   POST /api/system/restart           (admin) → 202: the server stops gracefully and exits with code 75
//! The `update_channel` setting picks what counts: `stable` = the newest full release (GitHub's latest), `hotfix`
//! = the newest of those and the hotfix prereleases CI publishes on every push to the main branch (tag
//! `hotfix-<last release>.<run>`, e.g. hotfix-0.35.2.17, image :0.35.2.17); a full release is newer than every
//! hotfix before it (0.35.3 > 0.35.2.17). Both channels read GitHub's release list and take the highest version
//! in it, never GitHub's "Latest" flag alone: a hotfix built while a full release was still on its way carries the
//! older release's number (0.42.5.191 after v0.42.6) yet became "Latest", and a server on it was told it had the
//! newest version.
//! The newest release comes from GitHub (`encore_http::github`, ETags and the token setting) and is kept
//! in the database, so the Diagnostics page and its banner show it without waiting; the scheduler looks once a
//! day (`check_updates`). "Update now" needs the Docker socket mounted into the container; selfupdate.rs does
//! the container swap. "Restart server" ends the process with exit code 75: the image's entrypoint starts it
//! again at once, and a service unit can do the same (`RestartForceExitStatus=75`). The routes are the API's
//! (encore_api, served by the kit's `kit_server_updates::router`); this module is what they call. The release
//! rules (what a release and a hotfix are, the newest of them, notes cut, newer by number) and the restart
//! planning are the kit's (`kit_server_updates`); the Docker self-update is Encore's own.
use crate::docker::{self, Docker};
use crate::selfupdate::{self, IMAGE};
use crate::update_report::{self, Report};
use crate::Ctx;
use encore_core::VERSION;
use encore_db::{self as db, p};
use encore_jobs::Supervisor;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

pub use kit_server_updates::server::{is_newer, Channel};
pub use kit_server_updates::{Restart, EXIT_RESTART, HOTFIX_PREFIX};

/// The GitHub repository releases are looked up in.
pub const REPO: &str = "Krifmaz/Encore";
/// A build without ENCORE_VERSION (cargo build on a desk) is not a release: any release counts as newer.
pub const DEV: bool = option_env!("ENCORE_VERSION").is_none();
/// The oldest image that has the self-update helper in it.
const FIRST_SELF_UPDATING: &str = "0.28.0";
/// How long a release check is trusted before GitHub is asked again (seconds).
const FRESH_SECS: f64 = 3600.0;
/// Where the last answer from GitHub is kept (sync_state, connection 0), one per channel.
const SCOPE: &str = "encore:update";
/// Where the last answer of the hotfix channel is kept.
const SCOPE_HOTFIX: &str = "encore:update:hotfix";
/// How many releases a check looks through (newest first; full releases and the three kept hotfixes fit).
const RELEASE_PAGE: usize = 30;

/// The channel the `update_channel` setting names (`Channel` is the kit's: anything but `hotfix` is stable).
pub fn channel(st: &impl Ctx) -> Channel {
    Channel::parse(&st.cfg().str("update_channel"))
}

/// Where a channel's last answer is kept (`sync_state` scope).
fn scope(c: Channel) -> &'static str {
    match c {
        Channel::Stable => SCOPE,
        Channel::Hotfix => SCOPE_HOTFIX,
    }
}

/// Encore's repository, as the kit's release rules take it.
pub fn repo() -> kit_server_updates::feed::Repo {
    let (owner, app) = REPO.split_once('/').unwrap_or((REPO, REPO));
    kit_server_updates::feed::Repo::new(owner, app)
}
/// Cache key of the self-update check (an hour).
const DOCKER_CACHE: &str = "update:docker";
/// Requests still open this long after "Restart server" are cut off.
const GRACE: Duration = Duration::from_secs(10);
/// A pull of a new image (a few hundred MB) on a slow line.
const PULL_TIMEOUT: Duration = Duration::from_secs(20 * 60);

// ---- the newest release -----------------------------------------------------------------------------


/// GitHub's release JSON → what is kept: {tag, latest, notes, notes_cut, url, published_at, checked_at}
/// (the kit's `server::parse_release` for Encore's repository).
pub fn parse_release(v: &Value, now: f64) -> Option<Value> {
    kit_server_updates::server::parse_release(v, now, &repo())
}

/// The channel's pick from GitHub's release list: the highest version among the releases it counts (the kit's
/// `server::counts`: never a draft or a beta; a hotfix prerelease on the hotfix channel only), whichever of them
/// GitHub marks "Latest".
pub fn pick_newest(list: &Value, channel: Channel, now: f64) -> Option<Value> {
    kit_server_updates::server::pick_newest(list, channel, now, &repo())
}

/// The last answer from GitHub kept in the database, for the channel the settings name.
pub fn stored(st: &impl Ctx) -> Option<Value> {
    let row = st.db().one("SELECT value FROM sync_state WHERE connection_id=0 AND scope=?", p![scope(channel(st))]).ok().flatten()?;
    serde_json::from_str::<Value>(row.get("value")?.as_str()?).ok().filter(|v| v["latest"].is_string())
}

/// Keep `release` as the channel's last answer.
fn store(st: &impl Ctx, channel: Channel, release: &Value) {
    let _ = st.db().execute("INSERT OR REPLACE INTO sync_state (connection_id, scope, value) VALUES (0,?,?)", p![scope(channel), release.to_string()]);
}

/// Ask GitHub (the ETag makes an unchanged answer free) and keep the answer.
pub async fn fetch(st: &impl Ctx) -> anyhow::Result<Value> {
    let channel = channel(st);
    // the list, not /releases/latest: "Latest" can sit on a hotfix numbered below a newer full release
    let path = format!("{}/repos/{REPO}/releases?per_page={RELEASE_PAGE}", st.system().github_api);
    let (code, v) = encore_http::github::get(st, &path).await?;
    match code {
        200 => {
            let r = pick_newest(&v, channel, db::now()).ok_or_else(|| anyhow::anyhow!("GitHub shows no Encore release yet"))?;
            store(st, channel, &r);
            Ok(r)
        }
        // GitHub answers 404 for a private repository too, without a token or with one that can't read it
        404 => anyhow::bail!("GitHub shows no Encore release (none yet, or the repository is private: set the GitHub token with read access to it)"),
        _ => anyhow::bail!("GitHub answered {code} for the latest Encore release: {}", v["message"].as_str().unwrap_or("")),
    }
}

/// The kept answer when it is less than an hour old (or when checks are off and nobody pressed Check now),
/// else a new one. An error keeps the old answer next to it.
pub async fn release(st: &impl Ctx, force: bool) -> (Option<Value>, Option<String>) {
    let kept = stored(st);
    let fresh = kept.as_ref().and_then(|r| r["checked_at"].as_f64()).is_some_and(|t| db::now() - t < FRESH_SECS);
    if !force && (fresh || !st.cfg().bool("check_updates")) {
        return (kept, None);
    }
    match fetch(st).await {
        Ok(r) => (Some(r), None),
        Err(e) => (kept, Some(e.to_string())),
    }
}

/// The scheduled task "Check for Encore updates".
pub async fn scheduled(st: &impl Ctx) -> anyhow::Result<String> {
    let r = fetch(st).await?;
    let latest = r["latest"].as_str().unwrap_or("");
    Ok(if is_newer(latest, VERSION, DEV) { format!("Encore {latest} is available") } else { format!("Encore {} is the newest version", VERSION) })
}

/// For the diagnostics answer's `machine` block (no network): whether the kept answer names a newer release.
pub fn machine_fields(st: &impl Ctx) -> (bool, Option<String>) {
    match stored(st) {
        Some(r) => {
            let latest = r["latest"].as_str().unwrap_or("").to_string();
            let newer = is_newer(&latest, VERSION, DEV);
            (newer, newer.then_some(latest))
        }
        None => (false, None),
    }
}

// ---- can this container replace itself -----------------------------------------------------------------

/// Whether "Update now" can replace this container, and the facts it needs to.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SelfUpdate {
    /// the container can update itself
    pub ready: bool,
    /// why not, when it can't
    pub reason: Option<String>,
    /// this container's full id, its name and the image it was created from (when ready)
    pub id: Option<String>,
    /// the container's name
    pub name: Option<String>,
    /// the image it was created from
    pub image: Option<String>,
}

impl SelfUpdate {
    /// A "cannot update" answer with `reason`.
    fn no(reason: impl Into<String>) -> SelfUpdate {
        SelfUpdate { reason: Some(reason.into()), ..Default::default() }
    }
    /// The answer as cached.
    fn json(&self) -> Value {
        json!({ "ready": self.ready, "reason": self.reason, "id": self.id, "name": self.name, "image": self.image })
    }
    /// The answer from its cached JSON.
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

/// This machine's host name (the container id inside Docker).
pub(crate) fn hostname() -> String {
    ["/proc/sys/kernel/hostname", "/etc/hostname"].iter().filter_map(|p| std::fs::read_to_string(p).ok())
        .map(|h| h.trim().to_string()).find(|h| !h.is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok()).unwrap_or_default()
}

/// Whether this process may write `path` (`access(W_OK)`).
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
pub async fn self_update(st: &impl Ctx, force: bool) -> SelfUpdate {
    if !force {
        if let Some(v) = st.cache_get(DOCKER_CACHE) {
            return SelfUpdate::from_json(&v);
        }
    }
    let mountinfo = std::fs::read_to_string("/proc/self/mountinfo").unwrap_or_default();
    let s = probe_with(&st.system().docker_socket, &st.cfg().str("self_update"), &hostname(), &mountinfo).await;
    st.cache_put(DOCKER_CACHE, s.json(), 3600);
    s
}

// ---- the answer -------------------------------------------------------------------------------------------

/// GET /api/system/update's answer from the kept release (the kit's shared part) and the socket check.
pub fn answer(release: Option<&Value>, su: &SelfUpdate, current: &str, dev: bool) -> Value {
    let mut out = kit_server_updates::server::answer(release, current, dev, &repo());
    let latest = out["latest"].as_str().map(String::from);
    out["self_update"] = json!(if su.ready { "ready" } else { "unavailable" });
    out["self_update_reason"] = json!(su.reason);
    out["image"] = json!(latest.as_ref().map(|l| format!("{IMAGE}:{l}")));
    out["container_image"] = json!(su.image);
    out
}

/// The update card: installed and newest version, release notes, whether "Update now" works (`force` asks
/// GitHub and Docker again), and the record of the last update (`last_update`, read back from its helper).
pub async fn status(st: &impl Ctx, force: bool) -> Value {
    let ((release, error), su, last) = tokio::join!(release(st, force), self_update(st, force), update_report::collect(st, VERSION));
    let mut out = answer(release.as_ref(), &su, VERSION, DEV);
    out["last_update"] = update_report::json(last.as_ref());
    out["error"] = json!(error);
    out["check_updates"] = json!(st.cfg().bool("check_updates"));
    out["channel"] = json!(channel(st).name());
    out
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
    if t.as_bytes()[0].is_ascii_digit() && encore_core::version::compare(t, FIRST_SELF_UPDATING) == std::cmp::Ordering::Less {
        return Err(format!("Encore {t} can't update itself; pick {FIRST_SELF_UPDATING} or newer"));
    }
    Ok(t.to_string())
}

/// The words for a failed pull. The pull carries no registry login, so "unauthorized" or "denied" means the image
/// is private on GHCR (or the tag doesn't exist there).
pub fn pull_error(image: &str, e: &str) -> String {
    let l = e.to_ascii_lowercase();
    if l.contains("unauthorized") || l.contains("denied") {
        format!("Could not pull {image}: {e}. GitHub refused an anonymous pull: the encore package on ghcr.io must be public (GitHub → Packages → encore → Package settings → Change visibility).")
    } else {
        format!("Could not pull {image}: {e}")
    }
}

/// Pull `IMAGE:tag`, then create and start the helper container from it, writing each step into `r`.
/// Ok(the helper's name); Err((the step that failed, its error)).
pub async fn start_update(d: &Docker, self_id: &str, tag: &str, r: &mut Report) -> Result<String, (&'static str, String)> {
    let image = format!("{IMAGE}:{tag}");
    r.line("pull", &format!("pulling {image}"));
    d.pull(IMAGE, tag, PULL_TIMEOUT).await.map_err(|e| ("pull", pull_error(&image, &e)))?;
    r.line("pull", &format!("{image} is on this host"));
    let me = d.inspect(self_id).await.map_err(|e| ("helper", format!("The Docker socket refused the request: {e}")))?
        .ok_or_else(|| ("helper", "Not running in a container".to_string()))?;
    let body = selfupdate::plan_helper_body(&me, &image, db::SCHEMA_VERSION)
        .ok_or_else(|| ("helper", "The Docker socket is not mounted from the host (no bind for /var/run/docker.sock)".to_string()))?;
    // an ended helper of an earlier try would be read as this one's: it goes first
    if let Ok(old) = update_report::helpers(d).await {
        for (id, running, _) in old {
            if running {
                return Err(("helper", "An update helper is still running: wait for it to end".into()));
            }
            let _ = d.delete(&format!("/containers/{id}?force=true")).await;
        }
    }
    let name = format!("{}-update-{}", me["Name"].as_str().unwrap_or("encore").trim_start_matches('/'), db::now() as i64);
    r.line("helper", &format!("creating the helper container {name} (database schema {})", db::SCHEMA_VERSION));
    let (code, v) = d.post(&format!("/containers/create?name={}", urlencoding::encode(&name)), Some(&body)).await
        .map_err(|e| ("helper", format!("Could not start the update helper: {e}")))?;
    if code != 201 {
        return Err(("helper", format!("Could not start the update helper: Docker answered {code}: {}", docker::message(&v))));
    }
    let id = v["Id"].as_str().unwrap_or(&name).to_string();
    let (code, v) = d.post(&format!("/containers/{id}/start"), None).await.map_err(|e| ("helper", format!("Could not start the update helper: {e}")))?;
    if !(200..300).contains(&code) {
        let _ = d.delete(&format!("/containers/{id}?force=true")).await;
        return Err(("helper", format!("Could not start the update helper: Docker answered {code}: {}", docker::message(&v))));
    }
    r.line("helper", &format!("{name} started: it stops this server and swaps the container (the server is down for about a minute)"));
    Ok(name)
}

/// The file of Encore's repository that holds the database schema a version writes (`SCHEMA_VERSION`).
const SCHEMA_FILE: &str = "server/crates/db/src/lib.rs";

/// `pub const SCHEMA_VERSION: i64 = 42;` in a copy of [`SCHEMA_FILE`] → 42.
pub fn schema_in_source(src: &str) -> Option<i64> {
    let rest = &src[src.find("pub const SCHEMA_VERSION: i64 =")? + "pub const SCHEMA_VERSION: i64 =".len()..];
    rest[..rest.find(';')?].trim().parse().ok()
}

/// Why `tag` must not be installed over a database at schema `db`: its Encore knows only `target`, so it would
/// refuse the database and stop at once (0.42.17 and 0.42.18 were built from older code than 0.42.16).
pub fn older_schema_refusal(tag: &str, target: i64, db: i64) -> Option<String> {
    (target < db).then(|| format!("Encore {tag} can't open this server's database: it knows database schema {target}, the database is at schema {db} \
        (that release was built from older code than the one running). Nothing was changed; pick a newer release"))
}

/// The database schema release `tag` writes, read from its source on GitHub (the tag `v<tag>`). None when it
/// can't be told (no such tag on GitHub, GitHub unreachable or rate-limited): the helper checks again from
/// inside the new image before anything is stopped.
pub async fn release_schema(st: &impl Ctx, tag: &str) -> Option<i64> {
    use base64::Engine as _;
    let url = format!("{}/repos/{REPO}/contents/{SCHEMA_FILE}?ref=v{tag}", st.system().github_api);
    let (code, v) = encore_http::github::get(st, &url).await.map_err(|e| tracing::warn!("update: the schema of {tag} could not be read: {e}")).ok()?;
    if code != 200 {
        return None;
    }
    // GitHub sends the file base64-encoded, in lines
    let b64: String = v["content"].as_str()?.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;
    schema_in_source(&String::from_utf8_lossy(&bytes))
}

/// Why "Update now" was refused or failed, with the HTTP status the API answers.
#[derive(Debug, PartialEq)]
pub struct Refused(pub u16, pub String);

/// "Update now" to `tag` (none: the newest release), asked for by `who`: the new image is pulled and the
/// helper container started. Ok({ok, message, image, helper}) for the 202 answer.
pub async fn update_now(st: &impl Ctx, who: &str, tag: Option<String>) -> Result<Value, Refused> {
    let su = self_update(st, true).await;
    if !su.ready {
        return Err(Refused(409, su.reason.unwrap_or_else(|| "Update now is not available here".into())));
    }
    let tag = match tag.filter(|t| !t.trim().is_empty()) {
        Some(t) => t,
        None => {
            let (r, e) = release(st, false).await;
            r.and_then(|r| r["latest"].as_str().map(String::from))
                .ok_or_else(|| Refused(409, e.unwrap_or_else(|| "No Encore release to update to".into())))?
        }
    };
    let tag = check_tag(&tag).map_err(|e| Refused(400, e))?;
    // the release's own database schema, before anything is pulled: an older one would refuse this database
    let target_schema = release_schema(st, &tag).await;
    if let Some(why) = target_schema.and_then(|s| older_schema_refusal(&tag, s, db::SCHEMA_VERSION)) {
        let mut r = Report::start(&format!("{IMAGE}:{tag}"), &tag, VERSION, who, db::now());
        r.fail("check", &why, db::now());
        update_report::store(st, &r);
        st.db().log("error", &format!("Update to {IMAGE}:{tag} refused: {why}"));
        return Err(Refused(409, why));
    }
    let updating = st.system().updating.clone();
    if updating.swap(true, Ordering::SeqCst) {
        return Err(Refused(409, "An update is already on its way".into()));
    }
    st.db().log("warning", &format!("{who} started an update to {IMAGE}:{tag}"));
    let mut report = Report::start(&format!("{IMAGE}:{tag}"), &tag, VERSION, who, db::now());
    report.line("check", &format!("Encore {VERSION} → {tag}; container {} ({})", su.name.as_deref().unwrap_or("?"), su.image.as_deref().unwrap_or("?")));
    report.line("check", &match target_schema {
        Some(s) => format!("Encore {tag} knows database schema {s}; this database is at {}", db::SCHEMA_VERSION),
        None => format!("the database schema of Encore {tag} could not be read from GitHub: the update helper checks it from inside the new image"),
    });
    update_report::store(st, &report);
    // The pull can take minutes: it runs in a task of its own, so a request cut off on the way (a reverse
    // proxy's read timeout, a closed tab) neither stops the update half-way nor leaves it marked as running.
    let (st2, sock, id, tag2) = (st.clone(), st.system().docker_socket.clone(), su.id.clone().unwrap_or_default(), tag.clone());
    let res = tokio::spawn(async move {
        let res = start_update(&Docker::new(sock), &id, &tag2, &mut report).await;
        updating.store(false, Ordering::SeqCst);
        match &res {
            Ok(helper) => tracing::info!("update to {IMAGE}:{tag2}: helper container {helper} started"),
            Err((step, e)) => {
                report.fail(step, e, db::now());
                st2.db().log("error", &format!("Update to {IMAGE}:{tag2} failed at step {step}: {e}"));
            }
        }
        update_report::store(&st2, &report);
        res.map_err(|(_, e)| e)
    })
    .await
    .unwrap_or_else(|e| Err(format!("The update stopped: {e}")));
    match res {
        Ok(helper) => Ok(json!({ "ok": true, "message": format!("Updating to {tag}; the server restarts in a moment"), "image": format!("{IMAGE}:{tag}"), "helper": helper })),
        Err(e) => Err(Refused(502, e)),
    }
}

// ---- Restart server ---------------------------------------------------------------------------------------

/// "Restart server" asked for by `who`: the restart half a second later, so the 202 answer gets out first.
pub fn restart(st: &impl Ctx, who: &str) {
    if st.system().restart.schedule(kit_server_updates::RESTART_DELAY) {
        st.db().log("warning", &format!("{who} restarted the server"));
        tracing::info!("restart asked for by {who}");
    }
}

/// The web server until a restart is asked for: then no new connections, open requests get up to 10 s,
/// the database's WAL is written back, the helper programs of the background loops (slskd, the translator)
/// are stopped, the other children are killed, and the process exits with 75.
pub async fn serve(listener: tokio::net::TcpListener, app: axum::Router, st: impl Ctx, loops: Vec<Arc<dyn Supervisor>>) -> anyhow::Result<()> {
    let restart = st.system().restart.clone();
    let server = axum::serve(listener, app.into_make_service_with_connect_info::<std::net::SocketAddr>()).with_graceful_shutdown({
        let r = restart.clone();
        async move { r.requested().await }
    });
    let server = std::future::IntoFuture::into_future(server);
    tokio::pin!(server);
    tokio::select! {
        r = &mut server => r?,
        _ = async { restart.requested().await; tokio::time::sleep(GRACE).await } => tracing::info!("restart: requests still open after 10 s are cut off"),
    }
    if !restart.is_requested() {
        return Ok(());
    }
    finish(&st, &loops).await;
    tracing::info!("restart: exiting with code {EXIT_RESTART}");
    std::process::exit(EXIT_RESTART);
}

/// Write the WAL back into the database and stop the child programs.
async fn finish(st: &impl Ctx, loops: &[Arc<dyn Supervisor>]) {
    if let Err(e) = st.db().query("PRAGMA wal_checkpoint(TRUNCATE)", p![]) {
        tracing::warn!("restart: WAL checkpoint failed: {e}");
    }
    encore_jobs::supervisor::stop_all(loops).await;
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
pub async fn stop_pid(pid: i32, wait: Duration) {
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
