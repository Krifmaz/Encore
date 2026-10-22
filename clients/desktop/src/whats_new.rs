//! What's new: the popup on the first start after the server was updated. Behaviour reference:
//! web-rs/src/whats_new.rs. After the session is known the app asks GET /api/changelog/new; when it says
//! `show`, the popup lists each new release's highlights, and POST /api/changelog/seen marks this version
//! seen for the account (so the website does not show it again either). Full changelog opens Settings →
//! What's new; the switch turns the popup off (`prefs.whats_new_popup`).

use crate::api::ApiError;
use crate::fmt;
use crate::pages::settings::{whats_new as page, Section};
use crate::pages::{Ctx, El, Page};
use crate::theme::{self, Btn};
use crate::widgets::{self, t};
use iced::widget::{column, row, space, toggler};
use iced::{Alignment, Length, Task};
use serde_json::{json, Value};

#[derive(Debug, Default)]
pub struct State {
    /// The new releases while the popup is open.
    releases: Option<Vec<Value>>,
    on: bool,
}

#[derive(Clone, Debug)]
pub enum Message {
    Loaded(Result<Value, ApiError>),
    Seen,
    Close,
    Full,
    Popup(bool),
    Saved(bool, Result<Value, ApiError>),
}

/// Ask the server whether anything is new for this account.
pub fn check(ctx: &Ctx) -> Task<Message> {
    let api = ctx.api.clone();
    Task::perform(async move { api.get("/changelog/new").await }, Message::Loaded)
}

pub fn open(state: &State) -> bool {
    state.releases.is_some()
}

pub fn update(state: &mut State, msg: Message, ctx: &mut Ctx) -> Task<Message> {
    match msg {
        Message::Loaded(Ok(v)) => {
            if fmt::truthy(&v, "show") {
                state.releases = Some(v["releases"].as_array().cloned().unwrap_or_default());
                state.on = true;
                let api = ctx.api.clone();
                return Task::perform(async move { api.post("/changelog/seen", json!({})).await }, |_| Message::Seen);
            }
        }
        // an older server has no changelog: nothing to show
        Message::Loaded(Err(_)) | Message::Seen => {}
        Message::Close => state.releases = None,
        Message::Full => {
            state.releases = None;
            ctx.navigate(Page::Settings(Section::WhatsNew));
        }
        Message::Popup(on) => {
            state.on = on;
            let api = ctx.api.clone();
            return Task::perform(async move { api.put("/auth/prefs", json!({ "whats_new_popup": on })).await }, move |r| Message::Saved(on, r));
        }
        Message::Saved(on, Ok(_)) => ctx.toast_ok(page::saved_words(on)),
        Message::Saved(on, Err(e)) => {
            state.on = !on;
            ctx.toast_err(e.message);
        }
    }
    Task::none()
}

pub fn view<'a>(state: &'a State) -> Option<El<'a, Message>> {
    let releases = state.releases.as_ref()?;
    let p = theme::look();
    let mut body = column![].spacing(18).width(Length::Fill);
    for r in releases {
        let mut c = column![widgets::h3(fmt::s(r, "title"))].spacing(6);
        let summary = fmt::s(r, "summary");
        if !summary.is_empty() {
            c = c.push(widgets::muted(summary).size(14));
        }
        for h in r["highlights"].as_array().into_iter().flatten() {
            c = c.push(row![t("•").size(14).color(p.muted), t(h.as_str().unwrap_or("").to_string()).size(14).width(Length::Fill)].spacing(8));
        }
        body = body.push(c);
    }
    let footer = row![
        toggler(state.on).label("Show this after updates").text_size(13).spacing(8).on_toggle(Message::Popup).style(theme::switch),
        space().width(Length::Fill),
        widgets::btn("Full changelog", Btn::Default, Some(Message::Full)),
        widgets::btn("Close", Btn::Primary, Some(Message::Close)),
    ]
    .spacing(10)
    .align_y(Alignment::Center)
    .width(Length::Fill);
    Some(widgets::modal("What's new", body, Some(footer.into()), false, Message::Close))
}
