//! Desktop only: Cadenza inside the app. The web app signs the engine in with an app token
//! (EncoreNative.engineLogin), the engine shows up as a device on the server and the music plays through it
//! into the DAC while the window shows the normal Encore page. Config and coefficient cache live under
//! `<app config dir>/cadenza/`. The engine starts lazily on the first login and is shut down when the app exits.

use crate::State;
use cadenza::decode::Source;
use cadenza::embed::EmbeddedPlayer;
use cadenza::engine::{Command, Track};
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
    ensure_started(app, &mut slot)?;
    let player = slot.as_ref().ok_or("engine missing")?;
    let link = player.connect(&server, req.token.trim(), &id);
    Ok(serde_json::json!({ "device_id": link.device_id(), "name": link.device_name() }))
}

/// Start the engine (no Encore link) if it isn't running yet: the offline player uses it on its own.
fn ensure_started<R: Runtime>(app: &AppHandle<R>, slot: &mut Option<EmbeddedPlayer>) -> Result<(), String> {
    if slot.is_none() {
        let player = EmbeddedPlayer::start(config_dir(app), device_name()).map_err(|e| format!("Cadenza could not start: {e}"))?;
        *slot = Some(player);
    }
    Ok(())
}

/// Offline player (ui/offline.html on the desktop): play saved songs `ids` through Cadenza from `index`.
pub fn offline_play<R: Runtime>(app: &AppHandle<R>, ids: &[String], index: usize) -> Result<(), String> {
    let songs = crate::offline::list(app);
    let tracks: Vec<Track> = ids.iter().filter_map(|id| {
        let s = songs.iter().find(|s| &s.id == id)?;
        let path = crate::offline::audio_path(app, id)?;
        Some(Track {
            id: Some(s.id.clone()), source: Source::Path { path }, title: s.title.clone(), artist: s.artist.clone(),
            album: s.album.clone(), ..Default::default()
        })
    }).collect();
    if tracks.is_empty() {
        return Err("Those songs aren't on this computer any more".into());
    }
    let index = index.min(tracks.len() - 1);
    let st = app.state::<State>();
    let mut slot = st.engine.lock().map_err(|_| "engine busy")?;
    ensure_started(app, &mut slot)?;
    let player = slot.as_ref().ok_or("engine missing")?;
    player.engine().play_queue(tracks, index, 0.0);
    Ok(())
}

/// Offline player buttons: toggle / next / prev / stop / seek (seconds) / volume (change, -1…1).
pub fn offline_control<R: Runtime>(app: &AppHandle<R>, action: &str, value: f64) {
    let st = app.state::<State>();
    let Ok(slot) = st.engine.lock() else { return };
    let Some(p) = slot.as_ref() else { return };
    let e = p.engine();
    match action {
        "toggle" => e.send(Command::Toggle),
        "next" => e.send(Command::Next),
        "prev" => e.send(Command::Prev),
        "stop" => e.send(Command::Stop),
        "seek" if value.is_finite() => e.send(Command::Seek(value.max(0.0))),
        "volume" if value.is_finite() => e.set_volume_linear((e.status().volume_linear + value).clamp(0.0, 1.0)),
        _ => {}
    }
}

/// `{state: playing|paused|stopped, id, index, position, duration, volume, error}` of the offline player.
pub fn offline_status<R: Runtime>(app: &AppHandle<R>) -> serde_json::Value {
    let st = app.state::<State>();
    let Ok(slot) = st.engine.lock() else { return serde_json::json!({ "state": "stopped" }) };
    let Some(p) = slot.as_ref() else { return serde_json::json!({ "state": "stopped" }) };
    let s = p.engine().status();
    serde_json::json!({
        "state": s.state, "id": s.track.as_ref().and_then(|t| t.id.clone()), "index": s.index,
        "position": s.position, "duration": s.duration.unwrap_or(0.0), "volume": s.volume_linear, "error": s.error,
    })
}

#[tauri::command(async)]
pub fn offline_play_songs<R: Runtime>(app: AppHandle<R>, ids: Vec<String>, index: usize) -> Result<(), String> {
    offline_play(&app, &ids, index)
}

#[tauri::command(async)]
pub fn offline_engine<R: Runtime>(app: AppHandle<R>, action: String, value: Option<f64>) {
    offline_control(&app, &action, value.unwrap_or(0.0));
}

#[tauri::command(async)]
pub fn offline_engine_status<R: Runtime>(app: AppHandle<R>) -> serde_json::Value {
    offline_status(&app)
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
