//! Soulseek, run by Encore: slskd (github.com/slskd/slskd) as a headless child process. Encore
//! downloads it from slskd's GitHub releases (Download tools → Install/Update), writes its slskd.yml
//! from Encore's settings, starts it on 127.0.0.1 with an API key only Encore knows, restarts it when
//! it exits or the settings change, and keeps it inside the VPN when the VPN is on (a local SOCKS5
//! proxy for every outbound connection, the forwarded port relayed in). `sources::slskd` talks to it.
//!
//! Layout under the config folder: tools/slskd/<version>/slskd (+ `current` naming the version),
//! slskd/ (its --app-dir: slskd.yml, database, encore-run.log).
use crate::state::AppState;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
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
/// The empty folder slskd.yml names as the web UI's content (beside the binary; wwwroot is deleted).
const NO_WEB_UI: &str = "no-web-ui";
const RELEASES: &str = "https://api.github.com/repos/slskd/slskd/releases/latest";

pub const WAITING_FOR_VPN: &str = "Waiting for the VPN – Soulseek stays off until it is connected";
pub const NOT_INSTALLED: &str = "Soulseek is not installed – install it under Settings → Download tools (slskd – Soulseek engine)";

// ---- status ---------------------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
struct Status {
    /// off | not_installed | starting | running | crashed | waiting_vpn
    state: &'static str,
    version: Option<String>,
    pid: Option<u32>,
    error: Option<String>,
    listen_port: Option<u16>,
    vpn: bool,
}

static STATUS: LazyLock<Mutex<Status>> = LazyLock::new(|| Mutex::new(Status { state: "off", ..Default::default() }));
static WAKE: LazyLock<tokio::sync::Notify> = LazyLock::new(tokio::sync::Notify::new);
static INSTALLING: AtomicBool = AtomicBool::new(false);

/// {state: off|not_installed|starting|running|crashed|waiting_vpn, version, pid, error, listen_port, vpn}
pub fn status() -> Value {
    let s = STATUS.lock().unwrap().clone();
    json!({ "state": s.state, "version": s.version, "pid": s.pid, "error": s.error, "listen_port": s.listen_port, "vpn": s.vpn })
}

fn set_status(f: impl FnOnce(&mut Status)) {
    f(&mut STATUS.lock().unwrap());
}

/// Look at the settings now instead of at the next check (after saving settings, installing, removing).
pub fn poke() {
    WAKE.notify_one();
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

pub fn installed_version(st: &AppState) -> Option<String> {
    installed_in(st.data_dir()).map(|(v, _)| v)
}

/// Soulseek is switched on with a login (whether or not slskd is installed yet).
pub fn configured(st: &AppState) -> bool {
    st.cfg.bool("slskd_enabled") && !st.cfg.str("soulseek_username").trim().is_empty() && !st.cfg.str("soulseek_password").is_empty()
}

pub fn base_url() -> String {
    format!("http://127.0.0.1:{WEB_PORT}")
}

/// The key slskd.yml gives Encore (48 random characters, generated once and kept in the settings).
pub fn api_key(st: &AppState) -> String {
    let k = st.cfg.str(API_KEY_SETTING);
    if k.len() >= 16 {
        return k;
    }
    let k = random_alnum(48);
    st.cfg.set_many(&json!({ API_KEY_SETTING: k }));
    st.cfg.str(API_KEY_SETTING)
}

fn random_alnum(n: usize) -> String {
    use rand::Rng;
    const ABC: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut r = rand::rng();
    (0..n).map(|_| ABC[r.random_range(0..ABC.len())] as char).collect()
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
pub async fn latest(st: &AppState) -> anyhow::Result<(String, String)> {
    let r = st.http.get(RELEASES).header("Accept", "application/vnd.github+json").timeout(Duration::from_secs(20)).send().await
        .map_err(|e| anyhow::anyhow!("GitHub unreachable: {e}"))?;
    let code = r.status().as_u16();
    let v: Value = r.json().await.unwrap_or(Value::Null);
    if code >= 400 {
        anyhow::bail!("GitHub returned {code} for the latest slskd release: {}", v["message"].as_str().unwrap_or(""));
    }
    pick_asset(&v, std::env::consts::ARCH)
        .ok_or_else(|| anyhow::anyhow!("The latest slskd release has no Linux build for {}", std::env::consts::ARCH))
}

/// Clears INSTALLING when an install ends, however it ends.
struct InstallGuard;
impl Drop for InstallGuard {
    fn drop(&mut self) {
        INSTALLING.store(false, Ordering::SeqCst);
    }
}

/// Download `url`, unpack it as `ver`, check that it runs, switch to it, restart slskd on it and
/// delete older versions.
pub async fn install(st: &AppState, ver: &str, url: &str) -> anyhow::Result<()> {
    let ver = ver.trim().trim_start_matches('v').to_string();
    if ver.is_empty() || !ver.chars().all(|c| c.is_ascii_alphanumeric() || ".-+_".contains(c)) {
        anyhow::bail!("not a version: {ver:?}");
    }
    if INSTALLING.swap(true, Ordering::SeqCst) {
        anyhow::bail!("slskd is already being installed");
    }
    let _guard = InstallGuard;
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
    st.db.log("info", &format!("Soulseek engine slskd {ver} installed"));
    poke(); // the supervisor (re)starts it on the new binary
    Ok(())
}

async fn download(st: &AppState, url: &str, to: &Path) -> anyhow::Result<()> {
    use futures_util::StreamExt;
    let r = st.http.get(url).timeout(Duration::from_secs(900)).send().await.map_err(|e| anyhow::anyhow!("download failed: {e}"))?;
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
    Ok(crate::tools::extract_version(&text).unwrap_or_else(|| text.trim().to_string()))
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
pub fn reset(st: &AppState) {
    if let Some(pid) = STATUS.lock().unwrap().pid {
        signal(pid, "KILL");
    }
    let _ = std::fs::remove_dir_all(tools_dir(st.data_dir()));
    set_status(|s| {
        s.state = "not_installed";
        s.version = None;
        s.pid = None;
        s.error = None;
    });
    st.db.log("info", "Soulseek engine slskd removed");
    poke();
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
    o += "flags:\n  no_logo: true\n  no_version_check: true\n";
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
    if let Some((port, user, pass)) = &y.proxy {
        o += &format!("  connection:\n    proxy:\n      enabled: true\n      address: \"127.0.0.1\"\n      port: {port}\n      username: {}\n      password: {}\n", q(user), q(pass));
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

fn yml_from_settings(st: &AppState) -> Yml {
    let downloads = st.cfg.str("slskd_download_path");
    let library = st.cfg.str("library_path");
    let share = st.cfg.bool("soulseek_share") && !library.trim().is_empty() && library.trim() != "/";
    Yml {
        username: st.cfg.str("soulseek_username").trim().to_string(),
        password: st.cfg.str("soulseek_password"),
        server: server(),
        listen_ip: "0.0.0.0".into(),
        listen_port: configured_port(st),
        incomplete: Path::new(&downloads).join(".incomplete").to_string_lossy().to_string(),
        downloads,
        shares: if share { vec![library.trim().to_string()] } else { vec![] },
        upload_slots: st.cfg.int("soulseek_upload_slots").clamp(1, 20),
        content_path: NO_WEB_UI.into(),
        api_key: api_key(st),
        proxy: None,
    }
}

fn configured_port(st: &AppState) -> u16 {
    st.cfg.int("soulseek_port").clamp(1, 65535) as u16
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

fn route(st: &AppState) -> Route {
    let on = st.cfg.bool("vpn_enabled");
    if !on {
        return Route::Direct;
    }
    let usable = crate::vpn::enabled(st);
    route_for(on, usable, usable && crate::vpn::active(), st.cfg.bool("vpn_kill_switch"), crate::vpn::forwarded_port())
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

fn pipe<R: AsyncRead + Unpin + Send + 'static>(r: R, log: Arc<RunLog>) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(r).lines();
        while let Ok(Some(l)) = lines.next_line().await {
            log.line(&l);
        }
    });
}

fn signal(pid: u32, sig: &str) {
    let _ = std::process::Command::new("kill").arg(format!("-{sig}")).arg(pid.to_string()).stdout(Stdio::null()).stderr(Stdio::null()).status();
}

/// Starts, watches, restarts and stops one slskd process.
pub struct Runner {
    child: Option<Child>,
    key: u64,
    started: Instant,
    backoff: Duration,
    next_start: Option<Instant>,
    log: Option<Arc<RunLog>>,
}

const BACKOFF_MIN: Duration = Duration::from_secs(5);
const BACKOFF_MAX: Duration = Duration::from_secs(300);

impl Default for Runner {
    fn default() -> Self {
        Runner { child: None, key: 0, started: Instant::now(), backoff: BACKOFF_MIN, next_start: None, log: None }
    }
}

impl Runner {
    /// Stop gently (SIGTERM, then SIGKILL after 10 s).
    async fn stop(&mut self) {
        if let Some(mut c) = self.child.take() {
            if let Some(pid) = c.id() {
                signal(pid, "TERM");
            }
            if tokio::time::timeout(Duration::from_secs(10), c.wait()).await.is_err() {
                let _ = c.kill().await;
            }
        }
        self.key = 0;
    }

    /// Bring the process in line with `want` and update the status.
    pub async fn apply(&mut self, want: Want) {
        let plan = match want {
            Want::Off | Want::NotInstalled | Want::WaitingVpn | Want::Blocked(_) => {
                self.stop().await;
                self.next_start = None;
                self.backoff = BACKOFF_MIN;
                set_status(|s| {
                    *s = Status { state: "off", ..Default::default() };
                    match want {
                        Want::NotInstalled => { s.state = "not_installed"; s.error = Some(NOT_INSTALLED.into()); }
                        Want::WaitingVpn => { s.state = "waiting_vpn"; s.error = Some(WAITING_FOR_VPN.into()); s.vpn = true; }
                        Want::Blocked(e) => { s.state = "starting"; s.error = Some(e); }
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
                    set_status(|s| {
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
                    set_status(|s| {
                        s.state = "crashed";
                        s.pid = None;
                        s.error = Some(msg);
                    });
                    return;
                }
            }
        }
        if self.next_start.is_some_and(|t| Instant::now() < t) {
            return; // crashed: wait out the backoff
        }
        if let Err(e) = self.start(&plan) {
            let msg = format!("slskd didn't start: {e}");
            tracing::warn!("soulseek: {msg}");
            self.next_start = Some(Instant::now() + self.backoff);
            self.backoff = (self.backoff * 2).min(BACKOFF_MAX);
            set_status(|s| {
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
        set_status(|s| {
            *s = Status { state: "starting", version: Some(plan.version.clone()), pid, error: None, listen_port: Some(plan.listen_port), vpn: plan.vpn };
        });
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
            pipe(o, log.clone());
        }
        if let Some(e) = child.stderr.take() {
            pipe(e, log.clone());
        }
        if let Some(pid) = child.id() {
            let _ = std::fs::write(plan.app_dir.join("encore.pid"), pid.to_string());
        }
        tracing::info!("soulseek: started slskd {} (pid {:?}, port {}{})", plan.version, child.id(), plan.listen_port, if plan.vpn { ", through the VPN" } else { "" });
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
async fn want(st: &AppState) -> Want {
    if !configured(st) {
        crate::vpn::relay::ensure(None);
        return Want::Off;
    }
    let Some((version, bin)) = installed_in(st.data_dir()) else {
        crate::vpn::relay::ensure(None);
        return Want::NotInstalled;
    };
    let mut y = yml_from_settings(st);
    let r = route(st);
    match r {
        Route::Wait => {
            crate::vpn::relay::ensure(None);
            return Want::WaitingVpn;
        }
        Route::Direct => crate::vpn::relay::ensure(None),
        Route::Vpn(fwd) => {
            let proxy = match crate::vpn::socks::start().await {
                Ok(p) => p,
                Err(e) => return Want::Blocked(format!("Couldn't start the VPN proxy for Soulseek: {e}")),
            };
            // slskd would look the server's name up itself, outside the VPN: hand it the address
            // (a failed lookup later on keeps the last answer: a DNS hiccup must not stop a running slskd)
            static LAST: Mutex<Option<(String, std::net::IpAddr)>> = Mutex::new(None);
            let (host, port) = y.server.clone();
            let last = LAST.lock().unwrap().clone().filter(|(h, _)| *h == host).map(|(_, ip)| ip);
            let ip = match (crate::vpn::resolve(&host).await.map(|ips| ips.into_iter().find(|i| i.is_ipv4())), last) {
                (Ok(Some(ip)), _) => {
                    *LAST.lock().unwrap() = Some((host.clone(), ip));
                    ip
                }
                (_, Some(ip)) => ip,
                (Ok(None), None) => return Want::Blocked(format!("{host} has no IPv4 address (looked up through the VPN)")),
                (Err(e), None) => return Want::Blocked(format!("Couldn't look up the Soulseek server {host} through the VPN: {e}")),
            };
            y.server = (ip.to_string(), port);
            y.proxy = Some(proxy);
            // peers reach slskd only through the tunnel: the forwarded port is relayed to loopback
            y.listen_ip = "127.0.0.1".into();
            if let Some(p) = fwd {
                y.listen_port = p;
            }
            crate::vpn::relay::ensure(fwd);
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
    })
}

/// slskd's API answers (any answer: it is up).
async fn answers(st: &AppState) -> bool {
    st.http.get(format!("{}/api/v0/application", base_url())).header("X-API-Key", api_key(st)).timeout(Duration::from_secs(3)).send().await
        .map(|r| r.status().as_u16() < 500).unwrap_or(false)
}

/// Keeps slskd in step with the settings: installs nothing by itself unless tools_auto_slskd is on.
pub async fn supervisor(st: AppState) {
    kill_stale(&app_dir(st.data_dir()));
    let mut run = Runner::default();
    let mut auto_tried: Option<Instant> = None;
    loop {
        let w = want(&st).await;
        if matches!(w, Want::NotInstalled) && st.cfg.bool("tools_auto_slskd") && !INSTALLING.load(Ordering::SeqCst)
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
        if STATUS.lock().unwrap().state == "starting" && run.child.is_some() && answers(&st).await {
            set_status(|s| {
                s.state = "running";
                s.error = None;
            });
        }
        let wait = if st.cfg.bool("vpn_enabled") || STATUS.lock().unwrap().state == "starting" { 3 } else { 10 };
        tokio::select! {
            _ = WAKE.notified() => {}
            _ = tokio::time::sleep(Duration::from_secs(wait)) => {}
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// The status is one per process: tests that drive a Runner take turns.
    static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("encore-slskd-{name}-{}-{}", std::process::id(), random_alnum(6)));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

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
        let y = render_yml(&yml(false, false));
        assert!(y.contains("shares:\n  directories: []"), "{y}");
        let mut v = yml(true, true);
        v.listen_ip = "127.0.0.1".into();
        v.listen_port = 51234;
        v.server = ("208.76.170.59".into(), 2271);
        let y = render_yml(&v);
        assert!(y.contains("  connection:\n    proxy:\n      enabled: true\n      address: \"127.0.0.1\"\n      port: 40000\n      username: \"u\"\n      password: \"p\""), "{y}");
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
        Plan { bin, app_dir: dir.join("app"), version: "0.26.1".into(), yml: yml.into(), dirs: vec![dir.join("dl")], listen_port: 2234, vpn: false }
    }

    fn state() -> (String, Option<u32>, Option<String>) {
        let s = status();
        (s["state"].as_str().unwrap().to_string(), s["pid"].as_u64().map(|p| p as u32), s["error"].as_str().map(String::from))
    }

    /// A fake slskd that keeps running: started once, left alone while nothing changes, restarted
    /// for new settings, stopped when Soulseek is switched off; and the other states.
    #[tokio::test]
    async fn supervisor_states_with_a_running_fake() {
        let _l = LOCK.lock().await;
        let dir = tmp("run");
        let bin = script(&dir, "echo started \"$@\"; exec sleep 30");
        let mut r = Runner::default();
        r.apply(plan(&dir, bin.clone(), "a: 1\n").into()).await;
        let (s, pid, _) = state();
        assert_eq!(s, "starting");
        assert!(pid.is_some());
        assert_eq!(std::fs::read_to_string(dir.join("app").join("slskd.yml")).unwrap(), "a: 1\n");
        assert!(dir.join("dl").is_dir(), "folders slskd needs are made first");
        assert_eq!(std::fs::metadata(dir.join("app").join("slskd.yml")).unwrap().permissions().mode() & 0o777, 0o600);
        r.apply(plan(&dir, bin.clone(), "a: 1\n").into()).await;
        assert_eq!(state().1, pid, "nothing changed: same process");
        r.apply(plan(&dir, bin.clone(), "a: 2\n").into()).await;
        let (s, pid2, _) = state();
        assert_eq!(s, "starting");
        assert!(pid2.is_some() && pid2 != pid, "new settings: restarted");
        assert_eq!(std::fs::read_to_string(dir.join("app").join("slskd.yml")).unwrap(), "a: 2\n");
        tokio::time::sleep(Duration::from_millis(300)).await;
        let log = std::fs::read_to_string(dir.join("app").join("encore-run.log")).unwrap();
        assert!(log.contains(&format!("started --app-dir {}", dir.join("app").display())), "{log}");
        r.apply(Want::Off).await;
        assert_eq!(state(), ("off".into(), None, None));
        assert!(r.child.is_none());
        r.apply(Want::NotInstalled).await;
        assert_eq!(state().0, "not_installed");
        r.apply(Want::WaitingVpn).await;
        assert_eq!(state(), ("waiting_vpn".into(), None, Some(WAITING_FOR_VPN.into())));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A fake slskd that exits at once: crashed with its last words, restarted only after the backoff.
    #[tokio::test]
    async fn supervisor_restarts_a_crashing_fake_with_backoff() {
        let _l = LOCK.lock().await;
        let dir = tmp("crash");
        let bin = script(&dir, "echo '[ERR] Invalid configuration: soulseek.username'; exit 3");
        let mut r = Runner::default();
        r.apply(plan(&dir, bin.clone(), "x\n").into()).await;
        assert_eq!(state().0, "starting");
        tokio::time::sleep(Duration::from_millis(400)).await;
        r.apply(plan(&dir, bin.clone(), "x\n").into()).await;
        let (s, pid, err) = state();
        assert_eq!(s, "crashed");
        assert_eq!(pid, None);
        let err = err.unwrap();
        assert!(err.contains("exit status: 3") && err.contains("Invalid configuration") && err.contains("again in 5 s"), "{err}");
        r.apply(plan(&dir, bin.clone(), "x\n").into()).await;
        assert_eq!(state().0, "crashed", "waits out the backoff");
        assert!(r.child.is_none());
        r.next_start = Some(Instant::now()); // the backoff is over
        r.apply(plan(&dir, bin.clone(), "x\n").into()).await;
        assert_eq!(state().0, "starting", "started again");
        tokio::time::sleep(Duration::from_millis(400)).await;
        r.apply(plan(&dir, bin, "x\n").into()).await;
        assert!(state().2.unwrap().contains("again in 10 s"), "the backoff doubles");
        r.apply(Want::Off).await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The real thing: SLSKD_ZIP=<a downloaded slskd-…-linux-x64.zip> cargo test --release -- --ignored real_slskd
    /// unpacks it the way Install does, starts it on a generated slskd.yml and checks that the API
    /// answers Encore's key (and nothing else), headless.
    #[tokio::test]
    #[ignore]
    async fn real_slskd() {
        let _l = LOCK.lock().await;
        let zip = PathBuf::from(std::env::var("SLSKD_ZIP").expect("SLSKD_ZIP"));
        let dir = tmp("real");
        let root = dir.join("tools");
        std::fs::create_dir_all(&root).unwrap();
        let bin = unpack(&root, "0.0.0", &zip).unwrap();
        println!("--version: {}", verify(&bin).await.unwrap());
        switch_to(&root, "0.0.0").unwrap();
        let bin = root.join("0.0.0").join("slskd");
        let mut y = yml(false, false);
        let _vpn = crate::vpn::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        y.downloads = dir.join("dl").to_string_lossy().to_string();
        y.incomplete = dir.join("dl").join(".incomplete").to_string_lossy().to_string();
        y.shares = vec![dir.join("share").to_string_lossy().to_string()];
        std::fs::create_dir_all(dir.join("share")).unwrap();
        y.username = format!("encoretest{}", random_alnum(6));
        let p = Plan { bin, app_dir: dir.join("app"), version: "0.0.0".into(), yml: render_yml(&y), dirs: vec![dir.join("dl"), dir.join("dl").join(".incomplete")], listen_port: 2234, vpn: false };
        let mut r = Runner::default();
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
            assert_ne!(state().0, "crashed", "{:?}\n{}", state(), std::fs::read_to_string(dir.join("app").join("encore-run.log")).unwrap_or_default());
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
        crate::vpn::install(None);
        y.proxy = Some(crate::vpn::socks::start().await.unwrap());
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

    impl From<Plan> for Want {
        fn from(p: Plan) -> Want {
            Want::Run(p)
        }
    }
}
