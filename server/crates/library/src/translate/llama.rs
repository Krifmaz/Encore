//! The Translator plugin's program: llama.cpp's `llama-server` plus one GGUF model from Hugging Face. The
//! plugin installs both (encore_plugins: the program into `<config>/plugins/translator/<build>/`, the model the
//! translator_model setting picks into its data folder); an Encore before plugins kept them under
//! `<config>/tools/translator`, which the plugin adopts in place. The server is started when there is
//! something to translate (and the plugin is switched on), listens on 127.0.0.1 only, runs at low priority
//! through the plugins' spawn builder, and is stopped after two minutes without work.
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use crate::Ctx;
use std::time::{Duration, Instant};

/// Stop the model this long after the last request.
pub const IDLE: Duration = Duration::from_secs(120);
/// How long a model may take to load before the start counts as failed.
const LOAD_LIMIT: Duration = Duration::from_secs(300);

/// A model the settings offer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Model {
    pub id: &'static str,
    pub label: &'static str,
    pub repo: &'static str,
    pub file: &'static str,
    /// rough download size, for the settings text
    pub gb: f32,
    /// a hybrid Qwen3 model that thinks unless told `/no_think`
    pub thinking: bool,
}

/// Small: Qwen3-4B-Instruct-2507 (non-thinking instruct model), Q4_K_M. Best: Qwen3-8B (Qwen
/// publishes no 8B "Instruct-2507"; the 8B is the hybrid model, run with thinking off), Q4_K_M.
pub const MODELS: [Model; 2] = [
    Model { id: "small", label: "Small – Qwen3 4B Instruct (about 2.5 GB)", repo: "unsloth/Qwen3-4B-Instruct-2507-GGUF", file: "Qwen3-4B-Instruct-2507-Q4_K_M.gguf", gb: 2.5, thinking: false },
    Model { id: "best", label: "Best – Qwen3 8B (about 5 GB)", repo: "Qwen/Qwen3-8B-GGUF", file: "Qwen3-8B-Q4_K_M.gguf", gb: 5.0, thinking: true },
];

/// The model chosen on the Translator's card (translator_model).
pub fn model(st: &impl Ctx) -> Model {
    model_by_id(&st.cfg().str("translator_model"))
}

pub fn model_by_id(id: &str) -> Model {
    MODELS.iter().copied().find(|m| m.id == id).unwrap_or(MODELS[0])
}

pub fn model_url(m: &Model) -> String {
    format!("https://huggingface.co/{}/resolve/main/{}", m.repo, m.file)
}

// ---- files --------------------------------------------------------------------------------------

/// `<config>/tools/translator`: where an Encore before plugins kept the program and the models (adopted).
pub fn dir(data: &Path) -> PathBuf {
    data.join("tools").join("translator")
}

/// The plugin's own models folder (`[[download]] path = "models/…"` in its data folder).
fn plugin_models(st: &impl Ctx) -> PathBuf {
    st.plugins().folders("translator").data().join("models")
}

/// Where the chosen model is: the plugin's own copy, else the adopted one; a model that is in neither goes into
/// the plugin's folder.
pub fn model_path(st: &impl Ctx, m: &Model) -> PathBuf {
    let own = plugin_models(st).join(m.file);
    let adopted = dir(st.data_dir()).join("models").join(m.file);
    if !own.is_file() && adopted.is_file() { adopted } else { own }
}

fn server_root(data: &Path) -> PathBuf {
    dir(data).join("llama")
}

fn current(data: &Path) -> Option<String> {
    let v = std::fs::read_to_string(server_root(data).join("current")).ok()?;
    let v = v.trim();
    (!v.is_empty() && !v.contains('/') && !v.starts_with('.')).then(|| v.to_string())
}

fn find_server(dir: &Path) -> Option<PathBuf> {
    walkdir::WalkDir::new(dir).max_depth(4).into_iter().flatten()
        .find(|e| e.file_type().is_file() && e.file_name() == "llama-server")
        .map(|e| e.into_path())
}

/// (llama-server, build) while the Translator plugin is installed and switched on: its own install, else the
/// one an Encore before plugins installed (adopted). The build reads as llama.cpp tags it ("b6800").
pub fn installed(st: &impl Ctx) -> Option<(PathBuf, String)> {
    if !st.plugins().enabled("translator") {
        return None;
    }
    if let Some((v, bin)) = st.plugins().program("translator") {
        let tag = if v.starts_with(['b', 'B']) { v } else { format!("b{v}") };
        return Some((bin, tag));
    }
    let tag = current(st.data_dir())?;
    Some((find_server(&server_root(st.data_dir()).join(&tag))?, tag))
}

/// Whether the translator can run now: the server and the chosen model are there.
pub fn ready(st: &impl Ctx) -> bool {
    installed(st).is_some() && model_path(st, &model(st)).is_file()
}

/// llama.cpp tags are build numbers ("b6800"); newer = higher number.
pub fn newer_build(latest: &str, installed: &str) -> bool {
    let n = |s: &str| s.trim().trim_start_matches(['b', 'B']).parse::<u64>().ok();
    match (n(latest), n(installed)) {
        (Some(a), Some(b)) => a > b,
        _ => latest != installed && encore_core::version::newer(latest, installed),
    }
}

// ---- the model ----------------------------------------------------------------------------------

/// The SHA-256 Hugging Face lists for a model file (its LFS oid), when the API answers.
async fn model_sha256(st: &impl Ctx, m: &Model) -> Option<String> {
    let v: Value = st.http().get(format!("https://huggingface.co/api/models/{}/tree/main", m.repo)).timeout(Duration::from_secs(20)).send().await.ok()?.json().await.ok()?;
    v.as_array()?.iter().find(|f| f["path"].as_str() == Some(m.file))?["lfs"]["oid"].as_str().map(|s| s.to_ascii_lowercase())
}

/// A GGUF file starts with the bytes "GGUF".
pub fn is_gguf(path: &Path) -> bool {
    use std::io::Read;
    let mut magic = [0u8; 4];
    std::fs::File::open(path).and_then(|mut f| f.read_exact(&mut magic)).is_ok() && &magic == b"GGUF"
}

/// Download the chosen model when it is missing (a model chosen after the plugin was installed), checked against
/// Hugging Face's SHA-256 when the API answers and the GGUF header always, through the one installer; the other
/// models in the plugin's folder are removed to free the disk.
pub async fn ensure_model(st: &impl Ctx) -> anyhow::Result<()> {
    let m = model(st);
    let path = model_path(st, &m);
    if path.is_file() && is_gguf(&path) {
        return Ok(());
    }
    let path = plugin_models(st).join(m.file);
    st.db().log("info", &format!("Translator: downloading the {} model ({:.1} GB)", m.id, m.gb));
    let want = model_sha256(st, &m).await;
    let net = encore_plugins::install::Net { http: st.http().clone(), allow_http: false };
    encore_plugins::install::data_file(&net, &model_url(&m), &path, want.as_deref(), Some("GGUF"), 12 << 30)
        .await
        .map_err(|e| anyhow::anyhow!("the model download failed: {e:#} – try again"))?;
    if let Ok(rd) = std::fs::read_dir(plugin_models(st)) {
        for e in rd.flatten() {
            if e.file_name().to_string_lossy() != m.file {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    super::wake(st);
    Ok(())
}

// ---- running ------------------------------------------------------------------------------------

/// llama-server's start through the plugins' spawn builder: Encore's environment (without its settings) with the shared libraries
/// next to the program, a session of its own, low priority (nice 10, idle disk) – it is background work.
fn command(exe: &Path) -> tokio::process::Command {
    let mut env: Vec<(String, String)> = encore_plugins::sandbox::program_env().into_iter().filter(|(k, _)| k != "LD_LIBRARY_PATH").collect();
    if let Some(d) = exe.parent() {
        let old = std::env::var("LD_LIBRARY_PATH").unwrap_or_default();
        env.push(("LD_LIBRARY_PATH".into(), if old.is_empty() { d.display().to_string() } else { format!("{}:{old}", d.display()) }));
    }
    let spawn = encore_plugins::sandbox::Spawn::new(&encore_plugins::sandbox::Sandbox::limits_only(), exe).env(env).low_priority(true);
    let mut c = spawn.tokio_command();
    c.stdin(std::process::Stdio::null());
    c
}

/// The model server while it runs.
pub(crate) struct Running {
    child: tokio::process::Child,
    port: u16,
    model: PathBuf,
    last: Instant,
}

/// A client that never goes through a proxy (the server is on 127.0.0.1).
fn local(st: &impl Ctx) -> &reqwest::Client {
    st.library().translator.0.local.get_or_init(|| encore_http::client::local().unwrap_or_default())
}

fn free_port() -> anyhow::Result<u16> {
    Ok(std::net::TcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
}

/// Whether the model server is running (for the status line).
pub fn running(st: &impl Ctx) -> bool {
    st.library().translator.0.server.try_lock().map(|g| g.is_some()).unwrap_or(true)
}

/// Stop the model server (idle, reset, update).
pub async fn stop(st: &impl Ctx) {
    if let Some(mut r) = st.library().translator.0.server.lock().await.take() {
        let _ = r.child.kill().await;
    }
}

async fn start(st: &impl Ctx) -> anyhow::Result<Running> {
    let (exe, _) = installed(st).ok_or_else(|| anyhow::anyhow!("The Translator is not installed (Settings → Plugins → Translator)"))?;
    let m = model(st);
    let model = model_path(st, &m);
    if !model.is_file() {
        anyhow::bail!("The {} model is not downloaded (Settings → Plugins → Translator)", m.id);
    }
    let port = free_port()?;
    let threads = std::thread::available_parallelism().map(|n| (n.get() / 2).max(1)).unwrap_or(2);
    let mut cmd = command(&exe);
    cmd.arg("-m").arg(&model).args(["--host", "127.0.0.1", "--port", &port.to_string(), "-c", "4096", "-t", &threads.to_string(), "-np", "1"])
        .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    // background work: below everything else on the box (nice 15, on top of the builder's low priority)
    unsafe {
        cmd.pre_exec(|| {
            libc::setpriority(libc::PRIO_PROCESS, 0, 15);
            encore_jobs::busy::idle_io_for_child();
            Ok(())
        });
    }
    let mut child = encore_core::fsx::spawn(encore_core::fsx::Step::Start, &exe, || cmd.spawn()).map_err(|e| anyhow::anyhow!("could not start llama-server: {e}"))?;
    let until = Instant::now() + LOAD_LIMIT;
    loop {
        if let Ok(Some(s)) = child.try_wait() {
            anyhow::bail!("llama-server stopped while loading the model ({s}) – is there enough memory for the {} model?", m.id);
        }
        let ok = local(st).get(format!("http://127.0.0.1:{port}/health")).timeout(Duration::from_secs(5)).send().await.is_ok_and(|r| r.status().is_success());
        if ok {
            break;
        }
        if Instant::now() > until {
            let _ = child.kill().await;
            anyhow::bail!("llama-server did not finish loading the model within {} s", LOAD_LIMIT.as_secs());
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    tracing::info!("translator: llama-server started on port {port} with {}", m.file);
    Ok(Running { child, port, model, last: Instant::now() })
}

/// Ask the model; starts the server when it is not running (or runs another model).
pub async fn chat(st: &impl Ctx, messages: Value, thinking: bool) -> anyhow::Result<String> {
    let mut g = st.library().translator.0.server.lock().await;
    let want = model_path(st, &model(st));
    let alive = match g.as_mut() {
        Some(r) => r.model == want && matches!(r.child.try_wait(), Ok(None)),
        None => false,
    };
    if !alive {
        if let Some(mut old) = g.take() {
            let _ = old.child.kill().await;
        }
        *g = Some(start(st).await?);
        ensure_reaper(st);
    }
    let r = g.as_mut().expect("started above");
    r.last = Instant::now();
    let body = json!({
        "messages": messages, "temperature": 0.2, "top_p": 0.9, "max_tokens": if thinking { 3072 } else { 2048 },
        "response_format": { "type": "json_object" },
        // Qwen3 hybrid models: answer without thinking (ignored by templates that don't know it)
        "chat_template_kwargs": { "enable_thinking": false },
    });
    let resp = local(st).post(format!("http://127.0.0.1:{}/v1/chat/completions", r.port)).json(&body).timeout(Duration::from_secs(600)).send().await;
    r.last = Instant::now();
    let resp = resp.map_err(|e| anyhow::anyhow!("the model did not answer: {e}"))?;
    let code = resp.status().as_u16();
    let v: Value = resp.json().await.unwrap_or(Value::Null);
    if code >= 400 {
        anyhow::bail!("the model answered {code}: {}", v["error"]["message"].as_str().unwrap_or(""));
    }
    Ok(v["choices"][0]["message"]["content"].as_str().unwrap_or("").to_string())
}

/// Stops the server after [`IDLE`] without requests.
fn ensure_reaper(st: &impl Ctx) {
    let tr = st.library().translator.clone();
    if tr.0.reaper.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(15)).await;
            let mut g = tr.0.server.lock().await;
            if g.as_ref().is_some_and(|r| r.last.elapsed() > IDLE) {
                if let Some(mut r) = g.take() {
                    let _ = r.child.kill().await;
                    tracing::info!("translator: llama-server stopped after {} s idle", IDLE.as_secs());
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_numbers_compare() {
        assert!(newer_build("b11669", "b11662"));
        assert!(!newer_build("b11662", "b11669"));
        assert!(!newer_build("b11669", "b11669"));
        assert!(newer_build("b10000", "b9999"), "numbers, not text");
    }

    #[test]
    fn models_and_urls() {
        assert_eq!(model_by_id("best").file, "Qwen3-8B-Q4_K_M.gguf");
        assert_eq!(model_by_id("nope").id, "small");
        assert_eq!(model_url(&MODELS[0]), "https://huggingface.co/unsloth/Qwen3-4B-Instruct-2507-GGUF/resolve/main/Qwen3-4B-Instruct-2507-Q4_K_M.gguf");
        assert!(!MODELS[0].thinking && MODELS[1].thinking);
    }

    /// A .tar.gz made by the system's tar, with a program, a library and a link to it, unpacked by the one
    /// installer: the program is found, the link resolves inside.
    #[test]
    fn unpacks_files_and_links() {
        let root = std::env::temp_dir().join(format!("encore-llama-tar-{}-{}", std::process::id(), encore_core::ids::random_hex(4)));
        let src = root.join("src/build/bin");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("llama-server"), b"#!/bin/sh\necho version: 1\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(src.join("llama-server"), std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(src.join("libllama.so.0"), b"lib").unwrap();
        std::os::unix::fs::symlink("libllama.so.0", src.join("libllama.so")).unwrap();
        let made = std::process::Command::new("tar").arg("-czf").arg(root.join("a.tar.gz")).arg("-C").arg(root.join("src")).arg("build").status();
        if !made.is_ok_and(|s| s.success()) {
            return;
        }
        let out = root.join("out");
        encore_plugins::archive::unpack(&root.join("a.tar.gz"), encore_plugins::archive::Kind::TarGz, &out, Default::default()).unwrap();
        let exe = find_server(&out).unwrap();
        assert_eq!(exe, out.join("build/bin/llama-server"));
        assert_eq!(std::fs::metadata(&exe).unwrap().permissions().mode() & 0o777, 0o755);
        assert_eq!(std::fs::read(out.join("build/bin/libllama.so")).unwrap(), b"lib", "the link resolves inside");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn gguf_header() {
        let p = std::env::temp_dir().join(format!("encore-gguf-{}-{}", std::process::id(), encore_core::ids::random_hex(4)));
        std::fs::write(&p, b"GGUF\x03\x00").unwrap();
        assert!(is_gguf(&p));
        std::fs::write(&p, b"<html>").unwrap();
        assert!(!is_gguf(&p));
        let _ = std::fs::remove_file(&p);
    }
}
