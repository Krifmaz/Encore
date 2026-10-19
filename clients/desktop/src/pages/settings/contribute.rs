//! Settings → Contribute. Behaviour reference: web-rs/src/pages/contribute.rs: send AcoustID
//! fingerprints and MusicBrainz ISRCs for songs the audio confirmed, and how albums missing from
//! MusicBrainz get added.

use super::{env_locked, help, input, link, result_line_copy, switch_maybe};
use crate::api::ApiError;
use crate::fmt;
use crate::pages::{Ctx, El};
use crate::theme::Btn;
use crate::widgets::{self, btn_icon, stat, t, Icon};
use iced::widget::{column, container, row};
use iced::{Alignment, Length, Task};
use serde_json::{Map, Value};
use std::collections::HashMap;

#[derive(Debug, Default)]
pub struct State {
    values: Value,
    data: Option<Value>,
    /// The typed secrets and ids: empty means keep the saved value.
    edits: HashMap<&'static str, String>,
    toggles: HashMap<&'static str, bool>,
    flash: Option<(bool, String)>,
    busy: bool,
    error: String,
}

#[derive(Clone, Debug)]
pub enum Message {
    Values(Result<Value, ApiError>),
    Data(Result<Value, ApiError>),
    Toggle(&'static str, bool),
    Input(&'static str, String),
    Save(&'static str),
    Saved(Result<Value, ApiError>),
    Run(&'static str),
    Ran(Result<Value, ApiError>),
    MbConnect,
    MbConnected(Result<Value, ApiError>),
    MbDisconnect,
    MbDisconnected(Result<Value, ApiError>),
    OpenUrl(String),
    /// Copy a failed result's words.
    CopyText(String),
}

fn reload(ctx: &Ctx) -> Task<Message> {
    let api = ctx.api.clone();
    let api2 = ctx.api.clone();
    Task::batch([
        Task::perform(async move { api.get("/admin/settings").await }, Message::Values),
        Task::perform(async move { api2.get("/contribute").await }, Message::Data),
    ])
}

pub fn open(state: &mut State, ctx: &mut Ctx) -> Task<Message> {
    state.data = None;
    state.edits.clear();
    state.toggles.clear();
    state.flash = None;
    state.busy = false;
    state.error.clear();
    reload(ctx)
}

/// The body of a Save: the part's switch and its typed values (secrets only when typed).
pub fn body(state: &State, part: &str) -> Map<String, Value> {
    let mut m = Map::new();
    let (toggle, keys): (&'static str, &[&'static str]) = if part == "acoustid" {
        ("contribute_acoustid", &["acoustid_user_key"])
    } else {
        ("contribute_isrc", &["mb_client_id", "mb_client_secret"])
    };
    if !env_locked(&state.values, toggle) {
        let on = state.toggles.get(toggle).copied().unwrap_or_else(|| fmt::truthy(&state.values, toggle));
        m.insert(toggle.into(), Value::Bool(on));
    }
    for k in keys {
        if env_locked(&state.values, k) {
            continue;
        }
        let v = state.edits.get(k).map(|s| s.trim().to_string()).unwrap_or_default();
        let secret = k.ends_with("_key") || k.ends_with("_secret");
        if secret && (v.is_empty() || super::is_masked(&v)) {
            continue;
        }
        if !secret && !state.edits.contains_key(k) {
            continue;
        }
        m.insert((*k).into(), Value::String(v));
    }
    m
}

pub fn update(state: &mut State, msg: Message, ctx: &mut Ctx) -> Task<Message> {
    match msg {
        Message::Values(Ok(v)) => {
            state.values = v;
            state.toggles.clear();
        }
        Message::Values(Err(e)) => state.error = e.message,
        Message::Data(Ok(d)) => state.data = Some(d),
        Message::Data(Err(e)) => state.error = e.message,
        Message::Toggle(k, on) => {
            state.toggles.insert(k, on);
        }
        Message::Input(k, s) => {
            state.edits.insert(k, s);
        }
        Message::Save(part) => {
            state.busy = true;
            let api = ctx.api.clone();
            let b = body(state, part);
            return Task::perform(async move { api.put("/admin/settings", Value::Object(b)).await }, Message::Saved);
        }
        Message::Saved(r) => {
            state.busy = false;
            match r {
                Ok(_) => {
                    ctx.toast_ok("Settings saved");
                    state.edits.clear();
                    return reload(ctx);
                }
                Err(e) => ctx.toast_err(e.message),
            }
        }
        Message::Run(what) => {
            state.busy = true;
            let api = ctx.api.clone();
            return Task::perform(async move { api.post_empty(&format!("/contribute/{what}/run")).await }, Message::Ran);
        }
        Message::Ran(r) => {
            state.busy = false;
            match r {
                Ok(r) => ctx.toast_ok(if fmt::truthy(&r, "message") { fmt::s(&r, "message") } else { "Done".into() }),
                Err(e) => ctx.toast_err(e.message),
            }
            return reload(ctx);
        }
        Message::MbConnect => {
            state.busy = true;
            let api = ctx.api.clone();
            let b = body(state, "isrc");
            return Task::perform(
                async move {
                    api.put("/admin/settings", Value::Object(b)).await?;
                    api.post_empty("/contribute/mb/connect").await
                },
                Message::MbConnected,
            );
        }
        Message::MbConnected(r) => {
            state.busy = false;
            match r {
                Ok(r) => {
                    ctx.open_url(fmt::s(&r, "url"));
                    state.flash = Some((true, "Allow Encore in the MusicBrainz page that opened, then come back here.".into()));
                }
                Err(e) => ctx.toast_err(e.message),
            }
            return reload(ctx);
        }
        Message::MbDisconnect => {
            let api = ctx.api.clone();
            return Task::perform(async move { api.post_empty("/contribute/mb/disconnect").await }, Message::MbDisconnected);
        }
        Message::MbDisconnected(r) => {
            match r {
                Ok(_) => ctx.toast_ok("Disconnected"),
                Err(e) => ctx.toast_err(e.message),
            }
            return reload(ctx);
        }
        Message::OpenUrl(u) => ctx.open_url(u),
        Message::CopyText(text) => ctx.copy_text(text),
    }
    Task::none()
}

fn last_line<'a>(l: &Value) -> Option<El<'a, Message>> {
    if !fmt::truthy(l, "at") {
        return None;
    }
    let ok = fmt::truthy(l, "ok");
    let text = format!("Last run {}: {}", fmt::ago(fmt::f(l, "at")), fmt::s(l, "message"));
    Some(if ok { help(text).into() } else { super::err_text(text).into() })
}

fn secret_field<'a>(state: &'a State, key: &'static str, label: &'a str, hint: &'a str) -> El<'a, Message> {
    let locked = env_locked(&state.values, key);
    let set = fmt::truthy(&state.values, key);
    let placeholder = if locked {
        "set by environment"
    } else if set {
        "saved – paste a new value to replace"
    } else {
        "not set"
    };
    let value = state.edits.get(key).cloned().unwrap_or_default();
    let field: El<'a, Message> = if locked {
        super::input_locked::<Message>("").width(Length::Fill).into()
    } else {
        input(placeholder, &value, move |s| Message::Input(key, s)).secure(true).width(Length::Fill).into()
    };
    let mut c = column![widgets::field(label, field)].spacing(3);
    if !hint.is_empty() {
        c = c.push(help(hint));
    }
    c.into()
}

fn text_field<'a>(state: &'a State, key: &'static str, label: &'a str, hint: String) -> El<'a, Message> {
    let locked = env_locked(&state.values, key);
    let value = state.edits.get(key).cloned().unwrap_or_else(|| super::vs(&state.values[key]));
    let field: El<'a, Message> = if locked {
        super::input_locked::<Message>(&value).width(Length::Fill).into()
    } else {
        input("", &value, move |s| Message::Input(key, s)).width(Length::Fill).into()
    };
    column![widgets::field(label, field), help(hint)].spacing(3).into()
}

fn toggle<'a>(state: &'a State, key: &'static str, label: &'a str) -> El<'a, Message> {
    let locked = env_locked(&state.values, key);
    let on = state.toggles.get(key).copied().unwrap_or_else(|| fmt::truthy(&state.values, key));
    switch_maybe(on, label, (!locked).then_some(move |v| Message::Toggle(key, v)))
}

pub fn view<'a>(state: &'a State, _ctx: &Ctx<'a>) -> El<'a, Message> {
    if !state.error.is_empty() {
        return widgets::error_box(&state.error);
    }
    let Some(d) = &state.data else {
        return widgets::skeleton_rows(6);
    };
    if state.values.is_null() {
        return widgets::skeleton_rows(6);
    }
    let (a, m) = (&d["acoustid"], &d["isrc"]);
    let mut intro = vec![help("Encore leans on AcoustID and MusicBrainz to know what a file is. Here it can give back: fingerprints of songs whose recording the audio confirmed, ISRCs MusicBrainz is missing, and albums that are only on Deezer or Apple Music. Only songs Encore is sure about are sent; nothing is sent until you add your own keys.").into()];
    if let Some((ok, msg)) = &state.flash {
        intro.push(result_line_copy(*ok, msg, Message::CopyText));
    }
    let intro_card = super::card("Contribute", intro);

    let mut acoustid: Vec<El<'a, Message>> = vec![
        help("When a download's audio confirms the MusicBrainz recording it was asked for, its fingerprint is sent to AcoustID in batches, once per song. Your user key says who sent it; the API key under Apps & sources → Verification is Encore's application key.").into(),
        widgets::stat::stats(vec![
            stat::stat(stat::num(fmt::f(a, "queued")), "Waiting to send"),
            stat::stat(stat::num(fmt::f(a, "submitted")), "Sent, being processed"),
            stat::stat(stat::num(fmt::f(a, "imported")), "In AcoustID"),
            stat::stat(stat::num(fmt::f(a, "failed")), "Failed"),
        ]),
    ];
    if fmt::truthy(a, "why_not") && fmt::truthy(a, "enabled") {
        acoustid.push(help(format!("{}{}", fmt::s(a, "why_not"), if fmt::truthy(a, "can_fingerprint") { "" } else { ". fpcalc is not installed on this server" })).into());
    }
    if let Some(l) = last_line(&a["last"]) {
        acoustid.push(l);
    }
    acoustid.push(toggle(state, "contribute_acoustid", "Send fingerprints of confirmed songs to AcoustID"));
    acoustid.push(secret_field(
        state,
        "acoustid_user_key",
        "AcoustID user key",
        "Sign in at acoustid.org and copy the user API key shown on your profile. It is not the application key: that one identifies Encore, this one identifies you.",
    ));
    acoustid.push(container(btn_icon(Icon::Check, "Save", Btn::Primary, (!state.busy).then_some(Message::Save("acoustid")))).align_x(Alignment::End).width(Length::Fill).into());
    let acoustid_card = widgets::card(
        column![
            super::between(
                widgets::h2("AcoustID fingerprints").into(),
                btn_icon(Icon::Refresh, "Send now", Btn::Default, (fmt::truthy(a, "ready") && !state.busy).then_some(Message::Run("acoustid"))).into(),
            ),
            column(acoustid).spacing(12),
        ]
        .spacing(12)
        .width(Length::Fill),
    )
    .width(Length::Fill);

    let connected = fmt::truthy(m, "connected");
    let mb_buttons: El<'a, Message> = if connected {
        row![
            btn_icon(Icon::Refresh, "Send now", Btn::Default, (!state.busy).then_some(Message::Run("isrc"))),
            widgets::btn_sm("Disconnect", Btn::Default, Some(Message::MbDisconnect)),
        ]
        .spacing(8)
        .into()
    } else {
        btn_icon(Icon::Link, "Connect MusicBrainz", Btn::Default, (fmt::truthy(m, "has_client") && !state.busy).then_some(Message::MbConnect)).into()
    };
    let callback = if fmt::truthy(m, "redirect_uri") {
        fmt::s(m, "redirect_uri")
    } else {
        "<Public URL>/api/contribute/mb/callback (set the Public URL under General first)".to_string()
    };
    let account = if connected {
        format!("Connected as {}", if fmt::truthy(m, "account") { fmt::s(m, "account") } else { "your account".to_string() })
    } else {
        "Not connected".to_string()
    };
    let mut isrc: Vec<El<'a, Message>> = vec![
        help("Songs from Deezer, Tidal and Qobuz carry the ISRC of the recording. When the audio confirmed the MusicBrainz recording and MusicBrainz does not list that ISRC yet, Encore adds it under your MusicBrainz account. ISRCs that MusicBrainz has on another recording are left for a person to look at.").into(),
        t(account).size(13).into(),
        widgets::stat::stats(vec![
            stat::stat(stat::num(fmt::f(m, "queued")), "Waiting to send"),
            stat::stat(stat::num(fmt::f(m, "submitted")), "Added to MusicBrainz"),
            stat::stat(stat::num(fmt::f(m, "skipped")), "Skipped"),
            stat::stat(stat::num(fmt::f(m, "failed")), "Failed"),
        ]),
    ];
    if let Some(l) = last_line(&m["last"]) {
        isrc.push(l);
    }
    isrc.push(toggle(state, "contribute_isrc", "Add missing ISRCs to MusicBrainz"));
    isrc.push(text_field(
        state,
        "mb_client_id",
        "MusicBrainz application client ID",
        format!("Register an application at musicbrainz.org/account/applications (type: web application) with the callback URL {callback}."),
    ));
    isrc.push(secret_field(state, "mb_client_secret", "MusicBrainz application client secret", ""));
    isrc.push(container(btn_icon(Icon::Check, "Save", Btn::Primary, (!state.busy).then_some(Message::Save("isrc")))).align_x(Alignment::End).width(Length::Fill).into());
    let isrc_card = widgets::card(
        column![super::between(widgets::h2("MusicBrainz ISRCs").into(), mb_buttons), column(isrc).spacing(12)]
            .spacing(12)
            .width(Length::Fill),
    )
    .width(Length::Fill);

    let missing_card = super::card(
        "Albums missing from MusicBrainz",
        vec![
            help("Artist pages list albums found on Deezer and Apple Music that MusicBrainz does not have. Their Add to MusicBrainz link opens the MusicBrainz release editor with the title, artists, date, label, barcode, track list and store links filled in. Check it, then enter the edit under your own MusicBrainz account. Once the album is in MusicBrainz, Encore re-tags your copy on its own.").into(),
            link("musicbrainz.org/account/applications", Message::OpenUrl("https://musicbrainz.org/account/applications".into())),
        ],
    );
    column![intro_card, acoustid_card, isrc_card, missing_card].spacing(16).width(Length::Fill).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_save_body_keeps_saved_secrets() {
        let mut st = State {
            values: json!({ "contribute_acoustid": false, "acoustid_user_key": "•••• (saved)", "contribute_isrc": true, "mb_client_id": "abc", "mb_client_secret": "",
                "_meta": { "mb_client_id": { "source": "env" } } }),
            ..State::default()
        };
        st.toggles.insert("contribute_acoustid", true);
        let b = body(&st, "acoustid");
        assert_eq!(b["contribute_acoustid"], true);
        assert!(b.get("acoustid_user_key").is_none(), "nothing typed: the saved key stays");
        st.edits.insert("acoustid_user_key", " k1 ".into());
        assert_eq!(body(&st, "acoustid")["acoustid_user_key"], "k1");
        st.edits.insert("mb_client_id", "new".into());
        let b = body(&st, "isrc");
        assert_eq!(b["contribute_isrc"], true);
        assert!(b.get("mb_client_id").is_none(), "locked by the environment");
    }
}
