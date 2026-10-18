//! The lock screen and the OS media controls (Media Session API), and the native apps (`window.EncoreNative`,
//! `window.encorePlayer`): what's playing, their controls, songs kept offline on the phone.

use super::*;

thread_local! {
    static EXPOSED: Cell<bool> = const { Cell::new(false) };
    /// when the lock screen's position was last set
    pub(super) static POS_STATE_AT: Cell<f64> = const { Cell::new(0.0) };
}

/// `'mediaSession' in navigator`.
pub(super) fn has_media_session() -> bool { js_sys::Reflect::has(&dom::window().navigator(), &"mediaSession".into()).unwrap_or(false) }
/// `navigator.mediaSession` – untyped, because web-sys gates the Media Session API behind `web_sys_unstable_apis`.
fn media_session() -> Option<JsValue> {
    if !has_media_session() { return None; }
    js_sys::Reflect::get(&dom::window().navigator(), &"mediaSession".into()).ok().filter(|v| !v.is_undefined() && !v.is_null())
}

/// `navigator.mediaSession.metadata = new MediaMetadata({ title, artist, album, artwork })`.
pub(super) fn set_media_metadata(t: &Value) {
    let Some(ms) = media_session() else { return };
    let init = js_sys::Object::new();
    js_set(&init, "title", &crate::ui::tr_inline(t, "title").into());
    js_set(&init, "artist", &crate::ui::tr_inline(t, "artist").into());
    js_set(&init, "album", &crate::ui::tr_inline(t, "album").into());
    let artwork = js_sys::Array::new();
    let cov = cover_at(t, 600);
    if !cov.is_empty() {
        let img = js_sys::Object::new();
        js_set(&img, "src", &cov.into());
        js_set(&img, "sizes", &"600x600".into());
        artwork.push(&img);
    }
    js_set(&init, "artwork", &artwork);
    let Ok(ctor) = js_sys::Reflect::get(&js_sys::global(), &"MediaMetadata".into()).and_then(|c| c.dyn_into::<js_sys::Function>()) else { return };
    if let Ok(md) = js_sys::Reflect::construct(&ctor, &js_sys::Array::of1(&init)) { js_set(&ms, "metadata", &md); }
}

/// Lock screen / notification: playing or paused (what you want, also while a stream is being picked up) and where.
pub(super) fn media_state() {
    if audio().is_none() { return; }
    let Some(ms) = media_session() else { return };
    POS_STATE_AT.with(|c| c.set(now_ms()));
    let t = cur();
    let ps = if t.is_none() { "none" } else if shows_playing() { "playing" } else { "paused" };
    js_set(&ms, "playbackState", &ps.into());
    if t.is_none() { return; }
    let Ok(f) = js_get(&ms, "setPositionState").dyn_into::<js_sys::Function>() else { return };
    let d = duration();
    if !(d > 0.0) || !d.is_finite() { return; }
    let at = if rec_need() { REC.with(|r| r.borrow().at) } else { known_position() };
    let o = js_sys::Object::new();
    js_set(&o, "duration", &d.into());
    js_set(&o, "position", &at.min(d).max(0.0).into());
    js_set(&o, "playbackRate", &1.0.into());
    let _ = f.call1(&ms, &o);
}

/// `navigator.mediaSession.setActionHandler(action, f)` – called through the JS function so an unknown
/// action (older browsers) is caught like the JS `try { … } catch {}`.
pub(super) fn set_action(action: &str, f: impl Fn(JsValue) + 'static) {
    let Some(ms) = media_session() else { return };
    let Ok(func) = js_sys::Reflect::get(&ms, &"setActionHandler".into()).and_then(|f| f.dyn_into::<js_sys::Function>()) else { return };
    let c = Closure::<dyn Fn(JsValue)>::new(f);
    let _ = func.call2(&ms, &action.into(), c.as_ref());
    c.forget();
}
/// `d.seekTime` / `d.seekOffset` of a MediaSessionActionDetails.
pub(super) fn detail_num(d: &JsValue, key: &str) -> Option<f64> { js_sys::Reflect::get(d, &key.into()).ok().and_then(|v| v.as_f64()) }

/// Android / desktop apps: tell them what's playing (notification + lock screen + tray), and let them control us.
pub(super) fn native() {
    if !native::has_now_playing() { return; }
    if audio().is_none() { return; }
    let payload = match cur() {
        Some(t) => {
            let cov = cover_at(&t, 600);
            let art_url = if cov.is_empty() { String::new() } else { abs_url(&cov) };
            let dur = duration();
            let pos = position();
            let pos = if pos.is_finite() { pos.max(0.0) } else { 0.0 };
            let dur = if dur.is_finite() { dur } else { 0.0 };
            json!({ "title": t.s("title"), "artist": t.s("artist"), "album": t.s("album"), "playing": shows_playing(), "art": art_url,
                "position": pos, "duration": dur })
        }
        None => json!({ "stopped": true }),
    };
    native::now_playing(&payload);
}
/// `window.encorePlayer = { toggle, next, prev, open, seek, volumeBy }` (seek and volumeBy: the desktop app's mini
/// player, OS media controls and volume shortcuts).
pub(super) fn expose_global() {
    if EXPOSED.with(|e| e.replace(true)) { return; }
    let obj = js_sys::Object::new();
    let put = |name: &str, f: Box<dyn Fn()>| {
        let c = Closure::<dyn Fn()>::new(f);
        let _ = js_sys::Reflect::set(&obj, &name.into(), c.as_ref());
        c.forget();
    };
    put("toggle", Box::new(toggle));
    put("next", Box::new(|| next(false)));
    put("prev", Box::new(prev));
    put("open", Box::new(|| set_full(true)));
    let put1 = |name: &str, f: Box<dyn Fn(f64)>| {
        let c = Closure::<dyn Fn(JsValue)>::new(move |v: JsValue| f(js_sys::Number::from(v).value_of()));
        let _ = js_sys::Reflect::set(&obj, &name.into(), c.as_ref());
        c.forget();
    };
    put1("seek", Box::new(|sec| seek_to(if sec.is_nan() { 0.0 } else { sec.max(0.0) })));
    put1("volumeBy", Box::new(|d| {
        let base = LINK.with(|l| l.borrow().as_ref().map(|l| l.state.g("volume").as_f64().unwrap_or(1.0))).unwrap_or_else(|| USER_VOL.with(|u| u.get()));
        set_volume((base + if d.is_nan() { 0.0 } else { d }).clamp(0.0, 1.0));
    }));
    let _ = js_sys::Reflect::set(&dom::window(), &"encorePlayer".into(), &obj);
}

/// Android app: keep songs on the phone so they play with no server.
pub fn can_save_offline() -> bool { native::can_save_offline() }
pub fn save_offline(tracks: Vec<Value>) {
    for t in &tracks {
        if !t.g("id").is_number() { continue; }
        let conv = convert(t);
        let id = t.s("id");
        let ext = if conv { "flac".to_string() } else if t.truthy("format") { t.s("format") } else { "audio".to_string() };
        let url = abs_url(&format!("/api/library/tracks/{id}/stream{}", if conv { "?transcode=true" } else { "" }));
        let art_url = if t.truthy("album_id") { abs_url(&cover_at(t, 600)) } else { String::new() };
        native::save_offline(&json!({
            "id": id, "title": t.s("title"), "artist": t.s("artist"), "album": t.s("album"), "ext": ext,
            "url": url,
            "art": art_url,
        }));
    }
}
