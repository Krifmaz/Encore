//! Requests: Your requests and, for admins, everyone's. Behaviour reference: web-rs/src/pages/requests.rs.
//! The tabs filter by status, admins see everyone's requests with an "Only mine" switch, Retry
//! failed and Clear finished; the list reloads every 4 s. The rows, their buttons and menus
//! live in `widgets::request_list` (the Downloader shares them).

use crate::api::ApiError;
use crate::fmt;
use crate::pages::{Ctx, El, Page};
use crate::theme::{self, Btn};
use crate::widgets::request_list::{self, List};
use crate::widgets::{self, card_flat, h1, muted, small_muted, tabs, Icon};
use iced::widget::{button, column, row, space, toggler};
use iced::{Alignment, Length, Subscription, Task};
use serde_json::Value;
use std::time::Duration;

/// The status tabs: `(key, label)`.
const TABS: [(&str, &str); 5] = [
    ("all", "All"),
    ("active", "Active"),
    ("pending", "Awaiting approval"),
    ("completed", "Available"),
    ("failed", "Failed"),
];

#[derive(Debug, Default)]
pub struct State {
    /// The active tab's key.
    filter: String,
    admin: bool,
    /// "{n} of {m} requests used this week" for members with a limit.
    quota: String,
    list: List,
}

#[derive(Clone, Debug)]
pub enum Message {
    List(request_list::Msg),
    Filter(String),
    Mine(bool),
    /// `/api/auth/me` for the quota line.
    Me(Result<Value, ApiError>),
    Tick,
    GoDiscover,
}

/// The query of the current tab and switch, as the web builds it.
pub fn query(filter: &str, mine: bool) -> String {
    let mut q = String::from("limit=300");
    if filter != "all" {
        q.push_str(&format!("&status={}", crate::pages::urlencode(filter)));
    }
    if mine {
        q.push_str("&mine=true");
    }
    q
}

fn reload(state: &mut State, ctx: &Ctx) -> Task<Message> {
    state.list.query = query(&state.filter, state.list.mine);
    state.list.load(ctx).map(Message::List)
}

/// Called when the app navigates here.
pub fn open(state: &mut State, ctx: &mut Ctx) -> Task<Message> {
    state.admin = ctx.is_admin();
    if state.filter.is_empty() {
        state.filter = "all".into();
    }
    state.list.reset();
    state.list.admin = state.admin;
    state.list.mine = !state.admin;
    state.list.keep = None;
    state.list.take = 0;
    state.quota.clear();
    let api = ctx.api.clone();
    Task::batch([
        reload(state, ctx),
        Task::perform(async move { api.me().await }, Message::Me),
    ])
}

pub fn update(state: &mut State, msg: Message, ctx: &mut Ctx) -> Task<Message> {
    match msg {
        Message::List(m) => request_list::update(&mut state.list, m, ctx).map(Message::List),
        Message::Filter(f) => {
            state.filter = f;
            reload(state, ctx)
        }
        Message::Mine(on) => {
            state.list.mine = on;
            reload(state, ctx)
        }
        Message::Me(Ok(me)) => {
            let admin = matches!(me["role"].as_str(), Some("admin" | "owner"));
            if fmt::truthy(&me, "request_limit") && !admin {
                state.quota = format!(
                    "{} of {} requests used this week",
                    fmt::s(&me, "requests_this_week"),
                    fmt::s(&me, "request_limit")
                );
            }
            Task::none()
        }
        Message::Me(Err(_)) => Task::none(),
        Message::Tick => reload(state, ctx),
        Message::GoDiscover => {
            ctx.navigate(Page::Search { q: String::new() });
            Task::none()
        }
    }
}

pub fn view<'a>(state: &'a State, ctx: &Ctx<'a>) -> El<'a, Message> {
    let p = ctx.look;
    let mut title = column![h1("Requests")].spacing(4);
    if !state.quota.is_empty() {
        title = title.push(small_muted(state.quota.as_str()));
    }
    let mut controls = row![].spacing(8).align_y(Alignment::Center);
    if state.admin {
        controls = controls.push(
            toggler(state.list.mine)
                .label("Only mine")
                .text_size(13)
                .spacing(8)
                .on_toggle(Message::Mine)
                .style(theme::switch),
        );
    }
    controls = controls.push(
        request_list::retry_failed_btn(&state.list, "Queue every failed request again").map(Message::List),
    );
    if state.admin {
        controls = controls.push(request_list::clear_btn("Clear finished").map(Message::List));
    }
    let head = row![title, space().width(Length::Fill), controls]
        .align_y(Alignment::Center)
        .spacing(12);
    // "Head to Discover to request music."
    let note = row![
        muted("Head to").size(13),
        button(widgets::t("Discover").size(13).color(p.accent))
            .padding(0)
            .style(theme::btn(Btn::Row))
            .on_press(Message::GoDiscover),
        muted("to request music.").size(13),
    ]
    .spacing(4)
    .align_y(Alignment::Center);
    let list = match request_list::view(ctx, &state.list) {
        Some(rows) => rows.map(Message::List),
        None => widgets::empty::empty_with(Icon::Inbox, "Nothing here", "", Some(note.into())),
    };
    column![
        head,
        tabs(&TABS, &state.filter, Message::Filter),
        card_flat(list).width(Length::Fill),
    ]
    .spacing(16)
    .width(Length::Fill)
    .into()
}

/// A modal or ⋯ menu to lay over the whole window, when one is open.
pub fn overlay<'a>(state: &'a State, ctx: &Ctx<'a>) -> Option<El<'a, Message>> {
    request_list::overlay(&state.list, ctx).map(|e| e.map(Message::List))
}

pub fn subscription(_state: &State) -> Subscription<Message> {
    iced::time::every(Duration::from_secs(4)).map(|_| Message::Tick)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_query_matches_the_web() {
        assert_eq!(query("all", false), "limit=300");
        assert_eq!(query("pending", true), "limit=300&status=pending&mine=true");
        assert_eq!(TABS.iter().map(|(k, _)| *k).collect::<Vec<_>>(), ["all", "active", "pending", "completed", "failed"]);
    }
}
