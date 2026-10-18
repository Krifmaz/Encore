//! Port of web/js/ui.js: escaping, the icon set, toasts, modals, formatting, badges, the ⋯ menu and the
//! show/hide eye on secrets. Same HTML, same class names.

use crate::dom::{self, body, by_id, closest, create, document, keep, on, qs_in, qsa_in, set_html, EventListener};
use crate::json::J;
use serde_json::Value;
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::JsCast;
use web_sys::Element;

/// HTML-escape (the JS `esc`).
pub fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c { '&' => o.push_str("&amp;"), '<' => o.push_str("&lt;"), '>' => o.push_str("&gt;"), '"' => o.push_str("&quot;"), '\'' => o.push_str("&#39;"), _ => o.push(c) }
    }
    o
}
/// `esc(String(v ?? ''))` for JSON values.
pub fn escv(v: &Value) -> String { esc(&crate::json::js_string(v)) }
/// `esc(obj.key ?? '')`.
pub fn esck(v: &Value, key: &str) -> String { esc(&v.s(key)) }

fn p(d: &str) -> String {
    format!(r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">{d}</svg>"#)
}

/// `icon.<name>` – the same inline SVGs.
pub fn icon(name: &str) -> String {
    match name {
        "home" => p(r#"<path d="M3 10.5 12 3l9 7.5V20a1 1 0 0 1-1 1h-5v-6H9v6H4a1 1 0 0 1-1-1z"/>"#),
        "search" => p(r#"<circle cx="11" cy="11" r="7"/><path d="m21 21-4.3-4.3"/>"#),
        "library" => p(r#"<path d="M4 4v16M9 4v16M14 4l6 16"/>"#),
        "inbox" => p(r#"<path d="M22 12h-6l-2 3h-4l-2-3H2"/><path d="M5.45 5.11 2 12v6a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-6l-3.45-6.89A2 2 0 0 0 16.76 4H7.24a2 2 0 0 0-1.79 1.11z"/>"#),
        "download" => p(r#"<path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><path d="m7 10 5 5 5-5M12 15V3"/>"#),
        "activity" => p(r#"<path d="M22 12h-4l-3 9L9 3l-3 9H2"/>"#),
        "users" => p(r#"<path d="M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2"/><circle cx="9" cy="7" r="4"/><path d="M22 21v-2a4 4 0 0 0-3-3.87M16 3.13a4 4 0 0 1 0 7.75"/>"#),
        "ticket" => p(r#"<path d="M2 9a3 3 0 0 0 0 6v2a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-2a3 3 0 0 0 0-6V7a2 2 0 0 0-2-2H4a2 2 0 0 0-2 2z"/><path d="M13 5v2M13 17v2M13 11v2"/>"#),
        "settings" => p(r#"<circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"/>"#),
        "play" => r#"<svg viewBox="0 0 24 24" fill="currentColor"><path d="M7 4.5v15a1 1 0 0 0 1.5.86l12-7.5a1 1 0 0 0 0-1.72l-12-7.5A1 1 0 0 0 7 4.5z"/></svg>"#.into(),
        "pause" => r#"<svg viewBox="0 0 24 24" fill="currentColor"><rect x="6" y="4" width="4" height="16" rx="1"/><rect x="14" y="4" width="4" height="16" rx="1"/></svg>"#.into(),
        "next" => r#"<svg viewBox="0 0 24 24" fill="currentColor"><path d="M5 5.5v13a1 1 0 0 0 1.5.86L16 13.5V18a1 1 0 0 0 2 0V6a1 1 0 0 0-2 0v4.5L6.5 4.64A1 1 0 0 0 5 5.5z"/></svg>"#.into(),
        "prev" => r#"<svg viewBox="0 0 24 24" fill="currentColor"><path d="M19 5.5v13a1 1 0 0 1-1.5.86L8 13.5V18a1 1 0 0 1-2 0V6a1 1 0 0 1 2 0v4.5l9.5-5.86A1 1 0 0 1 19 5.5z"/></svg>"#.into(),
        "plus" => p(r#"<path d="M12 5v14M5 12h14"/>"#),
        "check" => p(r#"<path d="M20 6 9 17l-5-5"/>"#),
        "x" => p(r#"<path d="M18 6 6 18M6 6l12 12"/>"#),
        "menu" => p(r#"<path d="M3 6h18M3 12h18M3 18h18"/>"#),
        "logout" => p(r#"<path d="M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4M16 17l5-5-5-5M21 12H9"/>"#),
        "tag" => p(r#"<path d="M20.59 13.41 13.42 20.58a2 2 0 0 1-2.83 0L2 12V2h10l8.59 8.59a2 2 0 0 1 0 2.82z"/><circle cx="7" cy="7" r="1.5"/>"#),
        "edit" => p(r#"<path d="M12 20h9M16.5 3.5a2.12 2.12 0 1 1 3 3L7 19l-4 1 1-4z"/>"#),
        "trash" => p(r#"<path d="M3 6h18M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6"/>"#),
        "refresh" => p(r#"<path d="M21 12a9 9 0 1 1-2.64-6.36L21 8M21 3v5h-5"/>"#),
        "link" => p(r#"<path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71"/><path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71"/>"#),
        "music" => p(r#"<path d="M9 18V5l12-2v13"/><circle cx="6" cy="18" r="3"/><circle cx="18" cy="16" r="3"/>"#),
        "disc" => p(r#"<circle cx="12" cy="12" r="10"/><circle cx="12" cy="12" r="3"/>"#),
        "user" => p(r#"<path d="M20 21v-2a4 4 0 0 0-4-4H8a4 4 0 0 0-4 4v2"/><circle cx="12" cy="7" r="4"/>"#),
        "sun" => p(r#"<circle cx="12" cy="12" r="4"/><path d="M12 2v2M12 20v2M4.93 4.93l1.41 1.41M17.66 17.66l1.41 1.41M2 12h2M20 12h2M6.34 17.66l-1.41 1.41M19.07 4.93l-1.41 1.41"/>"#),
        "moon" => p(r#"<path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z"/>"#),
        "copy" => p(r#"<rect x="9" y="9" width="13" height="13" rx="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/>"#),
        "volume" => p(r#"<path d="M11 5 6 9H2v6h4l5 4zM15.54 8.46a5 5 0 0 1 0 7.07M19.07 4.93a10 10 0 0 1 0 14.14"/>"#),
        "external" => p(r#"<path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6M15 3h6v6M10 14 21 3"/>"#),
        "shield" => p(r#"<path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z"/>"#),
        "heart" => p(r#"<path d="M20.84 4.61a5.5 5.5 0 0 0-7.78 0L12 5.67l-1.06-1.06a5.5 5.5 0 0 0-7.78 7.78l1.06 1.06L12 21.23l7.78-7.78 1.06-1.06a5.5 5.5 0 0 0 0-7.78z"/>"#),
        "spark" => p(r#"<path d="M12 3l1.9 5.6L19.5 10l-5.6 1.9L12 17.5l-1.9-5.6L4.5 10l5.6-1.4z"/><path d="M19 15l.8 2.2L22 18l-2.2.8L19 21l-.8-2.2L16 18l2.2-.8z"/>"#),
        "chevron" => p(r#"<polyline points="6 9 12 15 18 9"/>"#),
        "more" => p(r#"<circle cx="12" cy="5" r="1.6"/><circle cx="12" cy="12" r="1.6"/><circle cx="12" cy="19" r="1.6"/>"#),
        "shuffle" => p(r#"<path d="M16 3h5v5M4 20 21 3M21 16v5h-5M15 15l6 6M4 4l5 5"/>"#),
        "list" => p(r#"<path d="M8 6h13M8 12h13M8 18h13M3 6h.01M3 12h.01M3 18h.01"/>"#),
        "devices" => p(r#"<rect x="2" y="4" width="13" height="10" rx="2"/><path d="M6 18h5M8.5 14v4"/><rect x="17" y="8" width="5" height="12" rx="1.5"/><path d="M19.5 17h.01"/>"#),
        "bug" => p(r#"<path d="M9 7V6a3 3 0 0 1 6 0v1"/><path d="M12 20c-3.3 0-6-2.7-6-6v-3a4 4 0 0 1 4-4h4a4 4 0 0 1 4 4v3c0 3.3-2.7 6-6 6z"/><path d="M12 20v-9M6 13H2M22 13h-4M3 21c0-2.1 1.7-3.9 3.8-4M20.2 17c2.1.1 3.8 1.9 3.8 4M3 5c0 2.1 1.6 3.8 3.5 4M21 5c0 2.1-1.6 3.8-3.5 4M8 2l1.9 1.9M16 2l-1.9 1.9"/>"#),
        _ => String::new(),
    }
}
/// `icon.x.replace('<svg', '<svg width="12" height="12"')` and friends.
pub fn icon_attr(name: &str, attrs: &str) -> String { icon(name).replacen("<svg", &format!("<svg {attrs}"), 1) }

/// `toast(msg, type)`.
pub fn toast(msg: &str, kind: &str) {
    let el = create("div");
    el.set_class_name(&format!("toast {kind}"));
    el.set_text_content(Some(msg));
    if let Some(t) = by_id("toasts") { let _ = t.append_child(&el); }
    let e1 = el.clone();
    let e2 = el.clone();
    keep_app(gloo_timers::callback::Timeout::new(3800, move || { dom::set_style(&e1, "opacity", "0"); dom::set_style(&e1, "transition", "opacity .3s"); }));
    keep_app(gloo_timers::callback::Timeout::new(4200, move || e2.remove()));
}
fn keep_app(x: impl std::any::Any + 'static) { dom::keep_forever(x) }

/// A modal on screen: `el` is the `.modal`, `close()` removes it.
#[derive(Clone)]
pub struct Modal {
    pub el: Element,
    pub bg: Element,
    close_fn: Rc<dyn Fn()>,
}
impl Modal {
    pub fn close(&self) { (self.close_fn)(); }
    pub fn qs(&self, sel: &str) -> Option<Element> { qs_in(&self.el, sel) }
    pub fn qsa(&self, sel: &str) -> Vec<Element> { qsa_in(&self.el, sel) }
    pub fn is_open(&self) -> bool { self.el.is_connected() }
}

pub struct ModalOpts<'a> {
    pub title: &'a str,
    pub body: &'a str,
    pub footer: &'a str,
    pub wide: bool,
}
impl<'a> ModalOpts<'a> {
    pub fn new(title: &'a str, body: &'a str) -> Self { Self { title, body, footer: "", wide: false } }
    pub fn footer(mut self, f: &'a str) -> Self { self.footer = f; self }
    pub fn wide(mut self, w: bool) -> Self { self.wide = w; self }
}

/// `modal({ title, body, footer, wide })` – title/body/footer are HTML, exactly like the JS.
pub fn modal(o: ModalOpts) -> Modal {
    let root = by_id("modal-root").expect("#modal-root");
    let bg = create("div");
    bg.set_class_name("modal-bg");
    let foot = if o.footer.is_empty() { String::new() } else { format!(r#"<div class="modal-foot">{}</div>"#, o.footer) };
    set_html(&bg, &format!(r#"<div class="modal {}" role="dialog" aria-modal="true">
    <div class="modal-head"><h2>{}</h2><button class="btn ghost icon" data-close aria-label="Close">{}</button></div>
    <div class="modal-body">{}</div>
    {}
  </div>"#, if o.wide { "wide" } else { "" }, o.title, icon("x"), o.body, foot));
    let _ = root.append_child(&bg);
    let el = qs_in(&bg, ".modal").expect(".modal");
    let key_listener: Rc<RefCell<Option<EventListener>>> = Rc::new(RefCell::new(None));
    let bg2 = bg.clone();
    let kl = key_listener.clone();
    let close_fn: Rc<dyn Fn()> = Rc::new(move || { bg2.remove(); kl.borrow_mut().take(); });
    let c = close_fn.clone();
    *key_listener.borrow_mut() = Some(dom::listen(&document(), "keydown", move |e| { if dom::key(e) == "Escape" { c(); } }));
    let c = close_fn.clone();
    let bg3 = bg.clone();
    on(&bg, "mousedown", move |e| { let is_bg = e.target().map(|t| { let bt: &web_sys::EventTarget = bg3.as_ref(); &t == bt }).unwrap_or(false); if is_bg { c(); } });
    for b in qsa_in(&bg, "[data-close]") { let c = close_fn.clone(); on(&b, "click", move |_| c()); }
    keep(key_listener);
    Modal { el, bg, close_fn }
}

/// `confirmDialog(title, text, okLabel, danger)` – resolves to true on OK, false when dismissed.
pub async fn confirm_dialog(title: &str, text: &str, ok_label: &str, danger: bool) -> bool {
    let (tx, rx) = futures::channel::oneshot::channel::<bool>();
    let tx = Rc::new(RefCell::new(Some(tx)));
    let m = modal(ModalOpts::new(&esc(title), &format!(r#"<p class="muted" style="margin:0">{}</p>"#, esc(text)))
        .footer(&format!(r#"<button class="btn" data-close>Cancel</button><button class="btn {}" data-ok>{}</button>"#, if danger { "danger" } else { "primary" }, esc(ok_label))));
    if let Some(ok) = m.qs("[data-ok]") {
        let m2 = m.clone();
        let tx2 = tx.clone();
        on(&ok, "click", move |_| { m2.close(); if let Some(t) = tx2.borrow_mut().take() { let _ = t.send(true); } });
    }
    // dismissed (Escape, backdrop, Cancel): the modal leaves the DOM without OK
    let el = m.el.clone();
    let tx3 = tx.clone();
    let obs = web_sys::MutationObserver::new(wasm_bindgen::closure::Closure::<dyn FnMut(js_sys::Array, web_sys::MutationObserver)>::new(move |_m, o: web_sys::MutationObserver| {
        if !el.is_connected() { o.disconnect(); if let Some(t) = tx3.borrow_mut().take() { let _ = t.send(false); } }
    }).into_js_value().unchecked_ref()).ok();
    if let (Some(o), Some(root)) = (obs, by_id("modal-root")) {
        let init = web_sys::MutationObserverInit::new();
        init.set_child_list(true);
        let _ = o.observe_with_options(&root, &init);
    }
    rx.await.unwrap_or(false)
}

pub mod fmt {
    /// `fmt.dur(s)`: 3:05 or 1:02:03.
    pub fn dur(s: f64) -> String {
        let s = s.max(0.0).round() as i64;
        let (h, m, sec) = (s / 3600, (s % 3600) / 60, s % 60);
        if h > 0 { format!("{h}:{m:02}:{sec:02}") } else { format!("{m}:{sec:02}") }
    }
    /// `fmt.longDur(s)`: "2 hr 5 min" / "12 min".
    /// "4 mo 22 days 8 hr 24 min" – years, months (30 days), days, hours, minutes; parts that are 0 are left out.
    pub fn long_dur(s: f64) -> String {
        let mut m = (s.max(0.0) / 60.0).round() as i64;
        let mut parts: Vec<String> = vec![];
        for (n, one, many) in [(525600, "yr", "yr"), (43200, "mo", "mo"), (1440, "day", "days"), (60, "hr", "hr")] {
            let v = m / n;
            if v > 0 { parts.push(format!("{v} {}", if v == 1 { one } else { many })); m -= v * n; }
        }
        if m > 0 || parts.is_empty() { parts.push(format!("{m} min")); }
        parts.join(" ")
    }
    /// `fmt.size(b)`.
    pub fn size(b: f64) -> String {
        if b <= 0.0 { return "0 B".into(); }
        let u = ["B", "KB", "MB", "GB", "TB"];
        let i = ((b.ln() / 1024f64.ln()).floor() as usize).min(4);
        let v = b / 1024f64.powi(i as i32);
        if i > 1 { format!("{v:.1} {}", u[i]) } else { format!("{} {}", v.round() as i64, u[i]) }
    }
    /// `fmt.ago(ts)`.
    pub fn ago(ts: f64) -> String {
        if ts <= 0.0 { return "never".into(); }
        let d = js_sys::Date::now() / 1000.0 - ts;
        if d < 60.0 { "just now".into() }
        else if d < 3600.0 { format!("{}m ago", (d / 60.0).floor() as i64) }
        else if d < 86400.0 { format!("{}h ago", (d / 3600.0).floor() as i64) }
        else if d < 86400.0 * 30.0 { format!("{}d ago", (d / 86400.0).floor() as i64) }
        else { js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ts * 1000.0)).to_locale_date_string("default", &wasm_bindgen::JsValue::UNDEFINED).into() }
    }
    /// `fmt.date(ts)`.
    pub fn date(ts: f64) -> String {
        if ts <= 0.0 { return "—".into(); }
        js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ts * 1000.0)).to_locale_string("default", &wasm_bindgen::JsValue::UNDEFINED).into()
    }
    /// `fmt.day(ts)`: `toLocaleDateString(undefined, { year: 'numeric', month: 'short', day: 'numeric' })`.
    pub fn day(ts: f64) -> String {
        if ts <= 0.0 { return "—".into(); }
        let opts = js_sys::Object::new();
        let _ = js_sys::Reflect::set(&opts, &"year".into(), &"numeric".into());
        let _ = js_sys::Reflect::set(&opts, &"month".into(), &"short".into());
        let _ = js_sys::Reflect::set(&opts, &"day".into(), &"numeric".into());
        js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ts * 1000.0)).to_locale_date_string("default", &opts).into()
    }
    /// `fmt.num(n)`: `(n || 0).toLocaleString()`.
    pub fn num(n: f64) -> String {
        js_sys::Number::from(n).to_locale_string("default").into()
    }
}

/// `initials(s)`.
pub fn initials(s: &str) -> String {
    let t = s.trim();
    // `t.get(..4)` is None when byte 4 is inside a multi-byte character (Japanese, curly quotes…): slicing
    // there would panic and take the whole page with it (0.26.0/0.26.1 showed empty grids for that reason).
    let t = match t.get(..4) { Some(h) if h.eq_ignore_ascii_case("the ") => t[4..].trim_start(), _ => t };
    let t = if t.is_empty() { "?" } else { t };
    t.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_else(|| "?".into())
}

/// `art(src, label, cls)`: placeholder letter plus the image (lists get the small cached cover).
pub fn art(src: &str, label: &str, cls: &str) -> String { art_sized(src, label, cls, 0) }

/// `art(src, label, cls, size)`: as `art`, with the cover size for library covers (0 = by `cls`).
/// A library cover whose colour the server sent (`cover_color`) shows that colour at once and fades the picture in over it.
pub fn art_sized(src: &str, label: &str, cls: &str, size: u32) -> String {
    let mut ph = format!(r#"<div class="ph">{}</div>"#, esc(&initials(label)));
    if src.is_empty() { return ph; }
    let mut src = src.to_string();
    let mut dim = String::new();
    let mut fade = "";
    let mut cls = cls.to_string();
    if let Some(base) = cover_base(&src) {
        let s = if size > 0 { size } else if cls == "big" { 1000 } else { 400 };
        let color = src[..base].strip_suffix("/cover").and_then(|h| h.rfind("/api/library/albums/").map(|i| h[i + "/api/library/albums/".len()..].to_string()))
            .and_then(|id| crate::api::cover_color(&id));
        let v = src[base..].strip_prefix("?v=").map(|v| format!("&v={v}")).unwrap_or_default();
        src = format!("{}?s={s}{v}", &src[..base]);
        dim = format!(r#" width="{s}" height="{s}""#);
        if let Some(color) = color {
            ph = format!(r#"<div class="ph" style="background:{color};color:transparent">{}</div>"#, esc(&initials(label)));
            fade = r#" onload="this.classList.add('in')""#;
            cls = if cls.is_empty() { "cover-fade".into() } else { format!("{cls} cover-fade") };
        }
    }
    format!(r#"{ph}<img class="{cls}" src="{}" alt=""{dim} loading="lazy" decoding="async"{fade} onerror="this.remove()">"#, esc(&src))
}

/// `preloadCovers(urls)`: fetches covers ahead of the scroll (the next page of a list), so they are cached when it shows.
pub fn preload_covers(urls: &[String]) {
    for u in urls.iter().take(120).filter(|u| !u.is_empty()) {
        if let Ok(img) = web_sys::HtmlImageElement::new() {
            img.set_decoding("async");
            img.set_src(u);
        }
    }
}

/// A library album's cover URL; `v` (the album/song's cover_v) makes it cacheable for good.
pub fn cover_url(album_id: &str, v: &str, size: u32) -> String {
    let q: Vec<String> = [if size > 0 { format!("s={size}") } else { String::new() }, if v.is_empty() { String::new() } else { format!("v={v}") }]
        .into_iter().filter(|x| !x.is_empty()).collect();
    format!("/api/library/albums/{album_id}/cover{}", if q.is_empty() { String::new() } else { format!("?{}", q.join("&")) })
}

/// `?v=…` of a JSON album/song's cover_v ("" when it has none).
pub fn cover_q(o: &serde_json::Value) -> String {
    match o.get("cover_v") {
        Some(serde_json::Value::Number(n)) => format!("?v={n}"),
        Some(serde_json::Value::String(s)) if !s.is_empty() => format!("?v={s}"),
        _ => String::new(),
    }
}

/// Where `/cover` ends in a library/share cover URL (optionally followed by `?v=<digits>`), like
/// /\/api\/(library\/albums\/[^/]+|share\/[^/]+)\/cover(\?v=\d+)?$/
fn cover_base(src: &str) -> Option<usize> {
    let end = match src.rfind("?v=") {
        Some(q) if q + 3 < src.len() && src[q + 3..].bytes().all(|b| b.is_ascii_digit()) => q,
        _ => src.len(),
    };
    let head = &src[..end];
    if !head.ends_with("/cover") { return None; }
    let rest = &head[..head.len() - "/cover".len()];
    for prefix in ["/api/library/albums/", "/api/share/"] {
        if let Some(i) = rest.find(prefix) {
            let id = &rest[i + prefix.len()..];
            if !id.is_empty() && !id.contains('/') { return Some(end); }
        }
    }
    None
}

pub fn status_info(status: &str) -> (&'static str, String) {
    match status {
        "pending" => ("warn", "Awaiting approval".into()),
        "queued" => ("info", "Queued".into()),
        "searching" => ("info", "Searching".into()),
        "monitoring" => ("accent", "Monitoring".into()),
        "downloading" => ("accent", "Downloading".into()),
        "importing" => ("accent", "Importing".into()),
        "completed" => ("ok", "Available".into()),
        "failed" => ("err", "Failed".into()),
        "denied" => ("err", "Denied".into()),
        "cancelled" => ("", "Cancelled".into()),
        other => ("", other.to_string()),
    }
}

/// `statusBadge(status)`.
pub fn status_badge(status: &str) -> String {
    let (cls, label) = status_info(status);
    let spin = if matches!(status, "searching" | "downloading" | "importing" | "monitoring") { r#"<span class="spinner" style="width:10px;height:10px;border-width:2px"></span>"# } else { "" };
    format!(r#"<span class="badge {cls}">{spin}{}</span>"#, esc(&label))
}

/// `availability(item)`.
pub fn availability(item: &Value) -> String {
    if item.truthy("in_library") { return format!(r#"<span class="badge ok">{} In library</span>"#, icon_attr("check", r#"width="12" height="12""#)); }
    if let Some(s) = item.st("request_status") { return status_badge(s); }
    String::new()
}

/// `debounce(fn, ms)`: call `.run()` as often as you like, `f` runs `ms` after the last call.
pub struct Debounce { f: Rc<dyn Fn()>, ms: u32, pending: Rc<RefCell<Option<gloo_timers::callback::Timeout>>> }
impl Debounce {
    pub fn new(ms: u32, f: impl Fn() + 'static) -> Self { Self { f: Rc::new(f), ms, pending: Rc::new(RefCell::new(None)) } }
    pub fn run(&self) {
        let f = self.f.clone();
        let p = self.pending.clone();
        *self.pending.borrow_mut() = Some(gloo_timers::callback::Timeout::new(self.ms, move || { f(); p.borrow_mut().take(); }));
    }
}

/// `emptyState(icon, title, text)`.
pub fn empty_state(ic: &str, title: &str, text: &str) -> String {
    format!(r#"<div class="empty">{ic}<h3>{}</h3>{}</div>"#, esc(title), if text.is_empty() { String::new() } else { format!(r#"<p class="muted">{text}</p>"#) })
}

/// `skeletonGrid(n)`.
pub fn skeleton_grid(n: usize) -> String {
    let cell = r#"<div><div class="skeleton" style="aspect-ratio:1"></div><div class="skeleton" style="height:14px;margin-top:10px;width:70%"></div></div>"#;
    format!(r#"<div class="grid">{}</div>"#, cell.repeat(n))
}

/// `copyText(t)`.
/// `navigator.share` when the browser has it.
fn navigator_share() -> Option<js_sys::Function> {
    let nav: wasm_bindgen::JsValue = dom::window().navigator().into();
    js_sys::Reflect::get(&nav, &"share".into()).ok().and_then(|f| f.dyn_into::<js_sys::Function>().ok())
}
/// Share a library song: the system share sheet where there is one, otherwise the link goes to the clipboard.
pub async fn share_track(t: Value) {
    let res = crate::api::post_empty(&format!("/library/tracks/{}/share", t.s("id"))).await;
    share_link(res, &t.s("title"), &format!("{} – {}", t.s("title"), t.s("artist"))).await
}
/// Share a library album (same as `share_track`).
pub async fn share_album(id: String, a: Value) {
    let res = crate::api::post_empty(&format!("/library/albums/{id}/share")).await;
    share_link(res, &a.s("album"), &format!("{} – {}", a.s("album"), a.s("albumartist"))).await
}
/// Share a library artist by any spelling of its name (same as `share_track`).
pub async fn share_artist(name: String) {
    let res = crate::api::post("/library/artists/share", serde_json::json!({ "name": name })).await;
    share_link(res, &name, &name).await
}
async fn share_link(res: crate::api::ApiResult, title: &str, text: &str) {
    match res {
        Ok(r) => {
            let href = dom::location().href().unwrap_or_default();
            let url = match web_sys::Url::new_with_base(&r.s("path"), &href) {
                Ok(u) => u.href(),
                Err(e) => { toast(&e.dyn_ref::<js_sys::Error>().map(|e| String::from(e.message())).unwrap_or_else(|| e.as_string().unwrap_or_default()), "err"); return; }
            };
            if let Some(share) = navigator_share() {
                let data = js_sys::Object::new();
                let _ = js_sys::Reflect::set(&data, &"title".into(), &title.into());
                let _ = js_sys::Reflect::set(&data, &"text".into(), &text.into());
                let _ = js_sys::Reflect::set(&data, &"url".into(), &url.as_str().into());
                let nav: wasm_bindgen::JsValue = dom::window().navigator().into();
                if let Ok(p) = share.call1(&nav, &data) {
                    if let Ok(p) = p.dyn_into::<js_sys::Promise>() {
                        if wasm_bindgen_futures::JsFuture::from(p).await.is_ok() { return; }
                    }
                }
                // cancelled → copy
            }
            copy_text(&url);
            toast("Link copied – anyone with it can play this song", "ok");
        }
        Err(e) => toast(&e.message, "err"),
    }
}

pub fn copy_text(t: &str) {
    let t = t.to_string();
    dom::spawn(async move {
        let clip = dom::window().navigator().clipboard();
        match wasm_bindgen_futures::JsFuture::from(clip.write_text(&t)).await {
            Ok(_) => toast("Copied to clipboard", "ok"),
            Err(_) => toast("Copy failed – select and copy manually", "err"),
        }
    });
}

/// One entry of the ⋯ menu (`None` in the list = separator).
#[derive(Clone)]
pub struct MenuItem {
    pub label: String,
    pub icon: String,
    pub danger: bool,
    pub href: Option<String>,
    pub download: bool,
    pub on_click: Option<Rc<dyn Fn()>>,
}
impl MenuItem {
    pub fn new(label: &str, icon: &str, on_click: impl Fn() + 'static) -> Option<Self> {
        Some(Self { label: label.into(), icon: icon.into(), danger: false, href: None, download: false, on_click: Some(Rc::new(on_click)) })
    }
    pub fn danger(label: &str, icon: &str, on_click: impl Fn() + 'static) -> Option<Self> {
        Self::new(label, icon, on_click).map(|mut m| { m.danger = true; m })
    }
    pub fn link(label: &str, icon: &str, href: &str, download: bool) -> Option<Self> {
        Some(Self { label: label.into(), icon: icon.into(), danger: false, href: Some(href.into()), download, on_click: None })
    }
    /// A separator ("-" in the JS).
    pub const SEP: Option<MenuItem> = None;
}

thread_local! {
    /// Where the next menu opens instead of under its button: set by the right-click handler (app.rs) just
    /// before it clicks the row's ⋯ button, read once by `pop_menu`.
    static MENU_AT: std::cell::Cell<Option<(f64, f64)>> = const { std::cell::Cell::new(None) };
}
/// The next `pop_menu` opens at this viewport point (a right-click), then the override is used up.
pub fn next_menu_at(x: f64, y: f64) { MENU_AT.with(|m| m.set(Some((x, y)))); }

/// `popMenu(anchor, items)`: pops up next to the button on desktop, slides up as a sheet on phones.
/// Items are `Option<MenuItem>`; `None` is a separator (the JS `'-'`). Entries you want to skip: filter them out before.
pub fn pop_menu(anchor: &Element, items: Vec<Option<MenuItem>>) {
    let at = MENU_AT.with(|m| m.take());
    for n in dom::qsa(".menu-pop, .menu-bg") { n.remove(); }
    let bg = create("div");
    bg.set_class_name("menu-bg");
    let el = create("div");
    el.set_class_name("menu-pop");
    let _ = el.set_attribute("role", "menu");
    let html: String = items.iter().enumerate().map(|(i, it)| match it {
        None => "<hr>".to_string(),
        Some(it) => {
            let cls = if it.danger { "danger" } else { "" };
            match &it.href {
                Some(h) => format!(r#"<a role="menuitem" class="{cls}" href="{}" {} data-i="{i}">{}<span>{}</span></a>"#, esc(h), if it.download { "download" } else { "" }, it.icon, esc(&it.label)),
                None => format!(r#"<button role="menuitem" class="{cls}" data-i="{i}">{}<span>{}</span></button>"#, it.icon, esc(&it.label)),
            }
        }
    }).collect();
    set_html(&el, &html);
    let key_listener: Rc<RefCell<Option<EventListener>>> = Rc::new(RefCell::new(None));
    let (el2, bg2, kl) = (el.clone(), bg.clone(), key_listener.clone());
    let close: Rc<dyn Fn()> = Rc::new(move || { el2.remove(); bg2.remove(); kl.borrow_mut().take(); });
    let c = close.clone();
    *key_listener.borrow_mut() = Some(dom::listen(&document(), "keydown", move |e| { if dom::key(e) == "Escape" { c(); } }));
    let c = close.clone();
    on(&bg, "click", move |_| c());
    let items = Rc::new(items);
    let c = close.clone();
    on(&el, "click", move |e| {
        let Some(b) = closest(e, "[data-i]") else { return };
        let i: usize = dom::data(&b, "i").parse().unwrap_or(usize::MAX);
        let Some(Some(it)) = items.get(i) else { return };
        c();
        if let Some(f) = &it.on_click { e.prevent_default(); f(); }
    });
    let _ = body().append_child(&bg);
    let _ = body().append_child(&el);
    if !dom::is_phone() {
        let h = el.dyn_ref::<web_sys::HtmlElement>().map(|h| (h.offset_width() as f64, h.offset_height() as f64)).unwrap_or((0.0, 0.0));
        let (w, hh) = h;
        let win = dom::window();
        let iw = win.inner_width().ok().and_then(|v| v.as_f64()).unwrap_or(0.0);
        let ih = win.inner_height().ok().and_then(|v| v.as_f64()).unwrap_or(0.0);
        let (left, top) = match at {
            // a right-click: the menu's corner sits at the pointer, flipped when it would leave the window
            Some((x, y)) => {
                let left = if x + w + 8.0 > iw { (x - w).max(8.0) } else { x };
                let top = if y + hh + 8.0 > ih { (y - hh).max(8.0) } else { y };
                (left, top)
            }
            None => {
                let r = anchor.get_bounding_client_rect();
                let left = (r.right() - w).min(iw - w - 8.0).max(8.0);
                let top = if r.bottom() + hh + 8.0 > ih { (r.top() - hh - 6.0).max(8.0) } else { r.bottom() + 6.0 };
                (left, top)
            }
        };
        dom::set_style(&el, "left", &format!("{left}px"));
        dom::set_style(&el, "top", &format!("{top}px"));
    }
    keep(key_listener);
}

// ---- show / hide for anything sensitive (same markup as ui.js) ----
pub const EYE: &str = r#"<svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M2 12s3.6-7 10-7 10 7 10 7-3.6 7-10 7S2 12 2 12z"/><circle cx="12" cy="12" r="3"/></svg>"#;
pub const EYE_OFF: &str = r#"<svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M17.9 17.9A10.4 10.4 0 0 1 12 19c-6.4 0-10-7-10-7a18.5 18.5 0 0 1 5.1-5.9M9.9 5.2A9.6 9.6 0 0 1 12 5c6.4 0 10 7 10 7a18.6 18.6 0 0 1-2.2 3.2M14.1 14.1a3 3 0 1 1-4.2-4.2"/><path d="m2 2 20 20"/></svg>"#;

fn eye_button(on_toggle: impl Fn(bool) + 'static) -> Element {
    let b = create("button");
    let _ = b.set_attribute("type", "button");
    b.set_class_name("eye-btn");
    let _ = b.set_attribute("aria-label", "Show");
    let _ = b.set_attribute("title", "Show");
    set_html(&b, EYE);
    let shown = Rc::new(RefCell::new(false));
    let b2 = b.clone();
    dom::on_forever(&b, "click", move |e| {
        e.prevent_default();
        e.stop_propagation();
        let s = { let mut v = shown.borrow_mut(); *v = !*v; *v };
        set_html(&b2, if s { EYE_OFF } else { EYE });
        let _ = b2.set_attribute("title", if s { "Hide" } else { "Show" });
        let _ = b2.set_attribute("aria-label", if s { "Hide" } else { "Show" });
        on_toggle(s);
    });
    b
}

fn wrap_password(input: &Element) {
    dom::set_data(input, "eye", "1");
    let wrap = create("span");
    wrap.set_class_name(if dom::has_class(input, "grow") { "pw-wrap grow" } else { "pw-wrap" });
    if let Some(h) = input.dyn_ref::<web_sys::HtmlElement>() {
        let mw = h.style().get_property_value("max-width").unwrap_or_default();
        if !mw.is_empty() { dom::set_style(&wrap, "max-width", &mw); }
    }
    if let Some(parent) = input.parent_node() { let _ = parent.insert_before(&wrap, Some(input)); }
    let _ = wrap.append_child(input);
    let i2 = input.clone();
    let _ = wrap.append_child(&eye_button(move |shown| { let _ = i2.set_attribute("type", if shown { "text" } else { "password" }); }));
}

fn dots(n: usize) -> String { "•".repeat(n.clamp(8, 32)) }

/// `setSecretText(el, value)`: dots with an eye toggle (value kept in data-value).
pub fn set_secret_text(el: &Element, value: &str) {
    dom::set_data(el, "value", value);
    let has_btn = el.next_element_sibling().map(|n| dom::has_class(&n, "eye-btn")).unwrap_or(false);
    el.set_text_content(Some(&if dom::data(el, "shown") == "1" { value.to_string() } else { dots(value.len()) }));
    if !has_btn {
        dom::set_data(el, "eye", "1");
        let e2 = el.clone();
        let _ = el.after_with_node_1(&eye_button(move |shown| {
            dom::set_data(&e2, "shown", if shown { "1" } else { "" });
            let v = dom::data(&e2, "value");
            e2.set_text_content(Some(&if shown { v.clone() } else { dots(v.len()) }));
        }));
    }
}

fn enhance_secrets(root: &Element) {
    for i in qsa_in(root, "input[type=password]:not([data-eye])") { wrap_password(&i); }
    for el in qsa_in(root, "[data-secret]:not([data-eye])") {
        let v = dom::data_opt(&el, "value").unwrap_or_else(|| el.text_content().unwrap_or_default());
        set_secret_text(&el, &v);
    }
}

/// Starts the password-eye observer (the JS did this at module load).
pub fn init() {
    let b: Element = body().into();
    enhance_secrets(&b);
    let cb = wasm_bindgen::closure::Closure::<dyn FnMut(js_sys::Array, web_sys::MutationObserver)>::new(move |muts: js_sys::Array, _o| {
        for m in muts.iter() {
            let Ok(m) = m.dyn_into::<web_sys::MutationRecord>() else { continue };
            let added = m.added_nodes();
            for i in 0..added.length() {
                let Some(n) = added.get(i) else { continue };
                let Ok(el) = n.dyn_into::<Element>() else { continue };
                let target = if el.matches("input[type=password]").unwrap_or(false) { el.parent_element().unwrap_or(el) } else { el };
                enhance_secrets(&target);
            }
        }
    });
    if let Ok(obs) = web_sys::MutationObserver::new(cb.as_ref().unchecked_ref()) {
        let init = web_sys::MutationObserverInit::new();
        init.set_child_list(true);
        init.set_subtree(true);
        let _ = obs.observe_with_options(&body(), &init);
        dom::keep_forever(obs);
    }
    cb.forget();
}

/// `artistUrl(a)`: where an artist name goes – its MusicBrainz page when the id is known, else the name (library or catalog page).
pub fn artist_url(a: &Value) -> String {
    if a.truthy("mbid") { format!("#/artist/{}", String::from(js_sys::encode_uri_component(&a.s("mbid")))) }
    else { format!("#/artist-name/{}", String::from(js_sys::encode_uri_component(&a.s("name")))) }
}
/// `artistLinks(t)`: a song's artist text with each of its artists as its own link ("Travis Scott feat. The Weeknd" →
/// two links, the words between them kept as they are). Uses `artists` from the server; without it the whole text is one link.
pub fn artist_links(t: &Value) -> String {
    let s = t.s("artist");
    if s.is_empty() { return String::new(); }
    // a translated name is one link to the artist as written
    if translate_mode() != "off" && (t.truthy("artist_t") || t.truthy("artist_r")) && tr_inline(t, "artist") != s {
        return format!(r#"<a class="artist-link" href="{}">{}</a>"#, artist_url(&serde_json::json!({ "name": s })), esc(&tr_inline(t, "artist")));
    }
    let given = t.a("artists");
    let list = if given.is_empty() { vec![serde_json::json!({ "name": s })] } else { given };
    let (mut out, mut at) = (String::new(), 0usize);
    for a in &list {
        let name = a.s("name");
        if name.is_empty() { continue; }
        let Some(i) = s[at..].find(&name).map(|i| i + at) else { continue };
        out.push_str(&format!(r#"{}<a class="artist-link" href="{}">{}</a>"#, esc(&s[at..i]), artist_url(a), esc(&name)));
        at = i + name.len();
    }
    if at == 0 { return format!(r#"<a class="artist-link" href="{}">{}</a>"#, artist_url(&serde_json::json!({ "name": s })), esc(&s)); }
    out + &esc(&s[at..])
}
/// `albumLink(t)`: a song's album name as a link to the album (library album page), or plain text when it has no album id.
pub fn album_link(t: &Value) -> String {
    if !t.truthy("album") { return String::new(); }
    if t.truthy("album_id") { format!(r##"<a class="album-link" href="#/library/album/{}">{}</a>"##, esck(t, "album_id"), esc(&tr_inline(t, "album"))) } else { esc(&tr_inline(t, "album")) }
}

// ---- translated titles (Settings → Language) ------------------------------------------------------
// The server adds <field>_t (translation) and <field>_r (romanization) to titles, albums and artists for
// listeners who turned translations on; these pick what to show for the mode (web/js/ui.js: tr, trSub, trInline).
thread_local! {
    static TR_MODE: RefCell<&'static str> = const { RefCell::new("off") };
}
/// `setTranslateMode(m)`.
pub fn set_translate_mode(m: &str) {
    let m: &'static str = match m { "under" => "under", "instead" => "instead", "romanized" => "romanized", _ => "off" };
    TR_MODE.with(|x| *x.borrow_mut() = m);
}
/// `translateMode()`.
pub fn translate_mode() -> &'static str { TR_MODE.with(|x| *x.borrow()) }
fn tr_pick(o: &Value, f: &str, suffixes: &[&str]) -> Option<String> {
    suffixes.iter().map(|x| o.s(&format!("{f}{x}"))).find(|v| !v.is_empty())
}
/// `tr(o, f)`: the original, its translation (instead; the romanization when there is none) or its romanization.
pub fn tr(o: &Value, f: &str) -> String {
    let orig = o.s(f);
    match translate_mode() {
        "instead" => tr_pick(o, f, &["_t", "_r"]).unwrap_or(orig),
        "romanized" => tr_pick(o, f, &["_r"]).unwrap_or(orig),
        _ => orig,
    }
}
/// `trSub(o, f)`: the line under a title when the translation shows under the original (HTML).
pub fn tr_sub(o: &Value, f: &str) -> String {
    if translate_mode() != "under" { return String::new(); }
    match tr_pick(o, f, &["_t", "_r"]) {
        Some(t) => format!(r#"<div class="tr-sub small faint ellipsis">{}</div>"#, esc(&t)),
        None => String::new(),
    }
}
/// `trInline(o, f)`: "Original (Translation)" under the original, else as `tr`.
pub fn tr_inline(o: &Value, f: &str) -> String {
    if translate_mode() != "under" { return tr(o, f); }
    match tr_pick(o, f, &["_t", "_r"]) {
        Some(t) => format!("{} ({t})", o.s(f)),
        None => o.s(f),
    }
}

#[cfg(test)]
mod tests {
    use super::initials;

    #[test]
    fn initials_never_panic_on_multibyte_names() {
        // byte 4 falls inside a character for every one of these
        for s in ["ペルソナ4 サウンドトラック", "平田志穂子", "“Quoted” Album", "한국어 앨범", "…and more", "é…x", "🎉🎉"] {
            let _ = initials(s);
        }
        assert_eq!(initials("ペルソナ4"), "ペ");
        assert_eq!(initials("the Beatles"), "B");
        assert_eq!(initials("The"), "T");
        assert_eq!(initials("  "), "?");
        assert_eq!(initials("éclair"), "É");
    }
}
