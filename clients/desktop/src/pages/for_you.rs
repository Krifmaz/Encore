//! For you: recommendations from ListenBrainz and Last.fm (songs you have play here, the rest
//! can be requested), artists and albums for you, and the genres to browse. Behaviour
//! reference: web-rs/src/pages/foryou.rs `render`.

use crate::api::ApiError;
use crate::fmt;
use crate::pages::{Ctx, El, Page, Section};
use crate::theme::Btn;
use crate::widgets::catalog::{self, AlbumTile, FyRow};
use crate::widgets::{self, btn, btn_icon, error_box, grid, h1, h2, icon_btn, muted, section_head, skeleton_rows, small_muted, Icon};
use iced::widget::{column, row, space};
use iced::{Alignment, Length, Subscription, Task};
use serde_json::Value;
use std::collections::HashSet;

/// One list of recommended songs (a ListenBrainz playlist, or Last.fm's "Songs for you").
#[derive(Debug)]
struct Playlist {
    id: Value,
    title: String,
    kind: String,
    tracks: Vec<Value>,
}

#[derive(Debug, Default)]
pub struct State {
    /// `GET /api/discover/for-you`: None while loading, Err(message) when it failed.
    data: Option<Result<Value, String>>,
    /// `GET /api/discover/tags`.
    tags: Option<Value>,
    playlists: Vec<Playlist>,
    /// `(playlist, row)` requests being sent.
    sending: HashSet<(usize, usize)>,
    /// The playlist whose "Request N missing" is running.
    busy: Option<usize>,
    more_like: Option<widgets::more_like::State>,
}

#[derive(Clone, Debug)]
pub enum Message {
    Loaded(Result<Value, ApiError>),
    Tags(Result<Value, ApiError>),
    Play(usize, usize),
    PlayOwned(usize),
    Request(usize, usize),
    Requested(usize, usize, Result<Value, ApiError>),
    RequestMissing(usize),
    /// `(playlist, [(row, status)], how many, the limit error)`.
    MissingDone(usize, Vec<(usize, Value)>, usize, Option<String>),
    More(usize, usize),
    MoreLike(widgets::more_like::Message),
    Open(Page),
    OpenUrl(String),
}

/// `KIND[kind]`: the line under a list's title.
pub fn kind_words(k: &str) -> &'static str {
    match k {
        "weekly-exploration" => "New music picked for you every Monday",
        "daily-jams" => "Songs you love, refreshed daily",
        "weekly-jams" => "Your favourites from the past weeks",
        "lastfm" => "Similar to your most played songs on Last.fm",
        _ => "",
    }
}

fn is_missing(t: &Value) -> bool {
    !fmt::truthy(t, "library") && !fmt::truthy(t, "request_status")
}

fn owned(t: &Value) -> bool {
    fmt::truthy(t, "library")
}

/// Called when the app navigates here (with the route's parameters).
pub fn open(state: &mut State, ctx: &mut Ctx) -> Task<Message> {
    state.data = None;
    state.playlists.clear();
    state.sending.clear();
    state.busy = None;
    state.more_like = None;
    let api = ctx.api.clone();
    Task::batch([
        Task::perform(
            {
                let api = api.clone();
                async move { api.get("/discover/for-you").await }
            },
            Message::Loaded,
        ),
        Task::perform(async move { api.get("/discover/tags").await }, Message::Tags),
    ])
}

fn reload(ctx: &Ctx) -> Task<Message> {
    let api = ctx.api.clone();
    Task::perform(async move { api.get("/discover/for-you").await }, Message::Loaded)
}

/// The lists the answer holds: Last.fm's songs first, then the ListenBrainz playlists.
fn playlists_of(d: &Value) -> Vec<Playlist> {
    let mut out: Vec<Playlist> = fmt::items(&d["playlists"])
        .into_iter()
        .map(|p| Playlist {
            id: p["id"].clone(),
            title: fmt::s(&p, "title"),
            kind: fmt::s(&p, "kind"),
            tracks: fmt::items(&p["tracks"]),
        })
        .collect();
    let lf = fmt::items(&d["lastfm"]["tracks"]);
    if !lf.is_empty() {
        out.insert(
            0,
            Playlist {
                id: Value::Null,
                title: "Songs for you".into(),
                kind: "lastfm".into(),
                tracks: lf,
            },
        );
    }
    out
}

pub fn update(state: &mut State, msg: Message, ctx: &mut Ctx) -> Task<Message> {
    match msg {
        Message::Loaded(Ok(d)) => {
            state.playlists = playlists_of(&d);
            state.data = Some(Ok(d));
        }
        Message::Loaded(Err(e)) => state.data = Some(Err(e.message)),
        Message::Tags(r) => state.tags = r.ok(),
        Message::Play(pi, i) => {
            if let Some(p) = state.playlists.get(pi) {
                let playable: Vec<Value> = p.tracks.iter().filter(|t| owned(t)).map(|t| t["library"].clone()).collect();
                let start = p.tracks[..i.min(p.tracks.len())].iter().filter(|t| owned(t)).count();
                if !playable.is_empty() {
                    ctx.play(playable, start);
                }
            }
        }
        Message::PlayOwned(pi) => {
            if let Some(p) = state.playlists.get(pi) {
                let playable: Vec<Value> = p.tracks.iter().filter(|t| owned(t)).map(|t| t["library"].clone()).collect();
                if !playable.is_empty() {
                    ctx.play(playable, 0);
                }
            }
        }
        Message::Request(pi, i) => {
            let Some(t) = state.playlists.get(pi).and_then(|p| p.tracks.get(i)) else {
                return Task::none();
            };
            if state.sending.contains(&(pi, i)) {
                return Task::none();
            }
            state.sending.insert((pi, i));
            let body = catalog::fy_body(t);
            let api = ctx.api.clone();
            return Task::perform(async move { api.post("/requests", body).await }, move |r| Message::Requested(pi, i, r));
        }
        Message::Requested(pi, i, r) => {
            state.sending.remove(&(pi, i));
            match r {
                Ok(res) => {
                    catalog::request_toast(ctx, &res);
                    if let Some(t) = state.playlists.get_mut(pi).and_then(|p| p.tracks.get_mut(i)) {
                        catalog::set_status(t, res["status"].clone(), Value::Null);
                    }
                }
                Err(e) => ctx.toast_err(e.message),
            }
        }
        Message::RequestMissing(pi) => {
            let Some(p) = state.playlists.get(pi) else {
                return Task::none();
            };
            if state.busy.is_some() {
                return Task::none();
            }
            let missing: Vec<(usize, Value)> = p
                .tracks
                .iter()
                .enumerate()
                .filter(|(_, t)| is_missing(t))
                .map(|(i, t)| (i, catalog::fy_body(t)))
                .collect();
            if missing.is_empty() {
                return Task::none();
            }
            state.busy = Some(pi);
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
                move |(done, ok, err)| Message::MissingDone(pi, done, ok, err),
            );
        }
        Message::MissingDone(pi, done, ok, err) => {
            state.busy = None;
            if let Some(p) = state.playlists.get_mut(pi) {
                for (i, status) in done {
                    if let Some(t) = p.tracks.get_mut(i) {
                        catalog::set_status(t, status, Value::Null);
                    }
                }
            }
            if let Some(e) = err {
                ctx.toast_err(e);
            }
            ctx.toast_ok(format!("{ok} song(s) requested"));
            // the web draws the page again after requesting
            return reload(ctx);
        }
        Message::More(pi, i) => {
            if let Some(t) = state.playlists.get(pi).and_then(|p| p.tracks.get(i)) {
                let (st, task) = widgets::more_like::open(&fmt::s(t, "artist"), &fmt::s(t, "title"), &fmt::s(t, "mbid"), ctx);
                state.more_like = Some(st);
                return task.map(Message::MoreLike);
            }
        }
        Message::MoreLike(m) => {
            if let Some(ml) = state.more_like.as_mut() {
                let task = widgets::more_like::update(ml, m, ctx).map(Message::MoreLike);
                if ml.closed {
                    state.more_like = None;
                }
                return task;
            }
        }
        Message::Open(p) => ctx.navigate(p),
        Message::OpenUrl(u) => ctx.open_url(u),
    }
    Task::none()
}

// ----- view -------------------------------------------------------------------------------

fn artist_open(a: &Value) -> Message {
    if fmt::truthy(a, "mbid") {
        Message::Open(Page::Artist(fmt::s(a, "mbid")))
    } else {
        Message::Open(Page::ArtistName(fmt::s(a, "name")))
    }
}

fn playlist_view<'a>(state: &'a State, ctx: &Ctx<'a>, pi: usize, p: &'a Playlist) -> El<'a, Message> {
    let missing = p.tracks.iter().filter(|t| is_missing(t)).count();
    let owned_n = p.tracks.iter().filter(|t| owned(t)).count();
    let mut sub = kind_words(&p.kind).to_string();
    if !p.tracks.is_empty() {
        sub = catalog::join_truthy(&[sub, format!("{owned_n}/{} in library", p.tracks.len())], " · ");
    }
    let mut actions = row![].spacing(8).align_y(Alignment::Center);
    if owned_n > 0 {
        actions = actions.push(btn_icon(Icon::Play, "Play owned", Btn::Default, Some(Message::PlayOwned(pi))));
    }
    if missing > 0 {
        actions = actions.push(btn_icon(
            Icon::Download,
            format!("Request {missing} missing"),
            Btn::Primary,
            (state.busy.is_none()).then_some(Message::RequestMissing(pi)),
        ));
    }
    let id = match &p.id {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    };
    if let Some(id) = id {
        actions = actions.push(icon_btn(
            Icon::External,
            "On ListenBrainz",
            Some(Message::OpenUrl(format!("https://listenbrainz.org/playlist/{id}"))),
        ));
    }
    let head = row![
        column![h2(p.title.as_str()), small_muted(sub)].spacing(2),
        space().width(Length::Fill),
        actions
    ]
    .align_y(Alignment::Center);
    let current = ctx.player.current_id();
    let mut list = column![];
    for (i, t) in p.tracks.iter().enumerate() {
        let is_owned = owned(t);
        let playing = is_owned && current.is_some() && current == t["library"]["id"].as_i64();
        list = list.push(catalog::fy_track_row(
            ctx,
            t,
            playing,
            FyRow {
                play: is_owned.then_some(Message::Play(pi, i)),
                open_artist: if fmt::truthy(t, "artist_mbid") {
                    Message::Open(Page::Artist(fmt::s(t, "artist_mbid")))
                } else {
                    Message::Open(Page::ArtistName(fmt::s(t, "artist")))
                },
                more: Message::More(pi, i),
                request: (!state.sending.contains(&(pi, i)) && state.busy != Some(pi)).then_some(Message::Request(pi, i)),
            },
        ));
    }
    column![head, widgets::card_flat(list)].spacing(12).into()
}

fn body<'a>(state: &'a State, ctx: &Ctx<'a>) -> El<'a, Message> {
    let d = match &state.data {
        None => return skeleton_rows(6),
        Some(Err(e)) => return error_box(e),
        Some(Ok(d)) => d,
    };
    if !fmt::truthy(d, "connected") {
        return widgets::card(widgets::empty::empty_with(
            Icon::Spark,
            "Connect Last.fm or ListenBrainz to get recommendations",
            "Connect either one on the Connections page. Encore scrobbles what you play here to them, and your other players (Navidrome, Finamp, Plexamp…) can scrobble too.",
            Some(btn("Connections", Btn::Default, Some(Message::Open(Page::Settings(Section::Connections)))).into()),
        ))
        .width(Length::Fill)
        .into();
    }
    let lf = &d["lastfm"];
    if state.playlists.is_empty() && !lf.is_object() {
        return widgets::card(catalog::empty_text(
            Icon::Spark,
            "No recommendations yet".into(),
            format!(
                "ListenBrainz builds Weekly Exploration once {} has a few weeks of listens. Check that your players scrobble to ListenBrainz.",
                fmt::s(d, "user")
            ),
        ))
        .width(Length::Fill)
        .into();
    }
    let mut col = column![].spacing(26);
    if let Some(errors) = d["errors"].as_array() {
        for e in errors {
            if let Some(msg) = e.as_str() {
                col = col.push(error_box(msg));
            }
        }
    }
    let lf_artists = fmt::items(&lf["artists"]);
    if !lf_artists.is_empty() {
        let head = column![
            h2("Artists for you"),
            small_muted(format!("Similar to what {} plays most on Last.fm", fmt::s(lf, "user")))
        ]
        .spacing(2);
        let tiles = lf_artists.iter().map(|a| catalog::artist_tile(ctx, a, String::new(), artist_open(a))).collect();
        col = col.push(column![head, grid(tiles)].spacing(14));
    }
    let lf_albums = fmt::items(&lf["albums"]);
    if !lf_albums.is_empty() {
        let head = column![h2("Albums for you"), small_muted("The best-known album of artists you might like")].spacing(2);
        let tiles = lf_albums
            .iter()
            .map(|a| {
                let q = format!("{} {}", fmt::s(a, "artist"), fmt::s(a, "title"));
                AlbumTile {
                    title: fmt::s(a, "title"),
                    sub: catalog::join_truthy(&[fmt::s(a, "artist"), fmt::s(a, "year"), fmt::s(a, "type")], " · "),
                    cover: fmt::s(a, "cover"),
                    corner: fmt::truthy(a, "in_library").then(catalog::in_library),
                    on_open: Message::Open(Page::Search { q }),
                    on_play: None,
                    on_request: None,
                }
                .view(ctx)
            })
            .collect();
        col = col.push(column![head, grid(tiles)].spacing(14));
    }
    for (pi, p) in state.playlists.iter().enumerate() {
        col = col.push(playlist_view(state, ctx, pi, p));
    }
    col.into()
}

/// "Browse by genre": the library's genres (accent) then Last.fm's top tags.
fn genres<'a>(state: &'a State) -> El<'a, Message> {
    let mut chips: Vec<El<'a, Message>> = Vec::new();
    if let Some(t) = &state.tags {
        let lib: Vec<String> = fmt::items(&t["library"]).iter().take(15).map(|g| g.as_str().unwrap_or("").to_string()).collect();
        for g in &lib {
            chips.push(catalog::chip(g.clone(), true, Message::Open(Page::Tag(g.clone()))));
        }
        for g in fmt::items(&t["top"]) {
            let name = g.as_str().unwrap_or("").to_string();
            if name.is_empty() || lib.contains(&name.to_lowercase()) {
                continue;
            }
            chips.push(catalog::chip(name.clone(), false, Message::Open(Page::Tag(name))));
        }
    }
    column![
        section_head("Browse by genre", None),
        iced::widget::row(chips).spacing(8).wrap()
    ]
    .spacing(12)
    .into()
}

pub fn view<'a>(state: &'a State, ctx: &Ctx<'a>) -> El<'a, Message> {
    column![
        column![
            h1("For you"),
            muted("Recommendations from your ListenBrainz and Last.fm listening. Songs you already have play right here; request the rest in one click.").size(14)
        ]
        .spacing(6),
        body(state, ctx),
        genres(state),
    ]
    .spacing(22)
    .width(Length::Fill)
    .into()
}

/// A modal or ⋯ menu to lay over the whole window, when one is open.
pub fn overlay<'a>(state: &'a State, ctx: &Ctx<'a>) -> Option<El<'a, Message>> {
    state
        .more_like
        .as_ref()
        .map(|ml| widgets::more_like::view(ml, ctx).map(Message::MoreLike))
}

pub fn subscription(_state: &State) -> Subscription<Message> {
    Subscription::none()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn lastfm_songs_come_first() {
        let d = json!({
            "playlists": [{ "id": "p1", "title": "Weekly Exploration", "kind": "weekly-exploration", "tracks": [{ "title": "a" }] }],
            "lastfm": { "tracks": [{ "title": "b" }, { "title": "c" }] }
        });
        let p = playlists_of(&d);
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].title, "Songs for you");
        assert_eq!(p[0].kind, "lastfm");
        assert_eq!(p[0].tracks.len(), 2);
        assert_eq!(p[1].id, "p1");
        assert_eq!(kind_words("daily-jams"), "Songs you love, refreshed daily");
        assert_eq!(kind_words("x"), "");
        assert!(is_missing(&json!({ "title": "t" })));
        assert!(!is_missing(&json!({ "library": { "id": 1 } })));
        assert!(!is_missing(&json!({ "request_status": "queued" })));
    }
}
