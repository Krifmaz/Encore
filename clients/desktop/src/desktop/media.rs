//! The OS media controls (SMTC on Windows, MPRIS on Linux, Now Playing on macOS): what is
//! playing with its cover, length and position, and the media keys, the lock screen and the
//! desktop's media overlay driving the player (play, pause, next, previous, seek, raise, quit).
//!
//! The controls live on the main thread (Windows needs the main window's handle, macOS its run
//! loop). On Linux they talk MPRIS over the D-Bus session bus; without one they stay off. The
//! cover is fetched signed in and kept as a file, since the OS cannot fetch it from the server.

use super::{guarded, send, send_ui, Message, Msg};
use crate::app::{App, Message as AppMsg};
use crate::player::Now;
use iced::{window, Task};
use souvlaki::{
    MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig,
    SeekDirection,
};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The song as the OS controls last got it.
#[derive(Clone, Debug, Default, PartialEq)]
struct Meta {
    title: String,
    artist: String,
    album: String,
    cover: Option<String>,
    duration: u64,
}

struct Controls {
    inner: MediaControls,
    meta: Option<Meta>,
    /// (playing, position, when) of the last playback report.
    play: Option<(bool, f64, Instant)>,
}

thread_local! {
    static CONTROLS: RefCell<Option<Controls>> = const { RefCell::new(None) };
}

/// Playing right now (the handler runs on another thread and turns Play / Pause into a toggle
/// only when it changes something).
static PLAYING: AtomicBool = AtomicBool::new(false);

/// (cover URL, its file once fetched) for the song playing.
static ART: Mutex<(String, Option<PathBuf>)> = Mutex::new((String::new(), None));
static ART_TURN: Mutex<u8> = Mutex::new(0);

/// Attach to the OS media controls (on the main thread, once the main window is open).
pub fn start(main: window::Id) -> Task<AppMsg> {
    #[cfg(target_os = "windows")]
    {
        window::run(main, |w| {
            use iced::window::raw_window_handle::{HasWindowHandle, RawWindowHandle};
            let hwnd = w.window_handle().ok().and_then(|h| match h.as_raw() {
                RawWindowHandle::Win32(h) => Some(h.hwnd.get() as *mut std::ffi::c_void),
                _ => None,
            });
            create(hwnd);
        })
        .discard()
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = main;
        create(None);
        Task::none()
    }
}

fn create(hwnd: Option<*mut std::ffi::c_void>) {
    #[cfg(target_os = "linux")]
    if !super::session_bus() {
        log::info!("no OS media controls: there is no D-Bus session bus");
        return;
    }
    #[cfg(target_os = "windows")]
    if hwnd.is_none() {
        log::warn!("no OS media controls: the window has no handle");
        return;
    }
    // One name per process, so a second Encore (or the classic app) never collides on the bus.
    let dbus_name = format!("encore.instance{}", std::process::id());
    let made = guarded("The OS media controls", || {
        let mut mc = MediaControls::new(PlatformConfig {
            display_name: "Encore",
            dbus_name: &dbus_name,
            hwnd,
        })
        .map_err(|e| format!("{e:?}"))?;
        mc.attach(on_event).map_err(|e| format!("{e:?}"))?;
        Ok::<_, String>(mc)
    });
    match made {
        Some(Ok(mc)) => {
            CONTROLS.with(|c| {
                *c.borrow_mut() = Some(Controls {
                    inner: mc,
                    meta: None,
                    play: None,
                })
            });
            log::info!("OS media controls attached");
        }
        Some(Err(e)) => log::warn!("The OS media controls are not available here: {e}"),
        None => {}
    }
}

/// What a media key, the lock screen or the overlay asked for.
fn on_event(ev: MediaControlEvent) {
    match action(&ev, PLAYING.load(Ordering::SeqCst)) {
        Some(Action::App(m)) => send(m),
        Some(Action::Ui(m)) => send_ui(m),
        None => {}
    }
}

#[derive(Debug)]
enum Action {
    App(Message),
    Ui(Msg),
}

/// The event as a message (None when it changes nothing, like Play while playing).
fn action(ev: &MediaControlEvent, playing: bool) -> Option<Action> {
    let seek = |forward: bool, by: f64| Action::Ui(Msg::SeekBy(if forward { by } else { -by }));
    Some(match ev {
        MediaControlEvent::Toggle => Action::App(Message::Toggle),
        MediaControlEvent::Play if !playing => Action::App(Message::Toggle),
        MediaControlEvent::Pause | MediaControlEvent::Stop if playing => {
            Action::App(Message::Toggle)
        }
        MediaControlEvent::Next => Action::App(Message::Next),
        MediaControlEvent::Previous => Action::App(Message::Prev),
        MediaControlEvent::Raise => Action::App(Message::Raise),
        MediaControlEvent::Quit => Action::App(Message::Quit),
        MediaControlEvent::SetPosition(MediaPosition(d)) => {
            Action::Ui(Msg::SeekTo(d.as_secs_f64()))
        }
        MediaControlEvent::SeekBy(dir, d) => {
            seek(matches!(dir, SeekDirection::Forward), d.as_secs_f64())
        }
        MediaControlEvent::Seek(dir) => seek(matches!(dir, SeekDirection::Forward), 10.0),
        MediaControlEvent::SetVolume(v) => Action::Ui(Msg::Volume(v.clamp(0.0, 1.0))),
        _ => return None,
    })
}

/// Follow the player (the app's tick): the song, its cover once fetched, play / pause and
/// the position. Returns the cover download when a new song needs one.
pub fn sync(app: &App, now: Option<&Now>) -> Task<AppMsg> {
    PLAYING.store(now.is_some_and(|n| n.playing), Ordering::SeqCst);
    if !attached() {
        return Task::none();
    }
    let (cover_file, fetch) = cover_for(&app.api, now);
    show(now, cover_file.as_deref());
    fetch
}

/// Whether the OS media controls are attached.
pub fn attached() -> bool {
    CONTROLS.with(|c| c.borrow().is_some())
}

/// Give the OS controls the song (None: nothing playing) and the cover file.
pub fn show(now: Option<&Now>, cover: Option<&Path>) {
    CONTROLS.with(|c| {
        if let Some(c) = c.borrow_mut().as_mut() {
            let _ = guarded("The OS media controls", || c.apply(now, cover));
        }
    });
}

/// The cover as a file for the OS: a song saved on this computer has one; a server song's
/// cover is downloaded (with the session) to `<config dir>/cache/now-playing-<0|1>.<ext>`.
fn cover_for(
    api: &std::sync::Arc<crate::api::Api>,
    now: Option<&Now>,
) -> (Option<PathBuf>, Task<AppMsg>) {
    let Some(url) = now.and_then(|n| n.cover.clone()).filter(|c| !c.is_empty()) else {
        return (None, Task::none());
    };
    if !url.starts_with("http") && Path::new(&url).is_absolute() && Path::new(&url).is_file() {
        return (Some(PathBuf::from(url)), Task::none());
    }
    let full = crate::widgets::cover::absolute(&api.base(), &url);
    let mut art = ART.lock().unwrap_or_else(|e| e.into_inner());
    if art.0 == full {
        return (art.1.clone(), Task::none());
    }
    // Claim it: one download per cover.
    *art = (full.clone(), None);
    drop(art);
    let api = api.clone();
    let turn = {
        let mut t = ART_TURN.lock().unwrap_or_else(|e| e.into_inner());
        *t = (*t + 1) % 2;
        *t
    };
    let task = Task::perform(
        async move {
            let bytes = api.get_bytes(&full).await.ok();
            let saved = bytes.as_deref().and_then(|b| {
                let ext = image_ext(b)?;
                let dir = crate::config::dir().join("cache");
                std::fs::create_dir_all(&dir).ok()?;
                // Two files in turn, so the OS notices the change.
                let file = dir.join(format!("now-playing-{turn}.{ext}"));
                std::fs::write(&file, b).ok()?;
                Some(file)
            });
            let mut art = ART.lock().unwrap_or_else(|e| e.into_inner());
            if art.0 == full {
                art.1 = saved;
            }
        },
        |_| (),
    );
    (None, task.discard())
}

/// The picture's kind by its first bytes; None for anything the OS might choke on (souvlaki on
/// macOS aborts on data NSImage cannot read).
pub fn image_ext(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(&[0xFF, 0xD8]) {
        Some("jpg")
    } else if b.starts_with(b"\x89PNG") {
        Some("png")
    } else if b.starts_with(b"RIFF") && b.get(8..12) == Some(b"WEBP") {
        Some("webp")
    } else if b.starts_with(b"GIF8") {
        Some("gif")
    } else {
        None
    }
}

/// `file://` + the path, as souvlaki wants it on every OS.
fn file_url(p: &Path) -> String {
    format!("file://{}", p.display())
}

impl Controls {
    fn apply(&mut self, now: Option<&Now>, cover: Option<&Path>) {
        let Some(n) = now.filter(|n| !n.title.is_empty()) else {
            if self.meta.is_some() || self.play.is_some() {
                let _ = self.inner.set_playback(MediaPlayback::Stopped);
                let _ = self.inner.set_metadata(MediaMetadata::default());
                self.meta = None;
                self.play = None;
            }
            return;
        };
        let meta = Meta {
            title: n.title.clone(),
            artist: n.artist.clone(),
            album: n.album.clone(),
            cover: cover.map(file_url),
            duration: secs(n.duration).round() as u64,
        };
        let song_changed = self.meta.as_ref().map(|m| (&m.title, &m.artist, &m.album))
            != Some((&meta.title, &meta.artist, &meta.album));
        if self.meta.as_ref() != Some(&meta) {
            let _ = self.inner.set_metadata(MediaMetadata {
                title: Some(&meta.title),
                artist: Some(&meta.artist),
                album: Some(&meta.album),
                cover_url: meta.cover.as_deref(),
                duration: (meta.duration > 0).then(|| Duration::from_secs(meta.duration)),
            });
            self.meta = Some(meta);
        }
        if song_changed || needs_report(self.play, n.playing, n.position, Instant::now()) {
            let progress = Some(MediaPosition(Duration::from_secs_f64(secs(n.position))));
            let _ = self.inner.set_playback(if n.playing {
                MediaPlayback::Playing { progress }
            } else {
                MediaPlayback::Paused { progress }
            });
            self.play = Some((n.playing, n.position, Instant::now()));
        }
    }
}

/// Seconds the OS can take: finite, not negative, under four months.
fn secs(v: f64) -> f64 {
    if v.is_finite() {
        v.clamp(0.0, 1e7)
    } else {
        0.0
    }
}

/// Whether the OS needs the play state again: it changed, the position jumped (a seek) from
/// where it should be by now, or 15 s passed.
fn needs_report(
    last: Option<(bool, f64, Instant)>,
    playing: bool,
    position: f64,
    at: Instant,
) -> bool {
    let Some((was, pos, when)) = last else {
        return true;
    };
    let since = at.saturating_duration_since(when).as_secs_f64();
    let expected = if was { pos + since } else { pos };
    was != playing || (position - expected).abs() > 2.0 || since > 15.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_keys_become_player_messages() {
        let a = |ev: MediaControlEvent, playing: bool| format!("{:?}", action(&ev, playing));
        let app = |m: Message| format!("{:?}", Some(Action::App(m)));
        let ui = |m: Msg| format!("{:?}", Some(Action::Ui(m)));
        assert_eq!(a(MediaControlEvent::Toggle, true), app(Message::Toggle));
        assert_eq!(a(MediaControlEvent::Play, false), app(Message::Toggle));
        assert_eq!(
            a(MediaControlEvent::Play, true),
            "None",
            "play while playing does nothing"
        );
        assert_eq!(a(MediaControlEvent::Pause, true), app(Message::Toggle));
        assert_eq!(a(MediaControlEvent::Pause, false), "None");
        assert_eq!(a(MediaControlEvent::Next, false), app(Message::Next));
        assert_eq!(a(MediaControlEvent::Previous, false), app(Message::Prev));
        assert_eq!(
            a(
                MediaControlEvent::SetPosition(MediaPosition(Duration::from_secs(42))),
                true
            ),
            ui(Msg::SeekTo(42.0))
        );
        assert_eq!(
            a(MediaControlEvent::Seek(SeekDirection::Backward), true),
            ui(Msg::SeekBy(-10.0))
        );
        assert_eq!(
            a(
                MediaControlEvent::SeekBy(SeekDirection::Forward, Duration::from_secs(5)),
                true
            ),
            ui(Msg::SeekBy(5.0))
        );
        assert_eq!(a(MediaControlEvent::OpenUri("x".into()), true), "None");
    }

    #[test]
    fn the_play_state_is_reported_when_it_matters() {
        let t0 = Instant::now();
        assert!(needs_report(None, true, 0.0, t0));
        let last = Some((true, 10.0, t0));
        assert!(
            !needs_report(last, true, 13.0, t0 + Duration::from_secs(3)),
            "the position moved on as expected"
        );
        assert!(
            needs_report(last, true, 60.0, t0 + Duration::from_secs(3)),
            "a seek"
        );
        assert!(
            needs_report(last, false, 13.0, t0 + Duration::from_secs(3)),
            "paused"
        );
        assert!(
            needs_report(last, true, 26.0, t0 + Duration::from_secs(16)),
            "a while passed"
        );
        assert!(!needs_report(
            Some((false, 10.0, t0)),
            false,
            10.0,
            t0 + Duration::from_secs(5)
        ));
    }

    #[test]
    fn only_real_pictures_go_to_the_os() {
        assert_eq!(image_ext(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("jpg"));
        assert_eq!(image_ext(b"\x89PNG\r\n"), Some("png"));
        assert_eq!(image_ext(b"RIFF\0\0\0\0WEBPVP8 "), Some("webp"));
        assert_eq!(image_ext(b"<html>"), None);
        assert_eq!(image_ext(b""), None);
        assert!(file_url(Path::new("/tmp/a.jpg")).starts_with("file:///tmp"));
        assert_eq!(
            (secs(f64::NAN), secs(f64::INFINITY), secs(-3.0), secs(61.5)),
            (0.0, 0.0, 0.0, 61.5)
        );
    }
}
