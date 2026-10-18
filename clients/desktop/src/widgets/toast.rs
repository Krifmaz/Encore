//! `.toast`: short messages bottom right, gone after four seconds.

use super::{t, El};
use crate::theme;
use iced::widget::{button, column, container, row, space};
use iced::{Alignment, Background, Border, Color, Length};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Ok,
    /// `toast(msg, "warn")`.
    Warn,
    Err,
}

#[derive(Clone, Debug)]
pub struct Toast {
    pub id: u64,
    pub text: String,
    pub kind: ToastKind,
    pub at: Instant,
}

impl Toast {
    /// How long a toast stays (`toast()` in web-rs/src/ui.rs: 3.8 s then fades).
    pub const LIFE: Duration = Duration::from_millis(4200);

    pub fn expired(&self) -> bool {
        self.at.elapsed() > Self::LIFE
    }
}

/// The toast column, to be laid over the shell bottom right. `on_dismiss(id)` closes one early.
pub fn view<'a, M: Clone + 'a>(toasts: &'a [Toast], on_dismiss: impl Fn(u64) -> M) -> El<'a, M> {
    let p = theme::look();
    let mut col = column![].spacing(8).align_x(Alignment::End);
    for toast in toasts {
        let edge = match toast.kind {
            ToastKind::Info => p.accent,
            ToastKind::Ok => p.ok,
            ToastKind::Warn => p.warn,
            ToastKind::Err => p.err,
        };
        let card = container(t(toast.text.clone()).size(13.5))
            .padding([12, 16])
            .max_width(380)
            .style(theme::toast);
        let bar = container(space())
            .width(4)
            .height(Length::Fill)
            .style(move |_| container::Style {
                background: Some(Background::Color(edge)),
                border: Border {
                    color: Color::TRANSPARENT,
                    width: 0.0,
                    radius: iced::border::Radius::new(0.0).top_left(12.0).bottom_left(12.0),
                },
                ..container::Style::default()
            });
        col = col.push(
            button(row![bar, card].height(Length::Shrink).align_y(Alignment::Center))
                .padding(0)
                .style(theme::btn(theme::Btn::Row))
                .on_press(on_dismiss(toast.id)),
        );
    }
    container(col)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Alignment::End)
        .align_y(Alignment::End)
        .padding([theme::PLAYER_H + 18.0, 18.0])
        .into()
}
