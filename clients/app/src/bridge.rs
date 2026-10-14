//! window.EncoreNative – what the Encore web app (and our own pages) call, and the script that defines it.
//! Every function here is a Tauri command; the web app only ever fires and forgets.

use crate::{config, offline, server, State};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, Runtime, WebviewWindow};

/// Runs in every page the window loads (before the page's own scripts). Defines window.EncoreNative and tells
/// Rust when a page is up (page_alive). Our own pages call invoke() directly and ignore EncoreNative.
pub fn script() -> String {
    let mobile = cfg!(any(target_os = "android", target_os = "ios"));
    format!(r#"
(function () {{
  if (window.EncoreNative) return;
  const invoke = (cmd, args) => {{ const t = window.__TAURI_INTERNALS__; return t ? t.invoke(cmd, args || {{}}) : Promise.reject(new Error('no bridge')); }};
  const fire = (cmd, args) => {{ invoke(cmd, args).catch((e) => {{ try {{ console.error('EncoreNative', cmd, String(e)); }} catch (_) {{}} }}); }};
  const n = {{
    app: 'encore', platform: '{platform}', version: '{version}',
    nowPlaying: (json) => fire('now_playing', {{ json: String(json) }}),
    changeServer: () => fire('change_server'),
    _notify: (m) => window.dispatchEvent(new CustomEvent('encore-native', {{ detail: m }})),
  }};
  if ({mobile}) {{
    n.saveOffline = (json) => fire('save_offline', {{ json: String(json) }});
    n.openOffline = () => fire('open_offline');
  }} else {{
    // Cadenza is built into the desktop app (src/engine.rs); the web app signs it in with an app token
    n.engine = true;
    n.engineLogin = (json) => invoke('engine_login', {{ json: String(json) }});
    n.engineLogout = () => fire('engine_logout');
    n.engineStatus = () => invoke('engine_status');
  }}
  window.EncoreNative = n;
  const alive = () => fire('page_alive', {{ url: location.href, encore: !!document.querySelector('script[src*="/js/app.js"]') }});
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', alive); else alive();
  // keyboard: reload like a browser (the window has no menu bar on Windows/Linux)
  window.addEventListener('keydown', (e) => {{ if ((e.ctrlKey || e.metaKey) && !e.shiftKey && e.key.toLowerCase() === 'r') {{ e.preventDefault(); location.reload(); }} }});
}})();
"#, platform = if mobile { "android" } else { "desktop" }, version = crate::VERSION, mobile = mobile)
}

/// What the web app reports with nowPlaying(): {title, artist, album, art, playing} or {stopped: true}.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct NowPlaying {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub art: String,
    pub playing: bool,
    pub stopped: bool,
}

/// Run a player action in the page: toggle / next / prev / open (see window.encorePlayer in web/js/player.js).
pub fn control<R: Runtime>(app: &AppHandle<R>, action: &str) {
    if !matches!(action, "toggle" | "next" | "prev" | "open") {
        return;
    }
    if let Some(w) = server::main_window(app) {
        let _ = w.eval(format!("window.encorePlayer && window.encorePlayer.{action}()"));
    }
}

/// A toast in the web app (web/js/app.js listens for the encore-native event).
pub fn notify<R: Runtime>(app: &AppHandle<R>, text: &str, kind: &str) {
    if let Some(w) = server::main_window(app) {
        let msg = serde_json::json!({ "text": text, "kind": kind });
        let _ = w.eval(format!("window.EncoreNative && window.EncoreNative._notify && window.EncoreNative._notify({msg})"));
    }
}

#[derive(Serialize)]
pub struct Servers {
    pub list: Vec<String>,
    pub current: String,
}

#[derive(Serialize)]
pub struct Outcome {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[tauri::command(async)]
pub fn now_playing<R: Runtime>(app: AppHandle<R>, json: String) {
    let np: NowPlaying = serde_json::from_str(&json).unwrap_or_default();
    if let Ok(mut l) = app.state::<State>().last.lock() {
        *l = np.clone();
    }
    crate::platform::now_playing(&app, np);
}

#[tauri::command(async)]
pub fn page_alive<R: Runtime>(app: AppHandle<R>, url: String, encore: bool) {
    server::page_alive(&app, &url, encore);
    #[cfg(mobile)]
    if encore {
        crate::mobile::deliver_share(&app);
    }
}

#[tauri::command]
pub fn servers<R: Runtime>(app: AppHandle<R>) -> Servers {
    let s = app.state::<State>().settings.lock().map(|s| s.clone()).unwrap_or_default();
    Servers { list: s.list(), current: s.server }
}

#[tauri::command]
pub async fn connect<R: Runtime>(app: AppHandle<R>, url: String) -> Outcome {
    match server::connect(&app, &url).await {
        Ok(()) => Outcome { ok: true, error: None },
        Err(e) => Outcome { ok: false, error: Some(e) },
    }
}

#[tauri::command]
pub fn remove_server<R: Runtime>(app: AppHandle<R>, url: String) {
    let st = app.state::<State>();
    let Ok(mut s) = st.settings.lock() else { return };
    s.forget(&url);
    config::save(&app, &s);
}

#[tauri::command]
pub fn change_server<R: Runtime>(app: AppHandle<R>) {
    // another server means another account: the engine's token for this one is no longer wanted
    #[cfg(desktop)]
    crate::engine::logout(&app);
    server::show_picker(&app, None);
}

#[tauri::command]
pub fn retry_server<R: Runtime>(app: AppHandle<R>) {
    server::open_server(&app);
}

#[tauri::command]
pub fn open_offline<R: Runtime>(app: AppHandle<R>) {
    server::disarm(&app);
    if let Some(w) = server::main_window(&app) {
        let _ = w.navigate(server::app_url("offline.html"));
    }
}

#[tauri::command]
pub fn offline_list<R: Runtime>(app: AppHandle<R>) -> Vec<offline::Song> {
    offline::list(&app)
}

#[tauri::command]
pub fn offline_ids<R: Runtime>(app: AppHandle<R>) -> Vec<String> {
    offline::ids(&app)
}

#[tauri::command]
pub fn remove_offline<R: Runtime>(app: AppHandle<R>, id: String) {
    offline::remove(&app, &id);
}

/// Download a song for offline listening, signed in with the window's cookies for the server.
#[tauri::command]
pub async fn save_offline<R: Runtime>(app: AppHandle<R>, window: WebviewWindow<R>, json: String) -> Outcome {
    let Ok(req) = serde_json::from_str::<offline::Request>(&json) else { return Outcome { ok: false, error: Some("bad request".into()) } };
    let server = app.state::<State>().settings.lock().map(|s| s.server.clone()).unwrap_or_default();
    // cookies are read on a worker thread: on Windows this call deadlocks on the main thread
    let cookie = tauri::async_runtime::spawn_blocking(move || {
        let Ok(u) = tauri::Url::parse(&format!("{server}/")) else { return String::new() };
        window.cookies_for_url(u).map(|cs| cs.iter().map(|c| format!("{}={}", c.name(), c.value())).collect::<Vec<_>>().join("; ")).unwrap_or_default()
    }).await.unwrap_or_default();
    let http = app.state::<State>().http.clone();
    match offline::save(&app, &http, &cookie, req).await {
        Ok(title) => { notify(&app, &format!("Saved for offline: {title}"), "ok"); Outcome { ok: true, error: None } }
        Err(e) => { notify(&app, &format!("Couldn't save: {e}"), "err"); Outcome { ok: false, error: Some(e) } }
    }
}
