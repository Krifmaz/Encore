//! Album: a library album and its songs. Behaviour reference: web-rs/src/pages/library.rs
//! `render_album` (the header with Play / Shuffle / Share / Edit details and the ⋯ menu, the song
//! rows with likes and the song menu), `album_menu`, `match_modal` (Tag this album), `edit_modal`
//! (Edit details) and web-rs/src/pages/artwork.rs (Change artwork).
//!
//! A Surround album (0.35, an `s…` id) has its own songs, a link to its stereo album and none of
//! the stereo library's tools; a stereo album links to its surround versions and, for admins,
//! has "Get Dolby Atmos version" (web `album_versions`, `atmos_button`, `surround_album_menu`).

use super::library::{self, format_text, long_dur, size_text, tr, verify_badge, Act, Shared};
use super::library_types as lt;
use crate::api::ApiError;
use crate::fmt;
use crate::pages::{urlencode, Ctx, El, Page};
use crate::theme::{self, Badge, Btn};
use crate::widgets::drop_zone::{self, DropZone, Outcome};
use crate::widgets::{
    self, badge, btn, btn_icon, empty, grid, h1, h3, icon, icon_btn, modal, pop_menu, skeleton_rows,
    small_muted, t, tabs, Icon, MenuItem, Tile, TileKind, TrackRow,
};
use iced::widget::{button, column, container, image, radio, row, space, text_input, toggler, tooltip};
use iced::{Alignment, ContentFit, Length, Point, Subscription, Task};
use serde_json::{json, Value};
use std::collections::HashSet;

/// Edit details.
#[derive(Debug)]
pub struct EditState {
    /// albumartist, album, date, genre.
    pub fields: [String; 4],
    /// disc, track, title, artist per song.
    pub tracks: Vec<[String; 4]>,
    pub rename: bool,
    pub busy: bool,
}

/// Tag this album.
#[derive(Debug)]
pub struct MatchState {
    pub provider: String,
    pub artist: String,
    pub album: String,
    pub link: String,
    pub searching: bool,
    pub candidates: Option<Vec<Value>>,
    pub track_count: String,
    pub error: String,
    pub preview: Option<Value>,
    pub previewing: bool,
    pub rename: bool,
    pub applying: bool,
}

/// Change artwork.
#[derive(Debug)]
pub struct ArtState {
    pub q: String,
    pub searching: bool,
    pub candidates: Option<Vec<Value>>,
    pub error: String,
    pub busy: bool,
    /// A picture dragged onto the window: it is uploaded like *Upload…*.
    pub drop: DropZone,
}

#[derive(Debug)]
pub enum Dialog {
    Edit(Box<EditState>),
    Match(Box<MatchState>),
    Artwork(Box<ArtState>),
    /// Change type… (0.36): the format picked ("" = Automatic).
    ChangeType { picked: &'static str, busy: bool },
    /// Move album to Trash (0.36): the dry run's title and words; `can_go` false when the Trash has no room.
    Trash { title: String, text: String, can_go: bool, keep: bool },
    /// Get the rest of the album for an album MusicBrainz doesn't list: another catalogue's track list, or
    /// one to type in (super::rest_of_album).
    Rest(Box<super::rest_of_album::State>),
}

#[derive(Debug, Default)]
pub struct State {
    pub id: String,
    /// The album with its `tracks` (None while loading).
    pub album: Option<Value>,
    pub error: String,
    pub liked: HashSet<i64>,
    /// The album's ⋯ menu is open at this point.
    pub menu: Option<Point>,
    pub dialog: Option<Dialog>,
    pub shared: Shared,
    /// A Report a problem dialog for the whole album is open (the web goes to the library after).
    reporting_album: bool,
    seq: u64,
}

const PROVIDERS: [(&str, &str); 5] = [
    ("musicbrainz", "MusicBrainz"),
    ("deezer", "Deezer"),
    ("apple", "Apple Music"),
    ("discogs", "Discogs"),
    ("fingerprint", "By audio (AcoustID)"),
];

fn match_help(provider: &str) -> &'static str {
    match provider {
        "musicbrainz" => "Full MusicBrainz tags (ids that Picard, Navidrome and Jellyfin use).",
        "deezer" => "Titles, track numbers, ISRCs, label, barcode and cover from Deezer – for releases MusicBrainz lacks or gets wrong.",
        "apple" => "Titles, track numbers, genre and a large cover from Apple Music.",
        "discogs" => "Vinyl, cassettes, bootlegs and old releases (needs a Discogs token in Settings).",
        "fingerprint" => "Ignores names: identifies each song by its audio, then finds the release most of them are on (needs an AcoustID key).",
        _ => "",
    }
}

#[derive(Clone, Debug)]
pub enum Message {
    Loaded(u64, Result<Value, ApiError>),
    Likes(Result<Value, ApiError>),
    Play(usize),
    PlayAll,
    ShuffleAll,
    Like(i64),
    Liked(i64, bool, Result<Value, ApiError>),
    OpenArtist(String),
    /// One artist of a song (its line links each one).
    OpenPage(Page),
    Share,
    /// Save the album's songs on this computer.
    SaveOffline,
    ShareGot(Result<Value, ApiError>),
    Menu,
    CloseMenu,
    AddToQueue,
    PlayNext,
    AddToPlaylist,
    ConfirmAll,
    ConfirmedAll(Result<Value, ApiError>),
    DownloadZip,
    Zipped(Result<String, String>),
    AlbumLyrics,
    AlbumLyricsDone(Result<Value, ApiError>),
    GetRest,
    RestLookup(Result<Value, ApiError>),
    /// The Get the rest of the album dialog's own messages.
    Rest(super::rest_of_album::Message),
    ReleaseInfo,
    ReportAlbum,
    EditOpen,
    EditField(usize, String),
    EditTrack(usize, usize, String),
    EditRename(bool),
    EditSave,
    EditSaved(Result<Value, ApiError>),
    MatchOpen,
    MatchProvider(String),
    MatchArtist(String),
    MatchAlbum(String),
    MatchLink(String),
    MatchSearch,
    MatchResults(Result<Value, ApiError>),
    MatchPick(String),
    MatchPreview(Result<Value, ApiError>),
    MatchBack,
    MatchRename(bool),
    MatchApply,
    MatchApplied(String, Result<Value, ApiError>),
    ArtOpen,
    ArtQuery(String),
    ArtSearch,
    ArtResults(Result<Value, ApiError>),
    ArtPick(usize),
    ArtUpload,
    /// A file dragged over or dropped on the window while *Change artwork* is open.
    ArtDrop(drop_zone::Event),
    /// The picked file as a data URL ("" = cancelled), or why it was refused.
    ArtFile(Result<String, String>),
    ArtSet(Result<Value, ApiError>),
    FindArt,
    FoundArt(Result<Value, ApiError>),
    /// Change type… (0.36): open the dialog, pick, save.
    TypeOpen,
    TypePick(&'static str),
    TypeSave,
    TypeSaved(Result<Value, ApiError>),
    /// Move album to Trash (0.36): a dry run of this album, the dialog, then DELETE.
    TrashAsk,
    TrashDry(Result<Value, ApiError>),
    TrashGo,
    Trashed(Result<Value, ApiError>),
    TrackMenu(usize),
    Suspect(usize),
    /// The other version of this album (stereo or surround).
    OpenVersion(String),
    CloseDialog,
    Shared(Act),
}

/// Called when the app navigates here (with the route's parameters).
pub fn open(state: &mut State, id: String, ctx: &mut Ctx) -> Task<Message> {
    state.id = id;
    state.menu = None;
    state.dialog = None;
    state.shared.reset();
    state.reporting_album = false;
    state.liked.clear();
    let likes = {
        let api = ctx.api.clone();
        Task::perform(async move { api.get("/likes").await }, Message::Likes)
    };
    Task::batch([load(state, ctx), likes])
}

fn load(state: &mut State, ctx: &Ctx) -> Task<Message> {
    state.seq += 1;
    let seq = state.seq;
    state.album = None;
    state.error.clear();
    let api = ctx.api.clone();
    let path = format!("/library/albums/{}", state.id);
    Task::perform(async move { api.get(&path).await }, move |r| Message::Loaded(seq, r))
}

/// The album's songs with `album_id` set (what the player queues).
fn queue_tracks(state: &State) -> Vec<Value> {
    match &state.album {
        Some(a) => library::with_album_id(&fmt::items(&a["tracks"]), &state.id),
        None => Vec::new(),
    }
}

/// `str.match(uuid | deezer | apple | discogs)` of the Tag this album link box.
pub fn parse_release_link(link: &str) -> Option<String> {
    let l = link.trim();
    if l.is_empty() {
        return None;
    }
    if let Some(u) = find_uuid(l) {
        return Some(u);
    }
    let digits_after = |hay: &str, marker: &str| -> Option<String> {
        let i = hay.find(marker)? + marker.len();
        let d: String = hay[i..].chars().take_while(|c| c.is_ascii_digit()).collect();
        (!d.is_empty()).then_some(d)
    };
    if l.contains("deezer.com/") {
        if let Some(d) = digits_after(l, "/album/") {
            return Some(format!("deezer:{d}"));
        }
    }
    if l.contains("apple.com/") {
        if let Some(i) = l.find("/album/") {
            let rest = &l[i + 7..];
            // `/album/<slug>/<id>` or `/album/<id>` or `/album/id<id>`
            let last = rest.trim_end_matches('/').rsplit('/').next().unwrap_or("");
            let last = last.split('?').next().unwrap_or("");
            let last = last.strip_prefix("id").unwrap_or(last);
            let d: String = last.chars().take_while(|c| c.is_ascii_digit()).collect();
            if !d.is_empty() {
                return Some(format!("apple:{d}"));
            }
        }
    }
    if l.contains("discogs.com/") {
        for (kind, letter) in [("/release/", "r"), ("/master/", "m")] {
            if let Some(d) = digits_after(l, kind) {
                return Some(format!("discogs:{letter}{d}"));
            }
        }
    }
    None
}

/// A MusicBrainz id anywhere in the text.
fn find_uuid(s: &str) -> Option<String> {
    let groups = [8usize, 4, 4, 4, 12];
    let b = s.as_bytes();
    let mut i = 0;
    while i + 36 <= b.len() {
        let cand = &s[i..i + 36];
        let mut ok = cand.is_ascii();
        if ok {
            let mut at = 0;
            for (gi, g) in groups.iter().enumerate() {
                if !cand[at..at + g].bytes().all(|c| c.is_ascii_hexdigit()) {
                    ok = false;
                    break;
                }
                at += g;
                if gi < 4 {
                    if cand.as_bytes()[at] != b'-' {
                        ok = false;
                        break;
                    }
                    at += 1;
                }
            }
        }
        if ok {
            return Some(cand.to_ascii_lowercase());
        }
        i += 1;
    }
    None
}

/// Standard base64 (for the uploaded picture's data URL).
pub fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk.len();
        let v = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        out.push(T[(v >> 18) as usize & 63] as char);
        out.push(T[(v >> 12) as usize & 63] as char);
        out.push(if n > 1 { T[(v >> 6) as usize & 63] as char } else { '=' });
        out.push(if n > 2 { T[v as usize & 63] as char } else { '=' });
    }
    out
}

/// The pictures *Upload…* takes, picked or dropped.
const PICTURE_FILES: drop_zone::Accept = drop_zone::Accept::one(&["jpg", "jpeg", "png"]);

/// Pick a picture and read it as a data URL ("" when cancelled).
async fn pick_picture() -> Result<String, String> {
    let Some(f) = rfd::AsyncFileDialog::new()
        .add_filter("Pictures", PICTURE_FILES.exts)
        .pick_file()
        .await
    else {
        return Ok(String::new());
    };
    let bytes = f.read().await;
    picture_data_url(&f.file_name(), &bytes)
}

/// A picked or dropped picture (its file name and bytes) as a data URL; over 15 MB is refused, as on the web.
fn picture_data_url(name: &str, bytes: &[u8]) -> Result<String, String> {
    if bytes.len() > 15 * 1024 * 1024 {
        return Err("Picture is larger than 15 MB".into());
    }
    let mime = if name.to_ascii_lowercase().ends_with(".png") { "image/png" } else { "image/jpeg" };
    Ok(format!("data:{mime};base64,{}", base64(bytes)))
}

fn match_search(state: &mut State, ctx: &Ctx) -> Task<Message> {
    let Some(Dialog::Match(m)) = &mut state.dialog else {
        return Task::none();
    };
    m.searching = true;
    m.preview = None;
    m.error.clear();
    let url = format!(
        "/library/albums/{}/match?provider={}&artist={}&album={}",
        state.id,
        m.provider,
        urlencode(&m.artist),
        urlencode(&m.album)
    );
    let api = ctx.api.clone();
    Task::perform(async move { api.get(&url).await }, Message::MatchResults)
}

fn match_preview(state: &mut State, rid: String, ctx: &Ctx) -> Task<Message> {
    let Some(Dialog::Match(m)) = &mut state.dialog else {
        return Task::none();
    };
    m.previewing = true;
    m.error.clear();
    let url = format!("/library/albums/{}/preview?release_id={}", state.id, urlencode(&rid));
    let api = ctx.api.clone();
    Task::perform(async move { api.get(&url).await }, Message::MatchPreview)
}

fn art_search(state: &mut State, ctx: &Ctx) -> Task<Message> {
    let Some(Dialog::Artwork(a)) = &mut state.dialog else {
        return Task::none();
    };
    a.searching = true;
    a.error.clear();
    let url = format!("/library/albums/{}/art-candidates?q={}", state.id, urlencode(&a.q));
    let api = ctx.api.clone();
    Task::perform(async move { api.get(&url).await }, Message::ArtResults)
}

fn set_art(state: &mut State, body: Value, ctx: &Ctx) -> Task<Message> {
    if let Some(Dialog::Artwork(a)) = &mut state.dialog {
        a.busy = true;
    }
    let api = ctx.api.clone();
    let path = format!("/library/albums/{}/art", state.id);
    Task::perform(async move { api.post(&path, body).await }, Message::ArtSet)
}

/// The links between an album's stereo and surround versions (web `album_versions`): a Surround
/// album names its stereo album; a stereo album its surround ones and a Dolby Atmos version being
/// fetched. (label, tooltip, the album it opens – None for a label only.)
fn version_links(a: &Value) -> Vec<(String, String, Option<String>)> {
    let mut out = Vec::new();
    if library::is_surround(a) {
        let stereo = fmt::s(&a["stereo"], "album_id");
        if !stereo.is_empty() {
            out.push(("Stereo version →".into(), "The stereo version of this album".into(), Some(stereo)));
        }
        return out;
    }
    let sur = &a["surround"];
    for h in fmt::items(&sur["have"]) {
        out.push((
            format!("{} version →", library::surround_label(&fmt::s(&h, "quality"))),
            format!("{} song(s) in the Surround library", fmt::s(&h, "tracks")),
            Some(fmt::s(&h, "album_id")),
        ));
    }
    let req = &sur["request"];
    if fmt::truthy(req, "id") {
        out.push((
            format!("Dolby Atmos version: {}", fmt::s(req, "status")),
            format!("Request #{}", fmt::s(req, "id")),
            None,
        ));
    }
    out
}

/// "Get Dolby Atmos version" on a stereo album (web `atmos_button`): its tooltip, for admins when
/// gamdl can fetch Atmos and no version is here or coming; None for no button.
fn atmos_button_tip(a: &Value, admin: bool) -> Option<&'static str> {
    let sur = &a["surround"];
    if !admin
        || library::is_surround(a)
        || !fmt::truthy(sur, "can_fetch")
        || !fmt::items(&sur["have"]).is_empty()
        || fmt::truthy(&sur["request"], "id")
    {
        return None;
    }
    Some(if sur["available"] == json!(false) {
        "Apple Music had no Dolby Atmos version when last asked – ask again"
    } else {
        "Download the Dolby Atmos version from Apple Music into the Surround library"
    })
}

/// `t.key || fallback`.
fn or_str(t: &Value, key: &str, fallback: &str) -> String {
    if fmt::truthy(t, key) {
        fmt::s(t, key)
    } else {
        fallback.to_string()
    }
}

pub fn update(state: &mut State, msg: Message, ctx: &mut Ctx) -> Task<Message> {
    match msg {
        Message::Loaded(seq, res) => {
            if seq != state.seq {
                return Task::none();
            }
            match res {
                Ok(a) => state.album = Some(a),
                Err(e) => {
                    state.album = Some(Value::Null);
                    state.error = e.message;
                }
            }
        }
        Message::Likes(Ok(v)) => {
            state.liked = fmt::items(&v).iter().filter_map(Value::as_i64).collect();
        }
        Message::Likes(Err(_)) => {}
        Message::Play(i) => {
            let q = queue_tracks(state);
            if !q.is_empty() {
                ctx.play(q, i);
            }
        }
        Message::PlayAll => {
            let q = queue_tracks(state);
            if !q.is_empty() {
                ctx.play(q, 0);
            }
        }
        Message::ShuffleAll => {
            let q = queue_tracks(state);
            if !q.is_empty() {
                ctx.play_shuffled(q);
            }
        }
        Message::Like(id) => {
            let has = state.liked.contains(&id);
            let api = ctx.api.clone();
            return Task::perform(
                async move {
                    if has {
                        api.del(&format!("/likes/{id}")).await
                    } else {
                        api.post_empty(&format!("/likes/{id}")).await
                    }
                },
                move |r| Message::Liked(id, !has, r),
            );
        }
        Message::Liked(id, now, Ok(_)) => {
            if now {
                state.liked.insert(id);
            } else {
                state.liked.remove(&id);
            }
            // the player's heart follows
            ctx.player_msg(crate::player::Message::RefreshLikes);
        }
        Message::Liked(_, _, Err(e)) => ctx.toast_err(e.message),
        Message::OpenArtist(name) => ctx.navigate(Page::ArtistName(name)),
        Message::OpenPage(p) => ctx.navigate(p),
        Message::Share => {
            state.menu = None;
            let api = ctx.api.clone();
            let path = format!("/library/albums/{}/share", state.id);
            return Task::perform(async move { api.post_empty(&path).await }, Message::ShareGot);
        }
        Message::SaveOffline => {
            state.menu = None;
            crate::desktop::offline::save(ctx, queue_tracks(state));
        }
        Message::ShareGot(Ok(r)) => {
            ctx.copy(format!("{}{}", ctx.base(), fmt::s(&r, "path")));
            ctx.toast_ok("Link copied – anyone with it can play this album");
        }
        Message::ShareGot(Err(e)) => ctx.toast_err(e.message),
        Message::Menu => state.menu = Some(ctx.cursor),
        Message::CloseMenu => state.menu = None,
        Message::AddToQueue => {
            state.menu = None;
            ctx.enqueue(queue_tracks(state));
        }
        Message::PlayNext => {
            state.menu = None;
            ctx.play_next(queue_tracks(state));
        }
        Message::AddToPlaylist => {
            state.menu = None;
            let ids: Vec<i64> = queue_tracks(state).iter().map(|t| fmt::i(t, "id")).collect();
            return library::open_add_to_playlist(&mut state.shared, ids, ctx).map(Message::Shared);
        }
        Message::ConfirmAll => {
            state.menu = None;
            let ids: Vec<Value> = queue_tracks(state).iter().map(|t| t["id"].clone()).collect();
            let api = ctx.api.clone();
            return Task::perform(
                async move { api.post("/library/verify-manual", json!({ "track_ids": ids })).await },
                Message::ConfirmedAll,
            );
        }
        Message::ConfirmedAll(Ok(_)) => {
            ctx.toast_ok("Album confirmed");
            return load(state, ctx);
        }
        Message::ConfirmedAll(Err(e)) => ctx.toast_err(e.message),
        Message::DownloadZip => {
            state.menu = None;
            let a = state.album.clone().unwrap_or(Value::Null);
            let name = library::safe_file_name(&format!("{} - {}.zip", fmt::s(&a, "albumartist"), fmt::s(&a, "album")));
            let api = ctx.api.clone();
            return Task::perform(
                library::save_as(api, format!("/library/albums/{}/download", state.id), name),
                Message::Zipped,
            );
        }
        Message::Zipped(Ok(path)) => {
            if !path.is_empty() {
                ctx.toast_ok(format!("Saved to {path}"));
            }
        }
        Message::Zipped(Err(e)) => ctx.toast_err(e),
        Message::AlbumLyrics => {
            state.menu = None;
            ctx.toast("Looking for lyrics…");
            let api = ctx.api.clone();
            let path = format!("/library/albums/{}/lyrics", state.id);
            return Task::perform(async move { api.post_empty(&path).await }, Message::AlbumLyricsDone);
        }
        Message::AlbumLyricsDone(Ok(r)) => ctx.toast_ok(if fmt::truthy(&r, "checked") {
            format!("Lyrics found for {} of {} song(s)", fmt::s(&r, "found"), fmt::s(&r, "checked"))
        } else {
            "Every song already has lyrics".to_string()
        }),
        Message::AlbumLyricsDone(Err(e)) => ctx.toast_err(e.message),
        Message::GetRest => {
            state.menu = None;
            let a = state.album.clone().unwrap_or(Value::Null);
            let rgid = fmt::s(&a, "mb_releasegroupid");
            if !rgid.is_empty() {
                ctx.navigate(Page::Album(rgid));
                return Task::none();
            }
            let artist = if fmt::truthy(&a, "albumartist") { fmt::s(&a, "albumartist") } else { fmt::s(&a, "artist") };
            let url = format!(
                "/discover/album-lookup?artist={}&album={}",
                urlencode(&artist),
                urlencode(&fmt::s(&a, "album"))
            );
            let api = ctx.api.clone();
            return Task::perform(async move { api.get(&url).await }, Message::RestLookup);
        }
        Message::RestLookup(Ok(r)) => {
            if fmt::truthy(&r, "rgid") {
                ctx.navigate(Page::Album(fmt::s(&r, "rgid")));
            } else {
                // not on MusicBrainz: another catalogue's track list, or one to type in (never a dead end)
                let a = state.album.clone().unwrap_or(Value::Null);
                let artist = if fmt::truthy(&a, "albumartist") { fmt::s(&a, "albumartist") } else { fmt::s(&a, "artist") };
                state.dialog = Some(Dialog::Rest(Box::new(super::rest_of_album::State::new(artist, fmt::s(&a, "album"), r))));
            }
        }
        Message::Rest(m) => {
            let Some(Dialog::Rest(rest)) = state.dialog.as_mut() else { return Task::none() };
            let (task, after) = super::rest_of_album::update(rest, m, ctx, Message::Rest);
            if matches!(after, super::rest_of_album::After::Close) {
                state.dialog = None;
            }
            return task;
        }
        Message::RestLookup(Err(e)) => ctx.toast(e.message),
        Message::ReleaseInfo => {
            state.menu = None;
            if let Some(a) = &state.album {
                let rgid = fmt::s(a, "mb_releasegroupid");
                if !rgid.is_empty() {
                    ctx.navigate(Page::Album(rgid));
                }
            }
        }
        Message::ReportAlbum => {
            state.menu = None;
            let title = state.album.as_ref().map(|a| fmt::s(a, "album")).unwrap_or_default();
            state.reporting_album = true;
            library::open_report(&mut state.shared, "album", json!(state.id), &title, "");
        }
        Message::EditOpen => {
            state.menu = None;
            let Some(a) = &state.album else {
                return Task::none();
            };
            let tracks = fmt::items(&a["tracks"])
                .iter()
                .map(|t| [or_str(t, "disc", "1"), or_str(t, "track", ""), fmt::s(t, "title"), fmt::s(t, "artist")])
                .collect();
            state.dialog = Some(Dialog::Edit(Box::new(EditState {
                fields: [fmt::s(a, "albumartist"), fmt::s(a, "album"), fmt::s(a, "year"), fmt::s(a, "genre")],
                tracks,
                rename: true,
                busy: false,
            })));
        }
        Message::EditField(i, v) => {
            if let Some(Dialog::Edit(e)) = &mut state.dialog {
                if let Some(f) = e.fields.get_mut(i) {
                    *f = v;
                }
            }
        }
        Message::EditTrack(ti, fi, v) => {
            if let Some(Dialog::Edit(e)) = &mut state.dialog {
                if let Some(f) = e.tracks.get_mut(ti).and_then(|r| r.get_mut(fi)) {
                    *f = v;
                }
            }
        }
        Message::EditRename(on) => {
            if let Some(Dialog::Edit(e)) = &mut state.dialog {
                e.rename = on;
            }
        }
        Message::EditSave => {
            let (Some(Dialog::Edit(e)), Some(a)) = (&mut state.dialog, &state.album) else {
                return Task::none();
            };
            if e.busy {
                return Task::none();
            }
            e.busy = true;
            let tracks = fmt::items(&a["tracks"]);
            let ids: Vec<Value> = tracks.iter().map(|t| t["id"].clone()).collect();
            let album_values = json!({
                "albumartist": e.fields[0].trim(), "album": e.fields[1].trim(),
                "date": e.fields[2].trim(), "genre": e.fields[3].trim()
            });
            let mut per_track: Vec<(Value, Value)> = Vec::new();
            for (t, row) in tracks.iter().zip(e.tracks.iter()) {
                let changed = row[2] != fmt::s(t, "title")
                    || row[3] != fmt::s(t, "artist")
                    || row[1] != or_str(t, "track", "")
                    || row[0] != or_str(t, "disc", "1");
                if changed {
                    per_track.push((
                        t["id"].clone(),
                        json!({ "discnumber": row[0].trim(), "tracknumber": row[1].trim(), "title": row[2].trim(), "artist": row[3].trim() }),
                    ));
                }
            }
            let rename = e.rename;
            let api = ctx.api.clone();
            return Task::perform(
                async move {
                    let mut out = api
                        .post("/library/tags", json!({ "track_ids": ids, "values": album_values }))
                        .await?;
                    for (id, values) in per_track {
                        out = api
                            .post("/library/tags", json!({ "track_ids": [id], "values": values }))
                            .await?;
                    }
                    if rename {
                        out = api
                            .post("/library/tags", json!({ "track_ids": ids, "values": {}, "rename": true }))
                            .await?;
                    }
                    Ok(out)
                },
                Message::EditSaved,
            );
        }
        Message::EditSaved(Ok(out)) => {
            state.dialog = None;
            ctx.toast_ok("Saved");
            let new_id = fmt::s(&out, "album_id");
            if fmt::truthy(&out, "album_id") && new_id != state.id {
                ctx.navigate(Page::LibraryAlbum(new_id));
            } else {
                return load(state, ctx);
            }
        }
        Message::EditSaved(Err(e)) => {
            if let Some(Dialog::Edit(ed)) = &mut state.dialog {
                ed.busy = false;
            }
            ctx.toast_err(e.message);
        }
        Message::MatchOpen => {
            state.menu = None;
            let Some(a) = &state.album else {
                return Task::none();
            };
            let artist0 = if a["albumartist_orig"].is_null() {
                fmt::s(a, "albumartist")
            } else {
                fmt::s(a, "albumartist_orig")
            };
            state.dialog = Some(Dialog::Match(Box::new(MatchState {
                provider: "musicbrainz".into(),
                artist: if artist0 == "Unknown Artist" { String::new() } else { artist0 },
                album: fmt::s(a, "album"),
                link: String::new(),
                searching: false,
                candidates: None,
                track_count: String::new(),
                error: String::new(),
                preview: None,
                previewing: false,
                rename: true,
                applying: false,
            })));
            return match_search(state, ctx);
        }
        Message::MatchProvider(p) => {
            if let Some(Dialog::Match(m)) = &mut state.dialog {
                m.provider = p;
                return match_search(state, ctx);
            }
        }
        Message::MatchArtist(v) => {
            if let Some(Dialog::Match(m)) = &mut state.dialog {
                m.artist = v;
            }
        }
        Message::MatchAlbum(v) => {
            if let Some(Dialog::Match(m)) = &mut state.dialog {
                m.album = v;
            }
        }
        Message::MatchLink(v) => {
            if let Some(Dialog::Match(m)) = &mut state.dialog {
                m.link = v;
            }
        }
        Message::MatchSearch => {
            if let Some(Dialog::Match(m)) = &state.dialog {
                if let Some(rid) = parse_release_link(&m.link) {
                    return match_preview(state, rid, ctx);
                }
                return match_search(state, ctx);
            }
        }
        Message::MatchResults(r) => {
            if let Some(Dialog::Match(m)) = &mut state.dialog {
                m.searching = false;
                match r {
                    Ok(d) => {
                        m.track_count = fmt::s(&d, "track_count");
                        m.candidates = Some(fmt::items(&d["candidates"]));
                    }
                    Err(e) => m.error = e.message,
                }
            }
        }
        Message::MatchPick(rid) => return match_preview(state, rid, ctx),
        Message::MatchPreview(r) => {
            if let Some(Dialog::Match(m)) = &mut state.dialog {
                m.previewing = false;
                match r {
                    Ok(pv) => m.preview = Some(pv),
                    Err(e) => m.error = e.message,
                }
            }
        }
        Message::MatchBack => {
            if let Some(Dialog::Match(m)) = &mut state.dialog {
                m.preview = None;
                return match_search(state, ctx);
            }
        }
        Message::MatchRename(on) => {
            if let Some(Dialog::Match(m)) = &mut state.dialog {
                m.rename = on;
            }
        }
        Message::MatchApply => {
            if let Some(Dialog::Match(m)) = &mut state.dialog {
                let Some(pv) = &m.preview else {
                    return Task::none();
                };
                if m.applying {
                    return Task::none();
                }
                m.applying = true;
                let rid = fmt::s(&pv["release"], "id");
                let rid = if rid.is_empty() { fmt::s(pv, "release_id") } else { rid };
                let provider = or_str(&pv["release"], "provider", "MusicBrainz");
                let body = json!({ "release_id": rid, "rename": m.rename });
                let api = ctx.api.clone();
                let path = format!("/library/albums/{}/retag", state.id);
                return Task::perform(async move { api.post(&path, body).await }, move |r| {
                    Message::MatchApplied(provider.clone(), r)
                });
            }
        }
        Message::MatchApplied(provider, Ok(out)) => {
            state.dialog = None;
            ctx.toast_ok(format!("Album tagged from {provider}"));
            let new_id = fmt::s(&out, "album_id");
            if fmt::truthy(&out, "album_id") && new_id != state.id {
                ctx.navigate(Page::LibraryAlbum(new_id));
            } else {
                return load(state, ctx);
            }
        }
        Message::MatchApplied(_, Err(e)) => {
            if let Some(Dialog::Match(m)) = &mut state.dialog {
                m.applying = false;
            }
            ctx.toast_err(e.message);
        }
        Message::ArtOpen => {
            state.menu = None;
            let q = state.album.as_ref().map(|a| fmt::s(a, "album")).unwrap_or_default();
            state.dialog = Some(Dialog::Artwork(Box::new(ArtState {
                q,
                searching: false,
                candidates: None,
                error: String::new(),
                busy: false,
                drop: DropZone::default(),
            })));
            return art_search(state, ctx);
        }
        Message::ArtQuery(q) => {
            if let Some(Dialog::Artwork(a)) = &mut state.dialog {
                a.q = q;
            }
        }
        Message::ArtSearch => return art_search(state, ctx),
        Message::ArtResults(r) => {
            if let Some(Dialog::Artwork(a)) = &mut state.dialog {
                a.searching = false;
                match r {
                    Ok(c) => a.candidates = Some(fmt::items(&c)),
                    Err(e) => a.error = e.message,
                }
            }
        }
        Message::ArtPick(i) => {
            let url = match &state.dialog {
                Some(Dialog::Artwork(a)) if !a.busy => a
                    .candidates
                    .as_ref()
                    .and_then(|c| c.get(i))
                    .map(|x| x["url"].clone()),
                _ => None,
            };
            if let Some(url) = url {
                return set_art(state, json!({ "url": url }), ctx);
            }
        }
        Message::ArtUpload => return Task::perform(pick_picture(), Message::ArtFile),
        Message::ArtDrop(ev) => {
            let Some(Dialog::Artwork(a)) = &mut state.dialog else { return Task::none() };
            if a.busy {
                a.drop.hovering = false;
                return Task::none();
            }
            match a.drop.handle(ev, &PICTURE_FILES) {
                Outcome::Take(path) => {
                    return Task::perform(
                        async move {
                            let bytes = tokio::fs::read(&path).await.map_err(|e| e.to_string())?;
                            picture_data_url(&path.to_string_lossy(), &bytes)
                        },
                        Message::ArtFile,
                    );
                }
                Outcome::Refused(m) => ctx.toast_err(m),
                Outcome::Nothing => {}
            }
        }
        Message::ArtFile(Ok(data)) => {
            if !data.is_empty() {
                return set_art(state, json!({ "data": data }), ctx);
            }
        }
        Message::ArtFile(Err(e)) => ctx.toast_err(e),
        Message::ArtSet(Ok(r)) => {
            state.dialog = None;
            ctx.toast_ok(format!("Artwork embedded in {} file(s)", fmt::s(&r, "embedded")));
            return load(state, ctx);
        }
        Message::ArtSet(Err(e)) => {
            if let Some(Dialog::Artwork(a)) = &mut state.dialog {
                a.busy = false;
            }
            ctx.toast_err(e.message);
        }
        Message::FindArt => {
            state.menu = None;
            let api = ctx.api.clone();
            let path = format!("/library/albums/{}/artwork", state.id);
            return Task::perform(async move { api.post_empty(&path).await }, Message::FoundArt);
        }
        Message::FoundArt(Ok(r)) => {
            ctx.toast_ok(format!("Artwork embedded in {} file(s)", fmt::s(&r, "embedded")));
            return load(state, ctx);
        }
        Message::FoundArt(Err(e)) => ctx.toast_err(e.message),
        Message::TypeOpen => {
            state.menu = None;
            let chosen = state.album.as_ref().map(|a| fmt::s(&a["type"], "chosen")).unwrap_or_default();
            let picked = ["album", "ep", "single", "one_off"].into_iter().find(|k| *k == chosen).unwrap_or("");
            state.dialog = Some(Dialog::ChangeType { picked, busy: false });
        }
        Message::TypePick(v) => {
            if let Some(Dialog::ChangeType { picked, .. }) = &mut state.dialog {
                *picked = v;
            }
        }
        Message::TypeSave => {
            if let Some(Dialog::ChangeType { picked, busy }) = &mut state.dialog {
                if *busy {
                    return Task::none();
                }
                *busy = true;
                let format = if picked.is_empty() { Value::Null } else { json!(picked) };
                let api = ctx.api.clone();
                let path = format!("/library/albums/{}/type", state.id);
                return Task::perform(async move { api.put(&path, json!({ "format": format })).await }, Message::TypeSaved);
            }
        }
        Message::TypeSaved(Ok(_)) => {
            state.dialog = None;
            ctx.toast_ok(lt::TYPE_SAVED);
            return load(state, ctx);
        }
        Message::TypeSaved(Err(e)) => {
            if let Some(Dialog::ChangeType { busy, .. }) = &mut state.dialog {
                *busy = false;
            }
            ctx.toast_err(e.message);
        }
        Message::TrashAsk => {
            state.menu = None;
            let api = ctx.api.clone();
            let id = state.id.clone();
            return Task::perform(
                async move { api.post("/library/albums/trash", json!({ "ids": [id], "dry_run": true })).await },
                Message::TrashDry,
            );
        }
        Message::TrashDry(Ok(d)) => {
            let (title, text, can_go) = lt::trash_dialog_words(&d);
            state.dialog = Some(Dialog::Trash { title, text, can_go, keep: lt::keeps(&d) });
        }
        Message::TrashDry(Err(e)) => ctx.toast_err(e.message),
        Message::TrashGo => {
            state.dialog = None;
            let api = ctx.api.clone();
            let path = format!("/library/albums/{}", state.id);
            return Task::perform(async move { api.del(&path).await }, Message::Trashed);
        }
        Message::Trashed(Ok(r)) => {
            let text = if fmt::truthy(&r, "text") { fmt::s(&r, "text") } else { lt::moved_text(1, 1, "") };
            if lt::keeps(&r) {
                ctx.toast_action(text, lt::OPEN_TRASH, crate::player::Message::OpenTrash, lt::TOAST_MS);
            } else {
                ctx.toast_ok(text);
            }
            ctx.navigate(Page::library(""));
        }
        Message::Trashed(Err(e)) => ctx.toast_err(e.message),
        Message::TrackMenu(i) => {
            if let Some(t) = queue_tracks(state).get(i) {
                return library::track_menu(&mut state.shared, t, ctx).map(Message::Shared);
            }
        }
        Message::OpenVersion(id) => ctx.navigate(Page::LibraryAlbum(id)),
        Message::Suspect(i) => {
            if let Some(t) = queue_tracks(state).get(i).cloned() {
                return library::shared_update(&mut state.shared, Act::SuspectQuality(Box::new(t)), ctx)
                    .map(Message::Shared);
            }
        }
        Message::CloseDialog => state.dialog = None,
        Message::Shared(a) => {
            if matches!(a, Act::CloseDialog) {
                state.reporting_album = false;
            }
            let task = library::shared_update(&mut state.shared, a, ctx).map(Message::Shared);
            if state.shared.take_changed() {
                if state.reporting_album {
                    state.reporting_album = false;
                    ctx.navigate(Page::library(""));
                    return task;
                }
                if let Some(new) = state.shared.moved_to.take() {
                    if new != state.id {
                        ctx.navigate(Page::LibraryAlbum(new));
                        return task;
                    }
                }
                return Task::batch([task, load(state, ctx)]);
            }
            return task;
        }
    }
    Task::none()
}

/// `sourceLine(t)`: where a song came from.
fn source_line(t: &Value) -> String {
    if !fmt::truthy(t, "source") {
        return String::new();
    }
    let mut s = format!("⬇ {}", fmt::s(t, "source"));
    if fmt::truthy(t, "source_detail") {
        s.push_str(&format!(" · {}", fmt::s(t, "source_detail")));
    }
    if fmt::truthy(t, "downloaded_at") {
        s.push_str(&format!(" · {}", fmt::ago(fmt::f(t, "downloaded_at"))));
    }
    // who asked for it: kept on the song since 0.36, so Clear finished doesn't lose the name
    let (by, at) = lt::requester(t);
    if !by.is_empty() {
        s.push_str(&format!(" · requested by {by}"));
        if at > 0.0 {
            s.push_str(&format!(" on {}", fmt::date(at)));
        }
    }
    s
}

pub fn view<'a>(state: &'a State, ctx: &Ctx<'a>) -> El<'a, Message> {
    let p = ctx.look;
    let admin = ctx.is_admin();
    let a = match &state.album {
        None => return column![skeleton_rows(8)].into(),
        Some(Value::Null) => return widgets::error_box(&state.error),
        Some(a) => a,
    };
    // a Surround album: its own songs, linked to the stereo album; the stereo library's tools stay off
    let surround = library::is_surround(a);
    let tracks = fmt::items(&a["tracks"]);
    let multi = tracks
        .iter()
        .map(|t| fmt::s(t, "disc"))
        .collect::<HashSet<_>>()
        .len()
        > 1;
    let mut formats: Vec<String> = Vec::new();
    for t in &tracks {
        let f = fmt::s(t, "format").to_uppercase();
        if !formats.contains(&f) {
            formats.push(f);
        }
    }
    let untagged = tracks.iter().filter(|t| !fmt::truthy(t, "mb_albumid")).count();
    // who added it (0.36): the server's groups, kept on the songs; an older server: its requesters
    let history = lt::album_history(a, fmt::date);

    // the header
    let cover: El<'a, Message> = match ctx.cover(&state.id, &a["cover_v"], 600) {
        Some(h) => image(h)
            .width(220)
            .height(220)
            .content_fit(ContentFit::Cover)
            .border_radius(14.0)
            .into(),
        None => container(icon(Icon::Disc, 48.0, p.faint))
            .width(220)
            .height(220)
            .center(220)
            .style(theme::art)
            .into(),
    };
    let album_name = if fmt::truthy(a, "album") { tr(ctx, a, "album") } else { "Unknown album".to_string() };
    let artist_name = fmt::s(a, "albumartist");
    let mut meta = String::new();
    if fmt::truthy(a, "year") {
        meta.push_str(&format!(" · {}", fmt::s(a, "year")));
    }
    meta.push_str(&format!(
        " · {} tracks · {} · {}",
        tracks.len(),
        long_dur(fmt::f(a, "duration")),
        size_text(fmt::f(a, "size"))
    ));
    let artist_line = row![
        button(t(tr(ctx, a, "albumartist")).size(14).font(theme::semibold()))
            .padding(0)
            .style(theme::btn(Btn::Row))
            .on_press(Message::OpenArtist(artist_name.clone())),
        widgets::muted(meta).size(14),
    ]
    .spacing(0)
    .align_y(Alignment::Center);
    let quality = if surround { fmt::s(a, "quality") } else { formats.join(" / ") };
    let mut chips = row![].spacing(6);
    // its type (0.36): "Album", "Live · EP", "Compilation · One-off"; admins see why on hover
    if a["type"].is_object() {
        let t = &a["type"];
        let chip = badge(fmt::s(t, "label"), Badge::Accent);
        chips = chips.push(if admin && fmt::truthy(t, "why_text") {
            tooltip(chip, widgets::tip_box(fmt::s(t, "why_text")), tooltip::Position::Top).gap(4).into()
        } else {
            chip
        });
    }
    chips = chips.push(badge(quality, Badge::Plain));
    if fmt::truthy(a, "genre") {
        chips = chips.push(badge(fmt::s(a, "genre"), Badge::Plain));
    }
    if surround {
        if fmt::truthy(a, "atmos") {
            chips = chips.push(badge("Dolby Atmos", Badge::Accent));
        }
    } else {
        chips = chips.push(if untagged > 0 {
            badge(format!("{untagged} track(s) not MusicBrainz-tagged"), Badge::Warn)
        } else {
            badge("MusicBrainz tagged", Badge::Ok)
        });
    }
    for (label, tip, to) in version_links(a) {
        let chip: El<'a, Message> = match to {
            Some(id) => button(badge(label, if surround { Badge::Plain } else { Badge::Accent }))
                .padding(0)
                .style(theme::btn(Btn::Row))
                .on_press(Message::OpenVersion(id))
                .into(),
            None => badge(label, Badge::Plain),
        };
        chips = chips.push(tooltip(chip, widgets::tip_box(tip), tooltip::Position::Top).gap(4));
    }
    let mut actions = row![
        btn_icon(Icon::Play, "Play", Btn::Primary, Some(Message::PlayAll)),
        btn_icon(Icon::Shuffle, "Shuffle", Btn::Default, Some(Message::ShuffleAll)),
        tooltip(
            btn_icon(Icon::External, "Share", Btn::Default, Some(Message::Share)),
            widgets::tip_box("Share link"),
            tooltip::Position::Bottom
        )
        .gap(4),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    if admin && !surround {
        actions = actions.push(
            tooltip(
                btn_icon(Icon::Edit, "Edit details", Btn::Default, Some(Message::EditOpen)),
                widgets::tip_box("Change the album, artist and song names"),
                tooltip::Position::Bottom,
            )
            .gap(4),
        );
    }
    if let Some(tip) = atmos_button_tip(a, admin) {
        let path = format!("/library/albums/{}/surround", state.id);
        actions = actions.push(
            tooltip(
                btn_icon(Icon::Download, "Get Dolby Atmos version", Btn::Default, Some(Message::Shared(Act::GetAtmos(path)))),
                widgets::tip_box(tip),
                tooltip::Position::Bottom,
            )
            .gap(4),
        );
    }
    actions = actions.push(icon_btn(Icon::More, "More options", Some(Message::Menu)));
    let kicker = if surround { "Surround album · In your library" } else { "Album · In your library" };
    let mut info = column![
        small_muted(kicker).font(theme::semibold()),
        h1(album_name),
        artist_line,
    ]
    .spacing(6)
    .width(Length::Fill);
    if !history.is_empty() {
        info = info.push(small_muted(history));
    }
    info = info.push(container(chips.wrap()).padding([6, 0]));
    info = info.push(actions.wrap());
    // a right-click on the header opens "More options" at the pointer (the web's album header)
    let head = iced::widget::mouse_area(row![cover, info].spacing(24).align_y(Alignment::End)).on_right_press(Message::Menu);

    // the songs
    let cur = ctx.player.current_id();
    let mut list = column![];
    for (i, tr_) in tracks.iter().enumerate() {
        let id = fmt::i(tr_, "id");
        let number = format!(
            "{}{}",
            if multi { format!("{}-", fmt::s(tr_, "disc")) } else { String::new() },
            if fmt::truthy(tr_, "track") { fmt::s(tr_, "track") } else { (i + 1).to_string() }
        );
        let mut r = TrackRow::new(tr(ctx, tr_, "title"))
            .label(number)
            .sub_links(crate::widgets::catalog::credit_parts(tr_, &tr(ctx, tr_, "artist")).into_iter().map(|(x, p)| (x, p.map(Message::OpenPage))).collect(), "")
            .quality(if surround && fmt::truthy(tr_, "quality") { fmt::s(tr_, "quality") } else { format_text(tr_) })
            .duration(Some(fmt::f(tr_, "duration")))
            .playing(cur == Some(id))
            .on_play(Message::Play(i))
            .on_menu(Message::TrackMenu(i));
        // Surround songs have no likes
        if !surround {
            r = r.liked(state.liked.contains(&id), Message::Like(id));
        }
        if admin {
            r = r.note(fmt::s(tr_, "relpath"));
        }
        r = r.note(source_line(tr_));
        if fmt::truthy(tr_, "surround_id") {
            r = r.badge(
                tooltip(
                    badge("Surround", Badge::Accent),
                    widgets::tip_box("Has a surround version (⋯ → Play the surround version)"),
                    tooltip::Position::Top,
                )
                .gap(4)
                .into(),
            );
        }
        if !surround && !fmt::truthy(tr_, "mb_trackid") {
            r = r.badge(
                tooltip(badge("?", Badge::Warn), widgets::tip_box("No MusicBrainz id"), tooltip::Position::Top)
                    .gap(4)
                    .into(),
            );
        }
        r = r.badge_maybe(verify_badge(tr_));
        if fmt::s(tr_, "quality_status") == "suspect" {
            r = r.extra(
                button(badge("Suspect quality", Badge::Warn))
                    .padding(0)
                    .style(theme::btn(Btn::Row))
                    .on_press(Message::Suspect(i))
                    .into(),
            );
        }
        list = list.push(r.view());
    }
    let songs: El<'a, Message> = if tracks.is_empty() {
        empty(Icon::Music, "No songs found", "")
    } else {
        widgets::card_flat(list).width(Length::Fill).into()
    };
    column![head, songs].spacing(22).width(Length::Fill).into()
}

/// The album's ⋯ menu (web `album_menu`).
fn album_menu<'a>(state: &State, ctx: &Ctx<'a>, at: Point) -> El<'a, Message> {
    let admin = ctx.is_admin();
    let a = state.album.clone().unwrap_or(Value::Null);
    // a Surround album's (web `surround_album_menu`): playing it; the stereo library's tools don't apply
    if library::is_surround(&a) {
        let mut items = vec![
            MenuItem::new("Add to queue", Message::AddToQueue).icon(Icon::List),
            MenuItem::new("Play next", Message::PlayNext).icon(Icon::Next),
        ];
        let stereo = fmt::s(&a["stereo"], "album_id");
        if !stereo.is_empty() {
            items.push(MenuItem::new("Go to the stereo version", Message::OpenVersion(stereo)).icon(Icon::Disc));
        }
        return pop_menu(at, items, Message::CloseMenu, ctx.size);
    }
    let mut items = vec![
        MenuItem::new("Add to queue", Message::AddToQueue).icon(Icon::List),
        MenuItem::new("Play next", Message::PlayNext).icon(Icon::Next),
        MenuItem::new("Add to playlist", Message::AddToPlaylist).icon(Icon::Plus),
        MenuItem::new("Share link", Message::Share).icon(Icon::External),
        MenuItem::new("Save album for offline", Message::SaveOffline).icon(Icon::Download),
    ];
    if admin {
        items.push(MenuItem::new("All songs are right (confirm)", Message::ConfirmAll).icon(Icon::Check));
    }
    items.push(MenuItem::new("Download album (zip)", Message::DownloadZip).icon(Icon::Download));
    items.push(MenuItem::new("Lyrics for all songs", Message::AlbumLyrics).icon(Icon::Music));
    items.push(MenuItem::new("Get the rest of the album", Message::GetRest).icon(Icon::Download));
    if fmt::truthy(&a, "mb_releasegroupid") {
        items.push(MenuItem::new("Release info", Message::ReleaseInfo).icon(Icon::Disc));
    }
    items.push(MenuItem::new("Report a problem", Message::ReportAlbum).icon(Icon::Shield));
    // the admin's part, in the web's order: Change type… (0.36) after Edit details; the album goes to the Trash
    // of Files, restorable there
    for label in lt::album_admin_items(admin, a["type"].is_object()) {
        items.push(match label {
            "Tag / match metadata" => MenuItem::new(label, Message::MatchOpen).icon(Icon::Tag),
            "Edit details (album, artist, songs)" => MenuItem::new(label, Message::EditOpen).icon(Icon::Edit),
            lt::CHANGE_TYPE => MenuItem::new(label, Message::TypeOpen).icon(Icon::Tag),
            "Change artwork…" => MenuItem::new(label, Message::ArtOpen).icon(Icon::Disc),
            "Find artwork automatically" => MenuItem::new(label, Message::FindArt).icon(Icon::Refresh),
            _ => MenuItem::new(label, Message::TrashAsk).icon(Icon::Trash).danger(),
        });
    }
    pop_menu(at, items, Message::CloseMenu, ctx.size)
}

fn input<'a>(placeholder: &str, value: &str, on: impl Fn(String) -> Message + 'a) -> text_input::TextInput<'a, Message> {
    text_input(placeholder, value)
        .on_input(on)
        .padding([8, 11])
        .size(14)
        .style(theme::input)
}

fn edit_dialog<'a>(e: &'a EditState) -> El<'a, Message> {
    let field = |i: usize, label: &'static str| -> El<'a, Message> {
        widgets::field(label, input("", &e.fields[i], move |v| Message::EditField(i, v)))
    };
    let mut rows = column![
        row![
            small_muted("Disc").width(70),
            small_muted("#").width(70),
            small_muted("Title").width(Length::FillPortion(3)),
            small_muted("Artist").width(Length::FillPortion(2)),
        ]
        .spacing(8),
        widgets::divider(),
    ]
    .spacing(6);
    for (ti, tr_) in e.tracks.iter().enumerate() {
        rows = rows.push(
            row![
                container(input("", &tr_[0], move |v| Message::EditTrack(ti, 0, v))).width(70),
                container(input("", &tr_[1], move |v| Message::EditTrack(ti, 1, v))).width(70),
                container(input("", &tr_[2], move |v| Message::EditTrack(ti, 2, v))).width(Length::FillPortion(3)),
                container(input("", &tr_[3], move |v| Message::EditTrack(ti, 3, v))).width(Length::FillPortion(2)),
            ]
            .spacing(8),
        );
    }
    let body = column![
        widgets::muted("Type what this album really is. The names are written into the files, and automatic tagging won't change them back.").size(13),
        row![field(0, "Album artist"), field(1, "Album")].spacing(12),
        row![field(2, "Year / date"), field(3, "Genre")].spacing(12),
        widgets::card(rows).width(Length::Fill),
        toggler(e.rename)
            .label("Move and rename the files to match (naming template)")
            .text_size(13.5)
            .spacing(10)
            .on_toggle(Message::EditRename)
            .style(theme::switch),
    ]
    .spacing(14);
    let footer = row![
        btn("Cancel", Btn::Default, Some(Message::CloseDialog)),
        btn("Save", Btn::Primary, (!e.busy).then_some(Message::EditSave)),
    ]
    .spacing(10);
    modal("Edit details", body, Some(footer.into()), true, Message::CloseDialog)
}

fn match_dialog<'a>(m: &'a MatchState, ctx: &Ctx<'a>) -> El<'a, Message> {
    let p = ctx.look;
    let thumb = |cover: &str, size: f32| -> El<'a, Message> {
        match ctx.picture(cover) {
            Some(h) => image(h)
                .width(size)
                .height(size)
                .content_fit(ContentFit::Cover)
                .border_radius(8.0)
                .into(),
            None => container(icon(Icon::Disc, size * 0.4, p.faint))
                .width(size)
                .height(size)
                .center(size)
                .style(theme::art)
                .into(),
        }
    };
    let join = |v: &Value, keys: &[&str]| -> String {
        keys.iter()
            .filter(|k| fmt::truthy(v, k))
            .map(|k| fmt::s(v, k))
            .collect::<Vec<_>>()
            .join(" · ")
    };
    let pct_badge = |x: f64, text: String| -> El<'a, Message> {
        badge(text, if x > 0.85 { Badge::Ok } else if x > 0.6 { Badge::Warn } else { Badge::Err })
    };
    let results: El<'a, Message> = if let Some(pv) = &m.preview {
        let r = &pv["release"];
        let conf = fmt::f(pv, "confidence");
        let provider = or_str(r, "provider", "MusicBrainz");
        let mut rows = column![
            row![
                small_muted("Current").width(Length::FillPortion(2)),
                small_muted(format!("→ {provider}")).width(Length::FillPortion(2)),
                small_muted("New path").width(Length::FillPortion(3)),
            ]
            .spacing(10),
            widgets::divider(),
        ]
        .spacing(6);
        for row_ in fmt::items(&pv["rows"]) {
            let old_track = if row_["old_track"].is_null() { "–".to_string() } else { fmt::s(&row_, "old_track") };
            let old = column![
                t(format!("{old_track}. {}", fmt::s(&row_, "old_title"))).size(13),
                widgets::faint(fmt::s(&row_, "old_artist")).size(11.5),
            ]
            .spacing(2)
            .width(Length::FillPortion(2));
            let new: El<'a, Message> = if fmt::truthy(&row_, "new_title") {
                let diff = row_["new_title"] != row_["old_title"];
                column![
                    t(format!(
                        "{}{}. {}",
                        if fmt::f(&row_, "new_disc") > 1.0 { format!("{}-", fmt::s(&row_, "new_disc")) } else { String::new() },
                        fmt::s(&row_, "new_track"),
                        fmt::s(&row_, "new_title")
                    ))
                    .size(13)
                    .color(if diff { p.accent } else { p.text }),
                    widgets::faint(fmt::s(&row_, "new_artist")).size(11.5),
                ]
                .spacing(2)
                .width(Length::FillPortion(2))
                .into()
            } else {
                container(badge("no match – left untouched", Badge::Err))
                    .width(Length::FillPortion(2))
                    .into()
            };
            rows = rows.push(
                row![
                    old,
                    new,
                    widgets::faint(fmt::s(&row_, "new_path"))
                        .size(11.5)
                        .font(theme::mono())
                        .width(Length::FillPortion(3)),
                ]
                .spacing(10)
                .align_y(Alignment::Center),
            );
        }
        column![
            row![
                thumb(&fmt::s(r, "cover"), 64.0),
                column![h3(fmt::s(r, "title")), small_muted(join(r, &["artist", "date", "country", "label"]))]
                    .spacing(3)
                    .width(Length::Fill),
                pct_badge(conf, format!("Confidence {}%", (conf * 100.0).round() as i64)),
            ]
            .spacing(12)
            .align_y(Alignment::Center),
            widgets::card(rows).width(Length::Fill),
            row![
                toggler(m.rename)
                    .label("Rename & move files using naming template")
                    .text_size(13.5)
                    .spacing(10)
                    .on_toggle(Message::MatchRename)
                    .style(theme::switch),
                space().width(Length::Fill),
                btn("Back", Btn::Default, Some(Message::MatchBack)),
                btn_icon(Icon::Check, if m.applying { "Writing tags…" } else { "Apply tags" }, Btn::Primary, (!m.applying).then_some(Message::MatchApply)),
            ]
            .spacing(10)
            .align_y(Alignment::Center),
        ]
        .spacing(14)
        .into()
    } else if m.searching || m.previewing {
        widgets::muted(if m.provider == "fingerprint" && m.searching {
            "Listening to every song… this takes a few seconds per song"
        } else {
            "Searching…"
        })
        .size(14)
        .into()
    } else if !m.error.is_empty() {
        widgets::error_box(&m.error)
    } else {
        match &m.candidates {
            None => column![].into(),
            Some(c) if c.is_empty() => empty(
                Icon::Search,
                "No releases found",
                "Try editing the artist/album, or paste a release ID.",
            ),
            Some(cands) => {
                let mut list = column![];
                for c in cands {
                    let mt = fmt::f(c, "match");
                    let mut right: Vec<El<'a, Message>> = Vec::new();
                    if fmt::truthy(c, "track_count") {
                        right.push(badge(
                            format!("{} tracks", fmt::s(c, "track_count")),
                            if fmt::s(c, "track_count") == m.track_count { Badge::Ok } else { Badge::Warn },
                        ));
                    }
                    right.push(pct_badge(mt, format!("{}%", (mt * 100.0).round() as i64)));
                    list = list.push(widgets::list_row(
                        Some(thumb(&fmt::s(c, "cover"), 44.0)),
                        format!("{} · {}", fmt::s(c, "title"), fmt::s(c, "artist")),
                        join(c, &["provider", "date", "country", "formats", "label", "status", "type"]),
                        right,
                        Some(Message::MatchPick(fmt::s(c, "id"))),
                    ));
                }
                column![
                    small_muted(format!("Your files: {} tracks. Pick the matching release:", m.track_count)),
                    widgets::card_flat(list).width(Length::Fill),
                ]
                .spacing(8)
                .into()
            }
        }
    };
    let body = column![
        tabs(&PROVIDERS, &m.provider, Message::MatchProvider),
        small_muted(match_help(&m.provider)),
        row![
            container(input("Artist", &m.artist, Message::MatchArtist).on_submit(Message::MatchSearch)).width(Length::FillPortion(2)),
            container(input("Album", &m.album, Message::MatchAlbum).on_submit(Message::MatchSearch)).width(Length::FillPortion(3)),
            btn_icon(Icon::Search, "Search", Btn::Default, Some(Message::MatchSearch)),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
        input("…or paste a MusicBrainz / Deezer / Apple / Discogs link", &m.link, Message::MatchLink)
            .on_submit(Message::MatchSearch),
        results,
    ]
    .spacing(12);
    modal("Tag this album", body, None, true, Message::CloseDialog)
}

fn art_dialog<'a>(a: &'a ArtState, ctx: &Ctx<'a>) -> El<'a, Message> {
    let out: El<'a, Message> = if a.searching {
        widgets::muted("Searching every catalogue…").size(14).into()
    } else if !a.error.is_empty() {
        widgets::error_box(&a.error)
    } else {
        match &a.candidates {
            None => column![].into(),
            Some(c) if c.is_empty() => widgets::muted("No artwork found – try another title, or upload a picture.")
                .size(14)
                .into(),
            Some(c) => grid(
                c.iter()
                    .enumerate()
                    .map(|(i, x)| {
                        let mut title = fmt::s(x, "source");
                        if fmt::truthy(x, "size") {
                            title.push_str(&format!(" · {}", fmt::s(x, "size")));
                        }
                        Tile::new(title, TileKind::Album, Message::ArtPick(i))
                            .sub(fmt::s(x, "label"))
                            .cover(ctx.picture(&fmt::s(x, "thumb")))
                            .width(140.0)
                            .view()
                    })
                    .collect(),
            ),
        }
    };
    let body = column![
        row![
            container(input("Album title to search for", &a.q, Message::ArtQuery).on_submit(Message::ArtSearch)).width(Length::Fill),
            btn("Search", Btn::Primary, (!a.busy).then_some(Message::ArtSearch)),
            btn("Upload…", Btn::Default, (!a.busy).then_some(Message::ArtUpload)),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
        small_muted("Cover Art Archive, Deezer, Apple Music, YouTube Music and Discogs. The picture is embedded in every song and saved as cover.jpg. Or drop a .jpg or .png here."),
        out,
    ]
    .spacing(12);
    let body = drop_zone::view(body, &a.drop, "Drop a .jpg or .png to use it as the cover");
    modal("Change artwork", body, None, true, Message::CloseDialog)
}

/// Change type… (0.36): Album, EP, Single, One-off (with what a One-off is), or Automatic with what the server
/// guesses and why; the current one is checked. Save → PUT …/type.
fn change_type_dialog<'a>(state: &'a State, picked: &'static str, busy: bool) -> El<'a, Message> {
    let a = state.album.as_ref().unwrap_or(&Value::Null);
    let mut opts = column![].spacing(10);
    for (v, label) in lt::type_choices(&a["type"]) {
        let r = radio(label, v, Some(picked), Message::TypePick).size(16).text_size(13.5).spacing(8);
        opts = opts.push(if v == "one_off" {
            column![r, container(small_muted(lt::ONE_OFF_HELP)).padding(iced::Padding::ZERO.left(24))].spacing(3).into()
        } else {
            El::from(r)
        });
    }
    let body = column![widgets::muted(lt::what_is(&fmt::s(a, "album"))).size(13.5), opts].spacing(14);
    let footer = row![
        btn("Cancel", Btn::Default, Some(Message::CloseDialog)),
        btn("Save", Btn::Primary, (!busy).then_some(Message::TypeSave)),
    ]
    .spacing(10);
    modal(lt::CHANGE_TYPE_TITLE, body, Some(footer.into()), false, Message::CloseDialog)
}

/// Delete album… (0.36): the dry run's words; Move to Trash (Delete while deletes are for good) / Cancel, or Close
/// when there is no room.
fn trash_dialog<'a>(title: &'a str, text: &'a str, can_go: bool, keep: bool) -> El<'a, Message> {
    let footer = if can_go {
        row![
            btn("Cancel", Btn::Default, Some(Message::CloseDialog)),
            btn(lt::go_label(keep), Btn::Danger, Some(Message::TrashGo)),
        ]
        .spacing(10)
    } else {
        row![btn("Close", Btn::Default, Some(Message::CloseDialog))]
    };
    modal(title, widgets::muted(text).size(14), Some(footer.into()), false, Message::CloseDialog)
}

/// A modal or ⋯ menu to lay over the whole window, when one is open.
pub fn overlay<'a>(state: &'a State, ctx: &Ctx<'a>) -> Option<El<'a, Message>> {
    if let Some(o) = library::shared_overlay(&state.shared, ctx) {
        return Some(o.map(Message::Shared));
    }
    match &state.dialog {
        Some(Dialog::Edit(e)) => return Some(edit_dialog(e)),
        Some(Dialog::Match(m)) => return Some(match_dialog(m, ctx)),
        Some(Dialog::Artwork(a)) => return Some(art_dialog(a, ctx)),
        Some(Dialog::ChangeType { picked, busy }) => return Some(change_type_dialog(state, picked, *busy)),
        Some(Dialog::Trash { title, text, can_go, keep }) => return Some(trash_dialog(title, text, *can_go, *keep)),
        Some(Dialog::Rest(r)) => return Some(super::rest_of_album::view(r, Message::Rest, Message::CloseDialog)),
        None => {}
    }
    state.menu.map(|at| album_menu(state, ctx, at))
}

/// A picture dragged onto the window while *Change artwork* is open.
pub fn subscription(state: &State) -> Subscription<Message> {
    match state.dialog {
        Some(Dialog::Artwork(_)) => drop_zone::events().map(Message::ArtDrop),
        // files dropped on Replace with my own file
        _ => super::library::shared_subscription(&state.shared).map(Message::Shared),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_links_are_understood() {
        assert_eq!(
            parse_release_link("https://musicbrainz.org/release/4F8A9C0D-1234-5678-9abc-def012345678"),
            Some("4f8a9c0d-1234-5678-9abc-def012345678".into())
        );
        assert_eq!(parse_release_link("https://www.deezer.com/en/album/302127"), Some("deezer:302127".into()));
        assert_eq!(
            parse_release_link("https://music.apple.com/us/album/thriller/269572838"),
            Some("apple:269572838".into())
        );
        assert_eq!(
            parse_release_link("https://music.apple.com/album/id269572838?i=1"),
            Some("apple:269572838".into())
        );
        assert_eq!(parse_release_link("https://www.discogs.com/release/249504-x"), Some("discogs:r249504".into()));
        assert_eq!(parse_release_link("https://www.discogs.com/Artist/master/1234"), Some("discogs:m1234".into()));
        assert_eq!(parse_release_link("just words"), None);
        assert_eq!(parse_release_link(""), None);
    }

    #[test]
    fn base64_is_standard() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn the_source_line_reads_like_the_web() {
        assert_eq!(source_line(&json!({})), "");
        let s = source_line(&json!({ "source": "soulseek", "source_detail": "user1", "requested_by": "ann" }));
        assert_eq!(s, "⬇ soulseek · user1 · requested by ann");
        // 0.36: the adder kept on the song, after Clear finished too; an upload names nobody here
        let s = source_line(&json!({ "source": "soulseek", "added_via": "soulseek", "added_by_name": "alex", "requested_by": null }));
        assert_eq!(s, "⬇ soulseek · requested by alex");
        let s = source_line(&json!({ "source": "upload", "added_via": "upload", "added_by_name": "sam", "requested_by": "sam" }));
        assert_eq!(s, "⬇ upload");
    }
    #[test]
    fn stereo_and_surround_versions_link_to_each_other() {
        // a Surround album names its stereo album
        let sur = json!({ "id": "s0123456789abcde", "class": "surround", "stereo": { "album_id": "ab12" } });
        assert_eq!(
            version_links(&sur),
            vec![("Stereo version →".to_string(), "The stereo version of this album".to_string(), Some("ab12".to_string()))]
        );
        assert!(version_links(&json!({ "class": "surround", "stereo": null })).is_empty());
        assert_eq!(atmos_button_tip(&sur, true), None);
        // a stereo album: its surround versions and the request on its way
        let stereo = json!({
            "id": "ab12",
            "surround": {
                "have": [
                    { "album_id": "s1", "tracks": 10, "quality": "Dolby Atmos (E-AC-3 5.1)" },
                    { "album_id": "s2", "tracks": 3, "quality": "AC-3 5.1" }
                ],
                "request": { "id": 7, "status": "queued" },
                "available": true,
                "can_fetch": true
            }
        });
        let links = version_links(&stereo);
        assert_eq!(links[0].0, "Dolby Atmos version →");
        assert_eq!(links[0].1, "10 song(s) in the Surround library");
        assert_eq!(links[0].2.as_deref(), Some("s1"));
        assert_eq!(links[1].0, "AC-3 5.1 version →");
        assert_eq!(links[2], ("Dolby Atmos version: queued".to_string(), "Request #7".to_string(), None));
        assert_eq!(atmos_button_tip(&stereo, true), None, "already here and requested");
        // nothing here or coming: admins can ask, again when Apple had none last time
        let none = json!({ "surround": { "have": [], "request": null, "available": null, "can_fetch": true } });
        assert!(atmos_button_tip(&none, true).is_some_and(|t| t.starts_with("Download")));
        assert_eq!(atmos_button_tip(&none, false), None);
        let missing = json!({ "surround": { "have": [], "request": null, "available": false, "can_fetch": true } });
        assert!(atmos_button_tip(&missing, true).is_some_and(|t| t.contains("ask again")));
        let no_gamdl = json!({ "surround": { "have": [], "request": null, "available": null, "can_fetch": false } });
        assert_eq!(atmos_button_tip(&no_gamdl, true), None);
        // an older server sends no `surround`: no links, no button
        let old = json!({ "id": "ab12", "album": "X", "tracks": [] });
        assert!(version_links(&old).is_empty());
        assert_eq!(atmos_button_tip(&old, true), None);
    }
}
