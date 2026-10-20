//! Settings → Language. Behaviour reference: web-rs/src/pages/translate.rs `render_language`: how
//! translated titles show (the server's modes) and into which language, saved to the account.

use super::{block, help, warn_text};
use crate::api::ApiError;
use crate::fmt;
use crate::pages::{Ctx, El};
use crate::theme::{self, Btn};
use crate::widgets::{self, faint, h2, t};
use iced::widget::{button, column, container, pick_list, row};
use iced::{Length, Task};
use serde_json::{json, Value};

/// One language of `/api/translate/languages`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lang {
    pub code: String,
    pub name: String,
}

impl std::fmt::Display for Lang {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.name)
    }
}

#[derive(Debug, Default)]
pub struct State {
    /// `/api/translate/languages`: modes and languages.
    opts: Option<Value>,
    status: Value,
    mode: String,
    lang: String,
    error: String,
}

#[derive(Clone, Debug)]
pub enum Message {
    Options(Result<Value, ApiError>),
    Status(Result<Value, ApiError>),
    Mode(String),
    Lang(Lang),
    Saved(Result<Value, ApiError>),
    /// `/translate/run` was asked for (its answer does not matter).
    Ran,
}

pub fn open(state: &mut State, ctx: &mut Ctx) -> Task<Message> {
    let prefs = &ctx.session.user["prefs"];
    state.mode = prefs["translate_mode"].as_str().unwrap_or("off").to_string();
    state.lang = prefs["translate_lang"].as_str().unwrap_or("").to_string();
    state.opts = None;
    state.error.clear();
    let api = ctx.api.clone();
    let api2 = ctx.api.clone();
    Task::batch([
        Task::perform(async move { api.get("/translate/languages").await }, Message::Options),
        Task::perform(async move { api2.get("/translate/status").await }, Message::Status),
    ])
}

fn save(patch: Value, ctx: &mut Ctx) -> Task<Message> {
    let api = ctx.api.clone();
    Task::perform(async move { api.put("/auth/prefs", patch).await }, Message::Saved)
}

pub fn update(state: &mut State, msg: Message, ctx: &mut Ctx) -> Task<Message> {
    match msg {
        Message::Options(Ok(o)) => {
            if state.lang.is_empty() {
                state.lang = o["default"].as_str().unwrap_or("en").to_string();
            }
            state.opts = Some(o);
        }
        Message::Options(Err(e)) => state.error = e.message,
        Message::Status(Ok(s)) => state.status = s,
        Message::Status(Err(_)) => state.status = Value::Null,
        Message::Mode(m) => return save(json!({ "translate_mode": m }), ctx),
        Message::Lang(l) => return save(json!({ "translate_lang": l.code }), ctx),
        Message::Saved(Ok(p)) => {
            state.mode = p["translate_mode"].as_str().unwrap_or("off").to_string();
            if let Some(l) = p["translate_lang"].as_str() {
                state.lang = l.to_string();
            }
            ctx.refresh_session();
            ctx.toast_ok("Saved");
            let api = ctx.api.clone();
            let api2 = ctx.api.clone();
            return Task::batch([
                Task::perform(async move { api.post_empty("/translate/run").await }, |_| Message::Ran),
                Task::perform(async move { api2.get("/translate/status").await }, Message::Status),
            ]);
        }
        Message::Saved(Err(e)) => ctx.toast_err(e.message),
        Message::Ran => {}
    }
    Task::none()
}

pub fn view<'a>(state: &'a State, _ctx: &Ctx<'a>) -> El<'a, Message> {
    if !state.error.is_empty() {
        return widgets::error_box(&state.error);
    }
    let Some(opts) = &state.opts else {
        return widgets::skeleton_rows(4);
    };
    let modes: Vec<(String, String)> = fmt::items(&opts["modes"]).iter().map(|m| (fmt::s(m, "id"), fmt::s(m, "label"))).collect();
    let mode_row: El<'a, Message> = if modes.is_empty() {
        help("No modes.").into()
    } else {
        let mut r = row![].spacing(0);
        for (id, label) in modes {
            let is = id == state.mode;
            r = r.push(
                button(t(label).size(13.5).font(if is { theme::semibold() } else { iced::Font::DEFAULT }))
                    .padding([8, 14])
                    .style(theme::btn(if is { Btn::SegOn } else { Btn::Seg }))
                    .on_press(Message::Mode(id)),
            );
        }
        container(r).style(theme::tabs_frame).clip(true).into()
    };
    let langs: Vec<Lang> = fmt::items(&opts["languages"])
        .iter()
        .map(|l| Lang {
            code: fmt::s(l, "code"),
            name: fmt::s(l, "name"),
        })
        .collect();
    let current = langs.iter().find(|l| l.code == state.lang).cloned();
    // the web disables the select while translations are off; here it stays usable and the
    // choice is kept for when they are switched on
    let picker: El<'a, Message> = pick_list(langs, current, Message::Lang)
        .text_size(13)
        .padding([7, 10])
        .style(theme::pick)
        .menu_style(theme::pick_menu)
        .into();
    let s = &state.status;
    let mut lines = vec![
        h2("Language").into(),
        help("Song, album and artist names in another script – Japanese, Korean, Chinese, Cyrillic and more – can show translated or romanized. Saved to your account, so it follows you to every device and to the car.").into(),
        block("Translated titles", vec![mode_row]),
        widgets::field("Translate into", container(picker).width(Length::Shrink)),
        help("Official translations come first (MusicBrainz and Apple, where the label published one); the rest come from the Translator on this server. Titles on your screen are translated first, the rest of the library in the background. Fix any title with ⋯ → Edit translation.").into(),
    ];
    if s["translator"].as_bool() == Some(false) {
        lines.push(warn_text("The Translator is not installed on this server, so only official translations show. An admin can install it under Settings → Downloads → Download tools.").into());
    }
    if s.get("rows").is_some_and(|r| !r.is_null()) {
        lines.push(
            faint(format!(
                "{} of {} titles have a translation · {} waiting for the Translator{}",
                fmt::i(s, "shown"),
                fmt::i(s, "rows"),
                fmt::i(s, "waiting"),
                if fmt::truthy(s, "running") { " · running now" } else { "" }
            ))
            .size(12)
            .into(),
        );
    }
    container(widgets::card(column(lines).spacing(14).width(Length::Fill)).width(Length::Fill))
        .max_width(820)
        .into()
}
