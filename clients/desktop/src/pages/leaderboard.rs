//! Leaderboard: Who listens most. Behaviour reference: web-rs/src/pages/leaderboard.rs.
//!
//! The ranking boards from `/api/stats/leaderboard` (requests, plays, likes, time listened),
//! this week's top listener, the most played songs and artists, the biggest artists in the
//! library and the fun facts.

use crate::api::ApiError;
use crate::fmt;
use crate::pages::{Ctx, El, Page};
use crate::theme;
use crate::widgets::catalog;
use crate::widgets::stat::{between, long_dur, num, stat, stats};
use crate::widgets::{self, card, empty, error_box, h1, h2, muted, section_head, skeleton_rows, small_muted, t, Icon};
use iced::widget::{column, container, image, row, space};
use iced::{Alignment, ContentFit, Length, Subscription, Task};
use serde_json::Value;

const MEDAL: [&str; 3] = ["🥇", "🥈", "🥉"];

/// `MEDAL[i] || (i + 1) + '.'`.
pub fn medal(i: usize) -> String {
    MEDAL.get(i).map(|m| m.to_string()).unwrap_or_else(|| format!("{}.", i + 1))
}

/// `fmtV(k, v)`: time listened as a duration, everything else as a count.
pub fn fmt_v(k: &str, v: f64) -> String {
    if k == "listened" {
        long_dur(v)
    } else {
        num(v)
    }
}

/// "1 listener" / "3 listeners".
pub fn listeners(n: f64) -> String {
    format!("{} listener{}", fmt::s(&Value::from(n), ""), if n == 1.0 { "" } else { "s" })
        .replace("  ", " ")
        .replacen(" listener", &format!("{} listener", num(n)), 1)
        .trim_start()
        .to_string()
}

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
    Task::perform(async move { api.get("/stats/leaderboard").await }, Message::Loaded)
}

pub fn update(state: &mut State, msg: Message, ctx: &mut Ctx) -> Task<Message> {
    match msg {
        Message::Loaded(Ok(v)) => state.data = Some(Ok(Box::new(v))),
        Message::Loaded(Err(e)) => state.data = Some(Err(e.message)),
        Message::Open(p) => ctx.navigate(p),
    }
    Task::none()
}

/// One ranking card: medal, name ("(you)" in the accent colour), value.
fn board<'a>(key: &str, b: &Value, ctx: &Ctx<'a>) -> El<'a, Message> {
    let look = ctx.look;
    let rows = fmt::items(&b["rows"]);
    let mut col = column![h2(fmt::s(b, "label"))].spacing(10);
    if rows.is_empty() {
        col = col.push(small_muted("Nobody yet – be the first."));
    }
    for (i, r) in rows.iter().enumerate() {
        let me = fmt::truthy(r, "me");
        let color = if me { look.accent } else { look.text };
        let name = format!("{}{}", fmt::s(r, "username"), if me { " (you)" } else { "" });
        let left = row![
            t(medal(i)).size(14).width(26),
            t(name).size(14).color(color).font(if me { theme::bold() } else { iced::Font::DEFAULT }),
        ]
        .spacing(4)
        .align_y(Alignment::Center);
        col = col.push(between(
            left.into(),
            t(fmt_v(key, fmt::f(r, "value"))).size(14).font(theme::bold()).color(color).into(),
        ));
    }
    card(col).width(Length::Fill).into()
}

fn artist_page(name: &str) -> Message {
    Message::Open(Page::ArtistName(name.to_string()))
}

fn song_row<'a>(i: usize, s: &Value, ctx: &Ctx<'a>) -> El<'a, Message> {
    let look = ctx.look;
    let thumb: El<'a, Message> = match ctx.cover(&fmt::s(s, "album_id"), &s["cover_v"], 80) {
        Some(h) => image(h)
            .width(36)
            .height(36)
            .content_fit(ContentFit::Cover)
            .border_radius(6.0)
            .into(),
        None => container(widgets::icon(Icon::Music, 16.0, look.faint))
            .width(36)
            .height(36)
            .center(36)
            .style(theme::art)
            .into(),
    };
    row![
        t(format!("{}", i + 1)).size(13).color(look.faint).width(20),
        thumb,
        column![
            t(fmt::s(s, "title")).size(14).font(theme::semibold()),
            small_muted(fmt::s(s, "artist")).size(12.5),
        ]
        .spacing(2)
        .width(Length::Fill),
        t(num(fmt::f(s, "plays"))).size(14).font(theme::bold()),
    ]
    .spacing(10)
    .align_y(Alignment::Center)
    .into()
}

pub fn view<'a>(state: &'a State, ctx: &Ctx<'a>) -> El<'a, Message> {
    let look = ctx.look;
    let head = column![
        h1("Leaderboard"),
        muted("Plays count after 30 seconds in the web player. Bragging rights only.").size(14),
    ]
    .spacing(6);
    let body: El<'a, Message> = match &state.data {
        None => skeleton_rows(6),
        Some(Err(e)) => error_box(e),
        Some(Ok(d)) => {
            let mut boards = row![].spacing(16).align_y(Alignment::Start);
            if let Some(o) = d["boards"].as_object() {
                for (k, b) in o {
                    boards = boards.push(container(board(k, b, ctx)).width(Length::Fixed(300.0)));
                }
            }
            let mut col = column![boards.wrap()].spacing(20);
            if let Some(w) = fmt::items(&d["this_week"]).first() {
                col = col.push(section_head("This week's top listener", None));
                col = col.push(card(
                    row![
                        t(format!("👑 {}", fmt::s(w, "username"))).size(20).font(theme::bold()),
                        muted(format!("· {} songs played this week", num(fmt::f(w, "plays")))).size(14),
                    ]
                    .spacing(8)
                    .align_y(Alignment::Center),
                ));
            }
            // Most played songs
            let songs = fmt::items(&d["top_songs"]);
            let mut songs_col = column![h2("Most played songs")].spacing(10);
            if songs.is_empty() {
                songs_col = songs_col.push(empty(Icon::Play, "No plays yet", ""));
            }
            for (i, s) in songs.iter().enumerate() {
                songs_col = songs_col.push(song_row(i, s, ctx));
            }
            // Most played artists
            let artists = fmt::items(&d["top_artists"]);
            let mut artists_col = column![h2("Most played artists")].spacing(10);
            if artists.is_empty() {
                artists_col = artists_col.push(small_muted("No plays yet."));
            }
            for (i, a) in artists.iter().enumerate() {
                let name = fmt::s(a, "artist");
                let n = fmt::f(a, "listeners");
                let left = row![
                    t(medal(i)).size(14).width(26),
                    catalog::link(name.clone(), 14.0, artist_page(&name)),
                    small_muted(format!("· {}", listeners(n))).color(look.faint),
                ]
                .spacing(4)
                .align_y(Alignment::Center);
                artists_col = artists_col.push(between(
                    left.into(),
                    t(num(fmt::f(a, "plays"))).size(14).font(theme::bold()).into(),
                ));
            }
            // Biggest in the library
            let mut library_col = column![h2("Biggest in the library")].spacing(10);
            for (i, a) in fmt::items(&d["library_artists"]).iter().enumerate() {
                let name = fmt::s(a, "artist");
                let left = row![
                    t(medal(i)).size(14).width(26),
                    catalog::link(name.clone(), 14.0, artist_page(&name)),
                ]
                .spacing(4)
                .align_y(Alignment::Center);
                let right = row![
                    small_muted(format!("{} albums · ", fmt::s(a, "albums"))),
                    t(num(fmt::f(a, "songs"))).size(12).font(theme::bold()),
                    small_muted(" songs"),
                ]
                .align_y(Alignment::Center);
                library_col = library_col.push(between(left.into(), right.into()));
            }
            col = col.push(
                row![
                    container(card(songs_col).width(Length::Fill)).width(Length::FillPortion(1)),
                    container(card(artists_col).width(Length::Fill)).width(Length::FillPortion(1)),
                    container(card(library_col).width(Length::Fill)).width(Length::FillPortion(1)),
                ]
                .spacing(16)
                .align_y(Alignment::Start),
            );
            // Fun facts
            let facts: Vec<El<'a, Message>> = fmt::items(&d["facts"])
                .iter()
                .map(|f| stat(fmt::s(f, "value"), fmt::s(f, "label")))
                .collect();
            if !facts.is_empty() {
                col = col.push(section_head("Fun facts", None));
                col = col.push(stats(facts));
            }
            col = col.push(space().height(8));
            col.into()
        }
    };
    column![head, body].spacing(20).width(Length::Fill).into()
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
    fn medals_and_values() {
        assert_eq!(medal(0), "🥇");
        assert_eq!(medal(2), "🥉");
        assert_eq!(medal(3), "4.");
        assert_eq!(fmt_v("plays", 1234.0), "1,234");
        assert_eq!(fmt_v("listened", 7500.0), "2 hr 5 min");
        assert_eq!(listeners(1.0), "1 listener");
        assert_eq!(listeners(3.0), "3 listeners");
    }
}
