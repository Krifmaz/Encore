//! The first start after an upgrade from a release without plugins (0.30 and before): every catalog plugin that
//! was in use is adopted (its row, switched on, its old auto-update switch carried over) where its program and
//! data are now (`[migration] adopt`: nothing moves, so the previous image still runs on the same folders), and
//! a program that was in use but is not there or does not run here is installed, so downloads keep working
//! with no new setup. A fresh install adopts nothing: every plugin is "available, not installed".
//!
//! Which start is which: migration 0028 writes the `plugin_state` row `migration` once, `adopt` on a database
//! that was in use before (it has accounts or settings) and `fresh` on a new one. The adoption turns
//! it into `"install"` and the install pass into `"done"`; an install that fails (no network at that start) is
//! tried again while the server runs ([`Registry::migrate_until_done`]: after 1, 2, 5, 10 and 30 minutes, then
//! every hour, whatever the "Update plugins" task's settings are) and at the next start, and the "Update
//! plugins" task installs a missing program of a plugin that is switched on as well. Until then the request
//! worker waits for a source whose program is still being installed ([`Registry::installing_after_upgrade`])
//! instead of leaving it out.
//!
//! Where a builtin's program is: [`Registry::program_or_adopted`] – its own install under
//! `<config>/plugins/<id>` first, then what it adopted.
use crate::install::{self, Versions};
use crate::manifest::Manifest;
use crate::registry::{CfgFacts, Refused, Registry};
use crate::{catalog, Origin};
use encore_db::{p, RowExt};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The `plugin_state` row that says which start this is (see the module's text).
pub const MIGRATION_KEY: &str = "migration";

/// How long the install pass waits before it tries again after its `n`-th failed try (1-based): 1, 2, 5, 10
/// and 30 minutes, then every hour.
pub fn retry_after(n: usize) -> Duration {
    const MINUTES: [u64; 5] = [1, 2, 5, 10, 30];
    Duration::from_secs(60 * MINUTES.get(n.saturating_sub(1)).copied().unwrap_or(60))
}

/// A program a builtin plugin runs: the version, the program file, and whether it is an adopted copy (a folder
/// of Encore before plugins) rather than the plugin's own install.
#[derive(Debug, Clone, PartialEq)]
pub struct ProgramAt {
    pub version: String,
    pub path: PathBuf,
    pub adopted: bool,
}

/// A folder's `current` file: the version folder it names (nothing that leaves the folder).
fn current_in(dir: &Path) -> Option<String> {
    let v = std::fs::read_to_string(dir.join("current")).ok()?;
    let v = v.trim();
    (!v.is_empty() && !v.contains('/') && !v.starts_with('.') && v != "..").then(|| v.to_string())
}

/// The program a plugin adopted, in one of its `[migration] adopt` folders under `config_dir`: a venv's program
/// (`[python] bin`), or a downloaded program in the version folder `current` names (in the folder itself or one
/// level down: the Translator keeps llama.cpp in `tools/translator/llama`). The version is the folder's name (a
/// venv's is unknown: empty).
pub fn adopted_program(m: &Manifest, config_dir: &Path) -> Option<ProgramAt> {
    let mig = m.migration.as_ref()?;
    for rel in &mig.adopt {
        if !crate::manifest::is_rel_path(rel, false) {
            continue;
        }
        let dir = config_dir.join(rel);
        if let Some(py) = &m.python {
            if let Some(bin) = py.bin.0.iter().map(|b| dir.join("bin").join(b)).find(|p| p.is_file()) {
                return Some(ProgramAt { version: String::new(), path: bin, adopted: true });
            }
            continue;
        }
        let Some(b) = m.binary_for(std::env::consts::ARCH).or_else(|| m.binaries.first()) else { continue };
        let mut roots = vec![dir.clone()];
        if let Ok(rd) = std::fs::read_dir(&dir) {
            let mut subs: Vec<PathBuf> = rd.flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_dir())).map(|e| e.path()).collect();
            subs.sort();
            roots.extend(subs);
        }
        for root in roots {
            let Some(v) = current_in(&root) else { continue };
            if let Some(bin) = crate::archive::find_bin(&root.join(&v), &b.bin) {
                return Some(ProgramAt { version: v, path: bin, adopted: true });
            }
        }
    }
    None
}

/// The plugin's program as the system has it on PATH (a server run without Docker installs yt-dlp itself).
fn system_copies(m: &Manifest) -> Vec<ProgramAt> {
    let names: Vec<String> = match (&m.python, m.binary_for(std::env::consts::ARCH)) {
        (Some(py), _) => py.bin.0.clone(),
        (None, Some(b)) => vec![b.bin.clone()],
        _ => vec![],
    };
    names
        .iter()
        .filter_map(|n| encore_core::which::which(n))
        .map(|path| ProgramAt { version: String::new(), path, adopted: true })
        .collect()
}

/// "streamrip, yt-dlp" from a list of plugin ids (their names).
fn names_of(ids: &Value) -> String {
    ids.as_array()
        .map(|a| a.iter().filter_map(Value::as_str).map(|id| catalog::find(id).map(|m| m.plugin.name.clone()).unwrap_or_else(|| id.to_string())).collect::<Vec<_>>().join(", "))
        .unwrap_or_default()
}

/// The plugin needs its program to do anything (a source's or a tool's); Cadenza's engine is built into Encore.
fn program_needed(m: &Manifest) -> bool {
    m.installs_program() && m.plugin.kind != "playback"
}

impl Registry {
    /// Which start this is (see the module's text): "adopt", "install", "done" or "fresh".
    pub fn migration_marker(&self) -> String {
        self.db().one("SELECT value FROM plugin_state WHERE key=?", p![MIGRATION_KEY]).ok().flatten().map(|r| r.s("value")).unwrap_or_default()
    }

    fn set_migration_marker(&self, v: &str) {
        let _ = self.db().execute("INSERT OR REPLACE INTO plugin_state (key, value) VALUES (?,?)", p![MIGRATION_KEY, v]);
    }

    /// The program a catalog plugin runs: its own install (`<config>/plugins/<id>/<current>`), else the one it
    /// adopted from before plugins. None: neither is there.
    pub fn program_or_adopted(&self, id: &str) -> Option<ProgramAt> {
        if let Some((version, path)) = self.program(id) {
            return Some(ProgramAt { version, path, adopted: false });
        }
        adopted_program(catalog::find(id)?, self.config_dir())
    }

    /// Whether a program of `m` runs here: its version command (`[install] verify`) and its checks
    /// (`[[install.check]]`: a venv's code, the version command's JSON). The version it names, when it runs.
    pub fn program_runs(&self, m: &Manifest, p: &ProgramAt) -> Result<Option<String>, String> {
        if m.python.is_some() {
            // the venv's root: <venv>/bin/<program>
            if let Some(venv) = p.path.parent().and_then(Path::parent) {
                install::check_files(&m.install.as_ref().map(|i| i.checks.clone()).unwrap_or_default(), venv).map_err(|e| format!("{e:#}"))?;
            }
        }
        let root = self.folders(m.id()).root;
        let env = install::run_env(m, &p.path, &root);
        let home = std::env::temp_dir().join(format!("encore-plugin-check-{}-{}", std::process::id(), encore_core::ids::random_hex(4)));
        let v = install::verify(m, &p.path, &env, &home).map(|v| v.version).map_err(|e| format!("{e:#}"));
        let _ = std::fs::remove_dir_all(&home);
        v
    }

    /// Adopt a catalog plugin: its row (first-party, switched on), the version of what it adopted, and its
    /// auto-update switch from the old `tools_auto_<id>` (its saved value or its default) when it had one.
    /// Nothing is downloaded. A plugin that has a row already is left as it is.
    pub fn adopt(&self, id: &str, version: Option<&str>) -> Result<bool, Refused> {
        let m = catalog::find(id).ok_or_else(|| Refused::new(404, "Unknown plugin"))?;
        if self.installed(id) {
            return Ok(false);
        }
        let auto = match m.migration.as_ref().and_then(|g| g.auto_key.as_deref()) {
            Some(k) if self.cfg().def(k).is_some() => self.cfg().bool(k),
            _ => m.update.as_ref().is_some_and(|u| u.auto),
        };
        let now = encore_db::now();
        self.db()
            .execute(
                "INSERT OR IGNORE INTO plugins (id, version, enabled, installed_at, updated_at, origin, trust, manifest, sha256, auto_update, last_check, last_error)
                 VALUES (?, ?, 1, ?, ?, ?, ?, ?, NULL, ?, NULL, NULL)",
                p![id, version.filter(|v| !v.is_empty()), now, now, Origin::Catalog.as_str(), Origin::Catalog.trust(), catalog::text(id).unwrap_or_default(), auto as i64],
            )
            .map_err(|e| Refused::new(500, e.to_string()))?;
        self.log(id).line("adopt", &format!("adopted{}", version.filter(|v| !v.is_empty()).map(|v| format!(" {v}")).unwrap_or_default()));
        Ok(true)
    }

    /// The first start after an upgrade (see the module's text), its quick half: adopt every catalog plugin that
    /// was in use (rows only; nothing runs or downloads), so the request worker finds its sources switched on
    /// from the first request. The adopted ids; none on a fresh install or once done.
    pub fn adopt_in_use(&self) -> Vec<String> {
        let mut adopted = vec![];
        if self.migration_marker() != "adopt" {
            return adopted;
        }
        let facts = CfgFacts { cfg: self.cfg(), dir: self.config_dir() };
        for m in catalog::catalog() {
            if !m.in_use(&facts) {
                continue;
            }
            let found = adopted_program(m, self.config_dir());
            let version = found.as_ref().map(|p| p.version.clone()).filter(|v| !v.is_empty());
            match self.adopt(m.id(), version.as_deref()) {
                Ok(true) => adopted.push(m.id().to_string()),
                Ok(false) => {}
                Err(e) => tracing::warn!("plugins: adopting {} failed: {e}", m.id()),
            }
        }
        if !adopted.is_empty() {
            self.db().log("info", &format!("Plugins: adopted what was in use before this version: {}", adopted.join(", ")));
        }
        self.set_migration_marker("install");
        adopted
    }

    /// The first start after an upgrade (see the module's text): adopt what was in use ([`Registry::adopt_in_use`]),
    /// then install the programs that are missing or do not run here. Nothing on a fresh install or once done.
    /// The answer says what happened: `{state, adopted: [id], installed: [id], failed: [{id, error}]}`.
    pub async fn migrate(&self) -> Value {
        let adopted = self.adopt_in_use();
        if self.migration_marker() != "install" {
            return json!({ "state": self.migration_marker(), "adopted": adopted, "installed": [], "failed": [] });
        }
        let (installed, failed) = self.install_missing(&[]).await;
        if failed.is_empty() {
            self.set_migration_marker("done");
        }
        json!({ "state": self.migration_marker(), "adopted": adopted, "installed": installed, "failed": failed })
    }

    /// [`Registry::migrate`] until its install pass is done: a try that fails (no network yet at the start, a
    /// proxy, GitHub or PyPI down) is tried again after `wait(n)` (n: the failed tries so far; the server passes
    /// [`retry_after`]), so the programs come back without a restart. Each failed try is one line in the
    /// activity log. Returns the last try's answer (state "done" or "fresh").
    pub async fn migrate_until_done(&self, wait: impl Fn(usize) -> Duration) -> Value {
        let mut failures = 0usize;
        loop {
            let r = self.migrate().await;
            if self.migration_marker() != "install" {
                if failures > 0 {
                    self.db().log("info", &format!("Plugins: the programs that were missing after the upgrade are installed ({})", names_of(&r["installed"])));
                }
                return r;
            }
            failures += 1;
            let next = wait(failures);
            let what: Vec<String> = r["failed"]
                .as_array()
                .map(|a| a.iter().map(|f| format!("{}: {}", f["id"].as_str().unwrap_or(""), f["error"].as_str().unwrap_or("").lines().next().unwrap_or(""))).collect())
                .unwrap_or_default();
            let when = match next.as_secs() {
                s if s >= 3600 && s % 3600 == 0 => format!("{} hour{}", s / 3600, if s == 3600 { "" } else { "s" }),
                s if s >= 60 => format!("{} minute{}", s / 60, if s < 120 { "" } else { "s" }),
                s => format!("{s} seconds"),
            };
            self.db().log("warning", &format!("Plugins: installing what was in use before the upgrade failed ({}) – trying again in {when}", what.join(" · ")));
            tokio::time::sleep(next).await;
        }
    }

    /// A source plugin the upgrade adopted whose program Encore is still installing: the first start's install
    /// pass is not done yet (marker "install"), the plugin is switched on, and it is being installed now or has
    /// no program of its own or adopted yet. The request worker waits for it instead of leaving the source out.
    pub fn installing_after_upgrade(&self, id: &str) -> bool {
        if self.migration_marker() != "install" {
            return false;
        }
        let Some(row) = self.row(id) else { return false };
        if !row.b("enabled") || row.s("origin") != Origin::Catalog.as_str() || !catalog::find(id).is_some_and(program_needed) {
            return false;
        }
        self.busy_with(id) == Some("installing") || self.program_or_adopted(id).is_none()
    }

    /// Install the program of every switched-on catalog plugin that needs one and has none that runs here (all
    /// of them, or the `only` ones): `(installed ids, [{id, error}])`.
    pub async fn install_missing(&self, only: &[&str]) -> (Vec<String>, Vec<Value>) {
        let (mut installed, mut failed) = (vec![], vec![]);
        for row in self.rows() {
            let id = row.s("id");
            if !row.b("enabled") || (!only.is_empty() && !only.contains(&id.as_str())) {
                continue;
            }
            let Some(m) = catalog::find(&id) else { continue };
            if row.s("origin") != Origin::Catalog.as_str() || !program_needed(m) {
                continue;
            }
            if self.custom_program(m) {
                continue; // the owner's own copy (streamrip_bin, ENCORE_YTDLP): Encore installs none beside it
            }
            // its own install or what it adopted, else a copy of the system's (a server run without Docker)
            let own = self.program_or_adopted(&id);
            let mut candidates: Vec<ProgramAt> = own.clone().into_iter().collect();
            if self.system_copies() {
                candidates.extend(system_copies(m));
            }
            let mut runs = false;
            for p in candidates {
                let (reg, m2, at) = (self.clone(), m.clone(), p.clone());
                if let Ok(Ok(version)) = tokio::task::spawn_blocking(move || reg.program_runs(&m2, &at)).await {
                    // an adopted venv has no version folder: its card shows the version its program names
                    if p.adopted && own.as_ref() == Some(&p) && row.s("version").is_empty() {
                        if let Some(v) = version.filter(|v| !v.is_empty()) {
                            let _ = self.db().execute("UPDATE plugins SET version=? WHERE id=? AND (version IS NULL OR version='')", p![v, id.as_str()]);
                        }
                    }
                    runs = true;
                    break;
                }
            }
            if runs {
                continue;
            }
            match self.install(&id).await {
                Ok(_) => installed.push(id),
                Err(e) => {
                    tracing::warn!("plugins: installing {id} failed: {e}");
                    failed.push(json!({ "id": id, "error": e.message }));
                }
            }
        }
        (installed, failed)
    }

    /// A program the owner chose for a builtin (the streamrip_bin setting, ENCORE_YTDLP – as the old Download
    /// tools card counts them, encore_system::tools::custom_copy): it runs instead of the plugin's.
    fn custom_program(&self, m: &Manifest) -> bool {
        match m.id() {
            "streamrip" => !self.cfg().str("streamrip_bin").trim().is_empty(),
            "ytdlp" => std::env::var("ENCORE_YTDLP").is_ok_and(|v| !v.trim().is_empty()),
            _ => false,
        }
    }

    /// The versions folder of a plugin's own installs.
    pub fn versions(&self, id: &str) -> Versions {
        Versions::new(&self.folders(id).root)
    }
}
