//! Requests: Your requests and, for admins, everyone's. Behaviour reference: web-rs/src/pages/requests.rs.
//! The tabs filter by status, admins see everyone's requests or one person's with a User list
//! ("All users", then each person with their requests: "Caz (612)", `GET /api/requests/users`),
//! Retry failed and Clear finished; everyone has Clear waiting (admins: everyone's waiting
//! requests, or the person picked; members: their own), counted by the server before it asks
//! "Cancel N waiting requests?"; the list reloads every 4 s. The rows, their buttons and menus
//! live in `widgets::request_list` (the Downloader shares them), and so does the banner that says
//! imports are paused while the library can't be written. "Active" includes waiting requests
//! (the server's `status=active`). Admins also get Download all now beside Retry failed, on a
//! server with Download now: every waiting request is woken and requests move up in their lines
//! (with a person picked, only theirs) – never more at once than each source allows.
//! The tabs carry how many requests each holds ("Failed 3"), and badges under the title say how
//! many downloads are still to go, await approval or failed (`GET /api/requests/counts`, whose
//! requests the list shows), fetched again with the list.

use crate::api::ApiError;
use crate::fmt;
use crate::pages::{Ctx, El, Page};
use crate::theme::{self, Btn};
use crate::widgets::request_list::{self, List};
use crate::widgets::{self, card_flat, h1, muted, small_muted, tabs, Icon};
use iced::widget::{button, column, pick_list, row, space};
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
    /// The admin's User list: the people who have requests (`GET /api/requests/users`).
    users: Vec<Value>,
    /// The person picked in it (kept while they have no requests left).
    picked: Option<UserChoice>,
    /// `GET /api/requests/counts` for whose requests the list shows; None until it answers.
    counts: Option<Value>,
    /// The tab labels with their counts ("Failed 3"), one per [`TABS`] entry; the plain labels
    /// until the counts are in.
    tab_labels: Vec<String>,
}

/// An entry of the User list: everyone's requests (`id` None) or one person's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserChoice {
    pub id: Option<i64>,
    pub label: String,
}

impl std::fmt::Display for UserChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label)
    }
}

/// The User list's first entry.
pub const ALL_USERS: &str = "All users";

/// "Caz (612)": a person in the User list with their requests.
pub fn user_label(name: &str, count: i64) -> String {
    format!("{name} ({count})")
}

/// The User list (the web's `user_options`): "All users", then each person who has requests with
/// their count; the person picked stays in it with 0 once their requests are gone (the list
/// doesn't jump to everyone's).
pub fn user_options(users: &[Value], picked: Option<(i64, &str)>) -> Vec<UserChoice> {
    let mut out = vec![UserChoice { id: None, label: ALL_USERS.into() }];
    for u in users.iter().filter(|u| u["id"].is_i64()) {
        out.push(UserChoice { id: Some(fmt::i(u, "id")), label: user_label(&fmt::s(u, "username"), fmt::i(u, "count")) });
    }
    if let Some((id, name)) = picked {
        if !users.iter().any(|u| u["id"].as_i64() == Some(id)) {
            out.push(UserChoice { id: Some(id), label: user_label(name, 0) });
        }
    }
    out
}

/// The name in a User list entry ("Caz (612)" → "Caz").
fn name_of(label: &str) -> &str {
    match label.rfind(" (") {
        Some(i) if label.ends_with(')') => &label[..i],
        _ => label,
    }
}

#[derive(Clone, Debug)]
pub enum Message {
    List(request_list::Msg),
    Filter(String),
    /// A person picked in the admin's User list.
    User(UserChoice),
    /// `GET /api/requests/users`.
    Users(Result<Value, ApiError>),
    /// `GET /api/requests/counts`.
    Counts(Result<Value, ApiError>),
    /// `/api/auth/me` for the quota line.
    Me(Result<Value, ApiError>),
    Tick,
    GoDiscover,
}

impl State {
    /// The request whose details the screenshot tour opens: the first that did not complete,
    /// else the first.
    pub fn first_request(&self) -> Option<i64> {
        self.first_request_row().map(|r| fmt::i(&r, "id"))
    }

    /// The same request's row (the tour opens the picker for it).
    pub fn first_request_row(&self) -> Option<Value> {
        let rows = self.list.rows();
        rows.iter()
            .find(|r| r["status"].as_str() != Some("completed"))
            .or(rows.first())
            .cloned()
    }
}

/// A tab's label with its count beside it ("Failed 3"), as the web draws it; just the label
/// while there are no counts.
pub fn tab_label(label: &str, count: Option<i64>) -> String {
    match count {
        Some(n) => format!("{label} {n}"),
        None => label.to_string(),
    }
}

/// The badges under the title, as the web's `counts_badges`: the downloads still to go, the
/// requests awaiting approval and the failed ones, each with its kind; a group with none is left
/// out.
pub fn counts_badges(c: &Value) -> Vec<(String, theme::Badge)> {
    [("remaining", theme::Badge::Info, "remaining"), ("pending", theme::Badge::Warn, "awaiting approval"), ("failed", theme::Badge::Err, "failed")]
        .into_iter()
        .filter(|(k, _, _)| fmt::i(c, k) > 0)
        .map(|(k, kind, text)| (format!("{} {text}", fmt::i(c, k)), kind))
        .collect()
}

/// The counts' query: whose requests, the same as the list's ("mine=true", "user=7" or nothing).
pub fn counts_query(mine: bool, user: Option<i64>) -> String {
    if mine {
        "mine=true".into()
    } else {
        user.map(|u| format!("user={u}")).unwrap_or_default()
    }
}

/// The query of the current tab and whose requests, as the web builds it.
pub fn query(filter: &str, mine: bool, user: Option<i64>) -> String {
    let mut q = String::from("limit=300");
    if filter != "all" {
        q.push_str(&format!("&status={}", crate::pages::urlencode(filter)));
    }
    if mine {
        q.push_str("&mine=true");
    } else if let Some(u) = user {
        q.push_str(&format!("&user={u}"));
    }
    q
}

fn reload(state: &mut State, ctx: &Ctx) -> Task<Message> {
    state.list.query = query(&state.filter, state.list.mine, state.list.user);
    let list = state.list.load(ctx).map(Message::List);
    // the tabs' and badges' counts follow the list
    let (api, q) = (ctx.api.clone(), counts_query(state.list.mine, state.list.user));
    let counts = Task::perform(async move { api.get(&format!("/requests/counts?{q}")).await }, Message::Counts);
    if !state.admin {
        return Task::batch([list, counts]);
    }
    // the User list's counts follow the list
    let api = ctx.api.clone();
    Task::batch([list, counts, Task::perform(async move { api.get("/requests/users").await }, Message::Users)])
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
    state.list.user = state.picked.as_ref().and_then(|p| p.id);
    state.list.keep = None;
    state.list.take = 0;
    state.quota.clear();
    state.counts = None;
    state.tab_labels = TABS.iter().map(|(_, l)| tab_label(l, None)).collect();
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
        Message::User(choice) => {
            state.list.user = choice.id;
            state.picked = choice.id.map(|_| choice);
            reload(state, ctx)
        }
        Message::Users(Ok(v)) => {
            state.users = v.as_array().cloned().unwrap_or_default();
            Task::none()
        }
        Message::Users(Err(_)) => Task::none(),
        Message::Counts(Ok(c)) => {
            state.tab_labels = TABS.iter().map(|(k, l)| tab_label(l, Some(fmt::i(&c, k)))).collect();
            state.counts = Some(c);
            Task::none()
        }
        // a failed fetch keeps the counts drawn last (the list works without them)
        Message::Counts(Err(_)) => Task::none(),
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
    // "12 remaining", "3 awaiting approval", "2 failed"
    if let Some(c) = &state.counts {
        let badges = counts_badges(c);
        if !badges.is_empty() {
            title = title.push(iced::widget::Row::with_children(badges.into_iter().map(|(l, kind)| widgets::badge(l, kind))).spacing(6));
        }
    }
    if !state.quota.is_empty() {
        title = title.push(small_muted(state.quota.as_str()));
    }
    let mut controls = row![].spacing(8).align_y(Alignment::Center);
    if state.admin {
        // everyone's requests, or one person's (the buttons beside it take the same)
        let picked = state.picked.as_ref().and_then(|p| Some((p.id?, name_of(&p.label))));
        let options = user_options(&state.users, picked);
        let selected = options.iter().find(|o| o.id == state.list.user).cloned();
        controls = controls.push(
            row![
                small_muted("User"),
                pick_list(options, selected, Message::User)
                    .text_size(13)
                    .padding([6, 10])
                    .style(theme::pick)
                    .menu_style(theme::pick_menu),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
        );
    }
    // members without Fix their own downloads may not retry (the server answers "Ask an admin to retry")
    if request_list::may_fix(ctx.is_admin(), ctx.session.perm("fix")) {
        controls = controls.push(
            request_list::retry_failed_btn(&state.list, "Queue every failed request again").map(Message::List),
        );
    }
    // admins, on a server with Download now (its rows say whether it offers the button)
    if state.admin && request_list::has_download_now(state.list.rows()) {
        controls = controls.push(request_list::download_all_now_btn(&state.list).map(Message::List));
    }
    if state.admin {
        controls = controls.push(request_list::clear_btn("Clear finished").map(Message::List));
    }
    // admins: everyone's waiting requests, or the person picked; members: their own
    controls = controls.push(request_list::clear_waiting_btn(&state.list).map(Message::List));
    // the controls wrap onto a second line in a narrow window (four of them for an admin)
    let head = row![title, space().width(Length::Fill), controls.wrap()]
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
    let mut page = column![head].spacing(16).width(Length::Fill);
    if let Some(b) = request_list::storage_banner(ctx, &state.list) {
        page = page.push(b.map(Message::List));
    }
    // the tabs with their counts once they are in (the plain labels before the page opened)
    let items: Vec<(&str, &str)> = TABS
        .iter()
        .enumerate()
        .map(|(i, (k, l))| (*k, state.tab_labels.get(i).map(String::as_str).unwrap_or(l)))
        .collect();
    page.push(tabs(&items, &state.filter, Message::Filter))
        .push(card_flat(list).width(Length::Fill))
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
        assert_eq!(query("all", false, None), "limit=300");
        assert_eq!(query("pending", true, None), "limit=300&status=pending&mine=true");
        assert_eq!(query("failed", false, Some(7)), "limit=300&status=failed&user=7");
        assert_eq!(TABS.iter().map(|(k, _)| *k).collect::<Vec<_>>(), ["all", "active", "pending", "completed", "failed"]);
    }

    /// The admin's User list, as the web draws it: "All users", then each person with their count;
    /// the person picked stays with 0 once their requests were cleared.
    #[test]
    fn the_user_list() {
        let users = vec![serde_json::json!({ "id": 1, "username": "boss", "count": 1 }), serde_json::json!({ "id": 7, "username": "Caz", "count": 612 })];
        let o = user_options(&users, None);
        assert_eq!(o.iter().map(|c| c.label.as_str()).collect::<Vec<_>>(), ["All users", "boss (1)", "Caz (612)"]);
        assert_eq!((o[0].id, o[2].id), (None, Some(7)));
        let o = user_options(&users[..1], Some((7, "Caz")));
        assert_eq!(o.last(), Some(&UserChoice { id: Some(7), label: "Caz (0)".into() }));
        assert_eq!(user_options(&users, Some((7, "Caz"))).len(), 3, "listed once");
        assert_eq!(user_label("Caz", 612), "Caz (612)");
        assert_eq!(name_of("Caz (612)"), "Caz");
        assert_eq!(name_of("Dr. (Who) (3)"), "Dr. (Who)");
    }

    /// The tabs carry their counts once they are in, the badges only the groups that have
    /// requests, and the counts are whose requests the list shows.
    #[test]
    fn the_counts() {
        assert_eq!(tab_label("Failed", None), "Failed");
        assert_eq!(tab_label("Failed", Some(3)), "Failed 3");
        let c = serde_json::json!({ "all": 20, "remaining": 12, "pending": 0, "active": 12, "completed": 6, "failed": 2 });
        let labels: Vec<String> = counts_badges(&c).into_iter().map(|(l, _)| l).collect();
        assert_eq!(labels, ["12 remaining", "2 failed"]);
        assert!(counts_badges(&serde_json::json!({ "all": 4, "completed": 4 })).is_empty());
        assert_eq!(counts_query(true, Some(7)), "mine=true");
        assert_eq!(counts_query(false, Some(7)), "user=7");
        assert_eq!(counts_query(false, None), "");
    }
}
