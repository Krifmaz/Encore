//! What's new: the popup on the first start after the server was updated. Behaviour reference:
//! web-rs/src/whats_new.rs. After the session is known the app asks GET /api/changelog/new; when it says
//! `show`, the popup lists each new release's highlights, and POST /api/changelog/seen marks this version
//! seen for the account (so the website does not show it again either). Full changelog opens Settings →
//! What's new; the switch turns the popup off (`prefs.whats_new_popup`).
//!
//! The popup is the kit's (`kit_iced::whats_new::Popup`); this module makes its requests.

use crate::api::ApiError;
use crate::pages::settings::Section;
use crate::pages::{Ctx, El, Page};
use iced::Task;
use kit_iced::whats_new::{self as kit, Popup, PopupMsg};
use serde_json::{json, Value};

#[derive(Debug, Default)]
pub struct State {
    popup: Popup,
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

impl From<PopupMsg> for Message {
    fn from(m: PopupMsg) -> Self {
        match m {
            PopupMsg::Close => Message::Close,
            PopupMsg::Full => Message::Full,
            PopupMsg::Popup(on) => Message::Popup(on),
        }
    }
}

/// Ask the server whether anything is new for this account.
pub fn check(ctx: &Ctx) -> Task<Message> {
    let api = ctx.api.clone();
    Task::perform(async move { api.get("/changelog/new").await }, Message::Loaded)
}

pub fn open(state: &State) -> bool {
    state.popup.is_open()
}

pub fn update(state: &mut State, msg: Message, ctx: &mut Ctx) -> Task<Message> {
    match msg {
        Message::Loaded(Ok(v)) => {
            if state.popup.loaded(&v) {
                let api = ctx.api.clone();
                return Task::perform(async move { api.post("/changelog/seen", json!({})).await }, |_| Message::Seen);
            }
        }
        // an older server has no changelog: nothing to show
        Message::Loaded(Err(_)) | Message::Seen => {}
        Message::Close => state.popup.close(),
        Message::Full => {
            state.popup.close();
            ctx.navigate(Page::Settings(Section::WhatsNew));
        }
        Message::Popup(on) => {
            state.popup.set_on(on);
            let api = ctx.api.clone();
            return Task::perform(async move { api.put("/auth/prefs", json!({ "whats_new_popup": on })).await }, move |r| Message::Saved(on, r));
        }
        Message::Saved(on, Ok(_)) => ctx.toast_ok(kit::saved_words(on)),
        Message::Saved(on, Err(e)) => {
            state.popup.save_failed(on);
            ctx.toast_err(e.message);
        }
    }
    Task::none()
}

pub fn view<'a>(state: &'a State) -> Option<El<'a, Message>> {
    kit::popup(&state.popup).map(|e| e.map(Message::from))
}
