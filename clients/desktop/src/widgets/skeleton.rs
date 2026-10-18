//! `.skeleton`: grey blocks where content is about to appear.

use super::El;
use crate::theme;
use iced::widget::{column, container, row, space};
use iced::Length;

/// One grey block.
pub fn block<'a, M: 'a>(width: impl Into<Length>, height: f32) -> El<'a, M> {
    container(space())
        .width(width)
        .height(height)
        .style(theme::skeleton)
        .into()
}

/// `n` list rows (a thumb and two lines each).
pub fn skeleton_rows<'a, M: 'a>(n: usize) -> El<'a, M> {
    let mut c = column![].spacing(14);
    for _ in 0..n {
        c = c.push(
            row![
                block(44, 44.0),
                column![block(Length::FillPortion(2), 13.0), block(Length::FillPortion(1), 11.0)].spacing(8)
            ]
            .spacing(14)
            .align_y(iced::Alignment::Center),
        );
    }
    c.width(Length::Fill).into()
}

/// `n` album tiles of `width`.
pub fn skeleton_tiles<'a, M: 'a>(n: usize, width: f32) -> El<'a, M> {
    let mut r = row![].spacing(16);
    for _ in 0..n {
        r = r.push(
            column![block(width, width), block(width * 0.8, 13.0), block(width * 0.5, 11.0)]
                .spacing(8)
                .width(width),
        );
    }
    r.wrap().into()
}
