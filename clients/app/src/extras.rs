//! Desktop only: what Settings → This computer controls, plus the mini player.
//! * global shortcuts (tauri-plugin-global-shortcut): play / pause, next, previous, volume up / down
//! * start with the computer, in the tray (tauri-plugin-autostart, started with `--minimized`)
//! * updates from the GitHub release (tauri-plugin-updater; only when the build carries the updater public key)
//! * the mini player: a small always-on-top window (ui/mini.html) with cover, song, controls and seek bar
//!
//! The web app reaches all of it through window.EncoreNative (bridge.rs): desktopSettings(),
//! setDesktopSettings(json), miniPlayer(), checkUpdate(), installUpdate().

use crate::bridge::{control, control_with};
use crate::config::{self, Hotkeys};
use crate::{State, VERSION};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;
use tauri::{AppHandle, Manager, Runtime, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_autostart::ManagerExt as _;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutEvent, ShortcutState};

/// The argument the OS starts us with at log-in: no window, just the tray.
pub const MINIMIZED_ARG: &str = "--minimized";

/// Volume step of the volume shortcuts (of 0…1).
const VOLUME_STEP: f64 = 0.05;

/// Registered shortcut id → action, and the shortcuts that could not be registered (action → why).
static BOUND: Mutex<Option<HashMap<u32, &'static str>>> = Mutex::new(None);
static KEY_ERRORS: Mutex<BTreeMap<String, String>> = Mutex::new(BTreeMap::new());
/// The newer version the last check found (shown in Settings, announced once).
static AVAILABLE: Mutex<Option<String>> = Mutex::new(None);

pub fn started_minimized() -> bool {
    std::env::args().any(|a| a == MINIMIZED_ARG)
}

// ---- plugins ----------------------------------------------------------------------------------------------

pub fn shortcut_plugin<R: Runtime>() -> tauri::plugin::TauriPlugin<R> {
    tauri_plugin_global_shortcut::Builder::new()
        .with_handler(|app: &AppHandle<R>, shortcut: &Shortcut, event: ShortcutEvent| {
            if event.state != ShortcutState::Pressed {
                return;
            }
            let action = BOUND.lock().ok().and_then(|b| b.as_ref().and_then(|m| m.get(&shortcut.id()).copied()));
            if let Some(a) = action {
                run_action(app, a);
            }
        })
        .build()
}

pub fn autostart_plugin<R: Runtime>() -> tauri::plugin::TauriPlugin<R> {
    let b = tauri_plugin_autostart::Builder::new().args([MINIMIZED_ARG]);
    #[cfg(target_os = "macos")]
    let b = b.macos_launcher(tauri_plugin_autostart::MacosLauncher::LaunchAgent);
    b.build()
}

pub fn updater_plugin<R: Runtime>() -> tauri::plugin::TauriPlugin<R, tauri_plugin_updater::Config> {
    tauri_plugin_updater::Builder::new().build()
}

/// One shortcut or menu action: toggle / next / prev / volume_up / volume_down / mini.
pub fn run_action<R: Runtime>(app: &AppHandle<R>, action: &str) {
    match action {
        "volume_up" => control_with(app, "volume", VOLUME_STEP),
        "volume_down" => control_with(app, "volume", -VOLUME_STEP),
        "mini" => toggle_mini(app),
        a => control(app, a),
    }
}

// ---- shortcuts --------------------------------------------------------------------------------------------

/// What a shortcut string means to the user when it can't be used.
fn parse(key: &str) -> Result<Shortcut, String> {
    key.parse::<Shortcut>().map_err(|_| format!("“{key}” isn't a shortcut Encore understands"))
}

/// (Re-)register the shortcuts. Returns action → problem for the ones that didn't work.
pub fn apply_hotkeys<R: Runtime>(app: &AppHandle<R>, h: &Hotkeys) -> BTreeMap<String, String> {
    let gs = app.global_shortcut();
    let _ = gs.unregister_all();
    let mut bound = HashMap::new();
    let mut errors = BTreeMap::new();
    if h.on {
        for (action, key) in h.bindings() {
            match parse(key) {
                Err(e) => { errors.insert(action.to_string(), e); }
                Ok(sc) if bound.contains_key(&sc.id()) => { errors.insert(action.to_string(), format!("{key} is already used above")); }
                Ok(sc) => match gs.register(sc) {
                    Ok(()) => { bound.insert(sc.id(), action); }
                    Err(_) => { errors.insert(action.to_string(), format!("{key} is taken by another program")); }
                },
            }
        }
    }
    if let Ok(mut b) = BOUND.lock() { *b = Some(bound); }
    if let Ok(mut e) = KEY_ERRORS.lock() { *e = errors.clone(); }
    errors
}

// ---- settings ---------------------------------------------------------------------------------------------

fn updater_configured<R: Runtime>(app: &AppHandle<R>) -> bool {
    app.config().plugins.0.get("updater").and_then(|u| u.get("pubkey")).and_then(|p| p.as_str()).map(|p| !p.trim().is_empty()).unwrap_or(false)
}

/// `desktopSettings()`: {autostart, hotkeys: {on, toggle, next, prev, volume_up, volume_down}, hotkey_errors,
/// check_updates, updates (the build can update itself), update (newer version found, or null), version,
/// offline (songs saved), platform}.
pub fn settings_json<R: Runtime>(app: &AppHandle<R>) -> Value {
    let s = app.state::<State>().settings.lock().map(|s| s.clone()).unwrap_or_default();
    let platform = if cfg!(target_os = "windows") { "windows" } else if cfg!(target_os = "macos") { "macos" } else { "linux" };
    json!({
        "autostart": app.autolaunch().is_enabled().unwrap_or(false),
        "hotkeys": s.hotkeys,
        "hotkey_errors": KEY_ERRORS.lock().map(|e| e.clone()).unwrap_or_default(),
        "default_hotkeys": Hotkeys::default(),
        "check_updates": s.check_updates,
        "updates": updater_configured(app),
        "update": AVAILABLE.lock().ok().and_then(|a| a.clone()),
        "version": VERSION,
        "offline": crate::offline::list(app).len(),
        "platform": platform,
    })
}

/// What `setDesktopSettings(json)` may change; missing fields stay as they are.
#[derive(Deserialize, Default)]
#[serde(default)]
struct Patch {
    autostart: Option<bool>,
    hotkeys: Option<Hotkeys>,
    check_updates: Option<bool>,
}

pub fn set_settings<R: Runtime>(app: &AppHandle<R>, json: &str) -> Result<Value, String> {
    let p: Patch = serde_json::from_str(json).map_err(|e| format!("bad settings: {e}"))?;
    if let Some(on) = p.autostart {
        let al = app.autolaunch();
        let r = if on { al.enable() } else { al.disable() };
        r.map_err(|e| format!("Couldn't change start with the computer: {e}"))?;
    }
    let mut hotkeys = None;
    {
        let st = app.state::<State>();
        let mut s = st.settings.lock().map_err(|_| "settings busy")?;
        if let Some(h) = p.hotkeys {
            s.hotkeys = Hotkeys { toggle: h.toggle.trim().into(), next: h.next.trim().into(), prev: h.prev.trim().into(),
                volume_up: h.volume_up.trim().into(), volume_down: h.volume_down.trim().into(), on: h.on };
            hotkeys = Some(s.hotkeys.clone());
        }
        if let Some(c) = p.check_updates {
            s.check_updates = c;
        }
        config::save(app, &s);
    }
    if let Some(h) = hotkeys {
        apply_hotkeys(app, &h);
    }
    Ok(settings_json(app))
}

// ---- updates ----------------------------------------------------------------------------------------------

/// `checkUpdate()`: {available, version, notes, current, updates, error}.
pub async fn check_for_update<R: Runtime>(app: &AppHandle<R>) -> Value {
    use tauri_plugin_updater::UpdaterExt;
    if !updater_configured(app) {
        return json!({ "available": false, "updates": false, "current": VERSION });
    }
    let found = match app.updater() {
        Ok(u) => u.check().await.map_err(|e| e.to_string()),
        Err(e) => Err(e.to_string()),
    };
    match found {
        Ok(Some(u)) => {
            if let Ok(mut a) = AVAILABLE.lock() { *a = Some(u.version.clone()); }
            json!({ "available": true, "version": u.version, "notes": u.body.unwrap_or_default(), "current": VERSION, "updates": true })
        }
        Ok(None) => {
            if let Ok(mut a) = AVAILABLE.lock() { *a = None; }
            json!({ "available": false, "current": VERSION, "updates": true })
        }
        Err(e) => json!({ "available": false, "current": VERSION, "updates": true, "error": format!("Couldn't check for updates: {e}") }),
    }
}

/// `installUpdate()`: download, install and restart. Only returns when something went wrong.
pub async fn install_now<R: Runtime>(app: &AppHandle<R>) -> Value {
    use tauri_plugin_updater::UpdaterExt;
    if !updater_configured(app) {
        return json!({ "ok": false, "error": "This build can't update itself. Download the new version from the releases page." });
    }
    let update = match app.updater() {
        Ok(u) => u.check().await.map_err(|e| e.to_string()),
        Err(e) => Err(e.to_string()),
    };
    let update = match update {
        Ok(Some(u)) => u,
        Ok(None) => return json!({ "ok": false, "error": "Encore is up to date." }),
        Err(e) => return json!({ "ok": false, "error": format!("Couldn't check for updates: {e}") }),
    };
    if let Err(e) = update.download_and_install(|_, _| {}, || {}).await {
        return json!({ "ok": false, "error": format!("The update didn't install: {e}") });
    }
    // the engine lets go of the DAC before the restart
    crate::engine::shutdown(app);
    app.restart();
}

/// At start and every 6 hours: look for a new version and say so once per version (a toast in the web app).
pub fn watch_updates<R: Runtime>(app: &AppHandle<R>) {
    if !updater_configured(app) {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut told = String::new();
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        loop {
            let on = app.state::<State>().settings.lock().map(|s| s.check_updates).unwrap_or(false);
            if on {
                let r = check_for_update(&app).await;
                let v = r.get("version").and_then(|v| v.as_str()).unwrap_or_default().to_string();
                if r.get("available").and_then(|a| a.as_bool()).unwrap_or(false) && v != told {
                    crate::bridge::notify(&app, &format!("Encore {v} is ready to install. Settings → This computer → Updates."), "ok");
                    told = v;
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(6 * 3600)).await;
        }
    });
}

// ---- mini player ------------------------------------------------------------------------------------------

pub const MINI: &str = "mini";

/// Open the mini player, or close it when it is open.
pub fn toggle_mini<R: Runtime>(app: &AppHandle<R>) {
    if let Some(w) = app.get_webview_window(MINI) {
        let _ = w.close();
        return;
    }
    let built = WebviewWindowBuilder::new(app, MINI, WebviewUrl::App("mini.html".into()))
        .title("Encore mini player")
        .inner_size(380.0, 132.0)
        .min_inner_size(300.0, 132.0)
        .max_inner_size(720.0, 132.0)
        .always_on_top(true)
        .decorations(false)
        .skip_taskbar(true)
        .background_color(tauri::window::Color(34, 34, 36, 255))
        .build();
    if let Err(e) = built {
        eprintln!("mini player: {e}");
    }
}

/// `{title, artist, album, art, playing, stopped, position, duration}` for ui/mini.html (polled).
pub fn mini_state_json<R: Runtime>(app: &AppHandle<R>) -> Value {
    let st = app.state::<State>();
    let np = st.last.lock().map(|l| l.clone()).unwrap_or_default();
    let art = st.art.lock().ok().and_then(|a| (!np.art.is_empty() && a.0 == np.art && !a.1.as_os_str().is_empty()).then(|| crate::offline::asset_url(&a.1)));
    json!({
        "title": np.title, "artist": np.artist, "album": np.album, "art": art.unwrap_or_default(),
        "playing": np.playing, "stopped": np.stopped || np.title.is_empty(),
        "position": crate::desktop::position_now(&np), "duration": np.duration,
    })
}

// ---- commands ---------------------------------------------------------------------------------------------

#[tauri::command(async)]
pub fn desktop_settings<R: Runtime>(app: AppHandle<R>) -> Value {
    settings_json(&app)
}

#[tauri::command(async)]
pub fn set_desktop_settings<R: Runtime>(app: AppHandle<R>, json: String) -> Result<Value, String> {
    set_settings(&app, &json)
}

#[tauri::command(async)]
pub fn mini_player<R: Runtime>(app: AppHandle<R>) {
    // windows are made on the main thread (a window built from a command thread can hang on Windows)
    let h = app.clone();
    let _ = app.run_on_main_thread(move || toggle_mini(&h));
}

#[tauri::command(async)]
pub fn mini_state<R: Runtime>(app: AppHandle<R>) -> Value {
    mini_state_json(&app)
}

/// The mini player's buttons and seek bar: toggle / next / prev / open / seek (seconds) / volume (change).
#[tauri::command(async)]
pub fn player_control<R: Runtime>(app: AppHandle<R>, action: String, value: Option<f64>) {
    if action == "open" {
        crate::desktop::show_window(&app);
    }
    control_with(&app, &action, value.unwrap_or(0.0));
    if action == "seek" {
        // move the reported position right away, so the bar doesn't jump back until the page reports
        if let Ok(mut l) = app.state::<State>().last.lock() {
            l.position = value.unwrap_or(0.0).max(0.0);
        }
        crate::desktop::reported_now();
    }
}

#[tauri::command]
pub async fn check_update<R: Runtime>(app: AppHandle<R>) -> Result<Value, String> {
    Ok(check_for_update(&app).await)
}

#[tauri::command]
pub async fn install_update<R: Runtime>(app: AppHandle<R>) -> Result<Value, String> {
    Ok(install_now(&app).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_updater_is_configured_but_keyless_in_the_repo() {
        // the release workflow fills in the public key (TAURI_UPDATER_PUBKEY); a build without it can't update itself
        let conf: Value = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let u = &conf["plugins"]["updater"];
        assert_eq!(u["endpoints"][0], "https://github.com/Krifmaz/Encore/releases/latest/download/latest.json");
        assert_eq!(u["pubkey"], "");
        assert!(conf["app"]["security"]["assetProtocol"]["scope"].as_array().unwrap().iter().any(|s| s == "$APPCACHE/now-playing-*.jpg"));
    }

    #[test]
    fn default_shortcuts_parse() {
        for (_, k) in Hotkeys::default().bindings() {
            assert!(parse(k).is_ok(), "{k}");
        }
        // what Settings → This computer records (web/js/pages/computer.js accelerator())
        for k in ["Ctrl+Alt+P", "Ctrl+Shift+Up", "Alt+1", "Super+Shift+F7", "Ctrl+Alt+Space", "Ctrl+Alt+KeyP", "Ctrl+Shift+ArrowUp", "Ctrl+Alt+Comma"] {
            assert!(parse(k).is_ok(), "{k}");
        }
        assert!(parse("Ctrl+Nonsense").is_err());
    }

    #[test]
    fn a_patch_changes_only_what_it_names() {
        let p: Patch = serde_json::from_str(r#"{"check_updates":false}"#).unwrap();
        assert!(p.autostart.is_none() && p.hotkeys.is_none());
        assert_eq!(p.check_updates, Some(false));
        let p: Patch = serde_json::from_str(r#"{"hotkeys":{"on":false}}"#).unwrap();
        let h = p.hotkeys.unwrap();
        assert!(!h.on);
        assert_eq!(h.toggle, Hotkeys::default().toggle, "missing keys keep their defaults");
    }
}
