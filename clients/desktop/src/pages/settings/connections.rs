//! Settings → Connections. Behaviour reference: web-rs/src/pages/connections.rs. One card per
//! service, the connect forms (user/password, token, Deezer profile, Plex PIN, YouTube Music
//! device code / pasted headers, Last.fm), remote playlist import and Sync now. Sign-ins that
//! happen on a web page (Spotify, Tidal, Last.fm, Plex) open the browser.

use super::{help, input, link_muted, patch, switch};
use crate::api::ApiError;
use crate::fmt;
use crate::pages::{Ctx, El, Page};
use crate::theme::{Badge, Btn};
use crate::widgets::text_area::text_area;
use crate::widgets::{self, badge, btn, btn_icon, btn_sm, h3, modal, t, tabs, Icon};
use iced::widget::text_editor;
use iced::widget::{column, container, row};
use iced::{Alignment, Length, Subscription, Task};
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use std::time::Duration;

/// `INFO[k]`: what a service does and which connect form it uses.
pub fn info(k: &str) -> (&'static str, &'static str) {
    match k {
        "jellyfin" => ("Two-way favourites (Finamp ♥) and playlist sync", "userpass"),
        "plex" => ("Two-way loves (Plexamp ♥) and playlist sync", "plex"),
        "navidrome" => ("Two-way stars and playlist sync", "userpass"),
        "ytmusic" => ("Two-way likes (👍) and playlist sync; import your library & Liked Music", "ytmusic"),
        "spotify" => ("Import private playlists & Liked Songs; likes flow into Encore", "oauth"),
        "tidal" => ("Import playlists & collection; likes flow into Encore", "oauth"),
        "deezer" => ("Import your public favourites & playlists", "profile"),
        "listenbrainz" => ("Scrobbles what you play here; two-way loves; playlists pushed (needs MusicBrainz-tagged tracks); For you recommendations", "token"),
        "lastfm" => ("Scrobbles what you play here; imports loved tracks; For you recommendations", "lastfm"),
        _ => ("", ""),
    }
}

/// The fields of the plain connect forms: `(name, label, help, secret)`.
fn form_fields(form: &str) -> Vec<(&'static str, &'static str, &'static str, bool)> {
    match form {
        "userpass" => vec![("username", "Username", "", false), ("password", "Password", "", true)],
        "token" => vec![("token", "User token", "listenbrainz.org → Settings → User token", false)],
        "profile" => vec![("profile", "Deezer profile link or user id", "Your favourites must be public", false)],
        _ => vec![],
    }
}

/// Where a service keeps its liked songs (deezer: none).
fn liked_url(svc: &str) -> Option<&'static str> {
    match svc {
        "spotify" => Some("https://open.spotify.com/collection/tracks"),
        "ytmusic" => Some("https://music.youtube.com/playlist?list=LM"),
        _ => None,
    }
}

#[derive(Debug)]
pub enum Dialog {
    /// Username/password, token or profile.
    Form {
        svc: String,
        form: &'static str,
        values: Vec<String>,
        busy: bool,
    },
    Disconnect {
        svc: String,
        label: String,
    },
    /// The browser is on plex.tv; `/connections/plex/check` is asked every 2 s.
    Plex {
        url: String,
        pin_id: Value,
        tries: u32,
    },
    Lastfm {
        username: String,
    },
    YtMusic {
        oauth: bool,
        tab: String,
        code: Option<Value>,
        headers: text_editor::Content,
        tries: u32,
        busy: bool,
    },
    /// The admin side is not ready (no Public URL / client id).
    Note {
        title: String,
        message: String,
        uri: Option<String>,
    },
    Playlists {
        svc: String,
        label: String,
        rows: Option<Vec<Value>>,
        error: String,
        follow: HashMap<String, bool>,
        imported: HashSet<String>,
        busy: Option<String>,
    },
}

#[derive(Debug, Default)]
pub struct State {
    data: Option<Value>,
    error: String,
    syncing: bool,
    dialog: Option<Dialog>,
}

#[derive(Clone, Debug)]
pub enum Message {
    Loaded(Result<Value, ApiError>),
    SyncNow,
    Synced(Result<Value, ApiError>),
    Connect(String),
    AskDisconnect(String),
    Disconnect,
    Done(Result<Value, ApiError>),
    Flag(String, &'static str, bool),
    Flagged(Result<Value, ApiError>),
    FormInput(usize, String),
    FormSubmit,
    FormDone(Result<Value, ApiError>),
    OauthStarted(String, Result<Value, ApiError>),
    PlexStarted(Result<Value, ApiError>),
    PlexTick,
    PlexChecked(Result<Value, ApiError>),
    LastfmAuth,
    LastfmUser(String),
    LastfmSubmit,
    YtTab(String),
    YtStart,
    YtStarted(Result<Value, ApiError>),
    YtTick,
    YtChecked(Result<Value, ApiError>),
    YtHeaders(text_editor::Action),
    YtSaveHeaders,
    Playlists(String),
    PlaylistsLoaded(Result<Value, ApiError>),
    Follow(String, bool),
    Import(String),
    Imported(String, String, Result<Value, ApiError>),
    ImportLiked(String),
    OpenUrl(String),
    CloseDialog,
}

fn load(ctx: &Ctx) -> Task<Message> {
    let api = ctx.api.clone();
    Task::perform(async move { api.get("/connections").await }, Message::Loaded)
}

pub fn open(state: &mut State, ctx: &mut Ctx) -> Task<Message> {
    state.data = None;
    state.error.clear();
    state.dialog = None;
    state.syncing = false;
    load(ctx)
}

fn label_of(state: &State, svc: &str) -> String {
    let l = state.data.as_ref().map(|d| fmt::s(&d["services"][svc], "label")).unwrap_or_default();
    if l.is_empty() { svc.to_string() } else { l }
}

pub fn update(state: &mut State, msg: Message, ctx: &mut Ctx) -> Task<Message> {
    match msg {
        Message::Loaded(Ok(d)) => state.data = Some(d),
        Message::Loaded(Err(e)) => state.error = e.message,
        Message::SyncNow => {
            state.syncing = true;
            let api = ctx.api.clone();
            return Task::perform(async move { api.post_empty("/sync").await }, Message::Synced);
        }
        Message::Synced(r) => {
            state.syncing = false;
            match r {
                Ok(r) => {
                    let errors = fmt::truthy(&r, "errors");
                    let text = format!(
                        "Synced: +{} / −{} likes, {} changes pushed, {} playlist(s){}",
                        fmt::s(&r, "liked_added"),
                        fmt::s(&r, "liked_removed"),
                        fmt::s(&r, "pushed"),
                        fmt::s(&r, "playlists"),
                        if errors { format!(" · {} error(s)", fmt::s(&r, "errors")) } else { String::new() }
                    );
                    if errors { ctx.toast_err(text) } else { ctx.toast_ok(text) }
                    return load(ctx);
                }
                Err(e) => ctx.toast_err(e.message),
            }
        }
        Message::Connect(svc) => {
            let (_, form) = info(&svc);
            match form {
                "oauth" => {
                    let api = ctx.api.clone();
                    let s = svc.clone();
                    return Task::perform(async move { api.post(&format!("/connections/{s}"), json!({})).await }, move |r| Message::OauthStarted(svc, r));
                }
                "ytmusic" => {
                    let oauth = state.data.as_ref().is_some_and(|d| fmt::truthy(&d["available_oauth"], "ytmusic"));
                    state.dialog = Some(Dialog::YtMusic {
                        oauth,
                        tab: if oauth { "code".into() } else { "headers".into() },
                        code: None,
                        headers: text_editor::Content::new(),
                        tries: 0,
                        busy: false,
                    });
                }
                "lastfm" => state.dialog = Some(Dialog::Lastfm { username: String::new() }),
                "plex" => {
                    let api = ctx.api.clone();
                    return Task::perform(async move { api.post("/connections/plex", json!({})).await }, Message::PlexStarted);
                }
                _ => {
                    let n = form_fields(form).len();
                    state.dialog = Some(Dialog::Form {
                        svc,
                        form,
                        values: vec![String::new(); n],
                        busy: false,
                    });
                }
            }
        }
        Message::AskDisconnect(svc) => {
            let label = label_of(state, &svc);
            state.dialog = Some(Dialog::Disconnect { svc, label });
        }
        Message::Disconnect => {
            if let Some(Dialog::Disconnect { svc, .. }) = state.dialog.take() {
                let api = ctx.api.clone();
                return Task::perform(async move { api.del(&format!("/connections/{svc}")).await }, Message::Done);
            }
        }
        Message::Done(Ok(_)) => return load(ctx),
        Message::Done(Err(e)) => ctx.toast_err(e.message),
        Message::Flag(svc, flag, on) => {
            if let Some(d) = state.data.as_mut() {
                d["connections"][&svc][flag] = json!(on);
            }
            let api = ctx.api.clone();
            let mut body = Map::new();
            body.insert(flag.to_string(), Value::Bool(on));
            return Task::perform(async move { patch(&api, &format!("/connections/{svc}"), Value::Object(body)).await }, Message::Flagged);
        }
        Message::Flagged(Err(e)) => {
            ctx.toast_err(e.message);
            return load(ctx);
        }
        Message::Flagged(Ok(_)) => {}
        Message::FormInput(i, v) => {
            if let Some(Dialog::Form { values, .. }) = state.dialog.as_mut() {
                if let Some(x) = values.get_mut(i) {
                    *x = v;
                }
            }
        }
        Message::FormSubmit => {
            if let Some(Dialog::Form { svc, form, values, busy }) = state.dialog.as_mut() {
                *busy = true;
                let mut body = Map::new();
                for ((name, _, _, _), v) in form_fields(form).iter().zip(values.iter()) {
                    body.insert(name.to_string(), Value::String(v.clone()));
                }
                let api = ctx.api.clone();
                let svc = svc.clone();
                return Task::perform(async move { api.post(&format!("/connections/{svc}"), Value::Object(body)).await }, Message::FormDone);
            }
        }
        Message::FormDone(Ok(r)) => {
            ctx.toast_ok(format!("Connected as {}", fmt::s(&r, "account")));
            state.dialog = None;
            return load(ctx);
        }
        Message::FormDone(Err(e)) => {
            if let Some(Dialog::Form { busy, .. }) = state.dialog.as_mut() {
                *busy = false;
            }
            ctx.toast_err(e.message);
        }
        Message::OauthStarted(svc, Ok(r)) => {
            ctx.open_url(fmt::s(&r, "url"));
            ctx.toast(format!("Sign in to {} in the browser, then come back and press Sync now", label_of(state, &svc)));
        }
        Message::OauthStarted(svc, Err(e)) => {
            if !(e.message.contains("Public URL") || e.message.contains("Client ID")) {
                ctx.toast_err(e.message);
                return Task::none();
            }
            let base = state
                .data
                .as_ref()
                .map(|d| fmt::s(d, "base_url"))
                .filter(|b| !b.is_empty())
                .unwrap_or_else(|| ctx.base());
            let base = base.trim_end_matches('/').to_string();
            state.dialog = Some(Dialog::Note {
                title: format!("Connect {}", label_of(state, &svc)),
                message: e.message,
                uri: ctx.is_admin().then(|| format!("Redirect URI to register at the service: {base}/api/connections/{svc}/callback")),
            });
        }
        Message::PlexStarted(Ok(r)) => {
            let url = fmt::s(&r, "url");
            ctx.open_url(url.clone());
            state.dialog = Some(Dialog::Plex {
                url,
                pin_id: r["pin_id"].clone(),
                tries: 0,
            });
        }
        Message::PlexStarted(Err(e)) => ctx.toast_err(e.message),
        Message::PlexTick => {
            if let Some(Dialog::Plex { pin_id, tries, .. }) = state.dialog.as_mut() {
                *tries += 1;
                if *tries > 120 {
                    state.dialog = None;
                    ctx.toast_err("Plex sign-in timed out");
                    return Task::none();
                }
                let api = ctx.api.clone();
                let body = json!({ "pin_id": pin_id });
                return Task::perform(async move { api.post("/connections/plex/check", body).await }, Message::PlexChecked);
            }
        }
        Message::PlexChecked(Ok(c)) => {
            if fmt::truthy(&c, "done") {
                state.dialog = None;
                ctx.toast_ok(format!("Plex connected ({})", fmt::s(&c, "account")));
                return load(ctx);
            }
        }
        Message::PlexChecked(Err(e)) => {
            state.dialog = None;
            ctx.toast_err(e.message);
        }
        Message::LastfmAuth => {
            let api = ctx.api.clone();
            return Task::perform(async move { api.post("/connections/lastfm", json!({})).await }, |r| Message::OauthStarted("lastfm".into(), r));
        }
        Message::LastfmUser(s) => {
            if let Some(Dialog::Lastfm { username }) = state.dialog.as_mut() {
                *username = s;
            }
        }
        Message::LastfmSubmit => {
            if let Some(Dialog::Lastfm { username }) = &state.dialog {
                let api = ctx.api.clone();
                let body = json!({ "username": username.trim() });
                return Task::perform(async move { api.post("/connections/lastfm", body).await }, Message::FormDone);
            }
        }
        Message::YtTab(tab) => {
            if let Some(Dialog::YtMusic { tab: t, .. }) = state.dialog.as_mut() {
                *t = tab;
            }
        }
        Message::YtStart => {
            if let Some(Dialog::YtMusic { busy, .. }) = state.dialog.as_mut() {
                *busy = true;
            }
            let api = ctx.api.clone();
            return Task::perform(async move { api.post("/connections/ytmusic", json!({})).await }, Message::YtStarted);
        }
        Message::YtStarted(Ok(c)) => {
            if let Some(Dialog::YtMusic { code, tries, .. }) = state.dialog.as_mut() {
                ctx.open_url(fmt::s(&c, "url"));
                *code = Some(c);
                *tries = 0;
            }
        }
        Message::YtStarted(Err(e)) => {
            if let Some(Dialog::YtMusic { busy, .. }) = state.dialog.as_mut() {
                *busy = false;
            }
            ctx.toast_err(e.message);
        }
        Message::YtTick => {
            if let Some(Dialog::YtMusic { code: Some(c), tries, .. }) = state.dialog.as_mut() {
                *tries += 1;
                if *tries > 180 {
                    return Task::none();
                }
                let api = ctx.api.clone();
                let body = json!({ "device_code": c["device_code"] });
                return Task::perform(async move { api.post("/connections/ytmusic/check", body).await }, Message::YtChecked);
            }
        }
        Message::YtChecked(Ok(r)) => {
            if fmt::truthy(&r, "done") {
                state.dialog = None;
                ctx.toast_ok(format!("YouTube Music connected ({})", fmt::s(&r, "account")));
                return load(ctx);
            }
        }
        Message::YtChecked(Err(_)) => {}
        Message::YtHeaders(a) => {
            if let Some(Dialog::YtMusic { headers, .. }) = state.dialog.as_mut() {
                headers.perform(a);
            }
        }
        Message::YtSaveHeaders => {
            if let Some(Dialog::YtMusic { headers, busy, .. }) = state.dialog.as_mut() {
                *busy = true;
                let api = ctx.api.clone();
                let body = json!({ "headers": headers.text() });
                return Task::perform(async move { api.post("/connections/ytmusic", body).await }, |r| {
                    Message::FormDone(r.map(|v| json!({ "account": format!("YouTube Music ({})", fmt::s(&v, "account")) })))
                });
            }
        }
        Message::Playlists(svc) => {
            let label = label_of(state, &svc);
            state.dialog = Some(Dialog::Playlists {
                svc: svc.clone(),
                label,
                rows: None,
                error: String::new(),
                follow: HashMap::new(),
                imported: HashSet::new(),
                busy: None,
            });
            let api = ctx.api.clone();
            return Task::perform(async move { api.get(&format!("/connections/{svc}/playlists")).await }, Message::PlaylistsLoaded);
        }
        Message::PlaylistsLoaded(r) => {
            if let Some(Dialog::Playlists { rows, error, .. }) = state.dialog.as_mut() {
                match r {
                    Ok(v) => *rows = Some(fmt::items(&v)),
                    Err(e) => {
                        *error = e.message;
                        *rows = Some(Vec::new());
                    }
                }
            }
        }
        Message::Follow(rid, on) => {
            if let Some(Dialog::Playlists { follow, .. }) = state.dialog.as_mut() {
                follow.insert(rid, on);
            }
        }
        Message::Import(rid) => {
            if let Some(Dialog::Playlists { svc, rows, follow, busy, .. }) = state.dialog.as_mut() {
                let Some(p) = rows.as_ref().and_then(|r| r.iter().find(|p| fmt::s(p, "id") == rid)).cloned() else { return Task::none() };
                *busy = Some(rid.clone());
                let name = fmt::s(&p, "name");
                let url = fmt::s(&p, "url");
                let body = json!({
                    "name": name,
                    "url": if url.is_empty() { Value::Null } else { Value::String(url) },
                    "follow": follow.get(&rid).copied().unwrap_or(true),
                });
                let api = ctx.api.clone();
                let path = format!("/connections/{svc}/playlists/{}/import", crate::pages::urlencode(&rid));
                return Task::perform(async move { api.post(&path, body).await }, move |r| Message::Imported(rid.clone(), name, r));
            }
        }
        Message::Imported(rid, name, r) => {
            if let Some(Dialog::Playlists { imported, busy, .. }) = state.dialog.as_mut() {
                *busy = None;
                match r {
                    Ok(r) => {
                        let dup = fmt::truthy(&r, "duplicates_skipped");
                        ctx.toast_ok(format!(
                            "Imported {name}: {} tracks{}",
                            fmt::s(&r, "added"),
                            if dup { format!(", {} duplicates skipped", fmt::s(&r, "duplicates_skipped")) } else { String::new() }
                        ));
                        imported.insert(rid);
                    }
                    Err(e) => ctx.toast_err(e.message),
                }
            }
        }
        Message::ImportLiked(url) => {
            state.dialog = None;
            ctx.copy(url);
            ctx.toast_ok("Link copied – paste it into Import on the Playlists page");
            ctx.navigate(Page::Playlists);
        }
        Message::OpenUrl(u) => ctx.open_url(u),
        Message::CloseDialog => state.dialog = None,
    }
    Task::none()
}

/// One service card (`c` is the connection or Null).
fn service_card<'a>(k: &'a str, s: &'a Value, c: &'a Value, available: bool) -> El<'a, Message> {
    let (what, _) = info(k);
    let connected = super::truthy(c);
    let mut head = row![h3(fmt::s(s, "label"))].spacing(8).align_y(Alignment::Center).width(Length::Fill);
    if connected {
        head = head.push(iced::widget::space().width(Length::Fill));
        head = head.push(if fmt::truthy(c, "last_error") { badge("error", Badge::Err) } else { badge("connected", Badge::Ok) });
    }
    let mut body = column![head, help(what)].spacing(10).width(Length::Fill);
    if connected {
        let synced = if fmt::truthy(c, "last_sync") { format!(" · synced {}", fmt::ago(fmt::f(c, "last_sync"))) } else { String::new() };
        body = body.push(
            row![t("Signed in as").size(13), t(fmt::s(c, "account")).size(13).font(crate::theme::bold()), t(synced).size(13)]
                .spacing(4)
                .align_y(Alignment::Center),
        );
        if fmt::truthy(c, "last_error") {
            body = body.push(super::err_text(fmt::s(c, "last_error")));
        }
        let svc = k.to_string();
        if fmt::truthy(c, "can_scrobble") {
            let s2 = svc.clone();
            body = body.push(switch(fmt::truthy(c, "scrobble"), "Scrobble what I play", move |on| Message::Flag(s2.clone(), "scrobble", on)));
        }
        let s2 = svc.clone();
        body = body.push(switch(fmt::truthy(c, "sync_likes"), "Sync likes", move |on| Message::Flag(s2.clone(), "sync_likes", on)));
        if fmt::s(s, "kind") != "stream" {
            let s2 = svc.clone();
            body = body.push(switch(fmt::truthy(c, "sync_playlists"), "Sync playlists", move |on| Message::Flag(s2.clone(), "sync_playlists", on)));
        }
        if k == "lastfm" && !fmt::truthy(c, "can_scrobble") && c.get("can_scrobble").is_some() {
            body = body.push(
                row![help("Connected by username only –"), link_muted("Sign in with Last.fm", Message::Connect(svc.clone())), help("to scrobble.")]
                    .spacing(4)
                    .align_y(Alignment::Center),
            );
        }
        let mut actions = row![].spacing(8).align_y(Alignment::Center);
        if k != "lastfm" {
            actions = actions.push(btn_icon(Icon::Download, "Import playlists", Btn::Default, Some(Message::Playlists(svc.clone()))));
        }
        actions = actions.push(btn_sm("Disconnect", Btn::Danger, Some(Message::AskDisconnect(svc))));
        body = body.push(actions.wrap());
    } else if available {
        body = body.push(container(btn_icon(Icon::Link, "Connect", Btn::Default, Some(Message::Connect(k.to_string())))));
    } else {
        body = body.push(widgets::faint("Not set up yet – an admin needs to add it in Settings.").size(12.5));
    }
    widgets::card(body).width(320).into()
}

pub fn view<'a>(state: &'a State, _ctx: &Ctx<'a>) -> El<'a, Message> {
    if !state.error.is_empty() {
        return widgets::error_box(&state.error);
    }
    let Some(d) = &state.data else {
        return widgets::skeleton_tiles(3, 320.0);
    };
    let sync = &d["sync"];
    let last = if fmt::truthy(sync, "last") { format!(" Last sync {}.", fmt::ago(fmt::f(sync, "last"))) } else { String::new() };
    let head = widgets::page_head(
        "Connections",
        Some(help(format!("Link your accounts so likes and playlists stay the same in Finamp, Plexamp, Navidrome and here.{last}")).into()),
        vec![btn_icon(Icon::Refresh, if state.syncing { "Syncing…" } else { "Sync now" }, Btn::Primary, (!state.syncing).then_some(Message::SyncNow)).into()],
    );
    let mut grid = row![].spacing(14);
    if let Some(services) = d["services"].as_object() {
        for (k, s) in services {
            grid = grid.push(service_card(k, s, &d["connections"][k], fmt::truthy(&d["available"], k)));
        }
    }
    column![head, grid.wrap()].spacing(18).width(Length::Fill).into()
}

pub fn overlay<'a>(state: &'a State, ctx: &Ctx<'a>) -> Option<El<'a, Message>> {
    let p = ctx.look;
    let dialog = state.dialog.as_ref()?;
    Some(match dialog {
        Dialog::Form { form, values, busy, .. } => {
            let mut c = column![].spacing(12);
            for (i, ((_, label, hint, secret), v)) in form_fields(form).iter().zip(values.iter()).enumerate() {
                let mut f = column![widgets::field(*label, input("", v, move |s| Message::FormInput(i, s)).secure(*secret).on_submit(Message::FormSubmit).width(Length::Fill))].spacing(4);
                if !hint.is_empty() {
                    f = f.push(help(*hint));
                }
                c = c.push(f);
            }
            modal(
                "Connect",
                c,
                Some(
                    row![
                        btn("Cancel", Btn::Default, Some(Message::CloseDialog)),
                        btn(if *busy { "Connecting…" } else { "Connect" }, Btn::Primary, (!busy).then_some(Message::FormSubmit)),
                    ]
                    .spacing(10)
                    .into(),
                ),
                false,
                Message::CloseDialog,
            )
        }
        Dialog::Disconnect { label, .. } => {
            let text = format!("Disconnect {label}?");
            modal(
                "Disconnect",
                widgets::muted(text).size(14),
                Some(
                    row![
                        btn("Cancel", Btn::Default, Some(Message::CloseDialog)),
                        btn("Disconnect", Btn::Danger, Some(Message::Disconnect)),
                    ]
                    .spacing(10)
                    .into(),
                ),
                false,
                Message::CloseDialog,
            )
        }
        Dialog::Plex { url, .. } => modal(
            "Sign in to Plex",
            column![
                btn_icon(Icon::Link, "Open Plex sign-in", Btn::Primary, Some(Message::OpenUrl(url.clone()))),
                help("Approve Encore in the Plex page that opens, then come back here. Waiting…"),
            ]
            .spacing(12),
            None,
            false,
            Message::CloseDialog,
        ),
        Dialog::Lastfm { username } => modal(
            "Connect Last.fm",
            column![
                widgets::muted("Sign in on last.fm and allow Encore – then everything you play here is scrobbled, and For you suggests artists from your listening.").size(14),
                help("Only approve Encore on Last.fm when you started the sign-in yourself."),
                container(btn_icon(Icon::Link, "Sign in with Last.fm", Btn::Primary, Some(Message::LastfmAuth))),
                help("Or just enter a username (loved tracks and recommendations, no scrobbling)"),
                row![
                    input("Last.fm username", username, Message::LastfmUser).on_submit(Message::LastfmSubmit).width(Length::Fill),
                    btn("Connect", Btn::Default, Some(Message::LastfmSubmit)),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            ]
            .spacing(12),
            None,
            false,
            Message::CloseDialog,
        ),
        Dialog::YtMusic { oauth, tab, code, headers, busy, .. } => {
            let mut c = column![].spacing(12);
            if *oauth {
                c = c.push(tabs(&[("code", "Sign in with a code"), ("headers", "Paste browser headers")], tab, Message::YtTab));
            }
            if *oauth && tab == "code" {
                c = c.push(widgets::muted("You'll get a short code to enter at google.com/device with the Google account you use for YouTube Music.").size(14));
                match code {
                    None => c = c.push(container(btn_icon(Icon::Link, if *busy { "Asking for a code…" } else { "Get code" }, Btn::Primary, (!busy).then_some(Message::YtStart)))),
                    Some(cd) => {
                        c = c.push(
                            row![
                                t("Go to").size(14),
                                btn_sm(fmt::s(cd, "url"), Btn::Default, Some(Message::OpenUrl(fmt::s(cd, "url")))),
                                t("and enter").size(14),
                                t(fmt::s(cd, "user_code")).size(22).font(crate::theme::bold()),
                            ]
                            .spacing(8)
                            .align_y(Alignment::Center)
                            .wrap(),
                        );
                        c = c.push(help("Waiting for you to approve…"));
                    }
                }
            } else {
                if !*oauth {
                    c = c.push(widgets::muted("Signing in with a code needs a Google OAuth client, which no admin has added in Settings → Integrations yet. Paste the headers of a signed-in music.youtube.com request instead – it works the same.").size(14));
                }
                c = c.push(help("1. Open music.youtube.com in a desktop browser while signed in, press F12 → Network."));
                c = c.push(help("2. Click around, find a browse request (POST), right-click → Copy → Copy request headers."));
                c = c.push(help("3. Paste below."));
                c = c.push(text_area(headers, "accept: */*\ncookie: …", 8, Message::YtHeaders));
                c = c.push(container(btn(if *busy { "Connecting…" } else { "Connect" }, Btn::Primary, (!busy).then_some(Message::YtSaveHeaders))));
            }
            modal("Connect YouTube Music", c, None, true, Message::CloseDialog)
        }
        Dialog::Note { title, message, uri } => {
            let mut c = column![widgets::muted(message.as_str()).size(14)].spacing(10);
            if let Some(u) = uri {
                c = c.push(t(u.as_str()).size(13).font(crate::theme::mono()));
            }
            modal(title.as_str(), c, Some(btn("Close", Btn::Default, Some(Message::CloseDialog)).into()), false, Message::CloseDialog)
        }
        Dialog::Playlists { svc, label, rows, error, follow, imported, busy } => {
            let title = format!("Playlists on {label}");
            let body: El<'a, Message> = match rows {
                None => widgets::skeleton_rows(4),
                Some(_) if !error.is_empty() => widgets::error_box(error),
                Some(list) => {
                    let mut c = column![].spacing(10);
                    if let Some(url) = liked_url(svc) {
                        c = c.push(container(btn("♥ Import Liked Songs", Btn::Default, Some(Message::ImportLiked(url.to_string())))));
                    }
                    if list.is_empty() {
                        c = c.push(widgets::muted("No playlists found").size(14));
                    }
                    let mut rows_col = column![].spacing(0);
                    for pl in list {
                        let rid = fmt::s(pl, "id");
                        let name = fmt::s(pl, "name");
                        let done = imported.contains(&rid);
                        let on = follow.get(&rid).copied().unwrap_or(true);
                        let r2 = rid.clone();
                        let right: Vec<El<'a, Message>> = vec![
                            switch(on, "Follow", move |v| Message::Follow(r2.clone(), v)),
                            if done {
                                badge("imported", Badge::Ok)
                            } else {
                                btn_icon(
                                    Icon::Download,
                                    if busy.as_deref() == Some(rid.as_str()) { "Importing…" } else { "Import" },
                                    Btn::Primary,
                                    busy.is_none().then(|| Message::Import(rid.clone())),
                                )
                                .into()
                            },
                        ];
                        rows_col = rows_col.push(widgets::list_row(None, name, "", right, None::<Message>));
                    }
                    c = c.push(widgets::card_flat(rows_col).width(Length::Fill));
                    c.into()
                }
            };
            let _ = p;
            modal(title, body, None, true, Message::CloseDialog)
        }
    })
}

pub fn subscription(state: &State) -> Subscription<Message> {
    match &state.dialog {
        Some(Dialog::Plex { .. }) => iced::time::every(Duration::from_secs(2)).map(|_| Message::PlexTick),
        Some(Dialog::YtMusic { code: Some(c), .. }) => {
            let secs = fmt::f(c, "interval");
            let secs = if secs <= 0.0 { 5.0 } else { secs };
            iced::time::every(Duration::from_secs_f64(secs)).map(|_| Message::YtTick)
        }
        _ => Subscription::none(),
    }
}

/// The options a select of services would list (the tests check the words).
#[cfg(test)]
fn service_options(d: &Value) -> Vec<super::Opt> {
    d["services"]
        .as_object()
        .map(|m| m.iter().map(|(k, s)| super::Opt::new(k.clone(), fmt::s(s, "label"))).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forms_and_words_match_the_web() {
        assert_eq!(info("jellyfin").1, "userpass");
        assert_eq!(info("spotify").1, "oauth");
        assert_eq!(info("nothing"), ("", ""));
        assert_eq!(form_fields("userpass").len(), 2);
        assert_eq!(form_fields("token")[0].0, "token");
        assert_eq!(liked_url("spotify"), Some("https://open.spotify.com/collection/tracks"));
        assert_eq!(liked_url("deezer"), None);
        let d = json!({ "services": { "plex": { "label": "Plex" } } });
        assert_eq!(service_options(&d)[0].label, "Plex");
    }
}
