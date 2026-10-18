//! `window.EncoreNative`: the bridge the apps hang on the window – the Tauri desktop app (Cadenza engine, mini
//! player, server picker) and the Android app (now playing, songs kept offline). One typed function per call
//! the web app makes, over the untyped `native_has` / `native_call` helpers (which the pages still use through
//! `app::`). In a plain browser there is no bridge: every check says no and every call does nothing.

use crate::dom;
use serde_json::{json, Value};
use wasm_bindgen::{JsCast, JsValue};

/// `window.EncoreNative` when the app provides it.
fn bridge() -> Option<JsValue> {
    js_sys::Reflect::get(&dom::window(), &"EncoreNative".into()).ok().filter(|v| !v.is_undefined() && !v.is_null())
}

/// `!!window.EncoreNative?.<name>` – a function the app provides.
pub fn native_has(name: &str) -> bool {
    bridge().and_then(|n| js_sys::Reflect::get(&n, &name.into()).ok()).map(|f| f.is_function()).unwrap_or(false)
}
/// `!!window.EncoreNative?.<name>` for a plain value (e.g. `engine: true`), not only functions.
pub fn native_truthy(name: &str) -> bool {
    bridge().and_then(|n| js_sys::Reflect::get(&n, &name.into()).ok()).map(|v| v.is_truthy()).unwrap_or(false)
}
/// `window.EncoreNative.<name>(arg)` when the app provides it.
pub fn native_call(name: &str) { native_call_with(name, None); }
pub fn native_call_with(name: &str, arg: Option<&str>) {
    let Some(n) = bridge() else { return };
    let Ok(f) = js_sys::Reflect::get(&n, &name.into()) else { return };
    let Ok(f) = f.dyn_into::<js_sys::Function>() else { return };
    let _ = match arg { Some(a) => f.call1(&n, &a.into()), None => f.call0(&n) };
}
/// `await window.EncoreNative.<name>(arg)` as JSON – None when the app has no such function or it failed.
/// A JSON string answer is parsed like the JS does (`typeof s === 'string' ? JSON.parse(s) : s`).
pub async fn native_call_async(name: &str, arg: Option<&str>) -> Option<Value> {
    let n = bridge()?;
    let f = js_sys::Reflect::get(&n, &name.into()).ok()?.dyn_into::<js_sys::Function>().ok()?;
    let r = match arg { Some(a) => f.call1(&n, &a.into()), None => f.call0(&n) }.ok()?;
    let v = match r.dyn_into::<js_sys::Promise>() { Ok(p) => wasm_bindgen_futures::JsFuture::from(p).await.ok()?, Err(v) => v };
    if let Some(s) = v.as_string() { return serde_json::from_str(&s).ok(); }
    serde_wasm_bindgen::from_value::<Value>(v).ok()
}

// ---- the calls, typed -----------------------------------------------------------------------------

/// `window.EncoreNative?.platform`: "android", "desktop" … – None in a browser.
pub fn platform() -> Option<String> {
    js_sys::Reflect::get(&bridge()?, &"platform".into()).ok()?.as_string()
}

/// The desktop app carries Cadenza (`EncoreNative.engine`).
pub fn has_engine() -> bool { native_truthy("engine") }
/// The app can report the engine and sign it in (`engineStatus` and `engineLogin`).
pub fn has_engine_sign_in() -> bool { native_has("engineStatus") && native_has("engineLogin") }
/// `await EncoreNative.engineStatus()`: `{ device_id, name, running, error, … }` – None when there is no engine or the call failed.
pub async fn engine_status() -> Option<Value> { native_call_async("engineStatus", None).await }
/// `await EncoreNative.engineLogin({ token, server })`: the app starts the engine with the server's token and
/// answers with the id (and name) it signed in under.
pub async fn engine_login(token: &Value, server: &str) -> Option<Value> {
    native_call_async("engineLogin", Some(&json!({ "token": token, "server": server }).to_string())).await
}
/// Signing out of Encore signs the desktop app's Cadenza out too.
pub fn engine_logout() { native_call("engineLogout"); }

/// Android / desktop apps: they show what's playing (notification, lock screen, tray) and control the player.
pub fn has_now_playing() -> bool { native_has("nowPlaying") }
/// `EncoreNative.nowPlaying(JSON)`: `{ title, artist, album, playing, art, position, duration }` or `{ stopped: true }`.
pub fn now_playing(payload: &Value) { native_call_with("nowPlaying", Some(&payload.to_string())); }

/// Android app: keep songs on the phone so they play with no server.
pub fn can_save_offline() -> bool { native_has("saveOffline") }
/// `EncoreNative.saveOffline(JSON)`: `{ id, title, artist, album, ext, url, art }`.
pub fn save_offline(item: &Value) { native_call_with("saveOffline", Some(&item.to_string())); }

/// Desktop app: the mini player window.
pub fn has_mini_player() -> bool { native_has("miniPlayer") }
pub fn mini_player() { native_call("miniPlayer"); }

/// Android app: the offline music screen (offered when the server can't be reached).
pub fn has_open_offline() -> bool { native_has("openOffline") }
pub fn open_offline() { native_call("openOffline"); }

/// The apps' server picker.
pub fn has_change_server() -> bool { native_has("changeServer") }
pub fn change_server() { native_call("changeServer"); }

/// Desktop app: Settings → This computer (`EncoreNative.desktopSettings`).
pub fn has_desktop_settings() -> bool { native_has("desktopSettings") }
