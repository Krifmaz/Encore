//! Album: a catalogue release group from MusicBrainz with the best release's track list; the
//! songs you own play right here, the rest can be requested one by one or as the album; Pick
//! download, Open in library, More like this. Behaviour reference: web-rs/src/pages/discover.rs
//! `render_album`.

use crate::api::ApiError;
use crate::fmt;
use crate::pages::{Ctx, El, Page};
use crate::theme::{self, Badge, Btn};
use crate::widgets::catalog::{self, Live};
use crate::widgets::{self, badge, btn_icon, empty, error_box, h1, icon_btn, muted, picker, small_muted, t, Icon};
use iced::widget::{column, container, row, space};
use iced::{Alignment, Length, Subscription, Task};
use serde_json::{json, Map, Value};
use std::collections::HashSet;

#[derive(Debug, Default)]
pub struct State {
    pub id: String,
    /// `GET /api/discover/album/{id}`: None while loading, Err(message) when it failed.
    a: Option<Result<Value, String>>,
    /// Request keys being sent.
    sending: HashSet<String>,
    live: Option<(String, String)>,
    more_like: Option<widgets::more_like::State>,
    /// The download picker, while open.
    picker: Option<picker::State>,
}

#[derive(Clone, Debug)]
pub enum Message {
    Loaded(Result<Value, ApiError>),
    Open(Page),
    OpenUrl(String),
    /// Play the owned songs from the n-th owned one (shuffled when true).
    PlayOwned(usize, bool),
    /// Play from the row with this recording id.
    PlayRow(String),
    Request(String, Box<Value>),
    Requested(String, Result<Value, ApiError>),
    /// "Pick download" for the album (None) or a song row.
    Pick(Option<usize>),
    Picker(picker::Message),
    /// More like this on a row.
    More(usize),
    MoreLike(widgets::more_like::Message),
    Live(Live),
}

/// Called when the app navigates here (with the route's parameters).
pub fn open(state: &mut State, id: String, ctx: &mut Ctx) -> Task<Message> {
    state.id = id;
    state.a = None;
    state.sending.clear();
    state.more_like = None;
    state.picker = None;
    state.live = catalog::live_key(ctx);
    load(state, ctx)
}

fn load(state: &State, ctx: &Ctx) -> Task<Message> {
    let api = ctx.api.clone();
    let url = format!("/discover/album/{}", state.id);
    Task::perform(async move { api.get(&url).await }, Message::Loaded)
}

fn album(state: &State) -> Option<&Value> {
    match &state.a {
        Some(Ok(a)) => Some(a),
        _ => None,
    }
}

/// The best release's tracks (empty when MusicBrainz has no track list).
fn tracks(a: &Value) -> Vec<Value> {
    if a["release"].is_object() {
        fmt::items(&a["release"]["tracks"])
    } else {
        Vec::new()
    }
}

fn owned_rows(tracks: &[Value]) -> Vec<Value> {
    tracks.iter().filter(|t| fmt::truthy(t, "library")).map(|t| t["library"].clone()).collect()
}

/// The request body of one song of this album (`album_mode: song`).
fn track_body(a: &Value, tr: &Value) -> Value {
    let mut j = Map::new();
    j.insert("kind".into(), json!("track"));
    catalog::copy(&mut j, "mbid", tr, "recording_id");
    catalog::copy(&mut j, "artist", tr, "artist");
    catalog::copy(&mut j, "title", tr, "title");
    catalog::copy(&mut j, "album", a, "title");
    catalog::copy(&mut j, "cover_url", a, "cover");
    catalog::copy(&mut j, "year", a, "year");
    catalog::copy(&mut j, "length", tr, "length");
    if fmt::truthy(tr, "isrc") {
        catalog::copy(&mut j, "isrc", tr, "isrc");
    }
    j.insert("album_mode".into(), json!("song"));
    Value::Object(j)
}

/// The album's own request body (`kind: album`, no `ext`).
fn album_body(a: &Value) -> Value {
    let mut j = Map::new();
    j.insert("kind".into(), json!("album"));
    catalog::copy(&mut j, "mbid", a, "id");
    catalog::copy(&mut j, "artist", a, "artist");
    catalog::copy(&mut j, "title", a, "title");
    catalog::copy(&mut j, "album", a, "title");
    catalog::copy(&mut j, "year", a, "year");
    catalog::copy(&mut j, "cover_url", a, "cover");
    Value::Object(j)
}

/// A request's outcome or a live event, applied to the album and its rows.
fn patch(state: &mut State, ev: &Live) {
    let Some(Ok(a)) = state.a.as_mut() else {
        return;
    };
    let key = format!("rg:{}", fmt::s(a, "id"));
    if let Live::Request { keys, status, label } = ev {
        if keys.contains(&key) {
            catalog::set_status(a, json!(status), json!(label));
        }
    }
    if let Some(ts) = a.get_mut("release").and_then(|r| r.get_mut("tracks")).and_then(Value::as_array_mut) {
        catalog::apply_live(ts, catalog::song_key, ev);
    }
}

pub fn update(state: &mut State, msg: Message, ctx: &mut Ctx) -> Task<Message> {
    match msg {
        Message::Loaded(r) => state.a = Some(r.map_err(|e| e.message)),
        Message::Open(p) => ctx.navigate(p),
        Message::OpenUrl(u) => ctx.open_url(u),
        Message::PlayOwned(start, shuffle) => {
            let queue = album(state).map(|a| owned_rows(&tracks(a))).unwrap_or_default();
            if queue.is_empty() {
                ctx.toast_err("None of these songs are in your library yet");
            } else if shuffle {
                ctx.play(catalog::shuffle(queue), 0);
            } else {
                let at = start.min(queue.len() - 1);
                ctx.play(queue, at);
            }
        }
        Message::PlayRow(rid) => {
            let Some(a) = album(state) else {
                return Task::none();
            };
            let ts = tracks(a);
            let owned: Vec<&Value> = ts.iter().filter(|t| fmt::truthy(t, "library")).collect();
            let pos = owned.iter().position(|t| fmt::s(t, "recording_id") == rid).unwrap_or(0);
            let queue: Vec<Value> = owned.iter().map(|t| t["library"].clone()).collect();
            if queue.is_empty() {
                ctx.toast_err("None of these songs are in your library yet");
            } else {
                ctx.play(queue, pos);
            }
        }
        Message::Request(key, body) => {
            if state.sending.contains(&key) {
                return Task::none();
            }
            state.sending.insert(key.clone());
            let api = ctx.api.clone();
            return Task::perform(async move { api.post("/requests", *body).await }, move |r| Message::Requested(key.clone(), r));
        }
        Message::Requested(key, r) => {
            state.sending.remove(&key);
            match r {
                Ok(res) => {
                    catalog::request_toast(ctx, &res);
                    patch(
                        state,
                        &Live::Request {
                            keys: vec![key],
                            status: fmt::s(&res, "status"),
                            label: String::new(),
                        },
                    );
                }
                Err(e) => ctx.toast_err(e.message),
            }
        }
        Message::Pick(row) => {
            let Some(a) = album(state) else {
                return Task::none();
            };
            let opts = match row {
                None => {
                    let n = if a["release"].is_object() { json!(tracks(a).len()) } else { Value::Null };
                    picker::opts(vec![
                        ("kind", json!("album")),
                        ("mbid", a["id"].clone()),
                        ("artist", a["artist"].clone()),
                        ("artist_mbid", a["artist_id"].clone()),
                        ("title", a["title"].clone()),
                        ("album", a["title"].clone()),
                        ("year", a["year"].clone()),
                        ("cover_url", a["cover"].clone()),
                        ("track_count", n),
                    ])
                }
                Some(i) => {
                    let Some(t) = tracks(a).get(i).cloned() else {
                        return Task::none();
                    };
                    picker::opts(vec![
                        ("kind", json!("track")),
                        ("mbid", t["recording_id"].clone()),
                        ("artist", t["artist"].clone()),
                        ("title", t["title"].clone()),
                        ("album", a["title"].clone()),
                        ("year", a["year"].clone()),
                        ("cover_url", a["cover"].clone()),
                        ("length", t["length"].clone()),
                    ])
                }
            };
            let (st, task) = picker::open(opts, ctx);
            state.picker = Some(st);
            return task.map(Message::Picker);
        }
        Message::Picker(m) => {
            let Some(p) = state.picker.as_mut() else {
                return Task::none();
            };
            let mut tasks = vec![picker::update(p, m, ctx).map(Message::Picker)];
            // onDone: reload the page
            if p.picked.take().is_some() {
                tasks.push(load(state, ctx));
            }
            if state.picker.as_ref().is_some_and(|p| p.closed) {
                state.picker = None;
            }
            return Task::batch(tasks);
        }
        Message::More(i) => {
            if let Some(tr) = album(state).and_then(|a| tracks(a).get(i).cloned()) {
                let (st, task) = widgets::more_like::open(&fmt::s(&tr, "artist"), &fmt::s(&tr, "title"), &fmt::s(&tr, "recording_id"), ctx);
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
        Message::Live(Live::Resync) => return load(state, ctx),
        Message::Live(ev) => patch(state, &ev),
    }
    Task::none()
}

// ----- view -------------------------------------------------------------------------------

fn head<'a>(state: &'a State, ctx: &Ctx<'a>, a: &'a Value, ts: &[Value]) -> El<'a, Message> {
    let has_release = a["release"].is_object();
    let total: f64 = ts.iter().map(|t| fmt::f(t, "length")).sum();
    let secondary: Vec<String> = fmt::items(&a["secondary"]).iter().filter_map(|s| s.as_str().map(str::to_string)).collect();
    let kicker = catalog::join_truthy(
        &[
            if fmt::truthy(a, "type") { fmt::s(a, "type") } else { "Release".into() },
            secondary.join(", "),
        ],
        " · ",
    );
    let mut meta = row![catalog::link(fmt::s(a, "artist"), 14.0, Message::Open(catalog::artist_page(a)))]
        .spacing(6)
        .align_y(Alignment::Center);
    let mut rest: Vec<String> = Vec::new();
    if fmt::truthy(a, "year") {
        rest.push(fmt::s(a, "year"));
    }
    if has_release {
        rest.push(format!("{} tracks", ts.len()));
        rest.push(fmt::duration_long(total));
        if fmt::truthy(&a["release"], "label") {
            rest.push(fmt::s(&a["release"], "label"));
        }
    }
    if !rest.is_empty() {
        meta = meta.push(muted(format!("· {}", rest.join(" · "))).size(14));
    }
    let mut text = column![catalog::kicker(kicker), h1(fmt::s(a, "title")), meta].spacing(6);
    let tags: Vec<El<'a, Message>> = fmt::items(&a["tags"])
        .iter()
        .filter_map(|s| s.as_str().map(str::to_string))
        .map(|tag| badge(tag, Badge::Plain))
        .collect();
    if !tags.is_empty() {
        text = text.push(iced::widget::row(tags).spacing(6).wrap());
    }
    let owned_n = ts.iter().filter(|t| fmt::truthy(t, "library")).count();
    let mut actions = row![].spacing(8).align_y(Alignment::Center);
    if owned_n > 0 {
        actions = actions.push(btn_icon(Icon::Play, "Play", Btn::Primary, Some(Message::PlayOwned(0, false))));
        actions = actions.push(btn_icon(Icon::Shuffle, "Shuffle", Btn::Default, Some(Message::PlayOwned(0, true))));
    }
    if fmt::truthy(a, "library_album_id") {
        actions = actions.push(btn_icon(
            Icon::Disc,
            "Open in library",
            Btn::Default,
            Some(Message::Open(Page::LibraryAlbum(fmt::s(a, "library_album_id")))),
        ));
    }
    if fmt::truthy(a, "in_library") {
        if !fmt::truthy(a, "library_album_id") {
            if let Some(av) = catalog::availability(a) {
                actions = actions.push(av);
            }
        }
    } else if fmt::truthy(a, "request_status") {
        actions = actions.push(catalog::live_badge(&fmt::s(a, "request_status"), &fmt::s(a, "request_label")));
    } else {
        let key = format!("rg:{}", fmt::s(a, "id"));
        let on = (!state.sending.contains(&key)).then(|| Message::Request(key.clone(), Box::new(album_body(a))));
        actions = actions.push(btn_icon(Icon::Plus, "Request album", Btn::Primary, on));
    }
    if owned_n > 0 && has_release && owned_n < ts.len() {
        actions = actions.push(small_muted(format!("{owned_n} of {} songs in your library", ts.len())));
    }
    actions = actions.push(iced::widget::tooltip(btn_icon(Icon::Search, "Pick download", Btn::Default, Some(Message::Pick(None))), crate::widgets::tip_box("Search every source and choose the exact download"), iced::widget::tooltip::Position::Bottom).gap(4));
    actions = actions.push(btn_icon(
        Icon::External,
        "MusicBrainz",
        Btn::Ghost,
        Some(Message::OpenUrl(format!("https://musicbrainz.org/release-group/{}", fmt::s(a, "id")))),
    ));
    text = text.push(container(actions.wrap()).padding([6, 0]));
    let cover = fmt::s(a, "cover").replacen("front-250", "front-500", 1);
    row![catalog::art(ctx, &cover, 220.0, false, Icon::Disc), text.width(Length::Fill)]
        .spacing(26)
        .align_y(Alignment::Center)
        .into()
}

fn track_table<'a>(state: &'a State, ctx: &Ctx<'a>, a: &'a Value, ts: &[Value]) -> El<'a, Message> {
    let p = ctx.look;
    let multi = fmt::f(&a["release"], "disc_count") > 1.0;
    let current = ctx.player.current_id();
    let album_requested = fmt::truthy(a, "request_status");
    let header = container(
        row![
            container(small_muted("#")).width(32).align_x(Alignment::Center),
            small_muted("Title").width(Length::Fill),
            small_muted("Artist").width(Length::FillPortion(1)),
            small_muted("Length").width(52),
            space().width(200),
        ]
        .spacing(12)
        .align_y(Alignment::Center),
    )
    .padding([8, 12]);
    let mut list = column![header, widgets::divider()];
    for (i, tr) in ts.iter().enumerate() {
        let has = fmt::truthy(tr, "library");
        let rid = fmt::s(tr, "recording_id");
        let playing = has && current.is_some() && current == tr["library"]["id"].as_i64();
        let num: El<'a, Message> = if has {
            icon_btn(Icon::Play, "Play", Some(Message::PlayRow(rid.clone())))
        } else {
            let n = format!("{}{}", if multi { format!("{}-", fmt::s(tr, "disc")) } else { String::new() }, fmt::s(tr, "track"));
            container(small_muted(n)).width(32).align_x(Alignment::Center).into()
        };
        let title_text = t(fmt::s(tr, "title"))
            .size(14)
            .font(theme::semibold())
            .color(if playing { p.accent } else { p.text });
        let title: El<'a, Message> = if has {
            iced::widget::button(title_text)
                .padding(0)
                .style(theme::btn(Btn::Row))
                .on_press(Message::PlayRow(rid.clone()))
                .into()
        } else {
            title_text.into()
        };
        let mut right = row![
            icon_btn(Icon::Spark, "More like this", Some(Message::More(i))),
            icon_btn(Icon::Search, "Pick a download for this song", Some(Message::Pick(Some(i)))),
        ]
        .spacing(2)
        .align_y(Alignment::Center);
        if fmt::truthy(tr, "in_library") {
            right = right.push(catalog::in_library());
        } else if fmt::truthy(tr, "request_status") {
            right = right.push(catalog::live_badge(&fmt::s(tr, "request_status"), &fmt::s(tr, "request_label")));
        } else if !album_requested {
            let key = catalog::song_key(tr);
            let on = (!state.sending.contains(&key)).then(|| Message::Request(key.clone(), Box::new(track_body(a, tr))));
            right = right.push(btn_icon(Icon::Plus, "Track", Btn::Default, on));
        }
        let r = row![
            num,
            container(title).width(Length::Fill),
            small_muted(fmt::s(tr, "artist")).width(Length::FillPortion(1)),
            small_muted(fmt::mmss(fmt::f(tr, "length"))).width(52),
            container(right).width(200).align_x(Alignment::End),
        ]
        .spacing(12)
        .align_y(Alignment::Center);
        list = list.push(column![container(r).padding([6, 12]).width(Length::Fill), widgets::divider()]);
    }
    widgets::card_flat(list).width(Length::Fill).into()
}

pub fn view<'a>(state: &'a State, ctx: &Ctx<'a>) -> El<'a, Message> {
    let a = match &state.a {
        None => return widgets::skeleton::block(Length::Fill, 220.0),
        Some(Err(e)) => return column![h1("Album"), error_box(e)].spacing(12).into(),
        Some(Ok(a)) => a,
    };
    let ts = tracks(a);
    let body: El<'a, Message> = if a["release"].is_object() {
        track_table(state, ctx, a, &ts)
    } else {
        empty(Icon::Disc, "No track listing available", "")
    };
    column![head(state, ctx, a, &ts), body].spacing(26).width(Length::Fill).into()
}

/// A modal or ⋯ menu to lay over the whole window, when one is open.
pub fn overlay<'a>(state: &'a State, ctx: &Ctx<'a>) -> Option<El<'a, Message>> {
    if let Some(p) = &state.picker {
        return Some(picker::view(p, ctx).map(Message::Picker));
    }
    state
        .more_like
        .as_ref()
        .map(|ml| widgets::more_like::view(ml, ctx).map(Message::MoreLike))
}

pub fn subscription(state: &State) -> Subscription<Message> {
    match (&state.live, state.a.is_some()) {
        (Some(key), true) => catalog::live_requests(key.clone()).map(Message::Live),
        _ => Subscription::none(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn album_json() -> Value {
        json!({
            "id": "rg1", "title": "T", "artist": "A", "artist_id": "m1", "year": "2001", "cover": "https://x/front-250",
            "release": { "disc_count": 1, "tracks": [
                { "recording_id": "r1", "title": "One", "artist": "A", "length": 100.0, "library": { "id": 7, "title": "One" }, "in_library": true },
                { "recording_id": "r2", "title": "Two", "artist": "A", "length": 200.0, "isrc": "X" },
                { "recording_id": "r3", "title": "Three", "artist": "A", "length": 50.0, "library": { "id": 9 }, "in_library": true }
            ] }
        })
    }

    #[test]
    fn bodies_follow_the_web() {
        let a = album_json();
        let ts = tracks(&a);
        assert_eq!(ts.len(), 3);
        assert_eq!(owned_rows(&ts).iter().map(|t| fmt::i(t, "id")).collect::<Vec<_>>(), vec![7, 9]);
        let b = track_body(&a, &ts[1]);
        assert_eq!(b["kind"], "track");
        assert_eq!(b["mbid"], "r2");
        assert_eq!(b["album"], "T");
        assert_eq!(b["isrc"], "X");
        assert_eq!(b["album_mode"], "song");
        let ab = album_body(&a);
        assert_eq!(ab["mbid"], "rg1");
        assert!(ab.get("ext").is_none());
        assert!(tracks(&json!({ "release": null })).is_empty());
    }

    #[test]
    fn events_patch_album_and_rows() {
        let mut st = State {
            a: Some(Ok(album_json())),
            ..State::default()
        };
        patch(
            &mut st,
            &Live::Request {
                keys: vec!["rg:rg1".into(), "rec:r2".into()],
                status: "queued".into(),
                label: "Requested".into(),
            },
        );
        let a = album(&st).unwrap();
        assert_eq!(a["request_status"], "queued");
        assert_eq!(a["release"]["tracks"][1]["request_status"], "queued");
        assert!(a["release"]["tracks"][0].get("request_status").is_none());
    }
}
