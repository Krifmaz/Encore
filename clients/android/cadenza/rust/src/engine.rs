//! The effects graph of the web player (`web/js/player.js` applyFx) on cadenza-dsp, plus an optional Cadenza
//! upsampler, as one object the JNI layer wraps:
//!
//! ```text
//! float PCM → preamp → EQ (cadenza Equalizer) → crossfeed → mono → balance → [Cadenza resampler + dither] → float PCM
//! ```
//!
//! Preamp and EQ are cadenza-dsp's `Volume` and `Equalizer` (the same code as the server). Crossfeed, balance and
//! mono are not in cadenza-dsp at the pinned rev, so they live here and copy the web graph exactly: each channel
//! gets a 700 Hz low-passed (Q 0.5 dB), 0.3 ms delayed copy of the other at −9 / −6 / −4.5 dB, trimmed by
//! 1 / (1 + gain); mono averages both channels; balance is Web Audio's equal-power StereoPannerNode.
//!
//! The parameters are the JSON document `PlaybackPrefs.engineParams()` makes on the phone:
//! `{"fx_on", "preamp_db", "bands": [{"type","freq","gain","q"}], "crossfeed", "balance", "mono", …}`.

use cadenza_dsp::{
    Band, BandKind, CoefficientCache, DitherKind, FilterKind, InputFormat, Pipeline, PipelineConfig,
    SignalPath, Volume,
};
use cadenza_dsp::eq::Equalizer;
use serde::Deserialize;
use std::f64::consts::PI;

/// How a band arrives from the phone (the web player's `{type, freq, gain, q}`).
#[derive(Debug, Clone, Deserialize)]
pub struct BandIn {
    #[serde(default = "default_type")]
    r#type: String,
    #[serde(default = "default_freq")]
    freq: f64,
    #[serde(default)]
    gain: f64,
    #[serde(default = "default_q")]
    q: f64,
}

fn default_type() -> String {
    "peaking".into()
}
fn default_freq() -> f64 {
    1000.0
}
fn default_q() -> f64 {
    1.0
}

/// The parameters the engine understands; anything else in the document (`dsp_filter`…) is ignored here.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct Params {
    pub fx_on: bool,
    pub preamp_db: f64,
    pub bands: Vec<BandIn>,
    pub crossfeed: String,
    pub balance: f64,
    pub mono: bool,
}

impl Params {
    /// Parse the JSON document; a broken document means "effects off".
    pub fn parse(json: &str) -> Params {
        serde_json::from_str::<Params>(json).unwrap_or_default()
    }

    /// Crossfeed gain in dB from the web's `XFEED_DB` table (`None` = off).
    fn crossfeed_db(&self) -> Option<f64> {
        if !self.fx_on || self.mono {
            return None;
        }
        match self.crossfeed.as_str() {
            "light" => Some(-9.0),
            "medium" => Some(-6.0),
            "strong" => Some(-4.5),
            _ => None,
        }
    }

    fn balance(&self) -> i32 {
        if !self.fx_on {
            return 0;
        }
        let b = if self.balance.is_finite() { self.balance } else { 0.0 };
        (b.round() as i32).clamp(-100, 100)
    }
}

/// A web-player band as a cadenza band. Web Audio's shelves ignore Q (slope 1, i.e. Q = 1/√2) and its
/// low-/high-pass Q is in dB; peaking and notch use Q as it is.
fn band_of(b: &BandIn) -> Option<Band> {
    let (kind, q) = match b.r#type.as_str() {
        "peaking" => (BandKind::Peaking, b.q),
        "lowshelf" => (BandKind::LowShelf, std::f64::consts::FRAC_1_SQRT_2),
        "highshelf" => (BandKind::HighShelf, std::f64::consts::FRAC_1_SQRT_2),
        "lowpass" => (BandKind::LowPass, 10f64.powf(b.q / 20.0)),
        "highpass" => (BandKind::HighPass, 10f64.powf(b.q / 20.0)),
        "notch" => (BandKind::Notch, b.q),
        _ => return None,
    };
    if !b.freq.is_finite() || !b.gain.is_finite() || !q.is_finite() {
        return None;
    }
    Some(Band {
        kind,
        freq: b.freq,
        gain_db: b.gain,
        q: q.max(0.0001),
        enabled: true,
    })
}

/// Headphone crossfeed, stereo only.
struct Crossfeed {
    lp: Equalizer,
    delay: [Vec<f64>; 2],
    pos: usize,
    gain: f64,
    trim: f64,
    pair: [f64; 2],
}

impl Crossfeed {
    fn new(rate: u32, db: f64) -> Self {
        let lp = Equalizer::new(
            rate,
            2,
            &[Band {
                kind: BandKind::LowPass,
                freq: 700.0,
                gain_db: 0.0,
                q: 10f64.powf(0.5 / 20.0),
                enabled: true,
            }],
        );
        let n = ((0.0003 * rate as f64).round() as usize).max(1);
        let gain = 10f64.powf(db / 20.0);
        Self {
            lp,
            delay: [vec![0.0; n], vec![0.0; n]],
            pos: 0,
            gain,
            trim: 1.0 / (1.0 + gain),
            pair: [0.0; 2],
        }
    }

    fn reset(&mut self) {
        self.lp.reset();
        self.delay[0].iter_mut().for_each(|v| *v = 0.0);
        self.delay[1].iter_mut().for_each(|v| *v = 0.0);
        self.pos = 0;
    }

    fn process(&mut self, frames: &mut [f64]) {
        let n = self.delay[0].len();
        for f in frames.chunks_exact_mut(2) {
            self.pair[0] = f[0];
            self.pair[1] = f[1];
            self.lp.process(&mut self.pair);
            let old_l = self.delay[0][self.pos];
            let old_r = self.delay[1][self.pos];
            self.delay[0][self.pos] = self.pair[0];
            self.delay[1][self.pos] = self.pair[1];
            self.pos = (self.pos + 1) % n;
            f[0] = (f[0] + old_r * self.gain) * self.trim;
            f[1] = (f[1] + old_l * self.gain) * self.trim;
        }
    }
}

/// The bit depth Cadenza's quantiser rounds to when the engine upsamples.
pub const OUTPUT_BITS: u32 = 24;

/// Full scale of the resampler's integers (`2^(OUTPUT_BITS-1)`): its output divided by this is in [-1, 1).
const OUTPUT_SCALE: f32 = (1u32 << (OUTPUT_BITS - 1)) as f32;

/// One engine for one stream format.
pub struct Engine {
    rate: u32,
    channels: usize,
    out_rate: u32,
    filter: FilterKind,
    dither: DitherKind,
    params_json: String,
    params: Params,
    preamp: Volume,
    eq: Option<Equalizer>,
    crossfeed: Option<Crossfeed>,
    mono: bool,
    balance: i32,
    resampler: Option<Pipeline>,
    scratch: Vec<f64>,
    out: Vec<f32>,
}

impl Engine {
    /// `out_rate` 0 (or the input rate) means no resampling; `cache_dir` "" means in-memory filter designs only.
    pub fn new(
        rate: u32,
        channels: u32,
        filter_id: &str,
        dither_id: &str,
        out_rate: u32,
        cache_dir: &str,
    ) -> Result<Engine, String> {
        if rate == 0 || channels == 0 || channels > 8 {
            return Err(format!("unsupported format: {channels} channels at {rate} Hz"));
        }
        let filter = FilterKind::parse(filter_id).unwrap_or_default();
        let dither = DitherKind::parse(dither_id).unwrap_or_default();
        let want = if out_rate == 0 { rate } else { out_rate };
        let resampler = if want != rate {
            let cache = if cache_dir.is_empty() {
                CoefficientCache::disabled()
            } else {
                CoefficientCache::in_dir(cache_dir)
            };
            let input = InputFormat {
                rate,
                bits: 32,
                channels,
                codec: "PCM".into(),
            };
            let cfg = PipelineConfig {
                filter,
                output_rate: Some(want),
                // Cadenza's dither / noise shaping quantise to 24 bits (what a USB DAC or Android's 32-bit
                // path takes); the floats handed back are those 24-bit values / 2^23, so the phone only scales
                output_bits: OUTPUT_BITS,
                dither,
                eq: Vec::new(),
                eq_enabled: false,
                // the player owns the volume: no automatic -3 dB headroom (the soft clipper still guards overs)
                headroom_db: Some(0.0),
                ..Default::default()
            };
            Some(Pipeline::new(input, cfg, &cache).map_err(|e| e.to_string())?)
        } else {
            None
        };
        Ok(Engine {
            rate,
            channels: channels as usize,
            out_rate: want,
            filter,
            dither,
            params_json: String::new(),
            params: Params::default(),
            preamp: Volume::UNITY,
            eq: None,
            crossfeed: None,
            mono: false,
            balance: 0,
            resampler,
            scratch: Vec::new(),
            out: Vec::new(),
        })
    }

    pub fn output_rate(&self) -> u32 {
        self.out_rate
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Apply a parameter document (no-op when it is the one already applied).
    pub fn set_params(&mut self, json: &str) {
        if json == self.params_json {
            return;
        }
        self.params_json = json.to_string();
        self.params = Params::parse(json);
        self.rebuild();
    }

    fn rebuild(&mut self) {
        let p = &self.params;
        self.preamp = if p.fx_on && p.preamp_db.is_finite() && p.preamp_db != 0.0 {
            Volume {
                gain_db: p.preamp_db,
                muted: false,
            }
        } else {
            Volume::UNITY
        };
        let bands: Vec<Band> = if p.fx_on { p.bands.iter().filter_map(band_of).collect() } else { Vec::new() };
        self.eq = if bands.is_empty() {
            None
        } else {
            Some(Equalizer::new(self.rate, self.channels, &bands)).filter(|e| !e.is_identity())
        };
        self.crossfeed = match (self.channels, p.crossfeed_db()) {
            (2, Some(db)) => Some(Crossfeed::new(self.rate, db)),
            _ => None,
        };
        self.mono = p.fx_on && p.mono && self.channels == 2;
        self.balance = if self.channels == 2 { p.balance() } else { 0 };
    }

    /// `true` when nothing is applied (the processor may pass the bytes through).
    pub fn is_identity(&self) -> bool {
        self.preamp.is_unity()
            && self.eq.is_none()
            && self.crossfeed.is_none()
            && !self.mono
            && self.balance == 0
            && self.resampler.is_none()
    }

    /// Process `frames` interleaved frames (`input.len() >= frames * channels`); the result is interleaved at
    /// the output rate and may be empty while the resampler gathers a block.
    pub fn process(&mut self, input: &[f32], frames: usize) -> &[f32] {
        let n = (frames * self.channels).min(input.len());
        self.scratch.clear();
        self.scratch.extend(input[..n].iter().map(|v| *v as f64));
        self.run_effects();
        self.finish(false)
    }

    /// Whatever is still inside (the resampler's tail) at the end of a stream; resets every filter.
    pub fn flush(&mut self) -> &[f32] {
        self.scratch.clear();
        let out_len = {
            let r = match self.resampler.as_mut() {
                Some(r) => r.flush(),
                None => Vec::new(),
            };
            self.out.clear();
            self.out.extend(r.iter().map(|v| *v as f32 / OUTPUT_SCALE));
            self.out.len()
        };
        if let Some(eq) = self.eq.as_mut() {
            eq.reset();
        }
        if let Some(x) = self.crossfeed.as_mut() {
            x.reset();
        }
        &self.out[..out_len]
    }

    fn run_effects(&mut self) {
        self.preamp.apply(&mut self.scratch);
        if let Some(eq) = self.eq.as_mut() {
            eq.process(&mut self.scratch);
        }
        if let Some(x) = self.crossfeed.as_mut() {
            x.process(&mut self.scratch);
        }
        if self.mono {
            for f in self.scratch.chunks_exact_mut(2) {
                let m = (f[0] + f[1]) * 0.5;
                f[0] = m;
                f[1] = m;
            }
        }
        if self.balance != 0 {
            // Web Audio StereoPannerNode, stereo input (equal power)
            let pan = self.balance as f64 / 100.0;
            let x = if pan <= 0.0 { pan + 1.0 } else { pan };
            let gl = (x * PI / 2.0).cos();
            let gr = (x * PI / 2.0).sin();
            for f in self.scratch.chunks_exact_mut(2) {
                let (l, r) = (f[0], f[1]);
                if pan <= 0.0 {
                    f[0] = l + r * gl;
                    f[1] = r * gr;
                } else {
                    f[0] = l * gl;
                    f[1] = r + l * gr;
                }
            }
        }
    }

    fn finish(&mut self, _flush: bool) -> &[f32] {
        self.out.clear();
        match self.resampler.as_mut() {
            Some(r) => {
                let ints = r.process(&self.scratch);
                self.out.extend(ints.iter().map(|v| *v as f32 / OUTPUT_SCALE));
            }
            None => self.out.extend(self.scratch.iter().map(|v| *v as f32)),
        }
        &self.out
    }

    /// The resampler's latency in seconds (0 without one).
    pub fn latency_seconds(&self) -> f64 {
        self.resampler.as_ref().map(Pipeline::latency_seconds).unwrap_or(0.0)
    }

    /// One line for the Signal path sheet, in the web's effectsSummary() wording:
    /// "Cadenza on this phone · EQ: 3 bands, preamp -6 dB · Crossfeed: medium · Balance: L10 · Mono".
    pub fn summary(&self) -> String {
        let mut parts = vec!["Cadenza on this phone".to_string()];
        let p = &self.params;
        if p.fx_on {
            let n = self.eq.as_ref().map(Equalizer::active_stages).unwrap_or(0);
            if n > 0 || !self.preamp.is_unity() {
                let mut s = format!("EQ: {n} band{}", if n == 1 { "" } else { "s" });
                if !self.preamp.is_unity() {
                    s.push_str(&format!(", preamp {}", db_text(self.preamp.gain_db)));
                }
                parts.push(s);
            }
            if self.crossfeed.is_some() {
                parts.push(format!("Crossfeed: {}", p.crossfeed));
            }
            if self.balance != 0 {
                parts.push(format!("Balance: {}", balance_text(self.balance)));
            }
            if self.mono {
                parts.push("Mono".into());
            }
        }
        if let Some(r) = &self.resampler {
            parts.push(format!(
                "{} → {} kHz",
                self.filter.label(),
                cadenza_dsp::signal_path::rate_label(r.output_rate())
            ));
        }
        if parts.len() == 1 {
            parts.push("effects off".into());
        }
        parts.join(" · ")
    }

    /// The whole path as JSON: `{summary, engine, fx_on, input_rate, output_rate, channels, latency_seconds,
    /// preamp_db, eq_bands, crossfeed, balance, mono, identity, pipeline: cadenza SignalPath | null}`.
    pub fn signal_path_json(&self) -> String {
        let pipeline: Option<SignalPath> = self.resampler.as_ref().map(Pipeline::signal_path);
        let v = serde_json::json!({
            "engine": "cadenza",
            "summary": self.summary(),
            "fx_on": self.params.fx_on,
            "input_rate": self.rate,
            "output_rate": self.out_rate,
            "channels": self.channels,
            "latency_seconds": self.latency_seconds(),
            "preamp_db": self.preamp.gain_db,
            "eq_bands": self.eq.as_ref().map(Equalizer::active_stages).unwrap_or(0),
            "crossfeed": if self.crossfeed.is_some() { self.params.crossfeed.as_str() } else { "off" },
            "balance": self.balance,
            "mono": self.mono,
            "identity": self.is_identity(),
            "filter": self.filter.id(),
            "dither": self.dither.id(),
            "pipeline": pipeline,
        });
        v.to_string()
    }
}

fn db_text(v: f64) -> String {
    let s = if (v - v.round()).abs() < 1e-9 { format!("{}", v.round() as i64) } else { format!("{v}") };
    format!("{}{} dB", if v > 0.0 { "+" } else { "" }, s)
}

fn balance_text(v: i32) -> String {
    if v == 0 {
        "Centre".into()
    } else if v < 0 {
        format!("L{}", -v)
    } else {
        format!("R{v}")
    }
}

/// What the engine offers, like the server's GET /api/playback/dsp: `{filters: [[id,label]…], dithers: [[id,label]…],
/// default_filter, default_dither}`.
pub fn catalog_json() -> String {
    let filters: Vec<serde_json::Value> =
        FilterKind::catalog().iter().map(|f| serde_json::json!([f.id(), f.label()])).collect();
    let dithers: Vec<serde_json::Value> =
        DitherKind::catalog().iter().map(|d| serde_json::json!([d.id(), d.label()])).collect();
    serde_json::json!({
        "filters": filters,
        "dithers": dithers,
        "default_filter": FilterKind::default().id(),
        "default_dither": DitherKind::default().id(),
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stereo(n: usize, f: impl Fn(usize) -> (f32, f32)) -> Vec<f32> {
        (0..n).flat_map(|i| { let (l, r) = f(i); [l, r] }).collect()
    }

    #[test]
    fn effects_off_is_a_pass_through() {
        let mut e = Engine::new(44100, 2, "", "tpdf", 0, "").unwrap();
        assert_eq!(e.output_rate(), 44100);
        e.set_params(r#"{"fx_on":false,"preamp_db":-6,"bands":[{"type":"peaking","freq":1000,"gain":6,"q":1}],"crossfeed":"medium","balance":50,"mono":true}"#);
        assert!(e.is_identity());
        let input = stereo(100, |i| (i as f32 / 100.0, -(i as f32) / 100.0));
        let out = e.process(&input, 100).to_vec();
        assert_eq!(out, input);
        assert!(e.flush().is_empty());
        assert_eq!(e.summary(), "Cadenza on this phone · effects off");
    }

    #[test]
    fn preamp_scales_by_db() {
        let mut e = Engine::new(48000, 2, "", "", 0, "").unwrap();
        e.set_params(r#"{"fx_on":true,"preamp_db":-6,"bands":[]}"#);
        assert!(!e.is_identity());
        let input = stereo(10, |_| (0.5, -0.5));
        let out = e.process(&input, 10).to_vec();
        let g = 10f32.powf(-6.0 / 20.0);
        for f in out.chunks(2) {
            assert!((f[0] - 0.5 * g).abs() < 1e-6);
            assert!((f[1] + 0.5 * g).abs() < 1e-6);
        }
        assert_eq!(e.summary(), "Cadenza on this phone · EQ: 0 bands, preamp -6 dB");
    }

    #[test]
    fn eq_bands_change_the_signal_and_count() {
        let mut e = Engine::new(44100, 2, "", "", 0, "").unwrap();
        e.set_params(r#"{"fx_on":true,"preamp_db":0,"bands":[{"type":"lowshelf","freq":105,"gain":6,"q":0.7},{"type":"peaking","freq":60,"gain":2,"q":1},{"type":"bogus","freq":1,"gain":1,"q":1}]}"#);
        let n = 44100;
        let input = stereo(n, |i| {
            let v = (2.0 * std::f32::consts::PI * 50.0 * i as f32 / 44100.0).sin() * 0.25;
            (v, v)
        });
        let out = e.process(&input, n).to_vec();
        assert_eq!(out.len(), input.len());
        let rms = |s: &[f32]| (s.iter().step_by(2).skip(n / 2).map(|v| v * v).sum::<f32>() / (n / 2) as f32).sqrt();
        let gain_db = 20.0 * (rms(&out) / rms(&input)).log10();
        assert!(gain_db > 6.5 && gain_db < 9.0, "50 Hz gain {gain_db} dB");
        assert!(e.summary().starts_with("Cadenza on this phone · EQ: 2 bands"));
        let j: serde_json::Value = serde_json::from_str(&e.signal_path_json()).unwrap();
        assert_eq!(j["eq_bands"], 2);
        assert_eq!(j["engine"], "cadenza");
        assert!(j["pipeline"].is_null());
    }

    #[test]
    fn crossfeed_bleeds_a_quieter_copy_into_the_other_channel() {
        let mut e = Engine::new(48000, 2, "", "", 0, "").unwrap();
        e.set_params(r#"{"fx_on":true,"preamp_db":0,"bands":[],"crossfeed":"medium","balance":0,"mono":false}"#);
        // left only, 100 Hz (well below the 700 Hz low-pass)
        let n = 48000;
        let input = stereo(n, |i| ((2.0 * std::f32::consts::PI * 100.0 * i as f32 / 48000.0).sin() * 0.5, 0.0));
        let out = e.process(&input, n).to_vec();
        let rms = |s: &[f32], ch: usize| (s.iter().skip(ch).step_by(2).skip(n / 2).map(|v| v * v).sum::<f32>() / (n / 2) as f32).sqrt();
        let l = rms(&out, 0);
        let r = rms(&out, 1);
        assert!(r > 0.0 && r < l, "right {r} should be a quieter copy of left {l}");
        let db = 20.0 * (r / l).log10();
        assert!(db > -8.0 && db < -4.0, "bleed {db} dB");
        assert_eq!(e.summary(), "Cadenza on this phone · Crossfeed: medium");
        // mono wins over crossfeed, like the web
        e.set_params(r#"{"fx_on":true,"preamp_db":0,"bands":[],"crossfeed":"medium","balance":0,"mono":true}"#);
        let out = e.process(&stereo(4, |_| (0.4, 0.0)), 4).to_vec();
        for f in out.chunks(2) {
            assert!((f[0] - 0.2).abs() < 1e-6 && (f[1] - 0.2).abs() < 1e-6);
        }
        assert_eq!(e.summary(), "Cadenza on this phone · Mono");
    }

    #[test]
    fn balance_is_equal_power_like_web_audio() {
        let mut e = Engine::new(48000, 2, "", "", 0, "").unwrap();
        e.set_params(r#"{"fx_on":true,"preamp_db":0,"bands":[],"crossfeed":"off","balance":-100,"mono":false}"#);
        let out = e.process(&stereo(2, |_| (0.5, 0.5)), 2).to_vec();
        assert!((out[0] - 1.0).abs() < 1e-6, "hard left sums both into L: {}", out[0]);
        assert!(out[1].abs() < 1e-6);
        assert_eq!(e.summary(), "Cadenza on this phone · Balance: L100");
        e.set_params(r#"{"fx_on":true,"preamp_db":0,"bands":[],"crossfeed":"off","balance":10,"mono":false}"#);
        assert_eq!(e.summary(), "Cadenza on this phone · Balance: R10");
    }

    #[test]
    fn upsampling_uses_the_cadenza_pipeline() {
        let mut e = Engine::new(44100, 2, "sinc-l-4k", "tpdf", 88200, "").unwrap();
        assert_eq!(e.output_rate(), 88200);
        assert!(!e.is_identity());
        let n = 4410;
        let input = stereo(n, |i| {
            let v = (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / 44100.0).sin() * 0.5;
            (v, -v)
        });
        let mut out = Vec::new();
        for _ in 0..10 {
            out.extend_from_slice(e.process(&input, n));
        }
        out.extend_from_slice(e.flush());
        assert_eq!(out.len(), n * 10 * 2 * 2);
        assert!(out.iter().all(|v| v.abs() <= 1.0));
        let mid = &out[out.len() / 4..out.len() / 2];
        let peak = mid.iter().fold(0f32, |m, v| m.max(v.abs()));
        assert!((peak - 0.5).abs() < 0.02, "peak {peak}");
        let j: serde_json::Value = serde_json::from_str(&e.signal_path_json()).unwrap();
        assert_eq!(j["output_rate"], 88200);
        assert!(j["pipeline"]["stages"].as_array().unwrap().iter().any(|s| s["id"] == "resampler" && s["active"] == true));
        assert!(j["latency_seconds"].as_f64().unwrap() > 0.0);
        assert!(e.summary().contains("→ 88.2 kHz"));
        // dithered to 24 bits: every sample is an exact multiple of 2^-23 (the phone scales it to an int as it is)
        assert!(mid.iter().all(|v| {
            let i = *v * OUTPUT_SCALE;
            i == i.round() && i.abs() <= OUTPUT_SCALE
        }));
    }

    #[test]
    fn bad_input_is_refused_or_ignored() {
        assert!(Engine::new(0, 2, "", "", 0, "").is_err());
        assert!(Engine::new(44100, 0, "", "", 0, "").is_err());
        let mut e = Engine::new(44100, 2, "not-a-filter", "nope", 0, "").unwrap();
        e.set_params("{broken");
        assert!(e.is_identity());
        // frames beyond the buffer are clipped to what is there
        let out = e.process(&[0.1, 0.2], 10).to_vec();
        assert_eq!(out, vec![0.1, 0.2]);
    }

    #[test]
    fn catalog_lists_filters_and_dithers() {
        let j: serde_json::Value = serde_json::from_str(&catalog_json()).unwrap();
        assert!(j["filters"].as_array().unwrap().len() >= 8);
        assert_eq!(j["dithers"][2][0], "tpdf");
        assert_eq!(j["default_dither"], "tpdf");
        assert_eq!(j["default_filter"], "sinc-l-64k");
    }
}
