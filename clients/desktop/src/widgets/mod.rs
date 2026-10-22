//! The building blocks every page uses, styled like the web app's components. Page agents add
//! `pub mod <theirs>;` lines here and a `widgets/<theirs>.rs` file; everything else in this
//! file belongs to the architect and the integrator.

pub mod badge;
pub mod catalog;
pub mod choice;
pub mod cover;
pub mod details;
pub mod empty;
pub mod eq_graph;
pub mod icons;
pub mod list_row;
pub mod marquee;
pub mod modal;
pub mod more_like;
pub mod picker;
pub mod pop_menu;
pub mod request_list;
pub mod search_box;
pub mod sidebar;
pub mod skeleton;
pub mod stat;
pub mod tabs;
pub mod text_area;
pub mod tile;
pub mod toast;
pub mod track_row;

// The re-exports are the pages' toolbox.
pub use badge::badge;
pub use cover::Covers;
pub use empty::empty;
pub use icons::{icon, logo, Icon};
pub use list_row::list_row;
pub use modal::{confirm, modal};
pub use pop_menu::{pop_menu, MenuItem};
pub use search_box::search_box;
pub use skeleton::{skeleton_rows, skeleton_tiles};
pub use tabs::{seg, tabs, tabs_maybe};
pub use tile::{grid, rail, Tile, TileKind};
pub use toast::{Toast, ToastKind};
pub use track_row::TrackRow;

use crate::theme::{self, Btn};
use iced::widget::text::{IntoFragment, Shaping};
use iced::widget::{button, column, container, row, text, tooltip, Text};
use iced::{Alignment, Length};

/// An element of this app: `iced::Element` with the default theme and renderer.
pub type El<'a, M> = iced::Element<'a, M>;

/// Text with advanced shaping, so album and artist names in any script render right.
pub fn t<'a>(s: impl IntoFragment<'a>) -> Text<'a> {
    text(s).shaping(Shaping::Advanced)
}

/// `h1`: 28px, weight 750.
pub fn h1<'a>(s: impl IntoFragment<'a>) -> Text<'a> {
    t(s).size(28).font(theme::bold())
}

/// `h2`: 19px, weight 700.
pub fn h2<'a>(s: impl IntoFragment<'a>) -> Text<'a> {
    t(s).size(19).font(theme::bold())
}

/// `h3`: 15px, weight 650.
pub fn h3<'a>(s: impl IntoFragment<'a>) -> Text<'a> {
    t(s).size(15).font(theme::semibold())
}

/// `.muted`.
pub fn muted<'a>(s: impl IntoFragment<'a>) -> Text<'a> {
    t(s).color(theme::look().muted)
}

/// `.faint`.
pub fn faint<'a>(s: impl IntoFragment<'a>) -> Text<'a> {
    t(s).color(theme::look().faint)
}

/// `.small`: 12px.
pub fn small<'a>(s: impl IntoFragment<'a>) -> Text<'a> {
    t(s).size(12)
}

/// `.small.muted`.
pub fn small_muted<'a>(s: impl IntoFragment<'a>) -> Text<'a> {
    t(s).size(12).color(theme::look().muted)
}

/// `.btn <kind>` with a text label. `on` None = disabled.
pub fn btn<'a, M: Clone + 'a>(label: impl IntoFragment<'a>, kind: Btn, on: Option<M>) -> button::Button<'a, M> {
    let size = if matches!(kind, Btn::Default | Btn::Primary | Btn::Danger) { 14 } else { 13 };
    button(t(label).size(size).font(theme::semibold()))
        .padding([9, 16])
        .style(theme::btn(kind))
        .on_press_maybe(on)
}

/// `.btn.sm`: a smaller button.
pub fn btn_sm<'a, M: Clone + 'a>(label: impl IntoFragment<'a>, kind: Btn, on: Option<M>) -> button::Button<'a, M> {
    button(t(label).size(13).font(theme::semibold()))
        .padding([6, 11])
        .style(theme::btn(kind))
        .on_press_maybe(on)
}

/// A button with an icon before its label (`.btn` with an svg).
pub fn btn_icon<'a, M: Clone + 'a>(
    i: Icon,
    label: impl IntoFragment<'a>,
    kind: Btn,
    on: Option<M>,
) -> button::Button<'a, M> {
    let p = theme::look();
    let color = match kind {
        Btn::Primary => p.on_accent,
        Btn::Danger => p.err,
        _ => p.text,
    };
    button(
        row![icon(i, 16.0, color), t(label).size(13).font(theme::semibold())]
            .spacing(8)
            .align_y(Alignment::Center),
    )
    .padding([7, 12])
    .style(theme::btn(kind))
    .on_press_maybe(on)
}

/// `.btn.ghost.icon`: a square icon button with a tooltip. `on` None = disabled.
pub fn icon_btn<'a, M: Clone + 'a>(i: Icon, tip: &'a str, on: Option<M>) -> El<'a, M> {
    let p = theme::look();
    let color = if on.is_some() { p.text } else { p.text.scale_alpha(0.4) };
    let b = button(icon(i, 16.0, color))
        .padding(8)
        .style(theme::btn(Btn::Icon))
        .on_press_maybe(on);
    if tip.is_empty() {
        return b.into();
    }
    tooltip(b, tip_box(tip), tooltip::Position::Bottom)
        .gap(4)
        .into()
}

/// The small dark box a tooltip shows.
pub fn tip_box<'a, M: 'a>(tip: impl IntoFragment<'a>) -> El<'a, M> {
    container(t(tip).size(12))
        .padding([5, 9])
        .style(theme::menu_pop)
        .into()
}

/// `.error-box`: a red box with a message, hidden when the message is empty.
pub fn error_box<'a, M: 'a>(msg: &'a str) -> El<'a, M> {
    if msg.is_empty() {
        return column![].into();
    }
    container(t(msg).size(13))
        .padding([10, 14])
        .width(Length::Fill)
        .style(theme::error_box)
        .into()
}

/// `.card`: a bordered panel with 18px padding.
pub fn card<'a, M: 'a>(content: impl Into<El<'a, M>>) -> container::Container<'a, M> {
    container(content).padding(18).style(theme::card)
}

/// `.card.flat`: a bordered panel without padding (lists).
pub fn card_flat<'a, M: 'a>(content: impl Into<El<'a, M>>) -> container::Container<'a, M> {
    container(content).padding(0).style(theme::card)
}

/// `.section-head`: a heading with an action on the right.
pub fn section_head<'a, M: 'a>(title: impl IntoFragment<'a>, action: Option<El<'a, M>>) -> El<'a, M> {
    let mut r = row![h2(title)].align_y(Alignment::Center).spacing(12);
    r = r.push(iced::widget::space().width(Length::Fill));
    if let Some(a) = action {
        r = r.push(a);
    }
    container(r).padding([0, 0]).width(Length::Fill).into()
}

/// A page's header: the title, an optional line under it and actions on the right.
pub fn page_head<'a, M: 'a>(
    title: impl IntoFragment<'a>,
    sub: Option<El<'a, M>>,
    actions: Vec<El<'a, M>>,
) -> El<'a, M> {
    let mut left = column![h1(title)].spacing(4);
    if let Some(s) = sub {
        left = left.push(s);
    }
    let mut r = row![left].align_y(Alignment::Center).spacing(12);
    r = r.push(iced::widget::space().width(Length::Fill));
    for a in actions {
        r = r.push(a);
    }
    r.width(Length::Fill).into()
}

/// `.field`: a label over an input.
pub fn field<'a, M: 'a>(label: impl IntoFragment<'a>, input: impl Into<El<'a, M>>) -> El<'a, M> {
    column![small_muted(label).font(theme::semibold()), input.into()]
        .spacing(6)
        .width(Length::Fill)
        .into()
}

/// `Padding` with four sides (iced's arrays take two).
pub fn pad(top: f32, right: f32, bottom: f32, left: f32) -> iced::Padding {
    iced::Padding {
        top,
        right,
        bottom,
        left,
    }
}

/// A horizontal `--border` line.
pub fn divider<'a, M: 'a>() -> El<'a, M> {
    iced::widget::rule::horizontal(1).style(theme::rule_style).into()
}

/// A round avatar: the account's first letter in the accent gradient (web `avatar_html`).
pub fn avatar<'a, M: 'a>(username: &str, size: f32) -> El<'a, M> {
    let first = username
        .chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "?".into());
    container(t(first).size(size * 0.42).font(theme::bold()))
        .width(size)
        .height(size)
        .center(size)
        .style(theme::avatar)
        .into()
}
