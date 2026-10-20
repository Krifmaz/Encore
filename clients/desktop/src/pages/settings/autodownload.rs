//! Settings → Auto-download. Behaviour reference: web-rs/src/pages/autodownload.rs: what Encore
//! downloads on its own, how it picks quality, and for whom.

use super::{help, input, select_table, switch};
use crate::api::ApiError;
use crate::fmt;
use crate::pages::{Ctx, El};
use crate::theme::{self, Badge, Btn};
use crate::widgets::{self, badge, btn_icon, stat, t, Icon};
use iced::widget::{column, container, row};
use iced::{Alignment, Length, Task};
use serde_json::{json, Map, Value};
use std::collections::HashMap;

const ALBUM_UPGRADE: [(&str, &str); 3] = [
    ("album_ep", "Download the whole album or EP it’s from (recommended)"),
    ("album", "Download the whole album (studio albums only)"),
    ("off", "Only download the song"),
];

#[derive(Debug, Default)]
pub struct State {
    data: Option<Value>,
    /// The settings as edited (switches as bools, boxes as text).
    settings: Map<String, Value>,
    users: HashMap<String, bool>,
    error: String,
    busy: bool,
}

#[derive(Clone, Debug)]
pub enum Message {
    Loaded(Result<Value, ApiError>),
    Toggle(&'static str, bool),
    Text(&'static str, String),
    Album(String),
    User(String, bool),
    Save,
    Saved(Result<Value, ApiError>),
    RunNow,
    Ran(Result<Value, ApiError>),
    Upgrade,
    Upgraded(Result<Value, ApiError>),
}

pub fn open(state: &mut State, ctx: &mut Ctx) -> Task<Message> {
    state.data = None;
    state.error.clear();
    state.busy = false;
    let api = ctx.api.clone();
    Task::perform(async move { api.get("/admin/autodownload").await }, Message::Loaded)
}

/// `collect()`: `{ settings: { key: checked | value }, users: { id: checked } }`.
pub fn collect(state: &State) -> Value {
    let mut settings = Map::new();
    for (k, v) in &state.settings {
        settings.insert(k.clone(), v.clone());
    }
    let mut users = Map::new();
    for (id, on) in &state.users {
        users.insert(id.clone(), Value::Bool(*on));
    }
    json!({ "settings": settings, "users": users })
}

fn take(state: &mut State, d: &Value) {
    let s = &d["settings"];
    let mut m = Map::new();
    for k in ["auto_request_likes", "discover_catalog_merge", "auto_request_followed", "auto_approve", "quality_first", "upgrade_lossy"] {
        m.insert(k.into(), Value::Bool(fmt::truthy(s, k)));
    }
    for k in ["discover_auto_count", "sync_interval_minutes", "upgrade_lossy_days", "album_upgrade"] {
        m.insert(k.into(), Value::String(super::vs(&s[k])));
    }
    state.settings = m;
    state.users = fmt::items(&d["users"]).iter().map(|u| (fmt::s(u, "id"), fmt::truthy(u, "auto_download"))).collect();
}

fn save(state: &mut State, ctx: &Ctx) -> Task<Message> {
    state.busy = true;
    let api = ctx.api.clone();
    let body = collect(state);
    Task::perform(async move { api.put("/admin/autodownload", body).await }, Message::Saved)
}

pub fn update(state: &mut State, msg: Message, ctx: &mut Ctx) -> Task<Message> {
    match msg {
        Message::Loaded(Ok(d)) => {
            take(state, &d);
            state.data = Some(d);
        }
        Message::Loaded(Err(e)) => state.error = e.message,
        Message::Toggle(k, on) => {
            state.settings.insert(k.into(), Value::Bool(on));
        }
        Message::Text(k, s) => {
            state.settings.insert(k.into(), Value::String(s));
        }
        Message::Album(v) => {
            state.settings.insert("album_upgrade".into(), Value::String(v));
        }
        Message::User(id, on) => {
            state.users.insert(id, on);
        }
        Message::Save => return save(state, ctx),
        Message::Saved(r) => {
            state.busy = false;
            match r {
                Ok(d) => {
                    ctx.toast_ok("Auto-download rules saved");
                    state.data = Some(d);
                }
                Err(e) => ctx.toast_err(e.message),
            }
        }
        Message::RunNow => {
            let api = ctx.api.clone();
            let body = collect(state);
            return Task::perform(
                async move {
                    api.put("/admin/autodownload", body).await?;
                    api.post_empty("/admin/autodownload/run-now").await
                },
                Message::Ran,
            );
        }
        Message::Ran(Ok(_)) => ctx.toast_ok("Checking likes and followed playlists now…"),
        Message::Ran(Err(e)) => ctx.toast_err(e.message),
        Message::Upgrade => {
            let api = ctx.api.clone();
            let body = collect(state);
            return Task::perform(
                async move {
                    api.put("/admin/autodownload", body).await?;
                    api.post_empty("/admin/autodownload/upgrade-now").await
                },
                Message::Upgraded,
            );
        }
        Message::Upgraded(Ok(r)) => ctx.toast_ok(if fmt::truthy(&r, "queued") {
            format!("Looking for lossless copies of {} album(s)", fmt::s(&r, "queued"))
        } else {
            "Nothing to upgrade right now".to_string()
        }),
        Message::Upgraded(Err(e)) => ctx.toast_err(e.message),
    }
    Task::none()
}

/// A switch with a bold label and a help line under it (`sw()` of the web).
fn sw<'a>(state: &'a State, k: &'static str, label: &'a str, hint: &'a str) -> El<'a, Message> {
    let on = state.settings.get(k).and_then(Value::as_bool).unwrap_or(false);
    column![switch(on, label, move |v| Message::Toggle(k, v)), container(help(hint)).padding(widgets::pad(0.0, 0.0, 0.0, 58.0))]
        .spacing(2)
        .into()
}

fn text_of(state: &State, k: &str) -> String {
    state.settings.get(k).map(super::vs).unwrap_or_default()
}

pub fn view<'a>(state: &'a State, _ctx: &Ctx<'a>) -> El<'a, Message> {
    if !state.error.is_empty() {
        return widgets::error_box(&state.error);
    }
    let Some(d) = &state.data else {
        return widgets::skeleton_rows(6);
    };
    let stats = &d["stats"];
    let head = widgets::page_head(
        "Auto-download",
        Some(help("Decide what Encore downloads on its own, how it picks quality, and for whom.").into()),
        vec![
            btn_icon(Icon::Refresh, "Run now", Btn::Default, Some(Message::RunNow)).into(),
            btn_icon(Icon::Check, if state.busy { "Saving…" } else { "Save" }, Btn::Primary, (!state.busy).then_some(Message::Save)).into(),
        ],
    );
    let tiles = widgets::stat::stats(vec![
        stat::stat(stat::num(fmt::f(stats, "auto_active")), "Auto-downloads in progress"),
        stat::stat(stat::num(fmt::f(stats, "album_waiting")), "Songs waiting on their album"),
        stat::stat(stat::num(fmt::f(stats, "lossy_albums")), "Albums only in lossy quality"),
        stat::stat(stat::num(fmt::f(stats, "upgrades_done")), "Upgraded to lossless"),
    ]);
    let what = widgets::card(
        column![
            widgets::h2("What gets downloaded automatically"),
            sw(state, "auto_request_likes", "Liked songs", "A song liked in Finamp, Plexamp, YouTube Music, Spotify, Tidal, Deezer… that isn’t in the library yet gets downloaded."),
            column![
                help("Auto-discover: from each user’s ListenBrainz Weekly Exploration, download the top"),
                row![
                    input("", &text_of(state, "discover_auto_count"), |s| Message::Text("discover_auto_count", s)).width(90),
                    help("new songs every week (0 = off)"),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            ]
            .spacing(4),
            sw(state, "discover_catalog_merge", "Show releases MusicBrainz is missing", "Artist pages also list albums/EPs/singles found on Deezer and Apple Music that aren’t in MusicBrainz, so they can be requested."),
            sw(state, "auto_request_followed", "New tracks in followed playlists", "Playlists set to “Follow source” pull new tracks every sync and download the missing ones."),
            sw(state, "auto_approve", "Skip the approval queue for everyone", "Off: users’ manual requests wait for an admin. Auto-downloads for enabled users are always approved."),
            column![
                help("Check for new likes / playlist tracks every"),
                row![
                    input("", &text_of(state, "sync_interval_minutes"), |s| Message::Text("sync_interval_minutes", s)).width(90),
                    help("minutes (0 = never)"),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            ]
            .spacing(4),
        ]
        .spacing(14)
        .width(Length::Fill),
    )
    .width(Length::FillPortion(1));
    let quality = widgets::card(
        column![
            widgets::h2("Quality"),
            widgets::field("When a song is part of an album", select_table(&ALBUM_UPGRADE, &text_of(state, "album_upgrade"), Message::Album)),
            help("Uses the official studio release from MusicBrainz – never compilations, live albums or soundtracks. If the album doesn’t contain that exact version (e.g. a remix), the song is fetched on its own."),
            sw(state, "quality_first", "Highest quality first", "Try every lossless source (Qobuz/Tidal/Deezer, then FLAC on Soulseek – 24-bit preferred) before torrents/Usenet, MP3s and YouTube Music."),
            sw(state, "upgrade_lossy", "Keep upgrading lossy albums", "Periodically look for lossless copies of albums you only have as MP3/YouTube rips. Better files replace the old ones; playlists follow automatically."),
            column![
                help("Retry each lossy album every"),
                row![
                    input("", &text_of(state, "upgrade_lossy_days"), |s| Message::Text("upgrade_lossy_days", s)).width(90),
                    help("days"),
                    btn_icon(Icon::Refresh, "Look for upgrades now", Btn::Default, Some(Message::Upgrade)),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            ]
            .spacing(4),
            help("Which sources are searched, and in what order: Settings → Downloads → Download pipeline."),
        ]
        .spacing(14)
        .width(Length::Fill),
    )
    .width(Length::FillPortion(1));
    let p = theme::look();
    let th = |s: &'a str| t(s).size(12).color(p.muted).font(theme::semibold());
    let mut users = column![
        container(
            row![
                container(th("User")).width(Length::Fill),
                container(th("Followed playlists")).width(150),
                container(th("Connected accounts")).width(150),
                container(th("Auto-download")).width(110),
            ]
            .spacing(10)
            .align_y(Alignment::Center)
        )
        .padding([8, 14]),
        widgets::divider(),
    ];
    for u in fmt::items(&d["users"]) {
        let id = fmt::s(&u, "id");
        let on = state.users.get(&id).copied().unwrap_or(false);
        let admin = matches!(fmt::s(&u, "role").as_str(), "admin" | "owner");
        let mut name = row![t(fmt::s(&u, "username")).size(14).font(theme::semibold())].spacing(6).align_y(Alignment::Center);
        if admin {
            name = name.push(badge("admin", Badge::Plain));
        }
        let id2 = id.clone();
        users = users.push(column![
            container(
                row![
                    container(name).width(Length::Fill),
                    container(help(fmt::s(&u, "followed"))).width(150),
                    container(help(fmt::s(&u, "connections"))).width(150),
                    container(iced::widget::toggler(on).on_toggle(move |v| Message::User(id2.clone(), v)).style(theme::switch)).width(110),
                ]
                .spacing(10)
                .align_y(Alignment::Center)
            )
            .padding([10, 14]),
            widgets::divider(),
        ]);
    }
    column![
        head,
        tiles,
        row![what, quality].spacing(16).align_y(Alignment::Start),
        widgets::section_head("Users", Some(help("Who auto-downloads are enabled for").into())),
        widgets::card_flat(users).width(Length::Fill),
    ]
    .spacing(16)
    .width(Length::Fill)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collect_sends_switches_as_bools_and_boxes_as_text() {
        let mut st = State::default();
        take(&mut st, &json!({ "settings": { "auto_request_likes": 1, "discover_auto_count": 5, "album_upgrade": "album" }, "users": [{ "id": 3, "auto_download": true }] }));
        let c = collect(&st);
        assert_eq!(c["settings"]["auto_request_likes"], true);
        assert_eq!(c["settings"]["quality_first"], false);
        assert_eq!(c["settings"]["discover_auto_count"], "5");
        assert_eq!(c["settings"]["album_upgrade"], "album");
        assert_eq!(c["users"]["3"], true);
    }
}
