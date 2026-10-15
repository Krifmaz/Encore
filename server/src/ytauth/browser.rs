//! The managed browser: a headless Chrome Encore downloads (Download tools → "Browser for YouTube
//! sign-in") and drives over the Chrome DevTools Protocol. It keeps its own profile in
//! `<config>/youtube-browser/profile`, so a sign-in done once through the live view stays; a refresh
//! opens music.youtube.com in that profile (which lets Google rotate the cookies the normal way),
//! checks it is still signed in and exports the cookies to `<config>/youtube-browser/cookies.txt`.
//!
//! x86_64: Chrome for Testing (`chrome-linux64`, the last-known-good Stable); aarch64: Playwright's
//! Chromium build for linux-arm64 (Google publishes no arm64 Linux Chrome).
use super::jar::Jar;
use crate::state::AppState;
use crate::tools::ToolInfo;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, oneshot};

pub const CFT_JSON: &str = "https://googlechromelabs.github.io/chrome-for-testing/last-known-good-versions-with-downloads.json";
pub const PLAYWRIGHT_JSON: &str = "https://raw.githubusercontent.com/microsoft/playwright/main/packages/playwright-core/browsers.json";
const PLAYWRIGHT_CDNS: [&str; 2] = ["https://cdn.playwright.dev/dbazure/download/playwright", "https://playwright.download.prss.microsoft.com/dbazure/download/playwright"];
/// Where a sign-in starts: Google's sign-in page, continuing to YouTube Music.
pub const SIGN_IN_URL: &str = "https://accounts.google.com/ServiceLogin?service=youtube&hl=en&continue=https%3A%2F%2Fmusic.youtube.com%2F";

/// One browser at a time: the profile can only be open once.
pub static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

// ---- install ------------------------------------------------------------------------------------

/// `<config>/youtube-browser`: the profile and the exported cookies.
pub fn home(data: &Path) -> PathBuf {
    data.join("youtube-browser")
}

pub fn profile_dir(data: &Path) -> PathBuf {
    home(data).join("profile")
}

/// `<config>/tools/browser/<version>/…` plus `current`.
pub fn tool_dir(data: &Path) -> PathBuf {
    data.join("tools").join("browser")
}

fn current(data: &Path) -> Option<String> {
    let v = std::fs::read_to_string(tool_dir(data).join("current")).ok()?;
    let v = v.trim();
    (!v.is_empty() && !v.contains('/') && !v.starts_with('.')).then(|| v.to_string())
}

/// The `chrome` program inside an unpacked build (chrome-linux64/chrome, chrome-linux/chrome …).
fn find_exe(dir: &Path) -> Option<PathBuf> {
    walkdir::WalkDir::new(dir).max_depth(3).into_iter().flatten()
        .find(|e| e.file_type().is_file() && e.file_name() == "chrome")
        .map(|e| e.into_path())
}

/// (program, version) of the installed browser.
pub fn installed(data: &Path) -> Option<(PathBuf, String)> {
    let ver = current(data)?;
    let exe = find_exe(&tool_dir(data).join(&ver))?;
    Some((exe, ver))
}

/// Chrome for Testing's JSON → (version, linux64 zip url).
pub fn parse_cft(v: &Value) -> Option<(String, String)> {
    let s = &v["channels"]["Stable"];
    let ver = s["version"].as_str()?.to_string();
    let url = s["downloads"]["chrome"].as_array()?.iter().find(|d| d["platform"] == "linux64")?["url"].as_str()?.to_string();
    Some((ver, url))
}

/// Playwright's browsers.json → (Chromium version, revision).
pub fn parse_playwright(v: &Value) -> Option<(String, String)> {
    let b = v["browsers"].as_array()?.iter().find(|b| b["name"] == "chromium")?;
    let rev = b["revision"].as_str()?.to_string();
    let ver = b["browserVersion"].as_str().map(String::from).unwrap_or_else(|| format!("r{rev}"));
    Some((ver, rev))
}

/// The newest build for this server: (version, download url).
pub async fn latest(st: &AppState) -> anyhow::Result<(String, String)> {
    match std::env::consts::ARCH {
        "x86_64" => {
            let v: Value = st.http.get(CFT_JSON).timeout(Duration::from_secs(20)).send().await?.json().await?;
            parse_cft(&v).ok_or_else(|| anyhow::anyhow!("Chrome for Testing lists no linux64 Stable build"))
        }
        "aarch64" => {
            let v: Value = st.http.get(PLAYWRIGHT_JSON).timeout(Duration::from_secs(20)).send().await?.json().await?;
            let (ver, rev) = parse_playwright(&v).ok_or_else(|| anyhow::anyhow!("Playwright lists no Chromium build"))?;
            // the first CDN that has the build
            for cdn in PLAYWRIGHT_CDNS {
                let url = format!("{cdn}/builds/chromium/{rev}/chromium-linux-arm64.zip");
                if st.http.head(&url).timeout(Duration::from_secs(20)).send().await.is_ok_and(|r| r.status().is_success()) {
                    return Ok((ver, url));
                }
            }
            Ok((ver, format!("{}/builds/chromium/{rev}/chromium-linux-arm64.zip", PLAYWRIGHT_CDNS[0])))
        }
        a => anyhow::bail!("No browser build for this server ({a})"),
    }
}

/// Download and unpack a build, check it starts, make it `current` and drop older ones.
pub async fn install(st: &AppState, ver: &str, url: &str) -> anyhow::Result<String> {
    use futures_util::StreamExt;
    if ver.is_empty() || ver.contains('/') || ver.starts_with('.') {
        anyhow::bail!("odd version {ver:?}");
    }
    let root = tool_dir(st.data_dir());
    std::fs::create_dir_all(&root)?;
    let zip = root.join(format!(".{ver}.zip"));
    let r = st.http.get(url).timeout(Duration::from_secs(1800)).send().await.map_err(|e| anyhow::anyhow!("download failed: {e}"))?;
    if !r.status().is_success() {
        anyhow::bail!("download failed: {url} returned {}", r.status());
    }
    {
        let mut f = tokio::fs::File::create(&zip).await?;
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
    }
    let (root2, ver2, zip2) = (root.clone(), ver.to_string(), zip.clone());
    let res = tokio::task::spawn_blocking(move || -> anyhow::Result<PathBuf> {
        let tmp = root2.join(format!(".{ver2}.new"));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp)?;
        let mut a = zip::ZipArchive::new(std::fs::File::open(&zip2)?).map_err(|e| anyhow::anyhow!("not a zip file: {e}"))?;
        a.extract(&tmp).map_err(|e| anyhow::anyhow!("unpacking failed: {e}"))?;
        let dest = root2.join(&ver2);
        let _ = std::fs::remove_dir_all(&dest);
        std::fs::rename(&tmp, &dest)?;
        find_exe(&dest).ok_or_else(|| anyhow::anyhow!("the download has no chrome program"))
    }).await?;
    let _ = std::fs::remove_file(&zip);
    let exe = res?;
    // it has to start here (a server without Chrome's libraries can't run it)
    let o = tokio::time::timeout(Duration::from_secs(60), tokio::process::Command::new(&exe).arg("--version").stdin(std::process::Stdio::null()).kill_on_drop(true).output()).await;
    let ok = matches!(&o, Ok(Ok(o)) if o.status.success());
    if !ok {
        let why = match o {
            Ok(Ok(o)) => String::from_utf8_lossy(&o.stderr).lines().last().unwrap_or("").to_string(),
            Ok(Err(e)) => e.to_string(),
            Err(_) => "no answer within 60 s".into(),
        };
        let _ = std::fs::remove_dir_all(root.join(ver));
        anyhow::bail!("The browser was downloaded but does not start here ({why}). The Encore Docker image has the libraries it needs; on other systems install Chrome's dependencies.");
    }
    std::fs::write(root.join(".current.tmp"), format!("{ver}\n"))?;
    std::fs::rename(root.join(".current.tmp"), root.join("current"))?;
    // older builds go only while no browser runs (a refresh or the live sign-in uses their files);
    // when it stays busy they are left for the next install
    if let Ok(_g) = tokio::time::timeout(Duration::from_secs(300), LOCK.lock()).await {
        if let Ok(rd) = std::fs::read_dir(&root) {
            for e in rd.flatten() {
                if e.path().is_dir() && e.file_name().to_string_lossy() != ver {
                    let _ = std::fs::remove_dir_all(e.path());
                }
            }
        }
    }
    st.db.log("info", &format!("Browser for YouTube sign-in {ver} installed"));
    Ok(ver.to_string())
}

/// Remove the program (the profile, and with it the sign-in, stays).
pub fn reset(data: &Path) -> anyhow::Result<()> {
    // never under a running browser
    let _g = LOCK.try_lock().map_err(|_| anyhow::anyhow!("The browser is in use (a refresh or the sign-in window) – try again in a minute"))?;
    let d = tool_dir(data);
    if d.exists() {
        std::fs::remove_dir_all(d)?;
    }
    Ok(())
}

/// The Download tools row.
pub fn info(st: &AppState, last: &Value) -> ToolInfo {
    let inst = installed(st.data_dir()).map(|(_, v)| v);
    let latest = last["latest"].as_str().map(String::from);
    let update_available = inst.is_some() && latest.as_deref().is_some_and(|l| inst.as_deref().is_some_and(|i| crate::tools::newer(l, i)));
    let note = if inst.is_some() {
        "Signs in to YouTube for yt-dlp and keeps the cookies fresh (Settings → YouTube sign-in)"
    } else {
        "Not installed – Install downloads a headless Chrome (about 150 MB) so Encore can sign in to YouTube by itself"
    };
    ToolInfo { id: "browser".into(), name: crate::tools::label("browser").into(), source: if inst.is_some() { "updated" } else { "missing" }.into(),
        resettable: inst.is_some(), installed: inst, latest, update_available, checked_at: last["checked_at"].as_f64(),
        error: last["error"].as_str().map(String::from), note: note.into(), updatable: true }
}

// ---- Chrome DevTools Protocol -------------------------------------------------------------------

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>>;

/// One DevTools connection to the browser (flat sessions: page commands carry a sessionId).
pub struct Cdp {
    tx: mpsc::UnboundedSender<String>,
    next: AtomicU64,
    pending: Pending,
    events: broadcast::Sender<Value>,
}

impl Cdp {
    /// DevTools over `--remote-debugging-pipe`: JSON messages ended by a NUL byte, commands written to
    /// the browser's fd 3, answers and events read from its fd 4.
    pub fn over_pipe<R, W>(read: R, mut write: W) -> Cdp
    where
        R: tokio::io::AsyncRead + Unpin + Send + 'static,
        W: tokio::io::AsyncWrite + Unpin + Send + 'static,
    {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let pending: Pending = Default::default();
        let (events, _) = broadcast::channel(256);
        tokio::spawn(async move {
            while let Some(m) = rx.recv().await {
                let mut b = m.into_bytes();
                b.push(0);
                if write.write_all(&b).await.is_err() || write.flush().await.is_err() {
                    break;
                }
            }
        });
        let (p2, ev2) = (pending.clone(), events.clone());
        tokio::spawn(async move {
            let mut r = tokio::io::BufReader::new(read);
            let mut buf = Vec::new();
            loop {
                buf.clear();
                match r.read_until(0, &mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                if buf.last() == Some(&0) {
                    buf.pop();
                }
                let Ok(v) = serde_json::from_slice::<Value>(&buf) else { continue };
                if let Some(id) = v["id"].as_u64() {
                    if let Some(s) = p2.lock().unwrap().remove(&id) {
                        let r = if v["error"].is_object() { Err(v["error"]["message"].as_str().unwrap_or("error").to_string()) } else { Ok(v["result"].clone()) };
                        let _ = s.send(r);
                    }
                } else if v["method"].is_string() {
                    let _ = ev2.send(v);
                }
            }
            // the browser went away: wake every waiting call
            p2.lock().unwrap().clear();
        });
        Cdp { tx, next: AtomicU64::new(1), pending, events }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Value> {
        self.events.subscribe()
    }

    pub async fn call(&self, method: &str, params: Value, session: Option<&str>) -> anyhow::Result<Value> {
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        let mut msg = json!({ "id": id, "method": method, "params": params });
        if let Some(s) = session {
            msg["sessionId"] = json!(s);
        }
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        self.tx.send(msg.to_string()).map_err(|_| anyhow::anyhow!("the browser connection is closed"))?;
        match tokio::time::timeout(Duration::from_secs(30), rx).await {
            Ok(Ok(Ok(v))) => Ok(v),
            Ok(Ok(Err(e))) => anyhow::bail!("{method}: {e}"),
            Ok(Err(_)) => anyhow::bail!("the browser closed"),
            Err(_) => {
                self.pending.lock().unwrap().remove(&id);
                anyhow::bail!("{method}: no answer from the browser")
            }
        }
    }
}

/// A desktop Chrome user agent for the installed version (never "HeadlessChrome").
pub fn user_agent(version: &str) -> String {
    let major = version.split('.').next().filter(|m| m.chars().all(|c| c.is_ascii_digit()) && !m.is_empty()).unwrap_or("141");
    format!("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{major}.0.0.0 Safari/537.36")
}

/// The command line: new headless mode, the persistent profile, a normal user agent. Never
/// --enable-automation; --no-sandbox only when running as root (Chrome refuses to start otherwise).
pub fn args(profile: &Path, ua: &str, root: bool) -> Vec<String> {
    let mut a: Vec<String> = [
        // DevTools over fds 3/4 only: no TCP port another local process could connect to and read the
        // Google session from
        "--headless=new", "--remote-debugging-pipe", "--no-first-run", "--no-default-browser-check",
        "--disable-dev-shm-usage", "--password-store=basic", "--use-mock-keychain", "--window-size=1280,860", "--lang=en-US",
        "--disable-features=Translate,MediaRouter", "--mute-audio",
    ].iter().map(|s| s.to_string()).collect();
    a.push(format!("--user-data-dir={}", profile.display()));
    a.push(format!("--user-agent={ua}"));
    if root {
        a.push("--no-sandbox".into());
    }
    a.push("about:blank".into());
    a
}

/// A running browser with one page attached.
pub struct Browser {
    child: tokio::process::Child,
    pub cdp: Cdp,
    pub session: String,
}

/// A pipe, both ends close-on-exec (the child gets its ends as fds 3 and 4 only).
fn pipe() -> std::io::Result<(std::os::fd::OwnedFd, std::os::fd::OwnedFd)> {
    use std::os::fd::FromRawFd;
    let mut fds = [0; 2];
    // SAFETY: pipe2 writes two fds into the array; each is owned once below
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(unsafe { (std::os::fd::OwnedFd::from_raw_fd(fds[0]), std::os::fd::OwnedFd::from_raw_fd(fds[1])) })
}

fn is_root() -> bool {
    // SAFETY: geteuid has no preconditions
    unsafe { libc::geteuid() == 0 }
}

impl Browser {
    pub async fn launch(data: &Path) -> anyhow::Result<Browser> {
        use tokio::io::AsyncBufReadExt;
        let (exe, ver) = installed(data).ok_or_else(|| anyhow::anyhow!("The browser for YouTube sign-in is not installed – install it under Download tools"))?;
        let profile = profile_dir(data);
        std::fs::create_dir_all(&profile)?;
        // the profile holds the Google session: Encore's alone
        for d in [home(data), profile.clone()] {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o700));
        }
        // a browser that crashed leaves its lock behind
        for f in ["SingletonLock", "SingletonSocket", "SingletonCookie"] {
            let _ = std::fs::remove_file(profile.join(f));
        }
        let ua = user_agent(&ver);
        // fd 3: commands to the browser, fd 4: its answers (--remote-debugging-pipe)
        let (cmd_r, cmd_w) = pipe()?;
        let (ev_r, ev_w) = pipe()?;
        let (cr, ew) = (std::os::fd::AsRawFd::as_raw_fd(&cmd_r), std::os::fd::AsRawFd::as_raw_fd(&ev_w));
        let mut cmd = tokio::process::Command::new(&exe);
        cmd.args(args(&profile, &ua, is_root())).stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::piped()).kill_on_drop(true);
        // SAFETY: only async-signal-safe calls (fcntl, dup2) between fork and exec
        unsafe {
            cmd.pre_exec(move || {
                // out of the way of 3 and 4 first, then into place (dup2 leaves them open across exec)
                let a = libc::fcntl(cr, libc::F_DUPFD_CLOEXEC, 10);
                let b = libc::fcntl(ew, libc::F_DUPFD_CLOEXEC, 10);
                if a < 0 || b < 0 || libc::dup2(a, 3) < 0 || libc::dup2(b, 4) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = cmd.spawn().map_err(|e| anyhow::anyhow!("could not start the browser: {e}"))?;
        drop(cmd_r);
        drop(ev_w);
        let tail: Arc<Mutex<Vec<String>>> = Default::default();
        {
            // keep reading its output so it never blocks on a full pipe (the last lines explain a failed start)
            let mut lines = tokio::io::BufReader::new(child.stderr.take().unwrap()).lines();
            let tail = tail.clone();
            tokio::spawn(async move {
                while let Ok(Some(l)) = lines.next_line().await {
                    let mut t = tail.lock().unwrap();
                    t.push(l);
                    if t.len() > 5 {
                        t.remove(0);
                    }
                }
            });
        }
        let cdp = Cdp::over_pipe(tokio::net::unix::pipe::Receiver::from_owned_fd(ev_r)?, tokio::net::unix::pipe::Sender::from_owned_fd(cmd_w)?);
        let targets = match tokio::time::timeout(Duration::from_secs(40), cdp.call("Target.getTargets", json!({}), None)).await {
            Ok(Ok(t)) => t,
            _ => {
                let _ = child.kill().await;
                let last = tail.lock().unwrap().last().cloned();
                anyhow::bail!("the browser did not start{}", last.map(|l| format!(": {l}")).unwrap_or_default());
            }
        };
        let target = match targets["targetInfos"].as_array().and_then(|t| t.iter().find(|t| t["type"] == "page")).and_then(|t| t["targetId"].as_str()) {
            Some(t) => t.to_string(),
            None => cdp.call("Target.createTarget", json!({ "url": "about:blank" }), None).await?["targetId"].as_str().unwrap_or("").to_string(),
        };
        let session = cdp.call("Target.attachToTarget", json!({ "targetId": target, "flatten": true }), None).await?["sessionId"].as_str().unwrap_or("").to_string();
        let b = Browser { child, cdp, session };
        b.page("Network.setUserAgentOverride", json!({ "userAgent": ua, "acceptLanguage": "en-US,en", "platform": "Linux" })).await?;
        b.page("Page.enable", json!({})).await?;
        b.page("Emulation.setDeviceMetricsOverride", json!({ "width": 1280, "height": 860, "deviceScaleFactor": 1, "mobile": false })).await?;
        Ok(b)
    }

    /// A command for the page.
    pub async fn page(&self, method: &str, params: Value) -> anyhow::Result<Value> {
        self.cdp.call(method, params, Some(&self.session)).await
    }

    pub async fn navigate(&self, url: &str) -> anyhow::Result<()> {
        self.page("Page.navigate", json!({ "url": url })).await.map(|_| ())
    }

    /// Wait for the page's load event (at most `limit`).
    pub async fn wait_load(&self, limit: Duration) {
        let mut ev = self.cdp.subscribe();
        let _ = tokio::time::timeout(limit, async {
            while let Ok(e) = ev.recv().await {
                if e["method"] == "Page.loadEventFired" {
                    break;
                }
            }
        }).await;
    }

    /// YouTube's own page config says it is signed in (ytcfg LOGGED_IN).
    pub async fn logged_in(&self) -> bool {
        let r = self.page("Runtime.evaluate", json!({ "expression": "(function(){try{return !!(window.ytcfg && ytcfg.get && ytcfg.get('LOGGED_IN'))}catch(e){return false}})()", "returnByValue": true })).await;
        r.ok().and_then(|v| v["result"]["value"].as_bool()).unwrap_or(false)
    }

    pub async fn url(&self) -> String {
        let r = self.page("Runtime.evaluate", json!({ "expression": "location.href", "returnByValue": true })).await;
        r.ok().and_then(|v| v["result"]["value"].as_str().map(String::from)).unwrap_or_default()
    }

    /// Every youtube.com / google.com cookie of the profile.
    pub async fn cookies(&self) -> anyhow::Result<Jar> {
        let v = match self.cdp.call("Storage.getCookies", json!({}), None).await {
            Ok(v) => v,
            Err(_) => self.page("Network.getAllCookies", json!({})).await?,
        };
        Ok(Jar::from_cdp(&v))
    }

    pub async fn close(mut self) {
        let _ = tokio::time::timeout(Duration::from_secs(5), self.cdp.call("Browser.close", json!({}), None)).await;
        if tokio::time::timeout(Duration::from_secs(5), self.child.wait()).await.is_err() {
            let _ = self.child.kill().await;
        }
    }
}

/// Write cookies.txt for yt-dlp, readable only by Encore.
pub fn write_cookies(data: &Path, jar: &Jar) -> anyhow::Result<()> {
    write_private(&home(data).join("cookies.txt"), &jar.to_netscape())
}

pub fn write_private(path: &Path, text: &str) -> anyhow::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p)?;
    }
    let tmp = path.with_extension("tmp");
    let _ = std::fs::remove_file(&tmp);
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?;
    f.write_all(text.as_bytes())?;
    f.sync_all()?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// What a refresh found.
pub struct Refreshed {
    pub signed_in: bool,
    pub jar: Jar,
}

/// Open YouTube Music in the profile, wait for it, check the sign-in and export the cookies.
pub async fn refresh(data: &Path) -> anyhow::Result<Refreshed> {
    let _g = tokio::time::timeout(Duration::from_secs(300), LOCK.lock()).await.map_err(|_| anyhow::anyhow!("the browser is busy"))?;
    let b = Browser::launch(data).await?;
    let res: anyhow::Result<Refreshed> = async {
        b.navigate("https://music.youtube.com/").await?;
        b.wait_load(Duration::from_secs(30)).await;
        // the page's scripts set and rotate cookies after the load event
        tokio::time::sleep(Duration::from_secs(6)).await;
        let li = b.logged_in().await;
        let jar = b.cookies().await?;
        let signed_in = li && jar.get("music.youtube.com", "SAPISID").is_some();
        Ok(Refreshed { signed_in, jar })
    }.await;
    b.close().await;
    let r = res?;
    if r.signed_in {
        write_cookies(data, &r.jar)?;
    }
    Ok(r)
}

// ---- live view (sign-in) -----------------------------------------------------------------------

/// Map one input message from the live view to DevTools commands.
pub fn input_commands(m: &Value) -> Vec<(&'static str, Value)> {
    let num = |k: &str| m[k].as_f64().unwrap_or(0.0);
    match m["type"].as_str().unwrap_or("") {
        "mouse" => {
            let kind = match m["event"].as_str().unwrap_or("") {
                "down" => "mousePressed",
                "up" => "mouseReleased",
                "wheel" => "mouseWheel",
                _ => "mouseMoved",
            };
            let mut p = json!({ "type": kind, "x": num("x"), "y": num("y"), "modifiers": 0 });
            if kind == "mousePressed" || kind == "mouseReleased" {
                p["button"] = json!(m["button"].as_str().unwrap_or("left"));
                p["clickCount"] = json!(m["clickCount"].as_i64().unwrap_or(1));
            }
            if kind == "mouseWheel" {
                p["deltaX"] = json!(num("deltaX"));
                p["deltaY"] = json!(num("deltaY"));
            }
            vec![("Input.dispatchMouseEvent", p)]
        }
        "text" => {
            let t = m["text"].as_str().unwrap_or("");
            if t.is_empty() { vec![] } else { vec![("Input.insertText", json!({ "text": t }))] }
        }
        "key" => {
            let key = m["key"].as_str().unwrap_or("");
            let code = m["code"].as_str().unwrap_or(key);
            let vk = m["keyCode"].as_i64().unwrap_or(match key {
                "Enter" => 13, "Backspace" => 8, "Tab" => 9, "Escape" => 27, "Delete" => 46,
                "ArrowLeft" => 37, "ArrowUp" => 38, "ArrowRight" => 39, "ArrowDown" => 40, "Home" => 36, "End" => 35,
                _ => 0,
            });
            let mut down = json!({ "type": "rawKeyDown", "key": key, "code": code, "windowsVirtualKeyCode": vk, "nativeVirtualKeyCode": vk });
            if key == "Enter" {
                down["type"] = json!("keyDown");
                down["text"] = json!("\r");
            }
            vec![("Input.dispatchKeyEvent", down), ("Input.dispatchKeyEvent", json!({ "type": "keyUp", "key": key, "code": code, "windowsVirtualKeyCode": vk, "nativeVirtualKeyCode": vk }))]
        }
        "nav" => match m["action"].as_str().unwrap_or("") {
            "back" => vec![("Runtime.evaluate", json!({ "expression": "history.back()" }))],
            "reload" => vec![("Page.reload", json!({}))],
            "signin" => vec![("Page.navigate", json!({ "url": SIGN_IN_URL }))],
            _ => vec![],
        },
        _ => vec![],
    }
}
