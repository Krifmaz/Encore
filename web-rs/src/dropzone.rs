//! Drag and drop for every file upload: one set of listeners on the document turns any upload into a drop place.
//!
//! A drop place ("zone") is either
//! - an element marked `data-dropzone`: its value is the `id` of the file input it feeds (`data-dropzone="bk-upload"`),
//!   or, left empty, the first `input[type=file]` inside it; or
//! - a `<label>` that holds a file input (the usual "Upload…" button), which needs no mark at all.
//!
//! A drop goes through the input itself: the files are checked against the input's `accept` (the same rule the
//! browser's file chooser uses), only the first is kept unless the input has `multiple`, and they are put in
//! `input.files` before a bubbling `change` event fires. So each upload keeps one code path – its own `change`
//! handler – and a dropped file is checked exactly like a picked one.
//!
//! While files from the computer are dragged over the window every zone on screen shows the `drop-ready`
//! outline, and the zone under the pointer gets `drop-over` (the styles are in `static/css/app.css`). Files
//! dropped anywhere else are swallowed, so a missed drop never opens the file in the tab and leaves Encore.
//! Pages with drag and drop of their own (the file manager) handle the event first and call `preventDefault()`;
//! the listeners here then stay out of the way.

use crate::dom;
use crate::ui::toast;
use js_sys::{Array, Reflect};
use std::cell::Cell;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{Element, Event, File, HtmlInputElement};

/// The class every zone on screen carries while files are dragged over the window.
pub const READY_CLASS: &str = "drop-ready";
/// The class of the zone under the pointer while files are dragged over it.
pub const OVER_CLASS: &str = "drop-over";

/// Reads `o[k]` from a JS object (`undefined` when it is missing).
fn get(o: &JsValue, k: &str) -> JsValue { Reflect::get(o, &JsValue::from_str(k)).unwrap_or(JsValue::UNDEFINED) }

/// Whether a drag carries files from the computer (not text or a link from the page).
pub fn has_files(e: &Event) -> bool {
    let types = get(&get(e.as_ref(), "dataTransfer"), "types");
    if types.is_undefined() || types.is_null() { return false; }
    Array::from(&types).iter().any(|t| t.as_string().as_deref() == Some("Files"))
}

/// Sets what the pointer shows over the drop place ("copy", or "none" where nothing may be dropped).
pub fn set_drop_effect(e: &Event, effect: &str) {
    let dt = get(e.as_ref(), "dataTransfer");
    if dt.is_object() { let _ = Reflect::set(&dt, &"dropEffect".into(), &effect.into()); }
}

// ---- which files an input takes ------------------------------------------------------------------------------

/// Whether a file with this `name` and MIME type `mime` passes an input's `accept` attribute, the way the
/// browser's file chooser reads it: a comma separated list of `.ext` (the name's ending, any case), `type/*`
/// (any subtype) and `type/subtype` (exactly). An empty `accept` takes every file.
pub fn accepts(accept: &str, name: &str, mime: &str) -> bool {
    let tokens: Vec<String> = accept.split(',').map(|t| t.trim().to_ascii_lowercase()).filter(|t| !t.is_empty()).collect();
    if tokens.is_empty() { return true; }
    let (name, mime) = (name.to_ascii_lowercase(), mime.trim().to_ascii_lowercase());
    tokens.iter().any(|t| {
        if t.starts_with('.') {
            name.ends_with(t.as_str())
        } else if let Some(major) = t.strip_suffix("/*") {
            // "image/*" takes "image/png"; a file the browser has no type for ("") never matches a type token
            mime.split_once('/').is_some_and(|(m, _)| m == major)
        } else {
            !mime.is_empty() && mime == *t
        }
    })
}

/// The extensions an `accept` names, for the message about a refused file (".zip, .db"; "" when it names only
/// types such as `image/*`).
pub fn accept_hint(accept: &str) -> String {
    let exts: Vec<&str> = accept.split(',').map(str::trim).filter(|t| t.starts_with('.')).collect();
    exts.join(", ")
}

/// Splits a drop by an input's rule: (the files to hand over, the names of those refused). Without `multiple`
/// only the first acceptable file is kept and every other one counts as refused.
pub fn choose<T>(files: Vec<T>, accept: &str, multiple: bool, name: impl Fn(&T) -> String, mime: impl Fn(&T) -> String) -> (Vec<T>, Vec<String>) {
    let (mut keep, mut refused) = (vec![], vec![]);
    for f in files {
        if accepts(accept, &name(&f), &mime(&f)) && (multiple || keep.is_empty()) { keep.push(f); } else { refused.push(name(&f)); }
    }
    (keep, refused)
}

/// The toast after a drop where some files were refused ("" when none were).
pub fn refused_message(refused: &[String], accept: &str, multiple: bool, kept: usize) -> String {
    if refused.is_empty() { return String::new(); }
    let hint = accept_hint(accept);
    let takes = if hint.is_empty() { String::new() } else { format!(" (it takes {hint})") };
    match (kept, refused) {
        // one file wanted and one used: the rest were extras
        (1, _) if !multiple => "Only one file can be uploaded here: the first was used".to_string(),
        (_, [one]) => format!("{one} can't be uploaded here{takes}"),
        (_, many) => format!("{} files can't be uploaded here{takes}", many.len()),
    }
}

// ---- finding the zone and its input ----------------------------------------------------------------------------

/// The zone an element sits in: the nearest `[data-dropzone]`, or a `<label>` holding a file input, that has an
/// input to feed. Returns (zone, input).
fn zone_of(start: &Element) -> Option<(Element, HtmlInputElement)> {
    let mut el = Some(start.clone());
    while let Some(cur) = el {
        if let Some(id) = cur.get_attribute("data-dropzone") {
            let input = if id.trim().is_empty() { dom::qs_in(&cur, "input[type=file]") } else { dom::by_id(id.trim()) };
            if let Some(i) = input.and_then(|i| i.dyn_into::<HtmlInputElement>().ok()) { return Some((cur, i)); }
        } else if cur.tag_name().eq_ignore_ascii_case("label") {
            if let Some(i) = dom::qs_in(&cur, "input[type=file]").and_then(|i| i.dyn_into::<HtmlInputElement>().ok()) { return Some((cur, i)); }
        }
        el = cur.parent_element();
    }
    None
}

/// The zone under an event's target, if any.
fn zone_at(e: &Event) -> Option<(Element, HtmlInputElement)> { dom::target_el(e).and_then(|t| zone_of(&t)) }

/// Whether an input may take a drop now (not disabled, not opted out with `data-no-drop`).
fn usable(input: &HtmlInputElement) -> bool { !input.disabled() && !input.has_attribute("data-no-drop") }

/// Every zone on screen: the marked elements and the labels around file inputs (left out when their input
/// opted out or is disabled).
fn all_zones() -> Vec<Element> {
    let mut out = dom::qsa("[data-dropzone]");
    for i in dom::qsa("label input[type=file]") {
        if let Some(l) = i.closest("label").ok().flatten() {
            if !out.contains(&l) { out.push(l); }
        }
    }
    out.retain(|z| zone_of(z).is_some_and(|(_, i)| usable(&i)));
    out
}

thread_local! {
    /// How deep the drag is inside the window: `dragenter` adds one, `dragleave` takes one (they come in pairs
    /// for every element crossed), so 0 means the files left the window.
    static DEPTH: Cell<i32> = const { Cell::new(0) };
}

/// Shows (or takes away) the `drop-ready` outline on every zone on screen.
fn set_ready(on: bool) {
    let zones = if on { all_zones() } else { dom::qsa(&format!(".{READY_CLASS}, .{OVER_CLASS}")) };
    for z in zones {
        dom::toggle_class(&z, READY_CLASS, on);
        if !on { dom::remove_class(&z, OVER_CLASS); }
    }
}

/// Moves the `drop-over` highlight to `zone` (or takes it off everywhere).
fn set_over(zone: Option<&Element>) {
    for z in dom::qsa(&format!(".{OVER_CLASS}")) {
        if Some(&z) != zone { dom::remove_class(&z, OVER_CLASS); }
    }
    if let Some(z) = zone { dom::add_class(z, OVER_CLASS); }
}

/// The drag ended (dropped, cancelled or left the window): every highlight goes.
fn reset() {
    DEPTH.with(|d| d.set(0));
    set_ready(false);
}

/// Puts `files` into `input` and fires its `change` event, as if they had been picked.
fn hand_over(input: &HtmlInputElement, files: &[File]) {
    let Ok(dt) = web_sys::DataTransfer::new() else { return };
    for f in files { let _ = dt.items().add_with_file(f); }
    input.set_files(dt.files().as_ref());
    let init = web_sys::EventInit::new();
    init.set_bubbles(true);
    if let Ok(ev) = Event::new_with_event_init_dict("change", &init) { let _ = input.dispatch_event(&ev); }
}

/// The files a drop carries (`dataTransfer.files`; folders are left out, they have no type and no size).
fn dropped_files(e: &Event) -> Vec<File> {
    let list = get(&get(e.as_ref(), "dataTransfer"), "files");
    let n = get(&list, "length").as_f64().unwrap_or(0.0) as u32;
    (0..n).filter_map(|i| Reflect::get_u32(&list, i).ok()?.dyn_into::<File>().ok()).collect()
}

/// A drop on a zone: check the files like the input's chooser does and hand the good ones to it.
fn drop_on(input: &HtmlInputElement, files: Vec<File>) {
    let accept = input.accept();
    let (keep, refused) = choose(files, &accept, input.multiple(), |f| f.name(), |f| f.type_());
    let msg = refused_message(&refused, &accept, input.multiple(), keep.len());
    if !msg.is_empty() { toast(&msg, if keep.is_empty() { "err" } else { "" }); }
    if !keep.is_empty() { hand_over(input, &keep); }
}

/// Starts drag and drop for the whole app (called once at start-up). The listeners sit on the document for
/// the whole session, so zones drawn later – pages, dialogs, cards – work without any setup of their own.
pub fn init() {
    dom::on_forever(&dom::document(), "dragenter", |e| {
        if !has_files(e) { return; }
        let first = DEPTH.with(|d| { d.set(d.get() + 1); d.get() == 1 });
        if first { set_ready(true); }
    });
    dom::on_forever(&dom::document(), "dragleave", |e| {
        if !has_files(e) { return; }
        let left = DEPTH.with(|d| { d.set((d.get() - 1).max(0)); d.get() == 0 });
        if left { reset(); }
    });
    dom::on_forever(&dom::document(), "dragover", |e| {
        if !has_files(e) { return; }
        // a page with its own drop handling (the file manager) already answered
        if e.default_prevented() { set_over(None); return; }
        dom::prevent(e);
        match zone_at(e).filter(|(_, i)| usable(i)) {
            Some((zone, _)) => { set_drop_effect(e, "copy"); set_over(Some(&zone)); }
            None => { set_drop_effect(e, "none"); set_over(None); }
        }
    });
    dom::on_forever(&dom::document(), "drop", |e| {
        if !has_files(e) { return; }
        let handled = e.default_prevented();
        // never let the browser open a file dropped beside a zone (it would leave Encore)
        dom::prevent(e);
        let target = zone_at(e);
        reset();
        if handled { return; }
        let Some((_, input)) = target else { return };
        if !usable(&input) { toast("Uploading isn't possible right now", "err"); return; }
        drop_on(&input, dropped_files(e));
    });
    dom::on_forever(&dom::document(), "dragend", |_| reset());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_accept_takes_anything() {
        assert!(accepts("", "song.flac", "audio/flac"));
        assert!(accepts(" , ", "x", ""));
    }

    #[test]
    fn extensions_match_the_end_of_the_name_in_any_case() {
        assert!(accepts(".zip,application/zip", "Plugin.ZIP", ""));
        assert!(accepts(".db,.sqlite,.sqlite3", "encore-2026.sqlite3", "application/octet-stream"));
        assert!(!accepts(".db", "encore.db.bak", ""));
        assert!(accepts(".apk,.apkm", "AppleMusic.apkm", ""));
    }

    #[test]
    fn wildcard_and_exact_types() {
        assert!(accepts("image/*", "me.webp", "image/webp"));
        assert!(!accepts("image/*", "me.txt", "text/plain"));
        assert!(accepts("image/jpeg,image/png", "c.png", "image/png"));
        assert!(!accepts("image/jpeg,image/png", "c.gif", "image/gif"));
        // a file the browser has no type for only passes on its name
        assert!(!accepts("image/*", "c.heic", ""));
        assert!(accepts("image/*,.txt,.log", "server.log", ""));
    }

    #[test]
    fn single_inputs_keep_only_the_first_good_file() {
        let files = vec![("a.txt", "text/plain"), ("b.zip", "application/zip"), ("c.zip", "application/zip")];
        let (keep, refused) = choose(files, ".zip", false, |f| f.0.to_string(), |f| f.1.to_string());
        assert_eq!(keep, vec![("b.zip", "application/zip")]);
        assert_eq!(refused, vec!["a.txt", "c.zip"]);
    }

    #[test]
    fn multiple_inputs_keep_every_good_file() {
        let files = vec!["a.png", "b.exe", "c.log"];
        let (keep, refused) = choose(files, "image/*,.txt,.log", true, |f| f.to_string(), |f| if f.ends_with("png") { "image/png".into() } else { String::new() });
        assert_eq!(keep, vec!["a.png", "c.log"]);
        assert_eq!(refused, vec!["b.exe"]);
    }

    #[test]
    fn messages_about_refused_files() {
        assert_eq!(refused_message(&[], ".zip", false, 1), "");
        assert_eq!(refused_message(&["a.txt".into()], ".zip,application/zip", false, 0), "a.txt can't be uploaded here (it takes .zip)");
        assert_eq!(refused_message(&["a".into(), "b".into()], "image/*", true, 0), "2 files can't be uploaded here");
        assert_eq!(refused_message(&["b.zip".into()], ".zip", false, 1), "Only one file can be uploaded here: the first was used");
    }
}
