//! Settings: defaults (the table in settings.toml, see `settings`), overridden by values saved in
//! the UI, overridden by environment variables (shown as locked in the UI).
pub mod perms;
pub mod release_types;
pub mod settings;
pub mod when;

use encore_db::{p, Db};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

/// One setting as the loader keeps it (built from `settings::TABLE`).
#[derive(Clone)]
pub struct Def {
    /// the environment variable that sets it (and locks it in the UI)
    pub env: Option<String>,
    /// the value until one is saved, as JSON
    pub default: Value,
    /// never sent to a browser
    pub secret: bool,
}

/// Where the environment variables come from: the process's own in use, a fixed set in tests.
pub type EnvLookup = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// The settings in effect: defaults, then the values saved in the UI, then the environment. Clones share them.
#[derive(Clone)]
pub struct Config {
    /// every setting's definition, in `settings::TABLE` order
    pub defs: Arc<Vec<(String, Def)>>,
    /// the saved value (or the default) of every key
    cache: Arc<RwLock<HashMap<String, Value>>>,
    /// the keys an environment variable sets, with its value
    env: Arc<RwLock<HashMap<String, Value>>>,
    /// moves on every reload and every save, so what is worked out from the settings can be kept until then
    generation: Arc<AtomicU64>,
    /// where the saved values are
    db: Db,
    /// reads an environment variable (on every reload)
    lookup: EnvLookup,
}

/// Sent to the browser in place of a secret's value; a value that starts with it was not changed in the UI.
pub const MASK: &str = "••••";
/// settings row that marks the one-time discography_singles → release_types migration as done
const RELEASE_TYPES_MIGRATED: &str = "release_types_migrated";

/// `raw` (saved, or from the environment) as the type of `default`: "1"/"yes" for a boolean, a numeric string
/// for a number, any value as text; the default when it cannot be read as that type.
fn coerce(default: &Value, raw: &Value) -> Value {
    match default {
        Value::Bool(_) => match raw {
            Value::Bool(b) => Value::Bool(*b),
            Value::Number(n) => Value::Bool(n.as_f64().unwrap_or(0.0) != 0.0),
            Value::String(s) => Value::Bool(matches!(s.trim().to_lowercase().as_str(), "1" | "true" | "yes" | "on")),
            _ => default.clone(),
        },
        Value::Number(n) if n.is_i64() || n.is_u64() => match raw {
            Value::Number(x) => json!(x.as_f64().unwrap_or(0.0) as i64),
            Value::String(s) => s.trim().parse::<f64>().map(|f| json!(f as i64)).unwrap_or(default.clone()),
            Value::Bool(b) => json!(*b as i64),
            _ => default.clone(),
        },
        Value::Number(_) => match raw {
            Value::Number(x) => json!(x.as_f64().unwrap_or(0.0)),
            Value::String(s) => s.trim().parse::<f64>().map(|f| json!(f)).unwrap_or(default.clone()),
            _ => default.clone(),
        },
        _ => match raw {
            Value::Null => json!(""),
            Value::String(s) => json!(s),
            other => json!(other.to_string()),
        },
    }
}

/// Compose list syntax keeps quotes literally (- KEY="abc"), and copy/paste adds whitespace.
fn clean_env(raw: &str) -> String {
    let v = raw.trim();
    let b = v.as_bytes();
    if b.len() >= 2 && b[0] == b[b.len() - 1] && (b[0] == b'"' || b[0] == b'\'') {
        return v[1..v.len() - 1].trim().to_string();
    }
    v.to_string()
}

impl Config {
    /// The settings on `db`, with the process's environment variables over them.
    pub fn load(db: Db) -> anyhow::Result<Config> {
        Config::load_with(db, |name| std::env::var(name).ok())
    }

    /// [`Config::load`] with the environment variables `lookup` gives (a test's own set, the process's
    /// environment untouched).
    pub fn load_with(db: Db, lookup: impl Fn(&str) -> Option<String> + Send + Sync + 'static) -> anyhow::Result<Config> {
        let defs: Vec<(String, Def)> = settings::TABLE
            .iter()
            .map(|d| (d.key.to_string(), Def { env: d.env.map(String::from), default: d.default.to_json(), secret: d.secret }))
            .collect();
        let cfg = Config { defs: Arc::new(defs), cache: Default::default(), env: Default::default(), generation: Default::default(), db, lookup: Arc::new(lookup) };
        cfg.reload()?;
        Ok(cfg)
    }

    /// Read the saved values and the environment again (and generate an API key on first start).
    pub fn reload(&self) -> anyhow::Result<()> {
        let mut stored: HashMap<String, Value> = self
            .db
            .query("SELECT key, value FROM settings", p![])?
            .into_iter()
            .filter_map(|r| {
                let k = r.get("key")?.as_str()?.to_string();
                let v: Value = serde_json::from_str(r.get("value")?.as_str()?).ok()?;
                Some((k, v))
            })
            .collect();
        // "discography includes singles" (before release types) becomes the Single box, once, on installs
        // that existed before (a fresh install has nothing stored and keeps the new defaults)
        if !stored.contains_key(RELEASE_TYPES_MIGRATED) {
            if !stored.is_empty() {
                let env_singles = (self.lookup)("ENCORE_DISCOGRAPHY_SINGLES").map(|r| clean_env(&r)).filter(|c| !c.is_empty()).map(Value::String);
                let singles = env_singles.as_ref().or(stored.get("discography_singles"));
                if let Some(kinds) = crate::release_types::migrated_kinds(stored.get("release_types"), singles) {
                    self.db.execute("INSERT OR REPLACE INTO settings VALUES (?,?)", p!["release_types", json!(kinds).to_string()])?;
                    stored.insert("release_types".into(), json!(kinds));
                }
            }
            self.db.execute("INSERT OR REPLACE INTO settings VALUES (?,?)", p![RELEASE_TYPES_MIGRATED, "true"])?;
        }
        let mut cache = self.cache.write().unwrap();
        let mut env = self.env.write().unwrap();
        cache.clear();
        env.clear();
        for (key, def) in self.defs.iter() {
            if let Some(name) = &def.env {
                if let Some(raw) = (self.lookup)(name) {
                    let c = clean_env(&raw);
                    if !c.is_empty() {
                        env.insert(key.clone(), coerce(&def.default, &json!(c)));
                    }
                }
            }
            cache.insert(key.clone(), stored.get(key).map(|v| coerce(&def.default, v)).unwrap_or(def.default.clone()));
        }
        drop(cache);
        drop(env);
        self.generation.fetch_add(1, Ordering::SeqCst);
        if self.str("api_key").is_empty() {
            self.set_many(&json!({ "api_key": encore_core::ids::random_hex(24) }));
        }
        // the identity the apps check when they try the local network address (GET /api/auth/info)
        if self.str("server_id").is_empty() {
            self.set_many(&json!({ "server_id": encore_core::ids::random_hex(16) }));
        }
        Ok(())
    }

    /// The definition of `key`.
    pub fn def(&self, key: &str) -> Option<&Def> {
        self.defs.iter().find(|(k, _)| k == key).map(|(_, d)| d)
    }

    /// The value in effect for `key` (environment first, then saved or default; null for an unknown key).
    pub fn get(&self, key: &str) -> Value {
        if let Some(v) = self.env.read().unwrap().get(key) {
            return v.clone();
        }
        self.cache.read().unwrap().get(key).cloned().unwrap_or(Value::Null)
    }
    /// `key` as text (empty for null).
    pub fn str(&self, key: &str) -> String {
        match self.get(key) {
            Value::String(s) => s,
            Value::Null => String::new(),
            v => v.to_string(),
        }
    }
    /// `key` as a boolean (a non-zero number counts as true).
    pub fn bool(&self, key: &str) -> bool {
        match self.get(key) {
            Value::Bool(b) => b,
            Value::Number(n) => n.as_f64().unwrap_or(0.0) != 0.0,
            Value::String(s) => !s.is_empty(),
            _ => false,
        }
    }
    /// `key` as an integer (0 when not a number).
    pub fn int(&self, key: &str) -> i64 {
        self.get(key).as_f64().map(|f| f as i64).unwrap_or(0)
    }
    /// `key` as a real (0 when not a number).
    pub fn float(&self, key: &str) -> f64 {
        self.get(key).as_f64().unwrap_or(0.0)
    }
    /// Whether an environment variable sets `key` (the UI shows it locked).
    pub fn env_locked(&self, key: &str) -> bool {
        self.env.read().unwrap().contains_key(key)
    }

    /// Save `values` (`{key: value}`), each coerced to its setting's type; unknown keys are skipped.
    pub fn set_many(&self, values: &Value) {
        let Some(obj) = values.as_object() else { return };
        for (key, val) in obj {
            let Some(def) = self.def(key) else { continue };
            let v = coerce(&def.default, val);
            let _ = self.db.execute("INSERT OR REPLACE INTO settings VALUES (?,?)", p![key, v.to_string()]);
            self.cache.write().unwrap().insert(key.clone(), v);
        }
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    /// A number that changes whenever any setting may have changed (a reload, a save). Something worked
    /// out from the settings is still right while this number is the one it was worked out at.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    /// Whether `v` is the masked form of a secret (sent back unchanged from the UI).
    pub fn is_masked(v: &Value) -> bool {
        v.as_str().map(|s| s.starts_with(MASK)).unwrap_or(false)
    }

    /// All settings for the admin UI – secrets only say whether they are set, never their value.
    pub fn public_dump(&self) -> Value {
        let saved: std::collections::HashSet<String> =
            self.db.query("SELECT key FROM settings", p![]).unwrap_or_default().into_iter().filter_map(|r| r.get("key")?.as_str().map(String::from)).collect();
        let mut out = Map::new();
        let mut meta = Map::new();
        for (key, def) in self.defs.iter() {
            let v = self.get(key);
            let shown = if def.secret && key != "api_key" {
                let set = !matches!(&v, Value::String(s) if s.is_empty()) && !v.is_null();
                json!(if set { format!("{MASK} (saved)") } else { String::new() })
            } else {
                v
            };
            out.insert(key.clone(), shown);
            let source = if self.env_locked(key) { "env" } else if saved.contains(key) { "ui" } else { "default" };
            meta.insert(key.clone(), json!({ "source": source, "env": def.env }));
        }
        out.insert("_meta".into(), Value::Object(meta));
        Value::Object(out)
    }
}

#[cfg(test)]
mod settings_tests;

#[cfg(test)]
mod release_type_tests {
    use super::*;

    /// An artist's own boxes count for them alone; going back to the setting forgets them.
    #[test]
    fn an_artist_can_have_their_own_release_types() {
        use crate::release_types::ReleaseFilter;
        let d = tmp("artist-own");
        let cfg = Config::load(Db::open(&d).unwrap()).unwrap();
        assert_eq!(ReleaseFilter::artist_own(&cfg, "a1"), None);
        let own = ReleaseFilter::from_csv("Album,Single", "Compilation,Live");
        ReleaseFilter::save_for_artist(&cfg, "a1", Some(&own)).unwrap();
        assert_eq!(ReleaseFilter::for_artist(&cfg, "a1"), own);
        assert_eq!(ReleaseFilter::for_artist(&cfg, "a2"), ReleaseFilter::from_cfg(&cfg));
        // nothing skipped for them: kept as nothing, not as the setting's
        ReleaseFilter::save_for_artist(&cfg, "a1", Some(&ReleaseFilter::from_csv("Album", ""))).unwrap();
        assert!(ReleaseFilter::for_artist(&cfg, "a1").skip.is_empty());
        ReleaseFilter::save_for_artist(&cfg, "a1", None).unwrap();
        assert_eq!(ReleaseFilter::for_artist(&cfg, "a1"), ReleaseFilter::from_cfg(&cfg));
        let _ = std::fs::remove_dir_all(d);
    }

    fn tmp(name: &str) -> std::path::PathBuf {
        encore_core::tmp::dir(&format!("cfg-{name}"))
    }

    #[test]
    fn discography_singles_becomes_the_single_release_type() {
        let d = tmp("singles");
        let db = Db::open(&d).unwrap();
        db.execute("INSERT INTO settings VALUES ('discography_singles', 'true')", p![]).unwrap();
        let cfg = Config::load(db.clone()).unwrap();
        assert_eq!(cfg.str("release_types"), "Album,EP,Single");
        assert_eq!(crate::release_types::ReleaseFilter::from_cfg(&cfg).kinds, vec!["Album", "EP", "Single"]);
        // stored once: changing the boxes later is not undone by the old switch
        cfg.set_many(&json!({ "release_types": "Album" }));
        cfg.reload().unwrap();
        assert_eq!(cfg.str("release_types"), "Album");
        // without the old switch: the defaults, nothing stored
        let d2 = tmp("plain");
        let cfg = Config::load(Db::open(&d2).unwrap()).unwrap();
        assert_eq!(cfg.str("release_types"), "Album,EP");
        assert_eq!(crate::release_types::ReleaseFilter::from_cfg(&cfg), crate::release_types::ReleaseFilter::default());
        // a fresh install stays on the defaults across restarts (api_key is stored by now)
        cfg.reload().unwrap();
        assert_eq!(cfg.str("release_types"), "Album,EP");
        // an older install that never saved the switch had singles on (its default)
        let d3 = tmp("older");
        let db3 = Db::open(&d3).unwrap();
        db3.execute("INSERT INTO settings VALUES ('api_key', '\"k\"')", p![]).unwrap();
        let cfg = Config::load(db3).unwrap();
        assert_eq!(cfg.str("release_types"), "Album,EP,Single");
        let _ = std::fs::remove_dir_all(d);
        let _ = std::fs::remove_dir_all(d2);
        let _ = std::fs::remove_dir_all(d3);
    }

    #[test]
    fn the_generation_moves_with_every_save_and_reload() {
        let d = tmp("generation");
        let cfg = Config::load(Db::open(&d).unwrap()).unwrap();
        let g = cfg.generation();
        assert_eq!(cfg.generation(), g, "reading changes nothing");
        cfg.set_many(&json!({ "cadenza_max_rate": 96000 }));
        assert!(cfg.generation() > g);
        let g = cfg.generation();
        cfg.set_many(&json!({ "no_such_key": 1 }));
        assert!(cfg.generation() >= g);
        let g = cfg.generation();
        cfg.reload().unwrap();
        assert!(cfg.generation() > g);
        // clones share it
        let c2 = cfg.clone();
        c2.set_many(&json!({ "cadenza_max_rate": 192000 }));
        assert_eq!(cfg.generation(), c2.generation());
        let _ = std::fs::remove_dir_all(d);
    }
}
