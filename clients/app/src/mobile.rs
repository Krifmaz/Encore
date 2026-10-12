//! Android: the media notification / lock-screen controls and "Share → Encore" come from the Kotlin plugin
//! in plugins/media (tauri-plugin-encore-media).

use crate::bridge::NowPlaying;
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_encore_media::Media;

pub fn now_playing<R: Runtime>(app: &AppHandle<R>, np: NowPlaying) {
    let Some(m) = app.try_state::<Media<R>>() else { return };
    let _ = m.now_playing(&tauri_plugin_encore_media::NowPlaying {
        title: np.title, artist: np.artist, album: np.album, art: np.art, playing: np.playing, stopped: np.stopped,
    });
}

/// A link shared into the app while it was closed (or while the picker was up) goes to the Encore page now.
pub fn deliver_share<R: Runtime>(app: &AppHandle<R>) {
    let Some(m) = app.try_state::<Media<R>>() else { return };
    let Some(link) = m.take_share() else { return };
    if let Some(w) = crate::server::main_window(app) {
        let js = format!(
            "fetch('/api/requests/link',{{method:'POST',headers:{{'Content-Type':'application/json','X-Requested-With':'encore'}},body:JSON.stringify({{url:{}}})}}).then(r=>r.json().then(d=>alert(r.ok?(d.mode==='direct'?'Downloading':'Requested '+d.requested+' song(s)'):(d.detail||'Failed'))))",
            serde_json::to_string(&link).unwrap_or_default()
        );
        let _ = w.eval(js);
    }
}

/// The car service (Android Auto) plays from the server the app is connected to.
pub fn remember_server<R: Runtime>(app: &AppHandle<R>, base: &str) {
    if base.is_empty() { return; }
    if let Some(m) = app.try_state::<Media<R>>() {
        m.set_server(base);
    }
}
