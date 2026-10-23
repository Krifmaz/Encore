//! `.search`: a text box with the search icon in front.

use super::{icon, El, Icon};
use crate::theme;
use iced::widget::{container, row, text_input};
use iced::{Alignment, Background, Border, Color, Length};

/// `on_input(text)` on every keystroke, `on_submit` on Enter.
pub fn search_box<'a, M: Clone + 'a>(
    placeholder: &str,
    value: &str,
    on_input: impl Fn(String) -> M + 'a,
    on_submit: Option<M>,
) -> El<'a, M> {
    let p = theme::look();
    let input = text_input(placeholder, value)
        .on_input(on_input)
        .on_submit_maybe(on_submit)
        .padding([8, 6])
        .size(14)
        .style(move |_, _| text_input::Style {
            background: Background::Color(Color::TRANSPARENT),
            border: Border::default(),
            icon: p.faint,
            placeholder: p.faint,
            value: p.text,
            selection: p.accent.scale_alpha(0.35),
        });
    container(
        row![icon(Icon::Search, 17.0, p.faint), input]
            .spacing(6)
            .align_y(Alignment::Center),
    )
    .padding(super::pad(1.0, 6.0, 1.0, 13.0))
    .width(Length::Fill)
    .style(|th| {
        let mut s = theme::panel2(th);
        s.border.radius = 10.0.into();
        s
    })
    .into()
}

/// A [`search_box`] with an `id` (so a page can focus it with `operation::focus`) and a × that
/// sends `on_clear` while there is text (the playlist search and the Playlists page).
pub fn search_box_clearable<'a, M: Clone + 'a>(
    id: &'static str,
    placeholder: &str,
    value: &str,
    on_input: impl Fn(String) -> M + 'a,
    on_clear: M,
) -> El<'a, M> {
    let p = theme::look();
    let input = text_input(placeholder, value)
        .id(id)
        .on_input(on_input)
        .padding([8, 6])
        .size(14)
        .style(move |_, _| text_input::Style {
            background: Background::Color(Color::TRANSPARENT),
            border: Border::default(),
            icon: p.faint,
            placeholder: p.faint,
            value: p.text,
            selection: p.accent.scale_alpha(0.35),
        });
    let mut r = row![icon(Icon::Search, 17.0, p.faint), input].spacing(6).align_y(Alignment::Center);
    // the × only while there is something to clear
    if !value.is_empty() {
        r = r.push(super::icon_btn(Icon::X, "Clear search", Some(on_clear)));
    }
    container(r)
        .padding(super::pad(1.0, 4.0, 1.0, 13.0))
        .width(Length::Fill)
        .style(|th| {
            let mut s = theme::panel2(th);
            s.border.radius = 10.0.into();
            s
        })
        .into()
}
