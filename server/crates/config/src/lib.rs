//! Settings: defaults (the table in settings.toml, see `settings`), overridden by values saved in
//! the UI, overridden by environment variables (shown as locked in the UI).
pub mod perms;
pub mod release_types;
pub mod settings;
pub mod when;

use encore_db::{p, Db};
use kit_server_settings::{Store, Table, Values};
use serde_json::{json, Value};
use std::sync::Arc;

pub use kit_server_settings::{Def, EnvLookup, MASK};

/// The settings in effect: defaults, then the values saved in the UI, then the environment. Clones share them.
/// The kit's settings [`Store`] keeps them; Config adds Encore's start-up steps and typed accessors.
#[derive(Clone)]
pub struct Config {
    /// every setting's definition, in `settings::TABLE` order
    pub defs: Arc<Vec<(String, Def)>>,
    /// the values: environment, then saved, then default
    store: Store<Db>,
    /// where the saved values are
    db: Db,
}

/// settings row that marks the one-time discography_singles → release_types migration as done
const RELEASE_TYPES_MIGRATED: &str = "release_types_migrated";

impl Config {
    /// The settings on `db`, with the process's environment variables over them.
    pub fn load(db: Db) -> anyhow::Result<Config> {
        Config::load_with(db, |name| std::env::var(name).ok())
    }

    /// [`Config::load`] with the environment variables `lookup` gives (a test's own set, the process's
    /// environment untouched).
    pub fn load_with(db: Db, lookup: impl Fn(&str) -> Option<String> + Send + Sync + 'static) -> anyhow::Result<Config> {
        let store = Store::new(db.clone(), settings::ENCORE, lookup);
        let cfg = Config { defs: store.defs.clone(), store, db };
        cfg.reload()?;
        Ok(cfg)
    }

    /// Read the saved values and the environment again (and generate an API key on first start).
    pub fn reload(&self) -> anyhow::Result<()> {
        let mut stored = self.store.saved()?;
        // "discography includes singles" (before release types) becomes the Single box, once, on installs
        // that existed before (a fresh install has nothing stored and keeps the new defaults)
        if !stored.contains_key(RELEASE_TYPES_MIGRATED) {
            if !stored.is_empty() {
                let env_singles = self
                    .store
                    .env_var("ENCORE_DISCOGRAPHY_SINGLES")
                    .map(|r| kit_server_settings::values::clean_env(&r))
                    .filter(|c| !c.is_empty())
                    .map(Value::String);
                let singles = env_singles.as_ref().or(stored.get("discography_singles"));
                if let Some(kinds) = crate::release_types::migrated_kinds(stored.get("release_types"), singles) {
                    self.db.execute("INSERT OR REPLACE INTO settings VALUES (?,?)", p!["release_types", json!(kinds).to_string()])?;
                    stored.insert("release_types".into(), json!(kinds));
                }
            }
            self.db.execute("INSERT OR REPLACE INTO settings VALUES (?,?)", p![RELEASE_TYPES_MIGRATED, "true"])?;
        }
        self.store.apply(&stored);
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
        self.store.def(key)
    }

    /// The value in effect for `key` (environment first, then saved or default; null for an unknown key).
    pub fn get(&self, key: &str) -> Value {
        self.store.get(key)
    }
    /// `key` as text (empty for null).
    pub fn str(&self, key: &str) -> String {
        self.store.str(key)
    }
    /// `key` as a boolean (a non-zero number counts as true).
    pub fn bool(&self, key: &str) -> bool {
        self.store.bool(key)
    }
    /// `key` as an integer (0 when not a number).
    pub fn int(&self, key: &str) -> i64 {
        self.store.int(key)
    }
    /// `key` as a real (0 when not a number).
    pub fn float(&self, key: &str) -> f64 {
        self.store.float(key)
    }
    /// Whether an environment variable sets `key` (the UI shows it locked).
    pub fn env_locked(&self, key: &str) -> bool {
        self.store.env_locked(key)
    }

    /// Save `values` (`{key: value}`), each coerced to its setting's type; unknown keys are skipped.
    pub fn set_many(&self, values: &Value) {
        self.store.set_many(values)
    }

    /// A number that changes whenever any setting may have changed (a reload, a save). Something worked
    /// out from the settings is still right while this number is the one it was worked out at.
    pub fn generation(&self) -> u64 {
        self.store.generation()
    }

    /// Whether `v` is the masked form of a secret (sent back unchanged from the UI).
    pub fn is_masked(v: &Value) -> bool {
        kit_server_settings::is_masked(v)
    }

    /// All settings for the admin UI – secrets only say whether they are set, never their value (except the
    /// API key, which the owner's API key card shows).
    pub fn public_dump(&self) -> Value {
        self.store.dump(&["api_key"])
    }
}

/// The settings routes (kit-server-settings) read and save Encore's settings through it.
impl Values for Config {
    fn table(&self) -> Table {
        settings::ENCORE
    }
    fn known(&self, key: &str) -> bool {
        self.def(key).is_some()
    }
    fn get(&self, key: &str) -> Value {
        Config::get(self, key)
    }
    fn env_locked(&self, key: &str) -> bool {
        Config::env_locked(self, key)
    }
    fn set_many(&self, values: &Value) {
        Config::set_many(self, values)
    }
    fn dump(&self) -> Value {
        self.public_dump()
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
