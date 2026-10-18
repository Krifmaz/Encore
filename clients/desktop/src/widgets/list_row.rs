//! `.list-item`: a row with something on the left, a title and a line under it, and content on
//! the right; the whole row presses.

use super::{small_muted, t, El};
use crate::theme::{self, Btn};
use iced::widget::{button, column, container, row};
use iced::{Alignment, Length};

/// `left` is a thumb or icon, `right` badges or buttons.
pub fn list_row<'a, M: Clone + 'a>(
    left: Option<El<'a, M>>,
    title: impl Into<String>,
    sub: impl Into<String>,
    right: Vec<El<'a, M>>,
    on_press: Option<M>,
) -> El<'a, M> {
    let sub = sub.into();
    let mut lines = column![t(title.into()).size(14).font(theme::semibold())].spacing(2);
    if !sub.is_empty() {
        lines = lines.push(small_muted(sub).size(12.5));
    }
    let mut r = row![].spacing(14).align_y(Alignment::Center);
    if let Some(l) = left {
        r = r.push(l);
    }
    r = r.push(lines.width(Length::Fill));
    for x in right {
        r = r.push(x);
    }
    let b = button(container(r).padding([11, 16]))
        .padding(0)
        .width(Length::Fill)
        .style(theme::btn(Btn::Row))
        .on_press_maybe(on_press);
    column![b, super::divider()].into()
}
