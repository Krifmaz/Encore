//! Settings → Playback. Behaviour reference: web-rs/src/pages/settings.rs `paint_playback`
//! (quality, buffering, when the queue ends, server processing) plus the card of the engine
//! inside this app: Cadenza on this device. Every song plays through that engine, so the card
//! edits `cadenza::config::Settings` and hands them over with `engine::settings::apply`
//! (checked, then `engine.update_settings`, persisted by the embedded player, or written to its
//! config.toml before it has started). The Test sound and the Signal path are the player's.
//!
//! The "Play through Cadenza on this device" switch is the DSP pipeline on or off: off is the
//! engine's bit-perfect pass-through, on shows the whole card.

use super::{block, card, help, select, select_table, switch, switch_maybe, Opt};
use crate::api::ApiError;
use crate::engine::{self, normalize as norm, settings as es};
use crate::fmt;
use crate::pages::{Ctx, El};
use crate::theme::{self, Badge, Btn};
use crate::widgets::eq_graph::{self, GraphEvent};
use crate::widgets::{self, badge, btn_sm, icon_btn, seg, Icon};
use cadenza::config::{RateSetting, Settings};
use cadenza::engine::Status;
use cadenza::output::{choose_dsd, choose_rate, DeviceInfo, DsdCaps, Probe, MAX_PCM_RATE};
use cadenza_dsp::dsd::{DsdModulator, DsdOversampling, DsdPacking, DsdRate, OutputMode};
use cadenza_dsp::eq::{Band, BandKind, MAX_BANDS};
use cadenza_dsp::signal_path::rate_label;
use cadenza_dsp::{DitherKind, Family, FilterKind, GpuMode, HeadphoneMode, Phase};
use iced::widget::{column, container, row, slider};
use iced::{Alignment, Length, Subscription, Task};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// The web's `playback.defaults()`; the keys the desktop keeps on the account.
pub fn defaults() -> Value {
    json!({ "quality": "auto", "preload": true, "autoplay": true, "resume": true, "dsp": false, "dsp_filter": "", "dsp_dither": "tpdf", "dsp_rate": "device" })
}

/// `playback.clean(p)` for the keys this page shows: unknown values fall back to the defaults.
pub fn clean(p: &Value) -> Value {
    let mut c = defaults();
    if let (Some(dst), Some(src)) = (c.as_object_mut(), p.as_object()) {
        for (k, v) in src {
            dst.insert(k.clone(), v.clone());
        }
    }
    if !matches!(c["quality"].as_str(), Some("auto" | "original" | "saver")) {
        c["quality"] = json!("auto");
    }
    for k in ["preload", "autoplay", "resume"] {
        c[k] = json!(c[k] != json!(false));
    }
    c["dsp"] = json!(c["dsp"] == json!(true));
    if !c["dsp_filter"].is_string() {
        c["dsp_filter"] = json!("");
    }
    if !c["dsp_dither"].is_string() {
        c["dsp_dither"] = json!("tpdf");
    }
    let rate = c["dsp_rate"].clone();
    if rate != json!("device") && rate != json!("auto") {
        let r = rate.as_f64().or_else(|| rate.as_str().and_then(|s| s.parse().ok())).unwrap_or(0.0);
        c["dsp_rate"] = if r.is_finite() && r.fract() == 0.0 && r > 0.0 { json!(r as i64) } else { json!("device") };
    }
    c
}

const QUALITY_HELP: [(&str, &str); 3] = [
    ("auto", "The original file. Encore measures your connection while it plays and steps the stream down as far as it needs to keep the music playing – down to 96 kbps (64 kbps only on a very poor connection) – then back up as soon as the connection allows."),
    ("original", "Always the original file, even if it stutters on a slow connection. Formats a browser can't play are converted to FLAC (lossless)."),
    ("saver", "Always a small 96 kbps stream (AAC, or Opus in browsers without AAC) – for mobile data and slow links."),
];

const DSP_RATES: [i64; 8] = [88200, 96000, 176400, 192000, 352800, 384000, 705600, 768000];

/// The link between this computer and the DAC, with the highest PCM rate it carries. Applied
/// as the engine's rate ceiling (`output.max_rate`) until the engine has an `output.link`.
pub const LINKS: [(&str, &str, u32); 4] = [
    ("usb", "USB (no limit)", MAX_PCM_RATE),
    ("i2s", "I2S / HDMI (up to 768 kHz)", 768_000),
    ("spdif", "S/PDIF coaxial (up to 192 kHz)", 192_000),
    ("optical", "Optical (up to 96 kHz)", 96_000),
];

/// The link whose ceiling is `max_rate` ("custom" when none matches).
pub fn link_of(max_rate: u32) -> &'static str {
    LINKS.iter().find(|(_, _, hz)| *hz == max_rate).map(|(id, _, _)| *id).unwrap_or("custom")
}

const PHASES: [(&str, &str); 4] = [
    ("l", "Linear phase"),
    ("m", "Minimum phase"),
    ("la", "Linear phase, apodizing"),
    ("ma", "Minimum phase, apodizing"),
];

fn phase_of(id: &str) -> (Phase, bool) {
    match id {
        "m" => (Phase::Minimum, false),
        "la" => (Phase::Linear, true),
        "ma" => (Phase::Minimum, true),
        _ => (Phase::Linear, false),
    }
}

fn phase_id(phase: Phase, apodizing: bool) -> &'static str {
    match (phase, apodizing) {
        (Phase::Linear, false) => "l",
        (Phase::Minimum, false) => "m",
        (Phase::Linear, true) => "la",
        (Phase::Minimum, true) => "ma",
    }
}

const FAMILIES: [Family; 6] = [Family::Sinc, Family::ClosedForm, Family::PolySincGauss, Family::SincXtr, Family::SincMx, Family::PolySincExt3];

/// Cadenza's names and order, low to high.
const BAND_KINDS: [(BandKind, &str, &str); 6] = [
    (BandKind::HighPass, "high-pass", "Low cut"),
    (BandKind::LowShelf, "low-shelf", "Low shelf"),
    (BandKind::Peaking, "peaking", "Bell"),
    (BandKind::Notch, "notch", "Notch"),
    (BandKind::HighShelf, "high-shelf", "High shelf"),
    (BandKind::LowPass, "low-pass", "High cut"),
];
const EQ_SLOPES: [u32; 4] = [12, 24, 36, 48];

fn band_kind_id(k: BandKind) -> &'static str {
    BAND_KINDS.iter().find(|(x, _, _)| *x == k).map(|(_, id, _)| *id).unwrap_or("peaking")
}

/// The text boxes of one EQ band while they are edited (frequency, gain, Q).
#[derive(Clone, Debug, Default)]
pub struct BandEdit {
    pub freq: String,
    pub gain: String,
    pub q: String,
}

fn band_edit(b: &Band) -> BandEdit {
    BandEdit {
        freq: super::js_num(b.freq),
        gain: super::js_num(b.gain_db),
        q: super::js_num(b.q),
    }
}

#[derive(Debug)]
pub struct State {
    /// The playback preferences (the web's `encore.playback`), from the account.
    prefs: Value,
    /// `/api/playback/dsp`: server processing, if allowed.
    dsp: Value,
    /// The engine's settings as this page last read or wrote them.
    settings: Settings,
    devices: Option<Vec<DeviceInfo>>,
    gpu: Option<(bool, Option<String>)>,
    bands: Vec<BandEdit>,
    /// The EQ graph: the band picked, the gain it shows (±dB), and while a handle is dragged
    /// the bands as dragged with when the engine last got them.
    eq_sel: Option<usize>,
    eq_range: f32,
    eq_live: Option<(Vec<Band>, Instant)>,
    convolution: String,
    test_busy: bool,
    /// A slider is being dragged: the 2 s re-read leaves the settings alone.
    dragging: bool,
    /// Settings were just handed over; the engine may not report them yet.
    wrote_at: Option<Instant>,
    /// Volume normalization on this computer (kept in the config, as the web keeps it in
    /// localStorage).
    norm: norm::Prefs,
}

impl Default for State {
    fn default() -> Self {
        Self {
            prefs: defaults(),
            dsp: Value::Null,
            settings: Settings::default(),
            devices: None,
            gpu: None,
            bands: Vec::new(),
            eq_sel: None,
            eq_range: 12.0,
            eq_live: None,
            convolution: String::new(),
            test_busy: false,
            dragging: false,
            wrote_at: None,
            norm: norm::Prefs::default(),
        }
    }
}

#[derive(Clone, Debug)]
pub enum Message {
    Dsp(Result<Value, ApiError>),
    Devices(Vec<DeviceInfo>),
    Gpu(bool, Option<String>),
    /// A playback preference changed (saved to the account).
    Pref(String, Value),
    PrefSaved(Result<Value, ApiError>),
    /// The engine's settings were read again (every couple of seconds while the page is open).
    Tick,
    EngineOn(bool),
    Device(String),
    Exclusive(bool),
    Rate(String),
    Bits(String),
    Link(String),
    Mode(String),
    DsdRate(String),
    DsdModulator(String),
    DsdPacking(String),
    DsdOversampling(String),
    DsdGain(f64),
    DsdGainDone,
    Family(String),
    Phase(String),
    Length(String),
    Dither(String),
    HeadroomAuto(bool),
    Headroom(f64),
    HeadroomDone,
    SoftClip(bool),
    Headphones(String),
    Gpu2(String),
    Matrix(String),
    EqOn(bool),
    EqAdd,
    EqRemove(usize),
    EqKind(usize, String),
    /// `(band, column 0 freq / 1 gain / 2 q, text)`.
    EqEdit(usize, u8, String),
    EqCommit(usize),
    EqGraph(GraphEvent),
    EqSelect(usize),
    EqEnabled(usize, bool),
    EqSlope(usize, String),
    EqRange(String),
    /// Every gain back to 0 dB.
    EqFlat,
    EqClear,
    ConvInput(String),
    ConvPick,
    ConvPicked(Option<PathBuf>),
    ConvCommit,
    ConvClear,
    TestSound,
    TestDone(Result<(), String>),
    Reopen,
    RefreshDevices,
    /// The player's Signal path dialog (source, server step, the engine's stages, output).
    SignalPath,
    /// Volume normalization: Off / Track / Album.
    NormMode(String),
    /// The target level while the slider moves (LUFS).
    NormTarget(f64),
    NormTargetDone,
    /// Prevent clipping.
    NormClip(bool),
}

fn list_devices() -> Task<Message> {
    Task::perform(
        async move {
            tokio::task::spawn_blocking(cadenza::output::list_devices)
                .await
                .unwrap_or_default()
        },
        Message::Devices,
    )
}

pub fn open(state: &mut State, ctx: &mut Ctx) -> Task<Message> {
    state.prefs = clean(&ctx.session.user["prefs"]["playback"]);
    state.norm = norm::prefs();
    state.dsp = Value::Null;
    state.settings = es::settings();
    state.bands = state.settings.dsp.eq.iter().map(band_edit).collect();
    state.convolution = state.settings.dsp.convolution.clone();
    state.devices = None;
    state.dragging = false;
    state.wrote_at = None;
    let api = ctx.api.clone();
    let gpu_device = state.settings.dsp.gpu.device;
    let mut tasks = vec![
        Task::perform(async move { api.get("/playback/dsp").await }, Message::Dsp),
        list_devices(),
    ];
    if cadenza_dsp::gpu::COMPILED_IN {
        tasks.push(Task::perform(
            async move {
                tokio::task::spawn_blocking(move || cadenza::encore::state::gpu_available(gpu_device))
                    .await
                    .unwrap_or((false, None))
            },
            |(ok, name)| Message::Gpu(ok, name),
        ));
    }
    Task::batch(tasks)
}

/// Change the engine's settings and hand them over; what the engine refuses is undone.
fn apply(state: &mut State, ctx: &mut Ctx, f: impl FnOnce(&mut Settings)) {
    let mut s = state.settings.clone();
    f(&mut s);
    state.dragging = false;
    match es::apply(s.clone()) {
        Ok(()) => {
            state.settings = s;
            state.wrote_at = Some(Instant::now());
        }
        Err(e) => {
            ctx.toast_err(format!("Cadenza did not take the setting: {e}"));
            state.settings = es::settings();
            state.bands = state.settings.dsp.eq.iter().map(band_edit).collect();
        }
    }
}

/// The bands the EQ shows: the dragged copy while a handle is dragged, else the engine's.
fn eq_bands(state: &State) -> Vec<Band> {
    match &state.eq_live {
        Some((b, _)) => b.clone(),
        None => state.settings.dsp.eq.clone(),
    }
}

/// A band of the EQ graph was picked, dragged, scrolled on, or a bell added (as on Cadenza's
/// Sound page). While a handle moves the engine gets the bands about 15 times a second.
fn eq_graph_event(state: &mut State, ctx: &mut Ctx, e: GraphEvent) -> Task<Message> {
    match e {
        GraphEvent::Select(i) => state.eq_sel = Some(i),
        GraphEvent::Drag(i, f, g) => {
            let mut b = eq_bands(state);
            if let Some(band) = b.get_mut(i) {
                band.freq = (f * 10.0).round() / 10.0;
                if eq_graph::has_gain(band.kind) {
                    band.gain_db = (g * 10.0).round() / 10.0;
                }
            }
            state.eq_sel = Some(i);
            let send = state.eq_live.as_ref().is_none_or(|(_, t)| t.elapsed() > Duration::from_millis(66));
            if send {
                let s = b.clone();
                apply(state, ctx, move |x| x.dsp.eq = s);
                state.eq_live = Some((b, Instant::now()));
            } else if let Some((live, _)) = state.eq_live.as_mut() {
                *live = b;
            }
            // the 2 s re-read leaves the bands alone until the drag ends
            state.dragging = true;
        }
        GraphEvent::DragEnd => {
            if let Some((b, _)) = state.eq_live.take() {
                apply(state, ctx, move |x| x.dsp.eq = b);
            }
            state.bands = state.settings.dsp.eq.iter().map(band_edit).collect();
        }
        GraphEvent::Wheel(i, dy) => {
            let f = if dy > 0.0 { 1.1 } else { 1.0 / 1.1 };
            state.eq_sel = Some(i);
            apply(state, ctx, |s| {
                if let Some(b) = s.dsp.eq.get_mut(i) {
                    b.q = ((b.q * f).clamp(0.1, 18.0) * 100.0).round() / 100.0;
                }
            });
            state.bands = state.settings.dsp.eq.iter().map(band_edit).collect();
        }
        GraphEvent::Add(f, g) => {
            if state.settings.dsp.eq.len() >= MAX_BANDS {
                ctx.toast_err(format!("Up to {MAX_BANDS} bands"));
                return Task::none();
            }
            let band = Band { kind: BandKind::Peaking, freq: f.round(), gain_db: (g * 10.0).round() / 10.0, q: 1.0, enabled: true, slope_db: 12 };
            apply(state, ctx, |s| s.dsp.eq.push(band));
            state.eq_sel = Some(state.settings.dsp.eq.len() - 1);
            state.bands = state.settings.dsp.eq.iter().map(band_edit).collect();
        }
    }
    Task::none()
}

/// The shared (OS mixer) and the exclusive backend of this build, when it has both.
fn exclusive_switchable() -> bool {
    es::exclusive_backend().is_some() && cadenza::output::backends().contains(&"cpal")
}

/// The device the settings name, in the listing.
fn current_device<'a>(settings: &Settings, devices: &'a [DeviceInfo]) -> Option<&'a DeviceInfo> {
    let (b, d) = cadenza::output::split_backend(&settings.output.backend, &settings.output.device);
    devices
        .iter()
        .find(|x| x.backend == b && x.id == d)
        .or_else(|| devices.iter().find(|x| x.backend == b && d.is_empty() && x.default))
        .or_else(|| devices.iter().find(|x| x.backend == b && d.is_empty()))
}

/// `"<backend>:<device>"`, the id of an output in the device select.
fn device_key(d: &DeviceInfo) -> String {
    format!("{}:{}", d.backend, d.id)
}

/// The words of one device in the select: name plus what it can do.
pub fn device_label(d: &DeviceInfo) -> String {
    let mut notes = Vec::new();
    if let Some(r) = d.rates.iter().max() {
        notes.push(format!("up to {} kHz", rate_label(*r)));
    }
    if d.dsd.any() {
        notes.push(d.dsd.max_rate_label().unwrap_or_else(|| "DSD".into()));
    }
    notes.push(if d.exclusive { "exclusive" } else { "shared" }.into());
    format!("{} · {}", d.name, notes.join(" · "))
}

/// The DSD abilities to clamp the DSD lists with: the open output's, else the listed device's.
fn dsd_caps(st: &Status, dev: Option<&DeviceInfo>) -> DsdCaps {
    if st.output_dsd.any() {
        return st.output_dsd.clone();
    }
    dev.map(|d| d.dsd.clone()).unwrap_or(DsdCaps::NONE)
}

pub fn update(state: &mut State, msg: Message, ctx: &mut Ctx) -> Task<Message> {
    match msg {
        Message::Dsp(Ok(d)) => state.dsp = d,
        Message::Dsp(Err(_)) => state.dsp = json!({ "enabled": false }),
        Message::Devices(list) => state.devices = Some(list),
        Message::Gpu(ok, name) => state.gpu = Some((ok, name)),
        Message::Pref(key, v) => {
            state.prefs[key] = v;
            state.prefs = clean(&state.prefs);
            if ctx.has_feature("theme") {
                let api = ctx.api.clone();
                let body = json!({ "playback": state.prefs });
                return Task::perform(async move { api.put("/auth/prefs", body).await }, Message::PrefSaved);
            }
        }
        Message::PrefSaved(Ok(_)) => ctx.refresh_session(),
        Message::PrefSaved(Err(e)) => ctx.toast_err(e.message),
        Message::Tick => {
            let settling = state.wrote_at.is_some_and(|at| at.elapsed() < Duration::from_secs(3));
            if state.dragging || settling {
                return Task::none();
            }
            if let Some(e) = engine::engine() {
                let live = e.settings();
                if live != state.settings {
                    state.settings = live;
                    state.bands = state.settings.dsp.eq.iter().map(band_edit).collect();
                    state.convolution = state.settings.dsp.convolution.clone();
                }
            }
        }
        Message::EngineOn(on) => apply(state, ctx, |s| {
            s.dsp.bit_perfect = !on;
            if !on {
                s.dsp.output_mode = OutputMode::Pcm;
            }
        }),
        Message::Device(key) => {
            let Some((backend, id)) = key.split_once(':') else { return Task::none() };
            let (backend, id) = (backend.to_string(), id.to_string());
            apply(state, ctx, |s| {
                s.output.backend = backend;
                s.output.device = id;
            });
        }
        Message::Exclusive(on) => {
            let devices = state.devices.as_deref().unwrap_or(&[]);
            let name = current_device(&state.settings, devices).map(|d| d.name.clone()).unwrap_or_default();
            let pick = devices
                .iter()
                .find(|d| d.name == name && d.exclusive == on && d.backend != "null")
                .or_else(|| devices.iter().find(|d| d.exclusive == on && d.default && d.backend != "null"))
                .cloned();
            // the same DAC through the other backend; else that backend's default device
            let fallback = if on { es::exclusive_backend() } else { Some("cpal").filter(|b| cadenza::output::backends().contains(b)) };
            match (pick, fallback) {
                (Some(d), _) => apply(state, ctx, |s| {
                    s.output.backend = d.backend;
                    s.output.device = d.id;
                }),
                (None, Some(b)) => apply(state, ctx, |s| {
                    s.output.backend = b.to_string();
                    s.output.device.clear();
                }),
                (None, None) => ctx.toast_err(if on {
                    "No exclusive output for that device on this computer"
                } else {
                    "No shared output for that device on this computer"
                }),
            }
        }
        Message::Rate(r) => {
            let Some(rate) = RateSetting::parse(&r) else { return Task::none() };
            apply(state, ctx, |s| s.output.rate = rate);
        }
        Message::Bits(b) => {
            if let Ok(bits) = b.parse::<u32>() {
                apply(state, ctx, |s| s.output.bits = bits);
            }
        }
        Message::Link(id) => {
            if let Some((_, _, hz)) = LINKS.iter().find(|(k, _, _)| *k == id) {
                let hz = *hz;
                apply(state, ctx, |s| {
                    s.output.max_rate = hz;
                    // a fixed rate above the new ceiling would never be reached
                    if matches!(s.output.rate, RateSetting::Fixed(r) if r > hz) {
                        s.output.rate = RateSetting::Auto;
                    }
                });
            }
        }
        Message::Mode(m) => {
            let Some(mode) = OutputMode::parse(&m) else { return Task::none() };
            apply(state, ctx, |s| s.dsp.output_mode = mode);
        }
        Message::DsdRate(r) => {
            if let Some(v) = DsdRate::parse(&r) {
                apply(state, ctx, |s| s.dsp.dsd_rate = v);
            }
        }
        Message::DsdModulator(m) => {
            if let Some(v) = DsdModulator::parse(&m) {
                apply(state, ctx, |s| s.dsp.dsd_modulator = v);
            }
        }
        Message::DsdPacking(p) => {
            if let Some(v) = DsdPacking::parse(&p) {
                apply(state, ctx, |s| s.dsp.dsd_packing = v);
            }
        }
        Message::DsdOversampling(o) => {
            if let Some(v) = DsdOversampling::parse(&o) {
                apply(state, ctx, |s| s.dsp.dsd_oversampling = v);
            }
        }
        Message::DsdGain(g) => {
            state.dragging = true;
            state.settings.dsp.dsd_gain_db = g;
        }
        Message::DsdGainDone => apply(state, ctx, |_| {}),
        Message::Family(f) => {
            let Some(family) = FAMILIES.iter().copied().find(|x| x.id() == f) else { return Task::none() };
            let cur = FilterKind::parse(&state.settings.dsp.filter).unwrap_or_default();
            let taps = if family.catalog_taps().contains(&cur.taps) { cur.taps } else { family.catalog_taps()[0] };
            let id = FilterKind::new(family, cur.phase, cur.apodizing, taps).id();
            apply(state, ctx, |s| s.dsp.filter = id);
        }
        Message::Phase(p) => {
            let (phase, apodizing) = phase_of(&p);
            let cur = FilterKind::parse(&state.settings.dsp.filter).unwrap_or_default();
            let id = FilterKind::new(cur.family, phase, apodizing, cur.taps).id();
            apply(state, ctx, |s| s.dsp.filter = id);
        }
        Message::Length(l) => {
            let Ok(taps) = l.parse::<usize>() else { return Task::none() };
            let cur = FilterKind::parse(&state.settings.dsp.filter).unwrap_or_default();
            let id = FilterKind::new(cur.family, cur.phase, cur.apodizing, taps).id();
            apply(state, ctx, |s| s.dsp.filter = id);
        }
        Message::Dither(d) => apply(state, ctx, |s| s.dsp.dither = d),
        Message::HeadroomAuto(auto) => apply(state, ctx, |s| s.dsp.headroom_db = if auto { None } else { Some(-3.0) }),
        Message::Headroom(v) => {
            state.dragging = true;
            state.settings.dsp.headroom_db = Some(v);
        }
        Message::HeadroomDone => apply(state, ctx, |_| {}),
        Message::SoftClip(on) => apply(state, ctx, |s| s.dsp.soft_clip = on),
        Message::Headphones(id) => {
            if let Some(m) = HeadphoneMode::from_id(&id) {
                apply(state, ctx, |s| s.dsp.headphones = m);
            }
        }
        Message::Gpu2(m) => {
            let mode = match m.as_str() {
                "on" => GpuMode::On,
                "off" => GpuMode::Off,
                _ => GpuMode::Auto,
            };
            apply(state, ctx, |s| s.dsp.gpu.mode = mode);
        }
        Message::Matrix(id) => apply(state, ctx, |s| {
            if id == "off" {
                s.matrix.enabled = false;
            } else {
                s.matrix.enabled = true;
                s.matrix.preset = id;
            }
        }),
        Message::EqOn(on) => apply(state, ctx, |s| s.dsp.eq_enabled = on),
        Message::EqAdd => {
            if state.settings.dsp.eq.len() >= MAX_BANDS {
                ctx.toast_err(format!("Up to {MAX_BANDS} bands"));
                return Task::none();
            }
            let band = Band {
                kind: BandKind::Peaking,
                freq: 1000.0,
                gain_db: 0.0,
                q: 1.0,
                enabled: true,
                slope_db: 12,
            };
            state.bands.push(band_edit(&band));
            apply(state, ctx, |s| s.dsp.eq.push(band));
        }
        Message::EqRemove(i) => {
            if i < state.bands.len() {
                state.bands.remove(i);
            }
            state.eq_sel = None;
            apply(state, ctx, |s| {
                if i < s.dsp.eq.len() {
                    s.dsp.eq.remove(i);
                }
            });
        }
        Message::EqKind(i, id) => {
            let Some(kind) = BAND_KINDS.iter().find(|(_, k, _)| *k == id).map(|(k, _, _)| *k) else { return Task::none() };
            apply(state, ctx, |s| {
                if let Some(b) = s.dsp.eq.get_mut(i) {
                    b.kind = kind;
                }
            });
        }
        Message::EqEdit(i, col, text) => {
            if let Some(b) = state.bands.get_mut(i) {
                match col {
                    0 => b.freq = text,
                    1 => b.gain = text,
                    _ => b.q = text,
                }
            }
        }
        Message::EqCommit(i) => {
            let Some(e) = state.bands.get(i).cloned() else { return Task::none() };
            let num = |s: &str, lo: f64, hi: f64, dflt: f64| s.trim().parse::<f64>().ok().filter(|v| v.is_finite()).map(|v| v.clamp(lo, hi)).unwrap_or(dflt);
            apply(state, ctx, |s| {
                if let Some(b) = s.dsp.eq.get_mut(i) {
                    b.freq = num(&e.freq, 20.0, 20_000.0, b.freq);
                    b.gain_db = num(&e.gain, -24.0, 24.0, b.gain_db);
                    b.q = num(&e.q, 0.1, 18.0, b.q);
                }
            });
            state.bands = state.settings.dsp.eq.iter().map(band_edit).collect();
        }
        Message::EqGraph(e) => return eq_graph_event(state, ctx, e),
        Message::EqSelect(i) => state.eq_sel = Some(i),
        Message::EqEnabled(i, on) => apply(state, ctx, |s| {
            if let Some(b) = s.dsp.eq.get_mut(i) {
                b.enabled = on;
            }
        }),
        Message::EqSlope(i, v) => {
            let slope = v.parse().unwrap_or(12);
            apply(state, ctx, |s| {
                if let Some(b) = s.dsp.eq.get_mut(i) {
                    b.slope_db = slope;
                }
            });
        }
        Message::EqRange(v) => state.eq_range = v.parse().unwrap_or(12.0),
        Message::EqFlat => {
            apply(state, ctx, |s| {
                for b in s.dsp.eq.iter_mut().filter(|b| eq_graph::has_gain(b.kind)) {
                    b.gain_db = 0.0;
                }
            });
            state.bands = state.settings.dsp.eq.iter().map(band_edit).collect();
        }
        Message::EqClear => {
            state.eq_sel = None;
            state.bands.clear();
            apply(state, ctx, |s| s.dsp.eq.clear());
        }
        Message::ConvInput(s) => state.convolution = s,
        Message::ConvPick => {
            return Task::perform(
                async move {
                    rfd::AsyncFileDialog::new()
                        .set_title("Impulse response")
                        .add_filter("Impulse responses", &["wav", "txt"])
                        .pick_file()
                        .await
                        .map(|f| f.path().to_path_buf())
                },
                Message::ConvPicked,
            );
        }
        Message::ConvPicked(Some(path)) => {
            state.convolution = path.to_string_lossy().to_string();
            let c = state.convolution.clone();
            apply(state, ctx, |s| s.dsp.convolution = c);
        }
        Message::ConvPicked(None) => {}
        Message::ConvCommit => {
            let c = state.convolution.trim().to_string();
            apply(state, ctx, |s| s.dsp.convolution = c);
        }
        Message::ConvClear => {
            state.convolution.clear();
            apply(state, ctx, |s| s.dsp.convolution.clear());
        }
        Message::TestSound => {
            state.test_busy = true;
            return Task::perform(
                async move { tokio::task::spawn_blocking(es::test_sound).await.unwrap_or_else(|e| Err(e.to_string())) },
                Message::TestDone,
            );
        }
        Message::TestDone(r) => {
            state.test_busy = false;
            match r {
                Ok(()) => ctx.toast_ok("Playing a 2 s test tone"),
                Err(e) => ctx.toast_err(e),
            }
        }
        Message::Reopen => {
            engine::send(cadenza::engine::Command::Reconnect);
            ctx.toast("Opening the output again");
        }
        Message::RefreshDevices => {
            state.devices = None;
            return list_devices();
        }
        Message::SignalPath => ctx.player_msg(crate::player::Message::Signal),
        Message::NormMode(m) => {
            state.norm.mode = norm::Mode::parse(&m);
            store_norm(state, ctx);
        }
        Message::NormTarget(v) => {
            state.norm.target = (v.round() as i32).clamp(norm::MIN_TARGET, norm::MAX_TARGET);
            norm::set_prefs(state.norm);
        }
        Message::NormTargetDone => store_norm(state, ctx),
        Message::NormClip(on) => {
            state.norm.clip = on;
            store_norm(state, ctx);
        }
    }
    Task::none()
}

/// Normalization changed: the song that plays gets its new gain, and the config keeps it.
fn store_norm(state: &State, ctx: &mut Ctx) {
    let p = state.norm;
    norm::set_prefs(p);
    ctx.edit_config(move |c| p.store(c));
}

/// Settings → Playback → Sound (on this device): volume normalization (client-shapes §5.7), the
/// first thing of the web's section. It rides on the engine's volume, which bit-perfect playback
/// leaves alone, so it says so while "Play through Cadenza on this device" is off.
fn sound<'a>(state: &'a State) -> Vec<El<'a, Message>> {
    let n = state.norm;
    let mut lines: Vec<El<'a, Message>> = vec![
        help("Volume normalization").into(),
        seg(&[("off", "Off"), ("track", "Track"), ("album", "Album")], n.mode.id(), Message::NormMode),
        help(n.mode.help()).into(),
        column![
            row![help("Target level"), help(norm::lufs_text(n.target))].spacing(6),
            slider(f64::from(norm::MIN_TARGET)..=f64::from(norm::MAX_TARGET), f64::from(n.target), Message::NormTarget)
                .on_release(Message::NormTargetDone)
                .step(1.0)
                .style(theme::slider_style)
                .width(260),
        ]
        .spacing(4)
        .into(),
        switch(n.clip, "Prevent clipping", Message::NormClip),
        help("Uses the ReplayGain tags in your files, or Encore's own measurement when a file has none. Higher targets are louder; \u{2212}14 LUFS is about what streaming services play at. Prevent clipping never turns a song up further than its loudest peak allows. Only on this device.").into(),
    ];
    if state.settings.dsp.bit_perfect {
        lines.push(help("Normalization is off while this computer plays bit-perfect (Cadenza on this device, above).").into());
    }
    lines
}

fn seg_str<'a>(items: &[(&'a str, &'a str)], current: &str, key: &'static str) -> El<'a, Message> {
    seg(items, current, move |v| Message::Pref(key.to_string(), json!(v)))
}

/// A slider with its label and value on one line.
fn gain_slider<'a>(label: &'a str, value: f64, range: std::ops::RangeInclusive<f64>, text: String, on: fn(f64) -> Message, done: Message) -> El<'a, Message> {
    column![
        row![help(label), help(text)].spacing(6),
        slider(range, value, on).on_release(done).step(0.5).style(theme::slider_style).width(260),
    ]
    .spacing(4)
    .into()
}

/// Settings → Playback → Server processing (Cadenza): `dsp_html` of the web.
fn server_processing<'a>(state: &'a State, ctx: &Ctx<'a>) -> Vec<El<'a, Message>> {
    let p = &state.prefs;
    let dsp = &state.dsp;
    if dsp.is_null() {
        return vec![widgets::skeleton_rows(1)];
    }
    if !fmt::truthy(dsp, "enabled") {
        return vec![help("Turned off on this server. Cadenza upsampling for streams uses the server's processor, so an admin has to switch it on (Settings → Server processing) and allow your account.").into()];
    }
    if !fmt::truthy(dsp, "allowed") {
        return vec![help("Your account isn't allowed to use the server's processing. Ask an admin.").into()];
    }
    let on = fmt::truthy(p, "dsp");
    let dev_rate = ctx.player.status.output_format.map(|f| f.rate).unwrap_or(0);
    let max_rate = fmt::f(dsp, "max_rate");
    let mut rates = vec![
        Opt::new("device", if dev_rate > 0 { format!("Match this device ({} kHz)", rate_label(dev_rate)) } else { "Match this device".to_string() }),
        Opt::new("auto", "Highest the server allows"),
    ];
    rates.extend(DSP_RATES.iter().filter(|r| (**r as f64) <= max_rate).map(|r| Opt::new(r.to_string(), format!("{} kHz", rate_label(*r as u32)))));
    let pairs = |key: &str| -> Vec<Opt> {
        fmt::items(&dsp[key])
            .iter()
            .map(|o| Opt::new(super::vs(&o[0]), super::vs(&o[1])))
            .collect()
    };
    let mut filters = vec![Opt::new("", "Server default")];
    filters.extend(pairs("filters"));
    let mut lines: Vec<El<'a, Message>> = vec![switch(on, "Upsample my streams on the server", |v| Message::Pref("dsp".into(), json!(v)))];
    if on {
        lines.push(widgets::field("Upsampling filter", select(filters, &super::vs(&p["dsp_filter"]), |v| Message::Pref("dsp_filter".into(), json!(v)))));
        lines.push(widgets::field("Dither / noise shaping", select(pairs("dithers"), &super::vs(&p["dsp_dither"]), |v| Message::Pref("dsp_dither".into(), json!(v)))));
        lines.push(widgets::field(
            "Output rate",
            select(rates, &super::vs(&p["dsp_rate"]), |v| {
                let val = if !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()) { json!(v.parse::<i64>().unwrap_or(0)) } else { json!(v) };
                Message::Pref("dsp_rate".into(), val)
            }),
        ));
    }
    lines.push(help("The server's Cadenza upsamples the original file and sends lossless 24-bit FLAC. It uses more data (about 4.6 Mbit/s at 96 kHz, 9.2 Mbit/s at 192 kHz) and the server's processor; Data saver and Auto still step down to a lighter stream when the connection can't keep up.").into());
    lines
}

/// "Output: <device> · <format> · Exclusive | Shared | not opened yet".
pub fn output_line(st: &Status, s: &Settings) -> String {
    let name = if st.output.is_empty() {
        let (b, d) = cadenza::output::split_backend(&s.output.backend, &s.output.device);
        if d.is_empty() { b.to_string() } else { format!("{b} · {d}") }
    } else {
        st.output.clone()
    };
    match st.output_format {
        Some(f) => format!("Output: {name} · {} · {}", f.label(), if st.output_exclusive { "Exclusive" } else { "Shared" }),
        None => format!("Output: {name} · not opened yet"),
    }
}

/// The card of the engine inside this app.
fn cadenza_card<'a>(state: &'a State, ctx: &Ctx<'a>) -> El<'a, Message> {
    let s = &state.settings;
    let st: &Status = &ctx.player.status;
    let on = !s.dsp.bit_perfect;
    let devices = state.devices.as_deref().unwrap_or(&[]);
    let dev = current_device(s, devices);
    let excl = es::exclusive_wanted(s);

    let mut lines: Vec<El<'a, Message>> = vec![
        switch(on, "Play through Cadenza on this device", Message::EngineOn),
        help(if on {
            "Cadenza upsamples, dithers and shapes the noise of every song on this computer before it reaches the DAC, with the settings below."
        } else {
            "Bit-perfect – every song goes to the DAC untouched (no upsampling, dither or volume). Switch it on for the full engine."
        })
        .into(),
    ];

    // output device + exclusive
    let device_opts: Vec<Opt> = devices.iter().map(|d| Opt::new(device_key(d), device_label(d))).collect();
    let selected = dev.map(device_key).unwrap_or_else(|| format!("{}:{}", s.output.backend, s.output.device));
    let device_row: El<'a, Message> = if state.devices.is_none() {
        help("Looking for sound devices…").into()
    } else if device_opts.is_empty() {
        row![
            help("No sound device found – Cadenza plays silently until one is plugged in.").width(Length::Fill),
            icon_btn(Icon::Refresh, "Look again", Some(Message::RefreshDevices)),
        ]
        .spacing(6)
        .align_y(Alignment::Center)
        .into()
    } else {
        row![
            container(select(device_opts, &selected, Message::Device)).width(Length::Fill),
            icon_btn(Icon::Refresh, "Look again", Some(Message::RefreshDevices)),
        ]
        .spacing(6)
        .align_y(Alignment::Center)
        .into()
    };
    lines.push(widgets::field("Output device", device_row));
    let can_toggle = exclusive_switchable()
        || (devices.iter().any(|d| d.exclusive && d.backend != "null") && devices.iter().any(|d| !d.exclusive && d.backend != "null"));
    lines.push(switch_maybe(excl, "Exclusive – Cadenza alone uses the DAC, nothing else is mixed in", can_toggle.then_some(Message::Exclusive)));
    if let Some(d) = dev {
        if !d.dsd.any() && on && s.dsp.output_mode == OutputMode::Dsd {
            lines.push(super::warn_text("This device takes no DSD: songs play as PCM.").into());
        }
    }

    if on {
        // link and rates
        let link = link_of(s.output.max_rate);
        let mut links: Vec<Opt> = LINKS.iter().map(|(k, l, _)| Opt::new(*k, *l)).collect();
        if link == "custom" {
            links.push(Opt::new("custom", format!("Custom ({} kHz)", rate_label(s.output.max_rate))));
        }
        lines.push(widgets::field("Link to the DAC", select(links, link, Message::Link)));
        lines.push(help("The highest rate the cable carries. Rates above it are never asked for.").into());

        let dev_rates: Vec<u32> = if !st.output_rates.is_empty() { st.output_rates.clone() } else { dev.map(|d| d.rates.clone()).unwrap_or_default() };
        let mut rates = vec![Opt::new("auto", "Auto (highest)"), Opt::new("source", "Source rate")];
        let mut listed: Vec<u32> = dev_rates.iter().copied().filter(|r| *r <= s.output.max_rate).collect();
        listed.sort_unstable();
        listed.dedup();
        for r in &listed {
            rates.push(Opt::new(r.to_string(), format!("{} kHz", rate_label(*r))));
        }
        if let RateSetting::Fixed(r) = s.output.rate {
            if !listed.contains(&r) {
                rates.push(Opt::new(r.to_string(), format!("{} kHz", rate_label(r))));
            }
        }
        let bits: Vec<Opt> = [16u32, 24, 32]
            .iter()
            .filter(|b| dev.map(|d| d.bits.is_empty() || d.bits.contains(b)).unwrap_or(true) || **b == s.output.bits)
            .map(|b| Opt::new(b.to_string(), format!("{b}-bit")))
            .collect();
        lines.push(super::two(
            widgets::field("Output rate", select(rates, &s.output.rate.to_string(), Message::Rate)),
            Some(widgets::field("Bit depth", select(bits, &s.output.bits.to_string(), Message::Bits))),
        ));
        if let Some(d) = dev {
            let probe = Probe(d.clone());
            let preferred = match s.output.rate {
                RateSetting::Auto => None,
                RateSetting::Source => Some(0),
                RateSetting::Fixed(r) => Some(r),
            };
            let words = match preferred {
                Some(0) => "Songs play at their own rate.".to_string(),
                pref => {
                    let a = choose_rate(&probe, 44_100, pref, s.output.max_rate, s.output.bits);
                    let b = choose_rate(&probe, 48_000, pref, s.output.max_rate, s.output.bits);
                    format!("A 44.1 kHz song plays at {} kHz, a 48 kHz song at {} kHz.", rate_label(a), rate_label(b))
                }
            };
            lines.push(help(words).into());
        }

        // output mode (PCM / DSD)
        let caps = dsd_caps(st, dev);
        let mode = s.dsp.output_mode.id();
        if excl {
            lines.push(block("Output mode", vec![seg(&[("pcm", "PCM"), ("dsd", "DSD")], mode, Message::Mode)]));
        } else {
            lines.push(block(
                "Output mode",
                vec![row![badge("PCM", Badge::Plain), help("DSD output needs exclusive mode – switch Exclusive on first.")]
                    .spacing(8)
                    .align_y(Alignment::Center)
                    .into()],
            ));
        }
        if s.dsp.output_mode == OutputMode::Dsd && excl {
            let rates: Vec<Opt> = cadenza::encore::state::dsd_rates(&caps)
                .iter()
                .map(|o| Opt::new(super::vs(&o[0]), super::vs(&o[1])))
                .collect();
            let packings: Vec<Opt> = cadenza::encore::state::dsd_packings(&caps)
                .iter()
                .map(|o| Opt::new(super::vs(&o[0]), super::vs(&o[1])))
                .collect();
            let modulators: Vec<Opt> = DsdModulator::catalog().iter().map(|m| Opt::new(m.id(), m.label())).collect();
            lines.push(super::two(
                widgets::field("DSD rate", select(rates, &s.dsp.dsd_rate.id(), Message::DsdRate)),
                Some(widgets::field("DSD modulator", select(modulators, s.dsp.dsd_modulator.id(), Message::DsdModulator))),
            ));
            lines.push(super::two(
                widgets::field("DSD packing", select(packings, s.dsp.dsd_packing.id(), Message::DsdPacking)),
                Some(widgets::field(
                    "DSD oversampling",
                    seg(&[("staged", "Staged"), ("full", "Full")], s.dsp.dsd_oversampling.id(), Message::DsdOversampling),
                )),
            ));
            lines.push(gain_slider("DSD gain", s.dsp.dsd_gain_db, -6.0..=0.0, format!("{:.1} dB", s.dsp.dsd_gain_db), Message::DsdGain, Message::DsdGainDone));
            if let Some(d) = dev {
                let probe = Probe(d.clone());
                match choose_dsd(&probe, 44_100, s.dsp.dsd_rate, s.dsp.dsd_packing, 2) {
                    Some((r, pk)) => lines.push(help(format!("Songs open at {} ({}) on this device.", r.label(), pk.label())).into()),
                    None => lines.push(super::warn_text("This device takes no DSD: songs play as PCM.").into()),
                }
            }
            if let Some(l) = caps.max_rate_label() {
                lines.push(help(format!("The device stops at {l}.")).into());
            }
        } else {
            // the filter: family, phase, length
            let cur = FilterKind::parse(&s.dsp.filter);
            let kind = cur.unwrap_or_default();
            let families: Vec<Opt> = FAMILIES.iter().map(|f| Opt::new(f.id(), format!("{} – {}", f.label(), f.description()))).collect();
            let lengths: Vec<Opt> = kind
                .family
                .catalog_taps()
                .iter()
                .map(|t| Opt::new(t.to_string(), format!("{} taps", FilterKind::new(kind.family, kind.phase, kind.apodizing, *t).length_token())))
                .collect();
            lines.push(widgets::field("Upsampling filter", select(families, kind.family.id(), Message::Family)));
            lines.push(super::two(
                widgets::field("Phase", select_table(&PHASES, phase_id(kind.phase, kind.apodizing), Message::Phase)),
                Some(widgets::field("Length", select(lengths, &kind.taps.to_string(), Message::Length))),
            ));
            if cur.is_none() {
                lines.push(help(format!("The engine uses the designed filter “{}”; picking a family or length replaces it.", s.dsp.filter)).into());
            }
            let dithers: Vec<Opt> = DitherKind::catalog().iter().map(|d| Opt::new(d.id(), d.label())).collect();
            lines.push(widgets::field("Dither / noise shaping", select(dithers, &s.dsp.dither, Message::Dither)));
        }

        // headroom, soft clipper
        let manual = s.dsp.headroom_db.is_some();
        lines.push(switch(!manual, "Automatic headroom – room for the filters, so loud peaks do not clip", Message::HeadroomAuto));
        if let Some(h) = s.dsp.headroom_db {
            lines.push(gain_slider("Headroom", h, -12.0..=0.0, format!("{h:.1} dB"), Message::Headroom, Message::HeadroomDone));
        }
        lines.push(switch(s.dsp.soft_clip, "Soft clipper – round off peaks that would clip instead of cutting them", Message::SoftClip));

        // headphones
        let modes: Vec<Opt> = HeadphoneMode::ALL.iter().map(|(_, id, label)| Opt::new(*id, *label)).collect();
        lines.push(widgets::field("Headphones (crossfeed)", select(modes, s.dsp.headphones.id(), Message::Headphones)));

        // EQ: Cadenza's graph, then one row per band
        let shown = eq_bands(state);
        let mut eq: Vec<El<'a, Message>> = vec![switch(s.dsp.eq_enabled, "Parametric EQ", Message::EqOn)];
        eq.push(
            row![
                btn_sm("Add band", Btn::Default, (s.dsp.eq.len() < MAX_BANDS).then_some(Message::EqAdd)),
                btn_sm("Flat", Btn::Default, Some(Message::EqFlat)),
                btn_sm("Clear", Btn::Default, (!s.dsp.eq.is_empty()).then_some(Message::EqClear)),
                iced::widget::space().width(Length::Fill),
                widgets::small_muted("Range"),
                seg(&[("6", "±6"), ("12", "±12"), ("24", "±24")], &format!("{}", state.eq_range as u32), Message::EqRange),
            ]
            .spacing(8)
            .align_y(Alignment::Center)
            .into(),
        );
        let l = theme::look();
        eq.push(
            container(eq_graph::eq_graph(shown.clone(), s.dsp.eq_enabled, state.eq_range as f64, state.eq_sel, 270.0, Message::EqGraph))
                .style(move |_| container::Style {
                    background: Some(iced::Background::Color(l.bg2)),
                    border: iced::Border { color: l.border, width: 1.0, radius: 12.0.into() },
                    ..Default::default()
                })
                .into(),
        );
        if shown.is_empty() {
            eq.push(help("No bands yet. Press Add band, or double-click the graph.").into());
        }
        for (i, b) in shown.iter().enumerate() {
            let e = state.bands.get(i).cloned().unwrap_or_else(|| band_edit(b));
            let kinds: Vec<Opt> = BAND_KINDS.iter().map(|(_, id, l)| Opt::new(*id, *l)).collect();
            let num = |value: &str, col: u8, label: &'a str| -> El<'a, Message> {
                widgets::field(
                    label,
                    super::input("", value, move |v| Message::EqEdit(i, col, v))
                        .on_submit(Message::EqCommit(i))
                        .width(90),
                )
            };
            let c = eq_graph::color(i);
            let sel = state.eq_sel == Some(i);
            let mut cols = row![
                iced::widget::checkbox(b.enabled).on_toggle(move |v| Message::EqEnabled(i, v)).size(16),
                iced::widget::button(iced::widget::text(format!("{}", i + 1)).font(theme::bold()).size(13).color(c))
                    .padding([2, 6])
                    .style(theme::btn(if sel { Btn::Default } else { Btn::Ghost }))
                    .on_press(Message::EqSelect(i)),
                container(widgets::field("Type", select(kinds, band_kind_id(b.kind), move |k| Message::EqKind(i, k)))).width(150),
                num(&e.freq, 0, "Frequency (Hz)"),
            ]
            .spacing(10)
            .align_y(Alignment::End);
            if eq_graph::has_gain(b.kind) {
                cols = cols.push(num(&e.gain, 1, "Gain (dB)"));
            }
            cols = cols.push(num(&e.q, 2, "Q (width)"));
            if matches!(b.kind, BandKind::LowPass | BandKind::HighPass) {
                let slopes: Vec<Opt> = EQ_SLOPES.iter().map(|v| Opt::new(v.to_string(), format!("{v} dB/oct"))).collect();
                cols = cols.push(container(widgets::field("Slope", select(slopes, &b.slope_db.to_string(), move |v| Message::EqSlope(i, v)))).width(120));
            }
            cols = cols.push(container(icon_btn(Icon::X, "Remove band", Some(Message::EqRemove(i)))).padding([18, 0]));
            eq.push(cols.into());
        }
        eq.push(help("Drag a point on the graph to move it (left/right = frequency, up/down = gain), scroll on it to change the width (Q), double-click the graph to add a bell there. Or type the numbers and press Enter. Up to 16 bands.").into());
        lines.push(block("Equalizer", eq));

        // convolution
        lines.push(block(
            "Convolution",
            vec![
                row![
                    super::input("Impulse response file (WAV or text)", &state.convolution, Message::ConvInput)
                        .on_submit(Message::ConvCommit)
                        .width(Length::Fill),
                    btn_sm("Choose file…", Btn::Default, Some(Message::ConvPick)),
                    btn_sm("Clear", Btn::Ghost, (!state.convolution.is_empty()).then_some(Message::ConvClear)),
                ]
                .spacing(8)
                .align_y(Alignment::Center)
                .into(),
                help("Room correction or a speaker filter, applied after the EQ.").into(),
            ],
        ));

        // GPU
        if cadenza_dsp::gpu::COMPILED_IN {
            let gpu_id = cadenza::encore::state::gpu_mode_id(s.dsp.gpu.mode);
            let in_use = st.signal_path.as_ref().is_some_and(|sp| sp.stages.iter().any(|x| x.id == "gpu" && x.active));
            let line = match &state.gpu {
                Some((true, name)) if in_use => format!("In use: {}", name.clone().unwrap_or_else(|| "NVIDIA GPU".into())),
                Some((true, name)) => format!("{} found, not in use", name.clone().unwrap_or_else(|| "NVIDIA GPU".into())),
                Some((false, _)) => "No NVIDIA GPU found".to_string(),
                None => "Looking for a GPU…".to_string(),
            };
            lines.push(block("GPU", vec![seg(&[("auto", "Auto"), ("on", "On"), ("off", "Off")], gpu_id, Message::Gpu2), help(line).into()]));
        }

        // matrix
        let mut presets = vec![Opt::new("off", "Off")];
        presets.extend(cadenza_dsp::matrix::presets().iter().map(|x| Opt::new(x.id, x.label)));
        let matrix = if s.matrix.enabled { s.matrix.preset.clone() } else { "off".into() };
        lines.push(widgets::field("Matrix (channel mixing)", select(presets, &matrix, Message::Matrix)));
    }

    // status, errors, test sound, signal path
    let mut status_col = column![help(output_line(st, s))].spacing(4);
    if let Some(failed) = es::exclusive_failure(st) {
        status_col = status_col.push(super::err_text(failed));
    } else if let Some(e) = &st.error {
        status_col = status_col.push(super::err_text(e.clone()));
    } else if excl && st.output_format.is_some() && !st.output_exclusive {
        status_col = status_col.push(super::warn_text("Exclusive mode failed: the device opened in shared mode (another program may hold it)."));
    }
    if let Some(sp) = &st.signal_path {
        status_col = status_col.push(help(format!("Signal path: {}", sp.label)));
    }
    lines.push(status_col.into());
    lines.push(
        row![
            btn_sm(if state.test_busy { "Starting the tone…" } else { "Test sound" }, Btn::Default, (!state.test_busy).then_some(Message::TestSound)),
            btn_sm("Signal path", Btn::Default, st.signal_path.as_ref().map(|_| Message::SignalPath)),
            btn_sm("Reopen the output", Btn::Ghost, engine::running().then_some(Message::Reopen)),
        ]
        .spacing(8)
        .into(),
    );
    lines.push(help("Sound effects and song transitions of the web player do not apply here: this computer plays through Cadenza.").into());
    card("Cadenza on this device", lines)
}

pub fn view<'a>(state: &'a State, ctx: &Ctx<'a>) -> El<'a, Message> {
    let p = &state.prefs;
    let saved = if ctx.has_feature("theme") {
        "Saved to your account, so it follows you to every device."
    } else {
        "Saved on this device."
    };
    let quality = fmt::s(p, "quality");
    let quality_help = QUALITY_HELP.iter().find(|(k, _)| *k == quality).map(|(_, h)| *h).unwrap_or("");
    let prefs_card = card(
        "Playback",
        vec![
            help(saved).into(),
            block(
                "Quality",
                vec![
                    seg_str(&[("auto", "Auto"), ("original", "Original"), ("saver", "Data saver")], &quality, "quality"),
                    help(quality_help).into(),
                    help("On this computer Auto plays the original file (Cadenza does not step the stream down); Data saver and server upsampling apply here too.").into(),
                ],
            ),
            block(
                "Buffering",
                vec![
                    switch(fmt::truthy(p, "preload"), "Pre-load the next song while this one plays", |v| Message::Pref("preload".into(), json!(v))),
                    help("The next song is fetched once the current one is safely buffered, so it starts without a gap. Turn it off on a very slow connection to leave all the bandwidth to the song playing.").into(),
                    help("On this computer Cadenza always opens the next song a few seconds before this one ends, whatever this switch says, so songs follow without a gap.").into(),
                ],
            ),
            block(
                "When the queue ends",
                vec![
                    switch(fmt::truthy(p, "autoplay"), "Keep going with a shuffle of the whole library", |v| Message::Pref("autoplay".into(), json!(v))),
                    switch(fmt::truthy(p, "resume"), "Resume where I left off when the app opens", |v| Message::Pref("resume".into(), json!(v))),
                ],
            ),
            block("Sound (on this device)", sound(state)),
            block(
                "Pure playback",
                vec![help("Encore streams the original file untouched – the quality badge on the player opens the Signal path, which shows what the file is, what was sent and what this device does with it. On this computer the sound goes through Cadenza, which can hand it to the DAC bit-perfect in exclusive mode.").into()],
            ),
            block("Server processing (Cadenza)", server_processing(state, ctx)),
        ],
    );
    // every song on this computer goes through Cadenza, so its card comes first
    column![cadenza_card(state, ctx), prefs_card]
        .spacing(16)
        .width(Length::Fill)
        .into()
}

/// The Signal path is the player's dialog; this page has none of its own.
pub fn overlay<'a>(_state: &'a State, _ctx: &Ctx<'a>) -> Option<El<'a, Message>> {
    None
}

pub fn subscription(_state: &State) -> Subscription<Message> {
    iced::time::every(Duration::from_secs(2)).map(|_| Message::Tick)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefs_clean_like_the_web() {
        let c = clean(&json!({ "quality": "nonsense", "preload": false, "dsp_rate": "96000", "dsp": true }));
        assert_eq!(c["quality"], "auto");
        assert_eq!(c["preload"], false);
        assert_eq!(c["autoplay"], true);
        assert_eq!(c["dsp_rate"], 96000);
        assert_eq!(c["dsp"], true);
        assert_eq!(clean(&Value::Null)["dsp_rate"], "device");
        assert_eq!(clean(&json!({ "dsp_rate": "x" }))["dsp_rate"], "device");
    }

    #[test]
    fn links_filters_and_words() {
        assert_eq!(link_of(192_000), "spdif");
        assert_eq!(link_of(MAX_PCM_RATE), "usb");
        assert_eq!(link_of(12_345), "custom");
        assert_eq!(phase_of("ma"), (Phase::Minimum, true));
        assert_eq!(phase_id(Phase::Linear, false), "l");
        let k = FilterKind::new(Family::Sinc, Phase::Minimum, true, 1 << 20);
        assert_eq!(k.id(), "sinc-ma-1m");
        assert_eq!(band_kind_id(BandKind::LowShelf), "low-shelf");
        let mut s = Settings::default();
        s.output.backend = "alsa".into();
        let st = Status::default();
        assert_eq!(output_line(&st, &s), "Output: alsa · not opened yet");
        let d = DeviceInfo {
            backend: "alsa".into(),
            id: "hw:1,0".into(),
            name: "DAC".into(),
            rates: vec![44100, 96000],
            exclusive: true,
            ..DeviceInfo::default()
        };
        assert_eq!(device_label(&d), "DAC · up to 96 kHz · exclusive");
        // an empty device id means the backend's default, else its first device
        let null = DeviceInfo {
            backend: "null".into(),
            id: "null".into(),
            name: "No output (silent)".into(),
            ..DeviceInfo::default()
        };
        let mut n = Settings::default();
        n.output.backend = "null".into();
        n.output.device.clear();
        let list = [d.clone(), null.clone()];
        assert_eq!(current_device(&n, &list).map(device_key).as_deref(), Some("null:null"));
        assert_eq!(device_key(&d), "alsa:hw:1,0");
    }
}
