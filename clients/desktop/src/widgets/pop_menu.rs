//! `.menu-pop`: the ⋯ menu, opened at a point (the button or the pointer on a right-click).

use super::{icon, t, El, Icon};
use crate::theme::{self, Btn};
use iced::widget::{button, column, container, mouse_area, opaque, row, space, stack};
use iced::{Alignment, Length, Padding, Point, Size};

/// One line of the menu; `on` None shows it greyed out.
#[derive(Clone, Debug)]
pub struct MenuItem<M> {
    pub label: String,
    pub icon: Option<Icon>,
    pub danger: bool,
    pub on: Option<M>,
}

impl<M> MenuItem<M> {
    pub fn new(label: impl Into<String>, on: M) -> Self {
        Self {
            label: label.into(),
            icon: None,
            danger: false,
            on: Some(on),
        }
    }

    pub fn icon(mut self, i: Icon) -> Self {
        self.icon = Some(i);
        self
    }

    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }
}

/// The menu at `at` (window coordinates), kept inside `window`. Clicking outside sends `on_close`.
pub fn pop_menu<'a, M: Clone + 'a>(at: Point, items: Vec<MenuItem<M>>, on_close: M, window: Size) -> El<'a, M> {
    let p = theme::look();
    let width = 230.0;
    let height = items.len() as f32 * 36.0 + 12.0;
    let x = at.x.min(window.width - width - 8.0).max(8.0);
    let y = at.y.min(window.height - height - 8.0).max(8.0);
    let mut col = column![].spacing(0);
    for it in items {
        let color = if it.on.is_none() {
            p.faint
        } else if it.danger {
            p.err
        } else {
            p.text
        };
        let mut r = row![].spacing(10).align_y(Alignment::Center);
        if let Some(i) = it.icon {
            r = r.push(icon(i, 16.0, color));
        }
        r = r.push(t(it.label).size(13.5).color(color));
        col = col.push(
            button(r)
                .width(Length::Fill)
                .padding([8, 12])
                .style(theme::btn(Btn::Row))
                .on_press_maybe(it.on),
        );
    }
    let card = container(col).padding(6).width(width).style(theme::menu_pop);
    stack![
        mouse_area(container(space()).width(Length::Fill).height(Length::Fill))
            .on_press(on_close.clone())
            .on_right_press(on_close),
        container(opaque(card)).padding(Padding::ZERO.top(y).left(x)),
    ]
    .into()
}
