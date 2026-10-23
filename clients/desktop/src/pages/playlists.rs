//! Playlists: Your playlists, imports and Liked Songs. Behaviour reference: web-rs/src/pages/playlists.rs render.
//!
//! The grid of playlist tiles (Liked Songs first), New, Import (the dialog with "From a link" and
//! "Paste a list", a preview, Download missing tracks and the album choice) and Merge of the
//! selected playlists. The import dialog is shared with the playlist page ("Add from link").
//!
//! "Add a shared playlist" takes a pasted share link of someone on this server
//! (`POST /api/playlists/shared {link}`): their playlist, live and read-only, joins the grid as a
//! `kind: linked` row ("12 tracks · by anna", or "anna stopped sharing this playlist" once the link
//! ended). The texts are the web's (web-rs/src/pages/playlists.rs).

use crate::api::ApiError;
use crate::fmt;
use crate::pages::{Ctx, El, Page};
use crate::theme::{self, Badge, Btn};
use crate::widgets::choice::select;
use crate::widgets::text_area::text_area;
use crate::widgets::{
    self, badge, btn, btn_icon, btn_sm, card, empty, error_box, field, grid, icon, modal, muted, page_head, skeleton_tiles, small,
    small_muted, t, Icon, Tile, TileKind,
};
use iced::widget::{checkbox, column, container, row, text_editor, text_input, toggler};
use iced::{Alignment, Length, Subscription, Task};
use serde_json::{json, Value};

/// `SRC[k]`: the display name of a playlist source.
pub fn src_name(k: &str) -> Option<&'static str> {
    Some(match k {
        "spotify" => "Spotify",
        "ytmusic" => "YouTube Music",
        "deezer" => "Deezer",
        "tidal" => "Tidal",
        "qobuz" => "Qobuz",
        "apple" => "Apple Music",
        "amazon" => "Amazon Music",
        "ytdlp" => "YouTube / SoundCloud",
        "listenbrainz" => "ListenBrainz",
        "web" => "Web",
        "text" => "Pasted list",
        "merged" => "Merged",
        "manual" | "library" => "Encore",
        "jellyfin" => "Jellyfin",
        "plex" => "Plex",
        "navidrome" => "Navidrome",
        "share" => LINKED_SOURCE,
        _ => return None,
    })
}

// ----- someone else's playlist, added from its share link ----------------------------------

pub const ADD_SHARED_TITLE: &str = "Add a shared playlist";
pub const ADD_SHARED_HINT: &str = "Paste a playlist link from someone on this server. You get their playlist as it is now, and it changes when they change it.";
pub const ADD_SHARED_PLACEHOLDER: &str = "https://…/s/…";
pub const ADD_TO_MINE: &str = "Add to my playlists";
pub const ADDED_TO_MINE: &str = "Added to your playlists";
pub const ALREADY_MINE: &str = "Already in your playlists";
pub const REMOVE_FROM_MINE: &str = "Remove from my playlists";
pub const REMOVED_FROM_MINE: &str = "Removed from your playlists";
pub const SAVED_COPY: &str = "Saved to your playlists";
pub const SAVE_COPY: &str = "Save a copy";
pub const LINKED_SOURCE: &str = "Added from a link";
pub const LINKED_EMPTY: &str = "No songs here right now";

/// Someone else's playlist added from its share link (`kind: linked`): read-only.
pub fn is_linked(p: &Value) -> bool {
    p["kind"].as_str() == Some("linked")
}

/// Whether an added playlist's link still works (`linked.active`).
pub fn linked_active(p: &Value) -> bool {
    fmt::truthy(&p["linked"], "active")
}

/// The owner of an added playlist (`linked.owner`).
pub fn linked_owner(p: &Value) -> String {
    fmt::s(&p["linked"], "owner")
}

/// "Playlist · by anna" / "Liked songs · by anna".
pub fn linked_kicker(p: &Value) -> String {
    let what = if fmt::truthy(&p["linked"], "liked") { "Liked songs" } else { "Playlist" };
    format!("{what} · by {}", linked_owner(p))
}

/// The line under the count while the link works.
pub fn linked_hint(owner: &str) -> String {
    format!("Changes {owner} makes show here. Save a copy to make your own.")
}

/// An added playlist once its link ended.
pub fn linked_stopped(owner: &str) -> String {
    format!("{owner} stopped sharing this playlist")
}

/// The tile's first line: "12 tracks · by anna", or the ended line.
pub fn linked_tile_line(p: &Value) -> String {
    if linked_active(p) {
        format!("{} tracks · by {}", fmt::i(p, "total"), linked_owner(p))
    } else {
        linked_stopped(&linked_owner(p))
    }
}

/// The Remove from my playlists confirm.
pub fn remove_text(name: &str) -> String {
    format!("Remove “{name}” from your playlists? You can add it again from its link.")
}

/// The owner's count: "" / "added by 1 person" / "added by 3 people" (never who).
pub fn added_by_text(n: i64) -> String {
    match n {
        n if n <= 0 => String::new(),
        1 => "added by 1 person".into(),
        n => format!("added by {n} people"),
    }
}

/// The playlists songs can be added to: not someone else's added from a link.
pub fn can_add_to(p: &Value) -> bool {
    !fmt::truthy(p, "read_only") && !is_linked(p)
}

/// The toast after Add to my playlists (a second add does nothing new).
pub fn add_toast(r: &Value) -> &'static str {
    if fmt::truthy(r, "added") {
        ADDED_TO_MINE
    } else {
        ALREADY_MINE
    }
}

/// `TARGETS[k]`: the display name of a sync target.
pub fn target_name(k: &str) -> Option<&'static str> {
    Some(match k {
        "jellyfin" => "Jellyfin / Finamp",
        "plex" => "Plex / Plexamp",
        "navidrome" => "Navidrome",
        "ytmusic" => "YouTube Music",
        "listenbrainz" => "ListenBrainz",
        _ => return None,
    })
}

/// `SRC[p.source] || p.source || ''`.
pub fn src_label(p: &Value) -> String {
    let source = fmt::s(p, "source");
    src_name(&source).map(str::to_string).unwrap_or(source)
}

/// The album choice of an import: `(value sent as album_mode, label)`.
pub const ALBUM_MODES: [(&str, &str); 3] = [
    ("", "Albums: admin default"),
    ("song", "Just the songs"),
    ("album", "Whole albums"),
];

// ----- the Import dialog (shared with the playlist page) ----------------------------------

/// What the preview request is up to.
#[derive(Debug, Default)]
pub enum Preview {
    #[default]
    Idle,
    Loading,
    Ready(Box<Value>),
    Failed(String),
}

/// The "Import playlist" / "Add tracks from a link" dialog.
#[derive(Debug)]
pub struct Import {
    /// `Some(id)` adds the tracks into that playlist; None makes a new one.
    pub into: Option<i64>,
    /// "url" or "text".
    pub mode: String,
    pub link: String,
    pub lines: text_editor::Content,
    pub name: String,
    pub follow: bool,
    pub request_missing: bool,
    /// A key of [`ALBUM_MODES`].
    pub album_mode: String,
    pub preview: Preview,
    pub importing: bool,
}

impl Import {
    pub fn new(url: &str, into: Option<i64>) -> Self {
        Self {
            into: into.filter(|i| *i != 0),
            mode: "url".into(),
            link: url.to_string(),
            lines: text_editor::Content::new(),
            name: String::new(),
            follow: false,
            request_missing: false,
            album_mode: String::new(),
            preview: Preview::Idle,
            importing: false,
        }
    }

    pub fn has_into(&self) -> bool {
        self.into.is_some()
    }

    /// `{url}` or `{text}`: what the preview and the import send.
    fn body(&self) -> Value {
        if self.mode == "url" {
            json!({ "url": self.link.trim() })
        } else {
            json!({ "text": self.lines.text() })
        }
    }
}

#[derive(Clone, Debug)]
pub enum ImportMsg {
    Mode(String),
    Link(String),
    Lines(text_editor::Action),
    /// "Choose a .txt file": its lines go into the list as if they were pasted.
    PickFile,
    Picked(Option<String>),
    Name(String),
    Follow(bool),
    RequestMissing(bool),
    AlbumMode(String),
    Preview,
    Previewed(Result<Value, ApiError>),
    Go,
    Done(Result<Value, ApiError>),
}

/// "Imported 12 track(s), skipped 2 duplicate(s), requested 3".
pub fn imported_text(r: &Value) -> String {
    let mut s = format!("Imported {} track(s)", fmt::s(r, "added"));
    if fmt::truthy(r, "duplicates_skipped") {
        s += &format!(", skipped {} duplicate(s)", fmt::s(r, "duplicates_skipped"));
    }
    if fmt::truthy(r, "requested") {
        s += &format!(", requested {}", fmt::s(r, "requested"));
    }
    s
}

/// Drive the dialog. `Some(id)` comes back when the import finished: the playlist to show.
pub fn import_update(imp: &mut Import, msg: ImportMsg, ctx: &mut Ctx) -> (Task<ImportMsg>, Option<i64>) {
    match msg {
        ImportMsg::Mode(m) => imp.mode = m,
        ImportMsg::Link(s) => imp.link = s,
        ImportMsg::Lines(a) => imp.lines.perform(a),
        ImportMsg::PickFile => {
            return (
                Task::perform(
                    async move {
                        let f = rfd::AsyncFileDialog::new()
                            .set_title("Choose a list")
                            .add_filter("Text", &["txt"])
                            .pick_file()
                            .await?;
                        Some(String::from_utf8_lossy(&f.read().await).into_owned())
                    },
                    ImportMsg::Picked,
                ),
                None,
            );
        }
        ImportMsg::Picked(Some(t)) => imp.lines = text_editor::Content::with_text(&t),
        ImportMsg::Picked(None) => {}
        ImportMsg::Name(s) => imp.name = s,
        ImportMsg::Follow(b) => imp.follow = b,
        ImportMsg::RequestMissing(b) => imp.request_missing = b,
        ImportMsg::AlbumMode(m) => imp.album_mode = m,
        ImportMsg::Preview => {
            imp.preview = Preview::Loading;
            let api = ctx.api.clone();
            let body = imp.body();
            return (
                Task::perform(async move { api.post("/playlists/preview", body).await }, ImportMsg::Previewed),
                None,
            );
        }
        ImportMsg::Previewed(Ok(p)) => {
            if imp.name.is_empty() {
                imp.name = fmt::s(&p, "name");
            }
            imp.preview = Preview::Ready(Box::new(p));
        }
        ImportMsg::Previewed(Err(e)) => imp.preview = Preview::Failed(e.message),
        ImportMsg::Go => {
            if imp.importing {
                return (Task::none(), None);
            }
            imp.importing = true;
            let mut b = imp.body();
            if let Some(o) = b.as_object_mut() {
                o.insert("into".into(), json!(imp.into));
                let name = imp.name.trim();
                o.insert("name".into(), if name.is_empty() { Value::Null } else { json!(name) });
                o.insert("follow".into(), json!(imp.follow && !imp.has_into()));
                o.insert("request_missing".into(), json!(imp.request_missing));
                o.insert(
                    "album_mode".into(),
                    if imp.album_mode.is_empty() { Value::Null } else { json!(imp.album_mode) },
                );
            }
            let api = ctx.api.clone();
            return (
                Task::perform(async move { api.post("/playlists/import", b).await }, ImportMsg::Done),
                None,
            );
        }
        ImportMsg::Done(Ok(r)) => {
            imp.importing = false;
            ctx.toast_ok(imported_text(&r));
            return (Task::none(), Some(fmt::i(&r, "id")));
        }
        ImportMsg::Done(Err(e)) => {
            imp.importing = false;
            ctx.toast_err(e.message);
        }
    }
    (Task::none(), None)
}

/// One row of the preview list: "artist – title · album", ISRC, in library / missing.
fn preview_row<'a>(tr: &Value) -> El<'a, ImportMsg> {
    let p = theme::look();
    let mut r = row![].spacing(10).align_y(Alignment::Center);
    let mut words = row![t(format!("{} – {}", fmt::s(tr, "artist"), fmt::s(tr, "title"))).size(13.5)]
        .spacing(6)
        .align_y(Alignment::Center);
    if fmt::truthy(tr, "album") {
        words = words.push(small(format!("· {}", fmt::s(tr, "album"))).color(p.faint));
    }
    r = r.push(words.width(Length::Fill));
    if fmt::truthy(tr, "isrc") {
        r = r.push(iced::widget::tooltip(badge("ISRC", Badge::Plain), crate::widgets::tip_box("Has ISRC – exact matching"), iced::widget::tooltip::Position::Top).gap(4));
    }
    r = r.push(if fmt::truthy(tr, "track_id") {
        badge("in library", Badge::Ok)
    } else {
        badge("missing", Badge::Plain)
    });
    column![container(r).padding([7, 14]), widgets::divider()].into()
}

/// The preview block under the form.
fn preview_view<'a>(imp: &'a Import) -> El<'a, ImportMsg> {
    match &imp.preview {
        Preview::Idle => column![].into(),
        Preview::Loading => container(muted("Looking it up…").size(13))
            .padding([12, 0])
            .width(Length::Fill)
            .into(),
        Preview::Failed(e) => error_box(e),
        Preview::Ready(p) => {
            let mut head = row![
                t(fmt::s(p, "name")).size(14).font(theme::bold()),
                badge(src_label(p), Badge::Plain),
                badge(format!("{} tracks", fmt::s(p, "count")), Badge::Plain),
                badge(format!("{} already in library", fmt::s(p, "in_library")), Badge::Ok),
            ]
            .spacing(8)
            .align_y(Alignment::Center);
            if fmt::truthy(p, "duplicates") {
                head = head.push(badge(
                    format!("{} duplicate(s) will be skipped", fmt::s(p, "duplicates")),
                    Badge::Warn,
                ));
            }
            let mut col = column![head.wrap()].spacing(10);
            if fmt::truthy(p, "warning") {
                col = col.push(error_box(p["warning"].as_str().unwrap_or("")));
            }
            let rows: Vec<El<'a, ImportMsg>> = fmt::items(&p["tracks"]).iter().map(preview_row).collect();
            col = col.push(widgets::card_flat(column(rows)));
            col.into()
        }
    }
}

/// The dialog's body (the page wraps it with `.map`).
fn import_body<'a>(imp: &'a Import) -> El<'a, ImportMsg> {
    let p = theme::look();
    let mut col = column![widgets::tabs(
        &[("url", "From a link"), ("text", "Paste a list")],
        &imp.mode,
        ImportMsg::Mode
    )]
    .spacing(14);
    if imp.mode == "url" {
        col = col.push(
            column![
                text_input(
                    "YouTube Music, Spotify, Deezer, Tidal, Apple Music, Amazon Music, Qobuz, YouTube (Music), SoundCloud, Bandcamp, ListenBrainz…",
                    &imp.link
                )
                .on_input(ImportMsg::Link)
                .on_submit(ImportMsg::Preview)
                .padding([9, 12])
                .size(14)
                .style(theme::input),
                small("Connect Spotify/Tidal on the Connections page to import private playlists and Liked Songs (e.g. https://music.youtube.com/playlist?list=LM or https://open.spotify.com/collection/tracks). Amazon Music playlists and albums import from their public share links.")
                    .color(p.faint),
            ]
            .spacing(6),
        );
    } else {
        col = col.push(text_area(&imp.lines, "One per line:  Artist - Title", 8, ImportMsg::Lines));
        col = col.push(btn_icon(Icon::Plus, "Choose a .txt file", Btn::Default, Some(ImportMsg::PickFile)));
    }
    let mut opts = row![].spacing(14).align_y(Alignment::Center);
    if !imp.has_into() {
        opts = opts.push(
            text_input("Playlist name (optional)", &imp.name)
                .on_input(ImportMsg::Name)
                .padding([8, 12])
                .size(14)
                .width(280)
                .style(theme::input),
        );
        if imp.mode == "url" {
            opts = opts.push(
                toggler(imp.follow)
                    .label("Keep following (pull new tracks)")
                    .text_size(13.5)
                    .spacing(8)
                    .on_toggle(ImportMsg::Follow)
                    .style(theme::switch),
            );
        }
    }
    opts = opts.push(
        toggler(imp.request_missing)
            .label("Download missing tracks")
            .text_size(13.5)
            .spacing(8)
            .on_toggle(ImportMsg::RequestMissing)
            .style(theme::switch),
    );
    opts = opts.push(select(&ALBUM_MODES, &imp.album_mode, ImportMsg::AlbumMode));
    let can_preview = !matches!(imp.preview, Preview::Loading);
    opts = opts.push(btn_icon(
        Icon::Search,
        "Preview",
        Btn::Default,
        can_preview.then_some(ImportMsg::Preview),
    ));
    col = col.push(opts.wrap());
    col = col.push(preview_view(imp));
    col.width(Length::Fill).into()
}

/// The whole modal in the page's message type. `wrap` lifts the dialog's messages, `on_close`
/// is the page's close message.
pub fn import_dialog<'a, M: Clone + 'a>(
    imp: &'a Import,
    wrap: impl Fn(ImportMsg) -> M + 'a,
    on_close: M,
) -> El<'a, M> {
    let go = wrap(ImportMsg::Go);
    let title = if imp.has_into() { "Add tracks from a link" } else { "Import playlist" };
    let footer = row![
        btn("Cancel", Btn::Default, Some(on_close.clone())),
        btn_icon(
            Icon::Download,
            if imp.importing { "Importing…" } else { "Import" },
            Btn::Primary,
            (!imp.importing).then_some(go),
        ),
    ]
    .spacing(10);
    modal(title, import_body(imp).map(wrap), Some(footer.into()), true, on_close)
}

// ----- the page ---------------------------------------------------------------------------

/// The Merge playlists dialog.
#[derive(Debug)]
pub struct Merge {
    pub name: String,
    pub delete_sources: bool,
    pub busy: bool,
}

#[derive(Debug)]
pub enum Dialog {
    Import(Import),
    Merge(Merge),
    /// Add a shared playlist: the pasted link, and the server's answer when it said no.
    AddShared { link: String, busy: bool, error: Option<String> },
}

#[derive(Debug, Default)]
pub struct State {
    /// None while loading; Err(message) when the server said no.
    rows: Option<Result<Vec<Value>, String>>,
    /// Invites to collaborate waiting for the user (`/api/collab/invites`).
    invites: Vec<Value>,
    /// Playlists ticked for a merge.
    selected: Vec<i64>,
    dialog: Option<Dialog>,
    creating: bool,
}

#[derive(Clone, Debug)]
pub enum Message {
    Loaded(Result<Value, ApiError>),
    Open(i64),
    Select(i64, bool),
    New,
    Created(Result<Value, ApiError>),
    OpenImport,
    Import(ImportMsg),
    OpenMerge,
    MergeName(String),
    MergeDelete(bool),
    MergeGo,
    Merged(Result<Value, ApiError>),
    OpenAddShared,
    AddSharedLink(String),
    AddSharedGo,
    AddSharedDone(Result<Value, ApiError>),
    CloseDialog,
    Invites(Result<Value, ApiError>),
    /// Join / Decline an invite (the playlist's id).
    AcceptInvite(i64),
    DeclineInvite(i64),
    Answered(Option<i64>, Result<Value, ApiError>),
    /// Joined with an invite link pasted into Add a shared playlist.
    Joined(Result<Value, ApiError>),
}

fn load(ctx: &Ctx) -> Task<Message> {
    let api = ctx.api.clone();
    let api2 = ctx.api.clone();
    Task::batch([
        Task::perform(async move { api.get("/playlists").await }, Message::Loaded),
        Task::perform(async move { api2.get("/collab/invites").await }, Message::Invites),
    ])
}

/// A pasted text that is a collaborative playlist's invite link rather than a share link.
pub fn is_invite_link(text: &str) -> bool {
    text.contains("collab/")
}

/// "anna invited you to collaborate on Road trip · 12 tracks".
pub fn invite_text(i: &Value) -> String {
    format!("{} invited you to collaborate on {} · {} tracks", fmt::s(i, "owner"), fmt::s(i, "name"), fmt::i(i, "count"))
}

/// Called when the app navigates here (with the route's parameters).
pub fn open(state: &mut State, ctx: &mut Ctx) -> Task<Message> {
    state.rows = None;
    state.selected.clear();
    state.dialog = None;
    state.creating = false;
    load(ctx)
}

pub fn update(state: &mut State, msg: Message, ctx: &mut Ctx) -> Task<Message> {
    match msg {
        Message::Loaded(Ok(v)) => state.rows = Some(Ok(fmt::items(&v))),
        Message::Loaded(Err(e)) => state.rows = Some(Err(e.message)),
        Message::Open(id) => ctx.navigate(Page::Playlist(id)),
        Message::Select(id, on) => {
            state.selected.retain(|x| *x != id);
            if on {
                state.selected.push(id);
            }
        }
        Message::New => {
            if state.creating {
                return Task::none();
            }
            state.creating = true;
            let api = ctx.api.clone();
            return Task::perform(
                async move { api.post("/playlists", json!({ "name": "New playlist" })).await },
                Message::Created,
            );
        }
        Message::Created(r) => {
            state.creating = false;
            match r {
                Ok(v) => ctx.navigate(Page::Playlist(fmt::i(&v, "id"))),
                Err(e) => ctx.toast_err(e.message),
            }
        }
        Message::OpenImport => state.dialog = Some(Dialog::Import(Import::new("", None))),
        Message::Import(m) => {
            if let Some(Dialog::Import(imp)) = state.dialog.as_mut() {
                let (task, done) = import_update(imp, m, ctx);
                if let Some(id) = done {
                    state.dialog = None;
                    ctx.navigate(Page::Playlist(id));
                }
                return task.map(Message::Import);
            }
        }
        Message::OpenMerge => {
            if state.selected.len() >= 2 {
                state.dialog = Some(Dialog::Merge(Merge {
                    name: "Merged playlist".into(),
                    delete_sources: false,
                    busy: false,
                }));
            }
        }
        Message::MergeName(s) => {
            if let Some(Dialog::Merge(m)) = state.dialog.as_mut() {
                m.name = s;
            }
        }
        Message::MergeDelete(b) => {
            if let Some(Dialog::Merge(m)) = state.dialog.as_mut() {
                m.delete_sources = b;
            }
        }
        Message::MergeGo => {
            if let Some(Dialog::Merge(m)) = state.dialog.as_mut() {
                if m.busy {
                    return Task::none();
                }
                m.busy = true;
                let api = ctx.api.clone();
                let body = json!({ "ids": state.selected, "name": m.name, "delete_sources": m.delete_sources });
                return Task::perform(async move { api.post("/playlists/merge", body).await }, Message::Merged);
            }
        }
        Message::Merged(Ok(r)) => {
            state.dialog = None;
            state.selected.clear();
            ctx.toast_ok(merged_text(&r));
            ctx.navigate(Page::Playlist(fmt::i(&r, "id")));
        }
        Message::Merged(Err(e)) => {
            if let Some(Dialog::Merge(m)) = state.dialog.as_mut() {
                m.busy = false;
            }
            ctx.toast_err(e.message);
        }
        Message::OpenAddShared => {
            state.dialog = Some(Dialog::AddShared { link: String::new(), busy: false, error: None });
        }
        Message::AddSharedLink(s) => {
            if let Some(Dialog::AddShared { link, .. }) = state.dialog.as_mut() {
                *link = s;
            }
        }
        Message::AddSharedGo => {
            if let Some(Dialog::AddShared { link, busy, error }) = state.dialog.as_mut() {
                if *busy || link.trim().is_empty() {
                    return Task::none();
                }
                *busy = true;
                *error = None;
                let api = ctx.api.clone();
                let body = json!({ "link": link.trim() });
                if is_invite_link(link) {
                    return Task::perform(async move { api.post("/collab/join", body).await }, Message::Joined);
                }
                return Task::perform(async move { api.post("/playlists/shared", body).await }, Message::AddSharedDone);
            }
        }
        Message::AddSharedDone(Ok(r)) => {
            state.dialog = None;
            ctx.toast_ok(add_toast(&r));
            ctx.navigate(Page::Playlist(fmt::i(&r, "id")));
        }
        Message::AddSharedDone(Err(e)) => {
            if let Some(Dialog::AddShared { busy, error, .. }) = state.dialog.as_mut() {
                *busy = false;
                *error = Some(e.message);
            }
        }
        Message::CloseDialog => state.dialog = None,
        Message::Invites(r) => state.invites = r.map(|v| fmt::items(&v)).unwrap_or_default(),
        Message::AcceptInvite(pid) => {
            let api = ctx.api.clone();
            return Task::perform(async move { api.post_empty(&format!("/collab/invites/{pid}/accept")).await }, move |r| {
                Message::Answered(Some(pid), r)
            });
        }
        Message::DeclineInvite(pid) => {
            state.invites.retain(|i| fmt::i(i, "playlist_id") != pid);
            let api = ctx.api.clone();
            return Task::perform(async move { api.post_empty(&format!("/collab/invites/{pid}/decline")).await }, |r| {
                Message::Answered(None, r)
            });
        }
        Message::Answered(joined, r) => match (joined, r) {
            (Some(pid), Ok(_)) => {
                ctx.toast_ok("Joined: you can change its songs now");
                ctx.navigate(Page::Playlist(pid));
            }
            (_, Err(e)) => {
                ctx.toast_err(e.message);
                return load(ctx);
            }
            _ => {}
        },
        Message::Joined(Ok(r)) => {
            state.dialog = None;
            ctx.toast_ok(format!("Joined {}'s playlist", fmt::s(&r, "owner")));
            ctx.navigate(Page::Playlist(fmt::i(&r, "id")));
        }
        Message::Joined(Err(e)) => {
            if let Some(Dialog::AddShared { busy, error, .. }) = state.dialog.as_mut() {
                *busy = false;
                *error = Some(e.message);
            }
        }
    }
    Task::none()
}

/// "Merged 40 tracks → 36 unique (4 duplicates removed)".
pub fn merged_text(r: &Value) -> String {
    format!(
        "Merged {} tracks → {} unique ({} duplicates removed)",
        fmt::s(r, "total"),
        fmt::s(r, "added"),
        fmt::s(r, "duplicates_skipped")
    )
}

/// The two lines under a playlist's name: "12 tracks · 80% in library · following · shared" and the
/// source with its sync targets.
pub fn tile_sub(p: &Value) -> String {
    if is_linked(p) {
        return format!("{}\n{LINKED_SOURCE}", linked_tile_line(p));
    }
    let (total, matched) = (fmt::f(p, "total"), fmt::f(p, "matched"));
    let pct = if total > 0.0 { (matched / total * 100.0).round() as i64 } else { 0 };
    let follow = if fmt::truthy(p, "follow") { " · following" } else { "" };
    let shared = if p["share"].is_object() { " · shared" } else { "" };
    let collab = match p["collab"]["role"].as_str() {
        Some("collaborator") => format!(" · by {}", fmt::s(&p["collab"], "owner")),
        _ if fmt::truthy(p, "collaborative") => " · collaborative".to_string(),
        _ => String::new(),
    };
    let targets: Vec<String> = fmt::items(&p["sync_targets"])
        .iter()
        .filter_map(|t| t.as_str().map(|t| target_name(t).unwrap_or(t).to_string()))
        .collect();
    let sync = if targets.is_empty() {
        String::new()
    } else {
        format!(" → {}", targets.join(", "))
    };
    let second = format!("{}{sync}", src_label(p));
    let first = format!("{} tracks · {pct}% in library{follow}{shared}{collab}", fmt::s(p, "total"));
    if second.is_empty() {
        first
    } else {
        format!("{first}\n{second}")
    }
}

/// The heart on the accent gradient that stands for Liked Songs.
fn heart_art<'a, M: 'a>(size: f32) -> El<'a, M> {
    let p = theme::look();
    container(icon(Icon::Heart, size * 0.42, p.on_accent))
        .width(size)
        .height(size)
        .center(size)
        .style(theme::accent_box)
        .into()
}

fn tile<'a>(ctx: &Ctx<'a>, p: &Value, selected: bool) -> El<'a, Message> {
    let id = fmt::i(p, "id");
    let liked = fmt::s(p, "kind") == "liked";
    let mut tile = Tile::new(fmt::s(p, "name"), TileKind::Playlist, Message::Open(id)).sub(tile_sub(p));
    if liked {
        tile = tile.art(heart_art(widgets::tile::WIDTH));
    } else if is_linked(p) {
        // someone else's playlist: the link's mosaic (a server path), and nothing to merge
        tile = tile.cover(ctx.picture(&fmt::s(p, "cover_url")));
    } else {
        tile = tile
            .cover(ctx.picture(&fmt::s(p, "cover_url")))
            .corner(
                iced::widget::tooltip(checkbox(selected).size(18).on_toggle(move |on| Message::Select(id, on)), crate::widgets::tip_box("Select to merge"), iced::widget::tooltip::Position::Bottom).gap(4)
                .into(),
            );
    }
    tile.view()
}

pub fn view<'a>(state: &'a State, ctx: &Ctx<'a>) -> El<'a, Message> {
    let mut actions: Vec<El<'a, Message>> = Vec::new();
    let n = state.selected.len();
    if n >= 2 {
        actions.push(
            btn_icon(
                Icon::List,
                format!("Merge {n} playlists"),
                Btn::Default,
                Some(Message::OpenMerge),
            )
            .into(),
        );
    }
    actions.push(
        btn_icon(
            Icon::Plus,
            "New",
            Btn::Default,
            (!state.creating).then_some(Message::New),
        )
        .into(),
    );
    actions.push(btn_icon(Icon::Link, ADD_SHARED_TITLE, Btn::Default, Some(Message::OpenAddShared)).into());
    actions.push(btn_icon(Icon::Download, "Import", Btn::Primary, Some(Message::OpenImport)).into());
    let head = page_head(
        "Playlists",
        Some(small_muted("Import from anywhere, merge without duplicates, and sync to Finamp / Plexamp.").into()),
        actions,
    );
    let body: El<'a, Message> = match &state.rows {
        None => skeleton_tiles(6, widgets::tile::WIDTH),
        Some(Err(e)) => error_box(e),
        Some(Ok(rows)) if rows.is_empty() => empty(Icon::List, "No playlists yet", ""),
        Some(Ok(rows)) => grid(
            rows.iter()
                .map(|p| tile(ctx, p, state.selected.contains(&fmt::i(p, "id"))))
                .collect(),
        ),
    };
    let mut col = column![head].spacing(20).width(Length::Fill);
    if !state.invites.is_empty() {
        let mut list = column![].spacing(8);
        for i in &state.invites {
            let pid = fmt::i(i, "playlist_id");
            list = list.push(
                row![
                    container(t(invite_text(i)).size(14)).width(Length::Fill),
                    btn_sm("Join", Btn::Primary, Some(Message::AcceptInvite(pid))),
                    btn_sm("Decline", Btn::Ghost, Some(Message::DeclineInvite(pid))),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            );
        }
        col = col.push(card(list).width(Length::Fill));
    }
    col.push(body).into()
}

/// A modal or ⋯ menu to lay over the whole window, when one is open.
pub fn overlay<'a>(state: &'a State, _ctx: &Ctx<'a>) -> Option<El<'a, Message>> {
    match state.dialog.as_ref()? {
        Dialog::Import(imp) => Some(import_dialog(imp, Message::Import, Message::CloseDialog)),
        Dialog::Merge(m) => {
            let body = column![
                muted(format!(
                    "Tracks from {} playlists are combined into one. The same song + version is only kept once – remixes, live versions and covers stay separate.",
                    state.selected.len()
                ))
                .size(14),
                field(
                    "Name",
                    text_input("Merged playlist", &m.name)
                        .on_input(Message::MergeName)
                        .on_submit(Message::MergeGo)
                        .padding([9, 12])
                        .size(14)
                        .style(theme::input)
                ),
                toggler(m.delete_sources)
                    .label("Delete the original playlists")
                    .text_size(14)
                    .spacing(10)
                    .on_toggle(Message::MergeDelete)
                    .style(theme::switch),
            ]
            .spacing(14);
            let footer = row![
                btn("Cancel", Btn::Default, Some(Message::CloseDialog)),
                btn(
                    if m.busy { "Merging…" } else { "Merge" },
                    Btn::Primary,
                    (!m.busy).then_some(Message::MergeGo)
                ),
            ]
            .spacing(10);
            Some(modal("Merge playlists", body, Some(footer.into()), false, Message::CloseDialog))
        }
        Dialog::AddShared { link, busy, error } => {
            let mut body = column![
                small_muted(ADD_SHARED_HINT),
                text_input(ADD_SHARED_PLACEHOLDER, link)
                    .on_input(Message::AddSharedLink)
                    .on_submit(Message::AddSharedGo)
                    .padding([9, 12])
                    .size(14)
                    .style(theme::input),
            ]
            .spacing(14);
            if let Some(e) = error {
                body = body.push(error_box(e));
            }
            let footer = row![
                btn("Cancel", Btn::Default, Some(Message::CloseDialog)),
                btn_icon(
                    Icon::Plus,
                    if *busy { "Adding…" } else { ADD_TO_MINE },
                    Btn::Primary,
                    (!*busy && !link.trim().is_empty()).then_some(Message::AddSharedGo),
                ),
            ]
            .spacing(10);
            Some(modal(ADD_SHARED_TITLE, body, Some(footer.into()), false, Message::CloseDialog))
        }
    }
}

pub fn subscription(_state: &State) -> Subscription<Message> {
    Subscription::none()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_lines_match_the_web() {
        assert_eq!(src_label(&json!({ "source": "ytmusic" })), "YouTube Music");
        assert_eq!(src_label(&json!({ "source": "odd" })), "odd");
        assert_eq!(src_label(&json!({})), "");
        assert_eq!(target_name("plex"), Some("Plex / Plexamp"));
        let p = json!({ "total": 10, "matched": 8, "follow": 1, "source": "spotify", "sync_targets": ["plex", "x"] });
        assert_eq!(tile_sub(&p), "10 tracks · 80% in library · following\nSpotify → Plex / Plexamp, x");
        assert_eq!(tile_sub(&json!({ "total": 0, "matched": 0, "source": "" })), "0 tracks · 0% in library");
        let mut shared = p.clone();
        shared["share"] = json!({ "token": "abc", "path": "/s/abc", "url": "http://x/s/abc" });
        assert_eq!(tile_sub(&shared), "10 tracks · 80% in library · following · shared\nSpotify → Plex / Plexamp, x");
        assert_eq!(
            tile_sub(&json!({ "total": 0, "matched": 0, "share": null })),
            "0 tracks · 0% in library",
            "not shared"
        );
    }

    #[test]
    fn toasts_read_like_the_web() {
        assert_eq!(imported_text(&json!({ "added": 12 })), "Imported 12 track(s)");
        assert_eq!(
            imported_text(&json!({ "added": 12, "duplicates_skipped": 2, "requested": 3 })),
            "Imported 12 track(s), skipped 2 duplicate(s), requested 3"
        );
        assert_eq!(
            merged_text(&json!({ "total": 40, "added": 36, "duplicates_skipped": 4 })),
            "Merged 40 tracks → 36 unique (4 duplicates removed)"
        );
    }

    #[test]
    fn linked_tiles_and_pickers() {
        let active = json!({ "id": 41, "name": "Road trip", "kind": "linked", "source": "share", "total": 12, "matched": 12,
            "read_only": true, "share": null, "sync_targets": [], "linked": { "owner": "anna", "active": true, "liked": false } });
        assert_eq!(tile_sub(&active), "12 tracks · by anna\nAdded from a link");
        let mut ended = active.clone();
        ended["linked"]["active"] = json!(false);
        ended["total"] = json!(0);
        assert_eq!(tile_sub(&ended), "anna stopped sharing this playlist\nAdded from a link");
        assert!(!can_add_to(&active), "someone else's playlist takes no songs");
        assert!(!can_add_to(&json!({ "kind": "playlist", "read_only": true })));
        assert!(can_add_to(&json!({ "kind": "playlist", "read_only": false })));
        assert!(can_add_to(&json!({ "kind": "liked" })));
        assert!(can_add_to(&json!({ "kind": "playlist" })), "an older server has no read_only");
        assert_eq!(src_name("share"), Some("Added from a link"));
        assert!(is_linked(&active));
        assert!(!is_linked(&json!({ "kind": "playlist" })));
    }

    #[test]
    fn linked_texts_match_the_web() {
        // web-rs/src/pages/playlists.rs: the constants and the linked helpers
        assert_eq!(ADD_SHARED_TITLE, "Add a shared playlist");
        assert_eq!(
            ADD_SHARED_HINT,
            "Paste a playlist link from someone on this server. You get their playlist as it is now, and it changes when they change it."
        );
        assert_eq!(ADD_SHARED_PLACEHOLDER, "https://…/s/…");
        assert_eq!(ADD_TO_MINE, "Add to my playlists");
        assert_eq!(ADDED_TO_MINE, "Added to your playlists");
        assert_eq!(ALREADY_MINE, "Already in your playlists");
        assert_eq!(REMOVE_FROM_MINE, "Remove from my playlists");
        assert_eq!(REMOVED_FROM_MINE, "Removed from your playlists");
        assert_eq!(SAVED_COPY, "Saved to your playlists");
        assert_eq!(SAVE_COPY, "Save a copy");
        assert_eq!(LINKED_SOURCE, "Added from a link");
        assert_eq!(LINKED_EMPTY, "No songs here right now");
        let pl = json!({ "kind": "linked", "total": 12, "linked": { "owner": "anna", "active": true, "liked": false } });
        assert_eq!(linked_kicker(&pl), "Playlist · by anna");
        let liked = json!({ "kind": "linked", "linked": { "owner": "anna", "active": true, "liked": true } });
        assert_eq!(linked_kicker(&liked), "Liked songs · by anna");
        assert_eq!(linked_hint("anna"), "Changes anna makes show here. Save a copy to make your own.");
        assert_eq!(linked_stopped("anna"), "anna stopped sharing this playlist");
        assert_eq!(linked_tile_line(&pl), "12 tracks · by anna");
        assert_eq!(
            linked_tile_line(&json!({ "kind": "linked", "linked": { "owner": "anna", "active": false } })),
            "anna stopped sharing this playlist"
        );
        assert_eq!(
            remove_text("Road trip"),
            "Remove “Road trip” from your playlists? You can add it again from its link."
        );
        assert_eq!(added_by_text(0), "");
        assert_eq!(added_by_text(1), "added by 1 person");
        assert_eq!(added_by_text(3), "added by 3 people");
        assert_eq!(add_toast(&json!({ "id": 4, "added": true })), "Added to your playlists");
        assert_eq!(add_toast(&json!({ "id": 4, "added": false })), "Already in your playlists");
    }

    #[test]
    fn the_import_body_follows_the_mode() {
        let mut imp = Import::new(" https://x ", Some(0));
        assert!(!imp.has_into(), "0 is no playlist");
        assert_eq!(imp.body(), json!({ "url": "https://x" }));
        imp.mode = "text".into();
        imp.lines = text_editor::Content::with_text("A - B");
        assert_eq!(imp.body(), json!({ "text": "A - B" }));
        assert!(Import::new("", Some(4)).has_into());
    }
}
