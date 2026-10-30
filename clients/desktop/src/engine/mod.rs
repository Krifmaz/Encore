//! Cadenza on this device. Every song the app plays goes through the embedded engine
//! (`cadenza::embed::EmbeddedPlayer`): server tracks stream from `/api/library/tracks/{id}/stream`
//! with the app token, offline songs play from disk. The engine is started once, on the first
//! play or when the device link needs it, and lives for the process.
//!
//! Package F (player + link) owns this module: `settings.rs` is the Playback card's reads and
//! writes, `test_sound.rs` the two-second tone, `tracks.rs` the Encore rows as queue entries.
//! The functions here keep their signatures: the app and the pages call them.

pub mod normalize;
pub mod settings;
pub mod test_sound;
pub mod tracks;

pub use cadenza::engine::{Command, Engine, Event, PlayState, Status, Track};

use cadenza::embed::EmbeddedPlayer;
use iced::Subscription;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

static PLAYER: Mutex<Option<Arc<EmbeddedPlayer>>> = Mutex::new(None);

/// Shuffle mode of the player: when on, every new queue plays its rest in random order
/// (the player keeps the original order to put it back, see `player::queue`).
static SHUFFLE: AtomicBool = AtomicBool::new(false);

/// This computer's name, as the device list shows it.
pub fn hostname() -> String {
    let h = gethostname::gethostname().to_string_lossy().trim().to_string();
    if h.is_empty() {
        "this computer".into()
    } else {
        h
    }
}

/// "Encore on <hostname>": the device name in Your devices and in the account's sessions.
pub fn device_name() -> String {
    format!("Encore on {}", hostname())
}

/// Start the engine (once) with its config under `<config dir>/encore-desktop/cadenza`.
///
/// On the very first start (no `config.toml` yet) the engine is put in its plain mode
/// (bit-perfect pass-through: the "Play through Cadenza on this device" switch is off until the
/// user turns it on) on the shared output (the OS mixer), so other apps keep their sound and
/// exclusive mode is the user's choice (the Exclusive switch). A computer with no sound device
/// at all then gets the null output, so the app plays silently rather than failing, and the
/// backend it replaced is remembered (`output-fallback`): a later start puts that backend back
/// once it lists a device. The fallback is never written over a backend the user had: with the
/// device unplugged or switched off, the engine's open error shows on the player bar instead.
///
/// Device listing probes every device of every backend and can take seconds, so an ordinary
/// start does none on this thread: only the first start does (once per install), and the
/// check for a remembered fallback runs on a thread of its own.
pub fn start() -> Result<Engine, String> {
    let mut slot = PLAYER.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(p) = slot.as_ref() {
        return Ok(p.engine().clone());
    }
    let dir = crate::config::cadenza_dir();
    let first_run = !dir.join("config.toml").exists();
    let player = EmbeddedPlayer::start(dir.clone(), device_name())
        .map_err(|e| format!("Cadenza could not start: {e}"))?;
    let engine = player.engine().clone();
    if first_run {
        let mut s = engine.settings();
        first_run_settings(&mut s, &cadenza::output::backends());
        if s.output.backend != "null" && !has_output_device(&s.output.backend) {
            log::warn!(
                "no sound device for the {} output: playing through the null output until one is there",
                s.output.backend
            );
            remember_fallback(&dir, &s.output.backend);
            s.output.backend = "null".into();
            s.output.device.clear();
        }
        engine.update_settings(s);
    }
    // the fader, plus the normalization gain of each song as it starts
    normalize::watch(&engine, crate::config::Config::load().volume);
    log::info!("Cadenza engine running ({})", dir.display());
    *slot = Some(Arc::new(player));
    drop(slot);
    if !first_run && fallback_file(&dir).exists() {
        let (engine, dir) = (engine.clone(), dir);
        let _ = std::thread::Builder::new()
            .name("encore-output-check".into())
            .spawn(move || restore_output(&engine, &dir));
    }
    Ok(engine)
}

/// The first start's settings: plain mode, and the shared output when this build has one and
/// the default is an exclusive backend (WASAPI exclusive on Windows, CoreAudio hog mode on
/// macOS).
fn first_run_settings(s: &mut cadenza::config::Settings, backends: &[&str]) {
    s.dsp.bit_perfect = true;
    if backends.contains(&"cpal") && settings::exclusive_wanted(s) {
        s.output.backend = "cpal".into();
        s.output.device.clear();
    }
}

/// `<cadenza dir>/output-fallback`: the backend the null output stands in for.
fn fallback_file(dir: &std::path::Path) -> std::path::PathBuf {
    dir.join("output-fallback")
}

fn remember_fallback(dir: &std::path::Path, backend: &str) {
    if let Err(e) = std::fs::write(fallback_file(dir), backend) {
        log::warn!("could not remember the output to come back to: {e}");
    }
}

/// Put the remembered backend back when it lists a device again. The user choosing any output
/// in the meantime ends the stand-in too.
fn restore_output(engine: &Engine, dir: &std::path::Path) {
    let file = fallback_file(dir);
    let Ok(was) = std::fs::read_to_string(&file) else {
        return;
    };
    let was = was.trim().to_string();
    let mut s = engine.settings();
    if s.output.backend != "null" || was.is_empty() || was == "null" {
        let _ = std::fs::remove_file(&file);
        return;
    }
    if has_output_device(&was) {
        log::info!("a sound device is there for the {was} output again: using it");
        s.output.backend = was;
        s.output.device.clear();
        engine.update_settings(s);
        let _ = std::fs::remove_file(&file);
    }
}

/// Whether `backend` lists at least one device (`endpoint://` outputs are network players and
/// always count).
fn has_output_device(backend: &str) -> bool {
    if backend.starts_with("endpoint") {
        return true;
    }
    cadenza::output::list_devices()
        .iter()
        .any(|d| d.backend == backend)
}

/// The embedded player, when started.
pub fn player() -> Option<Arc<EmbeddedPlayer>> {
    PLAYER.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The engine handle, when started.
pub fn engine() -> Option<Engine> {
    player().map(|p| p.engine().clone())
}

/// Whether the engine has been started.
pub fn running() -> bool {
    player().is_some()
}

/// The latest status (an empty one before the engine starts).
pub fn status() -> Arc<Status> {
    engine()
        .map(|e| e.status())
        .unwrap_or_else(|| Arc::new(Status::default()))
}

/// Send a transport command; nothing happens before the engine starts.
pub fn send(cmd: Command) {
    if let Some(e) = engine() {
        e.send(cmd);
    }
}

/// Shuffle mode on or off (the player's switch; `play` reads it).
pub fn set_shuffle(on: bool) {
    SHUFFLE.store(on, Ordering::SeqCst);
}

/// Whether shuffle mode is on.
pub fn shuffle_on() -> bool {
    SHUFFLE.load(Ordering::SeqCst)
}

/// Replace the queue and play `index` (starting the engine if needed). In a group listen the
/// list becomes the group's queue (see `player::group`); while this computer is linked to
/// another device the list goes there instead (see `player::devices`), and with shuffle on the
/// songs after `index` play in random order.
pub fn play(tracks: Vec<Track>, index: usize, position: f64) -> Result<(), String> {
    if tracks.is_empty() {
        return Err("Nothing to play".into());
    }
    if crate::player::group::forward_play(&tracks, index, position) {
        return Ok(());
    }
    if crate::player::devices::forward_play(&tracks, index) {
        return Ok(());
    }
    let e = start()?;
    let index = index.min(tracks.len() - 1);
    let tracks = if shuffle_on() {
        crate::player::queue::shuffle_new(tracks, index)
    } else {
        crate::player::queue::forget_order();
        tracks
    };
    e.play_queue(tracks, index, position);
    Ok(())
}

/// Append to the queue (starts the engine if needed; plays when nothing is playing).
pub fn enqueue(tracks: Vec<Track>) -> Result<(), String> {
    if crate::player::group::forward_queue(&tracks, false) || crate::player::devices::forward_queue(&tracks, false) {
        return Ok(());
    }
    let e = start()?;
    crate::player::queue::order_push(&tracks);
    if e.status().queue.is_empty() {
        e.play_queue(tracks, 0, 0.0);
    } else {
        e.send(Command::Enqueue(tracks));
    }
    Ok(())
}

/// Insert after the current song.
pub fn play_next(tracks: Vec<Track>) -> Result<(), String> {
    if crate::player::group::forward_queue(&tracks, true) || crate::player::devices::forward_queue(&tracks, true) {
        return Ok(());
    }
    let e = start()?;
    let st = e.status();
    crate::player::queue::order_insert_after(&st, &tracks);
    if st.queue.is_empty() {
        e.play_queue(tracks, 0, 0.0);
    } else {
        e.send(Command::PlayNext(tracks));
    }
    Ok(())
}

/// Volume as a 0…1 fader (the song's normalization gain goes on top, see `normalize`).
pub fn set_volume(level: f64) {
    normalize::set_level(engine().as_ref(), level);
}

/// The fader as the user set it (0…1): what the volume slider shows. The engine's own volume
/// also carries the song's normalization gain.
pub fn volume_level() -> f64 {
    normalize::level()
}

/// Stop the link and the engine (the output device is released). Called when the app quits.
pub fn shutdown() {
    let p = PLAYER.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(p) = p {
        match Arc::try_unwrap(p) {
            Ok(p) => p.shutdown(),
            Err(p) => {
                p.disconnect();
                p.engine().shutdown();
            }
        }
    }
}

/// The engine's events as an iced subscription: status snapshots (position, queue, state),
/// track starts and "played enough" marks. Waits for the engine to start.
pub fn subscription() -> Subscription<Event> {
    Subscription::run(events)
}

fn events() -> impl iced::futures::Stream<Item = Event> {
    iced::stream::channel(64, |mut out: iced::futures::channel::mpsc::Sender<Event>| async move {
        use iced::futures::SinkExt;
        use tokio::sync::broadcast::error::RecvError;
        loop {
            let Some(e) = engine() else {
                tokio::time::sleep(Duration::from_millis(250)).await;
                continue;
            };
            let mut rx = e.subscribe();
            // The status of the moment, so a late subscriber shows what already plays.
            if out.send(Event::Status(e.status())).await.is_err() {
                return;
            }
            loop {
                match rx.recv().await {
                    Ok(ev) => {
                        if out.send(ev).await.is_err() {
                            return;
                        }
                    }
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => break,
                }
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_start_plays_plain_on_the_shared_output() {
        let mut s = cadenza::config::Settings::default();
        s.output.backend = "wasapi".into();
        s.output.device = "Speakers".into();
        first_run_settings(&mut s, &["null", "cpal", "wasapi", "endpoint"]);
        assert!(s.dsp.bit_perfect);
        assert_eq!((s.output.backend.as_str(), s.output.device.as_str()), ("cpal", ""));
        // without a shared backend the default stays
        let mut s = cadenza::config::Settings::default();
        s.output.backend = "coreaudio".into();
        first_run_settings(&mut s, &["null", "coreaudio"]);
        assert_eq!(s.output.backend, "coreaudio");
        // a shared default is left as it is
        let mut s = cadenza::config::Settings::default();
        s.output.backend = "cpal".into();
        s.output.device = "pipewire".into();
        first_run_settings(&mut s, &["null", "cpal", "alsa"]);
        assert_eq!((s.output.backend.as_str(), s.output.device.as_str()), ("cpal", "pipewire"));
    }
}
