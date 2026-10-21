//! Lossless downloads from Qobuz / Tidal / Deezer through the external `streamrip` tool.
//! Encore only finds the right release (ISRC or version-aware search) and hands the link to streamrip,
//! which signs in with your own subscription.
use crate::Ctx;
use encore_core::audio::STREAMRIP_EXTS as AUDIO;
use encore_core::fsx::{self, Step, StepError};
use encore_library::matcher::{compare, describe};
use regex::Regex;
use serde_json::Value;
use std::path::{Path, PathBuf};

const DEFAULT_CMD: &str = "{bin} --config-path {config} --folder {folder} --quality {quality} --no-db --no-progress url {url}";

/// The streamrip to run: the owner's own copy (streamrip_bin) when set, else the newest that runs of the
/// plugin's venv, the venv an Encore before plugins kept (adopted), an older image's /opt/tools/bin and PATH
/// (encore_system::tools). None while the streamrip plugin is not installed.
pub fn binary(st: &impl Ctx) -> Option<String> {
    encore_system::tools::path(st, "streamrip").map(|p| p.to_string_lossy().into())
}

/// Qobuz, Tidal and Deezer are sources now: the streamrip plugin is installed and switched on, streamrip is on
/// in its settings, and its program is there.
pub fn enabled(st: &impl Ctx) -> bool {
    plugin_on(st) && st.cfg().bool("streamrip_enabled") && binary(st).is_some()
}

/// The streamrip plugin is installed and switched on.
pub fn plugin_on(st: &impl Ctx) -> bool {
    st.plugins().enabled("streamrip")
}

pub fn config_path(st: &impl Ctx) -> PathBuf {
    st.data_dir().join("streamrip").join("config.toml")
}

/// Minimal in-place TOML edit (keeps the user's comments and other settings).
pub fn set_toml(text: &str, section: &str, key: &str, val: &str) -> String {
    let sec = Regex::new(&format!(r"(?m)^\[{}\]\s*$", regex::escape(section))).unwrap();
    let Some(m) = sec.find(text) else { return format!("{text}\n[{section}]\n{key} = {val}\n") };
    let start = m.end();
    let end = Regex::new(r"(?m)^\[").unwrap().find(&text[start..]).map(|n| start + n.start()).unwrap_or(text.len());
    let body = &text[start..end];
    let line = Regex::new(&format!(r"(?m)^{}\s*=.*$", regex::escape(key))).unwrap();
    let body = if line.is_match(body) { line.replacen(body, 1, format!("{key} = {val}").as_str()).to_string() } else { format!("{}\n{key} = {val}\n\n", body.trim_end_matches('\n')) };
    format!("{}{}{}", &text[..start], body, &text[end..])
}

fn tstr(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

// ---- the config file ----------------------------------------------------------------------------
// streamrip reads one TOML file. Encore lets streamrip write the defaults (it does so for a path that
// has no file yet), checks that the file loads with `rip --config-path X config path`, and when it does
// not – a half-written file, the format of an older streamrip, a key it does not know – moves the file
// aside and writes a fresh one, carrying the sign-ins over. The credentials from Encore's settings go
// in before every run, not only when the file is new.

pub const NOT_INSTALLED_MSG: &str = "streamrip is not installed – install the streamrip plugin under Settings → Plugins (or set its path)";
/// Deezer runs need the ARL cookie; streamrip would ask for it on a terminal, which a server has none of.
pub const ARL_MSG: &str = "Deezer needs an ARL cookie – Settings → Plugins → streamrip";
pub const CONFIG_BROKEN_MSG: &str = "streamrip can't load its config – press Reset streamrip config in Settings → Plugins → streamrip";
/// The line streamrip prints when its config does not load (the text after the colon says why).
const CONFIG_ERROR: &str = "Error loading config";
/// How much of a run's output the request's log keeps.
const LOG_CAP: usize = 20_000;
/// How much of it the request's message shows.
const MESSAGE_CAP: usize = 300;
/// Time limit for a config check or regeneration (a Python start-up, normally a second).
const CHECK_SECS: u64 = 90;
/// Time limit for a download run.
const RUN_SECS: u64 = 3600;

/// The body of a `[section]` (without the header), or "" when there is none.
pub fn section_body<'a>(text: &'a str, name: &str) -> &'a str {
    let Some(m) = Regex::new(&format!(r"(?m)^\s*\[{}\]\s*$", regex::escape(name))).unwrap().find(text) else { return "" };
    let body = &text[m.end()..];
    let end = Regex::new(r"(?m)^\s*\[").unwrap().find(body).map(|n| n.start()).unwrap_or(body.len());
    &body[..end]
}

/// One `key = value` of a section as plain text ("" when missing).
pub fn toml_value(text: &str, section: &str, key: &str) -> String {
    let line = Regex::new(&format!(r"(?m)^\s*{}\s*=\s*(.*)$", regex::escape(key))).unwrap();
    line.captures(section_body(text, section)).map(|c| toml_scalar(&c[1])).unwrap_or_default()
}

/// streamrip 2.x names the Qobuz login `email_or_userid` / `password_or_token` (with `use_auth_token`);
/// 1.x had `user_id` / `auth_token`. The file streamrip wrote says which it is.
fn qobuz_v2(text: &str) -> bool {
    let q = section_body(text, "qobuz");
    q.contains("email_or_userid") || !q.contains("auth_token")
}

/// Write the credentials entered in Encore's settings into streamrip's config file. Keys the settings
/// leave empty stay as they are in the file, so a login typed into config.toml by hand is kept.
pub fn apply_credentials(st: &impl Ctx) -> anyhow::Result<PathBuf> {
    apply_credentials_for(st, None, None)
}

/// [`apply_credentials`] for the streamrip of `version` (its `--version`, when known) whose own code does or
/// does not read the update-check switch (`switch`, when known): the update check is switched off the way
/// that streamrip reads it ([`quiet_config`]).
pub fn apply_credentials_for(st: &impl Ctx, version: Option<&str>, switch: Option<bool>) -> anyhow::Result<PathBuf> {
    let path = config_path(st);
    fsx::mkdir_all(path.parent().unwrap(), Step::Config)?;
    // a file that is there but can't be read is never written over with the credentials alone
    let mut text = read_config(&path)?;
    let arl = st.cfg().str("streamrip_deezer_arl");
    if !arl.is_empty() {
        text = set_toml(&text, "deezer", "arl", &tstr(&arl));
    }
    let (qu, qt) = (st.cfg().str("streamrip_qobuz_user"), st.cfg().str("streamrip_qobuz_token"));
    if !qu.is_empty() && !qt.is_empty() {
        if qobuz_v2(&text) {
            text = set_toml(&text, "qobuz", "use_auth_token", "true");
            text = set_toml(&text, "qobuz", "email_or_userid", &tstr(&qu));
            text = set_toml(&text, "qobuz", "password_or_token", &tstr(&qt));
        } else {
            text = set_toml(&text, "qobuz", "user_id", &tstr(&qu));
            text = set_toml(&text, "qobuz", "auth_token", &tstr(&qt));
        }
    }
    text = set_toml(&text, "database", "downloads_enabled", "false");
    text = quiet_config(&text, version, switch);
    write_config(&path, &text)?;
    Ok(path)
}

/// streamrip's config text, "" when there is no file yet. Err (naming "read <path>") when the file is there
/// but can't be read: an EIO must not read as an empty config that is then written back.
pub fn read_config(path: &Path) -> Result<String, StepError> {
    fsx::fault_point(Step::Config, "read", path)?;
    match std::fs::read_to_string(path) {
        Ok(t) => Ok(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(StepError::new(Step::Config, "read", path, None, e)),
    }
}

/// Write streamrip's config whole or not at all: into a new file beside it, then renamed over it, so a crash
/// or a full disk halfway never leaves a cut-off file (the Tidal sign-in lives only there). The new file
/// keeps the old one's permissions (it holds the sign-ins), and a symlinked config stays a symlink. The
/// error names "write <config>" (or the rename).
pub fn write_config(path: &Path, text: &str) -> Result<(), StepError> {
    let target = match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()),
        _ => path.to_path_buf(),
    };
    fsx::fault_point(Step::Config, "write", &target)?;
    let name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "config.toml".into());
    let tmp = target.with_file_name(format!(".{name}.encore-new"));
    let written = std::fs::write(&tmp, text).and_then(|_| match std::fs::metadata(&target) {
        Ok(m) => std::fs::set_permissions(&tmp, m.permissions()),
        Err(_) => Ok(()),
    });
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(StepError::new(Step::Config, "write", &target, None, e));
    }
    fsx::rename(&tmp, &target, Step::Config).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// The config's switches for a run without a terminal, written before every run:
/// - `[cli] progress_bars = false`, what `--no-progress` does (for a command line of the owner's without it);
/// - the update check off, the way the streamrip in use reads it. The fork (Stensel8/streamrip) from
///   2.4.10 reads `[cli] no_update_check = true` (its template carries the line commented out); nathom's 2.x
///   reads `[misc] check_for_updates = false`. The fork's 2.4.9 (the image's before 0.29) has no switch at all: it asks
///   GitHub before every command (10 s at most, never fatal, its notice lands in the output), and its
///   `[cli]` refuses a key it does not know ("CliConfig.__init__() got an unexpected keyword argument"),
///   so nothing is written for it.
///
/// `switch` is what the streamrip's own code says (see [`reads_update_switch`]) and wins over the version:
/// the fork's v2.4.10 tag still calls itself 2.4.9.
///
/// Only keys whose section the file already has are set: a section of only Encore's keys would not load.
pub fn quiet_config(text: &str, version: Option<&str>, switch: Option<bool>) -> String {
    let mut text = text.to_string();
    let cli = section_body(&text, "cli").to_string();
    let has_key = |body: &str, key: &str| Regex::new(&format!(r"(?m)^\s*{key}\s*=")).unwrap().is_match(body);
    if has_key(&cli, "progress_bars") {
        text = set_toml(&text, "cli", "progress_bars", "false");
        let reads_it = switch.unwrap_or_else(|| match version.and_then(encore_core::version::extract_version) {
            Some(v) => encore_core::version::compare(&v, "2.4.10") != std::cmp::Ordering::Less,
            // unknown (a streamrip of the owner's): the template of a streamrip that reads it names the key
            None => cli.contains("no_update_check"),
        });
        if reads_it {
            text = set_toml(&text, "cli", "no_update_check", "true");
        }
    }
    if has_key(section_body(&text, "misc"), "check_for_updates") {
        text = set_toml(&text, "misc", "check_for_updates", "false");
    }
    text
}

// ---- running streamrip without a terminal ---------------------------------------------------------

/// How a streamrip run ended.
#[derive(Debug)]
pub struct RunOutput {
    /// Its exit; None when it was stopped at the time limit.
    pub status: Option<std::process::ExitStatus>,
    /// stdout and stderr together, in the order they came, read to the end (the middle of a huge output
    /// is left out, see [`OUTPUT_KEEP`]).
    pub output: String,
}

impl RunOutput {
    pub fn success(&self) -> bool {
        self.status.is_some_and(|s| s.success())
    }
}

/// How much of a run's output is held while it runs: the first and the last this many bytes.
pub const OUTPUT_KEEP: usize = 1 << 20;

/// A run's output as it comes, from both pipes: whole lines, in the order they came.
#[derive(Default)]
struct Collected {
    head: Vec<u8>,
    tail: std::collections::VecDeque<u8>,
    left_out: usize,
}

impl Collected {
    fn push(&mut self, bytes: &[u8]) {
        let room = OUTPUT_KEEP.saturating_sub(self.head.len());
        let (h, rest) = bytes.split_at(room.min(bytes.len()));
        self.head.extend_from_slice(h);
        self.tail.extend(rest);
        while self.tail.len() > OUTPUT_KEEP {
            self.tail.pop_front();
            self.left_out += 1;
        }
    }

    fn text(&self) -> String {
        let mut out = String::from_utf8_lossy(&self.head).into_owned();
        if self.left_out > 0 {
            out.push_str(&format!("\n… ({} bytes of output left out) …\n", self.left_out));
        }
        let tail: Vec<u8> = self.tail.iter().copied().collect();
        out.push_str(&String::from_utf8_lossy(&tail));
        out
    }
}

/// Read one pipe to its end into `sink`, a whole line at a time (so stdout and stderr don't cut into each
/// other's lines; a line that never ends is passed on every 64 KiB).
async fn pump(mut r: impl tokio::io::AsyncRead + Unpin, sink: std::sync::Arc<std::sync::Mutex<Collected>>) {
    use tokio::io::AsyncReadExt;
    let mut buf = vec![0u8; 8192];
    let mut pending: Vec<u8> = vec![];
    loop {
        match r.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                pending.extend_from_slice(&buf[..n]);
                if let Some(i) = pending.iter().rposition(|b| *b == b'\n') {
                    let rest = pending.split_off(i + 1);
                    sink.lock().unwrap().push(&pending);
                    pending = rest;
                } else if pending.len() >= 64 * 1024 {
                    sink.lock().unwrap().push(&pending);
                    pending.clear();
                }
            }
        }
    }
    if !pending.is_empty() {
        pending.push(b'\n');
        sink.lock().unwrap().push(&pending);
    }
}

/// The session a run started (its process group has the run's pid): stopped as a whole, the run's own
/// children (ffmpeg, a shell's program) with it, when the run is given up – the time limit, or the request's
/// task cancelled while it runs.
struct Session(Option<i32>);

impl Session {
    fn stop(&mut self) {
        if let Some(pid) = self.0.take() {
            // the run's pid is still ours (not waited for yet), or its group still has members: never another's
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Run streamrip with nothing interactive about it, the same for a download and for a config check:
/// - stdin is /dev/null: a prompt (the Deezer ARL, a login) reads EOF at once instead of waiting;
/// - a session of its own (`setsid`): no controlling terminal even when the container runs with `tty: true`,
///   so a write to or a read from /dev/tty fails at once (ENXIO) instead of drawing on or waiting for a
///   terminal nobody sees;
/// - stdout and stderr piped and read to the end, together, for the request's log;
/// - plain output: TERM=dumb, NO_COLOR=1, COLUMNS=200 (the fork's rich console ignores COLUMNS and takes 80
///   without a terminal), PYTHONUNBUFFERED=1 (lines as they happen), PYTHONIOENCODING=utf-8, no forced colour;
/// - HOME is Encore's data folder (`home`): streamrip's app folder and caches go there, not into a home
///   folder Encore may not write.
///
/// At the time limit the whole session is stopped. Err only when the program could not be started; the
/// error names it ("download failed: start /config/tools/venv/bin/rip: Input/output error (os error 5)").
pub async fn run_quiet(program: &str, args: &[String], home: &Path, secs: u64, step: Step) -> Result<RunOutput, StepError> {
    use std::process::Stdio;
    let plain = [("TERM", "dumb"), ("NO_COLOR", "1"), ("COLUMNS", "200"), ("PYTHONUNBUFFERED", "1"), ("PYTHONIOENCODING", "utf-8")];
    // Encore's environment without Encore's settings (encore_plugins::sandbox::program_env)
    let mut env: Vec<(String, String)> = encore_plugins::sandbox::program_env()
        .into_iter()
        .filter(|(k, _)| k != "HOME" && !plain.iter().any(|(p, _)| p == k))
        .collect();
    env.extend(plain.iter().map(|(k, v)| (k.to_string(), v.to_string())));
    env.push(("HOME".into(), home.to_string_lossy().into_owned()));
    // through the plugins' spawn builder: a session of its own (setsid), no new privileges, no other open files
    let spawn = encore_plugins::sandbox::Spawn::new(&encore_plugins::sandbox::Sandbox::limits_only(), Path::new(program)).args(args.iter().cloned()).env(env);
    let mut cmd = spawn.tokio_command();
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = fsx::spawn(step, program, || cmd.spawn())?;
    let mut session = Session(child.id().map(|p| p as i32));
    let sink = std::sync::Arc::new(std::sync::Mutex::new(Collected::default()));
    let readers: Vec<tokio::task::JoinHandle<()>> = [
        child.stdout.take().map(|o| tokio::spawn(pump(o, sink.clone()))),
        child.stderr.take().map(|e| tokio::spawn(pump(e, sink.clone()))),
    ].into_iter().flatten().collect();
    let status = match tokio::time::timeout(std::time::Duration::from_secs(secs), child.wait()).await {
        Ok(Ok(s)) => Some(s),
        Ok(Err(e)) => {
            sink.lock().unwrap().push(format!("(Encore lost track of streamrip: {e})\n").as_bytes());
            session.stop();
            None
        }
        Err(_) => {
            session.stop();
            let _ = child.kill().await;
            sink.lock().unwrap().push(format!("(Encore stopped streamrip after {} s)\n", secs).as_bytes());
            None
        }
    };
    // the pipes close once every process of the run has ended; one that keeps them open (a child left
    // behind) is not waited for long, and is stopped with its session
    let aborts: Vec<_> = readers.iter().map(|h| h.abort_handle()).collect();
    if tokio::time::timeout(std::time::Duration::from_secs(5), futures_util::future::join_all(readers)).await.is_err() {
        session.stop();
        aborts.iter().for_each(|a| a.abort());
    }
    // done: the run's pid is waited for and may be handed out again – never signal it from here on
    session.0 = None;
    let output = sink.lock().unwrap().text();
    Ok(RunOutput { status, output })
}

/// Run streamrip for a config check (`config path`, `config reset`): [`run_quiet`] with a short limit.
/// Ok((exited 0, combined output)).
async fn run_rip(bin: &str, args: &[&str], home: &Path, secs: u64) -> anyhow::Result<(bool, String)> {
    let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    let run = run_quiet(bin, &args, home, secs, Step::Config).await?;
    if run.status.is_none() {
        anyhow::bail!("streamrip did not answer within {secs} s");
    }
    Ok((run.success(), run.output))
}

/// Does the config at `path` load? `rip --config-path X config path` says so: a non-zero exit or the
/// fork's "Error loading config" line means no. Err carries the complaint, one line. `home`: Encore's data
/// folder (see [`run_quiet`]).
pub async fn validate_config(bin: &str, path: &Path, home: &Path) -> Result<(), String> {
    let p = path.to_string_lossy().to_string();
    match run_rip(bin, &["--config-path", &p, "config", "path"], home, CHECK_SECS).await {
        Err(e) => Err(format!("{e:#}")),
        Ok((true, text)) if !text.contains(CONFIG_ERROR) => Ok(()),
        Ok((_, text)) => Err(summary(&text, MESSAGE_CAP)),
    }
}

/// Move the config aside (config.toml.bak-<date>) and let streamrip write its defaults. Returns the backup.
async fn regenerate(bin: &str, path: &Path, home: &Path) -> anyhow::Result<Option<PathBuf>> {
    let backup = if path.exists() {
        let b = path.with_file_name(format!("config.toml.bak-{}", encore_db::backup::stamp(encore_db::now())));
        fsx::rename(path, &b, Step::Config)?;
        Some(b)
    } else {
        None
    };
    let p = path.to_string_lossy().to_string();
    // streamrip writes its default config for a path that has no file yet; `config reset` is the fallback
    run_rip(bin, &["--config-path", &p, "config", "path"], home, CHECK_SECS).await.map_err(|e| e.context("streamrip config path"))?;
    if !path.is_file() {
        let _ = run_rip(bin, &["--config-path", &p, "config", "reset", "-y"], home, CHECK_SECS).await;
    }
    if !path.is_file() {
        anyhow::bail!("streamrip did not write a default config at {}", path.display());
    }
    Ok(backup)
}

/// The sign-ins an old config held, written into a fresh one: the [tidal] tokens (and the user's own
/// client and hires_client), the Deezer ARL, the Qobuz pair under whichever key names the fresh file uses.
fn carry_over(old: &str, fresh: &str) -> String {
    let mut text = fresh.to_string();
    let t = parse_tidal_toml(old);
    for k in TIDAL_TOKEN_KEYS.iter().chain(["hires_client", "client_id", "client_secret"].iter()) {
        if let Some(v) = t.get(*k).filter(|v| !v.is_empty()) {
            text = set_toml(&text, "tidal", k, &tidal_toml_value(k, v));
        }
    }
    let arl = toml_value(old, "deezer", "arl");
    if !arl.is_empty() {
        text = set_toml(&text, "deezer", "arl", &tstr(&arl));
    }
    let user = [toml_value(old, "qobuz", "email_or_userid"), toml_value(old, "qobuz", "user_id")].into_iter().find(|v| !v.is_empty());
    let token = [toml_value(old, "qobuz", "password_or_token"), toml_value(old, "qobuz", "auth_token")].into_iter().find(|v| !v.is_empty());
    if let (Some(u), Some(k)) = (user, token) {
        if qobuz_v2(&text) {
            text = set_toml(&text, "qobuz", "use_auth_token", "true");
            text = set_toml(&text, "qobuz", "email_or_userid", &tstr(&u));
            text = set_toml(&text, "qobuz", "password_or_token", &tstr(&k));
        } else {
            text = set_toml(&text, "qobuz", "user_id", &tstr(&u));
            text = set_toml(&text, "qobuz", "auth_token", &tstr(&k));
        }
    }
    text
}

/// What `check_config` found and did.
#[derive(Debug, Default)]
pub struct ConfigCheck {
    pub path: PathBuf,
    /// A fresh file was written (the old one, if any, is `backup`).
    pub regenerated: bool,
    pub backup: Option<PathBuf>,
    /// Why the config still does not load (or why it could not be checked), when it does not.
    pub problem: Option<String>,
}

impl ConfigCheck {
    pub fn json(&self) -> Value {
        serde_json::json!({ "ok": self.problem.is_none(), "path": self.path.to_string_lossy(), "regenerated": self.regenerated,
            "backup": self.backup.as_ref().map(|b| b.to_string_lossy().to_string()), "message": self.message() })
    }

    pub fn message(&self) -> String {
        let where_ = self.path.display();
        match (&self.problem, self.regenerated) {
            (Some(p), true) => format!("streamrip wrote a fresh config at {where_}, but it still doesn't load: {p}"),
            (Some(p), false) => format!("streamrip can't load its config at {where_}: {p}"),
            (None, true) => match &self.backup {
                Some(b) => format!("streamrip config written fresh at {where_} – the old file is kept as {} and its Tidal sign-in, Deezer ARL and Qobuz login were carried over", b.display()),
                None => format!("streamrip config written fresh at {where_}"),
            },
            (None, false) => format!("streamrip config loads ({where_})"),
        }
    }
}

/// Make sure streamrip's config loads: a missing file is written from streamrip's defaults; one that
/// fails the check (or `force`) is backed up, regenerated and gets its sign-ins carried over. The
/// credentials from Encore's settings are written in every time.
pub async fn check_config(st: &impl Ctx, force: bool) -> anyhow::Result<ConfigCheck> {
    let path = config_path(st);
    fsx::mkdir_all(path.parent().unwrap(), Step::Config)?;
    let home = st.data_dir().to_path_buf();
    let mut out = ConfigCheck { path: path.clone(), ..Default::default() };
    let Some(bin) = binary(st) else {
        // nothing can run without the program; the credentials still land in the file for when it is there
        apply_credentials(st)?;
        out.problem = Some(NOT_INSTALLED_MSG.into());
        return Ok(out);
    };
    let why = if !path.is_file() {
        Some("no config file yet".to_string())
    } else if force {
        Some("reset asked for".to_string())
    } else {
        validate_config(&bin, &path, &home).await.err()
    };
    if let Some(why) = why {
        // the sign-ins to carry over: a file that can't be read stops here rather than lose them
        let old = read_config(&path)?;
        out.backup = regenerate(&bin, &path, &home).await?;
        out.regenerated = true;
        if !old.is_empty() {
            let fresh = fsx::read_to_string(&path, Step::Config)?;
            write_config(&path, &carry_over(&old, &fresh))?;
        }
        match &out.backup {
            Some(b) => st.db().log("warning", &format!("streamrip: config regenerated ({why}); the old file is kept as {}", b.display())),
            None => st.db().log("info", &format!("streamrip: config written at {} ({why})", path.display())),
        }
        out.problem = validate_config(&bin, &path, &home).await.err();
    }
    let version = version_of(st, &bin).await;
    apply_credentials_for(st, version.as_deref(), reads_update_switch(Path::new(&bin)))?;
    Ok(out)
}

/// Does the streamrip at `bin` read `[cli] no_update_check`? Its own code says so, where a venv keeps it
/// (`lib/python*/site-packages/streamrip/config.py` beside the program's `bin` folder): the fork's v2.4.10
/// tag still calls itself 2.4.9, so its version alone can't tell. None when the code is not found there.
fn reads_update_switch(bin: &Path) -> Option<bool> {
    let root = std::fs::canonicalize(bin).ok()?.parent()?.parent()?.to_path_buf();
    for dir in std::fs::read_dir(root.join("lib")).ok()?.flatten() {
        if let Ok(code) = std::fs::read_to_string(dir.path().join("site-packages/streamrip/config.py")) {
            return Some(code.contains("no_update_check"));
        }
    }
    None
}

/// The version of the streamrip at `bin` (its `--version`, kept by the tools' cache for 10 minutes; the
/// first look runs it, off the async threads).
async fn version_of(st: &impl Ctx, bin: &str) -> Option<String> {
    let (st, bin) = (st.clone(), PathBuf::from(bin));
    tokio::task::spawn_blocking(move || st.system().tools.version_of(&bin)).await.ok().flatten()
}

/// The config file, made to load when it can be (see `check_config`); its path.
pub async fn ensure_config(st: &impl Ctx) -> anyhow::Result<PathBuf> {
    check_config(st, false).await.map(|c| c.path)
}

/// "Reset streamrip config": a fresh file whatever the state of the old one, sign-ins carried over.
pub async fn reset_config(st: &impl Ctx) -> anyhow::Result<ConfigCheck> {
    check_config(st, true).await
}

/// Diagnostics: does streamrip load its config? Read-only – nothing is written or regenerated here.
pub async fn config_loads_check(st: &impl Ctx) -> Value {
    let path = config_path(st);
    let bin = binary(st).unwrap_or("streamrip".into());
    let (ok, detail) = match binary(st) {
        None => (false, NOT_INSTALLED_MSG.to_string()),
        Some(_) if !path.is_file() => (false, format!("No config file at {} yet – it is written on the first download, or press Reset streamrip config in Settings → Plugins → streamrip", path.display())),
        Some(bin) => match validate_config(&bin, &path, st.data_dir()).await {
            Ok(()) => (true, format!("OK – {}", path.display())),
            Err(e) => (false, format!("{e} – press Reset streamrip config in Settings → Plugins → streamrip")),
        },
    };
    serde_json::json!({ "name": "streamrip config loads", "kind": "tool", "request": format!("{bin} --config-path {} config path", path.display()), "ok": ok, "detail": detail })
}

/// The useful part of streamrip's output, one line: box drawing stripped, numbered progress lines
/// dropped, the last three lines that name an error (each with the line that continues it), capped.
fn summary(text: &str, max: usize) -> String {
    let boxes = Regex::new(r"[│╭╰╮╯─━┃]+").unwrap();
    let num = Regex::new(r"^\d+\s").unwrap();
    let errs = Regex::new(r"(?i)error|invalid|fail|not (found|available)|login|arl|token|subscription|traceback|exception").unwrap();
    let lines: Vec<String> = text.lines().map(|l| boxes.replace_all(l, " ").split_whitespace().collect::<Vec<_>>().join(" ")).filter(|l| !l.is_empty() && !num.is_match(l)).collect();
    let mut groups: Vec<String> = vec![];
    let mut i = 0;
    while i < lines.len() {
        if errs.is_match(&lines[i]) {
            let mut g = lines[i].clone();
            if i + 1 < lines.len() && !errs.is_match(&lines[i + 1]) {
                g.push(' ');
                g.push_str(&lines[i + 1]);
                i += 1;
            }
            groups.push(g);
        }
        i += 1;
    }
    let pick: Vec<String> = if groups.is_empty() { lines.iter().rev().take(3).rev().cloned().collect() } else { groups.into_iter().rev().take(3).rev().collect() };
    cap(&pick.join(" / "), max)
}

fn cap(s: &str, max: usize) -> String {
    if s.chars().count() <= max { s.to_string() } else { s.chars().take(max.saturating_sub(1)).collect::<String>() + "…" }
}

/// A run's output for the request's log: whole when it fits in `max` characters, else its first quarter
/// and its last three quarters (where the error is), with the length of the part left out between.
pub fn cap_log(text: &str, max: usize) -> String {
    let n = text.chars().count();
    if n <= max {
        return text.to_string();
    }
    let (head, tail) = (max / 4, max - max / 4);
    let first: String = text.chars().take(head).collect();
    let last: String = text.chars().skip(n - tail).collect();
    format!("{first}\n… ({} characters left out) …\n{last}", n - head - tail)
}

/// streamrip stopped to ask for the Deezer ARL (a prompt that reads EOF on a server).
fn asks_for_arl(text: &str) -> bool {
    let arl = Regex::new(r"(?i)\barl\b|Finding-Your-Deezer-ARL").unwrap();
    let prompt = Regex::new(r"(?i)enter your|paste|copy the arl|finding-your-deezer-arl|prompt|cookie|eoferror|eof when reading").unwrap();
    arl.is_match(text) && prompt.is_match(text)
}

/// The one-line reason a run produced nothing: the Deezer prompt and a config that does not load get
/// plain words; anything else the error lines of the output, first 300 characters.
pub fn failure_message(text: &str) -> String {
    if asks_for_arl(text) {
        return ARL_MSG.to_string();
    }
    if text.contains(CONFIG_ERROR) {
        return format!("{CONFIG_BROKEN_MSG} – {}", summary(text, MESSAGE_CAP));
    }
    format!("streamrip produced no files: {}", summary(text, MESSAGE_CAP))
}

/// The reason a run produced nothing, from its output and how it ended: [`failure_message`], or what is
/// known when it printed nothing, or that it was stopped at the time limit.
fn run_failure(run: &RunOutput, secs: u64) -> String {
    let words = if run.output.trim().is_empty() { None } else { Some(failure_message(&run.output)) };
    match (run.status, words) {
        (None, Some(w)) => format!("streamrip did not finish within {} minutes and was stopped – {w}", secs / 60),
        (None, None) => format!("streamrip did not finish within {} minutes and was stopped", secs / 60),
        (Some(_), Some(w)) => w,
        (Some(s), None) => match s.code() {
            Some(c) => format!("streamrip produced no files and printed nothing (exit code {c})"),
            None => "streamrip produced no files and printed nothing (it was stopped by a signal)".to_string(),
        },
    }
}

/// A run that produced no files: the message for the request (streamrip's own words), its whole output for
/// the request's log.
#[derive(Debug)]
pub struct RipError {
    pub message: String,
    pub log: String,
}

impl std::fmt::Display for RipError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for RipError {}

/// The output of a failed run, when the error came from one (for the request's log).
pub fn run_log(e: &anyhow::Error) -> Option<&str> {
    // the error itself, or the context of a file error after the run ("its folder can't be read")
    e.downcast_ref::<RipError>().or_else(|| e.chain().find_map(|c| c.downcast_ref::<RipError>())).map(|r| r.log.as_str()).filter(|l| !l.is_empty())
}

// ---- resolving a request to a service link ------------------------------------------------------

pub async fn deezer_isrc(st: &impl Ctx, isrc: &str) -> Option<Value> {
    let d: Value = encore_http::deezer::track_by_isrc(st, isrc).send().await.ok()?.json().await.ok()?;
    d["error"].is_null().then_some(d)
}

async fn deezer_search(st: &impl Ctx, kind: &str, q: &str) -> Vec<Value> {
    let Ok(r) = encore_http::deezer::search(st, kind, q, 15).send().await else { return vec![] };
    r.json::<Value>().await.ok().and_then(|d| d["data"].as_array().cloned()).unwrap_or_default()
}

/// Tidal developer-API token (client credentials), cached until it expires.
pub async fn tidal_token(st: &impl Ctx) -> anyhow::Result<String> {
    if let Some(v) = st.cache_get("tidal_token") {
        return Ok(v.as_str().unwrap_or("").to_string());
    }
    let (cid, sec) = (st.cfg().str("tidal_client_id"), st.cfg().str("tidal_client_secret"));
    if cid.is_empty() || sec.is_empty() {
        anyhow::bail!("Tidal needs a Client ID/Secret from developer.tidal.com (Settings → Integrations)");
    }
    let r = st.http().post("https://auth.tidal.com/v1/oauth2/token").basic_auth(cid, Some(sec)).form(&[("grant_type", "client_credentials")]).send().await?;
    if !r.status().is_success() {
        anyhow::bail!("Tidal auth failed: {}", r.text().await.unwrap_or_default().chars().take(200).collect::<String>());
    }
    let d: Value = r.json().await?;
    let tok = d["access_token"].as_str().unwrap_or("").to_string();
    st.cache_put("tidal_token", serde_json::json!(tok), (d["expires_in"].as_u64().unwrap_or(3600)).saturating_sub(60));
    Ok(tok)
}

async fn tidal_by_isrc(st: &impl Ctx, isrc: &str, albums: bool) -> anyhow::Result<Option<String>> {
    let tok = tidal_token(st).await?;
    let mut q = vec![("countryCode", "US"), ("filter[isrc]", isrc)];
    if albums {
        q.push(("include", "albums"));
    }
    // an error answer is Tidal's words in the log, not "not on Tidal"
    let d: Value = encore_http::tidal::read(encore_http::tidal::get(st, &tok, "/tracks").query(&q).send().await?).await?;
    Ok(if albums {
        d["included"].as_array().and_then(|i| i.iter().find(|x| x["type"] == "albums")).and_then(|x| x["id"].as_str()).map(|id| format!("https://tidal.com/browse/album/{id}"))
    } else {
        d["data"].as_array().and_then(|a| a.first()).and_then(|x| x["id"].as_str()).map(|id| format!("https://tidal.com/browse/track/{id}"))
    })
}

pub const SERVICES: [&str; 3] = ["qobuz", "tidal", "deezer"];

/// The streamrip services to search, in the order they appear in the one Source order setting
/// (e.g. "tidal, deezer, slskd, ytdlp"). Older setups that just say "streamrip" keep their old list.
pub fn services(st: &impl Ctx) -> Vec<String> {
    let order: Vec<String> = st.cfg().str("source_order").split(',').map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()).collect();
    let picked: Vec<String> = order.iter().filter(|s| SERVICES.contains(&s.as_str())).cloned().collect();
    if !picked.is_empty() {
        return picked;
    }
    let legacy: Vec<String> = st.cfg().str("streamrip_sources").split(',').map(|s| s.trim().to_lowercase()).filter(|s| SERVICES.contains(&s.as_str())).collect();
    if legacy.is_empty() { vec!["tidal".into(), "deezer".into()] } else { legacy }
}

pub async fn resolve_album(st: &impl Ctx, artist: &str, album: &str, track_count: Option<usize>, first_isrc: Option<&str>) -> Vec<String> {
    let mut urls = vec![];
    let want = describe(album, artist, None, None, None);
    for svc in services(st) {
        if svc == "deezer" {
            if let Some(t) = match first_isrc { Some(i) => deezer_isrc(st, i).await, None => None } {
                if let Some(id) = t["album"]["id"].as_i64() {
                    urls.push(format!("https://www.deezer.com/album/{id}"));
                    continue;
                }
            }
            let mut res = deezer_search(st, "album", &format!("artist:\"{artist}\" album:\"{album}\"")).await;
            if res.is_empty() {
                res = deezer_search(st, "album", &format!("{artist} {album}")).await;
            }
            let mut best: Option<(f64, i64)> = None;
            for a in res {
                let v = compare(&want, &describe(a["title"].as_str().unwrap_or(""), a["artist"]["name"].as_str().unwrap_or(""), None, None, None), true);
                if !v.matched {
                    continue;
                }
                let s = v.score + if track_count.is_some_and(|n| a["nb_tracks"].as_u64() == Some(n as u64)) { 0.2 } else { 0.0 };
                if best.is_none_or(|(b, _)| s > b) {
                    best = Some((s, a["id"].as_i64().unwrap_or(0)));
                }
            }
            if let Some((_, id)) = best {
                urls.push(format!("https://www.deezer.com/album/{id}"));
            }
        } else if svc == "tidal" && first_isrc.is_some() && !st.cfg().str("tidal_client_id").is_empty() {
            match tidal_by_isrc(st, first_isrc.unwrap(), true).await {
                Ok(Some(u)) => urls.push(u),
                Ok(None) => {}
                Err(e) => st.db().log("warning", &format!("streamrip: tidal lookup failed: {e}")),
            }
        }
    }
    urls
}

pub async fn resolve_track(st: &impl Ctx, artist: &str, title: &str, isrc: Option<&str>, length: Option<f64>) -> Vec<String> {
    let mut urls = vec![];
    let want = describe(title, artist, length, isrc, None);
    for svc in services(st) {
        if svc == "deezer" {
            let mut t = match isrc { Some(i) => deezer_isrc(st, i).await, None => None };
            if t.is_none() {
                for c in deezer_search(st, "track", &format!("artist:\"{artist}\" track:\"{title}\"")).await {
                    let mut ct = c["title"].as_str().unwrap_or("").to_string();
                    if let Some(v) = c["title_version"].as_str().filter(|v| !v.is_empty() && !ct.to_lowercase().contains(&v.to_lowercase())) {
                        ct = format!("{ct} {v}");
                    }
                    if compare(&want, &describe(&ct, c["artist"]["name"].as_str().unwrap_or(""), c["duration"].as_f64(), None, None), true).matched {
                        t = Some(c);
                        break;
                    }
                }
            }
            if let Some(id) = t.and_then(|t| t["id"].as_i64()) {
                urls.push(format!("https://www.deezer.com/track/{id}"));
            }
        } else if svc == "tidal" && isrc.is_some() && !st.cfg().str("tidal_client_id").is_empty() {
            match tidal_by_isrc(st, isrc.unwrap(), false).await {
                Ok(Some(u)) => urls.push(u),
                Ok(None) => {}
                Err(e) => st.db().log("warning", &format!("streamrip: tidal lookup failed: {e}")),
            }
        }
    }
    urls
}

// ---- running streamrip --------------------------------------------------------------------------

/// [`audio_files`] that says when the folder can't be read: an EIO there is the disk's ("staging failed: read
/// /downloads/encore/request-12/streamrip/…: Input/output error (os error 5)"), never "streamrip produced no
/// files".
pub fn audio_files_checked(dir: &Path, exts: &[&str]) -> Result<Vec<PathBuf>, StepError> {
    fsx::fault_point(Step::Stage, "read", dir)?;
    let mut f = vec![];
    for e in walkdir::WalkDir::new(dir) {
        let e = match e {
            Ok(e) => e,
            Err(err) => {
                let at = err.path().unwrap_or(dir).to_path_buf();
                match err.into_io_error() {
                    Some(io) if io.kind() != std::io::ErrorKind::NotFound => return Err(StepError::new(Step::Stage, "read", &at, None, io)),
                    _ => continue, // gone while walking, or a symlink loop
                }
            }
        };
        if e.file_type().is_file() && e.path().extension().is_some_and(|x| exts.contains(&x.to_string_lossy().to_lowercase().as_str())) {
            f.push(e.into_path());
        }
    }
    f.sort();
    Ok(f)
}

pub fn audio_files(dir: &Path, exts: &[&str]) -> Vec<PathBuf> {
    let mut f: Vec<PathBuf> = walkdir::WalkDir::new(dir).into_iter().flatten().filter(|e| e.file_type().is_file())
        .map(|e| e.into_path()).filter(|p| p.extension().is_some_and(|x| exts.contains(&x.to_string_lossy().to_lowercase().as_str()))).collect();
    f.sort();
    f
}

/// What a streamrip download left: the audio files, and streamrip's output for the request's log.
#[derive(Debug, Default)]
pub struct Ripped {
    pub files: Vec<PathBuf>,
    /// stdout and stderr together, in the order they came ([`cap_log`] at 20 000 characters)
    pub log: String,
}

/// The command line of a run: the program and its arguments, never a shell (the plugins run no shell). An
/// owner's own command (`streamrip_command`) is split into words the way a shell would split it (quotes and
/// backslashes; no variables, globs or pipes) and each word filled in: `{bin}`, `{config}`, `{folder}`,
/// `{quality}`, `{url}` – a value goes into its word whole, spaces and quotes included. A command that does not
/// split (an open quote) runs the default.
fn command(st: &impl Ctx, bin: &str, config: &Path, folder: &Path, url: &str) -> (String, Vec<String>) {
    let quality = st.cfg().int("streamrip_quality").to_string();
    let (config, folder) = (config.to_string_lossy().to_string(), folder.to_string_lossy().to_string());
    let tpl = st.cfg().str("streamrip_command");
    let words = if tpl.trim().is_empty() || tpl.trim() == DEFAULT_CMD { None } else { split_words(&tpl).filter(|w| !w.is_empty()) };
    let words = words.unwrap_or_else(|| split_words(DEFAULT_CMD).unwrap_or_default());
    let fill = |w: &str| w.replace("{bin}", bin).replace("{config}", &config).replace("{folder}", &folder).replace("{quality}", &quality).replace("{url}", url);
    let mut argv: Vec<String> = words.iter().map(|w| fill(w)).collect();
    let program = argv.remove(0);
    (program, argv)
}

/// A command line split into words as `sh` would split it, without expanding anything: whitespace separates,
/// '…' keeps everything, "…" keeps everything but `\"` and `\\`, a backslash outside quotes keeps the next
/// character. None: a quote is left open.
pub fn split_words(line: &str) -> Option<Vec<String>> {
    let (mut out, mut cur, mut in_word) = (vec![], String::new(), false);
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                in_word = true;
                loop {
                    match chars.next()? {
                        '\'' => break,
                        x => cur.push(x),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next()? {
                        '"' => break,
                        '\\' => match chars.next()? {
                            x @ ('"' | '\\' | '$' | '`') => cur.push(x),
                            x => {
                                cur.push('\\');
                                cur.push(x);
                            }
                        },
                        x => cur.push(x),
                    }
                }
            }
            '\\' => {
                in_word = true;
                if let Some(x) = chars.next() {
                    cur.push(x);
                }
            }
            c if c.is_whitespace() => {
                if in_word {
                    out.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            c => {
                in_word = true;
                cur.push(c);
            }
        }
    }
    if in_word {
        out.push(cur);
    }
    Some(out)
}

pub async fn download(st: &impl Ctx, url: &str, folder: &Path) -> anyhow::Result<Vec<PathBuf>> {
    Ok(download_run(st, url, folder).await?.files)
}

/// Download `url` into `folder` with streamrip, without a terminal ([`run_quiet`]). Every file step and the
/// start name their operation and path ("staging failed: mkdir /downloads/encore/request-12/streamrip: …",
/// "config failed: write /config/streamrip/config.toml: …", "download failed: start /config/tools/venv/bin/rip:
/// …", errors of [`encore_core::fsx`]). streamrip's output comes back on every outcome of a run: in
/// [`Ripped::log`], or in the [`RipError`] of a run that produced no files ([`run_log`]).
pub async fn download_run(st: &impl Ctx, url: &str, folder: &Path) -> anyhow::Result<Ripped> {
    fsx::mkdir_all(folder, Step::Stage)?;
    let Some(bin) = binary(st) else { anyhow::bail!("{NOT_INSTALLED_MSG}") };
    // the config is checked (and mended) and the credentials written before every run, not only the first
    let check = check_config(st, false).await?;
    if let Some(p) = check.problem {
        anyhow::bail!("{CONFIG_BROKEN_MSG} – {p}");
    }
    if url.contains("deezer.com") && toml_value(&read_config(&check.path)?, "deezer", "arl").is_empty() {
        anyhow::bail!("{ARL_MSG}");
    }
    let (program, args) = command(st, &bin, &check.path, folder, url);
    let run = run_quiet(&program, &args, st.data_dir(), RUN_SECS, Step::Download).await?;
    let log = cap_log(run.output.trim(), LOG_CAP);
    let files = match audio_files_checked(folder, &AUDIO) {
        Ok(f) => f,
        // the run's output still goes with the error, for the log
        Err(e) => return Err(anyhow::Error::from(e).context(RipError { message: "streamrip ran, but its folder can't be read".into(), log })),
    };
    // streamrip's own words for a disk that failed under it (its files can't be trusted to be whole): the
    // storage's problem, not the source's – the step waits for the storage
    crate::worker::storage::register_roots(st);
    if let Some(se) = storage_error_in(&log) {
        let message = if files.is_empty() || run.status.is_none() { run_failure(&run, RUN_SECS) } else { format!("streamrip hit a storage error: {se}") };
        return Err(anyhow::Error::from(se).context(RipError { message, log }));
    }
    if files.is_empty() || run.status.is_none() {
        return Err(RipError { message: run_failure(&run, RUN_SECS), log }.into());
    }
    Ok(Ripped { files, log })
}

/// A storage error streamrip itself met while writing into one of Encore's folders ("[Errno 5] Input/output
/// error: '/downloads/encore/request-12/streamrip/…'"): the step error it stands for, when the errno is a
/// storage one ([`fsx::is_storage`]) and the path lies in a folder Encore uses (the staging folder). A
/// storage errno elsewhere (streamrip's own config or cache) is streamrip's failure as before.
pub fn storage_error_in(output: &str) -> Option<StepError> {
    static ERRNO: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| Regex::new(r"\[Errno (\d+)\][^'\n]*'([^'\n]+)'").unwrap());
    ERRNO.captures_iter(output).find_map(|c| {
        let n: i32 = c[1].parse().ok()?;
        let path = Path::new(&c[2]);
        if !fsx::is_storage(n) || !path.is_absolute() {
            return None;
        }
        let e = StepError::new(Step::Download, "write", path, None, std::io::Error::from_raw_os_error(n));
        e.is_storage().then_some(e)
    })
}


// ---- Tidal account sign-in: its own file (streamrip_tidal.rs) --------------------------------------

#[path = "streamrip_tidal.rs"]
mod tidal;
pub use tidal::*;

// ---- the picker's catalogue searches: the accounts streamrip signs in with --------------------------

/// The Deezer ARL is set, in Encore's settings or by hand in streamrip's config.toml.
pub fn deezer_arl_set(st: &impl Ctx) -> bool {
    !st.cfg().str("streamrip_deezer_arl").is_empty() || !toml_value(&std::fs::read_to_string(config_path(st)).unwrap_or_default(), "deezer", "arl").is_empty()
}

/// The Qobuz login streamrip has, as (user id or email, token or md5 password, is a token): Encore's settings
/// first, else config.toml under either naming (`user_id`/`auth_token`, or `email_or_userid`/`password_or_token`
/// with `use_auth_token`). None when there is no login.
pub fn qobuz_login(st: &impl Ctx) -> Option<(String, String, bool)> {
    let (u, t) = (st.cfg().str("streamrip_qobuz_user"), st.cfg().str("streamrip_qobuz_token"));
    if !u.is_empty() && !t.is_empty() {
        return Some((u, t, true));
    }
    let text = std::fs::read_to_string(config_path(st)).unwrap_or_default();
    let v = |k: &str| toml_value(&text, "qobuz", k);
    let (user, token) = (v("user_id"), v("auth_token"));
    if !user.is_empty() && !token.is_empty() {
        return Some((user, token, true));
    }
    let (user, secret) = (v("email_or_userid"), v("password_or_token"));
    if user.is_empty() || secret.is_empty() {
        return None;
    }
    Some((user, secret, v("use_auth_token").trim() != "false"))
}

pub const QOBUZ_APP_ID_MSG: &str = "Qobuz's web player has changed and its app id can't be read – run one Qobuz download so streamrip stores the id in config.toml";

/// The app id Qobuz's API wants in `X-App-Id`: the one streamrip stored in config.toml after its first run,
/// else read from the web player's bundle the way streamrip's own client does (kept for a day).
pub async fn qobuz_app_id(st: &impl Ctx) -> anyhow::Result<String> {
    let stored = toml_value(&std::fs::read_to_string(config_path(st)).unwrap_or_default(), "qobuz", "app_id");
    if !stored.is_empty() {
        return Ok(stored);
    }
    if let Some(v) = st.cache_get("qobuz_app_id") {
        return Ok(v.as_str().unwrap_or("").to_string());
    }
    let player = st.urls().qobuz_player.clone();
    let login = st.http().get(format!("{player}/login")).send().await?.text().await?;
    let bundle = Regex::new(r#"<script src="(/resources/\d+\.\d+\.\d+-[a-z]\d{3}/bundle\.js)"></script>"#).unwrap().captures(&login)
        .ok_or_else(|| anyhow::anyhow!("{QOBUZ_APP_ID_MSG}"))?[1].to_string();
    let js = st.http().get(format!("{player}{bundle}")).send().await?.text().await?;
    let id = Regex::new(r#"production:\{api:\{appId:"(\d{9})",appSecret:"\w{32}"#).unwrap().captures(&js).ok_or_else(|| anyhow::anyhow!("{QOBUZ_APP_ID_MSG}"))?[1].to_string();
    st.cache_put("qobuz_app_id", Value::String(id.clone()), 24 * 3600);
    Ok(id)
}

/// The `X-User-Auth-Token` for Qobuz's API: the stored token, or the token Qobuz hands out for an
/// email + md5 password login (kept for twelve hours). None without a login.
pub async fn qobuz_user_token(st: &impl Ctx, app_id: &str) -> anyhow::Result<Option<String>> {
    let Some((user, secret, is_token)) = qobuz_login(st) else { return Ok(None) };
    if is_token {
        return Ok(Some(secret));
    }
    if let Some(v) = st.cache_get("qobuz_user_token") {
        return Ok(v.as_str().map(String::from));
    }
    // the login goes in the query string (streamrip's way): a failed call must not echo it (reqwest's
    // message would carry the whole URL, e-mail and password hash included)
    let r = st.http().get(format!("{}/user/login", st.urls().qobuz_api)).query(&[("email", user.as_str()), ("password", secret.as_str()), ("app_id", app_id)]).header("X-App-Id", app_id).send().await
        .map_err(|e| anyhow::anyhow!("Qobuz login: {}", http_error_without_url(e)))?;
    let code = r.status().as_u16();
    let d: Value = r.json().await.unwrap_or(Value::Null);
    let tok = d["user_auth_token"].as_str().filter(|t| !t.is_empty()).map(String::from);
    match (code, tok) {
        (200, Some(t)) => {
            st.cache_put("qobuz_user_token", Value::String(t.clone()), 12 * 3600);
            Ok(Some(t))
        }
        (401, _) => anyhow::bail!("Qobuz refused the login – check the email and password in streamrip's config.toml"),
        _ => anyhow::bail!("Qobuz login failed (HTTP {code}): {}", d["message"].as_str().unwrap_or("no message")),
    }
}

/// Qobuz's catalogue search (`kind` album or track) with the app id and login streamrip uses: the answer as
/// Qobuz gives it, an error in words when Qobuz turns the call down.
pub async fn qobuz_search(st: &impl Ctx, kind: &str, q: &str) -> anyhow::Result<Value> {
    let app_id = qobuz_app_id(st).await?;
    let token = qobuz_user_token(st, &app_id).await?;
    let mut req = st.http().get(format!("{}/{kind}/search", st.urls().qobuz_api)).query(&[("query", q), ("limit", "25"), ("offset", "0")]).header("X-App-Id", &app_id);
    if let Some(t) = &token {
        req = req.header("X-User-Auth-Token", t);
    }
    let r = req.send().await?;
    let code = r.status().as_u16();
    if code >= 400 {
        let d: Value = r.json().await.unwrap_or(Value::Null);
        let msg = d["message"].as_str().unwrap_or("no message");
        anyhow::bail!(match code {
            401 => "Qobuz rejected the login – check the user id and token in Settings → Plugins → streamrip".to_string(),
            400 => format!("Qobuz rejected the app id ({msg}) – run one Qobuz download so streamrip stores a fresh one"),
            _ => format!("Qobuz API {code}: {msg}"),
        });
    }
    Ok(r.json().await?)
}

/// A request error as words, without the URL reqwest appends to its message, but with its cause
/// ("connection refused", "dns error …").
fn http_error_without_url(e: reqwest::Error) -> String {
    let e = e.without_url();
    let mut s = e.to_string();
    let mut cause = std::error::Error::source(&e);
    while let Some(c) = cause {
        s.push_str(": ");
        s.push_str(&c.to_string());
        cause = std::error::Error::source(c);
    }
    s
}

pub const TIDAL_LAPSED_MSG: &str = "Tidal sign-in has lapsed – sign in again in Settings → Plugins → streamrip";

/// The signed-in Tidal account's token for Encore's own API calls (the picker's Tidal search), as
/// (access_token, country_code). A lapsed token is refreshed through the OAuth client it came from and
/// written back, so streamrip's next run finds it fresh. None when not signed in.
pub async fn tidal_user_token(st: &impl Ctx) -> anyhow::Result<Option<(String, String)>> {
    let cfg = tidal_config(st);
    let get = |k: &str| cfg.get(k).cloned().unwrap_or_default();
    let token = get("access_token");
    if token.len() <= 10 {
        return Ok(None);
    }
    let cc = get("country_code");
    let cc = if cc.is_empty() { "US".to_string() } else { cc };
    let expiry = get("token_expiry").trim().parse::<f64>().unwrap_or(0.0);
    if expiry <= 0.0 || expiry > encore_core::time::now() + 60.0 {
        return Ok(Some((token, cc)));
    }
    let refresh = get("refresh_token");
    if refresh.is_empty() {
        anyhow::bail!("{TIDAL_LAPSED_MSG}");
    }
    let cid = { let c = get("token_client_id"); if c.is_empty() { TIDAL_DEFAULT_CLIENT.0.to_string() } else { c } };
    let secret = if cid == TIDAL_DEFAULT_CLIENT.0 { TIDAL_DEFAULT_CLIENT.1.to_string() }
        else if cid == TIDAL_HIRES_CLIENT.0 { TIDAL_HIRES_CLIENT.1.to_string() }
        else if cid == get("client_id") { get("client_secret") } else { String::new() };
    let form = [("client_id", cid.as_str()), ("refresh_token", refresh.as_str()), ("grant_type", "refresh_token"), ("scope", TIDAL_SCOPE)];
    let (code, d) = tidal_auth_post(st, "token", &form, Some((cid.as_str(), secret.as_str()))).await.map_err(|e| anyhow::anyhow!("{e}"))?;
    let fresh = d["access_token"].as_str().unwrap_or("").to_string();
    if code != 200 || fresh.len() <= 10 {
        anyhow::bail!("{TIDAL_LAPSED_MSG}");
    }
    let mut values = vec![("access_token".to_string(), tstr(&fresh)), ("token_expiry".to_string(), toml_float(encore_core::time::now() + d["expires_in"].as_f64().unwrap_or(3600.0)))];
    if let Some(r) = d["refresh_token"].as_str().filter(|r| !r.is_empty()) {
        values.push(("refresh_token".to_string(), tstr(r)));
    }
    tidal_write(st, &values).await?;
    Ok(Some((fresh, cc)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The owner's own streamrip command runs without a shell: split into words as `sh` would (quotes, a
    /// backslash), each word filled in whole – a path with spaces or quotes stays one argument.
    #[test]
    fn an_owners_command_is_split_into_words_not_run_by_a_shell() {
        assert_eq!(split_words(DEFAULT_CMD).unwrap()[..3], ["{bin}", "--config-path", "{config}"]);
        assert_eq!(split_words(r#"nice -n 5 "{bin}" --folder '{folder}' url {url}"#).unwrap(), ["nice", "-n", "5", "{bin}", "--folder", "{folder}", "url", "{url}"]);
        assert_eq!(split_words(r#"a\ b "c \"d\"" 'e"f' g\$h"#).unwrap(), ["a b", "c \"d\"", "e\"f", "g$h"]);
        assert_eq!(split_words("  ").unwrap(), Vec::<String>::new());
        assert_eq!(split_words(r#"echo "open"#), None, "an open quote");
        // no shell: `;`, `|` and `$(…)` are words like any other
        assert_eq!(split_words("{bin} url {url} ; rm -rf /").unwrap()[3], ";");
        assert_eq!(split_words("{bin} $(id)").unwrap()[1], "$(id)");
    }

    #[test]
    fn streamrips_own_storage_errors_are_the_storage_s() {
        let staging = std::env::temp_dir().join(format!("encore-rip-errno-{}", std::process::id()));
        fsx::roots::add([&staging]);
        let file = staging.join("request-12/streamrip/Duumu - Illuminate EP/01. Light.flac");
        let out = format!("Downloading Illuminate EP\nTraceback (most recent call last):\nOSError: [Errno 5] Input/output error: '{}'\n", file.display());
        let e = storage_error_in(&out).unwrap();
        assert_eq!((e.step, e.op, e.errno), (Step::Download, "write", Some(5)));
        assert_eq!(e.path, file);
        assert!(e.to_string().ends_with("Input/output error (os error 5) – storage problem, not the source"), "{e}");
        assert!(storage_error_in(&format!("[Errno 28] No space left on device: '{}' -> '/tmp/x'", file.display())).is_some());
        // not a storage errno, outside Encore's folders, or no path: streamrip's own failure
        assert!(storage_error_in(&format!("[Errno 2] No such file or directory: '{}'", file.display())).is_none());
        assert!(storage_error_in("[Errno 5] Input/output error: '/root/.cache/streamrip/x'").is_none());
        assert!(storage_error_in("OSError: [Errno 28] No space left on device").is_none());
    }

    const SECTION: &str = r#"
user_id = "12345678"
country_code = "US"
access_token = "eyJhbGciOiJIUzI1NiJ9.default-token-abcdef"
refresh_token = "refresh-default"
token_expiry = 1760000000.5
token_client_id = "4N3n6Q1x95LL5K7p"
hires_access_token = "eyJhbGciOiJIUzI1NiJ9.hires-token-abcdef"
hires_refresh_token = "refresh-hires"
hires_token_expiry = "1760000001.0"
hires_token_client_id = "fX2JxdmntZWK0ixT"
"#;

    #[test]
    fn parses_quoted_and_unquoted_section() {
        let got = parse_tidal_toml(SECTION);
        assert_eq!(got["user_id"], "12345678");
        assert_eq!(got["token_expiry"], "1760000000.5");
        assert_eq!(got["hires_token_expiry"], "1760000001.0");
        assert_eq!(got["hires_access_token"], "eyJhbGciOiJIUzI1NiJ9.hires-token-abcdef");
        assert_eq!(got["hires_token_client_id"], "fX2JxdmntZWK0ixT");
        let bare = parse_tidal_toml("access_token = abc-def-ghi-jkl # comment\ncountry_code = 'NL'\nuser_id = 42");
        assert_eq!(bare["access_token"], "abc-def-ghi-jkl");
        assert_eq!(bare["country_code"], "NL");
        assert_eq!(bare["user_id"], "42");
    }

    #[test]
    fn parses_whole_file_and_only_its_tidal_section() {
        let file = format!("[downloads]\nfolder = \"/x\"\n\n[qobuz]\nuser_id = \"q\"\nauth_token = \"zzz\"\n\n[tidal]\nquality = 3\nhires_client = true{SECTION}\n[deezer]\narl = \"d\"\nuser_id = \"dz\"\n");
        let got = parse_tidal_toml(&file);
        assert_eq!(got["user_id"], "12345678");
        assert_eq!(got["hires_client"], "true");
        assert_eq!(got["refresh_token"], "refresh-default");
        assert!(!got.contains_key("arl"));
        assert!(!got.contains_key("auth_token"));
        // a pasted section that still has its header is fine too
        assert_eq!(parse_tidal_toml(&format!("[tidal]{SECTION}"))["country_code"], "US");
        assert!(parse_tidal_toml("[qobuz]\nuser_id = \"q\"\n").is_empty());
    }

    #[test]
    fn set_toml_round_trips_both_lanes() {
        let mut text = "[downloads]\nfolder = \"/x\"\n\n[tidal]\nquality = 3\naccess_token = \"\"\ntoken_expiry = \"\"\nhires_client = false\n\n[deezer]\narl = \"d\"\n".to_string();
        let (e1, e2) = (toml_float(1760000000.0), toml_float(1760000001.75));
        let lanes: Vec<(&str, &str)> = vec![("user_id", "\"12345678\""), ("country_code", "\"US\""), ("access_token", "\"default-token-abcdef\""), ("refresh_token", "\"r1\""),
            ("token_expiry", &e1), ("token_client_id", "\"4N3n6Q1x95LL5K7p\""), ("hires_access_token", "\"hires-token-abcdef\""),
            ("hires_refresh_token", "\"r2\""), ("hires_token_expiry", &e2), ("hires_token_client_id", "\"fX2JxdmntZWK0ixT\""), ("hires_client", "true")];
        for (k, v) in &lanes {
            text = set_toml(&text, "tidal", k, v);
        }
        let got = parse_tidal_toml(&text);
        assert_eq!(got["access_token"], "default-token-abcdef");
        assert_eq!(got["token_expiry"], "1760000000.0");
        assert_eq!(got["hires_token_expiry"], "1760000001.8");
        assert_eq!(got["token_client_id"], "4N3n6Q1x95LL5K7p");
        assert_eq!(got["hires_token_client_id"], "fX2JxdmntZWK0ixT");
        assert_eq!(got["hires_client"], "true");
        assert_eq!(got["quality"], "3");
        // the other sections are untouched and nothing leaked into them
        assert!(text.contains("[downloads]\nfolder = \"/x\""));
        assert!(text.trim_end().ends_with("[deezer]\narl = \"d\""));
        assert_eq!(text.matches("access_token").count(), 2, "one plain, one hires_ – no duplicate lines");
        assert_eq!(tidal_toml_value("token_expiry", "1760000000"), "1760000000.0");
        assert_eq!(tidal_toml_value("hires_client", "yes"), "false");
        assert_eq!(tidal_toml_value("access_token", "a\"b"), "\"a\\\"b\"");
    }

    #[test]
    fn lanes_follow_the_config() {
        let two = tidal_lanes(&parse_tidal_toml("quality = 3"));
        assert_eq!(two.len(), 2);
        assert_eq!(two[0].0, TIDAL_DEFAULT_CLIENT.0);
        assert_eq!(two[1].2, "hires_");
        assert_eq!(tidal_lanes(&parse_tidal_toml("hires_client = false")).len(), 1);
        let own = tidal_lanes(&parse_tidal_toml("client_id = \"me\"\nclient_secret = \"s\"\nhires_client = true"));
        assert_eq!(own.len(), 1);
        assert_eq!(own[0].0, "me");
    }

    #[test]
    fn html_403_message() {
        let html = "<html><head>\n<title>  Access denied | auth.tidal.com used\n Cloudflare to restrict access</title></head><body>blocked</body></html>";
        let m = tidal_blocked_message(403, html);
        assert!(m.starts_with("Tidal refused this server's address (HTTP 403, an HTML page instead of an answer). Tidal blocks many hosting and VPN addresses. Sign in on your own computer instead and paste the tokens below, or give the container a different route to the internet."));
        assert!(m.ends_with(" – Access denied | auth.tidal.com used Cloudflare to restrict access"), "{m}");
        assert!(!m.contains('!'));
        let plain = tidal_blocked_message(502, "bad gateway");
        assert!(plain.contains("HTTP 502"));
        assert!(!plain.contains(" – "));
        let long = format!("<title>{}</title>", "x".repeat(300));
        assert_eq!(tidal_blocked_message(403, &long).split(" – ").nth(1).unwrap().len(), 120);
    }

    #[test]
    fn partial_status_when_hires_token_missing() {
        let has = |cfg: &std::collections::BTreeMap<String, String>, k: &str| cfg.get(k).map(|v| v.len() > 10).unwrap_or(false);
        let partial = |text: &str| { let c = parse_tidal_toml(text); has(&c, "access_token") && tidal_hires_client(&c) && !has(&c, "hires_access_token") };
        let signed = |text: &str| { let c = parse_tidal_toml(text); has(&c, "access_token") && (!tidal_hires_client(&c) || has(&c, "hires_access_token")) };
        let only_default = "hires_client = true\naccess_token = \"default-token-abcdef\"\nhires_access_token = \"\"";
        assert!(partial(only_default));
        assert!(!signed(only_default));
        assert!(signed(SECTION));
        assert!(!partial(SECTION));
        assert!(signed("hires_client = false\naccess_token = \"default-token-abcdef\""));
        assert!(!partial("access_token = \"\""));
        assert_eq!(TIDAL_PARTIAL_MSG, "Signed in for 16/44.1 FLAC only – sign in again for hi-res (step 2 was not finished).");
    }

    #[test]
    fn pasted_text_needs_an_access_token() {
        assert!(tidal_toml_has_token(SECTION));
        assert!(tidal_toml_has_token(&format!("[tidal]{SECTION}")));
        assert!(!tidal_toml_has_token(""));
        assert!(!tidal_toml_has_token("user_id = \"12345678\"\naccess_token = \"\""));
        assert!(!tidal_toml_has_token("[qobuz]\nuser_id = \"q\"\n"));
        assert!(!tidal_toml_has_token("hires_access_token = \"eyJhbGciOiJIUzI1NiJ9.hires-token-abcdef\""));
        assert!(TIDAL_NO_TOKEN_MSG.starts_with("No access_token found"));
    }

    #[test]
    fn a_new_default_lane_blanks_the_hires_lane() {
        // the write for lane 1 carries blank hires_ fields, so a stale hi-res token never outlives a new sign-in
        let mut text = "[tidal]\nquality = 3\nhires_client = true".to_string();
        for (k, v) in [("access_token", "\"old-default-token-abcdef\""), ("hires_access_token", "\"old-hires-token-abcdef\""), ("hires_token_client_id", "\"fX2JxdmntZWK0ixT\"")] {
            text = set_toml(&text, "tidal", k, v);
        }
        let mut values: Vec<(String, String)> = vec![("access_token".into(), "\"new-default-token-abcdef\"".into())];
        for k in TIDAL_HIRES_KEYS {
            values.push((k.to_string(), "\"\"".to_string()));
        }
        for (k, v) in &values {
            text = set_toml(&text, "tidal", k, v);
        }
        let got = parse_tidal_toml(&text);
        assert_eq!(got["access_token"], "new-default-token-abcdef");
        assert_eq!(got["hires_access_token"], "");
        assert_eq!(got["hires_token_client_id"], "");
        assert_eq!(TIDAL_HIRES_KEYS.len(), 4);
        assert!(TIDAL_HIRES_KEYS.iter().all(|k| TIDAL_TOKEN_KEYS.contains(k) && k.starts_with("hires_")));
    }

    #[test]
    fn failure_messages_read_plainly() {
        // the owner's case: the complaint continues on the next line, which the old picker lost
        let owner = "Error loading config from /config/streamrip/config.toml: \nQobuzConfig.__init__() got an unexpected keyword argument 'user_id'\nTry running rip config reset\n";
        let m = failure_message(owner);
        assert!(m.starts_with("streamrip can't load its config – press Reset streamrip config in Settings → Plugins → streamrip – "), "{m}");
        assert!(m.contains("unexpected keyword argument 'user_id'"), "{m}");
        assert_eq!(ARL_MSG, "Deezer needs an ARL cookie – Settings → Plugins → streamrip");
        assert_eq!(failure_message("Copy the ARL cookie from your browser and paste it here\nEnter your ARL:"), ARL_MSG);
        assert_eq!(failure_message("Finding-Your-Deezer-ARL-Cookie\nEOFError: EOF when reading a line"), ARL_MSG);
        // an unrelated failure keeps streamrip's own words, boxes and progress lines stripped
        let other = "╭──────╮\n│ Searching │\n╰──────╯\n12 downloading\nNo results found for the query\n";
        assert_eq!(failure_message(other), "streamrip produced no files: Searching / No results found for the query");
        // 300 characters at most
        let long = format!("Error: {}", "x".repeat(400));
        let m = failure_message(&long);
        assert_eq!(m.chars().count(), "streamrip produced no files: ".len() + 300, "{m}");
        assert!(m.ends_with('…'));
        // "early" is not the ARL
        assert!(failure_message("Login failed early: token expired").starts_with("streamrip produced no files: Login failed early"));
        assert!(!asks_for_arl("an early prompt"));
        assert_eq!(toml_value("[deezer]\nquality = 2\narl = \"abc\" # mine\n[tidal]\narl = \"no\"", "deezer", "arl"), "abc");
        assert_eq!(toml_value("[tidal]\nx = 1", "deezer", "arl"), "");
        assert!(qobuz_v2("[qobuz]\nemail_or_userid = \"\"\nauth_token = \"x\""));
        assert!(!qobuz_v2("[qobuz]\nuser_id = \"\"\nauth_token = \"\""));
        assert!(qobuz_v2(""));
    }

    /// The fork's 2.4.9 template ([cli] without the key), 2.4.10's (the key commented out), nathom's 2.x ([misc]).
    const CLI_249: &str = "[database]\ndownloads_enabled = true\n\n[cli]\n# Show resolve, download progress bars\nprogress_bars = true\nmax_search_results = 100\n";
    const CLI_2410: &str = "[cli]\nprogress_bars = true\nmax_search_results = 100\n# streamrip asks GitHub for a newer release before every command. Uncomment this\n# no_update_check = true\n";
    const MISC_20: &str = "[cli]\ntext_output = true\nprogress_bars = true\n\n[misc]\nversion = \"2.0.6\"\ncheck_for_updates = true\n";

    #[test]
    fn the_update_switch_is_read_from_the_streamrip_code_in_its_venv() {
        let root = std::env::temp_dir().join(format!("encore-rip-switch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let bin = root.join("bin/streamrip");
        std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
        std::fs::write(&bin, "#!/bin/sh\n").unwrap();
        assert_eq!(reads_update_switch(&bin), None, "no code beside it");
        let code = root.join("lib/python3.14/site-packages/streamrip");
        std::fs::create_dir_all(&code).unwrap();
        std::fs::write(code.join("config.py"), "class CliConfig:\n    progress_bars: bool\n").unwrap();
        assert_eq!(reads_update_switch(&bin), Some(false));
        std::fs::write(code.join("config.py"), "class CliConfig:\n    no_update_check: bool = False\n").unwrap();
        assert_eq!(reads_update_switch(&bin), Some(true));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_update_check_is_switched_off_the_way_each_streamrip_reads_it() {
        // 2.4.9 (the image's before 0.29): progress bars off, no update-check key (its [cli] would refuse one)
        for v in [Some("streamrip, version 2.4.9"), Some("2.4.9"), None] {
            let t = quiet_config(CLI_249, v, None);
            assert_eq!(toml_value(&t, "cli", "progress_bars"), "false", "{v:?}");
            assert!(!t.lines().any(|l| l.trim_start().starts_with("no_update_check")), "{v:?}: {t}");
            assert_eq!(toml_value(&t, "cli", "max_search_results"), "100");
        }
        // 2.4.10 and later read [cli] no_update_check; known by the version or by the template's line
        for v in [Some("streamrip, version 2.4.10"), Some("2.5.0"), None] {
            let t = quiet_config(CLI_2410, v, None);
            assert_eq!(toml_value(&t, "cli", "no_update_check"), "true", "{v:?}: {t}");
            assert_eq!(toml_value(&t, "cli", "progress_bars"), "false");
        }
        assert_eq!(toml_value(&quiet_config(CLI_249, Some("2.4.10"), None), "cli", "no_update_check"), "true", "a config from 2.4.9 run by 2.4.10");
        assert_eq!(toml_value(&quiet_config(CLI_2410, Some("2.4.9"), None), "cli", "no_update_check"), "", "a downgraded streamrip never gets the key");
        // nathom's 2.x: [misc] check_for_updates
        let t = quiet_config(MISC_20, Some("2.0.6"), None);
        assert_eq!(toml_value(&t, "misc", "check_for_updates"), "false");
        assert_eq!(toml_value(&t, "misc", "version"), "2.0.6");
        assert_eq!(toml_value(&t, "cli", "no_update_check"), "");
        // a file without those sections gets none of Encore's (a section of only its keys would not load)
        let bare = "[deezer]\narl = \"x\"\n";
        assert_eq!(quiet_config(bare, Some("2.4.10"), None), bare);
        // the streamrip's own code wins over its version (the fork's v2.4.10 tag says 2.4.9)
        assert_eq!(toml_value(&quiet_config(CLI_2410, Some("streamrip, version 2.4.9"), Some(true)), "cli", "no_update_check"), "true");
        assert_eq!(toml_value(&quiet_config(CLI_249, Some("2.4.10"), Some(false)), "cli", "no_update_check"), "");
        // written twice, the same file
        let once = quiet_config(CLI_2410, None, None);
        assert_eq!(quiet_config(&once, None, None), once);
    }

    #[test]
    fn a_long_run_log_keeps_its_end() {
        let text: String = (0..3000).map(|i| format!("line {i}\n")).collect::<String>() + "Traceback: the real error";
        let log = cap_log(&text, 2000);
        assert!(log.starts_with("line 0\n"), "{log}");
        assert!(log.ends_with("Traceback: the real error"), "{log}");
        assert!(log.contains("characters left out"), "{log}");
        assert!(log.chars().count() < 2100);
        assert_eq!(cap_log("short", 2000), "short");
    }

    #[test]
    fn a_run_that_printed_nothing_says_how_it_ended() {
        use std::os::unix::process::ExitStatusExt;
        let ended = |code: i32, out: &str| RunOutput { status: Some(std::process::ExitStatus::from_raw(code << 8)), output: out.into() };
        assert_eq!(run_failure(&ended(1, ""), RUN_SECS), "streamrip produced no files and printed nothing (exit code 1)");
        assert_eq!(run_failure(&ended(1, "Error: album not available in your country\n"), RUN_SECS), "streamrip produced no files: Error: album not available in your country");
        let stopped = RunOutput { status: None, output: "Downloading…\n".into() };
        assert_eq!(run_failure(&stopped, 3600), "streamrip did not finish within 60 minutes and was stopped – streamrip produced no files: Downloading…");
    }

    #[test]
    fn the_config_is_written_whole_with_its_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let d = std::env::temp_dir().join(format!("encore-rip-write-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("config.toml");
        assert_eq!(read_config(&p).unwrap(), "", "no file yet");
        write_config(&p, "[deezer]\narl = \"a\"\n").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
        write_config(&p, "[deezer]\narl = \"b\"\n").unwrap();
        assert_eq!(read_config(&p).unwrap(), "[deezer]\narl = \"b\"\n");
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600, "the sign-ins stay private");
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 1, "no new file left beside it");
        // a symlinked config stays a symlink
        let real = d.join("real.toml");
        std::fs::rename(&p, &real).unwrap();
        std::os::unix::fs::symlink(&real, &p).unwrap();
        write_config(&p, "x = 1\n").unwrap();
        assert!(std::fs::symlink_metadata(&p).unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "x = 1\n");
        // a config folder that is a file: the error names the write of the config
        let blocked = d.join("real.toml").join("config.toml");
        let e = write_config(&blocked, "x").unwrap_err();
        assert_eq!((e.step, e.op), (Step::Config, "write"));
        assert!(e.to_string().starts_with(&format!("config failed: write {}: ", blocked.display())), "{e}");
        // so does an unreadable one, for the read
        let e = read_config(&blocked).unwrap_err();
        assert!(e.to_string().starts_with(&format!("config failed: read {}: Not a directory (os error 20)", blocked.display())), "{e}");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A script in a fresh folder, for the runs below.
    fn script(name: &str, body: &str) -> (PathBuf, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let d = std::env::temp_dir().join(format!("encore-rip-run-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("run.sh");
        let tmp = d.join(".run.tmp");
        std::fs::write(&tmp, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::rename(&tmp, &p).unwrap();
        (d, p)
    }

    /// Field `n` (1-based, after the command's parentheses) of /proc/<pid>/stat.
    fn stat_field(stat: &str, n: usize) -> i64 {
        stat.rsplit_once(')').unwrap().1.split_whitespace().nth(n - 3).unwrap().parse().unwrap()
    }

    #[tokio::test]
    async fn a_run_has_no_terminal_and_hands_back_all_its_output() {
        let (d, sh) = script("quiet", r#"here=$(dirname "$0")
cat /proc/$$/stat > "$here/stat"
echo "env TERM=$TERM NO_COLOR=$NO_COLOR COLUMNS=$COLUMNS PYTHONUNBUFFERED=$PYTHONUNBUFFERED HOME=$HOME" > "$here/env"
echo "to the terminal" > /dev/tty
read answer < /dev/tty
echo "out 1"
echo "err 1" >&2
echo "out 2"
printf 'no newline at the end' >&2
exit 3"#);
        let started = std::time::Instant::now();
        let run = run_quiet(&sh.to_string_lossy(), &[], &d, 30, Step::Download).await.unwrap();
        assert!(started.elapsed() < std::time::Duration::from_secs(20), "a read from /dev/tty must not wait");
        assert_eq!(run.status.and_then(|s| s.code()), Some(3));
        assert!(!run.success());
        for line in ["out 1", "err 1", "out 2", "no newline at the end"] {
            assert!(run.output.contains(line), "{line} in {}", run.output);
        }
        assert!(run.output.find("out 1").unwrap() < run.output.find("out 2").unwrap());
        // its own session (setsid), so no controlling terminal: tty_nr 0, a session id that is not ours
        let stat = std::fs::read_to_string(d.join("stat")).unwrap();
        let mine = std::fs::read_to_string("/proc/self/stat").unwrap();
        assert_eq!(stat_field(&stat, 7), 0, "tty_nr: {stat}");
        assert_ne!(stat_field(&stat, 6), stat_field(&mine, 6), "session: {stat}");
        assert_eq!(stat_field(&stat, 6), stat_field(&stat, 5), "the run leads its session's process group");
        let env = std::fs::read_to_string(d.join("env")).unwrap();
        assert_eq!(env.trim(), format!("env TERM=dumb NO_COLOR=1 COLUMNS=200 PYTHONUNBUFFERED=1 HOME={}", d.display()));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[tokio::test]
    async fn a_run_past_its_limit_is_stopped_with_everything_it_started() {
        let (d, sh) = script("limit", r#"here=$(dirname "$0")
sleep 300 &
echo $! > "$here/child"
echo "started"
sleep 300"#);
        let started = std::time::Instant::now();
        let run = run_quiet(&sh.to_string_lossy(), &[], &d, 2, Step::Download).await.unwrap();
        assert!(started.elapsed() < std::time::Duration::from_secs(15));
        assert!(run.status.is_none());
        assert!(run.output.contains("started") && run.output.contains("Encore stopped streamrip after 2 s"), "{}", run.output);
        // the program's own child went with it
        let child: i32 = std::fs::read_to_string(d.join("child")).unwrap().trim().parse().unwrap();
        let mut gone = false;
        for _ in 0..50 {
            let stat = std::fs::read_to_string(format!("/proc/{child}/stat")).unwrap_or_default();
            if stat.is_empty() || stat.rsplit_once(')').is_some_and(|(_, r)| r.trim_start().starts_with('Z')) {
                gone = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        assert!(gone, "sleep {child} still runs");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn an_unreadable_folder_after_a_run_is_the_disks_and_keeps_the_output() {
        let d = std::env::temp_dir().join(format!("encore-rip-walk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("Album")).unwrap();
        std::fs::write(d.join("Album/01.flac"), b"x").unwrap();
        assert_eq!(audio_files_checked(&d, &AUDIO).unwrap(), vec![d.join("Album/01.flac")]);
        let e = audio_files_checked(&d.join("missing"), &AUDIO).unwrap();
        assert!(e.is_empty(), "a folder that is not there has no files");
        // a folder whose reading fails
        let file = d.join("Album/01.flac");
        let e = audio_files_checked(&file.join("x"), &AUDIO).unwrap_err();
        assert_eq!((e.step, e.op, e.errno), (Step::Stage, "read", Some(20)));
        let wrapped = anyhow::Error::from(e).context(RipError { message: "streamrip ran, but its folder can't be read".into(), log: "the output".into() });
        assert_eq!(run_log(&wrapped), Some("the output"));
        assert!(fsx::find(&wrapped).is_some());
        assert!(format!("{wrapped:#}").starts_with(&format!("streamrip ran, but its folder can't be read: staging failed: read {}", file.join("x").display())), "{wrapped:#}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[tokio::test]
    async fn a_program_that_cannot_start_is_named() {
        let d = std::env::temp_dir().join(format!("encore-rip-nostart-{}", std::process::id()));
        let missing = d.join("venv/bin/rip");
        let e = run_quiet(&missing.to_string_lossy(), &[], &d, 5, Step::Download).await.unwrap_err();
        assert_eq!(e.to_string(), format!("download failed: start {}: No such file or directory (os error 2) – the program is missing", missing.display()));
        // something that can't be run (a folder): the error of the start, with its path and errno
        let (d2, _) = script("noexec", "echo hi");
        let e = run_quiet(&d2.to_string_lossy(), &[], &d2, 5, Step::Config).await.unwrap_err();
        assert!(e.to_string().starts_with(&format!("config failed: start {}: Permission denied (os error 13)", d2.display())), "{e}");
        assert_eq!(e.errno, Some(13));
        let _ = std::fs::remove_dir_all(&d2);
    }
}
