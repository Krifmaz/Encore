//! `.modal`: a dialog with a title, a body and a footer over a dimmed backdrop. Returned from
//! a page's `overlay()`; the app lays it over the shell.

use super::{btn, icon_btn, t, El, Icon};
use crate::theme::{self, Btn};
use iced::widget::{column, container, mouse_area, opaque, row, scrollable, space, stack};
use iced::{Alignment, Length};

/// `on_close` is sent by the X, the backdrop and Escape. `footer` holds the buttons.
pub fn modal<'a, M: Clone + 'a>(
    title: impl Into<String>,
    body: impl Into<El<'a, M>>,
    footer: Option<El<'a, M>>,
    wide: bool,
    on_close: M,
) -> El<'a, M> {
    let p = theme::look();
    let head = container(
        row![
            t(title.into()).size(19).font(theme::bold()).width(Length::Fill),
            icon_btn(Icon::X, "Close", Some(on_close.clone()))
        ]
        .align_y(Alignment::Center)
        .spacing(10),
    )
    .padding([18, 22])
    .style(move |_| container::Style {
        border: iced::Border {
            color: p.border,
            width: 0.0,
            radius: 0.0.into(),
        },
        ..container::Style::default()
    });
    let mut card = column![head, super::divider(), scrollable(container(body).padding([20, 22])).style(theme::scroll)];
    if let Some(f) = footer {
        card = card.push(super::divider());
        card = card.push(
            container(row![space().width(Length::Fill), f].spacing(10).align_y(Alignment::Center))
                .padding([14, 22]),
        );
    }
    let card = container(card)
        .max_width(if wide { 980 } else { 560 })
        .max_height(760)
        .width(Length::Fill)
        .style(theme::modal);
    stack![
        mouse_area(
            container(space())
                .width(Length::Fill)
                .height(Length::Fill)
                .style(theme::modal_bg)
        )
        .on_press(on_close),
        container(opaque(card))
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(20)
            .center(Length::Fill),
    ]
    .into()
}

/// `confirmDialog(title, text, okLabel, danger)`: Cancel + the OK button.
pub fn confirm<'a, M: Clone + 'a>(
    title: &'a str,
    text: &'a str,
    ok_label: &'a str,
    danger: bool,
    on_ok: M,
    on_cancel: M,
) -> El<'a, M> {
    let footer = row![
        btn("Cancel", Btn::Default, Some(on_cancel.clone())),
        btn(ok_label, if danger { Btn::Danger } else { Btn::Primary }, Some(on_ok)),
    ]
    .spacing(10);
    modal(title, super::muted(text).size(14), Some(footer.into()), false, on_cancel)
}
