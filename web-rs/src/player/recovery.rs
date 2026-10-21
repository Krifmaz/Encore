//! Keeping a song going when its stream breaks off or stalls: telling a real end from a cut-off stream, loading
//! the song again at the same spot with backoff, and playing on when the page is visible again.

use super::*;

thread_local! {
    /// the song whose stream broke off, and its pick-up (see `retry`)
    pub(super) static REC: RefCell<Rec> = RefCell::new(Rec::default());
    /// waiting / a stalled download: checked again after 10 s
    pub(super) static STALL_WATCH: RefCell<Option<Timeout>> = const { RefCell::new(None) };
}

// ---- keeping the music going on a weak connection ---------------------------------------------
/// Seconds of this song already in the buffer past the play position.
pub(super) fn buffered_ahead() -> f64 {
    let Some(a) = audio() else { return 0.0 };
    let b = a.buffered();
    let p = a.current_time();
    for i in 0..b.length() {
        let (Ok(s), Ok(e)) = (b.start(i), b.end(i)) else { continue };
        if s <= p + 0.5 && e >= p { return e - p; }
    }
    0.0
}
pub(super) fn fully_buffered() -> bool {
    let Some(a) = audio() else { return false };
    let d = a.duration();
    if !truthy_num(d) || !d.is_finite() { return false; }
    let b = a.buffered();
    b.length() > 0 && b.end(b.length() - 1).unwrap_or(0.0) >= d - 1.0
}

/// Where the downloaded part of the playing element ends (seconds of its own clock).
fn buffered_end() -> f64 {
    let Some(a) = audio() else { return 0.0 };
    let b = a.buffered();
    if b.length() == 0 { return 0.0; }
    b.end(b.length() - 1).unwrap_or(0.0)
}

// ---- keeping a song going when its stream breaks off ---------------------------------------------
// A stream the server makes (converted, lighter, Data saver, Cadenza) is sent as it is made, its clock starting where it
// was asked to. When the connection drops (mobile network, a locked phone, Safari suspending the page) the browser may
// take the cut-off stream for the whole song and say it ended, report an error, or sit waiting. None of that is the end
// of the song: it is loaded again at the same spot (1 s, 2 s, 4 s … at most 30 s apart, at once when the network comes
// back) and keeps playing. Only a song that fails 6 times in a row is skipped.
const RETRY_MAX: u32 = 6;
const RETRY_MAX_WAIT: f64 = 30000.0;
/// t: the song · need: it has to be loaded again · at: where · from: where the last reload started · fails: in a row
/// ends: when it cut off early (3 within 60 s → one rung down on Auto, once) · empty: reloads that ended at once
/// deferred: the stream broke off but the element still plays what it has – the reload waits until that runs out
#[derive(Default)]
pub(super) struct Rec { pub(super) t: Option<Rc<Value>>, pub(super) need: bool, pub(super) at: f64, pub(super) from: f64, pub(super) fails: u32, pub(super) ends: Vec<f64>, pub(super) empty: u32, pub(super) stepped: bool, pub(super) deferred: bool, pub(super) timer: Option<Timeout> }
pub(super) fn reset_rec(t: Option<Rc<Value>>) {
    let old = REC.with(|r| std::mem::replace(&mut *r.borrow_mut(), Rec { t, ..Default::default() }));
    drop(old);
}
/// `rec.t === t`
pub(super) fn rec_is(t: &Rc<Value>) -> bool { REC.with(|r| r.borrow().t.as_ref().is_some_and(|x| Rc::ptr_eq(x, t))) }
pub(super) fn rec_need() -> bool { REC.with(|r| r.borrow().need) }
pub(super) fn rec_deferred() -> bool { REC.with(|r| r.borrow().deferred) }
/// How close to its end a song may stop and still count as finished: 2 % of it, at least 3 s, at most 10 s.
fn end_slack(d: f64) -> f64 { 3f64.max(d * 0.02).min(10.0) }
/// How close to the end reloads that give nothing mean the file is shorter than the library says: 10 %, at least 15 s, at most 30 s.
fn short_slack(d: f64) -> f64 { 15f64.max(d * 0.1).min(30.0) }
/// The last reload gave nothing at all (it ended or failed right where it started).
fn reload_got_nothing(at: f64) -> bool { REC.with(|r| { let r = r.borrow(); r.fails > 0 && at - r.from < 0.5 }) }
/// A reload of a server-made stream near the end failed at once: it asked past the file's real end (an empty stream is no
/// audio, which browsers report as an error) – the song is over.
pub(super) fn failed_past_end() -> bool {
    let Some(t) = cur() else { return false };
    if t.truthy("preview") || is_linked() || !st(|s| s.transcoded) || !rec_is(&t) { return false; }
    let d = duration();
    d > 0.0 && reload_got_nothing(known_position()) && d - REC.with(|r| r.borrow().from) < short_slack(d)
}
/// Where the song is – the last spot seen for sure when the element has lost it (emptied, no data).
pub(super) fn known_position() -> f64 {
    if audio().is_some_and(|a| a.ready_state() >= 1) { position() } else { st(|s| s.at) }
}
/// 'ended' from the playing element: did the song really reach its end, or did its stream break off?
pub(super) fn really_ended() -> bool {
    let Some(t) = cur() else { return true };
    if t.truthy("preview") || is_linked() { return true; }
    // the song's own length (from the library for server-made streams – their element's duration may be wrong)
    let d = duration();
    if !(d > 0.0) { return true; }
    let at = known_position();
    let left = d - at;
    if left <= end_slack(d) { return true; }
    if !rec_is(&t) { reset_rec(Some(t.clone())); }
    // reloaded there and it ended at once again, close to the end: the file is a little shorter than the library says
    let nothing = reload_got_nothing(at);
    let empty = REC.with(|r| {
        let mut r = r.borrow_mut();
        if nothing { r.empty += 1; } else { r.empty = 0; }
        r.empty
    });
    if empty >= 2 && left < short_slack(d) { return true; }
    let now = now_ms();
    let step = REC.with(|r| {
        let mut r = r.borrow_mut();
        r.ends.retain(|x| now - *x < 60000.0);
        r.ends.push(now);
        let step = r.ends.len() >= 3 && !r.stepped && prefs().s("quality") == "auto";
        if step { r.stepped = true; }
        step
    });
    // (in a group listen the rung is kept for the song: one drop at most)
    if step && group_may_drop() {
        // it keeps breaking off: one rung down (once), the reload asks for the lighter stream
        let (rung, floor) = st(|s| (s.rung, s.floor));
        let lower = playback::step_down(rung, orig_kbps(Some(&t)), floor);
        if lower != rung { note_rung_change(rung, lower); st_mut(|s| { s.rung = lower; s.step_at = now_ms(); }); }
    }
    retry(Some(at));
    false
}
/// The stream broke off at `at`: load the song again there – later, with backoff; skip it after 6 failures in a row.
pub(super) fn retry(at: Option<f64>) -> bool {
    let Some(t) = cur() else { return false };
    if t.truthy("preview") || is_linked() { return false; }
    let at = at.unwrap_or_else(known_position);
    if !rec_is(&t) { reset_rec(Some(t.clone())); }
    if rec_need() { return true; }   // one is on its way already
    let w = STALL_WATCH.with(|w| w.borrow_mut().take());
    drop(w);
    let d = duration();
    let at = (if d > 0.0 { at.min(d - 1.0) } else { at }).max(0.0);
    let fails = REC.with(|r| {
        let mut r = r.borrow_mut();
        r.need = true;
        r.at = at;
        r.fails += 1;
        r.fails
    });
    st_mut(|s| s.at = at);
    if fails >= RETRY_MAX { give_up(&t); return true; }
    if st(|s| s.want) { later(RETRY_MAX_WAIT.min(1000.0 * 2f64.powi(fails as i32 - 1))); }
    render(); emit(); media_state();
    true
}
pub(super) fn later(ms: f64) {
    let timer = Timeout::new(ms as u32, || {
        // fired: leave the spent timer be (dropping it here would free the running callback)
        if let Some(t) = REC.with(|r| r.borrow_mut().timer.take()) { t.forget(); }
        retry_now();
    });
    let old = REC.with(|r| r.borrow_mut().timer.replace(timer));
    drop(old);
}
/// Load the song again where it broke off (the same quality, Cadenza, conversion) – no new play, scrobble or transition.
pub(super) fn retry_now() {
    let old = REC.with(|r| r.borrow_mut().timer.take());
    drop(old);
    let Some(t) = cur() else { return };
    if !rec_need() || !rec_is(&t) || is_linked() { return; }
    if !st(|s| s.want) { return; }   // paused meanwhile: play picks it up
    let offline = js_get(dom::window().navigator().as_ref(), "onLine").as_bool() == Some(false);
    if offline { later(RETRY_MAX_WAIT); return; }   // offline: the 'online' event comes first
    if let Some(a) = audio().filter(|a| !a.paused() && a.ready_state() >= 2) {
        // still playing what it has (a network error leaves the buffer playing): pick it up where it is now, and only once
        // that runs low ('waiting' / 'ended' bring it on at once)
        let d = duration();
        let p = position();
        let at = (if d > 0.0 { p.min(d - 1.0) } else { p }).max(0.0);
        REC.with(|r| r.borrow_mut().at = at);
        if a.ready_state() >= 3 && buffered_ahead() > 3.0 {
            REC.with(|r| r.borrow_mut().deferred = true);
            later(1000.0);
            return;
        }
    }
    let at = REC.with(|r| { let mut r = r.borrow_mut(); r.deferred = false; r.from = r.at; r.at });
    // the reload's own waiting is not a slow link
    st_mut(|s| s.seek_at = now_ms());
    let (transcoded, rung, dsp) = st(|s| (s.transcoded, s.rung, s.dsp));
    // in a group listen: where the group is by now (never a seek sent to the group)
    let at = group_spot(transcoded || rung > 0 || dsp).unwrap_or(at);
    load_opts(LoadOpts { offset: at, resume: true, transcode: transcoded, rung: Some(rung), dsp: Some(dsp), ..Default::default() });
}
/// Six failures in a row: move on, and say so.
fn give_up(t: &Rc<Value>) {
    reset_rec(None);
    st_mut(|s| s.want = false);
    let title = if t.truthy("title") { t.s("title") } else { "this song".to_string() };
    toast(&format!("Couldn't keep {title} playing – skipped"), "err");
    log_play();
    next(false);
}

/// Waiting (or a stalled download) that lasts more than 10 s while playing, with no progress and nothing more arriving: reload at the spot.
pub(super) fn watch_stall() {
    let Some(t) = cur() else { return };
    if STALL_WATCH.with(|w| w.borrow().is_some()) || t.truthy("preview") || audio().map(|a| a.paused()).unwrap_or(true) { return; }
    let at = position();
    let el = audio();
    let got = buffered_end();
    let timer = Timeout::new(10000, move || {
        if let Some(w) = STALL_WATCH.with(|w| w.borrow_mut().take()) { w.forget(); }
        let same = cur().is_some_and(|c| Rc::ptr_eq(&c, &t)) && audio() == el;
        let stuck = audio().is_some_and(|a| !a.paused() && a.ready_state() < 3);
        if !(same && st(|s| s.want) && stuck && (position() - at).abs() < 1.0) { return; }
        // still downloading, only slowly: starting it over would throw that away – keep waiting
        if buffered_end() > got + 0.1 { watch_stall(); return; }
        stall_retry_step(&t, at);
        retry(Some(at));
    });
    STALL_WATCH.with(|w| *w.borrow_mut() = Some(timer));
}
/// Auto, a reload because the stream stalled: a stall at 96 counts toward 64, and the reload asks for one rung down when
/// that has just become allowed or the song never got going at all.
fn stall_retry_step(t: &Rc<Value>, at: f64) {
    if t.truthy("preview") || prefs().s("quality") != "auto" { return; }
    let was = st(|s| s.floor);
    note_low_stall(now_ms());
    let (rung, floor) = st(|s| (s.rung, s.floor));
    if !(at < 1.0 || floor != was) { return; }
    let lower = playback::step_down(rung, orig_kbps(Some(t)), floor);
    if lower != rung && group_may_drop() { note_rung_change(rung, lower); st_mut(|s| { s.rung = lower; s.step_at = now_ms(); }); }
}
/// The page is visible again (phone unlocked, app back in front): music you wanted and nobody paused plays on. A pause the
/// browser made itself (headphones unplugged, another app took the sound) counts as yours: see the 'pause' listener.
pub(super) fn wake() {
    let visible = js_get(document().as_ref(), "visibilityState").as_string().as_deref() == Some("visible");
    if !visible || is_linked() || HELD.with(|h| h.get()) || cur().is_none() || !st(|s| s.want) { return; }
    let Some(a) = audio() else { return };
    if rec_need() { retry_now(); return; }
    if a.paused() && !a.ended() { resume_fx(); play_el(&a); }
}
