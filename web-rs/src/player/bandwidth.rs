//! Auto quality: the bandwidth estimate (how fast the buffer grows, a small probe when nothing downloads),
//! stalls, and stepping the stream down a rung and back up.

use super::*;

thread_local! {
    /// the bandwidth estimate (Auto quality)
    pub(super) static EST: RefCell<Est> = RefCell::new(Est::new());
    /// when the "Connection is slow" toast last showed (at most once a minute)
    static SLOW_TOAST_AT: Cell<f64> = const { Cell::new(0.0) };
}

pub(super) fn est_kbps() -> Option<f64> { EST.with(|e| e.borrow().kbps) }

/// The browser ran out of buffer mid-song. Give it the bandwidth back, and on Auto quality step the stream down.
pub(super) fn stalled() {
    let now = now_ms();
    let paused = audio().map(|a| a.paused()).unwrap_or(true);
    if now - st(|s| s.seek_at) < 3000.0 || position() < 1.0 || paused { return; }   // a seek, a switch or the start, not a slow link
    if st(|s| s.local_copy) { return; }   // a downloaded copy: the network has nothing to do with it
    st_mut(|s| { s.stall_at = now; s.stalls.push(now); });
    if !fully_buffered() { cancel_preload(); }
    if prefs().s("quality") != "auto" || cur().is_some_and(|t| t.truthy("preview")) { return; }
    note_low_stall(now);
    // stalling again within 90 s of stepping down: the new rung isn't small enough either
    let step_at = st(|s| s.step_at);
    if truthy_num(step_at) && now - step_at < 90000.0 { step_down_now(); return; }
    let recent = st(|s| s.stalls.iter().filter(|x| now - **x < 90000.0).count());
    if recent >= 2 { step_down_now(); return; }
    // one long stall counts too: still waiting four seconds later
    Timeout::new(4000, move || {
        let waiting = audio().is_some_and(|a| !a.paused() && a.ready_state() < 3);
        if waiting && st(|s| s.stall_at == now) { step_down_now(); }
    }).forget();
}
/// Auto, a stall at 96 kbps (mid-song, or a song that can't get going): 96 keeps stalling (3 times within 2 minutes) → only then may Auto go down to 64.
pub(super) fn note_low_stall(now: f64) {
    if prefs().s("quality") != "auto" || cur().is_some_and(|t| t.truthy("preview")) { return; }
    st_mut(|s| {
        if s.rung != playback::AUTO_FLOOR || s.floor <= playback::AUTO_LOW { return; }
        s.low_stalls.retain(|x| now - *x < 120000.0);
        s.low_stalls.push(now);
        if s.low_stalls.len() >= 3 { s.floor = playback::AUTO_LOW; s.low_stalls.clear(); }
    });
}
/// Auto: a stall forces one rung down right away, and the estimate drops to 60 % of what the rung needed.
pub(super) fn step_down_now() {
    let Some(t) = cur() else { return };
    if t.truthy("preview") || st(|s| s.local_copy) || prefs().s("quality") != "auto" { return; }
    st_mut(|s| s.slow_start = None);
    let orig = orig_kbps(Some(&t));
    let rung = st(|s| s.rung);
    let lower = playback::step_down(rung, orig, st(|s| s.floor));
    if lower == rung { return; }   // already at the floor
    EST.with(|e| {
        let mut e = e.borrow_mut();
        e.kbps = Some(0.6 * playback::needed(rung, orig));
        e.at = now_ms();
        e.fresh = false;
        e.up_since = 0.0;
    });
    st_mut(|s| { s.stalls.clear(); s.step_at = now_ms(); });
    switch_rung(lower);
}
/// Reload the song at the current position on another rung.
fn switch_rung(rung: i64) {
    if cur().is_none() { return; }
    let at = position();
    note_rung_change(st(|s| s.rung), rung);
    let now = now_ms();
    st_mut(|s| { s.switched_at = now; s.seek_at = now; });
    load_opts(LoadOpts { rung: Some(rung), offset: at, resume: true, transcode: rung > 0, ..Default::default() });
}
pub(super) fn note_rung_change(was: i64, now: i64) {
    if was == now { return; }
    if now > 0 && (was == 0 || now < was) {
        if now_ms() - SLOW_TOAST_AT.with(|c| c.get()) > 60000.0 {
            SLOW_TOAST_AT.with(|c| c.set(now_ms()));
            toast(&format!("Connection is slow – lowering quality to {now} kbps to keep playing"), "");
        }
    } else if now == 0 { toast("Connection recovered – back to the original file", "ok"); }
}

// ---- the bandwidth estimate (Auto quality) -----------------------------------------------------
// An exponentially weighted average of measured throughput in kbit/s: from how fast the playing song's buffer
// grows, from a small probe when nothing is downloading, and – until the first sample – the browser's own guess.
pub(super) struct Est { kbps: Option<f64>, at: f64, fresh: bool, up_since: f64, el: Option<HtmlMediaElement>, url: String, end: f64, wall: f64, probing: bool, probe_at: f64 }
impl Est {
    fn new() -> Self {
        let mut e = Est { kbps: None, at: 0.0, fresh: false, up_since: 0.0, el: None, url: String::new(), end: 0.0, wall: 0.0, probing: false, probe_at: 0.0 };
        // `navigator.connection?.downlink` (Mbit/s) – untyped, the Network Information API isn't in every browser
        let d = js_sys::Reflect::get(&dom::window().navigator(), &"connection".into()).ok()
            .filter(|c| c.is_object())
            .and_then(|c| js_sys::Reflect::get(&c, &"downlink".into()).ok())
            .and_then(|v| v.as_f64());
        if let Some(d) = d { if d > 0.0 { e.kbps = Some(d * 1000.0); e.at = now_ms(); } }
        e
    }
}
fn perf_now() -> f64 { dom::window().performance().map(|p| p.now()).unwrap_or_else(now_ms) }
fn add_sample(kbps: f64) {
    if !(kbps >= 0.0) || !kbps.is_finite() { return; }
    EST.with(|e| {
        let mut e = e.borrow_mut();
        e.kbps = Some(playback::ewma(e.kbps, kbps));
        e.at = now_ms();
        e.fresh = true;
    });
}
/// kbit/s of the stream the element is playing (None when the server, not the network, sets the pace).
fn stream_kbps() -> Option<f64> {
    let (rung, dsp) = st(|s| (s.rung, s.dsp));
    if rung > 0 { return Some(rung as f64); }
    if dsp { return None; }   // Cadenza renders at its own speed: its download rate says nothing about the connection
    let o = file_kbps(cur().as_deref());
    Some(if truthy_num(o) { o } else { 1411.0 })
}
/// Every 2 s: how far the buffer grew × the stream's bitrate ÷ the time it took. Returns whether a download is in progress.
fn sample_progress() -> bool {
    let Some(a) = audio() else { return false };
    let now = perf_now();
    let b = a.buffered();
    let end = if b.length() > 0 { b.end(b.length() - 1).unwrap_or(0.0) } else { 0.0 };
    let d = a.duration();
    let downloading = a.network_state() == 2 && !(d.is_finite() && d > 0.0 && end >= d - 0.5);
    let url = dom::data(&a, "url");
    let kbps = stream_kbps();
    let same = EST.with(|e| { let e = e.borrow(); e.el.as_ref().is_some_and(|x| same_el(x, &a)) && e.url == url });
    // only while the element is still downloading: a finished download would read as a slow link
    let kbps = kbps.filter(|k| truthy_num(*k));
    let Some(kbps) = kbps.filter(|_| same && downloading) else {
        EST.with(|e| { let mut e = e.borrow_mut(); e.el = Some(a.clone()); e.url = url; e.end = end; e.wall = now; });
        return downloading;
    };
    let (secs, grown) = EST.with(|e| {
        let mut e = e.borrow_mut();
        let r = ((now - e.wall) / 1000.0, end - e.end);
        e.end = end; e.wall = now;
        r
    });
    if secs < 0.25 || grown < 0.0 { return true; }
    if grown == 0.0 && buffered_ahead() > 30.0 { return true; }   // the browser holding back with a full buffer, not a slow link
    add_sample(grown * kbps / secs);
    true
}
/// Probe sizes: 128 KB, and when that arrives in under 0.25 s (too fast to time), 1 MB, then 4 MB.
const PROBE_SIZES: [f64; 3] = [131072.0, 1048576.0, 4194304.0];
/// Nothing downloading and the estimate is older than 20 s: time a piece of the current or next song.
fn probe() {
    let now = now_ms();
    let busy = EST.with(|e| { let e = e.borrow(); e.probing || now - e.at < 20000.0 || now - e.probe_at < 20000.0 });
    if busy { return; }
    let cands = st(|s| {
        let c = if s.index < 0 { None } else { s.queue.get(s.index as usize).cloned() };
        let n = if s.index + 1 < 0 { None } else { s.queue.get((s.index + 1) as usize).cloned() };
        [c, n]
    });
    let Some(t) = cands.into_iter().flatten().find(|x| !x.truthy("preview") && x.g("id").is_number() && !convert(x)) else { return };
    EST.with(|e| { let mut e = e.borrow_mut(); e.probing = true; e.probe_at = now; });
    let id = t.s("id");
    dom::spawn(async move {
        for (i, &size) in PROBE_SIZES.iter().enumerate() {
            let Some((bytes, secs)) = probe_once(&id, size).await else { break };
            if let Some(kbps) = playback::rate_of(bytes, secs) { add_sample(kbps); break; }
            if bytes < size || i == PROBE_SIZES.len() - 1 {
                // still under 0.25 s (the whole song, or 4 MB): "at least this fast" – it may lift the estimate, never lower it
                let at_least = bytes * 8.0 / 1000.0 / 0.25;
                if est_kbps().is_none_or(|k| k < at_least) { add_sample(at_least); }
                break;
            }
        }
        EST.with(|e| e.borrow_mut().probing = false);
    });
}

/// `fetch(stream, { headers: { Range: 'bytes=0-<size - 1>' }, credentials: 'same-origin', cache: 'no-store' })`, timed:
/// (bytes, seconds); None when offline or refused.
async fn probe_once(id: &str, size: f64) -> Option<(f64, f64)> {
    let init = web_sys::RequestInit::new();
    init.set_credentials(web_sys::RequestCredentials::SameOrigin);
    init.set_cache(web_sys::RequestCache::NoStore);
    let headers = web_sys::Headers::new().ok()?;
    headers.set("Range", &format!("bytes=0-{}", js_num(size - 1.0))).ok()?;
    init.set_headers(&headers);
    let t0 = perf_now();
    let r = js_await(dom::window().fetch_with_str_and_init(&format!("/api/library/tracks/{id}/stream"), &init).into()).await?;
    let r: web_sys::Response = r.dyn_into().ok()?;
    let body = js_get(r.as_ref(), "body");
    if r.status() != 206 && r.status() != 200 {
        if body.is_object() { if let Some(p) = js_call0(&body, "cancel") { let _ = js_await(p).await; } }
        return None;
    }
    if !body.is_object() { return None; }
    let reader = js_call0(&body, "getReader")?;
    let mut bytes = 0f64;
    while bytes < size {
        let Some(chunk) = js_call0(&reader, "read") else { break };
        let Some(res) = js_await(chunk).await else { break };
        if js_get(&res, "done").is_truthy() { break; }
        bytes += js_get(&js_get(&res, "value"), "byteLength").as_f64().unwrap_or(0.0);
    }
    if let Some(p) = js_call0(&reader, "cancel") { dom::spawn(async move { let _ = js_await(p).await; }); }
    Some((bytes.min(size), (perf_now() - t0) / 1000.0))
}
pub(super) fn adapt_tick() {
    if is_linked() || audio().is_none() { return; }
    let Some(t) = cur() else { return };
    if t.truthy("preview") || st(|s| s.local_copy) { return; }   // a downloaded copy uses no bandwidth
    let downloading = sample_progress();
    if audio().is_some_and(|a| a.paused()) { EST.with(|e| e.borrow_mut().up_since = 0.0); return; }
    if !downloading { probe(); }
    if prefs().s("quality") == "auto" { decide(); }
}
/// Auto: follow the estimate – down at once (when the buffer is short), up when it has held 1.5× the higher rung's need for 7.5 s.
fn decide() {
    let t = cur();
    let orig = orig_kbps(t.as_deref());
    let floor = st(|s| s.floor);
    let l = playback::ladder(orig, floor);
    let rung = st(|s| s.rung);
    let ci = l.iter().position(|&r| r == rung);
    let (fresh, est) = EST.with(|e| { let mut e = e.borrow_mut(); let f = e.fresh; e.fresh = false; (f, e.kbps) });
    let (Some(est), Some(ci)) = (est, ci) else { return };
    let now = now_ms();
    let target = playback::choose_rung(Some(est), orig, floor);
    let ti = l.iter().position(|&r| r == target).unwrap_or(0);
    if ti > ci {
        EST.with(|e| e.borrow_mut().up_since = 0.0);
        if fresh && now - st(|s| s.switched_at) > 10000.0 && !fully_buffered() && buffered_ahead() < 20.0 { switch_rung(l[ti]); }
        return;
    }
    let up = l.iter().copied().find(|&r| playback::needed(r, orig) * 1.5 <= est);
    let Some(up) = up.filter(|u| l.iter().position(|r| r == u).unwrap_or(0) < ci) else { EST.with(|e| e.borrow_mut().up_since = 0.0); return };
    let since = EST.with(|e| e.borrow().up_since);
    if !truthy_num(since) { EST.with(|e| e.borrow_mut().up_since = now); return; }
    // safe to reload: 10 s in hand, or a server-made stream the browser has finished fetching (it shows little of it as buffered)
    let safe = buffered_ahead() > 10.0 || (st(|s| s.transcoded) && audio().is_some_and(|a| a.network_state() != 2));
    if now - since >= 7500.0 && safe {
        EST.with(|e| e.borrow_mut().up_since = 0.0);
        switch_rung(up);
    }
}
