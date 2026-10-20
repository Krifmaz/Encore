//! `.tabs` (the framed tab row of Library, the full player) and `.seg` (the segmented control
//! of settings).

use super::{t, El};
use crate::theme::{self, Btn};
use iced::widget::{button, container, row};

/// `items` are `(id, label)`; `on(id)` is sent when a tab is pressed.
pub fn tabs<'a, M: Clone + 'a>(items: &[(&'a str, &'a str)], active: &str, on: impl Fn(String) -> M) -> El<'a, M> {
    tabs_maybe(items, active, Some(on))
}

/// [`tabs`] that can't be pressed when `on` is None (a value an environment variable sets).
pub fn tabs_maybe<'a, M: Clone + 'a, F: Fn(String) -> M>(items: &[(&'a str, &'a str)], active: &str, on: Option<F>) -> El<'a, M> {
    let mut r = row![].spacing(4);
    for (id, label) in items {
        let is = *id == active;
        r = r.push(
            button(t(*label).size(13.5).font(theme::semibold()))
                .padding([7, 14])
                .style(theme::btn(if is { Btn::TabOn } else { Btn::Tab }))
                .on_press_maybe(on.as_ref().map(|f| f(id.to_string()))),
        );
    }
    container(r).padding(4).style(theme::tabs_frame).into()
}

/// The segmented control: the same, with square inner buttons.
pub fn seg<'a, M: Clone + 'a>(items: &[(&'a str, &'a str)], active: &str, on: impl Fn(String) -> M) -> El<'a, M> {
    let mut r = row![].spacing(0);
    for (id, label) in items {
        let is = *id == active;
        r = r.push(
            button(t(*label).size(13.5).font(if is { theme::semibold() } else { iced::Font::DEFAULT }))
                .padding([8, 14])
                .style(theme::btn(if is { Btn::SegOn } else { Btn::Seg }))
                .on_press(on(id.to_string())),
        );
    }
    container(r).style(theme::tabs_frame).clip(true).into()
}
