//! Desktop only (Windows, macOS, Linux): the tray with what's playing, the OS media controls / media keys
//! (SMTC, MPRIS, Now Playing) and the single-instance / window-state plumbing.
//! Tray and media controls are UI objects, so everything here runs on the main thread.

use crate::bridge::{control, NowPlaying};
use crate::{server, State};
use souvlaki::{MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, PlatformConfig};
use std::cell::RefCell;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Runtime};

struct Ui<R: Runtime> {
    tray: tauri::tray::TrayIcon<R>,
    np_item: MenuItem<R>,
    toggle_item: MenuItem<R>,
    media: Option<MediaControls>,
}

thread_local! {
    static UI: RefCell<Option<Box<dyn std::any::Any>>> = const { RefCell::new(None) };
}

fn with_ui<R: Runtime, T>(f: impl FnOnce(&mut Ui<R>) -> T) -> Option<T> {
    UI.with(|u| u.borrow_mut().as_mut().and_then(|b| b.downcast_mut::<Ui<R>>()).map(f))
}

pub fn show_window<R: Runtime>(app: &AppHandle<R>) {
    if let Some(w) = server::main_window(app) {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// Build the tray and attach to the OS media controls. Called from Tauri's setup (main thread).
pub fn setup<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let np_item = MenuItem::with_id(app, "np", "Encore", false, None::<&str>)?;
    let toggle_item = MenuItem::with_id(app, "toggle", "Play / Pause", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[
        &np_item,
        &PredefinedMenuItem::separator(app)?,
        &toggle_item,
        &MenuItem::with_id(app, "next", "Next", true, None::<&str>)?,
        &MenuItem::with_id(app, "prev", "Previous", true, None::<&str>)?,
        &PredefinedMenuItem::separator(app)?,
        &MenuItem::with_id(app, "show", "Show Encore", true, None::<&str>)?,
        &MenuItem::with_id(app, "server", "Change server…", true, None::<&str>)?,
        &PredefinedMenuItem::separator(app)?,
        &MenuItem::with_id(app, "quit", "Quit Encore", true, None::<&str>)?,
    ])?;
    let mut tray = TrayIconBuilder::with_id("main").tooltip("Encore").menu(&menu).show_menu_on_left_click(false)
        .on_menu_event(|app, e| match e.id().as_ref() {
            "toggle" => control(app, "toggle"),
            "next" => control(app, "next"),
            "prev" => control(app, "prev"),
            "show" => show_window(app),
            "server" => server::show_picker(app, None),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, e| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
                show_window(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    let tray = tray.build(app)?;

    // OS media controls: media keys, lock screen / media overlay, MPRIS (playerctl, GNOME, KDE)
    let media = media_controls(app);
    UI.with(|u| *u.borrow_mut() = Some(Box::new(Ui::<R> { tray, np_item, toggle_item, media })));
    Ok(())
}

fn media_controls<R: Runtime>(app: &AppHandle<R>) -> Option<MediaControls> {
    #[allow(unused_mut, unused_assignments)]
    let mut hwnd: Option<*mut std::ffi::c_void> = None;
    #[cfg(target_os = "windows")]
    {
        hwnd = server::main_window(app).and_then(|w| w.hwnd().ok()).map(|h| h.0 as *mut std::ffi::c_void);
    }
    let mut mc = match MediaControls::new(PlatformConfig { display_name: "Encore", dbus_name: "encore", hwnd }) {
        Ok(m) => m,
        Err(e) => { eprintln!("media controls unavailable: {e:?}"); return None; }
    };
    let handle = app.clone();
    if let Err(e) = mc.attach(move |ev| {
        match ev {
            MediaControlEvent::Play | MediaControlEvent::Pause | MediaControlEvent::Toggle => control(&handle, "toggle"),
            MediaControlEvent::Next => control(&handle, "next"),
            MediaControlEvent::Previous => control(&handle, "prev"),
            MediaControlEvent::Raise => show_window(&handle),
            MediaControlEvent::Quit => handle.exit(0),
            _ => {}
        }
    }) {
        eprintln!("media controls: {e:?}");
        return None;
    }
    Some(mc)
}

/// The web app reported what's playing: update tray + OS controls (on the main thread).
pub fn now_playing<R: Runtime>(app: &AppHandle<R>, np: NowPlaying) {
    let handle = app.clone();
    let np2 = np.clone();
    let _ = app.run_on_main_thread(move || { apply::<R>(&handle, &np2); });
    // cover art for the lock screen / media overlay: fetched signed-in, kept as a file
    if !np.stopped && !np.art.is_empty() {
        fetch_art(app, np.art);
    }
}

fn apply<R: Runtime>(app: &AppHandle<R>, np: &NowPlaying) {
    // the cover file, once it has been fetched for this song's art URL
    let cover = app.state::<State>().art.lock().ok().and_then(|a| (!np.art.is_empty() && a.0 == np.art).then(|| format!("file://{}", a.1.display())));
    with_ui::<R, _>(|ui| {
        let line = if np.stopped { String::new() } else { format!("{} – {}", np.title, np.artist) };
        let _ = ui.tray.set_tooltip(Some(if line.is_empty() { "Encore".to_string() } else { line.clone() }));
        let _ = ui.np_item.set_text(if line.is_empty() { "Encore".to_string() } else { line.chars().take(60).collect() });
        let _ = ui.toggle_item.set_text(if np.playing { "Pause" } else { "Play" });
        if let Some(m) = ui.media.as_mut() {
            if np.stopped {
                let _ = m.set_playback(MediaPlayback::Stopped);
                let _ = m.set_metadata(MediaMetadata::default());
            } else {
                let _ = m.set_metadata(MediaMetadata { title: Some(&np.title), artist: Some(&np.artist), album: Some(&np.album), cover_url: cover.as_deref(), duration: None });
                let _ = m.set_playback(if np.playing { MediaPlayback::Playing { progress: None } } else { MediaPlayback::Paused { progress: None } });
            }
        }
    });
}

/// Download the cover (with the window's cookies) and show it in the OS controls.
fn fetch_art<R: Runtime>(app: &AppHandle<R>, art: String) {
    {
        let st = app.state::<State>();
        let Ok(mut a) = st.art.lock() else { return };
        if a.0 == art {
            return;
        }
        a.0 = art.clone();       // claim it: one download per cover, even if reports come in quickly
        a.1 = std::path::PathBuf::new();
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let Some(w) = server::main_window(&app) else { return };
        let server = app.state::<State>().settings.lock().map(|s| s.server.clone()).unwrap_or_default();
        let cookie = tauri::async_runtime::spawn_blocking(move || {
            tauri::Url::parse(&format!("{server}/")).ok().and_then(|u| w.cookies_for_url(u).ok())
                .map(|cs| cs.iter().map(|c| format!("{}={}", c.name(), c.value())).collect::<Vec<_>>().join("; ")).unwrap_or_default()
        }).await.unwrap_or_default();
        let Ok(dir) = app.path().app_cache_dir() else { return };
        let _ = std::fs::create_dir_all(&dir);
        // alternate between two files so the OS notices the change
        let n = app.state::<State>().generation.load(std::sync::atomic::Ordering::Relaxed) % 2;
        let file = dir.join(format!("now-playing-{n}.jpg"));
        let _ = std::fs::remove_file(&file);
        if let Err(e) = crate::offline::download(&app.state::<State>().http, &art, &cookie, &file).await {
            #[cfg(debug_assertions)]
            eprintln!("cover {art}: {e} (cookie {} bytes)", cookie.len());
            if let Ok(mut a) = app.state::<State>().art.lock() { if a.0 == art { a.0.clear(); } }
            return;
        }
        // only a real image goes to the OS (souvlaki on macOS aborts on anything NSImage can't read)
        let head = std::fs::read(&file).unwrap_or_default();
        let image = head.starts_with(&[0xFF, 0xD8]) || head.starts_with(b"\x89PNG") || (head.starts_with(b"RIFF") && head.get(8..12) == Some(b"WEBP")) || head.starts_with(b"GIF8") || head.starts_with(b"BM");
        if !image {
            let _ = std::fs::remove_file(&file);
            if let Ok(mut a) = app.state::<State>().art.lock() { if a.0 == art { a.0.clear(); } }
            return;
        }
        if let Ok(mut a) = app.state::<State>().art.lock() {
            if a.0 != art { return; }   // a newer song took over meanwhile
            a.1 = file;
        }
        let handle = app.clone();
        let _ = app.run_on_main_thread(move || {
            // re-apply the last report, now with the cover
            let np = handle.state::<State>().last.lock().map(|l| l.clone()).unwrap_or_default();
            apply::<R>(&handle, &np);
        });
    });
}
