//! The guest page (client-shapes §3.19): an invite link `#/group/<code>` opened by someone who is not signed in,
//! while the server lets people without an account join (`group_listen_guests`) and the host switched "Guests can
//! join with the link" on. It asks "Your name" (2–32 characters), joins with `POST /api/group/guest-join` and plays
//! the group's songs on its own <audio>: the token comes back as the `encore_guest` cookie (only sent to
//! /api/group), so the event stream, the songs (`/api/group/{id}/tracks/{track}/stream`) and the covers
//! (`/api/group/{id}/albums/{album}/cover`) need nothing else. Guests listen, see the queue and the people, and can
//! leave; they can't add songs or control playback (a pause pauses only here: "Paused here · Catch up"). Every name
//! on the page is escaped: guests choose their own.
//!
//! When guests can't join (or the code is wrong; the server doesn't say which), the sign-in page shows with the
//! reason, and the link is asked about again once signed in.

use super::rules::{self, SeekLag};
use crate::api;
use crate::dom::{self, EventListener};
use crate::json::J;
use crate::ui::{self, esc, fmt, icon_attr};
use serde_json::{json, Value};
use std::cell::RefCell;
use wasm_bindgen::JsCast;
use web_sys::{Element, HtmlMediaElement};

/// The name a guest last joined with, offered again.
const NAME_KEY: &str = "encore.group.guestName";

/// The page while listening: the group, its <audio>, the event stream, the clock and the sync's own state.
struct Guest {
    root: Element,
    code: String,
    state: Value,
    audio: HtmlMediaElement,
    /// the queue entry loaded on the <audio>, and where its stream starts (a server-made stream starts at `t=`)
    loaded: String,
    offset: f64,
    server_made: bool,
    local_paused: bool,
    adjusting: bool,
    quiet_until: f64,
    /// when this page last paused the <audio> itself (its 'pause' event is not the listener's)
    own_pause_at: f64,
    lag: SeekLag,
    es: Option<web_sys::EventSource>,
    keep: Vec<EventListener>,
    /// the half-second look at the drift (dropping it stops it)
    _tick: Option<dom::Interval>,
    drawn: String,
}

thread_local! {
    static G: RefCell<Option<Guest>> = const { RefCell::new(None) };
}

fn mono() -> f64 { dom::window().performance().map(|p| p.now()).unwrap_or_else(dom::now_ms) }
fn enc(s: &str) -> String { String::from(js_sys::encode_uri_component(s)) }
fn with<R>(f: impl FnOnce(&mut Guest) -> R) -> Option<R> { G.with(|g| g.borrow_mut().as_mut().map(f)) }

fn shell(root: &Element, inner: &str) {
    let info = crate::app::info();
    let name = { let n = info.s("app_name"); if n.is_empty() { "Encore".to_string() } else { n } };
    dom::set_html(root, &format!(r#"<div class="auth-wrap guest-wrap"><div class="auth-card guest-card"><div class="brand">{}{}</div><div id="guest-body">{inner}</div></div></div>"#,
        crate::appearance::logo(30), esc(&name)));
}

/// `#/group/<code>` signed out.
pub async fn render(root: Element, code: String) {
    let code = rules::code_from(&format!("#/group/{code}")).unwrap_or(code);
    // back after a reload: this browser's token (the cookie) still names a member of that group
    if let Ok(s) = api::get("/group/current").await {
        if s.truthy("id") && s.s("code") == code { listen(root, code, s, true).await; return; }
    }
    ask(root, code, None).await;
}

/// "Join <host>'s group listen?" with "Your name", or the sign-in page when a guest can't join.
async fn ask(root: Element, code: String, notice: Option<String>) {
    let g = match super::lookup(&code).await {
        Ok(g) if g.truthy("guests") => g,
        Ok(_) => return sign_in(root, &code, "Only people with an account on this server can join this group listen", notice),
        Err(e) => {
            let why = match e.status {
                403 | 429 if !e.message.is_empty() => e.message,
                // after an ending the notice says what happened: the code answering 404 now only means the group is
                // gone (or takes no guests any more), not that this server never lets guests in
                404 if notice.is_some() => String::new(),
                _ => "Only people with an account on this server can join this group listen".to_string(),
            };
            return sign_in(root, &code, &why, notice);
        }
    };
    let playing = rules::playing_now(&g);
    shell(&root, &format!(r##"{notice}<h2 style="margin:6px 0 4px;text-align:center">{title}</h2>
      {playing}
      <form id="g-guest" autocomplete="on">
        <label class="field"><span>Your name</span><input class="input" name="name" autocomplete="nickname" minlength="2" maxlength="32" required value="{name}"></label>
        <div class="error-box hidden" id="g-err"></div>
        <button class="btn primary" type="submit" style="padding:12px">Join</button>
      </form>
      <p class="small muted" style="text-align:center;margin:16px 0 0"><a href="#/login" id="g-signin" style="color:var(--accent)">Sign in</a></p>"##,
        notice = notice.map(|n| format!(r#"<div class="test-result"><span>{}</span></div>"#, esc(&n))).unwrap_or_default(),
        title = esc(&rules::join_title(Some(&g))),
        playing = if playing.is_empty() { String::new() } else { format!(r#"<p class="muted" style="text-align:center;margin:0">{}</p>"#, esc(&playing)) },
        name = esc(&dom::ls_get(NAME_KEY).unwrap_or_default())));
    let code2 = code.clone();
    dom::on_sel(&root, "#g-signin", "click", move |_| remember(&code2));
    let Some(form) = dom::qs_in(&root, "#g-guest") else { return };
    let (f, root2) = (form.clone(), root.clone());
    dom::keep(dom::listen(&form, "submit", move |e| {
        dom::prevent(e);
        let name = dom::value_of(&f, "[name=name]").split_whitespace().collect::<Vec<_>>().join(" ");
        let (Some(btn), Some(err)) = (dom::qs_in(&f, "button[type=submit]"), dom::qs_in(&f, "#g-err")) else { return };
        dom::add_class(&err, "hidden");
        dom::set_disabled(&btn, true);
        dom::set_text(&btn, "Joining…");
        let (root, code) = (root2.clone(), code.clone());
        dom::spawn(async move {
            super::clock().await;
            match api::post("/group/guest-join", json!({ "code": code, "name": name })).await {
                Ok(s) if s.truthy("id") => {
                    dom::ls_set(NAME_KEY, &name);
                    listen(root, code, s, false).await;
                }
                Ok(_) => { dom::set_disabled(&btn, false); dom::set_text(&btn, "Join"); }
                Err(e) => {
                    dom::set_disabled(&btn, false);
                    dom::set_text(&btn, "Join");
                    dom::set_text(&err, if e.message.is_empty() { "Couldn't join" } else { &e.message });
                    dom::remove_class(&err, "hidden");
                }
            }
        });
    }));
}

/// The link is asked about again once signed in (app boot → group::start).
fn remember(code: &str) {
    if let Some(s) = dom::window().session_storage().ok().flatten() { let _ = s.set_item(super::PENDING, code); }
}

/// The sign-in page, with why the link needs an account.
fn sign_in(root: Element, code: &str, why: &str, notice: Option<String>) {
    remember(code);
    let why = sign_in_text(why, notice);
    dom::spawn(async move {
        crate::pages::auth::render(root.clone(), "/login".into(), Default::default()).await;
        if let Some(err) = dom::qs_in(&root, "#auth-err") { dom::set_text(&err, &why); dom::remove_class(&err, "hidden"); }
    });
}

/// The sign-in page's line: what happened (the notice of an ending, when there is one) and why a guest can't join;
/// one of them alone when the other is empty or says the same, else the two as two sentences.
fn sign_in_text(why: &str, notice: Option<String>) -> String {
    match notice {
        Some(n) if why.is_empty() || n == why => n,
        Some(n) => format!("{n}. {why}"),
        None => why.to_string(),
    }
}

// ---- listening ---------------------------------------------------------------------------------------------------

/// In the group as a guest: the page plays the group's songs. `quiet`: back after a reload – paused here until
/// Catch up (a browser needs a tap before it plays sound).
async fn listen(root: Element, code: String, s: Value, quiet: bool) {
    stop();
    if rules::host_name(&s).is_empty() { return; }
    let audio: HtmlMediaElement = match dom::by_id("guest-audio").and_then(|e| e.dyn_into().ok()) {
        Some(a) => a,
        None => {
            let a = dom::create("audio");
            a.set_id("guest-audio");
            let _ = dom::body().append_child(&a);
            a.dyn_into().expect("audio")
        }
    };
    audio.set_preload("auto");
    let vol = js_sys::parse_float(&dom::ls_get("encore.volume").unwrap_or_else(|| "0.9".into()));
    audio.set_volume(if vol.is_finite() { vol.clamp(0.0, 1.0) } else { 0.9 });
    shell(&root, "");
    let mut keep = vec![];
    // the song reached its end here: on to the next entry, as the server does at the same moment
    keep.push(dom::listener(&audio, "ended", |_| { roll_over(); draw(); }));
    keep.push(dom::listener(&audio, "timeupdate", |_| paint_time()));
    // a pause nobody here asked for (headphones out, another app took the sound): only here
    keep.push(dom::listener(&audio, "pause", |_| {
        let mine = with(|g| {
            if g.audio.ended() || mono() - g.own_pause_at < 1000.0 { return true; }
            if g.state.truthy("playing") { g.local_paused = true; }
            false
        }).unwrap_or(true);
        if !mine { draw(); }
    }));
    let g = Guest { root, code, state: s, audio, loaded: String::new(), offset: 0.0, server_made: false, local_paused: quiet, adjusting: false,
        quiet_until: 0.0, own_pause_at: f64::NEG_INFINITY, lag: SeekLag::default(), es: None, keep, _tick: Some(dom::Interval::new(500, check)), drawn: String::new() };
    let id = g.state.s("id");
    G.with(|x| *x.borrow_mut() = Some(g));
    open_events(&id);
    draw();
    follow();
}

/// Out of the group: the audio stops, the stream closes.
fn stop() {
    if let Some(g) = G.with(|x| x.borrow_mut().take()) {
        if let Some(es) = &g.es { es.close(); }
        let _ = g.audio.pause();
        let _ = g.audio.remove_attribute("src");
        g.audio.load();
    }
}

/// Out with a message, then the join form again (or the sign-in page when a guest can't join any more).
fn ended(message: String) {
    let Some((root, code)) = with(|g| (g.root.clone(), g.code.clone())) else { return };
    stop();
    dom::spawn(ask(root, code, Some(message)));
}

fn open_events(id: &str) {
    let Ok(es) = web_sys::EventSource::new(&format!("/api/group/{}/events", enc(id))) else { return };
    let data = |e: &web_sys::Event| -> Option<Value> {
        serde_json::from_str::<Value>(&e.dyn_ref::<web_sys::MessageEvent>()?.data().as_string()?).ok()
    };
    let mut ls = vec![];
    let gid = id.to_string();
    ls.push(dom::listener(&es, "state", move |e| {
        let Some(v) = data(e) else { return };
        let fresh = with(|g| if v.s("id") == gid && v.i("rev") >= g.state.i("rev") {
            for t in rules::comings(&g.state, &v) { ui::toast(&t, ""); }
            g.state = v.clone();
            true
        } else { false }).unwrap_or(false);
        if fresh { draw(); follow(); }
    }));
    ls.push(dom::listener(&es, "ended", move |e| {
        let reason = data(e).map(|v| v.s("reason")).unwrap_or_default();
        let host = with(|g| rules::host_name(&g.state)).unwrap_or_default();
        ended(rules::ended_text(&reason, &host, false));
    }));
    let gid = id.to_string();
    ls.push(dom::listener(&es, "error", move |e| {
        let closed = e.target().and_then(|t| t.dyn_into::<web_sys::EventSource>().ok()).is_some_and(|es| es.ready_state() == 2);
        if !closed { return; }
        let gid = gid.clone();
        // refused: still a member (open it again), or not (the token stopped working: dropped, or the group is gone)
        dom::Timeout::new(1500, move || dom::spawn(async move {
            match api::get("/group/current").await {
                Ok(s) if s.s("id") == gid => { with(|g| g.state = s); open_events(&gid); }
                Ok(_) | Err(api::ApiError { status: 401 | 403 | 404, .. }) => ended("The group listen ended".into()),
                Err(_) => open_events(&gid),
            }
        })).forget();
    }));
    with(move |g| { if let Some(old) = g.es.replace(es) { old.close(); } g.keep.extend(ls); });
}

// ---- playing with the group -----------------------------------------------------------------------------------------

/// Where this page is in the song (a server-made stream's offset included).
fn position(g: &Guest) -> f64 { g.offset + g.audio.current_time() }

/// The stream of entry `e` at `at` seconds: the original file where this browser plays its format, else FLAC made
/// by the server (asked for at `t=`, as it can't seek in itself).
fn load(g: &mut Guest, e: &Value, at: f64, play: bool) {
    let id = g.state.s("id");
    let convert = !crate::player::can_play(&e.s("format"));
    let mut url = format!("/api/group/{}/tracks/{}/stream", enc(&id), e.s("id"));
    g.server_made = convert;
    g.offset = 0.0;
    if convert {
        url.push_str("?transcode=1");
        if at > 0.5 { url.push_str(&format!("&t={}", crate::num::to_fixed1(at))); g.offset = at; }
    }
    g.loaded = e.s("qid");
    g.audio.set_src(&url);
    if !convert && at > 0.3 {
        let a = g.audio.clone();
        EventListener::once(&g.audio, "loadedmetadata", move |_| a.set_current_time(at)).forget();
    }
    g.adjusting = false;
    g.audio.set_playback_rate(1.0);
    g.quiet_until = mono() + 2500.0;
    if play { play_now(&g.audio); }
}

/// A pause this page makes (never counted as the listener's own).
fn own_pause(g: &mut Guest) {
    if !g.audio.paused() { g.own_pause_at = mono(); let _ = g.audio.pause(); }
}

/// The song ended here: on to the next entry, as the server does at the same moment (no gap while it says so).
fn roll_over() {
    with(|g| {
        let q = g.state.a("queue");
        let Some(next) = q.iter().position(|e| e.s("qid") == g.loaded).and_then(|i| q.get(i + 1)).cloned() else { return };
        let want = g.state.truthy("playing") && !g.local_paused;
        load(g, &next, 0.0, want);
    });
}

/// A song change a moment before (or after) the group's: the page is on the next entry at its start, or the
/// group just started the next one while this song still plays its last moment. It settles by itself.
fn near_boundary(q: &[Value], loaded: &str, pos: f64, ei: i64, epos: f64) -> bool {
    let Some(li) = q.iter().position(|e| e.s("qid") == loaded) else { return false };
    let li = li as i64;
    (li == ei + 1 && pos < 1.5) || (li + 1 == ei && epos < 1.5)
}

/// Within the song: to `at`, playing or not.
fn place(g: &mut Guest, e: &Value, at: f64, play: bool) {
    if !play { own_pause(g); }
    if g.server_made || g.audio.ready_state() < 1 { load(g, e, at, play); return; }
    g.audio.set_current_time(at);
    g.quiet_until = mono() + 2500.0;
    if play && g.audio.paused() { play_now(&g.audio); }
}

/// play() – when the browser refuses (it wants a tap first), paused here with Catch up.
fn play_now(a: &HtmlMediaElement) {
    let Ok(p) = a.play() else { return };
    dom::spawn(async move {
        if wasm_bindgen_futures::JsFuture::from(p).await.is_err() {
            with(|g| g.local_paused = true);
            draw();
        }
    });
}

fn wants_play(s: &Value, i: i64, pos: f64, local_paused: bool) -> bool { s.truthy("playing") && !local_paused && !rules::at_end(s, i, pos) }

/// The page follows the group: its song, position and play / pause.
fn follow() {
    let changed = with(|g| {
        let (ei, epos) = rules::expected(&g.state, super::server_now());
        let q = g.state.a("queue");
        let Some(e) = q.get(ei.max(0) as usize).filter(|_| ei >= 0).cloned() else {
            own_pause(g);
            return false;
        };
        let want = wants_play(&g.state, ei, epos, g.local_paused);
        if g.loaded != e.s("qid") {
            if want && !g.audio.paused() && near_boundary(&q, &g.loaded, position(g), ei, epos) { return false; }
            let at = if want { g.lag.target(&e.s("qid"), epos * 1000.0, !crate::player::can_play(&e.s("format"))) / 1000.0 } else { epos };
            load(g, &e, at, want);
            return true;
        }
        if want && g.audio.paused() {
            if (position(g) - epos).abs() > 0.5 { let at = g.lag.target(&e.s("qid"), epos * 1000.0, g.server_made) / 1000.0; place(g, &e, at, true); } else { play_now(&g.audio); }
        } else if !want && !g.audio.paused() {
            own_pause(g);
        }
        if !g.state.truthy("playing") && (position(g) - epos).abs() > 0.5 && !g.audio.seeking() { place(g, &e, epos, false); }
        false
    }).unwrap_or(false);
    if changed { draw(); }
}

/// Every half second: the song, the drift, the rate.
fn check() {
    let redraw = with(|g| {
        let (ei, epos) = rules::expected(&g.state, super::server_now());
        let q = g.state.a("queue");
        let Some(e) = q.get(ei.max(0) as usize).filter(|_| ei >= 0).cloned() else { return false };
        if !wants_play(&g.state, ei, epos, g.local_paused) {
            if g.adjusting { g.adjusting = false; g.audio.set_playback_rate(1.0); }
            g.lag.cancel();
            return !g.audio.paused() || g.loaded != e.s("qid");
        }
        if g.loaded != e.s("qid") {
            // gapless into the next song a little before (or after) the group: wait for the song change to settle
            return g.audio.paused() || mono() >= g.quiet_until && !near_boundary(&q, &g.loaded, position(g), ei, epos);
        }
        if g.audio.paused() { return !g.audio.ended(); }
        if mono() < g.quiet_until || g.audio.ready_state() < 3 || g.audio.seeking() { return false; }
        let d = (position(g) - epos) * 1000.0;
        let song = e.s("qid");
        if g.lag.waiting() { g.lag.landed(&song, d, mono()); }
        match rules::drift(d, g.adjusting, rate_ok(&g.audio), g.lag.seek_above_ms(&song)) {
            rules::Drift::Seek => {
                g.adjusting = false;
                g.audio.set_playback_rate(1.0);
                let at = g.lag.target(&song, epos * 1000.0, g.server_made) / 1000.0;
                place(g, &e, at, true);
            }
            rules::Drift::Faster => { g.adjusting = true; g.audio.set_playback_rate(1.03); }
            rules::Drift::Slower => { g.adjusting = true; g.audio.set_playback_rate(0.97); }
            rules::Drift::Normal => { g.adjusting = false; g.audio.set_playback_rate(1.0); }
            rules::Drift::None => {}
        }
        false
    }).unwrap_or(false);
    if redraw { follow(); }
    paint_time();
}

/// Whether the browser changes the rate without changing the pitch.
fn rate_ok(a: &HtmlMediaElement) -> bool {
    for k in ["preservesPitch", "mozPreservesPitch", "webkitPreservesPitch"] {
        if js_sys::Reflect::has(a, &k.into()).unwrap_or(false) {
            let _ = js_sys::Reflect::set(a, &k.into(), &wasm_bindgen::JsValue::TRUE);
            return js_sys::Reflect::get(a, &k.into()).ok().and_then(|v| v.as_bool()) == Some(true);
        }
    }
    false
}

// ---- the page --------------------------------------------------------------------------------------------------

fn cover(id: &str, t: &Value, size: u32) -> String {
    if !t.truthy("album_id") { return ui::art("", &t.s("title"), ""); }
    let v = t.s("cover_v");
    ui::art(&format!("/api/group/{}/albums/{}/cover?s={size}{}", enc(id), enc(&t.s("album_id")), if v.is_empty() { String::new() } else { format!("&v={}", enc(&v)) }), &t.s("title"), "")
}

/// The page for the group as it is (drawn again only when what it shows changed).
fn draw() {
    let Some((body, html)) = with(|g| {
        let s = &g.state;
        let id = s.s("id");
        let (ei, _) = rules::expected(s, super::server_now());
        let q = s.a("queue");
        let t = q.get(ei.max(0) as usize).filter(|_| ei >= 0).cloned().unwrap_or(Value::Null);
        let you = s.g("you").s("member");
        let people: String = s.a("members").iter().map(|m| format!(
            r#"<div class="list-item"><div class="avatar" style="width:30px;height:30px;font-size:13px">{}</div><div class="grow" style="min-width:0"><div class="row" style="gap:6px;min-width:0"><b class="ellipsis">{}</b>{}{}{}</div></div></div>"#,
            esc(&ui::initials(&m.s("name"))), esc(&m.s("name")),
            if m.truthy("host") { r#"<span class="badge accent">Host</span>"# } else { "" },
            if m.truthy("guest") { r#"<span class="badge">Guest</span>"# } else { "" },
            if m.s("member") == you { r#"<span class="badge">You</span>"# } else { "" })).collect();
        let next: String = q.iter().skip((ei + 1).max(0) as usize).take(100).map(|x| {
            let by = x.g("added_by").s("name");
            format!(r#"<div class="list-item"><div class="thumb">{}</div><div class="grow" style="min-width:0"><div class="ellipsis" style="font-weight:600">{}</div><div class="small muted ellipsis">{}</div>{}</div></div>"#,
                cover(&id, x, 200), esc(&x.s("title")), esc(&x.s("artist")),
                if by.is_empty() { String::new() } else { format!(r#"<div class="small faint ellipsis">added by {}</div>"#, esc(&by)) })
        }).collect();
        let paused = if g.local_paused || (s.truthy("playing") && g.audio.paused() && !g.loaded.is_empty() && mono() > g.quiet_until) {
            r#"<div class="row between card flat" style="padding:10px 12px;margin-top:12px"><span>Paused here</span><button class="btn sm primary" id="g-catchup">Catch up</button></div>"#.to_string()
        } else { String::new() };
        let now = if t.is_null() {
            r#"<p class="muted" style="text-align:center">Nothing queued.</p>"#.to_string()
        } else {
            format!(r#"<div class="guest-art">{art}</div>
              <h2 class="ellipsis" style="margin:12px 0 2px;text-align:center">{title}</h2>
              <div class="muted ellipsis" style="text-align:center">{artist}</div>
              <div class="seek guest-seek"><span id="g-cur">0:00</span><div class="guest-line"><i id="g-line"></i></div><span id="g-dur">{dur}</span></div>"#,
                art = cover(&id, &t, 400), title = esc(&t.s("title")), artist = esc(&t.s("artist")), dur = fmt::dur(t.f("duration")))
        };
        let html = format!(r#"<div class="row between" style="gap:10px;margin:4px 0 14px"><b class="ellipsis">{group}</b><button class="btn sm" id="g-leave">Leave</button></div>
          {now}
          {paused}
          <div class="row" style="gap:8px;margin-top:12px"><span class="faint">{vol_ic}</span><input type="range" id="g-vol" min="0" max="1" step="0.01" value="{vol}" style="flex:1"></div>
          <h3 style="margin:18px 0 6px">People ({n})</h3><div class="list">{people}</div>
          <h3 style="margin:18px 0 6px">Up next</h3><div class="list">{next}</div>"#,
            group = esc(&format!("{}'s group listen", rules::host_name(s))), vol_ic = icon_attr("volume", r#"width="18""#),
            vol = crate::num::js_num(g.audio.volume()), n = s.a("members").len(),
            next = if next.is_empty() { r#"<p class="small muted" style="margin:0">Nothing queued.</p>"#.to_string() } else { next });
        if g.drawn == html { return None; }
        g.drawn = html.clone();
        dom::qs_in(&g.root, "#guest-body").map(|b| (b, html))
    }).flatten() else { return };
    dom::set_html(&body, &html);
    dom::on_sel(&body, "#g-leave", "click", |_| leave());
    dom::on_sel(&body, "#g-catchup", "click", |_| {
        with(|g| { g.local_paused = false; g.quiet_until = 0.0; });
        follow();
        draw();
    });
    dom::on_sel(&body, "#g-vol", "input", |e| {
        if let Some(v) = dom::target_el(e) { let x = crate::num::num(&dom::value(&v)).clamp(0.0, 1.0); with(|g| g.audio.set_volume(x)); dom::ls_set("encore.volume", &crate::num::js_num(x)); }
    });
    paint_time();
}

/// The time line under the song (no seeking for guests).
fn paint_time() {
    let Some((pos, dur)) = with(|g| {
        let (ei, epos) = rules::expected(&g.state, super::server_now());
        let d = g.state.a("queue").get(ei.max(0) as usize).map(|t| t.f("duration")).unwrap_or(0.0);
        let mine = if g.loaded.is_empty() || g.audio.paused() { epos } else { position(g) };
        (mine.min(if d > 0.0 { d } else { f64::INFINITY }), d)
    }) else { return };
    if let Some(c) = dom::by_id("g-cur") { dom::set_text(&c, &fmt::dur(pos)); }
    if let Some(l) = dom::by_id("g-line") { dom::set_style(&l, "width", &format!("{}%", if dur > 0.0 { (pos / dur * 100.0).clamp(0.0, 100.0) } else { 0.0 })); }
    // a Paused here line that should show now (the browser refused to start) or go
    let stale = with(|g| {
        let should = g.local_paused || (g.state.truthy("playing") && g.audio.paused() && !g.loaded.is_empty() && mono() > g.quiet_until);
        should != dom::by_id("g-catchup").is_some()
    }).unwrap_or(false);
    if stale { with(|g| g.drawn.clear()); draw(); }
}

/// Leave: the token stops working, and the join form comes back (the name kept).
fn leave() {
    let Some((root, code, id)) = with(|g| (g.root.clone(), g.code.clone(), g.state.s("id"))) else { return };
    stop();
    ui::toast("You left the group listen", "");
    dom::spawn(async move {
        let _ = api::post(&format!("/group/{}/leave", enc(&id)), json!({})).await;
        ask(root, code, None).await;
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_song_change_at_the_boundary_settles_by_itself() {
        let q = vec![json!({"qid": "a"}), json!({"qid": "b"}), json!({"qid": "c"})];
        // here on b's start, the group still at a's end
        assert!(near_boundary(&q, "b", 0.4, 0, 179.8));
        // the group on b's start, here still at a's end
        assert!(near_boundary(&q, "a", 179.9, 1, 0.3));
        // far apart: no
        assert!(!near_boundary(&q, "a", 50.0, 2, 10.0));
        assert!(!near_boundary(&q, "c", 30.0, 1, 10.0));
        assert!(!near_boundary(&q, "x", 0.1, 0, 0.1));
    }

    #[test]
    fn the_sign_in_line_after_an_ending_says_only_what_happened() {
        let no_account = "Only people with an account on this server can join this group listen";
        // the group ended: its code answers 404, and the page says only that it ended
        assert_eq!(sign_in_text("", Some("Sam ended the group listen".into())), "Sam ended the group listen");
        // a link opened with guests off: the reason alone
        assert_eq!(sign_in_text(no_account, None), no_account);
        // removed, and the server now refuses: two sentences
        assert_eq!(sign_in_text(no_account, Some("You were removed from the group listen".into())),
            format!("You were removed from the group listen. {no_account}"));
        // the same words once
        assert_eq!(sign_in_text("Group listens are turned off on this server", Some("Group listens are turned off on this server".into())),
            "Group listens are turned off on this server");
    }
}
