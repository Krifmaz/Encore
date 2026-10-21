//! Storage problems. A download whose import hits a storage error (EIO, EROFS, ENOSPC, EDQUOT, ENOTCONN,
//! ESTALE, EHOSTDOWN on the library, the staging folder or a download client's folder: [`fsx::StepError`])
//! is not a failure of its source: the request waits (`waiting`, reason `storage_import`) with its files
//! kept where they are (`detail.kept`), no other source is tried, nothing is downloaded again, and only the
//! import runs again: every 10 minutes, and at once when the storage is writable again, an admin presses
//! Check again or someone presses Import again. A storage error before any file existed (the staging folder
//! itself) waits the same way (reason `storage_download`) and then runs the source step again.
//!
//! The health check: before each import a folder `.encore-probe-<pid>-<n>` is made (with 4 KiB written and
//! flushed in it) and removed in the library, in the album's folder as far as it exists (with mergerfs the
//! library's root can be fine while the disk holding the artist's folder is not) and in the staging folder,
//! each at most once a minute; a waiting request's own folder, and the folder a standing problem names while
//! a request waits for it, are probed exactly each time. A storage error (a permission is the import's own
//! failure, with its own words) pauses every import, shows a banner on the Requests page (`GET
//! /api/requests/storage`), is kept in the settings table so it survives a restart, and is looked at again
//! every 10 minutes. The waiting imports resume when that look or Check again passes, or when a passing
//! check before an import clears a problem a probe had found; a problem only an import's file step saw
//! (a probe can't see every failing write) is cleared by the 10-minute look, Check again, or once no request
//! waits for it. One log line per change, in the activity log and on stdout. Checks run one at a time on
//! the blocking pool, never in the worker's loop: a hung network mount holds up only the imports.
//!
//! The attempts rule: `detail.attempts` counts the runs of a request at its sources that got a real answer
//! from a source (it is raised when someone retries or asks again after such a run). Waiting for the
//! storage or for Soulseek, a paused import, Import again, the 10-minute import retry, the start-up rescue
//! of kept files and a cancelled run never count.
use super::*;
use encore_core::fsx::{self, Step, StepError};

/// How long a waiting import waits before it is tried again, and how often a failing health check is
/// looked at again (seconds).
pub const RETRY_SECS: f64 = 600.0;
/// A probe of the same folder is reused for this long (seconds).
const PROBE_SECS: f64 = 60.0;
/// The settings row the health is kept in across a restart.
const STATE_KEY: &str = "storage_health";
/// The reasons a request waits for the storage (`detail.wait.reason`, `wait_reason` in the API).
pub const STORAGE_IMPORT: &str = "storage_import";
pub const STORAGE_DOWNLOAD: &str = "storage_download";
/// The reason a request waits for Soulseek.
pub const SOULSEEK: &str = "soulseek";
/// The reason a request waits for a source plugin the upgrade is still installing (streamrip, yt-dlp).
pub const PLUGIN_INSTALL: &str = "plugin_install";
/// What other users see while imports are paused.
pub const PLAIN_BANNER: &str = "Imports are paused because of a storage problem on the server. Your downloads are kept and will be added when it is fixed.";

/// The storage health while the server runs (kept in [`crate::Requests`]). Clones share it.
#[derive(Clone, Default)]
pub struct Health {
    state: Arc<Mutex<HealthState>>,
    /// held while a check runs: one at a time (two exact probes of one folder would trip over each other)
    checking: Arc<Mutex<()>>,
    /// the 10-minute look is running (the worker's loop starts it and goes on)
    ticking: Arc<std::sync::atomic::AtomicBool>,
    /// the download folders' probe every minute ([`super::downloads`])
    pub(super) downloads: Arc<super::downloads::Roots>,
}

#[derive(Default)]
struct HealthState {
    /// the settings row was read
    loaded: bool,
    /// the problem that pauses imports (None: imports go on)
    problem: Option<Problem>,
    /// the last full check
    checked_at: Option<f64>,
    /// folder → (when it was probed, what the probe found)
    probes: HashMap<PathBuf, (f64, Option<Probe>)>,
}

/// A failed probe or file step, kept without its io::Error (which can't be cloned).
#[derive(Clone, Debug)]
struct Probe {
    step: Step,
    op: &'static str,
    path: PathBuf,
    to: Option<PathBuf>,
    errno: Option<i32>,
    error: String,
}

impl Probe {
    fn of(e: &StepError) -> Probe {
        Probe { step: e.step, op: e.op, path: e.path.clone(), to: e.to.clone(), errno: e.errno, error: e.source.to_string() }
    }
    fn error(&self) -> StepError {
        let io = match self.errno {
            Some(n) => std::io::Error::from_raw_os_error(n),
            None => std::io::Error::other(self.error.clone()),
        };
        StepError::new(self.step, self.op, &self.path, self.to.as_deref(), io)
    }
}

/// What pauses the imports: the folder that is not writable and the step that said so.
#[derive(Clone, Debug)]
struct Problem {
    /// the folder that is not writable
    dir: PathBuf,
    probe: Probe,
    since: f64,
    checked_at: f64,
    next_check_at: f64,
    /// a probe found it (a passing probe of the folder means it is fixed); false: an import's file step did,
    /// which a probe may not see (a write that fails where a new folder doesn't)
    by_probe: bool,
}

impl Problem {
    fn json(&self) -> Value {
        let e = self.probe.error();
        json!({ "ok": false, "path": self.dir.to_string_lossy(), "op": e.op, "op_path": e.path.to_string_lossy(), "to": e.to.as_ref().map(|t| t.to_string_lossy()),
                "error": e.source.to_string(), "errno": e.errno, "step": e.step.as_str(), "what": e.what(), "since": self.since, "checked_at": self.checked_at,
                "next_check_at": self.next_check_at, "by_probe": self.by_probe })
    }
    fn from_json(v: &Value) -> Option<Problem> {
        if v["ok"].as_bool() != Some(false) {
            return None;
        }
        let op = static_op(v["op"].as_str()?);
        let path = PathBuf::from(v["path"].as_str()?);
        let op_path = v["op_path"].as_str().map(PathBuf::from).unwrap_or_else(|| path.clone());
        Some(Problem {
            dir: path,
            probe: Probe { step: Step::parse(v["step"].as_str().unwrap_or("")).unwrap_or(Step::Import), op, path: op_path, to: v["to"].as_str().map(PathBuf::from),
                           errno: v["errno"].as_i64().map(|n| n as i32), error: v["error"].as_str().unwrap_or("").to_string() },
            since: v["since"].as_f64().unwrap_or_else(now),
            checked_at: v["checked_at"].as_f64().unwrap_or_else(now),
            next_check_at: v["next_check_at"].as_f64().unwrap_or_else(now),
            by_probe: v["by_probe"].as_bool().unwrap_or(true),
        })
    }
}

fn static_op(s: &str) -> &'static str {
    ["mkdir", "write", "rename", "copy", "link", "remove", "open", "read", "stat", "start", "fsync", "set times"].into_iter().find(|o| *o == s).unwrap_or("write")
}

/// The folder a failed step says is not writable: a probe's folder, the folder a mkdir wanted, else the
/// folder of the file.
fn problem_dir(e: &StepError) -> PathBuf {
    let p = e.failing_path();
    let probe = p.file_name().is_some_and(|n| n.to_string_lossy().starts_with(".encore-probe-"));
    if probe || e.op != "mkdir" { p.parent().map(Path::to_path_buf).unwrap_or_else(|| p.to_path_buf()) } else { p.to_path_buf() }
}

fn lock(st: &impl Ctx) -> std::sync::MutexGuard<'_, HealthState> {
    let mut h = st.requests().storage.state.lock().unwrap_or_else(|p| p.into_inner());
    if !h.loaded {
        h.loaded = true;
        if let Some(v) = st.db().one("SELECT value FROM settings WHERE key=?", p![STATE_KEY]).ok().flatten().and_then(|r| serde_json::from_str::<Value>(&r.s("value")).ok()) {
            h.problem = Problem::from_json(&v);
            h.checked_at = v["checked_at"].as_f64();
        }
    }
    h
}

fn save(st: &impl Ctx, h: &HealthState) {
    let v = match &h.problem {
        Some(p) => p.json(),
        None => json!({ "ok": true, "checked_at": h.checked_at }),
    };
    let _ = st.db().execute("INSERT OR REPLACE INTO settings (key, value) VALUES (?,?)", p![STATE_KEY, v.to_string()]);
}

/// The folders Encore downloads into or imports to, registered with [`fsx::roots`] so a storage error on
/// them reads as one: the library and the extra library folders, the staging folder, Soulseek's and the
/// download clients' folders, the blackhole folders.
pub fn register_roots(st: &impl Ctx) {
    let mut roots: Vec<PathBuf> = ["library_path", "staging_path", "slskd_download_path", "qbit_download_path", "transmission_download_path",
        "deluge_download_path", "rtorrent_download_path", "sab_download_path", "nzbget_download_path", "torrent_blackhole_watch",
        "torrent_blackhole_completed", "nzb_blackhole_watch", "nzb_blackhole_completed"]
        .iter().map(|k| st.cfg().str(k)).filter(|s| !s.trim().is_empty()).map(PathBuf::from).collect();
    roots.extend(encore_library::folders::extra_folders(st.cfg()));
    roots.push(crate::slskd_managed::download_dir(st));
    fsx::roots::add(roots);
}

fn library_root(st: &impl Ctx) -> PathBuf {
    PathBuf::from(st.cfg().str("library_path"))
}

fn staging_root(st: &impl Ctx) -> PathBuf {
    PathBuf::from(st.cfg().str("staging_path"))
}

/// Probe `dir` (cached for a minute unless `fresh`): None when Encore can write there. A folder that isn't
/// there yet is made (the staging folder on a fresh install). `exact`: the folder a failed step wanted (an
/// album's folder, say) is made as that step made it, probed, and what the probe made is removed again – the
/// same operation on the same path, which with mergerfs lands on the same disk.
fn probe(st: &impl Ctx, dir: &Path, step: Step, fresh: bool, exact: bool) -> Option<Probe> {
    let t = now();
    if !fresh {
        if let Some((at, res)) = lock(st).probes.get(dir) {
            if t - at < PROBE_SECS {
                return res.clone();
            }
        }
    }
    let before = fsx::existing_ancestor(dir);
    let res = fsx::mkdir_all(dir, step).and_then(|_| fsx::probe(dir, step)).err().map(|e| Probe::of(&e));
    if exact {
        // the folders the probe made go again (only empty ones, never above what was there)
        let mut d = dir.to_path_buf();
        while d != before && d.starts_with(&before) && std::fs::remove_dir(&d).is_ok() {
            match d.parent() {
                Some(up) => d = up.to_path_buf(),
                None => break,
            }
        }
    }
    lock(st).probes.insert(dir.to_path_buf(), (t, res.clone()));
    res
}

/// The folder a waiting request waits for (`detail.wait.dir`) and the step that failed there, probed
/// exactly when it is tried again.
fn own_dir(d: &Value) -> Option<(PathBuf, Step)> {
    let w = &d["wait"];
    if !matches!(w["reason"].as_str(), Some(STORAGE_IMPORT | STORAGE_DOWNLOAD)) {
        return None;
    }
    let dir = PathBuf::from(w["dir"].as_str().filter(|x| !x.is_empty())?);
    Some((dir, Step::parse(w["error"]["step"].as_str().unwrap_or("")).unwrap_or(Step::Import)))
}

/// Does a request wait for the storage in `dir` (its failed step's folder or path lies there)? A folder no
/// request waits for any more (cancelled, deleted, imported another way) no longer holds up the imports.
fn waits_in(st: &impl Ctx, dir: &Path) -> bool {
    st.db().query("SELECT json_extract(detail, '$.wait.dir') d, json_extract(detail, '$.wait.error.path') p, json_extract(detail, '$.wait.error.to') t
                   FROM requests WHERE status IN ('waiting','importing') AND json_extract(detail, '$.wait.reason') IN (?,?)", p![STORAGE_IMPORT, STORAGE_DOWNLOAD])
        .unwrap_or_default().iter()
        .any(|r| ["d", "p", "t"].iter().any(|k| { let x = r.s(k); !x.is_empty() && Path::new(&x).starts_with(dir) }))
}

/// The folders a health check looks at: the library, `extra` (the album's folder as far as it exists),
/// `own` (a waiting request's own folder, exactly: see [`probe`]), the folder a standing problem names
/// (exactly, while a request waits for it) and the staging folder.
fn check_dirs(st: &impl Ctx, extra: Option<&Path>, own: Option<(PathBuf, Step)>) -> Vec<(PathBuf, Step, bool)> {
    let mut dirs = vec![(library_root(st), Step::Import, false)];
    if let Some(x) = extra {
        dirs.push((fsx::existing_ancestor(x), Step::Import, false));
    }
    if let Some((d, step)) = own {
        dirs.push((d, step, true));
    }
    let problem = lock(st).problem.clone();
    if let Some(p) = problem.filter(|p| waits_in(st, &p.dir)) {
        dirs.push((p.dir, p.probe.step, true));
    }
    dirs.push((staging_root(st), Step::Stage, false));
    let mut seen = std::collections::HashSet::new();
    dirs.retain(|(d, _, _)| !d.as_os_str().is_empty() && seen.insert(d.clone()));
    dirs
}

/// Run a health check (one at a time; called on the blocking pool): Ok when no folder looked at has a
/// storage error, else that error (imports are paused). A probe that fails for another reason (a permission,
/// a folder that is missing) doesn't pause anything: the import itself fails with its own words then.
/// `explicit`: the 10-minute look or Check again – a pass ends any pause and resumes the waiting imports at
/// once; a pass before an import ends only a pause a probe had found, or one no request waits for any more.
fn health_check(st: &impl Ctx, extra: Option<&Path>, own: Option<(PathBuf, Step)>, fresh: bool, explicit: bool) -> Result<(), StepError> {
    register_roots(st);
    let one = st.requests().storage.checking.clone();
    let _one = one.lock().unwrap_or_else(|p| p.into_inner());
    for (dir, step, exact) in check_dirs(st, extra, own) {
        if let Some(p) = probe(st, &dir, step, fresh || exact, exact) {
            let e = p.error();
            if !e.is_storage() {
                continue;
            }
            mark_failed(st, &e, true);
            return Err(e);
        }
    }
    mark_ok(st, explicit);
    Ok(())
}

/// [`health_check`] before an import, on the blocking pool (a hung network mount holds up only this import).
async fn checked(st: &impl Ctx, extra: Option<PathBuf>, own: Option<(PathBuf, Step)>, fresh: bool) -> Result<(), StepError> {
    let s = st.clone();
    tokio::task::spawn_blocking(move || health_check(&s, extra.as_deref(), own, fresh, false)).await.unwrap_or(Ok(()))
}

/// A storage error was seen (`by_probe`: a probe's, else a file step of an import): imports are paused. One
/// log line when that is news.
pub(super) fn mark_failed(st: &impl Ctx, e: &StepError, by_probe: bool) {
    let t = now();
    let dir = problem_dir(e);
    let mut h = lock(st);
    let news = h.problem.is_none();
    let since = h.problem.as_ref().map(|p| p.since).unwrap_or(t);
    h.problem = Some(Problem { dir: dir.clone(), probe: Probe::of(e), since, checked_at: t, next_check_at: t + RETRY_SECS, by_probe });
    h.checked_at = Some(t);
    save(st, &h);
    drop(h);
    if news {
        let line = format!("storage: {} not writable – {}; imports paused", dir.display(), e.what());
        tracing::warn!("{line}");
        st.db().log("warning", &line);
    }
}

/// Every folder looked at was writable: the pause ends when the look was the 10-minute one or Check again
/// (`explicit`), when a probe had found the problem, or when no request waits for the storage any more; the
/// waiting imports then resume (after an `explicit` look at once, else each a minute after its last try at
/// the soonest, so imports a probe can't vouch for are not tried over and over).
fn mark_ok(st: &impl Ctx, explicit: bool) {
    let mut h = lock(st);
    h.checked_at = Some(now());
    let Some(p) = h.problem.as_ref() else { return };
    let by_probe = p.by_probe;
    drop(h);
    if !(explicit || by_probe || storage_waits(st) == 0) {
        return;
    }
    let mut h = lock(st);
    let Some(old) = h.problem.take() else { return };
    save(st, &h);
    drop(h);
    let n = resume_waiting(st, !explicit);
    let line = format!("storage: {} writable again; {n} import{} resumed", old.dir.display(), if n == 1 { "" } else { "s" });
    tracing::info!("{line}");
    st.db().log("info", &line);
}

/// The requests waiting for the storage take their turn at the queue's next look (`backoff`: not sooner
/// than a minute after their last try). → how many
pub fn resume_waiting(st: &impl Ctx, backoff: bool) -> usize {
    let ids: Vec<i64> = st.db().query("SELECT id FROM requests WHERE status='waiting' AND json_extract(detail, '$.wait.reason') IN (?,?)", p![STORAGE_IMPORT, STORAGE_DOWNLOAD])
        .unwrap_or_default().iter().map(|r| r.i("id")).collect();
    let t = now();
    for id in &ids {
        let _ = if backoff {
            // its last try was RETRY_SECS before its next one
            st.db().execute("UPDATE requests SET detail=json_set(detail, '$.wait.next_try_at', max(?, COALESCE(json_extract(detail, '$.wait.next_try_at'), 0) - ?)) WHERE id=? AND status='waiting'",
                p![t, RETRY_SECS - 60.0, id])
        } else {
            st.db().execute("UPDATE requests SET detail=json_set(detail, '$.wait.next_try_at', ?) WHERE id=? AND status='waiting'", p![t, id])
        };
    }
    ids.len()
}

/// Clears `ticking` when the 10-minute look ends (also when it panicked).
struct Ticking(Arc<std::sync::atomic::AtomicBool>);

impl Drop for Ticking {
    fn drop(&mut self) {
        self.0.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

/// The worker's look every few seconds: the folders are registered, and once a pause's 10 minutes are up its
/// health check starts on the blocking pool – the worker's loop goes on without waiting for it (a hung
/// network mount must not hold up the queue), and no second one starts while it runs. → the check started
/// now, if any (tests wait for it).
pub fn tick<C: Ctx>(st: &C) -> Option<tokio::task::JoinHandle<()>> {
    register_roots(st);
    let due = lock(st).problem.as_ref().is_some_and(|p| now() >= p.next_check_at);
    let flag = st.requests().storage.ticking.clone();
    if !due || flag.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return None;
    }
    let st = st.clone();
    Some(tokio::task::spawn_blocking(move || {
        let _done = Ticking(flag);
        let _ = health_check(&st, None, None, true, true);
    }))
}

/// Check again (`POST /api/requests/storage/check`): every folder probed now, the download folders too
/// ([`super::downloads`]: a request waiting for one goes on at the queue's next look); the answer is the new
/// state. A check that doesn't end (a network mount that stopped answering) is not waited for longer than
/// 15 s: the answer then says a check still runs (`downloads.checking`), and the folder it waits on counts as
/// failing after half a minute.
pub async fn check_now<C: Ctx>(st: &C, admin: bool) -> Value {
    let s = st.clone();
    let check = tokio::task::spawn_blocking(move || {
        let _ = health_check(&s, None, None, true, true);
        super::downloads::probe_now(&s);
    });
    let _ = tokio::time::timeout(std::time::Duration::from_secs(15), check).await;
    status(st, admin)
}

/// Requests waiting for the storage now.
fn waiting_count(st: &impl Ctx) -> i64 {
    st.db().one("SELECT COUNT(*) n FROM requests WHERE status='waiting' AND json_extract(detail, '$.wait.reason') IN (?,?)", p![STORAGE_IMPORT, STORAGE_DOWNLOAD])
        .ok().flatten().map(|r| r.i("n")).unwrap_or(0)
}

/// Requests waiting for the storage, the one being tried again now included.
fn storage_waits(st: &impl Ctx) -> i64 {
    st.db().one("SELECT COUNT(*) n FROM requests WHERE status IN ('waiting','importing') AND json_extract(detail, '$.wait.reason') IN (?,?)", p![STORAGE_IMPORT, STORAGE_DOWNLOAD])
        .ok().flatten().map(|r| r.i("n")).unwrap_or(0)
}

/// A pause no request waits for any more (each was cancelled, deleted or imported) ends, a minute after it was
/// last seen: imports go on, and the next one that hits the problem pauses them again. → whether it ended
fn forget_stale(st: &impl Ctx, p: &Problem) -> bool {
    if now() - p.checked_at < PROBE_SECS || storage_waits(st) > 0 {
        return false;
    }
    let mut h = lock(st);
    if !h.problem.as_ref().is_some_and(|q| q.since == p.since && q.checked_at == p.checked_at) {
        return false;
    }
    h.problem = None;
    save(st, &h);
    drop(h);
    let line = format!("storage: no request waits for {} any more; imports go on", p.dir.display());
    tracing::info!("{line}");
    st.db().log("info", &line);
    true
}

/// `GET /api/requests/storage`. Admins: `{ok, since, checked_at, next_check_at, path, op, error, errno, step,
/// waiting, text, downloads}` (`text`: "Imports are paused: /music is not writable – mkdir /music/.encore-probe-1-0:
/// Input/output error (os error 5). Checked 14:10, next check 14:20", null while ok). Everyone else:
/// `{ok, since, text, downloads}` with the plain words. `downloads`: the download folders' probe, the banner on
/// every page ([`super::downloads::status`]). Times are unix seconds.
pub fn status(st: &impl Ctx, admin: bool) -> Value {
    let mut v = imports_status(st, admin);
    v["downloads"] = super::downloads::status(st, admin);
    v
}

/// [`status`] without the download folders.
fn imports_status(st: &impl Ctx, admin: bool) -> Value {
    let h = lock(st);
    let (problem, checked_at) = (h.problem.clone(), h.checked_at);
    drop(h);
    let Some(p) = problem.filter(|p| !forget_stale(st, p)) else {
        return if admin {
            json!({ "ok": true, "since": null, "checked_at": checked_at, "next_check_at": null, "path": null, "op": null, "error": null, "errno": null,
                    "step": null, "waiting": waiting_count(st), "text": null })
        } else {
            json!({ "ok": true, "since": null, "text": null })
        };
    };
    if !admin {
        return json!({ "ok": false, "since": p.since, "text": PLAIN_BANNER });
    }
    let e = p.probe.error();
    let clock = crate::slskd_session::clock;
    json!({ "ok": false, "since": p.since, "checked_at": p.checked_at, "next_check_at": p.next_check_at, "path": p.dir.to_string_lossy(), "op": e.op,
            "error": e.source.to_string(), "errno": e.errno, "step": e.step.as_str(), "waiting": waiting_count(st),
            "text": format!("Imports are paused: {} is not writable – {}. Checked {}, next check {}", p.dir.display(), e.what(), clock(p.checked_at), clock(p.next_check_at)) })
}

// ---- a request's kept files --------------------------------------------------------------------

/// How a download is imported and finished, kept on the request (`detail.kept`) with its files so the
/// import can run again later exactly as it would have run now.
pub(super) struct Spec {
    /// the source that downloaded the files ("streamrip", "ytdlp", "slskd", "torznab"; "plugin" for a plugin
    /// program's, named in `origin.plugin`)
    pub source: &'static str,
    /// "request": the album or song asked for; "url": a link's files by their own tags; "rip_url": a
    /// streamrip link's folders by their tags
    pub how: &'static str,
    /// the files sit in Encore's own staging folder (removed once they are in the library); false: a download
    /// client's folder, which Encore never clears
    pub own: bool,
    /// what [`complete`] records as the songs' origin
    pub origin: Value,
    /// the request's words when it is done ("Downloaded from Deezer via streamrip"); a link's words come
    /// from its counts
    pub done: String,
    /// a cover to look at when tagging (a YouTube Music hit's)
    pub hint: Option<String>,
    /// for links: what yt-dlp said about each file ({album, artist, title})
    pub entries: Vec<Value>,
    /// a torrent or Usenet download to remove from its client once the files are in
    pub torznab_id: Option<String>,
}

impl Spec {
    pub fn new(source: &'static str, own: bool, origin: Value, done: impl Into<String>) -> Spec {
        Spec { source, how: "request", own, origin, done: done.into(), hint: None, entries: vec![], torznab_id: None }
    }
}

/// What happened to an import that [`import_checked`] ran.
pub(super) enum Imported {
    /// the request is finished (completed; failed for a link that imported nothing, or for a hand pick the
    /// audio check turned away)
    Finished,
    /// a storage problem: the request waits with its files kept
    Waiting,
    /// a file step failed for another reason (a permission, say): the request failed with its files kept,
    /// since another source would fail the same way
    Stopped,
}

/// The files a request keeps that are still there. Only a file that is not found is gone: Err when one of
/// them, or the folder they are in, can't be looked at (a network or FUSE mount that went away, a disk
/// error) – that says nothing about whether they are there, and they are never given up for it.
pub(super) fn kept_files_checked(d: &Value) -> Result<Vec<PathBuf>, StepError> {
    let gone = |e: &StepError| e.source.kind() == std::io::ErrorKind::NotFound;
    let mut out = vec![];
    for f in d["kept"]["files"].as_array().into_iter().flatten().filter_map(|f| f.as_str()) {
        match fsx::metadata(f, Step::Import) {
            Ok(m) if m.is_file() => out.push(PathBuf::from(f)),
            Ok(_) => {}
            Err(e) if gone(&e) => {}
            Err(e) => return Err(e),
        }
    }
    if out.is_empty() {
        if let Some(root) = d["kept"]["root"].as_str().filter(|x| !x.is_empty()) {
            match fsx::read_dir(root, Step::Import) {
                Err(e) if !gone(&e) => return Err(e),
                _ => {}
            }
        }
    }
    Ok(out)
}

/// Does the request keep downloaded files that are still there (Retry imports them instead of downloading)?
/// Files that can't be looked at now count as there: Retry and Import again stay on the import.
pub fn has_files(d: &Value) -> bool {
    d["kept"].is_object() && kept_files_checked(d).map_or(true, |f| !f.is_empty())
}

/// The album's folder in the library as the naming template puts it (for the health check's probe).
fn album_dir(st: &impl Ctx, r: &Req) -> Option<PathBuf> {
    if !matches!(r.s("kind").as_str(), "album" | "track") {
        return None;
    }
    let mut v = encore_db::Row::new();
    for (k, val) in [("artist", r.s("artist")), ("albumartist", r.s("artist")), ("album", r.album_title()), ("title", r.s("title"))] {
        v.insert(k.into(), json!(val));
    }
    tagger::render_path(st, &v, ".flac", r.s("kind") == "track").parent().map(Path::to_path_buf)
}

/// Import downloaded files with the storage rules, and finish the request the way the source would: the
/// health check first (while imports are paused the request waits), then the import. A storage error makes
/// the request wait with its files kept; another file step that fails stops the request with its files kept
/// (other sources would fail the same way). A wrong download ([`Rejected`]) and other errors come back as
/// Err for the source's own handling (its next candidate or source).
pub(super) async fn import_checked(st: &impl Ctx, r: &mut Req, files: &[PathBuf], root: &Path, spec: Spec) -> anyhow::Result<Imported> {
    register_roots(st);
    let mut kept = json!({ "files": files, "root": root, "source": spec.source, "how": spec.how, "own": spec.own, "origin": spec.origin,
                           "done": spec.done, "hint": spec.hint, "entries": spec.entries, "torznab_id": spec.torznab_id, "reason": "storage" });
    // what an earlier try of a link already imported stays counted (its errors don't: every file still
    // there is tried again)
    for k in ["new", "already"] {
        if !r.d["kept"][k].is_null() {
            kept[k] = r.d["kept"][k].clone();
        }
    }
    r.d["kept"] = kept;
    // kept, and `importing`, before the import: a restart in the middle finds the files again, whatever the
    // source (the start-up turns an import that was running into a wait for its import)
    if r.s("status") == "importing" {
        upd(st, r, &[]);
    } else {
        upd(st, r, &[("status", json!("importing")), ("progress", json!(100)), ("message", json!("Verifying, tagging and importing…"))]);
    }
    // a waiting request's next try looks at the folders afresh (its tries are 10 minutes apart), its own
    // folder exactly
    let fresh = r.d["wait"]["reason"].is_string();
    if let Err(e) = checked(st, album_dir(st, r), own_dir(&r.d), fresh).await {
        park(st, r, &e, STORAGE_IMPORT)?;
        return Ok(Imported::Waiting);
    }
    // each file's first block is read before the import judges the files by what they hold: a disk or mount
    // that can't be read is the storage's problem, never a wrong download
    let heads = files.to_vec();
    let unreadable = tokio::task::spawn_blocking(move || heads.iter().find_map(|f| fsx::read_head(f, Step::Import).err().filter(StepError::is_storage)))
        .await.ok().flatten();
    if let Some(e) = unreadable {
        wait_for_storage(st, r, &e, STORAGE_IMPORT)?;
        return Ok(Imported::Waiting);
    }
    // the import's MusicBrainz lookups go before the other requests' (a page someone looks at still goes
    // first), and its message says what it is at: "Importing – checking the audio", "Importing – waiting for
    // MusicBrainz (about 2 minutes, 120 lookups ahead)"
    let (id, before) = (r.id, r.s("message"));
    let res = Box::pin(mb::with_priority(mb::Priority::Import, with_steps(st, id, "importing", "Importing", &before, run_import(st, r, files, root)))).await;
    match res {
        Ok(()) => Ok(Imported::Finished),
        Err(e) if e.is::<Cancelled>() => Err(e),
        Err(e) => match fsx::find(&e) {
            Some(se) if se.is_storage() => {
                wait_for_storage(st, r, se, STORAGE_IMPORT)?;
                Ok(Imported::Waiting)
            }
            Some(se) if se.step == Step::Import => {
                fail_kept(st, r, &e).await;
                Ok(Imported::Stopped)
            }
            _ => {
                // a download the audio check turned away is kept for Keep anyway ([`rejected`]); a hand pick
                // fails at once with the reason (it never falls through to other sources)
                if let Some(rej) = e.downcast_ref::<Rejected>().filter(|x| x.check == Check::Audio) {
                    let kept = r.d["kept"].clone();
                    rejected::keep(st, r, files, &kept, rej).await;
                    if r.d["pick"].is_object() {
                        if let Some(d) = r.d.as_object_mut() {
                            d.remove("kept");
                            d.remove("wait");
                        }
                        if let Some(id) = kept["torznab_id"].as_str() {
                            torznab::remove(st, id).await;
                        }
                        rejected::fail_pick(st, r, rej).await;
                        return Ok(Imported::Finished);
                    }
                }
                // the source's own handling decides; the files are not kept for a later import (written now:
                // the queue reads the request again after an error)
                if let Some(d) = r.d.as_object_mut() {
                    d.remove("kept");
                    d.remove("wait");
                }
                upd(st, r, &[]);
                Err(e)
            }
        },
    }
}

/// The import itself, as `detail.kept.how` says, and the request finished.
async fn run_import(st: &impl Ctx, r: &mut Req, files: &[PathBuf], root: &Path) -> anyhow::Result<()> {
    let kept = r.d["kept"].clone();
    let origin = kept["origin"].clone();
    match kept["how"].as_str().unwrap_or("request") {
        "url" | "rip_url" => {
            let rip = kept["how"] == "rip_url";
            let (new, already, errors) = import_link_files(st, r, files, root, rip).await?;
            match url_outcome(new.len(), already, &errors, if rip { " via streamrip" } else { "" }) {
                Ok(m) => {
                    // what was imported left the folder; files that could not be imported stay for Import again
                    let left: Vec<PathBuf> = if errors.is_empty() { vec![] } else { files.iter().filter(|f| f.is_file()).cloned().collect() };
                    if left.is_empty() {
                        finish(st, r).await;
                    } else {
                        r.d["kept"]["files"] = json!(left);
                        r.d["kept"]["new"] = json!(new);
                        r.d["kept"]["already"] = json!(already);
                        r.d["kept"]["reason"] = json!("error");
                        if let Some(d) = r.d.as_object_mut() {
                            d.remove("wait");
                        }
                    }
                    complete(st, r, &m, &new, origin).await;
                }
                Err(m) => {
                    // nothing landed in the library: the files stay for Import again
                    r.d["kept"]["reason"] = json!("error");
                    if let Some(d) = r.d.as_object_mut() {
                        d.remove("wait");
                    }
                    fail(st, r, &m).await;
                }
            }
        }
        _ => {
            let hint = kept["hint"].as_str().map(String::from);
            let (new, note) = import(st, r, files, root, hint.as_deref()).await?;
            finish(st, r).await;
            complete(st, r, &with_note(kept["done"].as_str().unwrap_or("Downloaded"), &note), &new, origin).await;
        }
    }
    Ok(())
}

/// The files are in the library: Encore's own staging folder goes, a torrent leaves its client, the kept
/// record and the wait are cleared; so do the downloads the audio check turned away and a check that was
/// off for this download ([`rejected`]).
async fn finish(st: &impl Ctx, r: &mut Req) {
    let kept = r.d["kept"].clone();
    if kept["own"].as_bool().unwrap_or(false) {
        rm(&staging(st, r));
    }
    if let Some(id) = kept["torznab_id"].as_str() {
        torznab::remove(st, id).await;
    }
    rejected::remove(st, r.id);
    if let Some(d) = r.d.as_object_mut() {
        d.remove("kept");
        d.remove("wait");
        d.remove("skip_check");
    }
}

/// A link's files into the library ("url": each file by its own tags, or one album when they share one;
/// "rip_url": each folder streamrip made). A storage error stops it (what was imported so far stays
/// counted in `kept.new`); other failures of single files are counted. → (every file imported, how many of
/// them were already in the library, the errors)
async fn import_link_files(st: &impl Ctx, r: &mut Req, files: &[PathBuf], root: &Path, rip: bool) -> anyhow::Result<(Vec<PathBuf>, usize, Vec<String>)> {
    let paths = |v: &Value| v.as_array().map(|a| a.iter().filter_map(|x| x.as_str()).map(PathBuf::from).collect::<Vec<_>>()).unwrap_or_default();
    let mut new = paths(&r.d["kept"]["new"]);
    let mut already = r.d["kept"]["already"].as_u64().unwrap_or(0) as usize;
    let mut errors: Vec<String> = vec![];
    let hint = r.d["kept"]["hint"].as_str().map(String::from);
    let entries = r.d["kept"]["entries"].as_array().cloned().unwrap_or_default();
    // the album the files share, when they all say the same one: (album artist and album, its files)
    type Group = (Option<(String, String)>, Vec<PathBuf>);
    let mut groups: Vec<Group> = vec![];
    if rip {
        let mut by_dir: std::collections::BTreeMap<PathBuf, Vec<PathBuf>> = Default::default();
        for f in files {
            by_dir.entry(f.parent().unwrap_or(root).to_path_buf()).or_default().push(f.clone());
        }
        for fs in by_dir.into_values() {
            let t = encore_media::tags::read_tags(&fs[0].to_string_lossy());
            groups.push(((fs.len() > 1).then(|| (t.s("albumartist"), t.s("album"))), fs));
        }
    } else {
        let albums: std::collections::HashSet<String> = entries.iter().filter_map(|e| e["album"].as_str().filter(|a| !a.is_empty()).map(String::from)).collect();
        if files.len() > 1 && albums.len() == 1 {
            let artist = entries.first().and_then(|e| e["artist"].as_str().map(String::from)).or(r.opt("artist")).unwrap_or("Unknown Artist".into());
            groups.push((Some((artist, albums.into_iter().next().unwrap())), files.to_vec()));
        } else {
            groups.extend(files.iter().map(|f| (None, vec![f.clone()])));
        }
    }
    for (album, fs) in groups {
        let res: anyhow::Result<()> = async {
            if let Some((artist, album)) = &album {
                let got = tagger::import_album_files(st, &fs, tagger::AlbumImport {
                    artist, album, rgid: None, source_root: root, reject: false, audio: false, source_meta: None, hint: hint.as_deref(),
                }).await?;
                new.extend(got);
                return Ok(());
            }
            let f = &fs[0];
            let t = encore_media::tags::read_tags(&f.to_string_lossy());
            let (mut artist, mut title) = (t.s("artist"), t.s("title"));
            let base = f.file_stem().unwrap_or_default().to_string_lossy().to_string();
            if !rip {
                if (artist.is_empty() || artist == "Unknown Artist") && base.contains(" - ") {
                    let (a, b) = base.split_once(" - ").unwrap();
                    (artist, title) = (a.to_string(), b.to_string());
                }
                let prefix = format!("{} - ", artist.to_lowercase());
                if !artist.is_empty() && title.to_lowercase().starts_with(&prefix) {
                    title = title[prefix.len()..].to_string();
                }
                if artist.is_empty() {
                    artist = "Unknown Artist".into();
                }
            }
            let (p, note) = tagger::import_single(st, f, &artist, &title, None, root, None, false, false, if rip { None } else { hint.as_deref() }).await?;
            if note.starts_with("already in library") {
                already += 1;
            }
            new.push(p);
            Ok(())
        }.await;
        if let Err(e) = res {
            if fsx::storage(&e).is_some() || e.is::<Cancelled>() {
                r.d["kept"]["new"] = json!(new);
                r.d["kept"]["already"] = json!(already);
                r.d["kept"]["errors"] = json!(errors);
                return Err(e);
            }
            let name = fs[0].file_name().unwrap_or_default().to_string_lossy().to_string();
            let why = fsx::explain(&e);
            st.db().log("warning", &format!("Request #{} ({}): skipped {name}: {why}", r.id, r.label()));
            tracing::warn!("request {}: skipped {name}: {why}", r.id);
            log_push(r, format!("{name}: {why}"));
            errors.push(why);
        }
    }
    Ok((new, already, errors))
}

/// The request's words after a link's import, from its counts: Err when nothing landed in the library (a
/// failure with its reason, never "Downloaded 0 file(s)"), "Already in your library" when everything was
/// there already.
pub(super) fn url_outcome(imported: usize, already: usize, errors: &[String], via: &str) -> Result<String, String> {
    let fresh = imported.saturating_sub(already);
    match (imported, errors.first()) {
        (0, Some(e)) => Err(format!("Downloaded, but nothing could be imported: {e}")),
        (0, None) => Err("Downloaded, but nothing could be imported".into()),
        (n, None) if n == already => Ok("Already in your library".into()),
        (_, None) if already > 0 => Ok(format!("Downloaded {fresh} file(s){via}, {already} already in your library")),
        (_, None) => Ok(format!("Downloaded {fresh} file(s){via}")),
        (n, Some(e)) => Ok(format!("Downloaded {fresh} of {} file(s){via} – the rest could not be imported: {e}", n + errors.len())),
    }
}

/// The request waits for the storage: `waiting` with its reason, the next try in 10 minutes, its files kept
/// (`detail.kept`), the step's error and its folder kept for its details and its next try; imports are
/// paused; no attempt is counted and no other source tried. A request that was cancelled meanwhile stays
/// cancelled (Err([`Cancelled`])).
pub(super) fn wait_for_storage(st: &impl Ctx, r: &mut Req, e: &StepError, reason: &str) -> anyhow::Result<()> {
    check_active(st, r.id)?;
    mark_failed(st, e, false);
    park(st, r, e, reason)
}

/// [`wait_for_storage`] for an error the health check found (and already paused the imports for).
fn park(st: &impl Ctx, r: &mut Req, e: &StepError, reason: &str) -> anyhow::Result<()> {
    check_active(st, r.id)?;
    let t = now();
    let at = t + RETRY_SECS;
    let same = r.d["wait"]["reason"].as_str() == Some(reason);
    let since = if same { r.d["wait"]["since"].as_f64().unwrap_or(t) } else { t };
    let text = e.to_string();
    let news = r.d["wait"]["error"]["text"].as_str() != Some(text.as_str());
    if reason == STORAGE_DOWNLOAD && !r.d["pick"].is_object() && r.s("kind") != "url" && !finished_in_client(r) {
        // the source step runs again: it goes back first in the sources
        let src = r.s("source");
        let mut srcs: Vec<Value> = r.d["sources"].as_array().cloned().unwrap_or_default();
        if !src.is_empty() && !same && srcs.first().and_then(|x| x.as_str()) != Some(src.as_str()) {
            srcs.insert(0, json!(src));
        }
        r.d["sources"] = json!(srcs);
    }
    r.d["wait"] = json!({ "reason": reason, "since": since, "next_try_at": at, "error": e.json(), "dir": problem_dir(e) });
    let lib = library_root(st);
    let place = if e.failing_path().starts_with(&lib) && !lib.as_os_str().is_empty() { "library storage error" } else { "storage error" };
    let msg = format!("Waiting: {place} – {}. Next try at {}", e.short(), crate::slskd_session::clock(at));
    if news {
        let src = r.opt("source").unwrap_or("import".into());
        log_push(r, format!("{src}: {text}"));
        let line = format!("Request #{} ({}) waits for the storage: {text}", r.id, r.label());
        tracing::warn!("{line}");
        st.db().log("warning", &line);
    }
    let progress = if reason == STORAGE_IMPORT { 100 } else { 0 };
    upd(st, r, &[("status", json!("waiting")), ("progress", json!(progress)), ("message", json!(msg))]);
    Ok(())
}

/// Like [`super::check`], for a request whose run may be importing (`importing` is active too).
fn check_active(st: &impl Ctx, id: i64) -> anyhow::Result<()> {
    super::check(st, id)
}

/// A file step of the import failed for a reason that isn't the storage's (a permission, say): the request
/// fails with its files kept (Retry imports them again), without trying another source.
async fn fail_kept(st: &impl Ctx, r: &mut Req, e: &anyhow::Error) {
    r.d["kept"]["reason"] = json!("error");
    if let Some(se) = fsx::find(e) {
        r.d["kept"]["error"] = se.json();
    }
    if let Some(d) = r.d.as_object_mut() {
        d.remove("wait");
    }
    let text = fsx::explain(e);
    let src = r.opt("source").unwrap_or("import".into());
    log_push(r, format!("{src}: {text}"));
    tracing::warn!("request {} ({}): {text}", r.id, r.label());
    fail(st, r, &text).await;
}

/// A request whose torrent or Usenet download is still its client's (`detail.torznab.id`): a storage wait
/// with it looks at that download again, not at the indexers.
fn finished_in_client(r: &Req) -> bool {
    r.s("source") == "torznab" && r.d["torznab"]["id"].is_string()
}

/// A waiting request's turn ([`super::queue::step`]): once its next try is due, the import runs again from
/// the kept files (`storage_import`), or the source step does (`storage_download`) once the staging folder is
/// writable.
pub(super) async fn retry(st: &impl Ctx, r: &mut Req) -> anyhow::Result<()> {
    if r.d["wait"]["next_try_at"].as_f64().unwrap_or(0.0) > now() {
        return Ok(());
    }
    match r.d["wait"]["reason"].as_str().unwrap_or("") {
        STORAGE_DOWNLOAD => {
            if let Err(e) = checked(st, None, own_dir(&r.d), true).await {
                return park(st, r, &e, STORAGE_DOWNLOAD);
            }
            if let Some(d) = r.d.as_object_mut() {
                d.remove("wait");
            }
            if finished_in_client(r) {
                // the torrent or Usenet download it waited with is looked at again where it is (never
                // searched for and added again)
                if let Some(o) = r.d["torznab"].as_object_mut() {
                    o.insert("last_change".into(), json!(now()));
                    o.remove("err_since");
                }
                upd(st, r, &[("status", json!("downloading")), ("message", json!("Looking at the finished download again…"))]);
                return poll_torznab(st, r).await;
            }
            if r.d["pick"].is_object() || r.s("kind") == "url" {
                return start(st, r).await;
            }
            next_source(st, r, "").await
        }
        _ => import_kept(st, r).await,
    }
}

/// The import again from the kept files (the 10-minute retry, Import again, a request asked for again).
pub(super) async fn import_kept(st: &impl Ctx, r: &mut Req) -> anyhow::Result<()> {
    let kept = r.d["kept"].clone();
    register_roots(st);
    let files = match kept_files_checked(&r.d) {
        Ok(f) => f,
        // the files can't be looked at (a mount that went away): they are not gone – the import waits for
        // the storage, or (a permission) fails with them kept
        Err(e) if e.is_storage() => return wait_for_storage(st, r, &e, STORAGE_IMPORT),
        Err(e) => {
            fail_kept(st, r, &anyhow::Error::from(e)).await;
            return Ok(());
        }
    };
    if files.is_empty() {
        // the files went (removed by hand, say): what's left is a new download
        let where_ = kept["root"].as_str().unwrap_or("").to_string();
        if let Some(d) = r.d.as_object_mut() {
            d.remove("kept");
            d.remove("wait");
        }
        let why = format!("the downloaded files are no longer in {where_}");
        if r.d["sources"].as_array().is_some_and(|a| !a.is_empty()) && !r.d["pick"].is_object() {
            return next_source(st, r, &why).await;
        }
        fail(st, r, &format!("Import failed: {why} – request it again to download it again")).await;
        return Ok(());
    }
    let root = kept["root"].as_str().map(PathBuf::from).unwrap_or_else(|| staging(st, r));
    upd(st, r, &[("status", json!("importing")), ("progress", json!(100)), ("message", json!("Importing the downloaded files…"))]);
    let spec = Spec {
        source: static_source(kept["source"].as_str().unwrap_or("")),
        how: if kept["how"] == "url" { "url" } else if kept["how"] == "rip_url" { "rip_url" } else { "request" },
        own: kept["own"].as_bool().unwrap_or(false),
        origin: kept["origin"].clone(),
        done: kept["done"].as_str().unwrap_or("Downloaded").to_string(),
        hint: kept["hint"].as_str().map(String::from),
        entries: kept["entries"].as_array().cloned().unwrap_or_default(),
        torznab_id: kept["torznab_id"].as_str().map(String::from),
    };
    match import_checked(st, r, &files, &root, spec).await {
        Ok(_) => Ok(()),
        Err(e) if e.is::<Cancelled>() => Err(e),
        Err(e) if e.is::<Rejected>() => wrong_files(st, r, &kept, &files, &e).await,
        Err(e) => {
            // something other than the files (a database error, say): the files stay for another try
            r.d["kept"] = kept;
            fail_kept(st, r, &e).await;
            Ok(())
        }
    }
}

fn static_source(s: &str) -> &'static str {
    ["streamrip", "ytdlp", "slskd", "slskd_lossless", "torznab", "plugin"].into_iter().find(|x| *x == s).unwrap_or("import")
}

/// The kept files turned out to be the wrong audio when imported again: the normal path of their source
/// (its next candidate, then the next source).
async fn wrong_files(st: &impl Ctx, r: &mut Req, kept: &Value, files: &[PathBuf], e: &anyhow::Error) -> anyhow::Result<()> {
    let why = format!("wrong audio ({e})");
    st.db().log("warning", &format!("Rejected the kept files of {}: {e}", r.label()));
    if r.d["skip_check"]["why"] == "keep" {
        return rejected::kept_turned_away(st, r, e).await;
    }
    match kept["source"].as_str().unwrap_or("") {
        "slskd" | "slskd_lossless" if r.d["slskd"]["candidates"].is_array() => {
            let root = PathBuf::from(kept["root"].as_str().unwrap_or(""));
            for p in files {
                let _ = std::fs::remove_file(p);
                encore_library::prune_empty(p.parent().unwrap_or(&root), &root);
            }
            let o = slskd_try_next(st, r, &why).await?;
            after_slskd(st, r, o).await
        }
        "torznab" if r.d["torznab"]["candidates"].is_array() => {
            let o = torznab_try_next(st, r, &why).await?;
            after_slskd(st, r, o).await
        }
        _ => {
            if kept["own"].as_bool().unwrap_or(false) {
                rm(&staging(st, r));
            }
            if r.d["sources"].as_array().is_some_and(|a| !a.is_empty()) && !r.d["pick"].is_object() {
                next_source(st, r, &why).await
            } else {
                fail(st, r, &format!("The download was the wrong audio ({e})")).await;
                Ok(())
            }
        }
    }
}

/// Import again (`POST /api/requests/{id}/import-again`, Retry on a request with kept files): a failed or
/// waiting request with its files still there (or a link's request that kept the files it could not
/// import) takes its import's turn at the queue's next look. Nothing is downloaded again and no attempt is
/// counted. Err: why it can't.
pub fn import_again(st: &impl Ctx, id: i64) -> Result<(), String> {
    let Some(mut r) = get(st, id) else { return Err("Request not found".into()) };
    let status = r.s("status");
    if status == "importing" {
        return Err("It is being imported now".into());
    }
    if !matches!(status.as_str(), "failed" | "waiting" | "completed") {
        return Err("Only a failed or waiting import can be imported again".into());
    }
    // a wait for Soulseek, for a plugin's install or for a downloads folder has nothing downloaded yet
    if status == "waiting" && (r.d["wait"]["reason"] == SOULSEEK || r.d["wait"]["reason"] == PLUGIN_INSTALL || r.d["wait"]["reason"] == super::downloads::REASON) {
        return Err("There are no downloaded files to import".into());
    }
    if !has_files(&r.d) {
        return Err("Nothing to import – the downloaded files are gone".into());
    }
    let t = now();
    let since = r.d["wait"]["since"].as_f64().unwrap_or(t);
    let (error, dir) = (r.d["wait"]["error"].clone(), r.d["wait"]["dir"].clone());
    r.d["wait"] = json!({ "reason": STORAGE_IMPORT, "since": since, "next_try_at": t, "error": error, "dir": dir });
    r.d["kept"]["reason"] = json!("storage");
    // only while it still is what was looked at: a run that finished meanwhile is left alone
    let n = st.db().execute("UPDATE requests SET status='waiting', progress=100, message=?, detail=?, updated_at=? WHERE id=? AND status=?",
        p!["Waiting to import the downloaded files", r.d.to_string(), t, id, status]).map_err(|e| e.to_string())?;
    let _ = n;
    crate::events::emit(st, id);
    Ok(())
}

/// Start-up: what was mid-import goes back to waiting (it imports at the queue's first look), waiting
/// imports try at once, and a failed request whose downloaded files are still there – kept after a storage
/// problem, or left in its staging folder by a version without this rescue – waits to be imported.
pub(super) fn recover(st: &impl Ctx) {
    let t = now();
    for r in st.db().query("SELECT id, status, kind, source, detail FROM requests WHERE status IN ('importing','waiting','failed')", p![]).unwrap_or_default() {
        let id = r.i("id");
        let mut d: Value = serde_json::from_str(&r.s("detail")).unwrap_or(json!({}));
        if !d.is_object() {
            continue;
        }
        let status = r.s("status");
        let reason = d["wait"]["reason"].as_str().unwrap_or("").to_string();
        if status == "waiting" {
            if reason == STORAGE_IMPORT || reason == STORAGE_DOWNLOAD {
                let _ = st.db().execute("UPDATE requests SET detail=json_set(detail, '$.wait.next_try_at', ?) WHERE id=?", p![t, id]);
            }
            continue;
        }
        let kept_ok = d["kept"].is_object() && has_files(&d);
        if status == "failed" && kept_ok && d["kept"]["reason"] != "storage" {
            continue; // failed for another reason: Import again is the admin's
        }
        if !kept_ok {
            // a version without kept files left them in the staging folder (an import that failed or stopped)
            let dir = PathBuf::from(st.cfg().str("staging_path")).join(format!("request-{id}"));
            let files = audio_in(&dir);
            if files.is_empty() || (status == "importing" && ["slskd", "slskd_lossless", "torznab"].contains(&r.s("source").as_str())) {
                continue;
            }
            let how = if r.s("kind") == "url" { if r.s("source") == "streamrip" { "rip_url" } else { "url" } } else { "request" };
            d["kept"] = json!({ "files": files, "root": dir, "source": r.s("source"), "how": how, "own": true, "origin": { "source": r.s("source") },
                                "done": "Downloaded", "reason": "storage" });
        }
        d["wait"] = json!({ "reason": STORAGE_IMPORT, "since": d["wait"]["since"].as_f64().unwrap_or(t), "next_try_at": t, "error": d["wait"]["error"].clone(),
                            "dir": d["wait"]["dir"].clone() });
        let _ = st.db().execute("UPDATE requests SET status='waiting', progress=100, message=?, detail=?, updated_at=? WHERE id=?",
            p!["Waiting to import the downloaded files", d.to_string(), t, id]);
        let line = format!("Request #{id}: its downloaded files are still there – they will be imported");
        tracing::info!("{line}");
        st.db().log("info", &line);
    }
}

/// The audio files under `dir` (none when it isn't there).
fn audio_in(dir: &Path) -> Vec<PathBuf> {
    if !dir.is_dir() {
        return vec![];
    }
    let mut f: Vec<PathBuf> = walkdir::WalkDir::new(dir).into_iter().flatten().filter(|e| e.file_type().is_file()).map(|e| e.into_path())
        .filter(|p| !p.to_string_lossy().ends_with(".encore-part"))
        .filter(|p| p.extension().is_some_and(|x| encore_core::audio::DOWNLOAD_EXTS.contains(&x.to_string_lossy().to_lowercase().as_str()))).collect();
    f.sort();
    f
}

/// Remove Encore's own staging folder of a request (cancelled or deleted by someone). A download client's
/// folder (Soulseek, torrents, Usenet) is never touched.
pub fn remove_staging(st: &impl Ctx, id: i64) -> bool {
    let root = PathBuf::from(st.cfg().str("staging_path"));
    if root.as_os_str().is_empty() {
        return false;
    }
    let dir = root.join(format!("request-{id}"));
    // a folder kept in place as an extra library folder (or holding one) is the library's now: it stays
    if encore_library::folders::touches_extra(st.cfg(), &dir) {
        tracing::info!("{} holds an extra library folder: it is not removed with its request", dir.display());
        return false;
    }
    dir.is_dir() && std::fs::remove_dir_all(&dir).is_ok()
}

/// What the requests list adds to a row from its `detail`: `wait_reason` ("soulseek", "storage_import",
/// "storage_download", "waiting_disk", "plugin_install" or null), `next_try_at` (unix seconds or null), `has_files` (downloaded files kept for
/// Import again), `kept_path` (where they are), `error` (the last file or program step that failed: {step,
/// op, path, to, errno, error, storage, text} or null) and `output` (a source's own output: {source, url,
/// text, ok} or null). Other users than admins see no server folders, as in the storage banner: no
/// `kept_path` or `output`, and `error` as `{storage, text}` in plain words.
pub fn row_fields(status: &str, d: &Value, admin: bool) -> serde_json::Map<String, Value> {
    let mut m = serde_json::Map::new();
    let waiting = status == "waiting";
    // a request parked for Soulseek by an older version (queued with soulseek_wait) reads as waiting too.
    // Keep anyway parks its import as a kept-files import, which only waits for the queue's next look: no
    // reason, unless the storage really failed it
    let kept_anyway = d["skip_check"]["why"] == "keep" && !d["wait"]["error"].is_object();
    let reason = if waiting && !kept_anyway {
        d["wait"]["reason"].as_str().map(String::from).or_else(|| d["soulseek_wait"].is_object().then(|| SOULSEEK.to_string()))
    } else {
        None
    };
    m.insert("wait_reason".into(), json!(reason));
    m.insert("next_try_at".into(), if waiting { d["wait"]["next_try_at"].clone() } else { Value::Null });
    let has = d["kept"].is_object() && matches!(status, "failed" | "waiting" | "importing" | "completed") && has_files(d);
    m.insert("has_files".into(), json!(has));
    m.insert("kept_path".into(), if has && admin { d["kept"]["root"].clone() } else { Value::Null });
    let err = if waiting && d["wait"]["error"].is_object() { d["wait"]["error"].clone() } else if d["kept"]["error"].is_object() { d["kept"]["error"].clone() } else { d["error"].clone() };
    let err = match (err.is_object(), admin) {
        (false, _) => Value::Null,
        (true, true) => err,
        (true, false) => {
            let storage = err["storage"].as_bool().unwrap_or(false);
            json!({ "storage": storage, "text": if storage { "A storage problem on the server – not the source" } else { "The server could not handle the downloaded files – an admin can see why" } })
        }
    };
    m.insert("error".into(), err);
    m.insert("output".into(), if admin && d["output"].is_object() { d["output"].clone() } else { Value::Null });
    m
}

/// `text` with the server's own folders (the library, the staging folder, the download clients') left out,
/// for other users than admins: "mkdir …/Duumu/Illuminate EP: Input/output error (os error 5)", "Waiting for
/// the downloads folder – … is read-only". A folder is left out where its name ends (a "/", ":", space,
/// bracket, comma or the end of the text follows), never inside a longer name.
pub fn hide_folders(text: &str) -> String {
    let mut roots: Vec<String> = fsx::roots::all().iter().map(|r| r.to_string_lossy().trim_end_matches('/').to_string()).filter(|r| !r.is_empty()).collect();
    roots.sort_by_key(|r| std::cmp::Reverse(r.len()));
    let mut out = text.to_string();
    for r in roots {
        let mut from = 0;
        while let Some(i) = out[from..].find(&r).map(|i| i + from) {
            let end = i + r.len();
            if out[end..].chars().next().is_none_or(|c| matches!(c, '/' | ':' | ' ' | ')' | ',')) {
                out.replace_range(i..end, "…");
                from = i + "…".len();
            } else {
                from = end;
            }
        }
    }
    out
}

/// A link or a pick that failed because its plugin can't run: what admins read (it names the admin's page)
/// and what everyone else reads.
pub const LINKS_NEED_YTDLP: (&str, &str) = ("Links need the yt-dlp plugin – install it under Settings → Plugins",
    "Links can't be downloaded on this server yet – an admin needs to install the yt-dlp plugin");
pub const PICKS_NEED_YTDLP: (&str, &str) = ("YouTube Music and links need the yt-dlp plugin – install it under Settings → Plugins",
    "YouTube Music and links can't be downloaded on this server yet – an admin needs to install the yt-dlp plugin");
pub const LINKS_NEED_STREAMRIP: (&str, &str) = ("Qobuz/Tidal/Deezer links need the streamrip plugin switched on in Settings → Plugins → streamrip",
    "Qobuz/Tidal/Deezer links can't be downloaded on this server yet – an admin needs to switch on the streamrip plugin");

/// A request's message as other users than admins read it: a wait for the storage or for a downloads folder,
/// and a link that needs a plugin, in plain words, without the server's folders or the admin's pages (None:
/// the message as it is).
pub fn plain_message(status: &str, message: &str, d: &Value) -> Option<String> {
    if status == "failed" {
        return [LINKS_NEED_YTDLP, PICKS_NEED_YTDLP, LINKS_NEED_STREAMRIP].iter().find(|(admin, _)| message == *admin).map(|(_, plain)| plain.to_string());
    }
    if status == "waiting" && d["wait"]["reason"] == super::downloads::REASON && message.starts_with(super::downloads::WAITING) {
        return Some(super::downloads::PLAIN_WAITING.to_string());
    }
    if status != "waiting" || !(message.starts_with("Waiting: library storage error") || message.starts_with("Waiting: storage error")) {
        return None;
    }
    let at = d["wait"]["next_try_at"].as_f64().map(|t| format!(". Next try at {}", crate::slskd_session::clock(t))).unwrap_or_default();
    match d["wait"]["reason"].as_str()? {
        STORAGE_IMPORT => Some(format!("Waiting: a storage problem on the server – your download is kept and will be added when it is fixed{at}")),
        STORAGE_DOWNLOAD => Some(format!("Waiting: a storage problem on the server – the download starts again when it is fixed{at}")),
        _ => None,
    }
}

/// Keep a source's own output on the request for its details (`detail.output`, `output` in the list):
/// streamrip's combined stdout and stderr, on every outcome. The source hands it over with its files or its
/// error (see `sources::streamrip`); `ok` says whether the run produced files.
pub(super) fn keep_output(r: &mut Req, source: &str, url: &str, text: &str, ok: bool) {
    if text.trim().is_empty() {
        return;
    }
    const CAP: usize = 20_000;
    let text: String = if text.chars().count() > CAP { text.chars().skip(text.chars().count() - CAP).collect() } else { text.to_string() };
    r.d["output"] = json!({ "source": source, "url": url, "text": text, "ok": ok, "at": now() });
}

/// A source's failure: one line in the activity log and on stdout with the request, the source and the
/// error in full (the step, operation, path and errno when a file step failed).
pub(super) fn log_failure(st: &impl Ctx, r: &Req, source: &str, why: &str) {
    if why.is_empty() {
        return;
    }
    let line = format!("Request #{} ({}): {source}: {why}", r.id, r.label());
    tracing::warn!("{line}");
    st.db().log("warning", &line);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_that_imported_nothing_is_a_failure() {
        assert_eq!(url_outcome(0, 0, &["import failed: mkdir /music/A: Input/output error (os error 5)".into()], ""),
            Err("Downloaded, but nothing could be imported: import failed: mkdir /music/A: Input/output error (os error 5)".into()));
        assert!(url_outcome(0, 0, &[], "").is_err());
        assert_eq!(url_outcome(1, 1, &[], ""), Ok("Already in your library".into()));
        assert_eq!(url_outcome(2, 0, &[], ""), Ok("Downloaded 2 file(s)".into()));
        assert_eq!(url_outcome(3, 1, &[], " via streamrip"), Ok("Downloaded 2 file(s) via streamrip, 1 already in your library".into()));
        assert_eq!(url_outcome(1, 0, &["x".into()], ""), Ok("Downloaded 1 of 2 file(s) – the rest could not be imported: x".into()));
    }

    #[test]
    fn the_list_fields_of_a_waiting_request() {
        let d = json!({ "wait": { "reason": "storage_import", "next_try_at": 100.0, "error": { "step": "import", "op": "mkdir" } },
                        "kept": { "files": ["/definitely/not/here.flac"], "root": "/x" } });
        let m = row_fields("waiting", &d, true);
        assert_eq!(m["wait_reason"], "storage_import");
        assert_eq!(m["next_try_at"], 100.0);
        assert_eq!(m["has_files"], false, "the file is gone");
        assert_eq!(m["error"]["op"], "mkdir");
        let m = row_fields("queued", &json!({ "soulseek_wait": { "since": 1.0 } }), true);
        assert_eq!(m["wait_reason"], Value::Null);
        assert_eq!(row_fields("waiting", &json!({ "soulseek_wait": { "since": 1.0 } }), true)["wait_reason"], "soulseek");
        // Keep anyway's import waits for nothing but the queue's next look; a storage error is still named
        let mut kept = json!({ "skip_check": { "by": "a", "why": "keep" }, "wait": { "reason": "storage_import", "next_try_at": 1.0 } });
        assert_eq!(row_fields("waiting", &kept, true)["wait_reason"], Value::Null);
        kept["wait"]["error"] = json!({ "step": "import", "op": "mkdir", "storage": true });
        assert_eq!(row_fields("waiting", &kept, true)["wait_reason"], "storage_import");
        // other users see no server folders
        let d = json!({ "wait": { "reason": "storage_import", "next_try_at": 100.0, "error": { "step": "import", "op": "mkdir", "path": "/music/A", "storage": true, "text": "import failed: mkdir /music/A: …" } },
                        "output": { "source": "streamrip", "text": "/downloads/encore/request-1" } });
        let m = row_fields("waiting", &d, false);
        assert_eq!(m["error"], json!({ "storage": true, "text": "A storage problem on the server – not the source" }));
        assert_eq!((m["output"].clone(), m["kept_path"].clone()), (Value::Null, Value::Null));
        let plain = plain_message("waiting", "Waiting: library storage error – import failed: mkdir /music/A: Input/output error (os error 5). Next try at 14:20", &d).unwrap();
        assert!(plain.starts_with("Waiting: a storage problem on the server – your download is kept and will be added when it is fixed. Next try at "), "{plain}");
        assert!(!plain.contains("/music"));
        assert_eq!(plain_message("waiting", "Waiting to import the downloaded files", &d), None);
        assert_eq!(plain_message("failed", "Waiting: storage error", &d), None);
        // a wait for a downloads folder: the reason and the plain words, no folder
        let disk = json!({ "wait": { "reason": "waiting_disk", "source": "ytdlp", "dir": "/downloads/encore", "next_try_at": null,
                                     "error": { "step": "stage", "op": "write", "path": "/downloads/encore/.encore-probe-1", "errno": 30, "storage": true, "text": "staging failed: …" } } });
        let m = row_fields("waiting", &disk, true);
        assert_eq!((m["wait_reason"].clone(), m["next_try_at"].clone(), m["error"]["errno"].clone()), (json!("waiting_disk"), Value::Null, json!(30)));
        assert_eq!(row_fields("waiting", &disk, false)["error"], json!({ "storage": true, "text": "A storage problem on the server – not the source" }));
        assert_eq!(plain_message("waiting", "Waiting for the downloads folder – /downloads/encore is read-only", &disk).as_deref(),
            Some("Waiting for the downloads folder – a storage problem on the server; the download starts when it is fixed"));
        fsx::roots::add(["/srv/encore-hide-test"]);
        assert_eq!(hide_folders("ytdlp: Waiting for the downloads folder – /srv/encore-hide-test is read-only"), "ytdlp: Waiting for the downloads folder – … is read-only");
        // a link whose plugin can't run: members read no admin page
        assert_eq!(plain_message("failed", LINKS_NEED_YTDLP.0, &json!({})).as_deref(), Some("Links can't be downloaded on this server yet – an admin needs to install the yt-dlp plugin"));
        assert_eq!(plain_message("failed", LINKS_NEED_STREAMRIP.0, &json!({})).as_deref(), Some(LINKS_NEED_STREAMRIP.1));
        assert_eq!(plain_message("failed", "All sources exhausted (no download sources enabled)", &json!({})), None);
        assert_eq!(hide_folders("mkdir /srv/encore-hide-test/A: x (/srv/encore-hide-test)"), "mkdir …/A: x (…)");
        assert_eq!(hide_folders("/srv/encore-hide-test2 and /srv/encore-hide-test"), "/srv/encore-hide-test2 and …");
    }
}
