//! Port of web/js/pages/youtube.js – Settings → YouTube sign-in: which cookies yt-dlp uses (Encore's
//! browser, the browser extension or pasted cookies), the guided sign-in (Encore's own form in front of
//! Google's pages) with the live view as the fallback, Refresh now / Sign out, and the extension token.
//! Same words and DOM as the JS.

use crate::api;
use crate::dom;
use crate::json::J;
use crate::ui::{copy_text, esc, icon, toast};
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use web_sys::{Element, Event};

fn js_round(x: f64) -> i64 { (x + 0.5).floor() as i64 }
fn ago(t: f64) -> String {
    if t == 0.0 { return "never".into(); }
    let s = dom::now_s() - t;
    if s < 90.0 { "just now".into() } else if s < 5400.0 { format!("{} min ago", js_round(s / 60.0)) } else if s < 129600.0 { format!("{} h ago", js_round(s / 3600.0)) } else { format!("{} days ago", js_round(s / 86400.0)) }
}
fn until(t: f64) -> String {
    if t == 0.0 { return String::new(); }
    let s = t - dom::now_s();
    if s <= 60.0 { "any moment".into() } else if s < 5400.0 { format!("in {} min", js_round(s / 60.0)) } else if s < 129600.0 { format!("in {} h", js_round(s / 3600.0)) } else { format!("in {} days", js_round(s / 86400.0)) }
}

/// The card's frame; the status inside is filled in by `bind_youtube`.
pub fn youtube_block() -> String {
    format!(r##"<div class="card flat" id="yt-auth" style="padding:12px 14px;margin-bottom:14px">
    <div data-yt-status><div class="small muted">Loading…</div></div>
    <div class="row wrap mt" style="gap:8px">
      <button class="btn sm primary" type="button" data-yt="signin">{user}Sign in to YouTube</button>
      <button class="btn sm" type="button" data-yt="refresh">{refresh}Refresh now</button>
      <button class="btn sm" type="button" data-yt="extension">{link}Connect browser extension</button>
      <button class="btn sm danger" type="button" data-yt="signout">{x}Sign out</button>
    </div>
    <div data-yt-ext hidden></div>
    <div data-yt-guided hidden></div>
    <div data-yt-live hidden></div>
  </div>"##, user = icon("user"), refresh = icon("refresh"), link = icon("link"), x = icon("x"))
}

fn source_words(key: &str) -> &'static str {
    match key { "managed" => "Encore’s browser", "extension" => "Browser extension", _ => "Pasted cookies" }
}

fn source_line(key: &str, s: &Value) -> String {
    let state = if !s.truthy("has_cookies") {
        match key { "extension" => "not connected".to_string(), "managed" => "not signed in".to_string(), _ => "none".to_string() }
    } else if s.truthy("signed_in") {
        format!("signed in{}", if s.truthy("account") { format!(" as {}", s.s("account")) } else { String::new() })
    } else {
        "signed out".to_string()
    };
    format!("<span>{}: {}</span>", esc(source_words(key)), esc(&state))
}

/// The status part of the card (from GET /api/youtube/auth).
pub fn youtube_status_html(r: &Value) -> String {
    let source = r.s("source");
    let has_source = r.truthy("source");
    let sources = r.o("sources");
    let ok = has_source && sources.o(&source).truthy("signed_in");
    let badge = if !has_source { r##"<span class="badge">No cookies</span>"##.to_string() } else if ok { format!(r##"<span class="badge ok">{} Signed in</span>"##, icon("check")) } else { r##"<span class="badge warn">Signed out</span>"##.to_string() };
    let when = if has_source {
        format!("Last refresh {}{}", ago(r.f("last_refresh")), if r.truthy("next_check") { format!(" · next check {}", until(r.f("next_check"))) } else { String::new() })
    } else { String::new() };
    let label = if r.truthy("source_label") { r.s("source_label") } else { "Not signed in".to_string() };
    format!(r##"<div class="row between wrap" style="gap:8px;align-items:flex-start">
      <div><div style="font-weight:650">{label}</div>{account}</div>{badge}</div>
    {when}
    <div class="small muted row wrap" style="gap:4px 14px;margin-top:6px">{lines}</div>
    <div class="small faint" style="margin-top:6px">{diag}</div>"##,
        label = esc(&label),
        account = if r.truthy("account") { format!(r##"<div class="small muted">Account: {}</div>"##, esc(&r.s("account"))) } else { String::new() },
        when = if when.is_empty() { String::new() } else { format!(r##"<div class="small muted" style="margin-top:6px">{}</div>"##, esc(&when)) },
        lines = ["managed", "extension", "pasted"].iter().map(|k| source_line(k, &sources.o(k))).collect::<String>(),
        diag = esc(&r.s("diagnostics")))
}

/// The token box shown after "Connect browser extension".
pub fn extension_html(token: &str) -> String {
    format!(r##"<div class="card flat mt" style="padding:12px 14px">
    <div style="font-weight:650">Connect the Encore extension</div>
    <ol class="small muted" style="margin:6px 0 10px;padding-left:18px">
      <li>Install the extension: download Encore-youtube-extension from the releases page and unzip it. Chrome, Edge or Brave: Extensions → Developer mode → Load unpacked → the chrome folder. Firefox: about:debugging → Load Temporary Add-on → firefox/manifest.json.</li>
      <li>Open its options, enter this Encore’s address and the token below, and save.</li>
      <li>Stay signed in to YouTube in that browser. The extension sends the cookies when they change and every 6 hours.</li>
    </ol>
    <div class="row wrap" style="gap:8px"><div class="code grow mono" data-yt-token>{token}</div><button class="btn sm" type="button" data-yt-copy>{copy}Copy</button></div>
    <div class="small faint" style="margin-top:6px">Shown once. It can only send YouTube cookies to Encore; making a new one stops the old one.</div>
  </div>"##, token = esc(token), copy = icon("copy"))
}

/// One step of the guided sign-in (from POST /api/youtube/guided/start or /step).
pub fn guided_html(s: &Value) -> String {
    let kind = s.s("kind");
    let err = if s.truthy("error") { format!(r##"<div class="small" data-g-error style="color:var(--err);margin-top:6px">{}</div>"##, esc(&s.s("error"))) } else { String::new() };
    let text = if s.truthy("text") { format!(r##"<div class="small muted" style="margin-top:4px;white-space:pre-line">{}</div>"##, esc(&s.s("text"))) } else { String::new() };
    let choices: String = s.a("choices").iter().map(|c| format!(r##"<button class="btn sm{}" type="button" data-g-choice="{}" style="justify-content:flex-start;text-align:left;white-space:normal">{}</button>"##,
        if c.truthy("default") { " primary" } else { "" }, esc(&c.s("id")), esc(&c.s("label")))).collect();
    let list = if choices.is_empty() { String::new() } else { format!(r##"<div style="display:flex;flex-direction:column;gap:6px;margin-top:10px">{choices}</div>"##) };
    let cancel = r##"<button class="btn sm" type="button" data-g="cancel">Cancel</button>"##;
    let live = r##"<button class="btn sm" type="button" data-g="live">Open the live view</button>"##;
    let check = r##"<button class="btn sm" type="button" data-g="check">Check again</button>"##;
    let body = match kind.as_str() {
        "email" => format!(r##"<form data-g-form style="margin-top:10px"><input class="input" data-g-input type="email" name="username" autocomplete="username" inputmode="email" autocapitalize="off" spellcheck="false" placeholder="Email or phone" required>{err}
      <div class="row wrap" style="gap:8px;margin-top:10px"><button class="btn sm primary" type="submit">Next</button>{cancel}</div></form>"##),
        "password" => format!(r##"<form data-g-form style="margin-top:10px"><div class="row" style="gap:8px"><input class="input grow" data-g-input type="password" name="password" autocomplete="current-password" autocapitalize="off" spellcheck="false" placeholder="Password" required><button class="btn sm" type="button" data-g="show">Show</button></div>{err}
      <div class="row wrap" style="gap:8px;margin-top:10px"><button class="btn sm primary" type="submit">Next</button>{cancel}</div></form>"##),
        "code" => {
            let field = if s.s("code_hint") == "totp" {
                r##"<input class="input" data-g-input type="text" name="code" inputmode="numeric" autocomplete="one-time-code" pattern="[0-9 ]*" maxlength="9" placeholder="6-digit code" required>"##
            } else {
                r##"<input class="input" data-g-input type="text" name="code" autocomplete="one-time-code" autocapitalize="off" spellcheck="false" placeholder="Code" required>"##
            };
            format!(r##"<form data-g-form style="margin-top:10px">{field}{err}
      <div class="row wrap" style="gap:8px;margin-top:10px"><button class="btn sm primary" type="submit">Verify</button>{cancel}</div></form>{list}"##)
        }
        "choice" => format!(r##"{err}{list}<div class="row wrap" style="gap:8px;margin-top:10px">{cancel}</div>"##),
        "wait" => format!(r##"{err}{list}<div class="row wrap" style="gap:8px;margin-top:10px">{check}{cancel}</div>"##),
        "error" => format!(r##"{err}<div class="row wrap" style="gap:8px;margin-top:10px"><button class="btn sm primary" type="button" data-g="restart">Start again</button>{live}<button class="btn sm" type="button" data-g="close">Close</button></div>"##),
        _ => {
            let shot = if s.truthy("screenshot") {
                format!(r##"<img data-g-shot alt="Google’s page" src="data:image/jpeg;base64,{}" style="width:100%;max-width:640px;border-radius:8px;border:1px solid var(--line);display:block;margin-top:10px">"##, esc(&s.s("screenshot")))
            } else { String::new() };
            format!(r##"{err}{shot}{list}<div class="row wrap" style="gap:8px;margin-top:10px"><button class="btn sm primary" type="button" data-g="live">Open the live view</button>{check}{cancel}</div>"##)
        }
    };
    let title = if s.truthy("title") { s.s("title") } else { "Sign in to YouTube".to_string() };
    format!(r##"<div class="card flat mt" style="padding:12px 14px" data-g-kind="{kind}">
    <div style="font-weight:650">{title}</div>{text}{body}
    <div class="small faint" style="margin-top:10px">Encore types this into Google’s sign-in in its own browser on this server and keeps only the sign-in cookies. Security keys and passkeys can’t be used there: choose the authenticator app instead.</div>
  </div>"##, kind = esc(&kind), title = esc(&title))
}

/// The guided sign-in while it waits for the browser or Google.
pub fn guided_busy_html(message: &str) -> String {
    format!(r##"<div class="card flat mt" style="padding:12px 14px"><div class="small muted" data-g-busy>{}</div></div>"##, esc(message))
}

/// The live view: the browser's screen, typing box and buttons.
pub fn live_html() -> String {
    r##"<div class="card flat mt" style="padding:12px 14px">
    <div class="row between wrap" style="gap:8px;margin-bottom:6px"><span class="small muted" data-live-status>Starting the browser…</span>
      <span class="row" style="gap:6px"><button class="btn sm" type="button" data-live="back">Back</button><button class="btn sm" type="button" data-live="reload">Reload</button><button class="btn sm" type="button" data-live="close">Close</button></span></div>
    <div class="small faint mono" data-live-url style="overflow:hidden;text-overflow:ellipsis;white-space:nowrap"></div>
    <img data-live-screen alt="YouTube sign-in" tabindex="0" draggable="false" style="width:100%;max-width:1280px;border-radius:8px;border:1px solid var(--line);display:block;outline:none;touch-action:none;object-fit:contain;margin-top:6px;min-height:120px;background:var(--bg-2)">
    <form data-live-form style="margin-top:8px"><input class="input" data-live-input placeholder="Tap a field in the picture, then type here" autocomplete="off" autocapitalize="off" autocorrect="off" spellcheck="false" enterkeyhint="send"></form>
    <div class="row wrap" style="gap:6px;margin-top:6px"><button class="btn sm" type="button" data-live-key="Enter">Enter</button><button class="btn sm" type="button" data-live-key="Tab">Tab</button><button class="btn sm" type="button" data-live-key="Backspace">Backspace</button></div>
    <div class="small muted" style="margin-top:6px">Tap or click a field in the picture, then type in the box: the text goes to the page as you type. What you type goes to Google through Encore’s browser on this server; Encore keeps only the sign-in cookies.</div>
  </div>"##.to_string()
}

/// A point on the shown picture → the browser's own pixels. The box (left, top, width, height) is the
/// image's content box in CSS pixels, like clientX/Y, so the device pixel ratio cancels out; the picture
/// keeps its shape inside it (object-fit: contain), so bars at the sides or top are left out.
pub fn map_point(cx: f64, cy: f64, bx: (f64, f64, f64, f64), w: f64, h: f64) -> (i64, i64) {
    let (left0, top0, bw, bh) = bx;
    let s = (bw / w).min(bh / h);
    let scale = if s.is_nan() || s == 0.0 { 1.0 } else { s };
    let left = left0 + (bw - w * scale) / 2.0;
    let top = top0 + (bh - h * scale) / 2.0;
    let clamp = |v: f64, max: f64| (js_round(v).max(0) as f64).min(max - 1.0) as i64;
    (clamp((cx - left) / scale, w), clamp((cy - top) / scale, h))
}

const KEYS: [&str; 11] = ["Enter", "Backspace", "Tab", "Escape", "Delete", "ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End"];
const G_FAILED: &str = "The sign-in stopped";

/// The open live view: its socket and the socket's handlers.
struct Live {
    ws: web_sys::WebSocket,
    _onmessage: Closure<dyn FnMut(web_sys::MessageEvent)>,
    _onclose: Closure<dyn FnMut(Event)>,
}

struct Card {
    status: Element,
    ext_box: Element,
    live_box: Element,
    g_box: Element,
    info: RefCell<Option<Value>>,
    live: RefCell<Option<Live>>,
    /// bumped by every guided request and by Cancel: a late answer to an older one is dropped
    gen: Cell<u32>,
}

impl Card {
    fn paint(&self, r: &Value) {
        dom::set_html(&self.status, &youtube_status_html(r));
        *self.info.borrow_mut() = Some(r.clone());
    }
    fn stop_live(&self) {
        if let Some(l) = self.live.borrow_mut().take() {
            l.ws.set_onmessage(None);
            l.ws.set_onclose(None);
            let _ = l.ws.close();
        }
        dom::hide(&self.live_box, true);
        dom::set_html(&self.live_box, "");
    }
    fn g_hide(&self) {
        dom::hide(&self.g_box, true);
        dom::set_html(&self.g_box, "");
    }
    fn g_busy(&self, msg: &str) {
        dom::hide(&self.g_box, false);
        dom::set_html(&self.g_box, &guided_busy_html(msg));
    }
    fn bump(&self) -> u32 {
        self.gen.set(self.gen.get() + 1);
        self.gen.get()
    }
}

fn load(card: Rc<Card>) {
    dom::spawn(async move {
        match api::get("/youtube/auth").await {
            Ok(r) => card.paint(&r),
            Err(e) => dom::set_html(&card.status, &format!(r##"<div class="small" style="color:var(--err)">{}</div>"##, esc(&e.message))),
        }
    });
}

// ---- guided sign-in

fn g_fail(message: &str) -> Value {
    json!({ "kind": "error", "title": G_FAILED, "error": message })
}

fn g_show(card: &Rc<Card>, s: &Value) {
    if !card.g_box.is_connected() { return; }
    if s.s("kind") == "done" {
        card.bump();
        card.g_hide();
        toast(&if s.truthy("account") { format!("Signed in as {}", s.s("account")) } else { "Signed in to YouTube".to_string() }, "ok");
        load(card.clone());
        return;
    }
    dom::hide(&card.g_box, false);
    dom::set_html(&card.g_box, &guided_html(s));
    let input = dom::qs_in(&card.g_box, "[data-g-input]");
    if let Some(i) = &input { dom::focus(i); }
    let kind = s.s("kind");
    if let (Some(form), Some(input)) = (dom::qs_in(&card.g_box, "[data-g-form]"), input.clone()) {
        let (card, kind) = (card.clone(), kind.clone());
        dom::on(&form, "submit", move |e| {
            dom::prevent(e);
            let v = dom::value(&input);
            if v.trim().is_empty() { return; }
            dom::set_value(&input, "");
            g_send(&card, json!({ "kind": kind, "value": v }));
        });
    }
    {
        let card2 = card.clone();
        dom::on_each(&card.g_box, "[data-g-choice]", "click", move |b, _e| {
            g_send(&card2, json!({ "kind": "choice", "value": dom::data(b, "gChoice") }));
        });
    }
    {
        let card2 = card.clone();
        dom::on_each(&card.g_box, "[data-g]", "click", move |b, _e| {
            match dom::data(b, "g").as_str() {
                "cancel" | "close" => g_cancel(&card2),
                "check" => g_send(&card2, json!({ "kind": "check" })),
                "restart" => g_start(&card2),
                "live" => { card2.g_hide(); start_live(&card2, kind != "error"); }
                "show" => {
                    if let Some(i) = &input {
                        let shown = i.get_attribute("type").as_deref() == Some("password");
                        let _ = i.set_attribute("type", if shown { "text" } else { "password" });
                        dom::set_text(b, if shown { "Hide" } else { "Show" });
                        dom::focus(i);
                    }
                }
                _ => {}
            }
        });
    }
}

fn g_send(card: &Rc<Card>, body: Value) {
    let mine = card.bump();
    card.g_busy("Waiting for Google…");
    let card = card.clone();
    dom::spawn(async move {
        let s = match api::post("/youtube/guided/step", body).await { Ok(s) => s, Err(e) => g_fail(&e.message) };
        if mine == card.gen.get() { g_show(&card, &s); }
    });
}

fn g_start(card: &Rc<Card>) {
    card.stop_live();
    let mine = card.bump();
    card.g_busy("Starting the browser…");
    let card = card.clone();
    dom::spawn(async move {
        let s = match api::post_empty("/youtube/guided/start").await { Ok(s) => s, Err(e) => g_fail(&e.message) };
        if mine == card.gen.get() { g_show(&card, &s); }
    });
}

fn g_cancel(card: &Rc<Card>) {
    card.bump();
    card.g_hide();
    dom::spawn(async move { let _ = api::post_empty("/youtube/guided/cancel").await; });
}

// ---- live view (fallback)

fn send(ws: &web_sys::WebSocket, m: Value) {
    if ws.ready_state() == 1 { let _ = ws.send_with_str(&m.to_string()); }
}

/// A point of the viewport in the browser's own pixels.
fn at(img: &Element, x: f64, y: f64, size: &Cell<(f64, f64)>) -> (i64, i64) {
    let b = img.get_bounding_client_rect();
    let (w, h) = size.get();
    map_point(x, y, (b.left() + img.client_left() as f64, b.top() + img.client_top() as f64, img.client_width() as f64, img.client_height() as f64), w, h)
}

fn mouse_xy(e: &Event) -> (f64, f64) {
    e.dyn_ref::<web_sys::MouseEvent>().map(|m| (m.client_x() as f64, m.client_y() as f64)).unwrap_or((0.0, 0.0))
}

fn is_touch(e: &Event) -> bool {
    e.dyn_ref::<web_sys::PointerEvent>().map(|p| p.pointer_type() == "touch").unwrap_or(false)
}

/// One finger on the picture: where it started, where it was last, and whether it is a drag.
#[derive(Clone, Copy)]
struct Touch { x: f64, y: f64, last_x: f64, last_y: f64, drag: bool }

/// The text box: what of it the page already has, and the pending send.
struct Typing { sent: String, timer: Option<dom::Timeout> }

/// Send the box's change since the last send: Backspaces for what was removed, then the new text.
fn flush(ws: &web_sys::WebSocket, input: &Element, t: &RefCell<Typing>) {
    let mut t = t.borrow_mut();
    t.timer = None;
    let now_s = dom::value(input);
    let (was, now): (Vec<char>, Vec<char>) = (t.sent.chars().collect(), now_s.chars().collect());
    let mut i = 0;
    while i < was.len() && i < now.len() && was[i] == now[i] { i += 1; }
    for _ in i..was.len() { send(ws, json!({ "type": "key", "key": "Backspace", "code": "Backspace" })); }
    if now.len() > i { send(ws, json!({ "type": "text", "text": now[i..].iter().collect::<String>() })); }
    t.sent = now_s;
}

fn press(ws: &web_sys::WebSocket, input: &Element, t: &RefCell<Typing>, key: &str) {
    flush(ws, input, t);
    send(ws, json!({ "type": "key", "key": key, "code": key }));
    let v = dom::value(input);
    if key == "Backspace" {
        if !v.is_empty() {
            let mut c: Vec<char> = v.chars().collect();
            c.pop();
            let v2: String = c.into_iter().collect();
            dom::set_value(input, &v2);
            t.borrow_mut().sent = v2;
        }
    } else {
        dom::set_value(input, "");
        t.borrow_mut().sent = String::new();
    }
}

/// `guided`: the live view shows the guided sign-in's browser (Close goes back to the form).
fn start_live(card: &Rc<Card>, guided: bool) {
    card.stop_live();
    dom::hide(&card.live_box, false);
    dom::set_html(&card.live_box, &live_html());
    let (Some(img), Some(say), Some(url_line), Some(input)) = (dom::qs_in(&card.live_box, "[data-live-screen]"), dom::qs_in(&card.live_box, "[data-live-status]"),
        dom::qs_in(&card.live_box, "[data-live-url]"), dom::qs_in(&card.live_box, "[data-live-input]")) else { return };
    let loc = dom::location();
    let proto = if loc.protocol().unwrap_or_default() == "https:" { "wss" } else { "ws" };
    let Ok(ws) = web_sys::WebSocket::new(&format!("{proto}://{}/api/youtube/browser/live", loc.host().unwrap_or_default())) else {
        toast("Could not open the browser view", "err");
        return;
    };
    let size = Rc::new(Cell::new((1280.0, 860.0)));
    // set too when the server says it attached the guided sign-in's browser
    let guided = Rc::new(Cell::new(guided));
    let onmessage = {
        let (card, img, say, url_line, size, guided) = (card.clone(), img.clone(), say.clone(), url_line.clone(), size.clone(), guided.clone());
        Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |ev: web_sys::MessageEvent| {
            let Some(text) = ev.data().as_string() else { return };
            let Ok(m) = serde_json::from_str::<Value>(&text) else { return };
            if !img.is_connected() {
                // the page was left
                let c = card.clone();
                dom::spawn(async move { c.stop_live() });
                return;
            }
            match m.s("type").as_str() {
                "frame" => {
                    size.set((m.f("width"), m.f("height")));
                    let _ = img.set_attribute("src", &format!("data:image/jpeg;base64,{}", m.s("data")));
                    dom::set_text(&say, "Sign in with your Google account");
                }
                "guided" => guided.set(true),
                "url" => dom::set_text(&url_line, &m.s("url")),
                "status" => dom::set_text(&say, &m.s("message")),
                "error" => {
                    let msg = if m.truthy("message") { m.s("message") } else { "The browser stopped".to_string() };
                    dom::set_text(&say, &msg);
                    toast(&msg, "err");
                }
                "done" => {
                    card.bump();
                    card.g_hide();
                    toast(&if m.truthy("account") { format!("Signed in as {}", m.s("account")) } else { "Signed in to YouTube".to_string() }, "ok");
                    let c = card.clone();
                    dom::spawn(async move {
                        c.stop_live();
                        load(c.clone());
                    });
                }
                _ => {}
            }
        })
    };
    let onclose = {
        let (card, say, ws2) = (card.clone(), say.clone(), ws.clone());
        Closure::<dyn FnMut(Event)>::new(move |_e: Event| {
            let same = card.live.borrow().as_ref().map(|l| l.ws == ws2).unwrap_or(false);
            if same {
                dom::set_text(&say, "The browser closed");
            }
        })
    };
    ws.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
    ws.set_onclose(Some(onclose.as_ref().unchecked_ref()));
    *card.live.borrow_mut() = Some(Live { ws: ws.clone(), _onmessage: onmessage, _onclose: onclose });
    // the box's text goes to the page as it changes (Android keyboards send no usable key events)
    let typing = Rc::new(RefCell::new(Typing { sent: String::new(), timer: None }));
    let button = |e: &Event| if e.dyn_ref::<web_sys::MouseEvent>().map(|m| m.button()) == Some(2) { "right" } else { "left" };
    // mouse and pen; touch is below (a tap is a click, a drag scrolls)
    {
        let (ws, img2, size, input2, typing) = (ws.clone(), img.clone(), size.clone(), input.clone(), typing.clone());
        dom::on(&img, "pointerdown", move |e| {
            if is_touch(e) { return; }
            dom::prevent(e);
            // what is still in the box goes first (it belongs to the field focused before)
            flush(&ws, &input2, &typing);
            dom::focus(&img2);
            let (cx, cy) = mouse_xy(e);
            let (x, y) = at(&img2, cx, cy, &size);
            send(&ws, json!({ "type": "mouse", "event": "down", "x": x, "y": y, "button": button(e), "clickCount": 1 }));
        });
    }
    {
        let (ws, img2, size) = (ws.clone(), img.clone(), size.clone());
        dom::on(&img, "pointerup", move |e| {
            if is_touch(e) { return; }
            dom::prevent(e);
            let (cx, cy) = mouse_xy(e);
            let (x, y) = at(&img2, cx, cy, &size);
            send(&ws, json!({ "type": "mouse", "event": "up", "x": x, "y": y, "button": button(e), "clickCount": 1 }));
        });
    }
    {
        let (ws, img2, size) = (ws.clone(), img.clone(), size.clone());
        let last = Cell::new(0.0);
        dom::on(&img, "pointermove", move |e| {
            if is_touch(e) { return; }
            let t = dom::now_ms();
            if t - last.get() < 80.0 { return; }
            last.set(t);
            let (cx, cy) = mouse_xy(e);
            let (x, y) = at(&img2, cx, cy, &size);
            send(&ws, json!({ "type": "mouse", "event": "move", "x": x, "y": y }));
        });
    }
    {
        let (ws, img2, size) = (ws.clone(), img.clone(), size.clone());
        dom::on(&img, "wheel", move |e| {
            dom::prevent(e);
            let (cx, cy) = mouse_xy(e);
            let (x, y) = at(&img2, cx, cy, &size);
            let (dx, dy) = e.dyn_ref::<web_sys::WheelEvent>().map(|w| (w.delta_x(), w.delta_y())).unwrap_or((0.0, 0.0));
            send(&ws, json!({ "type": "mouse", "event": "wheel", "x": x, "y": y, "deltaX": dx, "deltaY": dy }));
        });
    }
    let touch: Rc<Cell<Option<Touch>>> = Rc::new(Cell::new(None));
    {
        let touch = touch.clone();
        dom::on(&img, "touchstart", move |e| {
            let Some(te) = e.dyn_ref::<web_sys::TouchEvent>() else { return };
            if te.touches().length() != 1 { touch.set(None); return; }
            dom::prevent(e);
            let Some(t) = te.touches().get(0) else { return };
            let (x, y) = (t.client_x() as f64, t.client_y() as f64);
            touch.set(Some(Touch { x, y, last_x: x, last_y: y, drag: false }));
        });
    }
    {
        let (ws, img2, size, touch) = (ws.clone(), img.clone(), size.clone(), touch.clone());
        dom::on(&img, "touchmove", move |e| {
            let Some(mut tc) = touch.get() else { return };
            let Some(te) = e.dyn_ref::<web_sys::TouchEvent>() else { return };
            if te.touches().length() != 1 { return; }
            dom::prevent(e);
            let Some(t) = te.touches().get(0) else { return };
            let (x, y) = (t.client_x() as f64, t.client_y() as f64);
            if !tc.drag && (x - tc.x).hypot(y - tc.y) > 10.0 { tc.drag = true; }
            if tc.drag {
                let cw = img2.client_width() as f64;
                let k = size.get().0 / if cw > 0.0 { cw } else { size.get().0 };
                let (px, py) = at(&img2, x, y, &size);
                send(&ws, json!({ "type": "mouse", "event": "wheel", "x": px, "y": py, "deltaX": (tc.last_x - x) * k, "deltaY": (tc.last_y - y) * k }));
                tc.last_x = x;
                tc.last_y = y;
            }
            touch.set(Some(tc));
        });
    }
    {
        let (ws, img2, size, touch, input2, typing) = (ws.clone(), img.clone(), size.clone(), touch.clone(), input.clone(), typing.clone());
        dom::on(&img, "touchend", move |e| {
            let Some(tc) = touch.get() else { return };
            dom::prevent(e);
            if let Some(t) = e.dyn_ref::<web_sys::TouchEvent>().and_then(|te| te.changed_touches().get(0)) {
                if !tc.drag {
                    flush(&ws, &input2, &typing);
                    let (x, y) = at(&img2, t.client_x() as f64, t.client_y() as f64, &size);
                    send(&ws, json!({ "type": "mouse", "event": "move", "x": x, "y": y }));
                    send(&ws, json!({ "type": "mouse", "event": "down", "x": x, "y": y, "button": "left", "clickCount": 1 }));
                    send(&ws, json!({ "type": "mouse", "event": "up", "x": x, "y": y, "button": "left", "clickCount": 1 }));
                }
            }
            touch.set(None);
        });
    }
    {
        let touch = touch.clone();
        dom::on(&img, "touchcancel", move |_| touch.set(None));
    }
    dom::on(&img, "contextmenu", |e| dom::prevent(e));
    {
        let ws = ws.clone();
        dom::on(&img, "keydown", move |e| {
            let Some(k) = e.dyn_ref::<web_sys::KeyboardEvent>() else { return };
            if k.ctrl_key() || k.meta_key() || k.alt_key() { return; }
            let key = k.key();
            if key.chars().count() == 1 { dom::prevent(e); send(&ws, json!({ "type": "text", "text": key })); }
            else if KEYS.contains(&key.as_str()) { dom::prevent(e); send(&ws, json!({ "type": "key", "key": key, "code": k.code() })); }
        });
    }
    {
        let (ws, input2, typing) = (ws.clone(), input.clone(), typing.clone());
        dom::on(&input, "input", move |_| {
            let (ws, input3, t2) = (ws.clone(), input2.clone(), typing.clone());
            let timer = dom::Timeout::new(300, move || flush(&ws, &input3, &t2));
            typing.borrow_mut().timer = Some(timer);
        });
    }
    if let Some(form) = dom::qs_in(&card.live_box, "[data-live-form]") {
        let (ws, input2, typing) = (ws.clone(), input.clone(), typing.clone());
        dom::on(&form, "submit", move |e| {
            dom::prevent(e);
            press(&ws, &input2, &typing, "Enter");
        });
    }
    {
        let (ws, input2, typing) = (ws.clone(), input.clone(), typing.clone());
        dom::on(&input, "keydown", move |e| {
            let key = dom::key(e);
            if key == "Tab" {
                dom::prevent(e);
                press(&ws, &input2, &typing, "Tab");
            } else if key == "Backspace" && dom::value(&input2).is_empty() {
                dom::prevent(e);
                press(&ws, &input2, &typing, "Backspace");
            }
        });
    }
    // keep the focus (and the phone's keyboard) in the box
    dom::on_each(&card.live_box.clone(), "[data-live-key]", "mousedown", |_b, e| dom::prevent(e));
    {
        let (ws, input2, typing) = (ws.clone(), input.clone(), typing.clone());
        dom::on_each(&card.live_box.clone(), "[data-live-key]", "click", move |b, _e| {
            press(&ws, &input2, &typing, &dom::data(b, "liveKey"));
            if dom::value(&input2).is_empty() { dom::focus(&input2); }
        });
    }
    {
        let (ws, card, guided) = (ws.clone(), card.clone(), guided.clone());
        dom::on_each(&card.live_box.clone(), "[data-live]", "click", move |b, _e| {
            let a = dom::data(b, "live");
            if a == "close" {
                card.stop_live();
                if guided.get() { g_send(&card, json!({ "kind": "check" })); }
            } else {
                send(&ws, json!({ "type": "nav", "action": a }));
            }
        });
    }
}

/// Wire the card: status, buttons, the extension token, the guided sign-in and the live view.
pub fn bind_youtube(root: &Element) {
    let Some(host) = dom::qs_in(root, "#yt-auth") else { return };
    let (Some(status), Some(ext_box), Some(live_box), Some(g_box)) = (dom::qs_in(&host, "[data-yt-status]"), dom::qs_in(&host, "[data-yt-ext]"),
        dom::qs_in(&host, "[data-yt-live]"), dom::qs_in(&host, "[data-yt-guided]")) else { return };
    let card = Rc::new(Card { status, ext_box, live_box, g_box, info: RefCell::new(None), live: RefCell::new(None), gen: Cell::new(0) });
    {
        let card = card.clone();
        dom::on_each(&host, "[data-yt]", "click", move |b, e| {
            dom::prevent(e);
            let a = dom::data(b, "yt");
            let (card, b) = (card.clone(), b.clone());
            match a.as_str() {
                "signin" => {
                    let installed = card.info.borrow().as_ref().map(|i| i.o("browser").truthy("installed"));
                    if installed == Some(false) { toast("Install “Browser for YouTube sign-in” under Download tools first", "err"); return; }
                    g_start(&card);
                }
                "refresh" => dom::spawn(async move {
                    dom::set_disabled(&b, true);
                    match api::post_empty("/youtube/refresh").await {
                        Ok(r) => {
                            card.paint(&r);
                            let ok = r.truthy("refreshed");
                            toast(if ok { "Refreshed – still signed in" } else { "YouTube signed out" }, if ok { "ok" } else { "err" });
                        }
                        Err(err) => toast(&err.message, "err"),
                    }
                    dom::set_disabled(&b, false);
                }),
                "signout" => {
                    if !dom::confirm("Sign Encore’s browser out of YouTube and forget the extension’s cookies? Pasted cookies stay (clear them below).") { return; }
                    card.stop_live();
                    card.bump();
                    card.g_hide();
                    dom::spawn(async move {
                        match api::post("/youtube/signout", json!({ "source": "all" })).await {
                            Ok(r) => { card.paint(&r); toast("Signed out", "ok"); }
                            Err(err) => toast(&err.message, "err"),
                        }
                    });
                }
                "extension" => {
                    let had = card.info.borrow().as_ref().map(|i| i.o("extension").truthy("token_at")).unwrap_or(false);
                    if had && !dom::confirm("Make a new token? The extension stops sending cookies until you enter the new one.") { return; }
                    dom::spawn(async move {
                        match api::post_empty("/youtube/extension-token").await {
                            Ok(r) => {
                                let token = r.s("token");
                                dom::hide(&card.ext_box, false);
                                dom::set_html(&card.ext_box, &extension_html(&token));
                                if let Some(c) = dom::qs_in(&card.ext_box, "[data-yt-copy]") {
                                    dom::on(&c, "click", move |_| copy_text(&token));
                                }
                                load(card.clone());
                            }
                            Err(err) => toast(&err.message, "err"),
                        }
                    });
                }
                _ => {}
            }
        });
    }
    load(card);
}
