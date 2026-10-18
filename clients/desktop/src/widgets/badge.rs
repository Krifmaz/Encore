//! `.badge`: a small rounded label (`ok`, `warn`, `err`, `info`, `accent` or plain).

use super::{t, El};
use crate::theme::{self, Badge};
use iced::widget::container;

pub fn badge<'a, M: 'a>(label: impl Into<String>, kind: Badge) -> El<'a, M> {
    container(t(label.into()).size(11.5).font(theme::semibold()))
        .padding([2, 9])
        .style(theme::badge(kind))
        .into()
}
