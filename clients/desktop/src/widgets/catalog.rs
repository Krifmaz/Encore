//! What the Discover pages share (web-rs/src/pages/discover.rs and foryou.rs): the request
//! badges and their words, the keys a release or song is known by in request events, the
//! request bodies, the catalogue tiles and rows, and the live request stream
//! (`GET /api/requests/events`) that keeps the badges current while a page is open.

use super::{badge, icon, small_muted, t, El, Icon, Tile, TileKind};
use crate::api;
use crate::fmt;
use crate::pages::{Ctx, Page};
use crate::theme::{self, Badge, Btn};
use iced::futures::{SinkExt, Stream, StreamExt};
use iced::widget::{button, column, container, hover, image, row, space, stack};
use iced::{Alignment, ContentFit, Length, Subscription};
use serde_json::{json, Map, Value};
use std::time::Duration;

// ----- words and badges ------------------------------------------------------------------

/// `liveBadge(status, label)`: the colour and words of a request's badge. The server's label
/// wins when it sent one (Not found, Discography requested, ...).
pub fn live_status(status: &str, label: &str) -> (Badge, String) {
    let (kind, words) = match status {
        "pending" => (Badge::Warn, "Awaiting approval"),
        "queued" => (Badge::Info, "Requested"),
        "searching" => (Badge::Info, "Searching"),
        "monitoring" | "downloading" => (Badge::Accent, "Downloading"),
        "importing" => (Badge::Accent, "Importing"),
        // the worker waits (for Soulseek, or for the library to be writable again)
        "waiting" => (Badge::Warn, "Waiting"),
        "completed" => (Badge::Ok, "In library"),
        "failed" => (Badge::Err, "Failed"),
        "denied" => (Badge::Err, "Denied"),
        "cancelled" => (Badge::Plain, "Cancelled"),
        _ => (Badge::Info, "Requested"),
    };
    (kind, if label.is_empty() { words.to_string() } else { label.to_string() })
}

pub fn live_badge<'a, M: 'a>(status: &str, label: &str) -> El<'a, M> {
    let (kind, words) = live_status(status, label);
    badge(words, kind)
}

/// `statusBadge(status)` of web-rs/src/ui.rs: the words of a request row.
pub fn status_words(status: &str) -> (Badge, String) {
    match status {
        "pending" => (Badge::Warn, "Awaiting approval".into()),
        "queued" => (Badge::Info, "Queued".into()),
        "searching" => (Badge::Info, "Searching".into()),
        "monitoring" => (Badge::Accent, "Monitoring".into()),
        "downloading" => (Badge::Accent, "Downloading".into()),
        "importing" => (Badge::Accent, "Importing".into()),
        "waiting" => (Badge::Warn, "Waiting".into()),
        "completed" => (Badge::Ok, "Available".into()),
        "failed" => (Badge::Err, "Failed".into()),
        "denied" => (Badge::Err, "Denied".into()),
        "cancelled" => (Badge::Plain, "Cancelled".into()),
        other => (Badge::Plain, other.to_string()),
    }
}

pub fn status_badge<'a, M: 'a>(status: &str) -> El<'a, M> {
    let (kind, words) = status_words(status);
    badge(words, kind)
}

/// The green "In library" badge.
pub fn in_library<'a, M: 'a>() -> El<'a, M> {
    badge("In library", Badge::Ok)
}

/// `availability(item)`: In library, else the request's status, else nothing.
pub fn availability<'a, M: 'a>(item: &Value) -> Option<El<'a, M>> {
    if fmt::truthy(item, "in_library") {
        return Some(in_library());
    }
    if fmt::truthy(item, "request_status") {
        return Some(status_badge(&fmt::s(item, "request_status")));
    }
    None
}

/// `albumBadge(a)`: the corner of a catalogue album tile.
pub fn album_badge<'a, M: 'a>(a: &Value) -> Option<El<'a, M>> {
    if fmt::truthy(a, "in_library") {
        Some(in_library())
    } else if fmt::truthy(a, "request_status") {
        Some(live_badge(&fmt::s(a, "request_status"), &fmt::s(a, "request_label")))
    } else {
        None
    }
}

/// The toast after `POST /api/requests` (web-rs common.rs `submit_request`).
pub fn request_toast(ctx: &mut Ctx, r: &Value) {
    ctx.toast_ok(if fmt::s(r, "status") == "pending" {
        "Request sent – waiting for approval"
    } else {
        "Request queued"
    });
    // the web's `encore:requests-changed`: the sidebar's pending badge follows
    ctx.refresh_shell();
}

// ----- keys and small helpers -------------------------------------------------------------

/// `nk(s)`: lower case, letters and digits only.
pub fn nk(s: &str) -> String {
    s.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect()
}

const EXT_ORDER: [&str; 4] = ["deezer", "apple", "bandcamp", "discogs"];

/// A JSON scalar as JavaScript prints it ("" for null).
fn js(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// `extKey(ext)`: `ext:deezer:123`.
pub fn ext_key(ext: &Value) -> String {
    EXT_ORDER
        .iter()
        .find(|k| !js(&ext[**k]).is_empty())
        .map(|k| format!("ext:{k}:{}", js(&ext[*k])))
        .unwrap_or_default()
}

/// `albumKey(a)`: the key a release is known by in request events (server/src/req_events.rs).
pub fn album_key(a: &Value) -> String {
    if fmt::truthy(a, "ext") {
        let k = ext_key(&a["ext"]);
        if !k.is_empty() {
            return k;
        }
    }
    let id = fmt::s(a, "id");
    if !id.is_empty() && !id.starts_with("ext-") {
        format!("rg:{id}")
    } else {
        format!("a:{}|{}", nk(&fmt::s(a, "artist")), nk(&fmt::s(a, "title")))
    }
}

/// `songKey(t)`.
pub fn song_key(t: &Value) -> String {
    let id = first_truthy(t, &["recording_id", "id"]);
    if !id.is_empty() {
        format!("rec:{id}")
    } else {
        format!("t:{}|{}", nk(&fmt::s(t, "artist")), nk(&fmt::s(t, "title")))
    }
}

/// A MusicBrainz id: 36 characters of hex digits and dashes.
pub fn is_mbid(s: &str) -> bool {
    s.len() == 36 && s.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

/// `[a, b, c].filter(Boolean).join(sep)`.
pub fn join_truthy(parts: &[String], sep: &str) -> String {
    parts.iter().filter(|p| !p.is_empty()).cloned().collect::<Vec<_>>().join(sep)
}

/// `obj.a || obj.b || ...` as a string.
pub fn first_truthy(v: &Value, keys: &[&str]) -> String {
    keys.iter().find(|k| fmt::truthy(v, k)).map(|k| fmt::s(v, k)).unwrap_or_default()
}

/// `names.join(', ')` with " and " before the last one.
pub fn and_join(names: &[String]) -> String {
    match names.len() {
        0 => String::new(),
        1 => names[0].clone(),
        n => format!("{} and {}", names[..n - 1].join(", "), names[n - 1]),
    }
}

/// `{ dst: src.key }` the JSON.stringify way: a missing field is left out, null stays null.
pub fn copy(o: &mut Map<String, Value>, dst: &str, src: &Value, key: &str) {
    if let Some(v) = src.get(key) {
        o.insert(dst.to_string(), v.clone());
    }
}

/// Set a row's request status after a request was made (or an event said so).
pub fn set_status(item: &mut Value, status: Value, label: Value) {
    if let Some(o) = item.as_object_mut() {
        o.insert("request_status".into(), status);
        o.insert("request_label".into(), label);
    }
}

// ----- request bodies ---------------------------------------------------------------------

/// `albumBody(a)` with `kind: album`: a MusicBrainz release group, or a Deezer / Apple album.
pub fn album_body(a: &Value) -> Value {
    let mut j = Map::new();
    j.insert("kind".into(), json!("album"));
    if fmt::truthy(a, "ext") {
        copy(&mut j, "artist", a, "artist");
        copy(&mut j, "title", a, "title");
        copy(&mut j, "album", a, "title");
        copy(&mut j, "year", a, "year");
        copy(&mut j, "cover_url", a, "cover");
        copy(&mut j, "ext", a, "ext");
    } else {
        copy(&mut j, "mbid", a, "id");
        copy(&mut j, "artist", a, "artist");
        copy(&mut j, "title", a, "title");
        copy(&mut j, "album", a, "title");
        copy(&mut j, "year", a, "year");
        copy(&mut j, "cover_url", a, "cover");
    }
    Value::Object(j)
}

/// `songRequest(t)` with `album_mode` "song" or "album": a catalogue song (search, Popular).
pub fn song_body(t: &Value, mode: &str) -> Value {
    let mut j = Map::new();
    j.insert("kind".into(), json!("track"));
    copy(&mut j, "mbid", t, "id");
    copy(&mut j, "artist", t, "artist");
    copy(&mut j, "title", t, "title");
    copy(&mut j, "album", t, "album");
    copy(&mut j, "cover_url", t, "cover");
    copy(&mut j, "year", t, "year");
    copy(&mut j, "length", t, "length");
    copy(&mut j, "isrc", t, "isrc");
    j.insert("album_mode".into(), json!(mode));
    Value::Object(j)
}

/// foryou.js `reqBody(t)`: a recommended song.
pub fn fy_body(t: &Value) -> Value {
    let or_null = |k: &str| if fmt::truthy(t, k) { t[k].clone() } else { Value::Null };
    let mut b = Map::new();
    b.insert("kind".into(), json!("track"));
    b.insert("mbid".into(), or_null("mbid"));
    copy(&mut b, "artist", t, "artist");
    copy(&mut b, "title", t, "title");
    b.insert("album".into(), or_null("album"));
    copy(&mut b, "cover_url", t, "cover");
    copy(&mut b, "length", t, "length");
    if fmt::truthy(t, "isrc") {
        copy(&mut b, "isrc", t, "isrc");
    }
    Value::Object(b)
}

// ----- release kinds (Settings → Release types) ------------------------------------------

pub const ORDER: [&str; 15] = [
    "Album", "EP", "Single", "Compilation", "Live", "Soundtrack", "Remix", "DJ-mix", "Mixtape/Street", "Demo", "Other",
    "Broadcast", "Interview", "Spokenword", "Audiobook",
];

/// Where a kind sorts (unknown kinds last).
pub fn rank(kind: &str) -> usize {
    ORDER.iter().position(|o| *o == kind).unwrap_or(ORDER.len())
}

/// A release's group: the server's canonical `kind`, else the first secondary type, else the
/// primary type, else Other.
pub fn kind_of(rg: &Value) -> String {
    if fmt::truthy(rg, "kind") {
        return fmt::s(rg, "kind");
    }
    let sec = fmt::items(&rg["secondary"]);
    if let Some(first) = sec.first() {
        js(first)
    } else if fmt::truthy(rg, "type") {
        fmt::s(rg, "type")
    } else {
        "Other".to_string()
    }
}

/// Whether Settings → Release types wants a release: `allowed`, else "not skipped".
pub fn allowed(rg: &Value) -> bool {
    match rg.get("allowed") {
        Some(_) => fmt::truthy(rg, "allowed"),
        None => !fmt::truthy(rg, "skipped"),
    }
}

// ----- links ------------------------------------------------------------------------------

/// `artistHref(x)`: the catalogue artist by MusicBrainz id, else looked up by name.
pub fn artist_page(x: &Value) -> Page {
    if fmt::truthy(x, "artist_id") {
        Page::Artist(fmt::s(x, "artist_id"))
    } else {
        Page::ArtistName(fmt::s(x, "artist"))
    }
}

/// `albumHref(a)`: a catalogue album's page, or (Deezer / Apple only) its artist.
pub fn album_page(a: &Value) -> Page {
    if fmt::truthy(a, "ext") {
        artist_page(a)
    } else {
        Page::Album(fmt::s(a, "id"))
    }
}

/// `mbSeed(m)` for apps: the body of `POST /api/contribute/mb-seed-link`, which answers with a
/// signed link to the page that opens the MusicBrainz release editor filled in (the browser has
/// no Encore session, so the plain seed path would answer 401). None when the release has no
/// Deezer or Apple id.
pub fn mb_seed_body(m: &Value, aid: &str, aname: &str) -> Option<Value> {
    let e = &m["ext"];
    let (dz, ap) = (fmt::truthy(e, "deezer"), fmt::truthy(e, "apple"));
    let key = if dz {
        format!("deezer-{}", js(&e["deezer"]))
    } else if ap {
        format!("apple-{}", js(&e["apple"]))
    } else {
        return None;
    };
    let apple = if dz && ap { js(&e["apple"]) } else { String::new() };
    Some(json!({ "key": key, "apple": apple, "artist": aid, "artist_name": aname }))
}

/// `emptyState` with an owned title (the web's "No results for “q”").
pub fn empty_text<'a, M: 'a>(i: Icon, title: String, note: String) -> El<'a, M> {
    let p = theme::look();
    let mut c = column![icon(i, 34.0, p.faint), t(title).size(15).font(theme::semibold())]
        .spacing(10)
        .align_x(Alignment::Center);
    if !note.is_empty() {
        c = c.push(super::muted(note).size(13).center());
    }
    container(c).width(Length::Fill).padding([44, 20]).center_x(Length::Fill).into()
}

/// `.kicker`: the small line over a title (ARTIST, SONG, GENRE).
pub fn kicker<'a, M: 'a>(s: String) -> El<'a, M> {
    let p = theme::look();
    t(s.to_uppercase()).size(11.5).font(theme::semibold()).color(p.faint).into()
}

/// A genre or tag chip: a badge sized for pressing (`a.badge` with 6px 12px padding).
pub fn chip<'a, M: Clone + 'a>(label: String, accent: bool, on: M) -> El<'a, M> {
    let kind = if accent { Badge::Accent } else { Badge::Plain };
    button(container(t(label).size(13)).padding([5, 12]).style(theme::badge(kind)))
        .padding(0)
        .style(theme::btn(Btn::Row))
        .on_press(on)
        .into()
}

/// A text link: the label in the text colour, pressed like a button.
pub fn link<'a, M: Clone + 'a>(label: impl Into<String>, size: f32, on: M) -> El<'a, M> {
    let p = theme::look();
    button(t(label.into()).size(size).color(p.text).font(theme::semibold()))
        .padding([0, 2])
        .style(theme::btn(Btn::Row))
        .on_press(on)
        .into()
}

/// A muted text link (`a.muted`).
pub fn muted_link<'a, M: Clone + 'a>(label: impl Into<String>, size: f32, on: M) -> El<'a, M> {
    let p = theme::look();
    button(t(label.into()).size(size).color(p.muted))
        .padding([0, 2])
        .style(theme::btn(Btn::Row))
        .on_press(on)
        .into()
}

// ----- pictures ---------------------------------------------------------------------------

/// `art(src, label)`: the picture at `size` px, or the placeholder with an icon.
pub fn art<'a, M: 'a>(ctx: &Ctx<'a>, url: &str, size: f32, round: bool, fallback: Icon) -> El<'a, M> {
    let p = ctx.look;
    let radius = if round { size / 2.0 } else { (size / 16.0).clamp(6.0, 10.0) };
    match ctx.picture(url) {
        Some(h) => image(h)
            .width(size)
            .height(size)
            .content_fit(ContentFit::Cover)
            .border_radius(radius)
            .into(),
        None => container(icon(fallback, (size * 0.42).max(14.0), p.faint))
            .width(size)
            .height(size)
            .center(size)
            .style(move |th| {
                let mut s = theme::art(th);
                s.border.radius = radius.into();
                s
            })
            .into(),
    }
}

/// A catalogue album tile: cover, corner badge, a round Play (owned) or Request (`+`) button on
/// hover, the title and a line under it. `on_play` wins over `on_request`; both None shows no
/// button. Fill the fields, then [`AlbumTile::view`].
pub struct AlbumTile<'a, M> {
    pub title: String,
    pub sub: String,
    /// The picture's URL ("" for the placeholder).
    pub cover: String,
    pub corner: Option<El<'a, M>>,
    pub on_open: M,
    pub on_play: Option<M>,
    pub on_request: Option<M>,
}

impl<'a, M: Clone + 'a> AlbumTile<'a, M> {
    pub fn view(self, ctx: &Ctx<'a>) -> El<'a, M> {
        let p = ctx.look;
        let w = super::tile::WIDTH;
        let mut layers = stack![art(ctx, &self.cover, w, false, Icon::Disc)];
        if let Some(c) = self.corner {
            layers = layers.push(container(c).padding(8));
        }
        let mut hover_row = row![space().width(Length::Fill)].align_y(Alignment::End);
        let round = |i: Icon, on: M| {
            button(container(icon(i, 18.0, p.on_accent)).center(40))
                .padding(0)
                .style(theme::btn(Btn::Play))
                .on_press(on)
        };
        if let Some(pl) = self.on_play {
            hover_row = hover_row.push(round(Icon::Play, pl));
        } else if let Some(rq) = self.on_request {
            hover_row = hover_row.push(round(Icon::Plus, rq));
        }
        let overlay = container(hover_row).padding(10).width(w).height(w).align_y(Alignment::End);
        let with_hover: El<'a, M> = hover(layers, overlay);
        let mut lines = column![t(self.title).size(14).font(theme::semibold()).width(w)].spacing(2);
        if !self.sub.is_empty() {
            lines = lines.push(small_muted(self.sub).size(13).width(w));
        }
        button(column![with_hover, lines].spacing(9).width(w))
            .padding(0)
            .style(theme::btn(Btn::Row))
            .on_press(self.on_open)
            .into()
    }
}

/// A round artist tile (search results, similar artists, genre pages): the picture when the
/// server has one, the name, a line under it, "In library" in the corner.
pub fn artist_tile<'a, M: Clone + 'a>(ctx: &Ctx<'a>, a: &Value, sub: String, on_open: M) -> El<'a, M> {
    let name = fmt::s(a, "name");
    let image = fmt::s(a, "image");
    let mut tile = Tile::new(if name.is_empty() { "?".to_string() } else { name }, TileKind::Artist, on_open)
        .sub(sub)
        .cover(if image.is_empty() { None } else { ctx.picture(&image) });
    if fmt::truthy(a, "in_library") {
        tile = tile.corner(in_library());
    }
    tile.view()
}

/// A library album tile (`libraryTile`): the cover from the server, the album and the artist.
/// `a` is the library row (`album`, `albumartist`, `year`, `cover_v`); `id` its album id.
pub fn library_tile<'a, M: Clone + 'a>(ctx: &Ctx<'a>, id: &str, a: &Value, on_open: M, on_play: M) -> El<'a, M> {
    let album = fmt::s(a, "album");
    let title = if album.is_empty() { "Unknown album".to_string() } else { album };
    let sub = join_truthy(&[fmt::s(a, "albumartist"), fmt::s(a, "year")], " · ");
    Tile::new(title, TileKind::Album, on_open)
        .sub(sub)
        .cover(ctx.cover(id, &a["cover_v"], 400))
        .play(on_play)
        .view()
}

// ----- rows -------------------------------------------------------------------------------

/// What a catalogue song row can do.
pub struct SongRow<M> {
    /// The song is in the library: play it (the thumb).
    pub play: Option<M>,
    pub open_artist: M,
    /// The album's catalogue page (when the song has a release group).
    pub open_album: Option<M>,
    /// "Song" and "+ Album"; None while the request is being sent.
    pub request_song: Option<M>,
    pub request_album: Option<M>,
}

/// discover.js `trackRow(t)`: a catalogue song with its availability or request buttons.
pub fn song_row<'a, M: Clone + 'a>(ctx: &Ctx<'a>, tr: &Value, m: SongRow<M>) -> El<'a, M> {
    let p = ctx.look;
    let cover = fmt::s(tr, "cover");
    let thumb: El<'a, M> = match m.play {
        Some(pl) => button(
            stack![
                art(ctx, &cover, 44.0, false, Icon::Music),
                container(icon(Icon::Play, 16.0, p.text)).center(44)
            ],
        )
        .padding(0)
        .style(theme::btn(Btn::Row))
        .on_press(pl)
        .into(),
        None => art(ctx, &cover, 44.0, false, Icon::Music),
    };
    let mut meta = row![link(fmt::s(tr, "artist"), 12.5, m.open_artist)]
        .spacing(4)
        .align_y(Alignment::Center);
    if fmt::truthy(tr, "album") {
        meta = meta.push(small_muted("·"));
        meta = meta.push(match m.open_album {
            Some(oa) => muted_link(fmt::s(tr, "album"), 12.5, oa),
            None => small_muted(fmt::s(tr, "album")).size(12.5).into(),
        });
    }
    if fmt::truthy(tr, "year") {
        meta = meta.push(small_muted(format!("· {}", fmt::s(tr, "year"))).size(12.5));
    }
    let lines = column![t(fmt::s(tr, "title")).size(14).font(theme::semibold()), meta].spacing(2);
    let mut r = row![thumb, lines.width(Length::Fill)].spacing(14).align_y(Alignment::Center);
    if fmt::truthy(tr, "length") {
        r = r.push(small_muted(fmt::mmss(fmt::f(tr, "length"))).color(p.faint));
    }
    if fmt::truthy(tr, "in_library") {
        if let Some(a) = availability(tr) {
            r = r.push(a);
        }
    } else if fmt::truthy(tr, "request_status") {
        r = r.push(live_badge(&fmt::s(tr, "request_status"), &fmt::s(tr, "request_label")));
    } else {
        r = r.push(
            row![
                super::btn_icon(Icon::Plus, "Song", Btn::Primary, m.request_song),
                super::btn_icon(Icon::Disc, "+ Album", Btn::Default, m.request_album),
            ]
            .spacing(6),
        );
    }
    column![container(r).padding([9, 14]).width(Length::Fill), super::divider()].into()
}

/// What a recommended song row can do.
pub struct FyRow<M> {
    pub play: Option<M>,
    pub open_artist: M,
    /// "More like this".
    pub more: M,
    /// "Request"; None while it is being sent or when the row cannot be requested.
    pub request: Option<M>,
}

/// foryou.js `trackRow(t, i)`: play it when it is in the library, request it otherwise.
pub fn fy_track_row<'a, M: Clone + 'a>(ctx: &Ctx<'a>, tr: &Value, playing: bool, m: FyRow<M>) -> El<'a, M> {
    let p = ctx.look;
    let color = if playing { p.accent } else { p.text };
    let play: El<'a, M> = match m.play {
        Some(pl) => super::icon_btn(Icon::Play, "Play", Some(pl)),
        None => space().width(32).into(),
    };
    let mut meta = row![link(fmt::s(tr, "artist"), 12.5, m.open_artist)]
        .spacing(4)
        .align_y(Alignment::Center);
    if fmt::truthy(tr, "album") {
        meta = meta.push(small_muted(format!("· {}", fmt::s(tr, "album"))).size(12.5));
    }
    let lines = column![t(fmt::s(tr, "title")).size(14).font(theme::semibold()).color(color), meta].spacing(2);
    let mut r = row![play, art(ctx, &fmt::s(tr, "cover"), 40.0, false, Icon::Music), lines.width(Length::Fill)]
        .spacing(12)
        .align_y(Alignment::Center);
    if fmt::truthy(tr, "length") {
        r = r.push(small_muted(fmt::mmss(fmt::f(tr, "length"))));
    }
    r = r.push(super::icon_btn(Icon::Spark, "More like this", Some(m.more)));
    if fmt::truthy(tr, "library") {
        r = r.push(in_library());
    } else if fmt::truthy(tr, "request_status") {
        r = r.push(status_badge(&fmt::s(tr, "request_status")));
    } else {
        r = r.push(super::btn_icon(Icon::Plus, "Request", Btn::Default, m.request));
    }
    column![container(r).padding([8, 12]).width(Length::Fill), super::divider()].into()
}

// ----- the live request stream ----------------------------------------------------------

/// What `GET /api/requests/events` says while a page is open.
#[derive(Clone, Debug, PartialEq)]
pub enum Live {
    /// A request changed: every row whose key is in `keys` shows `status` (and `label`).
    Request {
        keys: Vec<String>,
        status: String,
        label: String,
    },
    /// The stream is back after a drop (server restart, network): the events in between were
    /// missed, so the page should reload its statuses.
    Resync,
}

/// The stream's identity: the server and the session cookie (None before a sign-in).
pub fn live_key(ctx: &Ctx) -> Option<(String, String)> {
    let base = ctx.api.base();
    let cookie = ctx.api.session_cookie()?;
    (!base.is_empty()).then_some((base, cookie))
}

/// The subscription a page returns while it is open; the pages map it to their own message.
pub fn live_requests(key: (String, String)) -> Subscription<Live> {
    Subscription::run_with(key, live_stream)
}

fn live_stream(key: &(String, String)) -> impl Stream<Item = Live> {
    let (base, cookie) = key.clone();
    iced::stream::channel(64, async move |mut out: iced::futures::channel::mpsc::Sender<Live>| {
        let Ok(client) = reqwest::Client::builder()
            .user_agent(api::user_agent())
            .connect_timeout(Duration::from_secs(8))
            .build()
        else {
            return;
        };
        let mut dropped = false;
        let mut wait = 3u64;
        loop {
            let sent = client
                .get(format!("{base}/api/requests/events"))
                .header("Accept", "text/event-stream")
                .header("Cookie", format!("{}={cookie}", api::COOKIE))
                .send()
                .await;
            match sent {
                Ok(res) if res.status().is_success() => {
                    wait = 3;
                    let mut body = res.bytes_stream();
                    let mut buf: Vec<u8> = Vec::new();
                    while let Some(Ok(chunk)) = body.next().await {
                        buf.extend_from_slice(&chunk);
                        while let Some(end) = buf.windows(2).position(|w| w == b"\n\n") {
                            let block: Vec<u8> = buf.drain(..end + 2).collect();
                            let text = String::from_utf8_lossy(&block);
                            if let Some(ev) = parse_sse(&text) {
                                match ev {
                                    Live::Resync => {
                                        if dropped {
                                            dropped = false;
                                            if out.send(Live::Resync).await.is_err() {
                                                return;
                                            }
                                        }
                                    }
                                    other => {
                                        if out.send(other).await.is_err() {
                                            return;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                Ok(res) if res.status().as_u16() == 401 => {
                    // the session is gone; the app signs out on its next request
                    wait = 30;
                }
                _ => {}
            }
            dropped = true;
            tokio::time::sleep(Duration::from_secs(wait)).await;
            wait = (wait * 2).min(30);
        }
    })
}

/// One server-sent event block (`event:` and `data:` lines) as a [`Live`]: `hello` is
/// [`Live::Resync`] (the caller decides whether it follows a drop); anything else is None.
pub fn parse_sse(block: &str) -> Option<Live> {
    let mut event = String::new();
    let mut data = String::new();
    for line in block.lines() {
        if let Some(v) = line.strip_prefix("event:") {
            event = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(v.trim_start());
        }
    }
    match event.as_str() {
        "hello" => Some(Live::Resync),
        "request" => {
            let v: Value = serde_json::from_str(&data).ok()?;
            Some(Live::Request {
                keys: fmt::items(&v["keys"]).iter().map(js).collect(),
                status: fmt::s(&v, "status"),
                label: fmt::s(&v, "label"),
            })
        }
        _ => None,
    }
}

/// Apply a request event to rows: the ones whose key is in the event get its status. Returns
/// whether anything changed.
pub fn apply_live(items: &mut [Value], key: impl Fn(&Value) -> String, ev: &Live) -> bool {
    let Live::Request { keys, status, label } = ev else {
        return false;
    };
    let mut changed = false;
    for it in items.iter_mut() {
        if keys.contains(&key(it)) {
            set_status(it, json!(status), json!(label));
            changed = true;
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_match_the_web() {
        assert_eq!(album_key(&json!({ "id": "abc", "artist": "X", "title": "Y" })), "rg:abc");
        assert_eq!(album_key(&json!({ "id": "ext-d1", "artist": "Travis Scott", "title": "Rodeo (Deluxe)" })), "a:travisscott|rodeodeluxe");
        assert_eq!(album_key(&json!({ "title": "T", "artist": "A", "ext": { "apple": 7, "deezer": 3 } })), "ext:deezer:3");
        assert_eq!(album_key(&json!({ "title": "T", "artist": "A", "ext": { "apple": "7" } })), "ext:apple:7");
        assert_eq!(song_key(&json!({ "recording_id": "r1", "id": "x" })), "rec:r1");
        assert_eq!(song_key(&json!({ "artist": "The Band", "title": "Go!" })), "t:theband|go");
        assert!(is_mbid("aaaaaaaa-1111-2222-3333-444444444444") && !is_mbid("Beta Band"));
    }

    #[test]
    fn words_and_joins() {
        assert_eq!(live_status("queued", ""), (Badge::Info, "Requested".to_string()));
        assert_eq!(live_status("completed", "").1, "In library");
        assert_eq!(live_status("failed", "Not found").1, "Not found");
        assert_eq!(status_words("completed").1, "Available");
        assert_eq!(and_join(&["Deezer".into(), "Apple Music".into(), "Bandcamp".into()]), "Deezer, Apple Music and Bandcamp");
        assert_eq!(and_join(&["Deezer".into()]), "Deezer");
        assert_eq!(join_truthy(&["2015".into(), String::new(), "Live".into()], " · "), "2015 · Live");
        assert_eq!(first_truthy(&json!({ "date": "", "year": "2001" }), &["date", "year"]), "2001");
    }

    #[test]
    fn kinds_follow_the_settings() {
        assert_eq!(kind_of(&json!({ "kind": "Live", "type": "Album" })), "Live");
        assert_eq!(kind_of(&json!({ "type": "Album", "secondary": ["Compilation"] })), "Compilation");
        assert_eq!(kind_of(&json!({ "type": "EP", "secondary": [] })), "EP");
        assert_eq!(kind_of(&json!({})), "Other");
        assert!(rank("Album") < rank("Single") && rank("Single") < rank("Nonsense"));
        assert!(allowed(&json!({ "allowed": true, "skipped": "Live" })));
        assert!(!allowed(&json!({ "skipped": "Live" })) && allowed(&json!({})));
    }

    #[test]
    fn bodies_leave_missing_fields_out() {
        let b = album_body(&json!({ "id": "rg1", "artist": "A", "title": "T", "cover": null }));
        assert_eq!(b["kind"], "album");
        assert_eq!(b["mbid"], "rg1");
        assert_eq!(b["album"], "T");
        assert!(b["cover_url"].is_null() && b.get("year").is_none());
        let e = album_body(&json!({ "title": "T", "ext": { "deezer": 1 } }));
        assert!(e.get("mbid").is_none() && e["ext"]["deezer"] == 1);
        let s = song_body(&json!({ "id": "rec1", "title": "S", "artist": "A", "length": 200.0 }), "album");
        assert_eq!(s["album_mode"], "album");
        assert_eq!(s["mbid"], "rec1");
        let f = fy_body(&json!({ "title": "S", "artist": "A", "album": "", "isrc": "" }));
        assert!(f["mbid"].is_null() && f["album"].is_null() && f.get("isrc").is_none());
    }

    #[test]
    fn sse_blocks_parse() {
        assert_eq!(parse_sse("event: hello\ndata: {\"type\":\"hello\"}\n\n"), Some(Live::Resync));
        let ev = parse_sse("event: request\ndata: {\"keys\":[\"rg:1\",\"a:x|y\"],\"status\":\"queued\",\"label\":\"Requested\"}\n\n");
        assert_eq!(
            ev,
            Some(Live::Request {
                keys: vec!["rg:1".into(), "a:x|y".into()],
                status: "queued".into(),
                label: "Requested".into()
            })
        );
        assert_eq!(parse_sse(": ping\n\n"), None);
        let mut rows = vec![json!({ "id": "1", "title": "A" }), json!({ "id": "2", "title": "B" })];
        assert!(apply_live(&mut rows, album_key, ev.as_ref().unwrap()));
        assert_eq!(rows[0]["request_status"], "queued");
        assert!(rows[1].get("request_status").is_none());
    }

    #[test]
    fn links_and_seeds() {
        assert_eq!(artist_page(&json!({ "artist_id": "m1", "artist": "A" })), Page::Artist("m1".into()));
        assert_eq!(artist_page(&json!({ "artist": "A B" })), Page::ArtistName("A B".into()));
        assert_eq!(album_page(&json!({ "id": "rg1" })), Page::Album("rg1".into()));
        assert_eq!(album_page(&json!({ "id": "ext-1", "ext": { "deezer": 1 }, "artist": "A" })), Page::ArtistName("A".into()));
        let seed = mb_seed_body(&json!({ "ext": { "deezer": 5, "apple": 9 } }), "aid", "A B");
        assert_eq!(seed, Some(json!({ "key": "deezer-5", "apple": "9", "artist": "aid", "artist_name": "A B" })));
        let apple = mb_seed_body(&json!({ "ext": { "apple": "7" } }), "", "A");
        assert_eq!(apple.map(|b| b["key"].clone()), Some(json!("apple-7")));
        assert_eq!(mb_seed_body(&json!({ "ext": { "bandcamp": "x" } }), "aid", "A"), None);
    }
}
