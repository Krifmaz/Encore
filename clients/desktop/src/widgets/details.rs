//! Readable errors: the details dialog that shows a text in full – wrapped, selectable, with a
//! Copy button – and the small Copy button the error lines carry (the web's details modal with
//! its `<pre>` and `ui::copy_text`). The app holds the one open dialog (`Ctx::details` opens it);
//! a page only builds the text – and, for a request the audio check turned away, the one thing
//! that fixes it ([`Act`]: Keep anyway or Download anyway), which the app sends itself.

use super::{btn, btn_icon, El, Icon};
use crate::theme::{self, Btn};
use iced::widget::text::Wrapping;
use iced::widget::text_editor::{self, Action, Content};
use iced::widget::{button, column, row};
use iced::{Alignment, Length};
use serde_json::Value;

/// The dialog's words.
pub const COPY: &str = "Copy";
pub const COPIED: &str = "Copied to clipboard";
pub const COPY_FAILED: &str = "Copy failed – select and copy manually";

/// A button beside Copy that acts on what the dialog shows: `POST path` with `body`, then the
/// `done` toast (the server's own words when it says no). The page's list shows the change on
/// its next reload.
#[derive(Clone, Debug, PartialEq)]
pub struct Act {
    pub label: String,
    pub path: String,
    pub body: Value,
    pub done: String,
}

/// A few words above the text, in a warning box: why (in full) and where it came from – the
/// reason the audio check turned a download away (the web's `anyway_box_html`).
#[derive(Clone, Debug, PartialEq)]
pub struct Lead {
    pub text: String,
    pub from: String,
}

/// The text a dialog shows, in an editor that selects but never edits.
#[derive(Debug)]
pub struct Details {
    pub title: String,
    pub text: String,
    pub lead: Option<Lead>,
    pub act: Option<Act>,
    content: Content,
}

#[derive(Clone, Debug)]
pub enum Msg {
    /// Selection, cursor moves and scrolling in the text (edits are dropped).
    Text(Action),
    Copy,
    /// The dialog's [`Act`] button.
    Act,
    Close,
}

/// What the app does after a message.
#[derive(Debug, PartialEq)]
pub enum Out {
    /// Copy this text.
    Copy(String),
    /// Close the dialog and send this.
    Act(Act),
}

impl Details {
    pub fn new(title: String, text: String, lead: Option<Lead>, act: Option<Act>) -> Details {
        let content = Content::with_text(&text);
        Details { title, text, lead, act, content }
    }

    /// Apply a message; `Some(out)` asks the app to copy the text or to send the action.
    pub fn update(&mut self, msg: Msg) -> Option<Out> {
        match msg {
            Msg::Text(a) if !a.is_edit() => self.content.perform(a),
            Msg::Text(_) | Msg::Close => {}
            Msg::Copy => return Some(Out::Copy(self.text.clone())),
            Msg::Act => return self.act.clone().map(Out::Act),
        }
        None
    }
}

/// Whether what the clipboard holds after a copy is what was copied (line ends may change).
pub fn copied(sent: &str, got: Option<&str>) -> bool {
    let norm = |s: &str| s.replace("\r\n", "\n").trim().to_string();
    got.is_some_and(|g| !g.is_empty() && norm(g) == norm(sent))
}

/// The dialog: the lead when there is one, the text in full (monospace, wrapped, selectable),
/// Close and Copy (and the action, when the text came with one).
pub fn view(d: &Details) -> El<'_, Msg> {
    let lines = d.text.lines().count().max(1) as f32;
    let editor = text_editor::TextEditor::new(&d.content)
        .on_action(Msg::Text)
        .font(theme::mono())
        .size(12.5)
        .padding(12)
        .wrapping(Wrapping::WordOrGlyph)
        .height(Length::Fixed((lines * 19.0 + 24.0).clamp(60.0, 480.0)))
        .style(super::text_area::style);
    let mut body = column![].spacing(12).width(Length::Fill);
    if let Some(l) = &d.lead {
        let mut words = column![super::t(l.text.clone())
            .size(14)
            .font(theme::semibold())
            .wrapping(Wrapping::WordOrGlyph)]
        .spacing(3);
        if !l.from.is_empty() {
            words = words.push(super::small_muted(l.from.clone()).wrapping(Wrapping::WordOrGlyph));
        }
        body = body.push(
            iced::widget::container(words)
                .padding([10, 14])
                .width(Length::Fill)
                .style(theme::warn_box),
        );
    }
    body = body.push(editor);
    // the action (Keep anyway) is the dialog's main button; Copy is then a plain one
    let mut footer = row![
        btn("Close", Btn::Default, Some(Msg::Close)),
        btn_icon(Icon::Copy, COPY, if d.act.is_some() { Btn::Default } else { Btn::Primary }, Some(Msg::Copy)),
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    if let Some(a) = &d.act {
        footer = footer.push(btn(a.label.clone(), Btn::Primary, Some(Msg::Act)));
    }
    super::modal(d.title.clone(), body, Some(footer.into()), true, Msg::Close)
}

/// The small Copy button beside an error line.
pub fn copy_btn<'a, M: Clone + 'a>(on: M) -> El<'a, M> {
    let p = theme::look();
    button(
        row![super::icon(Icon::Copy, 13.0, p.muted), super::t(COPY).size(12).color(p.muted)]
            .spacing(5)
            .align_y(Alignment::Center),
    )
    .padding([3, 7])
    .style(theme::btn(Btn::Ghost))
    .on_press(on)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(d.content.text().trim_end(), "line one\nline two");
        assert_eq!(d.update(Msg::Text(Action::SelectAll)), None);
        assert_eq!(d.content.selection().as_deref(), Some("line one\nline two"));
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
