//! Settings: defaults (defaults.json, shared with older servers), overridden by values saved in
//! the UI, overridden by environment variables (shown as locked in the UI).
use crate::db::Db;
use crate::p;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

#[derive(Clone)]
pub struct Def {
    pub env: Option<String>,
    pub default: Value,
    pub secret: bool,
}

#[derive(Clone)]
pub struct Config {
    pub defs: Arc<Vec<(String, Def)>>,
    cache: Arc<RwLock<HashMap<String, Value>>>,
    env: Arc<RwLock<HashMap<String, Value>>>,
    db: Db,
}

pub const MASK: &str = "••••";

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
    pub fn load(db: Db) -> anyhow::Result<Config> {
        let raw: Map<String, Value> = serde_json::from_str(include_str!("defaults.json"))?;
        let mut defs: Vec<(String, Def)> = raw
            .into_iter()
            .map(|(k, v)| (k, Def { env: v["env"].as_str().map(String::from), default: v["default"].clone(), secret: v["secret"].as_bool().unwrap_or(false) }))
            .filter(|(k, _)| !k.starts_with("lidarr_")) // Lidarr is replaced by Prowlarr → qBittorrent/SABnzbd
            .map(|(k, mut d)| {
                if k == "source_order" {
                    d.default = json!("tidal,deezer,slskd,torznab,ytdlp");
                }
                (k, d)
            })
            .collect();
        // settings added by newer servers
        for (k, env, default, secret) in [
            ("email_login", Some("ENCORE_EMAIL_LOGIN"), json!(true), false),
            // the native app's sign-in: counted from the last use (sliding), unlike session_days
            ("app_session_days", Some("ENCORE_APP_SESSION_DAYS"), json!(365), false),
            ("lastfm_shared_secret", Some("LASTFM_SHARED_SECRET"), json!(""), true),
            ("torznab_enabled", Some("TORZNAB_ENABLED"), json!(false), false),
            ("prowlarr_url", Some("PROWLARR_URL"), json!(""), false),
            ("prowlarr_api_key", Some("PROWLARR_API_KEY"), json!(""), true),
            ("torznab_min_seeders", Some("TORZNAB_MIN_SEEDERS"), json!(2), false),
            ("torznab_timeout_minutes", Some("TORZNAB_TIMEOUT_MINUTES"), json!(120), false),
            ("qbit_url", Some("QBIT_URL"), json!(""), false),
            ("qbit_user", Some("QBIT_USER"), json!("admin"), false),
            ("qbit_password", Some("QBIT_PASSWORD"), json!(""), true),
            ("qbit_category", Some("QBIT_CATEGORY"), json!("encore"), false),
            ("qbit_download_path", Some("QBIT_DOWNLOAD_PATH"), json!("/downloads/torrents"), false),
            ("sab_url", Some("SAB_URL"), json!(""), false),
            ("sab_api_key", Some("SAB_API_KEY"), json!(""), true),
            ("sab_category", Some("SAB_CATEGORY"), json!("encore"), false),
            ("sab_download_path", Some("SAB_DOWNLOAD_PATH"), json!("/downloads/usenet"), false),
            // which download client gets torrents / NZBs (Settings → Torrents & Usenet shows only its fields)
            ("torrent_client", Some("TORRENT_CLIENT"), json!("qbittorrent"), false),
            ("transmission_url", Some("TRANSMISSION_URL"), json!(""), false),
            ("transmission_user", Some("TRANSMISSION_USER"), json!(""), false),
            ("transmission_password", Some("TRANSMISSION_PASSWORD"), json!(""), true),
            ("transmission_label", Some("TRANSMISSION_LABEL"), json!("encore"), false),
            ("transmission_download_path", Some("TRANSMISSION_DOWNLOAD_PATH"), json!("/downloads/torrents"), false),
            ("deluge_url", Some("DELUGE_URL"), json!(""), false),
            ("deluge_password", Some("DELUGE_PASSWORD"), json!(""), true),
            ("deluge_label", Some("DELUGE_LABEL"), json!("encore"), false),
            ("deluge_download_path", Some("DELUGE_DOWNLOAD_PATH"), json!("/downloads/torrents"), false),
            ("rtorrent_url", Some("RTORRENT_URL"), json!(""), false),
            ("rtorrent_user", Some("RTORRENT_USER"), json!(""), false),
            ("rtorrent_password", Some("RTORRENT_PASSWORD"), json!(""), true),
            ("rtorrent_label", Some("RTORRENT_LABEL"), json!("encore"), false),
            ("rtorrent_download_path", Some("RTORRENT_DOWNLOAD_PATH"), json!("/downloads/torrents"), false),
            ("torrent_blackhole_watch", Some("TORRENT_BLACKHOLE_WATCH"), json!("/downloads/watch"), false),
            ("torrent_blackhole_completed", Some("TORRENT_BLACKHOLE_COMPLETED"), json!("/downloads/torrents"), false),
            ("nzb_client", Some("NZB_CLIENT"), json!("sabnzbd"), false),
            ("nzbget_url", Some("NZBGET_URL"), json!(""), false),
            ("nzbget_user", Some("NZBGET_USER"), json!("nzbget"), false),
            ("nzbget_password", Some("NZBGET_PASSWORD"), json!(""), true),
            ("nzbget_category", Some("NZBGET_CATEGORY"), json!("encore"), false),
            ("nzbget_download_path", Some("NZBGET_DOWNLOAD_PATH"), json!("/downloads/usenet"), false),
            ("nzb_blackhole_watch", Some("NZB_BLACKHOLE_WATCH"), json!("/downloads/watch"), false),
            ("nzb_blackhole_completed", Some("NZB_BLACKHOLE_COMPLETED"), json!("/downloads/usenet"), false),
            // daily database backups (Settings → Scheduled tasks; the copies are under config/backups)
            ("task_backup_enabled", Some("TASK_BACKUP_ENABLED"), json!(true), false),
            ("task_backup_minutes", Some("TASK_BACKUP_MINUTES"), json!(1440), false),
        ] {
            if !defs.iter().any(|(n, _)| n == k) {
                defs.push((k.to_string(), Def { env: env.map(String::from), default, secret }));
            }
        }
        let cfg = Config { defs: Arc::new(defs), cache: Default::default(), env: Default::default(), db };
        cfg.reload()?;
        Ok(cfg)
    }

    pub fn reload(&self) -> anyhow::Result<()> {
        let stored: HashMap<String, Value> = self
            .db
            .query("SELECT key, value FROM settings", p![])?
            .into_iter()
            .filter_map(|r| {
                let k = r.get("key")?.as_str()?.to_string();
                let v: Value = serde_json::from_str(r.get("value")?.as_str()?).ok()?;
                Some((k, v))
            })
            .collect();
        let mut cache = self.cache.write().unwrap();
        let mut env = self.env.write().unwrap();
        cache.clear();
        env.clear();
        for (key, def) in self.defs.iter() {
            if let Some(name) = &def.env {
                if let Ok(raw) = std::env::var(name) {
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
        if self.str("api_key").is_empty() {
            self.set_many(&json!({ "api_key": crate::auth::random_hex(24) }));
        }
        Ok(())
    }

    pub fn def(&self, key: &str) -> Option<&Def> {
        self.defs.iter().find(|(k, _)| k == key).map(|(_, d)| d)
    }

    pub fn get(&self, key: &str) -> Value {
        if let Some(v) = self.env.read().unwrap().get(key) {
            return v.clone();
        }
        self.cache.read().unwrap().get(key).cloned().unwrap_or(Value::Null)
    }
    pub fn str(&self, key: &str) -> String {
        match self.get(key) {
            Value::String(s) => s,
            Value::Null => String::new(),
            v => v.to_string(),
        }
    }
    pub fn bool(&self, key: &str) -> bool {
        match self.get(key) {
            Value::Bool(b) => b,
            Value::Number(n) => n.as_f64().unwrap_or(0.0) != 0.0,
            Value::String(s) => !s.is_empty(),
            _ => false,
        }
    }
    pub fn int(&self, key: &str) -> i64 {
        self.get(key).as_f64().map(|f| f as i64).unwrap_or(0)
    }
    pub fn float(&self, key: &str) -> f64 {
        self.get(key).as_f64().unwrap_or(0.0)
    }
    pub fn env_locked(&self, key: &str) -> bool {
        self.env.read().unwrap().contains_key(key)
    }

    pub fn set_many(&self, values: &Value) {
        let Some(obj) = values.as_object() else { return };
        for (key, val) in obj {
            let Some(def) = self.def(key) else { continue };
            let v = coerce(&def.default, val);
            let _ = self.db.execute("INSERT OR REPLACE INTO settings VALUES (?,?)", p![key, v.to_string()]);
            self.cache.write().unwrap().insert(key.clone(), v);
        }
    }

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
