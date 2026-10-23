//! The downloaded Cadenza program server processing runs (the cadenza plugin): where it lives, what
//! it says about itself (`cadenza render --describe`), and the version of the engine built into Encore.
use encore_core::version::extract_version;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant, SystemTime};

/// The `cadenza render` interface Encore speaks (`protocol` in `cadenza render --describe`).
pub const CADENZA_PROTOCOL: u64 = 1;

/// `<config>/tools/cadenza`: one folder per downloaded version and `current` (the version in use).
pub fn cadenza_dir(data: &Path) -> PathBuf {
    data.join("tools").join("cadenza")
}

/// The program of downloaded version `ver`.
pub fn cadenza_bin(data: &Path, ver: &str) -> PathBuf {
    cadenza_dir(data).join(ver).join("cadenza")
}

/// The version `current` points at (it may not work: see [`Programs::external`]).
pub fn cadenza_current(data: &Path) -> Option<String> {
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
    static V: LazyLock<String> = LazyLock::new(|| lock_version(include_str!("../../../Cargo.lock"), "cadenza-dsp").unwrap_or_else(|| "built in".into()));
    V.clone()
}

/// What `cadenza render --describe` says (protocol already checked).
#[derive(Clone, Debug, PartialEq)]
pub struct Describe {
    /// the program's version
    pub version: String,
    /// the render protocol it speaks
    pub protocol: u64,
    /// it was built with GPU processing
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

/// The downloaded program's start, through the plugins' spawn builder (encore_plugins::sandbox: a session of
/// its own, no new privileges, no other open files; Encore's environment without its settings; full priority – it streams in real
/// time). Killed when dropped.
pub fn command<S: AsRef<std::ffi::OsStr>>(exe: &Path, args: &[S]) -> tokio::process::Command {
    use encore_plugins::sandbox::{Sandbox, Spawn};
    let env = encore_plugins::sandbox::program_env();
    Spawn::new(&Sandbox::limits_only(), exe).args(args.iter().map(|a| a.as_ref().to_os_string())).env(env).tokio_command()
}

/// Run `<exe> args…` with stdin closed (through the spawn builder), stopping it after `limit`.
fn output_within(exe: &Path, args: &[&str], limit: Duration) -> Result<std::process::Output, String> {
    use encore_plugins::sandbox::{Sandbox, Spawn};
    let env = encore_plugins::sandbox::program_env();
    let spawn = Spawn::new(&Sandbox::limits_only(), exe).args(args.iter().copied()).env(env);
    let mut tries = 0;
    let child = loop {
        let r = spawn.command().stdin(std::process::Stdio::null())
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
        let err = encore_core::text::last_lines(&String::from_utf8_lossy(&o.stderr), 3);
        return Err(format!("`cadenza render --describe` failed ({}){}", o.status, if err.is_empty() { String::new() } else { format!(": {err}") }));
    }
    parse_describe(&String::from_utf8_lossy(&o.stdout))
}

/// Each program file's description, with the file's modification time it was read at (a new file is asked again).
type DescribeCache = HashMap<PathBuf, (Option<SystemTime>, Result<Describe, String>)>;

/// What each downloaded Cadenza program said about itself, kept per program file. The server keeps one
/// (in [`crate::Playback`]); clones share it.
#[derive(Clone, Default)]
pub struct Programs {
    /// what each program said, by its path
    described: Arc<Mutex<DescribeCache>>,
    /// moves with every [`Programs::forget`], so what was worked out from the programs is known to be old
    epoch: Arc<AtomicU64>,
}

impl Programs {
    /// [`describe_now`], once per program file (asked again when the file changes or a check runs).
    pub fn describe(&self, exe: &Path) -> Result<Describe, String> {
        let mtime = encore_core::fs::mtime(exe);
        if let Some((mt, d)) = self.described.lock().unwrap().get(exe) {
            if *mt == mtime {
                return d.clone();
            }
        }
        let d = describe_now(exe);
        self.described.lock().unwrap().insert(exe.to_path_buf(), (mtime, d.clone()));
        d
    }

    /// Ask every program again next time (after an install, a reset or a check for updates).
    pub fn forget(&self) {
        self.described.lock().unwrap().clear();
        self.epoch.fetch_add(1, Ordering::SeqCst);
    }

    /// A number that changes with every [`Programs::forget`]: the engine worked out from the programs
    /// (`dsp::Engine::of`) is kept while it stays the same.
    pub fn epoch(&self) -> u64 {
        self.epoch.load(Ordering::SeqCst)
    }

    /// The downloaded program `current` points at, when its description works and speaks our protocol;
    /// None → the built-in engine.
    /// The program server processing runs: the cadenza plugin's (its own install under
    /// `<config>/plugins/cadenza`, else the `tools/cadenza` it adopted) while the plugin is installed and
    /// switched on and the program's description works and speaks our protocol; None → the built-in engine.
    pub fn external_of(&self, plugins: &encore_plugins::Registry) -> Option<CadenzaProgram> {
        if !plugins.enabled("cadenza") {
            return None;
        }
        let p = plugins.program_or_adopted("cadenza")?;
        let d = self.describe(&p.path).ok()?;
        Some(CadenzaProgram { path: p.path, version: d.version, settings: d.settings })
    }
}

/// The downloaded Cadenza server processing runs.
#[derive(Clone, Debug, PartialEq)]
pub struct CadenzaProgram {
    /// the program file
    pub path: PathBuf,
    /// its version
    pub version: String,
    /// its engine settings schema
    pub settings: Value,
}
