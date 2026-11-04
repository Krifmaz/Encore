//! Settings → What's new. Behaviour reference: web-rs/src/whats_new.rs `render`: every release this server
//! includes with its notes (GET /api/changelog, newest first and open), and the switch for the popup that
//! shows once after an update (`prefs.whats_new_popup`, saved to the account). The popup itself is
//! src/whats_new.rs; both draw a release's notes with the kit's `whats_new::notes`.
//!
//! The page is the kit's (`kit_iced::whats_new::Changelog`); this module makes its requests.

use crate::api::ApiError;
use crate::pages::{Ctx, El};
use iced::Task;
use kit_iced::whats_new::{self as kit, Changelog, ChangelogMsg};
use serde_json::{json, Value};

#[derive(Debug, Default)]
pub struct State {
    page: Changelog,
}

#[derive(Clone, Debug)]
pub enum Message {
    Loaded(Result<Value, ApiError>),
    Page(ChangelogMsg),
    Saved(Result<Value, ApiError>),
}

pub fn open(state: &mut State, ctx: &mut Ctx) -> Task<Message> {
    state.page.reset();
    let api = ctx.api.clone();
    Task::perform(async move { api.get("/changelog").await }, Message::Loaded)
}

/// What the switch says when it was saved.
pub fn saved_words(on: bool) -> &'static str {
    kit::saved_words(on)
}

pub fn update(state: &mut State, msg: Message, ctx: &mut Ctx) -> Task<Message> {
    match msg {
        Message::Loaded(r) => state.page.loaded(r.map_err(|e| e.message)),
        Message::Page(m) => {
            if let Some(on) = state.page.update(m) {
                let api = ctx.api.clone();
                return Task::perform(async move { api.put("/auth/prefs", json!({ "whats_new_popup": on })).await }, Message::Saved);
            }
        }
        Message::Saved(Ok(_)) => {
            ctx.toast_ok(saved_words(state.page.popup()));
            ctx.refresh_session();
        }
        Message::Saved(Err(e)) => {
            state.page.save_failed();
            ctx.toast_err(e.message);
        }
    }
    Task::none()
}

pub fn view<'a>(state: &'a State, _ctx: &Ctx<'a>) -> El<'a, Message> {
    kit::changelog(&state.page, "Encore").map(Message::Page)
}
