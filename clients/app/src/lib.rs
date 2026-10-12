//! Encore app: the Encore web app from your own server in a window (desktop) or full screen (Android),
//! with media keys, tray / notification controls, saved servers and songs kept for offline listening.

mod bridge;
mod config;
mod offline;
mod server;

#[cfg(desktop)]
mod desktop;
#[cfg(mobile)]
mod mobile;

/// Platform-specific "what's playing" handling (tray + OS media controls, or the Android notification).
mod platform {
    #[cfg(desktop)]
    pub use crate::desktop::now_playing;
    #[cfg(mobile)]
    pub use crate::mobile::now_playing;
}

use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::Mutex;
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

/// The Encore release this app was built for (set by the release workflow), else the crate version.
pub const VERSION: &str = match option_env!("ENCORE_VERSION") { Some(v) => v, None => env!("CARGO_PKG_VERSION") };

pub struct State {
    pub settings: Mutex<config::Settings>,
    pub http: reqwest::Client,
    /// watchdog for the server page (see server::arm)
    pub armed: AtomicBool,
    pub generation: AtomicU64,
    /// last nowPlaying() report, re-applied when the cover arrives
    pub last: Mutex<bridge::NowPlaying>,
    /// (cover URL being shown or fetched, its downloaded file – empty while the download runs)
    pub art: Mutex<(String, std::path::PathBuf)>,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let http = reqwest::Client::builder().user_agent(format!("Encore app/{VERSION}")).build().expect("http client");

    let mut builder = tauri::Builder::default();
    #[cfg(desktop)]
    {
        builder = builder
            .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| desktop::show_window(app)))
            .plugin(tauri_plugin_window_state::Builder::new()
                .with_state_flags(tauri_plugin_window_state::StateFlags::SIZE | tauri_plugin_window_state::StateFlags::POSITION | tauri_plugin_window_state::StateFlags::MAXIMIZED)
                .build());
    }
    #[cfg(mobile)]
    {
        builder = builder.plugin(tauri_plugin_encore_media::init());
    }
    let builder = builder
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            bridge::now_playing, bridge::page_alive, bridge::servers, bridge::connect, bridge::remove_server, bridge::change_server,
            bridge::retry_server, bridge::open_offline, bridge::offline_list, bridge::offline_ids, bridge::remove_offline, bridge::save_offline,
        ])
        .setup(move |app| {
            let settings = config::load(app.handle());
            let start = if settings.server.is_empty() { None } else { tauri::Url::parse(&format!("{}/", settings.server)).ok() };
            #[cfg(mobile)]
            mobile::remember_server(app.handle(), &settings.server);
            app.manage(State { settings: Mutex::new(settings), http, armed: AtomicBool::new(false), generation: AtomicU64::new(0), last: Mutex::new(Default::default()), art: Mutex::new(Default::default()) });

            let url = match &start { Some(u) => WebviewUrl::External(u.clone()), None => WebviewUrl::App("index.html".into()) };
            let nav = app.handle().clone();
            #[allow(unused_mut)]
            let mut w = WebviewWindowBuilder::new(app, "main", url)
                .title("Encore")
                .initialization_script(bridge::script())
                .user_agent(&server::user_agent())
                .on_navigation(move |u| server::allow_navigation(&nav, u));
            #[cfg(desktop)]
            {
                let ext = app.handle().clone();
                let dl = app.handle().clone();
                w = w.inner_size(1280.0, 860.0).min_inner_size(380.0, 500.0)
                    .background_color(tauri::window::Color(11, 11, 18, 255))
                    // window.open / target=_blank → the browser
                    .on_new_window(move |u, _features| { server::open_external(&ext, u.as_str()); tauri::webview::NewWindowResponse::Deny })
                    // file downloads from the web app (album zips, exports) → the Downloads folder
                    .on_download(move |_wv, ev| {
                        if let tauri::webview::DownloadEvent::Requested { url, destination } = ev {
                            let name = destination.file_name().map(|n| n.to_string_lossy().to_string()).filter(|n| !n.is_empty())
                                .or_else(|| url.path_segments().and_then(|mut s| s.next_back()).map(|s| s.to_string())).unwrap_or_else(|| "download".into());
                            if let Ok(dir) = dl.path().download_dir() {
                                let mut p = dir.join(&name);
                                let mut n = 1;
                                while p.exists() {
                                    let (stem, ext) = match name.rsplit_once('.') { Some((s, e)) => (s.to_string(), format!(".{e}")), None => (name.clone(), String::new()) };
                                    p = dir.join(format!("{stem} ({n}){ext}"));
                                    n += 1;
                                }
                                *destination = p;
                            }
                        }
                        true
                    });
            }
            w.build()?;
            if start.is_some() {
                server::arm(app.handle());
            }
            // dev builds only: JavaScript typed on stdin runs in the page (used by the automated checks)
            #[cfg(debug_assertions)]
            {
                let h = app.handle().clone();
                std::thread::spawn(move || {
                    for line in std::io::stdin().lines().map_while(Result::ok) {
                        if let Some(w) = server::main_window(&h) {
                            let _ = w.eval(line);
                        }
                    }
                });
            }
            #[cfg(desktop)]
            desktop::setup(app.handle())?;
            Ok(())
        });

    let app = builder.build(tauri::generate_context!()).expect("error while building Encore");
    app.run(|app, event| {
        // macOS: closing the window keeps the app (and the music) running; clicking the dock icon brings it back
        #[cfg(target_os = "macos")]
        match event {
            tauri::RunEvent::WindowEvent { event: tauri::WindowEvent::CloseRequested { api, .. }, .. } => {
                if let Some(w) = server::main_window(app) { let _ = w.hide(); }
                api.prevent_close();
            }
            tauri::RunEvent::Reopen { .. } => desktop::show_window(app),
            _ => {}
        }
        #[cfg(not(target_os = "macos"))]
        let _ = (app, event);
    });
}
