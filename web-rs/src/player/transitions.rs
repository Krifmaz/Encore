//! Song transitions – crossfade, overlap and smart overlap – between the playing element and the spare.

use super::*;

thread_local! {
    /// the crossfade in progress
    pub(super) static FADE: RefCell<Option<Fade>> = const { RefCell::new(None) };
    /// track id → (start_s, end_s) once known (None: asked, unknown)
    static EDGES: RefCell<HashMap<i64, Option<(f64, f64)>>> = RefCell::new(HashMap::new());
}

pub(super) fn fading() -> bool { FADE.with(|f| f.borrow().is_some()) }

// ---- song transitions ----------------------------------------------------------------------------
// Off | Crossfade (equal-power, both fade) | Overlap (the next song starts at full volume early, only the old one
// fades out) | Smart overlap (the silent ends of both songs are skipped: the next song starts at its first audible
// moment, overlapping the old one up to its last audible moment). The next song plays on the spare element; when
// the window opens and it isn't buffered (pre-load off, a slow server stream), it starts loading then and the
// transition begins as soon as it can play, over what is left of the window.
// The gains follow the new song's own clock and are set from its timeupdate events as well as a timer: in a
// hidden or minimised window (or a phone with the screen off) timers are throttled or stopped, and the fade must
// not stall there – that is what left the next song silent before ("it just makes the audio quiet").
pub(super) struct Fade { pub(super) out: HtmlMediaElement, pub(super) mode: String, pub(super) len: f64, pub(super) from: f64, pub(super) out_end: f64, pub(super) timer: Option<Interval> }

/// Where a song's sound starts and stops (Smart overlap) – None until the server has answered.
fn track_edges(t: &Value) -> Option<(f64, f64)> {
    if !is_lib(t) { return None; }
    let id = t.i("id");
    if let Some(e) = EDGES.with(|m| m.borrow().get(&id).cloned()) { return e; }
    EDGES.with(|m| m.borrow_mut().insert(id, None));
    dom::spawn(async move {
        if let Ok(e) = api::get(&format!("/library/tracks/{id}/edges")).await {
            let (s, end) = (e.f("start_s"), e.f("end_s"));
            if end > 0.0 { EDGES.with(|m| m.borrow_mut().insert(id, Some((s.max(0.0), end)))); }
        }
    });
    None
}
/// The URL load_opts() asks for with these options (offset 0) – the same decisions load_opts() makes.
fn planned_url(t: &Value, transcode: bool, rung: i64, dsp: bool) -> String {
    let lossy = rung > 0;
    let d = dsp && want_dsp(t, lossy);
    let transcoded = convert(t) || transcode || lossy || d;
    stream_url(t, transcoded && !lossy && !d, rung, d, 0.0)
}
/// The load options (transcode, rung, dsp) that make load_opts() use exactly what the spare element holds for song
/// `n` (any rung) – None when it holds something else.
fn spare_for(n: &Value) -> Option<(bool, i64, bool)> {
    let sp = spare()?;
    let u = dom::data_opt(&sp, "url")?;
    // the next song's downloaded copy: load_opts() takes it whatever the options say
    if is_local_url(n, &u) { return Some((false, 0, false)); }
    if n.truthy("preview") || !n.g("id").is_number() || !u.starts_with(&format!("/api/library/tracks/{}/stream", n.i("id"))) { return None; }
    let q = web_sys::UrlSearchParams::new_with_str(u.split_once('?').map(|x| x.1).unwrap_or("")).ok()?;
    if q.has("t") { return None; }
    let rung = q.get("kbps").and_then(|k| k.parse::<i64>().ok()).unwrap_or(0);
    let (dsp, transcode) = (q.get("dsp").as_deref() == Some("1"), q.get("transcode").as_deref() == Some("1"));
    (planned_url(n, transcode, rung, dsp) == u).then_some((transcode, rung, dsp))
}
pub(super) fn seekable_to(el: &HtmlMediaElement, at: f64) -> bool {
    let s = el.seekable();
    s.length() > 0 && s.end(s.length() - 1).unwrap_or(0.0) >= at
}
pub(super) fn maybe_transition() {
    let p = prefs();
    let mode = p.s("transition");
    // crossfade is off in a group listen (gapless stays)
    if mode.is_empty() || mode == "off" || fading() || is_linked() || grouped() || HELD.with(|h| h.get()) || st(|s| s.repeat == "one" || s.dragging) || audio().is_none_or(|a| a.paused()) { return; }
    let (t, n) = st(|s| (
        if s.index < 0 { None } else { s.queue.get(s.index as usize).cloned() },
        if s.index + 1 < 0 { None } else { s.queue.get((s.index + 1) as usize).cloned() },
    ));
    let (Some(t), Some(n)) = (t, n) else { return };
    if t.truthy("preview") || (n.truthy("preview") && fx_built()) { return; }
    if p.truthy("crossfade_album_gapless") && !t.is_nullish("album_id") && t.g("album_id") == n.g("album_id") { return; }
    let cf = p.f("crossfade");
    let secs = if cf > 0.0 { cf } else { playback::TRANSITION_DEFAULT_S }.max(1.0);
    let d = duration();
    if !truthy_num(d) || d < 2.0 * secs { return; }
    let smart = if mode == "smart" { Some((track_edges(&t), track_edges(&n))) } else { None };
    // the old song's last audible moment (Smart overlap), else its end
    let end = match smart { Some((Some((_, e)), _)) if e > 2.0 * secs && e < d => e, _ => d };
    let remaining = end - position();
    if remaining > secs || remaining < 1.0 { return; }
    let Some(sp) = spare() else { return };
    let o = spare_for(&n);
    if o.is_none() || sp.ready_state() < 3 {
        // not buffered yet: fetch it now (even with pre-load off) and start as soon as it can play
        if o.is_none() { maybe_preload_need(true, true); }
        return;
    }
    let Some((transcode, rung, dsp)) = o else { return };
    let Some(out) = audio() else { return };
    log_play();
    let from = match smart { Some((_, Some((s, _)))) if s > 0.05 && s < 30.0 => s, _ => 0.0 };
    let out_end = out.current_time() + remaining;
    FADE.with(|f| *f.borrow_mut() = Some(Fade { out, mode, len: remaining, from, out_end, timer: None }));
    st_mut(|s| s.index += 1);
    load_opts(LoadOpts { fade: true, transcode, rung: Some(rung), dsp: Some(dsp), from, ..Default::default() });
    if !fading() { return; }   // load_opts() could not use the spare after all: a plain start
    let started = audio().map(|a| a.current_time()).unwrap_or(0.0);
    let timer = Interval::new(50, step_fade);
    FADE.with(|f| if let Some(f) = f.borrow_mut().as_mut() { f.from = started; f.timer = Some(timer); });
    step_fade();
}
pub(super) fn step_fade() {
    let Some((out, mode, len, from, out_end)) = FADE.with(|f| f.borrow().as_ref().map(|f| (f.out.clone(), f.mode.clone(), f.len, f.from, f.out_end))) else { return };
    let Some(a) = audio() else { return };
    let x = ((a.current_time() - from).max(0.0) / len).min(1.0);
    let x = if x.is_nan() { 0.0 } else { x };
    let (g_out, g_in) = playback::transition_gains(&mode, x, x * len);
    set_fade_gain_smooth(&out, g_out, true);
    set_fade_gain_smooth(&a, g_in, true);
    if x >= 1.0 || out.ended() || out.current_time() >= out_end { end_fade(); }
}
/// Finish (or cut short – next/prev/seek) a transition: the old song stops, the new one plays at full volume.
pub(super) fn end_fade() {
    let Some(f) = FADE.with(|f| f.borrow_mut().take()) else { return };
    // the interval may be the caller: let it go once this turn is over
    if let Some(timer) = f.timer { dom::spawn(async move { drop(timer); }); }
    let out = f.out;
    let _ = out.pause();
    let _ = out.remove_attribute("src");
    let _ = out.remove_attribute("data-url");
    out.load();
    sweep();
    set_fade_gain(&out, 1.0);
    if let Some(a) = audio() { set_fade_gain(&a, 1.0); }
    route();
    maybe_preload(false);
}
/// `p.cancelAndHoldAtTime(t)` where the browser has it (not in web-sys), else `cancelScheduledValues(t)`.
pub(super) fn cancel_and_hold(p: &web_sys::AudioParam, t: f64) {
    let f = js_sys::Reflect::get(p, &JsValue::from_str("cancelAndHoldAtTime")).ok().and_then(|f| f.dyn_into::<js_sys::Function>().ok());
    match f {
        Some(f) => { let _ = f.call1(p, &JsValue::from_f64(t)); }
        None => { let _ = p.cancel_scheduled_values(t); }
    }
}
/// The fade level of one element: a Web Audio gain when it is routed through the effects graph, else its volume (under the slider).
pub(super) fn set_fade_gain(el: &HtmlMediaElement, g: f64) { set_fade_gain_smooth(el, g, false); }
/// smooth: glide there on the audio clock (the steps come a few times a second, more slowly in a hidden window).
fn set_fade_gain_smooth(el: &HtmlMediaElement, g: f64, smooth: bool) {
    let gain = FX.with(|fx| fx.borrow().sources.iter().find(|s| same_el(&s.el, el)).and_then(|s| s.gain.clone()));
    match (gain, audio_ctx()) {
        (Some(n), Some(ctx)) => {
            let p = n.gain();
            let now = ctx.current_time();
            cancel_and_hold(&p, now);
            let set = if smooth { p.set_target_at_time(g as f32, now, 0.05) } else { p.set_value_at_time(g as f32, now) };
            if set.is_err() { p.set_value(g as f32); }
            el.set_volume(user_vol());
        }
        (Some(n), None) => { n.gain().set_value(g as f32); el.set_volume(user_vol()); }
        _ => el.set_volume((user_vol() * g).clamp(0.0, 1.0)),
    }
}
