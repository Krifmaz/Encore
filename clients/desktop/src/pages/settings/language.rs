//! Settings → Language. Behaviour reference: web-rs/src/pages/translate.rs `render_language`: how
//! translated titles show (the server's modes) and into which language, saved to the account; for admins the
//! sources of translated titles with their state and switch (the server's `tiers`).

use super::{block, help, switch};
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
    /// An admin switched a source on or off: (its setting, on).
    Tier(String, bool),
    TierSaved(Result<Value, ApiError>),
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
        Message::Tier(key, on) => {
            // shown switched at once; the state lines come from the server after the save
            if let Some(tier) = state.status["tiers"].as_array_mut().and_then(|a| a.iter_mut().find(|t| t["setting"].as_str() == Some(key.as_str()))) {
                tier["on"] = json!(on);
            }
            let api = ctx.api.clone();
            let mut body = serde_json::Map::new();
            body.insert(key, json!(on));
            return Task::perform(async move { api.put("/admin/settings", Value::Object(body)).await }, Message::TierSaved);
        }
        Message::TierSaved(r) => {
            match r {
                Ok(_) => ctx.toast_ok("Saved"),
                Err(e) => ctx.toast_err(e.message),
            }
            let api = ctx.api.clone();
            let api2 = ctx.api.clone();
            return Task::batch([
                Task::perform(async move { api.post_empty("/translate/run").await }, |_| Message::Ran),
                Task::perform(async move { api2.get("/translate/status").await }, Message::Status),
            ]);
        }
    }
    Task::none()
}

/// One source of the Translation sources card: its setting (None: no switch, the corrections by hand), on,
/// its state line and what it does.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceRow {
    pub setting: Option<String>,
    pub on: bool,
    pub state: String,
    pub help: String,
}

/// The rows of the Translation sources card, strongest first (the server's `tiers`); None for an answer without
/// them (members, older servers).
pub fn source_rows(s: &Value) -> Option<Vec<SourceRow>> {
    let tiers = s["tiers"].as_array().filter(|a| !a.is_empty())?;
    Some(tiers.iter().map(|tier| SourceRow {
        setting: tier["setting"].as_str().filter(|k| !k.is_empty()).map(String::from),
        on: fmt::truthy(tier, "on"),
        state: fmt::s(tier, "state"),
        help: fmt::s(tier, "help"),
    }).collect())
}

/// Admins: how far the translations are ("3120 of 3400 titles have a translation"), with the titles left for
/// the local model only while it can run – installed, downloaded and switched on, as the web's; None without
/// the counts (members).
pub fn counts_line(s: &Value) -> Option<String> {
    s.get("rows").filter(|r| !r.is_null())?;
    let model_on = fmt::items(&s["tiers"]).iter().find(|t| fmt::s(t, "id") == "model").is_none_or(|t| fmt::truthy(t, "on"));
    let waiting = if fmt::truthy(s, "translator") && model_on { format!(" · {} waiting for the local model", fmt::i(s, "waiting")) } else { String::new() };
    Some(format!("{} of {} titles have a translation{waiting}{}", fmt::i(s, "shown"), fmt::i(s, "rows"), if fmt::truthy(s, "running") { " · running now" } else { "" }))
}

/// Admins: the sources of translated titles, strongest first – each one's state, what it does, and its switch
/// (the same settings as Server → Library → Translated titles).
fn sources<'a>(s: &'a Value) -> Option<El<'a, Message>> {
    let rows = source_rows(s)?;
    let mut lines: Vec<El<'a, Message>> = vec![help("Strongest first: a source higher up wins over the ones below it, and its results replace theirs when it gets to a title later.").into()];
    for r in rows {
        let row: El<'a, Message> = match r.setting {
            Some(key) => switch(r.on, r.state, move |on| Message::Tier(key.clone(), on)),
            None => t(r.state).size(14).into(),
        };
        lines.push(column![row, help(r.help)].spacing(2).into());
    }
    lines.push(help("The DeepL API key and the monthly limit are under Settings → Library → Translated titles.").into());
    Some(super::card("Translation sources", lines))
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
        help("Corrections by hand come first, then official names (MusicBrainz and Apple, where the label published one), romanization made on this server, then DeepL and the local model where an admin set them up. Titles on your screen are translated first, the rest of the library in the background. Fix any title with ⋯ → Edit translation.").into(),
    ];
    // which sources this server uses (the server's words)
    if let Some(line) = s["summary"].as_str().filter(|l| !l.is_empty()) {
        lines.push(help(line).into());
    }
    if let Some(counts) = counts_line(s) {
        lines.push(faint(counts).size(12).into());
    }
    let mut page = column![widgets::card(column(lines).spacing(14).width(Length::Fill)).width(Length::Fill)].spacing(14);
    if let Some(card) = sources(s) {
        page = page.push(card);
    }
    container(page).max_width(820).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiers() -> Value {
        json!({ "tiers": [
            { "id": "manual", "name": "Corrections by hand", "setting": null, "on": true, "state": "Corrections by hand: none yet", "help": "Made with ⋯ → Edit translation." },
            { "id": "official", "name": "Official names", "setting": "translate_official", "on": true, "state": "Official names: MusicBrainz and Apple", "help": "MusicBrainz." },
            { "id": "romanize", "name": "Romanization", "setting": "translate_romanize", "on": false, "state": "Romanization: off", "help": "Kana." },
            { "id": "deepl", "name": "DeepL", "setting": "translate_deepl", "on": true, "state": "DeepL: 12 340 of 400 000 characters this month", "help": "Titles." },
            { "id": "model", "name": "Local model", "setting": "translate_local_model", "on": true, "state": "Local model: not installed – install it in Plugins", "help": "Last." } ] })
    }

    /// One row per source with its state, a switch where it has a setting (none for the corrections by hand),
    /// nothing for a member's answer (no `tiers`), as the web's card.
    #[test]
    fn the_translation_sources() {
        let rows = source_rows(&tiers()).unwrap();
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[0], SourceRow { setting: None, on: true, state: "Corrections by hand: none yet".into(), help: "Made with ⋯ → Edit translation.".into() });
        assert_eq!(rows.iter().filter(|r| r.setting.is_some()).count(), 4);
        assert_eq!((rows[2].setting.as_deref(), rows[2].on), (Some("translate_romanize"), false));
        assert_eq!(rows[3].state, "DeepL: 12 340 of 400 000 characters this month");
        assert_eq!(source_rows(&json!({ "translator": false, "summary": "x" })), None);
    }

    /// The titles left for the local model are counted only while it can run.
    #[test]
    fn the_counts_say_the_model_only_when_it_runs() {
        let mut s = tiers();
        s["rows"] = json!(3400);
        s["shown"] = json!(3120);
        s["waiting"] = json!(280);
        s["translator"] = json!(false);
        assert_eq!(counts_line(&s).as_deref(), Some("3120 of 3400 titles have a translation"), "not installed");
        s["translator"] = json!(true);
        assert_eq!(counts_line(&s).as_deref(), Some("3120 of 3400 titles have a translation · 280 waiting for the local model"));
        s["tiers"][4]["on"] = json!(false);
        assert_eq!(counts_line(&s).as_deref(), Some("3120 of 3400 titles have a translation"), "switched off");
        assert_eq!(counts_line(&json!({ "translator": true })), None, "a member");
    }
}
