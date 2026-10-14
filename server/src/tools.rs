//! Download tools that update without a new Encore image.
//!
//! * yt-dlp and streamrip are pip-installed into a persistent venv under the config dir
//!   (`<config>/tools/venv`), made with the image's own Python. Encore runs whichever copy is newer –
//!   that overlay or the one bundled in the image (/opt/tools) – so an image that ships a newer version
//!   wins by itself.
//! * Cadenza (server processing): the engine compiled into Encore is the built-in one; an update
//!   downloads the `cadenza` program from Cadenza's GitHub releases into `<config>/tools/cadenza/<ver>/`
//!   (checked against the release's SHA256SUMS and its `render --describe` protocol) and server-side
//!   processing runs it instead (see dsp.rs).
//! * slskd (Encore's Soulseek engine) is installed and run by slskd_managed.rs; this module only lists it,
//!   checks for and installs its updates.
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
/// Where Cadenza's releases are published (the `cadenza` program for Linux x86_64 / aarch64).
pub const CADENZA_REPO: &str = "Krifmaz/Cadenza";
/// The `cadenza render` interface Encore speaks (`protocol` in `cadenza render --describe`).
pub const CADENZA_PROTOCOL: u64 = 1;
/// Every tool the Download tools card lists, in order.
pub const IDS: [&str; 4] = ["ytdlp", "streamrip", "cadenza", "slskd"];
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
    /// Encore can install updates of it (Update / Install button)
    pub updatable: bool,
    /// the reset button's action is available (an updated copy can be removed)
    pub resettable: bool,
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

/// Short name, for messages and the task's result line.
pub fn label(id: &str) -> &'static str {
    match id {
        "ytdlp" => "yt-dlp",
        "streamrip" => "streamrip",
        "cadenza" => "Cadenza",
        _ => "slskd",
    }
}

/// The name on the Download tools card.
fn title(id: &str) -> &'static str {
    match id {
        "cadenza" => "Cadenza (server processing)",
        "slskd" => "slskd – Soulseek engine",
        _ => label(id),
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
/// For "cadenza": the downloaded program when it works, else None (the built-in engine).
pub fn path(st: &AppState, id: &str) -> Option<PathBuf> {
    if id == "cadenza" {
        return cadenza_external(st.data_dir()).map(|c| c.path);
    }
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

/// Fetch the newest version of one tool: (version, tag/spec detail, download url when known).
async fn fetch_latest(st: &AppState, id: &str) -> anyhow::Result<(String, String, Option<String>)> {
    match id {
        "ytdlp" => {
            let (code, v) = get_json(st, PYPI_YTDLP).await?;
            let ver = parse_pypi(&v).ok_or_else(|| anyhow::anyhow!("PyPI returned {code} without a yt-dlp version"))?;
            Ok((ver.clone(), ver, None))
        }
        "streamrip" => github_latest(st, STREAMRIP_REPO, true).await.map(|(tag, ver)| (ver, tag, None)),
        "cadenza" => github_latest(st, CADENZA_REPO, false).await.map(|(tag, ver)| (ver, tag, None)),
        _ => crate::slskd_managed::latest(st).await.map(|(ver, url)| (ver.clone(), ver, Some(url))),
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
        Ok((ver, tag, url)) => json!({ "latest": ver, "tag": tag, "url": url, "checked_at": crate::db::now() }),
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

// ---- Cadenza (server processing) ---------------------------------------------------------------

/// `<config>/tools/cadenza`: one folder per downloaded version and `current` (the version in use).
pub fn cadenza_dir(data: &Path) -> PathBuf {
    data.join("tools").join("cadenza")
}

fn cadenza_bin(data: &Path, ver: &str) -> PathBuf {
    cadenza_dir(data).join(ver).join("cadenza")
}

/// The version `current` points at (it may not work: see [`cadenza_external`]).
fn cadenza_current(data: &Path) -> Option<String> {
    let v = std::fs::read_to_string(cadenza_dir(data).join("current")).ok()?;
    let v = v.trim();
    // a version is a folder name: nothing that leaves the cadenza folder
    (!v.is_empty() && !v.contains('/') && !v.starts_with('.')).then(|| v.to_string())
}

/// A version number from Cargo.lock: the `version` of the `[[package]]` named `name`.
pub fn lock_version(lock: &str, name: &str) -> Option<String> {
    let mut in_pkg = false;
    for line in lock.lines() {
        let l = line.trim();
        if l == "[[package]]" {
            in_pkg = false;
        } else if let Some(n) = l.strip_prefix("name = ") {
            in_pkg = n.trim_matches('"') == name;
        } else if let Some(v) = l.strip_prefix("version = ") {
            if in_pkg {
                return Some(v.trim_matches('"').to_string());
            }
        }
    }
    None
}

/// The version of the cadenza-dsp engine compiled into Encore.
pub fn builtin_cadenza_version() -> String {
    static V: LazyLock<String> = LazyLock::new(|| lock_version(include_str!("../Cargo.lock"), "cadenza-dsp").unwrap_or_else(|| "built in".into()));
    V.clone()
}

/// What `cadenza render --describe` says (protocol already checked).
#[derive(Clone, Debug, PartialEq)]
pub struct Describe {
    pub version: String,
    pub protocol: u64,
    pub gpu: bool,
    /// the engine settings schema: [{key, label, type, options?, default, help, min?, max?, step?}]
    pub settings: Value,
}

/// Parse `cadenza render --describe` output: the first line that is a JSON object, which must speak
/// [`CADENZA_PROTOCOL`].
pub fn parse_describe(text: &str) -> Result<Describe, String> {
    let v = text.lines().filter_map(|l| serde_json::from_str::<Value>(l.trim()).ok()).find(|v| v.is_object())
        .ok_or_else(|| "no description from `cadenza render --describe`".to_string())?;
    if v["name"].as_str().is_some_and(|n| n != "cadenza") {
        return Err(format!("not Cadenza ({})", v["name"]));
    }
    let protocol = v["protocol"].as_u64().ok_or_else(|| "no protocol in its description".to_string())?;
    if protocol != CADENZA_PROTOCOL {
        return Err(format!("it speaks protocol {protocol}, this Encore speaks {CADENZA_PROTOCOL} – update Encore"));
    }
    let version = v["version"].as_str().and_then(extract_version).ok_or_else(|| "no version in its description".to_string())?;
    let settings = if v["settings"].is_array() { v["settings"].clone() } else { json!([]) };
    Ok(Describe { version, protocol, gpu: v["gpu"].as_bool().unwrap_or(false), settings })
}

/// Run `<exe> args…` with stdin closed, stopping it after `limit`.
fn output_within(exe: &Path, args: &[&str], limit: Duration) -> Result<std::process::Output, String> {
    let mut tries = 0;
    let child = loop {
        let r = std::process::Command::new(exe).args(args).stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).spawn();
        match r {
            // "Text file busy": just written
            Err(e) if e.raw_os_error() == Some(26) && tries < 5 => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(50 * tries));
            }
            r => break r.map_err(|e| format!("could not run it: {e}"))?,
        }
    };
    let mut child = child;
    let reader = |r: Option<Box<dyn std::io::Read + Send>>| std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut r) = r {
            let _ = r.read_to_end(&mut buf);
        }
        buf
    });
    use std::io::Read;
    let out = reader(child.stdout.take().map(|r| Box::new(r) as Box<dyn Read + Send>));
    let err = reader(child.stderr.take().map(|r| Box::new(r) as Box<dyn Read + Send>));
    let deadline = Instant::now() + limit;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("no answer within {} s", limit.as_secs()));
            }
            Err(e) => return Err(e.to_string()),
        }
    };
    Ok(std::process::Output { status, stdout: out.join().unwrap_or_default(), stderr: err.join().unwrap_or_default() })
}

/// `<exe> render --describe`, without the cache.
pub fn describe_now(exe: &Path) -> Result<Describe, String> {
    if !exe.is_file() {
        return Err("the program is missing".into());
    }
    let o = output_within(exe, &["render", "--describe"], Duration::from_secs(30))?;
    if !o.status.success() {
        let err = tail(&String::from_utf8_lossy(&o.stderr), 3);
        return Err(format!("`cadenza render --describe` failed ({}){}", o.status, if err.is_empty() { String::new() } else { format!(": {err}") }));
    }
    parse_describe(&String::from_utf8_lossy(&o.stdout))
}

type DescribeCache = HashMap<PathBuf, (Option<SystemTime>, Result<Describe, String>)>;
static DESCRIBES: LazyLock<Mutex<DescribeCache>> = LazyLock::new(Default::default);

/// [`describe_now`], once per program file (asked again when the file changes or a check runs).
pub fn cadenza_describe(exe: &Path) -> Result<Describe, String> {
    let mtime = mtime_of(exe);
    if let Some((mt, d)) = DESCRIBES.lock().unwrap().get(exe) {
        if *mt == mtime {
            return d.clone();
        }
    }
    let d = describe_now(exe);
    DESCRIBES.lock().unwrap().insert(exe.to_path_buf(), (mtime, d.clone()));
    d
}

fn forget_describes() {
    DESCRIBES.lock().unwrap().clear();
}

/// The downloaded Cadenza server processing runs.
#[derive(Clone, Debug, PartialEq)]
pub struct CadenzaProgram {
    pub path: PathBuf,
    pub version: String,
    pub settings: Value,
}

/// The downloaded program `current` points at, when its description works and speaks our protocol;
/// None → the built-in engine.
pub fn cadenza_external(data: &Path) -> Option<CadenzaProgram> {
    let ver = cadenza_current(data)?;
    let path = cadenza_bin(data, &ver);
    let d = cadenza_describe(&path).ok()?;
    Some(CadenzaProgram { path, version: d.version, settings: d.settings })
}

/// The release asset for this server: `cadenza-<ver>-linux-x86_64.tar.gz` / `…-linux-aarch64.tar.gz`.
pub fn cadenza_asset(ver: &str, arch: &str) -> Option<String> {
    matches!(arch, "x86_64" | "aarch64").then(|| format!("cadenza-{ver}-linux-{arch}.tar.gz"))
}

/// `SHA256SUMS` (sha256sum format: `<hex>  <name>`, or `<hex> *<name>` for binary mode) → name → hex.
pub fn parse_sha256sums(text: &str) -> HashMap<String, String> {
    text.lines()
        .filter_map(|l| {
            let (hash, name) = l.trim().split_once(char::is_whitespace)?;
            let name = name.trim_start().trim_start_matches('*').trim();
            (hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit()) && !name.is_empty())
                .then(|| (name.trim_start_matches("./").to_string(), hash.to_ascii_lowercase()))
        })
        .collect()
}

/// The file named `want` (any folder) from a .tar.gz.
pub fn untar_gz_file(archive: &[u8], want: &str) -> anyhow::Result<Vec<u8>> {
    use std::io::Read;
    let mut tar = Vec::new();
    flate2::read::GzDecoder::new(archive).take(512 * 1024 * 1024).read_to_end(&mut tar)?;
    let mut at = 0usize;
    let mut long_name: Option<String> = None;
    while at + 512 <= tar.len() {
        let h = &tar[at..at + 512];
        if h.iter().all(|b| *b == 0) {
            break;
        }
        let field = |a: usize, b: usize| String::from_utf8_lossy(&h[a..b]).trim_end_matches('\0').to_string();
        let size_field = field(124, 136);
        let size = u64::from_str_radix(size_field.trim().trim_end_matches(' '), 8).map_err(|_| anyhow::anyhow!("broken archive (size {size_field:?})"))? as usize;
        let kind = h[156];
        let mut name = field(0, 100);
        if &h[257..262] == b"ustar" {
            let prefix = field(345, 500);
            if !prefix.is_empty() {
                name = format!("{prefix}/{name}");
            }
        }
        if let Some(n) = long_name.take() {
            name = n;
        }
        let start = at + 512;
        let end = start.checked_add(size).filter(|e| *e <= tar.len()).ok_or_else(|| anyhow::anyhow!("archive is cut off"))?;
        match kind {
            b'L' => long_name = Some(String::from_utf8_lossy(&tar[start..end]).trim_end_matches('\0').to_string()),
            b'0' | 0 if name.rsplit('/').next() == Some(want) => return Ok(tar[start..end].to_vec()),
            _ => {}
        }
        at = start + size.div_ceil(512) * 512;
    }
    anyhow::bail!("{want} is not in the archive")
}

fn sha256_hex(data: &[u8]) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(data))
}

/// Install a downloaded release: verify the archive against SHA256SUMS, unpack `cadenza` to
/// `<dir>/<ver>/cadenza`, require a working `render --describe` with our protocol, point `current` at
/// it and delete the other versions.
pub fn install_cadenza(dir: &Path, ver: &str, asset: &str, archive: &[u8], sums: &str) -> anyhow::Result<Describe> {
    use std::os::unix::fs::PermissionsExt;
    if ver.is_empty() || ver.contains('/') || ver.starts_with('.') {
        anyhow::bail!("odd version {ver:?}");
    }
    let want = parse_sha256sums(sums).remove(asset).ok_or_else(|| anyhow::anyhow!("{asset} is not listed in the release's SHA256SUMS"))?;
    let got = sha256_hex(archive);
    if got != want {
        anyhow::bail!("{asset} does not match its SHA256SUMS entry (downloaded {got}, expected {want})");
    }
    let bin = untar_gz_file(archive, "cadenza")?;
    let vdir = dir.join(ver);
    let _ = std::fs::remove_dir_all(&vdir);
    std::fs::create_dir_all(&vdir)?;
    let tmp = vdir.join(".cadenza.tmp");
    std::fs::write(&tmp, &bin)?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
    let exe = vdir.join("cadenza");
    std::fs::rename(&tmp, &exe)?;
    let d = match describe_now(&exe) {
        Ok(d) => d,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&vdir);
            anyhow::bail!("Cadenza {ver} was downloaded but does not work here: {e} – Encore keeps using the engine it has");
        }
    };
    let cur = dir.join(".current.tmp");
    std::fs::write(&cur, format!("{ver}\n"))?;
    std::fs::rename(&cur, dir.join("current"))?;
    forget_describes();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            if e.path().is_dir() && e.file_name().to_string_lossy() != ver {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
    Ok(d)
}

async fn download(st: &AppState, url: &str, max: usize) -> anyhow::Result<Vec<u8>> {
    let r = st.http.get(url).timeout(Duration::from_secs(600)).send().await.map_err(|e| anyhow::anyhow!("downloading {url}: {e}"))?;
    if !r.status().is_success() {
        anyhow::bail!("downloading {url}: HTTP {}", r.status().as_u16());
    }
    let b = r.bytes().await.map_err(|e| anyhow::anyhow!("downloading {url}: {e}"))?;
    if b.len() > max {
        anyhow::bail!("{url} is larger than expected ({} MB)", b.len() / 1_048_576);
    }
    Ok(b.to_vec())
}

/// Download and install the release `tag` of Cadenza for this server's architecture.
async fn update_cadenza(st: &AppState, tag: &str) -> anyhow::Result<String> {
    let ver = tag.trim_start_matches(['v', 'V']).to_string();
    let arch = std::env::consts::ARCH;
    let asset = cadenza_asset(&ver, arch).ok_or_else(|| anyhow::anyhow!("Cadenza has no release build for this server ({arch})"))?;
    let (code, rel) = get_json(st, &format!("https://api.github.com/repos/{CADENZA_REPO}/releases/tags/{tag}")).await?;
    if code >= 400 {
        anyhow::bail!("GitHub returned {code} for the Cadenza {tag} release: {}", rel["message"].as_str().unwrap_or(""));
    }
    let url_of = |name: &str| rel["assets"].as_array().and_then(|a| a.iter().find(|x| x["name"].as_str() == Some(name)))
        .and_then(|x| x["browser_download_url"].as_str()).map(String::from);
    let archive_url = url_of(&asset).ok_or_else(|| anyhow::anyhow!("The Cadenza {ver} release has no {asset}"))?;
    let sums_url = url_of("SHA256SUMS").ok_or_else(|| anyhow::anyhow!("The Cadenza {ver} release has no SHA256SUMS"))?;
    let sums = String::from_utf8_lossy(&download(st, &sums_url, 1 << 20).await?).to_string();
    let archive = download(st, &archive_url, 400 << 20).await?;
    let dir = cadenza_dir(st.data_dir());
    let d = tokio::task::spawn_blocking(move || install_cadenza(&dir, &ver, &asset, &archive, &sums)).await??;
    Ok(d.version)
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
    match id {
        "cadenza" => return cadenza_info(st),
        "slskd" => return slskd_info(st),
        _ => {}
    }
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
    let resettable = !custom && source == "updated";
    ToolInfo { id: id.into(), name: title(id).into(), installed, source: source.into(), latest, update_available,
        checked_at: last["checked_at"].as_f64(), error: last["error"].as_str().map(String::from), note, updatable: !custom, resettable }
}

/// Cadenza: the downloaded program when it works, else the engine built into Encore.
fn cadenza_info(st: &AppState) -> ToolInfo {
    let data = st.data_dir();
    let ext = cadenza_external(data);
    let (installed, source) = match &ext {
        Some(e) => (e.version.clone(), "updated"),
        None => (builtin_cadenza_version(), "bundled"),
    };
    let last = last_check(st, "cadenza");
    let latest = last["latest"].as_str().map(String::from);
    let update_available = latest.as_deref().is_some_and(|l| newer(l, &installed));
    let mut error = last["error"].as_str().map(String::from);
    let note = match (&ext, cadenza_current(data)) {
        (Some(_), _) => "Downloaded from Cadenza's releases – server processing runs it".to_string(),
        (None, Some(v)) => {
            let why = cadenza_describe(&cadenza_bin(data, &v)).err().unwrap_or_default();
            error.get_or_insert_with(|| format!("The downloaded Cadenza {v} does not work ({why}) – using the built-in engine"));
            "The engine built into Encore".to_string()
        }
        (None, None) => "The engine built into Encore".to_string(),
    };
    ToolInfo { id: "cadenza".into(), name: title("cadenza").into(), installed: Some(installed), source: source.into(), latest, update_available,
        checked_at: last["checked_at"].as_f64(), error, note, updatable: true, resettable: cadenza_dir(data).exists() }
}

/// slskd, installed and run by Encore (slskd_managed.rs).
fn slskd_info(st: &AppState) -> ToolInfo {
    let installed = crate::slskd_managed::installed_version(st);
    let last = last_check(st, "slskd");
    let latest = last["latest"].as_str().map(String::from);
    let update_available = latest.as_deref().is_some_and(|l| installed.as_deref().map(|i| newer(l, i)).unwrap_or(true));
    let note = if installed.is_some() { "Runs inside Encore for Soulseek" } else { "Not installed – Install downloads it from slskd's releases" };
    ToolInfo { id: "slskd".into(), name: title("slskd").into(), source: if installed.is_some() { "updated" } else { "missing" }.into(), resettable: installed.is_some(),
        installed, latest, update_available, checked_at: last["checked_at"].as_f64(), error: last["error"].as_str().map(String::from), note: note.into(), updatable: true }
}

/// The listing from what is known now (no network).
pub fn cached(st: &AppState) -> Vec<ToolInfo> {
    IDS.iter().map(|id| local_info(st, id)).collect()
}

/// Look up the newest versions (PyPI, GitHub) and the installed ones. An image that ships a newer copy
/// than the overlay makes the overlay copy redundant: it is removed.
pub async fn check(st: &AppState) -> Vec<ToolInfo> {
    tokio::join!(refresh_latest(st, "ytdlp"), refresh_latest(st, "streamrip"), refresh_latest(st, "cadenza"), refresh_latest(st, "slskd"));
    for id in ["ytdlp", "streamrip"] {
        let st2 = st.clone();
        let _ = tokio::task::spawn_blocking(move || drop_stale_overlay(&st2, id)).await;
    }
    forget_describes();
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

/// Tools with a download running (Cadenza, slskd – each its own folder, so they may run together
/// with a pip install).
static DOWNLOADING: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Default::default);

struct Downloading(String);
impl Downloading {
    fn start(id: &str) -> anyhow::Result<Downloading> {
        if !DOWNLOADING.lock().unwrap().insert(id.to_string()) {
            anyhow::bail!("An update of {} is already running", label(id));
        }
        Ok(Downloading(id.to_string()))
    }
}
impl Drop for Downloading {
    fn drop(&mut self) {
        DOWNLOADING.lock().unwrap().remove(&self.0);
    }
}

async fn update_now(st: &AppState, id: &str) -> anyhow::Result<ToolInfo> {
    if !IDS.contains(&id) {
        anyhow::bail!("{id} can't be updated from Encore");
    }
    if id == "cadenza" || id == "slskd" {
        let _busy = Downloading::start(id)?;
        let mut last = last_check(st, id);
        if last["tag"].as_str().is_none() || (id == "slskd" && last["url"].as_str().is_none()) {
            last = refresh_latest(st, id).await;
        }
        let Some(tag) = last["tag"].as_str().map(String::from) else {
            anyhow::bail!("{}", last["error"].as_str().unwrap_or("Could not find the newest version"));
        };
        let res = if id == "cadenza" {
            update_cadenza(st, &tag).await
        } else {
            let url = last["url"].as_str().unwrap_or_default().to_string();
            crate::slskd_managed::install(st, &tag, &url).await.map(|_| tag.clone())
        };
        match res {
            Ok(v) => st.db.log("info", &format!("{} updated to {v}", label(id))),
            Err(e) => {
                st.db.log("warning", &format!("Updating {} failed: {e}", label(id)));
                return Err(e);
            }
        }
        let st2 = st.clone();
        let id2 = id.to_string();
        return Ok(tokio::task::spawn_blocking(move || local_info(&st2, &id2)).await?);
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

/// "Use the bundled version": remove the overlay copy of a tool. Cadenza: back to the built-in engine
/// (removes `<config>/tools/cadenza`); slskd: stop and remove it.
pub fn reset(st: &AppState, id: &str) -> anyhow::Result<()> {
    if id == "cadenza" || id == "slskd" {
        let _busy = Downloading::start(id)?;
        if id == "slskd" {
            let _ = crate::slskd_managed::reset(st);
        } else {
            let dir = cadenza_dir(st.data_dir());
            if dir.exists() {
                std::fs::remove_dir_all(&dir)?;
            }
            forget_describes();
        }
        return Ok(());
    }
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

/// The auto-update switch of a tool (Download tools card).
pub fn auto_key(id: &str) -> &'static str {
    match id {
        "ytdlp" => "tools_auto_ytdlp",
        "streamrip" => "tools_auto_streamrip",
        "cadenza" => "tools_auto_cadenza",
        _ => "tools_auto_slskd",
    }
}

/// What the scheduled task does with one tool, from what the check found.
#[derive(Debug, PartialEq)]
pub enum Plan {
    /// a result line, nothing to install
    Say(String),
    Error(String),
    /// install `to` (from: installed version, if any)
    Install { from: Option<String>, to: String },
}

/// `auto`: the tool's switch is on; `wanted`: the owner set the tool up (an install of a missing
/// tool is only done for those).
pub fn plan(t: &ToolInfo, auto: bool, wanted: bool) -> Plan {
    let name = label(&t.id);
    let missing = t.source == "missing";
    if missing && !(auto && wanted && t.id == "slskd" && t.latest.is_some()) {
        return match (&t.latest, t.id.as_str()) {
            (Some(l), "slskd") => Plan::Say(format!("{name} {l} available (not installed)")),
            _ => Plan::Say(format!("{name} not installed")),
        };
    }
    if let Some(e) = &t.error {
        if t.latest.is_none() || e.starts_with("Could not check") {
            return Plan::Error(format!("{name}: {e}"));
        }
    }
    if !t.update_available {
        return Plan::Say(format!("{name} up to date"));
    }
    let to = t.latest.clone().unwrap_or_default();
    if !auto || !t.updatable {
        return Plan::Say(format!("{name} {to} available"));
    }
    Plan::Install { from: t.installed.clone(), to }
}

/// Whether the scheduled task may install for `id`: its switch is on, yt-dlp isn't pinned by the
/// environment, and streamrip is switched on (a streamrip the owner turned off is only reported:
/// "up to date" / "X available", or "not installed" when it isn't there).
fn task_auto(id: &str, switch: bool, wanted: bool, ytdlp_env: bool) -> bool {
    switch && (id != "ytdlp" || ytdlp_env) && (id != "streamrip" || wanted)
}

/// Scheduled task: check every tool, then install updates of those whose switch is on.
pub async fn scheduled(st: &AppState) -> anyhow::Result<String> {
    let list = check(st).await;
    let mut parts = vec![];
    let mut errors = vec![];
    for id in IDS {
        let Some(t) = list.iter().find(|t| t.id == id) else { continue };
        let wanted = match id {
            "streamrip" => st.cfg.bool("streamrip_enabled"),
            "slskd" => st.cfg.bool("slskd_enabled"),
            _ => true,
        };
        let auto = task_auto(id, st.cfg.bool(auto_key(id)), wanted, env_allows_ytdlp());
        match plan(t, auto, wanted) {
            Plan::Say(s) => parts.push(s),
            Plan::Error(e) => errors.push(e),
            Plan::Install { from, to } => match update(st, id).await {
                Ok(n) => parts.push(format!("{} {} → {}", label(id), from.unwrap_or_else(|| "not installed".into()), n.installed.unwrap_or(to))),
                Err(e) => errors.push(format!("{}: {}", label(id), e.to_string().lines().next().unwrap_or(""))),
            },
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
    fn scheduled_task_reports_a_switched_off_streamrip_without_installing() {
        assert!(!task_auto("streamrip", true, false, true));
        assert!(task_auto("streamrip", true, true, true));
        assert!(!task_auto("ytdlp", true, true, false));
        assert!(task_auto("cadenza", true, true, false));
        assert!(!task_auto("slskd", false, true, true));
        let t = info("streamrip", "pip", Some("2.4.9"), Some("2.5.0"));
        assert_eq!(plan(&t, task_auto("streamrip", true, false, true), false), Plan::Say("streamrip 2.5.0 available".into()));
        let t = info("streamrip", "pip", Some("2.5.0"), Some("2.5.0"));
        assert_eq!(plan(&t, false, false), Plan::Say("streamrip up to date".into()));
        let t = info("streamrip", "missing", None, Some("2.5.0"));
        assert_eq!(plan(&t, false, false), Plan::Say("streamrip not installed".into()));
    }

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

    const DESCRIBE: &str = r#"{"name":"cadenza","version":"2.1.0","protocol":1,"in_formats":["f32le","f64le"],"out_formats":["s32le"],"gpu":true,"filters":["sinc-l-64k"],"dithers":["tpdf"],"settings":[{"key":"filter","label":"Upsampling filter","type":"select","options":[["sinc-l-64k","Sinc L 64k"]],"default":"sinc-l-64k","help":""},{"key":"gpu","label":"GPU processing","type":"select","options":[["auto","Auto"],["on","On"],["off","Off"]],"default":"auto","help":""}]}"#;

    #[test]
    fn describe_needs_protocol_1() {
        let d = parse_describe(&format!("{DESCRIBE}\n")).unwrap();
        assert_eq!((d.version.as_str(), d.protocol, d.gpu), ("2.1.0", 1, true));
        assert_eq!(d.settings[1]["key"], "gpu");
        // stray lines before the JSON are skipped
        assert_eq!(parse_describe(&format!("warning: something\n{DESCRIBE}")).unwrap().version, "2.1.0");
        let other = DESCRIBE.replace(r#""protocol":1"#, r#""protocol":2"#);
        assert!(parse_describe(&other).unwrap_err().contains("protocol 2"));
        assert!(parse_describe(&DESCRIBE.replace(r#""protocol":1,"#, "")).is_err());
        assert!(parse_describe(&DESCRIBE.replace(r#""name":"cadenza""#, r#""name":"other""#)).is_err());
        assert!(parse_describe("not json").is_err());
        assert!(parse_describe("").is_err());
        // no settings: an empty schema
        let bare = parse_describe(r#"{"name":"cadenza","version":"v2.2.0","protocol":1}"#).unwrap();
        assert_eq!((bare.version.as_str(), bare.settings.clone()), ("2.2.0", json!([])));
    }

    #[test]
    fn asset_names_by_arch() {
        assert_eq!(cadenza_asset("2.1.0", "x86_64").as_deref(), Some("cadenza-2.1.0-linux-x86_64.tar.gz"));
        assert_eq!(cadenza_asset("2.1.0", "aarch64").as_deref(), Some("cadenza-2.1.0-linux-aarch64.tar.gz"));
        assert_eq!(cadenza_asset("2.1.0", "arm"), None);
        assert_eq!(cadenza_asset("2.1.0", "riscv64"), None);
        assert!(cadenza_asset("2.1.0", std::env::consts::ARCH).is_some() || !matches!(std::env::consts::ARCH, "x86_64" | "aarch64"));
    }

    #[test]
    fn sha256sums_lines() {
        let a = "a".repeat(64);
        let b = "B".repeat(64);
        let text = format!("{a}  cadenza-2.1.0-linux-x86_64.tar.gz\n{b} *cadenza-2.1.0-linux-aarch64.tar.gz\n\nnot a line\n{}  short\n{a}  ./dot.tar.gz\n", "c".repeat(10));
        let m = parse_sha256sums(&text);
        assert_eq!(m.get("cadenza-2.1.0-linux-x86_64.tar.gz"), Some(&a));
        assert_eq!(m.get("cadenza-2.1.0-linux-aarch64.tar.gz"), Some(&"b".repeat(64)), "binary mode, lower-cased");
        assert_eq!(m.get("dot.tar.gz"), Some(&a));
        assert_eq!(m.len(), 3);
        assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    #[test]
    fn cargo_lock_versions() {
        let lock = "[[package]]\nname = \"cadenza\"\nversion = \"9.9.9\"\n\n[[package]]\nname = \"cadenza-dsp\"\nversion = \"0.1.0\"\nsource = \"git+…\"\n";
        assert_eq!(lock_version(lock, "cadenza-dsp").as_deref(), Some("0.1.0"));
        assert_eq!(lock_version(lock, "nope"), None);
        // Encore's own lock file has the engine it is built with
        assert!(!parts(&builtin_cadenza_version()).is_empty());
    }

    /// A ustar .tar.gz of (name, contents, mode).
    fn tar_gz(files: &[(&str, &[u8])]) -> Vec<u8> {
        use std::io::Write;
        let mut tar = Vec::new();
        for (name, data) in files {
            let mut h = [0u8; 512];
            h[..name.len()].copy_from_slice(name.as_bytes());
            h[100..107].copy_from_slice(b"0000755");
            h[108..115].copy_from_slice(b"0000000");
            h[116..123].copy_from_slice(b"0000000");
            h[124..135].copy_from_slice(format!("{:011o}", data.len()).as_bytes());
            h[136..147].copy_from_slice(b"00000000000");
            h[156] = b'0';
            h[257..263].copy_from_slice(b"ustar\0");
            h[263..265].copy_from_slice(b"00");
            h[148..156].copy_from_slice(b"        ");
            let sum: u32 = h.iter().map(|b| *b as u32).sum();
            h[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
            tar.extend_from_slice(&h);
            tar.extend_from_slice(data);
            tar.resize(tar.len().div_ceil(512) * 512, 0);
        }
        tar.extend_from_slice(&[0u8; 1024]);
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gz.write_all(&tar).unwrap();
        gz.finish().unwrap()
    }

    #[test]
    fn untar_finds_the_program() {
        let a = tar_gz(&[("README", b"hi"), ("cadenza", b"#!/bin/sh\n")]);
        assert_eq!(untar_gz_file(&a, "cadenza").unwrap(), b"#!/bin/sh\n");
        assert!(untar_gz_file(&a, "missing").is_err());
        let nested = tar_gz(&[("cadenza-2.1.0/cadenza", b"x")]);
        assert_eq!(untar_gz_file(&nested, "cadenza").unwrap(), b"x");
        assert!(untar_gz_file(b"not gzip", "cadenza").is_err());
        // an archive made by the system's tar (GNU or bsdtar)
        let root = std::env::temp_dir().join(format!("encore-tools-tar-{}-{}", std::process::id(), crate::auth::random_hex(4)));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("cadenza"), b"real tar").unwrap();
        let made = std::process::Command::new("tar").arg("-czf").arg(root.join("a.tar.gz")).arg("-C").arg(&root).arg("cadenza").status();
        if made.is_ok_and(|s| s.success()) {
            let bytes = std::fs::read(root.join("a.tar.gz")).unwrap();
            assert_eq!(untar_gz_file(&bytes, "cadenza").unwrap(), b"real tar");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn installs_a_release_and_keeps_only_it() {
        let data = std::env::temp_dir().join(format!("encore-tools-cadenza-{}-{}", std::process::id(), crate::auth::random_hex(4)));
        let dir = cadenza_dir(&data);
        let program = |describe: &str| format!("#!/bin/sh\n[ \"$1 $2\" = \"render --describe\" ] || exit 3\necho '{describe}'\n").into_bytes();
        let asset = "cadenza-2.1.0-linux-x86_64.tar.gz";
        let archive = tar_gz(&[("cadenza", &program(DESCRIBE))]);
        let sums = format!("{}  {asset}\n", sha256_hex(&archive));
        // an older version is there
        std::fs::create_dir_all(dir.join("2.0.0")).unwrap();
        std::fs::write(dir.join("2.0.0").join("cadenza"), b"old").unwrap();
        // a checksum mismatch installs nothing
        let bad = format!("{}  {asset}\n", "0".repeat(64));
        let e = install_cadenza(&dir, "2.1.0", asset, &archive, &bad).unwrap_err().to_string();
        assert!(e.contains("does not match"), "{e}");
        assert!(install_cadenza(&dir, "2.1.0", asset, &archive, "").unwrap_err().to_string().contains("not listed"));
        assert!(!dir.join("2.1.0").exists() && cadenza_current(&data).is_none());
        // a release speaking another protocol is refused and removed
        let v2 = tar_gz(&[("cadenza", &program(&DESCRIBE.replace(r#""protocol":1"#, r#""protocol":2"#)))]);
        let e = install_cadenza(&dir, "2.1.0", asset, &v2, &format!("{}  {asset}\n", sha256_hex(&v2))).unwrap_err().to_string();
        assert!(e.contains("protocol 2"), "{e}");
        assert!(!dir.join("2.1.0").exists() && cadenza_current(&data).is_none());
        assert!(cadenza_external(&data).is_none());
        // the real thing
        let d = install_cadenza(&dir, "2.1.0", asset, &archive, &sums).unwrap();
        assert_eq!(d.version, "2.1.0");
        assert_eq!(cadenza_current(&data).as_deref(), Some("2.1.0"));
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(dir.join("2.1.0/cadenza")).unwrap().permissions().mode() & 0o777, 0o755);
        assert!(!dir.join("2.0.0").exists(), "older versions are deleted");
        let ext = cadenza_external(&data).unwrap();
        assert_eq!((ext.path, ext.version.as_str()), (dir.join("2.1.0").join("cadenza"), "2.1.0"));
        assert_eq!(ext.settings[1]["key"], "gpu");
        // `current` pointing at a broken program → the built-in engine
        std::fs::write(dir.join("current"), "9.9.9\n").unwrap();
        assert!(cadenza_external(&data).is_none());
        std::fs::write(dir.join("current"), "../../etc\n").unwrap();
        assert!(cadenza_current(&data).is_none());
        let _ = std::fs::remove_dir_all(&data);
    }

    fn info(id: &str, source: &str, installed: Option<&str>, latest: Option<&str>) -> ToolInfo {
        let update_available = latest.is_some_and(|l| installed.map(|i| newer(l, i)).unwrap_or(true));
        ToolInfo { id: id.into(), name: title(id).into(), installed: installed.map(String::from), source: source.into(), latest: latest.map(String::from),
            update_available, checked_at: None, error: None, note: String::new(), updatable: true, resettable: false }
    }

    #[test]
    fn scheduled_plan_per_tool() {
        // Cadenza: built in, a release is out – installed only with its switch on
        let c = info("cadenza", "bundled", Some("0.1.0"), Some("2.1.0"));
        assert_eq!(plan(&c, false, true), Plan::Say("Cadenza 2.1.0 available".into()));
        assert_eq!(plan(&c, true, true), Plan::Install { from: Some("0.1.0".into()), to: "2.1.0".into() });
        assert_eq!(plan(&info("cadenza", "updated", Some("2.1.0"), Some("2.1.0")), true, true), Plan::Say("Cadenza up to date".into()));
        // slskd: not installed – nothing downloads by itself unless its switch is on (and Soulseek is)
        let s = info("slskd", "missing", None, Some("0.24.1"));
        assert_eq!(plan(&s, false, true), Plan::Say("slskd 0.24.1 available (not installed)".into()));
        assert_eq!(plan(&s, true, false), Plan::Say("slskd 0.24.1 available (not installed)".into()));
        assert_eq!(plan(&s, true, true), Plan::Install { from: None, to: "0.24.1".into() });
        assert_eq!(plan(&info("slskd", "updated", Some("0.24.0"), Some("0.24.1")), false, true), Plan::Say("slskd 0.24.1 available".into()));
        assert_eq!(plan(&info("slskd", "missing", None, None), true, true), Plan::Say("slskd not installed".into()));
        // yt-dlp: missing is never installed by the task; a failed check is an error
        assert_eq!(plan(&info("ytdlp", "missing", None, Some("2026.10.14")), true, true), Plan::Say("yt-dlp not installed".into()));
        let mut y = info("ytdlp", "bundled", Some("2026.10.09"), Some("2026.10.14"));
        assert_eq!(plan(&y, true, true), Plan::Install { from: Some("2026.10.09".into()), to: "2026.10.14".into() });
        y.error = Some("Could not check for a new version: timeout".into());
        assert!(matches!(plan(&y, true, true), Plan::Error(e) if e.starts_with("yt-dlp: Could not check")));
        // a copy Encore must not touch
        let mut st = info("streamrip", "system", Some("2.4.9"), Some("2.5.0"));
        st.updatable = false;
        assert_eq!(plan(&st, true, true), Plan::Say("streamrip 2.5.0 available".into()));
        assert_eq!(IDS.map(auto_key), ["tools_auto_ytdlp", "tools_auto_streamrip", "tools_auto_cadenza", "tools_auto_slskd"]);
    }

    #[test]
    fn auto_switch_defaults() {
        let d: Value = serde_json::from_str(include_str!("defaults.json")).unwrap();
        assert_eq!(d["tools_auto_ytdlp"]["default"], true);
        assert_eq!(d["tools_auto_streamrip"]["default"], true);
        assert_eq!(d["tools_auto_cadenza"]["default"], false);
        assert_eq!(d["tools_auto_slskd"]["default"], false);
        assert_eq!(d["cadenza_engine_settings"]["default"], "{}");
    }

    #[test]
    fn pip_errors_keep_the_last_lines() {
        let text = (1..=30).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        let t = tail(&text, 15);
        assert!(t.starts_with("line 16") && t.ends_with("line 30"));
    }
}
