//! Readable errors: the details dialog that shows a text in full – wrapped, selectable, with a
//! Copy button – and the small Copy button the error lines carry (the web's details modal with
//! its `<pre>` and `ui::copy_text`). The app holds the one open dialog (`Ctx::details` opens it);
//! a page only builds the text – and, for a request the audio check turned away, the one thing
//! that fixes it ([`Act`]: Keep anyway or Download anyway), which the app sends itself.
//!
//! The dialog is the kit's (`kit_iced::details`); its links open this app's pages.

use crate::pages::Page;

pub use kit_iced::details::{copied, copy_btn, Act, Lead, COPIED, COPY_FAILED};

/// What a request's details link to and play (the owner on Oct 23: "from the download i should
/// be able to click on the artist song or album or play it if its downloaded"): the title and the
/// artist as links to their pages, and Play with the library path of what it brought in
/// (`/library/tracks/<id>` or `/library/albums/<id>`).
pub type Links = kit_iced::details::Links<Page>;

/// The text a dialog shows, in an editor that selects but never edits.
pub type Details = kit_iced::details::Details<Page>;

/// The dialog's messages.
pub type Msg = kit_iced::details::Msg<Page>;

/// What the app does after a message.
pub type Out = kit_iced::details::Out<Page>;

/// The dialog: the lead when there is one, the text in full (monospace, wrapped, selectable),
/// Close and Copy (and the action, when the text came with one).
pub fn view(d: &Details) -> super::El<'_, Msg> {
    kit_iced::details::view(d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::widget::text_editor::{self, Action};

    #[test]
    fn a_copy_is_checked_against_what_the_clipboard_holds() {
        assert!(copied("a\nb", Some("a\r\nb\n")));
        assert!(!copied("a", None));
        assert!(!copied("a", Some("")));
        assert!(!copied("a", Some("something older")));
        assert_eq!(COPY_FAILED, "Copy failed – select and copy manually");
    }

    #[test]
    fn the_text_selects_but_never_changes() {
        let mut d = Details::new("Request details".into(), "line one\nline two".into(), None, None);
        assert_eq!(d.update(Msg::Text(Action::Edit(text_editor::Edit::Insert('x')))), None);
        assert_eq!(d.content().text().trim_end(), "line one\nline two");
        assert_eq!(d.update(Msg::Text(Action::SelectAll)), None);
        assert_eq!(d.content().selection().as_deref(), Some("line one\nline two"));
        assert_eq!(d.update(Msg::Copy), Some(Out::Copy("line one\nline two".into())));
        // no action: its message does nothing
        assert_eq!(d.update(Msg::Act), None);
    }

    #[test]
    fn the_action_goes_to_the_app() {
        let act = Act {
            label: "Keep anyway".into(),
            path: "/requests/12/keep-anyway".into(),
            body: serde_json::json!({ "rejected": 1 }),
            done: "It will be imported without the audio check".into(),
        };
        let mut d = Details::new("Moonlight Shadow – Machine Time".into(), "Encore request #12".into(), None, Some(act.clone()));
        assert_eq!(d.update(Msg::Act), Some(Out::Act(act)));
    }
}
