//! The whole text of an error or a status, readable and copyable (R1, the owner on Oct 19: "can we also make the
//! errors readable fully they cut off or at least an option to copy the error").
//!
//! One small piece every page uses the same way:
//! - [`attrs`] on any element that shows a cut-off line: a click opens [`show`], a dialog with the whole text,
//!   wrapped and selectable, and a **Copy** button;
//! - [`copy_button`] for a text that is shown in full but should be easy to paste into a bug report;
//! - [`init`] installs the one click listener both need (from `lib.rs`, once).
//!
//! Copying goes through `ui::copy_text`, with its words: "Copy", "Copied to clipboard", "Copy failed – select and
//! copy manually".

use crate::dom;
use crate::ui::{esc, icon};

/// `data-details="…" data-details-title="…"`: a click on the element opens the whole text in [`show`]. The text is
/// kept in the attribute (escaped), so it is the text the element stood for when the page drew it.
pub fn attrs(title: &str, text: &str) -> String {
    format!(r#" data-details="{}" data-details-title="{}""#, esc(text), esc(title))
}

/// A small Copy button (an icon, "Copy" as its title) that puts `text` on the clipboard.
pub fn copy_button(text: &str) -> String {
    format!(r#"<button type="button" class="btn sm ghost icon copy-btn" data-copy-text="{}" title="Copy" aria-label="Copy">{}</button>"#, esc(text), icon("copy"))
}

/// The dialog's body: the whole text in a wrapped, selectable block.
pub fn body_html(text: &str) -> String {
    format!(r#"<pre class="details-text" tabindex="0">{}</pre>"#, esc(text))
}

/// The dialog's buttons: Close and Copy (which copies exactly the text shown).
pub fn footer_html(text: &str) -> String {
    format!(r#"<button class="btn" data-close>Close</button><button class="btn primary" data-copy-text="{}">{}Copy</button>"#, esc(text), icon("copy"))
}

/// The dialog with the whole text: wrapped, selectable, with Copy and Close ("Details" when the title is
/// blank). Drawn by the kit (`kit_leptos::details`) with [`body_html`] and [`footer_html`]'s markup.
pub fn show(title: &str, text: &str) {
    kit_leptos::details::show(title, text);
}

/// Whether the user has text selected on the page: a click that ends a selection (dragging over a status line to
/// copy part of it) must not open the dialog.
pub fn selecting() -> bool {
    let w = dom::window();
    let Ok(f) = js_sys::Reflect::get(&w, &"getSelection".into()) else { return false };
    let Ok(f) = wasm_bindgen::JsCast::dyn_into::<js_sys::Function>(f) else { return false };
    f.call0(&w).ok()
        .and_then(|s| js_sys::Reflect::get(&s, &"toString".into()).ok().and_then(|t| wasm_bindgen::JsCast::dyn_into::<js_sys::Function>(t).ok()).and_then(|t| t.call0(&s).ok()))
        .and_then(|t| t.as_string())
        .is_some_and(|t| !t.trim().is_empty())
}

/// The click listener for every page: `[data-copy-text]` copies, `[data-details]` opens the whole text, an error
/// toast's `[data-toast-close]` closes it. Links and buttons inside a `[data-details]` element keep their own action.
/// The kit's listener (`kit_leptos::details::init`), the same one this module installed.
pub fn init() {
    kit_leptos::details::init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_text_is_escaped_once_and_copied_as_shown() {
        let t = "import failed: mkdir /music/<Band> & \"Co\": Input/output error (os error 5)\nline 2";
        let a = attrs("Request #12", t);
        assert!(a.contains(r#"data-details="import failed: mkdir /music/&lt;Band&gt; &amp; &quot;Co&quot;: Input/output error (os error 5)"#), "{a}");
        assert!(a.contains(r#"data-details-title="Request #12""#), "{a}");
        // the dialog shows and copies the same text
        assert!(body_html(t).starts_with(r#"<pre class="details-text" tabindex="0">import failed: mkdir /music/&lt;Band&gt;"#));
        let f = footer_html(t);
        assert!(f.contains(">Close</button>") && f.ends_with("Copy</button>"), "{f}");
        assert!(f.contains(r#"data-copy-text="import failed: mkdir /music/&lt;Band&gt; &amp; &quot;Co&quot;"#), "{f}");
        let b = copy_button("x");
        assert!(b.contains(r#"data-copy-text="x""#) && b.contains(r#"title="Copy""#), "{b}");
    }
}
