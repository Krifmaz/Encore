//! What the app remembers between starts: `<config dir>/encore-desktop/config.json` (the
//! servers you use, the look, the bar buttons, shortcuts, the window). Loaded at start, saved on
//! change. Cadenza's own settings live next to it in `cadenza/config.toml`, written by the engine.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// The same ids and default as web-rs `player::BAR_ACTIONS` / `BAR_DEFAULT`.
pub const BAR_ACTIONS: &[(&str, &str, &str)] = &[
    ("quality", "Quality badge", ""),
    ("shuffle", "Shuffle", "shuffle"),
    ("repeat", "Repeat", "refresh"),
    ("like", "Like", "heart"),
    ("album", "View album", "disc"),
    ("playlist", "Add to playlist", "plus"),
    ("lyrics", "Lyrics", "music"),
    ("share", "Share link", "external"),
    ("queue", "Up next", "list"),
    ("devices", "Other devices", "devices"),
];
/// The web's `encore.hideSkipped` key ("1" / "0"), shared by Home and the artist page.
pub const HIDE_SKIPPED: &str = "encore.hideSkipped";

pub const BAR_DEFAULT: &[&str] = &["quality", "shuffle", "repeat", "like", "lyrics", "queue", "devices"];

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    /// The server the app opens: "http://192.168.1.10:8080" (no trailing slash); "" = none yet.
    pub server: String,
    /// Every server that worked, newest first, with what the app keeps per server.
    pub servers: Vec<Server>,
    /// Theme, accent and background (the web's `encore.appearance`).
    pub appearance: Appearance,
    /// The player bar's buttons, in order (ids from [`BAR_ACTIONS`]).
    pub bar_buttons: Vec<String>,
    /// Global keyboard shortcuts (Settings → This computer).
    pub hotkeys: Hotkeys,
    /// Start with the computer (in the tray).
    pub autostart: bool,
    /// Look for a new version on GitHub at start and every few hours.
    pub check_updates: bool,
    /// The main window as it was last closed.
    pub window: WindowState,
    /// The mini player window is on.
    pub mini_player: bool,
    /// Volume as a 0…1 fader, restored at start.
    pub volume: f64,
    /// Repeat mode of the player: "off", "all", "one".
    pub repeat: String,
    /// Shuffle on.
    pub shuffle: bool,
    /// What the web app keeps in localStorage, under the same keys: `encore.libSort` (the
    /// library sort per tab), `encore.hideSkipped`, `encore.artistFilter`, `encore.albumMode`
    /// (the Downloader), `encore.bugFilter`.
    pub local: BTreeMap<String, Value>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server: String::new(),
            servers: Vec::new(),
            appearance: Appearance::default(),
            bar_buttons: BAR_DEFAULT.iter().map(|s| s.to_string()).collect(),
            hotkeys: Hotkeys::default(),
            autostart: false,
            check_updates: true,
            window: WindowState::default(),
            mini_player: false,
            volume: 1.0,
            repeat: "off".into(),
            shuffle: false,
            local: BTreeMap::new(),
        }
    }
}

/// One server the app knows.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Server {
    /// "http://host:port", no trailing slash.
    pub url: String,
    /// The signed-in user's name (shown in the picker).
    pub username: String,
    /// The `encore_session` cookie, so the app stays signed in between starts ("" = signed out).
    pub cookie: String,
    /// The app token the engine link and the stream use (`POST /api/auth/app-token`).
    pub token: String,
    /// The device id the token was issued for.
    pub device_id: String,
}

/// The web's `encore.appearance` object.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Appearance {
    /// `encore`, `dark`, `oled`, `light`, `system`.
    pub theme: String,
    /// An accent id or `custom`.
    pub accent: String,
    /// `#rrggbb` for `custom`.
    pub custom: String,
    /// `solid` or `fade`.
    pub background: String,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            theme: "encore".into(),
            accent: "logo".into(),
            custom: "#f0ead6".into(),
            background: "solid".into(),
        }
    }
}

/// Global shortcuts: accelerator strings like "CmdOrCtrl+Alt+P" ("" = not set). Same defaults as
/// the classic app had (removed in 0.29).
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
        [
            ("toggle", &self.toggle),
            ("next", &self.next),
            ("prev", &self.prev),
            ("volume_up", &self.volume_up),
            ("volume_down", &self.volume_down),
        ]
        .into_iter()
        .filter(|(_, k)| !k.trim().is_empty())
        .map(|(a, k)| (a, k.trim()))
        .collect()
    }
}

/// Size and position of the main window.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowState {
    pub width: f32,
    pub height: f32,
    pub x: Option<f32>,
    pub y: Option<f32>,
    pub maximized: bool,
}

impl Default for WindowState {
    fn default() -> Self {
        Self {
            width: 1280.0,
            height: 820.0,
            x: None,
            y: None,
            maximized: false,
        }
    }
}

static DIR: OnceLock<PathBuf> = OnceLock::new();

/// Use `dir` instead of the platform config directory (the screenshot tour and tests).
pub fn set_dir(dir: PathBuf) {
    let _ = DIR.set(dir);
}

/// `<config dir>/encore-desktop` (`~/.config/encore-desktop`, `%APPDATA%\encore-desktop`,
/// `~/Library/Application Support/encore-desktop`), or `ENCORE_DESKTOP_CONFIG_DIR`.
pub fn dir() -> PathBuf {
    DIR.get_or_init(|| {
        if let Some(d) = std::env::var_os("ENCORE_DESKTOP_CONFIG_DIR") {
            return PathBuf::from(d);
        }
        directories::BaseDirs::new()
            .map(|b| b.config_dir().join("encore-desktop"))
            .unwrap_or_else(|| std::env::temp_dir().join("encore-desktop"))
    })
    .clone()
}

/// `<data dir>/encore-desktop` for the offline songs (next to the config dir on Linux).
pub fn data_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("ENCORE_DESKTOP_DATA_DIR") {
        return PathBuf::from(d);
    }
    if DIR.get().is_some() || std::env::var_os("ENCORE_DESKTOP_CONFIG_DIR").is_some() {
        return dir().join("data");
    }
    directories::BaseDirs::new()
        .map(|b| b.data_local_dir().join("encore-desktop"))
        .unwrap_or_else(|| std::env::temp_dir().join("encore-desktop"))
}

/// Where the embedded Cadenza keeps `config.toml` and its coefficient cache.
pub fn cadenza_dir() -> PathBuf {
    dir().join("cadenza")
}

pub fn path() -> PathBuf {
    dir().join("config.json")
}

/// `<file>.<ext>` next to `file` (`config.json.bak`, `config.json.tmp`).
fn sibling(file: &Path, ext: &str) -> PathBuf {
    let mut name = file.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(".");
    name.push(ext);
    file.with_file_name(name)
}

/// Write a file only this user can read (it holds a session cookie and an app token) so that
/// a crash or a power cut never leaves half of it: the bytes go to `<file>.tmp` (0600 on unix,
/// in a 0700 folder), are flushed to disk, and the temp file is renamed over `file`. The file
/// it replaces is kept as `<file>.bak`. On Windows the folder under %APPDATA% is per user.
pub fn write_private(file: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = file.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
        }
    }
    let tmp = sibling(file, "tmp");
    let mut open = std::fs::OpenOptions::new();
    open.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        open.mode(0o600);
    }
    {
        let mut f = open.open(&tmp)?;
        // `mode` only applies to a new file: one left behind by an older version keeps its own.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    if file.exists() {
        let bak = sibling(file, "bak");
        if std::fs::rename(file, &bak).is_ok() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&bak, std::fs::Permissions::from_mode(0o600));
            }
        }
    }
    std::fs::rename(&tmp, file)
}

/// The config in `file`, else in `<file>.bak` (a save cut off half-way, a file that does not
/// parse); None when neither gives one.
fn read_from(file: &Path) -> Option<Config> {
    let parse = |p: &Path| -> Option<Config> {
        let bytes = std::fs::read(p).ok()?;
        serde_json::from_slice(&bytes)
            .map_err(|e| log::warn!("{} does not parse: {e}", p.display()))
            .ok()
    };
    parse(file).or_else(|| {
        let bak = sibling(file, "bak");
        let c = parse(&bak)?;
        log::warn!("using {}", bak.display());
        Some(c)
    })
}

/// "192.168.1.10:8080" → "http://192.168.1.10:8080"; trailing slashes dropped.
pub fn normalize(raw: &str) -> String {
    let t = raw.trim();
    let with_scheme = if t.starts_with("http://") || t.starts_with("https://") {
        t.to_string()
    } else {
        format!("http://{t}")
    };
    with_scheme.trim_end_matches('/').to_string()
}

impl Config {
    /// The saved file (or the copy kept from the save before it), or the defaults when there
    /// is neither.
    pub fn load() -> Config {
        let mut c: Config = read_from(&path()).unwrap_or_default();
        c.bar_buttons
            .retain(|id| BAR_ACTIONS.iter().any(|(k, _, _)| k == id));
        c.volume = c.volume.clamp(0.0, 1.0);
        c
    }

    /// Write the file with [`write_private`] (the folder is created). Errors are logged, not
    /// raised: a config that cannot be saved must not stop the app.
    pub fn save(&self) {
        static SAVING: Mutex<()> = Mutex::new(());
        let _one_at_a_time = SAVING.lock().unwrap_or_else(|e| e.into_inner());
        let p = path();
        match serde_json::to_vec_pretty(self) {
            Ok(b) => {
                if let Err(e) = write_private(&p, &b) {
                    log::warn!("could not save {}: {e}", p.display());
                }
            }
            Err(e) => log::warn!("could not encode the config: {e}"),
        }
    }

    /// The current server's record.
    pub fn current(&self) -> Option<&Server> {
        self.servers.iter().find(|s| s.url == self.server)
    }

    /// The record of `url`, created when missing, moved to the top as the current server.
    pub fn remember(&mut self, url: &str) -> &mut Server {
        let url = normalize(url);
        if let Some(i) = self.servers.iter().position(|s| s.url == url) {
            let s = self.servers.remove(i);
            self.servers.insert(0, s);
        } else {
            self.servers.insert(
                0,
                Server {
                    url: url.clone(),
                    ..Server::default()
                },
            );
        }
        self.server = url;
        &mut self.servers[0]
    }

    /// The record of `url`, if known.
    pub fn server_mut(&mut self, url: &str) -> Option<&mut Server> {
        self.servers.iter_mut().find(|s| s.url == url)
    }

    /// Drop a saved server (the current one too: the app shows the picker).
    pub fn forget(&mut self, url: &str) {
        self.servers.retain(|s| s.url != url);
        if self.server == url {
            self.server.clear();
        }
    }

    /// A value the web keeps in localStorage (`encore.libSort` …).
    pub fn local(&self, key: &str) -> Option<&Value> {
        self.local.get(key)
    }

    /// A string the web keeps in localStorage ("" when unset).
    pub fn local_str(&self, key: &str) -> &str {
        self.local(key).and_then(Value::as_str).unwrap_or("")
    }

    /// The web's `encore.hideSkipped` (on unless it was turned off).
    pub fn hide_skipped(&self) -> bool {
        self.local_str(HIDE_SKIPPED) != "0"
    }

    /// The bar buttons in order, every id known (the default when the list is empty).
    pub fn bar_buttons(&self) -> Vec<String> {
        if self.bar_buttons.is_empty() {
            BAR_DEFAULT.iter().map(|s| s.to_string()).collect()
        } else {
            self.bar_buttons.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remember_moves_the_server_to_the_top_and_keeps_its_record() {
        let mut c = Config::default();
        c.remember("http://a:8080/").token = "t1".into();
        c.remember("b:8080");
        assert_eq!(c.server, "http://b:8080");
        assert_eq!(c.servers.iter().map(|s| s.url.as_str()).collect::<Vec<_>>(), ["http://b:8080", "http://a:8080"]);
        c.remember("http://a:8080");
        assert_eq!(c.servers[0].token, "t1", "the record travels with the server");
        c.forget("http://a:8080");
        assert!(c.server.is_empty() && c.servers.len() == 1);
    }

    #[test]
    fn saves_are_private_whole_and_keep_the_previous_file() {
        let d = tempfile::tempdir().unwrap();
        let file = d.path().join("encore-desktop").join("config.json");
        let mut c = Config::default();
        c.remember("http://a:8080").token = "t1".into();
        write_private(&file, &serde_json::to_vec(&c).unwrap()).unwrap();
        c.remember("http://a:8080").token = "t2".into();
        write_private(&file, &serde_json::to_vec(&c).unwrap()).unwrap();
        assert_eq!(read_from(&file).unwrap().servers[0].token, "t2");
        assert!(!sibling(&file, "tmp").exists(), "the temp file became the file");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&file), 0o600);
            assert_eq!(mode(&sibling(&file, "bak")), 0o600);
            assert_eq!(mode(file.parent().unwrap()), 0o700);
        }
        // a file cut off half-way: the copy from the save before is used
        std::fs::write(&file, b"{\"server\": \"http://a:80").unwrap();
        assert_eq!(read_from(&file).unwrap().servers[0].token, "t1");
        std::fs::remove_file(&file).unwrap();
        assert_eq!(read_from(&file).unwrap().servers[0].token, "t1");
        std::fs::remove_file(sibling(&file, "bak")).unwrap();
        assert!(read_from(&file).is_none());
    }

    #[test]
    fn old_files_get_the_defaults() {
        let c: Config = serde_json::from_str(r#"{"server":"http://nas:8080"}"#).unwrap();
        assert_eq!(c.bar_buttons, BAR_DEFAULT);
        assert!(c.hotkeys.on && c.check_updates);
        assert_eq!(c.hotkeys.bindings().len(), 5);
        assert_eq!(c.appearance.theme, "encore");
        assert_eq!(c.window.width, 1280.0);
        assert!(c.hide_skipped() && c.local_str("encore.albumMode").is_empty());
        let c: Config = serde_json::from_str(r#"{"local":{"encore.hideSkipped":"0","encore.albumMode":"album"}}"#).unwrap();
        assert!(!c.hide_skipped());
        assert_eq!(c.local_str("encore.albumMode"), "album");
    }
}
