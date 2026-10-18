//! Storage: What the library takes on disk. Behaviour reference: web-rs/src/pages/storage.rs.
//!
//! `/api/library/storage`: the disk (music, other files, free), the library's totals, the
//! download folders, the songs by format, the biggest artists and the biggest albums.

use crate::api::ApiError;
use crate::fmt;
use crate::pages::{Ctx, El, Page};
use crate::theme;
use crate::widgets::catalog;
use crate::widgets::stat::{bar, between, fraction, num, stat, stats};
use crate::widgets::{card, card_flat, error_box, h1, h2, list_row, section_head, skeleton_rows, small, small_muted, t};
use iced::widget::{column, container, row, space};
use iced::{Alignment, Background, Border, Color, Length, Subscription, Task};
use serde_json::Value;

#[derive(Debug, Default)]
pub struct State {
    /// None while loading; Err(message) when the server said no.
    data: Option<Result<Box<Value>, String>>,
}

#[derive(Clone, Debug)]
pub enum Message {
    Loaded(Result<Value, ApiError>),
    Open(Page),
}

/// Called when the app navigates here (with the route's parameters).
pub fn open(state: &mut State, ctx: &mut Ctx) -> Task<Message> {
    state.data = None;
    let api = ctx.api.clone();
    Task::perform(async move { api.get("/library/storage").await }, Message::Loaded)
}

pub fn update(state: &mut State, msg: Message, ctx: &mut Ctx) -> Task<Message> {
    match msg {
        Message::Loaded(Ok(v)) => state.data = Some(Ok(Box::new(v))),
        Message::Loaded(Err(e)) => state.data = Some(Err(e.message)),
        Message::Open(p) => ctx.navigate(p),
    }
    Task::none()
}

/// The words of a download folder row.
pub fn download_label(name: &str) -> &'static str {
    if name == "soulseek" {
        "Soulseek downloads folder"
    } else {
        "Staging (in progress)"
    }
}

/// The parts of the disk bar in thousandths: (music, other, free).
pub fn disk_parts(lib_size: f64, used: f64, total: f64) -> (u16, u16, u16) {
    let music = (fraction(lib_size, total) * 1000.0).round() as u16;
    let other = (fraction((used - lib_size).max(0.0), total) * 1000.0).round() as u16;
    let other = other.min(1000u16.saturating_sub(music));
    (music, other, 1000u16.saturating_sub(music + other))
}

fn disk_card<'a>(s: &Value, lib_size: f64, ctx: &Ctx<'a>) -> Option<El<'a, Message>> {
    if !fmt::truthy(s, "disk") {
        return None;
    }
    let look = ctx.look;
    let disk = &s["disk"];
    let (used, free, total) = (fmt::f(disk, "used"), fmt::f(disk, "free"), fmt::f(disk, "total"));
    let other = (used - lib_size).max(0.0);
    let (music_p, other_p, free_p) = disk_parts(lib_size, used, total);
    let piece = |color: Color| {
        move |_: &iced::Theme| container::Style {
            background: Some(Background::Color(color)),
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: 99.0.into(),
            },
            ..container::Style::default()
        }
    };
    let mut meter = row![].height(14).spacing(2);
    if music_p > 0 {
        meter = meter.push(
            container(space())
                .width(Length::FillPortion(music_p))
                .height(14)
                .style(piece(look.accent)),
        );
    }
    if other_p > 0 {
        meter = meter.push(
            container(space())
                .width(Length::FillPortion(other_p))
                .height(14)
                .style(piece(look.muted.scale_alpha(0.5))),
        );
    }
    if free_p > 0 {
        meter = meter.push(space().width(Length::FillPortion(free_p)));
    }
    let meter = container(meter).width(Length::Fill).height(14).style(piece(look.border));
    let legend = row![
        row![
            t("■").size(12).color(look.accent),
            t("Music").size(12).font(theme::bold()).color(look.accent),
            small_muted(fmt::size(lib_size))
        ]
        .spacing(5),
        iced::widget::tooltip(row![t("■").size(12).color(look.muted), small_muted(format!("Other {}", fmt::size(other)))].spacing(5), crate::widgets::tip_box("Other files"), iced::widget::tooltip::Position::Top).gap(4),
        row![t("□").size(12).color(look.muted), small_muted(format!("Free {}", fmt::size(free)))].spacing(5),
    ]
    .spacing(16);
    Some(
        card(
            column![
                between(
                    t("Disk").size(14).font(theme::bold()).into(),
                    small_muted(format!(
                        "{} used · {} free of {}",
                        fmt::size(used),
                        fmt::size(free),
                        fmt::size(total)
                    ))
                    .into(),
                ),
                meter,
                legend,
            ]
            .spacing(12),
        )
        .width(Length::Fill)
        .into(),
    )
}

/// "name  N songs · size" over a meter.
fn meter_row<'a>(name: El<'a, Message>, words: String, part: f64, whole: f64, color: Color) -> El<'a, Message> {
    column![between(name, small_muted(words).into()), bar(part, whole, color)]
        .spacing(6)
        .into()
}

pub fn view<'a>(state: &'a State, ctx: &Ctx<'a>) -> El<'a, Message> {
    let look = ctx.look;
    let body: El<'a, Message> = match &state.data {
        None => skeleton_rows(6),
        Some(Err(e)) => error_box(e),
        Some(Ok(s)) => {
            let library = &s["library"];
            let lib_size = fmt::f(library, "size");
            let artists = fmt::items(&s["artists"]);
            let formats = fmt::items(&s["formats"]);
            let max_a = artists.iter().map(|a| fmt::f(a, "size")).fold(1.0_f64, f64::max);
            let max_f = formats.iter().map(|f| fmt::f(f, "size")).fold(1.0_f64, f64::max);
            let mut col = column![].spacing(20);
            if let Some(d) = disk_card(s, lib_size, ctx) {
                col = col.push(d);
            }
            let mut tiles: Vec<El<'a, Message>> = vec![
                stat(fmt::size(lib_size), "Library"),
                stat(num(fmt::f(library, "albums")), "Albums"),
                stat(num(fmt::f(library, "tracks")), "Songs"),
            ];
            for d in fmt::items(&s["downloads"]) {
                tiles.push(stat(fmt::size(fmt::f(&d, "size")), download_label(&fmt::s(&d, "name"))));
            }
            col = col.push(stats(tiles));

            let mut formats_col = column![h2("By format")].spacing(12);
            for f in &formats {
                let name = if fmt::truthy(f, "format") { fmt::s(f, "format") } else { "?".into() };
                formats_col = formats_col.push(meter_row(
                    small(name).font(theme::bold()).into(),
                    format!("{} songs · {}", num(fmt::f(f, "tracks")), fmt::size(fmt::f(f, "size"))),
                    fmt::f(f, "size"),
                    max_f,
                    look.accent,
                ));
            }
            let mut artists_col = column![h2("Biggest artists")].spacing(12);
            for a in &artists {
                let name = fmt::s(a, "name");
                artists_col = artists_col.push(meter_row(
                    catalog::link(name.clone(), 12.0, Message::Open(Page::ArtistName(name))),
                    fmt::size(fmt::f(a, "size")),
                    fmt::f(a, "size"),
                    max_a,
                    look.accent2,
                ));
            }
            col = col.push(
                row![
                    container(card(formats_col).width(Length::Fill)).width(Length::FillPortion(1)),
                    container(card(artists_col).width(Length::Fill)).width(Length::FillPortion(1)),
                ]
                .spacing(16)
                .align_y(Alignment::Start),
            );

            col = col.push(section_head("Biggest albums", None));
            let albums: Vec<El<'a, Message>> = fmt::items(&s["albums"])
                .iter()
                .map(|a| {
                    let album = if fmt::truthy(a, "album") { fmt::s(a, "album") } else { "Unknown album".into() };
                    list_row(
                        None,
                        album,
                        format!("{} · {} songs", fmt::s(a, "albumartist"), fmt::s(a, "tracks")),
                        vec![t(fmt::size(fmt::f(a, "size"))).size(14).font(theme::bold()).into()],
                        Some(Message::Open(Page::LibraryAlbum(fmt::s(a, "id")))),
                    )
                })
                .collect();
            col = col.push(card_flat(column(albums)).width(Length::Fill));
            col.into()
        }
    };
    column![h1("Storage"), body].spacing(20).width(Length::Fill).into()
}

/// A modal or ⋯ menu to lay over the whole window, when one is open.
pub fn overlay<'a>(_state: &'a State, _ctx: &Ctx<'a>) -> Option<El<'a, Message>> {
    None
}

pub fn subscription(_state: &State) -> Subscription<Message> {
    Subscription::none()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_disk_bar_adds_up() {
        assert_eq!(disk_parts(250.0, 500.0, 1000.0), (250, 250, 500));
        assert_eq!(disk_parts(0.0, 0.0, 0.0), (0, 0, 1000));
        assert_eq!(disk_parts(900.0, 800.0, 1000.0), (900, 0, 100), "the library larger than used: no other");
        assert_eq!(download_label("soulseek"), "Soulseek downloads folder");
        assert_eq!(download_label("staging"), "Staging (in progress)");
    }
}
