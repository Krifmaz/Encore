//! What the app remembers between starts: the server you use and the ones you added before.
//! Stored as config.json in the app's config folder (desktop) or private files (Android).

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::{AppHandle, Manager, Runtime};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The server the app opens: "http://192.168.1.10:8080" (no trailing slash).
    pub server: String,
    /// Every server that worked, newest first.
    pub servers: Vec<String>,
}

pub fn path<R: Runtime>(app: &AppHandle<R>) -> PathBuf {
    let dir = app.path().app_config_dir().unwrap_or_else(|_| std::env::temp_dir().join("encore"));
    let _ = std::fs::create_dir_all(&dir);
    dir.join("config.json")
}

pub fn load<R: Runtime>(app: &AppHandle<R>) -> Settings {
    std::fs::read(path(app)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

pub fn save<R: Runtime>(app: &AppHandle<R>, s: &Settings) {
    if let Ok(b) = serde_json::to_vec_pretty(s) {
        let _ = std::fs::write(path(app), b);
    }
}

/// "192.168.1.10:8080" → "http://192.168.1.10:8080"; trailing slashes dropped.
pub fn normalize(raw: &str) -> String {
    let t = raw.trim();
    let with_scheme = if t.starts_with("http://") || t.starts_with("https://") { t.to_string() } else { format!("http://{t}") };
    with_scheme.trim_end_matches('/').to_string()
}

impl Settings {
    /// Make `base` the current server and move it to the top of the list.
    pub fn remember(&mut self, base: &str) {
        self.servers.retain(|s| s != base);
        self.servers.insert(0, base.to_string());
        self.server = base.to_string();
    }

    pub fn forget(&mut self, base: &str) {
        self.servers.retain(|s| s != base);
    }

    /// The saved servers plus the current one, for the picker page.
    pub fn list(&self) -> Vec<String> {
        let mut l = self.servers.clone();
        if !self.server.is_empty() && !l.contains(&self.server) {
            l.push(self.server.clone());
        }
        l
    }
}
