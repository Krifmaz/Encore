//! Port of web/js/playback.js – see docs/ui-rewrite/CONTRACT.md
//!
//! Playback preferences (Settings → Playback): stream quality, pre-loading the next song, what happens
//! when the queue ends, sound effects on this device and crossfade. Kept on this device, and – on servers
//! that store preferences – on the account too, the same way appearance.rs does. The player reads them through current().

use crate::dom::{self, ls_get, ls_set};
use crate::json::J;
use serde_json::{json, Value};

/// quality: 'auto'     – the original file; Encore measures the connection while it plays and steps the stream down
///                       (256 → 160 → 96 → 64 → 32 kbps) as far as it needs to keep playing, then back up
///          'original' – always the original file, whatever the connection does
///          'saver'    – always the small 96 kbps stream (mobile data, slow links)
/// preload: buffer the next song while this one plays (off on very slow connections)
/// autoplay: when the queue ends, keep going with a shuffle of the whole library
/// resume:  when the app opens, pick up where you left off (this device's queue and spot, or another device's)
/// dsp:     have the server upsample streams with Cadenza (only where an admin switched it on and allowed the account)
/// dsp_filter: Cadenza upsampling filter id ('' = the server's default)
/// dsp_dither: Cadenza dither / noise shaping id
/// dsp_rate: 'device' (this device's output rate), 'auto' (highest the server allows) or a rate in Hz
/// fx_on:   sound effects on this device (Web Audio): the master switch for everything below except crossfade
/// eq_preset / eq_bands / eq_preamp: parametric EQ (a named preset, or 'custom' with your own bands), preamp in dB
/// crossfeed: headphone crossfeed 'off' | 'light' | 'medium' | 'strong'
/// balance: -100 (left) … +100 (right) · mono: sum both channels
/// crossfade: seconds the end of one song blends into the next (0 = off) · crossfade_album_gapless: no fade within an album
pub fn defaults() -> Value {
    json!({ "quality": "auto", "preload": true, "autoplay": true, "resume": true, "dsp": false, "dsp_filter": "", "dsp_dither": "tpdf", "dsp_rate": "device",
        "fx_on": false, "eq_preset": "flat", "eq_bands": [], "eq_preamp": 0, "crossfeed": "off", "balance": 0, "mono": false,
        "crossfade": 0, "crossfade_album_gapless": true })
}
const KEY: &str = "encore.playback";
const QUALITIES: [&str; 3] = ["auto", "original", "saver"];

// ---- sound effects -----------------------------------------------------------------------------
pub const EQ_TYPES: [(&str, &str); 6] = [("peaking", "Peaking"), ("lowshelf", "Low shelf"), ("highshelf", "High shelf"), ("lowpass", "Low-pass"), ("highpass", "High-pass"), ("notch", "Notch")];
/// One EQ band: type, frequency (Hz), gain (dB), Q.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Band { pub ty: &'static str, pub freq: f64, pub gain: f64, pub q: f64 }
const fn b(ty: &'static str, freq: f64, gain: f64, q: f64) -> Band { Band { ty, freq, gain, q } }
/// EQ presets: id, label, preamp (dB, keeps boosts from clipping) and bands.
pub struct Preset { pub id: &'static str, pub label: &'static str, pub preamp: f64, pub bands: &'static [Band] }
pub const EQ_PRESETS: [Preset; 8] = [
    Preset { id: "flat", label: "Flat", preamp: 0.0, bands: &[] },
    Preset { id: "bass", label: "Bass boost", preamp: -6.0, bands: &[b("lowshelf", 105.0, 6.0, 0.7), b("peaking", 60.0, 2.0, 1.0)] },
    Preset { id: "treble", label: "Treble boost", preamp: -5.0, bands: &[b("highshelf", 6000.0, 5.0, 0.7)] },
    Preset { id: "vocal", label: "Vocal", preamp: -3.0, bands: &[b("highpass", 80.0, 0.0, 0.7), b("peaking", 250.0, -2.0, 1.0), b("peaking", 3000.0, 3.0, 1.0), b("peaking", 5000.0, 2.0, 1.2)] },
    Preset { id: "loudness", label: "Loudness (V-shape)", preamp: -5.0, bands: &[b("lowshelf", 90.0, 5.0, 0.7), b("peaking", 1000.0, -3.0, 0.7), b("highshelf", 10000.0, 4.0, 0.7)] },
    Preset { id: "classical", label: "Classical", preamp: -2.0, bands: &[b("lowshelf", 80.0, 2.0, 0.7), b("peaking", 500.0, -1.0, 0.8), b("highshelf", 12000.0, 2.0, 0.7)] },
    Preset { id: "electronic", label: "Electronic", preamp: -5.0, bands: &[b("peaking", 60.0, 5.0, 1.0), b("peaking", 400.0, -2.0, 1.0), b("peaking", 2500.0, 1.0, 1.0), b("highshelf", 10000.0, 4.0, 0.7)] },
    Preset { id: "podcast", label: "Podcast / voice", preamp: -4.0, bands: &[b("highpass", 90.0, 0.0, 0.7), b("peaking", 300.0, -3.0, 1.0), b("peaking", 3000.0, 4.0, 1.0), b("lowpass", 12000.0, 0.0, 0.7)] },
];
pub const CROSSFEED: [(&str, &str); 4] = [("off", "Off"), ("light", "Light"), ("medium", "Medium"), ("strong", "Strong")];

/// A JSON number the way JS would print it (whole numbers without ".0").
pub fn jnum(x: f64) -> Value {
    if x.is_finite() && x.fract() == 0.0 && x.abs() < 1e15 { json!(x as i64) } else { json!(x) }
}
/// `Math.round(x)` (halves round up, also for negatives).
pub fn js_round(x: f64) -> f64 { (x + 0.5).floor() }
/// `Number(x.toFixed(digits))`.
fn round_to(x: f64, digits: i32) -> f64 { let m = 10f64.powi(digits); (x * m).round() / m }
/// `num(v, lo, hi, dflt)`: Number(v) clamped, the default when it isn't a finite number.
fn clamp_num(v: &Value, lo: f64, hi: f64, dflt: f64) -> f64 {
    let n = js_number(v);
    if !n.is_finite() { return dflt; }
    n.max(lo).min(hi)
}
fn band_json(b: &Band) -> Value { json!({ "type": b.ty, "freq": jnum(b.freq), "gain": jnum(b.gain), "q": jnum(b.q) }) }
pub fn preset_bands_json(p: &Preset) -> Value { Value::Array(p.bands.iter().map(band_json).collect()) }
fn clean_band(x: &Value) -> Value {
    let ty = x.g("type").as_str().filter(|t| EQ_TYPES.iter().any(|(v, _)| v == t)).unwrap_or("peaking");
    json!({
        "type": ty,
        "freq": jnum(js_round(clamp_num(x.g("freq"), 20.0, 20000.0, 1000.0))),
        "gain": jnum(round_to(clamp_num(x.g("gain"), -24.0, 24.0, 0.0), 1)),
        "q": jnum(round_to(clamp_num(x.g("q"), 0.1, 18.0, 1.0), 2)),
    })
}
/// The bands that play: a named preset's own, or your custom ones (as JSON band objects).
pub fn eq_bands(p: &Value) -> Vec<Value> {
    if p.s("eq_preset") == "custom" { return p.a("eq_bands"); }
    let id = p.s("eq_preset");
    let pr = EQ_PRESETS.iter().find(|x| x.id == id).unwrap_or(&EQ_PRESETS[0]);
    pr.bands.iter().map(band_json).collect()
}
pub fn preset_label(id: &str) -> &'static str {
    if id == "custom" { return "Custom"; }
    EQ_PRESETS.iter().find(|x| x.id == id).unwrap_or(&EQ_PRESETS[0]).label
}
/// Does any effect change the sound? (the master switch on and something set)
pub fn fx_active(p: &Value) -> bool {
    if p.g("fx_on") != &json!(true) { return false; }
    !eq_bands(p).is_empty() || p.f("eq_preamp") != 0.0 || p.s("crossfeed") != "off" || p.f("balance") != 0.0 || p.truthy("mono")
}
/// `${n}` of a JS number.
fn n2s(x: f64) -> String { x.to_string() }
fn db(v: f64) -> String { format!("{}{} dB", if v > 0.0 { "+" } else { "" }, n2s(v)) }
pub fn balance_text(v: f64) -> String {
    if v == 0.0 { "Centre".into() } else if v < 0.0 { format!("L{}", n2s(-v)) } else { format!("R{}", n2s(v)) }
}
/// "EQ: Bass boost, preamp -6 dB · Crossfeed: medium · Balance: L10 · Mono · Crossfade: 6 s" – '' when nothing applies.
pub fn effects_summary(p: &Value, crossfade: bool) -> String {
    let mut parts: Vec<String> = vec![];
    if fx_active(p) {
        let pre = p.f("eq_preamp");
        if !eq_bands(p).is_empty() || pre != 0.0 {
            parts.push(format!("EQ: {}{}", preset_label(&p.s("eq_preset")), if pre != 0.0 { format!(", preamp {}", db(pre)) } else { String::new() }));
        }
        if p.s("crossfeed") != "off" && !p.truthy("mono") { parts.push(format!("Crossfeed: {}", p.s("crossfeed"))); }
        if p.f("balance") != 0.0 { parts.push(format!("Balance: {}", balance_text(p.f("balance")))); }
        if p.truthy("mono") { parts.push("Mono".into()); }
    }
    if crossfade && p.f("crossfade") > 0.0 { parts.push(format!("Crossfade: {} s", n2s(p.f("crossfade")))); }
    parts.join(" · ")
}

// ---- adaptive quality (Auto): the ladder and the choice of rung from the estimated bandwidth ------------
/// Rungs in kbps, best first; 0 = the original file.
pub const RUNGS: [i64; 6] = [0, 256, 160, 96, 64, 32];
pub const SAVER_KBPS: i64 = 96;
pub const EWMA_ALPHA: f64 = 0.3;
/// The rungs a song can use: the original, then only the converted streams smaller than it.
pub fn ladder(orig_kbps: f64) -> Vec<i64> {
    RUNGS.iter().copied().filter(|&r| r == 0 || !(orig_kbps > 0.0) || (r as f64) < orig_kbps).collect()
}
/// kbit/s a rung needs: the rung itself, the original's bitrate for the original (1411 when unknown).
pub fn needed(rung: i64, orig_kbps: f64) -> f64 { if rung == 0 { if orig_kbps > 0.0 { orig_kbps } else { 1411.0 } } else { rung as f64 } }
/// Highest rung that fits in 75 % of the estimate; the original while there is no estimate yet; 32 kbps at worst.
pub fn choose_rung(estimate: Option<f64>, orig_kbps: f64) -> i64 {
    let l = ladder(orig_kbps);
    let last = *l.last().unwrap_or(&0);
    let Some(e) = estimate else { return 0 };
    if !(e > 0.0) { return last; }
    l.iter().copied().find(|&r| needed(r, orig_kbps) <= 0.75 * e).unwrap_or(last)
}
/// One rung down from `rung` (stays at the bottom).
pub fn step_down(rung: i64, orig_kbps: f64) -> i64 {
    let l = ladder(orig_kbps);
    let i = l.iter().position(|&r| r == rung).map(|i| i as i64).unwrap_or(-1);
    let j = ((if i < 0 { 0 } else { i }) + 1).min(l.len() as i64 - 1);
    l[j.max(0) as usize]
}
/// Exponentially weighted moving average of throughput samples (kbit/s); the first sample sets it.
pub fn ewma(prev: Option<f64>, sample: f64) -> f64 { match prev { None => sample, Some(p) => p + EWMA_ALPHA * (sample - p) } }
/// kbit/s from bytes moved in `seconds`; None for transfers under 0.25 s (too short to say anything).
pub fn rate_of(bytes: f64, seconds: f64) -> Option<f64> { if seconds < 0.25 { None } else { Some((bytes * 8.0 / 1000.0) / seconds) } }

pub fn clean(p: &Value) -> Value {
    let mut c = defaults();
    if let (Some(dst), Some(src)) = (c.as_object_mut(), p.as_object()) {
        for (k, v) in src { dst.insert(k.clone(), v.clone()); }
    }
    // the player owns the stream bitrate now (adaptive)
    if let Some(o) = c.as_object_mut() { o.remove("kbps"); }
    let quality = c.s("quality");
    if !QUALITIES.contains(&quality.as_str()) { c["quality"] = json!("auto"); }
    c["preload"] = json!(c.g("preload") != &json!(false));
    c["autoplay"] = json!(c.g("autoplay") != &json!(false));
    c["resume"] = json!(c.g("resume") != &json!(false));
    c["dsp"] = json!(c.g("dsp") == &json!(true));
    if !c.g("dsp_filter").is_string() { c["dsp_filter"] = json!(""); }
    if c.st("dsp_dither").is_none() { c["dsp_dither"] = json!("tpdf"); }
    let rate = c.g("dsp_rate");
    if rate != &json!("device") && rate != &json!("auto") {
        // `Number(x)`, kept only when it is a positive integer
        let r = js_number(rate);
        c["dsp_rate"] = if r.is_finite() && r.fract() == 0.0 && r > 0.0 {
            if r <= i64::MAX as f64 { json!(r as i64) } else { json!(r) }
        } else { json!("device") };
    }
    c["fx_on"] = json!(c.g("fx_on") == &json!(true));
    let preset = c.g("eq_preset").as_str().filter(|x| *x == "custom" || EQ_PRESETS.iter().any(|p| p.id == *x)).unwrap_or("flat").to_string();
    c["eq_preset"] = json!(preset);
    c["eq_bands"] = match c.g("eq_bands").as_array() { Some(a) => Value::Array(a.iter().take(10).map(clean_band).collect()), None => json!([]) };
    c["eq_preamp"] = jnum(round_to(clamp_num(c.g("eq_preamp"), -24.0, 12.0, 0.0), 1));
    let xf = c.g("crossfeed").as_str().filter(|x| CROSSFEED.iter().any(|(v, _)| v == x)).unwrap_or("off").to_string();
    c["crossfeed"] = json!(xf);
    c["balance"] = jnum(js_round(clamp_num(c.g("balance"), -100.0, 100.0, 0.0)));
    c["mono"] = json!(c.g("mono") == &json!(true));
    c["crossfade"] = jnum(js_round(clamp_num(c.g("crossfade"), 0.0, 12.0, 0.0)));
    c["crossfade_album_gapless"] = json!(c.g("crossfade_album_gapless") != &json!(false));
    c
}

/// JavaScript's `Number(x)` for a JSON value (NaN where JS gives NaN).
fn js_number(v: &Value) -> f64 {
    match v {
        Value::Null => 0.0,
        Value::Bool(b) => if *b { 1.0 } else { 0.0 },
        Value::Number(n) => n.as_f64().unwrap_or(f64::NAN),
        Value::String(s) => { let t = s.trim(); if t.is_empty() { 0.0 } else { t.parse::<f64>().unwrap_or(f64::NAN) } }
        Value::Array(a) => match a.as_slice() { [] => 0.0, [x] if !x.is_array() && !x.is_object() => js_number(x), _ => f64::NAN },
        Value::Object(_) => f64::NAN,
    }
}

pub fn current() -> Value {
    match ls_get(KEY).filter(|s| !s.is_empty()) {
        Some(stored) => match serde_json::from_str::<Value>(&stored) { Ok(v) => clean(&v), Err(_) => clean(&json!({})) },
        // before Settings → Playback existed, "Then shuffle library" lived in its own key
        None => clean(&json!({ "autoplay": ls_get("encore.autoplay").as_deref() != Some("off") })),
    }
}

/// Save on this device and – where the server keeps preferences – on the account (follows you everywhere).
pub async fn save(p: &Value, server_prefs: bool) -> Value {
    let c = clean(p);
    ls_set(KEY, &c.to_string());
    ls_set("encore.autoplay", if c.truthy("autoplay") { "on" } else { "off" });
    dom::dispatch("encore:playback-prefs", Some(serde_wasm_bindgen::to_value(&c).unwrap_or(wasm_bindgen::JsValue::NULL)));
    if server_prefs {
        let _ = crate::api::put("/auth/prefs", json!({ "playback": c })).await;
    }
    c
}

/// After sign-in: the account's playback settings win over this device's.
pub fn adopt_account(user: &Value) {
    let a = user.g("prefs").g("playback").clone();
    if a.is_object() {
        dom::spawn(async move { save(&a, false).await; });
    }
}
