//! Artist: a catalogue artist from MusicBrainz – discography by kind with Settings → Release
//! types, what the library has, Popular, the bio, similar artists, the releases only Deezer /
//! Apple Music know, the Request discography dialog, links. Opened by name it looks the artist
//! up first. Behaviour reference: web-rs/src/pages/discover.rs `render_artist`,
//! `render_artist_by_name`, `discography_dialog`, `ext_sheet`.
//!
//! Between Popular and the discography (p6 item 5, client-shapes §3.2 / §3.3): "Liked from this
//! artist" (the songs the listener liked, Play all; hidden when there are none) and "In your
//! library" with the artist's Songs (the first ten, "Show all N" loads every one) and Albums.
//! Both are asked only when the library has songs of the artist; an older server's 404 hides
//! them. After them the One-offs shelf (0.36, web `ArtistSongs::one_offs_html`): the artist's songs on
//! compilations, remix albums and soundtracks the library has only a few songs of, with the song ⋯ menu.

use crate::api::ApiError;
use crate::fmt;
use crate::pages::library::{self as lib, Act};
use crate::pages::library_types as lt;
use crate::pages::{urlencode, Ctx, El, Page};
use crate::theme::{self, Badge, Btn};
use crate::widgets::catalog::{self, AlbumTile, Live};
use crate::widgets::{self, badge, btn, btn_icon, btn_sm, empty, error_box, grid, h1, h2, h3, icon_btn, modal, muted, picker, section_head, small_muted, t, Icon, TrackRow};
use iced::widget::{checkbox, column, container, row, space, toggler};
use iced::{Alignment, Length, Subscription, Task};
use serde_json::{json, Map, Value};
use std::collections::HashSet;

const LINK_KEYS: [&str; 9] = [
    "official homepage", "bandcamp", "soundcloud", "youtube", "streaming", "free streaming", "wikidata", "discogs", "last.fm",
];

/// How much of the bio shows before "Show more".
const BIO_CLAMP: usize = 320;

/// One kind of release (Albums, EPs, Singles ...) with its release groups, in page order.
#[derive(Debug)]
struct Group {
    kind: String,
    items: Vec<Value>,
}

/// One box of the Request discography dialog.
#[derive(Clone, Debug)]
struct DiscoRow {
    id: String,
    kind: String,
    title: String,
    line: String,
    checked: bool,
    /// Owned or requested: cannot be ticked.
    fixed: bool,
}

#[derive(Debug)]
enum Dialog {
    /// Request discography: every release on the page with a tick.
    Discography { rows: Vec<DiscoRow>, sending: bool },
    /// "Request N <kind>(s)?" before the missing ones of a group are requested.
    Missing { kind: String, items: Vec<(usize, Value)> },
    /// The track list of a release only Deezer / Apple Music know.
    Ext { m: Box<Value>, tracks: Vec<Value> },
    /// The artist's own release types (admins): `(key, label, ticked)` per kind and per extra to skip.
    Types { kinds: Vec<TypeBox>, skip: Vec<TypeBox>, own: bool, sending: bool },
}

/// A box of the Release types dialog.
#[derive(Debug, Clone)]
struct TypeBox {
    key: String,
    label: String,
    on: bool,
}

/// The boxes of the artist answer's `release_types.<list>` (`[{key, label, on}]`).
fn type_boxes(a: &Value, list: &str) -> Vec<TypeBox> {
    a["release_types"][list].as_array().map(|l| l.iter().map(|o| TypeBox { key: fmt::s(o, "key"), label: fmt::s(o, "label"), on: fmt::truthy(o, "on") }).collect()).unwrap_or_default()
}

#[derive(Debug)]
pub struct State {
    /// The MusicBrainz id, or the name while it is being looked up.
    pub id: String,
    /// `GET /api/discover/artist/{id}`: None while loading, Err(message) when it failed.
    a: Option<Result<Value, String>>,
    groups: Vec<Group>,
    /// Discography filter: `all`, `missing` or `owned` (the web's `encore.artistFilter`).
    filter: String,
    /// The web's `encore.hideSkipped`.
    hide_skipped: bool,
    /// Photo, bio and Popular (`/extras`).
    extras: Option<Value>,
    bio_open: bool,
    /// `/similar`.
    similar: Option<Value>,
    /// `/catalog`: releases MusicBrainz is missing; None while loading.
    ext: Option<Vec<Value>>,
    /// The discography sections opened with "View all" (a grid instead of the sideways row), by kind.
    expanded: HashSet<String>,
    dialog: Option<Dialog>,
    /// Request keys being sent.
    sending: HashSet<String>,
    /// Missing-of-a-kind requests running, by kind.
    busy_kinds: HashSet<String>,
    live: Option<(String, String)>,
    more_like: Option<widgets::more_like::State>,
    /// The download picker, while open.
    picker: Option<picker::State>,
    /// `/library/artists/{name}/liked`: `(total, songs)`; None until it answers (or when it
    /// failed: the section is hidden).
    liked: Option<(usize, Vec<Value>)>,
    /// `/library/artists/{name}/tracks`, the first page: `(total, songs)`.
    songs: Option<(usize, Vec<Value>)>,
    /// Every song of the artist, after "Show all N".
    all_songs: Option<Vec<Value>>,
    loading_all: bool,
    /// The library name and mbid the two lists are for (answers for another artist are dropped).
    lib_for: (String, String),
    /// `/library/artists/{name}/one-offs` (0.36): `(total, songs)` – the first ten, or up to 500 after
    /// "Show all N"; None until it answers (or when it failed: no shelf).
    one_offs: Option<(usize, Vec<Value>)>,
    loading_one_offs: bool,
    /// The song ⋯ menu of the One-offs shelf and its dialogs.
    shared: lib::Shared,
}

impl Default for State {
    fn default() -> Self {
        Self {
            id: String::new(),
            a: None,
            groups: Vec::new(),
            filter: "all".into(),
            hide_skipped: true,
            extras: None,
            bio_open: false,
            similar: None,
            ext: None,
            expanded: HashSet::new(),
            dialog: None,
            sending: HashSet::new(),
            busy_kinds: HashSet::new(),
            live: None,
            more_like: None,
            picker: None,
            liked: None,
            songs: None,
            all_songs: None,
            loading_all: false,
            lib_for: (String::new(), String::new()),
            one_offs: None,
            loading_one_offs: false,
            shared: lib::Shared::default(),
        }
    }
}

#[derive(Clone, Debug)]
pub enum Message {
    /// `/discover/artist-lookup?name=` for an artist opened by name.
    Lookup(Result<Value, ApiError>),
    Loaded(Result<Value, ApiError>),
    Extras(Result<Value, ApiError>),
    Similar(Result<Value, ApiError>),
    Catalog(Result<Value, ApiError>),
    Filter(String),
    HideSkipped(bool),
    /// "View all" / "Show less" of a discography section, by kind.
    ViewAll(String),
    BioToggle,
    Open(Page),
    OpenUrl(String),
    /// Play every library song of the artist (shuffled when true).
    PlayAll(bool),
    LibraryTracks(bool, Result<Value, ApiError>),
    Share,
    Shared(Result<Value, ApiError>),
    PlayAlbum(String),
    AlbumTracks(Result<Value, ApiError>),
    /// A Popular row.
    PlayTop(usize),
    /// A row of the ext track list (shuffled when true).
    PlayExt(usize, bool),
    Request(String, Box<Value>),
    Requested(String, Result<Value, ApiError>),
    RequestMissing(String),
    ConfirmMissing,
    /// `(kind, [(row, status)], how many, the limit error)`.
    MissingDone(String, Vec<(usize, Value)>, usize, Option<String>),
    OpenDiscography,
    /// The artist's own release types: open the dialog, tick a box (true: an extra to skip), save, back to the
    /// setting's, and the server's answer.
    OpenTypes,
    TypeTick(bool, usize, bool),
    SaveTypes,
    ResetTypes,
    TypesSaved(bool, Result<Value, ApiError>),
    Tick(usize, bool),
    SelectAll(bool),
    SelectGroup(String, bool),
    SubmitDiscography,
    DiscographyDone(Result<Value, ApiError>),
    /// An ext release tile: its track list (or the library album when it is owned).
    OpenExt(usize),
    ExtTracks(usize, Result<Value, ApiError>),
    /// "Pick the exact download" of an ext release.
    PickExt(usize),
    /// "Add to MusicBrainz" of an ext release: ask the server for a signed link (the browser
    /// has no Encore session), then open it.
    SeedLink(usize),
    SeedLinkGot(Result<Value, ApiError>),
    Picker(picker::Message),
    /// More like this: `(artist, title, mbid)`.
    More(String, String, String),
    MoreLike(widgets::more_like::Message),
    CloseDialog,
    Live(Live),
    /// `/library/artists/{name}/liked` for (name, mbid).
    Liked((String, String), Result<Value, ApiError>),
    /// `/library/artists/{name}/tracks`, the first ten.
    Songs((String, String), Result<Value, ApiError>),
    /// "Show all N": every song, page by page.
    ShowAllSongs,
    AllSongs((String, String), Result<Vec<Value>, ApiError>),
    /// Play the liked songs from this one (Play all: 0).
    PlayLiked(usize),
    /// Play the artist's library songs from this one.
    PlaySong(usize),
    /// `/library/artists/{name}/one-offs` for (name, mbid) (0.36).
    OneOffs((String, String), Result<Value, ApiError>),
    /// "Show all N" of the One-offs shelf (up to 500).
    OneOffsAll,
    /// Play the shelf from this song.
    PlayOneOff(usize),
    /// The ⋯ of a One-off.
    OneOffMenu(usize),
    /// The song menu and its dialogs.
    Song(Act),
}

/// `/library/artists/{name}/one-offs` (0.36): the first `limit` songs of the One-offs shelf.
fn load_one_offs(ctx: &Ctx, key: (String, String), limit: usize) -> Task<Message> {
    let api = ctx.api.clone();
    let url = artist_route(&key.0, &key.1, "one-offs") + &format!("&offset=0&limit={limit}");
    Task::perform(async move { api.get(&url).await }, move |r| Message::OneOffs(key, r))
}

/// The first songs "In your library" shows.
const SONGS_FIRST: usize = 10;
/// Songs per page of "Show all N", and the most loaded.
const SONGS_PAGE: usize = 200;
const SONGS_MAX: usize = 5000;

/// `/library/artists/<name>/<tail>?name=<name>[&mbid=]` (client-shapes §3.2): the name also goes
/// in `name=`, which the server prefers. The path gets `_` in place of a name that can't stand
/// there: one with "/" or "\" (some proxies answer 404 for `%2F` in a path: "AC/DC"), a blank one
/// ("//" matches no route) and one made only of dots ("." and ".." are path steps the HTTP client
/// resolves, so the request would go to another route). The phone's rule (EncoreApi.kt `artistSegment`).
pub fn artist_route(name: &str, mbid: &str, tail: &str) -> String {
    let odd = name.trim().is_empty() || name.contains('/') || name.contains('\\') || name.trim_matches('.').is_empty();
    let seg = if odd { "_".to_string() } else { urlencode(name) };
    let mut url = format!("/library/artists/{seg}/{tail}?name={}", urlencode(name));
    if !mbid.is_empty() {
        url.push_str(&format!("&mbid={}", urlencode(mbid)));
    }
    url
}

/// `{total, tracks}` (a bare array is the whole list).
pub fn songs_answer(v: &Value) -> (usize, Vec<Value>) {
    if let Some(a) = v.as_array() {
        return (a.len(), a.clone());
    }
    let tracks = fmt::items(&v["tracks"]);
    let total = v["total"].as_u64().map(|n| n as usize).unwrap_or(tracks.len()).max(tracks.len());
    (total, tracks)
}

/// "1 song", "12 songs".
fn plural(n: usize, one: &str) -> String {
    format!("{n} {one}{}", if n == 1 { "" } else { "s" })
}

/// The web's `encore.artistFilter`: the discography filter (`all`, `missing`, `owned`).
const FILTER_KEY: &str = "encore.artistFilter";

/// Called when the app navigates here (with the route's parameters).
pub fn open(state: &mut State, id: String, ctx: &mut Ctx) -> Task<Message> {
    state.id = id.clone();
    state.a = None;
    state.groups.clear();
    state.extras = None;
    state.bio_open = false;
    state.similar = None;
    state.ext = None;
    state.dialog = None;
    state.sending.clear();
    state.busy_kinds.clear();
    state.more_like = None;
    state.picker = None;
    state.liked = None;
    state.songs = None;
    state.all_songs = None;
    state.loading_all = false;
    state.lib_for = (String::new(), String::new());
    state.one_offs = None;
    state.loading_one_offs = false;
    state.shared.reset();
    state.filter = match ctx.config.local_str(FILTER_KEY) {
        "" => "all".into(),
        f => f.to_string(),
    };
    state.hide_skipped = ctx.config.hide_skipped();
    state.live = catalog::live_key(ctx);
    if !catalog::is_mbid(&id) {
        // Library artist → full artist page (resolved through MusicBrainz), else the library filter
        let api = ctx.api.clone();
        let url = format!("/discover/artist-lookup?name={}", urlencode(&id));
        return Task::perform(async move { api.get(&url).await }, Message::Lookup);
    }
    load(state, ctx)
}

fn load(state: &State, ctx: &Ctx) -> Task<Message> {
    let api = ctx.api.clone();
    let id = state.id.clone();
    let get = |path: String, f: fn(Result<Value, ApiError>) -> Message| {
        let api = api.clone();
        Task::perform(async move { api.get(&path).await }, f)
    };
    Task::batch([
        get(format!("/discover/artist/{id}"), Message::Loaded),
        get(format!("/discover/artist/{id}/extras"), Message::Extras),
        get(format!("/discover/artist/{id}/similar"), Message::Similar),
        get(format!("/discover/artist/{id}/catalog"), Message::Catalog),
    ])
}

fn artist(state: &State) -> Option<&Value> {
    match &state.a {
        Some(Ok(a)) => Some(a),
        _ => None,
    }
}

/// The page's own grouping: the first secondary type, else the primary type, else Other.
fn page_kind(rg: &Value) -> String {
    let sec = fmt::items(&rg["secondary"]);
    if let Some(first) = sec.first() {
        first.as_str().unwrap_or("").to_string()
    } else if fmt::truthy(rg, "type") {
        fmt::s(rg, "type")
    } else {
        "Other".to_string()
    }
}

fn group(release_groups: &[Value]) -> Vec<Group> {
    let mut groups: Vec<Group> = Vec::new();
    for rg in release_groups {
        let key = page_kind(rg);
        match groups.iter_mut().find(|g| g.kind == key) {
            Some(g) => g.items.push(rg.clone()),
            None => groups.push(Group {
                kind: key,
                items: vec![rg.clone()],
            }),
        }
    }
    groups.sort_by_key(|g| catalog::rank(&g.kind));
    groups
}

/// The cover of the first plain album, the page's photo until the extras arrive.
fn first_cover(a: &Value) -> String {
    fmt::items(&a["release_groups"])
        .iter()
        .find(|r| r["type"].as_str() == Some("Album") && fmt::items(&r["secondary"]).is_empty())
        .map(|r| fmt::s(r, "cover"))
        .unwrap_or_default()
}

fn skipped(rg: &Value) -> bool {
    fmt::truthy(rg, "skipped")
}

fn owned(rg: &Value) -> bool {
    fmt::truthy(rg, "in_library")
}

fn requested(rg: &Value) -> bool {
    fmt::truthy(rg, "request_status")
}

/// `ext_body(m)`: the request body of a release only Deezer / Apple know, under the artist's name.
fn ext_body(m: &Value, name: &str) -> Value {
    let mut b = catalog::album_body(m);
    b["artist"] = Value::String(name.to_string());
    b
}

/// The year of a release: `year`, else the first four characters of `date`.
fn year_of(r: &Value) -> String {
    if fmt::truthy(r, "year") {
        fmt::s(r, "year")
    } else {
        fmt::s(r, "date").chars().take(4).collect()
    }
}

/// The rows of the Request discography dialog: every release on the page grouped by kind,
/// ticked when Settings → Release types allows it and it is not owned or requested.
fn disco_rows(on_page: &[Value]) -> Vec<DiscoRow> {
    let mut groups: Vec<(String, Vec<&Value>)> = Vec::new();
    for r in on_page {
        let k = catalog::kind_of(r);
        match groups.iter_mut().find(|(kk, _)| *kk == k) {
            Some(g) => g.1.push(r),
            None => groups.push((k, vec![r])),
        }
    }
    groups.sort_by_key(|(k, _)| catalog::rank(k));
    let mut rows = Vec::new();
    for (k, rs) in groups {
        for r in rs {
            let fixed = owned(r) || requested(r);
            let note = if owned(r) {
                "In library".to_string()
            } else if requested(r) {
                let l = fmt::s(r, "request_label");
                if l.is_empty() { "Requested".to_string() } else { l }
            } else if skipped(r) {
                format!("{} – off in Release types", fmt::s(r, "skipped"))
            } else {
                String::new()
            };
            rows.push(DiscoRow {
                id: fmt::s(r, "id"),
                kind: k.clone(),
                title: fmt::s(r, "title"),
                line: catalog::join_truthy(&[year_of(r), note], " · "),
                checked: !fixed && catalog::allowed(r),
                fixed,
            });
        }
    }
    rows
}

/// A request's outcome or a live event, applied to every row of the page that carries the key.
fn patch(state: &mut State, ev: &Live) {
    for g in state.groups.iter_mut() {
        catalog::apply_live(&mut g.items, catalog::album_key, ev);
    }
    if let Some(ext) = state.ext.as_mut() {
        catalog::apply_live(ext, catalog::album_key, ev);
    }
    if let Some(Dialog::Ext { tracks, .. }) = state.dialog.as_mut() {
        catalog::apply_live(tracks, catalog::song_key, ev);
    }
    if let Some(Ok(a)) = state.a.as_mut() {
        if let Live::Request { keys, status, label } = ev {
            if keys.contains(&format!("ar:{}", fmt::s(a, "id"))) {
                catalog::set_status(a, json!(status), json!(label));
            }
        }
    }
}

/// Request these bodies one after the other; stop at the request limit.
fn request_many(ctx: &Ctx, bodies: Vec<(usize, Value)>, done: impl Fn(Vec<(usize, Value)>, usize, Option<String>) -> Message + Send + 'static) -> Task<Message> {
    let api = ctx.api.clone();
    Task::perform(
        async move {
            let mut out = Vec::new();
            let mut ok = 0;
            let mut err = None;
            for (i, body) in bodies {
                match api.post("/requests", body).await {
                    Ok(r) => {
                        out.push((i, r["status"].clone()));
                        ok += 1;
                    }
                    Err(e) => {
                        err = Some(e.message.clone());
                        if e.message.to_lowercase().contains("limit") {
                            break;
                        }
                    }
                }
            }
            (out, ok, err)
        },
        move |(out, ok, err)| done(out, ok, err),
    )
}

pub fn update(state: &mut State, msg: Message, ctx: &mut Ctx) -> Task<Message> {
    match msg {
        Message::Lookup(r) => match r {
            Ok(v) if fmt::truthy(&v, "mbid") => ctx.replace(Page::Artist(fmt::s(&v, "mbid"))),
            _ => ctx.replace(Page::LibraryArtist(state.id.clone())),
        },
        Message::Loaded(Ok(a)) => {
            state.groups = group(&fmt::items(&a["release_groups"]));
            // the liked and library songs, when the library has songs of the artist
            let task = if fmt::f(&a, "library_tracks") > 0.0 {
                let key = (fmt::s(&a, "name"), fmt::s(&a, "id"));
                state.lib_for = key.clone();
                let api = ctx.api.clone();
                let liked = artist_route(&key.0, &key.1, "liked") + "&limit=200";
                let first = artist_route(&key.0, &key.1, "tracks") + &format!("&offset=0&limit={SONGS_FIRST}");
                let (k1, k2) = (key.clone(), key.clone());
                let api2 = api.clone();
                Task::batch([
                    Task::perform(async move { api.get(&liked).await }, move |r| Message::Liked(k1, r)),
                    Task::perform(async move { api2.get(&first).await }, move |r| Message::Songs(k2, r)),
                    load_one_offs(ctx, key, lt::ONE_OFFS_FIRST),
                ])
            } else {
                Task::none()
            };
            state.a = Some(Ok(a));
            return task;
        }
        Message::Liked(key, r) => {
            if key == state.lib_for {
                state.liked = r.ok().map(|v| songs_answer(&v));
            }
        }
        Message::Songs(key, r) => {
            if key == state.lib_for {
                state.songs = r.ok().map(|v| songs_answer(&v));
            }
        }
        Message::ShowAllSongs => {
            if state.loading_all || state.all_songs.is_some() || state.lib_for.0.is_empty() {
                return Task::none();
            }
            state.loading_all = true;
            let key = state.lib_for.clone();
            let api = ctx.api.clone();
            let (name, mbid) = key.clone();
            return Task::perform(
                async move {
                    let mut out: Vec<Value> = Vec::new();
                    while out.len() < SONGS_MAX {
                        let url = artist_route(&name, &mbid, "tracks") + &format!("&offset={}&limit={SONGS_PAGE}", out.len());
                        let (total, page) = songs_answer(&api.get(&url).await?);
                        let n = page.len();
                        out.extend(page);
                        if n < SONGS_PAGE || out.len() >= total {
                            break;
                        }
                    }
                    Ok(out)
                },
                move |r| Message::AllSongs(key, r),
            );
        }
        Message::AllSongs(key, r) => {
            if key == state.lib_for {
                state.loading_all = false;
                match r {
                    Ok(list) => state.all_songs = Some(list),
                    Err(e) => ctx.toast_err(e.message),
                }
            }
        }
        Message::PlayLiked(i) => {
            if let Some((_, list)) = &state.liked {
                if !list.is_empty() {
                    ctx.play(list.clone(), i.min(list.len() - 1));
                }
            }
        }
        Message::PlaySong(i) => {
            let list = state.all_songs.clone().or_else(|| state.songs.as_ref().map(|s| s.1.clone())).unwrap_or_default();
            if !list.is_empty() {
                ctx.play(list.clone(), i.min(list.len() - 1));
            }
        }
        Message::OneOffs(key, r) => {
            if key == state.lib_for {
                let all = state.loading_one_offs;
                state.loading_one_offs = false;
                match r {
                    Ok(v) => state.one_offs = Some(songs_answer(&v)),
                    // the first ten failing hides the shelf (an older server); Show all says why
                    Err(e) if all => ctx.toast_err(e.message),
                    Err(_) => {}
                }
            }
        }
        Message::OneOffsAll => {
            if !state.loading_one_offs && !state.lib_for.0.is_empty() {
                state.loading_one_offs = true;
                return load_one_offs(ctx, state.lib_for.clone(), lt::ONE_OFFS_ALL);
            }
        }
        Message::PlayOneOff(i) => {
            if let Some((_, list)) = &state.one_offs {
                if i < list.len() {
                    ctx.play(list.clone(), i);
                }
            }
        }
        Message::OneOffMenu(i) => {
            if let Some(t) = state.one_offs.as_ref().and_then(|(_, l)| l.get(i)).cloned() {
                return lib::track_menu(&mut state.shared, &t, ctx).map(Message::Song);
            }
        }
        Message::Song(a) => {
            let task = lib::shared_update(&mut state.shared, a, ctx).map(Message::Song);
            if state.shared.take_changed() {
                state.shared.moved_to = None;
                // a song changed or went: the shelf again
                if !state.lib_for.0.is_empty() {
                    let n = state.one_offs.as_ref().map_or(lt::ONE_OFFS_FIRST, |(_, l)| l.len().max(lt::ONE_OFFS_FIRST));
                    return Task::batch([task, load_one_offs(ctx, state.lib_for.clone(), n)]);
                }
            }
            return task;
        }
        Message::Loaded(Err(e)) => state.a = Some(Err(e.message)),
        Message::Extras(r) => state.extras = r.ok(),
        Message::Similar(r) => state.similar = r.ok(),
        Message::Catalog(r) => {
            state.ext = Some(r.map(|x| fmt::items(&x["releases"])).unwrap_or_default());
        }
        Message::Filter(f) => {
            state.filter = f.clone();
            ctx.edit_config(move |c| {
                c.local.insert(FILTER_KEY.into(), Value::String(f));
            });
        }
        Message::HideSkipped(on) => {
            state.hide_skipped = on;
            ctx.edit_config(move |c| {
                c.local.insert(crate::config::HIDE_SKIPPED.into(), Value::String(if on { "1" } else { "0" }.into()));
            });
        }
        Message::ViewAll(kind) => {
            if !state.expanded.remove(&kind) {
                state.expanded.insert(kind);
            }
        }
        Message::BioToggle => state.bio_open = !state.bio_open,
        Message::Open(p) => ctx.navigate(p),
        Message::OpenUrl(u) => ctx.open_url(u),
        Message::PlayAll(shuffle) => {
            if let Some(a) = artist(state) {
                let api = ctx.api.clone();
                let url = format!("/discover/artist/{}/library-tracks?name={}", fmt::s(a, "id"), urlencode(&fmt::s(a, "name")));
                return Task::perform(async move { api.get(&url).await }, move |r| Message::LibraryTracks(shuffle, r));
            }
        }
        Message::LibraryTracks(shuffle, Ok(v)) => {
            let tracks = fmt::items(&v);
            if tracks.is_empty() {
                ctx.toast_err("Nothing from this artist in the library yet");
            } else if shuffle {
                ctx.play_shuffled(tracks);
            } else {
                ctx.play(tracks, 0);
            }
        }
        Message::LibraryTracks(_, Err(e)) => ctx.toast_err(e.message),
        Message::Share => {
            if let Some(a) = artist(state) {
                let api = ctx.api.clone();
                let name = fmt::s(a, "name");
                return Task::perform(
                    async move { api.post("/library/artists/share", json!({ "name": name })).await },
                    Message::Shared,
                );
            }
        }
        Message::Shared(Ok(r)) => {
            let url = format!("{}{}", ctx.base(), fmt::s(&r, "path"));
            ctx.copy(url);
            ctx.toast_ok("Link copied – anyone with it can play this artist");
        }
        Message::Shared(Err(e)) => ctx.toast_err(e.message),
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
        Message::PlayTop(i) => {
            let top = state.extras.as_ref().map(|x| fmt::items(&x["top"])).unwrap_or_default();
            let playable: Vec<Value> = top.iter().filter(|t| fmt::truthy(t, "library")).map(|t| t["library"].clone()).collect();
            let pos = top[..i.min(top.len())].iter().filter(|t| fmt::truthy(t, "library")).count();
            if !playable.is_empty() {
                ctx.play(playable, pos);
            }
        }
        Message::PlayExt(i, shuffle) => {
            if let Some(Dialog::Ext { tracks, .. }) = &state.dialog {
                let queue: Vec<Value> = tracks.iter().filter(|t| fmt::truthy(t, "library")).map(|t| t["library"].clone()).collect();
                let pos = tracks[..i.min(tracks.len())].iter().filter(|t| fmt::truthy(t, "library")).count();
                if queue.is_empty() {
                    return Task::none();
                }
                if shuffle {
                    ctx.play_shuffled(queue);
                } else {
                    ctx.play(queue, pos);
                }
            }
        }
        Message::Request(key, body) => {
            if state.sending.contains(&key) {
                return Task::none();
            }
            state.sending.insert(key.clone());
            let api = ctx.api.clone();
            return Task::perform(async move { api.post("/requests", *body).await }, move |r| Message::Requested(key, r));
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
        Message::RequestMissing(kind) => {
            let hide = state.hide_skipped;
            let Some(g) = state.groups.iter().find(|g| g.kind == kind) else {
                return Task::none();
            };
            let items: Vec<(usize, Value)> = g
                .items
                .iter()
                .enumerate()
                .filter(|(_, rg)| !(hide && skipped(rg)) && !owned(rg) && !requested(rg))
                .map(|(i, rg)| (i, rg.clone()))
                .collect();
            if !items.is_empty() {
                state.dialog = Some(Dialog::Missing { kind, items });
            }
        }
        Message::ConfirmMissing => {
            let Some(Dialog::Missing { kind, items }) = state.dialog.take() else {
                return Task::none();
            };
            if state.busy_kinds.contains(&kind) {
                return Task::none();
            }
            state.busy_kinds.insert(kind.clone());
            let bodies: Vec<(usize, Value)> = items.iter().map(|(i, rg)| (*i, catalog::album_body(rg))).collect();
            return request_many(ctx, bodies, move |out, ok, err| Message::MissingDone(kind.clone(), out, ok, err));
        }
        Message::MissingDone(kind, out, ok, err) => {
            state.busy_kinds.remove(&kind);
            if let Some(g) = state.groups.iter_mut().find(|g| g.kind == kind) {
                for (i, status) in out {
                    if let Some(rg) = g.items.get_mut(i) {
                        catalog::set_status(rg, status, Value::Null);
                    }
                }
            }
            if let Some(e) = err {
                ctx.toast_err(e);
            }
            ctx.toast_ok(format!("{ok} release(s) requested"));
        }
        Message::OpenDiscography => {
            let Some(a) = artist(state) else {
                return Task::none();
            };
            if !fmt::truthy(a, "release_types") {
                // older servers have no release types: the plain request
                return submit_discography(state, ctx, None);
            }
            let on_page: Vec<Value> = state
                .groups
                .iter()
                .flat_map(|g| g.items.iter().cloned())
                .chain(state.ext.iter().flatten().cloned())
                .collect();
            state.dialog = Some(Dialog::Discography {
                rows: disco_rows(&on_page),
                sending: false,
            });
        }
        Message::Tick(i, on) => {
            if let Some(Dialog::Discography { rows, .. }) = state.dialog.as_mut() {
                if let Some(r) = rows.get_mut(i) {
                    if !r.fixed {
                        r.checked = on;
                    }
                }
            }
        }
        Message::SelectAll(on) => {
            if let Some(Dialog::Discography { rows, .. }) = state.dialog.as_mut() {
                for r in rows.iter_mut().filter(|r| !r.fixed) {
                    r.checked = on;
                }
            }
        }
        Message::SelectGroup(kind, on) => {
            if let Some(Dialog::Discography { rows, .. }) = state.dialog.as_mut() {
                for r in rows.iter_mut().filter(|r| !r.fixed && r.kind == kind) {
                    r.checked = on;
                }
            }
        }
        Message::SubmitDiscography => {
            let Some(Dialog::Discography { rows, sending }) = state.dialog.as_mut() else {
                return Task::none();
            };
            if *sending {
                return Task::none();
            }
            let ids: Vec<String> = rows.iter().filter(|r| r.checked).map(|r| r.id.clone()).collect();
            if ids.is_empty() {
                ctx.toast_err("Tick at least one release");
                return Task::none();
            }
            *sending = true;
            return submit_discography(state, ctx, Some(ids));
        }
        Message::DiscographyDone(r) => {
            state.dialog = None;
            match r {
                Ok(res) => {
                    catalog::request_toast(ctx, &res);
                    let pending = fmt::s(&res, "status") == "pending";
                    if let Some(Ok(a)) = state.a.as_mut() {
                        catalog::set_status(
                            a,
                            res["status"].clone(),
                            json!(if pending { "Awaiting approval" } else { "Discography requested" }),
                        );
                    }
                }
                Err(e) => ctx.toast_err(e.message),
            }
        }
        Message::OpenExt(i) => {
            let Some(m) = state.ext.as_ref().and_then(|e| e.get(i)).cloned() else {
                return Task::none();
            };
            // an owned single plays as a song; any other owned release opens its library album
            if let Some(id) = catalog::single_of(&m) {
                return update(state, Message::PlayAlbum(id), ctx);
            }
            if fmt::truthy(&m, "library_album_id") {
                ctx.navigate(Page::LibraryAlbum(fmt::s(&m, "library_album_id")));
                return Task::none();
            }
            let name = artist(state).map(|a| fmt::s(a, "name")).unwrap_or_default();
            let mut body: Map<String, Value> = m["ext"].as_object().cloned().unwrap_or_default();
            body.insert("artist".into(), Value::String(name));
            let api = ctx.api.clone();
            return Task::perform(
                async move { api.post("/discover/catalog/tracks", Value::Object(body)).await },
                move |r| Message::ExtTracks(i, r),
            );
        }
        Message::ExtTracks(i, r) => match r {
            Ok(tracks) => {
                if let Some(m) = state.ext.as_ref().and_then(|e| e.get(i)).cloned() {
                    state.dialog = Some(Dialog::Ext {
                        m: Box::new(m),
                        tracks: fmt::items(&tracks),
                    });
                }
            }
            Err(e) => ctx.toast_err(e.message),
        },
        Message::SeedLink(i) => {
            let Some(m) = state.ext.as_ref().and_then(|e| e.get(i)) else {
                return Task::none();
            };
            let (aid, aname) = artist(state).map(|a| (fmt::s(a, "id"), fmt::s(a, "name"))).unwrap_or_default();
            let Some(body) = catalog::mb_seed_body(m, &aid, &aname) else {
                return Task::none();
            };
            let api = ctx.api.clone();
            return Task::perform(
                async move { api.post("/contribute/mb-seed-link", body).await },
                Message::SeedLinkGot,
            );
        }
        Message::SeedLinkGot(Ok(r)) => ctx.open_url(format!("{}{}", ctx.base(), fmt::s(&r, "url"))),
        Message::SeedLinkGot(Err(e)) => ctx.toast_err(e.message),
        Message::PickExt(i) => {
            let Some(m) = state.ext.as_ref().and_then(|e| e.get(i)).cloned() else {
                return Task::none();
            };
            let name = artist(state).map(|a| fmt::s(a, "name")).unwrap_or_default();
            let b = ext_body(&m, &name);
            let opts = picker::opts(vec![
                ("kind", json!("album")),
                ("artist", json!(name)),
                ("title", m["title"].clone()),
                ("album", m["title"].clone()),
                ("year", m["year"].clone()),
                ("cover_url", m["cover"].clone()),
                ("track_count", m["tracks"].clone()),
                ("ext", b["ext"].clone()),
            ]);
            let (st, task) = picker::open(opts, ctx);
            state.picker = Some(st);
            return task.map(Message::Picker);
        }
        Message::Picker(m) => {
            let Some(p) = state.picker.as_mut() else {
                return Task::none();
            };
            let task = picker::update(p, m, ctx).map(Message::Picker);
            p.picked = None;
            if p.closed {
                state.picker = None;
            }
            return task;
        }
        Message::More(artist_name, title, mbid) => {
            let (st, task) = widgets::more_like::open(&artist_name, &title, &mbid, ctx);
            state.more_like = Some(st);
            return task.map(Message::MoreLike);
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
        Message::OpenTypes => {
            if let Some(a) = artist(state) {
                state.dialog = Some(Dialog::Types { kinds: type_boxes(a, "kinds"), skip: type_boxes(a, "skip"), own: fmt::truthy(a, "release_types_own"), sending: false });
            }
        }
        Message::TypeTick(extra, i, on) => {
            if let Some(Dialog::Types { kinds, skip, .. }) = state.dialog.as_mut() {
                if let Some(b) = if extra { skip.get_mut(i) } else { kinds.get_mut(i) } {
                    b.on = on;
                }
            }
        }
        Message::SaveTypes | Message::ResetTypes => {
            let reset = matches!(msg, Message::ResetTypes);
            let (Some(a), Some(Dialog::Types { kinds, skip, sending, .. })) = (state.a.as_ref().and_then(|a| a.as_ref().ok()), state.dialog.as_mut()) else {
                return Task::none();
            };
            let ticked = |l: &[TypeBox]| l.iter().filter(|b| b.on).map(|b| b.key.clone()).collect::<Vec<_>>();
            let body = json!({ "kinds": ticked(kinds), "skip": ticked(skip) });
            if !reset && body["kinds"].as_array().is_none_or(|k| k.is_empty()) {
                ctx.toast_err("Tick at least one kind");
                return Task::none();
            }
            if *sending {
                return Task::none();
            }
            *sending = true;
            let (api, path) = (ctx.api.clone(), format!("/discover/artist/{}/release-types", fmt::s(a, "id")));
            return Task::perform(async move { if reset { api.del(&path).await } else { api.put(&path, body).await } }, move |r| Message::TypesSaved(reset, r));
        }
        Message::TypesSaved(reset, r) => {
            state.dialog = None;
            match r {
                Ok(_) => {
                    ctx.toast_ok(if reset { "This artist uses Settings → Release types again" } else { "Release types saved for this artist" });
                    return load(state, ctx);
                }
                Err(e) => ctx.toast_err(e.to_string()),
            }
        }
        Message::CloseDialog => state.dialog = None,
        Message::Live(Live::Resync) => return load(state, ctx),
        Message::Live(ev) => patch(state, &ev),
    }
    Task::none()
}

/// `POST /api/requests {kind: artist, ...}` with the ticked releases (None: the server's types).
fn submit_discography(state: &State, ctx: &Ctx, release_ids: Option<Vec<String>>) -> Task<Message> {
    let Some(a) = artist(state) else {
        return Task::none();
    };
    let mut body = Map::new();
    body.insert("kind".into(), json!("artist"));
    catalog::copy(&mut body, "mbid", a, "id");
    catalog::copy(&mut body, "artist", a, "name");
    let cover = first_cover(a);
    if !cover.is_empty() {
        body.insert("cover_url".into(), json!(cover));
    }
    if let Some(ids) = release_ids {
        body.insert("release_ids".into(), json!(ids));
    }
    let api = ctx.api.clone();
    Task::perform(async move { api.post("/requests", Value::Object(body)).await }, Message::DiscographyDone)
}

// ----- view -------------------------------------------------------------------------------

fn request_of(state: &State, a: &Value) -> Option<Message> {
    if owned(a) || requested(a) {
        return None;
    }
    let key = catalog::album_key(a);
    (!state.sending.contains(&key)).then(|| Message::Request(key, Box::new(catalog::album_body(a))))
}

/// The "Hide skipped types" switch with its "+N hidden" chip.
fn skipped_toggle<'a>(state: &State, n: usize) -> El<'a, Message> {
    let mut r = row![toggler(state.hide_skipped)
        .label("Hide skipped types")
        .text_size(12.5)
        .size(18)
        .on_toggle(Message::HideSkipped)
        .style(theme::switch)]
    .spacing(8)
    .align_y(Alignment::Center);
    if state.hide_skipped && n > 0 {
        r = r.push(tip(
            iced::widget::button(badge(format!("+{n} hidden"), Badge::Plain))
                .padding(0)
                .style(theme::btn(Btn::Row))
                .on_press(Message::HideSkipped(false))
                .into(),
            "Releases of the kinds Settings → Release types skips",
        ));
    }
    r.into()
}

/// A tile of the artist's library albums (a single plays as a song: [`catalog::library_tile`]).
fn library_tile<'a>(ctx: &Ctx<'a>, l: &Value) -> El<'a, Message> {
    let id = fmt::s(l, "album_id");
    catalog::library_tile(ctx, &id, l, Message::Open(Page::LibraryAlbum(id.clone())), Message::PlayAlbum(id.clone()))
}

fn head<'a>(state: &'a State, ctx: &Ctx<'a>, a: &'a Value) -> El<'a, Message> {
    let release_groups = fmt::items(&a["release_groups"]);
    let n_rg = release_groups.len();
    let owned_n = release_groups.iter().filter(|r| owned(r)).count();
    let photo = state
        .extras
        .as_ref()
        .map(|x| fmt::s(x, "image"))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| first_cover(a));
    let kicker = catalog::join_truthy(
        &[
            if fmt::truthy(a, "type") { fmt::s(a, "type") } else { "Artist".into() },
            fmt::s(a, "country"),
            if fmt::truthy(a, "begin") { format!("since {}", fmt::s(a, "begin").chars().take(4).collect::<String>()) } else { String::new() },
        ],
        " · ",
    );
    let mut text = column![catalog::kicker(kicker), h1(fmt::s(a, "name"))].spacing(6);
    if fmt::truthy(a, "disambiguation") {
        text = text.push(muted(fmt::s(a, "disambiguation")).size(14));
    }
    let tags: Vec<El<'a, Message>> = fmt::items(&a["tags"])
        .iter()
        .filter_map(|t| t.as_str().map(str::to_string))
        .map(|tag| catalog::chip(tag.clone(), false, Message::Open(Page::Tag(tag))))
        .collect();
    if !tags.is_empty() {
        text = text.push(iced::widget::row(tags).spacing(6).wrap());
    }
    let mut actions = row![].spacing(8).align_y(Alignment::Center);
    if fmt::truthy(a, "library_tracks") {
        actions = actions.push(btn_icon(Icon::Play, "Play", Btn::Primary, Some(Message::PlayAll(false))));
        actions = actions.push(btn("Shuffle", Btn::Default, Some(Message::PlayAll(true))));
        actions = actions.push(btn_icon(Icon::External, "Share", Btn::Default, Some(Message::Share)));
    }
    if requested(a) {
        actions = actions.push(catalog::live_badge(&fmt::s(a, "request_status"), &fmt::s(a, "request_label")));
    } else {
        actions = actions.push(btn_icon(Icon::Download, "Request discography", Btn::Default, Some(Message::OpenDiscography)));
    }
    // the artist's own release types (admins; servers from 0.37 on)
    if ctx.is_admin() && a.get("release_types_own").is_some() {
        let label = if fmt::truthy(a, "release_types_own") { "Release types (own)" } else { "Release types" };
        actions = actions.push(btn(label, Btn::Default, Some(Message::OpenTypes)));
    }
    text = text.push(container(actions).padding([6, 0]));
    let mut facts = row![badge(
        format!("{owned_n} / {n_rg} releases in library"),
        if owned_n > 0 { Badge::Ok } else { Badge::Plain }
    )]
    .spacing(8)
    .align_y(Alignment::Center);
    if fmt::truthy(a, "library_tracks") {
        facts = facts.push(badge(format!("{} tracks", fmt::s(a, "library_tracks")), Badge::Plain));
    }
    facts = facts.push(btn_icon(
        Icon::External,
        "MusicBrainz",
        Btn::Ghost,
        Some(Message::OpenUrl(format!("https://musicbrainz.org/artist/{}", fmt::s(a, "id")))),
    ));
    if let Some(links) = a["links"].as_object() {
        for (k, u) in links.iter().filter(|(k, _)| LINK_KEYS.contains(&k.as_str())).take(6) {
            let url = u.as_str().unwrap_or("").to_string();
            facts = facts.push(btn_sm(k.clone(), Btn::Ghost, Some(Message::OpenUrl(url))));
        }
    }
    text = text.push(facts.wrap());
    row![catalog::art(ctx, &photo, 200.0, true, Icon::User), text.width(Length::Fill)]
        .spacing(26)
        .align_y(Alignment::Center)
        .into()
}

fn bio<'a>(state: &'a State, x: &'a Value) -> Option<El<'a, Message>> {
    let text = x["bio"].as_str().filter(|b| !b.is_empty())?;
    let long = text.chars().count() > BIO_CLAMP;
    let shown: String = if long && !state.bio_open {
        text.chars().take(BIO_CLAMP).collect::<String>() + "…"
    } else {
        text.to_string()
    };
    let mut foot = row![].spacing(10).align_y(Alignment::Center);
    if long {
        foot = foot.push(btn_sm(if state.bio_open { "Show less" } else { "Show more" }, Btn::Ghost, Some(Message::BioToggle)));
    }
    if let Some(u) = x["bio_url"].as_str().filter(|u| !u.is_empty()) {
        foot = foot.push(catalog::muted_link("Wikipedia", 12.5, Message::OpenUrl(u.to_string())));
    }
    Some(widgets::card(column![t(shown).size(14), foot].spacing(10)).width(Length::Fill).into())
}

fn popular<'a>(state: &'a State, ctx: &Ctx<'a>, a: &'a Value, x: &'a Value) -> Option<El<'a, Message>> {
    let top = x["top"].as_array().filter(|t| !t.is_empty())?;
    let name = fmt::s(a, "name");
    let current = ctx.player.current_id();
    let mut list = column![];
    for (i, tr) in top.iter().enumerate() {
        let has = fmt::truthy(tr, "library");
        let playing = has && current.is_some() && current == tr["library"]["id"].as_i64();
        let play: El<'a, Message> = if has {
            icon_btn(Icon::Play, "Play", Some(Message::PlayTop(i)))
        } else {
            container(small_muted((i + 1).to_string())).width(32).align_x(Alignment::Center).into()
        };
        let title = fmt::s(tr, "title");
        let full_title = catalog::join_truthy(&[title.clone(), fmt::s(tr, "version")], " ");
        let mut name_row = row![t(title).size(14).font(theme::semibold()).color(if playing { ctx.look.accent } else { ctx.look.text })].spacing(6);
        if fmt::truthy(tr, "version") {
            name_row = name_row.push(muted(fmt::s(tr, "version")).size(13));
        }
        let lines = column![name_row, small_muted(fmt::s(tr, "album")).size(12.5)].spacing(2);
        let mut r = row![play, catalog::art(ctx, &fmt::s(tr, "cover"), 40.0, false, Icon::Music), lines.width(Length::Fill)]
            .spacing(12)
            .align_y(Alignment::Center);
        if fmt::f(tr, "duration") > 0.0 {
            r = r.push(small_muted(fmt::mmss(fmt::f(tr, "duration"))));
        }
        r = r.push(icon_btn(
            Icon::Spark,
            "More like this",
            Some(Message::More(name.clone(), fmt::s(tr, "title"), String::new())),
        ));
        if has {
            r = r.push(catalog::in_library());
        } else {
            let mut j = Map::new();
            j.insert("kind".into(), json!("track"));
            j.insert("artist".into(), json!(name));
            j.insert("title".into(), json!(full_title));
            catalog::copy(&mut j, "album", tr, "album");
            catalog::copy(&mut j, "cover_url", tr, "cover");
            catalog::copy(&mut j, "length", tr, "duration");
            if fmt::truthy(tr, "isrc") {
                catalog::copy(&mut j, "isrc", tr, "isrc");
            }
            j.insert("album_mode".into(), json!("song"));
            let body = Value::Object(j);
            let key = catalog::song_key(&body);
            let on = (!state.sending.contains(&key)).then(|| Message::Request(key.clone(), Box::new(body.clone())));
            r = r.push(btn_icon(Icon::Plus, "Track", Btn::Default, on));
        }
        list = list.push(column![container(r).padding([8, 12]).width(Length::Fill), widgets::divider()]);
    }
    Some(column![section_head("Popular", None), widgets::card_flat(list)].spacing(14).into())
}

/// How many tiles a discography row holds before its section offers "View all".
const RAIL_TILES: usize = 6;

/// One tile of a discography section: a MusicBrainz release group, or a release only another
/// service lists (with its index in `State::ext`, which its messages carry).
enum DiscoTile<'a> {
    /// A MusicBrainz release group.
    Mb(&'a Value),
    /// A Deezer / Apple Music / Bandcamp / Discogs release MusicBrainz doesn't have, and its index.
    Ext(usize, &'a Value),
}

impl DiscoTile<'_> {
    /// The release behind the tile.
    fn release(&self) -> &Value {
        match self {
            DiscoTile::Mb(v) | DiscoTile::Ext(_, v) => v,
        }
    }
}

/// A release's year for the newest-first order, as the server sorts MusicBrainz's: no year first.
fn year_key(v: &Value) -> String {
    let y: String = catalog::first_truthy(v, &["year", "date"]).chars().take(4).collect();
    if y.is_empty() { "9999".to_string() } else { y }
}

/// The section a release MusicBrainz doesn't list goes in: the server's canonical `kind` (Album,
/// EP, Single, Live, Compilation …; inferred from its track count when the service gave no type),
/// else its `type`, else Album. Encore's own extras (Acoustic, Covers …) aren't sections of their
/// own: those go by their type.
fn ext_section(m: &Value) -> String {
    let (k, t) = (fmt::s(m, "kind"), fmt::s(m, "type"));
    if catalog::ORDER.contains(&k.as_str()) {
        k
    } else if catalog::ORDER.contains(&t.as_str()) {
        t
    } else {
        "Album".to_string()
    }
}

/// The discography: one section per kind (Albums, EPs, Singles, Live …), each a sideways row with
/// "View all" for the full grid. The releases only Deezer, Apple Music, Bandcamp or Discogs list
/// sit in their kind's section, tagged "Not in MusicBrainz".
fn discography<'a>(state: &'a State, ctx: &Ctx<'a>, a: &'a Value, n_rg: usize) -> El<'a, Message> {
    let f = state.filter.as_str();
    let hide = state.hide_skipped;
    let shown = |rg: &Value| !(hide && skipped(rg));
    let keep = |rg: &Value| shown(rg) && (f == "all" || (if f == "owned" { owned(rg) } else { !owned(rg) }));
    let ext: &[Value] = state.ext.as_deref().unwrap_or(&[]);
    let (aid, aname) = (fmt::s(a, "id"), fmt::s(a, "name"));
    let n_skipped: usize = state.groups.iter().map(|g| g.items.iter().filter(|rg| skipped(rg)).count()).sum::<usize>()
        + ext.iter().filter(|m| skipped(m)).count();
    let mut right = row![].spacing(10).align_y(Alignment::Center);
    if n_skipped > 0 {
        right = right.push(skipped_toggle(state, n_skipped));
    }
    right = right.push(widgets::tabs(&[("all", "All"), ("missing", "Missing"), ("owned", "In library")], f, Message::Filter));
    let head = row![
        row![h2("Discography"), small_muted(n_rg.to_string()).color(ctx.look.faint)]
            .spacing(8)
            .align_y(Alignment::Center),
        space().width(Length::Fill),
        right
    ]
    .align_y(Alignment::Center);
    let mut col = column![head].spacing(16);
    // MusicBrainz's sections, plus any kind only the other services have
    let mut kinds: Vec<String> = state.groups.iter().map(|g| g.kind.clone()).collect();
    for m in ext {
        let k = ext_section(m);
        if !kinds.contains(&k) {
            kinds.push(k);
        }
    }
    kinds.sort_by_key(|k| catalog::rank(k));
    let mut any = false;
    for kind in &kinds {
        let items: &[Value] = state.groups.iter().find(|g| g.kind == *kind).map(|g| g.items.as_slice()).unwrap_or(&[]);
        // one list, newest first like MusicBrainz's (a stable sort keeps its order within a year)
        let mut list: Vec<DiscoTile> = items.iter().filter(|rg| keep(rg)).map(DiscoTile::Mb).collect();
        list.extend(ext.iter().enumerate().filter(|(_, m)| ext_section(m) == *kind && keep(m)).map(|(i, m)| DiscoTile::Ext(i, m)));
        if list.is_empty() {
            continue;
        }
        list.sort_by_key(|t| std::cmp::Reverse(year_key(t.release())));
        any = true;
        let missing = items.iter().filter(|rg| shown(rg) && !owned(rg) && !requested(rg)).count();
        let mut gh = row![
            row![h3(format!("{kind}s")), small_muted(list.len().to_string()).color(ctx.look.faint)]
                .spacing(8)
                .align_y(Alignment::Center),
            space().width(Length::Fill)
        ]
        .spacing(8)
        .align_y(Alignment::Center);
        if missing > 0 {
            gh = gh.push(btn_icon(
                Icon::Download,
                format!("Request {missing} missing"),
                Btn::Default,
                (!state.busy_kinds.contains(kind)).then(|| Message::RequestMissing(kind.clone())),
            ));
        }
        let open = state.expanded.contains(kind);
        // "View all" only when the row is longer than a window's worth of tiles
        if open {
            gh = gh.push(btn_sm("Show less", Btn::Ghost, Some(Message::ViewAll(kind.clone()))));
        } else if list.len() > RAIL_TILES {
            gh = gh.push(btn_sm("View all", Btn::Ghost, Some(Message::ViewAll(kind.clone()))));
        }
        let tiles: Vec<El<'a, Message>> = list
            .iter()
            .map(|t| match t {
                DiscoTile::Mb(rg) => mb_tile(state, ctx, rg),
                DiscoTile::Ext(i, m) => ext_tile(state, ctx, *i, m, &aid, &aname),
            })
            .collect();
        let body = if open { grid(tiles) } else { widgets::rail(tiles) };
        col = col.push(column![gh, body].spacing(12));
    }
    if !any {
        let title = if n_rg > 0 || !ext.is_empty() {
            if f == "owned" { "Nothing from this artist in the library yet" } else { "You have everything" }
        } else {
            "No releases on MusicBrainz"
        };
        col = col.push(empty(Icon::Disc, title, ""));
    }
    col = col.push(ext_note(state));
    col.into()
}

/// The tile of a MusicBrainz release group.
fn mb_tile<'a>(state: &'a State, ctx: &Ctx<'a>, rg: &'a Value) -> El<'a, Message> {
    // MusicBrainz's name for one of several albums of one title ("Blue Album") first
    let sub = catalog::join_truthy(
        &[
            fmt::s(rg, "disambiguation"),
            catalog::first_truthy(rg, &["date", "year"]),
            if fmt::items(&rg["secondary"]).is_empty() { String::new() } else { fmt::s(rg, "type") },
        ],
        " · ",
    );
    AlbumTile {
        title: fmt::s(rg, "title"),
        sub,
        cover: fmt::s(rg, "cover"),
        corner: catalog::album_badge(rg),
        // an owned single plays as a song instead of opening the release's page
        on_open: catalog::single_of(rg).map_or_else(|| Message::Open(Page::Album(fmt::s(rg, "id"))), Message::PlayAlbum),
        on_play: fmt::truthy(rg, "library_album_id").then(|| Message::PlayAlbum(fmt::s(rg, "library_album_id"))),
        on_request: request_of(state, rg),
    }
    .view(ctx)
}

/// Under the discography: "Checking …" while `/catalog` loads, then where the releases tagged
/// "Not in MusicBrainz" came from and what requesting them does. Nothing when there are none.
fn ext_note<'a>(state: &'a State) -> El<'a, Message> {
    let Some(releases) = &state.ext else {
        return small_muted("Checking Deezer and Apple Music for releases MusicBrainz is missing…").into();
    };
    if releases.is_empty() {
        return column![].into();
    }
    let mut names: Vec<String> = Vec::new();
    for m in releases {
        for s in fmt::items(&m["sources"]) {
            let n = fmt::s(&s, "name");
            if !n.is_empty() && !names.contains(&n) {
                names.push(n);
            }
        }
    }
    let n = releases.len();
    small_muted(format!(
        "{n} release{} found only on {} are in their sections, tagged Not in MusicBrainz. Requests download from Soulseek, streamrip, YouTube Music or Bandcamp; add them to MusicBrainz with Harmony and Encore re-tags them automatically.",
        if n == 1 { "" } else { "s" },
        catalog::and_join(&names)
    ))
    .into()
}

/// The tile of a release MusicBrainz doesn't list (`i` in `State::ext`): its "Not in
/// MusicBrainz" tag (or In library / its request), Request or Play, and links to its services,
/// the MusicBrainz release editor, Harmony and the download picker under it.
fn ext_tile<'a>(state: &'a State, ctx: &Ctx<'a>, i: usize, m: &'a Value, aid: &str, aname: &str) -> El<'a, Message> {
    let corner: El<'a, Message> = if owned(m) {
        catalog::in_library()
    } else if requested(m) {
        catalog::live_badge(&fmt::s(m, "request_status"), &fmt::s(m, "request_label"))
    } else {
        badge("Not in MusicBrainz", Badge::Warn)
    };
    let sub = catalog::join_truthy(
        &[
            catalog::first_truthy(m, &["date", "year"]),
            fmt::s(m, "type"),
            if fmt::truthy(m, "tracks") { format!("{} tracks", fmt::s(m, "tracks")) } else { String::new() },
        ],
        " · ",
    );
    let key = catalog::album_key(m);
    let on_request = (!owned(m) && !requested(m) && !state.sending.contains(&key))
        .then(|| Message::Request(key.clone(), Box::new(ext_body(m, aname))));
    let tile = AlbumTile {
        title: fmt::s(m, "title"),
        sub,
        cover: fmt::s(m, "cover"),
        corner: Some(corner),
        on_open: Message::OpenExt(i),
        on_play: fmt::truthy(m, "library_album_id").then(|| Message::PlayAlbum(fmt::s(m, "library_album_id"))),
        on_request,
    }
    .view(ctx);
    let mut links: Vec<El<'a, Message>> = fmt::items(&m["sources"])
        .iter()
        .map(|s| catalog::muted_link(fmt::s(s, "name"), 12.0, Message::OpenUrl(fmt::s(s, "url"))))
        .collect();
    if catalog::mb_seed_body(m, aid, aname).is_some() {
        links.push(tip(
            catalog::muted_link("Add to MusicBrainz", 12.0, Message::SeedLink(i)),
            "Open the MusicBrainz release editor with this album filled in",
        ));
    }
    if fmt::truthy(m, "harmony") {
        links.push(tip(
            catalog::muted_link("Harmony", 12.0, Message::OpenUrl(fmt::s(m, "harmony"))),
            "Import into MusicBrainz with Harmony",
        ));
    }
    links.push(tip(catalog::muted_link("Pick", 12.0, Message::PickExt(i)), "Pick the exact download"));
    column![tile, iced::widget::row(links).spacing(8).wrap()]
        .spacing(4)
        .width(widgets::tile::WIDTH)
        .into()
}

/// A control with a tooltip under it (the web's `title`).
fn tip<'a>(el: El<'a, Message>, text: &'static str) -> El<'a, Message> {
    iced::widget::tooltip(el, widgets::tip_box(text), iced::widget::tooltip::Position::Bottom)
        .gap(4)
        .into()
}

pub fn view<'a>(state: &'a State, ctx: &Ctx<'a>) -> El<'a, Message> {
    let a = match &state.a {
        None => {
            return column![widgets::skeleton::block(Length::Fill, 220.0), widgets::skeleton_tiles(8, widgets::tile::WIDTH)]
                .spacing(24)
                .into()
        }
        Some(Err(e)) => return column![h1("Artist"), error_box(e)].spacing(12).into(),
        Some(Ok(a)) => a,
    };
    let release_groups = fmt::items(&a["release_groups"]);
    let n_rg = release_groups.len();
    let library_albums = fmt::items(&a["library_albums"]);
    let mut col = column![head(state, ctx, a)].spacing(26).width(Length::Fill);
    if let Some(x) = &state.extras {
        if let Some(b) = bio(state, x) {
            col = col.push(b);
        }
        if let Some(pp) = popular(state, ctx, a, x) {
            col = col.push(pp);
        }
    }
    if let Some(l) = liked_section(state, ctx) {
        col = col.push(l);
    }
    if let Some(l) = library_section(state, ctx, &library_albums) {
        col = col.push(l);
    }
    if let Some(l) = lt::one_offs_section(
        ctx,
        state.one_offs.as_ref(),
        state.loading_one_offs,
        Message::PlayOneOff,
        Message::OneOffMenu,
        Message::OneOffsAll,
    ) {
        col = col.push(l);
    }
    col = col.push(discography(state, ctx, a, n_rg));
    if let Some(x) = &state.similar {
        let artists = fmt::items(&x["artists"]);
        if !artists.is_empty() {
            let tiles = artists
                .iter()
                .map(|s| {
                    let on = if fmt::truthy(s, "mbid") {
                        Message::Open(Page::Artist(fmt::s(s, "mbid")))
                    } else {
                        Message::Open(Page::ArtistName(fmt::s(s, "name")))
                    };
                    catalog::artist_tile(ctx, s, String::new(), on)
                })
                .collect();
            col = col.push(
                column![
                    row![h2("Similar artists"), small_muted(fmt::s(x, "source")).color(ctx.look.faint)]
                        .spacing(8)
                        .align_y(Alignment::Center),
                    grid(tiles)
                ]
                .spacing(14),
            );
        }
    }
    // library albums MusicBrainz doesn't list (bootlegs, untagged rips ...) still show up
    let rg_ids: HashSet<String> = release_groups.iter().map(|r| fmt::s(r, "id")).collect();
    let titles: HashSet<String> = release_groups.iter().map(|r| fmt::s(r, "title").to_lowercase()).collect();
    let local_only: Vec<&Value> = library_albums
        .iter()
        .filter(|l| !(fmt::truthy(l, "rgid") && rg_ids.contains(&fmt::s(l, "rgid"))) && !titles.contains(&fmt::s(l, "album").to_lowercase()))
        .collect();
    if !local_only.is_empty() {
        let tiles = local_only.iter().map(|l| library_tile(ctx, l)).collect();
        col = col.push(
            column![
                row![h2("Only in your library"), small_muted(local_only.len().to_string()).color(ctx.look.faint)]
                    .spacing(8)
                    .align_y(Alignment::Center),
                small_muted("Albums MusicBrainz doesn't list for this artist (or not matched yet)."),
                grid(tiles)
            ]
            .spacing(10),
        );
    }
    col.into()
}

/// A song of the artist's lists: the cover, title, album · year, quality, length.
fn song_row<'a>(ctx: &Ctx<'a>, v: &Value, on_play: Message, sub: String) -> El<'a, Message> {
    let id = fmt::i(v, "id");
    TrackRow::new(crate::pages::library::tr(ctx, v, "title"))
        .sub(sub)
        .cover(ctx.cover(&fmt::s(v, "album_id"), &v["cover_v"], 80))
        .quality(fmt::quality(v))
        .duration(Some(fmt::f(v, "duration")).filter(|d| *d > 0.0))
        .playing(ctx.player.current_id() == Some(id))
        .on_play(on_play)
        .view()
}

/// "Liked from this artist": the songs the listener liked, newest first, with Play all.
fn liked_section<'a>(state: &'a State, ctx: &Ctx<'a>) -> Option<El<'a, Message>> {
    let (total, list) = state.liked.as_ref().filter(|(_, l)| !l.is_empty())?;
    let head = row![
        row![h2("Liked from this artist"), small_muted(plural(*total, "song")).color(ctx.look.faint)]
            .spacing(8)
            .align_y(Alignment::Center),
        space().width(Length::Fill),
        btn_icon(Icon::Play, "Play all", Btn::Primary, Some(Message::PlayLiked(0))),
    ]
    .align_y(Alignment::Center);
    let mut rows = column![];
    for (i, v) in list.iter().enumerate() {
        let sub = {
            let album = crate::pages::library::tr(ctx, v, "album");
            if album.is_empty() { crate::pages::library::tr(ctx, v, "artist") } else { album }
        };
        rows = rows.push(song_row(ctx, v, Message::PlayLiked(i), sub));
    }
    Some(column![head, widgets::card_flat(rows)].spacing(14).into())
}

/// "In your library": the artist's songs (the first ten, "Show all N") and albums.
fn library_section<'a>(state: &'a State, ctx: &Ctx<'a>, albums: &[Value]) -> Option<El<'a, Message>> {
    let (total, first) = state.songs.clone().unwrap_or((0, Vec::new()));
    let songs: &[Value] = match &state.all_songs {
        Some(all) => all,
        None => match &state.songs {
            Some((_, l)) => l,
            None => &[],
        },
    };
    if songs.is_empty() && albums.is_empty() {
        return None;
    }
    let counts = [
        (total > 0).then(|| plural(total, "song")),
        (!albums.is_empty()).then(|| plural(albums.len(), "album")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ");
    let mut col = column![row![h2("In your library"), small_muted(counts).color(ctx.look.faint)]
        .spacing(8)
        .align_y(Alignment::Center)]
    .spacing(14);
    if !songs.is_empty() {
        let mut head = row![h3("Songs"), space().width(Length::Fill)].align_y(Alignment::Center);
        if state.all_songs.is_none() && total > first.len() {
            head = head.push(if state.loading_all {
                btn_sm("Loading…", Btn::Ghost, None)
            } else {
                btn_sm(format!("Show all {total}"), Btn::Ghost, Some(Message::ShowAllSongs))
            });
        }
        let mut rows = column![];
        for (i, v) in songs.iter().enumerate() {
            let sub = catalog::join_truthy(&[crate::pages::library::tr(ctx, v, "album"), fmt::s(v, "year").chars().take(4).collect()], " · ");
            rows = rows.push(song_row(ctx, v, Message::PlaySong(i), sub));
        }
        col = col.push(column![head, widgets::card_flat(rows)].spacing(10));
    }
    if !albums.is_empty() {
        let tiles = albums.iter().map(|l| library_tile(ctx, l)).collect();
        let mut part = column![].spacing(10);
        if !songs.is_empty() {
            part = part.push(h3("Albums"));
        }
        col = col.push(part.push(grid(tiles)));
    }
    Some(col.into())
}

fn discography_dialog<'a>(a: &'a Value, rows: &'a [DiscoRow], sending: bool) -> El<'a, Message> {
    let p = theme::look();
    let n = rows.iter().filter(|r| r.checked).count();
    let intro = format!(
        "Every release of {} on this page, from MusicBrainz{}. Ticked to start: what Settings → Release types allows and you don't have or haven't requested yet.",
        fmt::s(a, "name"),
        if fmt::truthy(a, "catalog_merge") { ", Deezer and Apple Music" } else { "" }
    );
    let mut body = column![
        small_muted(intro),
        row![
            small_muted(format!("{} releases", rows.len())),
            space().width(Length::Fill),
            btn_sm("Select all", Btn::Ghost, Some(Message::SelectAll(true))),
            btn_sm("Deselect all", Btn::Ghost, Some(Message::SelectAll(false))),
        ]
        .spacing(6)
        .align_y(Alignment::Center)
    ]
    .spacing(10);
    let mut kinds: Vec<&str> = Vec::new();
    for r in rows {
        if !kinds.contains(&r.kind.as_str()) {
            kinds.push(&r.kind);
        }
    }
    for k in kinds {
        let in_kind: Vec<(usize, &DiscoRow)> = rows.iter().enumerate().filter(|(_, r)| r.kind == k).collect();
        let gh = row![
            row![h3(format!("{k}s")), small_muted(in_kind.len().to_string()).color(p.faint)]
                .spacing(8)
                .align_y(Alignment::Center),
            space().width(Length::Fill),
            btn_sm("Select all", Btn::Ghost, Some(Message::SelectGroup(k.to_string(), true))),
            btn_sm("Deselect all", Btn::Ghost, Some(Message::SelectGroup(k.to_string(), false))),
        ]
        .spacing(6)
        .align_y(Alignment::Center);
        let mut list = column![].spacing(4);
        for (i, r) in in_kind {
            let mut cb = checkbox(r.checked).label(r.title.as_str()).text_size(14).size(18);
            if !r.fixed {
                cb = cb.on_toggle(move |on| Message::Tick(i, on));
            }
            let mut line = row![cb].spacing(8).align_y(Alignment::Center);
            line = line.push(space().width(Length::Fill));
            if !r.line.is_empty() {
                line = line.push(small_muted(r.line.as_str()));
            }
            list = list.push(container(line).padding([2, 0]));
        }
        body = body.push(column![gh, list].spacing(6));
    }
    let label = format!("Request {n} release{}", if n == 1 { "" } else { "s" });
    let footer = row![
        btn("Cancel", Btn::Default, Some(Message::CloseDialog)),
        btn_icon(Icon::Download, label, Btn::Primary, (!sending).then_some(Message::SubmitDiscography)),
    ]
    .spacing(10);
    modal("Request discography", body, Some(footer.into()), false, Message::CloseDialog)
}

/// The artist's own release types: the kinds to request and the extras to skip (compilations, greatest hits, live …).
fn types_dialog<'a>(a: &'a Value, kinds: &'a [TypeBox], skip: &'a [TypeBox], own: bool, sending: bool) -> El<'a, Message> {
    let boxes = |l: &'a [TypeBox], extra: bool| {
        let items: Vec<El<'a, Message>> = l.iter().enumerate()
            .map(|(i, b)| container(checkbox(b.on).label(b.label.as_str()).text_size(14).size(18).on_toggle(move |on| Message::TypeTick(extra, i, on))).width(170).into())
            .collect();
        iced::widget::row(items).spacing(8).wrap()
    };
    let intro = format!("What counts for {} alone: Request discography and the hidden releases on this page. {}", fmt::s(a, "name"),
        if own { "These are the artist's own." } else { "Now: Settings → Release types." });
    let body = column![small_muted(intro), h3("Kinds to request"), boxes(kinds, false), h3("Skip releases that are"), boxes(skip, true)].spacing(10);
    let mut footer = row![].spacing(10);
    if own {
        footer = footer.push(btn("Use Settings → Release types", Btn::Ghost, (!sending).then_some(Message::ResetTypes)));
    }
    footer = footer.push(btn("Cancel", Btn::Default, Some(Message::CloseDialog)));
    footer = footer.push(btn("Save for this artist", Btn::Primary, (!sending).then_some(Message::SaveTypes)));
    modal("Release types", body, Some(footer.into()), false, Message::CloseDialog)
}

fn ext_sheet<'a>(state: &'a State, a: &'a Value, m: &'a Value, tracks: &'a [Value]) -> El<'a, Message> {
    let name = fmt::s(a, "name");
    let owned_n = tracks.iter().filter(|t| fmt::truthy(t, "library")).count();
    let body: El<'a, Message> = if tracks.is_empty() {
        muted("No track list available.").into()
    } else {
        let mut col = column![].spacing(12);
        if owned_n > 0 {
            col = col.push(
                row![
                    btn_icon(Icon::Play, "Play", Btn::Primary, Some(Message::PlayExt(0, false))),
                    btn("Shuffle", Btn::Default, Some(Message::PlayExt(0, true))),
                    small_muted(format!("{owned_n} of {} songs in your library", tracks.len())),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            );
        }
        let mut list = column![];
        for (i, tr) in tracks.iter().enumerate() {
            let has = fmt::truthy(tr, "library");
            let num: El<'a, Message> = if has {
                icon_btn(Icon::Play, "Play", Some(Message::PlayExt(i, false)))
            } else {
                container(small_muted(fmt::s(tr, "track"))).width(32).align_x(Alignment::Center).into()
            };
            let mut lines = column![t(fmt::s(tr, "title")).size(14).font(theme::semibold())].spacing(2);
            if fmt::truthy(tr, "artist") {
                lines = lines.push(small_muted(fmt::s(tr, "artist")).size(12.5));
            }
            let mut r = row![num, lines.width(Length::Fill)].spacing(12).align_y(Alignment::Center);
            if fmt::f(tr, "length") > 0.0 {
                r = r.push(small_muted(fmt::mmss(fmt::f(tr, "length"))));
            }
            if fmt::truthy(tr, "in_library") {
                r = r.push(catalog::in_library());
            } else if requested(tr) {
                r = r.push(catalog::live_badge(&fmt::s(tr, "request_status"), &fmt::s(tr, "request_label")));
            } else if !requested(m) {
                let mut j = Map::new();
                j.insert("kind".into(), json!("track"));
                j.insert("artist".into(), json!(if fmt::truthy(tr, "artist") { fmt::s(tr, "artist") } else { name.clone() }));
                catalog::copy(&mut j, "title", tr, "title");
                catalog::copy(&mut j, "album", m, "title");
                catalog::copy(&mut j, "cover_url", m, "cover");
                catalog::copy(&mut j, "year", m, "year");
                if fmt::truthy(tr, "length") {
                    catalog::copy(&mut j, "length", tr, "length");
                }
                if fmt::truthy(tr, "isrc") {
                    catalog::copy(&mut j, "isrc", tr, "isrc");
                }
                j.insert("album_mode".into(), json!("song"));
                let key = catalog::song_key(tr);
                let on = (!state.sending.contains(&key)).then(|| Message::Request(key.clone(), Box::new(Value::Object(j.clone()))));
                r = r.push(btn_icon(Icon::Plus, "Track", Btn::Default, on));
            }
            list = list.push(column![container(r).padding([8, 10]).width(Length::Fill), widgets::divider()]);
        }
        col = col.push(widgets::card_flat(list));
        col.into()
    };
    modal(fmt::s(m, "title"), body, None, false, Message::CloseDialog)
}

/// A modal or ⋯ menu to lay over the whole window, when one is open.
pub fn overlay<'a>(state: &'a State, ctx: &Ctx<'a>) -> Option<El<'a, Message>> {
    if let Some(o) = lib::shared_overlay(&state.shared, ctx) {
        return Some(o.map(Message::Song));
    }
    if let Some(p) = &state.picker {
        return Some(picker::view(p, ctx).map(Message::Picker));
    }
    if let Some(ml) = &state.more_like {
        return Some(widgets::more_like::view(ml, ctx).map(Message::MoreLike));
    }
    let a = artist(state)?;
    match state.dialog.as_ref()? {
        Dialog::Discography { rows, sending } => Some(discography_dialog(a, rows, *sending)),
        Dialog::Missing { kind, items } => {
            let title = format!("Request {} {}(s)?", items.len(), kind.to_lowercase());
            let text = items.iter().map(|(_, rg)| fmt::s(rg, "title")).collect::<Vec<_>>().join(" · ");
            let footer = row![
                btn("Cancel", Btn::Default, Some(Message::CloseDialog)),
                btn("Request", Btn::Primary, Some(Message::ConfirmMissing)),
            ]
            .spacing(10);
            Some(modal(title, muted(text).size(14), Some(footer.into()), false, Message::CloseDialog))
        }
        Dialog::Ext { m, tracks } => Some(ext_sheet(state, a, m, tracks)),
        Dialog::Types { kinds, skip, own, sending } => Some(types_dialog(a, kinds, skip, *own, *sending)),
    }
}

pub fn subscription(state: &State) -> Subscription<Message> {
    match (&state.live, state.a.is_some()) {
        (Some(key), true) => catalog::live_requests(key.clone()).map(Message::Live),
        _ => Subscription::none(),
    }
}

#[cfg(test)]
fn counts(groups: &[Group]) -> std::collections::HashMap<String, usize> {
    groups.iter().map(|g| (g.kind.clone(), g.items.len())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rg(id: &str, typ: &str, sec: &[&str]) -> Value {
        json!({ "id": id, "title": id, "artist": "A", "type": typ, "secondary": sec, "year": "2001", "cover": "" })
    }

    #[test]
    fn the_artist_routes_and_their_answers() {
        assert_eq!(artist_route("Beta Band", "", "liked"), "/library/artists/Beta%20Band/liked?name=Beta%20Band");
        assert_eq!(
            artist_route("AC/DC", "66c662b6", "tracks"),
            "/library/artists/_/tracks?name=AC%2FDC&mbid=66c662b6",
            "a name with / goes as _ in the path and whole in name="
        );
        // path steps ("%2E%2E" is ".." to the URL parser too), blank names and "\\" go as _ as well
        assert_eq!(artist_route("..", "", "liked"), "/library/artists/_/liked?name=%2E%2E");
        assert_eq!(artist_route(".", "", "tracks"), "/library/artists/_/tracks?name=%2E");
        assert_eq!(artist_route("", "", "liked"), "/library/artists/_/liked?name=");
        assert_eq!(artist_route(" ", "", "liked"), "/library/artists/_/liked?name=%20");
        assert_eq!(artist_route("a\\b", "", "liked"), "/library/artists/_/liked?name=a%5Cb");
        assert_eq!(artist_route("...And You", "", "liked"), "/library/artists/%2E%2E%2EAnd%20You/liked?name=%2E%2E%2EAnd%20You", "dots and more: as it is");
        let (total, list) = songs_answer(&json!({ "total": 143, "offset": 0, "tracks": [{ "id": 1 }, { "id": 2 }] }));
        assert_eq!((total, list.len()), (143, 2));
        assert_eq!(songs_answer(&json!([{ "id": 1 }])).0, 1, "a bare array is the whole list");
        assert_eq!(songs_answer(&json!({ "total": 0, "tracks": [] })).0, 0);
        assert_eq!(plural(1, "song"), "1 song");
        assert_eq!(plural(3, "album"), "3 albums");
    }

    #[test]
    fn groups_follow_the_release_types() {
        let g = group(&[rg("s1", "Single", &[]), rg("a1", "Album", &[]), rg("l1", "Album", &["Live"]), rg("a2", "Album", &[]), json!({ "id": "x" })]);
        let kinds: Vec<&str> = g.iter().map(|g| g.kind.as_str()).collect();
        assert_eq!(kinds, vec!["Album", "Single", "Live", "Other"]);
        assert_eq!(counts(&g)["Album"], 2);
        assert_eq!(first_cover(&json!({ "release_groups": [rg("l1", "Album", &["Live"]), { "id": "a", "type": "Album", "secondary": [], "cover": "c" }] })), "c");
    }

    #[test]
    fn dialog_rows_tick_what_the_settings_allow() {
        let rows = disco_rows(&[
            json!({ "id": "a", "title": "A", "kind": "Album", "allowed": true, "year": "2001" }),
            json!({ "id": "b", "title": "B", "kind": "Album", "allowed": true, "in_library": true }),
            json!({ "id": "c", "title": "C", "kind": "Live", "allowed": false, "skipped": "Live", "date": "1999-02-01" }),
            json!({ "id": "d", "title": "D", "kind": "Single", "request_status": "queued", "request_label": "" }),
        ]);
        assert_eq!(rows.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), vec!["a", "b", "d", "c"], "kinds in ORDER");
        assert!(rows[0].checked && !rows[0].fixed);
        assert!(!rows[1].checked && rows[1].fixed && rows[1].line == "In library");
        assert!(rows[2].fixed && rows[2].line == "Requested");
        assert!(!rows[3].checked && rows[3].line == "1999 · Live – off in Release types");
    }

    #[test]
    fn bodies_and_patches() {
        let b = ext_body(&json!({ "title": "T", "ext": { "deezer": 1 } }), "Name");
        assert_eq!(b["artist"], "Name");
        assert_eq!(b["ext"]["deezer"], 1);
        let mut st = State {
            a: Some(Ok(json!({ "id": "m1", "name": "N" }))),
            groups: group(&[rg("r1", "Album", &[])]),
            ext: Some(vec![json!({ "title": "E", "artist": "N", "ext": { "deezer": 9 } })]),
            ..State::default()
        };
        patch(
            &mut st,
            &Live::Request {
                keys: vec!["rg:r1".into(), "ext:deezer:9".into(), "ar:m1".into()],
                status: "queued".into(),
                label: "Requested".into(),
            },
        );
        assert_eq!(st.groups[0].items[0]["request_status"], "queued");
        assert_eq!(st.ext.as_ref().unwrap()[0]["request_status"], "queued");
        assert_eq!(artist(&st).unwrap()["request_label"], "Requested");
    }
}
