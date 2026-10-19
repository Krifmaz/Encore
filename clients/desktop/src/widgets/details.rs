//! Readable errors: the details dialog that shows a text in full – wrapped, selectable, with a
//! Copy button – and the small Copy button the error lines carry (the web's details modal with
//! its `<pre>` and `ui::copy_text`). The app holds the one open dialog (`Ctx::details` opens it);
//! a page only builds the text.

use super::{btn, btn_icon, El, Icon};
use crate::theme::{self, Btn};
use iced::widget::text::Wrapping;
use iced::widget::text_editor::{self, Action, Content};
use iced::widget::{button, column, row};
use iced::{Alignment, Length};

/// The dialog's words.
pub const COPY: &str = "Copy";
pub const COPIED: &str = "Copied to clipboard";
pub const COPY_FAILED: &str = "Copy failed – select and copy manually";

/// The text a dialog shows, in an editor that selects but never edits.
#[derive(Debug)]
pub struct Details {
    pub title: String,
    pub text: String,
    content: Content,
}

#[derive(Clone, Debug)]
pub enum Msg {
    /// Selection, cursor moves and scrolling in the text (edits are dropped).
    Text(Action),
    Copy,
    Close,
}

impl Details {
    pub fn new(title: String, text: String) -> Details {
        let content = Content::with_text(&text);
        Details { title, text, content }
    }

    /// Apply a message; `Some(text)` asks the app to copy it.
    pub fn update(&mut self, msg: Msg) -> Option<String> {
        match msg {
            Msg::Text(a) if !a.is_edit() => self.content.perform(a),
            Msg::Text(_) | Msg::Close => {}
            Msg::Copy => return Some(self.text.clone()),
        }
        None
    }
}

/// Whether what the clipboard holds after a copy is what was copied (line ends may change).
pub fn copied(sent: &str, got: Option<&str>) -> bool {
    let norm = |s: &str| s.replace("\r\n", "\n").trim().to_string();
    got.is_some_and(|g| !g.is_empty() && norm(g) == norm(sent))
}

/// The dialog: the text in full (monospace, wrapped, selectable), Close and Copy.
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
    let body = column![editor].width(Length::Fill);
    let footer = row![
        btn("Close", Btn::Default, Some(Msg::Close)),
        btn_icon(Icon::Copy, COPY, Btn::Primary, Some(Msg::Copy)),
    ]
    .spacing(10)
    .align_y(Alignment::Center);
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
        let mut d = Details::new("Request details".into(), "line one\nline two".into());
        assert_eq!(d.update(Msg::Text(Action::Edit(text_editor::Edit::Insert('x')))), None);
        assert_eq!(d.content.text().trim_end(), "line one\nline two");
        assert_eq!(d.update(Msg::Text(Action::SelectAll)), None);
        assert_eq!(d.content.selection().as_deref(), Some("line one\nline two"));
        assert_eq!(d.update(Msg::Copy).as_deref(), Some("line one\nline two"));
    }
}
