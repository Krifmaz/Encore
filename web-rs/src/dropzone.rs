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

use js_sys::{Array, Reflect};
use wasm_bindgen::JsValue;
use web_sys::Event;

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

/// Starts drag and drop for the whole app (called once at start-up): the kit's listeners
/// (`kit_leptos::dropzone::init`), the ones this module installed. They sit on the document for the whole
/// session, so zones drawn later – pages, dialogs, cards – work without any setup of their own: every usable
/// zone gets [`READY_CLASS`] while files are dragged over the window, the one under the pointer also
/// [`OVER_CLASS`], and a drop is checked like the input's chooser ([`choose`], [`refused_message`]) and handed
/// to the input as if the files had been picked.
pub fn init() {
    kit_leptos::dropzone::init();
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
