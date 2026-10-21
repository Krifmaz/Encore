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
    /// A muted line of text, not an action (0.36: the song ⋯ menu's "Requested by alex on 13 Oct 2026").
    pub info: bool,
}

impl<M> MenuItem<M> {
    pub fn new(label: impl Into<String>, on: M) -> Self {
        Self {
            label: label.into(),
            icon: None,
            danger: false,
            on: Some(on),
            info: false,
        }
    }

    /// A line of text at the top of a menu: muted, not hoverable, no message (0.36).
    pub fn info(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            icon: None,
            danger: false,
            on: None,
            info: true,
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
        if it.info {
            // text, not a button: nothing to hover or press
            col = col.push(
                container(t(it.label).size(12.5).color(p.muted))
                    .width(Length::Fill)
                    .padding([8, 12]),
            );
            continue;
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A menu's info line (0.36: who added a song) is text, not an action: no message, never pressed.
    #[test]
    fn an_info_line_is_not_clickable() {
        let it: MenuItem<u8> = MenuItem::info("Requested by alex on 13 Oct 2026");
        assert!(it.info && it.on.is_none() && !it.danger && it.icon.is_none());
        let action = MenuItem::new("Play next", 1u8);
        assert!(!action.info && action.on == Some(1));
        // the menu draws with it (no message to send)
        let _ = pop_menu(Point::ORIGIN, vec![it, action], 0u8, Size::new(800.0, 600.0));
    }
}
