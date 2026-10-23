//! The pages: one module per page (the same pages as the web app, web-rs/src/pages/*.rs is the
//! behaviour reference), the [`Page`] enum that names them with their parameters, and the
//! [`Ctx`] every page gets. See clients/desktop/ARCHITECTURE.md for how a page module is built.
//!
//! Every page module has these functions (the stubs show the exact shapes; keep them):
//!
//! ```ignore
//! pub struct State { … }                       // Default
//! pub enum Message { … }                       // Clone + Debug
//! pub fn open(state: &mut State, <params>, ctx: &mut Ctx) -> Task<Message>   // navigated to
//! pub fn update(state: &mut State, msg: Message, ctx: &mut Ctx) -> Task<Message>
//! pub fn view<'a>(state: &'a State, ctx: &Ctx<'a>) -> El<'a, Message>
//! pub fn overlay<'a>(state: &'a State, ctx: &Ctx<'a>) -> Option<El<'a, Message>>  // modal or menu
//! pub fn subscription(state: &State) -> Subscription<Message>
//! ```

pub mod album;
pub mod artist;
pub mod bug_report;
pub mod bug_reports;
pub mod download;
pub mod files;
pub mod for_you;
pub mod home;
pub mod leaderboard;
pub mod library;
pub mod library_album;
pub mod library_artist;
pub mod library_types;
pub mod login;
pub mod offline;
pub mod playlist;
pub mod playlists;
pub mod requests;
pub mod search;
pub mod settings;
pub mod soulseek;
pub mod storage;
pub mod tag;

pub use crate::widgets::El;
pub use settings::Section;

use crate::api::{Api, Session};
use crate::config::Config;
use crate::player;
use crate::theme::Palette;
use crate::widgets::{Covers, ToastKind};
use iced::widget::image;
use iced::{Point, Size};
use serde_json::Value;
use std::sync::Arc;

/// Where the app is: the page and its parameters (the web's hash routes).
#[derive(Clone, Debug, PartialEq)]
pub enum Page {
    /// Server picker and sign-in; the first page.
    Login,
    Home,
    /// Discover (`#/search?q=`).
    Search { q: String },
    ForYou,
    /// `#/tag/<name>`.
    Tag(String),
    /// A catalog artist, `#/artist/<id>` (MusicBrainz id).
    Artist(String),
    /// A catalog artist looked up by name, `#/artist-name/<name>`.
    ArtistName(String),
    /// A catalog album, `#/album/<id>`.
    Album(String),
    /// `#/library?tab=<tab>&sort=&dir=&q=&filter=` ("" = albums; an empty query part = the
    /// saved choice).
    Library { tab: String, query: LibraryQuery },
    /// `#/library/album/<id>`.
    LibraryAlbum(String),
    /// The library filtered to one artist (`#/library?artist=<name>`).
    LibraryArtist(String),
    Requests,
    Playlists,
    /// `#/playlists/<id>` (Liked Songs is the playlist the server marks `liked`).
    Playlist(i64),
    /// The Downloader.
    Download,
    Soulseek,
    Leaderboard,
    Storage,
    BugReports,
    BugReport(i64),
    /// `#/settings?s=<section>`.
    Settings(Section),
    /// The songs saved on this computer (reachable without a server).
    Offline,
}

/// The Library route's optional parts: sort and direction (over the saved ones), the filter
/// text and the album filter.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LibraryQuery {
    pub sort: String,
    pub dir: String,
    pub q: String,
    pub filter: String,
    /// The Albums tab's Type (0.36, `type=`: live, one_off …; "" = All types).
    pub album_type: String,
}

impl Page {
    /// The Library on this tab ("" = albums) with the saved sort.
    pub fn library(tab: &str) -> Page {
        Page::Library {
            tab: tab.to_string(),
            query: LibraryQuery::default(),
        }
    }

    /// Which sidebar entry is lit for this page.
    pub fn nav_key(&self) -> &'static str {
        match self {
            Page::Login => "login",
            Page::Home => "home",
            Page::Search { .. } | Page::Artist(_) | Page::ArtistName(_) | Page::Album(_) => "search",
            Page::ForYou | Page::Tag(_) => "for-you",
            Page::Library { .. } | Page::LibraryAlbum(_) | Page::LibraryArtist(_) => "library",
            Page::Requests => "requests",
            Page::Playlists | Page::Playlist(_) => "playlists",
            Page::Download => "download",
            Page::Soulseek => "soulseek",
            Page::Leaderboard => "leaderboard",
            Page::Storage => "storage",
            Page::BugReports | Page::BugReport(_) => "bug-reports",
            Page::Settings(_) => "settings",
            Page::Offline => "offline",
        }
    }

    /// The web's hash route (`#/library/album/<id>`), for links the app shows or copies.
    pub fn hash(&self) -> String {
        match self {
            Page::Login => "#/login".into(),
            Page::Home => "#/".into(),
            Page::Search { q } if q.is_empty() => "#/search".into(),
            Page::Search { q } => format!("#/search?q={}", urlencode(q)),
            Page::ForYou => "#/for-you".into(),
            Page::Tag(t) => format!("#/tag/{}", urlencode(t)),
            Page::Artist(id) => format!("#/artist/{id}"),
            Page::ArtistName(n) => format!("#/artist-name/{}", urlencode(n)),
            Page::Album(id) => format!("#/album/{id}"),
            Page::Library { tab, query } => {
                let mut p = Vec::new();
                for (k, v) in [
                    ("tab", tab),
                    ("sort", &query.sort),
                    ("dir", &query.dir),
                    ("q", &query.q),
                    ("filter", &query.filter),
                    ("type", &query.album_type),
                ] {
                    if !v.is_empty() {
                        p.push(format!("{k}={}", urlencode(v)));
                    }
                }
                if p.is_empty() {
                    "#/library".into()
                } else {
                    format!("#/library?{}", p.join("&"))
                }
            }
            Page::LibraryAlbum(id) => format!("#/library/album/{id}"),
            Page::LibraryArtist(n) => format!("#/library?artist={}", urlencode(n)),
            Page::Requests => "#/requests".into(),
            Page::Playlists => "#/playlists".into(),
            Page::Playlist(id) => format!("#/playlists/{id}"),
            Page::Download => "#/download".into(),
            Page::Soulseek => "#/soulseek".into(),
            Page::Leaderboard => "#/leaderboard".into(),
            Page::Storage => "#/storage".into(),
            Page::BugReports => "#/bug-reports".into(),
            Page::BugReport(id) => format!("#/bug-reports/{id}"),
            Page::Settings(s) => format!("#/settings?s={}", s.id()),
            Page::Offline => "#/offline".into(),
        }
    }

    /// The page of a web hash route (`#/library/album/<id>`, `/settings?s=playback`), as the
    /// web's `match_route`; None for routes the app does not have.
    pub fn from_hash(hash: &str) -> Option<Page> {
        let raw = hash.trim().trim_start_matches('#');
        let raw = if raw.is_empty() { "/" } else { raw };
        let (path, query) = raw.split_once('?').unwrap_or((raw, ""));
        let q = |key: &str| -> String {
            query
                .split('&')
                .filter_map(|kv| kv.split_once('='))
                .find(|(k, _)| *k == key)
                .map(|(_, v)| urldecode(v))
                .unwrap_or_default()
        };
        let path = path.trim_end_matches('/');
        let path = if path.is_empty() { "/" } else { path };
        Some(match path {
            "/" => Page::Home,
            "/login" => Page::Login,
            "/search" => Page::Search { q: q("q") },
            "/for-you" | "/following" => Page::ForYou,
            "/library" => {
                let artist = q("artist");
                if artist.is_empty() {
                    Page::Library {
                        tab: q("tab"),
                        query: LibraryQuery {
                            sort: q("sort"),
                            dir: q("dir"),
                            q: q("q"),
                            filter: q("filter"),
                            album_type: q("type"),
                        },
                    }
                } else {
                    Page::LibraryArtist(artist)
                }
            }
            "/requests" => Page::Requests,
            "/playlists" => Page::Playlists,
            "/download" | "/activity" => Page::Download,
            "/soulseek" => Page::Soulseek,
            "/leaderboard" => Page::Leaderboard,
            "/storage" => Page::Storage,
            "/bug-reports" => Page::BugReports,
            "/offline" => Page::Offline,
            "/settings" => Page::Settings(Section::by_id(&q("s")).unwrap_or(Section::Account)),
            "/account" => Page::Settings(Section::Account),
            "/connections" => Page::Settings(Section::Connections),
            "/admin/users" => Page::Settings(Section::Users),
            "/admin/invites" => Page::Settings(Section::Invites),
            "/admin/autodownload" => Page::Settings(Section::Autodownload),
            "/admin/diagnostics" => Page::Settings(Section::Diagnostics),
            _ => {
                if let Some(r) = path.strip_prefix("/bug-reports/") {
                    return r.parse().ok().map(Page::BugReport);
                }
                if let Some(r) = path.strip_prefix("/tag/") {
                    return Some(Page::Tag(urldecode(r)));
                }
                if let Some(r) = path.strip_prefix("/artist-name/") {
                    return Some(Page::ArtistName(urldecode(r)));
                }
                if let Some(r) = path.strip_prefix("/artist/") {
                    return segment(r).map(Page::Artist);
                }
                if let Some(r) = path.strip_prefix("/album/") {
                    return segment(r).map(Page::Album);
                }
                if let Some(r) = path.strip_prefix("/library/album/") {
                    return segment(r).map(Page::LibraryAlbum);
                }
                if let Some(r) = path.strip_prefix("/playlists/") {
                    return r.parse().ok().map(Page::Playlist);
                }
                return None;
            }
        })
    }
}

fn segment(s: &str) -> Option<String> {
    (!s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'))
        .then(|| s.to_string())
}

/// `encodeURIComponent`.
pub fn urlencode(s: &str) -> String {
    percent_encoding::utf8_percent_encode(s, percent_encoding::NON_ALPHANUMERIC).to_string()
}

/// `decodeURIComponent` (lossy).
pub fn urldecode(s: &str) -> String {
    percent_encoding::percent_decode_str(&s.replace('+', " "))
        .decode_utf8_lossy()
        .to_string()
}

/// What a page asks the app to do, collected during `update` and applied by the app after.
pub enum Effect {
    Navigate(Page),
    /// Navigate without adding to the history (a redirect).
    Replace(Page),
    /// The current page's route changed without opening it again (the Library's query, so
    /// Back keeps it).
    Route(Page),
    Toast(String, ToastKind),
    /// A toast with one action (`label`, e.g. "Undo") that sends `then` to the player; it stays
    /// `ms`.
    ToastAction { text: String, label: String, then: player::Message, ms: u64 },
    /// Play these Encore track rows (JSON as the server sends them) from `index`.
    Play { tracks: Vec<Value>, index: usize },
    /// A Shuffle button's rows: played in a random order (the group's, in a group listen).
    PlayShuffled(Vec<Value>),
    PlayNext(Vec<Value>),
    Enqueue(Vec<Value>),
    /// Copy to the clipboard.
    Clipboard(String),
    /// Copy to the clipboard and say whether it worked ("Copied to clipboard" or "Copy failed –
    /// select and copy manually", the web's `ui::copy_text`).
    CopyText(String),
    /// Show a text in full in a dialog: wrapped, selectable, with a Copy button (an error or a
    /// status that a row cuts short, a request's details).
    Details {
        title: String,
        text: String,
        lead: Option<crate::widgets::details::Lead>,
        act: Option<crate::widgets::details::Act>,
        links: Box<crate::widgets::details::Links>,
    },
    /// Open in the browser.
    OpenUrl(String),
    /// Change and save the config (the look is re-applied after).
    Config(Box<dyn FnOnce(&mut Config) + Send>),
    /// The login page signed in: `user` is the server's answer.
    SignedIn(Value),
    SignOut,
    /// Fetch `/api/auth/me` again (after a profile change).
    RefreshSession,
    /// Fetch the sidebar's counts again (the bug badge after a report was read or answered).
    RefreshShell,
    /// Something for the player (open the queue, like the song …).
    Player(player::Message),
    /// Drop a cached cover (after the artwork changed).
    ForgetCover(String),
}

impl std::fmt::Debug for Effect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Effect::Navigate(p) => write!(f, "Navigate({p:?})"),
            Effect::Replace(p) => write!(f, "Replace({p:?})"),
            Effect::Route(p) => write!(f, "Route({p:?})"),
            Effect::Toast(t, k) => write!(f, "Toast({t:?}, {k:?})"),
            Effect::ToastAction { text, label, .. } => write!(f, "ToastAction({text:?}, {label:?})"),
            Effect::Play { tracks, index } => write!(f, "Play({} tracks, {index})", tracks.len()),
            Effect::PlayShuffled(t) => write!(f, "PlayShuffled({})", t.len()),
            Effect::PlayNext(t) => write!(f, "PlayNext({})", t.len()),
            Effect::Enqueue(t) => write!(f, "Enqueue({})", t.len()),
            Effect::Clipboard(s) => write!(f, "Clipboard({s:?})"),
            Effect::CopyText(s) => write!(f, "CopyText({s:?})"),
            Effect::Details { title, .. } => write!(f, "Details({title:?})"),
            Effect::OpenUrl(u) => write!(f, "OpenUrl({u:?})"),
            Effect::Config(_) => f.write_str("Config(..)"),
            Effect::SignedIn(_) => f.write_str("SignedIn"),
            Effect::SignOut => f.write_str("SignOut"),
            Effect::RefreshSession => f.write_str("RefreshSession"),
            Effect::RefreshShell => f.write_str("RefreshShell"),
            Effect::Player(m) => write!(f, "Player({m:?})"),
            Effect::ForgetCover(u) => write!(f, "ForgetCover({u:?})"),
        }
    }
}

/// What a page sees of the app, and the way it asks for things. Built by the app for every
/// `open`, `update` and `view` call; effects are drained after `update`.
pub struct Ctx<'a> {
    /// The server. Clone it into async tasks: `let api = ctx.api.clone();`.
    pub api: Arc<Api>,
    pub config: &'a Config,
    /// The signed-in user and the server's info (features, app name).
    pub session: &'a Session,
    /// The player as it is now (engine status: track, state, position, queue).
    pub player: player::Snapshot,
    pub covers: &'a Covers,
    /// The colour tokens of the active look.
    pub look: Palette,
    /// The window size (for layouts and menus).
    pub size: Size,
    /// The pointer, in window coordinates (where a right-click menu opens).
    pub cursor: Point,
    /// The page before this one (the web's previous route; a bug report sends it along).
    pub previous: Option<&'a Page>,
    effects: Vec<Effect>,
}

impl<'a> Ctx<'a> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        api: Arc<Api>,
        config: &'a Config,
        session: &'a Session,
        player: player::Snapshot,
        covers: &'a Covers,
        look: Palette,
        size: Size,
        cursor: Point,
    ) -> Self {
        Self {
            api,
            config,
            session,
            player,
            covers,
            look,
            size,
            cursor,
            previous: None,
            effects: Vec::new(),
        }
    }

    /// The page before this one.
    pub fn with_previous(mut self, previous: Option<&'a Page>) -> Self {
        self.previous = previous;
        self
    }

    /// The server's base URL ("http://host:port").
    pub fn base(&self) -> String {
        self.api.base()
    }

    pub fn is_admin(&self) -> bool {
        self.session.is_admin()
    }

    pub fn has_feature(&self, name: &str) -> bool {
        self.session.has_feature(name)
    }

    pub fn username(&self) -> &'a str {
        self.session.username()
    }

    /// A library album's cover at `size` px (the web's sizes: 400 for tiles, 80 for rows, 600 for
    /// the player). None while it loads; the app fetches it and the next frame shows it.
    pub fn cover(&self, album_id: &str, cover_v: &Value, size: u32) -> Option<image::Handle> {
        if album_id.is_empty() {
            return None;
        }
        self.covers
            .get(&crate::widgets::cover::album_url(&self.base(), album_id, cover_v, size))
    }

    /// Any picture by URL or server path (`/api/discover/artist/<id>/image`).
    pub fn picture(&self, url_or_path: &str) -> Option<image::Handle> {
        if url_or_path.is_empty() {
            return None;
        }
        self.covers
            .get(&crate::widgets::cover::absolute(&self.base(), url_or_path))
    }

    /// Whether the window is at least `px` wide (two-column layouts).
    pub fn wide(&self, px: f32) -> bool {
        self.size.width >= px
    }

    // ----- effects ----------------------------------------------------------------------

    fn push(&mut self, e: Effect) {
        self.effects.push(e);
    }

    /// `toast(msg)`.
    pub fn toast(&mut self, text: impl Into<String>) {
        self.push(Effect::Toast(text.into(), ToastKind::Info));
    }

    /// `toast(msg, "ok")`.
    pub fn toast_ok(&mut self, text: impl Into<String>) {
        self.push(Effect::Toast(text.into(), ToastKind::Ok));
    }

    /// `toast(msg, "warn")`.
    pub fn toast_warn(&mut self, text: impl Into<String>) {
        self.push(Effect::Toast(text.into(), ToastKind::Warn));
    }

    /// `toast(msg, "err")`.
    pub fn toast_err(&mut self, text: impl Into<String>) {
        self.push(Effect::Toast(text.into(), ToastKind::Err));
    }

    /// `toast_action(msg, label, ms, then)`: a toast with one button that sends `then` to the
    /// player ("Up next cleared · Undo").
    pub fn toast_action(&mut self, text: impl Into<String>, label: &str, then: player::Message, ms: u64) {
        self.push(Effect::ToastAction { text: text.into(), label: label.to_string(), then, ms });
    }

    /// Go to a page (the current one goes on the history).
    pub fn navigate(&mut self, page: Page) {
        self.push(Effect::Navigate(page));
    }

    /// Go to a page without leaving the current one in the history.
    pub fn replace(&mut self, page: Page) {
        self.push(Effect::Replace(page));
    }

    /// Change the current page's route without opening it again (what Back comes back to).
    pub fn set_route(&mut self, page: Page) {
        self.push(Effect::Route(page));
    }

    /// Play these track rows (as the server sends them) from `index`, through the engine.
    pub fn play(&mut self, tracks: Vec<Value>, index: usize) {
        self.push(Effect::Play { tracks, index });
    }

    /// A Shuffle button: these rows in a random order. In a group listen the server shuffles
    /// them, the same order for everyone (the group's queue for those who control it, else added
    /// to its Up next).
    pub fn play_shuffled(&mut self, tracks: Vec<Value>) {
        self.push(Effect::PlayShuffled(tracks));
    }

    pub fn play_next(&mut self, tracks: Vec<Value>) {
        self.push(Effect::PlayNext(tracks));
    }

    pub fn enqueue(&mut self, tracks: Vec<Value>) {
        self.push(Effect::Enqueue(tracks));
    }

    /// Copy to the clipboard (pair it with a toast).
    pub fn copy(&mut self, text: impl Into<String>) {
        self.push(Effect::Clipboard(text.into()));
    }

    /// The web's `copy_text`: copy, then toast "Copied to clipboard", or "Copy failed – select and
    /// copy manually" when the clipboard did not take it.
    pub fn copy_text(&mut self, text: impl Into<String>) {
        self.push(Effect::CopyText(text.into()));
    }

    /// Open `text` in full in the details dialog (wrapped, selectable, with Copy).
    pub fn details(&mut self, title: impl Into<String>, text: impl Into<String>) {
        self.details_with(title, text, None, None);
    }

    /// The details dialog with a few words above the text (`lead`: why the audio check turned a
    /// download away) and a button that acts on what it shows (`act`: Keep anyway).
    pub fn details_with(
        &mut self,
        title: impl Into<String>,
        text: impl Into<String>,
        lead: Option<crate::widgets::details::Lead>,
        act: Option<crate::widgets::details::Act>,
    ) {
        self.request_details(title, text, lead, act, Default::default());
    }

    /// [`Ctx::details_with`] for a request: its title and artist as links, and Play once it is
    /// in the library (`links`).
    pub fn request_details(
        &mut self,
        title: impl Into<String>,
        text: impl Into<String>,
        lead: Option<crate::widgets::details::Lead>,
        act: Option<crate::widgets::details::Act>,
        links: crate::widgets::details::Links,
    ) {
        self.push(Effect::Details {
            title: title.into(),
            text: text.into(),
            lead,
            act,
            links: Box::new(links),
        });
    }

    /// Open a link in the browser.
    pub fn open_url(&mut self, url: impl Into<String>) {
        self.push(Effect::OpenUrl(url.into()));
    }

    /// Change the config; it is saved and the look re-applied.
    pub fn edit_config(&mut self, f: impl FnOnce(&mut Config) + Send + 'static) {
        self.push(Effect::Config(Box::new(f)));
    }

    /// The login page's success.
    pub fn signed_in(&mut self, user: Value) {
        self.push(Effect::SignedIn(user));
    }

    pub fn sign_out(&mut self) {
        self.push(Effect::SignOut);
    }

    pub fn refresh_session(&mut self) {
        self.push(Effect::RefreshSession);
    }

    /// Fetch the sidebar's counts again (the web's `encore:bugs-changed`).
    pub fn refresh_shell(&mut self) {
        self.push(Effect::RefreshShell);
    }

    /// Send the player a message (open Up next, toggle like, …).
    pub fn player_msg(&mut self, m: player::Message) {
        self.push(Effect::Player(m));
    }

    pub fn forget_cover(&mut self, url: impl Into<String>) {
        self.push(Effect::ForgetCover(url.into()));
    }

    /// The effects collected so far (the app calls this after `update`).
    pub fn take_effects(&mut self) -> Vec<Effect> {
        std::mem::take(&mut self.effects)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_routes_round_trip() {
        for p in [
            Page::Home,
            Page::Search { q: "a b".into() },
            Page::ForYou,
            Page::Tag("rock".into()),
            Page::Artist("abc-1".into()),
            Page::Album("deadbeef".into()),
            Page::library("tracks"),
            Page::Library {
                tab: "albums".into(),
                query: LibraryQuery {
                    sort: "added".into(),
                    dir: "desc".into(),
                    ..Default::default()
                },
            },
            Page::LibraryAlbum("150c2e63e67a8f86".into()),
            Page::LibraryArtist("Beta Band".into()),
            Page::Playlist(3),
            Page::BugReport(7),
            Page::Settings(Section::Playback),
            Page::Offline,
        ] {
            assert_eq!(Page::from_hash(&p.hash()), Some(p.clone()), "{}", p.hash());
        }
        assert_eq!(Page::from_hash("#/"), Some(Page::Home));
        assert_eq!(Page::library("").hash(), "#/library");
        assert_eq!(
            Page::from_hash("#/library?tab=albums&sort=added&dir=desc"),
            Some(Page::Library {
                tab: "albums".into(),
                query: LibraryQuery {
                    sort: "added".into(),
                    dir: "desc".into(),
                    ..Default::default()
                }
            })
        );
        // the Albums tab's Type stays in the route (0.36)
        let live = Page::Library {
            tab: String::new(),
            query: LibraryQuery {
                album_type: "live".into(),
                ..Default::default()
            },
        };
        assert_eq!(live.hash(), "#/library?type=live");
        assert_eq!(Page::from_hash("#/library?type=live"), Some(live));
        assert_eq!(Page::from_hash("#/account"), Some(Page::Settings(Section::Account)));
        assert_eq!(Page::from_hash("#/nowhere"), None);
        assert_eq!(Page::LibraryAlbum("x".into()).nav_key(), "library");
    }
}
