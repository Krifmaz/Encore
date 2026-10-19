//! The tray (the menu bar on macOS): the Encore icon, a tooltip with the song, and a menu with
//! what is playing, Play / Pause, Next, Previous, Show Encore, Mini player, the update line
//! when a new version is found, and Quit Encore (the words of the classic app, removed in 0.29).
//! A left click on the icon shows the window.
//!
//! Windows and macOS keep the icon on the main thread, whose event loop serves it; Linux keeps it
//! on a thread of its own and talks to the tray host over the D-Bus session bus. Without a
//! session bus (Xvfb, a bare X server) or a tray host there is no tray and the app carries on.
//!
//! Linux uses tray-icon's `ksni` backend (StatusNotifierItem in pure Rust, see Cargo.toml). Its
//! default backend links GTK, libappindicator and libxdo and needs a GTK main loop the app does
//! not run.

use crate::player::Now;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

/// What the tray shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct View {
    /// "Title – Artist", or "Encore".
    pub tooltip: String,
    /// The first (disabled) menu line: the song, shortened, or "Encore".
    pub line: String,
    pub playing: bool,
    /// A newer version, when one was found.
    pub update: Option<String>,
    /// The mini player is open.
    pub mini: bool,
}

impl View {
    pub fn new(now: Option<&Now>, update: Option<String>, mini: bool) -> View {
        let song = now.filter(|n| !n.title.is_empty()).map(|n| {
            if n.artist.is_empty() {
                n.title.clone()
            } else {
                format!("{} – {}", n.title, n.artist)
            }
        });
        View {
            tooltip: song
                .as_deref()
                .map(|s| shorten(s, 120))
                .unwrap_or_else(|| "Encore".into()),
            line: song
                .as_deref()
                .map(|s| shorten(s, 60))
                .unwrap_or_else(|| "Encore".into()),
            playing: now.is_some_and(|n| n.playing),
            update,
            mini,
        }
    }
}

/// At most `max` characters, with "…" when cut.
pub fn shorten(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", cut.trim_end())
}

/// The words of the update line.
pub fn update_line(version: &str) -> String {
    format!("Update available: Encore {version}")
}

static AVAILABLE: AtomicBool = AtomicBool::new(false);
static LAST: Mutex<Option<View>> = Mutex::new(None);

/// Whether the tray icon is up.
pub fn available() -> bool {
    AVAILABLE.load(Ordering::SeqCst)
}

/// Whether a tray can be expected here, before trying (`--minimized` starts in it).
pub fn likely() -> bool {
    host::likely()
}

/// Put the icon up (once; the main window is open).
pub fn start() {
    if available() {
        return;
    }
    host::start();
}

/// Show `view` (nothing happens when it is what the tray already shows).
pub fn show(view: View) {
    if !available() {
        return;
    }
    {
        let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
        if last.as_ref() == Some(&view) {
            return;
        }
        *last = Some(view.clone());
    }
    host::show(view);
}

use native as host;

mod native {
    use super::{update_line, View, AVAILABLE};
    use crate::desktop::{guarded, send, send_ui, Message, Msg};
    use crate::pages::{Page, Section};
    use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{
        Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
    };

    pub use host::{likely, show, start};

    /// The icon and the menu items that change.
    struct Tray {
        icon: TrayIcon,
        menu: Menu,
        line: MenuItem,
        toggle: MenuItem,
        mini: CheckMenuItem,
        update: MenuItem,
        update_shown: bool,
    }

    /// Where the update line goes: after Mini player, before the separator above Quit.
    const UPDATE_AT: usize = 8;

    pub(super) fn picture() -> Result<Icon, String> {
        let img = image::load_from_memory_with_format(
            include_bytes!("../../packaging/icons/64x64.png"),
            image::ImageFormat::Png,
        )
        .map_err(|e| e.to_string())?
        .into_rgba8();
        let (w, h) = img.dimensions();
        Icon::from_rgba(img.into_raw(), w, h).map_err(|e| e.to_string())
    }

    fn build() -> Result<Tray, String> {
        let line = MenuItem::with_id("np", "Encore", false, None);
        let toggle = MenuItem::with_id("toggle", "Play / Pause", true, None);
        let next = MenuItem::with_id("next", "Next", true, None);
        let prev = MenuItem::with_id("prev", "Previous", true, None);
        let show = MenuItem::with_id("show", "Show Encore", true, None);
        let mini = CheckMenuItem::with_id("mini", "Mini player", true, false, None);
        let update = MenuItem::with_id("update", update_line(""), true, None);
        let quit = MenuItem::with_id("quit", "Quit Encore", true, None);
        let menu = Menu::new();
        menu.append_items(&[
            &line,
            &PredefinedMenuItem::separator(),
            &toggle,
            &next,
            &prev,
            &PredefinedMenuItem::separator(),
            &show,
            &mini,
            &PredefinedMenuItem::separator(),
            &quit,
        ])
        .map_err(|e| e.to_string())?;
        let icon = TrayIconBuilder::new()
            .with_id("encore")
            .with_tooltip("Encore")
            .with_icon(picture()?)
            .with_menu(Box::new(menu.clone()))
            .with_menu_on_left_click(false)
            .build()
            .map_err(|e| e.to_string())?;
        MenuEvent::set_event_handler(Some(|e: MenuEvent| on_menu(e.id.0.as_str())));
        TrayIconEvent::set_event_handler(Some(|e: TrayIconEvent| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = e
            {
                send(Message::Raise);
            }
        }));
        Ok(Tray {
            icon,
            menu,
            line,
            toggle,
            mini,
            update,
            update_shown: false,
        })
    }

    /// A menu item was chosen (on the thread that serves the tray).
    fn on_menu(id: &str) {
        match id {
            "toggle" => send(Message::Toggle),
            "next" => send(Message::Next),
            "prev" => send(Message::Prev),
            "show" => send(Message::Raise),
            "mini" => send(Message::MiniPlayer(!crate::desktop::mini::is_open())),
            "update" => send_ui(Msg::Open(Page::Settings(Section::Computer))),
            "quit" => send(Message::Quit),
            _ => {}
        }
    }

    impl Tray {
        fn apply(&mut self, v: &View) {
            let _ = self.icon.set_tooltip(Some(&v.tooltip));
            self.line.set_text(&v.line);
            self.toggle
                .set_text(if v.playing { "Pause" } else { "Play" });
            self.mini.set_checked(v.mini);
            match (&v.update, self.update_shown) {
                (Some(ver), shown) => {
                    self.update.set_text(update_line(ver));
                    if !shown && self.menu.insert(&self.update, UPDATE_AT).is_ok() {
                        self.update_shown = true;
                    }
                }
                (None, true) => {
                    let _ = self.menu.remove(&self.update);
                    self.update_shown = false;
                }
                (None, false) => {}
            }
        }
    }

    #[cfg(target_os = "linux")]
    mod host {
        use super::{build, guarded, Tray, View, AVAILABLE};
        use std::sync::atomic::Ordering;
        use std::sync::mpsc::{channel, Sender};
        use std::sync::Mutex;
        use std::time::Duration;

        static TX: Mutex<Option<Sender<View>>> = Mutex::new(None);

        /// The tray host is reached over the session bus.
        pub fn likely() -> bool {
            crate::desktop::session_bus()
        }

        pub fn start() {
            if !crate::desktop::session_bus() {
                log::info!("no tray: there is no D-Bus session bus");
                return;
            }
            let (tx, rx) = channel::<View>();
            let (ready_tx, ready_rx) = channel::<bool>();
            let spawned = std::thread::Builder::new()
                .name("encore-tray".into())
                .spawn(move || {
                    let tray: Option<Tray> = match guarded("The tray", build) {
                        Some(Ok(t)) => Some(t),
                        Some(Err(e)) => {
                            log::warn!("The tray is not available here: {e}");
                            None
                        }
                        None => None,
                    };
                    let Some(mut tray) = tray else {
                        let _ = ready_tx.send(false);
                        return;
                    };
                    let _ = ready_tx.send(true);
                    while let Ok(v) = rx.recv() {
                        if guarded("The tray", || tray.apply(&v)).is_none() {
                            break;
                        }
                    }
                });
            if spawned.is_err() {
                return;
            }
            let ok = ready_rx
                .recv_timeout(Duration::from_secs(3))
                .unwrap_or(false);
            if ok {
                *TX.lock().unwrap_or_else(|e| e.into_inner()) = Some(tx);
                AVAILABLE.store(true, Ordering::SeqCst);
                log::info!("tray icon up");
            }
        }

        pub fn show(v: View) {
            if let Some(tx) = TX.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
                let _ = tx.send(v);
            }
        }
    }

    #[cfg(not(target_os = "linux"))]
    mod host {
        use super::{build, guarded, Tray, View, AVAILABLE};
        use std::cell::RefCell;
        use std::sync::atomic::Ordering;

        thread_local! {
            /// The tray lives on the main thread (Windows and macOS serve it from their event loop).
            static TRAY: RefCell<Option<Tray>> = const { RefCell::new(None) };
        }

        pub fn likely() -> bool {
            true
        }

        pub fn start() {
            match guarded("The tray", build) {
                Some(Ok(t)) => {
                    TRAY.with(|c| *c.borrow_mut() = Some(t));
                    AVAILABLE.store(true, Ordering::SeqCst);
                    log::info!("tray icon up");
                }
                Some(Err(e)) => log::warn!("The tray is not available here: {e}"),
                None => {}
            }
        }

        pub fn show(v: View) {
            TRAY.with(|c| {
                if let Some(t) = c.borrow_mut().as_mut() {
                    let _ = guarded("The tray", || t.apply(&v));
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now(title: &str, artist: &str, playing: bool) -> Now {
        Now {
            title: title.into(),
            artist: artist.into(),
            playing,
            ..Now::default()
        }
    }

    #[test]
    fn the_tray_shows_the_song_or_encore() {
        let v = View::new(Some(&now("Song", "Band", true)), None, false);
        assert_eq!(v.tooltip, "Song – Band");
        assert_eq!(v.line, "Song – Band");
        assert!(v.playing);
        let v = View::new(Some(&now("Song", "", false)), Some("0.28.0".into()), true);
        assert_eq!(v.tooltip, "Song");
        assert!(!v.playing && v.mini);
        assert_eq!(
            update_line(v.update.as_deref().unwrap()),
            "Update available: Encore 0.28.0"
        );
        let v = View::new(None, None, false);
        assert_eq!((v.tooltip.as_str(), v.line.as_str()), ("Encore", "Encore"));
        assert_eq!(
            View::new(Some(&now("", "Band", true)), None, false).line,
            "Encore"
        );
    }

    #[test]
    fn long_lines_are_cut_with_an_ellipsis() {
        assert_eq!(shorten("short", 60), "short");
        let long = "a".repeat(80);
        let s = shorten(&long, 60);
        assert_eq!(s.chars().count(), 60);
        assert!(s.ends_with('…'));
        assert_eq!(shorten("ääääää", 4), "äää…");
    }

    #[test]
    fn the_icon_decodes() {
        assert!(native::picture().is_ok());
    }
}
