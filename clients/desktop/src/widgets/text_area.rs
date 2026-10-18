//! A multi-line text box styled like `.input` (`textarea.input` on the web): the description of
//! a bug report, a reply, a pasted list of songs. The page holds a `text_editor::Content` in
//! its state and forwards the editor's `Action`s to it.

use super::El;
use crate::theme;
use iced::widget::text_editor::{self, Action, Content};
use iced::{Background, Border};

/// `rows` is the height in text lines, like `<textarea rows>`.
pub fn text_area<'a, M: 'a>(
    content: &'a Content,
    placeholder: &'a str,
    rows: u16,
    on_action: impl Fn(Action) -> M + 'a,
) -> El<'a, M> {
    text_editor::TextEditor::new(content)
        .placeholder(placeholder)
        .on_action(on_action)
        .padding(10)
        .size(14)
        .height(f32::from(rows) * 21.0 + 20.0)
        .style(style)
        .into()
}

/// The `.input` look for a text editor: panel background, border, accent ring when focused.
pub fn style(_: &iced::Theme, status: text_editor::Status) -> text_editor::Style {
    let p = theme::look();
    let focused = matches!(status, text_editor::Status::Focused { .. });
    text_editor::Style {
        background: Background::Color(p.panel2),
        border: Border {
            color: if focused { p.accent } else { p.border },
            width: 1.0,
            radius: 10.0.into(),
        },
        placeholder: p.faint,
        value: p.text,
        selection: p.accent.scale_alpha(0.35),
    }
}
