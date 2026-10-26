//! The Apple Music wrapper plugin (`wrapper`): wrapper-v2 (github.com/glomatico/wrapper-v2), the signed-in
//! service gamdl decrypts ALAC and Dolby Atmos through, run beside Encore so the gamdl card's "Wrapper URL"
//! fills itself in.
//!
//! Two ways to have one, the `wrapper_mode` setting:
//! - **managed** (the default): Encore builds wrapper-v2's image from its source at [`SOURCE_COMMIT`] and the
//!   owner's own Apple Music for Android app ([`stage`]), and runs it as a container next to its own through
//!   the Docker socket ([`sidecar`]). This needs the socket mounted into Encore's container, as "Update now"
//!   does; nothing runs inside Encore's container, which keeps no extra privileges;
//! - **external**: a wrapper-v2 the owner runs elsewhere, at `wrapper_url`.
//!
//! Either way the card signs the wrapper in to Apple Music with an Apple ID and the two-factor code Apple sends
//! ([`account`]; the password goes to the wrapper only, Encore keeps neither), shows its state, tests it, and
//! points gamdl at it. An Apple Music subscription is needed for anything to download.
//!
//! The long setup (download the source, take the libraries out of the app, build, start) runs as one job in
//! the background ([`Jobs`]); the card asks how it is going.
pub mod account;
pub mod sidecar;
pub mod stage;

use crate::Ctx;
use account::{Account, SignIn, State};
use encore_plugins::{Code, Failure, Status};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The plugin's id.
pub const ID: &str = "wrapper";
/// wrapper-v2's repository.
pub const SOURCE_REPO: &str = "glomatico/wrapper-v2";
/// The commit of wrapper-v2 this Encore builds: its API is the 0.0.2 gamdl 3.9 checks for, and its
/// `LIBS_VERSION.json` pins Apple Music for Android 3.6.0-beta (build 1109).
pub const SOURCE_COMMIT: &str = "100e0a864e883e03a3ac450a780dd9563fff5271";
/// The largest Apple Music upload taken (an `.apkm` bundle is a few hundred megabytes).
pub const BUNDLE_MAX_BYTES: u64 = 2 << 30;
/// The largest source download taken (the source carries Android's system libraries: about 100 MB).
const SOURCE_MAX_BYTES: u64 = 1 << 30;

/// The image Encore builds: `encore-wrapper-v2:<the commit's first 12 characters>`.
pub fn built_image() -> String {
    format!("encore-wrapper-v2:{}", &SOURCE_COMMIT[..12])
}

/// Where the source is downloaded from: GitHub's tarball of [`SOURCE_COMMIT`].
pub fn source_url() -> String {
    format!("https://codeload.github.com/{SOURCE_REPO}/tar.gz/{SOURCE_COMMIT}")
}

/// Which wrapper the plugin looks after (`wrapper_mode`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// built and run by Encore through the Docker socket
    Managed,
    /// the owner's own, at `wrapper_url`
    External,
}

/// The `wrapper_mode` setting.
pub fn mode(st: &impl encore_http::Ctx) -> Mode {
    if st.cfg().str("wrapper_mode") == "external" { Mode::External } else { Mode::Managed }
}

/// The image the managed wrapper runs: `wrapper_image` when the owner named one (pulled, never built),
/// else the one Encore builds.
pub fn image(st: &impl encore_http::Ctx) -> String {
    let named = st.cfg().str("wrapper_image").trim().to_string();
    if named.is_empty() { built_image() } else { named }
}

/// Where the wrapper answers, when Encore knows: the external one's address, or where the managed one was
/// last started (`wrapper_address`, written by [`start`]).
pub fn url(st: &impl encore_http::Ctx) -> Option<String> {
    let key = match mode(st) {
        Mode::External => "wrapper_url",
        Mode::Managed => "wrapper_address",
    };
    Some(st.cfg().str(key).trim().trim_end_matches('/').to_string()).filter(|u| !u.is_empty())
}

/// The decrypt socket of the managed wrapper (`host:port`), as [`start`] found it; empty for an external one
/// (gamdl then takes the URL's host and port 10020).
fn decrypt(st: &impl encore_http::Ctx) -> String {
    match mode(st) {
        Mode::Managed => st.cfg().str("wrapper_decrypt_address").trim().to_string(),
        Mode::External => String::new(),
    }
}

// ---- the setup job ---------------------------------------------------------------------------------------

/// The setup job running or last run, for the card.
#[derive(Clone, Debug, Default)]
pub struct Job {
    /// a job runs now
    pub running: bool,
    /// what it does now (or did last), in words
    pub step: String,
    /// why it stopped, when it failed
    pub error: Option<String>,
    /// when it ended (seconds since 1970)
    pub ended_at: Option<f64>,
}

/// The setup job, shared by the server's clones.
#[derive(Clone, Default)]
pub struct Jobs(Arc<Mutex<Job>>);

impl Jobs {
    /// The job as it stands.
    pub fn get(&self) -> Job {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Start a job: false when one runs already.
    fn begin(&self, step: &str) -> bool {
        let mut j = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if j.running {
            return false;
        }
        *j = Job { running: true, step: step.into(), error: None, ended_at: None };
        true
    }

    /// What the job does now.
    fn step(&self, step: impl Into<String>) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).step = step.into();
    }

    /// The job ended (with its error, when it failed).
    fn end(&self, error: Option<String>) {
        let mut j = self.0.lock().unwrap_or_else(|e| e.into_inner());
        j.running = false;
        j.error = error;
        j.ended_at = Some(encore_core::time::now());
    }
}

/// The plugin's data folder (made when missing): the uploaded app, the source, the staged build.
pub fn data(st: &impl Ctx) -> PathBuf {
    crate::sources::pytool::data(st, ID)
}

/// The uploaded Apple Music app.
pub fn bundle_path(st: &impl Ctx) -> PathBuf {
    data(st).join("apple-music.bundle")
}

/// What the card says about the uploaded app: its name, size and kind (`bundle.json`, written at upload).
pub fn bundle_info(st: &impl Ctx) -> Value {
    if !bundle_path(st).is_file() {
        return Value::Null;
    }
    std::fs::read(data(st).join("bundle.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_else(|| json!({ "name": "apple-music" }))
}

/// Keep an uploaded app: `tmp` (written by the upload) becomes the bundle when it is an APK or an APKM
/// bundle; `name` is the file's name as uploaded. Refused (and `tmp` removed) otherwise.
pub fn keep_bundle(st: &impl Ctx, tmp: &std::path::Path, name: &str) -> Result<Value, String> {
    let Some(kind) = stage::bundle_kind(tmp) else {
        let _ = std::fs::remove_file(tmp);
        return Err("That file is not an Android app: upload Apple Music for Android as an .apk or an .apkm bundle".into());
    };
    let size = std::fs::metadata(tmp).map(|m| m.len()).unwrap_or(0);
    std::fs::rename(tmp, bundle_path(st)).map_err(|e| format!("The app could not be kept: {e}"))?;
    // the name is the owner's text: kept short and on one line
    let name: String = name.chars().filter(|c| !c.is_control()).take(120).collect();
    let info = json!({ "name": name, "size": size, "kind": kind, "uploaded_at": encore_core::time::now() });
    let _ = std::fs::write(data(st).join("bundle.json"), info.to_string());
    st.plugins().log(ID).line("upload", &format!("Apple Music app kept: {name} ({kind}, {} MB)", size >> 20));
    Ok(info)
}

/// Start the setup in the background: download the source, take the libraries out of the uploaded app,
/// build the image (or pull the one named in `wrapper_image`), then start the container and point gamdl at
/// it. `rebuild` builds again even when the image is there. False when a job runs already.
pub fn set_up(st: &(impl Ctx + Clone + Send + Sync + 'static), rebuild: bool) -> bool {
    let jobs = st.requests().wrapper.clone();
    if !jobs.begin("Checking the Docker socket") {
        return false;
    }
    let st = st.clone();
    tokio::spawn(async move {
        let r = set_up_now(&st, &jobs, rebuild).await;
        let log = st.plugins().log(ID);
        match &r {
            Ok(a) => log.line("setup", &format!("the wrapper runs at {}", a.url)),
            Err(e) => log.line("setup", &format!("failed: {e}")),
        }
        jobs.end(r.err());
    });
    true
}

/// The setup itself (see [`set_up`]).
async fn set_up_now(st: &impl Ctx, jobs: &Jobs, rebuild: bool) -> Result<sidecar::Address, String> {
    let d = sidecar::probe(&st.system().docker_socket).await?;
    let image = image(st);
    let log = st.plugins().log(ID);
    let have = d.image_exists(&image).await.map_err(|e| e.to_string())?;
    if !st.cfg().str("wrapper_image").trim().is_empty() {
        if !have || rebuild {
            jobs.step(format!("Pulling {image}"));
            sidecar::pull(&d, &image).await?;
        }
    } else if !have || rebuild {
        build_image(st, &d, jobs, &image).await?;
    } else {
        log.line("setup", &format!("{image} is built already"));
    }
    jobs.step("Starting the wrapper");
    start_with(st, &d).await
}

/// Download, stage and build [`built_image`].
async fn build_image(st: &impl Ctx, d: &encore_system::docker::Docker, jobs: &Jobs, image: &str) -> Result<(), String> {
    let arch = stage::Arch::here().ok_or_else(|| format!("wrapper-v2 builds for x86_64 and arm64 machines, not {}", std::env::consts::ARCH))?;
    let bundle = bundle_path(st);
    if !bundle.is_file() {
        return Err("Upload Apple Music for Android first (an .apk or .apkm of version 3.6.0-beta, build 1109)".into());
    }
    let log = st.plugins().log(ID);
    let dir = data(st);
    // the source, fresh each time: staging writes into it
    jobs.step("Downloading wrapper-v2's source");
    let tarball = dir.join("source.tar.gz");
    encore_plugins::fetch::to_file(st.http(), &source_url(), &tarball, SOURCE_MAX_BYTES, Duration::from_secs(900), false).await.map_err(|e| e.to_string())?;
    let build = dir.join("build");
    let staged = {
        let (tarball, build, bundle) = (tarball.clone(), build.clone(), bundle.clone());
        jobs.step("Taking the libraries out of the Apple Music app");
        tokio::task::spawn_blocking(move || -> anyhow::Result<(PathBuf, stage::Prepared)> {
            let _ = std::fs::remove_dir_all(&build);
            encore_plugins::archive::unpack(&tarball, encore_plugins::archive::Kind::TarGz, &build, Default::default())?;
            let _ = std::fs::remove_file(&tarball);
            let src = stage::top_folder(&build);
            let prepared = stage::prepare(&src, &bundle, arch)?;
            let context = build.join("context.tar");
            stage::write_tar(&src, &context)?;
            Ok((context, prepared))
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?
    };
    let (context, prepared) = staged;
    log.line("setup", &format!("{} Apple libraries from {} ({}) and {} Android system files staged for {}", prepared.libs.libs, prepared.libs.from, prepared.libs.dir, prepared.system, arch.name()));
    jobs.step("Building the image (the first build takes 10 to 30 minutes)");
    let mut steps = 0usize;
    let r = sidecar::build(d, &context, image, arch, |line| {
        if let Some(text) = encore_system::docker::build_text(line) {
            // "Step 7/24 : RUN …" is the progress the card shows; every line goes to the log
            if text.starts_with("Step ") {
                steps += 1;
                jobs.step(format!("Building the image: {}", text.chars().take(120).collect::<String>()));
            }
            log.line("build", &text);
        }
    })
    .await;
    let _ = std::fs::remove_file(&context);
    r?;
    log.line("setup", &format!("{image} built in {steps} steps"));
    Ok(())
}

/// Start the managed wrapper's container (made when missing) and remember where it answers; gamdl is
/// pointed at it when `wrapper_fill_gamdl` is on.
pub async fn start(st: &impl Ctx) -> Result<sidecar::Address, String> {
    let d = sidecar::probe(&st.system().docker_socket).await?;
    if !d.image_exists(&image(st)).await.map_err(|e| e.to_string())? {
        return Err("The wrapper's image is not built yet: press Set up".into());
    }
    start_with(st, &d).await
}

async fn start_with(st: &impl Ctx, d: &encore_system::docker::Docker) -> Result<sidecar::Address, String> {
    let a = sidecar::run(d, &image(st)).await?;
    st.cfg().set_many(&json!({ "wrapper_address": a.url, "wrapper_decrypt_address": a.decrypt }));
    st.plugins().log(ID).line("start", &format!("the wrapper runs at {} (decrypt {})", a.url, a.decrypt));
    if st.cfg().bool("wrapper_fill_gamdl") {
        let _ = fill_gamdl(st);
    }
    Ok(a)
}

/// Stop the managed wrapper's container.
pub async fn stop(st: &impl Ctx) -> Result<(), String> {
    let d = sidecar::probe(&st.system().docker_socket).await?;
    sidecar::stop(&d).await?;
    st.plugins().log(ID).line("stop", "the wrapper is stopped");
    Ok(())
}

/// The plugin is being removed: its container goes (the image and the session's volume stay, for a new
/// install to start signed in).
pub async fn removing(st: &impl Ctx) {
    if mode(st) == Mode::Managed {
        if let Ok(d) = sidecar::probe(&st.system().docker_socket).await {
            if let Err(e) = sidecar::remove(&d).await {
                tracing::warn!("wrapper: {e}");
            }
        }
    }
}

/// The managed wrapper's last log lines (Docker's log of its container).
pub async fn container_logs(st: &impl Ctx, lines: usize) -> Result<String, String> {
    let d = sidecar::probe(&st.system().docker_socket).await?;
    match sidecar::container(&d).await? {
        Some(_) => d.logs(sidecar::CONTAINER, lines.clamp(1, 2000)).await.map_err(|e| e.to_string()),
        None => Ok(String::new()),
    }
}

// ---- gamdl ----------------------------------------------------------------------------------------------

/// Whether gamdl's "Wrapper URL" and decrypt address are this wrapper's. An empty decrypt address on either
/// side means what gamdl reads it as: the Wrapper URL's host, port 10020 (a URL typed in by hand and no
/// decrypt address is this wrapper when that is where its decrypt socket is).
pub fn gamdl_points_here(st: &impl encore_http::Ctx) -> bool {
    let Some(u) = url(st) else { return false };
    let gamdl_url = st.cfg().str("gamdl_wrapper_url");
    let gamdl_url = gamdl_url.trim().trim_end_matches('/');
    let effective = |d: String| if d.trim().is_empty() { default_decrypt(&u) } else { d.trim().to_string() };
    gamdl_url == u && effective(st.cfg().str("gamdl_wrapper_decrypt")) == effective(decrypt(st))
}

/// The decrypt address gamdl takes when none is set: the URL's host, port 10020.
fn default_decrypt(url: &str) -> String {
    let host = reqwest::Url::parse(url).ok().and_then(|u| u.host_str().map(String::from)).unwrap_or_default();
    format!("{host}:10020")
}

/// The plugin's name, as the settings page credits it.
pub const NAME: &str = "Apple Music wrapper";

/// Credit gamdl's wrapper fields to this plugin in a settings dump (`Config::public_dump`): their `_meta`
/// source becomes `plugin` with the plugin's name, so the gamdl card shows the values this wrapper filled in
/// read-only, and its Save leaves them alone. A field the environment sets stays the environment's.
pub fn credit_gamdl_fields(dump: &mut Value) {
    for key in ["gamdl_wrapper_url", "gamdl_wrapper_decrypt"] {
        let meta = &mut dump["_meta"][key];
        if meta["source"] == "env" { continue; }
        meta["source"] = json!("plugin");
        meta["plugin"] = json!(NAME);
    }
}

/// Point gamdl at this wrapper: its "Wrapper URL" and decrypt address. Refused when there is no address yet
/// or the environment sets gamdl's (GAMDL_WRAPPER_URL wins over saved settings).
pub fn fill_gamdl(st: &impl encore_http::Ctx) -> Result<String, String> {
    let u = url(st).ok_or_else(|| match mode(st) {
        Mode::External => "Enter your wrapper's address first".to_string(),
        Mode::Managed => "Start the wrapper first".to_string(),
    })?;
    if st.cfg().env_locked("gamdl_wrapper_url") {
        return Err("GAMDL_WRAPPER_URL is set in Encore's environment, which wins: change it there".into());
    }
    let mut values = json!({ "gamdl_wrapper_url": u });
    if !st.cfg().env_locked("gamdl_wrapper_decrypt") {
        values["gamdl_wrapper_decrypt"] = json!(decrypt(st));
    }
    st.cfg().set_many(&values);
    Ok(format!("gamdl uses the wrapper at {u}"))
}

// ---- sign-in --------------------------------------------------------------------------------------------

/// The wrapper's address, or why there is none, as a plugin failure.
fn need_url(st: &impl encore_http::Ctx) -> Result<String, Failure> {
    url(st).ok_or_else(|| match mode(st) {
        Mode::External => Failure::new(Code::Auth, "Enter your wrapper's address (\"Wrapper address\") first"),
        Mode::Managed => Failure::new(Code::NotFound, "The wrapper is not set up yet: upload the Apple Music app and press Set up"),
    })
}

/// A sign-in step's answer for the card: `{state, signed_in, apple_id, …, message}`.
fn sign_in_json(st: &impl encore_http::Ctx, r: SignIn) -> Value {
    match r {
        SignIn::Done(a) => {
            let mut v = a.json();
            let filled = if st.cfg().bool("wrapper_fill_gamdl") { fill_gamdl(st).ok() } else { None };
            v["message"] = json!(match filled {
                Some(f) => format!("{} · {f}", a.line()),
                None => a.line(),
            });
            v
        }
        SignIn::NeedsCode(a) => {
            let mut v = a.json();
            v["state"] = json!(State::Awaiting2fa.word());
            v["message"] = json!("Apple sent a code to your iPhone, iPad or Mac: enter it here");
            v
        }
        SignIn::Refused(why) => json!({ "state": "failed", "signed_in": false, "error": why, "message": why }),
    }
}

/// Sign the wrapper in with an Apple ID and its password (the first step).
pub async fn sign_in(st: &impl encore_http::Ctx, apple_id: &str, password: &str) -> Result<Value, Failure> {
    if apple_id.trim().is_empty() || password.is_empty() {
        return Err(Failure::new(Code::Auth, "Enter your Apple ID and its password"));
    }
    let u = need_url(st)?;
    let r = account::login(st.http(), &u, apple_id, password).await.map_err(|e| Failure::new(Code::Network, e))?;
    Ok(sign_in_json(st, r))
}

/// The two-factor code Apple sent (the second step).
pub async fn sign_in_code(st: &impl encore_http::Ctx, code: &str) -> Result<Value, Failure> {
    let u = need_url(st)?;
    let r = account::code(st.http(), &u, code).await.map_err(|e| Failure::new(Code::Network, e))?;
    Ok(sign_in_json(st, r))
}

/// Sign the wrapper out (or stop a sign-in half way).
pub async fn sign_out(st: &impl encore_http::Ctx) -> Result<(), Failure> {
    let u = need_url(st)?;
    account::logout(st.http(), &u).await.map_err(|e| Failure::new(Code::Network, e))
}

// ---- the card ---------------------------------------------------------------------------------------------

/// Everything the card shows: the mode, the Docker socket, the uploaded app, the image, the container, the
/// setup job, the account (never a token) and whether gamdl points here.
pub async fn overview(st: &impl Ctx) -> Value {
    let job = st.requests().wrapper.get();
    let m = mode(st);
    let mut v = json!({
        "mode": if m == Mode::External { "external" } else { "managed" },
        "url": url(st),
        "image": image(st),
        "builds": st.cfg().str("wrapper_image").trim().is_empty(),
        "arch": stage::Arch::here().map(|a| a.name()),
        "source": { "repo": SOURCE_REPO, "commit": SOURCE_COMMIT },
        "job": { "running": job.running, "step": job.step, "error": job.error, "ended_at": job.ended_at },
        "gamdl": { "url": st.cfg().str("gamdl_wrapper_url"), "decrypt": st.cfg().str("gamdl_wrapper_decrypt"), "points_here": gamdl_points_here(st), "locked": st.cfg().env_locked("gamdl_wrapper_url") },
    });
    if m == Mode::Managed {
        v["bundle"] = bundle_info(st);
        match sidecar::probe(&st.system().docker_socket).await {
            Ok(d) => {
                v["docker"] = json!({ "ok": true });
                v["image_built"] = json!(d.image_exists(&image(st)).await.unwrap_or(false));
                let c = sidecar::container(&d).await.ok().flatten();
                v["container"] = json!(sidecar::state_word(c.as_ref()));
            }
            Err(e) => v["docker"] = json!({ "ok": false, "reason": e }),
        }
    }
    let running = m == Mode::External || v["container"] == "running";
    if let (true, Some(u)) = (running, url(st)) {
        v["account"] = match account::me(st.http(), &u).await {
            Ok(a) => a.json(),
            Err(e) => json!({ "state": "unreachable", "signed_in": false, "error": e }),
        };
    }
    v
}

/// The card's status line.
pub async fn status(st: &impl Ctx) -> Result<Status, Failure> {
    let job = st.requests().wrapper.get();
    if job.running {
        return Ok(Status { message: format!("{}…", job.step), signed_in: None });
    }
    if mode(st) == Mode::Managed {
        let d = match sidecar::probe(&st.system().docker_socket).await {
            Ok(d) => d,
            Err(e) => return Ok(Status { message: e, signed_in: Some(false) }),
        };
        let c = sidecar::container(&d).await.map_err(Failure::internal)?;
        match sidecar::state_word(c.as_ref()).as_str() {
            "running" => {}
            "missing" if !bundle_path(st).is_file() => return Ok(Status { message: "Upload Apple Music for Android, then press Set up".into(), signed_in: Some(false) }),
            "missing" => {
                let msg = match &job.error {
                    Some(e) => format!("The last setup failed: {e}"),
                    None => "The app is uploaded: press Set up to build and start the wrapper".into(),
                };
                return Ok(Status { message: msg, signed_in: Some(false) });
            }
            other => return Ok(Status { message: format!("The wrapper's container is {other}: press Start"), signed_in: Some(false) }),
        }
    }
    let u = need_url(st)?;
    match account::me(st.http(), &u).await {
        Ok(a) => {
            let gamdl = if a.state == State::Authenticated && !gamdl_points_here(st) { " · gamdl does not use it yet (Use with gamdl)" } else { "" };
            Ok(Status { message: format!("{}{gamdl}", a.line()), signed_in: Some(a.state == State::Authenticated) })
        }
        Err(e) => Err(Failure::new(Code::Network, e)),
    }
}

/// The card's Test: the wrapper answers, speaks the API gamdl needs, its Apple libraries loaded, and it is
/// signed in.
pub async fn test(st: &impl Ctx) -> Result<String, Failure> {
    let u = need_url(st)?;
    let ver = account::health(st.http(), &u).await.map_err(|e| Failure::new(Code::Network, e))?;
    let a = account::me(st.http(), &u).await.map_err(|e| Failure::new(Code::Network, e))?;
    test_line(&a, &ver, gamdl_points_here(st))
}

/// [`test`]'s verdict on what the wrapper answered.
pub fn test_line(a: &Account, health_version: &str, gamdl_here: bool) -> Result<String, Failure> {
    let ver = a.version.as_deref().unwrap_or(health_version);
    if ver != account::API_VERSION {
        return Err(Failure::unsupported(format!("This wrapper speaks API {ver}; gamdl needs wrapper-v2 {}", account::API_VERSION)));
    }
    if a.loader_ok == Some(false) {
        return Err(Failure::internal("The wrapper runs, but Apple's libraries did not load: the image was built from another Apple Music version, or the container lacks its privileges (see the wrapper's log)"));
    }
    if a.state != State::Authenticated {
        return Err(Failure::new(Code::Auth, a.line()));
    }
    let gamdl = if gamdl_here { "gamdl uses it" } else { "gamdl does not use it yet: press Use with gamdl" };
    Ok(format!("{} · wrapper-v2 {ver} · {gamdl}", a.line()))
}

#[cfg(test)]
mod tests;
