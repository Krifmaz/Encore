//! The parts of the desktop around the window: one instance at a time (`single`), the tray
//! (`tray`), the OS media controls and media keys (`media`), global shortcuts (`hotkeys`), start
//! with the computer (`autostart`), the main window's size and place (`window_state`), the mini
//! player window (`mini`), updates from GitHub (`updater`) and the songs saved on this computer
//! (`offline`).
//!
//! Two channels reach the window from outside iced: [`send`] carries a [`Message`] (a second
//! launch, the tray's Show / Play / Next, a shortcut, a media key) and the app already routes
//! those; [`send_ui`] carries a [`Msg`] (the mini player's buttons, a seek from the OS controls,
//! the tray's update line), which the app routes to [`update`].
//!
//! The app calls in at five places (see the `on_*` functions): when the main window opens, on its
//! 500 ms tick, when a window asks to close, to draw the mini player window, and for [`Msg`].
//! Nothing here may stop the app: without a desktop session (no tray host, no D-Bus session bus,
//! no X display) every part logs why it is off and the app carries on.

pub mod autostart;
pub mod hotkeys;
pub mod media;
pub mod mini;
pub mod local;
pub mod offline;
pub mod single;
pub mod tray;
pub mod updater;
pub mod window_state;

use crate::app::{App, Message as AppMsg};
use crate::pages::{El, Page};
use crate::player;
use crate::widgets::ToastKind;
use iced::{window, Subscription, Task};
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Mutex, Once, OnceLock};

/// What the outside asks of the window.
#[derive(Clone, Debug, PartialEq)]
pub enum Message {
    /// Bring the main window forward (a second launch, the tray's Show).
    Raise,
    Quit,
    Toggle,
    Next,
    Prev,
    VolumeUp,
    VolumeDown,
    /// Open or close the mini player.
    MiniPlayer(bool),
}

/// What the desktop parts ask of the app beyond [`Message`]: the app routes it to [`update`].
#[derive(Clone, Debug)]
pub enum Msg {
    /// The mini player's transport and seek bar.
    Player(player::Message),
    /// The OS media controls moved the position (seconds).
    SeekTo(f64),
    /// The OS media controls skipped forward or back (seconds, negative = back).
    SeekBy(f64),
    /// The OS media controls set the volume (0…1).
    Volume(f64),
    /// Show the main window and open this page (the tray's update line).
    Open(Page),
    /// The mini player's background was pressed: move the window with the pointer.
    MiniDrag,
    /// The mini player's ✕.
    MiniClose,
    /// The mini player's "open Encore" button.
    MiniShowMain,
}

/// A channel from any thread into an iced subscription.
struct Channel<T> {
    tx: OnceLock<Mutex<Sender<T>>>,
    rx: Mutex<Option<Receiver<T>>>,
}

impl<T: Send + 'static> Channel<T> {
    const fn new() -> Self {
        Self {
            tx: OnceLock::new(),
            rx: Mutex::new(None),
        }
    }

    fn init(&self) {
        if self.tx.get().is_some() {
            return;
        }
        let (tx, rx) = channel();
        if self.tx.set(Mutex::new(tx)).is_ok() {
            *self.rx.lock().unwrap_or_else(|e| e.into_inner()) = Some(rx);
        }
    }

    fn send(&self, m: T) {
        if let Some(tx) = self.tx.get() {
            let _ = tx.lock().unwrap_or_else(|e| e.into_inner()).send(m);
        }
    }

    /// The receiving end as a stream (taken once: the subscription runs for the whole app).
    fn stream(&'static self) -> impl iced::futures::Stream<Item = T> {
        iced::stream::channel(
            64,
            move |mut out: iced::futures::channel::mpsc::Sender<T>| async move {
                let Some(rx) = self.rx.lock().unwrap_or_else(|e| e.into_inner()).take() else {
                    return;
                };
                let (atx, mut arx) = iced::futures::channel::mpsc::unbounded();
                std::thread::Builder::new()
                    .name("desktop-events".into())
                    .spawn(move || {
                        while let Ok(m) = rx.recv() {
                            if atx.unbounded_send(m).is_err() {
                                break;
                            }
                        }
                    })
                    .ok();
                use iced::futures::{SinkExt, StreamExt};
                while let Some(m) = arx.next().await {
                    if out.send(m).await.is_err() {
                        break;
                    }
                }
            },
        )
    }
}

static OUT: Channel<Message> = Channel::new();
static UI: Channel<Msg> = Channel::new();

/// Set up the channels (once, before the window runs) and the quiet panic hook for the
/// desktop libraries.
pub fn init() {
    OUT.init();
    UI.init();
    quiet_hook();
    let touring = std::env::args().any(|a| a == "--screenshot-tour");
    TOURING.store(touring, Ordering::SeqCst);
    updater::set_enabled(!touring && crate::config::Config::load().check_updates);
}

/// Hand a message to the window from any thread.
pub fn send(m: Message) {
    OUT.send(m);
}

/// Hand a [`Msg`] to the window from any thread.
pub fn send_ui(m: Msg) {
    UI.send(m);
}

fn out_stream() -> impl iced::futures::Stream<Item = Message> {
    OUT.stream()
}

fn ui_stream() -> impl iced::futures::Stream<Item = Msg> {
    UI.stream()
}

/// The messages sent with [`send`], as a subscription; it also follows the main window's place
/// and size (for `window_state`) and looks for updates in the background.
pub fn events() -> Subscription<Message> {
    Subscription::batch([
        Subscription::run(out_stream),
        window::events().filter_map(window_state::track),
        Subscription::run(updater::watch),
    ])
}

/// The messages sent with [`send_ui`], as a subscription (the app maps them to [`update`]).
pub fn ui_events() -> Subscription<Msg> {
    Subscription::run(ui_stream)
}

// ---- not stopping the app -------------------------------------------------------------------

static TOURING: AtomicBool = AtomicBool::new(false);

/// The screenshot tour runs: nothing here may touch the computer's settings or the network.
pub fn touring() -> bool {
    TOURING.load(Ordering::SeqCst)
}

thread_local! {
    /// Set while [`guarded`] runs on this thread: a panic there is logged as one line.
    static QUIET: Cell<bool> = const { Cell::new(false) };
}

/// The crates whose panics mean "this desktop feature is not available here" rather than a bug
/// of ours (their service threads unwrap D-Bus and display errors).
const QUIET_CRATES: [&str; 7] = [
    "souvlaki",
    "tray-icon",
    "tray_icon",
    "muda",
    "libappindicator",
    "ksni",
    "global-hotkey",
];

/// A panic hook that turns panics of the desktop libraries (and of code run through
/// [`guarded`]) into a log line; every other panic goes to the previous hook.
fn quiet_hook() {
    static HOOK: Once = Once::new();
    HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let ours = QUIET.with(|q| q.get())
                || info
                    .location()
                    .is_some_and(|l| QUIET_CRATES.iter().any(|c| l.file().contains(c)));
            if ours {
                log::warn!("a desktop feature stopped: {}", panic_text(info.payload()));
            } else {
                previous(info);
            }
        }));
    });
}

fn panic_text(p: &(dyn std::any::Any + Send)) -> String {
    p.downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown error".into())
}

/// Run `f`, turning a panic into `None` and one log line ("<what> is not available here: …").
pub(crate) fn guarded<T>(what: &str, f: impl FnOnce() -> T) -> Option<T> {
    quiet_hook();
    let was = QUIET.with(|q| q.replace(true));
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    QUIET.with(|q| q.set(was));
    match r {
        Ok(v) => Some(v),
        Err(e) => {
            log::warn!("{what} is not available here: {}", panic_text(e.as_ref()));
            None
        }
    }
}

/// Whether a D-Bus session bus answers (Linux: the tray and the media controls live on it).
#[cfg(target_os = "linux")]
pub(crate) fn session_bus() -> bool {
    let addr = std::env::var("DBUS_SESSION_BUS_ADDRESS")
        .ok()
        .filter(|a| !a.trim().is_empty())
        .or_else(|| {
            std::env::var("XDG_RUNTIME_DIR")
                .ok()
                .filter(|d| !d.is_empty())
                .map(|d| format!("unix:path={d}/bus"))
        });
    let Some(addr) = addr else { return false };
    parse_bus_address(&addr).iter().any(BusAddr::reachable)
}

/// One entry of a D-Bus address list.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) enum BusAddr {
    Path(String),
    Abstract(String),
    Tcp(String, u16),
}

impl BusAddr {
    #[cfg(target_os = "linux")]
    fn reachable(&self) -> bool {
        use std::os::unix::net::UnixStream;
        match self {
            BusAddr::Path(p) => UnixStream::connect(p).is_ok(),
            BusAddr::Abstract(name) => {
                use std::os::linux::net::SocketAddrExt;
                std::os::unix::net::SocketAddr::from_abstract_name(name.as_bytes())
                    .and_then(|a| UnixStream::connect_addr(&a))
                    .is_ok()
            }
            BusAddr::Tcp(host, port) => {
                use std::net::ToSocketAddrs;
                (host.as_str(), *port)
                    .to_socket_addrs()
                    .ok()
                    .and_then(|mut a| a.next())
                    .is_some_and(|a| {
                        std::net::TcpStream::connect_timeout(
                            &a,
                            std::time::Duration::from_millis(500),
                        )
                        .is_ok()
                    })
            }
        }
    }
}

/// `unix:path=/run/user/1000/bus,guid=…;tcp:host=…,port=…` → the entries this app can try.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn parse_bus_address(addr: &str) -> Vec<BusAddr> {
    // Values are percent-escaped (`%2f`).
    let unescape = |v: &str| -> String {
        let b = v.as_bytes();
        let mut out = Vec::with_capacity(b.len());
        let mut i = 0;
        while i < b.len() {
            let hex = (b[i] == b'%' && i + 3 <= b.len())
                .then(|| std::str::from_utf8(&b[i + 1..i + 3]).ok())
                .flatten()
                .and_then(|h| u8::from_str_radix(h, 16).ok());
            match hex {
                Some(n) => {
                    out.push(n);
                    i += 3;
                }
                None => {
                    out.push(b[i]);
                    i += 1;
                }
            }
        }
        String::from_utf8_lossy(&out).into_owned()
    };
    addr.split(';')
        .filter_map(|one| {
            let (kind, rest) = one.trim().split_once(':')?;
            let get = |key: &str| {
                rest.split(',')
                    .filter_map(|kv| kv.split_once('='))
                    .find(|(k, _)| *k == key)
                    .map(|(_, v)| unescape(v))
            };
            match kind {
                "unix" => get("path")
                    .map(BusAddr::Path)
                    .or_else(|| get("abstract").map(BusAddr::Abstract)),
                "tcp" => Some(BusAddr::Tcp(
                    get("host").unwrap_or_else(|| "localhost".into()),
                    get("port")?.parse().ok()?,
                )),
                _ => None,
            }
        })
        .collect()
}

// ---- the app's hooks --------------------------------------------------------------------------

static ATTACHED: AtomicBool = AtomicBool::new(false);
static TICKS: AtomicU64 = AtomicU64::new(0);

/// The main window is open (call from `Message::WindowOpened`): the tray, the OS media
/// controls, the global shortcuts and start with the computer. Runs on the main thread, which
/// Windows and macOS need for all of them. Once per process.
pub fn on_window_opened(app: &mut App, id: window::Id) -> Task<AppMsg> {
    if mini::is(id) || ATTACHED.swap(true, Ordering::SeqCst) {
        return Task::none();
    }
    window_state::set_main(id);
    tray::start();
    hotkeys::apply(&app.config.hotkeys);
    if !touring() && app.tour.is_none() {
        if let Some(e) = autostart::reconcile(app.config.autostart) {
            app.toast(e, ToastKind::Err);
        }
    }
    let mut tasks = vec![media::start(id)];
    // Started for the tray, but there is no tray here: show the window after all.
    if window_state::started_hidden() && !tray::available() {
        log::info!("no tray on this desktop: showing the window");
        tasks.push(show_main(app));
    }
    Task::batch(tasks)
}

/// The app's 500 ms tick: the tray and the OS controls follow the song, the mini player and
/// the shortcuts and start-with-the-computer follow the config, finished offline saves and a
/// found update become toasts.
pub fn on_tick(app: &mut App) -> Task<AppMsg> {
    let n = TICKS.fetch_add(1, Ordering::Relaxed);
    let mut tasks = vec![mini::sync(app)];
    if !ATTACHED.load(Ordering::SeqCst) {
        return Task::batch(tasks);
    }
    if hotkeys::applied().as_ref() != Some(&app.config.hotkeys) {
        hotkeys::apply(&app.config.hotkeys);
    }
    if !touring() && app.tour.is_none() && autostart::applied() != Some(app.config.autostart) {
        if let Some(e) = autostart::reconcile(app.config.autostart) {
            app.toast(e, ToastKind::Err);
        }
    }
    updater::set_enabled(app.config.check_updates && !touring() && app.tour.is_none());
    if let Some(v) = updater::announce() {
        app.toast(
            format!("Encore {v} is ready to install. Settings → This computer → Updates."),
            ToastKind::Ok,
        );
    }
    let now = app.player.now();
    tray::show(tray::View::new(
        now.as_ref(),
        updater::available().map(|r| r.version),
        mini::is_open(),
    ));
    tasks.push(media::sync(app, now.as_ref()));
    for (text, kind) in offline::take_notices() {
        app.toast(text, kind);
    }
    if n.is_multiple_of(4) {
        tasks.push(window_state::poll());
    }
    Task::batch(tasks)
}

/// A window asks to close (call first in `Message::CloseRequested`). The mini player closes
/// (and the config says so); for the main window the size, place and maximized state go into
/// the config and `None` lets the app quit as before.
pub fn on_close_requested(app: &mut App, id: window::Id) -> Option<Task<AppMsg>> {
    if mini::is(id) {
        app.config.mini_player = false;
        app.config.save();
        return Some(mini::close());
    }
    window_state::save_into(&mut app.config.window);
    app.config.save();
    None
}

/// What the app draws in a window other than the main one: the mini player (None for the main
/// window). The app maps it with its `Msg` variant.
pub fn window_view(app: &App, id: window::Id) -> Option<El<'_, Msg>> {
    mini::is(id).then(|| mini::view(app))
}

/// Show the main window and bring it forward (also when it was started hidden in the tray).
pub fn show_main(app: &App) -> Task<AppMsg> {
    match app.window {
        Some(id) => Task::batch([
            window::set_mode(id, window::Mode::Windowed),
            window::minimize(id, false),
            window::gain_focus(id),
        ]),
        None => Task::none(),
    }
}

/// The app's handling of [`Message`], in full (it can replace the match in `app.rs`): Raise also
/// shows a window started in the tray, Quit keeps the window's place, the mini player opens or
/// closes at once.
pub fn handle(app: &mut App, m: Message) -> Task<AppMsg> {
    match m {
        Message::Raise => show_main(app),
        Message::Quit => {
            window_state::save_into(&mut app.config.window);
            app.config.save();
            crate::engine::shutdown();
            iced::exit()
        }
        Message::Toggle => app.update(AppMsg::Player(player::Message::Toggle)),
        Message::Next => app.update(AppMsg::Player(player::Message::Next)),
        Message::Prev => app.update(AppMsg::Player(player::Message::Prev)),
        Message::VolumeUp | Message::VolumeDown => {
            let v = app
                .player
                .now()
                .map(|n| n.volume)
                .unwrap_or_else(crate::engine::volume_level);
            let step = if m == Message::VolumeUp { 0.05 } else { -0.05 };
            set_volume(app, v + step)
        }
        Message::MiniPlayer(on) => {
            app.config.mini_player = on;
            app.config.save();
            mini::sync(app)
        }
    }
}

/// A volume from outside the slider (a shortcut, the OS media controls): the slider's move and
/// its release together, so the linked device gets it, the config keeps it and the slider
/// follows the engine again afterwards.
fn set_volume(app: &mut App, v: f64) -> Task<AppMsg> {
    let moved = app.update(AppMsg::Player(player::Message::Volume(v.clamp(0.0, 1.0))));
    let done = app.update(AppMsg::Player(player::Message::VolumeDone));
    Task::batch([moved, done])
}

/// The app's handling of [`Msg`].
pub fn update(app: &mut App, m: Msg) -> Task<AppMsg> {
    match m {
        Msg::Player(p) => app.update(AppMsg::Player(p)),
        Msg::Volume(v) => set_volume(app, v),
        Msg::SeekTo(v) => app.update(AppMsg::Player(player::Message::SeekTo(v.max(0.0)))),
        Msg::SeekBy(d) => {
            let Some(now) = app.player.now() else {
                return Task::none();
            };
            let mut to = (now.position + d).max(0.0);
            if now.duration > 0.0 {
                to = to.min(now.duration);
            }
            app.update(AppMsg::Player(player::Message::SeekTo(to)))
        }
        Msg::Open(page) => {
            let show = show_main(app);
            Task::batch([show, app.navigate(page, true)])
        }
        Msg::MiniDrag => mini::id().map(window::drag).unwrap_or_else(Task::none),
        Msg::MiniClose => handle(app, Message::MiniPlayer(false)),
        Msg::MiniShowMain => show_main(app),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bus_addresses_parse_like_dbus_writes_them() {
        assert_eq!(
            parse_bus_address("unix:path=/run/user/1000/bus,guid=abc"),
            vec![BusAddr::Path("/run/user/1000/bus".into())]
        );
        assert_eq!(
            parse_bus_address("unix:abstract=/tmp/dbus-XyZ,guid=1;tcp:host=127.0.0.1,port=4000"),
            vec![
                BusAddr::Abstract("/tmp/dbus-XyZ".into()),
                BusAddr::Tcp("127.0.0.1".into(), 4000)
            ]
        );
        assert_eq!(
            parse_bus_address("unix:path=/tmp/a%20b"),
            vec![BusAddr::Path("/tmp/a b".into())]
        );
        assert!(parse_bus_address("launchd:env=X").is_empty());
        assert!(parse_bus_address("").is_empty());
        assert!(
            parse_bus_address("tcp:host=x").is_empty(),
            "a tcp address needs a port"
        );
    }

    /// Every desktop part started and fed once, as the app does, where there is no desktop
    /// session (`xvfb-run`) or only a bare session bus (`dbus-run-session -- xvfb-run …`).
    #[test]
    #[ignore = "talks to the desktop session: run by hand under xvfb-run, with and without dbus-run-session"]
    fn the_desktop_parts_never_stop_the_app() {
        let _ = env_logger::builder().is_test(true).try_init();
        tray::start();
        tray::show(tray::View::new(None, Some("9.9.9".into()), false));
        let _ = media::start(window::Id::unique());
        let now = player::Now {
            title: "Song".into(),
            artist: "Band".into(),
            album: "Album".into(),
            playing: true,
            position: 3.0,
            duration: 200.0,
            ..player::Now::default()
        };
        media::show(Some(&now), None);
        media::show(None, None);
        let errors = hotkeys::apply(&crate::config::Hotkeys::default());
        log::info!(
            "tray {}, media controls {}, shortcut problems {errors:?}, autostart {}",
            tray::available(),
            media::attached(),
            autostart::is_enabled()
        );
        hotkeys::apply(&crate::config::Hotkeys {
            on: false,
            ..Default::default()
        });
    }

    #[test]
    fn guarded_turns_a_panic_into_none() {
        assert_eq!(guarded("a test", || 7), Some(7));
        let r: Option<()> = guarded("a test", || panic!("no display"));
        assert!(r.is_none());
        assert!(!QUIET.with(|q| q.get()), "the flag is put back");
    }
}
