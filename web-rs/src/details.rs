//! The whole text of an error or a status, readable and copyable (R1, the owner on Oct 19: "can we also make the
//! errors readable fully they cut off or at least an option to copy the error").
//!
//! One small piece every page uses the same way:
//! - [`attrs`] on any element that shows a cut-off line: a click opens [`show`], a dialog with the whole text,
//!   wrapped and selectable, and a **Copy** button;
//! - [`copy_button`] for a text that is shown in full but should be easy to paste into a bug report;
//! - [`init`] installs the one click listener both need (from `lib.rs`, once).
//!
//! All of it is the kit's (`kit_leptos::details`); copying goes through `ui::copy_text`, with its words: "Copy", "Copied to clipboard", "Copy failed – select and
//! copy manually".

/// The kit's details: the `data-details` attributes, the dialog's body and buttons, the dialog itself and
/// whether text is selected.
pub use kit_leptos::details::{attrs, body_html, footer_html, selecting};

/// A small Copy button (an icon, "Copy" as its title) that puts `text` on the clipboard.
pub fn copy_button(text: &str) -> String { kit_leptos::details::copy_button_html(text) }

/// The dialog with the whole text: wrapped, selectable, with Copy and Close ("Details" for a blank title).
pub fn show(title: &str, text: &str) { kit_leptos::details::show(title, text); }

/// The click listener for every page (the kit's): `[data-copy-text]` copies, `[data-details]` opens the whole
/// text, an error toast's `[data-toast-close]` closes it.
pub fn init() { kit_leptos::details::init(); }

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
