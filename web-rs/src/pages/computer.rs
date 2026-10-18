//! Port of web/js/pages/computer.js – see docs/archive/ui-rewrite/CONTRACT.md
//! Settings → This computer: only in the Encore desktop app (window.EncoreNative.desktopSettings,
//! clients/app/src/extras.rs). Start with the computer, the mini player, global keyboard shortcuts, offline
//! songs and updates.

use crate::app::{self, Ctx, PageFuture};
use crate::dom;
use crate::json::{js_string, J};
use crate::ui::{esc, toast};
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{Element, KeyboardEvent};

pub const KEYS: [(&str, &str); 5] = [("toggle", "Play / pause"), ("next", "Next song"), ("prev", "Previous song"), ("volume_up", "Volume up"), ("volume_down", "Volume down")];

/// `String(err?.message || err)`.
fn err_text(e: &JsValue) -> String {
    if let Some(s) = e.as_string() { return s; }
    if let Ok(m) = js_sys::Reflect::get(e, &"message".into()) {
        if let Some(s) = m.as_string() { if !s.is_empty() { return s; } }
    }
    String::from(js_sys::JsString::from(e.clone()))
}

/// `parse(await window.EncoreNative.<name>(arg))` with the rejection as text.
async fn call(name: &str, arg: Option<&str>) -> Result<Value, String> {
    let n = js_sys::Reflect::get(&dom::window(), &"EncoreNative".into()).map_err(|e| err_text(&e))?;
    let f = js_sys::Reflect::get(&n, &name.into()).map_err(|e| err_text(&e))?
        .dyn_into::<js_sys::Function>().map_err(|_| format!("{name} is missing"))?;
    let r = match arg { Some(a) => f.call1(&n, &a.into()), None => f.call0(&n) }.map_err(|e| err_text(&e))?;
    let v = match r.dyn_into::<js_sys::Promise>() {
        Ok(p) => wasm_bindgen_futures::JsFuture::from(p).await.map_err(|e| err_text(&e))?,
        Err(v) => v,
    };
    if let Some(s) = v.as_string() { return serde_json::from_str(&s).map_err(|e| e.to_string()); }
    Ok(serde_wasm_bindgen::from_value::<Value>(v).ok().filter(|v| !v.is_null()).unwrap_or_else(|| json!({})))
}

/// `accelerator(e)`: a key press as a shortcut ("Ctrl+Alt+P"): Some("") clears (Backspace / Delete alone),
/// None while only modifiers are down.
pub fn accelerator(key: &str, code: &str, ctrl: bool, alt: bool, shift: bool, meta: bool) -> Option<String> {
    let mods = ctrl || alt || shift || meta;
    if (key == "Backspace" || key == "Delete") && !mods { return Some(String::new()); }
    if ["Control", "Alt", "Shift", "Meta", "OS", "AltGraph"].contains(&key) || code.is_empty() { return None; }
    let mut parts: Vec<String> = vec![];
    if ctrl { parts.push("Ctrl".into()); }
    if alt { parts.push("Alt".into()); }
    if shift { parts.push("Shift".into()); }
    if meta { parts.push("Super".into()); }
    let short = if let Some(c) = code.strip_prefix("Key").filter(|c| c.len() == 1 && c.chars().all(|ch| ch.is_ascii_uppercase())) { c.to_string() }
        else if let Some(d) = code.strip_prefix("Digit").filter(|d| d.len() == 1 && d.chars().all(|ch| ch.is_ascii_digit())) { d.to_string() }
        else if let Some(a) = code.strip_prefix("Arrow").filter(|a| matches!(*a, "Up" | "Down" | "Left" | "Right")) { a.to_string() }
        else { code.to_string() };
    parts.push(short);
    Some(parts.join("+"))
}

/// `pageHtml(s, upd, busy)`: `upd` is the last update check this visit (Null = none).
pub fn page_html(s: &Value, upd: &Value, busy: bool) -> String {
    let h = s.o("hotkeys");
    let errs = s.o("hotkey_errors");
    let on = h.truthy("on");
    let tray = if s.s("platform") == "macos" { "menu bar" } else { "tray" };
    let keys: String = KEYS.iter().map(|(k, l)| format!(r#"<label class="stack" style="gap:4px"><span class="small muted">{}</span><input class="input mono" data-key="{k}" value="{}" placeholder="Not set" readonly {}>{}</label>"#,
        esc(l), esc(&if h.truthy(k) { h.s(k) } else { String::new() }), if on { "" } else { "disabled" },
        if errs.truthy(k) { format!(r#"<span class="small" style="color:var(--err)">{}</span>"#, esc(&errs.s(k))) } else { String::new() })).collect();
    let has_upd = !upd.is_null();
    let version = if upd.truthy("available") { upd.s("version") } else if has_upd { String::new() } else if s.truthy("update") { s.s("update") } else { String::new() };
    let line = if busy { "Working…".to_string() }
        else if upd.truthy("error") { upd.s("error") }
        else if !version.is_empty() { format!("Encore {version} is ready to install.") }
        else if has_upd { "Encore is up to date.".to_string() } else { String::new() };
    let err = upd.truthy("error");
    let updates = if s.truthy("updates") {
        format!(r#"<label class="row" style="gap:8px;align-items:center"><input type="checkbox" id="pc-autoupdate" {}> Look for a new version when Encore starts</label>
      {}
      <div class="row wrap"><button type="button" class="btn sm" id="pc-check" {}>Check for updates</button>{}</div>"#,
            if s.truthy("check_updates") { "checked" } else { "" },
            if line.is_empty() { String::new() } else { format!(r#"<span class="small {}" {}>{}</span>"#, if err { "" } else { "muted" }, if err { r#"style="color:var(--err)""# } else { "" }, esc(&line)) },
            if busy { "disabled" } else { "" },
            if version.is_empty() { String::new() } else { format!(r#"<button type="button" class="btn sm primary" id="pc-install" {}>Install {} and restart</button>"#, if busy { "disabled" } else { "" }, esc(&version)) })
    } else {
        r#"<span class="small muted">This build can't update itself. New versions are on <a href="https://github.com/Krifmaz/Encore/releases" target="_blank" rel="noopener">github.com/Krifmaz/Encore/releases</a>.</span>"#.to_string()
    };
    let offline = s.i("offline");
    let offline_text = if offline > 0 { format!("{offline} song(s) saved on this computer.") } else { "No songs saved on this computer yet.".to_string() };
    let version_text = if s.truthy("version") { js_string(s.g("version")) } else { "?".to_string() };
    format!(r#"<div class="card stack">
    <h2>This computer</h2>
    <p class="small muted" style="margin:0">Settings of the Encore app on this computer. They stay on this computer and are not saved to your account.</p>
    <div class="stack" style="gap:6px"><h3>Start</h3>
      <label class="row" style="gap:8px;align-items:center"><input type="checkbox" id="pc-autostart" {autostart}> Start Encore when I sign in to this computer</label>
      <span class="small muted">It starts in the {tray} without a window. Click the icon to open it.</span></div>
    <div class="stack" style="gap:6px"><h3>Mini player</h3>
      <div class="row"><button type="button" class="btn sm" id="pc-mini">Open or close the mini player</button></div>
      <span class="small muted">A small window that stays on top of the others, with the cover, the song, the buttons and the seek bar. Also in the {tray} menu and in the player's ⋯ menu.</span></div>
    <div class="stack" style="gap:6px"><h3>Keyboard shortcuts</h3>
      <label class="row" style="gap:8px;align-items:center"><input type="checkbox" id="pc-keys" {keys_on}> Use these shortcuts in every program</label>
      <div class="stack" style="gap:8px;{dim}">{keys}</div>
      <span class="small muted">Click a box and press the keys you want, for example Ctrl+Alt+P. Backspace clears it. The media keys on your keyboard always work.</span>
      <div class="row"><button type="button" class="btn sm" id="pc-keys-reset">Back to the standard shortcuts</button></div></div>
    <div class="stack" style="gap:6px"><h3>Offline music</h3>
      <span class="small muted">{offline_text} Save songs, albums and playlists with ⋯ → Save for offline. When the server can't be reached, they play through Cadenza.</span>
      <div class="row"><button type="button" class="btn sm" id="pc-offline" {offline_dis}>Open offline music</button></div></div>
    <div class="stack" style="gap:6px"><h3>Updates</h3>
      <span class="small muted">Version {version}</span>
      {updates}</div>
  </div>"#,
        autostart = if s.truthy("autostart") { "checked" } else { "" },
        keys_on = if on { "checked" } else { "" },
        dim = if on { "" } else { "opacity:.45" },
        offline_dis = if offline > 0 { "" } else { "disabled" },
        version = esc(&version_text))
}

struct Page {
    el: Element,
    s: RefCell<Value>,
    upd: RefCell<Value>,
    busy: Cell<bool>,
}

fn set(p: &Rc<Page>, patch: Value) {
    let p = p.clone();
    dom::spawn(async move {
        match call("setDesktopSettings", Some(&patch.to_string())).await {
            Ok(s) => { *p.s.borrow_mut() = s; toast("Saved", "ok"); }
            Err(e) => toast(&e, "err"),
        }
        paint(&p);
    });
}

/// `{ ...h, <key>: value }` of the current shortcuts.
fn hotkeys_with(p: &Page, key: &str, value: Value) -> Value {
    let mut h = p.s.borrow().o("hotkeys");
    if let Some(o) = h.as_object_mut() { o.insert(key.to_string(), value); }
    h
}

fn paint(p: &Rc<Page>) {
    dom::set_html(&p.el, &page_html(&p.s.borrow(), &p.upd.borrow(), p.busy.get()));
    let el = &p.el;
    if let Some(b) = dom::qs_in(el, "#pc-autostart") {
        let (p2, b2) = (p.clone(), b.clone());
        dom::on(&b, "change", move |_| set(&p2, json!({ "autostart": dom::checked(&b2) })));
    }
    if let Some(b) = dom::qs_in(el, "#pc-mini") { dom::on(&b, "click", |_| app::native_call("miniPlayer")); }
    if let Some(b) = dom::qs_in(el, "#pc-keys") {
        let (p2, b2) = (p.clone(), b.clone());
        dom::on(&b, "change", move |_| { let h = hotkeys_with(&p2, "on", json!(dom::checked(&b2))); set(&p2, json!({ "hotkeys": h })); });
    }
    if let Some(b) = dom::qs_in(el, "#pc-keys-reset") {
        let p2 = p.clone();
        dom::on(&b, "click", move |_| { let d = p2.s.borrow().g("default_hotkeys").clone(); set(&p2, json!({ "hotkeys": d })); });
    }
    for b in dom::qsa_in(el, "[data-key]") {
        let (p2, b2) = (p.clone(), b.clone());
        dom::on(&b, "keydown", move |e| {
            let Some(k) = e.dyn_ref::<KeyboardEvent>() else { return };
            if k.key() == "Tab" && !k.ctrl_key() && !k.alt_key() && !k.meta_key() { return; }
            e.prevent_default();
            let Some(acc) = accelerator(&k.key(), &k.code(), k.ctrl_key(), k.alt_key(), k.shift_key(), k.meta_key()) else { return };
            let key = dom::data(&b2, "key");
            let now = { let h = p2.s.borrow().o("hotkeys"); if h.truthy(&key) { h.s(&key) } else { String::new() } };
            if acc == now { return; }
            let h = hotkeys_with(&p2, &key, json!(acc));
            set(&p2, json!({ "hotkeys": h }));
        });
    }
    if let Some(b) = dom::qs_in(el, "#pc-offline") { dom::on(&b, "click", |_| app::native_call("openOffline")); }
    if let Some(b) = dom::qs_in(el, "#pc-autoupdate") {
        let (p2, b2) = (p.clone(), b.clone());
        dom::on(&b, "change", move |_| set(&p2, json!({ "check_updates": dom::checked(&b2) })));
    }
    if let Some(b) = dom::qs_in(el, "#pc-check") {
        let p2 = p.clone();
        dom::on(&b, "click", move |_| {
            let p3 = p2.clone();
            dom::spawn(async move {
                p3.busy.set(true); paint(&p3);
                let r = call("checkUpdate", None).await.unwrap_or_else(|e| json!({ "error": e }));
                *p3.upd.borrow_mut() = r;
                p3.busy.set(false); paint(&p3);
            });
        });
    }
    if let Some(b) = dom::qs_in(el, "#pc-install") {
        let p2 = p.clone();
        dom::on(&b, "click", move |_| {
            let p3 = p2.clone();
            dom::spawn(async move {
                p3.busy.set(true); paint(&p3);
                let r = call("installUpdate", None).await.unwrap_or_else(|e| json!({ "ok": false, "error": e }));
                // on success the app restarts and this page is gone
                p3.busy.set(false);
                if !r.truthy("ok") {
                    let msg = if r.truthy("error") { r.s("error") } else { "The update didn't install.".to_string() };
                    toast(&msg, "err");
                    *p3.upd.borrow_mut() = json!({ "error": msg });
                }
                paint(&p3);
            });
        });
    }
}

pub fn render(el: Element, _ctx: Ctx) -> PageFuture {
    Box::pin(async move {
        if !app::native_has("desktopSettings") {
            dom::set_html(&el, r#"<div class="card stack"><h2>This computer</h2><p class="small muted" style="margin:0">These settings are in the Encore desktop app.</p></div>"#);
            return Ok(());
        }
        let s = match call("desktopSettings", None).await {
            Ok(s) => s,
            Err(e) => { dom::set_html(&el, &format!(r#"<div class="error-box">{}</div>"#, esc(&e))); return Ok(()); }
        };
        let p = Rc::new(Page { el, s: RefCell::new(s), upd: RefCell::new(Value::Null), busy: Cell::new(false) });
        paint(&p);
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accelerators_read_like_the_js() {
        assert_eq!(accelerator("p", "KeyP", true, true, false, false).as_deref(), Some("Ctrl+Alt+P"));
        assert_eq!(accelerator("ArrowUp", "ArrowUp", true, false, true, false).as_deref(), Some("Ctrl+Shift+Up"));
        assert_eq!(accelerator("1", "Digit1", false, true, false, false).as_deref(), Some("Alt+1"));
        assert_eq!(accelerator("F7", "F7", false, false, false, false).as_deref(), Some("F7"));
        assert_eq!(accelerator("Backspace", "Backspace", false, false, false, false).as_deref(), Some(""));
        assert_eq!(accelerator("Control", "ControlLeft", true, false, false, false), None);
    }

    #[test]
    fn the_page_shows_what_the_app_reports() {
        let s = json!({ "autostart": true, "platform": "windows", "version": "0.27.0", "offline": 3, "updates": true, "check_updates": true, "update": "0.27.1",
            "hotkeys": { "on": true, "toggle": "Ctrl+Alt+Space", "next": "" }, "hotkey_errors": { "toggle": "Ctrl+Alt+Space is taken by another program" } });
        let h = page_html(&s, &Value::Null, false);
        assert!(h.contains(r#"id="pc-autostart" checked"#));
        assert!(h.contains("3 song(s) saved on this computer."));
        assert!(h.contains("Install 0.27.1 and restart"));
        assert!(h.contains("is taken by another program"));
        assert!(h.contains("in the tray without a window"));
        let h = page_html(&s, &json!({ "available": false }), false);
        assert!(h.contains("Encore is up to date.") && !h.contains("pc-install"));
        let h = page_html(&json!({ "updates": false, "platform": "macos" }), &Value::Null, false);
        assert!(h.contains("can't update itself") && h.contains("menu bar") && h.contains("No songs saved"));
    }
}
