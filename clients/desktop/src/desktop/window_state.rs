//! The main window's size, place and maximized state, kept in the config between starts. The
//! place and size follow the window's own events; the maximized state is asked every two
//! seconds. While maximized the last normal size and place are kept, so the window comes back
//! the size it was.

use crate::app::Message as AppMsg;
use crate::config::WindowState;
use iced::{window, Point, Size, Task};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

/// The smallest main window (the app's layout needs it).
pub const MIN: Size = Size::new(760.0, 520.0);

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Geom {
    main: Option<window::Id>,
    /// The last normal (not maximized) place and size, and the ones before them.
    pos: Option<Point>,
    prev_pos: Option<Point>,
    size: Option<Size>,
    prev_size: Option<Size>,
    maximized: bool,
}

static GEOM: Mutex<Geom> = Mutex::new(Geom {
    main: None,
    pos: None,
    prev_pos: None,
    size: None,
    prev_size: None,
    maximized: false,
});

static STARTED_HIDDEN: AtomicBool = AtomicBool::new(false);

fn geom() -> std::sync::MutexGuard<'static, Geom> {
    GEOM.lock().unwrap_or_else(|e| e.into_inner())
}

/// The main window's id (its events are the ones kept).
pub fn set_main(id: window::Id) {
    geom().main = Some(id);
}

/// A saved place, when it is a believable one (screens come and go between starts).
pub fn restore_position(x: Option<f32>, y: Option<f32>) -> window::Position {
    match (x, y) {
        (Some(x), Some(y))
            if x.is_finite()
                && y.is_finite()
                && (-4000.0..16000.0).contains(&x)
                && (-200.0..12000.0).contains(&y) =>
        {
            window::Position::Specific(Point::new(x, y))
        }
        _ => window::Position::Centered,
    }
}

/// The main window's settings from the saved state. `hidden`: start in the tray (`--minimized`
/// where a tray is expected); the app shows it when the tray turns out to be missing.
pub fn settings(w: &WindowState, hidden: bool) -> window::Settings {
    STARTED_HIDDEN.store(hidden, Ordering::SeqCst);
    window::Settings {
        size: Size::new(w.width.max(MIN.width), w.height.max(MIN.height)),
        min_size: Some(MIN),
        position: restore_position(w.x, w.y),
        maximized: w.maximized,
        visible: !hidden,
        icon: window::icon::from_file_data(include_bytes!("../../packaging/icons/128x128.png"), None)
            .ok(),
        exit_on_close_request: false,
        // Wayland docks and X11 task bars match the window to encore-desktop.desktop by this.
        #[cfg(target_os = "linux")]
        platform_specific: window::settings::PlatformSpecific {
            application_id: "encore-desktop".into(),
            ..Default::default()
        },
        ..window::Settings::default()
    }
}

/// Whether the main window was started hidden (in the tray).
pub fn started_hidden() -> bool {
    STARTED_HIDDEN.load(Ordering::SeqCst)
}

/// A window event (from `desktop::events`): the main window's moves and sizes are kept. Never
/// produces a message.
pub fn track((id, ev): (window::Id, window::Event)) -> Option<super::Message> {
    if super::mini::is(id) {
        if matches!(ev, window::Event::Closed) {
            super::mini::forget(id);
        }
        return None;
    }
    let mut g = geom();
    if g.main.is_some_and(|m| m != id) {
        return None;
    }
    match ev {
        window::Event::Opened { position, size } => {
            g.pos = position.or(g.pos);
            g.size = Some(size);
        }
        window::Event::Moved(p) if !g.maximized => {
            g.prev_pos = g.pos;
            g.pos = Some(p);
        }
        window::Event::Resized(s) if !g.maximized => {
            g.prev_size = g.size;
            g.size = Some(s);
        }
        _ => {}
    }
    None
}

/// The maximized state as the window says (every two seconds from the app's tick).
pub fn poll() -> Task<AppMsg> {
    let Some(id) = geom().main else {
        return Task::none();
    };
    window::is_maximized(id).map(set_maximized).discard()
}

fn set_maximized(m: bool) {
    let mut g = geom();
    if m && !g.maximized {
        // The resize and move to full screen came before we knew: take the size and place from
        // before them.
        if let Some(s) = g.prev_size {
            g.size = Some(s);
        }
        if let Some(p) = g.prev_pos {
            g.pos = Some(p);
        }
    }
    g.maximized = m;
}

/// Write the window's size, place and maximized state into the config's record.
pub fn save_into(w: &mut WindowState) {
    let g = *geom();
    apply(&g, w);
}

fn apply(g: &Geom, w: &mut WindowState) {
    if let Some(s) = g.size.filter(|s| s.width >= 200.0 && s.height >= 200.0) {
        w.width = s.width.max(MIN.width);
        w.height = s.height.max(MIN.height);
    }
    if let Some(p) = g.pos {
        w.x = Some(p.x);
        w.y = Some(p.y);
    }
    w.maximized = g.maximized;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn odd_saved_places_open_centred() {
        let centred = |p: window::Position| matches!(p, window::Position::Centered);
        assert!(matches!(
            restore_position(Some(100.0), Some(80.0)),
            window::Position::Specific(p) if p == Point::new(100.0, 80.0)
        ));
        assert!(centred(restore_position(None, Some(80.0))));
        assert!(centred(restore_position(Some(-90000.0), Some(10.0))));
        assert!(centred(restore_position(Some(f32::NAN), Some(10.0))));
    }

    #[test]
    fn the_normal_size_survives_a_maximize() {
        let mut g = Geom {
            size: Some(Size::new(1200.0, 800.0)),
            pos: Some(Point::new(50.0, 40.0)),
            ..Geom::default()
        };
        // maximizing: the window moves and grows, then the poll says maximized
        g.prev_pos = g.pos;
        g.pos = Some(Point::new(0.0, 0.0));
        g.prev_size = g.size;
        g.size = Some(Size::new(2560.0, 1400.0));
        {
            let mut shared = geom();
            *shared = g;
        }
        set_maximized(true);
        let mut w = WindowState::default();
        save_into(&mut w);
        assert_eq!(
            (w.width, w.height, w.x, w.y, w.maximized),
            (1200.0, 800.0, Some(50.0), Some(40.0), true)
        );
        // tiny or missing sizes leave the record alone
        let mut w2 = WindowState::default();
        apply(
            &Geom {
                size: Some(Size::new(10.0, 10.0)),
                ..Geom::default()
            },
            &mut w2,
        );
        assert_eq!((w2.width, w2.height), (1280.0, 820.0));
        *geom() = Geom::default();
    }

    #[test]
    fn hidden_starts_are_remembered() {
        let s = settings(&WindowState::default(), false);
        assert!(s.visible && !s.exit_on_close_request);
        assert_eq!(s.min_size, Some(MIN));
        assert!(!started_hidden());
    }
}
