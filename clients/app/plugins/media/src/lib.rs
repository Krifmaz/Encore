//! Android side of the Encore app: a small Kotlin plugin (android/) that shows the media notification with
//! lock-screen controls while music plays, receives "Share → Encore" links, saves webview downloads and hosts
//! the Android Auto car service (CarService: the library as a browse tree, played natively).
//! On other platforms this plugin does nothing.

use serde::{Deserialize, Serialize};
use tauri::plugin::{Builder, TauriPlugin};
use tauri::Runtime;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct NowPlaying {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub art: String,
    pub playing: bool,
    pub stopped: bool,
}

#[derive(Deserialize)]
struct ShareResponse {
    url: String,
}

#[derive(Deserialize)]
struct CookieResponse {
    cookie: String,
}

#[derive(Serialize)]
struct UrlArgs<'a> {
    url: &'a str,
}

/// Handle to the Kotlin plugin (only registered on Android).
pub struct Media<R: Runtime>(tauri::plugin::PluginHandle<R>);

impl<R: Runtime> Media<R> {
    /// Show / update / hide the media notification.
    pub fn now_playing(&self, np: &NowPlaying) -> Result<(), tauri::plugin::mobile::PluginInvokeError> {
        self.0.run_mobile_plugin::<()>("nowPlaying", np)
    }

    /// The link shared into the app (Share → Encore), once.
    pub fn take_share(&self) -> Option<String> {
        self.0.run_mobile_plugin::<ShareResponse>("takeShare", ()).ok().map(|r| r.url).filter(|u| !u.is_empty())
    }

    /// The webview's cookies for `url` (the Encore session), as a Cookie header value.
    pub fn cookie(&self, url: &str) -> String {
        self.0.run_mobile_plugin::<CookieResponse>("cookie", UrlArgs { url }).map(|r| r.cookie).unwrap_or_default()
    }

    /// Tell the car service (Android Auto) which server to play from.
    pub fn set_server(&self, url: &str) {
        let _ = self.0.run_mobile_plugin::<()>("setServer", UrlArgs { url });
    }
}

pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("encore-media")
        .setup(|app, api| {
            #[cfg(target_os = "android")]
            {
                use tauri::Manager;
                let handle = api.register_android_plugin("app.encore.client.media", "MediaPlugin")?;
                app.manage(Media(handle));
            }
            let _ = (app, api);
            Ok(())
        })
        .build()
}
