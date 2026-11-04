//! Volume normalization on this device (Settings → Playback → Sound): the three settings, the gain math, the gain stage
//! of each <audio> element, the Signal path's line and the settings block (client-shapes §5.1).
//!
//! The settings are kept on this device only (never in the account's preferences): localStorage `encore.norm.mode`
//! ("off" | "track" | "album"), `encore.norm.target` (−23 … −5 LUFS) and `encore.norm.clip` – the phone's
//! SharedPreferences keys too. A linked device sees them in this device's snapshot (`norm`) and changes them with the
//! remote command `{action: "prefs", norm: {mode, target, clip}}`.
//!
//! The math, the same on the web, the desktop and the phone (playback/Normalization.kt):
//!
//! ```text
//! applied dB = rg_gain + (target − (−18))
//! ```
//!
//! rg_gain is the song's (or its album's) ReplayGain in dB relative to the −18 LUFS reference (the track row's `gain`,
//! client-shapes §2.1) and target the loudness chosen in LUFS. A song measured at −10 LUFS has rg_gain −8; at target −14
//! it plays at −8 + 4 = −4 dB, that is at −14 LUFS. Album mode takes the album gain (the track gain when the song has no
//! album gain), Track mode the track gain (the album gain when the song has no track gain); the peak goes with the gain
//! used (album peak else track peak, and the other way round). Prevent clipping keeps peak × gain ≤ 1, so applied dB ≤
//! −20·log10(peak); with no peak known it allows no boost above 0 dB (a cut is never limited by it). The result stays
//! within −30 … +15 dB, so a broken tag can't blast the speakers.
//!
//! The gain stage is a GainNode per <audio> element, first in the Web Audio graph (fx.rs): before the preamp, the EQ and
//! the other effects, and it works with the effects off. Each element keeps the gain of the song it plays, so during a
//! song transition the old song keeps its own. It applies to whatever stream arrives (the original, a lighter rung, a
//! FLAC conversion, a Cadenza render from the server – those stay gain-free on the server). 30-second previews play
//! outside the graph and are never normalized.

use super::*;

const KEY_MODE: &str = "encore.norm.mode";
const KEY_TARGET: &str = "encore.norm.target";
const KEY_CLIP: &str = "encore.norm.clip";
/// The modes and their labels (the segmented control).
pub const MODES: [(&str, &str); 3] = [("off", "Off"), ("track", "Track"), ("album", "Album")];
pub const DEFAULT_TARGET: i64 = -14;
pub const MIN_TARGET: i64 = -23;
pub const MAX_TARGET: i64 = -5;
/// ReplayGain's reference loudness: a gain of 0 dB plays a song at −18 LUFS.
const REFERENCE_LUFS: f64 = -18.0;
const MIN_DB: f64 = -30.0;
const MAX_DB: f64 = 15.0;

/// The three settings.
#[derive(Clone, Debug, PartialEq)]
pub struct NormPrefs { pub mode: String, pub target: i64, pub clip: bool }
impl Default for NormPrefs {
    fn default() -> Self { Self { mode: "off".into(), target: DEFAULT_TARGET, clip: true } }
}
impl NormPrefs {
    pub fn on(&self) -> bool { self.mode == "track" || self.mode == "album" }
    /// Within the limits: an unknown mode is Off, the target is kept within −23 … −5.
    pub fn clean(self) -> Self {
        let mode = if MODES.iter().any(|(m, _)| *m == self.mode) { self.mode } else { "off".into() };
        Self { mode, target: self.target.clamp(MIN_TARGET, MAX_TARGET), clip: self.clip }
    }
    /// The three stored values (a missing one takes its default).
    fn from_stored(mode: Option<&str>, target: Option<&str>, clip: Option<&str>) -> Self {
        let d = Self::default();
        Self {
            mode: mode.map(|m| m.trim().to_string()).unwrap_or(d.mode),
            target: target.and_then(|t| t.trim().parse::<f64>().ok()).filter(|t| t.is_finite()).map(|t| t.round() as i64).unwrap_or(d.target),
            clip: clip.map(|c| !matches!(c.trim(), "false" | "0" | "off")).unwrap_or(d.clip),
        }
        .clean()
    }
    /// `{mode, target, clip}`: what the snapshot reports and the `prefs` command carries.
    pub fn to_json(&self) -> Value { json!({ "mode": self.mode, "target": self.target, "clip": self.clip }) }
    /// These settings with the fields `v` has laid over them (a `prefs` command, a linked device's snapshot).
    pub fn with(&self, v: &Value) -> Self {
        let mut p = self.clone();
        if let Some(m) = v.g("mode").as_str() { p.mode = m.trim().to_string(); }
        if let Some(t) = finite(v.g("target")) { p.target = t.round() as i64; }
        if let Some(c) = v.g("clip").as_bool() { p.clip = c; }
        p.clean()
    }
}

/// A JSON number (or a number written as a string), when it is finite.
fn finite(v: &Value) -> Option<f64> {
    v.as_f64().or_else(|| v.as_str().and_then(|s| s.trim().parse::<f64>().ok())).filter(|x| x.is_finite())
}

/// A song's loudness data: the track row's `gain` (client-shapes §2.1), in dB relative to −18 LUFS and linear peaks.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TrackGain { pub track: Option<f64>, pub album: Option<f64>, pub track_peak: Option<f64>, pub album_peak: Option<f64> }
impl TrackGain {
    /// The row's `gain`; None when it is missing or knows neither the track gain nor the album gain (`{}` too).
    pub fn of(t: &Value) -> Option<Self> {
        let g = t.g("gain");
        if !g.is_object() { return None; }
        let n = |k: &str| finite(g.g(k));
        let out = Self { track: n("track"), album: n("album"), track_peak: n("track_peak"), album_peak: n("album_peak") };
        (out.track.is_some() || out.album.is_some()).then_some(out)
    }
}

/// What normalization does to one song. `basis` is "track" or "album" ("" when nothing is known or it is off),
/// `limited` when Prevent clipping lowered the gain, `requested_db` what the tags asked for.
#[derive(Clone, Debug, PartialEq)]
pub struct NormDecision { pub on: bool, pub db: f64, pub basis: &'static str, pub limited: bool, pub requested_db: f64, pub target: i64 }
impl NormDecision {
    fn off() -> Self { Self { on: false, db: 0.0, basis: "", limited: false, requested_db: 0.0, target: DEFAULT_TARGET } }
    fn no_data_at(target: i64) -> Self { Self { on: true, target, ..Self::off() } }
    /// On, but nothing is known about this song's loudness: it plays at its own level.
    pub fn no_data(&self) -> bool { self.on && self.basis.is_empty() }
    /// The factor the samples are multiplied by (1 = untouched).
    pub fn linear(&self) -> f64 { if !self.on || self.basis.is_empty() || self.db == 0.0 { 1.0 } else { linear(self.db) } }
    /// The Signal path's value: "−3.2 dB (album gain, target −14 LUFS)"; "" when off.
    pub fn line(&self) -> String {
        if !self.on { String::new() }
        else if self.basis.is_empty() { "No loudness data for this song".into() }
        else { format!("{} ({} gain, target {})", db_text(self.db), self.basis, lufs_text(self.target)) }
    }
    /// The Signal path's notes that apply (`mode` is the setting: a song without the gain asked for uses the other one).
    pub fn notes(&self, mode: &str) -> Vec<String> {
        if !self.on { return vec![]; }
        if self.no_data() { return vec!["It plays at its own level until Encore has measured it.".into()]; }
        let mut out = vec![];
        if self.limited { out.push(format!("Lowered to keep the peaks from clipping (the song asked for {}).", db_text(self.requested_db))); }
        if self.basis == "track" && mode == "album" { out.push("No album gain for this song, so its track gain is used.".into()); }
        if self.basis == "album" && mode == "track" { out.push("No track gain for this song, so its album gain is used.".into()); }
        out
    }
}

/// dB → the factor the samples are multiplied by.
pub fn linear(db: f64) -> f64 { 10f64.powf(db / 20.0) }
/// The gain for a ReplayGain value at a target loudness: the formula at the top, before clipping and the limits.
pub fn applied_db(rg_gain: f64, target: i64) -> f64 { rg_gain + (target as f64 - REFERENCE_LUFS) }
/// The most gain a song with this linear peak takes without clipping; 0 dB when the peak is unknown.
pub fn clip_ceiling_db(peak: Option<f64>) -> f64 {
    match peak { Some(p) if p > 0.0 => -20.0 * p.log10(), _ => 0.0 }
}
pub fn decide(p: &NormPrefs, g: Option<&TrackGain>) -> NormDecision {
    if !p.on() { return NormDecision::off(); }
    let p = p.clone().clean();
    let Some(g) = g else { return NormDecision::no_data_at(p.target) };
    let use_album = (p.mode == "album" && g.album.is_some()) || g.track.is_none();
    let Some(rg) = (if use_album { g.album } else { g.track }) else { return NormDecision::no_data_at(p.target) };
    let peak = if use_album { g.album_peak.or(g.track_peak) } else { g.track_peak.or(g.album_peak) };
    let wanted = applied_db(rg, p.target);
    let mut db = wanted;
    let mut limited = false;
    if p.clip {
        let ceiling = clip_ceiling_db(peak);
        if db > ceiling { db = ceiling; limited = true; }
    }
    NormDecision { on: true, db: db.clamp(MIN_DB, MAX_DB), basis: if use_album { "album" } else { "track" }, limited, requested_db: wanted, target: p.target }
}
/// −3.24 → "−3.2 dB", 4 → "+4.0 dB", −0.01 → "0.0 dB" (one decimal, a real minus sign).
pub fn db_text(db: f64) -> String {
    let r = playback::js_round(db * 10.0) / 10.0;
    let s = format!("{:.1}", r.abs());
    if r > 0.0 { format!("+{s} dB") } else if r < 0.0 { format!("\u{2212}{s} dB") } else { "0.0 dB".into() }
}
/// −14 → "−14 LUFS".
pub fn lufs_text(v: i64) -> String { if v < 0 { format!("\u{2212}{} LUFS", -v) } else { format!("{v} LUFS") } }
fn mode_help(mode: &str) -> &'static str {
    match mode {
        "track" => "Every song plays at about the same loudness.",
        "album" => "Every album plays at about the same loudness; the songs of an album keep their differences.",
        _ => "Every song plays at its own level.",
    }
}

// ---- this device's settings and its gain stage ---------------------------------------------------------------------
thread_local! {
    static NORM: RefCell<Option<NormPrefs>> = const { RefCell::new(None) };
}

fn stored() -> NormPrefs { NormPrefs::from_stored(dom::ls_get(KEY_MODE).as_deref(), dom::ls_get(KEY_TARGET).as_deref(), dom::ls_get(KEY_CLIP).as_deref()) }
/// This device's normalization settings.
pub fn norm_prefs() -> NormPrefs {
    if let Some(p) = NORM.with(|n| n.borrow().clone()) { return p; }
    let p = stored();
    NORM.with(|n| *n.borrow_mut() = Some(p.clone()));
    p
}
pub(super) fn norm_on() -> bool { norm_prefs().on() }
/// New settings for this device (Settings, or a `prefs` command from a device controlling this one): saved here,
/// and the song playing changes level at once (gliding); the next songs take them when they start.
pub fn set_norm_prefs(p: NormPrefs) {
    let p = p.clean();
    if p == norm_prefs() { return; }
    dom::ls_set(KEY_MODE, &p.mode);
    dom::ls_set(KEY_TARGET, &p.target.to_string());
    dom::ls_set(KEY_CLIP, if p.clip { "true" } else { "false" });
    NORM.with(|n| *n.borrow_mut() = Some(p));
    norm_changed();
}
/// Another tab changed the settings (the `storage` event).
pub(super) fn reload_norm() {
    let p = stored();
    if p == norm_prefs() { return; }
    NORM.with(|n| *n.borrow_mut() = Some(p));
    norm_changed();
}
fn norm_changed() {
    if norm_on() { ensure_fx(); }
    norm_now(true);
    // the devices that control this one see the new settings in the next check-in (emit → the remote's beat)
    emit();
}
/// What normalization does to song `t` here (a preview is never normalized: it plays outside the graph).
pub(super) fn norm_for(t: &Value) -> NormDecision {
    if t.truthy("preview") { return NormDecision::off(); }
    decide(&norm_prefs(), TrackGain::of(t).as_ref())
}
/// Set the gain stage of element `el` for song `t` (None: untouched): at once when a song starts on it, gliding when the
/// settings change in the middle of a song. Nothing to do while the graph isn't built (normalization and effects off).
pub(super) fn apply_norm(el: &HtmlMediaElement, t: Option<&Value>, smooth: bool) {
    let node = FX.with(|fx| fx.borrow().sources.iter().find(|s| same_el(&s.el, el)).and_then(|s| s.norm.clone()));
    let Some(n) = node else { return };
    let g = t.map(|t| norm_for(t).linear()).unwrap_or(1.0) as f32;
    let p = n.gain();
    match audio_ctx() {
        Some(ctx) => {
            let now = ctx.current_time();
            cancel_and_hold(&p, now);
            let set = if smooth { p.set_target_at_time(g, now, 0.05) } else { p.set_value_at_time(g, now) };
            if set.is_err() { p.set_value(g); }
        }
        None => p.set_value(g),
    }
}
/// The element playing now takes the gain of the song on it.
pub(super) fn norm_now(smooth: bool) {
    if let Some(a) = audio() { apply_norm(&a, cur().as_deref(), smooth); }
}
/// The gain normalization applies here to `t`, as the Signal path shows it: (value, note) – None while it is off.
pub(super) fn norm_row(t: &Value, info: Option<&Value>) -> Option<(String, String)> {
    let d = norm_for(t);
    if !d.on { return None; }
    let mut notes = d.notes(&norm_prefs().mode);
    if d.db != 0.0 {
        match info.and_then(|i| i.g("gain").st("source")) {
            Some("tags") => notes.push("From the file's ReplayGain tags.".into()),
            Some("measured") => notes.push("Measured by Encore.".into()),
            _ => {}
        }
    }
    Some((d.line(), notes.join(" ")))
}
/// Normalization changes the level of the song playing here (the Signal path's verdict: not "Pure playback").
pub(super) fn norm_changes(t: &Value) -> bool { let d = norm_for(t); d.on && !d.no_data() && d.db != 0.0 }

// ---- Settings → Playback → Sound (on this device) ----------------------------------------------------------------------
/// The settings the block shows: a linked device's when it reports them (its snapshot's `norm`), else this device's.
fn shown() -> (NormPrefs, Option<Link>) {
    if let Some(l) = linked() {
        let n = l.state.g("norm");
        if n.is_object() { return (NormPrefs::default().with(n), Some(l)); }
    }
    (norm_prefs(), None)
}
fn shown_key() -> String { let (p, l) = shown(); json!([p.to_json(), l.map(|l| l.name)]).to_string() }

/// Settings → Playback → Sound (on this device): "Volume normalization", first in the section (before "Use sound
/// effects"). While this device controls another one that reports its own settings, the block shows and changes those.
pub fn norm_setting(root: &Element) {
    if qs_in(root, "#pb-norm").is_some() { return; }
    let Some(anchor) = qs_in(root, "#pb-fx").and_then(|c| dom::closest_of(&c, "label")) else { return };
    let _ = anchor.insert_adjacent_html("beforebegin", r#"<div class="stack" id="pb-norm" style="gap:6px"></div>"#);
    let Some(b) = qs_in(root, "#pb-norm") else { return };
    paint_norm(&b);
    // settings changed elsewhere (a device controlling this one, another tab, the linked device's own): painted again
    if root.get_attribute("data-norm-watch").is_none() {
        let _ = root.set_attribute("data-norm-watch", "");
        let r = root.clone();
        dom::interval(root, 2000, move || {
            if let Some(b) = qs_in(&r, "#pb-norm") { if dom::data(&b, "k") != shown_key() { paint_norm(&b); } }
        });
    }
}
fn paint_norm(b: &Element) {
    let (p, link) = shown();
    dom::set_data(b, "k", &shown_key());
    let on = p.on();
    set_html(b, &format!(r#"<span class="small muted">Volume normalization{name}</span>
        <div class="seg" data-norm-mode>{modes}</div>
        <span class="small muted">{help}</span>
        <div class="stack" style="gap:8px;{dim}">
          <label class="stack" style="gap:4px"><span class="small muted">Target level <b id="pb-norm-target-v">{lufs}</b></span><input type="range" id="pb-norm-target" min="{min}" max="{max}" step="1" value="{target}" {disabled}></label>
          <label class="row" style="gap:8px;align-items:center"><input type="checkbox" id="pb-norm-clip" {clip} {disabled}> Prevent clipping</label>
        </div>
        <span class="small muted">Uses the ReplayGain tags in your files, or Encore's own measurement when a file has none. Higher targets are louder; −14 LUFS is about what streaming services play at. Prevent clipping never turns a song up further than its loudest peak allows. Only on this device.</span>"#,
        name = link.as_ref().map(|l| format!(" · {}", esc(&l.name))).unwrap_or_default(),
        modes = MODES.iter().map(|(v, l)| format!(r#"<button type="button" data-v="{v}" class="{}">{l}</button>"#, if p.mode == *v { "on" } else { "" })).collect::<String>(),
        help = mode_help(&p.mode),
        dim = if on { "" } else { "opacity:.45" },
        lufs = lufs_text(p.target), min = MIN_TARGET, max = MAX_TARGET, target = p.target,
        clip = if p.clip { "checked" } else { "" },
        disabled = if on { "" } else { "disabled" }));
    let save: Rc<dyn Fn(NormPrefs)> = {
        let (b, link) = (b.clone(), link);
        Rc::new(move |n: NormPrefs| {
            match &link {
                Some(l) => {
                    (l.send)(json!({ "action": "prefs", "norm": n.to_json() }));
                    toast(&format!("Sent to {}", l.name), "ok");
                    // shown at once; the device's next check-in confirms it
                    patch_link_state("norm", n.to_json());
                }
                None => set_norm_prefs(n),
            }
            paint_norm(&b);
        })
    };
    {
        let (save, p) = (save.clone(), p.clone());
        dom::on_each(b, "[data-norm-mode] button", "click", move |x, _| save(NormPrefs { mode: dom::data(x, "v"), ..p.clone() }));
    }
    if let Some(r) = qs_in(b, "#pb-norm-target") {
        let (b2, r2) = (b.clone(), r.clone());
        dom::on(&r, "input", move |_| { if let Some(v) = qs_in(&b2, "#pb-norm-target-v") { dom::set_text(&v, &lufs_text(num(&dom::value(&r2)) as i64)); } });
        let (save, p, r2) = (save.clone(), p.clone(), r.clone());
        dom::on(&r, "change", move |_| save(NormPrefs { target: num(&dom::value(&r2)) as i64, ..p.clone() }));
    }
    if let Some(c) = qs_in(b, "#pb-norm-clip") {
        let (save, p, c2) = (save.clone(), p, c.clone());
        dom::on(&c, "change", move |_| save(NormPrefs { clip: dom::checked(&c2), ..p.clone() }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(track: Option<f64>, album: Option<f64>, track_peak: Option<f64>, album_peak: Option<f64>) -> TrackGain { TrackGain { track, album, track_peak, album_peak } }
    fn prefs(mode: &str, target: i64, clip: bool) -> NormPrefs { NormPrefs { mode: mode.into(), target, clip } }
    const EPS: f64 = 1e-9;

    #[test]
    fn the_formula_is_rg_gain_plus_target_minus_reference() {
        // applied dB = rg_gain + (target − (−18)): a song at −10 LUFS (rg −8) plays at −4 dB for a −14 LUFS target
        assert!((applied_db(-8.0, -14) - -4.0).abs() < EPS);
        assert!((applied_db(-8.0, -18) - -8.0).abs() < EPS);
        assert!((applied_db(-8.0, -23) - -13.0).abs() < EPS);
        assert!((applied_db(3.0, -5) - 16.0).abs() < EPS);
        assert!((linear(0.0) - 1.0).abs() < EPS);
        assert!((linear(-6.0) - 0.501_187_233_6).abs() < 1e-9);
    }

    #[test]
    fn off_and_no_data() {
        let gain = g(Some(-8.0), Some(-7.0), Some(0.9), Some(1.0));
        let off = decide(&prefs("off", -14, true), Some(&gain));
        assert!(!off.on);
        assert_eq!(off.linear(), 1.0);
        assert_eq!(off.line(), "");
        assert!(off.notes("off").is_empty());
        let none = decide(&prefs("track", -14, true), None);
        assert!(none.no_data());
        assert_eq!(none.linear(), 1.0);
        assert_eq!(none.line(), "No loudness data for this song");
        assert_eq!(none.notes("track"), vec!["It plays at its own level until Encore has measured it.".to_string()]);
    }

    #[test]
    fn track_and_album_with_their_fallbacks() {
        let gain = g(Some(-8.0), Some(-6.5), Some(0.5), Some(0.6));
        let t = decide(&prefs("track", -14, true), Some(&gain));
        assert_eq!(t.basis, "track");
        assert!((t.db - -4.0).abs() < EPS);
        assert!((t.linear() - linear(-4.0)).abs() < EPS);
        let a = decide(&prefs("album", -14, true), Some(&gain));
        assert_eq!(a.basis, "album");
        assert!((a.db - -2.5).abs() < EPS);
        assert_eq!(a.line(), "\u{2212}2.5 dB (album gain, target \u{2212}14 LUFS)");
        // album mode without an album gain uses the track gain, and the other way round, and says so
        let only_track = decide(&prefs("album", -14, true), Some(&g(Some(-8.0), None, None, None)));
        assert_eq!(only_track.basis, "track");
        assert_eq!(only_track.notes("album"), vec!["No album gain for this song, so its track gain is used.".to_string()]);
        let only_album = decide(&prefs("track", -14, true), Some(&g(None, Some(-7.0), None, None)));
        assert_eq!(only_album.basis, "album");
        assert_eq!(only_album.notes("track"), vec!["No track gain for this song, so its album gain is used.".to_string()]);
    }

    #[test]
    fn prevent_clipping_limits_a_boost_to_the_peak() {
        // a quiet song (+7 dB at −14) whose peak is at half scale may rise only 6.02 dB
        let quiet = g(Some(3.0), None, Some(0.5), None);
        let on = decide(&prefs("track", -14, true), Some(&quiet));
        assert!(on.limited);
        assert!((on.db - 6.0206).abs() < 1e-4);
        assert!((on.requested_db - 7.0).abs() < EPS);
        assert!((linear(on.db) * 0.5 - 1.0).abs() < 1e-9);   // peak × gain = 1
        assert_eq!(on.notes("track"), vec!["Lowered to keep the peaks from clipping (the song asked for +7.0 dB).".to_string()]);
        assert!(!decide(&prefs("track", -14, true), Some(&g(Some(1.0), None, Some(0.5), None))).limited);
        // no peak known: no boost above 0 dB; a cut is never limited
        assert!((decide(&prefs("track", -14, true), Some(&g(Some(2.0), None, None, None))).db).abs() < EPS);
        assert!((decide(&prefs("track", -14, true), Some(&g(Some(-8.0), None, None, None))).db - -4.0).abs() < EPS);
        // the peak that goes with the gain: album mode takes the album peak, else the track peak
        let peaks = g(Some(3.0), Some(3.0), Some(0.25), Some(0.5));
        assert!((decide(&prefs("album", -14, true), Some(&peaks)).db - clip_ceiling_db(Some(0.5))).abs() < EPS);
        assert!((decide(&prefs("track", -14, true), Some(&peaks)).db - 7.0).abs() < EPS);
        // clipping allowed: the full boost, within +15 dB; and never below −30 dB
        assert!((decide(&prefs("track", -14, false), Some(&quiet)).db - 7.0).abs() < EPS);
        assert!((decide(&prefs("track", -5, false), Some(&g(Some(20.0), None, None, None))).db - 15.0).abs() < EPS);
        assert!((decide(&prefs("track", -23, true), Some(&g(Some(-40.0), None, None, None))).db - -30.0).abs() < EPS);
    }

    #[test]
    fn settings_are_cleaned_and_read_like_the_phone() {
        assert_eq!(NormPrefs::default(), prefs("off", -14, true));
        assert_eq!(prefs("loud", -40, false).clean(), prefs("off", -23, false));
        assert_eq!(prefs("album", 3, true).clean().target, -5);
        assert_eq!(NormPrefs::from_stored(None, None, None), NormPrefs::default());
        assert_eq!(NormPrefs::from_stored(Some("album"), Some("-9"), Some("false")), prefs("album", -9, false));
        assert_eq!(NormPrefs::from_stored(Some("track"), Some("x"), Some("true")), prefs("track", -14, true));
        assert_eq!(NormPrefs::from_stored(Some("track"), Some("-30"), Some("0")), prefs("track", -23, false));
        // a `prefs` command or a snapshot: only the fields it has
        let p = prefs("track", -14, true);
        assert_eq!(p.with(&json!({ "mode": "album" })), prefs("album", -14, true));
        assert_eq!(p.with(&json!({ "target": -9, "clip": false })), prefs("track", -9, false));
        assert_eq!(p.with(&json!({ "target": "-20" })), prefs("track", -20, true));
        assert_eq!(p.with(&json!({ "mode": "max", "target": 0 })), prefs("off", -5, true));
        assert_eq!(p.with(&p.to_json()), p);
    }

    #[test]
    fn texts() {
        assert_eq!(db_text(4.0), "+4.0 dB");
        assert_eq!(db_text(-3.24), "\u{2212}3.2 dB");
        assert_eq!(db_text(-0.01), "0.0 dB");
        assert_eq!(db_text(0.0), "0.0 dB");
        assert_eq!(lufs_text(-14), "\u{2212}14 LUFS");
        assert_eq!(decide(&prefs("track", -14, true), Some(&g(Some(-2.0), None, Some(0.4), None))).line(), "+2.0 dB (track gain, target \u{2212}14 LUFS)");
    }

    #[test]
    fn the_row_shape() {
        let row = json!({ "id": 1, "gain": { "track": -7.21, "album": null, "track_peak": 0.988 } });
        let gain = TrackGain::of(&row).expect("gain");
        assert_eq!(gain.track, Some(-7.21));
        assert_eq!(gain.album, None);
        assert_eq!(gain.track_peak, Some(0.988));
        assert_eq!(gain.album_peak, None);
        assert_eq!(TrackGain::of(&json!({ "gain": {} })), None);
        assert_eq!(TrackGain::of(&json!({ "gain": { "track_peak": 0.9 } })), None);
        assert_eq!(TrackGain::of(&json!({ "id": 1 })), None);
        assert_eq!(TrackGain::of(&json!({ "gain": { "album": "-6.84" } })).and_then(|g| g.album), Some(-6.84));
    }
}
