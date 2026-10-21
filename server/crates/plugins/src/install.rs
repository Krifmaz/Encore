//! The one installer: every program Encore downloads and runs comes in through here (a plugin's zip, a
//! `[[binary]]` program, a `[python]` venv, a `[[download]]` data file, and the old tools' own installs –
//! Cadenza, slskd, llama.cpp – until their plugins take them over).
//!
//! The steps, each through `fsx` so an error names its operation and path:
//! 1. **fetch**: streamed to a `.part` file with a size cap ([`crate::fetch`]);
//! 2. **sum**: the sha256 checked before anything is unpacked (a fixed sum, a `SHA256SUMS` file, GitHub's
//!    asset digest), a mismatch deletes the download;
//! 3. **unpack**: zip or tar.gz into `<root>/.<version>.new` with the names checked ([`crate::archive`]);
//! 4. **verify**: the version command (`[install] verify`, `{bin} --version` by default) must exit 0, the
//!    `[[install.check]]` rules must hold;
//! 5. **switch**: the folder becomes `<root>/<version>`, `current` names it (written to a temporary file and
//!    renamed, so it is never half written), and the other versions are removed (**prune**).
//!
//! A step that fails removes what the install made and leaves the version in use as it was.
use crate::archive::{self, Kind, Limits as UnpackLimits};
use crate::manifest::{is_version, Check, Manifest};
use crate::sandbox::{Limits, Sandbox, Spawn};
use encore_core::fsx::{self, Step};
use serde_json::Value;
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// `SHA256SUMS` (sha256sum format: `<hex>  <name>`, or `<hex> *<name>` for binary mode) → name → hex.
pub fn parse_sums(text: &str) -> HashMap<String, String> {
    text.lines()
        .filter_map(|l| {
            let (hash, name) = l.trim().split_once(char::is_whitespace)?;
            let name = name.trim_start().trim_start_matches('*').trim();
            (hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit()) && !name.is_empty())
                .then(|| (name.trim_start_matches("./").to_string(), hash.to_ascii_lowercase()))
        })
        .collect()
}

/// The sha256 `want` for `name` from a sums file's text.
pub fn sum_for(sums: &str, name: &str) -> anyhow::Result<String> {
    parse_sums(sums).remove(name).ok_or_else(|| anyhow::anyhow!("{name} is not listed in the release's sums"))
}

/// A download's sha256 against the one expected: a mismatch deletes the file.
pub fn check_sha256(file: &Path, got: &str, want: &str, what: &str) -> anyhow::Result<()> {
    let want = want.trim().trim_start_matches("sha256:").to_ascii_lowercase();
    if want.len() != 64 || !want.bytes().all(|b| b.is_ascii_hexdigit()) {
        let _ = std::fs::remove_file(file);
        anyhow::bail!("{what}: the expected sha256 is not one ({want:?})");
    }
    if !got.eq_ignore_ascii_case(&want) {
        let _ = std::fs::remove_file(file);
        anyhow::bail!("{what} does not match its sha256 (downloaded {got}, expected {want}) – it was deleted");
    }
    Ok(())
}

// ---- the version folders ------------------------------------------------------------------------------

/// The versions of one program: `<root>/<version>/` and `<root>/current` naming the one in use.
#[derive(Debug, Clone, PartialEq)]
pub struct Versions {
    pub root: PathBuf,
}

impl Versions {
    pub fn new(root: &Path) -> Versions {
        Versions { root: root.to_path_buf() }
    }

    /// The version `current` names, when it is a folder name (nothing that leaves the root).
    pub fn current(&self) -> Option<String> {
        let v = std::fs::read_to_string(self.root.join("current")).ok()?;
        let v = v.trim();
        (!v.is_empty() && !v.contains('/') && !v.starts_with('.') && v != "current").then(|| v.to_string())
    }

    pub fn dir(&self, version: &str) -> PathBuf {
        self.root.join(version)
    }

    /// The folder a new version is unpacked into: `<root>/.<version>.new`, empty.
    pub fn stage(&self, version: &str) -> anyhow::Result<PathBuf> {
        if !is_version(version) {
            anyhow::bail!("not a version: {version:?}");
        }
        fsx::mkdir_all(&self.root, Step::Plugin)?;
        let s = self.root.join(format!(".{version}.new"));
        if s.exists() {
            fsx::remove_dir_all(&s, Step::Plugin)?;
        }
        fsx::mkdir(&s, Step::Plugin)?;
        Ok(s)
    }

    /// Give up a staged version.
    pub fn abandon(&self, version: &str) {
        let _ = std::fs::remove_dir_all(self.root.join(format!(".{version}.new")));
    }

    /// Switch to the staged version: it becomes `<root>/<version>`, `current` names it, the others go.
    pub fn commit(&self, version: &str) -> anyhow::Result<PathBuf> {
        let staged = self.root.join(format!(".{version}.new"));
        let dest = self.dir(version);
        if dest.exists() {
            fsx::remove_dir_all(&dest, Step::Plugin)?;
        }
        fsx::rename(&staged, &dest, Step::Plugin)?;
        self.set_current(version)?;
        self.prune(version);
        Ok(dest)
    }

    /// `current` → `version`, atomically.
    pub fn set_current(&self, version: &str) -> anyhow::Result<()> {
        let tmp = self.root.join(".current.tmp");
        fsx::write(&tmp, format!("{version}\n"), Step::Plugin)?;
        fsx::rename(&tmp, self.root.join("current"), Step::Plugin)?;
        Ok(())
    }

    /// Remove every version folder but `keep` (folders named like versions only: a plugin's `data`, `logs`,
    /// `home` and `tmp` stay) and leftovers of stopped installs (`.<x>.new`, `.<x>.part`).
    pub fn prune(&self, keep: &str) {
        let Ok(rd) = std::fs::read_dir(&self.root) else { return };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let Ok(t) = e.file_type() else { continue };
            let leftover = name.starts_with('.') && (name.ends_with(".new") || name.ends_with(".part"));
            if t.is_dir() && name != keep && (is_version(&name) || leftover) {
                let _ = std::fs::remove_dir_all(e.path());
            } else if t.is_file() && leftover {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

// ---- running a program to check it ---------------------------------------------------------------------

/// What a program printed.
#[derive(Debug, Clone, PartialEq)]
pub struct Output {
    pub ok: bool,
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// Run a program to its end (at most `limit`), its output kept (1 MB each). "Text file busy" (a file just
/// written) is tried again a few times.
pub fn output(spawn: &Spawn, limit: Duration) -> anyhow::Result<Output> {
    use std::process::Stdio;
    let mut tries = 0;
    let mut child = loop {
        let mut cmd = spawn.command();
        cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        match fsx::spawn(Step::Plugin, spawn.program(), || cmd.spawn()) {
            Err(e) if e.errno == Some(26) && tries < 5 => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(50 * tries));
            }
            r => break r?,
        }
    };
    let read = |s: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut out = Vec::new();
            if let Some(mut s) = s {
                let _ = s.by_ref().take(1 << 20).read_to_end(&mut out);
                let _ = std::io::copy(&mut s, &mut std::io::sink());
            }
            String::from_utf8_lossy(&out).into_owned()
        })
    };
    let out = read(child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>));
    let err = read(child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>));
    let until = Instant::now() + limit;
    let status = loop {
        if let Some(s) = child.try_wait().map_err(fsx::at(Step::Plugin, "wait", spawn.program()))? {
            break Some(s);
        }
        if Instant::now() >= until {
            // SAFETY: kill only sends a signal, to the group the program leads (it called setsid)
            unsafe {
                libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL);
            }
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let stdout = out.join().unwrap_or_default();
    let stderr = err.join().unwrap_or_default();
    let Some(status) = status else {
        anyhow::bail!("{} did not finish within {} seconds", spawn.program().display(), limit.as_secs());
    };
    Ok(Output { ok: status.success(), code: status.code(), stdout, stderr })
}

/// What a version command said.
#[derive(Debug, Clone, PartialEq)]
pub struct Verified {
    /// the version it names (`[install] version_from`)
    pub version: Option<String>,
    pub output: Output,
}

/// Run `[install] verify` for the program `bin` (`{bin}` filled in), in limits-only mode with `env` (the
/// manifest's `[run] env`, filled), and check `[[install.check]]`'s `json` rules against what it printed.
pub fn verify(m: &Manifest, bin: &Path, env: &[(String, String)], home: &Path) -> anyhow::Result<Verified> {
    let argv: Vec<String> = m.verify_argv().iter().map(|a| a.replace("{bin}", &bin.display().to_string())).collect();
    let limit = Duration::from_secs(m.install.as_ref().and_then(|i| i.timeout).unwrap_or(crate::manifest::DEFAULT_VERIFY_SECS));
    let program = PathBuf::from(&argv[0]);
    let mut full_env: Vec<(String, String)> = vec![
        ("PATH".into(), "/usr/local/bin:/usr/bin:/bin".into()),
        ("HOME".into(), home.display().to_string()),
        ("LANG".into(), "C.UTF-8".into()),
        ("TERM".into(), "dumb".into()),
        ("NO_COLOR".into(), "1".into()),
    ];
    full_env.extend(env.iter().cloned());
    fsx::mkdir_all(home, Step::Plugin)?;
    let spawn = Spawn::new(&Sandbox::limits_only(), &program).args(argv[1..].iter().cloned()).env(full_env).cwd(home).limits(Limits::NONE);
    let name = &m.plugin.name;
    let o = output(&spawn, limit).map_err(|e| anyhow::anyhow!("{name} does not run on this server: {e:#}"))?;
    if !o.ok {
        let text = encore_core::text::last_lines(&format!("{}{}", o.stdout, o.stderr), 3);
        anyhow::bail!("`{}` failed (exit status {}){}", argv.join(" "), o.code.map(|c| c.to_string()).unwrap_or_else(|| "none".into()), if text.is_empty() { String::new() } else { format!(": {text}") });
    }
    let json: Option<Value> = o.stdout.lines().rev().filter_map(|l| serde_json::from_str::<Value>(l.trim()).ok()).find(Value::is_object);
    let from = m.install.as_ref().and_then(|i| i.version_from.clone()).unwrap_or_else(|| "output".into());
    let version = match from.strip_prefix("json:") {
        Some(field) => json.as_ref().and_then(|j| j.get(field)).and_then(Value::as_str).and_then(encore_core::version::extract_version),
        None => encore_core::version::extract_version(&format!("{}\n{}", o.stdout, o.stderr)),
    };
    for c in m.install.iter().flat_map(|i| i.checks.iter()) {
        if let (Some(field), Some(want)) = (&c.json, &c.equals) {
            if json.as_ref().and_then(|j| j.get(field)) != Some(want) {
                anyhow::bail!("{}", c.message);
            }
        }
    }
    Ok(Verified { version, output: o })
}

/// `[[install.check]]`'s `file` rules: a file under `base` (`*` within one name) contains a text.
pub fn check_files(checks: &[Check], base: &Path) -> anyhow::Result<()> {
    for c in checks {
        let (Some(file), Some(contains)) = (&c.file, &c.contains) else { continue };
        let found = glob(base, file).into_iter().any(|p| std::fs::read_to_string(p).is_ok_and(|t| t.contains(contains.as_str())));
        if !found {
            anyhow::bail!("{}", c.message);
        }
    }
    Ok(())
}

/// The files matching a relative pattern whose names may hold `*` (within one name).
pub fn glob(base: &Path, pattern: &str) -> Vec<PathBuf> {
    let mut at = vec![base.to_path_buf()];
    for part in pattern.split('/').filter(|p| !p.is_empty()) {
        let mut next = vec![];
        for dir in &at {
            if !part.contains('*') {
                let p = dir.join(part);
                if p.exists() {
                    next.push(p);
                }
                continue;
            }
            let Ok(rd) = std::fs::read_dir(dir) else { continue };
            for e in rd.flatten() {
                if wildcard(part, &e.file_name().to_string_lossy()) {
                    next.push(e.path());
                }
            }
        }
        next.sort();
        at = next;
    }
    at
}

fn wildcard(pattern: &str, name: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    let (first, last) = (parts[0], parts[parts.len() - 1]);
    if !name.starts_with(first) || name.len() < first.len() + last.len() || !name.ends_with(last) {
        return false;
    }
    let mut at = first.len();
    let end = name.len() - last.len();
    for mid in &parts[1..parts.len() - 1] {
        match name[at..end].find(mid) {
            Some(i) => at += i + mid.len(),
            None => return false,
        }
    }
    true
}

// ---- a program download ----------------------------------------------------------------------------------

/// What to install: a download (its URL, its expected sha256, how it unpacks) and the program in it.
#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub url: String,
    /// the file name the download has (the asset's name; for messages and unpacking by name)
    pub name: String,
    /// None: no sum is known (refused unless `allow_unsummed`)
    pub sha256: Option<String>,
    /// "zip", "tar.gz" or "file"
    pub unpack: String,
    /// `[[binary]] bin`
    pub bin: String,
    /// paths removed from what was unpacked
    pub remove: Vec<String>,
    pub max_bytes: u64,
}

/// An installed version.
#[derive(Debug, Clone, PartialEq)]
pub struct Installed {
    pub version: String,
    pub dir: PathBuf,
    pub bin: PathBuf,
    pub sha256: String,
    /// what the version command named
    pub reports: Option<String>,
}

/// How the download step reaches the network: the plugin client and whether plain http is allowed (tests).
#[derive(Clone)]
pub struct Net {
    pub http: reqwest::Client,
    pub allow_http: bool,
}

/// Fetch, check, unpack, verify and switch to `version` of a program under `versions` (a manifest gives the
/// version command, the checks and `[run] env`; `prepare` may change the unpacked folder before it is verified,
/// as slskd's web UI is replaced by an empty folder).
pub async fn install_program(
    net: &Net,
    m: &Manifest,
    versions: &Versions,
    version: &str,
    p: &Program,
    prepare: impl FnOnce(&Path) -> anyhow::Result<()> + Send + 'static,
) -> anyhow::Result<Installed> {
    let stage = versions.stage(version)?;
    let res = install_staged(net, m, versions, version, p, &stage, prepare).await;
    if res.is_err() {
        versions.abandon(version);
    }
    let _ = std::fs::remove_file(versions.root.join(format!(".{version}.download")));
    res
}

async fn install_staged(
    net: &Net,
    m: &Manifest,
    versions: &Versions,
    version: &str,
    p: &Program,
    stage: &Path,
    prepare: impl FnOnce(&Path) -> anyhow::Result<()> + Send + 'static,
) -> anyhow::Result<Installed> {
    let download = versions.root.join(format!(".{version}.download"));
    let got = crate::fetch::to_file(&net.http, &p.url, &download, p.max_bytes, Duration::from_secs(1800), net.allow_http).await?;
    let want = p.sha256.as_deref().ok_or_else(|| anyhow::anyhow!("{}: no sha256 is known for {}, so Encore does not install it", m.plugin.name, p.name))?;
    check_sha256(&download, &got.sha256, want, &p.name)?;
    let (stage2, download2, p2, m2) = (stage.to_path_buf(), download.clone(), p.clone(), m.clone());
    let root = versions.root.clone();
    let (bin, reports) = tokio::task::spawn_blocking(move || -> anyhow::Result<(PathBuf, Option<String>)> {
        unpack_program(&download2, &p2, &stage2)?;
        prepare(&stage2)?;
        let bin = archive::find_bin(&stage2, &p2.bin).ok_or_else(|| anyhow::anyhow!("the download has no {} program in it", p2.bin))?;
        make_executable(&bin)?;
        check_files(&m2.install.as_ref().map(|i| i.checks.clone()).unwrap_or_default(), &stage2)?;
        let env = run_env(&m2, &bin, &root);
        let v = verify(&m2, &bin, &env, &root.join(".home"))?;
        Ok((bin, v.version))
    })
    .await??;
    let rel = bin.strip_prefix(stage).map(Path::to_path_buf).unwrap_or_else(|_| PathBuf::from(&p.bin));
    let dir = versions.commit(version)?;
    let _ = std::fs::remove_dir_all(versions.root.join(".home"));
    Ok(Installed { version: version.to_string(), bin: dir.join(rel), dir, sha256: got.sha256, reports })
}

/// Unpack a program download into `stage` ("file": the download is the program itself).
pub fn unpack_program(download: &Path, p: &Program, stage: &Path) -> anyhow::Result<()> {
    match p.unpack.as_str() {
        "file" => {
            let name = Path::new(&p.bin).file_name().ok_or_else(|| anyhow::anyhow!("odd program name {:?}", p.bin))?;
            fsx::copy(download, stage.join(name), Step::Plugin)?;
        }
        other => {
            let kind = Kind::parse(other).or_else(|| Kind::of_name(&p.name)).or_else(|| Kind::sniff(download)).ok_or_else(|| anyhow::anyhow!("{} is neither a zip nor a .tar.gz", p.name))?;
            archive::unpack(download, kind, stage, UnpackLimits { bytes: p.max_bytes.saturating_mul(4).max(256 << 20), ..UnpackLimits::default() })
                .map_err(|e| anyhow::anyhow!("unpacking {} failed: {e:#}", p.name))?;
        }
    }
    for r in &p.remove {
        if let Ok(Some(rel)) = archive::entry_path(r) {
            let target = stage.join(rel);
            if target.is_dir() {
                fsx::remove_dir_all(&target, Step::Plugin)?;
            } else if target.exists() {
                fsx::remove_file(&target, Step::Plugin)?;
            }
        }
    }
    Ok(())
}

pub fn make_executable(bin: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(bin, std::fs::Permissions::from_mode(0o755)).map_err(fsx::at(Step::Plugin, "chmod", bin))?;
    Ok(())
}

/// `[run] env` with `{bin}`, `{bin_dir}`, `{data}` and `{plugin}` filled in (`plugin_dir`: the plugin's folder).
pub fn run_env(m: &Manifest, bin: &Path, plugin_dir: &Path) -> Vec<(String, String)> {
    let Some(run) = &m.run else { return vec![] };
    let bin_dir = bin.parent().map(|d| d.display().to_string()).unwrap_or_default();
    run.env
        .iter()
        .map(|(k, v)| {
            let v = v
                .replace("{bin_dir}", &bin_dir)
                .replace("{bin}", &bin.display().to_string())
                .replace("{data}", &plugin_dir.join("data").display().to_string())
                .replace("{plugin}", &plugin_dir.display().to_string());
            (k.clone(), v)
        })
        .collect()
}

// ---- a Python venv ----------------------------------------------------------------------------------------

/// Where uv is: the image's pinned copy (/opt/uv/bin/uv), else `uv` on PATH.
pub fn find_uv() -> Option<PathBuf> {
    let pinned = Path::new("/opt/uv/bin/uv");
    if pinned.is_file() { Some(pinned.to_path_buf()) } else { encore_core::which::which("uv") }
}

/// Make `<versions>/<version>/venv` with uv and the `[python]` packages (`{version}` and `{tag}` filled in),
/// check `[[install.check]]` and the version command, then switch to it. The venv is made where it stays (a
/// venv's scripts name their own path), so it is made in the version folder itself and only `current` moves.
pub async fn install_python(m: &Manifest, versions: &Versions, version: &str, tag: &str, uv: &Path) -> anyhow::Result<Installed> {
    install_python_reporting(m, versions, version, tag, uv, None).await
}

/// [`install_python`] that also requires the version command to name `reports` (a pinned release whose tag
/// calls itself another version: streamrip's v2.4.10 tag says 2.4.9) before `current` moves.
pub async fn install_python_reporting(m: &Manifest, versions: &Versions, version: &str, tag: &str, uv: &Path, reports: Option<&str>) -> anyhow::Result<Installed> {
    let reports = reports.map(String::from);
    let py = m.python.clone().ok_or_else(|| anyhow::anyhow!("{} has no [python] table", m.plugin.name))?;
    if !is_version(version) {
        anyhow::bail!("not a version: {version:?}");
    }
    fsx::mkdir_all(&versions.root, Step::Plugin)?;
    let dir = versions.dir(version);
    if dir.exists() && versions.current().as_deref() != Some(version) {
        fsx::remove_dir_all(&dir, Step::Plugin)?;
    }
    let venv = dir.join("venv");
    let (m2, uv2, venv2, py2, tag2, version2, root) = (m.clone(), uv.to_path_buf(), venv.clone(), py.clone(), tag.to_string(), version.to_string(), versions.root.clone());
    let res = tokio::task::spawn_blocking(move || -> anyhow::Result<(PathBuf, Option<String>)> {
        let home = root.join(".home");
        fsx::mkdir_all(&home, Step::Plugin)?;
        let mut env: Vec<(String, String)> = vec![
            ("PATH".into(), "/usr/local/bin:/usr/bin:/bin".into()),
            ("HOME".into(), home.display().to_string()),
            ("LANG".into(), "C.UTF-8".into()),
            ("UV_PYTHON_INSTALL_DIR".into(), "/opt/python".into()),
            ("UV_PYTHON_DOWNLOADS".into(), "never".into()),
            ("UV_NO_CACHE".into(), "1".into()),
            ("UV_NO_PROGRESS".into(), "1".into()),
        ];
        for k in ["HTTP_PROXY", "HTTPS_PROXY", "NO_PROXY", "http_proxy", "https_proxy", "no_proxy", "SSL_CERT_FILE", "SSL_CERT_DIR", "UV_INDEX_URL", "UV_DEFAULT_INDEX"] {
            if let Ok(v) = std::env::var(k) {
                env.push((k.into(), v));
            }
        }
        let uv_run = |args: Vec<String>, what: &str, limit: u64| -> anyhow::Result<()> {
            let spawn = Spawn::new(&Sandbox::limits_only(), &uv2).args(args).env(env.clone()).cwd(&home).limits(Limits::NONE).low_priority(true);
            let o = output(&spawn, Duration::from_secs(limit))?;
            if !o.ok {
                anyhow::bail!("{what} failed: {}", encore_core::text::last_lines(&format!("{}{}", o.stdout, o.stderr), 8));
            }
            Ok(())
        };
        uv_run(vec!["venv".into(), "--python".into(), py2.python.clone(), venv2.display().to_string()], "Making the venv", 300)?;
        let mut args: Vec<String> = vec!["pip".into(), "install".into(), "--python".into(), venv2.join("bin/python").display().to_string()];
        if !py2.overrides.is_empty() {
            // one requirement per line, beside the venv (removed with the version that made it)
            let file = venv2.with_file_name("overrides.txt");
            let lines: Vec<String> = py2.overrides.iter().map(|o| crate::manifest::fill(o, &version2, &tag2)).collect();
            std::fs::write(&file, lines.join("\n") + "\n")?;
            args.extend(["--override".into(), file.display().to_string()]);
        }
        args.extend(py2.packages.iter().map(|p| crate::manifest::fill(p, &version2, &tag2)));
        uv_run(args, &format!("Installing {}", m2.plugin.name), 900)?;
        let bin = py2.bin.0.iter().map(|b| venv2.join("bin").join(b)).find(|p| p.is_file()).ok_or_else(|| anyhow::anyhow!("{} was installed but its program is missing", m2.plugin.name))?;
        check_files(&m2.install.as_ref().map(|i| i.checks.clone()).unwrap_or_default(), &venv2)?;
        let env = run_env(&m2, &bin, &root);
        let v = verify(&m2, &bin, &env, &home)?;
        let _ = std::fs::remove_dir_all(&home);
        if let Some(want) = &reports {
            if v.version.as_deref() != Some(want.as_str()) {
                anyhow::bail!("{} {tag2} was installed but calls itself {} instead of {want} – it is not the release Encore was tested with", m2.plugin.name, v.version.as_deref().unwrap_or("nothing"));
            }
        }
        Ok((bin, v.version))
    })
    .await?;
    match res {
        Ok((bin, reports)) => {
            versions.set_current(version)?;
            versions.prune(version);
            Ok(Installed { version: version.to_string(), dir, bin, sha256: String::new(), reports })
        }
        Err(e) => {
            if versions.current().as_deref() != Some(version) {
                let _ = std::fs::remove_dir_all(&dir);
            }
            Err(e)
        }
    }
}

// ---- data files -------------------------------------------------------------------------------------------

/// A data file at `dest` (a model): fetched when missing or broken, checked against `sha256` when known and
/// against its first bytes (`magic`) always.
pub async fn data_file(net: &Net, url: &str, dest: &Path, sha256: Option<&str>, magic: Option<&str>, max: u64) -> anyhow::Result<bool> {
    if dest.is_file() && magic.is_none_or(|mg| starts_with(dest, mg.as_bytes())) {
        return Ok(false);
    }
    let got = crate::fetch::to_file(&net.http, url, dest, max, Duration::from_secs(4 * 3600), net.allow_http).await?;
    let name = dest.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    if let Some(want) = sha256 {
        check_sha256(dest, &got.sha256, want, &name)?;
    }
    if let Some(mg) = magic {
        if !starts_with(dest, mg.as_bytes()) {
            let _ = std::fs::remove_file(dest);
            anyhow::bail!("{name} is damaged (its first bytes are not {mg}) – it was deleted");
        }
    }
    Ok(true)
}

/// The file starts with these bytes.
pub fn starts_with(path: &Path, magic: &[u8]) -> bool {
    let mut head = vec![0u8; magic.len()];
    std::fs::File::open(path).and_then(|mut f| f.read_exact(&mut head)).is_ok() && head == magic
}
