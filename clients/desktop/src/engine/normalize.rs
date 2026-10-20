//! Volume normalization on this computer (p6 item 3, client-shapes §5.1): the three settings
//! the web keeps in localStorage (`encore.norm.mode`, `.target`, `.clip`, here in the config's
//! `local` map under the same keys), the gain math the web (player/fx.rs) and the phone
//! (playback/Normalization.kt) use, and the gain applied per song.
//!
//! The math:
//!
//! ```text
//! applied dB = rg_gain + (target − (−18))
//! ```
//!
//! where `rg_gain` is the song's (or its album's) ReplayGain in dB relative to −18 LUFS (the
//! server's `gain` object on every track row, §2.1) and `target` the chosen loudness in LUFS: a
//! song measured at −10 LUFS has rg_gain −8; at target −14 it plays at −8 + 4 = −4 dB, i.e. at
//! −14 LUFS. Album mode takes the album gain (the track gain when the album has none), Track mode
//! the track gain (the album gain when the track has none), each with the peak that goes with
//! it. "Prevent clipping" keeps peak × gain ≤ 1 (gain ≤ −20·log10(peak) dB); with no peak known
//! it allows no boost above 0 dB, and a cut is never limited. The result stays within −30…+15 dB.
//!
//! Where the gain goes: the pinned Cadenza (rev 17673b3) has no per-track gain (`Track` has no
//! gain field, `Command` no gain command), and the engine decodes the stream itself, so the app
//! cannot scale the samples before they reach it. The gain rides on the engine's volume instead:
//! `SetVolume(fader dB + gain dB)`, sent when a song starts (a thread of its own listens to the
//! engine's `TrackStarted`, so the first few milliseconds of a song can still have the gain of
//! the one before). The fader the user sees stays the user's ([`level`]). In the engine's
//! bit-perfect mode ("Play through Cadenza on this device" off) the engine ignores the volume,
//! so normalization is off there. docs/cadenza/2026-10-track-gain.md asks Cadenza for a real
//! per-track gain field (section 4.L of the 0.27 brief, not in the pinned rev).

use super::{Command, Engine, Event, Status, Track};
use cadenza_dsp::volume::Volume;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

/// The config keys, the web's localStorage keys.
pub const MODE_KEY: &str = "encore.norm.mode";
pub const TARGET_KEY: &str = "encore.norm.target";
pub const CLIP_KEY: &str = "encore.norm.clip";

/// The ReplayGain reference loudness.
pub const REFERENCE_LUFS: f64 = -18.0;
pub const MIN_DB: f64 = -30.0;
pub const MAX_DB: f64 = 15.0;
pub const MIN_TARGET: i32 = -23;
pub const MAX_TARGET: i32 = -5;
pub const DEFAULT_TARGET: i32 = -14;

/// Off / Track / Album.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Off,
    Track,
    Album,
}

impl Mode {
    pub fn id(self) -> &'static str {
        match self {
            Mode::Off => "off",
            Mode::Track => "track",
            Mode::Album => "album",
        }
    }

    /// An unknown value is Off.
    pub fn parse(s: &str) -> Mode {
        match s {
            "track" => Mode::Track,
            "album" => Mode::Album,
            _ => Mode::Off,
        }
    }

    /// The help line under the switch.
    pub fn help(self) -> &'static str {
        match self {
            Mode::Off => "Every song plays at its own level.",
            Mode::Track => "Every song plays at about the same loudness.",
            Mode::Album => "Every album plays at about the same loudness; the songs of an album keep their differences.",
        }
    }
}

/// The three settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Prefs {
    pub mode: Mode,
    /// −23 … −5 LUFS.
    pub target: i32,
    /// Prevent clipping.
    pub clip: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            mode: Mode::Off,
            target: DEFAULT_TARGET,
            clip: true,
        }
    }
}

impl Prefs {
    pub fn on(&self) -> bool {
        self.mode != Mode::Off
    }

    /// The settings as the config keeps them (anything unknown falls back to the default; the
    /// target is a number or a numeric string, rounded and kept within −23 … −5).
    pub fn from_config(c: &crate::config::Config) -> Prefs {
        let mode = Mode::parse(c.local_str(MODE_KEY));
        let target = c
            .local(TARGET_KEY)
            .and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok())))
            .filter(|t| t.is_finite())
            .map(|t| (t.round() as i64).clamp(MIN_TARGET as i64, MAX_TARGET as i64) as i32)
            .unwrap_or(DEFAULT_TARGET);
        let clip = match c.local(CLIP_KEY) {
            Some(Value::Bool(b)) => *b,
            Some(Value::String(s)) => !matches!(s.as_str(), "false" | "0"),
            _ => true,
        };
        Prefs { mode, target, clip }
    }

    /// Write the settings into the config.
    pub fn store(&self, c: &mut crate::config::Config) {
        c.local.insert(MODE_KEY.into(), Value::String(self.mode.id().into()));
        c.local.insert(TARGET_KEY.into(), Value::from(self.target));
        c.local.insert(CLIP_KEY.into(), Value::Bool(self.clip));
    }
}

/// The `gain` object of a track row (§2.1): every field optional.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Gain {
    pub track: Option<f64>,
    pub album: Option<f64>,
    pub track_peak: Option<f64>,
    pub album_peak: Option<f64>,
}

impl Gain {
    /// The row's `gain`; None when neither the track nor the album gain is known (`{}`, null,
    /// missing: an older server, or a song not measured yet).
    pub fn from_row(row: &Value) -> Option<Gain> {
        let g = row.get("gain")?;
        let num = |k: &str| g.get(k).and_then(Value::as_f64).filter(|x| x.is_finite());
        let gain = Gain {
            track: num("track"),
            album: num("album"),
            track_peak: num("track_peak").filter(|p| *p > 0.0),
            album_peak: num("album_peak").filter(|p| *p > 0.0),
        };
        (gain.track.is_some() || gain.album.is_some()).then_some(gain)
    }
}

/// Which gain a song plays with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Basis {
    /// Nothing known about the song's loudness.
    None,
    Track,
    Album,
}

/// What normalization does to one song.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Decision {
    pub prefs: Prefs,
    /// The dB applied (0 when nothing is).
    pub db: f64,
    pub basis: Basis,
    /// "Prevent clipping" lowered the gain.
    pub limited: bool,
    /// What the gain asked for before clipping and the limits.
    pub requested_db: f64,
    /// The engine plays bit-perfect: nothing is applied.
    pub bit_perfect: bool,
}

/// dB → the factor the samples are multiplied by (the engine's volume stage does it here).
#[cfg(test)]
fn linear(db: f64) -> f64 {
    10f64.powf(db / 20.0)
}

/// The gain for a ReplayGain value at a target loudness (before clipping and the limits).
pub fn applied_db(rg_gain: f64, target: i32) -> f64 {
    rg_gain + (f64::from(target) - REFERENCE_LUFS)
}

/// The most gain a song with this linear peak takes without clipping; 0 dB when it is unknown.
pub fn clip_ceiling_db(peak: Option<f64>) -> f64 {
    match peak {
        Some(p) if p > 0.0 => -20.0 * p.log10(),
        _ => 0.0,
    }
}

/// The decision for a song with `gain` (None: nothing known) under `prefs`.
pub fn decide(prefs: Prefs, gain: Option<&Gain>, bit_perfect: bool) -> Decision {
    let none = Decision {
        prefs,
        db: 0.0,
        basis: Basis::None,
        limited: false,
        requested_db: 0.0,
        bit_perfect,
    };
    if !prefs.on() {
        return none;
    }
    let Some(g) = gain else {
        return none;
    };
    let use_album = (prefs.mode == Mode::Album && g.album.is_some()) || g.track.is_none();
    let Some(rg) = (if use_album { g.album } else { g.track }) else {
        return none;
    };
    let peak = if use_album { g.album_peak.or(g.track_peak) } else { g.track_peak.or(g.album_peak) };
    let wanted = applied_db(rg, prefs.target);
    let mut db = wanted;
    let mut limited = false;
    if prefs.clip {
        let ceiling = clip_ceiling_db(peak);
        if db > ceiling {
            db = ceiling;
            limited = true;
        }
    }
    Decision {
        prefs,
        db: db.clamp(MIN_DB, MAX_DB),
        basis: if use_album { Basis::Album } else { Basis::Track },
        limited,
        requested_db: wanted,
        bit_perfect,
    }
}

impl Decision {
    /// The dB that reaches the samples (0 when off, without data, or bit-perfect).
    pub fn effective_db(&self) -> f64 {
        if !self.prefs.on() || self.bit_perfect || self.basis == Basis::None {
            0.0
        } else {
            self.db
        }
    }

    /// The Signal path's value: "−3.2 dB (album gain, target −14 LUFS)", "No loudness data for
    /// this song", "Off for bit-perfect output"; "" while normalization is off.
    pub fn line(&self) -> String {
        if !self.prefs.on() {
            return String::new();
        }
        if self.bit_perfect {
            return "Off for bit-perfect output".into();
        }
        match self.basis {
            Basis::None => "No loudness data for this song".into(),
            b => format!(
                "{} ({} gain, target {})",
                db_text(self.db),
                if b == Basis::Album { "album" } else { "track" },
                lufs_text(self.prefs.target)
            ),
        }
    }

    /// The row's note: every sentence that applies, joined by a space. `source` is the
    /// `gain.source` of the song's `/info` ("tags" | "measured" | "").
    pub fn note(&self, source: &str) -> String {
        if !self.prefs.on() {
            return String::new();
        }
        let mut out: Vec<String> = Vec::new();
        if self.bit_perfect {
            out.push("The DAC gets the samples untouched.".into());
            return out.join(" ");
        }
        match self.basis {
            Basis::None => out.push("It plays at its own level until Encore has measured it.".into()),
            b => {
                if self.limited {
                    out.push(format!(
                        "Lowered to keep the peaks from clipping (the song asked for {}).",
                        db_text(self.requested_db)
                    ));
                }
                if self.prefs.mode == Mode::Album && b == Basis::Track {
                    out.push("No album gain for this song, so its track gain is used.".into());
                }
                if self.prefs.mode == Mode::Track && b == Basis::Album {
                    out.push("No track gain for this song, so its album gain is used.".into());
                }
                match source {
                    "tags" => out.push("From the file's ReplayGain tags.".into()),
                    "measured" => out.push("Measured by Encore.".into()),
                    _ => {}
                }
            }
        }
        out.join(" ")
    }
}

/// −3.24 → "−3.2 dB", 4 → "+4.0 dB", −0.01 → "0.0 dB" (a real minus sign, one decimal). Halves round up
/// as the web (`Math.round`) and the phone do: −3.25 → "−3.2 dB".
pub fn db_text(db: f64) -> String {
    let r = ((db * 10.0) + 0.5).floor() / 10.0;
    let s = format!("{:.1}", r.abs());
    if r > 0.0 {
        format!("+{s} dB")
    } else if r < 0.0 {
        format!("\u{2212}{s} dB")
    } else {
        "0.0 dB".into()
    }
}

/// −14 → "−14 LUFS".
pub fn lufs_text(v: i32) -> String {
    if v < 0 {
        format!("\u{2212}{} LUFS", -v)
    } else {
        format!("{v} LUFS")
    }
}

// ----- the gain in force ---------------------------------------------------------------------

/// The settings in force (read from the config at first use).
fn prefs_slot() -> &'static RwLock<Prefs> {
    static P: OnceLock<RwLock<Prefs>> = OnceLock::new();
    P.get_or_init(|| RwLock::new(Prefs::from_config(&crate::config::Config::load())))
}

/// The gains of the songs this app queued, by Encore track id.
fn gains() -> &'static Mutex<HashMap<i64, Option<Gain>>> {
    static G: OnceLock<Mutex<HashMap<i64, Option<Gain>>>> = OnceLock::new();
    G.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The fader as the user set it (0…1), from the config at first use.
fn level_slot() -> &'static Mutex<f64> {
    static L: OnceLock<Mutex<f64>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(crate::config::Config::load().volume))
}

/// The volumes sent to the engine lately, newest last, and since when the newest one has been
/// on its way (None once a status showed it). While it is on its way a status may still show
/// the volume before it; once it arrived, a status with another volume is someone else's (see
/// [`reconcile`]).
static SENT: Mutex<(Vec<Volume>, Option<Instant>)> = Mutex::new((Vec::new(), None));

/// The settings in force.
pub fn prefs() -> Prefs {
    *prefs_slot().read().unwrap_or_else(|e| e.into_inner())
}

/// New settings: kept, and the song that plays gets its new gain at once.
pub fn set_prefs(p: Prefs) {
    *prefs_slot().write().unwrap_or_else(|e| e.into_inner()) = p;
    if let Some(e) = super::engine() {
        apply(&e, &e.status());
    }
}

/// Give the song that plays its gain again (its row just arrived).
pub fn reapply() {
    if let Some(e) = super::engine() {
        apply(&e, &e.status());
    }
}

/// Remember the gain of an Encore row (called for every row queued here). A row without the
/// field leaves a gain seen before in place.
pub fn remember(row: &Value) {
    let Some(id) = super::tracks::id_of(row) else {
        return;
    };
    let g = Gain::from_row(row);
    let mut map = gains().lock().unwrap_or_else(|e| e.into_inner());
    if map.len() > 20_000 {
        map.clear();
    }
    match g {
        Some(g) => {
            map.insert(id, Some(g));
        }
        None => {
            map.entry(id).or_insert(None);
        }
    }
}

/// Whether a row of this song was seen (with or without a gain): a song queued from elsewhere
/// (another device through the link) is not, and the player asks the server for its row.
pub fn seen(id: i64) -> bool {
    gains().lock().unwrap_or_else(|e| e.into_inner()).contains_key(&id)
}

/// The gain known for this song.
pub fn gain_of(id: i64) -> Option<Gain> {
    gains().lock().unwrap_or_else(|e| e.into_inner()).get(&id).copied().flatten()
}

/// The decision for a song of the engine's queue (an Encore id, or none for files).
pub fn decision_for(track: Option<&Track>, status: &Status) -> Decision {
    let g = track.and_then(super::tracks::encore_id).and_then(gain_of);
    decide(prefs(), g.as_ref(), status.settings.dsp.bit_perfect)
}

/// The fader the user set (0…1): what the volume slider shows.
pub fn level() -> f64 {
    *level_slot().lock().unwrap_or_else(|e| e.into_inner())
}

/// The engine's volume for a fader level and a normalization gain.
pub fn volume_for(level: f64, gain_db: f64) -> Volume {
    let fader = Volume::from_linear(level.clamp(0.0, 1.0));
    if fader.gain_db <= Volume::MIN_DB {
        return fader;
    }
    Volume {
        gain_db: (fader.gain_db + gain_db).clamp(Volume::MIN_DB, 20.0),
        muted: false,
    }
}

fn send(e: &Engine, v: Volume) {
    {
        let mut sent = SENT.lock().unwrap_or_else(|e| e.into_inner());
        sent.0.retain(|x| !same(x, &v));
        sent.0.push(v);
        if sent.0.len() > 4 {
            sent.0.remove(0);
        }
        sent.1 = Some(Instant::now());
    }
    e.send(Command::SetVolume(v));
}

fn same(a: &Volume, b: &Volume) -> bool {
    a.muted == b.muted && (a.gain_db - b.gain_db).abs() < 1e-6
}

/// The user moved the fader (before the engine runs it is only kept).
pub fn set_level(e: Option<&Engine>, level: f64) {
    *level_slot().lock().unwrap_or_else(|e| e.into_inner()) = level.clamp(0.0, 1.0);
    if let Some(e) = e {
        apply(e, &e.status());
    }
}

/// Send the volume for the song the status shows: the fader plus its gain.
pub fn apply(e: &Engine, st: &Status) {
    let d = decision_for(st.track.as_ref(), st);
    send(e, volume_for(level(), d.effective_db()));
}

/// What a status's volume means: ours (or ours on its way), or someone else's fader.
#[derive(Debug, PartialEq)]
enum Seen {
    Ours,
    Theirs,
}

/// A status's volume against what was sent. The newest volume counts as arrived once a status
/// shows it, or after two seconds (another volume may have come in right behind it).
fn classify(sent: &mut (Vec<Volume>, Option<Instant>), v: &Volume, now: Instant) -> Seen {
    if let Some(newest) = sent.0.last() {
        if same(newest, v) {
            sent.1 = None;
            return Seen::Ours;
        }
    }
    if let Some(at) = sent.1 {
        if now.duration_since(at) < Duration::from_secs(2) {
            return Seen::Ours;
        }
        sent.1 = None;
    }
    if v.muted || sent.0.is_empty() {
        return Seen::Ours;
    }
    Seen::Theirs
}

/// The fader under a volume another device set. That device sees the engine's volume (Cadenza's link
/// reports it), which is the fader with this song's gain on it, so the gain comes off before it becomes the
/// fader: the next [`apply`] then sends the volume the device asked for (up to what the fader at the top
/// gives). At the bottom, and muted, it is the volume itself.
fn fader_under(v: &Volume, gain_db: f64) -> f64 {
    if v.muted || v.gain_db <= Volume::MIN_DB {
        return v.to_linear();
    }
    Volume { gain_db: v.gain_db - gain_db, muted: false }.to_linear()
}

/// A status shows a volume this app did not send (another device through the link, a UPnP
/// control point): without the song's gain that is the new fader, and the gain goes back on top of it.
fn reconcile(e: &Engine, st: &Status) {
    let theirs = {
        let mut sent = SENT.lock().unwrap_or_else(|e| e.into_inner());
        classify(&mut sent, &st.volume, Instant::now()) == Seen::Theirs
    };
    if theirs {
        let g = decision_for(st.track.as_ref(), st).effective_db();
        *level_slot().lock().unwrap_or_else(|e| e.into_inner()) = fader_under(&st.volume, g);
        apply(e, st);
    }
}

/// Start the thread that gives every song its gain as it starts (once per engine), with the
/// fader at `level`.
pub fn watch(e: &Engine, level: f64) {
    *level_slot().lock().unwrap_or_else(|e| e.into_inner()) = level.clamp(0.0, 1.0);
    let mut rx = e.subscribe();
    apply(e, &e.status());
    let e = e.clone();
    let _ = std::thread::Builder::new().name("encore-normalize".into()).spawn(move || {
        use tokio::sync::broadcast::error::RecvError;
        // "Play through Cadenza on this device" switched: the gain comes or goes with it
        let mut bit_perfect: Option<bool> = None;
        loop {
            match rx.blocking_recv() {
                Ok(Event::TrackStarted(_)) => apply(&e, &e.status()),
                Ok(Event::Status(st)) => {
                    let bp = st.settings.dsp.bit_perfect;
                    if bit_perfect.is_some_and(|was| was != bp) {
                        apply(&e, &st);
                    } else {
                        reconcile(&e, &st);
                    }
                    bit_perfect = Some(bp);
                }
                Ok(_) => {}
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => break,
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn g(track: Option<f64>, album: Option<f64>, tp: Option<f64>, ap: Option<f64>) -> Gain {
        Gain { track, album, track_peak: tp, album_peak: ap }
    }

    fn p(mode: Mode, target: i32, clip: bool) -> Prefs {
        Prefs { mode, target, clip }
    }

    #[test]
    fn the_formula_is_the_webs_and_the_phones() {
        // applied dB = rg_gain + (target − (−18))
        assert!((applied_db(-8.0, -14) - -4.0).abs() < 1e-9);
        assert!((applied_db(-8.0, -18) - -8.0).abs() < 1e-9);
        assert!((applied_db(-8.0, -23) - -13.0).abs() < 1e-9);
        assert!((linear(0.0) - 1.0).abs() < 1e-12);
        assert!((linear(-6.0) - 0.501_187_233_6).abs() < 1e-9);
    }

    #[test]
    fn off_and_no_data() {
        let gain = g(Some(-8.0), Some(-7.0), Some(0.9), Some(1.0));
        let off = decide(p(Mode::Off, -14, true), Some(&gain), false);
        assert_eq!(off.effective_db(), 0.0);
        assert_eq!(off.line(), "");
        let none = decide(p(Mode::Track, -14, true), None, false);
        assert_eq!(none.basis, Basis::None);
        assert_eq!(none.effective_db(), 0.0);
        assert_eq!(none.line(), "No loudness data for this song");
        assert_eq!(none.note("tags"), "It plays at its own level until Encore has measured it.");
        let bp = decide(p(Mode::Album, -14, true), Some(&gain), true);
        assert_eq!(bp.effective_db(), 0.0);
        assert_eq!(bp.line(), "Off for bit-perfect output");
        assert_eq!(bp.note("tags"), "The DAC gets the samples untouched.");
    }

    #[test]
    fn track_and_album_with_their_fallbacks() {
        let gain = g(Some(-8.0), Some(-6.5), Some(0.5), Some(0.6));
        let t = decide(p(Mode::Track, -14, true), Some(&gain), false);
        assert_eq!(t.basis, Basis::Track);
        assert!((t.db - -4.0).abs() < 1e-9);
        let a = decide(p(Mode::Album, -14, true), Some(&gain), false);
        assert_eq!(a.basis, Basis::Album);
        assert!((a.db - -2.5).abs() < 1e-9);
        assert_eq!(a.line(), "\u{2212}2.5 dB (album gain, target \u{2212}14 LUFS)");
        assert_eq!(a.note("measured"), "Measured by Encore.");
        // album mode without an album gain uses the track gain, and the other way round
        let only_track = decide(p(Mode::Album, -14, true), Some(&g(Some(-8.0), None, None, None)), false);
        assert_eq!(only_track.basis, Basis::Track);
        assert_eq!(only_track.note("tags"), "No album gain for this song, so its track gain is used. From the file's ReplayGain tags.");
        let only_album = decide(p(Mode::Track, -14, true), Some(&g(None, Some(-7.0), None, None)), false);
        assert_eq!(only_album.basis, Basis::Album);
        assert_eq!(only_album.note(""), "No track gain for this song, so its album gain is used.");
    }

    #[test]
    fn prevent_clipping() {
        // a quiet song (+7 dB at −14) whose peak is at half scale may rise only 6.02 dB
        let quiet = g(Some(3.0), None, Some(0.5), None);
        let on = decide(p(Mode::Track, -14, true), Some(&quiet), false);
        assert!(on.limited);
        assert!((on.db - 6.0206).abs() < 1e-4);
        assert!((on.requested_db - 7.0).abs() < 1e-9);
        assert_eq!(on.note(""), "Lowered to keep the peaks from clipping (the song asked for +7.0 dB).");
        assert!(!decide(p(Mode::Track, -14, true), Some(&g(Some(1.0), None, Some(0.5), None)), false).limited);
        // no peak known: no boost above 0 dB; a cut is never limited
        assert_eq!(decide(p(Mode::Track, -14, true), Some(&g(Some(2.0), None, None, None)), false).db, 0.0);
        assert!((decide(p(Mode::Track, -14, true), Some(&g(Some(-8.0), None, None, None)), false).db - -4.0).abs() < 1e-9);
        // clipping allowed: the full boost, within the +15 dB limit
        assert!((decide(p(Mode::Track, -14, false), Some(&quiet), false).db - 7.0).abs() < 1e-9);
        assert_eq!(decide(p(Mode::Track, -5, false), Some(&g(Some(20.0), None, None, None)), false).db, 15.0);
        assert_eq!(decide(p(Mode::Track, -23, false), Some(&g(Some(-40.0), None, None, None)), false).db, -30.0);
    }

    #[test]
    fn settings_texts_and_rows() {
        let mut c = crate::config::Config::default();
        assert_eq!(Prefs::from_config(&c), Prefs::default());
        c.local.insert(MODE_KEY.into(), json!("loud"));
        c.local.insert(TARGET_KEY.into(), json!(-40));
        c.local.insert(CLIP_KEY.into(), json!(false));
        assert_eq!(Prefs::from_config(&c), p(Mode::Off, -23, false));
        c.local.insert(TARGET_KEY.into(), json!("3"));
        assert_eq!(Prefs::from_config(&c).target, -5);
        let mut d = crate::config::Config::default();
        p(Mode::Album, -16, true).store(&mut d);
        assert_eq!(Prefs::from_config(&d), p(Mode::Album, -16, true));
        assert_eq!(d.local[MODE_KEY], json!("album"));
        assert_eq!(db_text(4.0), "+4.0 dB");
        assert_eq!(db_text(-3.24), "\u{2212}3.2 dB");
        assert_eq!(db_text(-0.01), "0.0 dB");
        // halves round up, as on the web and the phone (rg −7.25 at target −14: −3.25 dB)
        assert_eq!(db_text(-3.25), "\u{2212}3.2 dB");
        assert_eq!(db_text(applied_db(-7.25, -14)), "\u{2212}3.2 dB");
        assert_eq!(db_text(2.35), "+2.4 dB");
        assert_eq!(lufs_text(-14), "\u{2212}14 LUFS");
        let row = json!({ "id": 5, "gain": { "track": -7.21, "album": null, "track_peak": 0.988 } });
        let gain = Gain::from_row(&row).unwrap();
        assert_eq!(gain.track, Some(-7.21));
        assert_eq!(gain.album, None);
        assert_eq!(gain.track_peak, Some(0.988));
        assert!(Gain::from_row(&json!({ "gain": {} })).is_none());
        assert!(Gain::from_row(&json!({ "gain": { "track_peak": 1.0 } })).is_none());
        assert!(Gain::from_row(&json!({ "id": 1 })).is_none());
        assert!(Gain::from_row(&json!({ "gain": { "track": "-7.2" } })).is_none(), "a number, never a string");
    }

    #[test]
    fn the_gain_rides_on_the_fader() {
        let v = volume_for(1.0, -6.0);
        assert!((v.gain_db - -6.0).abs() < 1e-9);
        // the fader at half (−18.06 dB on the engine's curve) plus a +4 dB boost
        let half = Volume::from_linear(0.5).gain_db;
        assert!((volume_for(0.5, 4.0).gain_db - (half + 4.0)).abs() < 1e-9);
        // a silent fader stays silent, whatever the gain
        assert_eq!(volume_for(0.0, 10.0).gain(), 0.0);
        // the engine's ceiling is +20 dB
        assert_eq!(volume_for(1.0, 30.0).gain_db, 20.0);
    }

    #[test]
    fn a_volume_from_another_device_loses_the_gain_before_it_becomes_the_fader() {
        // the fader at the top and a song at −8 dB: the engine plays, and another device sees, −8 dB
        assert!((volume_for(1.0, -8.0).gain_db - -8.0).abs() < 1e-9);
        // that device asks for −12 dB: the fader goes to −4 dB, and the volume sent is the −12 dB asked for
        let asked = Volume { gain_db: -12.0, muted: false };
        let level = fader_under(&asked, -8.0);
        assert!((Volume::from_linear(level).gain_db - -4.0).abs() < 1e-6);
        assert!((volume_for(level, -8.0).gain_db - -12.0).abs() < 1e-6, "not −20 dB");
        // a boost: the same the other way round
        let level = fader_under(&Volume { gain_db: -10.0, muted: false }, 4.0);
        assert!((volume_for(level, 4.0).gain_db - -10.0).abs() < 1e-6);
        // more than the fader at the top gives: the top
        assert_eq!(fader_under(&Volume { gain_db: -2.0, muted: false }, -8.0), 1.0);
        // no gain: the volume is the fader, as before; the bottom and mute stay as they are
        assert!((fader_under(&asked, 0.0) - asked.to_linear()).abs() < 1e-12);
        assert_eq!(fader_under(&Volume { gain_db: Volume::MIN_DB, muted: false }, 6.0), 0.0);
        assert_eq!(fader_under(&Volume { gain_db: -6.0, muted: true }, -8.0), 0.0);
    }

    #[test]
    fn rows_are_remembered_once_known() {
        assert!(!seen(987_654));
        remember(&json!({ "id": 987_654, "title": "x" }));
        assert!(seen(987_654) && gain_of(987_654).is_none());
        remember(&json!({ "id": 987_654, "gain": { "track": -5.0 } }));
        assert!(gain_of(987_654).is_some());
        // a later row without the field does not forget it
        remember(&json!({ "id": 987_654, "title": "x" }));
        assert_eq!(gain_of(987_654).and_then(|g| g.track), Some(-5.0));
    }

    #[test]
    fn a_volume_from_elsewhere_is_told_from_ours() {
        let t0 = Instant::now();
        let a = Volume { gain_db: -6.0, muted: false };
        let b = Volume { gain_db: -10.0, muted: false };
        let other = Volume { gain_db: -3.0, muted: false };
        let mut sent = (vec![a, b], Some(t0));
        // b is on its way: a status still showing a, or anything else, is not someone else's yet
        assert_eq!(classify(&mut sent, &a, t0), Seen::Ours);
        assert_eq!(classify(&mut sent, &other, t0 + Duration::from_millis(500)), Seen::Ours);
        // b arrived: the older one and b are ours, a new value is someone else's
        assert_eq!(classify(&mut sent, &b, t0 + Duration::from_millis(600)), Seen::Ours);
        assert_eq!(sent.1, None);
        assert_eq!(classify(&mut sent, &a, t0 + Duration::from_millis(700)), Seen::Theirs);
        assert_eq!(classify(&mut sent, &other, t0 + Duration::from_millis(700)), Seen::Theirs);
        // a newest volume that never shows counts as arrived after two seconds
        let mut lost = (vec![a, b], Some(t0));
        assert_eq!(classify(&mut lost, &other, t0 + Duration::from_secs(3)), Seen::Theirs);
        // muted, or nothing sent yet: left alone
        let mut none = (Vec::new(), None);
        assert_eq!(classify(&mut none, &other, t0), Seen::Ours);
        let mut fresh = (vec![b], None);
        assert_eq!(classify(&mut fresh, &Volume { gain_db: 0.0, muted: true }, t0), Seen::Ours);
    }
}
