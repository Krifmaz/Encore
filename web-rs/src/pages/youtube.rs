//! Port of web/js/pages/youtube.js – Settings → YouTube sign-in: which cookies yt-dlp uses (Encore's
//! browser, the browser extension or pasted cookies), the live view for signing Encore's browser in,
//! Refresh now / Sign out, and the extension token. Same words and DOM as the JS.

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

/// The live view: the browser's screen, typing box and buttons.
pub fn live_html() -> String {
    r##"<div class="card flat mt" style="padding:12px 14px">
    <div class="row between wrap" style="gap:8px;margin-bottom:6px"><span class="small muted" data-live-status>Starting the browser…</span>
      <span class="row" style="gap:6px"><button class="btn sm" type="button" data-live="back">Back</button><button class="btn sm" type="button" data-live="reload">Reload</button><button class="btn sm" type="button" data-live="close">Close</button></span></div>
    <div class="small faint mono" data-live-url style="overflow:hidden;text-overflow:ellipsis;white-space:nowrap"></div>
    <img data-live-screen alt="YouTube sign-in" tabindex="0" draggable="false" style="width:100%;max-width:1280px;border-radius:8px;border:1px solid var(--line);display:block;outline:none;touch-action:none;margin-top:6px;min-height:120px;background:var(--bg-2)">
    <input class="input" data-live-input placeholder="Type here to send text to the page (Enter submits)" autocomplete="off" autocapitalize="off" spellcheck="false" style="margin-top:8px">
    <div class="small muted" style="margin-top:6px">Click into the page and type, or use the box above. What you type goes to Google through Encore’s browser on this server; Encore keeps only the sign-in cookies.</div>
  </div>"##.to_string()
}

const KEYS: [&str; 11] = ["Enter", "Backspace", "Tab", "Escape", "Delete", "ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End"];

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
    info: RefCell<Option<Value>>,
    live: RefCell<Option<Live>>,
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
}

fn load(card: Rc<Card>) {
    dom::spawn(async move {
        match api::get("/youtube/auth").await {
            Ok(r) => card.paint(&r),
            Err(e) => dom::set_html(&card.status, &format!(r##"<div class="small" style="color:var(--err)">{}</div>"##, esc(&e.message))),
        }
    });
}

fn send(ws: &web_sys::WebSocket, m: Value) {
    if ws.ready_state() == 1 { let _ = ws.send_with_str(&m.to_string()); }
}

/// Mouse position in the browser's own pixels.
fn at(img: &Element, e: &Event, size: &Cell<(f64, f64)>) -> (i64, i64) {
    let Some(m) = e.dyn_ref::<web_sys::MouseEvent>() else { return (0, 0) };
    let b = img.get_bounding_client_rect();
    let (w, h) = size.get();
    (js_round((m.client_x() as f64 - b.left()) / b.width() * w), js_round((m.client_y() as f64 - b.top()) / b.height() * h))
}

fn start_live(card: &Rc<Card>) {
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
    let onmessage = {
        let (card, img, say, url_line, size) = (card.clone(), img.clone(), say.clone(), url_line.clone(), size.clone());
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
                "url" => dom::set_text(&url_line, &m.s("url")),
                "status" => dom::set_text(&say, &m.s("message")),
                "error" => {
                    let msg = if m.truthy("message") { m.s("message") } else { "The browser stopped".to_string() };
                    dom::set_text(&say, &msg);
                    toast(&msg, "err");
                }
                "done" => {
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
    let button = |e: &Event| if e.dyn_ref::<web_sys::MouseEvent>().map(|m| m.button()) == Some(2) { "right" } else { "left" };
    {
        let (ws, img2, size) = (ws.clone(), img.clone(), size.clone());
        dom::on(&img, "pointerdown", move |e| {
            dom::prevent(e);
            dom::focus(&img2);
            let (x, y) = at(&img2, e, &size);
            send(&ws, json!({ "type": "mouse", "event": "down", "x": x, "y": y, "button": button(e), "clickCount": 1 }));
        });
    }
    {
        let (ws, img2, size) = (ws.clone(), img.clone(), size.clone());
        dom::on(&img, "pointerup", move |e| {
            dom::prevent(e);
            let (x, y) = at(&img2, e, &size);
            send(&ws, json!({ "type": "mouse", "event": "up", "x": x, "y": y, "button": button(e), "clickCount": 1 }));
        });
    }
    {
        let (ws, img2, size) = (ws.clone(), img.clone(), size.clone());
        let last = Cell::new(0.0);
        dom::on(&img, "pointermove", move |e| {
            let t = dom::now_ms();
            if t - last.get() < 80.0 { return; }
            last.set(t);
            let (x, y) = at(&img2, e, &size);
            send(&ws, json!({ "type": "mouse", "event": "move", "x": x, "y": y }));
        });
    }
    {
        let (ws, img2, size) = (ws.clone(), img.clone(), size.clone());
        dom::on(&img, "wheel", move |e| {
            dom::prevent(e);
            let (x, y) = at(&img2, e, &size);
            let (dx, dy) = e.dyn_ref::<web_sys::WheelEvent>().map(|w| (w.delta_x(), w.delta_y())).unwrap_or((0.0, 0.0));
            send(&ws, json!({ "type": "mouse", "event": "wheel", "x": x, "y": y, "deltaX": dx, "deltaY": dy }));
        });
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
        let (ws, input2) = (ws.clone(), input.clone());
        dom::on(&input, "keydown", move |e| {
            let key = dom::key(e);
            let v = dom::value(&input2);
            if key == "Enter" {
                dom::prevent(e);
                if !v.is_empty() { send(&ws, json!({ "type": "text", "text": v })); }
                dom::set_value(&input2, "");
                send(&ws, json!({ "type": "key", "key": "Enter", "code": "Enter" }));
            } else if key == "Backspace" && v.is_empty() {
                dom::prevent(e);
                send(&ws, json!({ "type": "key", "key": "Backspace", "code": "Backspace" }));
            } else if key == "Tab" {
                dom::prevent(e);
                if !v.is_empty() { send(&ws, json!({ "type": "text", "text": v })); }
                dom::set_value(&input2, "");
                send(&ws, json!({ "type": "key", "key": "Tab", "code": "Tab" }));
            }
        });
    }
    {
        let (ws, card) = (ws.clone(), card.clone());
        dom::on_each(&card.live_box.clone(), "[data-live]", "click", move |b, _e| {
            let a = dom::data(b, "live");
            if a == "close" { card.stop_live(); } else { send(&ws, json!({ "type": "nav", "action": a })); }
        });
    }
}

/// Wire the card: status, buttons, the extension token and the live view.
pub fn bind_youtube(root: &Element) {
    let Some(host) = dom::qs_in(root, "#yt-auth") else { return };
    let (Some(status), Some(ext_box), Some(live_box)) = (dom::qs_in(&host, "[data-yt-status]"), dom::qs_in(&host, "[data-yt-ext]"), dom::qs_in(&host, "[data-yt-live]")) else { return };
    let card = Rc::new(Card { status, ext_box, live_box, info: RefCell::new(None), live: RefCell::new(None) });
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
                    start_live(&card);
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
