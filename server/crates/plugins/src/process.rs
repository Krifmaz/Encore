//! `ProcessPlugin`: a plugin program speaking protocol 1 exactly as the owner's SDK (plugins/sdk/encore_plugin.py)
//! does, as docs/plugins/CONTRACT.md describes it.
//!
//! One command, one process: `<interpreter> <script> <command>` in the sandbox ([`crate::sandbox`]), one JSON
//! request on stdin (settings and secrets travel there and nowhere else), the last JSON-object line of stdout
//! is the answer (at most 16 MB), stderr lines are `progress <0-100> [text]`, `log <text>` or anything else for
//! the log. A command that runs over its time, or is cancelled, is stopped with SIGTERM to its processes and
//! SIGKILL to its group 10 seconds later.
use crate::logs::Log;
use crate::manifest::{Manifest, PROTOCOL};
use crate::plugin::{BoxFuture, Call, Code, Downloaded, Failure, FileEntry, Plugin, Status};
use crate::sandbox::{self, Limits, Sandbox, Spawn};
use encore_core::fsx::{self, Step};
use serde_json::{json, Map, Value};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};

/// The most a plugin's answer may be.
pub const MAX_ANSWER: usize = 16 << 20;
/// The most a result's `payload` may be (serialized).
pub const MAX_PAYLOAD: usize = 64 << 10;
/// How long a stopped plugin gets to clean up before SIGKILL.
pub const GRACE: Duration = Duration::from_secs(10);
/// A file a download may deliver: the SDK's `AUDIO_EXT`.
pub const AUDIO_EXT: [&str; 13] = ["flac", "mp3", "m4a", "alac", "ogg", "opus", "wav", "aiff", "aif", "ape", "wv", "aac", "webm"];
/// The defaults of a plugin program's limits (`[permissions]` memory, processes, open_files; cpu: the timeout).
pub const DEFAULT_MEMORY_MB: u64 = 2048;
pub const DEFAULT_PROCESSES: u64 = 64;
pub const DEFAULT_OPEN_FILES: u64 = 1024;
/// The environment variables of Encore's own a networked plugin gets.
const PROXY_VARS: [&str; 8] = ["HTTP_PROXY", "HTTPS_PROXY", "NO_PROXY", "http_proxy", "https_proxy", "no_proxy", "SSL_CERT_FILE", "SSL_CERT_DIR"];

/// A plugin's folders under `<config>/plugins/<id>`.
#[derive(Debug, Clone, PartialEq)]
pub struct Folders {
    pub root: PathBuf,
}

impl Folders {
    pub fn new(root: &Path) -> Folders {
        Folders { root: root.to_path_buf() }
    }
    /// ENCORE_PLUGIN_DATA: kept across updates, removed with Remove
    pub fn data(&self) -> PathBuf {
        self.root.join("data")
    }
    pub fn logs(&self) -> PathBuf {
        self.root.join("logs")
    }
    pub fn home(&self) -> PathBuf {
        self.root.join("home")
    }
    pub fn tmp(&self) -> PathBuf {
        self.root.join("tmp")
    }
    pub fn version(&self, v: &str) -> PathBuf {
        self.root.join(v)
    }
    /// The private folders a run needs, made when missing.
    pub fn make(&self) -> Result<(), fsx::StepError> {
        for d in [self.data(), self.home(), self.home().join(".config"), self.home().join(".cache"), self.home().join(".local/share"), self.tmp()] {
            fsx::mkdir_all(&d, Step::Plugin)?;
        }
        Ok(())
    }
}

/// The Python plugin programs run with: the image's (the highest `/opt/python/*/bin/python3`), else
/// `python3` on PATH.
pub fn find_python() -> Option<PathBuf> {
    let mut found: Vec<(Vec<u64>, PathBuf)> = std::fs::read_dir("/opt/python")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let p = e.path().join("bin/python3");
            p.is_file().then(|| (encore_core::version::parts(&e.file_name().to_string_lossy()), p))
        })
        .collect();
    found.sort();
    found.pop().map(|(_, p)| p).or_else(|| encore_core::which::which("python3"))
}

/// The interpreter of `[run] interpreter` on this machine.
pub fn interpreter(name: &str) -> Option<PathBuf> {
    match name {
        "python3" => find_python(),
        "node" => encore_core::which::which("node"),
        "sh" => Some(PathBuf::from("/bin/sh")),
        _ => None,
    }
}

/// A folder a program outside the system folders needs at full isolation: its install's root (two levels above
/// the program: `<prefix>/bin/python3` → `<prefix>`), when it is not under /usr or /opt/python.
fn outside_prefix(program: &Path) -> Option<PathBuf> {
    let real = std::fs::canonicalize(program).ok()?;
    if real.starts_with("/usr") || real.starts_with("/opt/python") || real.starts_with("/bin") || real.starts_with("/lib") {
        return None;
    }
    real.parent()?.parent().map(Path::to_path_buf).filter(|p| p != Path::new("/"))
}

/// One plugin program.
pub struct ProcessPlugin {
    manifest: Manifest,
    folders: Folders,
    /// where the installed version's files are (the script, the SDK copy)
    version_dir: PathBuf,
    sandbox: Sandbox,
    log: Log,
    /// more folders it sees read-only (a library it asked to read)
    read_only: Vec<PathBuf>,
    /// the process groups running now (stopped on Disable and Remove)
    running: Mutex<HashSet<u32>>,
    /// rate_limited: no new command until then
    paused_until: Mutex<Option<Instant>>,
    /// request kinds it answered `unsupported` for
    unsupported_kinds: Mutex<HashSet<String>>,
}

/// Why a run was stopped.
enum Stopped {
    Timeout,
    Cancelled,
}

impl ProcessPlugin {
    pub fn new(manifest: Manifest, root: &Path, version_dir: &Path, sandbox: Sandbox, read_only: Vec<PathBuf>) -> ProcessPlugin {
        let folders = Folders::new(root);
        let log = Log::new(&folders.logs());
        ProcessPlugin {
            manifest,
            folders,
            version_dir: version_dir.to_path_buf(),
            sandbox,
            log,
            read_only,
            running: Mutex::default(),
            paused_until: Mutex::default(),
            unsupported_kinds: Mutex::default(),
        }
    }

    pub fn log(&self) -> &Log {
        &self.log
    }

    pub fn version_dir(&self) -> &Path {
        &self.version_dir
    }

    fn name(&self) -> &str {
        &self.manifest.plugin.name
    }

    /// The installed program of `[[binary]]` for this machine, when the manifest has one.
    fn bin(&self) -> Option<PathBuf> {
        let b = self.manifest.binary_for(std::env::consts::ARCH)?;
        crate::archive::find_bin(&self.version_dir, &b.bin)
    }

    /// The environment of a run (docs/plugins/CONTRACT.md, "Environment").
    pub fn environment(&self) -> Vec<(String, String)> {
        let m = &self.manifest;
        let home = self.folders.home();
        let data = self.folders.data();
        let mut env: Vec<(String, String)> = vec![
            ("ENCORE_PLUGIN_DATA".into(), data.display().to_string()),
            ("ENCORE_PLUGIN_ID".into(), m.plugin.id.clone()),
            ("ENCORE_PLUGIN_VERSION".into(), m.plugin.version.clone()),
            ("ENCORE_VERSION".into(), encore_core::VERSION.into()),
            ("ENCORE_PROTOCOL".into(), PROTOCOL.to_string()),
            ("HOME".into(), home.display().to_string()),
            ("TMPDIR".into(), self.folders.tmp().display().to_string()),
            ("XDG_CONFIG_HOME".into(), home.join(".config").display().to_string()),
            ("XDG_CACHE_HOME".into(), home.join(".cache").display().to_string()),
            ("XDG_DATA_HOME".into(), home.join(".local/share").display().to_string()),
            ("PATH".into(), "/usr/local/bin:/usr/bin:/bin".into()),
            ("LANG".into(), "C.UTF-8".into()),
            ("TZ".into(), std::env::var("TZ").ok().filter(|t| !t.trim().is_empty()).unwrap_or_else(|| "UTC".into())),
            ("PYTHONUNBUFFERED".into(), "1".into()),
            ("PYTHONDONTWRITEBYTECODE".into(), "1".into()),
            ("PYTHONIOENCODING".into(), "utf-8".into()),
        ];
        let bin = self.bin();
        if let Some(b) = &bin {
            env.push(("ENCORE_PLUGIN_BIN".into(), b.display().to_string()));
        }
        if m.permissions.network.allowed() {
            for k in PROXY_VARS {
                if let Ok(v) = std::env::var(k) {
                    if !v.is_empty() {
                        env.push((k.into(), v));
                    }
                }
            }
        }
        if let Some(run) = &m.run {
            let bin_s = bin.as_ref().map(|b| b.display().to_string()).unwrap_or_default();
            let bin_dir = bin.as_ref().and_then(|b| b.parent()).map(|d| d.display().to_string()).unwrap_or_default();
            for (k, v) in &run.env {
                let v = v
                    .replace("{bin_dir}", &bin_dir)
                    .replace("{bin}", &bin_s)
                    .replace("{data}", &data.display().to_string())
                    .replace("{plugin}", &self.folders.root.display().to_string());
                env.retain(|(name, _)| name != k);
                env.push((k.clone(), v));
            }
        }
        env
    }

    /// The limits of a command's run (`[permissions]`, else the defaults; cpu: the command's time limit).
    pub fn limits(&self, command: &str) -> Limits {
        let p = &self.manifest.permissions;
        Limits {
            memory_mb: p.memory.unwrap_or(DEFAULT_MEMORY_MB),
            cpu_secs: p.cpu.unwrap_or_else(|| self.manifest.timeout(command)),
            processes: p.processes.unwrap_or(DEFAULT_PROCESSES),
            open_files: p.open_files.unwrap_or(DEFAULT_OPEN_FILES),
        }
    }

    /// The start of one command (`folder`: the download's folder, writable while it runs).
    pub fn spawn_for(&self, command: &str, folder: Option<&Path>) -> Result<Spawn, Failure> {
        let run = self.manifest.run.clone().unwrap_or_default();
        let script_rel = run.script.as_deref().ok_or_else(|| Failure::internal(format!("{} names no program to run", self.name())))?;
        let script = self.version_dir.join(script_rel);
        if !script.is_file() {
            return Err(Failure::internal(format!("{}'s program {script_rel} is missing – install the plugin again", self.name())));
        }
        let (program, mut args) = match run.interpreter.as_deref() {
            Some(name) => {
                let p = interpreter(name).ok_or_else(|| Failure::internal(format!("{} needs {name}, which this server does not have", self.name())))?;
                (p, vec![script.into_os_string()])
            }
            None => (script, vec![]),
        };
        args.push(command.into());
        self.folders.make().map_err(|e| Failure::internal(e.to_string()))?;
        let mut s = Spawn::new(&self.sandbox, &program)
            .args(args)
            .env(self.environment())
            .cwd(&self.folders.data())
            .ro(&self.version_dir)
            .rw(&self.folders.data())
            .rw(&self.folders.home())
            .rw(&self.folders.tmp())
            .network(self.manifest.permissions.network.allowed())
            .limits(self.limits(command))
            .low_priority(true);
        if let Some(prefix) = outside_prefix(&program) {
            s = s.ro(&prefix);
        }
        for p in &self.read_only {
            if p.is_dir() {
                s = s.ro(p);
            }
        }
        if let Some(f) = folder {
            s = s.rw(f);
        }
        Ok(s)
    }

    /// Run one command: the answer's object when `ok` is true, else the plugin's error.
    pub async fn run(&self, command: &str, request: Option<Value>, call: &Call, folder: Option<&Path>) -> Result<Map<String, Value>, Failure> {
        if command != "describe" && command != "status" {
            let until = *self.paused_until.lock().unwrap();
            if let Some(t) = until.filter(|t| *t > Instant::now()) {
                let secs = (t - Instant::now()).as_secs().max(1);
                return Err(Failure { code: Code::RateLimited, message: format!("{} asked Encore to slow down: no new command for {secs} seconds", self.name()), retry_after: Some(secs as f64) });
            }
        }
        let spawn = self.spawn_for(command, folder)?;
        let timeout = Duration::from_secs(self.manifest.timeout(command));
        let mut child = spawn
            .spawn_tokio(|c| {
                c.stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
            })
            .map_err(|e| Failure::internal(e.to_string()))?;
        let pid = child.id();
        if let Some(p) = pid {
            self.running.lock().unwrap().insert(p);
        }
        let outcome = self.supervise(&mut child, command, request, call, timeout).await;
        if let Some(p) = pid {
            self.running.lock().unwrap().remove(&p);
            // a process it left behind in its group has no business running on
            if !sandbox::group_members(p as libc::pid_t).is_empty() {
                // SAFETY: kill only sends a signal to the group the plugin ran in
                unsafe {
                    libc::kill(-(p as libc::pid_t), libc::SIGKILL);
                }
            }
        }
        let (stdout, last_err, status) = match outcome {
            Ok(x) => x,
            Err(Stopped::Timeout) => {
                self.log.line(command, &format!("stopped: no answer within {} seconds", timeout.as_secs()));
                return Err(Failure::internal(format!("{} did not answer within {} seconds", self.name(), timeout.as_secs())));
            }
            Err(Stopped::Cancelled) => {
                self.log.line(command, "stopped: cancelled");
                return Err(Failure::cancelled());
            }
        };
        if stdout.len() > MAX_ANSWER {
            return Err(Failure::internal(format!("{} answered with more than {} MB", self.name(), MAX_ANSWER >> 20)));
        }
        let text = String::from_utf8_lossy(&stdout);
        let Some(answer) = text.lines().rev().filter_map(|l| serde_json::from_str::<Value>(l.trim()).ok()).find(Value::is_object) else {
            let why = last_err.map(|l| format!(": {l}")).unwrap_or_default();
            let code = status.map(|s| format!(" (exit status {s})")).unwrap_or_default();
            self.log.line(command, &format!("ended without an answer{code}"));
            return Err(Failure::internal(format!("{} ended without an answer{code}{why}", self.name())));
        };
        let Value::Object(answer) = answer else { unreachable!() };
        if answer.get("ok").and_then(Value::as_bool) == Some(true) {
            return Ok(answer);
        }
        let code = Code::parse(answer.get("code").and_then(Value::as_str).unwrap_or("internal"));
        let message = answer
            .get("error")
            .and_then(Value::as_str)
            .map(|s| s.chars().take(2000).collect::<String>())
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| format!("{} reported an error without saying which", self.name()));
        let mut f = Failure::new(code, message);
        if code == Code::RateLimited {
            let secs = answer.get("retry_after").and_then(Value::as_f64).filter(|s| *s > 0.0 && s.is_finite()).unwrap_or(60.0).min(86400.0);
            f.retry_after = Some(secs);
            *self.paused_until.lock().unwrap() = Some(Instant::now() + Duration::from_secs_f64(secs));
        }
        self.log.line(command, &format!("error ({}): {}", code.as_str(), f.message));
        Err(f)
    }

    /// Feed the request, read the answer and the log, wait for the end (or the time limit, or a cancel) →
    /// (stdout, the last stderr line, the exit status).
    async fn supervise(&self, child: &mut tokio::process::Child, command: &str, request: Option<Value>, call: &Call, timeout: Duration) -> Result<(Vec<u8>, Option<String>, Option<i32>), Stopped> {
        let mut stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let body = request.map(|r| format!("{r}\n")).unwrap_or_default();
        let writer = tokio::spawn(async move {
            if let Some(mut s) = stdin.take() {
                let _ = s.write_all(body.as_bytes()).await;
                let _ = s.shutdown().await;
            }
        });
        let reader = tokio::spawn(async move {
            let mut out = Vec::new();
            if let Some(mut s) = stdout {
                let _ = (&mut s).take(MAX_ANSWER as u64 + 1).read_to_end(&mut out).await;
                // past the limit: keep reading into nothing, so the plugin is not stuck on a full pipe
                let _ = tokio::io::copy(&mut s, &mut tokio::io::sink()).await;
            }
            out
        });
        let log = self.log.clone();
        let progress = call.progress.clone();
        let cmd = command.to_string();
        let errors = tokio::spawn(async move {
            let mut last: Option<String> = None;
            let Some(s) = stderr else { return last };
            let mut r = tokio::io::BufReader::new(s);
            let mut buf = Vec::new();
            loop {
                buf.clear();
                match (&mut r).take(64 << 10).read_until(b'\n', &mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                let line = String::from_utf8_lossy(&buf).trim_end_matches(['\n', '\r']).to_string();
                if line.trim().is_empty() {
                    continue;
                }
                if let Some(rest) = line.strip_prefix("progress ") {
                    let (pct, text) = rest.split_once(' ').map(|(a, b)| (a, Some(b.trim().to_string()))).unwrap_or((rest, None));
                    if let Ok(p) = pct.trim().parse::<f64>() {
                        if let Some(cb) = &progress {
                            cb(p.clamp(0.0, 100.0), text.filter(|t| !t.is_empty()));
                        }
                        continue;
                    }
                }
                let text = line.strip_prefix("log ").unwrap_or(&line).to_string();
                log.line(&cmd, &text);
                last = Some(text);
            }
            last
        });
        let ended = tokio::select! {
            s = child.wait() => Ok(s.ok().and_then(|s| s.code())),
            _ = tokio::time::sleep(timeout) => Err(Stopped::Timeout),
            _ = call.cancel.cancelled() => Err(Stopped::Cancelled),
        };
        if ended.is_err() {
            if let Some(pid) = child.id() {
                let full = self.sandbox.is_full();
                sandbox::stop_group(pid, full, GRACE, || matches!(child.try_wait(), Ok(Some(_)))).await;
            }
            let _ = child.wait().await;
        }
        // what it printed: the pipes close when it ends (a process it left holding them is killed above)
        let _ = joined(writer).await;
        let out: Vec<u8> = joined(reader).await.unwrap_or_default();
        let last: Option<String> = joined(errors).await.flatten();
        ended.map(|status| (out, last, status))
    }

    /// Stop every command running now (Disable, Remove, Encore stopping).
    pub async fn stop_all(&self) {
        let pids: Vec<u32> = self.running.lock().unwrap().iter().copied().collect();
        for pid in pids {
            sandbox::stop_group(pid, self.sandbox.is_full(), GRACE, || sandbox::group_members(pid as libc::pid_t).is_empty()).await;
        }
    }

    /// `describe`, checked against the manifest (docs/plugins/CONTRACT.md, "describe").
    pub async fn describe(&self) -> Result<Map<String, Value>, Failure> {
        let a = self.run("describe", None, &Call::default(), None).await?;
        check_describe(&self.manifest, &a).map_err(Failure::internal)?;
        Ok(a)
    }

    fn settings_request(call: &Call) -> Value {
        json!({ "settings": Value::Object(call.settings.clone()) })
    }
}

/// A reader's result, waited for at most 5 seconds.
async fn joined<T>(h: tokio::task::JoinHandle<T>) -> Option<T> {
    tokio::time::timeout(Duration::from_secs(5), h).await.ok().and_then(Result::ok)
}

/// `describe`'s answer fits the manifest: protocol 1, its id as `name`, its version and kind, and every
/// command the manifest lists.
pub fn check_describe(m: &Manifest, a: &Map<String, Value>) -> Result<(), String> {
    let name = &m.plugin.name;
    match a.get("protocol").and_then(Value::as_u64) {
        Some(p) if p == PROTOCOL as u64 => {}
        Some(p) => return Err(format!("{name} speaks protocol {p}; this Encore speaks {PROTOCOL}")),
        None => return Err(format!("{name}'s describe has no protocol")),
    }
    if a.get("name").and_then(Value::as_str) != Some(m.plugin.id.as_str()) {
        return Err(format!("{name}'s describe names {}, its manifest {}", a.get("name").and_then(Value::as_str).unwrap_or("nothing"), m.plugin.id));
    }
    if a.get("version").and_then(Value::as_str) != Some(m.plugin.version.as_str()) {
        return Err(format!("{name}'s describe says version {}, its manifest {}", a.get("version").and_then(Value::as_str).unwrap_or("nothing"), m.plugin.version));
    }
    if a.get("kind").and_then(Value::as_str).is_some_and(|k| k != m.plugin.kind) {
        return Err(format!("{name}'s describe says it is a {}, its manifest a {}", a["kind"].as_str().unwrap_or(""), m.plugin.kind));
    }
    let has: Vec<&str> = a.get("commands").and_then(Value::as_array).map(|c| c.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    let missing: Vec<&str> = m.plugin.commands.iter().map(String::as_str).filter(|c| !has.contains(c)).collect();
    if !missing.is_empty() {
        return Err(format!("{name}'s manifest promises {} but the program does not answer {}", missing.join(", "), if missing.len() == 1 { "it" } else { "them" }));
    }
    Ok(())
}

/// The results of a search, checked: objects with a `key` and a `title`, each key once, the score within
/// 0–100, a payload of at most 64 KB; at most `limit`. What does not fit is left out (and logged).
pub fn check_results(results: &[Value], limit: usize, log: &Log) -> Vec<Value> {
    let mut seen = HashSet::new();
    let mut out = vec![];
    for r in results {
        let Some(obj) = r.as_object() else { continue };
        let key = obj.get("key").and_then(Value::as_str).unwrap_or("").trim();
        let title = obj.get("title").and_then(Value::as_str).unwrap_or("").trim();
        if key.is_empty() || title.is_empty() {
            log.line("search", "a result without a key or a title was left out");
            continue;
        }
        if !seen.insert(key.to_string()) {
            continue;
        }
        if obj.get("payload").is_some_and(|p| p.to_string().len() > MAX_PAYLOAD) {
            log.line("search", &format!("result {key}: its payload is larger than 64 KB, left out"));
            continue;
        }
        let mut obj = obj.clone();
        if let Some(s) = obj.get("score").and_then(Value::as_f64) {
            obj.insert("score".into(), json!(s.clamp(0.0, 100.0)));
        }
        out.push(Value::Object(obj));
        if out.len() >= limit {
            break;
        }
    }
    out
}

/// The files of a download, checked: each inside `folder` (links resolved), a regular file, an audio file by
/// its extension, not a `.part`; at least one.
pub fn check_files(answer: &Map<String, Value>, folder: &Path) -> Result<Vec<FileEntry>, String> {
    let root = std::fs::canonicalize(folder).map_err(|e| format!("the download folder {} is gone: {e}", folder.display()))?;
    let files = answer.get("files").and_then(Value::as_array).ok_or("the download answer has no files")?;
    if files.is_empty() {
        return Err("the download delivered no file".into());
    }
    let mut out = vec![];
    for f in files {
        let raw = f.get("path").and_then(Value::as_str).ok_or("a downloaded file has no path")?;
        let p = Path::new(raw);
        let p = if p.is_absolute() { p.to_path_buf() } else { folder.join(p) };
        let real = std::fs::canonicalize(&p).map_err(|_| format!("{raw} is not there"))?;
        if !real.starts_with(&root) {
            return Err(format!("{raw} is outside the download folder"));
        }
        if !std::fs::metadata(&real).map(|m| m.is_file()).unwrap_or(false) {
            return Err(format!("{raw} is not a file"));
        }
        let ext = real.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        if !AUDIO_EXT.contains(&ext.as_str()) {
            return Err(format!("{raw} is not an audio file Encore takes"));
        }
        let s = |k: &str| f.get(k).and_then(Value::as_str).map(str::to_string).filter(|v| !v.is_empty());
        out.push(FileEntry {
            size: std::fs::metadata(&real).ok().map(|m| m.len()),
            path: real,
            title: s("title"),
            artist: s("artist"),
            album: s("album"),
            track: f.get("track").and_then(Value::as_i64),
            disc: f.get("disc").and_then(Value::as_i64),
            duration: f.get("duration").and_then(Value::as_f64),
            format: s("format").map(|x| x.to_ascii_lowercase()).or(Some(ext)),
            isrc: s("isrc"),
        });
    }
    Ok(out)
}

impl Plugin for ProcessPlugin {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    fn status<'a>(&'a self, call: &'a Call) -> BoxFuture<'a, Result<Status, Failure>> {
        Box::pin(async move {
            if !self.manifest.answers("status") {
                return Ok(Status { message: "Ready".into(), signed_in: None });
            }
            let a = self.run("status", Some(Self::settings_request(call)), call, None).await?;
            Ok(Status {
                message: a.get("message").and_then(Value::as_str).unwrap_or("Ready").chars().take(500).collect(),
                signed_in: a.get("signed_in").and_then(Value::as_bool),
            })
        })
    }

    fn test<'a>(&'a self, call: &'a Call) -> BoxFuture<'a, Result<String, Failure>> {
        Box::pin(async move {
            if !self.manifest.answers("test") {
                return Err(Failure::unsupported(format!("{} has no test", self.name())));
            }
            let a = self.run("test", Some(Self::settings_request(call)), call, None).await?;
            Ok(a.get("message").and_then(Value::as_str).unwrap_or("Passed").chars().take(500).collect())
        })
    }

    fn search<'a>(&'a self, call: &'a Call, request: &'a Value) -> BoxFuture<'a, Result<Vec<Value>, Failure>> {
        Box::pin(async move {
            if !self.manifest.answers("search") {
                return Err(Failure::unsupported(format!("{} does not search", self.name())));
            }
            let kind = request.get("kind").and_then(Value::as_str).unwrap_or("track").to_string();
            if !self.manifest.plugin.kinds.contains(&kind) || self.unsupported_kinds.lock().unwrap().contains(&kind) {
                return Err(Failure::unsupported(format!("{} does not search for {kind} requests", self.name())));
            }
            let limit = request.get("limit").and_then(Value::as_u64).unwrap_or(25).clamp(1, 200);
            let mut req = request.as_object().cloned().unwrap_or_default();
            req.insert("kind".into(), json!(kind));
            req.insert("limit".into(), json!(limit));
            req.insert("settings".into(), Value::Object(call.settings.clone()));
            match self.run("search", Some(Value::Object(req)), call, None).await {
                Ok(a) => {
                    let results = a.get("results").and_then(Value::as_array).cloned().unwrap_or_default();
                    Ok(check_results(&results, limit as usize, &self.log))
                }
                Err(f) if f.code == Code::Unsupported => {
                    self.unsupported_kinds.lock().unwrap().insert(kind);
                    Err(f)
                }
                Err(f) => Err(f),
            }
        })
    }

    fn download<'a>(&'a self, call: &'a Call, request: &'a Value, folder: &'a Path) -> BoxFuture<'a, Result<Downloaded, Failure>> {
        Box::pin(async move {
            if !self.manifest.answers("download") {
                return Err(Failure::unsupported(format!("{} does not download", self.name())));
            }
            fsx::mkdir_all(folder, Step::Plugin).map_err(|e| Failure::internal(e.to_string()))?;
            let mut req = request.as_object().cloned().unwrap_or_default();
            req.insert("folder".into(), json!(folder.display().to_string()));
            req.insert("settings".into(), Value::Object(call.settings.clone()));
            let a = self.run("download", Some(Value::Object(req)), call, Some(folder)).await?;
            let files = check_files(&a, folder).map_err(|e| {
                self.log.line("download", &e);
                Failure::internal(format!("{}: {e}", self.name()))
            })?;
            let s = |k: &str| a.get(k).and_then(Value::as_str).map(str::to_string).filter(|v| !v.is_empty());
            Ok(Downloaded { files, cover_url: s("cover_url"), source_url: s("source_url"), message: s("message") })
        })
    }

    fn installed<'a>(&'a self, call: &'a Call, _version: Option<&'a str>) -> BoxFuture<'a, Result<(), Failure>> {
        Box::pin(async move {
            if !self.manifest.answers("update") {
                return Ok(());
            }
            let req = json!({ "settings": Value::Object(call.settings.clone()), "version": self.manifest.plugin.version });
            let a = self.run("update", Some(req), call, None).await?;
            if let Some(m) = a.get("message").and_then(Value::as_str) {
                self.log.line("update", m);
            }
            Ok(())
        })
    }

    fn removing<'a>(&'a self) -> BoxFuture<'a, ()> {
        Box::pin(self.stop_all())
    }

    fn enable<'a>(&'a self, on: bool) -> BoxFuture<'a, Result<(), Failure>> {
        Box::pin(async move {
            if !on {
                self.stop_all().await;
            }
            Ok(())
        })
    }
}
