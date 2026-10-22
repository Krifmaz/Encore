//! Updates: the newest release of Encore on GitHub (`Krifmaz/Encore`, latest), compared with
//! this build by semver; the file for this computer (`Encore-<v>-windows-x86_64.exe`,
//! `Encore-<v>-macos-universal.dmg`, `Encore-<v>-linux-x86_64.AppImage`); checked against the
//! release's SHA256SUMS; then installed and Encore started again:
//!
//! - Windows: the installer runs silently (`/S`, per user) once Encore has quit, then Encore starts.
//! - macOS: the app in the disk image replaces this one once Encore has quit, then it opens.
//! - Linux AppImage: the new AppImage replaces this one, then it starts.
//! - Anything else (the .deb, a build from source, a folder Encore cannot write): the new file
//!   (the .deb on Linux) opens in the browser to download and install by hand, or the release
//!   page when the release has no file for this computer.
//!
//! A file whose SHA-256 is not the one SHA256SUMS lists is deleted and never run; when the
//! release has a SHA256SUMS that cannot be read, nothing is installed either. Only files under
//! the project's release downloads are fetched.
//!
//! At start (after 30 s) and every 6 hours, while `check_updates` is on, the app looks for a new
//! version in the background; the app's tick announces a new one once (toast and tray line).

use super::{send, Message};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The GitHub API answer for the newest releases (a hotfix can be "Latest" and carries no apps: [full_release]).
pub const LATEST: &str = "https://api.github.com/repos/Krifmaz/Encore/releases?per_page=30";
/// The releases page.
pub const RELEASES: &str = "https://github.com/Krifmaz/Encore/releases";
/// Every file the updater fetches is under this.
pub const DOWNLOADS: &str = "https://github.com/Krifmaz/Encore/releases/download/";
/// The checksum file of a release.
const SUMS: [&str; 2] = ["SHA256SUMS", "SHA256SUMS.txt"];
/// How often the background check runs.
const EVERY: Duration = Duration::from_secs(6 * 3600);

/// A release file.
#[derive(Clone, Debug, PartialEq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
}

/// A newer release.
#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub version: String,
    /// The release page.
    pub page: String,
    pub notes: String,
    /// The file for this computer, when the release has one.
    pub asset: Option<Asset>,
    /// The release's SHA256SUMS, when it has one.
    pub sums: Option<String>,
}

/// How this copy of Encore can be replaced.
#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    /// Windows: run the installer.
    #[cfg(target_os = "windows")]
    Installer,
    /// Linux: the AppImage at this path.
    AppImage(PathBuf),
    /// macOS: the app bundle at this path.
    #[cfg(target_os = "macos")]
    Bundle(PathBuf),
    /// The browser does it (a .deb, a build from source).
    Manual,
}

/// Where the updater is.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Phase {
    #[default]
    Idle,
    Checking,
    UpToDate,
    Available(Release),
    Downloading {
        version: String,
        got: u64,
        total: u64,
    },
    Verifying(String),
    Installing(String),
    Failed(String),
}

/// What an install did.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// The new version is being put in place; Encore quits and starts again.
    Restarting,
    /// Something to do by hand, in these words (the browser or the disk image is open).
    Opened(String),
}

static PHASE: Mutex<Phase> = Mutex::new(Phase::Idle);
static FOUND: Mutex<Option<Release>> = Mutex::new(None);
static ANNOUNCED: Mutex<String> = Mutex::new(String::new());
static LAST_CHECK: Mutex<Option<Instant>> = Mutex::new(None);
static ENABLED: AtomicBool = AtomicBool::new(false);
static BUSY: AtomicBool = AtomicBool::new(false);

fn set_phase(p: Phase) {
    *PHASE.lock().unwrap_or_else(|e| e.into_inner()) = p;
}

/// Where the updater is now (Settings → This computer shows it with [`describe`]).
pub fn state() -> Phase {
    PHASE.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The newer release found by the last check.
pub fn available() -> Option<Release> {
    FOUND.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The background check on or off (`config.check_updates`).
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::SeqCst);
}

/// A version found and not announced yet (once per version).
pub fn announce() -> Option<String> {
    let v = available()?.version;
    let mut told = ANNOUNCED.lock().unwrap_or_else(|e| e.into_inner());
    if *told == v {
        return None;
    }
    *told = v.clone();
    Some(v)
}

/// The words for a phase.
pub fn describe(p: &Phase) -> String {
    match p {
        Phase::Idle => String::new(),
        Phase::Checking => "Looking for a new version…".into(),
        Phase::UpToDate => "Encore is up to date.".into(),
        Phase::Available(r) => format!("Encore {} is ready to install.", r.version),
        Phase::Downloading {
            version,
            got,
            total,
        } if *total > 0 => format!(
            "Downloading Encore {version}: {} of {}…",
            crate::fmt::size(*got as f64),
            crate::fmt::size(*total as f64)
        ),
        Phase::Downloading { version, got, .. } => {
            format!(
                "Downloading Encore {version}: {}…",
                crate::fmt::size(*got as f64)
            )
        }
        Phase::Verifying(_) => "Checking the download…".into(),
        Phase::Installing(v) => format!("Installing Encore {v}. Encore starts again by itself."),
        Phase::Failed(e) => e.clone(),
    }
}

/// How this copy can be replaced.
pub fn kind() -> Kind {
    #[cfg(target_os = "linux")]
    {
        if let Some(a) = std::env::var_os("APPIMAGE")
            .map(PathBuf::from)
            .filter(|a| a.is_file())
        {
            return Kind::AppImage(a);
        }
        Kind::Manual
    }
    #[cfg(target_os = "windows")]
    {
        Kind::Installer
    }
    #[cfg(target_os = "macos")]
    {
        std::env::current_exe()
            .ok()
            .and_then(|exe| {
                exe.ancestors()
                    .find(|a| a.extension().is_some_and(|e| e == "app"))
                    .map(Path::to_path_buf)
            })
            .map(Kind::Bundle)
            .unwrap_or(Kind::Manual)
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    {
        Kind::Manual
    }
}

/// The release file for this computer and kind of install (H's names, scripts/desktop-package.sh).
pub fn asset_name(version: &str, kind: &Kind) -> Option<String> {
    let x64 = cfg!(target_arch = "x86_64");
    if cfg!(target_os = "windows") {
        x64.then(|| format!("Encore-{version}-windows-x86_64.exe"))
    } else if cfg!(target_os = "macos") {
        Some(format!("Encore-{version}-macos-universal.dmg"))
    } else if matches!(kind, Kind::AppImage(_)) {
        x64.then(|| format!("Encore-{version}-linux-x86_64.AppImage"))
    } else {
        x64.then(|| format!("encore_{version}_amd64.deb"))
    }
}

/// A newer release than `current` in the GitHub answer (None when there is none), with the file
/// for this computer and the checksum file, both only from the project's release downloads.
pub fn newest(release: &Value, current: &str, kind: &Kind) -> Result<Option<Release>, String> {
    let tag = release
        .get("tag_name")
        .and_then(Value::as_str)
        .unwrap_or("");
    if tag.is_empty() {
        let why = release
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("no release found");
        return Err(format!("Couldn't check for updates: {why}"));
    }
    if release.get("draft").and_then(Value::as_bool) == Some(true)
        || release.get("prerelease").and_then(Value::as_bool) == Some(true)
    {
        return Ok(None);
    }
    let version = tag.trim().trim_start_matches(['v', 'V']).to_string();
    let newer = match (
        semver::Version::parse(&version),
        semver::Version::parse(current),
    ) {
        (Ok(a), Ok(b)) => a > b,
        _ => false,
    };
    if !newer {
        return Ok(None);
    }
    let assets: Vec<Asset> = release
        .get("assets")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|x| Asset {
                    name: x
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    url: x
                        .get("browser_download_url")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    size: x.get("size").and_then(Value::as_u64).unwrap_or(0),
                })
                .filter(|x| !x.name.is_empty() && x.url.starts_with(DOWNLOADS))
                .collect()
        })
        .unwrap_or_default();
    let want = asset_name(&version, kind);
    let asset = want
        .as_ref()
        .and_then(|w| assets.iter().find(|a| &a.name == w).cloned());
    // A release published without the desktop apps (a server-only release) carries no file
    // for this computer: there is nothing to update to yet.
    if want.is_some() && asset.is_none() {
        return Ok(None);
    }
    Ok(Some(Release {
        page: release
            .get("html_url")
            .and_then(Value::as_str)
            .filter(|u| u.starts_with("https://github.com/"))
            .unwrap_or(RELEASES)
            .to_string(),
        notes: release
            .get("body")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        asset,
        sums: assets
            .iter()
            .find(|a| SUMS.contains(&a.name.as_str()))
            .map(|a| a.url.clone()),
        version,
    }))
}

/// `SHA256SUMS` (sha256sum format: `<hex>  <name>`, or `<hex> *<name>`) → name → hex.
pub fn parse_sums(text: &str) -> HashMap<String, String> {
    text.lines()
        .filter_map(|l| {
            let (hash, name) = l.trim().split_once(char::is_whitespace)?;
            let name = name.trim_start().trim_start_matches('*').trim();
            (hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit()) && !name.is_empty())
                .then(|| {
                    (
                        name.trim_start_matches("./").to_string(),
                        hash.to_ascii_lowercase(),
                    )
                })
        })
        .collect()
}

/// Whether a downloaded file may be used: it must be listed in the release's SHA256SUMS (`sums`)
/// with exactly this SHA-256. A release without one is never installed (release.yml always
/// writes it, so its absence means a broken or hand-edited release).
pub fn verify(sums: Option<&str>, name: &str, sha256_hex: &str) -> Result<(), String> {
    let Some(text) = sums else {
        return Err(format!(
            "The release has no SHA256SUMS, so {name} could not be checked and was not installed."
        ));
    };
    let want = parse_sums(text).remove(name).ok_or_else(|| {
        format!("{name} is not listed in the release's SHA256SUMS, so it was not installed.")
    })?;
    if !want.eq_ignore_ascii_case(sha256_hex) {
        return Err(format!(
            "The download does not match the release's SHA256SUMS, so it was not installed (got {sha256_hex}, expected {want})."
        ));
    }
    Ok(())
}

/// The SHA-256 of a file, as lowercase hex.
pub fn hash_file(p: &Path) -> Result<String, String> {
    use sha2::Digest;
    use std::io::Read;
    let mut f = std::fs::File::open(p).map_err(|e| e.to_string())?;
    let mut h = sha2::Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(crate::api::user_agent())
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(Duration::from_secs(60))
        .build()
        .map_err(|e| e.to_string())
}

fn net_words(e: &reqwest::Error) -> String {
    if e.is_connect() || e.is_timeout() {
        "GitHub can't be reached".into()
    } else {
        e.to_string()
    }
}

async fn fetch_latest() -> Result<Value, String> {
    let res = client()?
        .get(LATEST)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send()
        .await
        .map_err(|e| format!("Couldn't check for updates: {}", net_words(&e)))?;
    let status = res.status().as_u16();
    let limited = res
        .headers()
        .get("x-ratelimit-remaining")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.trim() == "0");
    if (status == 403 || status == 429) && limited {
        return Err("Couldn't check for updates: GitHub allows only so many checks an hour from this network. Try again later.".into());
    }
    if status == 404 {
        return Err("Couldn't check for updates: no release found".into());
    }
    if !(200..300).contains(&status) {
        return Err(format!(
            "Couldn't check for updates: GitHub answered {status}"
        ));
    }
    let list = res
        .json::<Value>()
        .await
        .map_err(|e| format!("Couldn't check for updates: {e}"))?;
    full_release(&list).ok_or_else(|| "Couldn't check for updates: no release found".into())
}

/// The newest full release (tag v<major>.<minor>.<patch>, not a draft or prerelease) of GitHub's release list,
/// which is newest first. Hotfixes (hotfix-<version>) are the server's only.
pub fn full_release(list: &Value) -> Option<Value> {
    list.as_array()?
        .iter()
        .find(|r| {
            let tag = r.get("tag_name").and_then(Value::as_str).unwrap_or("").trim();
            r.get("draft").and_then(Value::as_bool) != Some(true)
                && r.get("prerelease").and_then(Value::as_bool) != Some(true)
                && tag
                    .strip_prefix('v')
                    .is_some_and(|v| v.split('.').count() == 3 && semver::Version::parse(v).is_ok())
        })
        .cloned()
}

/// Look for a newer version now.
pub async fn check() -> Result<Option<Release>, String> {
    *LAST_CHECK.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
    if BUSY.load(Ordering::SeqCst) {
        return Ok(available());
    }
    set_phase(Phase::Checking);
    let r = match fetch_latest().await {
        Ok(v) => newest(&v, crate::api::VERSION, &kind()),
        Err(e) => Err(e),
    };
    match &r {
        Ok(Some(rel)) => {
            log::info!("update available: Encore {}", rel.version);
            *FOUND.lock().unwrap_or_else(|e| e.into_inner()) = Some(rel.clone());
            set_phase(Phase::Available(rel.clone()));
        }
        Ok(None) => {
            *FOUND.lock().unwrap_or_else(|e| e.into_inner()) = None;
            set_phase(Phase::UpToDate);
        }
        Err(e) => {
            log::info!("{e}");
            set_phase(Phase::Failed(e.clone()));
        }
    }
    r
}

/// The background check (part of `desktop::events`): 30 s after start, then every 6 hours, while
/// it is on. Never yields a message.
pub(crate) fn watch() -> impl iced::futures::Stream<Item = Message> {
    iced::stream::channel(
        1,
        |_out: iced::futures::channel::mpsc::Sender<Message>| async move {
            tokio::time::sleep(Duration::from_secs(30)).await;
            loop {
                let due = LAST_CHECK
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .is_none_or(|t| t.elapsed() >= EVERY);
                if due && ENABLED.load(Ordering::SeqCst) && !BUSY.load(Ordering::SeqCst) {
                    let _ = check().await;
                }
                tokio::time::sleep(Duration::from_secs(60)).await;
            }
        },
    )
}

/// Check, then install what was found (Settings → This computer's Install button).
pub async fn install_latest() -> Result<Outcome, String> {
    let rel = match available() {
        Some(r) => r,
        None => check().await?.ok_or("Encore is up to date.")?,
    };
    install(rel).await
}

/// Download, check and install `release`; on success Encore quits and starts again.
pub async fn install(release: Release) -> Result<Outcome, String> {
    if BUSY.swap(true, Ordering::SeqCst) {
        return Err("The update is already being installed.".into());
    }
    let r = install_inner(&release).await;
    BUSY.store(false, Ordering::SeqCst);
    match &r {
        Err(e) => {
            log::warn!("update not installed: {e}");
            set_phase(Phase::Failed(e.clone()));
        }
        Ok(Outcome::Opened(_)) => set_phase(Phase::Available(release)),
        Ok(Outcome::Restarting) => {}
    }
    r
}

fn open_in_browser(url: &str) -> Result<Outcome, String> {
    open::that_detached(url).map_err(|e| format!("Could not open the browser: {e}"))?;
    Ok(Outcome::Opened(
        "The download opens in the browser – install it and start Encore again".into(),
    ))
}

async fn install_inner(release: &Release) -> Result<Outcome, String> {
    let kind = kind();
    let Some(asset) = release.asset.clone() else {
        return open_in_browser(&release.page);
    };
    if kind == Kind::Manual {
        return open_in_browser(&asset.url);
    }
    // The checksums first: a release without them is not installed here (nothing unchecked
    // ever runs); its page opens instead, where the person can decide.
    let Some(sums_url) = release.sums.as_deref() else {
        log::warn!(
            "Encore {} has no SHA256SUMS: not installing it, opening the release page",
            release.version
        );
        open::that_detached(&release.page).map_err(|e| format!("Could not open the browser: {e}"))?;
        return Ok(Outcome::Opened(
            "This release has no checksums, so it is not installed from here – download it from the release page"
                .into(),
        ));
    };
    // Where the download goes: next to the AppImage (so it can be renamed over it), else a
    // folder of its own in the temp folder.
    let staging = match &kind {
        Kind::AppImage(target) => {
            let dir = target.parent().ok_or("the AppImage has no folder")?;
            if !writable(dir) {
                return open_in_browser(&asset.url);
            }
            dir.join(format!(".{}.update", asset.name))
        }
        _ => {
            let dir =
                std::env::temp_dir().join(format!("encore-update-{}", clean(&release.version)));
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            dir.join(&asset.name)
        }
    };
    let client = client()?;
    let sums = Some(fetch_text(&client, sums_url).await.map_err(|e| {
        format!("The release's SHA256SUMS could not be read ({e}), so nothing was installed.")
    })?);
    if let Err(e) = download(&client, &asset, &staging, &release.version).await {
        let _ = std::fs::remove_file(&staging);
        return Err(e);
    }
    set_phase(Phase::Verifying(release.version.clone()));
    let file = staging.clone();
    let hex = tokio::task::spawn_blocking(move || hash_file(&file))
        .await
        .map_err(|e| e.to_string())??;
    if let Err(e) = verify(sums.as_deref(), &asset.name, &hex) {
        let _ = std::fs::remove_file(&staging);
        return Err(e);
    }
    log::info!("Encore {} downloaded and checked ({hex})", release.version);
    set_phase(Phase::Installing(release.version.clone()));
    let out = hand_off(&kind, &staging).await?;
    if out == Outcome::Restarting {
        send(Message::Quit);
    }
    Ok(out)
}

fn clean(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        .collect()
}

fn writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".encore-write-test-{}", std::process::id()));
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

async fn fetch_text(client: &reqwest::Client, url: &str) -> Result<String, String> {
    if !url.starts_with(DOWNLOADS) {
        return Err("not a release file".into());
    }
    let res = client.get(url).send().await.map_err(|e| net_words(&e))?;
    if !res.status().is_success() {
        return Err(format!("GitHub answered {}", res.status().as_u16()));
    }
    let b = res.bytes().await.map_err(|e| net_words(&e))?;
    if b.len() > 1 << 20 {
        return Err("it is far too big".into());
    }
    Ok(String::from_utf8_lossy(&b).into_owned())
}

async fn download(
    client: &reqwest::Client,
    asset: &Asset,
    to: &Path,
    version: &str,
) -> Result<u64, String> {
    use tokio::io::AsyncWriteExt;
    if !asset.url.starts_with(DOWNLOADS) {
        return Err("The update is not a file of Encore's releases.".into());
    }
    let mut res = client
        .get(&asset.url)
        .header(reqwest::header::ACCEPT, "application/octet-stream")
        .send()
        .await
        .map_err(|e| format!("The download did not start: {}", net_words(&e)))?;
    if !res.status().is_success() {
        return Err(format!(
            "The download did not start: GitHub answered {}",
            res.status().as_u16()
        ));
    }
    let total = res
        .content_length()
        .filter(|n| *n > 0)
        .unwrap_or(asset.size);
    let mut out = tokio::fs::File::create(to)
        .await
        .map_err(|e| format!("The download cannot be saved: {e}"))?;
    let mut got = 0u64;
    let mut shown = 0u64;
    set_phase(Phase::Downloading {
        version: version.into(),
        got,
        total,
    });
    loop {
        match res.chunk().await {
            Ok(Some(c)) => {
                out.write_all(&c)
                    .await
                    .map_err(|e| format!("The download cannot be saved: {e}"))?;
                got += c.len() as u64;
                if got - shown >= 256 * 1024 {
                    shown = got;
                    set_phase(Phase::Downloading {
                        version: version.into(),
                        got,
                        total,
                    });
                }
            }
            Ok(None) => break,
            Err(e) => return Err(format!("The download stopped: {}", net_words(&e))),
        }
    }
    out.flush().await.map_err(|e| e.to_string())?;
    out.sync_all().await.map_err(|e| e.to_string())?;
    drop(out);
    if asset.size > 0 && got != asset.size {
        return Err("The download was cut off. Try again.".into());
    }
    Ok(got)
}

/// Put the checked file in place and start the helper that starts Encore again once it quit.
async fn hand_off(kind: &Kind, file: &Path) -> Result<Outcome, String> {
    match kind {
        #[cfg(unix)]
        Kind::AppImage(target) => {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o755))
                .map_err(|e| e.to_string())?;
            std::fs::rename(file, target).map_err(|e| format!("Couldn't replace the app: {e}"))?;
            relaunch_unix(
                "sleep 2; exec \"$1\" >/dev/null 2>&1",
                &[target.as_os_str()],
            )?;
            Ok(Outcome::Restarting)
        }
        #[cfg(target_os = "macos")]
        Kind::Bundle(app) => {
            let (app, dmg) = (app.clone(), file.to_path_buf());
            tokio::task::spawn_blocking(move || mac_replace(&app, &dmg))
                .await
                .map_err(|e| e.to_string())?
        }
        #[cfg(target_os = "windows")]
        Kind::Installer => {
            windows_run_installer(file)?;
            Ok(Outcome::Restarting)
        }
        _ => {
            let _ = file;
            open_in_browser(RELEASES)
        }
    }
}

/// `sh -c <script> sh <args…>`, detached from Encore (the arguments are never put in the script).
/// The new version it starts is told so (`single::RELAUNCH_ENV`): it waits for this one to quit
/// rather than handing over to it.
#[cfg(unix)]
fn relaunch_unix(script: &str, args: &[&std::ffi::OsStr]) -> Result<(), String> {
    use std::os::unix::process::CommandExt;
    std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(script)
        .arg("sh")
        .args(args)
        .env(super::single::RELAUNCH_ENV, "1")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .process_group(0)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Couldn't start the new version: {e}"))
}

/// Mount the disk image, copy its app next to this one, and let a helper swap them and open the
/// new one after Encore quit. A folder Encore cannot write gets the disk image opened instead.
#[cfg(target_os = "macos")]
fn mac_replace(app: &Path, dmg: &Path) -> Result<Outcome, String> {
    let parent = app.parent().ok_or("the app has no folder")?;
    if !writable(parent) {
        open::that_detached(dmg).map_err(|e| e.to_string())?;
        return Ok(Outcome::Opened(
            "The new version is open in Finder – drag Encore to Applications and start it again"
                .into(),
        ));
    }
    let mount = std::env::temp_dir().join(format!("encore-update-mount-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&mount);
    let st = std::process::Command::new("hdiutil")
        .args([
            "attach",
            "-nobrowse",
            "-readonly",
            "-noautoopen",
            "-mountpoint",
        ])
        .arg(&mount)
        .arg(dmg)
        .stdout(std::process::Stdio::null())
        .status()
        .map_err(|e| e.to_string())?;
    if !st.success() {
        return Err("The disk image of the update could not be opened.".into());
    }
    let found = std::fs::read_dir(&mount).ok().and_then(|rd| {
        rd.flatten()
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|x| x == "app"))
    });
    let staged = parent.join(".Encore-update.app");
    let _ = std::fs::remove_dir_all(&staged);
    let copied = found.as_ref().map(|src| {
        std::process::Command::new("ditto")
            .arg(src)
            .arg(&staged)
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    });
    let _ = std::process::Command::new("hdiutil")
        .args(["detach", "-quiet"])
        .arg(&mount)
        .status();
    if copied != Some(true) {
        let _ = std::fs::remove_dir_all(&staged);
        return Err("The update's app could not be copied.".into());
    }
    relaunch_unix(
        "sleep 2; rm -rf \"$1\" && mv \"$2\" \"$1\" && open \"$1\"",
        &[app.as_os_str(), staged.as_os_str()],
    )?;
    Ok(Outcome::Restarting)
}

/// A PowerShell string: single quotes, inner ones doubled.
#[cfg(any(target_os = "windows", test))]
pub fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// The PowerShell that waits for Encore to quit, runs the installer silently, and starts the
/// installed Encore: from the folder the installer recorded (InstallLocation of its Apps &
/// features entry, which a silent install updates in place), else the default folder, else this
/// copy. Never the classic app's Encore.exe, which may sit in the default folder too.
#[cfg(any(target_os = "windows", test))]
pub fn windows_script(installer: &str, current: &str) -> String {
    format!(
        "Start-Sleep -Seconds 2; \
         Start-Process -FilePath {inst} -ArgumentList '/S' -Wait; \
         $d = (Get-ItemProperty -Path 'HKCU:\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\EncoreDesktop' -ErrorAction SilentlyContinue).InstallLocation; \
         $c = @($(if ($d) {{ Join-Path $d 'encore-desktop.exe' }}), (Join-Path $env:LOCALAPPDATA 'Encore\\encore-desktop.exe'), {cur}) | Where-Object {{ $_ -and (Test-Path $_) }} | Select-Object -First 1; \
         if ($c) {{ Start-Process -FilePath $c }}",
        inst = ps_quote(installer),
        cur = ps_quote(current),
    )
}

#[cfg(target_os = "windows")]
fn windows_run_installer(installer: &Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    let current = std::env::current_exe().map_err(|e| e.to_string())?;
    let script = windows_script(&installer.to_string_lossy(), &current.to_string_lossy());
    std::process::Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-WindowStyle",
            "Hidden",
            "-Command",
        ])
        .arg(script)
        .env(super::single::RELAUNCH_ENV, "1")
        .creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Couldn't start the installer: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn release(tag: &str, names: &[&str]) -> Value {
        json!({
            "tag_name": tag,
            "html_url": "https://github.com/Krifmaz/Encore/releases/tag/v0.28.0",
            "body": "notes",
            "assets": names.iter().map(|n| json!({
                "name": n,
                "browser_download_url": format!("{DOWNLOADS}{tag}/{n}"),
                "size": 10,
            })).collect::<Vec<_>>(),
        })
    }

    #[test]
    fn hotfixes_and_prereleases_are_passed_over_for_the_newest_full_release() {
        let mut pre = release("v0.36.0", &[]);
        pre["prerelease"] = json!(true);
        let list = json!([release("hotfix-0.35.2.17", &[]), pre, release("v0.35.2", &[]), release("v0.35.1", &[])]);
        assert_eq!(full_release(&list).unwrap()["tag_name"], "v0.35.2");
        assert!(full_release(&json!([release("hotfix-0.35.2.17", &[])])).is_none());
        assert!(full_release(&json!({"message": "Not Found"})).is_none());
    }

    #[test]
    fn a_newer_release_brings_this_computers_file_and_the_sums() {
        let names = [
            "Encore-0.28.0-windows-x86_64.exe",
            "Encore-0.28.0-macos-universal.dmg",
            "Encore-0.28.0-linux-x86_64.AppImage",
            "encore_0.28.0_amd64.deb",
            "Encore_0.28.0_x64-setup.exe",
            "SHA256SUMS",
        ];
        let kind = if cfg!(target_os = "linux") {
            Kind::AppImage(PathBuf::from("/x/Encore.AppImage"))
        } else {
            kind()
        };
        let r = newest(&release("v0.28.0", &names), "0.27.0", &kind)
            .unwrap()
            .unwrap();
        assert_eq!(r.version, "0.28.0");
        assert_eq!(r.notes, "notes");
        assert!(r.sums.as_deref().unwrap().ends_with("/SHA256SUMS"));
        let a = r.asset.expect("a file for this computer");
        assert_eq!(Some(a.name.clone()), asset_name("0.28.0", &kind));
        assert!(
            !a.name.contains("x64-setup"),
            "never the classic app's installer"
        );
        if cfg!(target_os = "linux") {
            let deb = newest(&release("v0.28.0", &names), "0.27.0", &Kind::Manual)
                .unwrap()
                .unwrap();
            assert_eq!(deb.asset.unwrap().name, "encore_0.28.0_amd64.deb");
        }
    }

    #[test]
    fn same_older_and_odd_releases_are_not_offered() {
        assert_eq!(
            newest(&release("v0.27.0", &[]), "0.27.0", &Kind::Manual),
            Ok(None)
        );
        assert_eq!(
            newest(&release("0.26.9", &[]), "0.27.0", &Kind::Manual),
            Ok(None)
        );
        assert_eq!(
            newest(&release("junk", &[]), "0.27.0", &Kind::Manual),
            Ok(None)
        );
        let mut pre = release("v0.30.0", &[]);
        pre["prerelease"] = json!(true);
        assert_eq!(newest(&pre, "0.27.0", &Kind::Manual), Ok(None));
        assert!(
            newest(&json!({ "message": "Not Found" }), "0.27.0", &Kind::Manual)
                .unwrap_err()
                .contains("Not Found")
        );
        // a newer release without this computer's file (a server-only release) is not offered
        if asset_name("0.28.0", &Kind::Manual).is_some() {
            let old_files = release("v0.28.0", &["Encore-0.27.0-linux-x86_64.AppImage", "SHA256SUMS"]);
            assert_eq!(newest(&old_files, "0.27.0", &Kind::Manual), Ok(None));
        }
        // files from anywhere else are ignored
        let mut r = release(
            "v0.28.0",
            &[
                "SHA256SUMS",
                "encore_0.28.0_amd64.deb",
                "Encore-0.28.0-windows-x86_64.exe",
                "Encore-0.28.0-macos-universal.dmg",
            ],
        );
        r["assets"][0]["browser_download_url"] = json!("https://evil.example/SHA256SUMS");
        assert_eq!(
            newest(&r, "0.27.0", &Kind::Manual).unwrap().unwrap().sums,
            None
        );
    }

    #[test]
    fn sums_are_read_in_both_sha256sum_styles() {
        let h1 = "a".repeat(64);
        let h2 = "B".repeat(64);
        let text = format!(
            "{h1}  Encore-1-linux-x86_64.AppImage\n{h2} *./encore_1_amd64.deb\nnot a line\n"
        );
        let m = parse_sums(&text);
        assert_eq!(m["Encore-1-linux-x86_64.AppImage"], h1);
        assert_eq!(m["encore_1_amd64.deb"], "b".repeat(64));
        assert_eq!(m.len(), 2);
    }

    #[test]
    fn a_file_that_does_not_match_its_sum_is_refused() {
        let good = "c".repeat(64);
        let sums = format!("{good}  Encore-1.exe\n");
        assert!(verify(Some(&sums), "Encore-1.exe", &good).is_ok());
        assert!(verify(Some(&sums), "Encore-1.exe", &good.to_uppercase()).is_ok());
        let bad = verify(Some(&sums), "Encore-1.exe", &"d".repeat(64)).unwrap_err();
        assert!(bad.contains("does not match"), "{bad}");
        let missing = verify(Some(&sums), "Encore-2.exe", &good).unwrap_err();
        assert!(missing.contains("not listed"), "{missing}");
        assert!(
            verify(Some(""), "Encore-1.exe", &good).is_err(),
            "an empty SHA256SUMS lists nothing"
        );
        assert!(
            verify(None, "Encore-1.exe", "anything").is_err(),
            "a release without sums is not installed"
        );
    }

    #[test]
    fn files_hash_as_sha256sum_does() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("f");
        std::fs::write(&p, b"abc").unwrap();
        assert_eq!(
            hash_file(&p).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert!(hash_file(&d.path().join("missing")).is_err());
    }

    #[test]
    fn the_words_and_the_windows_helper() {
        assert_eq!(describe(&Phase::UpToDate), "Encore is up to date.");
        assert_eq!(
            describe(&Phase::Downloading {
                version: "0.28.0".into(),
                got: 2_000_000,
                total: 40_000_000
            }),
            "Downloading Encore 0.28.0: 2.0 MB of 40.0 MB…"
        );
        assert_eq!(
            ps_quote("C:\\Users\\O'Neil\\x.exe"),
            "'C:\\Users\\O''Neil\\x.exe'"
        );
        let s = windows_script(
            "C:\\T\\Encore-1-windows-x86_64.exe",
            "C:\\A\\encore-desktop.exe",
        );
        assert!(s.contains("-ArgumentList '/S' -Wait"));
        assert!(s.contains("'C:\\T\\Encore-1-windows-x86_64.exe'"));
        // the folder the installer recorded comes first; the classic app's program never starts
        assert!(
            s.find("Uninstall\\EncoreDesktop").unwrap() < s.find("'C:\\A\\encore-desktop.exe'").unwrap()
        );
        assert!(!s.contains("Encore.exe"));
        assert!(
            s.find("Start-Sleep").unwrap() < s.find("Start-Process").unwrap(),
            "Encore quits first"
        );
        assert_eq!(clean("0.28.0/../x"), "0.28.0..x");
    }
}
