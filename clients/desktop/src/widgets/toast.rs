//! `.toast`: short messages bottom right, gone after four seconds. An error stays until it is
//! dismissed and carries a Copy button, so it can be read in full and pasted into a report. A toast
//! can carry one action ("Up next cleared · Undo", the web's `toast_action`) and then stays as long
//! as the action can be taken.

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
    /// Its button ("Undo") and what it does, and how long the toast stays then.
    pub action: Option<(String, crate::player::Message, Duration)>,
}

impl Toast {
    /// How long a toast stays (`toast()` in web-rs/src/ui.rs: 3.8 s then fades).
    pub const LIFE: Duration = Duration::from_millis(4200);

    /// At most this many errors stay on screen; a new one pushes out the oldest (the web's).
    pub const STAYING: usize = 3;

    /// An error stays until it is closed and has Copy – except the words that say a copy failed,
    /// which tell what to do instead.
    pub fn stays(&self) -> bool {
        self.kind == ToastKind::Err && self.text != super::details::COPY_FAILED
    }

    /// Errors stay until they are dismissed (a click on the toast); the rest go after [`Self::LIFE`]
    /// (a toast with an action: as long as the action can be taken).
    pub fn expired(&self) -> bool {
        let life = self.action.as_ref().map_or(Self::LIFE, |a| a.2);
        !self.stays() && self.at.elapsed() > life
    }
}

/// The toast column, to be laid over the shell bottom right. `on_dismiss(id)` closes one early;
/// `on_copy(id)` copies an error's text; `on_action(id)` takes a toast's action.
pub fn view<'a, M: Clone + 'a>(
    toasts: &'a [Toast],
    on_dismiss: impl Fn(u64) -> M,
    on_copy: impl Fn(u64) -> M,
    on_action: impl Fn(u64) -> M,
) -> El<'a, M> {
    let p = theme::look();
    let mut col = column![].spacing(8).align_x(Alignment::End);
    for toast in toasts {
        let edge = match toast.kind {
            ToastKind::Info => p.accent,
            ToastKind::Ok => p.ok,
            ToastKind::Warn => p.warn,
            ToastKind::Err => p.err,
        };
        let words = t(toast.text.clone()).size(13.5);
        let inner: El<'a, M> = if toast.stays() {
            column![words, row![space().width(Length::Fill), super::details::copy_btn(on_copy(toast.id))]]
                .spacing(4)
                .into()
        } else if let Some((label, _, _)) = toast.action.as_ref() {
            row![
                words,
                space().width(Length::Fixed(12.0)),
                super::btn_sm(label.as_str(), theme::Btn::Default, Some(on_action(toast.id)))
            ]
            .align_y(Alignment::Center)
            .into()
        } else {
            words.into()
        };
        let card = container(inner)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_stay_until_closed_and_the_rest_go() {
        let at = Instant::now() - Duration::from_secs(60);
        let toast = |text: &str, kind| Toast { id: 1, text: text.into(), kind, at, action: None };
        assert!(toast("x", ToastKind::Err).stays() && !toast("x", ToastKind::Err).expired());
        assert!(toast("Copied to clipboard", ToastKind::Ok).expired());
        // the failed copy says what to do instead: it goes like any other toast
        let failed = toast(super::super::details::COPY_FAILED, ToastKind::Err);
        assert!(!failed.stays() && failed.expired());
        // a toast with Undo stays as long as Undo can be pressed
        let recent = Instant::now() - Duration::from_secs(5);
        let undo = Toast {
            at: recent,
            action: Some(("Undo".into(), crate::player::Message::UndoClear, Duration::from_millis(6000))),
            ..toast("Up next cleared", ToastKind::Ok)
        };
        assert!(!undo.expired());
        assert!(Toast { action: None, ..undo.clone() }.expired());
        assert!(Toast { at, ..undo }.expired());
    }
}
