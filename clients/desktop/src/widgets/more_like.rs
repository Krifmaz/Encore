//! The "More like <song>" dialog (web-rs/src/pages/foryou.rs `more_like_this`): similar songs
//! from `GET /api/discover/similar-tracks`; the ones in the library play, the rest can be
//! requested one by one or all at once. A page holds `Option<State>`, routes its messages here
//! and drops the state once `closed` is set.

use super::catalog::{self, FyRow};
use super::{btn_icon, empty, error_box, modal, muted, small_muted, El, Icon};
use crate::api::ApiError;
use crate::fmt;
use crate::pages::{Ctx, Page};
use crate::theme::Btn;
use iced::widget::{column, container, row, space};
use iced::{Alignment, Length, Task};
use serde_json::Value;
use std::collections::HashSet;

#[derive(Debug)]
pub struct State {
    pub title: String,
    artist: String,
    mbid: String,
    /// None while loading; Err(message) when the server failed.
    tracks: Option<Result<Vec<Value>, String>>,
    source: String,
    /// Rows whose request is being sent.
    sending: HashSet<usize>,
    /// "Request N missing" is running.
    busy: bool,
    /// Set when the dialog asked to be closed; the page drops the state.
    pub closed: bool,
}

#[derive(Clone, Debug)]
pub enum Message {
    Loaded(Result<Value, ApiError>),
    Play(usize),
    Request(usize),
    Requested(usize, Result<Value, ApiError>),
    RequestAll,
    /// `(row, status)` per request that went through, how many, and the limit error if any.
    AllDone(Vec<(usize, Value)>, usize, Option<String>),
    /// "More like this" on a row: the dialog searches again from that song.
    More(usize),
    OpenArtist(usize),
    Close,
}

/// Open the dialog for one song (`mbid` may be "").
pub fn open(artist: &str, title: &str, mbid: &str, ctx: &Ctx) -> (State, Task<Message>) {
    let state = State {
        title: title.to_string(),
        artist: artist.to_string(),
        mbid: mbid.to_string(),
        tracks: None,
        source: String::new(),
        sending: HashSet::new(),
        busy: false,
        closed: false,
    };
    let task = load(&state, ctx);
    (state, task)
}

fn load(state: &State, ctx: &Ctx) -> Task<Message> {
    let enc = crate::pages::urlencode;
    let mut url = format!("/discover/similar-tracks?artist={}&title={}", enc(&state.artist), enc(&state.title));
    if !state.mbid.is_empty() {
        url.push_str(&format!("&mbid={}", state.mbid));
    }
    let api = ctx.api.clone();
    Task::perform(async move { api.get(&url).await }, Message::Loaded)
}

fn rows(state: &State) -> &[Value] {
    match &state.tracks {
        Some(Ok(t)) => t,
        _ => &[],
    }
}

/// A row that is neither owned nor requested.
fn is_missing(t: &Value) -> bool {
    !fmt::truthy(t, "library") && !fmt::truthy(t, "request_status")
}

pub fn update(state: &mut State, msg: Message, ctx: &mut Ctx) -> Task<Message> {
    match msg {
        Message::Loaded(Ok(d)) => {
            state.source = fmt::s(&d, "source");
            state.tracks = Some(Ok(fmt::items(&d["tracks"])));
        }
        Message::Loaded(Err(e)) => state.tracks = Some(Err(e.message)),
        Message::Play(i) => {
            let list = rows(state);
            let playable: Vec<Value> = list.iter().filter(|t| fmt::truthy(t, "library")).map(|t| t["library"].clone()).collect();
            let start = list[..i.min(list.len())].iter().filter(|t| fmt::truthy(t, "library")).count();
            if !playable.is_empty() {
                ctx.play(playable, start);
            }
        }
        Message::Request(i) => {
            let Some(t) = rows(state).get(i) else {
                return Task::none();
            };
            let body = catalog::fy_body(t);
            state.sending.insert(i);
            let api = ctx.api.clone();
            return Task::perform(async move { api.post("/requests", body).await }, move |r| Message::Requested(i, r));
        }
        Message::Requested(i, r) => {
            state.sending.remove(&i);
            match r {
                Ok(res) => {
                    catalog::request_toast(ctx, &res);
                    if let Some(Ok(list)) = state.tracks.as_mut() {
                        if let Some(t) = list.get_mut(i) {
                            catalog::set_status(t, res["status"].clone(), Value::Null);
                        }
                    }
                }
                Err(e) => ctx.toast_err(e.message),
            }
        }
        Message::RequestAll => {
            let missing: Vec<(usize, Value)> = rows(state)
                .iter()
                .enumerate()
                .filter(|(_, t)| is_missing(t))
                .map(|(i, t)| (i, catalog::fy_body(t)))
                .collect();
            if missing.is_empty() || state.busy {
                return Task::none();
            }
            state.busy = true;
            let api = ctx.api.clone();
            return Task::perform(
                async move {
                    let mut done = Vec::new();
                    let mut ok = 0;
                    let mut err = None;
                    for (i, body) in missing {
                        match api.post("/requests", body).await {
                            Ok(r) => {
                                done.push((i, r["status"].clone()));
                                ok += 1;
                            }
                            Err(e) => {
                                if e.message.to_ascii_lowercase().contains("limit") {
                                    err = Some(e.message);
                                    break;
                                }
                            }
                        }
                    }
                    (done, ok, err)
                },
                |(done, ok, err)| Message::AllDone(done, ok, err),
            );
        }
        Message::AllDone(done, ok, err) => {
            state.busy = false;
            if let Some(Ok(list)) = state.tracks.as_mut() {
                for (i, status) in done {
                    if let Some(t) = list.get_mut(i) {
                        catalog::set_status(t, status, Value::Null);
                    }
                }
            }
            if let Some(e) = err {
                ctx.toast_err(e);
            }
            ctx.toast_ok(format!("{ok} song(s) requested"));
        }
        Message::More(i) => {
            if let Some(t) = rows(state).get(i).cloned() {
                state.artist = fmt::s(&t, "artist");
                state.title = fmt::s(&t, "title");
                state.mbid = fmt::s(&t, "mbid");
                state.tracks = None;
                state.source.clear();
                return load(state, ctx);
            }
        }
        Message::OpenArtist(i) => {
            if let Some(t) = rows(state).get(i) {
                let page = if fmt::truthy(t, "artist_mbid") {
                    Page::Artist(fmt::s(t, "artist_mbid"))
                } else {
                    Page::ArtistName(fmt::s(t, "artist"))
                };
                state.closed = true;
                ctx.navigate(page);
            }
        }
        Message::Close => state.closed = true,
    }
    Task::none()
}

pub fn view<'a>(state: &'a State, ctx: &Ctx<'a>) -> El<'a, Message> {
    let body: El<'a, Message> = match &state.tracks {
        None => row![muted("Finding similar songs…").size(14)].into(),
        Some(Err(e)) => error_box(e),
        Some(Ok(list)) if list.is_empty() => empty(
            Icon::Spark,
            "Nothing similar found",
            "Add a Last.fm API key in Settings for the best results.",
        ),
        Some(Ok(list)) => {
            let missing = list.iter().filter(|t| is_missing(t)).count();
            let mut head = row![small_muted(format!("from {}", state.source))]
                .spacing(10)
                .align_y(Alignment::Center);
            head = head.push(space().width(Length::Fill));
            if missing > 0 {
                head = head.push(btn_icon(
                    Icon::Download,
                    format!("Request {missing} missing"),
                    Btn::Default,
                    (!state.busy).then_some(Message::RequestAll),
                ));
            }
            let current = ctx.player.current_id();
            let mut rows_col = column![];
            for (i, t) in list.iter().enumerate() {
                let owned = fmt::truthy(t, "library");
                let playing = owned && current.is_some() && current == t["library"]["id"].as_i64();
                rows_col = rows_col.push(catalog::fy_track_row(
                    ctx,
                    t,
                    playing,
                    FyRow {
                        play: owned.then_some(Message::Play(i)),
                        open_artist: Message::OpenArtist(i),
                        more: Message::More(i),
                        request: (!state.sending.contains(&i) && !state.busy).then_some(Message::Request(i)),
                    },
                ));
            }
            column![head, container(rows_col).style(crate::theme::card).padding(0)]
                .spacing(10)
                .into()
        }
    };
    modal(format!("More like {}", state.title), body, None, true, Message::Close)
}
