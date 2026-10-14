//! Download tools that update without a new Encore image: yt-dlp and streamrip are pip-installed into a
//! persistent venv under the config dir (`<config>/tools/venv`), made with the image's own Python. Encore
//! runs whichever copy is newer – that overlay or the one bundled in the image (/opt/tools) – so an image
//! that ships a newer version wins by itself. Soulseek (built in) and slskd (its own container) are listed
//! for information only.
use crate::state::AppState;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant, SystemTime};

/// Where the image installs its tools.
pub const BUNDLED_DIR: &str = "/opt/tools/bin";
const STREAMRIP_REPO: &str = "Stensel8/streamrip";
const PYPI_YTDLP: &str = "https://pypi.org/pypi/yt-dlp/json";
/// How long a `--version` answer is trusted.
const VERSION_TTL: Duration = Duration::from_secs(600);
/// How long the latest versions found by a check are kept (shown again without asking the network).
const LATEST_TTL: u64 = 30 * 86400;
const PIP_TIMEOUT: Duration = Duration::from_secs(600);
/// Stamp inside the overlay venv: `--version` of the Python that built it.
const STAMP: &str = ".encore-python";

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ToolInfo {
    pub id: String,
    pub name: String,
    pub installed: Option<String>,
    /// "bundled" | "updated" | "system" | "missing"
    pub source: String,
    pub latest: Option<String>,
    pub update_available: bool,
    pub checked_at: Option<f64>,
    pub error: Option<String>,
    pub note: String,
}

// ---- versions -----------------------------------------------------------------------------------

/// The numeric dot parts of a version ("v2.4.10" → [2, 4, 10], "rip, version 2.4.9" → [2, 4, 9]).
fn parts(v: &str) -> Vec<u64> {
    let Some(s) = extract_version(v) else { return vec![] };
    s.split('.').map(|p| p.chars().take_while(|c| c.is_ascii_digit()).collect::<String>().parse().unwrap_or(0)).collect()
}

/// The first dotted version number in a tool's output ("rip, version 2.4.9" → "2.4.9", "v2.5.0" → "2.5.0").
pub fn extract_version(text: &str) -> Option<String> {
    let re = regex::Regex::new(r"\d+(?:\.\d+)+").ok()?;
    re.find(text).map(|m| m.as_str().to_string()).or_else(|| {
        let t = text.trim().trim_start_matches(['v', 'V']);
        (!t.is_empty() && t.chars().all(|c| c.is_ascii_digit())).then(|| t.to_string())
    })
}

/// Compare two versions by their numeric dot parts (missing parts count as 0).
pub fn compare(a: &str, b: &str) -> std::cmp::Ordering {
    let (x, y) = (parts(a), parts(b));
    for i in 0..x.len().max(y.len()) {
        let o = x.get(i).unwrap_or(&0).cmp(y.get(i).unwrap_or(&0));
        if o != std::cmp::Ordering::Equal {
            return o;
        }
    }
    std::cmp::Ordering::Equal
}

pub fn newer(latest: &str, installed: &str) -> bool {
    !parts(latest).is_empty() && compare(latest, installed) == std::cmp::Ordering::Greater
}

type VersionCache = HashMap<PathBuf, (Instant, Option<SystemTime>, Option<String>)>;
static VERSIONS: LazyLock<Mutex<VersionCache>> = LazyLock::new(Default::default);
/// One lock per path, so callers that arrive together run a probe once and share its answer.
static GATES: LazyLock<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>> = LazyLock::new(Default::default);
/// Paths with a refresh running in the background.
static REFRESHING: LazyLock<Mutex<HashSet<PathBuf>>> = LazyLock::new(Default::default);
/// Bumped by forget_versions: a probe that started before it does not store its (old) answer.
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// Run `<exe> --version` (stdout and stderr). A file that is still being written ("Text file busy")
/// is tried again a few times.
fn probe(exe: &Path) -> Option<String> {
    let mut tries = 0;
    let o = loop {
        match std::process::Command::new(exe).arg("--version").stdin(std::process::Stdio::null()).output() {
            Err(e) if e.raw_os_error() == Some(26) && tries < 5 => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(50 * tries));
            }
            r => break r.ok()?,
        }
    };
    if !o.status.success() {
        return None;
    }
    extract_version(&String::from_utf8_lossy(&[o.stdout, o.stderr].concat()))
}

fn mtime_of(exe: &Path) -> Option<SystemTime> {
    std::fs::metadata(exe).and_then(|m| m.modified()).ok()
}

/// Probe and store, unless another caller just did (then its answer is used).
fn probe_and_store(exe: &Path, mtime: Option<SystemTime>) -> Option<String> {
    let gate = GATES.lock().unwrap().entry(exe.to_path_buf()).or_default().clone();
    let _g = gate.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, mt, v)) = VERSIONS.lock().unwrap().get(exe) {
        if at.elapsed() < VERSION_TTL && *mt == mtime {
            return v.clone();
        }
    }
    let generation = GENERATION.load(Ordering::SeqCst);
    let v = probe(exe);
    let mut map = VERSIONS.lock().unwrap();
    if GENERATION.load(Ordering::SeqCst) == generation {
        map.insert(exe.to_path_buf(), (Instant::now(), mtime, v.clone()));
    }
    v
}

/// `<exe> --version`, at most once per 10 minutes per path. Once known, an expired answer is still
/// returned at once while a background thread asks again, so async callers are not held up; only the
/// first look at a path (or one whose file changed) waits for the probe.
fn version_of(exe: &Path) -> Option<String> {
    let mtime = mtime_of(exe);
    let stale = match VERSIONS.lock().unwrap().get(exe) {
        Some((at, mt, v)) if *mt == mtime => {
            if at.elapsed() < VERSION_TTL {
                return v.clone();
            }
            Some(v.clone())
        }
        _ => None,
    };
    let Some(old) = stale else { return probe_and_store(exe, mtime) };
    if REFRESHING.lock().unwrap().insert(exe.to_path_buf()) {
        let p = exe.to_path_buf();
        let spawned = std::thread::Builder::new().name("tool-version".into()).spawn(move || {
            probe_and_store(&p, mtime_of(&p));
            REFRESHING.lock().unwrap().remove(&p);
        });
        if spawned.is_err() {
            REFRESHING.lock().unwrap().remove(exe);
            return probe_and_store(exe, mtime);
        }
    }
    old
}

fn forget_versions() {
    GENERATION.fetch_add(1, Ordering::SeqCst);
    VERSIONS.lock().unwrap().clear();
}

// ---- where the tools are ------------------------------------------------------------------------

fn which(name: &str) -> Option<PathBuf> {
    if name.contains('/') {
        return Path::new(name).is_file().then(|| PathBuf::from(name));
    }
    std::env::var("PATH").ok()?.split(':').filter(|d| !d.is_empty()).map(|d| Path::new(d).join(name)).find(|p| p.is_file())
}

/// The executable each tool installs.
fn exes(id: &str) -> &'static [&'static str] {
    if id == "streamrip" { &["rip", "streamrip"] } else { &["yt-dlp"] }
}

/// A tool's executable in one bin folder.
fn in_dir(dir: &Path, id: &str) -> Option<PathBuf> {
    exes(id).iter().map(|n| dir.join(n)).find(|p| p.is_file())
}

fn overlay_bin(data: &Path) -> PathBuf {
    venv_dir(data).join("bin")
}

fn label(id: &str) -> &'static str {
    match id {
        "ytdlp" => "yt-dlp",
        "streamrip" => "streamrip",
        "soulseek" => "Soulseek",
        _ => "slskd",
    }
}

pub fn venv_dir(data: &Path) -> PathBuf {
    data.join("tools").join("venv")
}

/// The config dir when no AppState is at hand (same rule as the server's start-up).
pub fn config_dir() -> PathBuf {
    PathBuf::from(std::env::var("ENCORE_CONFIG_DIR").unwrap_or_else(|_| "/config".into()))
}

/// The Python the overlay venv is built with: the image's, or the system's on bare-metal installs.
fn base_python() -> PathBuf {
    let p = Path::new(BUNDLED_DIR).join("python");
    if p.is_file() { p } else { which("python3").unwrap_or_else(|| PathBuf::from("python3")) }
}

/// The overlay is only used while it was built by the same Python as the image's (an image with a new
/// Python would otherwise run packages built for the old one).
fn stamp_matches(venv: &Path, python_version: Option<&str>) -> bool {
    let Ok(stamp) = std::fs::read_to_string(venv.join(STAMP)) else { return false };
    python_version.is_some_and(|v| stamp.trim() == v.trim())
}

fn python_version(py: &Path) -> Option<String> {
    version_of(py)
}

fn overlay_ok(data: &Path) -> bool {
    stamp_matches(&venv_dir(data), python_version(&base_python()).as_deref())
}

/// The copy that ships with the image or the system: /opt/tools/bin first, then PATH.
fn base_copy(id: &str) -> Option<PathBuf> {
    if let Some(bundled) = in_dir(Path::new(BUNDLED_DIR), id) {
        return Some(bundled);
    }
    if id == "streamrip" { which("streamrip").or_else(|| which("rip")) } else { which("yt-dlp") }
}

/// Overlay copy when it runs and is at least as new as the base copy; otherwise the base copy.
fn pick(overlay: Option<&Path>, base: Option<&Path>) -> Option<(PathBuf, Option<String>, bool)> {
    let base_v = base.map(|b| (b.to_path_buf(), version_of(b)));
    if let Some(o) = overlay.filter(|o| o.is_file()) {
        if let Some(ov) = version_of(o) {
            let wins = match &base_v {
                Some((_, Some(bv))) => compare(&ov, bv) != std::cmp::Ordering::Less,
                _ => true,
            };
            if wins {
                return Some((o.to_path_buf(), Some(ov), true));
            }
        }
    }
    base_v.map(|(p, v)| (p, v, false))
}

/// (path, version, from the overlay) for "ytdlp" / "streamrip", given the config dir.
fn resolve(data: &Path, id: &str) -> Option<(PathBuf, Option<String>, bool)> {
    let overlay = in_dir(&overlay_bin(data), id).filter(|_| overlay_ok(data));
    pick(overlay.as_deref(), base_copy(id).as_deref())
}

/// The executable to run for a tool; for streamrip, the streamrip_bin setting still wins when set.
pub fn path(st: &AppState, id: &str) -> Option<PathBuf> {
    if id == "streamrip" {
        let custom = st.cfg.str("streamrip_bin");
        if !custom.is_empty() {
            return which(&custom);
        }
    }
    path_in(st.data_dir(), id)
}

pub fn path_in(data: &Path, id: &str) -> Option<PathBuf> {
    resolve(data, id).map(|(p, _, _)| p)
}

fn source_of(p: &Path, overlay: bool) -> &'static str {
    if overlay { "updated" } else if p.starts_with(BUNDLED_DIR) { "bundled" } else { "system" }
}

// ---- latest versions ----------------------------------------------------------------------------

pub fn parse_pypi(v: &Value) -> Option<String> {
    v["info"]["version"].as_str().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// GitHub release or tag JSON → (tag as published, version without the leading v). For a list of tags
/// (not sorted by version) the highest version wins; names without a version number are skipped.
pub fn parse_github(v: &Value) -> Option<(String, String)> {
    let tag = if let Some(list) = v.as_array() {
        list.iter().filter_map(|t| t["name"].as_str()).map(str::trim).filter(|n| !parts(n).is_empty())
            .max_by(|a, b| compare(a, b))?.to_string()
    } else {
        v["tag_name"].as_str().or_else(|| v["name"].as_str())?.trim().to_string()
    };
    if tag.is_empty() {
        return None;
    }
    let ver = tag.trim_start_matches(['v', 'V']).to_string();
    Some((tag, ver))
}

async fn get_json(st: &AppState, url: &str) -> anyhow::Result<(u16, Value)> {
    let r = st.http.get(url).header("Accept", "application/vnd.github+json").timeout(Duration::from_secs(20)).send().await?;
    let code = r.status().as_u16();
    let v = r.json::<Value>().await.unwrap_or(Value::Null);
    Ok((code, v))
}

async fn github_latest(st: &AppState, repo: &str, tags_fallback: bool) -> anyhow::Result<(String, String)> {
    let (code, v) = get_json(st, &format!("https://api.github.com/repos/{repo}/releases/latest")).await?;
    if code < 400 {
        if let Some(t) = parse_github(&v) {
            return Ok(t);
        }
    }
    if tags_fallback && (code == 404 || code < 400) {
        let (code, v) = get_json(st, &format!("https://api.github.com/repos/{repo}/tags")).await?;
        if code < 400 {
            if let Some(t) = parse_github(&v) {
                return Ok(t);
            }
        }
        anyhow::bail!("GitHub returned {code} for {repo} tags");
    }
    anyhow::bail!("GitHub returned {code} for the latest {repo} release: {}", v["message"].as_str().unwrap_or(""))
}

/// Fetch the newest version of one tool: (version, tag/spec detail).
async fn fetch_latest(st: &AppState, id: &str) -> anyhow::Result<(String, String)> {
    match id {
        "ytdlp" => {
            let (code, v) = get_json(st, PYPI_YTDLP).await?;
            let ver = parse_pypi(&v).ok_or_else(|| anyhow::anyhow!("PyPI returned {code} without a yt-dlp version"))?;
            Ok((ver.clone(), ver))
        }
        "streamrip" => github_latest(st, STREAMRIP_REPO, true).await.map(|(tag, ver)| (ver, tag)),
        _ => github_latest(st, "slskd/slskd", false).await.map(|(tag, ver)| (ver, tag)),
    }
}

fn latest_key(id: &str) -> String {
    format!("tools:latest:{id}")
}

/// What the last check found for a tool: {latest, tag, checked_at, error}.
fn last_check(st: &AppState, id: &str) -> Value {
    st.cache_get(&latest_key(id)).unwrap_or(Value::Null)
}

async fn refresh_latest(st: &AppState, id: &str) -> Value {
    let v = match fetch_latest(st, id).await {
        Ok((ver, tag)) => json!({ "latest": ver, "tag": tag, "checked_at": crate::db::now() }),
        Err(e) => {
            // keep what an earlier check found, with the error beside it
            let mut old = last_check(st, id);
            if !old.is_object() {
                old = json!({});
            }
            old["error"] = json!(format!("Could not check for a new version: {e}"));
            old["checked_at"] = json!(crate::db::now());
            old
        }
    };
    st.cache_put(&latest_key(id), v.clone(), LATEST_TTL);
    v
}

// ---- the listing --------------------------------------------------------------------------------

/// ENCORE_YTDLP: the yt-dlp Encore runs instead of its own (see sources::ytdlp::bin).
fn ytdlp_env() -> Option<String> {
    std::env::var("ENCORE_YTDLP").ok().filter(|v| !v.trim().is_empty())
}

/// A copy chosen by the owner (streamrip_bin setting, ENCORE_YTDLP) – Encore leaves it alone.
fn custom_copy(st: &AppState, id: &str) -> bool {
    match id {
        "streamrip" => !st.cfg.str("streamrip_bin").is_empty(),
        "ytdlp" => ytdlp_env().is_some(),
        _ => false,
    }
}

fn local_info(st: &AppState, id: &str) -> ToolInfo {
    let custom = custom_copy(st, id);
    let found = if !custom {
        resolve(st.data_dir(), id)
    } else if id == "ytdlp" {
        ytdlp_env().and_then(|e| which(&e)).map(|p| { let v = version_of(&p); (p, v, false) })
    } else {
        path(st, id).map(|p| { let v = version_of(&p); (p, v, false) })
    };
    let (installed, source) = match &found {
        Some((_, v, _)) if custom => (v.clone(), "system"),
        Some((p, v, overlay)) => (v.clone(), source_of(p, *overlay)),
        None => (None, "missing"),
    };
    let last = last_check(st, id);
    let latest = last["latest"].as_str().map(String::from);
    let update_available = !custom && latest.as_deref().is_some_and(|l| installed.as_deref().map(|i| newer(l, i)).unwrap_or(true));
    let note = match (custom, id) {
        (true, "ytdlp") => "Uses the yt-dlp set by ENCORE_YTDLP – Encore does not update that copy".to_string(),
        (true, _) => "Uses the streamrip set under streamrip → Path – Encore does not update that copy".to_string(),
        _ => String::new(),
    };
    ToolInfo { id: id.into(), name: label(id).into(), installed, source: source.into(), latest, update_available,
        checked_at: last["checked_at"].as_f64(), error: last["error"].as_str().map(String::from), note }
}

fn soulseek_info() -> ToolInfo {
    ToolInfo { id: "soulseek".into(), name: label("soulseek").into(), installed: Some(crate::VERSION.into()), source: "bundled".into(), latest: None,
        update_available: false, checked_at: None, error: None, note: "Built into Encore – updates with Encore".into() }
}

fn slskd_wanted(st: &AppState) -> bool {
    crate::soulseek::mode(st) == "slskd" && crate::sources::slskd::enabled(st)
}

fn slskd_info(st: &AppState) -> ToolInfo {
    let last = last_check(st, "slskd");
    let installed = st.cache_get("tools:slskd:installed").and_then(|v| v.as_str().map(String::from));
    let latest = last["latest"].as_str().map(String::from);
    let update_available = matches!((&latest, &installed), (Some(l), Some(i)) if newer(l, i));
    ToolInfo { id: "slskd".into(), name: label("slskd").into(), source: if installed.is_some() { "system" } else { "missing" }.into(), installed, latest,
        update_available, checked_at: last["checked_at"].as_f64(), error: last["error"].as_str().map(String::from),
        note: "Runs in its own container – update it by pulling the slskd image".into() }
}

/// slskd's own version, from its API.
async fn slskd_version(st: &AppState) -> anyhow::Result<String> {
    let url = format!("{}/api/v0/application", st.cfg.str("slskd_url").trim_end_matches('/'));
    let r = st.http.get(&url).header("X-API-Key", st.cfg.str("slskd_api_key")).timeout(Duration::from_secs(10)).send().await
        .map_err(|e| anyhow::anyhow!("slskd unreachable at {url}: {e}"))?;
    let code = r.status().as_u16();
    let v: Value = r.json().await.unwrap_or(Value::Null);
    let ver = v["version"]["current"].as_str().or(v["version"]["full"].as_str()).or(v["version"].as_str()).and_then(extract_version);
    ver.ok_or_else(|| anyhow::anyhow!("slskd returned {code} without a version"))
}

/// The listing from what is known now (no network).
pub fn cached(st: &AppState) -> Vec<ToolInfo> {
    let mut out = vec![local_info(st, "ytdlp"), local_info(st, "streamrip"), soulseek_info()];
    if slskd_wanted(st) {
        out.push(slskd_info(st));
    }
    out
}

/// Look up the newest versions (PyPI, GitHub) and the installed ones. An image that ships a newer copy
/// than the overlay makes the overlay copy redundant: it is removed.
pub async fn check(st: &AppState) -> Vec<ToolInfo> {
    tokio::join!(refresh_latest(st, "ytdlp"), refresh_latest(st, "streamrip"));
    if slskd_wanted(st) {
        refresh_latest(st, "slskd").await;
        match slskd_version(st).await {
            Ok(v) => st.cache_put("tools:slskd:installed", json!(v), LATEST_TTL),
            Err(e) => {
                st.cache.lock().unwrap().remove("tools:slskd:installed");
                let mut last = last_check(st, "slskd");
                if last.is_object() {
                    last["error"] = json!(e.to_string());
                    st.cache_put(&latest_key("slskd"), last, LATEST_TTL);
                }
            }
        }
    }
    for id in ["ytdlp", "streamrip"] {
        let st2 = st.clone();
        let _ = tokio::task::spawn_blocking(move || drop_stale_overlay(&st2, id)).await;
    }
    let st2 = st.clone();
    tokio::task::spawn_blocking(move || cached(&st2)).await.unwrap_or_default()
}

/// The overlay copy is older than the bundled one (a newer image): uninstall it.
fn drop_stale_overlay(st: &AppState, id: &str) {
    let Some(overlay) = in_dir(&overlay_bin(st.data_dir()), id) else { return };
    if UPDATING.load(Ordering::SeqCst) {
        return;
    }
    let Some(base) = in_dir(Path::new(BUNDLED_DIR), id) else { return };
    if let (Some(ov), Some(bv)) = (version_of(&overlay), version_of(&base)) {
        if compare(&bv, &ov) == std::cmp::Ordering::Greater {
            match reset(st, id) {
                Ok(_) => st.db.log("info", &format!("The image ships {} {bv}, newer than the updated {ov}: using the bundled one", label(id))),
                Err(e) => tracing::warn!("removing the old {} overlay failed: {e}", label(id)),
            }
        }
    }
}

// ---- updating -----------------------------------------------------------------------------------

static UPDATING: AtomicBool = AtomicBool::new(false);

struct Busy;
impl Drop for Busy {
    fn drop(&mut self) {
        UPDATING.store(false, Ordering::SeqCst);
    }
}

fn tail(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

async fn run(cmd: &Path, args: &[&str], limit: Duration) -> anyhow::Result<std::process::Output> {
    let fut = tokio::process::Command::new(cmd).args(args).env("PIP_NO_INPUT", "1").stdin(std::process::Stdio::null()).kill_on_drop(true).output();
    match tokio::time::timeout(limit, fut).await {
        Ok(o) => Ok(o.map_err(|e| anyhow::anyhow!("could not run {}: {e}", cmd.display()))?),
        Err(_) => anyhow::bail!("{} took longer than {} minutes and was stopped", cmd.display(), limit.as_secs() / 60),
    }
}

fn failed(what: &str, o: &std::process::Output) -> anyhow::Error {
    let text = String::from_utf8_lossy(&[o.stdout.clone(), o.stderr.clone()].concat()).to_string();
    let t = tail(&text, 15);
    anyhow::anyhow!("{what} failed{}", if t.is_empty() { String::new() } else { format!(":\n{t}") })
}

/// Create the overlay venv when it is missing or was built by another Python.
async fn ensure_venv(data: &Path) -> anyhow::Result<PathBuf> {
    let venv = venv_dir(data);
    let py = base_python();
    let pyv = {
        let p = py.clone();
        tokio::task::spawn_blocking(move || python_version(&p)).await?
    }.ok_or_else(|| anyhow::anyhow!("No Python found to install the update with ({} is missing)", py.display()))?;
    if venv.exists() && !stamp_matches(&venv, Some(&pyv)) {
        tokio::fs::remove_dir_all(&venv).await?;
    }
    let vpy = venv.join("bin").join("python");
    if !vpy.is_file() {
        tokio::fs::create_dir_all(venv.parent().unwrap()).await?;
        let o = run(&py, &["-m", "venv", &venv.to_string_lossy()], Duration::from_secs(120)).await?;
        if !o.status.success() {
            let e = failed("Creating the tools folder", &o);
            let _ = tokio::fs::remove_dir_all(&venv).await;
            return Err(e);
        }
        // some Pythons make venvs without pip
        let has_pip = run(&vpy, &["-m", "pip", "--version"], Duration::from_secs(60)).await.map(|o| o.status.success()).unwrap_or(false);
        if !has_pip {
            let o = run(&vpy, &["-m", "ensurepip", "--upgrade"], Duration::from_secs(180)).await?;
            if !o.status.success() {
                let e = failed("Setting up pip", &o);
                let _ = tokio::fs::remove_dir_all(&venv).await;
                return Err(e);
            }
        }
        tokio::fs::write(venv.join(STAMP), format!("{pyv}\n")).await?;
    }
    Ok(vpy)
}

/// The pip requirement for a tool's update.
pub fn spec(id: &str, tag: &str) -> String {
    match id {
        "streamrip" => format!("streamrip @ https://github.com/{STREAMRIP_REPO}/archive/refs/tags/{tag}.zip"),
        _ => "yt-dlp[default]".into(),
    }
}

/// Install the newest version of yt-dlp or streamrip into the overlay venv (as the Encore user, no root).
/// The work runs in its own task: a caller that goes away (a closed browser, a proxy timeout) stops
/// only the waiting, never pip halfway through an install.
pub async fn update(st: &AppState, id: &str) -> anyhow::Result<ToolInfo> {
    let (st, id) = (st.clone(), id.to_string());
    tokio::spawn(async move { update_now(&st, &id).await }).await?
}

async fn update_now(st: &AppState, id: &str) -> anyhow::Result<ToolInfo> {
    if id != "ytdlp" && id != "streamrip" {
        anyhow::bail!("{} can't be updated from Encore", label(id));
    }
    if UPDATING.swap(true, Ordering::SeqCst) {
        anyhow::bail!("An update is already running");
    }
    let _busy = Busy;
    let mut last = last_check(st, id);
    if last["tag"].as_str().is_none() {
        last = refresh_latest(st, id).await;
    }
    let Some(tag) = last["tag"].as_str().map(String::from) else {
        anyhow::bail!("{}", last["error"].as_str().unwrap_or("Could not find the newest version"));
    };
    let vpy = ensure_venv(st.data_dir()).await?;
    let spec = spec(id, &tag);
    let o = run(&vpy, &["-m", "pip", "install", "-q", "-U", "--disable-pip-version-check", "--no-cache-dir", &spec], PIP_TIMEOUT).await;
    forget_versions();
    let o = o?;
    if !o.status.success() {
        let e = failed(&format!("Updating {}", label(id)), &o);
        st.db.log("warning", &e.to_string());
        return Err(e);
    }
    let Some(bin) = in_dir(&overlay_bin(st.data_dir()), id) else {
        anyhow::bail!("{} was installed but its program is missing – Encore keeps using the bundled one", label(id));
    };
    let ver = run(&bin, &["--version"], Duration::from_secs(60)).await.ok()
        .filter(|o| o.status.success())
        .and_then(|o| extract_version(&String::from_utf8_lossy(&[o.stdout, o.stderr].concat())));
    let Some(ver) = ver else {
        anyhow::bail!("{} was installed but does not start – Encore keeps using the bundled one", label(id));
    };
    st.db.log("info", &format!("{} updated to {ver}", label(id)));
    let st2 = st.clone();
    let id2 = id.to_string();
    Ok(tokio::task::spawn_blocking(move || local_info(&st2, &id2)).await?)
}

/// "Use the bundled version": remove the overlay copy of a tool.
pub fn reset(st: &AppState, id: &str) -> anyhow::Result<()> {
    let pkg = match id {
        "ytdlp" => "yt-dlp",
        "streamrip" => "streamrip",
        _ => anyhow::bail!("{} has no updated copy to remove", label(id)),
    };
    // never uninstall while pip installs into the same venv
    if UPDATING.swap(true, Ordering::SeqCst) {
        anyhow::bail!("An update is already running");
    }
    let _busy = Busy;
    let venv = venv_dir(st.data_dir());
    let vpy = venv.join("bin").join("python");
    if vpy.is_file() {
        let o = std::process::Command::new(&vpy).args(["-m", "pip", "uninstall", "-y", "-q", "--disable-pip-version-check", pkg])
            .stdin(std::process::Stdio::null()).output();
        forget_versions();
        let o = o?;
        if !o.status.success() && in_dir(&venv.join("bin"), id).is_some() {
            return Err(failed(&format!("Removing the updated {}", label(id)), &o));
        }
    }
    // a leftover script without its package would still be found
    for n in exes(id) {
        let _ = std::fs::remove_file(venv.join("bin").join(n));
    }
    forget_versions();
    Ok(())
}

/// YTDLP_AUTO_UPDATE=false (the old start-up switch) keeps yt-dlp from updating by itself.
fn env_allows_ytdlp() -> bool {
    !std::env::var("YTDLP_AUTO_UPDATE").is_ok_and(|v| v.trim().eq_ignore_ascii_case("false"))
}

/// Scheduled task: check, then update what's switched to update by itself.
pub async fn scheduled(st: &AppState) -> anyhow::Result<String> {
    let list = check(st).await;
    let mut parts = vec![];
    let mut errors = vec![];
    for id in ["ytdlp", "streamrip"] {
        let Some(t) = list.iter().find(|t| t.id == id) else { continue };
        let auto = match id {
            "ytdlp" => st.cfg.bool("tools_auto_ytdlp") && env_allows_ytdlp(),
            _ => st.cfg.bool("tools_auto_streamrip"),
        };
        let name = label(id);
        // installing a tool the owner never set up is left to the Update button
        if t.source == "missing" || (id == "streamrip" && !st.cfg.bool("streamrip_enabled")) {
            parts.push(format!("{name} not installed"));
            continue;
        }
        if let Some(e) = &t.error {
            errors.push(format!("{name}: {e}"));
            continue;
        }
        if !t.update_available {
            parts.push(format!("{name} up to date"));
            continue;
        }
        let from = t.installed.clone().unwrap_or_else(|| "not installed".into());
        let to = t.latest.clone().unwrap_or_default();
        if !auto || custom_copy(st, id) {
            parts.push(format!("{name} {to} available"));
            continue;
        }
        match update(st, id).await {
            Ok(n) => parts.push(format!("{name} {from} → {}", n.installed.unwrap_or(to))),
            Err(e) => errors.push(format!("{name}: {}", e.to_string().lines().next().unwrap_or(""))),
        }
    }
    if !errors.is_empty() {
        if parts.is_empty() {
            anyhow::bail!("{}", errors.join(" · "));
        }
        parts.extend(errors);
    }
    Ok(parts.join(" · "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering::*;

    #[test]
    fn versions_compare_by_number() {
        assert_eq!(compare("2026.10.09", "2026.9.30"), Greater);
        assert_eq!(compare("2.4.9", "2.4.10"), Less);
        assert_eq!(compare("v2.5.0", "2.4.10"), Greater);
        assert_eq!(compare("v2.5.0", "2.5"), Equal);
        assert!(newer("2026.10.14", "2026.10.09"));
        assert!(!newer("2.4.9", "2.4.9"));
        assert!(!newer("", "2.4.9"));
        assert_eq!(extract_version("rip, version 2.4.9\n").as_deref(), Some("2.4.9"));
        assert_eq!(extract_version("v2.5.0").as_deref(), Some("2.5.0"));
        assert_eq!(extract_version("Python 3.14.0").as_deref(), Some("3.14.0"));
    }

    #[test]
    fn parses_pypi_and_github() {
        let pypi = json!({ "info": { "version": "2026.10.14", "name": "yt-dlp" }, "releases": {} });
        assert_eq!(parse_pypi(&pypi).as_deref(), Some("2026.10.14"));
        assert_eq!(parse_pypi(&json!({ "message": "Not Found" })), None);
        let rel = json!({ "tag_name": "v2.5.0", "name": "streamrip 2.5.0" });
        assert_eq!(parse_github(&rel), Some(("v2.5.0".into(), "2.5.0".into())));
        let tags = json!([{ "name": "v2.4.10", "commit": {} }, { "name": "v2.4.9" }]);
        assert_eq!(parse_github(&tags), Some(("v2.4.10".into(), "2.4.10".into())));
        // tags are not sorted by version, and some are not versions at all
        let tags = json!([{ "name": "nightly" }, { "name": "v2.4.9" }, { "name": "v2.4.10" }, { "name": "v2.3" }]);
        assert_eq!(parse_github(&tags), Some(("v2.4.10".into(), "2.4.10".into())));
        assert_eq!(parse_github(&json!([{ "name": "latest" }])), None);
        assert_eq!(parse_github(&json!({ "message": "Not Found" })), None);
        assert_eq!(parse_github(&json!([])), None);
        assert!(spec("streamrip", "v2.5.0").ends_with("/archive/refs/tags/v2.5.0.zip"));
        assert_eq!(spec("ytdlp", "2026.10.14"), "yt-dlp[default]");
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
    fn picks_the_newer_copy() {
        let root = std::env::temp_dir().join(format!("encore-tools-{}-{}", std::process::id(), crate::auth::random_hex(4)));
        let (ov, bu) = (root.join("overlay"), root.join("bundled"));
        // overlay newer
        let o = fake(&ov, "yt-dlp", "2026.10.14");
        let b = fake(&bu, "yt-dlp", "2026.10.09");
        let (p, v, from_overlay) = pick(Some(&o), Some(&b)).unwrap();
        assert_eq!((p, v.as_deref(), from_overlay), (o.clone(), Some("2026.10.14"), true));
        // bundled newer (a newer image)
        let o2 = fake(&ov, "rip", "rip, version 2.4.9");
        let b2 = fake(&bu, "rip", "rip, version 2.4.10");
        let (p, v, from_overlay) = pick(Some(&o2), Some(&b2)).unwrap();
        assert_eq!((p, v.as_deref(), from_overlay), (b2.clone(), Some("2.4.10"), false));
        // overlay missing
        let gone = ov.join("nothing");
        let (p, _, from_overlay) = pick(Some(&gone), Some(&b)).unwrap();
        assert_eq!((p, from_overlay), (b.clone(), false));
        // an overlay copy that does not start is ignored
        let broken = script(&ov, "broken", "exit 1");
        assert_eq!(pick(Some(&broken), Some(&b)).unwrap().0, b);
        // only the overlay (bare metal without a system copy)
        assert_eq!(pick(Some(&o), None).unwrap().0, o);
        assert!(pick(None, None).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn expired_versions_are_served_while_refreshing() {
        let root = std::env::temp_dir().join(format!("encore-tools-ttl-{}-{}", std::process::id(), crate::auth::random_hex(4)));
        let exe = fake(&root, "yt-dlp", "2026.10.14");
        assert_eq!(version_of(&exe).as_deref(), Some("2026.10.14"));
        // age the entry: the old answer comes back at once and a background probe stores a fresh one
        {
            let mut map = VERSIONS.lock().unwrap();
            let e = map.get_mut(&exe).unwrap();
            e.0 = Instant::now() - VERSION_TTL - Duration::from_secs(1);
            e.2 = Some("2026.10.09".into());
        }
        assert_eq!(version_of(&exe).as_deref(), Some("2026.10.09"));
        let mut fresh = None;
        for _ in 0..100 {
            if !REFRESHING.lock().unwrap().contains(&exe) {
                fresh = VERSIONS.lock().unwrap().get(&exe).and_then(|e| e.2.clone());
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
    fn overlay_needs_the_same_python() {
        let venv = std::env::temp_dir().join(format!("encore-tools-venv-{}-{}", std::process::id(), crate::auth::random_hex(4)));
        std::fs::create_dir_all(&venv).unwrap();
        assert!(!stamp_matches(&venv, Some("3.14.0")));
        std::fs::write(venv.join(STAMP), "3.14.0\n").unwrap();
        assert!(stamp_matches(&venv, Some("3.14.0")));
        assert!(!stamp_matches(&venv, Some("3.14.1")));
        assert!(!stamp_matches(&venv, None));
        let _ = std::fs::remove_dir_all(&venv);
    }

    #[test]
    fn pip_errors_keep_the_last_lines() {
        let text = (1..=30).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        let t = tail(&text, 15);
        assert!(t.starts_with("line 16") && t.ends_with("line 30"));
    }
}
