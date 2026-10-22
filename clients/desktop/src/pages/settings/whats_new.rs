//! Settings → What's new. Behaviour reference: web-rs/src/whats_new.rs `render`: every release this server
//! includes with its notes (GET /api/changelog, newest first and open), and the switch for the popup that
//! shows once after an update (`prefs.whats_new_popup`, saved to the account). The popup itself is
//! src/whats_new.rs; `notes` here draws a release's notes for both.

use super::{card, help, switch};
use crate::api::ApiError;
use crate::fmt;
use crate::pages::{Ctx, El};
use crate::theme::{self, Badge, Btn};
use crate::widgets::{self, t};
use iced::widget::{button, column, row, space, text};
use iced::{Alignment, Length, Task};
use serde_json::{json, Value};
use std::collections::HashSet;

#[derive(Debug, Default)]
pub struct State {
    data: Option<Value>,
    popup: bool,
    /// The releases shown open (by position); the newest is open at first.
    open: HashSet<usize>,
    error: String,
}

#[derive(Clone, Debug)]
pub enum Message {
    Loaded(Result<Value, ApiError>),
    Popup(bool),
    Saved(Result<Value, ApiError>),
    Toggle(usize),
}

pub fn open(state: &mut State, ctx: &mut Ctx) -> Task<Message> {
    state.data = None;
    state.error.clear();
    state.open = HashSet::from([0]);
    let api = ctx.api.clone();
    Task::perform(async move { api.get("/changelog").await }, Message::Loaded)
}

/// What the switch says when it was saved.
pub fn saved_words(on: bool) -> &'static str {
    if on { "What's new will show after updates" } else { "What's new won't show after updates – it stays in Settings" }
}

pub fn update(state: &mut State, msg: Message, ctx: &mut Ctx) -> Task<Message> {
    match msg {
        Message::Loaded(Ok(v)) => {
            state.popup = fmt::truthy(&v, "popup");
            state.data = Some(v);
        }
        Message::Loaded(Err(e)) => state.error = e.message,
        Message::Popup(on) => {
            state.popup = on;
            let api = ctx.api.clone();
            return Task::perform(async move { api.put("/auth/prefs", json!({ "whats_new_popup": on })).await }, Message::Saved);
        }
        Message::Saved(Ok(_)) => {
            ctx.toast_ok(saved_words(state.popup));
            ctx.refresh_session();
        }
        Message::Saved(Err(e)) => {
            state.popup = !state.popup;
            ctx.toast_err(e.message);
        }
        Message::Toggle(i) => {
            if !state.open.remove(&i) {
                state.open.insert(i);
            }
        }
    }
    Task::none()
}

/// A release's notes as the server sends them (`k`: h, p, li, row).
pub fn notes<'a, M: 'a>(blocks: &[Value]) -> El<'a, M> {
    let p = theme::look();
    let mut c = column![].spacing(8).width(Length::Fill);
    for b in blocks {
        let line: El<'a, M> = match fmt::s(b, "k").as_str() {
            "h" => container_top(widgets::h3(fmt::s(b, "text")).into()),
            "p" => t(fmt::s(b, "text")).size(14).into(),
            "li" => {
                let lead = fmt::s(b, "lead");
                let mut spans: Vec<iced::widget::text::Span<'a, ()>> = Vec::new();
                if !lead.is_empty() {
                    spans.push(iced::widget::span(format!("{lead} ")).font(theme::bold()));
                }
                spans.push(iced::widget::span(fmt::s(b, "text")));
                row![t("•").size(14).color(p.muted), iced::widget::rich_text(spans).size(14).width(Length::Fill)].spacing(8).into()
            }
            "row" => {
                let head = fmt::truthy(b, "head");
                let mut r = row![].spacing(12);
                for c in b["cells"].as_array().into_iter().flatten() {
                    let cell = t(c.as_str().unwrap_or("").to_string()).size(13).width(Length::FillPortion(1));
                    r = r.push(if head { cell.font(theme::semibold()) } else { cell.color(p.muted) });
                }
                r.into()
            }
            _ => continue,
        };
        c = c.push(line);
    }
    c.into()
}

fn container_top<'a, M: 'a>(e: El<'a, M>) -> El<'a, M> {
    iced::widget::container(e).padding(widgets::pad(8.0, 0.0, 0.0, 0.0)).into()
}

pub fn view<'a>(state: &'a State, _ctx: &Ctx<'a>) -> El<'a, Message> {
    if !state.error.is_empty() {
        return card("What's new", vec![super::err_text(state.error.clone()).into()]);
    }
    let Some(data) = &state.data else { return widgets::skeleton_rows(4) };
    let mut page = column![card("What's new", vec![
        help(format!("This server runs Encore {}. After an update, a short list of what changed shows once on the website and in the desktop app.", fmt::s(data, "version"))).into(),
        switch(state.popup, "Show what's new after updates", Message::Popup),
    ])]
    .spacing(14);
    let releases = data["releases"].as_array().cloned().unwrap_or_default();
    if releases.is_empty() {
        page = page.push(widgets::card(widgets::muted("No release notes in this build.")).width(Length::Fill));
    }
    for (i, r) in releases.into_iter().enumerate() {
        let open = state.open.contains(&i);
        let head = button(
            row![
                widgets::h2(fmt::s(&r, "title")),
                space().width(Length::Fill),
                widgets::badge(fmt::s(&r, "version"), Badge::Plain),
                text(if open { "▴" } else { "▾" }).size(16),
            ]
            .spacing(10)
            .align_y(Alignment::Center),
        )
        .padding(0)
        .width(Length::Fill)
        .style(theme::btn(Btn::Row))
        .on_press(Message::Toggle(i));
        let mut body = column![head].spacing(12);
        if open {
            let blocks = r["blocks"].as_array().cloned().unwrap_or_default();
            body = body.push(notes(&blocks));
        }
        page = page.push(widgets::card(body.width(Length::Fill)).width(Length::Fill));
    }
    page.into()
}
