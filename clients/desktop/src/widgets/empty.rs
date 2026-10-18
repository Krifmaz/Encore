//! `.empty`: what a list shows when there is nothing in it – an icon, a line and, when it
//! helps, an action.

use super::{icon, muted, t, El, Icon};
use crate::theme;
use iced::widget::{column, container};
use iced::{Alignment, Length};

pub fn empty<'a, M: 'a>(i: Icon, title: &'a str, note: &'a str) -> El<'a, M> {
    empty_with(i, title, note, None)
}

pub fn empty_with<'a, M: 'a>(i: Icon, title: &'a str, note: &'a str, action: Option<El<'a, M>>) -> El<'a, M> {
    let p = theme::look();
    let mut c = column![icon(i, 34.0, p.faint), t(title).size(15).font(theme::semibold())]
        .spacing(10)
        .align_x(Alignment::Center);
    if !note.is_empty() {
        c = c.push(muted(note).size(13).center());
    }
    if let Some(a) = action {
        c = c.push(container(a).padding([6, 0]));
    }
    container(c)
        .width(Length::Fill)
        .padding([44, 20])
        .center_x(Length::Fill)
        .into()
}
