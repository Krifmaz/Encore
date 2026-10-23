//! The programs of the builtin plugins the old Download tools card listed – yt-dlp, streamrip, Cadenza, slskd
//! and the Translator – and that card's routes (`/api/system/tools*`), which answer from the plugins for one
//! more release in their old shape. Installing, updating and removing are the plugins' (encore_plugins: the
//! registry and its one installer); this module says where each program is and what version it reports.
//!
//! * yt-dlp and streamrip: the plugin's own venv (`<config>/plugins/<id>/<version>/venv`), the venv an Encore
//!   before plugins kept updated copies in (`<config>/tools/venv`, adopted in place), an image's
//!   `/opt/tools/bin` (images before 0.32) or the system's copy – the first of these that runs. ENCORE_YTDLP and the
//!   streamrip_bin setting (the owner's own copies) still win.
//! * Cadenza (server processing): the engine compiled into Encore, unless the cadenza plugin is installed and
//!   switched on with a program that works (encore_playback::cadenza).
//! * slskd and the Translator are run by encore_requests::slskd_managed and encore_library::translate::llama;
//!   their rows here come through [`Ctx::helper_info`].
//!
//! The scheduled task "Update plugins" ([`scheduled`]) checks every installed plugin for a new version and
//! updates the ones whose auto-update is on.
use crate::Ctx;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

/// Where an image before 0.32 installed yt-dlp and streamrip (gone from 0.32's image: the plugins install them).
pub const BUNDLED_DIR: &str = "/opt/tools/bin";
/// Where Cadenza's releases are published (the `cadenza` program for Linux x86_64 / aarch64).
pub const CADENZA_REPO: &str = "Krifmaz/Cadenza";
/// Every tool the old Download tools card listed, in order (each is a catalog plugin now).
pub const IDS: [&str; 5] = ["ytdlp", "streamrip", "cadenza", "slskd", "translator"];
/// How long a `--version` answer is trusted.
const VERSION_TTL: Duration = Duration::from_secs(600);

/// One program on the tools card.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ToolInfo {
    /// its id (`ffmpeg`, `yt-dlp`, …)
    pub id: String,
    /// its name on the card
    pub name: String,
    /// the version installed, if any
    pub installed: Option<String>,
    /// "bundled" | "updated" | "system" | "missing"
    pub source: String,
    /// the newest version, when a check found one
    pub latest: Option<String>,
    /// `latest` is newer than `installed`
    pub update_available: bool,
    /// when the newest version was last looked up
    pub checked_at: Option<f64>,
    /// why the last check or update failed
    pub error: Option<String>,
    /// a line shown under it
    pub note: String,
    /// Encore can install updates of it (Update / Install button)
    pub updatable: bool,
    /// the reset button's action is available (an updated copy can be removed)
    pub resettable: bool,
}

// ---- versions -----------------------------------------------------------------------------------

use encore_core::version::{extract_version, newer};

/// A program and the argument it is asked who it is with (`--version`; ffmpeg and fpcalc: `-version`).
type ProbeKey = (PathBuf, &'static str);
/// Per program and argument: when it was asked, the file's time then, and its answer (None: it did not start).
type AnswerCache = HashMap<ProbeKey, (Instant, Option<SystemTime>, Option<Answer>)>;

/// What a program printed when asked its version.
#[derive(Clone, Debug, PartialEq)]
pub struct Answer {
    /// it exited cleanly
    pub ok: bool,
    /// what it printed on stdout
    pub stdout: String,
    /// what it printed on stderr
    pub stderr: String,
}

/// What the tools card and Diagnostics keep while the server runs: the `--version` answers per program. Clones
/// share it.
#[derive(Clone, Default)]
pub struct Tools(Arc<ToolsInner>);

/// The cache and locks behind [`Tools`].
#[derive(Default)]
struct ToolsInner {
    /// the answers so far
    answers: Mutex<AnswerCache>,
    /// One lock per program and argument, so callers that arrive together run a probe once and share its answer.
    gates: Mutex<HashMap<ProbeKey, Arc<Mutex<()>>>>,
    /// Programs with a refresh running in the background.
    refreshing: Mutex<HashSet<ProbeKey>>,
    /// Bumped by forget_versions: a probe that started before it does not store its (old) answer.
    generation: AtomicU64,
}

/// Run `<exe> <arg>` (None: it did not start) through the plugins' spawn builder (encore_plugins::sandbox: a
/// session of its own without a terminal – streamrip 2.4.9 draws a spinner for its update check even for
/// `--version` –, no new privileges, Encore's environment with plain output), stdin /dev/null, at most a minute.
/// A file that is still being written ("Text file busy") is tried again a few times.
fn probe(exe: &Path, arg: &str) -> Option<Answer> {
    use encore_plugins::sandbox::{Sandbox, Spawn};
    let mut env: Vec<(String, String)> = std::env::vars().filter(|(k, _)| k != "FORCE_COLOR" && k != "CLICOLOR_FORCE").collect();
    env.retain(|(k, _)| k != "TERM" && k != "NO_COLOR");
    env.extend([("TERM".to_string(), "dumb".to_string()), ("NO_COLOR".to_string(), "1".to_string())]);
    let spawn = Spawn::new(&Sandbox::limits_only(), exe).arg(arg).env(env);
    let o = encore_plugins::install::output(&spawn, Duration::from_secs(60)).ok()?;
    Some(Answer { ok: o.ok, stdout: o.stdout, stderr: o.stderr })
}

impl Tools {
    /// Probe and store, unless another caller just did (then its answer is used).
    fn probe_and_store(&self, key: &ProbeKey, mtime: Option<SystemTime>) -> Option<Answer> {
        let gate = self.0.gates.lock().unwrap().entry(key.clone()).or_default().clone();
        let _g = gate.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((at, mt, v)) = self.0.answers.lock().unwrap().get(key) {
            if at.elapsed() < VERSION_TTL && *mt == mtime {
                return v.clone();
            }
        }
        let generation = self.0.generation.load(Ordering::SeqCst);
        let v = probe(&key.0, key.1);
        let mut map = self.0.answers.lock().unwrap();
        if self.0.generation.load(Ordering::SeqCst) == generation {
            map.insert(key.clone(), (Instant::now(), mtime, v.clone()));
        }
        v
    }

    /// What `<exe> <arg>` prints (None: it did not start), asked at most once per 10 minutes per program
    /// file. Once known, an expired answer is still returned at once while a background thread asks again,
    /// so callers are not held up; only the first look at a program (or one whose file changed) waits for it.
    pub fn answer(&self, exe: &Path, arg: &'static str) -> Option<Answer> {
        let key: ProbeKey = (exe.to_path_buf(), arg);
        let mtime = encore_core::fs::mtime(exe);
        let stale = match self.0.answers.lock().unwrap().get(&key) {
            Some((at, mt, v)) if *mt == mtime => {
                if at.elapsed() < VERSION_TTL {
                    return v.clone();
                }
                Some(v.clone())
            }
            _ => None,
        };
        let Some(old) = stale else { return self.probe_and_store(&key, mtime) };
        if self.0.refreshing.lock().unwrap().insert(key.clone()) {
            let me = self.clone();
            let k = key.clone();
            let spawned = std::thread::Builder::new().name("tool-version".into()).spawn(move || {
                me.probe_and_store(&k, encore_core::fs::mtime(&k.0));
                me.0.refreshing.lock().unwrap().remove(&k);
            });
            if spawned.is_err() {
                self.0.refreshing.lock().unwrap().remove(&key);
                return self.probe_and_store(&key, mtime);
            }
        }
        old
    }

    /// The version `<exe> --version` names, when it runs cleanly (kept as [`Tools::answer`] keeps it).
    pub fn version_of(&self, exe: &Path) -> Option<String> {
        self.answer(exe, "--version").filter(|a| a.ok).and_then(|a| extract_version(&(a.stdout + &a.stderr)))
    }

    /// A program's version line for Diagnostics (`ffmpeg -version`, `yt-dlp --version`): whether it ran
    /// cleanly and the first line it printed (stdout, else stderr; "Not installed" when it did not start).
    /// `cmd` is a path or a program on PATH; the answer is kept as [`Tools::answer`] keeps it.
    pub fn version_line(&self, cmd: &str, arg: &'static str) -> (bool, String) {
        let exe = which(cmd).unwrap_or_else(|| PathBuf::from(cmd));
        match self.answer(&exe, arg) {
            Some(a) => {
                let text = if a.stdout.is_empty() { &a.stderr } else { &a.stdout };
                (a.ok, text.lines().next().unwrap_or("").chars().take(160).collect())
            }
            None => (false, "Not installed".into()),
        }
    }

    /// How many program answers are kept (for /api/admin/perf).
    pub fn kept(&self) -> usize {
        self.0.answers.lock().unwrap().len()
    }

    /// Ask every program again (after an install, an update or a removal).
    pub fn forget_versions(&self) {
        self.0.generation.fetch_add(1, Ordering::SeqCst);
        self.0.answers.lock().unwrap().clear();
    }
}

// ---- where the programs are ---------------------------------------------------------------------

use encore_core::which::which;

/// The executable each tool installs.
fn exes(id: &str) -> &'static [&'static str] {
    if id == "streamrip" { &["rip", "streamrip"] } else { &["yt-dlp"] }
}

/// A tool's executable in one bin folder.
fn in_dir(dir: &Path, id: &str) -> Option<PathBuf> {
    exes(id).iter().map(|n| dir.join(n)).find(|p| p.is_file())
}

/// Short name, for messages and the task's result line.
pub fn label(id: &str) -> &'static str {
    match id {
        "ytdlp" => "yt-dlp",
        "streamrip" => "streamrip",
        "cadenza" => "Cadenza",
        "translator" => "Translator",
        _ => "slskd",
    }
}

/// The name on the Download tools card.
fn title(id: &str) -> &'static str {
    match id {
        "cadenza" => "Cadenza (server processing)",
        "slskd" => "slskd – Soulseek engine",
        "translator" => "Translator – llama.cpp and a local model for titles",
        _ => label(id),
    }
}

/// The venv an Encore before plugins kept its updated yt-dlp and streamrip in (adopted by the plugins in place).
pub fn venv_dir(data: &Path) -> PathBuf {
    data.join("tools").join("venv")
}

/// Where a copy of yt-dlp or streamrip came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyFrom {
    /// the plugin's own install under `<config>/plugins/<id>`
    Plugin,
    /// the venv of an Encore before plugins (`<config>/tools/venv`)
    Adopted,
    /// an image before 0.32 (`/opt/tools/bin`)
    Bundled,
    /// the system's (PATH)
    System,
}

impl CopyFrom {
    /// The old card's `source`.
    pub fn source(self) -> &'static str {
        match self {
            CopyFrom::Plugin | CopyFrom::Adopted => "updated",
            CopyFrom::Bundled => "bundled",
            CopyFrom::System => "system",
        }
    }
}

/// The copies of yt-dlp / streamrip there are, in order of preference: the plugin's own install, the adopted
/// venv, an older image's, the system's.
fn copies(st: &impl Ctx, id: &str) -> Vec<(PathBuf, CopyFrom)> {
    let mut out = vec![];
    if let Some((_, p)) = st.plugins().program(id) {
        out.push((p, CopyFrom::Plugin));
    }
    if let Some(p) = in_dir(&venv_dir(st.data_dir()).join("bin"), id) {
        out.push((p, CopyFrom::Adopted));
    }
    if let Some(p) = in_dir(Path::new(BUNDLED_DIR), id) {
        out.push((p, CopyFrom::Bundled));
    }
    let system = if id == "streamrip" { which("streamrip").or_else(|| which("rip")) } else { which("yt-dlp") };
    if let Some(p) = system.filter(|p| !out.iter().any(|(o, _)| o == p)) {
        out.push((p, CopyFrom::System));
    }
    out
}

/// The copy of "ytdlp" / "streamrip" to run: (path, version, where from) – the first that runs, in the order
/// of [`copies`] (the plugin's own install wins: Update installs there). None: none runs (or the plugin is not
/// installed).
pub fn resolve(st: &impl Ctx, id: &str) -> Option<(PathBuf, Option<String>, CopyFrom)> {
    if !st.plugins().installed(id) {
        return None;
    }
    let tools = &st.system().tools;
    copies(st, id).into_iter().find_map(|(p, from)| tools.version_of(&p).map(|v| (p, Some(v), from)))
}

/// The executable to run for a tool: for streamrip, the streamrip_bin setting still wins when set; yt-dlp's
/// ENCORE_YTDLP is read by its source (encore_requests::sources::ytdlp::bin). For "cadenza": the plugin's
/// program when it is installed, switched on and works, else None (the built-in engine). None as well while a
/// tool's plugin is not installed.
pub fn path(st: &impl Ctx, id: &str) -> Option<PathBuf> {
    if id == "cadenza" {
        return st.playback().cadenza.external_of(st.plugins()).map(|c| c.path);
    }
    if id == "streamrip" {
        let custom = st.cfg().str("streamrip_bin");
        if !custom.trim().is_empty() {
            return which(custom.trim());
        }
    }
    resolve(st, id).map(|(p, _, _)| p)
}

// ---- the old card -------------------------------------------------------------------------------

/// ENCORE_YTDLP: the yt-dlp Encore runs instead of its own (see sources::ytdlp::bin).
fn ytdlp_env() -> Option<String> {
    std::env::var("ENCORE_YTDLP").ok().filter(|v| !v.trim().is_empty())
}

/// A copy chosen by the owner (streamrip_bin setting, ENCORE_YTDLP) – Encore leaves it alone.
fn custom_copy(st: &impl Ctx, id: &str) -> bool {
    match id {
        "streamrip" => !st.cfg().str("streamrip_bin").trim().is_empty(),
        "ytdlp" => ytdlp_env().is_some(),
        _ => false,
    }
}

/// What the plugin's card says about checks: {latest, checked_at, error} for the old card's fields.
pub fn last_check(st: &impl Ctx, id: &str) -> Value {
    let card = st.plugins().card(id).unwrap_or(Value::Null);
    json!({ "latest": card["latest"], "checked_at": card["last_check"], "error": card["last_error"] })
}

/// The card row of program `id` as known locally (installed version, source), with what the last check found.
fn local_info(st: &impl Ctx, id: &str) -> ToolInfo {
    match id {
        "cadenza" => return cadenza_info(st),
        "slskd" | "translator" => return st.helper_info(id, &last_check(st, id)),
        _ => {}
    }
    let custom = custom_copy(st, id);
    let installed_plugin = st.plugins().installed(id);
    let found = if !custom {
        resolve(st, id)
    } else if id == "ytdlp" {
        ytdlp_env().and_then(|e| which(&e)).map(|p| { let v = st.system().tools.version_of(&p); (p, v, CopyFrom::System) })
    } else {
        path(st, id).map(|p| { let v = st.system().tools.version_of(&p); (p, v, CopyFrom::System) })
    };
    let (installed, source) = match &found {
        Some((_, v, _)) if custom => (v.clone(), "system"),
        Some((_, v, from)) => (v.clone(), from.source()),
        None => (None, "missing"),
    };
    let last = last_check(st, id);
    let latest = last["latest"].as_str().map(String::from);
    let update_available = !custom && installed_plugin && latest.as_deref().is_some_and(|l| installed.as_deref().map(|i| newer(l, i)).unwrap_or(true));
    let note = match (custom, id, installed_plugin) {
        (true, "ytdlp", _) => "Uses the yt-dlp set by ENCORE_YTDLP – Encore does not update that copy".to_string(),
        (true, _, _) => "Uses the streamrip set under streamrip → Path – Encore does not update that copy".to_string(),
        (false, _, false) => format!("Not installed – install the {} plugin under Settings → Plugins", label(id)),
        _ => String::new(),
    };
    ToolInfo { id: id.into(), name: title(id).into(), installed, source: source.into(), latest, update_available,
        // no bundled copy since 0.32: there is nothing for the old "Use the bundled version" to go back to
        checked_at: last["checked_at"].as_f64(), error: last["error"].as_str().map(String::from), note, updatable: !custom, resettable: false }
}

/// Cadenza: the plugin's program when it works, else the engine built into Encore.
fn cadenza_info(st: &impl Ctx) -> ToolInfo {
    let ext = st.playback().cadenza.external_of(st.plugins());
    let (installed, source) = match &ext {
        Some(e) => (e.version.clone(), "updated"),
        None => (encore_playback::cadenza::builtin_cadenza_version(), "bundled"),
    };
    let last = last_check(st, "cadenza");
    let latest = last["latest"].as_str().map(String::from);
    let update_available = latest.as_deref().is_some_and(|l| newer(l, &installed));
    let mut error = last["error"].as_str().map(String::from);
    let program = st.plugins().program_or_adopted("cadenza").filter(|_| st.plugins().enabled("cadenza"));
    let note = match (&ext, program) {
        (Some(_), _) => "Downloaded from Cadenza's releases – server processing runs it".to_string(),
        (None, Some(p)) => {
            let why = st.playback().cadenza.describe(&p.path).err().unwrap_or_default();
            error.get_or_insert_with(|| format!("The downloaded Cadenza {} does not work ({why}) – using the built-in engine", p.version));
            "The engine built into Encore".to_string()
        }
        (None, None) => "The engine built into Encore".to_string(),
    };
    ToolInfo { id: "cadenza".into(), name: title("cadenza").into(), installed: Some(installed), source: source.into(), latest, update_available,
        checked_at: last["checked_at"].as_f64(), error, note, updatable: true, resettable: st.plugins().installed("cadenza") }
}

/// slskd, run by Encore (encore_requests::slskd_managed): its card from the installed version.
pub fn slskd_info(installed: Option<String>, last: &Value) -> ToolInfo {
    let latest = last["latest"].as_str().map(String::from);
    let update_available = latest.as_deref().is_some_and(|l| installed.as_deref().map(|i| newer(l, i)).unwrap_or(true));
    let note = if installed.is_some() { "Runs inside Encore for Soulseek" } else { "Not installed – install the Soulseek (slskd) plugin under Settings → Plugins" };
    ToolInfo { id: "slskd".into(), name: title("slskd").into(), source: if installed.is_some() { "updated" } else { "missing" }.into(), resettable: installed.is_some(),
        installed, latest, update_available, checked_at: last["checked_at"].as_f64(), error: last["error"].as_str().map(String::from), note: note.into(), updatable: true }
}

/// The listing from what is known now (no network).
pub fn cached(st: &impl Ctx) -> Vec<ToolInfo> {
    IDS.iter().map(|id| local_info(st, id)).collect()
}

/// Look up the newest versions of the installed ones (the plugins' checks), then list them.
pub async fn check(st: &impl Ctx) -> Vec<ToolInfo> {
    for id in IDS {
        if st.plugins().installed(id) {
            let _ = st.plugins().check(id).await;
        }
    }
    st.playback().cadenza.forget();
    let st2 = st.clone();
    tokio::task::spawn_blocking(move || cached(&st2)).await.unwrap_or_default()
}

/// The old card's Install / Update: the plugin's install (when it is not installed) or update.
pub async fn update(st: &impl Ctx, id: &str) -> anyhow::Result<ToolInfo> {
    if !IDS.contains(&id) {
        anyhow::bail!("{id} can't be updated from Encore");
    }
    let res = if st.plugins().installed(id) { st.plugins().update(id).await } else { st.plugins().install(id).await };
    st.system().tools.forget_versions();
    st.playback().cadenza.forget();
    res.map_err(|e| anyhow::anyhow!("{}", e.message))?;
    let (st2, id2) = (st.clone(), id.to_string());
    Ok(tokio::task::spawn_blocking(move || local_info(&st2, &id2)).await?)
}

/// The old card's reset button for Cadenza, slskd and the Translator: the plugin is removed (its own install;
/// adopted folders stay for the previous image, and its settings stay). yt-dlp and streamrip refuse: the old
/// "Use the bundled version" (still in the 0.30 apps) would remove YouTube Music, links or Qobuz/Tidal/Deezer
/// altogether, since the image has no bundled copy any more.
pub async fn reset(st: &impl Ctx, id: &str) -> anyhow::Result<()> {
    if !IDS.contains(&id) {
        anyhow::bail!("{} has no updated copy to remove", label(id));
    }
    if id == "ytdlp" || id == "streamrip" {
        anyhow::bail!("There is no bundled copy since 0.32 – remove or update the {} plugin under Settings → Plugins", label(id));
    }
    if st.plugins().installed(id) {
        st.plugins().remove(id).await.map_err(|e| anyhow::anyhow!("{}", e.message))?;
    }
    st.system().tools.forget_versions();
    st.playback().cadenza.forget();
    Ok(())
}

// ---- the scheduled task -------------------------------------------------------------------------

/// YTDLP_AUTO_UPDATE=false (the old start-up switch) keeps yt-dlp from updating by itself.
fn env_allows_ytdlp() -> bool {
    !std::env::var("YTDLP_AUTO_UPDATE").is_ok_and(|v| v.trim().eq_ignore_ascii_case("false"))
}

/// What the scheduled task does with one plugin, from what the check found.
#[derive(Debug, PartialEq)]
pub enum Plan {
    /// a result line, nothing to install
    Say(String),
    /// the check failed (the reason)
    Error(String),
    /// update `to` (from: installed version, if any)
    Install {
        /// the version installed now, if any
        from: Option<String>,
        /// the version to install
        to: String,
    },
}

/// One installed plugin as the task sees it (from its card).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Seen {
    /// its name
    pub name: String,
    /// the version installed, if any
    pub installed: Option<String>,
    /// the newest version the check found
    pub latest: Option<String>,
    /// the check found a newer version
    pub update_available: bool,
    /// what its last check or update said when it failed
    pub error: Option<String>,
    /// it has a place to update from
    pub can_update: bool,
}

impl Seen {
    /// What a plugin card says about the plugin.
    pub fn of(card: &Value) -> Seen {
        let s = |k: &str| card[k].as_str().filter(|v| !v.is_empty()).map(String::from);
        Seen { name: s("name").unwrap_or_default(), installed: s("installed_version"), latest: s("latest"), update_available: card["update_available"].as_bool().unwrap_or(false),
            error: s("last_error"), can_update: card["can_update"].as_bool().unwrap_or(false) }
    }
}

/// `auto`: the plugin's auto-update is on (and nothing holds it back).
pub fn plan(t: &Seen, auto: bool) -> Plan {
    let name = &t.name;
    if let Some(e) = &t.error {
        if e.starts_with("Could not check") {
            return Plan::Error(format!("{name}: {e}"));
        }
    }
    if !t.update_available {
        return Plan::Say(format!("{name} up to date"));
    }
    let to = t.latest.clone().unwrap_or_default();
    if !auto || !t.can_update {
        return Plan::Say(format!("{name} {to} available"));
    }
    Plan::Install { from: t.installed.clone(), to }
}

/// Whether the task may update plugin `id` by itself: its auto-update is on, yt-dlp isn't held back by the
/// environment (YTDLP_AUTO_UPDATE=false), and streamrip is switched on (a streamrip the owner turned off is only
/// reported), and its program is not the owner's own copy.
pub fn task_auto(id: &str, auto: bool, wanted: bool, ytdlp_env: bool) -> bool {
    auto && (id != "ytdlp" || ytdlp_env) && (id != "streamrip" || wanted)
}

/// The scheduled task "Update plugins": install the programs that are missing (a plugin switched on whose
/// program is not there or does not run, as after an upgrade without network), check every installed plugin
/// for a new version, then update the ones whose auto-update is on.
pub async fn scheduled(st: &impl Ctx) -> anyhow::Result<String> {
    let mut parts = vec![];
    let mut errors = vec![];
    let (installed, failed) = st.plugins().install_missing(&[]).await;
    for id in installed {
        parts.push(format!("{} installed", st.plugins().manifest(&id).map(|m| m.plugin.name).unwrap_or(id)));
    }
    for f in failed {
        errors.push(format!("{}: {}", f["id"].as_str().unwrap_or(""), f["error"].as_str().unwrap_or("").lines().next().unwrap_or("")));
    }
    st.plugins().check_all().await;
    for row in st.plugins().rows() {
        use encore_db::RowExt;
        let id = row.s("id");
        let Some(card) = st.plugins().card(&id) else { continue };
        if !card["can_update"].as_bool().unwrap_or(false) {
            continue;
        }
        let seen = Seen::of(&card);
        let wanted = id != "streamrip" || st.cfg().bool("streamrip_enabled");
        let auto = task_auto(&id, row.b("auto_update"), wanted, env_allows_ytdlp()) && !custom_copy(st, &id);
        match plan(&seen, auto) {
            Plan::Say(s) => parts.push(s),
            Plan::Error(e) => errors.push(e),
            Plan::Install { from, to } => match st.plugins().update_unattended(&id).await {
                Ok(c) => parts.push(format!("{} {} → {}", seen.name, from.unwrap_or_else(|| "not installed".into()), c["installed_version"].as_str().map(String::from).unwrap_or(to))),
                Err(e) => errors.push(format!("{}: {}", seen.name, e.message.lines().next().unwrap_or(""))),
            },
        }
    }
    st.system().tools.forget_versions();
    st.playback().cadenza.forget();
    if !errors.is_empty() {
        if parts.is_empty() {
            anyhow::bail!("{}", errors.join(" · "));
        }
        parts.extend(errors);
    }
    if parts.is_empty() {
        return Ok("No plugins installed".into());
    }
    Ok(parts.join(" · "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seen(name: &str, installed: Option<&str>, latest: Option<&str>) -> Seen {
        let update_available = latest.is_some_and(|l| installed.map(|i| newer(l, i)).unwrap_or(true));
        Seen { name: name.into(), installed: installed.map(String::from), latest: latest.map(String::from), update_available, error: None, can_update: true }
    }

    #[test]
    fn scheduled_task_reports_a_switched_off_streamrip_without_installing() {
        assert!(!task_auto("streamrip", true, false, true));
        assert!(task_auto("streamrip", true, true, true));
        assert!(!task_auto("ytdlp", true, true, false));
        assert!(task_auto("cadenza", true, true, false));
        assert!(!task_auto("slskd", false, true, true));
        let t = seen("streamrip", Some("2.4.9"), Some("2.5.0"));
        assert_eq!(plan(&t, task_auto("streamrip", true, false, true)), Plan::Say("streamrip 2.5.0 available".into()));
        let t = seen("streamrip", Some("2.5.0"), Some("2.5.0"));
        assert_eq!(plan(&t, false), Plan::Say("streamrip up to date".into()));
    }

    #[test]
    fn scheduled_plan_per_plugin() {
        let c = seen("Cadenza (server processing)", Some("2.0.0"), Some("2.1.0"));
        assert_eq!(plan(&c, false), Plan::Say("Cadenza (server processing) 2.1.0 available".into()));
        assert_eq!(plan(&c, true), Plan::Install { from: Some("2.0.0".into()), to: "2.1.0".into() });
        assert_eq!(plan(&seen("Cadenza (server processing)", Some("2.1.0"), Some("2.1.0")), true), Plan::Say("Cadenza (server processing) up to date".into()));
        let mut y = seen("yt-dlp", Some("2026.10.09"), Some("2026.10.14"));
        assert_eq!(plan(&y, true), Plan::Install { from: Some("2026.10.09".into()), to: "2026.10.14".into() });
        y.error = Some("Could not check for a new version: timeout".into());
        assert!(matches!(plan(&y, true), Plan::Error(e) if e.starts_with("yt-dlp: Could not check")));
        // an update that failed last time is tried again
        y.error = Some("Update failed: no network".into());
        assert!(matches!(plan(&y, true), Plan::Install { .. }));
        // nowhere to update from: reported only
        let mut s = seen("Soulseek (slskd)", Some("0.24.0"), Some("0.24.1"));
        s.can_update = false;
        assert_eq!(plan(&s, true), Plan::Say("Soulseek (slskd) 0.24.1 available".into()));
        assert_eq!(Seen::of(&json!({ "name": "x", "installed_version": "1.0", "latest": "1.1", "update_available": true, "last_error": null, "can_update": true })),
            Seen { name: "x".into(), installed: Some("1.0".into()), latest: Some("1.1".into()), update_available: true, error: None, can_update: true });
    }

    /// A script written under a temporary name and renamed into place, so no open write handle
    /// (one a parallel test's fork may have inherited) points at the file that is run.
    fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(dir).unwrap();
        let p = dir.join(name);
        let tmp = dir.join(format!(".{name}.tmp"));
        std::fs::write(&tmp, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::rename(&tmp, &p).unwrap();
        p
    }

    fn fake(dir: &Path, name: &str, out: &str) -> PathBuf {
        script(dir, name, &format!("echo '{out}'"))
    }

    #[test]
    fn versions_are_asked_through_the_spawn_builder() {
        let root = std::env::temp_dir().join(format!("encore-tools-{}-{}", std::process::id(), encore_core::ids::random_hex(4)));
        let t = Tools::default();
        let o = fake(&root, "yt-dlp", "2026.10.14");
        assert_eq!(t.version_of(&o).as_deref(), Some("2026.10.14"));
        // a session of its own: no controlling terminal
        let tty = script(&root, "tty-check", "if (exec </dev/tty) 2>/dev/null; then echo 'has 1.0'; else echo 'none 2.0'; fi");
        assert_eq!(t.version_of(&tty).as_deref(), Some("2.0"));
        let broken = script(&root, "broken", "exit 1");
        assert_eq!(t.version_of(&broken), None);
        assert_eq!(t.version_of(&root.join("missing")), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn expired_versions_are_served_while_refreshing() {
        let root = std::env::temp_dir().join(format!("encore-tools-ttl-{}-{}", std::process::id(), encore_core::ids::random_hex(4)));
        let exe = fake(&root, "yt-dlp", "2026.10.14");
        let t = Tools::default();
        assert_eq!(t.version_of(&exe).as_deref(), Some("2026.10.14"));
        let key: ProbeKey = (exe.clone(), "--version");
        // age the entry: the old answer comes back at once and a background probe stores a fresh one
        {
            let mut map = t.0.answers.lock().unwrap();
            let e = map.get_mut(&key).unwrap();
            e.0 = Instant::now() - VERSION_TTL - Duration::from_secs(1);
            e.2 = Some(Answer { ok: true, stdout: "2026.10.09\n".into(), stderr: String::new() });
        }
        assert_eq!(t.version_of(&exe).as_deref(), Some("2026.10.09"));
        let mut fresh = None;
        for _ in 0..100 {
            if !t.0.refreshing.lock().unwrap().contains(&key) {
                fresh = t.0.answers.lock().unwrap().get(&key).and_then(|e| e.2.clone()).and_then(|a| extract_version(&a.stdout));
                if fresh.as_deref() == Some("2026.10.14") {
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(fresh.as_deref(), Some("2026.10.14"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn old_sources_on_the_card() {
        assert_eq!([CopyFrom::Plugin, CopyFrom::Adopted, CopyFrom::Bundled, CopyFrom::System].map(CopyFrom::source), ["updated", "updated", "bundled", "system"]);
        assert_eq!(IDS.map(label), ["yt-dlp", "streamrip", "Cadenza", "slskd", "Translator"]);
    }
}
