//! The Translator tool: llama.cpp's `llama-server` from its GitHub releases plus one GGUF model from
//! Hugging Face, both under `<config>/tools/translator`. The server is started when there is
//! something to translate, listens on 127.0.0.1 only, runs at low priority, and is stopped after two
//! minutes without work.
use crate::state::AppState;
use crate::tools::ToolInfo;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

pub const LLAMA_REPO: &str = "ggml-org/llama.cpp";
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

/// The model chosen under Download tools (translator_model).
pub fn model(st: &AppState) -> Model {
    model_by_id(&st.cfg.str("translator_model"))
}

pub fn model_by_id(id: &str) -> Model {
    MODELS.iter().copied().find(|m| m.id == id).unwrap_or(MODELS[0])
}

pub fn model_url(m: &Model) -> String {
    format!("https://huggingface.co/{}/resolve/main/{}", m.repo, m.file)
}

// ---- files --------------------------------------------------------------------------------------

/// `<config>/tools/translator`
pub fn dir(data: &Path) -> PathBuf {
    data.join("tools").join("translator")
}

fn models_dir(data: &Path) -> PathBuf {
    dir(data).join("models")
}

pub fn model_path(data: &Path, m: &Model) -> PathBuf {
    models_dir(data).join(m.file)
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

/// (llama-server, build) when installed.
pub fn installed(data: &Path) -> Option<(PathBuf, String)> {
    let tag = current(data)?;
    Some((find_server(&server_root(data).join(&tag))?, tag))
}

/// Whether the translator can run now: the server and the chosen model are there.
pub fn ready(st: &AppState) -> bool {
    installed(st.data_dir()).is_some() && model_path(st.data_dir(), &model(st)).is_file()
}

// ---- releases -----------------------------------------------------------------------------------

/// The CPU build for this machine in a release's assets: `llama-<tag>-bin-ubuntu-x64.tar.gz` (or
/// .zip, arm64). GPU builds (vulkan, cuda, rocm, sycl, openvino) are skipped.
pub fn asset_for(release: &Value, arch: &str) -> Option<(String, String)> {
    let a = match arch {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        _ => return None,
    };
    let want = format!("-bin-ubuntu-{a}.");
    release["assets"].as_array()?.iter().find_map(|x| {
        let name = x["name"].as_str()?;
        let url = x["browser_download_url"].as_str()?;
        (name.starts_with("llama-") && name.contains(&want) && (name.ends_with(".tar.gz") || name.ends_with(".zip")))
            .then(|| (name.to_string(), url.to_string()))
    })
}

/// From GitHub's release list: the newest release with a CPU build for this machine
/// (tag, asset name, url). Drafts are skipped; llama.cpp marks its builds as pre-releases at times,
/// so those count.
pub fn pick_release(list: &Value, arch: &str) -> Option<(String, String, String)> {
    list.as_array()?.iter().filter(|r| !r["draft"].as_bool().unwrap_or(false)).find_map(|r| {
        let tag = r["tag_name"].as_str()?.to_string();
        let (name, url) = asset_for(r, arch)?;
        Some((tag, name, url))
    })
}

/// llama.cpp tags are build numbers ("b6800"); newer = higher number.
pub fn newer_build(latest: &str, installed: &str) -> bool {
    let n = |s: &str| s.trim().trim_start_matches(['b', 'B']).parse::<u64>().ok();
    match (n(latest), n(installed)) {
        (Some(a), Some(b)) => a > b,
        _ => latest != installed && crate::tools::newer(latest, installed),
    }
}

/// The newest llama.cpp build for this server: (tag, download url).
pub async fn latest(st: &AppState) -> anyhow::Result<(String, String)> {
    let r = st.http.get(format!("https://api.github.com/repos/{LLAMA_REPO}/releases?per_page=20"))
        .header("Accept", "application/vnd.github+json").timeout(Duration::from_secs(20)).send().await?;
    let code = r.status().as_u16();
    let v: Value = r.json().await.unwrap_or(Value::Null);
    if code >= 400 {
        anyhow::bail!("GitHub returned {code} for llama.cpp releases: {}", v["message"].as_str().unwrap_or(""));
    }
    let arch = std::env::consts::ARCH;
    pick_release(&v, arch).map(|(t, _, u)| (t, u)).ok_or_else(|| anyhow::anyhow!("llama.cpp has no recent CPU build for this server ({arch})"))
}

// ---- unpacking ----------------------------------------------------------------------------------

/// A path inside an archive that stays inside the folder it is unpacked to.
fn safe_rel(name: &str) -> Option<PathBuf> {
    let p = Path::new(name.trim_start_matches("./"));
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::Normal(x) => out.push(x),
            std::path::Component::CurDir => {}
            _ => return None,
        }
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

/// Unpack a .tar.gz into `dest`: files (with their mode), folders and symbolic links (which must point
/// inside the archive's folder: the shared libraries come as `libllama.so -> libllama.so.0`).
pub fn untar_gz_all(archive: &[u8], dest: &Path) -> anyhow::Result<()> {
    use std::io::Read;
    use std::os::unix::fs::PermissionsExt;
    let mut tar = Vec::new();
    flate2::read::GzDecoder::new(archive).take(2 << 30).read_to_end(&mut tar)?;
    let mut at = 0usize;
    let mut long_name: Option<String> = None;
    let mut long_link: Option<String> = None;
    while at + 512 <= tar.len() {
        let h = &tar[at..at + 512];
        if h.iter().all(|b| *b == 0) {
            break;
        }
        let field = |a: usize, b: usize| String::from_utf8_lossy(&h[a..b]).trim_end_matches('\0').to_string();
        let size_field = field(124, 136);
        let size = u64::from_str_radix(size_field.trim().trim_end_matches(' '), 8).map_err(|_| anyhow::anyhow!("broken archive (size {size_field:?})"))? as usize;
        let mode = u32::from_str_radix(field(100, 108).trim(), 8).unwrap_or(0o644);
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
        let mut link = field(157, 257);
        if let Some(l) = long_link.take() {
            link = l;
        }
        let start = at + 512;
        let end = start.checked_add(size).filter(|e| *e <= tar.len()).ok_or_else(|| anyhow::anyhow!("archive is cut off"))?;
        let data = &tar[start..end];
        match kind {
            b'L' => long_name = Some(String::from_utf8_lossy(data).trim_end_matches('\0').to_string()),
            b'K' => long_link = Some(String::from_utf8_lossy(data).trim_end_matches('\0').to_string()),
            b'x' | b'g' => {}
            b'5' => {
                if let Some(rel) = safe_rel(&name) {
                    std::fs::create_dir_all(dest.join(rel))?;
                }
            }
            b'0' | 0 | b'7' => {
                let rel = safe_rel(&name).ok_or_else(|| anyhow::anyhow!("odd path in the archive: {name}"))?;
                let p = dest.join(rel);
                if let Some(d) = p.parent() {
                    std::fs::create_dir_all(d)?;
                }
                std::fs::write(&p, data)?;
                std::fs::set_permissions(&p, std::fs::Permissions::from_mode(if mode & 0o111 != 0 { 0o755 } else { 0o644 }))?;
            }
            b'2' | b'1' => {
                let rel = safe_rel(&name).ok_or_else(|| anyhow::anyhow!("odd path in the archive: {name}"))?;
                let p = dest.join(&rel);
                // the target, seen from the link's folder, has to stay inside dest
                let base = rel.parent().map(Path::to_path_buf).unwrap_or_default();
                let joined = if kind == b'1' { safe_rel(&link) } else { safe_rel(&base.join(&link).to_string_lossy()).or_else(|| resolve_up(&base, &link)) };
                let Some(target_rel) = joined else { anyhow::bail!("a link in the archive points outside it: {name} -> {link}") };
                if let Some(d) = p.parent() {
                    std::fs::create_dir_all(d)?;
                }
                let _ = std::fs::remove_file(&p);
                if kind == b'1' {
                    std::fs::copy(dest.join(target_rel), &p)?;
                } else {
                    std::os::unix::fs::symlink(&link, &p)?;
                }
            }
            _ => {}
        }
        at = start + size.div_ceil(512) * 512;
    }
    Ok(())
}

/// `base` + a relative link with `..` parts, when it stays inside (base "a/b" + "../c" → "a/c").
fn resolve_up(base: &Path, link: &str) -> Option<PathBuf> {
    let mut parts: Vec<String> = base.components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect();
    for c in Path::new(link).components() {
        match c {
            std::path::Component::ParentDir => { parts.pop()?; }
            std::path::Component::Normal(x) => parts.push(x.to_string_lossy().to_string()),
            std::path::Component::CurDir => {}
            _ => return None,
        }
    }
    (!parts.is_empty()).then(|| parts.iter().collect())
}

fn unzip_all(archive: &Path, dest: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut a = zip::ZipArchive::new(std::fs::File::open(archive)?).map_err(|e| anyhow::anyhow!("not a zip file: {e}"))?;
    a.extract(dest).map_err(|e| anyhow::anyhow!("unpacking failed: {e}"))?;
    if let Some(s) = find_server(dest) {
        std::fs::set_permissions(&s, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

// ---- install ------------------------------------------------------------------------------------

/// Stream a download to `to` (through `<to>.part`), returning its SHA-256.
async fn fetch_to(st: &AppState, url: &str, to: &Path, max: u64, limit: Duration) -> anyhow::Result<String> {
    use futures_util::StreamExt;
    use sha2::Digest;
    let part = to.with_extension("part");
    if let Some(d) = to.parent() {
        tokio::fs::create_dir_all(d).await?;
    }
    let r = st.http.get(url).timeout(limit).send().await.map_err(|e| anyhow::anyhow!("download failed: {e}"))?;
    if !r.status().is_success() {
        anyhow::bail!("download failed: {url} returned {}", r.status());
    }
    let mut hash = sha2::Sha256::new();
    let res: anyhow::Result<()> = async {
        let mut f = tokio::fs::File::create(&part).await?;
        let mut body = r.bytes_stream();
        let mut n: u64 = 0;
        while let Some(chunk) = body.next().await {
            let chunk = chunk.map_err(|e| anyhow::anyhow!("download failed: {e}"))?;
            n += chunk.len() as u64;
            if n > max {
                anyhow::bail!("download failed: larger than {} MB", max >> 20);
            }
            hash.update(&chunk);
            tokio::io::AsyncWriteExt::write_all(&mut f, &chunk).await?;
        }
        tokio::io::AsyncWriteExt::flush(&mut f).await?;
        Ok(())
    }.await;
    if let Err(e) = res {
        let _ = tokio::fs::remove_file(&part).await;
        return Err(e);
    }
    tokio::fs::rename(&part, to).await?;
    Ok(hex::encode(hash.finalize()))
}

/// The SHA-256 Hugging Face lists for a model file (its LFS oid), when the API answers.
async fn model_sha256(st: &AppState, m: &Model) -> Option<String> {
    let v: Value = st.http.get(format!("https://huggingface.co/api/models/{}/tree/main", m.repo)).timeout(Duration::from_secs(20)).send().await.ok()?.json().await.ok()?;
    v.as_array()?.iter().find(|f| f["path"].as_str() == Some(m.file))?["lfs"]["oid"].as_str().map(|s| s.to_ascii_lowercase())
}

/// A GGUF file starts with the bytes "GGUF".
pub fn is_gguf(path: &Path) -> bool {
    use std::io::Read;
    let mut magic = [0u8; 4];
    std::fs::File::open(path).and_then(|mut f| f.read_exact(&mut magic)).is_ok() && &magic == b"GGUF"
}

/// Download the chosen model when it is missing (checked against Hugging Face's SHA-256 when the API
/// answers, and the GGUF header always); other models are removed to free the disk.
async fn ensure_model(st: &AppState) -> anyhow::Result<()> {
    let m = model(st);
    let path = model_path(st.data_dir(), &m);
    if path.is_file() && is_gguf(&path) {
        return Ok(());
    }
    st.db.log("info", &format!("Translator: downloading the {} model ({:.1} GB)", m.id, m.gb));
    let want = model_sha256(st, &m).await;
    let got = fetch_to(st, &model_url(&m), &path, 12 << 30, Duration::from_secs(4 * 3600)).await?;
    if want.as_ref().is_some_and(|w| *w != got) || !is_gguf(&path) {
        let _ = std::fs::remove_file(&path);
        anyhow::bail!("the model download is damaged (checksum or file header does not match) – try again");
    }
    if let Ok(rd) = std::fs::read_dir(models_dir(st.data_dir())) {
        for e in rd.flatten() {
            if e.file_name().to_string_lossy() != m.file {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    Ok(())
}

/// Install llama.cpp build `tag` from `url` (unless it is the one installed), then the chosen model.
pub async fn install(st: &AppState, tag: &str, url: &str) -> anyhow::Result<String> {
    if tag.is_empty() || tag.contains('/') || tag.starts_with('.') {
        anyhow::bail!("odd version {tag:?}");
    }
    let data = st.data_dir().to_path_buf();
    if installed(&data).map(|(_, t)| t) != Some(tag.to_string()) {
        stop().await;
        let root = server_root(&data);
        std::fs::create_dir_all(&root)?;
        let file = root.join(format!(".{tag}.{}", if url.ends_with(".zip") { "zip" } else { "tar.gz" }));
        fetch_to(st, url, &file, 1 << 30, Duration::from_secs(1800)).await?;
        let (root2, tag2, file2) = (root.clone(), tag.to_string(), file.clone());
        let unpacked = tokio::task::spawn_blocking(move || -> anyhow::Result<PathBuf> {
            let tmp = root2.join(format!(".{tag2}.new"));
            let _ = std::fs::remove_dir_all(&tmp);
            std::fs::create_dir_all(&tmp)?;
            if file2.extension().is_some_and(|e| e == "zip") {
                unzip_all(&file2, &tmp)?;
            } else {
                untar_gz_all(&std::fs::read(&file2)?, &tmp)?;
            }
            let dest = root2.join(&tag2);
            let _ = std::fs::remove_dir_all(&dest);
            std::fs::rename(&tmp, &dest)?;
            find_server(&dest).ok_or_else(|| anyhow::anyhow!("the download has no llama-server program"))
        }).await?;
        let _ = std::fs::remove_file(&file);
        let exe = unpacked?;
        let o = tokio::time::timeout(Duration::from_secs(60), command(&exe).arg("--version").output()).await;
        if !matches!(&o, Ok(Ok(o)) if o.status.success()) {
            let why = match o {
                Ok(Ok(o)) => String::from_utf8_lossy(&o.stderr).lines().last().unwrap_or("").to_string(),
                Ok(Err(e)) => e.to_string(),
                Err(_) => "no answer within 60 s".into(),
            };
            let _ = std::fs::remove_dir_all(root.join(tag));
            anyhow::bail!("llama.cpp {tag} was downloaded but does not start here ({why})");
        }
        std::fs::write(root.join(".current.tmp"), format!("{tag}\n"))?;
        std::fs::rename(root.join(".current.tmp"), root.join("current"))?;
        if let Ok(rd) = std::fs::read_dir(&root) {
            for e in rd.flatten() {
                if e.path().is_dir() && e.file_name().to_string_lossy() != tag {
                    let _ = std::fs::remove_dir_all(e.path());
                }
            }
        }
    }
    ensure_model(st).await?;
    st.db.log("info", &format!("Translator: llama.cpp {tag} with the {} model is ready", model(st).id));
    super::wake();
    Ok(tag.to_string())
}

/// Remove the program and the models (never under a running translation).
pub fn reset(data: &Path) -> anyhow::Result<()> {
    let mut g = SERVER.try_lock().map_err(|_| anyhow::anyhow!("The Translator is busy – try again in a minute"))?;
    if let Some(mut r) = g.take() {
        let _ = r.child.start_kill();
    }
    let d = dir(data);
    if d.exists() {
        std::fs::remove_dir_all(d)?;
    }
    Ok(())
}

/// The Download tools row.
pub fn info(st: &AppState, last: &Value) -> ToolInfo {
    let data = st.data_dir();
    let inst = installed(data).map(|(_, t)| t);
    let m = model(st);
    let has_model = model_path(data, &m).is_file();
    let latest = last["latest"].as_str().map(String::from);
    let newer = inst.as_deref().zip(latest.as_deref()).is_some_and(|(i, l)| newer_build(l, i));
    let note = match (&inst, has_model) {
        (Some(_), true) => format!("Translates titles on this server with the {} model – starts when needed, stops after 2 minutes idle", m.id),
        (Some(_), false) => format!("The {} model is not downloaded yet – Update downloads it (about {:.1} GB)", m.id, m.gb),
        (None, _) => format!("Not installed – Install downloads llama.cpp and the {} model (about {:.1} GB) to translate titles on this server", m.id, m.gb),
    };
    ToolInfo {
        id: "translator".into(), name: crate::tools::label("translator").into(),
        source: if inst.is_some() { "updated" } else { "missing" }.into(), resettable: inst.is_some() || dir(data).exists(),
        update_available: inst.is_some() && (newer || !has_model),
        // a missing model counts as an update: the button downloads it
        latest: if inst.is_some() && !has_model && !newer { inst.clone() } else { latest },
        installed: inst, checked_at: last["checked_at"].as_f64(), error: last["error"].as_str().map(String::from), note, updatable: true,
    }
}

// ---- running ------------------------------------------------------------------------------------

fn command(exe: &Path) -> tokio::process::Command {
    let mut c = tokio::process::Command::new(exe);
    if let Some(d) = exe.parent() {
        // the shared libraries sit next to the program
        let old = std::env::var("LD_LIBRARY_PATH").unwrap_or_default();
        c.env("LD_LIBRARY_PATH", if old.is_empty() { d.display().to_string() } else { format!("{}:{old}", d.display()) });
    }
    c.stdin(std::process::Stdio::null()).kill_on_drop(true);
    c
}

struct Running {
    child: tokio::process::Child,
    port: u16,
    model: PathBuf,
    last: Instant,
}

static SERVER: LazyLock<tokio::sync::Mutex<Option<Running>>> = LazyLock::new(Default::default);
/// A client that never goes through a proxy (the server is on 127.0.0.1).
static LOCAL: LazyLock<reqwest::Client> = LazyLock::new(|| reqwest::Client::builder().no_proxy().build().unwrap_or_default());

fn free_port() -> anyhow::Result<u16> {
    Ok(std::net::TcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
}

/// Whether the model server is running (for the status line).
pub fn running() -> bool {
    SERVER.try_lock().map(|g| g.is_some()).unwrap_or(true)
}

/// Stop the model server (idle, reset, update).
pub async fn stop() {
    if let Some(mut r) = SERVER.lock().await.take() {
        let _ = r.child.kill().await;
    }
}

async fn start(st: &AppState) -> anyhow::Result<Running> {
    let data = st.data_dir();
    let (exe, _) = installed(data).ok_or_else(|| anyhow::anyhow!("The Translator is not installed (Settings → Download tools)"))?;
    let m = model(st);
    let model = model_path(data, &m);
    if !model.is_file() {
        anyhow::bail!("The {} model is not downloaded (Settings → Download tools → Translator)", m.id);
    }
    let port = free_port()?;
    let threads = std::thread::available_parallelism().map(|n| (n.get() / 2).max(1)).unwrap_or(2);
    let mut cmd = command(&exe);
    cmd.arg("-m").arg(&model).args(["--host", "127.0.0.1", "--port", &port.to_string(), "-c", "4096", "-t", &threads.to_string(), "-np", "1"])
        .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    // background work: below everything else on the box
    unsafe {
        cmd.pre_exec(|| {
            libc::setpriority(libc::PRIO_PROCESS, 0, 15);
            crate::busy::idle_io_for_child();
            Ok(())
        });
    }
    let mut child = cmd.spawn().map_err(|e| anyhow::anyhow!("could not start llama-server: {e}"))?;
    let until = Instant::now() + LOAD_LIMIT;
    loop {
        if let Ok(Some(s)) = child.try_wait() {
            anyhow::bail!("llama-server stopped while loading the model ({s}) – is there enough memory for the {} model?", m.id);
        }
        let ok = LOCAL.get(format!("http://127.0.0.1:{port}/health")).timeout(Duration::from_secs(5)).send().await.is_ok_and(|r| r.status().is_success());
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
pub async fn chat(st: &AppState, messages: Value, thinking: bool) -> anyhow::Result<String> {
    let mut g = SERVER.lock().await;
    let want = model_path(st.data_dir(), &model(st));
    let alive = match g.as_mut() {
        Some(r) => r.model == want && matches!(r.child.try_wait(), Ok(None)),
        None => false,
    };
    if !alive {
        if let Some(mut old) = g.take() {
            let _ = old.child.kill().await;
        }
        *g = Some(start(st).await?);
        ensure_reaper();
    }
    let r = g.as_mut().expect("started above");
    r.last = Instant::now();
    let body = json!({
        "messages": messages, "temperature": 0.2, "top_p": 0.9, "max_tokens": if thinking { 3072 } else { 2048 },
        "response_format": { "type": "json_object" },
        // Qwen3 hybrid models: answer without thinking (ignored by templates that don't know it)
        "chat_template_kwargs": { "enable_thinking": false },
    });
    let resp = LOCAL.post(format!("http://127.0.0.1:{}/v1/chat/completions", r.port)).json(&body).timeout(Duration::from_secs(600)).send().await;
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
fn ensure_reaper() {
    static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if STARTED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    tokio::spawn(async {
        loop {
            tokio::time::sleep(Duration::from_secs(15)).await;
            let mut g = SERVER.lock().await;
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

    fn releases() -> Value {
        json!([
            { "tag_name": "v0.7.0", "draft": false, "assets": [{ "name": "source.tar.gz", "browser_download_url": "https://x/src" }] },
            { "tag_name": "b11669", "draft": false, "prerelease": true, "assets": [
                { "name": "llama-b11669-bin-ubuntu-vulkan-x64.tar.gz", "browser_download_url": "https://x/vulkan" },
                { "name": "llama-b11669-bin-ubuntu-cuda-12.8-x64.tar.gz", "browser_download_url": "https://x/cuda" },
                { "name": "llama-b11669-bin-ubuntu-x64.tar.gz", "browser_download_url": "https://x/cpu-x64" },
                { "name": "llama-b11669-bin-ubuntu-arm64.tar.gz", "browser_download_url": "https://x/cpu-arm64" },
                { "name": "cudart-llama-b11669-bin-ubuntu-cuda-13.4-x64.tar.gz", "browser_download_url": "https://x/cudart" },
            ] },
            { "tag_name": "b11662", "assets": [{ "name": "llama-b11662-bin-ubuntu-x64.zip", "browser_download_url": "https://x/old" }] },
        ])
    }

    #[test]
    fn picks_the_cpu_build_for_the_machine() {
        assert_eq!(pick_release(&releases(), "x86_64"), Some(("b11669".into(), "llama-b11669-bin-ubuntu-x64.tar.gz".into(), "https://x/cpu-x64".into())));
        assert_eq!(pick_release(&releases(), "aarch64").unwrap().2, "https://x/cpu-arm64");
        assert_eq!(pick_release(&releases(), "riscv64"), None);
        let zip_only = json!([{ "tag_name": "b6000", "assets": [{ "name": "llama-b6000-bin-ubuntu-x64.zip", "browser_download_url": "https://x/z" }] }]);
        assert_eq!(pick_release(&zip_only, "x86_64").unwrap().0, "b6000");
        let draft = json!([{ "tag_name": "b7000", "draft": true, "assets": [{ "name": "llama-b7000-bin-ubuntu-x64.zip", "browser_download_url": "https://x/d" }] }]);
        assert_eq!(pick_release(&draft, "x86_64"), None);
    }

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

    /// A .tar.gz made by the system's tar, with a program, a library and a link to it.
    #[test]
    fn unpacks_files_and_links() {
        let root = std::env::temp_dir().join(format!("encore-llama-tar-{}-{}", std::process::id(), crate::auth::random_hex(4)));
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
        untar_gz_all(&std::fs::read(root.join("a.tar.gz")).unwrap(), &out).unwrap();
        let exe = find_server(&out).unwrap();
        assert_eq!(exe, out.join("build/bin/llama-server"));
        assert_eq!(std::fs::metadata(&exe).unwrap().permissions().mode() & 0o777, 0o755);
        assert_eq!(std::fs::read(out.join("build/bin/libllama.so")).unwrap(), b"lib", "the link resolves inside");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn archive_paths_stay_inside() {
        assert_eq!(safe_rel("./build/bin/x"), Some(PathBuf::from("build/bin/x")));
        assert_eq!(safe_rel("../etc/passwd"), None);
        assert_eq!(safe_rel("/etc/passwd"), None);
        assert_eq!(resolve_up(Path::new("a/b"), "../c"), Some(PathBuf::from("a/c")));
        assert_eq!(resolve_up(Path::new("a"), "../../c"), None);
    }

    #[test]
    fn gguf_header() {
        let p = std::env::temp_dir().join(format!("encore-gguf-{}-{}", std::process::id(), crate::auth::random_hex(4)));
        std::fs::write(&p, b"GGUF\x03\x00").unwrap();
        assert!(is_gguf(&p));
        std::fs::write(&p, b"<html>").unwrap();
        assert!(!is_gguf(&p));
        let _ = std::fs::remove_file(&p);
    }
}
