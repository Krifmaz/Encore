//! The registry: every plugin Encore knows (the catalog it ships and what is installed), the plugins table, the
//! plugins' own settings, and the operations of the Plugins page: install, update, check, enable, disable,
//! remove, test, logs, upload and install from a URL.
//!
//! - A catalog plugin (first-party) is Encore's own code. The builtin crates register an adapter for it
//!   ([`Registry::register`]); until one is registered the registry answers for it from its manifest (installs
//!   its program, says what is installed, runs its version command as the test).
//! - An uploaded plugin, or one installed from a URL (third-party), is a [`ProcessPlugin`] in
//!   `<config>/plugins/<id>/<version>/`, run in the sandbox ([`crate::sandbox`]).
//! - A plugin program's settings are rows of the settings table named `plugin.<id>.<key>` (Encore's own
//!   settings ignore them); a builtin plugin's settings are Encore's keys, kept by `Config`.
use crate::catalog;
use crate::install::{self, Net as InstallNet, Program, Versions};
use crate::logs::Log;
use crate::manifest::{self, check_together, parse_for, Manifest, ManifestError, Origin};
use crate::plugin::{Call, Failure, Found, Plugin, Status};
use crate::process::{Folders, ProcessPlugin};
use crate::releases::{self, Hosts, Release};
use crate::sandbox::Sandbox;
use encore_config::Config;
use encore_core::fsx::{self, Step};
use encore_db::{p, Db, Row, RowExt};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

/// Ids the API's own routes take (`/api/plugins/check`, `/upload`, `/install-url`).
pub const RESERVED_IDS: [&str; 3] = ["check", "upload", "install-url"];

/// What a secret's value looks like when it is shown: set or not, never the value.
pub const MASK: &str = encore_config::MASK;

/// Why an operation was refused or failed: the words for the person, and the HTTP status that fits.
#[derive(Debug)]
pub struct Refused {
    /// the HTTP status
    pub status: u16,
    /// the words for the person
    pub message: String,
    /// a manifest's problems, each on its own
    pub problems: Vec<String>,
}

impl Refused {
    /// A refusal with this status and message.
    pub fn new(status: u16, message: impl Into<String>) -> Refused {
        Refused { status, message: message.into(), problems: vec![] }
    }
    /// As the API answers it: `{error, problems}`.
    pub fn json(&self) -> Value {
        json!({ "error": self.message, "problems": self.problems })
    }
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Refused {}

impl From<anyhow::Error> for Refused {
    fn from(e: anyhow::Error) -> Refused {
        match e.downcast::<Refused>() {
            Ok(r) => r,
            Err(e) => Refused::new(400, encore_core::fsx::explain(&e)),
        }
    }
}

impl From<ManifestError> for Refused {
    fn from(e: ManifestError) -> Refused {
        Refused { status: 422, message: e.to_string(), problems: e.problems }
    }
}

impl From<Failure> for Refused {
    fn from(f: Failure) -> Refused {
        Refused::new(400, f.message)
    }
}

impl From<fsx::StepError> for Refused {
    fn from(e: fsx::StepError) -> Refused {
        Refused::new(500, e.to_string())
    }
}

/// A result whose error is a [`Refused`].
type Result<T> = std::result::Result<T, Refused>;

/// Settings of the registry that tests change.
#[derive(Clone)]
struct Options {
    /// where the release services are
    hosts: Hosts,
    /// plain http:// is allowed (tests only)
    allow_http: bool,
    /// the uv program Python plugins are installed with, once known
    uv: Option<PathBuf>,
    /// a program the system has on PATH counts as installed (off in tests: the host's own copies)
    system_copies: bool,
}

/// The registry. Clones share it.
#[derive(Clone)]
pub struct Registry(Arc<Inner>);

/// The registry's shared state.
struct Inner {
    /// Encore's config folder (`plugins/` is in it)
    config_dir: PathBuf,
    /// the database (the `plugins` table)
    db: Db,
    /// Encore's settings
    cfg: Config,
    /// the HTTP client for downloads and release checks
    http: RwLock<reqwest::Client>,
    /// what tests change
    opts: RwLock<Options>,
    /// the sandbox level, found on first use
    sandbox: OnceLock<Sandbox>,
    /// the builtin plugins' adapters, by id
    builtins: RwLock<HashMap<String, Arc<dyn Plugin>>>,
    /// loaded plugin programs, by id: (the version folder they run from, the plugin)
    procs: Mutex<HashMap<String, (PathBuf, Arc<ProcessPlugin>)>>,
    /// what runs on a plugin now: "installing", "updating", "removing"
    busy: Mutex<HashMap<String, &'static str>>,
    /// the newest versions the last checks found
    latest: Mutex<HashMap<String, Found>>,
    /// the last status per plugin: {ok, message, signed_in, at}
    statuses: Mutex<HashMap<String, Value>>,
}

/// A source of an installed plugin program (`[[sources]]`, or the plugin's own id and name).
#[derive(Debug, Clone, PartialEq)]
pub struct PluginSource {
    /// its id in `source_order` and in requests
    pub id: String,
    /// its picker tab's id (the id unless the manifest names another)
    pub picker: String,
    /// its name in the picker
    pub name: String,
    /// the plugin that answers for it
    pub plugin: String,
    /// that plugin is switched on
    pub enabled: bool,
    /// the request kinds it searches (`[plugin] kinds`: track, album, url)
    pub kinds: Vec<String>,
    /// what it delivers is lossless
    pub lossless: bool,
}

/// Holds a plugin's busy mark until dropped.
struct Busy(Registry, String);

impl Drop for Busy {
    fn drop(&mut self) {
        self.0 .0.busy.lock().unwrap().remove(&self.1);
    }
}

/// The User-Agent of plugin downloads and release checks.
fn user_agent() -> String {
    format!("Encore/{} (+https://github.com/Krifmaz/Encore)", encore_core::VERSION)
}

impl Registry {
    /// The registry of the config folder `config_dir`.
    pub fn new(config_dir: &Path, db: Db, cfg: Config) -> Registry {
        // what an upload or a download left half-done when Encore stopped
        let _ = std::fs::remove_dir_all(config_dir.join("plugins").join(".incoming"));
        Registry(Arc::new(Inner {
            config_dir: config_dir.to_path_buf(),
            db,
            cfg,
            http: RwLock::new(crate::fetch::client(&user_agent(), false)),
            opts: RwLock::new(Options { hosts: Hosts::default(), allow_http: false, uv: None, system_copies: true }),
            sandbox: OnceLock::new(),
            builtins: RwLock::default(),
            procs: Mutex::default(),
            busy: Mutex::default(),
            latest: Mutex::default(),
            statuses: Mutex::default(),
        }))
    }

    /// Run plugins at this level instead of the one probed (tests run both levels). Before the first run only.
    pub fn set_sandbox(&self, s: Sandbox) {
        let _ = self.0.sandbox.set(s);
    }

    /// The sandbox level plugin programs run at (probed once). At limits only, a plugin runs as Encore's user:
    /// Encore makes itself non-dumpable then, so such a plugin cannot read Encore's /proc entries (its
    /// environment with the settings the environment sets, its memory, its open files).
    pub fn sandbox(&self) -> Sandbox {
        self.0
            .sandbox
            .get_or_init(|| {
                let s = Sandbox::detect();
                if !s.is_full() {
                    crate::sandbox::hide_own_proc();
                }
                s
            })
            .clone()
    }

    /// Where GitHub, PyPI and Hugging Face are (tests: stand-ins).
    pub fn set_hosts(&self, hosts: Hosts) {
        self.0.opts.write().unwrap().hosts = hosts;
    }

    /// The uv program for `[python]` installs (tests: a stand-in).
    pub fn set_uv(&self, uv: &Path) {
        self.0.opts.write().unwrap().uv = Some(uv.to_path_buf());
    }

    /// Fetch from plain-http stand-ins too (tests only; a release build never calls it).
    #[cfg(any(test, feature = "test-support"))]
    pub fn allow_plain_http(&self) {
        self.0.opts.write().unwrap().allow_http = true;
        *self.0.http.write().unwrap() = crate::fetch::client(&user_agent(), true);
    }

    /// Leave the system's own copies of the programs out ([`Registry::install_missing`]): tests, whose host may
    /// have any of them.
    #[cfg(any(test, feature = "test-support"))]
    pub fn without_system_copies(&self) {
        self.0.opts.write().unwrap().system_copies = false;
    }

    /// A program on PATH counts as installed (off in tests).
    pub(crate) fn system_copies(&self) -> bool {
        self.0.opts.read().unwrap().system_copies
    }

    /// Plain http:// is allowed (tests only).
    fn allow_http(&self) -> bool {
        self.0.opts.read().unwrap().allow_http
    }

    /// What the install step needs of the network.
    fn net(&self) -> InstallNet {
        InstallNet { http: self.0.http.read().unwrap().clone(), allow_http: self.allow_http() }
    }

    /// What release checks need of the network (with the GitHub token).
    fn release_net(&self) -> releases::Net {
        let o = self.0.opts.read().unwrap().clone();
        releases::Net { http: self.0.http.read().unwrap().clone(), allow_http: o.allow_http, hosts: o.hosts, github_token: self.0.cfg.str("github_token") }
    }

    /// The program a catalog plugin installed under `<config>/plugins/<id>` (the version `current` names): (version,
    /// program). None: not installed there (an adopted folder is the builtin adapter's to find).
    pub fn program(&self, id: &str) -> Option<(String, PathBuf)> {
        let m = catalog::find(id)?.clone();
        CatalogPlugin { m, registry: self.clone() }.program()
    }

    /// The builtin crates' adapter for a catalog plugin.
    pub fn register(&self, plugin: Arc<dyn Plugin>) {
        let id = plugin.manifest().plugin.id.clone();
        self.0.builtins.write().unwrap().insert(id, plugin);
    }

    /// The adapters registered, by id (the server's start registers one per catalog plugin).
    pub fn registered(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.0.builtins.read().unwrap().keys().cloned().collect();
        ids.sort();
        ids
    }

    /// Drop every registered adapter (a test server going away: its adapters hold a handle to it).
    pub fn unregister_all(&self) {
        self.0.builtins.write().unwrap().clear();
    }

    /// The database.
    pub fn db(&self) -> &Db {
        &self.0.db
    }

    /// Encore's settings.
    pub fn cfg(&self) -> &Config {
        &self.0.cfg
    }

    /// The config folder (`/config`).
    pub fn config_dir(&self) -> &Path {
        &self.0.config_dir
    }

    // ---- sources ------------------------------------------------------------------------------------------

    /// The plugin that answers for a source id of `source_order`, the picker or the request worker: the
    /// catalog's (`deezer`, `qobuz`, `tidal` and the worker's `streamrip` → streamrip; `ytmusic` → ytdlp;
    /// `slskd_lossless` → slskd), else an installed plugin program's (`[[sources]]`, or its own id).
    pub fn source_plugin(&self, source: &str) -> Option<String> {
        match source {
            "streamrip" => return Some("streamrip".into()),
            "slskd_lossless" => return Some("slskd".into()),
            _ => {}
        }
        if let Some(m) = catalog::for_source(source) {
            return Some(m.id().to_string());
        }
        self.plugin_sources().into_iter().find(|s| s.id == source || s.picker == source).map(|s| s.plugin)
    }

    /// The sources of the installed plugin programs, and the catalog's that go through the registry
    /// ([`catalog::THROUGH_REGISTRY`]: zotify, gamdl), in the Plugins page's order.
    pub fn plugin_sources(&self) -> Vec<PluginSource> {
        let rows: HashMap<String, Row> = self.rows().into_iter().map(|r| (r.s("id"), r)).collect();
        self.known()
            .into_iter()
            .filter(|m| (!m.plugin.builtin && catalog::find(m.id()).is_none()) || catalog::THROUGH_REGISTRY.contains(&m.id()))
            .flat_map(|m| {
                let enabled = rows.get(m.id()).is_some_and(|r| r.b("enabled"));
                let (kinds, lossless) = (m.plugin.kinds.clone(), m.plugin.lossless);
                m.source_ids().into_iter().map(move |s| PluginSource {
                    picker: s.picker.clone().unwrap_or_else(|| s.id.clone()),
                    id: s.id,
                    name: s.name,
                    plugin: m.id().to_string(),
                    enabled,
                    kinds: kinds.clone(),
                    lossless,
                })
            })
            .collect()
    }

    /// A source is ready to be asked: its plugin is installed and switched on.
    pub fn source_enabled(&self, source: &str) -> bool {
        self.source_plugin(source).is_some_and(|p| self.enabled(&p))
    }

    // ---- where things are -------------------------------------------------------------------------------

    /// `<config>/plugins`
    pub fn root(&self) -> PathBuf {
        self.0.config_dir.join("plugins")
    }

    /// `<config>/plugins/<id>`
    pub fn folders(&self, id: &str) -> Folders {
        Folders::new(&self.root().join(id))
    }

    /// The plugin's log (the running program's own, else one over its folder).
    pub fn log(&self, id: &str) -> Log {
        if let Some((_, p)) = self.0.procs.lock().unwrap().get(id) {
            return p.log().clone();
        }
        Log::new(&self.folders(id).logs())
    }

    // ---- the table ----------------------------------------------------------------------------------------

    /// The plugin's row.
    pub fn row(&self, id: &str) -> Option<Row> {
        self.0.db.one("SELECT * FROM plugins WHERE id=?", p![id]).ok().flatten()
    }

    /// Every plugin row, by id.
    pub fn rows(&self) -> Vec<Row> {
        self.0.db.query("SELECT * FROM plugins ORDER BY id", p![]).unwrap_or_default()
    }

    /// A plugin is installed (it has a row).
    pub fn installed(&self, id: &str) -> bool {
        self.row(id).is_some()
    }

    /// Installed and switched on.
    pub fn enabled(&self, id: &str) -> bool {
        self.row(id).is_some_and(|r| r.b("enabled"))
    }

    /// Where a row's plugin came from (the catalog when the row doesn't say).
    fn origin_of(row: &Row) -> Origin {
        Origin::parse(&row.s("origin")).unwrap_or(Origin::Catalog)
    }

    /// The manifest of a plugin: an installed program's own (as installed), else the catalog's.
    pub fn manifest(&self, id: &str) -> Option<Manifest> {
        if let Some(row) = self.row(id) {
            let origin = Self::origin_of(&row);
            if origin != Origin::Catalog {
                return parse_for(&row.s("manifest"), origin).ok();
            }
        }
        catalog::find(id).cloned()
    }

    /// Every plugin Encore knows, in the Plugins page's order: the catalog's, then the installed others by name.
    pub fn known(&self) -> Vec<Manifest> {
        let mut out: Vec<Manifest> = catalog::catalog().to_vec();
        let mut others: Vec<Manifest> = self
            .rows()
            .iter()
            .filter(|r| Self::origin_of(r) != Origin::Catalog)
            .filter_map(|r| parse_for(&r.s("manifest"), Self::origin_of(r)).ok())
            .filter(|m| catalog::find(m.id()).is_none())
            .collect();
        others.sort_by_key(|m| m.plugin.name.to_lowercase());
        out.extend(others);
        out
    }

    /// The plugin that answers for `id`: a builtin's adapter, an installed program, or the catalog's manifest.
    pub fn plugin(&self, id: &str) -> Option<Arc<dyn Plugin>> {
        if let Some(b) = self.0.builtins.read().unwrap().get(id) {
            return Some(b.clone());
        }
        let row = self.row(id);
        if let Some(row) = row.as_ref().filter(|r| Self::origin_of(r) != Origin::Catalog) {
            return self.process(id, row).map(|p| p as Arc<dyn Plugin>);
        }
        let m = catalog::find(id)?.clone();
        Some(Arc::new(CatalogPlugin { m, registry: self.clone() }))
    }

    /// The installed program of an uploaded or URL plugin, loaded once per version.
    fn process(&self, id: &str, row: &Row) -> Option<Arc<ProcessPlugin>> {
        let m = parse_for(&row.s("manifest"), Self::origin_of(row)).ok()?;
        let folders = self.folders(id);
        let version_dir = folders.version(&Versions::new(&folders.root).current().unwrap_or_else(|| row.s("version")));
        let mut procs = self.0.procs.lock().unwrap();
        if let Some((dir, p)) = procs.get(id) {
            if *dir == version_dir {
                return Some(p.clone());
            }
        }
        let p = Arc::new(ProcessPlugin::new(m.clone(), &folders.root, &version_dir, self.sandbox(), self.read_only(&m)));
        procs.insert(id.to_string(), (version_dir, p.clone()));
        Some(p)
    }

    /// The folders a plugin may read besides its own (`[permissions] paths` with `access = "read"`).
    fn read_only(&self, m: &Manifest) -> Vec<PathBuf> {
        m.permissions
            .paths
            .iter()
            .filter(|g| g.access == "read")
            .filter_map(|g| match (&g.setting, &g.config) {
                (Some(k), _) => Some(PathBuf::from(self.0.cfg.str(k))).filter(|p| p.is_absolute()),
                (_, Some(c)) => Some(self.0.config_dir.join(c)),
                _ => None,
            })
            .collect()
    }

    // ---- settings -------------------------------------------------------------------------------------------

    /// The values a call carries: a builtin plugin's Encore settings, or a program's own (`plugin.<id>.<key>`,
    /// the manifest's default for a key never saved). Secrets included: they go to the plugin on stdin only.
    pub fn values(&self, m: &Manifest) -> Map<String, Value> {
        let mut out = Map::new();
        if m.plugin.builtin {
            for s in &m.settings {
                out.insert(s.key.clone(), self.0.cfg.get(&s.key));
            }
            return out;
        }
        let saved: HashMap<String, Value> = self
            .0
            .db
            .query("SELECT key, value FROM settings WHERE key LIKE ?", p![format!("plugin.{}.%", m.id())])
            .unwrap_or_default()
            .into_iter()
            .filter_map(|r| Some((r.s("key"), serde_json::from_str::<Value>(&r.s("value")).ok()?)))
            .collect();
        for s in &m.settings {
            let v = saved.get(&m.store_key(s)).cloned().or_else(|| s.default.clone()).unwrap_or(Value::Null);
            out.insert(s.key.clone(), v);
        }
        out
    }

    /// The call of a command: the plugin's settings.
    pub fn call(&self, m: &Manifest) -> Call {
        Call::with_settings(self.values(m))
    }

    /// The settings card of a plugin: its fields (a builtin's from the settings table) and their values, secrets
    /// masked.
    pub fn settings(&self, id: &str) -> Result<Value> {
        let m = self.manifest(id).ok_or_else(|| Refused::new(404, "Unknown plugin"))?;
        let values = self.values(&m);
        let mut fields = vec![];
        let mut shown = Map::new();
        for s in &m.settings {
            let mut f = field_json(&m, s, &self.0.cfg);
            if f.is_null() {
                continue; // an internal key: the plugin keeps it, the card does not draw it
            }
            let secret = f["secret"].as_bool().unwrap_or(false);
            let v = values.get(&s.key).cloned().unwrap_or(Value::Null);
            let set = !(v.is_null() || v.as_str() == Some(""));
            shown.insert(s.key.clone(), if secret { json!(if set { format!("{MASK} (saved)") } else { String::new() }) } else { v });
            f["store_key"] = json!(m.store_key(s));
            fields.push(f);
        }
        Ok(json!({ "id": m.id(), "builtin": m.plugin.builtin, "settings_on": m.plugin.settings_on, "fields": fields, "values": shown }))
    }

    /// Save a plugin's settings (`values`: key → value; a masked secret is left as it is). A builtin's go to
    /// Encore's settings (a key the environment sets is left as it is), a program's to `plugin.<id>.<key>`.
    pub fn save_settings(&self, id: &str, values: &Map<String, Value>) -> Result<Value> {
        let m = self.manifest(id).ok_or_else(|| Refused::new(404, "Unknown plugin"))?;
        let mut clean = Map::new();
        for (k, v) in values {
            let s = m.setting(k).ok_or_else(|| Refused::new(400, format!("{} has no setting {k}", m.plugin.name)))?;
            if v.as_str().is_some_and(|t| t.starts_with(MASK)) {
                continue;
            }
            if m.plugin.builtin {
                if self.0.cfg.def(k).is_none() || self.0.cfg.env_locked(k) || k == "api_key" {
                    continue;
                }
                clean.insert(k.clone(), v.clone());
            } else {
                clean.insert(m.store_key(s), coerce(s, v).map_err(|e| Refused::new(400, e))?);
            }
        }
        if m.plugin.builtin {
            self.0.cfg.set_many(&Value::Object(clean));
        } else {
            for (k, v) in &clean {
                self.0.db.execute("INSERT OR REPLACE INTO settings (key, value) VALUES (?,?)", p![k, v.to_string()]).map_err(|e| Refused::new(500, e.to_string()))?;
            }
        }
        self.settings(id)
    }

    // ---- the cards ------------------------------------------------------------------------------------------

    /// GET /api/plugins: the sandbox level and every plugin's card.
    pub fn list(&self) -> Value {
        let rows: HashMap<String, Row> = self.rows().into_iter().map(|r| (r.s("id"), r)).collect();
        let cards: Vec<Value> = self.known().iter().map(|m| self.card_of(m, rows.get(m.id()))).collect();
        json!({ "sandbox": self.sandbox().json(), "encore_version": encore_core::VERSION, "protocol": manifest::PROTOCOL,
                "upload_max_mb": manifest::UPLOAD_MAX_BYTES >> 20, "plugins": cards })
    }

    /// One plugin's card.
    pub fn card(&self, id: &str) -> Option<Value> {
        let m = self.manifest(id)?;
        Some(self.card_of(&m, self.row(id).as_ref()))
    }

    /// The plugin's card for the Plugins page: manifest, row, state, latest version, settings.
    fn card_of(&self, m: &Manifest, row: Option<&Row>) -> Value {
        let id = m.id();
        let origin = row.map(Self::origin_of).unwrap_or(Origin::Catalog);
        let third = origin != Origin::Catalog;
        let installed_version = row.map(|r| r.s("version")).filter(|v| !v.is_empty());
        let latest = self.0.latest.lock().unwrap().get(id).cloned();
        let update_available = match (&latest, &installed_version) {
            (Some(l), Some(i)) => encore_core::version::newer(&l.version, i),
            _ => false,
        };
        let sandbox = self.sandbox();
        let warning = third.then(|| {
            if sandbox.is_full() {
                "Third-party plugin: not made by Encore. It runs in full isolation: it sees only its own folders, and the network only when its plugin.toml asks for it (the line below says what it can reach). Install only plugins you trust."
            } else {
                "Third-party plugin: not made by Encore. This server can only set limits on it (see the compose option in the README): it runs as Encore's user and can read what Encore can. Install only plugins you trust."
            }
        });
        let reads_library = m.permissions.paths.iter().any(|g| g.setting.as_deref() == Some("library_path"));
        // a catalog plugin's manifest version is Encore's, not its program's: the card shows the program's (the
        // installed one, else the newest known), or none
        let version = if m.plugin.builtin { installed_version.clone().or_else(|| latest.as_ref().map(|l| l.version.clone())) } else { Some(m.plugin.version.clone()) };
        json!({
            "id": id,
            "name": m.plugin.name,
            "description": m.plugin.description,
            "kind": m.plugin.kind,
            "version": version,
            "installed": row.is_some(),
            "installed_version": installed_version,
            "enabled": row.is_some_and(|r| r.b("enabled")),
            "origin": origin.as_str(),
            "trust": origin.trust(),
            "builtin": m.plugin.builtin,
            "warning": warning,
            "auto_update": row.map(|r| r.b("auto_update")).unwrap_or(m.update.as_ref().is_some_and(|u| u.auto)),
            "can_update": m.update.as_ref().is_some_and(|u| u.source() != "none"),
            "program": m.installs_program() || m.is_process(),
            "latest": latest.map(|l| l.version),
            "update_available": update_available,
            "last_check": row.and_then(|r| r.get("last_check").and_then(Value::as_f64)),
            "last_error": row.map(|r| r.s("last_error")).filter(|e| !e.is_empty()),
            "installed_at": row.and_then(|r| r.get("installed_at").and_then(Value::as_f64)),
            "updated_at": row.and_then(|r| r.get("updated_at").and_then(Value::as_f64)),
            "status": self.0.statuses.lock().unwrap().get(id).cloned(),
            "busy": self.0.busy.lock().unwrap().get(id).copied(),
            "sandbox": sandbox.text(),
            "network": m.permissions.network.allowed(),
            "hosts": m.permissions.network.hosts(),
            "reads_library": reads_library,
            "commands": m.plugin.commands,
            "kinds": m.plugin.kinds,
            "lossless": m.plugin.lossless,
            "needs_sign_in": m.plugin.needs_sign_in,
            "sources": m.source_ids().iter().map(|s| json!({ "id": s.id, "name": s.name, "picker": s.picker })).collect::<Vec<_>>(),
            "settings_on": m.plugin.settings_on,
            "has_settings": m.settings.iter().any(|s| !field_json(m, s, &self.0.cfg).is_null()),
            "homepage": m.plugin.homepage,
            "license": m.plugin.license,
            "min_encore": m.plugin.min_encore,
            "migration": m.migration.as_ref().and_then(|g| g.from_tool.clone()),
        })
    }

    /// What runs on a plugin now ("installing", "updating", "removing"), if anything.
    pub fn busy_with(&self, id: &str) -> Option<&'static str> {
        self.0.busy.lock().unwrap().get(id).copied()
    }

    /// Mark `id` busy with `what` until the guard drops; refused while it is busy with something else.
    fn busy(&self, id: &str, what: &'static str) -> Result<Busy> {
        let mut b = self.0.busy.lock().unwrap();
        if let Some(now) = b.get(id) {
            return Err(Refused::new(409, format!("{id} is {now} already – wait for it to finish")));
        }
        b.insert(id.to_string(), what);
        Ok(Busy(self.clone(), id.to_string()))
    }

    /// Set or clear the plugin's last error.
    fn set_error(&self, id: &str, e: Option<&str>) {
        let _ = self.0.db.execute("UPDATE plugins SET last_error=? WHERE id=?", p![e, id]);
    }

    // ---- status and test --------------------------------------------------------------------------------------

    /// Run `status` (an installed plugin) and keep the answer for the cards.
    pub async fn status(&self, id: &str) -> Result<Value> {
        let m = self.manifest(id).ok_or_else(|| Refused::new(404, "Unknown plugin"))?;
        let plugin = self.plugin(id).ok_or_else(|| Refused::new(404, "Unknown plugin"))?;
        let v = if !self.installed(id) {
            json!({ "ok": false, "message": "Not installed", "signed_in": null })
        } else {
            match plugin.status(&self.call(&m)).await {
                Ok(Status { message, signed_in }) => json!({ "ok": true, "message": message, "signed_in": signed_in }),
                Err(f) => json!({ "ok": false, "message": f.message, "code": f.code.as_str(), "signed_in": if f.code == crate::plugin::Code::Auth { json!(false) } else { Value::Null } }),
            }
        };
        let mut v = v;
        v["at"] = json!(encore_db::now());
        self.0.statuses.lock().unwrap().insert(id.to_string(), v.clone());
        Ok(v)
    }

    /// The card's Test button.
    pub async fn test(&self, id: &str) -> Result<Value> {
        let m = self.manifest(id).ok_or_else(|| Refused::new(404, "Unknown plugin"))?;
        if !self.installed(id) {
            return Err(Refused::new(400, format!("{} is not installed", m.plugin.name)));
        }
        let plugin = self.plugin(id).ok_or_else(|| Refused::new(404, "Unknown plugin"))?;
        let log = self.log(id);
        match plugin.test(&self.call(&m)).await {
            Ok(message) => {
                log.line("test", &format!("passed: {message}"));
                Ok(json!({ "ok": true, "message": message }))
            }
            Err(f) => {
                log.line("test", &format!("failed: {}", f.message));
                Ok(f.json())
            }
        }
    }

    /// A source's search through the plugin (installed and enabled).
    pub async fn search(&self, id: &str, request: &Value, call: Option<Call>) -> std::result::Result<Vec<Value>, Failure> {
        let (m, plugin) = self.ready(id)?;
        let call = call.map(|c| Call { settings: self.values(&m), ..c }).unwrap_or_else(|| self.call(&m));
        plugin.search(&call, request).await
    }

    /// A source's download through the plugin into `folder`; the folder is removed after a failure or a cancel.
    pub async fn download(&self, id: &str, request: &Value, folder: &Path, call: Option<Call>) -> std::result::Result<crate::plugin::Downloaded, Failure> {
        let (m, plugin) = self.ready(id)?;
        let call = call.map(|c| Call { settings: self.values(&m), ..c }).unwrap_or_else(|| self.call(&m));
        let r = plugin.download(&call, request, folder).await;
        if r.is_err() {
            let _ = std::fs::remove_dir_all(folder);
        }
        r
    }

    /// The manifest and adapter of a plugin that is installed and switched on.
    fn ready(&self, id: &str) -> std::result::Result<(Manifest, Arc<dyn Plugin>), Failure> {
        let m = self.manifest(id).ok_or_else(|| Failure::internal(format!("no plugin {id}")))?;
        if !self.enabled(id) {
            return Err(Failure::unsupported(format!("{} is not installed or switched off", m.plugin.name)));
        }
        let plugin = self.plugin(id).ok_or_else(|| Failure::internal(format!("no plugin {id}")))?;
        Ok((m, plugin))
    }

    /// The last `n` lines of a plugin's log.
    pub fn logs(&self, id: &str, n: usize) -> Result<Value> {
        if self.manifest(id).is_none() {
            return Err(Refused::new(404, "Unknown plugin"));
        }
        let log = self.log(id);
        Ok(json!({ "id": id, "lines": log.tail(n.clamp(1, 2000)), "file": log.path() }))
    }

    // ---- switching on and off -------------------------------------------------------------------------------

    /// Switch the plugin on or off → its card.
    pub async fn enable(&self, id: &str, on: bool) -> Result<Value> {
        let m = self.manifest(id).ok_or_else(|| Refused::new(404, "Unknown plugin"))?;
        if !self.installed(id) {
            return Err(Refused::new(400, format!("{} is not installed", m.plugin.name)));
        }
        self.0.db.execute("UPDATE plugins SET enabled=?, updated_at=updated_at WHERE id=?", p![on as i64, id]).map_err(|e| Refused::new(500, e.to_string()))?;
        if let Some(plugin) = self.plugin(id) {
            plugin.enable(on).await?;
        }
        self.log(id).line(if on { "enable" } else { "disable" }, if on { "switched on" } else { "switched off" });
        Ok(self.card(id).unwrap_or(Value::Null))
    }

    /// Turn automatic updates of the plugin on or off → its card.
    pub fn set_auto_update(&self, id: &str, on: bool) -> Result<Value> {
        if !self.installed(id) {
            return Err(Refused::new(400, "Not installed"));
        }
        self.0.db.execute("UPDATE plugins SET auto_update=? WHERE id=?", p![on as i64, id]).map_err(|e| Refused::new(500, e.to_string()))?;
        // the old apps' Download tools switch (`tools_auto_<id>`) shows the same, for one more release
        if let Some(k) = self.manifest(id).and_then(|m| m.migration).and_then(|g| g.auto_key) {
            if self.0.cfg.def(&k).is_some() && !self.0.cfg.env_locked(&k) {
                self.0.cfg.set_many(&json!({ k: on }));
            }
        }
        Ok(self.card(id).unwrap_or(Value::Null))
    }

    /// Settings saved through PUT /api/admin/settings: an old `tools_auto_<id>` switch (the 0.30 apps' Download
    /// tools card) becomes its installed plugin's auto-update, as the plugin's own switch does.
    pub fn follow_auto_keys(&self, saved: &[&str]) {
        for m in catalog::catalog() {
            let Some(k) = m.migration.as_ref().and_then(|g| g.auto_key.as_deref()) else { continue };
            if saved.contains(&k) && self.installed(m.id()) {
                let _ = self.0.db.execute("UPDATE plugins SET auto_update=? WHERE id=?", p![self.0.cfg.bool(k) as i64, m.id()]);
            }
        }
    }

    // ---- removing ---------------------------------------------------------------------------------------------

    /// Remove a plugin: stop what runs, delete its folder under `<config>/plugins` (its data and log too), its
    /// row and, for a plugin program, its settings. A catalog plugin keeps Encore's settings and the folders it
    /// adopted (the previous image may still use them).
    pub async fn remove(&self, id: &str) -> Result<Value> {
        let m = self.manifest(id).ok_or_else(|| Refused::new(404, "Unknown plugin"))?;
        if !self.installed(id) {
            return Err(Refused::new(400, format!("{} is not installed", m.plugin.name)));
        }
        let _b = self.busy(id, "removing")?;
        if let Some(plugin) = self.plugin(id) {
            plugin.removing().await;
        }
        let root = self.folders(id).root;
        if root.exists() {
            fsx::remove_dir_all(&root, Step::Plugin)?;
        }
        self.0.db.execute("DELETE FROM plugins WHERE id=?", p![id]).map_err(|e| Refused::new(500, e.to_string()))?;
        if !m.plugin.builtin {
            let _ = self.0.db.execute("DELETE FROM settings WHERE key LIKE ?", p![format!("plugin.{id}.%")]);
        }
        self.0.procs.lock().unwrap().remove(id);
        self.0.statuses.lock().unwrap().remove(id);
        self.0.latest.lock().unwrap().remove(id);
        self.0.db.log("info", &format!("Plugin {} removed", m.plugin.name));
        Ok(self.card(id).unwrap_or(json!({ "id": id, "removed": true })))
    }

    // ---- checking for updates -----------------------------------------------------------------------------------

    /// Look for a new version of one plugin (`[update]`, or the plugin's own check).
    pub async fn check(&self, id: &str) -> Result<Value> {
        let m = self.manifest(id).ok_or_else(|| Refused::new(404, "Unknown plugin"))?;
        let found = self.find_newest(&m).await;
        let now = encore_db::now();
        let _ = self.0.db.execute("UPDATE plugins SET last_check=? WHERE id=?", p![now, id]);
        match found {
            Ok(Some(f)) => {
                self.0.latest.lock().unwrap().insert(id.to_string(), f);
                self.set_error(id, None);
            }
            Ok(None) => {
                self.set_error(id, None);
            }
            Err(e) => {
                let msg = format!("Could not check for a new version: {}", e.message);
                self.set_error(id, Some(&msg));
                self.log(id).line("check", &msg);
            }
        }
        Ok(self.card(id).unwrap_or(Value::Null))
    }

    /// Check every installed plugin that has an update source.
    pub async fn check_all(&self) -> Value {
        for r in self.rows() {
            let id = r.s("id");
            if self.manifest(&id).is_some_and(|m| m.update.as_ref().is_some_and(|u| u.source() != "none")) {
                let _ = self.check(&id).await;
            }
        }
        self.list()
    }

    /// The newest release: the plugin's own check, else its `[update]` source.
    async fn find_newest(&self, m: &Manifest) -> Result<Option<Found>> {
        if let Some(plugin) = self.plugin(m.id()) {
            if let Some(r) = plugin.check_update(&self.call(m)).await {
                return Ok(Some(r?));
            }
        }
        let Some(u) = m.update.as_ref().filter(|u| u.source() != "none") else { return Ok(None) };
        let net = self.release_net();
        match u.source() {
            "pypi" => {
                let project = u.pypi.as_deref().unwrap_or_default();
                let v = releases::pypi_latest(&net, project).await?;
                Ok(Some(Found { tag: v.clone(), version: v }))
            }
            _ => {
                let arch = std::env::consts::ARCH;
                let m2 = m.clone();
                let has = move |r: &Release| -> bool {
                    if m2.python.is_some() {
                        true
                    } else if !m2.binaries.is_empty() {
                        releases::binary_asset(&m2, r, arch).is_some()
                    } else if let Some(a) = m2.update.as_ref().and_then(|u| u.asset.as_ref()) {
                        r.asset(&manifest::fill(a, &r.version, &r.tag)).is_some()
                    } else {
                        true
                    }
                };
                Ok(releases::newest(&net, u, &has).await?.map(|r| Found { version: r.version, tag: r.tag }))
            }
        }
    }

    // ---- installing a catalog plugin --------------------------------------------------------------------------

    /// Install: a catalog plugin (its program, pinned to the version this Encore was tested with, or the newest),
    /// or an installed program's newest release. The plugin is switched on.
    pub async fn install(&self, id: &str) -> Result<Value> {
        let m = self.manifest(id).ok_or_else(|| Refused::new(404, "Unknown plugin"))?;
        if let Some(row) = self.row(id).filter(|r| Self::origin_of(r) != Origin::Catalog) {
            let _ = row;
            return self.update(id).await;
        }
        if catalog::find(id).is_none() {
            return Err(Refused::new(404, "Unknown plugin"));
        }
        let _b = self.busy(id, "installing")?;
        let res = self.install_catalog(&m, true).await;
        self.after_install(&m, res, "install").await
    }

    /// Update: the newest version (a catalog program, or a program's next release).
    pub async fn update(&self, id: &str) -> Result<Value> {
        self.update_with(id, false).await
    }

    /// The update the scheduled task makes by itself: a third-party release that asks for more than the
    /// installed one (the network, hosts, folders, limits, another update place) is refused (409) and kept for
    /// the admin, whose Update installs it.
    pub async fn update_unattended(&self, id: &str) -> Result<Value> {
        self.update_with(id, true).await
    }

    /// Update one plugin: a catalog one reinstalls its program, another downloads its newest release. `unattended`: the scheduled update.
    async fn update_with(&self, id: &str, unattended: bool) -> Result<Value> {
        let m = self.manifest(id).ok_or_else(|| Refused::new(404, "Unknown plugin"))?;
        let row = self.row(id).ok_or_else(|| Refused::new(400, format!("{} is not installed", m.plugin.name)))?;
        let origin = Self::origin_of(&row);
        if origin == Origin::Catalog {
            let _b = self.busy(id, "updating")?;
            let res = self.install_catalog(&m, false).await;
            return self.after_install(&m, res, "update").await;
        }
        let u = m.update.clone().filter(|u| u.source() == "github" && u.asset.is_some()).ok_or_else(|| Refused::new(400, format!("{} names no place to update from", m.plugin.name)))?;
        let found = self.find_newest(&m).await?;
        let Some(found) = found else { return Err(Refused::new(400, format!("{} has no release to install", m.plugin.name))) };
        self.0.latest.lock().unwrap().insert(id.to_string(), found.clone());
        let installed = row.s("version");
        if !encore_core::version::newer(&found.version, &installed) {
            let _ = self.0.db.execute("UPDATE plugins SET last_check=? WHERE id=?", p![encore_db::now(), id]);
            return Ok(self.card(id).unwrap_or(Value::Null));
        }
        let net = self.release_net();
        let release = releases::by_tag(&net, &u, &found.tag).await?;
        let asset = manifest::fill(u.asset.as_deref().unwrap_or_default(), &release.version, &release.tag);
        let a = release.asset(&asset).ok_or_else(|| Refused::new(400, format!("the {} release has no {asset}", release.tag)))?;
        let sums = u.sums.as_deref().ok_or_else(|| Refused::new(400, format!("{} names no sums for its releases", m.plugin.name)))?;
        let want = releases::sha256_of(&net, &release, &asset, sums).await?;
        let scratch = self.scratch()?;
        let zip = scratch.join(&asset);
        let res: Result<Value> = async {
            let got = crate::fetch::to_file(&self.net().http, &a.url, &zip, manifest::UPLOAD_MAX_BYTES, std::time::Duration::from_secs(1800), self.allow_http()).await?;
            install::check_sha256(&zip, &got.sha256, &want, &asset)?;
            self.install_archive_with(&zip, origin, Some(got.sha256), None, Some(id), unattended).await
        }
        .await;
        let _ = std::fs::remove_dir_all(&scratch);
        if let Err(e) = &res {
            self.set_error(id, Some(&format!("Update failed: {}", e.message)));
            self.log(id).line("update", &format!("failed: {}", e.message));
        }
        res
    }

    /// After an install or update (`what`): run the plugin's `installed` hook and log, or record and log the error.
    async fn after_install(&self, m: &Manifest, res: std::result::Result<Option<String>, Refused>, what: &str) -> Result<Value> {
        let id = m.id();
        match res {
            Ok(version) => {
                if let Some(plugin) = self.plugin(id) {
                    if let Err(f) = plugin.installed(&self.call(m), version.as_deref()).await {
                        self.log(id).line(what, &format!("after the {what}: {}", f.message));
                    }
                }
                self.0.db.log("info", &format!("Plugin {} {}{}", m.plugin.name, if what == "install" { "installed" } else { "updated" }, version.map(|v| format!(" ({v})")).unwrap_or_default()));
                Ok(self.card(id).unwrap_or(Value::Null))
            }
            Err(e) => {
                if self.installed(id) {
                    self.set_error(id, Some(&format!("{} failed: {}", if what == "install" { "Install" } else { "Update" }, e.message)));
                }
                self.log(id).line(what, &format!("failed: {}", e.message));
                Err(e)
            }
        }
    }

    /// Install a catalog plugin's program (none: only the row) → the version installed.
    async fn install_catalog(&self, m: &Manifest, pinned: bool) -> std::result::Result<Option<String>, Refused> {
        let id = m.id();
        let versions = Versions::new(&self.folders(id).root);
        let mut version: Option<String> = None;
        let mut sha = None;
        if m.installs_program() {
            let u = m.update.clone().unwrap_or_default();
            let net = self.release_net();
            let (tag, ver) = match (pinned, &u.pin) {
                (true, Some(pin)) => (pin.clone(), u.version_in_tag(pin).unwrap_or_else(|| pin.trim_start_matches(['v', 'V']).to_string())),
                _ => {
                    let f = self.find_newest(m).await?.ok_or_else(|| Refused::new(400, format!("{} has no release for this server", m.plugin.name)))?;
                    (f.tag, f.version)
                }
            };
            if let Some(cur) = versions.current().filter(|c| *c == ver) {
                version = Some(cur);
            } else if m.python.is_some() {
                let uv = self.0.opts.read().unwrap().uv.clone().or_else(install::find_uv).ok_or_else(|| Refused::new(400, "uv is not installed on this server, so Python programs cannot be installed"))?;
                // the pinned release must call itself what it did when Encore was tested with it ([update] pin_reports)
                let reports = if pinned && u.pin.as_deref() == Some(tag.as_str()) { u.pin_reports.as_deref() } else { None };
                let inst = install::install_python_reporting(m, &versions, &ver, &tag, &uv, reports).await?;
                version = Some(inst.version);
            } else {
                let arch = std::env::consts::ARCH;
                let b = m.binary_for(arch).ok_or_else(|| Refused::new(400, format!("{} has no program for this server ({arch})", m.plugin.name)))?;
                let (url, name, want) = match (&b.url, &b.asset) {
                    (Some(url), _) => {
                        let name = url.rsplit('/').next().unwrap_or("download").split(['?', '#']).next().unwrap_or("download").to_string();
                        let want = match (&b.sha256, b.sums.as_deref().or(u.sums.as_deref())) {
                            (Some(s), _) => s.clone(),
                            (None, Some(s)) if s.starts_with("https://") || s.starts_with("http://") => {
                                let text = crate::fetch::text(&net.http, s, 1 << 20, net.allow_http).await?;
                                install::sum_for(&text, &name)?
                            }
                            _ => return Err(Refused::new(400, format!("{} names no sha256 for its program", m.plugin.name))),
                        };
                        (manifest::fill(url, &ver, &tag), name, want)
                    }
                    _ => {
                        let release = releases::by_tag(&net, &u, &tag).await?;
                        let a = releases::binary_asset(m, &release, arch).ok_or_else(|| Refused::new(400, format!("the {tag} release has no program for this server ({arch})")))?;
                        let want = match (&b.sha256, b.sums.as_deref().or(u.sums.as_deref())) {
                            (Some(s), _) => s.clone(),
                            (None, Some(rule)) => releases::sha256_of(&net, &release, &a.name, rule).await?,
                            (None, None) => return Err(Refused::new(400, format!("{} names no sha256 for its program", m.plugin.name))),
                        };
                        (a.url.clone(), a.name.clone(), want)
                    }
                };
                let unpack = b.unpack_for(&name).unwrap_or("file").to_string();
                let program = Program {
                    url,
                    name,
                    sha256: Some(want),
                    unpack,
                    bin: b.bin.clone(),
                    remove: m.install.as_ref().map(|i| i.remove.clone()).unwrap_or_default(),
                    max_bytes: m.max_bytes(),
                };
                let inst = install::install_program(&self.net(), m, &versions, &ver, &program, |_| Ok(())).await?;
                sha = Some(inst.sha256);
                version = Some(inst.version);
            }
        }
        self.fetch_data_files(m).await?;
        let now = encore_db::now();
        let auto = m.update.as_ref().is_some_and(|u| u.auto) as i64;
        self.0
            .db
            .execute(
                "INSERT INTO plugins (id, version, enabled, installed_at, updated_at, origin, trust, manifest, sha256, auto_update, last_check, last_error)
                 VALUES (?, ?, 1, ?, ?, 'catalog', 'first-party', ?, ?, ?, NULL, NULL)
                 ON CONFLICT(id) DO UPDATE SET version=excluded.version, enabled=1, updated_at=excluded.updated_at, manifest=excluded.manifest,
                   sha256=COALESCE(excluded.sha256, plugins.sha256), last_error=NULL",
                p![id, version, now, now, catalog::text(id).unwrap_or_default(), sha, auto],
            )
            .map_err(|e| Refused::new(500, e.to_string()))?;
        self.log(id).line("install", &format!("installed{}", version.as_ref().map(|v| format!(" {v}")).unwrap_or_default()));
        Ok(version)
    }

    /// `[[download]]` files whose rule holds now (a model chosen by a setting); the files of other values go.
    async fn fetch_data_files(&self, m: &Manifest) -> Result<()> {
        if m.downloads.is_empty() {
            return Ok(());
        }
        let data = self.folders(m.id()).data();
        let net = self.release_net();
        let facts = CfgFacts { cfg: &self.0.cfg, dir: &self.0.config_dir };
        for d in &m.downloads {
            let Ok(Some(rel)) = crate::archive::entry_path(&d.path) else { continue };
            let dest = data.join(rel);
            let wanted = d.when.as_deref().is_none_or(|w| manifest::Condition::parse(w).is_ok_and(|c| c.holds(&facts)));
            if !wanted {
                let _ = std::fs::remove_file(&dest);
                continue;
            }
            let want = match (&d.sha256, d.sums.as_deref()) {
                (Some(s), _) => Some(s.clone()),
                (None, Some("huggingface")) => releases::huggingface_sha256(&net, &d.url).await,
                (None, Some(s)) if s.starts_with("https://") => {
                    let text = crate::fetch::text(&net.http, s, 1 << 20, net.allow_http).await?;
                    Some(install::sum_for(&text, d.path.rsplit('/').next().unwrap_or(&d.path))?)
                }
                _ => None,
            };
            if want.is_none() && d.magic.is_none() {
                return Err(Refused::new(400, format!("{}: no sha256 is known for {}", m.plugin.name, d.path)));
            }
            let max = d.max_mb.unwrap_or(manifest::DEFAULT_MAX_MB) << 20;
            install::data_file(&self.net(), &d.url, &dest, want.as_deref(), d.magic.as_deref(), max).await?;
        }
        Ok(())
    }

    // ---- uploads and URLs -------------------------------------------------------------------------------------

    /// A new empty folder for a download or an upload: `<config>/plugins/.incoming/<random>`.
    pub fn scratch(&self) -> std::result::Result<PathBuf, fsx::StepError> {
        let d = self.root().join(".incoming").join(encore_core::ids::random_hex(8));
        fsx::mkdir_all(&d, Step::Plugin)?;
        Ok(d)
    }

    /// Install an uploaded zip (`file`, at most 200 MB, already on disk; removed afterwards).
    pub async fn upload(&self, file: &Path) -> Result<Value> {
        let res = self.install_archive(file, Origin::Upload, None, None, None).await;
        let _ = std::fs::remove_file(file);
        res
    }

    /// Install from a URL: an https address of a plugin zip, of a raw `plugin.toml` whose `[update]` names the zip,
    /// or `owner/repo@ref` (GitHub's archive zip of that ref; `owner/repo/sub/folder@ref` for a plugin in a
    /// folder of the repository).
    pub async fn install_url(&self, input: &str) -> Result<Value> {
        let input = input.trim();
        if input.is_empty() {
            return Err(Refused::new(400, "Give an https address or owner/repo@ref"));
        }
        let scratch = self.scratch()?;
        let res = self.install_url_in(input, &scratch).await;
        let _ = std::fs::remove_dir_all(&scratch);
        res
    }

    /// Install a plugin from a URL (`owner/repo@ref`, a zip, or a plugin.toml naming its release zip), downloading into `scratch`.
    async fn install_url_in(&self, input: &str, scratch: &Path) -> Result<Value> {
        let net = self.release_net();
        let max = manifest::UPLOAD_MAX_BYTES;
        let limit = std::time::Duration::from_secs(1800);
        if let Some((repo, sub, git_ref)) = github_ref(input) {
            let url = format!("{}/{repo}/archive/{git_ref}.zip", net.hosts.github.trim_end_matches('/'));
            let zip = scratch.join("archive.zip");
            let got = crate::fetch::to_file(&net.http, &url, &zip, max, limit, net.allow_http).await?;
            return self.install_archive(&zip, Origin::Url, Some(got.sha256), sub.as_deref(), None).await;
        }
        crate::fetch::check_url(input, net.allow_http)?;
        let file = scratch.join("download");
        let got = crate::fetch::to_file(&net.http, input, &file, max, limit, net.allow_http).await?;
        if crate::archive::Kind::sniff(&file) == Some(crate::archive::Kind::Zip) {
            return self.install_archive(&file, Origin::Url, Some(got.sha256), None, None).await;
        }
        // a raw plugin.toml: its [update] names the zip of the newest release and its sums
        let text = fsx::read_to_string(&file, Step::Plugin).map_err(|_| Refused::new(400, "The address gives neither a zip nor a plugin.toml"))?;
        if text.len() > manifest::MAX_MANIFEST_BYTES || toml_like(&text).is_none() {
            return Err(Refused::new(400, "The address gives neither a zip nor a plugin.toml"));
        }
        let m = parse_for(&text, Origin::Url)?;
        let u = m.update.clone().filter(|u| u.source() == "github" && u.asset.is_some() && u.sums.is_some()).ok_or_else(|| {
            Refused::new(400, format!("{}'s plugin.toml names no GitHub release with a zip and its sums ([update] github, asset, sums)", m.plugin.name))
        })?;
        let asset_t = u.asset.clone().unwrap_or_default();
        let has = move |r: &Release| r.asset(&manifest::fill(&asset_t, &r.version, &r.tag)).is_some();
        let release = releases::newest(&net, &u, &has).await?.ok_or_else(|| Refused::new(400, format!("{} has no release with its zip yet", m.plugin.name)))?;
        let release = if release.assets.is_empty() { releases::by_tag(&net, &u, &release.tag).await? } else { release };
        let asset = manifest::fill(u.asset.as_deref().unwrap_or_default(), &release.version, &release.tag);
        let a = release.asset(&asset).ok_or_else(|| Refused::new(400, format!("the {} release has no {asset}", release.tag)))?;
        let want = releases::sha256_of(&net, &release, &asset, u.sums.as_deref().unwrap_or_default()).await?;
        let zip = scratch.join(&asset);
        let got = crate::fetch::to_file(&net.http, &a.url, &zip, max, limit, net.allow_http).await?;
        install::check_sha256(&zip, &got.sha256, &want, &asset)?;
        self.install_archive(&zip, Origin::Url, Some(got.sha256), None, None).await
    }

    /// Install a plugin program from an archive: unpack it (names checked), find `plugin.toml` (at the root, in
    /// its one top folder, or in `sub`), check the manifest (the file, its origin's rules, beside every other
    /// plugin, this Encore's version), run `describe` from the staged folder, then switch to it. `only`: the id
    /// it must have (an update).
    pub async fn install_archive(&self, archive: &Path, origin: Origin, sha256: Option<String>, sub: Option<&str>, only: Option<&str>) -> Result<Value> {
        self.install_archive_with(archive, origin, sha256, sub, only, false).await
    }

    /// `unattended`: the scheduled task's update, refused when the new manifest asks for more than the row's.
    async fn install_archive_with(&self, archive: &Path, origin: Origin, sha256: Option<String>, sub: Option<&str>, only: Option<&str>, unattended: bool) -> Result<Value> {
        let sha = match sha256 {
            Some(s) => s,
            None => crate::fetch::sha256_file(archive)?,
        };
        let scratch = self.scratch()?;
        let res = self.install_unpacked(archive, &scratch, origin, &sha, sub, only, unattended).await;
        let _ = std::fs::remove_dir_all(&scratch);
        res
    }

    /// Unpack `archive` and install the plugin in it (`sub`: its folder inside; `only`: the id it must have).
    #[allow(clippy::too_many_arguments)]
    async fn install_unpacked(&self, archive: &Path, scratch: &Path, origin: Origin, sha: &str, sub: Option<&str>, only: Option<&str>, unattended: bool) -> Result<Value> {
        let kind = crate::archive::Kind::sniff(archive).ok_or_else(|| Refused::new(400, "Not a zip file"))?;
        let unpacked = scratch.join("unpacked");
        let (a, u) = (archive.to_path_buf(), unpacked.clone());
        let limits = crate::archive::Limits { bytes: 4 * manifest::UPLOAD_MAX_BYTES, entries: 20_000 };
        tokio::task::spawn_blocking(move || crate::archive::unpack(&a, kind, &u, limits))
            .await
            .map_err(|e| Refused::new(500, e.to_string()))?
            .map_err(|e| Refused::new(400, format!("{e:#}")))?;
        let dir = plugin_dir(&unpacked, sub)?;
        check_plugin_folder(&unpacked, &dir)?;
        if fsx::metadata(dir.join("plugin.toml"), Step::Plugin)?.len() > manifest::MAX_MANIFEST_BYTES as u64 {
            return Err(Refused::new(422, format!("plugin.toml is larger than {} KB", manifest::MAX_MANIFEST_BYTES / 1024)));
        }
        let text = fsx::read_to_string(dir.join("plugin.toml"), Step::Plugin)?;
        let m = parse_for(&text, origin)?;
        let id = m.id().to_string();
        if let Some(want) = only {
            if id != want {
                return Err(Refused::new(400, format!("The download is the plugin {id}, not {want}")));
            }
        }
        if catalog::find(&id).is_some() {
            return Err(Refused::new(409, format!("{id} is the id of a plugin Encore ships; an uploaded plugin needs another id")));
        }
        if RESERVED_IDS.contains(&id.as_str()) {
            return Err(Refused::new(400, format!("{id} is a word the Plugins page uses; the plugin needs another id")));
        }
        if let Some(row) = self.row(&id) {
            let was = Self::origin_of(&row);
            if was == Origin::Catalog {
                return Err(Refused::new(409, format!("{id} is the id of a plugin Encore ships")));
            }
            if unattended {
                let more = parse_for(&row.s("manifest"), was).map(|old| m.asks_for_more(&old)).unwrap_or_else(|_| vec!["a manifest Encore cannot compare".into()]);
                if !more.is_empty() {
                    return Err(Refused::new(409, format!("{} {} asks for more than the installed version: {} – update it by hand (Update on its card)", m.plugin.name, m.plugin.version, more.join(", "))));
                }
            }
        }
        let others: Vec<Manifest> = self.known().into_iter().filter(|o| o.id() != id).collect();
        let mut all: Vec<&Manifest> = others.iter().collect();
        all.push(&m);
        let clash: Vec<String> = check_together(&all).into_iter().filter(|p| p.contains(&id) || p.contains(&m.plugin.name)).collect();
        if !clash.is_empty() {
            return Err(Refused { status: 409, message: format!("{} clashes with a plugin Encore knows: {}", m.plugin.name, clash.join("; ")), problems: clash });
        }
        m.fits(encore_core::VERSION).map_err(|e| Refused::new(400, e))?;
        let _b = self.busy(&id, if self.installed(&id) { "updating" } else { "installing" })?;
        let folders = self.folders(&id);
        let versions = Versions::new(&folders.root);
        let version = m.plugin.version.clone();
        let stage = versions.stage(&version)?;
        let res: Result<()> = async {
            // the plugin's files: moved into the staged version folder
            fsx::remove_dir(&stage, Step::Plugin)?;
            fsx::rename(&dir, &stage, Step::Plugin)?;
            self.stage_program(&m, &stage, scratch).await?;
            let script = m.run.as_ref().and_then(|r| r.script.clone()).unwrap_or_default();
            let script_path = stage.join(&script);
            if !std::fs::symlink_metadata(&script_path).is_ok_and(|md| md.is_file()) {
                return Err(Refused::new(400, format!("{}'s program {script} is not in the zip", m.plugin.name)));
            }
            if m.run.as_ref().is_some_and(|r| r.interpreter.is_none()) {
                install::make_executable(&script_path)?;
            }
            let probe = ProcessPlugin::new(m.clone(), &folders.root, &stage, self.sandbox(), self.read_only(&m));
            probe.describe().await.map_err(|f| Refused::new(400, format!("{} does not start here: {}", m.plugin.name, f.message)))?;
            Ok(())
        }
        .await;
        if let Err(e) = res {
            versions.abandon(&version);
            if !self.installed(&id) {
                // a first install that failed leaves nothing but its log
                let _ = std::fs::remove_dir_all(folders.data());
                let _ = std::fs::remove_dir_all(folders.home());
                let _ = std::fs::remove_dir_all(folders.tmp());
            }
            Log::new(&folders.logs()).line("install", &format!("failed: {}", e.message));
            return Err(e);
        }
        versions.commit(&version)?;
        fsx::write(folders.root.join("plugin.toml"), &text, Step::Plugin)?;
        let now = encore_db::now();
        let auto = m.update.as_ref().is_some_and(|u| u.auto) as i64;
        self.0
            .db
            .execute(
                "INSERT INTO plugins (id, version, enabled, installed_at, updated_at, origin, trust, manifest, sha256, auto_update, last_check, last_error)
                 VALUES (?, ?, 1, ?, ?, ?, ?, ?, ?, ?, NULL, NULL)
                 ON CONFLICT(id) DO UPDATE SET version=excluded.version, updated_at=excluded.updated_at, origin=excluded.origin, trust=excluded.trust,
                   manifest=excluded.manifest, sha256=excluded.sha256, last_error=NULL",
                p![id, version, now, now, origin.as_str(), origin.trust(), text, sha, auto],
            )
            .map_err(|e| Refused::new(500, e.to_string()))?;
        self.0.procs.lock().unwrap().remove(&id);
        let log = self.log(&id);
        log.line("install", &format!("installed {version} ({}, sha256 {sha})", origin.as_str()));
        if let Some(plugin) = self.plugin(&id) {
            if let Err(f) = plugin.installed(&self.call(&m), Some(&version)).await {
                log.line("update", &format!("its update command failed: {}", f.message));
            }
        }
        self.0.db.log("info", &format!("Plugin {} {version} installed ({}, third-party)", m.plugin.name, origin.as_str()));
        drop(_b);
        Ok(self.card(&id).unwrap_or(Value::Null))
    }

    /// An uploaded plugin's `[[binary]]` for this machine (a fixed sha256 each): fetched into the scratch folder,
    /// checked and unpacked into `<staged>/program`, a new folder (the zip may not bring one of that name).
    async fn stage_program(&self, m: &Manifest, stage: &Path, scratch: &Path) -> Result<()> {
        let Some(b) = m.binary_for(std::env::consts::ARCH) else {
            if !m.binaries.is_empty() {
                return Err(Refused::new(400, format!("{} has no program for this server ({})", m.plugin.name, std::env::consts::ARCH)));
            }
            return Ok(());
        };
        let url = b.url.clone().ok_or_else(|| Refused::new(400, format!("{} names no address for its program", m.plugin.name)))?;
        let want = b.sha256.clone().ok_or_else(|| Refused::new(400, format!("{} names no sha256 for its program", m.plugin.name)))?;
        let name = url.rsplit('/').next().unwrap_or("download").split(['?', '#']).next().unwrap_or("download").to_string();
        let dest = stage.join("program");
        if std::fs::symlink_metadata(&dest).is_ok() {
            return Err(Refused::new(400, format!("{}'s zip has its own program in it; its [[binary]] download goes there", m.plugin.name)));
        }
        let file = scratch.join(format!("{name}.download"));
        let got = crate::fetch::to_file(&self.net().http, &url, &file, m.max_bytes(), std::time::Duration::from_secs(1800), self.allow_http()).await?;
        install::check_sha256(&file, &got.sha256, &want, &name)?;
        let p = Program { url, name: name.clone(), sha256: Some(want), unpack: b.unpack_for(&name).unwrap_or("file").to_string(), bin: b.bin.clone(), remove: vec![], max_bytes: m.max_bytes() };
        let (f2, d2, bin) = (file.clone(), dest.clone(), b.bin.clone());
        tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            fsx::mkdir(&d2, Step::Plugin)?;
            install::unpack_program(&f2, &p, &d2)?;
            let exe = crate::archive::find_bin(&d2, &bin).ok_or_else(|| anyhow::anyhow!("the download has no {bin} program in it"))?;
            install::make_executable(&exe)
        })
        .await
        .map_err(|e| Refused::new(500, e.to_string()))??;
        let _ = std::fs::remove_file(&file);
        Ok(())
    }

    /// Stop every plugin program that runs (Encore stopping).
    pub async fn stop_all(&self) {
        let procs: Vec<Arc<ProcessPlugin>> = self.0.procs.lock().unwrap().values().map(|(_, p)| p.clone()).collect();
        for p in procs {
            p.stop_all().await;
        }
    }
}

/// The folder of the plugin inside an unpacked archive: `sub` when given, else the root when it has
/// plugin.toml, else its one top folder.
fn plugin_dir(unpacked: &Path, sub: Option<&str>) -> Result<PathBuf> {
    let missing = || Refused::new(400, "The zip has no plugin.toml at its root or in its one top folder");
    let top_folder = || -> Option<PathBuf> {
        let entries: Vec<_> = std::fs::read_dir(unpacked).ok()?.flatten().collect();
        match entries.as_slice() {
            [one] if one.file_type().ok()?.is_dir() => Some(one.path()),
            _ => None,
        }
    };
    if let Some(sub) = sub {
        let rel = crate::archive::entry_path(sub).map_err(|e| Refused::new(400, e.to_string()))?.ok_or_else(missing)?;
        let base = top_folder().ok_or_else(missing)?;
        let d = base.join(rel);
        return if std::fs::symlink_metadata(d.join("plugin.toml")).is_ok_and(|m| m.is_file()) { Ok(d) } else { Err(Refused::new(400, format!("The archive has no plugin.toml in {sub}"))) };
    }
    if std::fs::symlink_metadata(unpacked.join("plugin.toml")).is_ok_and(|m| m.is_file()) {
        return Ok(unpacked.to_path_buf());
    }
    let top = top_folder().ok_or_else(missing)?;
    if std::fs::symlink_metadata(top.join("plugin.toml")).is_ok_and(|m| m.is_file()) {
        Ok(top)
    } else {
        Err(missing())
    }
}

/// The plugin's folder is moved out of the unpacked archive on its own, so the archive's own check (links stay
/// inside the archive) is not enough: the folder must be a real folder (no link on the way to it, inside the
/// archive), and every link in it must stay inside it, checked again from the folder itself.
fn check_plugin_folder(unpacked: &Path, dir: &Path) -> Result<()> {
    let leaves = |what: &Path| Refused::new(400, format!("The zip has a link that leaves the plugin's folder: {}", what.display()));
    let rel = dir.strip_prefix(unpacked).map_err(|_| leaves(dir))?;
    let mut at = unpacked.to_path_buf();
    for c in rel.components() {
        at.push(c);
        if !std::fs::symlink_metadata(&at).is_ok_and(|m| m.is_dir()) {
            return Err(Refused::new(400, format!("The plugin's folder in the zip is a link: {}", rel.display())));
        }
    }
    let real_root = std::fs::canonicalize(unpacked).map_err(fsx::at(Step::Plugin, "canonicalize", unpacked))?;
    let real_dir = std::fs::canonicalize(dir).map_err(fsx::at(Step::Plugin, "canonicalize", dir))?;
    if real_dir != real_root.join(rel) {
        return Err(Refused::new(400, format!("The plugin's folder in the zip is a link: {}", rel.display())));
    }
    let mut todo = vec![dir.to_path_buf()];
    while let Some(d) = todo.pop() {
        for e in fsx::read_dir(&d, Step::Plugin)? {
            let e = e.map_err(fsx::at(Step::Plugin, "read_dir", &d))?;
            let path = e.path();
            let md = std::fs::symlink_metadata(&path).map_err(fsx::at(Step::Plugin, "stat", &path))?;
            let inside = path.strip_prefix(dir).unwrap_or(&path).to_path_buf();
            if md.file_type().is_symlink() {
                let target = std::fs::read_link(&path).map_err(fsx::at(Step::Plugin, "readlink", &path))?;
                if !crate::archive::link_ok(&inside, &target.to_string_lossy()) {
                    return Err(leaves(&inside));
                }
            } else if md.is_dir() {
                todo.push(path);
            }
        }
    }
    Ok(())
}

/// `owner/repo@ref` or `owner/repo/sub/folder@ref` → (owner/repo, sub folder, ref).
pub fn github_ref(input: &str) -> Option<(String, Option<String>, String)> {
    if input.contains("://") || input.contains(char::is_whitespace) {
        return None;
    }
    let (path, git_ref) = input.rsplit_once('@')?;
    let ok = |s: &str| !s.is_empty() && s != "." && s != ".." && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b));
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() < 2 || !parts.iter().all(|p| ok(p)) {
        return None;
    }
    let ref_ok = !git_ref.is_empty() && !git_ref.contains("..") && git_ref.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_./".contains(&b)) && !git_ref.starts_with('/');
    if !ref_ok {
        return None;
    }
    let sub = (parts.len() > 2).then(|| parts[2..].join("/"));
    Some((format!("{}/{}", parts[0], parts[1]), sub, git_ref.to_string()))
}

/// Text that reads as TOML with a [plugin] table.
fn toml_like(text: &str) -> Option<()> {
    let v: toml::Value = toml::from_str(text).ok()?;
    v.get("plugin").map(|_| ())
}

/// A setting's value for a plugin program, checked against its field.
fn coerce(s: &manifest::Setting, v: &Value) -> std::result::Result<Value, String> {
    let label = s.label.clone().unwrap_or_else(|| s.key.clone());
    match s.kind.as_deref().unwrap_or("text") {
        "bool" => match v {
            Value::Bool(b) => Ok(json!(b)),
            Value::String(t) => Ok(json!(matches!(t.trim().to_lowercase().as_str(), "1" | "true" | "yes" | "on"))),
            Value::Number(n) => Ok(json!(n.as_f64().unwrap_or(0.0) != 0.0)),
            _ => Err(format!("{label}: on or off")),
        },
        "number" => {
            let n = match v {
                Value::Number(n) => n.as_f64(),
                Value::String(t) if t.trim().is_empty() => return Ok(s.default.clone().unwrap_or(Value::Null)),
                Value::String(t) => t.trim().parse::<f64>().ok(),
                _ => None,
            }
            .filter(|n| n.is_finite())
            .ok_or_else(|| format!("{label}: a number"))?;
            if s.min.is_some_and(|m| n < m) || s.max.is_some_and(|m| n > m) {
                return Err(format!("{label}: from {} to {}", s.min.map(|m| m.to_string()).unwrap_or_else(|| "any".into()), s.max.map(|m| m.to_string()).unwrap_or_else(|| "any".into())));
            }
            Ok(if n.fract() == 0.0 && n.abs() < 1e15 { json!(n as i64) } else { json!(n) })
        }
        "select" => {
            let t = text_of(v);
            if s.options.iter().any(|(value, _)| *value == t) {
                Ok(json!(t))
            } else {
                Err(format!("{label}: one of {}", s.options.iter().map(|(v, _)| v.as_str()).collect::<Vec<_>>().join(", ")))
            }
        }
        "checks" => {
            let t = text_of(v);
            let picked: Vec<&str> = t.split(',').map(str::trim).filter(|x| !x.is_empty()).collect();
            if let Some(bad) = picked.iter().find(|p| !s.options.iter().any(|(value, _)| value == *p)) {
                return Err(format!("{label}: {bad} is not one of the choices"));
            }
            Ok(json!(picked.join(",")))
        }
        _ => {
            let t = text_of(v);
            if t.chars().count() > 4000 {
                return Err(format!("{label}: at most 4000 characters"));
            }
            Ok(json!(t))
        }
    }
}

/// A value as text (null as "").
fn text_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// The settings table's field for a key (its card, its words), when the Settings pages draw it.
fn schema_field(key: &str) -> Option<&'static encore_config::settings::Field> {
    encore_config::settings::SCHEMA.iter().flat_map(|s| s.groups.iter()).flat_map(|g| g.fields.iter()).find(|f| f.key == key)
}

/// A number as JSON: whole numbers as integers.
fn number(n: f64) -> Value {
    if n.fract() == 0.0 && n.abs() < 1e15 { json!(n as i64) } else { json!(n) }
}

/// A field of a plugin's settings card, in the shape of GET /api/admin/settings/schema's fields (plus `part`):
/// a builtin key's words from the settings table, else the manifest's. Null: an internal key.
pub fn field_json(m: &Manifest, s: &manifest::Setting, cfg: &Config) -> Value {
    if m.plugin.builtin {
        let def = encore_config::settings::def(&s.key);
        if let Some(f) = schema_field(&s.key) {
            return json!({
                "key": f.key, "label": f.label, "help": f.help, "type": f.kind,
                "options": f.options.iter().map(|(value, label)| json!({ "value": value, "label": label })).collect::<Vec<_>>(),
                "when": f.when, "secret": def.is_some_and(|d| d.secret), "env": def.and_then(|d| d.env), "env_locked": cfg.env_locked(f.key),
                "default": def.map(|d| d.default.to_json()), "unit": f.unit, "min": f.min.map(number), "max": f.max.map(number),
                "step": f.step.map(number), "full": f.full, "advanced": f.advanced, "part": s.part,
            });
        }
        if s.label.is_none() {
            return Value::Null;
        }
        return json!({
            "key": s.key, "label": s.label, "help": s.help, "type": s.kind.as_deref().unwrap_or("text"),
            "options": s.options.iter().map(|(value, label)| json!({ "value": value, "label": label })).collect::<Vec<_>>(),
            "when": s.when, "secret": def.is_some_and(|d| d.secret) || s.kind.as_deref() == Some("password"),
            "env": def.and_then(|d| d.env), "env_locked": cfg.env_locked(&s.key), "default": def.map(|d| d.default.to_json()).or(s.default.clone()),
            "unit": s.unit, "min": s.min.map(number), "max": s.max.map(number), "step": s.step.map(number), "full": s.full, "advanced": s.advanced, "part": s.part,
        });
    }
    json!({
        "key": s.key, "label": s.label, "help": s.help, "type": s.kind.as_deref().unwrap_or("text"),
        "options": s.options.iter().map(|(value, label)| json!({ "value": value, "label": label })).collect::<Vec<_>>(),
        "when": s.when, "secret": s.kind.as_deref() == Some("password"), "env": null, "env_locked": false, "default": s.default,
        "unit": s.unit, "min": s.min.map(number), "max": s.max.map(number), "step": s.step.map(number), "full": s.full, "advanced": s.advanced, "part": s.part,
    })
}

/// [`manifest::Facts`] over Encore's settings and config folder.
pub struct CfgFacts<'a> {
    /// Encore's settings
    pub cfg: &'a Config,
    /// Encore's config folder
    pub dir: &'a Path,
}

impl manifest::Facts for CfgFacts<'_> {
    fn value(&self, key: &str) -> Option<Value> {
        self.cfg.def(key).map(|_| self.cfg.get(key))
    }
    fn saved(&self, key: &str) -> bool {
        self.cfg.env_locked(key) || encore_config::settings::default_of(key).is_some_and(|d| d != self.cfg.get(key))
    }
    fn exists(&self, rel: &str) -> bool {
        manifest::is_rel_path(rel, false) && self.dir.join(rel).exists()
    }
}

/// A catalog plugin no builtin crate has registered an adapter for yet: answers from its manifest.
struct CatalogPlugin {
    /// its manifest
    m: Manifest,
    /// the registry it reads its rows from
    registry: Registry,
}

impl CatalogPlugin {
    /// The installed program (the version `current` names).
    fn program(&self) -> Option<(String, PathBuf)> {
        let folders = self.registry.folders(self.m.id());
        let versions = Versions::new(&folders.root);
        let v = versions.current()?;
        let dir = versions.dir(&v);
        let bin = if let Some(py) = &self.m.python {
            py.bin.0.iter().map(|b| dir.join("venv/bin").join(b)).find(|p| p.is_file())?
        } else {
            let b = self.m.binary_for(std::env::consts::ARCH)?;
            crate::archive::find_bin(&dir, &b.bin)?
        };
        Some((v, bin))
    }
}

impl Plugin for CatalogPlugin {
    fn manifest(&self) -> &Manifest {
        &self.m
    }

    fn status<'a>(&'a self, _call: &'a Call) -> crate::plugin::BoxFuture<'a, std::result::Result<Status, Failure>> {
        Box::pin(async move {
            if !self.m.installs_program() {
                return Ok(Status { message: "Ready".into(), signed_in: None });
            }
            match self.program() {
                Some((v, _)) => Ok(Status { message: format!("{} {v} installed", self.m.plugin.name), signed_in: None }),
                None => Ok(Status { message: "Not installed".into(), signed_in: None }),
            }
        })
    }

    fn test<'a>(&'a self, _call: &'a Call) -> crate::plugin::BoxFuture<'a, std::result::Result<String, Failure>> {
        Box::pin(async move {
            let (_, bin) = self.program().ok_or_else(|| Failure::internal(format!("{} is not installed", self.m.plugin.name)))?;
            let (m, root) = (self.m.clone(), self.registry.folders(self.m.id()).root);
            let v = tokio::task::spawn_blocking(move || {
                let env = install::run_env(&m, &bin, &root);
                install::verify(&m, &bin, &env, &root.join("home"))
            })
            .await
            .map_err(|e| Failure::internal(e.to_string()))??;
            Ok(format!("{} {} runs", self.m.plugin.name, v.version.unwrap_or_default()).trim().to_string())
        })
    }

    fn check_update<'a>(&'a self, _call: &'a Call) -> crate::plugin::BoxFuture<'a, Option<std::result::Result<Found, Failure>>> {
        Box::pin(async { None })
    }
}

#[cfg(test)]
mod send_checks {
    fn is_send<T: Send>(_: T) {}

    /// The API's handlers run these on tokio's worker threads.
    #[allow(dead_code, unused_must_use)]
    fn futures_are_send(r: super::Registry) {
        is_send(r.install("x"));
        is_send(r.update("x"));
        is_send(r.check("x"));
        is_send(r.check_all());
        is_send(r.install_url("x"));
        is_send(r.upload(std::path::Path::new("x")));
        is_send(r.status("x"));
        is_send(r.test("x"));
        is_send(r.enable("x", true));
        is_send(r.remove("x"));
    }
}
