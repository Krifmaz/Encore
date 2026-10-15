//! What the app remembers between starts: the server you use and the ones you added before.
//! Stored as config.json in the app's config folder (desktop) or private files (Android).

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::{AppHandle, Manager, Runtime};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The server the app opens: "http://192.168.1.10:8080" (no trailing slash).
    pub server: String,
    /// Every server that worked, newest first.
    pub servers: Vec<String>,
    /// Desktop only: global keyboard shortcuts (Settings → This computer).
    pub hotkeys: Hotkeys,
    /// Desktop only: look for a new version of the app when it starts (and every few hours).
    pub check_updates: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self { server: String::new(), servers: Vec::new(), hotkeys: Hotkeys::default(), check_updates: true }
    }
}

/// Global shortcuts: accelerator strings like "CmdOrCtrl+Alt+P" ("" = not set). They work while Encore is
/// in the background; the keyboard's media keys go through the OS media controls and always work.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Hotkeys {
    pub on: bool,
    pub toggle: String,
    pub next: String,
    pub prev: String,
    pub volume_up: String,
    pub volume_down: String,
}

impl Default for Hotkeys {
    fn default() -> Self {
        Self {
            on: true,
            // letters and Page Up/Down rather than the arrows: Ctrl+Alt+arrows switch workspaces on many Linux desktops
            toggle: "CmdOrCtrl+Alt+P".into(),
            next: "CmdOrCtrl+Alt+N".into(),
            prev: "CmdOrCtrl+Alt+B".into(),
            volume_up: "CmdOrCtrl+Alt+PageUp".into(),
            volume_down: "CmdOrCtrl+Alt+PageDown".into(),
        }
    }
}

impl Hotkeys {
    /// (action, accelerator) for every shortcut that is set.
    pub fn bindings(&self) -> Vec<(&'static str, &str)> {
        [("toggle", &self.toggle), ("next", &self.next), ("prev", &self.prev), ("volume_up", &self.volume_up), ("volume_down", &self.volume_down)]
            .into_iter().filter(|(_, k)| !k.trim().is_empty()).map(|(a, k)| (a, k.trim())).collect()
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_config_files_get_the_default_shortcuts() {
        let s: Settings = serde_json::from_str(r#"{"server":"http://nas:8080","servers":["http://nas:8080"]}"#).unwrap();
        assert_eq!(s.server, "http://nas:8080");
        assert!(s.hotkeys.on && s.check_updates);
        assert_eq!(s.hotkeys.bindings().len(), 5);
        let h = Hotkeys { next: " ".into(), ..Hotkeys::default() };
        assert!(h.bindings().iter().all(|(a, _)| *a != "next"), "an empty shortcut is not bound");
    }
}
