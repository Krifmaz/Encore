//! Discover: search MusicBrainz (and the library) for artists, albums and songs; the best hit
//! big as the Top result; everything can be requested with one click. Behaviour reference:
//! web-rs/src/pages/discover.rs `render_search`.
//!
//! Every search also asks the library (`/api/library/search`, p6 item 9, client-shapes §3.4), on
//! every tab, the newest query winning: "In your library" comes first with the library's
//! artists, albums and songs of the tab's kind (all three on Top results). When the catalogue
//! has nothing but the library has, the library shows instead of "No results"; when the
//! catalogue fails, the library's matches stay with the reason under them.

use crate::api::ApiError;
use crate::fmt;
use crate::pages::{urlencode, Ctx, El, Page};
use crate::theme::{self, Btn};
use crate::widgets::catalog::{self, AlbumTile, Live, SongRow};
use crate::widgets::{self, btn_icon, empty, error_box, grid, h1, muted, search_box, section_head, skeleton_tiles, small_muted, Icon, Tile, TileKind, TrackRow};
use iced::widget::{button, column, container, row, space};
use iced::{Alignment, Length, Subscription, Task};
use serde_json::Value;
use std::collections::HashSet;
use std::time::Duration;

/// How long after the last keystroke the search runs (the web's debounce).
const DEBOUNCE_MS: u64 = 650;

const TABS: [(&str, &str); 4] = [("all", "Top results"), ("artist", "Artists"), ("album", "Albums"), ("track", "Songs")];

#[derive(Debug, Default)]
pub struct State {
    /// The query being shown.
    pub q: String,
    /// The search box.
    pub input: String,
    /// `all`, `artist`, `album` or `track`.
    pub typ: String,
    /// The run number of the newest search (older answers are dropped).
    seq: u64,
    /// The keystroke number (the debounce fires for the newest one only).
    typing: u64,
    /// The catalogue's answer: None while loading, Err(message) when it failed.
    results: Option<Result<Value, String>>,
    /// `GET /api/library/search` for `lib_for` (None while it runs or when it failed).
    lib: Option<Value>,
    lib_for: String,
    /// Request keys being sent.
    sending: HashSet<String>,
    /// The live request stream's identity (server + session).
    live: Option<(String, String)>,
    /// `GET /api/discover/for-you`, shown while the box is empty (asked once per visit).
    picks: Option<Result<Value, String>>,
}

#[derive(Clone, Debug)]
pub enum Message {
    Input(String),
    Debounce(u64),
    Submit,
    Tab(String),
    Results(u64, Result<Value, ApiError>),
    /// The library's answer for this query.
    Lib(String, Result<Value, ApiError>),
    Open(Page),
    /// Play a library album by its id (its tracks are fetched first).
    PlayAlbum(String),
    AlbumTracks(Result<Value, ApiError>),
    /// Play one library song row.
    PlaySong(Box<Value>),
    /// `POST /api/requests` with this body; the key names the rows that show the outcome.
    Request(String, Box<Value>),
    Requested(String, Result<Value, ApiError>),
    Live(Live),
    ForYou,
    /// The For you picks for the empty page.
    Picks(Result<Value, ApiError>),
}

/// Called when the app navigates here (with the route's parameters).
pub fn open(state: &mut State, q: String, ctx: &mut Ctx) -> Task<Message> {
    state.q = q.trim().to_string();
    state.input = state.q.clone();
    if state.typ.is_empty() {
        state.typ = "all".into();
    }
    state.live = catalog::live_key(ctx);
    state.picks = None;
    let api = ctx.api.clone();
    let picks = Task::perform(async move { api.get("/discover/for-you").await }, Message::Picks);
    Task::batch([run(state, ctx), picks])
}

/// Start the search for `state.q` (the library answers at once, the catalogue takes a moment).
/// The library is asked once per query, whatever the tab.
fn run(state: &mut State, ctx: &Ctx) -> Task<Message> {
    state.seq += 1;
    let seq = state.seq;
    state.results = None;
    if state.q.is_empty() {
        state.lib = None;
        state.lib_for.clear();
        return Task::none();
    }
    let api = ctx.api.clone();
    let disc_q = format!("/discover/search?q={}&type={}", urlencode(&state.q), state.typ);
    let mut tasks = vec![Task::perform(
        {
            let api = api.clone();
            async move { api.get(&disc_q).await }
        },
        move |r| Message::Results(seq, r),
    )];
    if state.lib_for != state.q {
        state.lib = None;
        state.lib_for = state.q.clone();
        let q = state.q.clone();
        let lib_q = format!("/library/search?q={}&limit=12", urlencode(&q));
        tasks.push(Task::perform(async move { api.get(&lib_q).await }, move |r| Message::Lib(q.clone(), r)));
    }
    Task::batch(tasks)
}

/// Rows whose key is in the event get its status (songs, albums and the Top result).
fn patch(state: &mut State, ev: &Live) {
    let Some(Ok(d)) = state.results.as_mut() else {
        return;
    };
    if let Some(arr) = d.get_mut("tracks").and_then(Value::as_array_mut) {
        catalog::apply_live(arr, catalog::song_key, ev);
    }
    if let Some(arr) = d.get_mut("albums").and_then(Value::as_array_mut) {
        catalog::apply_live(arr, catalog::album_key, ev);
    }
    let kind = fmt::s(&d["top"], "kind");
    if let Some(item) = d.get_mut("top").and_then(|t| t.get_mut("item")) {
        let key: fn(&Value) -> String = if kind == "album" { catalog::album_key } else { catalog::song_key };
        catalog::apply_live(std::slice::from_mut(item), key, ev);
    }
}

pub fn update(state: &mut State, msg: Message, ctx: &mut Ctx) -> Task<Message> {
    match msg {
        Message::Input(s) => {
            state.input = s;
            state.typing += 1;
            let n = state.typing;
            return Task::perform(tokio::time::sleep(Duration::from_millis(DEBOUNCE_MS)), move |_| Message::Debounce(n));
        }
        Message::Debounce(n) => {
            let v = state.input.trim().to_string();
            if n == state.typing && v.chars().count() > 2 && v != state.q {
                state.q = v;
                return run(state, ctx);
            }
        }
        Message::Submit => {
            state.q = state.input.trim().to_string();
            return run(state, ctx);
        }
        Message::Tab(tab) => {
            state.typ = tab;
            state.q = state.input.trim().to_string();
            return run(state, ctx);
        }
        Message::Results(seq, r) => {
            if seq == state.seq {
                state.results = Some(r.map_err(|e| e.message));
            }
        }
        Message::Lib(q, r) => {
            if q == state.lib_for {
                state.lib = r.ok();
            }
        }
        Message::Open(p) => ctx.navigate(p),
        Message::PlayAlbum(id) => {
            let api = ctx.api.clone();
            return Task::perform(async move { api.get(&format!("/library/albums/{id}")).await }, Message::AlbumTracks);
        }
        Message::AlbumTracks(Ok(album)) => {
            let id = fmt::s(&album, "id");
            let tracks: Vec<Value> = fmt::items(&album["tracks"])
                .into_iter()
                .map(|mut t| {
                    if !fmt::truthy(&t, "album_id") && !id.is_empty() {
                        t["album_id"] = Value::String(id.clone());
                    }
                    t
                })
                .collect();
            if tracks.is_empty() {
                ctx.toast_err("This album has no songs");
            } else {
                ctx.play(tracks, 0);
            }
        }
        Message::AlbumTracks(Err(e)) => ctx.toast_err(e.message),
        Message::PlaySong(t) => ctx.play(vec![*t], 0),
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
                    let ev = Live::Request {
                        keys: vec![key],
                        status: fmt::s(&res, "status"),
                        label: String::new(),
                    };
                    patch(state, &ev);
                }
                Err(e) => ctx.toast_err(e.message),
            }
        }
        Message::Live(Live::Resync) => return run(state, ctx),
        Message::Live(ev) => patch(state, &ev),
        Message::ForYou => ctx.navigate(Page::ForYou),
        Message::Picks(r) => state.picks = Some(r.map_err(|e| e.message)),
    }
    Task::none()
}

// ----- view -------------------------------------------------------------------------------

/// `type · country · disambiguation`.
fn artist_sub(a: &Value) -> String {
    catalog::join_truthy(&[fmt::s(a, "type"), fmt::s(a, "country"), fmt::s(a, "disambiguation")], " · ")
}

/// `artist · year · type`, the default line under a catalogue album tile.
fn album_sub(a: &Value) -> String {
    catalog::join_truthy(&[fmt::s(a, "artist"), fmt::s(a, "year"), fmt::s(a, "type")], " · ")
}

/// The request message of a catalogue album, when it can be requested.
fn album_request(a: &Value) -> Option<Message> {
    if fmt::truthy(a, "in_library") || fmt::truthy(a, "request_status") {
        return None;
    }
    Some(Message::Request(catalog::album_key(a), Box::new(catalog::album_body(a))))
}

fn song_row<'a>(state: &State, ctx: &Ctx<'a>, tr: &Value) -> El<'a, Message> {
    let key = catalog::song_key(tr);
    let can = !state.sending.contains(&key);
    catalog::song_row(
        ctx,
        tr,
        SongRow {
            play: fmt::truthy(tr, "library").then(|| Message::PlaySong(Box::new(tr["library"].clone()))),
            open_artist: Message::Open(catalog::artist_page(tr)),
            open_album: fmt::truthy(tr, "release_group_id").then(|| Message::Open(Page::Album(fmt::s(tr, "release_group_id")))),
            request_song: can.then(|| Message::Request(key.clone(), Box::new(catalog::song_body(tr, "song")))),
            request_album: can.then(|| Message::Request(key.clone(), Box::new(catalog::song_body(tr, "album")))),
        },
    )
}

/// `topResult(top)`: the best song, album or artist, big.
fn top_result<'a>(state: &State, ctx: &Ctx<'a>, top: &Value) -> Option<El<'a, Message>> {
    if !top.is_object() {
        return None;
    }
    let p = ctx.look;
    let x = &top["item"];
    let kind = fmt::s(top, "kind");
    let (kicker, title, cover, href): (String, String, String, Option<Page>) = match kind.as_str() {
        "artist" => ("Artist".into(), fmt::s(x, "name"), String::new(), Some(Page::Artist(fmt::s(x, "id")))),
        "album" => (
            if fmt::truthy(x, "type") { fmt::s(x, "type") } else { "Album".into() },
            fmt::s(x, "title"),
            fmt::s(x, "cover"),
            Some(catalog::album_page(x)),
        ),
        _ => ("Song".into(), fmt::s(x, "title"), fmt::s(x, "cover"), None),
    };
    let mut meta = row![].spacing(6).align_y(Alignment::Center);
    match kind.as_str() {
        "artist" => meta = meta.push(muted(artist_sub(x)).size(14)),
        _ => {
            meta = meta.push(catalog::link(fmt::s(x, "artist"), 14.0, Message::Open(catalog::artist_page(x))));
            if kind != "album" && fmt::truthy(x, "album") {
                meta = meta.push(muted("· on").size(14));
                meta = meta.push(if fmt::truthy(x, "release_group_id") {
                    catalog::muted_link(fmt::s(x, "album"), 14.0, Message::Open(Page::Album(fmt::s(x, "release_group_id"))))
                } else {
                    muted(fmt::s(x, "album")).size(14).into()
                });
            }
            if fmt::truthy(x, "year") {
                meta = meta.push(muted(format!("· {}", fmt::s(x, "year"))).size(14));
            }
        }
    }
    let mut actions = row![].spacing(10).align_y(Alignment::Center);
    match kind.as_str() {
        "artist" => {
            actions = actions.push(btn_icon(Icon::User, "Open artist", Btn::Primary, Some(Message::Open(Page::Artist(fmt::s(x, "id"))))));
            if fmt::truthy(x, "in_library") {
                actions = actions.push(catalog::in_library());
            }
        }
        "album" => {
            if fmt::truthy(x, "library_album_id") {
                actions = actions.push(btn_icon(Icon::Play, "Play", Btn::Primary, Some(Message::PlayAlbum(fmt::s(x, "library_album_id")))));
            } else if let Some(a) = catalog::availability(x) {
                actions = actions.push(a);
            } else if fmt::truthy(x, "request_status") {
                actions = actions.push(catalog::live_badge(&fmt::s(x, "request_status"), &fmt::s(x, "request_label")));
            } else {
                let key = catalog::album_key(x);
                let on = (!state.sending.contains(&key)).then(|| Message::Request(key, Box::new(catalog::album_body(x))));
                actions = actions.push(btn_icon(Icon::Plus, "Request album", Btn::Primary, on));
            }
        }
        _ => {
            if fmt::truthy(x, "library") {
                actions = actions.push(btn_icon(Icon::Play, "Play", Btn::Primary, Some(Message::PlaySong(Box::new(x["library"].clone())))));
            } else if let Some(a) = catalog::availability(x) {
                actions = actions.push(a);
            } else if fmt::truthy(x, "request_status") {
                actions = actions.push(catalog::live_badge(&fmt::s(x, "request_status"), &fmt::s(x, "request_label")));
            } else {
                let key = catalog::song_key(x);
                let can = !state.sending.contains(&key);
                actions = actions.push(
                    row![
                        iced::widget::tooltip(btn_icon(Icon::Plus, "Song", Btn::Primary, can.then(|| Message::Request(key.clone(), Box::new(catalog::song_body(x, "song"))))), crate::widgets::tip_box("Just this song"), iced::widget::tooltip::Position::Bottom).gap(4),
                        iced::widget::tooltip(btn_icon(Icon::Disc, "+ Album", Btn::Default, can.then(|| Message::Request(key.clone(), Box::new(catalog::song_body(x, "album"))))), crate::widgets::tip_box("Request the song and download the whole album it's on, in the best quality available"), iced::widget::tooltip::Position::Bottom).gap(4),
                    ]
                    .spacing(6),
                );
            }
        }
    }
    let fallback = if kind == "artist" { Icon::User } else { Icon::Disc };
    let picture = catalog::art(ctx, &cover, 160.0, kind == "artist", fallback);
    let heading: El<'a, Message> = match &href {
        Some(page) => button(h1(title))
            .padding(0)
            .style(theme::btn(Btn::Row))
            .on_press(Message::Open(page.clone()))
            .into(),
        None => h1(title).into(),
    };
    let text = column![
        small_muted(kicker.to_uppercase()).font(theme::semibold()).color(p.faint),
        heading,
        meta,
        container(actions).padding([8, 0])
    ]
    .spacing(6)
    .width(Length::Fill);
    Some(
        column![
            section_head("Top result", None),
            row![picture, text].spacing(22).align_y(Alignment::Center),
        ]
        .spacing(14)
        .into(),
    )
}

/// How many of the library's artists, albums and songs a tab shows: Top results all three,
/// the other tabs a short strip of their own kind.
pub fn library_counts(typ: &str) -> (usize, usize, usize) {
    match typ {
        "artist" => (6, 0, 0),
        "album" => (0, 6, 0),
        "track" => (0, 0, 3),
        _ => (6, 6, 5),
    }
}

/// `libraryRow(lib)`: what the library already has for the search, of the tab's kind.
fn library_row<'a>(ctx: &Ctx<'a>, lib: &Value, typ: &str) -> Option<El<'a, Message>> {
    let (na, nb, ns) = library_counts(typ);
    let artists: Vec<Value> = fmt::items(&lib["artists"]).into_iter().take(na).collect();
    let songs: Vec<Value> = fmt::items(&lib["songs"]).into_iter().take(ns).collect();
    let albums: Vec<Value> = fmt::items(&lib["albums"]).into_iter().take(nb).collect();
    if songs.is_empty() && albums.is_empty() && artists.is_empty() {
        return None;
    }
    let mut col = column![section_head("In your library", None)].spacing(14);
    if !artists.is_empty() {
        let tiles = artists
            .iter()
            .map(|a| {
                let name = fmt::s(a, "name");
                let open = if fmt::truthy(a, "mbid") { Page::Artist(fmt::s(a, "mbid")) } else { Page::ArtistName(name.clone()) };
                let n = fmt::i(a, "albums");
                Tile::new(crate::pages::library::tr(ctx, a, "name"), TileKind::Artist, Message::Open(open))
                    .sub(format!("{n} album{}", if n == 1 { "" } else { "s" }))
                    .cover(ctx.cover(&fmt::s(a, "cover_album"), &a["cover_v"], 400))
                    .view()
            })
            .collect();
        col = col.push(grid(tiles));
    }
    if !albums.is_empty() {
        let tiles = albums
            .iter()
            .take(6)
            .map(|a| {
                let id = fmt::s(a, "id");
                catalog::library_tile(ctx, &id, a, Message::Open(Page::LibraryAlbum(id.clone())), Message::PlayAlbum(id.clone()))
            })
            .collect();
        col = col.push(grid(tiles));
    }
    if !songs.is_empty() {
        let current = ctx.player.current_id();
        let mut list = column![];
        for tr in songs.iter() {
            let id = fmt::i(tr, "id");
            list = list.push(
                TrackRow::new(fmt::s(tr, "title"))
                    .sub_links(
                        catalog::credit_parts(tr, &fmt::s(tr, "artist")).into_iter().map(|(x, p)| (x, p.map(Message::Open))).collect(),
                        if fmt::truthy(tr, "album") { format!(" · {}", fmt::s(tr, "album")) } else { String::new() },
                    )
                    .cover(ctx.cover(&fmt::s(tr, "album_id"), &tr["cover_v"], 80))
                    .duration(Some(fmt::f(tr, "duration")).filter(|d| *d > 0.0))
                    .quality(fmt::quality(tr))
                    .playing(current == Some(id))
                    .on_play(Message::PlaySong(Box::new(tr.clone())))
                    .view(),
            );
        }
        col = col.push(widgets::card_flat(list));
    }
    Some(col.into())
}

/// A section head with "See all" (the For you page) over a sideways row of tiles.
fn pick_row<'a>(title: String, sub: String, tiles: Vec<El<'a, Message>>) -> El<'a, Message> {
    let mut head = column![widgets::h2(title)].spacing(2);
    if !sub.is_empty() {
        head = head.push(small_muted(sub));
    }
    let head = row![head, space().width(Length::Fill), btn_icon(Icon::Spark, "See all", Btn::Default, Some(Message::ForYou))].align_y(Alignment::Center);
    column![head, widgets::rail(tiles)].spacing(12).into()
}

/// Nothing typed: the For you picks as sideways rows (like Home's Recently added); owned songs
/// play from their tile, the rest open a search for them. None when there are none.
fn picks<'a>(d: &Value, ctx: &Ctx<'a>) -> Option<El<'a, Message>> {
    let lf = &d["lastfm"];
    let search = |q: String| Message::Open(Page::Search { q });
    let mut col = column![].spacing(26);
    let mut any = false;
    let artists = fmt::items(&lf["artists"]);
    if !artists.is_empty() {
        let tiles = artists
            .iter()
            .map(|a| {
                let open = if fmt::truthy(a, "mbid") { Page::Artist(fmt::s(a, "mbid")) } else { Page::ArtistName(fmt::s(a, "name")) };
                catalog::artist_tile(ctx, a, String::new(), Message::Open(open))
            })
            .collect();
        col = col.push(pick_row("Artists for you".into(), String::new(), tiles));
        any = true;
    }
    let albums = fmt::items(&lf["albums"]);
    if !albums.is_empty() {
        let tiles = albums
            .iter()
            .map(|a| {
                AlbumTile {
                    title: fmt::s(a, "title"),
                    sub: catalog::join_truthy(&[fmt::s(a, "artist"), fmt::s(a, "year")], " · "),
                    cover: fmt::s(a, "cover"),
                    corner: fmt::truthy(a, "in_library").then(catalog::in_library),
                    on_open: search(format!("{} {}", fmt::s(a, "artist"), fmt::s(a, "title"))),
                    on_play: None,
                    on_request: None,
                }
                .view(ctx)
            })
            .collect();
        col = col.push(pick_row("Albums for you".into(), String::new(), tiles));
        any = true;
    }
    let mut lists: Vec<(String, Vec<Value>)> = fmt::items(&d["playlists"]).iter().map(|p| (fmt::s(p, "title"), fmt::items(&p["tracks"]))).collect();
    let lf_songs = fmt::items(&lf["tracks"]);
    if !lf_songs.is_empty() {
        lists.insert(0, ("Songs for you".into(), lf_songs));
    }
    for (title, tracks) in lists.into_iter().filter(|(_, t)| !t.is_empty()) {
        let owned = tracks.iter().filter(|t| fmt::truthy(t, "library")).count();
        let sub = format!("{owned}/{} in library", tracks.len());
        let tiles = tracks
            .iter()
            .map(|t| {
                let lib = fmt::truthy(t, "library");
                AlbumTile {
                    title: fmt::s(t, "title"),
                    sub: fmt::s(t, "artist"),
                    cover: fmt::s(t, "cover"),
                    corner: lib.then(catalog::in_library),
                    on_open: search(format!("{} {}", fmt::s(t, "artist"), fmt::s(t, "title"))),
                    on_play: lib.then(|| Message::PlaySong(Box::new(t["library"].clone()))),
                    on_request: None,
                }
                .view(ctx)
            })
            .collect();
        col = col.push(pick_row(title, sub, tiles));
        any = true;
    }
    any.then(|| col.into())
}

fn results<'a>(state: &'a State, ctx: &Ctx<'a>) -> El<'a, Message> {
    if state.q.is_empty() {
        return match &state.picks {
            None => Some(skeleton_tiles(6, widgets::tile::WIDTH)),
            Some(Ok(d)) => picks(d, ctx),
            Some(Err(_)) => None,
        }
        .unwrap_or_else(|| {
            empty(
                Icon::Search,
                "Search your library and the whole catalogue",
                "What you already have shows first; anything else can be requested with one click.",
            )
        });
    }
    let mut col = column![].spacing(22);
    let lib = state.lib.as_ref().filter(|_| state.lib_for == state.q).and_then(|l| library_row(ctx, l, &state.typ));
    let lib_has = lib.is_some();
    if let Some(lib) = lib {
        col = col.push(lib);
    }
    match &state.results {
        None => col = col.push(skeleton_tiles(if lib_has { 8 } else { 12 }, widgets::tile::WIDTH)),
        // the catalogue failed: the library's matches stay, with the reason under them
        Some(Err(e)) if lib_has => col = col.push(muted(format!("Couldn't search the catalogue: {e}")).size(13)),
        Some(Err(e)) => col = col.push(error_box(e)),
        Some(Ok(data)) => {
            let mut any = false;
            if let Some(top) = top_result(state, ctx, &data["top"]) {
                col = col.push(top);
                any = true;
            }
            let tracks = fmt::items(&data["tracks"]);
            if !tracks.is_empty() {
                any = true;
                let mut list = column![];
                for tr in &tracks {
                    list = list.push(song_row(state, ctx, tr));
                }
                col = col.push(column![section_head("Songs", None), widgets::card_flat(list)].spacing(14));
            }
            let albums = fmt::items(&data["albums"]);
            if !albums.is_empty() {
                any = true;
                let tiles = albums
                    .iter()
                    .map(|a| {
                        AlbumTile {
                            title: fmt::s(a, "title"),
                            sub: album_sub(a),
                            cover: fmt::s(a, "cover"),
                            corner: catalog::album_badge(a),
                            on_open: Message::Open(catalog::album_page(a)),
                            on_play: fmt::truthy(a, "library_album_id").then(|| Message::PlayAlbum(fmt::s(a, "library_album_id"))),
                            on_request: album_request(a).filter(|_| !state.sending.contains(&catalog::album_key(a))),
                        }
                        .view(ctx)
                    })
                    .collect();
                col = col.push(column![section_head("Albums", None), grid(tiles)].spacing(14));
            }
            let artists = fmt::items(&data["artists"]);
            if !artists.is_empty() {
                any = true;
                let n = if state.typ == "all" { 6 } else { 60 };
                let tiles = artists
                    .iter()
                    .take(n)
                    .map(|a| catalog::artist_tile(ctx, a, artist_sub(a), Message::Open(Page::Artist(fmt::s(a, "id")))))
                    .collect();
                col = col.push(column![section_head("Artists", None), grid(tiles)].spacing(14));
            }
            if !any {
                col = col.push(if lib_has {
                    El::from(muted(format!("Nothing in the catalogue matches “{}”", state.q)).size(13))
                } else {
                    catalog::empty_text(Icon::Search, format!("No results for “{}”", state.q), String::new())
                });
            }
        }
    }
    col.into()
}

pub fn view<'a>(state: &'a State, ctx: &Ctx<'a>) -> El<'a, Message> {
    let head = row![
        h1("Discover"),
        space().width(Length::Fill),
        btn_icon(Icon::Spark, "For you", Btn::Default, Some(Message::ForYou)),
    ]
    .align_y(Alignment::Center);
    let search = container(search_box(
        "Search your library and MusicBrainz…",
        &state.input,
        Message::Input,
        Some(Message::Submit),
    ))
    .max_width(720);
    let tabs = widgets::tabs(&TABS, &state.typ, Message::Tab);
    column![head, search, row![tabs], results(state, ctx)]
        .spacing(18)
        .width(Length::Fill)
        .into()
}

/// A modal or ⋯ menu to lay over the whole window, when one is open.
pub fn overlay<'a>(_state: &'a State, _ctx: &Ctx<'a>) -> Option<El<'a, Message>> {
    None
}

pub fn subscription(state: &State) -> Subscription<Message> {
    match (&state.live, state.q.is_empty()) {
        (Some(key), false) => catalog::live_requests(key.clone()).map(Message::Live),
        _ => Subscription::none(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_library_strip_follows_the_tab() {
        assert_eq!(library_counts("all"), (6, 6, 5));
        assert_eq!(library_counts("artist"), (6, 0, 0));
        assert_eq!(library_counts("album"), (0, 6, 0));
        assert_eq!(library_counts("track"), (0, 0, 3));
    }

    #[test]
    fn requests_follow_the_rows() {
        let a = json!({ "id": "rg1", "artist": "A", "title": "T" });
        assert!(matches!(album_request(&a), Some(Message::Request(k, _)) if k == "rg:rg1"));
        assert!(album_request(&json!({ "id": "rg2", "in_library": true })).is_none());
        assert!(album_request(&json!({ "id": "rg3", "request_status": "queued" })).is_none());
        assert_eq!(album_sub(&json!({ "artist": "A", "year": "1999", "type": "Album" })), "A · 1999 · Album");
        assert_eq!(artist_sub(&json!({ "type": "Group", "country": "GB" })), "Group · GB");
    }

    #[test]
    fn live_events_patch_the_answer() {
        let mut st = State {
            results: Some(Ok(json!({
                "top": { "kind": "album", "item": { "id": "rg1", "artist": "A", "title": "T" } },
                "albums": [{ "id": "rg1", "artist": "A", "title": "T" }],
                "tracks": [{ "id": "rec1", "artist": "A", "title": "S" }]
            }))),
            ..State::default()
        };
        patch(
            &mut st,
            &Live::Request {
                keys: vec!["rg:rg1".into(), "rec:rec1".into()],
                status: "searching".into(),
                label: "Searching".into(),
            },
        );
        let d = st.results.as_ref().unwrap().as_ref().unwrap();
        assert_eq!(d["albums"][0]["request_status"], "searching");
        assert_eq!(d["tracks"][0]["request_status"], "searching");
        assert_eq!(d["top"]["item"]["request_label"], "Searching");
    }
}
