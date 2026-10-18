//! Tag: the top artists of one genre, 40 a page, with "Load more". Behaviour reference:
//! web-rs/src/pages/foryou.rs `render_tag`.

use crate::api::ApiError;
use crate::fmt;
use crate::pages::{urlencode, Ctx, El, Page};
use crate::theme::Btn;
use crate::widgets::catalog;
use crate::widgets::{self, btn, empty, error_box, grid, h1, muted, skeleton_tiles, small_muted, Icon};
use iced::widget::{column, container, row};
use iced::{Alignment, Length, Subscription, Task};
use serde_json::Value;

/// A page of artists; fewer means there is no next page.
const PAGE: usize = 40;

#[derive(Debug, Default)]
pub struct State {
    pub name: String,
    page: i64,
    /// None while the first page loads; Err(message) when it failed.
    artists: Option<Result<Vec<Value>, String>>,
    summary: String,
    source: String,
    /// The last page was full, so there may be another.
    more: bool,
    loading_more: bool,
}

#[derive(Clone, Debug)]
pub enum Message {
    Loaded(i64, Result<Value, ApiError>),
    LoadMore,
    Open(Page),
}

fn load(state: &State, ctx: &Ctx) -> Task<Message> {
    let api = ctx.api.clone();
    let url = format!("/discover/tag/{}?page={}", urlencode(&state.name), state.page);
    let page = state.page;
    Task::perform(async move { api.get(&url).await }, move |r| Message::Loaded(page, r))
}

/// Called when the app navigates here (with the route's parameters).
pub fn open(state: &mut State, name: String, ctx: &mut Ctx) -> Task<Message> {
    state.name = name;
    state.page = 1;
    state.artists = None;
    state.summary.clear();
    state.source.clear();
    state.more = false;
    state.loading_more = false;
    load(state, ctx)
}

pub fn update(state: &mut State, msg: Message, ctx: &mut Ctx) -> Task<Message> {
    match msg {
        Message::Loaded(page, r) => {
            if page != state.page {
                return Task::none();
            }
            state.loading_more = false;
            match r {
                Ok(d) => {
                    if page == 1 {
                        state.summary = fmt::s(&d, "summary");
                        state.source = fmt::s(&d, "source");
                    }
                    let got = fmt::items(&d["artists"]);
                    state.more = got.len() >= PAGE;
                    match state.artists.as_mut() {
                        Some(Ok(list)) if page > 1 => list.extend(got),
                        _ => state.artists = Some(Ok(got)),
                    }
                }
                Err(e) => {
                    if page == 1 {
                        state.artists = Some(Err(e.message));
                    } else {
                        log::error!("tag page {page}: {}", e.message);
                    }
                }
            }
        }
        Message::LoadMore => {
            if state.loading_more || !state.more {
                return Task::none();
            }
            state.loading_more = true;
            state.page += 1;
            return load(state, ctx);
        }
        Message::Open(p) => ctx.navigate(p),
    }
    Task::none()
}

fn artist_open(a: &Value) -> Message {
    if fmt::truthy(a, "mbid") {
        Message::Open(Page::Artist(fmt::s(a, "mbid")))
    } else {
        Message::Open(Page::ArtistName(fmt::s(a, "name")))
    }
}

/// The genre with a capital first letter (CSS `text-transform: capitalize` on the web).
pub fn capitalize(s: &str) -> String {
    s.split(' ')
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn view<'a>(state: &'a State, ctx: &Ctx<'a>) -> El<'a, Message> {
    let summary: El<'a, Message> = if !state.summary.is_empty() {
        row![
            muted(state.summary.as_str()).size(14),
            small_muted(format!("· {}", state.source)).color(ctx.look.faint)
        ]
        .spacing(6)
        .align_y(Alignment::Center)
        .into()
    } else if !state.source.is_empty() {
        small_muted(format!("Top artists from {}", state.source)).color(ctx.look.faint).into()
    } else {
        column![].into()
    };
    let body: El<'a, Message> = match &state.artists {
        None => skeleton_tiles(8, widgets::tile::WIDTH),
        Some(Err(e)) => error_box(e),
        Some(Ok(list)) if list.is_empty() => empty(Icon::Users, "No artists for this genre", ""),
        Some(Ok(list)) => {
            let tiles = list.iter().map(|a| catalog::artist_tile(ctx, a, String::new(), artist_open(a))).collect();
            let mut col = column![grid(tiles)].spacing(18);
            if state.more {
                let label = if state.loading_more { "Loading…" } else { "Load more" };
                col = col.push(
                    container(btn(label, Btn::Default, (!state.loading_more).then_some(Message::LoadMore)))
                        .width(Length::Fill)
                        .center_x(Length::Fill),
                );
            }
            col.into()
        }
    };
    column![
        column![catalog::kicker("Genre".into()), h1(capitalize(&state.name)), summary].spacing(4),
        body
    ]
    .spacing(20)
    .width(Length::Fill)
    .into()
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
    fn titles_and_links() {
        assert_eq!(capitalize("hip hop"), "Hip Hop");
        assert_eq!(capitalize("r&b"), "R&b");
        assert!(matches!(artist_open(&serde_json::json!({ "mbid": "m", "name": "n" })), Message::Open(Page::Artist(m)) if m == "m"));
        assert!(matches!(artist_open(&serde_json::json!({ "name": "n" })), Message::Open(Page::ArtistName(n)) if n == "n"));
    }
}
