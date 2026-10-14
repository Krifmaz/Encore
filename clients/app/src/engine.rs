//! Desktop only: Cadenza inside the app. The web app signs the engine in with an app token
//! (EncoreNative.engineLogin), the engine shows up as a device on the server and the music plays through it
//! into the DAC while the window shows the normal Encore page. Config and coefficient cache live under
//! `<app config dir>/cadenza/`. The engine starts lazily on the first login and is shut down when the app exits.

use crate::State;
use cadenza::embed::EmbeddedPlayer;
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{AppHandle, Manager, Runtime};

/// The player, once started (see `login`).
pub type Slot = Mutex<Option<EmbeddedPlayer>>;

pub fn slot() -> Slot {
    Mutex::new(None)
}

/// FNV-1a over `s`: the same hash Cadenza's own `cadenza serve` uses for its device ids.
pub fn fnv1a(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3))
}

pub fn hostname() -> String {
    let h = gethostname::gethostname().to_string_lossy().trim().to_string();
    if h.is_empty() { "this computer".into() } else { h }
}

/// "Cadenza on <hostname>": what the server lists the engine as.
pub fn device_name() -> String {
    format!("Cadenza on {}", hostname())
}

/// "cadenza-desktop-<fnv1a(hostname|server)>" (32 chars): stable per computer and server, so a reinstall
/// gets the same device and the same link in the web app.
pub fn device_id(server: &str) -> String {
    format!("cadenza-desktop-{:016x}", fnv1a(&format!("{}|{}", hostname(), server.trim().trim_end_matches('/'))))
}

fn config_dir<R: Runtime>(app: &AppHandle<R>) -> PathBuf {
    app.path().app_config_dir().unwrap_or_else(|_| std::env::temp_dir().join("encore")).join("cadenza")
}

fn current_server<R: Runtime>(app: &AppHandle<R>) -> String {
    app.state::<State>().settings.lock().map(|s| s.server.clone()).unwrap_or_default()
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Login {
    token: String,
    server: String,
}

/// Start the engine if needed and point its Encore client at `server` with `token`. Idempotent: the same
/// server and token keep the running link. Returns `{device_id, name}`.
pub fn login<R: Runtime>(app: &AppHandle<R>, json: &str) -> Result<serde_json::Value, String> {
    let req: Login = serde_json::from_str(json).map_err(|e| format!("bad engineLogin request: {e}"))?;
    let server = if req.server.trim().is_empty() { current_server(app) } else { crate::config::normalize(&req.server) };
    if req.token.trim().is_empty() {
        return Err("engineLogin needs a token".into());
    }
    if server.len() <= "http://".len() {
        return Err("engineLogin needs a server".into());
    }
    let id = device_id(&server);
    let st = app.state::<State>();
    let mut slot = st.engine.lock().map_err(|_| "engine busy")?;
    if slot.is_none() {
        let dir = config_dir(app);
        let player = EmbeddedPlayer::start(dir.clone(), device_name()).map_err(|e| format!("Cadenza could not start: {e}"))?;
        eprintln!("Cadenza engine started, config in {}", player.config_path().display());
        *slot = Some(player);
    }
    let player = slot.as_ref().ok_or("engine missing")?;
    let link = player.connect(&server, req.token.trim(), &id);
    Ok(serde_json::json!({ "device_id": link.device_id(), "name": link.device_name() }))
}

/// Stop the Encore client; the engine keeps its settings and stops playing. Nothing happens when the engine
/// was never started.
pub fn logout<R: Runtime>(app: &AppHandle<R>) {
    if let Ok(slot) = app.state::<State>().engine.lock() {
        if let Some(p) = slot.as_ref() {
            p.disconnect();
        }
    }
}

/// `{running, device_id, name, error}`. Before the first login (or when the engine isn't running) the id and
/// name are the ones a login for the current server would use, so the web app can ask for a token for them.
pub fn status<R: Runtime>(app: &AppHandle<R>) -> serde_json::Value {
    let fallback = |error: Option<String>| {
        serde_json::json!({ "running": false, "device_id": device_id(&current_server(app)), "name": device_name(), "error": error })
    };
    let st = app.state::<State>();
    let Ok(slot) = st.engine.lock() else { return fallback(Some("engine busy".into())) };
    let Some(p) = slot.as_ref() else { return fallback(None) };
    let mut v = p.status_json();
    if v.get("device_id").map(|d| d.is_null()).unwrap_or(true) {
        v["device_id"] = serde_json::Value::String(device_id(&current_server(app)));
    }
    v
}

/// App exit: stop the link and the engine so the DAC is released.
pub fn shutdown<R: Runtime>(app: &AppHandle<R>) {
    let player = app.state::<State>().engine.lock().ok().and_then(|mut s| s.take());
    if let Some(p) = player {
        p.shutdown();
    }
}

#[tauri::command(async)]
pub fn engine_login<R: Runtime>(app: AppHandle<R>, json: String) -> Result<serde_json::Value, String> {
    login(&app, &json)
}

#[tauri::command(async)]
pub fn engine_logout<R: Runtime>(app: AppHandle<R>) {
    logout(&app);
}

#[tauri::command(async)]
pub fn engine_status<R: Runtime>(app: AppHandle<R>) -> serde_json::Value {
    status(&app)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_id_is_stable_short_and_follows_the_server() {
        let a = device_id("http://nas:8080");
        assert_eq!(a, device_id("http://nas:8080/"), "a trailing slash changes nothing");
        assert_eq!(a, device_id(" http://nas:8080 "));
        assert!(a.starts_with("cadenza-desktop-"));
        assert_eq!(a.len(), 32);
        assert!(a.len() <= 64);
        assert_ne!(a, device_id("https://music.example.org"));
        assert!(device_name().starts_with("Cadenza on "));
    }

    #[test]
    fn fnv1a_matches_the_reference_vectors() {
        assert_eq!(fnv1a(""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a("a"), 0xaf63_dc4c_8601_ec8c);
    }

    #[test]
    fn login_needs_a_token_and_a_server() {
        let r: Login = serde_json::from_str(r#"{"token":"t","server":"http://x"}"#).unwrap();
        assert_eq!((r.token.as_str(), r.server.as_str()), ("t", "http://x"));
        let r: Login = serde_json::from_str("{}").unwrap();
        assert!(r.token.is_empty() && r.server.is_empty());
    }
}
