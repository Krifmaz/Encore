//! Home: the greeting, the search and link boxes, the stats, Made for you (ListenBrainz),
//! New releases from library artists, a taste of For you, the downloads in progress and the
//! Recently added albums. Behaviour reference: web-rs/src/pages/home.rs.

use super::library::{self, num, req_body, request_toast, status_badge, tr};
use crate::api::ApiError;
use crate::fmt;
use crate::pages::{urlencode, Ctx, El, Page};
use crate::theme::{self, Badge, Btn};
use crate::widgets::{
    self, badge, btn_icon, btn_sm, grid, h1, h2, icon, icon_btn, modal, muted, search_box, section_head, skeleton_tiles,
    small_muted, t, Icon, Tile, TileKind,
};
use iced::widget::{button, column, container, image, progress_bar, row, space, text_input, toggler};
use iced::{Alignment, ContentFit, Length, Subscription, Task};
use serde_json::Value;
use std::time::Duration;

/// The Made for you playlist that is open.
#[derive(Debug)]
pub struct MfyDialog {
    pub index: usize,
    pub requesting: bool,
}

#[derive(Debug, Default)]
pub struct State {
    /// Recently added albums (None while loading).
    pub recent: Option<Vec<Value>>,
    pub error: String,
    pub query: String,
    pub link: String,
    pub linking: bool,
    /// `/stats/me`.
    pub stats: Option<Value>,
    /// `/discover/new-releases`: the releases.
    pub releases: Vec<Value>,
    /// Hide the release kinds Settings → Release types skips (the web's `encore.hideSkipped`).
    pub hide_skipped: bool,
    /// `/discover/made-for-you`: the playlists, the ListenBrainz user.
    pub mfy: Vec<Value>,
    pub mfy_user: String,
    /// The first For you playlist's songs (up to six).
    pub fy: Vec<Value>,
    /// The user's active downloads.
    pub reqs: Option<Vec<Value>>,
    pub dialog: Option<MfyDialog>,
}

impl State {
    fn init() -> Self {
        Self {
            hide_skipped: true,
            ..Default::default()
        }
    }
}

#[derive(Clone, Debug)]
pub enum Message {
    Recent(Result<Value, ApiError>),
    Stats(Result<Value, ApiError>),
    Releases(Result<Value, ApiError>),
    MadeForYou(Result<Value, ApiError>),
    ForYou(Result<Value, ApiError>),
    Reqs(Result<Value, ApiError>),
    Tick,
    Query(String),
    Search,
    Link(String),
    SubmitLink,
    Linked(Result<Value, ApiError>),
    Open(String),
    Play(String),
    /// The album's tracks arrived for `Play`.
    Album(String, Result<Value, ApiError>),
    Library,
    Leaderboard,
    AllRequests,
    SeeForYou,
    HideSkipped(bool),
    OpenRelease(Box<Value>),
    OpenMfy(usize),
    PlayMfy(usize),
    /// Play the owned songs of the open playlist from this row.
    MfyRow(usize),
    MfyRequest(usize),
    MfyRequested(usize, Result<Value, ApiError>),
    MfyRequestMissing,
    MfyRequestedAll(Vec<(usize, Value)>, Option<String>),
    OnListenBrainz,
    PlayFy(usize),
    CancelReq(i64),
    ReqDone(Result<Value, ApiError>),
    CloseDialog,
}

fn load_reqs(ctx: &Ctx) -> Task<Message> {
    let api = ctx.api.clone();
    Task::perform(
        async move { api.get("/requests?status=active&mine=true&limit=8").await },
        Message::Reqs,
    )
}

pub fn open(state: &mut State, ctx: &mut Ctx) -> Task<Message> {
    *state = State::init();
    state.hide_skipped = ctx.config.hide_skipped();
    let api = ctx.api.clone();
    let get = |path: &'static str, f: fn(Result<Value, ApiError>) -> Message| {
        let api = api.clone();
        Task::perform(async move { api.get(path).await }, f)
    };
    Task::batch([
        get("/library/recent?limit=12", Message::Recent),
        get("/stats/me", Message::Stats),
        get("/discover/new-releases?limit=12", Message::Releases),
        get("/discover/made-for-you", Message::MadeForYou),
        get("/discover/for-you", Message::ForYou),
        load_reqs(ctx),
    ])
}

/// The owned songs of a Made for you playlist, as the library knows them.
fn owned_tracks(p: &Value) -> Vec<Value> {
    fmt::items(&p["tracks"])
        .iter()
        .filter(|t| fmt::truthy(t, "library"))
        .map(|t| t["library"].clone())
        .collect()
}

fn is_missing(t: &Value) -> bool {
    !fmt::truthy(t, "library") && !fmt::truthy(t, "request_status")
}

pub fn update(state: &mut State, msg: Message, ctx: &mut Ctx) -> Task<Message> {
    match msg {
        Message::Recent(Ok(v)) => state.recent = Some(fmt::items(&v)),
        Message::Recent(Err(e)) => {
            state.recent = Some(Vec::new());
            state.error = e.message;
        }
        Message::Stats(Ok(s)) => {
            // the JS threw (and swallowed it) without these objects
            if s["library"].is_object() && s["me"].is_object() {
                state.stats = Some(s);
            }
        }
        Message::Stats(Err(_)) => {}
        Message::Releases(Ok(d)) => state.releases = fmt::items(&d["releases"]),
        Message::Releases(Err(_)) => {}
        Message::MadeForYou(Ok(d)) => {
            if fmt::truthy(&d, "connected") {
                state.mfy = fmt::items(&d["playlists"]);
                state.mfy_user = fmt::s(&d, "user");
            }
        }
        Message::MadeForYou(Err(_)) => {}
        Message::ForYou(Ok(d)) => {
            if let Some(pl) = fmt::items(&d["playlists"]).into_iter().next() {
                if pl["tracks"].is_array() {
                    state.fy = fmt::items(&pl["tracks"]).into_iter().take(6).collect();
                }
            }
        }
        Message::ForYou(Err(_)) => {}
        Message::Reqs(Ok(v)) => state.reqs = Some(fmt::items(&v)),
        Message::Reqs(Err(_)) => {
            if state.reqs.is_none() {
                state.reqs = Some(Vec::new());
            }
        }
        Message::Tick => return load_reqs(ctx),
        Message::Query(q) => state.query = q,
        Message::Search => {
            let q = state.query.trim().to_string();
            if !q.is_empty() {
                ctx.navigate(Page::Search { q });
            }
        }
        Message::Link(l) => state.link = l,
        Message::SubmitLink => {
            let url = state.link.trim().to_string();
            if url.is_empty() || state.linking {
                return Task::none();
            }
            state.linking = true;
            let api = ctx.api.clone();
            return Task::perform(
                async move { api.post("/requests/link", serde_json::json!({ "url": url })).await },
                Message::Linked,
            );
        }
        Message::Linked(Ok(r)) => {
            state.linking = false;
            state.link.clear();
            ctx.refresh_shell();
            if fmt::s(&r, "mode") == "matched" {
                let mut msg = format!("{} song(s) requested", fmt::s(&r, "requested"));
                if fmt::truthy(&r, "name") {
                    msg.push_str(&format!(" from {}", fmt::s(&r, "name")));
                }
                if fmt::truthy(&r, "already_requested") {
                    msg.push_str(&format!(", {} already requested", fmt::s(&r, "already_requested")));
                }
                ctx.toast_ok(msg);
            } else {
                ctx.toast_ok("Download started");
            }
            return load_reqs(ctx);
        }
        Message::Linked(Err(e)) => {
            state.linking = false;
            ctx.toast_err(e.message);
        }
        Message::Open(id) => ctx.navigate(Page::LibraryAlbum(id)),
        Message::Play(id) => {
            let api = ctx.api.clone();
            let path = format!("/library/albums/{id}");
            return Task::perform(async move { api.get(&path).await }, move |r| Message::Album(id.clone(), r));
        }
        Message::Album(id, Ok(album)) => {
            let tracks = library::with_album_id(&fmt::items(&album["tracks"]), &id);
            if tracks.is_empty() {
                ctx.toast_err("This album has no songs");
            } else {
                ctx.play(tracks, 0);
            }
        }
        Message::Album(_, Err(e)) => ctx.toast_err(e.message),
        // the web's `#/library?tab=albums&sort=added&dir=desc` (Recently added)
        Message::Library => ctx.navigate(Page::Library {
            tab: "albums".into(),
            query: crate::pages::LibraryQuery {
                sort: "added".into(),
                dir: "desc".into(),
                ..Default::default()
            },
        }),
        Message::Leaderboard => ctx.navigate(Page::Leaderboard),
        Message::AllRequests => ctx.navigate(Page::Requests),
        Message::SeeForYou => ctx.navigate(Page::ForYou),
        Message::HideSkipped(on) => {
            state.hide_skipped = on;
            ctx.edit_config(move |c| {
                c.local.insert(crate::config::HIDE_SKIPPED.into(), Value::String(if on { "1" } else { "0" }.into()));
            });
        }
        Message::OpenRelease(r) => {
            if fmt::truthy(&r, "mbid") {
                ctx.navigate(Page::Album(fmt::s(&r, "mbid")));
            } else {
                ctx.navigate(Page::ArtistName(fmt::s(&r, "artist")));
            }
        }
        Message::OpenMfy(i) => {
            if i < state.mfy.len() {
                state.dialog = Some(MfyDialog {
                    index: i,
                    requesting: false,
                });
            }
        }
        Message::PlayMfy(i) => {
            if let Some(p) = state.mfy.get(i) {
                let owned = owned_tracks(p);
                if !owned.is_empty() {
                    ctx.play(owned, 0);
                }
            }
        }
        Message::MfyRow(row) => {
            if let Some(p) = state.dialog.as_ref().and_then(|d| state.mfy.get(d.index)) {
                let tracks = fmt::items(&p["tracks"]);
                let owned = owned_tracks(p);
                let start = tracks[..row.min(tracks.len())]
                    .iter()
                    .filter(|t| fmt::truthy(t, "library"))
                    .count();
                if !owned.is_empty() {
                    ctx.play(owned, start);
                }
            }
        }
        Message::MfyRequest(row) => {
            if let Some(t) = state
                .dialog
                .as_ref()
                .and_then(|d| state.mfy.get(d.index))
                .and_then(|p| p["tracks"].get(row))
            {
                let body = req_body(t);
                let api = ctx.api.clone();
                return Task::perform(async move { api.post("/requests", body).await }, move |r| {
                    Message::MfyRequested(row, r)
                });
            }
        }
        Message::MfyRequested(row, Ok(r)) => {
            ctx.toast_ok(request_toast(&r));
            ctx.refresh_shell();
            if let Some(t) = state
                .dialog
                .as_ref()
                .map(|d| d.index)
                .and_then(|i| state.mfy.get_mut(i))
                .and_then(|p| p["tracks"].get_mut(row))
            {
                t["request_status"] = r["status"].clone();
            }
        }
        Message::MfyRequested(_, Err(e)) => ctx.toast_err(e.message),
        Message::MfyRequestMissing => {
            let Some(d) = &mut state.dialog else {
                return Task::none();
            };
            if d.requesting {
                return Task::none();
            }
            let Some(p) = state.mfy.get(d.index) else {
                return Task::none();
            };
            d.requesting = true;
            let missing: Vec<(usize, Value)> = fmt::items(&p["tracks"])
                .iter()
                .enumerate()
                .filter(|(_, t)| is_missing(t))
                .map(|(i, t)| (i, req_body(t)))
                .collect();
            let api = ctx.api.clone();
            return Task::perform(
                async move {
                    let mut done = Vec::new();
                    let mut err = None;
                    for (i, body) in missing {
                        match api.post("/requests", body).await {
                            Ok(r) => done.push((i, r)),
                            Err(e) => {
                                if e.message.to_ascii_lowercase().contains("limit") {
                                    err = Some(e.message);
                                    break;
                                }
                            }
                        }
                    }
                    (done, err)
                },
                |(done, err)| Message::MfyRequestedAll(done, err),
            );
        }
        Message::MfyRequestedAll(done, err) => {
            if let Some(d) = &mut state.dialog {
                d.requesting = false;
                if let Some(p) = state.mfy.get_mut(d.index) {
                    for (i, r) in &done {
                        if let Some(t) = p["tracks"].get_mut(*i) {
                            t["request_status"] = r["status"].clone();
                        }
                    }
                }
            }
            if let Some(e) = err {
                ctx.toast_err(e);
            }
            ctx.toast_ok(format!("{} song(s) requested", done.len()));
            ctx.refresh_shell();
            return load_reqs(ctx);
        }
        Message::OnListenBrainz => ctx.open_url(format!(
            "https://listenbrainz.org/user/{}/playlists/",
            urlencode(&state.mfy_user)
        )),
        Message::PlayFy(i) => {
            if let Some(t) = state.fy.get(i) {
                if fmt::truthy(t, "library") {
                    // a single song: the library shuffle continues after it
                    ctx.play(vec![t["library"].clone()], 0);
                }
            }
        }
        Message::CancelReq(id) => {
            let api = ctx.api.clone();
            return Task::perform(
                async move { api.post_empty(&format!("/requests/{id}/cancel")).await },
                Message::ReqDone,
            );
        }
        Message::ReqDone(Ok(_)) => return load_reqs(ctx),
        Message::ReqDone(Err(e)) => ctx.toast_err(e.message),
        Message::CloseDialog => state.dialog = None,
    }
    Task::none()
}

fn greeting() -> &'static str {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // The words are the web's; the hour is the computer's clock in UTC (a local offset is not
    // worth a dependency).
    let hour = (secs / 3600) % 24;
    if hour < 12 {
        "Good morning"
    } else if hour < 18 {
        "Good afternoon"
    } else {
        "Good evening"
    }
}

/// `KIND[kind] || ''` (web foryou `kind`).
fn kind_text(k: &str) -> &'static str {
    match k {
        "weekly-exploration" => "New music picked for you every Monday",
        "daily-jams" => "Songs you love, refreshed daily",
        "weekly-jams" => "Your favourites from the past weeks",
        "lastfm" => "Similar to your most played songs on Last.fm",
        _ => "",
    }
}

/// One `.stat` box.
fn stat<'a>(value: String, label: El<'a, Message>) -> El<'a, Message> {
    container(
        column![t(value).size(22).font(theme::bold()), label]
            .spacing(2)
            .align_x(Alignment::Start),
    )
    .padding([12, 16])
    .width(Length::Fixed(200.0))
    .style(theme::card)
    .into()
}

fn stats_row<'a>(s: &'a Value) -> El<'a, Message> {
    let (lib, me) = (&s["library"], &s["me"]);
    let duration = fmt::f(lib, "duration");
    let mut boxes: Vec<El<'a, Message>> = vec![
        stat(num(fmt::f(lib, "tracks")), small_muted("songs in the library").into()),
        stat(
            library::long_dur(duration),
            small_muted(format!("to listen to{}", if duration > 86_400.0 { " non-stop" } else { "" })).into(),
        ),
        stat(
            num(fmt::f(me, "plays")),
            small_muted(format!(
                "songs you played{}",
                if fmt::truthy(me, "listened") {
                    format!(" · {}", library::long_dur(fmt::f(me, "listened")))
                } else {
                    String::new()
                }
            ))
            .into(),
        ),
    ];
    let requests = fmt::f(me, "requests");
    let things = if requests == 1.0 { "thing" } else { "things" };
    let rank_label: El<'a, Message> = if fmt::truthy(me, "rank") {
        row![
            small_muted(format!("{things} you requested · #{} on the", fmt::s(me, "rank"))),
            button(small_muted("leaderboard").color(theme::look().accent))
                .padding(0)
                .style(theme::btn(Btn::Row))
                .on_press(Message::Leaderboard),
        ]
        .spacing(4)
        .align_y(Alignment::Center)
        .into()
    } else {
        small_muted(format!("{things} you requested")).into()
    };
    boxes.push(stat(num(requests), rank_label));
    if fmt::truthy(me, "top_artist") {
        boxes.push(stat(fmt::s(me, "top_artist"), small_muted("your most played artist").into()));
    }
    if fmt::truthy(s, "fun") {
        boxes.push(stat(fmt::s(&s["fun"], "value"), small_muted(fmt::s(&s["fun"], "label")).into()));
    }
    iced::widget::row(boxes).spacing(12).wrap().into()
}

/// A request of "Your downloads in progress" (web `request_row`, the home's part of it).
fn request_row<'a>(r: &'a Value, ctx: &Ctx<'a>) -> El<'a, Message> {
    let p = ctx.look;
    let kind = fmt::s(r, "kind");
    let status = fmt::s(r, "status");
    let or = |a: &str, b: &str| if fmt::truthy(r, a) { fmt::s(r, a) } else { fmt::s(r, b) };
    let what = match kind.as_str() {
        "album" => format!("{} · {}", or("title", "album"), fmt::s(r, "artist")),
        "artist" => format!("{} · full discography", fmt::s(r, "artist")),
        "url" => or("title", "url"),
        "playlist" => format!("{} · playlist import", fmt::s(r, "title")),
        _ => format!("{} · {}", fmt::s(r, "title"), fmt::s(r, "artist")),
    };
    let active = matches!(status.as_str(), "searching" | "downloading" | "importing" | "monitoring");
    let thumb: El<'a, Message> = match ctx.picture(&fmt::s(r, "cover_url")) {
        Some(h) => image(h)
            .width(40)
            .height(40)
            .content_fit(ContentFit::Cover)
            .border_radius(6.0)
            .into(),
        None => container(icon(Icon::Download, 16.0, p.faint))
            .width(40)
            .height(40)
            .center(40)
            .style(theme::art)
            .into(),
    };
    let mut line = row![badge(kind.clone(), Badge::Plain)].spacing(6).align_y(Alignment::Center);
    let attempts = fmt::i(r, "attempts");
    if attempts > 1 {
        line = line.push(widgets::faint(format!("{attempts} attempts ·")).size(12));
    }
    if fmt::truthy(r, "source") {
        line = line.push(widgets::faint(format!("via {} ·", fmt::s(r, "source"))).size(12));
    }
    if fmt::truthy(r, "message") {
        line = line.push(small_muted(fmt::s(r, "message")));
    }
    line = line.push(widgets::faint(format!("· {}", fmt::ago(fmt::f(r, "created_at")))).size(12));
    let mut lines = column![t(what).size(14).font(theme::semibold()), line].spacing(3);
    if active && fmt::truthy(r, "progress") {
        lines = lines.push(
            container(
                progress_bar(0.0..=100.0, fmt::f(r, "progress") as f32)
                    .girth(5)
                    .style(theme::progress),
            )
            .max_width(420),
        );
    }
    let mut right = row![status_badge(&status)].spacing(6).align_y(Alignment::Center);
    if matches!(
        status.as_str(),
        "pending" | "queued" | "searching" | "downloading" | "monitoring" | "importing"
    ) {
        right = right.push(icon_btn(Icon::X, "Cancel", Some(Message::CancelReq(fmt::i(r, "id")))));
    }
    column![
        container(
            row![thumb, lines.width(Length::Fill), right]
                .spacing(14)
                .align_y(Alignment::Center)
        )
        .padding([11, 16]),
        widgets::divider(),
    ]
    .into()
}

pub fn view<'a>(state: &'a State, ctx: &Ctx<'a>) -> El<'a, Message> {
    let p = ctx.look;
    let hero = column![
        h1(format!("{}, {}", greeting(), ctx.username())),
        muted("Find any artist, album or song – or paste a link from anywhere.").size(14),
        container(search_box(
            "Search artists, albums, songs…",
            &state.query,
            Message::Query,
            Some(Message::Search)
        ))
        .max_width(560.0),
        container(
            text_input("…or paste a song, album or playlist link to download", &state.link)
                .on_input(Message::Link)
                .on_submit(Message::SubmitLink)
                .padding([9, 12])
                .size(14)
                .style(theme::input)
        )
        .max_width(560.0),
    ]
    .spacing(10);
    let mut page = column![hero].spacing(22).width(Length::Fill);

    if let Some(s) = &state.stats {
        page = page.push(stats_row(s));
    }

    // Made for you
    if !state.mfy.is_empty() {
        let head = row![
            column![
                h2("Made for you"),
                small_muted(format!("ListenBrainz builds these from what {} listens to", state.mfy_user)),
            ]
            .spacing(2),
            space().width(Length::Fill),
            icon_btn(Icon::External, "On ListenBrainz", Some(Message::OnListenBrainz)),
        ]
        .align_y(Alignment::Center);
        let tiles = state
            .mfy
            .iter()
            .enumerate()
            .map(|(i, pl)| {
                let covers: Vec<Option<image::Handle>> = fmt::items(&pl["covers"])
                    .iter()
                    .filter_map(Value::as_str)
                    .map(|c| ctx.picture(c))
                    .collect();
                let k = kind_text(&fmt::s(pl, "kind"));
                let mut tile = Tile::new(fmt::s(pl, "title"), TileKind::Playlist, Message::OpenMfy(i))
                    .sub(if k.is_empty() { "From ListenBrainz" } else { k })
                    .art(widgets::tile::collage(covers, widgets::tile::WIDTH, Icon::Spark))
                    .corner(badge(
                        format!("{}/{} in library", fmt::s(pl, "owned"), fmt::s(pl, "total")),
                        Badge::Plain,
                    ));
                if fmt::truthy(pl, "owned") {
                    tile = tile.play(Message::PlayMfy(i));
                }
                tile.view()
            })
            .collect();
        page = page.push(column![head, grid(tiles)].spacing(14));
    }

    // New releases
    if !state.releases.is_empty() {
        let skipped = state.releases.iter().filter(|r| fmt::truthy(r, "skipped")).count();
        let mut right = row![small_muted("from artists in your library")]
            .spacing(10)
            .align_y(Alignment::Center);
        if skipped > 0 {
            right = right.push(
                toggler(state.hide_skipped)
                    .label("Hide skipped types")
                    .text_size(12.5)
                    .spacing(6)
                    .on_toggle(Message::HideSkipped)
                    .style(theme::switch),
            );
            if state.hide_skipped {
                right = right.push(
                    button(badge(format!("+{skipped} hidden"), Badge::Plain))
                        .padding(0)
                        .style(theme::btn(Btn::Row))
                        .on_press(Message::HideSkipped(false)),
                );
            }
        }
        let tiles = state
            .releases
            .iter()
            .filter(|r| !(state.hide_skipped && fmt::truthy(r, "skipped")))
            .map(|r| {
                let mut sub = format!("{} · {}", fmt::s(r, "artist"), fmt::s(r, "date"));
                if fmt::truthy(r, "type") && fmt::s(r, "type") != "Album" {
                    sub.push_str(&format!(" · {}", fmt::s(r, "type")));
                }
                let mut tile = Tile::new(fmt::s(r, "title"), TileKind::Album, Message::OpenRelease(Box::new(r.clone())))
                    .sub(sub)
                    .cover(ctx.picture(&fmt::s(r, "cover")));
                if fmt::truthy(r, "in_library") {
                    tile = tile.corner(badge("In library", Badge::Ok));
                }
                tile.view()
            })
            .collect();
        page = page.push(column![section_head("New releases", Some(right.into())), grid(tiles)].spacing(14));
    }

    // a taste of For you
    if !state.fy.is_empty() {
        let mut list = column![];
        for (i, tr_) in state.fy.iter().enumerate() {
            let owned = fmt::truthy(tr_, "library");
            let thumb: El<'a, Message> = match ctx.picture(&fmt::s(tr_, "cover")) {
                Some(h) => image(h)
                    .width(40)
                    .height(40)
                    .content_fit(ContentFit::Cover)
                    .border_radius(6.0)
                    .into(),
                None => container(icon(Icon::Music, 16.0, p.faint))
                    .width(40)
                    .height(40)
                    .center(40)
                    .style(theme::art)
                    .into(),
            };
            let mut right: Vec<El<'a, Message>> = Vec::new();
            if owned {
                right.push(badge("In library", Badge::Ok));
            }
            list = list.push(widgets::list_row(
                Some(thumb),
                tr(ctx, tr_, "title"),
                tr(ctx, tr_, "artist"),
                right,
                owned.then_some(Message::PlayFy(i)),
            ));
        }
        page = page.push(
            column![
                section_head("For you", Some(btn_sm("See all", Btn::Default, Some(Message::SeeForYou)).into())),
                widgets::card_flat(list).width(Length::Fill),
            ]
            .spacing(14),
        );
    }

    // downloads in progress
    if let Some(reqs) = state.reqs.as_ref().filter(|r| !r.is_empty()) {
        let mut list = column![];
        for r in reqs {
            list = list.push(request_row(r, ctx));
        }
        page = page.push(
            column![
                section_head(
                    "Your downloads in progress",
                    Some(btn_sm("All requests", Btn::Default, Some(Message::AllRequests)).into())
                ),
                widgets::card_flat(list).width(Length::Fill),
            ]
            .spacing(14),
        );
    }

    // Recently added
    let recent: El<'a, Message> = match &state.recent {
        None => skeleton_tiles(6, widgets::tile::WIDTH),
        Some(albums) if albums.is_empty() => {
            if !state.error.is_empty() {
                widgets::error_box(&state.error)
            } else {
                widgets::empty(
                    Icon::Library,
                    "Your library is empty",
                    "Search above or paste a link to get started.",
                )
            }
        }
        Some(albums) => {
            let tiles = albums
                .iter()
                .map(|a| {
                    let id = fmt::s(a, "id");
                    let title = if fmt::truthy(a, "album") { tr(ctx, a, "album") } else { "Unknown album".to_string() };
                    let mut sub = tr(ctx, a, "albumartist");
                    if fmt::truthy(a, "year") {
                        sub.push_str(&format!(" · {}", fmt::s(a, "year")));
                    }
                    let mut tile = Tile::new(title, TileKind::Album, Message::Open(id.clone()))
                        .sub(sub)
                        .cover(ctx.cover(&id, &a["cover_v"], 400))
                        .play(Message::Play(id));
                    if fmt::truthy(a, "untagged") {
                        tile = tile.corner(
                            iced::widget::tooltip(badge("Untagged", Badge::Warn), crate::widgets::tip_box("Some tracks lack MusicBrainz tags"), iced::widget::tooltip::Position::Top).gap(4).into(),
                        );
                    }
                    tile.view()
                })
                .collect();
            grid(tiles)
        }
    };
    page = page.push(
        column![
            section_head(
                "Recently added",
                Some(btn_sm("Library", Btn::Default, Some(Message::Library)).into())
            ),
            recent,
        ]
        .spacing(14),
    );
    page.into()
}

/// The open Made for you playlist.
fn mfy_dialog<'a>(state: &'a State, d: &'a MfyDialog, ctx: &Ctx<'a>) -> Option<El<'a, Message>> {
    let p = ctx.look;
    let pl = state.mfy.get(d.index)?;
    let tracks = fmt::items(&pl["tracks"]);
    let missing = tracks.iter().filter(|t| is_missing(t)).count();
    let k = kind_text(&fmt::s(pl, "kind"));
    let mut head = row![
        small_muted(format!("{k} · {}/{} in library", fmt::s(pl, "owned"), fmt::s(pl, "total"))),
        space().width(Length::Fill),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    if fmt::truthy(pl, "owned") {
        head = head.push(btn_icon(Icon::Play, "Play owned", Btn::Default, Some(Message::PlayMfy(d.index))));
    }
    if missing > 0 {
        head = head.push(btn_icon(
            Icon::Download,
            format!("Request {missing} missing"),
            Btn::Primary,
            (!d.requesting).then_some(Message::MfyRequestMissing),
        ));
    }
    let mut list = column![];
    for (i, tr_) in tracks.iter().enumerate() {
        let owned = fmt::truthy(tr_, "library");
        let thumb: El<'a, Message> = match ctx.picture(&fmt::s(tr_, "cover")) {
            Some(h) => image(h)
                .width(40)
                .height(40)
                .content_fit(ContentFit::Cover)
                .border_radius(6.0)
                .into(),
            None => container(icon(Icon::Music, 16.0, p.faint))
                .width(40)
                .height(40)
                .center(40)
                .style(theme::art)
                .into(),
        };
        let mut sub = tr(ctx, tr_, "artist");
        if fmt::truthy(tr_, "album") {
            sub.push_str(&format!(" · {}", tr(ctx, tr_, "album")));
        }
        let mut right: Vec<El<'a, Message>> = Vec::new();
        if fmt::truthy(tr_, "length") {
            right.push(small_muted(fmt::mmss(fmt::f(tr_, "length"))).into());
        }
        right.push(if owned {
            badge("In library", Badge::Ok)
        } else if fmt::truthy(tr_, "request_status") {
            status_badge(&fmt::s(tr_, "request_status"))
        } else {
            btn_icon(Icon::Plus, "Request", Btn::Default, Some(Message::MfyRequest(i))).into()
        });
        list = list.push(widgets::list_row(
            Some(thumb),
            tr(ctx, tr_, "title"),
            sub,
            right,
            owned.then_some(Message::MfyRow(i)),
        ));
    }
    let body = column![head, widgets::card_flat(list).width(Length::Fill)].spacing(12);
    Some(modal(fmt::s(pl, "title"), body, None, true, Message::CloseDialog))
}

pub fn overlay<'a>(state: &'a State, ctx: &Ctx<'a>) -> Option<El<'a, Message>> {
    let d = state.dialog.as_ref()?;
    mfy_dialog(state, d, ctx)
}

/// The downloads in progress refresh every 5 s while the page is open.
pub fn subscription(_state: &State) -> Subscription<Message> {
    iced::time::every(Duration::from_secs(5)).map(|_| Message::Tick)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn made_for_you_helpers() {
        let p = serde_json::json!({ "tracks": [
            { "title": "a", "library": { "id": 1 } },
            { "title": "b" },
            { "title": "c", "request_status": "queued" },
        ]});
        assert_eq!(owned_tracks(&p).len(), 1);
        assert_eq!(owned_tracks(&p)[0]["id"], 1);
        let tr = fmt::items(&p["tracks"]);
        assert!(!is_missing(&tr[0]) && is_missing(&tr[1]) && !is_missing(&tr[2]));
        assert_eq!(kind_text("daily-jams"), "Songs you love, refreshed daily");
        assert_eq!(kind_text("x"), "");
    }
}
