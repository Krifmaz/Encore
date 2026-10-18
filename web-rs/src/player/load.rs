//! Starting a song: which stream to ask the server for (the original file, a FLAC conversion, a lighter lossy
//! rung, a Cadenza upsample), loading it into the <audio> element and pre-buffering the next one on the spare,
//! seeking, play and pause, the volume.

use super::*;

thread_local! {
    /// what this browser says it can decode, by file format
    static PLAYABLE: RefCell<HashMap<String, bool>> = RefCell::new(HashMap::new());
    /// the lighter streams' codec: AAC where the browser decodes it, Opus elsewhere (Chromium builds, Firefox on Linux)
    static LOSSY: String = detect_lossy();
    /// GET /api/playback/dsp: is server-side Cadenza processing on, and may this account use it
    static DSP_INFO: RefCell<Option<Value>> = const { RefCell::new(None) };
    /// a processed stream failed this session: play originals until the page reloads
    static DSP_FAILED: Cell<bool> = const { Cell::new(false) };
    static DSP_TOASTED: Cell<bool> = const { Cell::new(false) };
    /// track id → what the server's pre-render cache says: 'cached' | 'rendering' | 'queued' | 'busy'
    static DSP_CACHE: RefCell<HashMap<i64, String>> = RefCell::new(HashMap::new());
    /// track id → when we last asked (prepare)
    static DSP_ASKED: RefCell<HashMap<i64, f64>> = RefCell::new(HashMap::new());
}

pub(super) fn lossy_codec() -> String { LOSSY.with(|l| l.clone()) }

fn detect_lossy() -> String {
    if can_play_type("audio/aac") { "aac".into() } else if can_play_type("audio/ogg; codecs=opus") { "opus".into() } else { "aac".into() }
}

pub(super) fn fetch_dsp() {
    dom::spawn(async {
        let i = api::get("/playback/dsp").await.ok();
        DSP_INFO.with(|d| *d.borrow_mut() = i);
    });
}
fn dsp_info() -> Value { DSP_INFO.with(|d| d.borrow().clone()).unwrap_or(Value::Null) }
fn dsp_key(p: &Value) -> Value { json!([p.g("dsp"), p.g("dsp_filter"), p.g("dsp_dither"), p.g("dsp_rate")]) }
/// New settings: a different Cadenza setup forgets what the render cache said; the effects follow at once.
pub(super) fn new_prefs(p: Value) {
    if dsp_key(&p) != dsp_key(&prefs()) {
        DSP_CACHE.with(|m| m.borrow_mut().clear());
        DSP_ASKED.with(|m| m.borrow_mut().clear());
    }
    PREFS.with(|x| *x.borrow_mut() = p);
    if playback::fx_active(&prefs()) { ensure_fx(); }
    apply_fx();
}
pub(super) fn dsp_state(id: i64) -> String { DSP_CACHE.with(|m| m.borrow().get(&id).cloned()).unwrap_or_default() }
fn dsp_asked(id: i64) -> f64 { DSP_ASKED.with(|m| m.borrow().get(&id).copied()).unwrap_or(0.0) }

// formats browsers can't decode → the server converts to FLAC; the player then seeks via the server
const CONVERT: [&str; 10] = ["ape", "wv", "aiff", "aif", "wma", "alac", "dsf", "dff", "tta", "mpc"];
// what this browser says it can decode, by file format (a Chromium build without AAC, Firefox on Linux, Safari without Ogg …)
fn mime_of(format: &str) -> Option<&'static str> {
    Some(match format {
        "mp3" => "audio/mpeg", "flac" => "audio/flac", "m4a" | "mp4" => r#"audio/mp4; codecs="mp4a.40.2""#, "aac" => "audio/aac",
        "ogg" | "oga" => "audio/ogg; codecs=vorbis", "opus" => "audio/ogg; codecs=opus", "wav" => "audio/wav", "webm" => "audio/webm",
        _ => return None,
    })
}
pub(crate) fn can_play(format: &str) -> bool {
    let f = format.to_lowercase();
    let Some(mime) = mime_of(&f) else { return false };
    PLAYABLE.with(|p| {
        let mut p = p.borrow_mut();
        if let Some(v) = p.get(&f) { return *v; }
        let v = can_play_type(mime);
        p.insert(f, v);
        v
    })
}

/// The rate asked of the server's Cadenza: this device's output rate, 0 (= the highest the server allows) or a fixed rate.
fn dsp_request_rate() -> f64 {
    let p = prefs();
    let r = p.g("dsp_rate");
    if r.as_str() == Some("device") { let o = output_rate().round(); return if truthy_num(o) { o } else { 0.0 }; }
    if r.as_str() == Some("auto") { return 0.0; }
    let n = p.f("dsp_rate");
    if truthy_num(n) { n } else { 0.0 }
}
/// The rate the server will send (its rule: at most max_rate, never below the file's; 0 → the highest multiple of the file's rate).
fn dsp_out_rate(t: &Value) -> f64 {
    let src = INFO.with(|m| m.borrow().get(&t.i("id")).map(|i| i.f("sample_rate"))).filter(|x| truthy_num(*x)).unwrap_or(44100.0);
    let max = { let m = dsp_info().f("max_rate"); if truthy_num(m) { m } else { 192000.0 } };
    let want = dsp_request_rate();
    let mut r = if want > 0.0 { want.min(max) } else { src };
    if want <= 0.0 { while r * 2.0 <= max { r *= 2.0; } }
    r.max(src)
}
/// Upsample this library song on the server? Only when switched on and allowed; Data saver and Auto's lighter streams win.
pub(super) fn want_dsp(t: &Value, lossy: bool) -> bool {
    prefs().truthy("dsp") && dsp_info().truthy("allowed") && !t.truthy("preview") && t.g("id").is_number() && !lossy && !DSP_FAILED.with(|f| f.get())
}
/// The server keeps finished Cadenza renders (pre-render cache): seeking and the next song need no new job.
pub(super) fn dsp_cache_on() -> bool { dsp_info().g("cache").f("max_mb") > 0.0 }
/// transcode: FLAC made by the server · rung: kbps of the lossy stream (0 = not lossy) · dsp: upsampled by the server's Cadenza · offset: where a server-made stream starts
pub(super) fn stream_url(t: &Value, transcode: bool, rung: i64, dsp: bool, offset: f64) -> String {
    if t.truthy("preview") { return t.s("preview_url"); }
    let mut q: Vec<String> = vec![];
    if rung > 0 { q.push(format!("q={}", lossy_codec())); q.push(format!("kbps={rung}")); }
    else if dsp {
        let p = prefs();
        q.push("dsp=1".into());
        q.push(format!("rate={}", js_num(dsp_request_rate())));
        if p.truthy("dsp_filter") { q.push(format!("filter={}", String::from(js_sys::encode_uri_component(&p.s("dsp_filter"))))); }
        q.push(format!("dither={}", String::from(js_sys::encode_uri_component(&p.s("dsp_dither")))));
    }
    else if transcode { q.push("transcode=1".into()); }
    if truthy_num(offset) { q.push(format!("t={}", to_fixed1(offset))); }
    format!("/api/library/tracks/{}/stream{}", t.s("id"), if q.is_empty() { String::new() } else { format!("?{}", q.join("&")) })
}
pub(super) fn convert(t: &Value) -> bool { !t.truthy("preview") && (CONVERT.contains(&t.s("format").to_lowercase().as_str()) || !can_play(&t.s("format"))) }
/// kbit/s of a Cadenza stream at `rate` (24-bit stereo FLAC, roughly 65 % of the raw PCM).
fn dsp_kbps(rate: f64) -> f64 { playback::js_round(rate * 48.0 * 0.65 / 1000.0) }
/// kbit/s the song needs as the "original" rung: the Cadenza stream's when the server upsamples it, else the file's.
pub(super) fn orig_kbps(t: Option<&Value>) -> f64 {
    let Some(t) = t else { return 0.0 };
    if t.truthy("preview") { return 0.0; }
    if want_dsp(t, false) { return dsp_kbps(dsp_out_rate(t)); }
    file_kbps(Some(t))
}
/// kbit/s of the file itself (0 = unknown, counted as 1411).
pub(super) fn file_kbps(t: Option<&Value>) -> f64 {
    let Some(t) = t else { return 0.0 };
    if t.truthy("preview") { return 0.0; }
    if t.f("bitrate") > 0.0 { return t.f("bitrate"); }
    let i = INFO.with(|m| m.borrow().get(&t.i("id")).map(|i| i.f("bitrate"))).unwrap_or(0.0);
    if t.g("id").is_number() && i > 0.0 { return i; }
    if t.f("size") > 0.0 && t.f("duration") > 0.0 { playback::js_round(t.f("size") * 8.0 / 1000.0 / t.f("duration")) } else { 0.0 }
}

/// Which rung a song starts on: Original never converts, Data saver is fixed at 96 kbps, Auto follows the estimated bandwidth.
fn rung_for(t: &Value) -> i64 {
    let q = prefs().s("quality");
    if t.truthy("preview") || q == "original" { return 0; }
    if q == "saver" { return playback::SAVER_KBPS; }
    playback::choose_rung(est_kbps(), orig_kbps(Some(t)), st(|s| s.floor))
}
/// The URL a song starts with (what the spare element pre-loads, and what load() asks for at a song start).
fn start_url(t: &Value) -> String {
    if use_local(t) { if let Some(u) = url_for(t.i("id")) { return u; } }
    let rung = rung_for(t);
    stream_url(t, convert(t) && rung == 0, rung, want_dsp(t, rung > 0), 0.0)
}

pub(super) fn load(transcode: bool, offset: f64, rung: Option<i64>, dsp: Option<bool>) {
    load_opts(LoadOpts { transcode, offset, rung, dsp, ..Default::default() });
}
/// rung: kbps of the lossy stream (None = what the song should start on) · dsp: None = whatever the settings say
/// resume: the same song continues (a quality switch or a seek) – no new play / now-playing · fade: a song transition into it
/// hold: load it paused (a restored spot) – it starts when you press play
/// from: where the new song starts in a transition (Smart overlap skips its silent lead-in)
#[derive(Default)]
pub(super) struct LoadOpts { pub(super) transcode: bool, pub(super) offset: f64, pub(super) rung: Option<i64>, pub(super) dsp: Option<bool>, pub(super) resume: bool, pub(super) fade: bool, pub(super) hold: bool, pub(super) from: f64 }
/// Start the current song (or restart it at `offset`).
pub(super) fn load_opts(o: LoadOpts) {
    let Some(t) = cur() else { return };
    // a song downloaded to this device plays from its copy (local.rs): its object URL first, then the load
    let local = use_local(&t);
    if local && url_for(t.i("id")).is_none() {
        let t2 = t.clone();
        prepare(t.i("id"), move || if cur().is_some_and(|c| Rc::ptr_eq(&c, &t2)) { load_opts(o); } else { sweep(); });
        return;
    }
    HELD.with(|h| h.set(o.hold));
    HELD_AT.with(|h| h.set(if o.hold { o.offset } else { 0.0 }));
    if o.hold { PLAYED_SINCE.with(|p| p.set(false)); } else if !o.resume { PLAYED_SINCE.with(|p| p.set(true)); }
    if !o.fade { end_fade(); }
    let offset = o.offset;
    let start = !truthy_num(offset) && !o.resume;
    // a reload of the same song (a seek, a quality switch, picking a broken-off stream up again) keeps its retry count
    if start || !rec_is(&t) { reset_rec(Some(t.clone())); }
    let old_timer = REC.with(|r| { let mut r = r.borrow_mut(); r.need = false; r.timer.take() });
    drop(old_timer);
    let old_watch = STALL_WATCH.with(|w| w.borrow_mut().take());
    drop(old_watch);
    let now = now_ms();
    st_mut(|s| { s.want = !o.hold; s.at = offset; s.src_at = now; });
    mark_played(&t);
    // the saved copy is a whole file: no conversion, no lighter rung, no Cadenza
    let transcode = !local && (o.transcode || convert(&t));
    let prev_rung = st(|s| s.rung);
    let rung = if t.truthy("preview") || local { 0 } else { o.rung.unwrap_or_else(|| rung_for(&t)) };
    st_mut(|s| { s.rung = rung; s.local_copy = local; });
    if start && !local && !t.truthy("preview") && prefs().s("quality") == "auto" { note_rung_change(prev_rung, rung); }
    // playing above 96 kbps (or the original) on Auto: the connection has recovered, 64 is off the table again
    if !local && !t.truthy("preview") && prefs().s("quality") == "auto" && (rung == 0 || rung > playback::AUTO_FLOOR) {
        st_mut(|s| { s.floor = playback::AUTO_FLOOR; s.low_stalls.clear(); });
    }
    let is_lossy = rung > 0;
    let is_dsp = !local && o.dsp.unwrap_or(true) && want_dsp(&t, is_lossy);
    let dsp_rate = if is_dsp { dsp_out_rate(&t) } else { 0.0 };
    st_mut(|s| { s.lossy = is_lossy; s.dsp = is_dsp; s.dsp_rate = dsp_rate; });
    if is_dsp && !INFO.with(|m| m.borrow().contains_key(&t.i("id"))) {
        let t2 = t.clone();
        dom::spawn(async move {
            track_info(&t2).await;
            if cur().is_some_and(|c| Rc::ptr_eq(&c, &t2)) && st(|s| s.dsp) {
                let r = dsp_out_rate(&t2);
                st_mut(|s| s.dsp_rate = r);
                render(); emit();
            }
        });
    }
    let transcoded = !t.truthy("preview") && (transcode || is_lossy || is_dsp);
    // server-made streams restart at the seek point (their clock starts at `offset`); the original file and a
    // pre-rendered Cadenza file are whole files the browser seeks in itself
    let server_offset = if transcoded && !(is_dsp && dsp_state(t.i("id")) == "cached") { offset } else { 0.0 };
    st_mut(|s| {
        s.transcoded = transcoded;
        s.offset = server_offset;
        s.dragging = false;
    });
    if start {
        st_mut(|s| { s.logged = false; s.stalls.clear(); s.step_at = 0.0; });
        if !o.hold { announce(&t); }
    }
    let url = if local { url_for(t.i("id")).unwrap_or_default() } else { stream_url(&t, transcoded && !is_lossy && !is_dsp, rung, is_dsp, server_offset) };
    let (Some(mut a), Some(sp)) = (audio(), spare()) else { return };
    let use_plain = t.truthy("preview") && fx_built();
    let swap = !truthy_num(offset) && !use_plain && dom::data_opt(&sp, "url").as_deref() == Some(url.as_str());
    // a transition needs the pre-buffered element; without it the old song stops and this one starts plainly
    if o.fade && !swap { end_fade(); }
    if swap {
        // already buffered in the background: swap players and start right away
        let old = a;
        a = sp;
        AUDIO.with(|x| *x.borrow_mut() = Some(a.clone()));
        // the preview element never pre-loads: the parked wrapped one takes the spare's place
        let next_spare = if is_plain(&old) { PARKED.with(|x| x.borrow_mut().take()).unwrap_or_else(|| old.clone()) } else { old.clone() };
        SPARE.with(|x| *x.borrow_mut() = Some(next_spare));
        if !o.fade { let _ = old.pause(); let _ = old.remove_attribute("src"); }
        let _ = old.remove_attribute("data-url");
        a.set_current_time(if o.fade && o.from > 0.0 && seekable_to(&a, o.from) { o.from } else { 0.0 });
        let g = if o.fade { FADE.with(|f| f.borrow().as_ref().map(|f| playback::transition_gains(&f.mode, 0.0, 0.0).1)).unwrap_or(1.0) } else { 1.0 };
        set_fade_gain(&a, g);
    } else {
        if use_plain != is_plain(&a) {
            // a preview with the effects graph built plays on the never-wrapped element; anything else on a wrapped one
            let old = a.clone();
            let _ = old.pause(); let _ = old.remove_attribute("src"); let _ = old.remove_attribute("data-url");
            let next = if use_plain { PARKED.with(|x| *x.borrow_mut() = Some(old.clone())); plain() } else { PARKED.with(|x| x.borrow_mut().take()) };
            if let Some(n) = next {
                a = n;
                AUDIO.with(|x| *x.borrow_mut() = Some(a.clone()));
                set_fade_gain(&a, 1.0);
            }
        }
        // without the pre-render cache a pre-buffered processed stream holds one of the server's Cadenza slots: free it first
        if is_dsp && !dsp_cache_on() && dom::data_opt(&sp, "url").is_some_and(|u| u.contains("dsp=1")) { cancel_preload(); }
        a.set_src(&url);
    }
    dom::set_data(&a, "url", &url);
    sweep();
    if truthy_num(offset) && !truthy_num(server_offset) {
        let el = a.clone();
        if el.ready_state() >= 1 { el.set_current_time(offset); }
        else { EventListener::once(&a, "loadedmetadata", move |_| el.set_current_time(offset)).forget(); }
    }
    if playback::fx_active(&prefs()) { ensure_fx(); }
    route();
    resume_fx();
    if !o.hold { play_el(&a); }
    st_mut(|s| s.slow_start = None);
    if start && !o.hold && !local && !t.truthy("preview") && prefs().s("quality") == "auto" {
        // can't even get going: six seconds in and nothing has played yet → one rung down
        let t2 = t.clone();
        let timer = Timeout::new(6000, move || {
            let same = cur().is_some_and(|c| Rc::ptr_eq(&c, &t2));
            let stuck = audio().is_some_and(|a| !a.paused() && a.current_time() < 0.5 && a.ready_state() < 3);
            if same && stuck { note_low_stall(now_ms()); step_down_now(); }
        });
        st_mut(|s| s.slow_start = Some(timer));
    }
    if start && is_dsp && dsp_cache_on() { prepare_ahead(&t); }
    if has_media_session() { set_media_metadata(&t); }
    preload_next_cover();
    dom::add_class(&body(), "has-player");
    render();
    emit();
    media_state();
    if !o.hold { save_spot(true); }
    if st(|s| s.full) { load_lyrics(); }
    top_up();
}
/// "now playing" on Last.fm / ListenBrainz – once per song start.
pub(super) fn announce(t: &Rc<Value>) {
    if no_server() { return; }
    if t.g("id").is_number() && !st(|s| s.announced.as_ref().is_some_and(|a| Rc::ptr_eq(a, t))) {
        st_mut(|s| s.announced = Some(t.clone()));
        let id = t.g("id").clone();
        dom::spawn(async move { let _ = api::post("/library/now-playing", json!({ "track_id": id })).await; });
    }
}

/// The server's Cadenza is off, busy or failed: the same spot again without processing, and no more processing this session.
pub(super) fn dsp_fallback() {
    DSP_FAILED.with(|f| f.set(true));
    let at = spot_position();
    if !DSP_TOASTED.with(|t| t.replace(true)) { toast("Server processing isn't available right now – playing the original", ""); }
    // a restored song stays paused until you press play
    load_opts(LoadOpts { transcode: at > 0.5, offset: if at > 0.5 { at } else { 0.0 }, dsp: Some(false), hold: HELD.with(|h| h.get()), ..Default::default() });
}

/// Start buffering the song that plays next – once this one is safely ahead, never while it's short of bandwidth.
pub(super) fn maybe_preload(force: bool) { maybe_preload_need(force, false); }
/// need: the transition window is open – fetch the next song even with pre-load off.
pub(super) fn maybe_preload_need(force: bool, need: bool) {
    let Some(sp) = spare() else { return };
    if (!prefs().truthy("preload") && !need) || is_linked() || fading() { return; }
    let n = st(|s| if s.index + 1 < 0 { None } else { s.queue.get((s.index + 1) as usize).cloned() });
    let Some(n) = n else { return };
    if n.truthy("preview") && fx_built() { return; }   // a preview plays on the never-wrapped element, not the spare
    if use_local(&n) && url_for(n.i("id")).is_none() { prepare(n.i("id"), move || maybe_preload_need(force, need)); return; }
    let url = start_url(&n);
    if dom::data_opt(&sp, "url").as_deref() == Some(url.as_str()) { return; }
    let t = cur();
    let remaining = duration() - position();
    let quiet = now_ms() - st(|s| s.stall_at) > 15000.0;
    if url.contains("dsp=1") && dsp_cache_on() && st(|s| s.dsp) {
        // a processed next song: ask the server to render it ahead, and buffer it only once that render exists
        // (cached or in progress) – then the pre-load takes no extra processing slot
        let id = n.i("id");
        let s = dsp_state(id);
        if s != "cached" && s != "rendering" {
            if now_ms() - dsp_asked(id) > 15000.0 {
                let n2 = n.clone();
                dom::spawn(async move { prepare_dsp(&n2).await; maybe_preload(force); });
            }
            return;
        }
        if !force && !(fully_buffered() || (quiet && buffered_ahead() > 45.0) || remaining < 40.0) { return; }
    } else if !force {
        let ok = if t.as_ref().is_some_and(|t| t.truthy("preview")) || st(|s| s.transcoded) { remaining < 40.0 }
            else { fully_buffered() || (quiet && buffered_ahead() > 45.0) || remaining < 30.0 };
        if !ok { return; }
    }
    dom::set_data(&sp, "url", &url);
    sp.set_src(&url);
    sp.load();
}
pub(super) fn preload_next() { maybe_preload(true); }
pub(super) fn cancel_preload() {
    let Some(sp) = spare() else { return };
    if dom::data_opt(&sp, "url").is_none() || fading() { return; }
    let _ = sp.remove_attribute("src");
    let _ = sp.remove_attribute("data-url");
    sp.load();
}

/// `maybePreload()` at most every two seconds, from timeupdate and progress.
pub(super) fn preload_tick() {
    let now = now_ms();
    if now - st(|s| s.last_preload_check) > 2000.0 { st_mut(|s| s.last_preload_check = now); maybe_preload(false); }
}

// ---- server-side Cadenza: render ahead -----------------------------------------------------------
/// Ask the server to render a song ahead (or say how far it is): 'cached' | 'rendering' | 'queued' | 'busy'.
pub(super) async fn prepare_dsp(t: &Value) -> Option<String> {
    if !t.g("id").is_number() { return None; }
    let id = t.i("id");
    DSP_ASKED.with(|m| m.borrow_mut().insert(id, now_ms()));
    let p = prefs();
    match api::post("/playback/dsp/prepare", json!({ "track_id": t.g("id"), "rate": playback::jnum(dsp_request_rate()), "filter": p.g("dsp_filter"), "dither": p.g("dsp_dither") })).await {
        Ok(r) => {
            let s = r.g("state").as_str().unwrap_or("").to_string();
            DSP_CACHE.with(|m| m.borrow_mut().insert(id, s.clone()));
            Some(s)
        }
        Err(_) => None,
    }
}
/// The current song started as a processed stream: learn whether it is pre-rendered, and have the next one rendered now.
fn prepare_ahead(t: &Value) {
    let t2 = t.clone();
    dom::spawn(async move { prepare_dsp(&t2).await; });
    let n = st(|s| if s.index + 1 < 0 { None } else { s.queue.get((s.index + 1) as usize).cloned() });
    if let Some(n) = n {
        if want_dsp(&n, rung_for(&n) > 0) {
            dom::spawn(async move { prepare_dsp(&n).await; maybe_preload(false); });
        }
    }
}
pub(super) fn dsp_note(s: &str) -> &'static str { match s { "cached" => " (pre-rendered)", "rendering" | "queued" => " (rendering ahead)", _ => "" } }

pub(super) fn duration() -> f64 {
    let t = cur();
    let ad = audio().map(|a| a.duration()).unwrap_or(f64::NAN);
    let ad_ok = truthy_num(ad) && ad.is_finite();
    if t.as_ref().is_some_and(|t| t.truthy("preview")) { return if ad_ok { ad } else { 30.0 }; }
    let td = t.map(|t| t.f("duration")).unwrap_or(0.0);
    if st(|s| s.transcoded) { return td; }
    if ad_ok { ad } else { td }
}
pub(super) fn position() -> f64 { st(|s| s.offset) + audio().map(|a| a.current_time()).unwrap_or(0.0) }

pub fn seek_to(sec: f64) {
    if via_link(json!({ "action": "seek", "value": sec.max(0.0) })) {
        LINK.with(|l| if let Some(l) = l.borrow_mut().as_mut() { if let Some(o) = l.state.as_object_mut() { o.insert("position".into(), json!(sec)); } });
        update_linked_time();
        return;
    }
    let dur = duration();
    if cur().is_none() || !truthy_num(dur) { return; }
    let sec = sec.min(dur - 0.25).max(0.0);
    if HELD.with(|h| h.get()) { HELD_AT.with(|h| h.set(sec)); }
    end_fade();
    st_mut(|s| { s.seek_at = now_ms(); s.at = sec; });
    REC.with(|r| { let mut r = r.borrow_mut(); if r.need { r.at = sec; } });
    let Some(t) = cur() else { return };
    let id = t.i("id");
    if st(|s| s.dsp) && dsp_cache_on() && dsp_state(id) != "cached" && now_ms() - dsp_asked(id) > 3000.0 {
        // maybe the render has finished since: then the browser seeks in the pre-rendered file itself
        dom::spawn(async move {
            prepare_dsp(&t).await;
            if cur().is_some_and(|c| Rc::ptr_eq(&c, &t)) { seek_now(sec); }
        });
    } else { seek_now(sec); }
    update_time();
    emit();
}
/// Whole files (the original, a pre-rendered Cadenza file) seek in the browser; server-made streams restart at the point.
fn seek_now(sec: f64) {
    let Some(a) = audio() else { return };
    let cached = st(|s| s.dsp) && cur().is_some_and(|t| dsp_state(t.i("id")) == "cached");
    let mut local = !st(|s| s.transcoded);
    if cached && st(|s| s.offset) == 0.0 {
        let s = a.seekable();
        local = s.length() > 0 && s.end(s.length() - 1).map(|e| e >= sec - 0.5).unwrap_or(false);
    }
    st_mut(|s| s.seek_at = now_ms());
    // a stream that broke off can't seek in itself: load it again at the new spot
    if local && !rec_need() { a.set_current_time(sec); }
    else {
        let (rung, dsp, transcoded) = st(|s| (s.rung, s.dsp, s.transcoded));
        load_opts(LoadOpts { transcode: transcoded, rung: Some(rung), dsp: Some(dsp), offset: sec, resume: true, ..Default::default() });
    }
    update_time();
    media_state();
}

pub fn toggle() {
    if via_link(json!({ "action": "toggle" })) {
        LINK.with(|l| if let Some(l) = l.borrow_mut().as_mut() {
            let p = !l.state.truthy("playing");
            if let Some(o) = l.state.as_object_mut() { o.insert("playing".into(), json!(p)); }
        });
        render();
        if st(|s| s.full) { render_full(); }
        return;
    }
    if cur().is_some() { if shows_playing() { user_pause(); } else { user_play(); } }
}
/// Play pressed (button, headset, lock screen, another device): a stream that broke off is loaded again where it was.
pub(super) fn user_play() {
    let (Some(a), Some(t)) = (audio(), cur()) else { return };
    st_mut(|s| s.want = true);
    resume_fx();
    if rec_need() && rec_is(&t) { retry_now(); } else { play_el(&a); }
    media_state();
}
/// A real pause: it stays paused – nothing picks the song up again until play is pressed.
pub(super) fn user_pause() {
    st_mut(|s| s.want = false);
    let t = REC.with(|r| r.borrow_mut().timer.take());
    drop(t);
    if let Some(a) = audio() { let _ = a.pause(); }
    media_state();
    render(); emit();
}
/// The play button shows pause while the music plays – and while a broken-off stream is being picked up again.
pub(super) fn shows_playing() -> bool {
    let Some(a) = audio() else { return false };
    !a.paused() || (st(|s| s.want) && rec_need() && cur().is_some_and(|t| rec_is(&t)))
}
/// play() – when the browser refuses (autoplay rules), say so: never sit silently paused with a pause button showing.
pub(super) fn play_el(a: &HtmlMediaElement) {
    let Ok(p) = a.play() else { return };
    let el = a.clone();
    dom::spawn(async move {
        if let Err(e) = wasm_bindgen_futures::JsFuture::from(p).await {
            let name = js_get(&e, "name").as_string().unwrap_or_default();
            if name == "NotAllowedError" && audio().is_some_and(|a| same_el(&a, &el)) { blocked(); }
        }
    });
}
fn blocked() {
    st_mut(|s| s.want = false);
    render(); emit(); media_state();
    toast("Tap play to keep listening", "");
}

pub(super) fn log_play() {
    let Some(t) = cur() else { return };
    if t.truthy("preview") || st(|s| s.logged) || !t.g("id").is_number() { return; }
    let p = position();
    let d = duration();
    // counts (and scrobbles) like Last.fm: half the song or 4 minutes, whichever comes first; very short songs at the end
    let hit = if truthy_num(d) { if d < 30.0 { p >= d - 1.0 } else { p >= (d / 2.0).min(240.0) } } else { p >= 30.0 };
    if hit {
        st_mut(|s| s.logged = true);
        // without the server the play waits in the outbox, with the time it was played
        if no_server() { play_later(t.i("id"), p); return; }
        let (id, tid) = (t.g("id").clone(), t.i("id"));
        dom::spawn(async move {
            match api::post("/library/plays", json!({ "track_id": id, "seconds": p })).await {
                Ok(_) => sync_outbox(),
                Err(e) if unreachable(e.status) => play_later(tid, p),
                Err(_) => {}
            }
        });
    }
}

pub(super) fn local_volume(v: f64) {
    let (Some(a), Some(sp)) = (audio(), spare()) else { return };
    let vol = if v.is_finite() { 0f64.max(1f64.min(v)) } else { 0.9 };
    USER_VOL.with(|u| u.set(vol));
    if fading() { step_fade(); } else { set_fade_gain(&a, 1.0); set_fade_gain(&sp, 1.0); }
    dom::ls_set("encore.volume", &js_num(vol));
    for id in ["vol", "fvol"] {
        if let Some(el) = by_id(id) { if num(&dom::value(&el)) != vol { dom::set_value(&el, &js_num(vol)); } }
    }
    emit();
}
pub fn set_volume(v: f64) {
    if via_link(json!({ "action": "volume", "value": v })) {
        LINK.with(|l| if let Some(l) = l.borrow_mut().as_mut() { if let Some(o) = l.state.as_object_mut() { o.insert("volume".into(), json!(v)); } });
        return;
    }
    local_volume(v);
}
