//! Library: Albums, Singles, Artists, Songs, Surround, Duplicates, Wrong audio. Behaviour reference:
//! web-rs/src/pages/library.rs `render` (tabs, sorting, filters, Load more, Shuffle, the
//! admin's Scan / Auto-tag / Organize / Verify / Find artwork with the progress card, the
//! duplicates and wrong-audio lists). The Surround tab (0.35, the server's `surround` feature) lists the
//! Dolby Atmos and 5.1/7.1 albums, kept apart from the stereo library (web `render_surround`).
//!
//! The search box searches the whole library whatever tab is selected (p6 item 9, client-shapes
//! §3.4): while it has a query the page shows one results view – Artists, Albums (singles too)
//! and Songs, each with its first matches and "Show all N", which opens that tab filtered by the
//! query; a tab clicked while a query is typed shows that tab filtered. Nothing anywhere:
//! "Search everywhere" opens Discover. The query is in the route, so Back keeps it.
//!
//! This file also holds what the album and artist pages share with it: the song ⋯ menu and
//! its dialogs ([`Shared`], [`Act`], `track_menu`, `shared_update`, `shared_overlay`), and the
//! small helpers of the web's library.rs (`format_text`, `verify_badge`, `tr`, `long_dur`).

use crate::api::ApiError;
use crate::fmt;
use crate::pages::library_types as lt;
use crate::pages::{urlencode, Ctx, El, Page};
use crate::theme::{self, Badge, Btn};
use crate::widgets::{
    self, badge, btn, btn_icon, btn_sm, confirm, empty, grid, h1, icon, modal, pop_menu, search_box, skeleton_rows,
    skeleton_tiles, small_muted, t, tabs, Icon, MenuItem, Tile, TileKind, TrackRow,
};
use iced::widget::{button, column, container, pick_list, progress_bar, radio, row, space, text_input, toggler, tooltip};
use iced::{Alignment, Length, Point, Subscription, Task};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::Duration;

/// Albums per page (the web's `PAGE`).
pub const PAGE: usize = 120;
/// Songs per page.
const SONGS_PAGE: usize = 300;

// ---------------------------------------------------------------------------------------------
// Helpers the library pages share (the web's library.rs free functions)
// ---------------------------------------------------------------------------------------------

/// `${format.toUpperCase()}${bitrate ? ' ' + bitrate + 'k' : ''}`.
pub fn format_text(t: &Value) -> String {
    let f = fmt::s(t, "format").to_uppercase();
    if fmt::truthy(t, "bitrate") {
        format!("{f} {}k", fmt::i(t, "bitrate"))
    } else {
        f
    }
}

/// The verify badge of a song: confirmed / verified / wrong audio, or nothing.
pub fn verify_badge<'a, M: 'a>(t: &Value) -> Option<El<'a, M>> {
    match fmt::s(t, "verify_status").as_str() {
        "manual" => Some(badge("✓ confirmed", Badge::Ok)),
        "verified" => Some(badge("✓ verified", Badge::Ok)),
        "mismatch" => Some(badge("wrong audio", Badge::Err)),
        _ => None,
    }
}

/// A name the way the account's Language setting wants it (web `tr_inline`): the original, its
/// translation or romanization instead, or "Original (Translation)".
/// A song's artists as links of this page (see [`crate::widgets::catalog::credit_parts`]).
pub(crate) fn artist_links(ctx: &Ctx, v: &Value) -> Vec<(String, Option<Message>)> {
    crate::widgets::catalog::credit_parts(v, &tr(ctx, v, "artist")).into_iter().map(|(text, page)| (text, page.map(Message::OpenPage))).collect()
}

pub fn tr(ctx: &Ctx, v: &Value, field: &str) -> String {
    let orig = fmt::s(v, field);
    let mode = fmt::s(&ctx.session.user["prefs"], "translate_mode");
    let pick = |sfx: &[&str]| {
        sfx.iter()
            .map(|x| fmt::s(v, &format!("{field}{x}")))
            .find(|s| !s.is_empty())
    };
    match mode.as_str() {
        "instead" => pick(&["_t", "_r"]).unwrap_or(orig),
        "romanized" => pick(&["_r"]).unwrap_or(orig),
        "under" => match pick(&["_t", "_r"]) {
            Some(tr) if tr != orig => format!("{orig} ({tr})"),
            _ => orig,
        },
        _ => orig,
    }
}

/// `1,234` (the web's `toLocaleString`).
pub fn num(n: f64) -> String {
    let whole = n.round().abs() as u64;
    let digits = whole.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0.0 {
        format!("-{out}")
    } else {
        out
    }
}

/// `fmt.longDur(s)`: "2 hr 5 min", "4 mo 22 days 8 hr 24 min" – parts that are 0 are left out.
pub fn long_dur(s: f64) -> String {
    let mut m = (s.max(0.0) / 60.0).round() as i64;
    let mut parts: Vec<String> = vec![];
    for (n, one, many) in [(525_600, "yr", "yr"), (43_200, "mo", "mo"), (1440, "day", "days"), (60, "hr", "hr")] {
        let v = m / n;
        if v > 0 {
            parts.push(format!("{v} {}", if v == 1 { one } else { many }));
            m -= v * n;
        }
    }
    if m > 0 || parts.is_empty() {
        parts.push(format!("{m} min"));
    }
    parts.join(" ")
}

/// `fmt.size(b)` as the web shows it: `12.3 MB`, `512 KB`.
pub fn size_text(b: f64) -> String {
    if b <= 0.0 {
        return "0 B".into();
    }
    let u = ["B", "KB", "MB", "GB", "TB"];
    let i = ((b.ln() / 1024f64.ln()).floor() as usize).min(4);
    let v = b / 1024f64.powi(i as i32);
    if i > 1 {
        format!("{v:.1} {}", u[i])
    } else {
        format!("{} {}", v.round() as i64, u[i])
    }
}

/// `tracks.map(t => ({ ...t, album_id: id }))`.
pub fn with_album_id(tracks: &[Value], id: &str) -> Vec<Value> {
    tracks
        .iter()
        .map(|t| {
            let mut t = t.clone();
            if let Some(o) = t.as_object_mut() {
                o.insert("album_id".into(), Value::String(id.into()));
            }
            t
        })
        .collect()
}

/// A random order (Fisher–Yates with a clock-seeded xorshift; no dependency needed).
pub fn shuffled(tracks: &[Value]) -> Vec<Value> {
    let mut v = tracks.to_vec();
    let mut x = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9E37_79B9_7F4A_7C15)
        | 1;
    for i in (1..v.len()).rev() {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let j = (x % (i as u64 + 1)) as usize;
        v.swap(i, j);
    }
    v
}

/// The request body for a song that is not in the library (web `reqBody`).
pub fn req_body(t: &Value) -> Value {
    let or_null = |k: &str| if fmt::truthy(t, k) { t[k].clone() } else { Value::Null };
    let mut b = serde_json::Map::new();
    b.insert("kind".into(), json!("track"));
    b.insert("mbid".into(), or_null("mbid"));
    if t.get("artist").is_some() {
        b.insert("artist".into(), t["artist"].clone());
    }
    if t.get("title").is_some() {
        b.insert("title".into(), t["title"].clone());
    }
    b.insert("album".into(), or_null("album"));
    if t.get("cover").is_some() {
        b.insert("cover_url".into(), t["cover"].clone());
    }
    if t.get("length").is_some() {
        b.insert("length".into(), t["length"].clone());
    }
    if fmt::truthy(t, "isrc") {
        b.insert("isrc".into(), t["isrc"].clone());
    }
    Value::Object(b)
}

/// The toast after `POST /requests` (web `submit_request`).
pub fn request_toast(r: &Value) -> &'static str {
    if fmt::s(r, "status") == "pending" {
        "Request sent – waiting for approval"
    } else {
        "Request queued"
    }
}

/// Surround song ids start here (0.35): the server numbers them apart from the stereo ones.
pub const SURROUND_IDS: i64 = 1_000_000_000;

/// A song or album of the Surround library: `class: "surround"`, or a song id from [`SURROUND_IDS`].
/// Likes, playlists, editing and deleting don't apply to them.
pub fn is_surround(v: &Value) -> bool {
    fmt::s(v, "class") == "surround" || fmt::i(v, "id") >= SURROUND_IDS
}

/// The words for a surround version: "Dolby Atmos" when its quality says so, else the quality
/// ("E-AC-3 5.1").
pub fn surround_label(quality: &str) -> String {
    if quality.contains("Atmos") {
        "Dolby Atmos".into()
    } else {
        quality.into()
    }
}

/// The corner of a Surround album's tile: "Dolby Atmos", else its quality.
pub fn surround_corner(a: &Value) -> String {
    if fmt::truthy(a, "atmos") {
        "Dolby Atmos".into()
    } else {
        fmt::s(a, "quality")
    }
}

/// The request status words and badge colours (web `status_info`).
pub fn status_badge<'a, M: 'a>(status: &str) -> El<'a, M> {
    let (kind, label) = match status {
        "pending" => (Badge::Warn, "Awaiting approval"),
        "queued" => (Badge::Info, "Queued"),
        "searching" => (Badge::Info, "Searching"),
        "monitoring" => (Badge::Accent, "Monitoring"),
        "downloading" => (Badge::Accent, "Downloading"),
        "importing" => (Badge::Accent, "Importing"),
        "waiting" => (Badge::Warn, "Waiting"),
        "completed" => (Badge::Ok, "Available"),
        "failed" => (Badge::Err, "Failed"),
        "denied" => (Badge::Err, "Denied"),
        "cancelled" => (Badge::Plain, "Cancelled"),
        other => return badge(other.to_string(), Badge::Plain),
    };
    badge(label, kind)
}

/// Synced lyrics without their `[mm:ss.xx]` tags, runs of blank lines collapsed, trimmed.
pub fn plain_lyrics(lyrics: &str) -> String {
    let mut untagged = String::with_capacity(lyrics.len());
    let mut rest = lyrics;
    while let Some(i) = rest.find('[') {
        match rest[i + 1..].find(']') {
            Some(j) => {
                untagged.push_str(&rest[..i]);
                rest = &rest[i + 1 + j + 1..];
            }
            None => break,
        }
    }
    untagged.push_str(rest);
    let mut out = String::with_capacity(untagged.len());
    let mut run = 0;
    for c in untagged.chars() {
        if c == '\n' {
            run += 1;
            if run > 2 {
                continue;
            }
        } else {
            run = 0;
        }
        out.push(c);
    }
    out.trim().to_string()
}

/// The reasons of Report a problem (web report.rs `REASONS`).
pub const REASONS: [(&str, &str); 7] = [
    ("wrong_song", "Wrong song"),
    ("wrong_version", "Wrong version (live, remix, edit, missing parts)"),
    ("bad_audio", "Bad audio quality"),
    ("loudness", "Too quiet or too loud"),
    ("fake_lossless", "Fake lossless"),
    ("glitches", "Skips or glitches"),
    ("other", "Other"),
];

/// The reason a quality-check flag suggests (web `reason_for`).
pub fn reason_for(codes: &[String]) -> &'static str {
    let has = |c: &str| codes.iter().any(|x| x == c);
    if has("fake_lossless") || has("upsampled") || has("low_bitrate") {
        return "fake_lossless";
    }
    if has("quiet") {
        return "loudness";
    }
    if has("wrong_length") {
        return "wrong_version";
    }
    "bad_audio"
}

/// Save `path` (an API path) to a file the user picks; "" when they cancelled.
pub async fn save_as(api: std::sync::Arc<crate::api::Api>, path: String, file_name: String) -> Result<String, String> {
    let Some(chosen) = rfd::AsyncFileDialog::new().set_file_name(&file_name).save_file().await else {
        return Ok(String::new());
    };
    // Streamed to disk as it arrives: an album zip can be several GB.
    let dest = chosen.path().to_path_buf();
    api.download_to(&path, &dest).await.map_err(|e| e.message)?;
    Ok(dest.display().to_string())
}

// ---------------------------------------------------------------------------------------------
// The song ⋯ menu and its dialogs, shared by the library, album and artist pages
// ---------------------------------------------------------------------------------------------

/// What the song menu and its dialogs do. Pages wrap it (`Message::Shared(Act)`).
#[derive(Clone, Debug)]
pub enum Act {
    CloseMenu,
    CloseDialog,
    /// The Confirm dialog's OK.
    ConfirmOk,
    PlayNext(Box<Value>),
    Enqueue(Box<Value>),
    AddToPlaylist(Vec<i64>),
    Playlists(Result<Value, ApiError>),
    NewPlaylistName(String),
    CreatePlaylist,
    PlaylistCreated(Result<Value, ApiError>),
    PickPlaylist(String),
    AddedToPlaylist(Result<Value, ApiError>),
    MoreLike(Box<Value>),
    Similar(Result<Value, ApiError>),
    PlaySimilar(usize),
    RequestSimilar(usize),
    Requested(usize, Result<Value, ApiError>),
    RequestAllSimilar,
    RequestedAll(Vec<(usize, Value)>, Option<String>),
    Lyrics(Box<Value>),
    LyricsAgain,
    LyricsGot(Result<Value, ApiError>),
    Share(Box<Value>),
    ShareGot(Result<Value, ApiError>),
    DownloadFile(Box<Value>),
    Downloaded(Result<String, String>),
    /// Save these songs on this computer (the offline store).
    SaveOffline(Vec<Value>),
    GoAlbum(String),
    Report {
        kind: String,
        id: Value,
        title: String,
        reason: String,
    },
    ReportReason(String),
    ReportNote(String),
    SendReport,
    Reported(Result<Value, ApiError>),
    FixSong(Box<Value>),
    FixField(usize, String),
    FixMove(bool),
    Identify,
    Identified(Result<Value, ApiError>),
    UseIdentified(usize),
    SaveFix,
    Fixed(Result<Value, ApiError>),
    VerifyManual {
        ids: Vec<Value>,
        correct: bool,
    },
    Verified(bool, Result<Value, ApiError>),
    DeleteSongAsk(Box<Value>),
    DeleteSong(i64),
    Deleted(Result<Value, ApiError>),
    /// The "Suspect quality" badge: the quality issues decide the report's reason.
    SuspectQuality(Box<Value>),
    QualityInfo(Box<Value>, Result<Value, ApiError>),
    /// Play a stereo song's surround version (its `surround_id`).
    PlaySurround(i64),
    SurroundGot(Result<Value, ApiError>),
    /// Ask for the Dolby Atmos version: `POST <path>` (an album's or a song's `/surround`, admins).
    GetAtmos(String),
    AtmosAsked(Result<Value, ApiError>),
    /// `GET /library/tracks/{id}/added` for the open menu's song (0.36): who added it, its first line.
    AddedGot(i64, Result<Value, ApiError>),
}

/// Edit song details.
#[derive(Debug)]
pub struct FixState {
    pub track: Value,
    /// title, artist, album, albumartist, tracknumber, date.
    pub fields: [String; 6],
    pub mv: bool,
    pub identifying: bool,
    pub msg: String,
    pub results: Vec<Value>,
    pub busy: bool,
}

const FIX_FIELDS: [(&str, &str); 6] = [
    ("title", "Title"),
    ("artist", "Artist"),
    ("album", "Album"),
    ("albumartist", "Album artist"),
    ("tracknumber", "Track #"),
    ("date", "Year"),
];

/// More like this.
#[derive(Debug)]
pub struct SimilarState {
    pub title: String,
    pub source: String,
    pub tracks: Vec<Value>,
    pub loaded: bool,
    pub error: String,
    pub requesting: bool,
}

#[derive(Debug)]
pub enum Dialog {
    AddToPlaylist {
        ids: Vec<i64>,
        playlists: Option<Vec<Value>>,
        name: String,
        busy: bool,
    },
    MoreLike(Box<SimilarState>),
    Lyrics {
        id: i64,
        title: String,
        result: Option<Result<Value, ApiError>>,
    },
    Report {
        kind: String,
        id: Value,
        title: String,
        reason: String,
        note: String,
        busy: bool,
    },
    Fix(Box<FixState>),
    Confirm {
        title: String,
        text: String,
        ok: String,
        danger: bool,
        on_ok: Box<Act>,
    },
}

/// The menu, the open dialog and what came out of them.
#[derive(Debug, Default)]
pub struct Shared {
    pub menu: Option<(Point, Vec<MenuItem<Act>>)>,
    /// The song the open menu is for (its who-added line arrives after it opened).
    pub menu_song: i64,
    pub dialog: Option<Dialog>,
    /// A song changed or was removed; the page reloads and clears it.
    pub changed: bool,
    /// A fixed song now lives on this album.
    pub moved_to: Option<String>,
}

impl Shared {
    pub fn take_changed(&mut self) -> bool {
        std::mem::take(&mut self.changed)
    }

    pub fn reset(&mut self) {
        self.menu = None;
        self.dialog = None;
    }
}

/// Build the ⋯ menu of one song at the pointer (web `track_menu`); the task asks who added the song, whose
/// words become the menu's first line when they come (0.36, [`Act::AddedGot`]).
pub fn track_menu(sh: &mut Shared, t: &Value, ctx: &Ctx) -> Task<Act> {
    if is_surround(t) {
        surround_track_menu(sh, t, ctx);
    } else {
        song_menu(sh, t, ctx);
    }
    ask_added(sh, t, ctx)
}

/// `GET /library/tracks/{id}/added` for the song whose menu just opened.
fn ask_added(sh: &mut Shared, t: &Value, ctx: &Ctx) -> Task<Act> {
    let id = fmt::i(t, "id");
    sh.menu_song = id;
    if id <= 0 {
        return Task::none();
    }
    let api = ctx.api.clone();
    Task::perform(async move { api.get(&format!("/library/tracks/{id}/added")).await }, move |r| Act::AddedGot(id, r))
}

/// The menu of a stereo song.
fn song_menu(sh: &mut Shared, t: &Value, ctx: &Ctx) {
    let admin = ctx.is_admin();
    let can_fix = admin || (ctx.session.perm("fix") && fmt::truthy(t, "request_id"));
    let b = || Box::new(t.clone());
    let id = fmt::i(t, "id");
    let mut items = vec![
        MenuItem::new("Play next", Act::PlayNext(b())).icon(Icon::Next),
        MenuItem::new("Add to queue", Act::Enqueue(b())).icon(Icon::List),
        MenuItem::new("Add to playlist", Act::AddToPlaylist(vec![id])).icon(Icon::Plus),
        MenuItem::new("More like this", Act::MoreLike(b())).icon(Icon::Spark),
        MenuItem::new("Lyrics", Act::Lyrics(b())).icon(Icon::Music),
        MenuItem::new("Share link", Act::Share(b())).icon(Icon::External),
        MenuItem::new("Download this song", Act::DownloadFile(b())).icon(Icon::Download),
        MenuItem::new("Save for offline", Act::SaveOffline(vec![t.clone()])).icon(Icon::Download),
    ];
    if fmt::truthy(t, "album_id") {
        items.push(MenuItem::new("Go to album", Act::GoAlbum(fmt::s(t, "album_id"))).icon(Icon::Disc));
    }
    // its surround version (0.35): play it, or (admins) fetch the Dolby Atmos version
    if fmt::truthy(t, "surround_id") {
        items.push(
            MenuItem::new("Play the surround version", Act::PlaySurround(fmt::i(t, "surround_id"))).icon(Icon::Play),
        );
    } else if admin && ctx.has_feature("surround") {
        items.push(
            MenuItem::new("Get Dolby Atmos version", Act::GetAtmos(format!("/library/tracks/{id}/surround")))
                .icon(Icon::Download),
        );
    }
    items.push(
        MenuItem::new(
            "Report a problem",
            Act::Report {
                kind: "track".into(),
                id: t["id"].clone(),
                title: fmt::s(t, "title"),
                reason: String::new(),
            },
        )
        .icon(Icon::Shield),
    );
    if admin {
        items.push(MenuItem::new("Edit song details…", Act::FixSong(b())).icon(Icon::Edit));
    }
    if can_fix {
        if fmt::s(t, "verify_status") == "manual" {
            items.push(
                MenuItem::new(
                    "Undo “confirmed”",
                    Act::VerifyManual {
                        ids: vec![t["id"].clone()],
                        correct: false,
                    },
                )
                .icon(Icon::X),
            );
        } else {
            items.push(
                MenuItem::new(
                    "This is the right song",
                    Act::VerifyManual {
                        ids: vec![t["id"].clone()],
                        correct: true,
                    },
                )
                .icon(Icon::Check),
            );
        }
        items.push(
            MenuItem::new(
                if admin { "Delete song from disk" } else { "Wrong song – remove it" },
                Act::DeleteSongAsk(b()),
            )
            .icon(Icon::Trash)
            .danger(),
        );
    }
    sh.menu = Some((ctx.cursor, items));
}

/// "⋯" for a Surround song (web `surround_track_menu`): playing it, its lyrics and file, and its
/// stereo version; likes, playlists, editing and deleting don't apply.
fn surround_track_menu(sh: &mut Shared, t: &Value, ctx: &Ctx) {
    let b = || Box::new(t.clone());
    let mut items = vec![
        MenuItem::new("Play next", Act::PlayNext(b())).icon(Icon::Next),
        MenuItem::new("Add to queue", Act::Enqueue(b())).icon(Icon::List),
        MenuItem::new("Lyrics", Act::Lyrics(b())).icon(Icon::Music),
        MenuItem::new("Download this song", Act::DownloadFile(b())).icon(Icon::Download),
    ];
    if fmt::truthy(t, "album_id") {
        items.push(MenuItem::new("Go to album", Act::GoAlbum(fmt::s(t, "album_id"))).icon(Icon::Disc));
    }
    if fmt::truthy(t, "stereo_album_id") {
        items.push(
            MenuItem::new("Go to the stereo version", Act::GoAlbum(fmt::s(t, "stereo_album_id"))).icon(Icon::Disc),
        );
    }
    sh.menu = Some((ctx.cursor, items));
}

/// Open a Report a problem dialog (songs and albums).
pub fn open_report(sh: &mut Shared, kind: &str, id: Value, title: &str, reason: &str) {
    sh.menu = None;
    sh.dialog = Some(Dialog::Report {
        kind: kind.into(),
        id,
        title: title.into(),
        reason: reason.into(),
        note: String::new(),
        busy: false,
    });
}

/// Open the Add to playlist dialog for these songs.
pub fn open_add_to_playlist(sh: &mut Shared, ids: Vec<i64>, ctx: &Ctx) -> Task<Act> {
    sh.menu = None;
    sh.dialog = Some(Dialog::AddToPlaylist {
        ids,
        playlists: None,
        name: String::new(),
        busy: false,
    });
    let api = ctx.api.clone();
    Task::perform(async move { api.get("/playlists").await }, Act::Playlists)
}

/// Open a yes/no dialog whose OK runs `on_ok`.
pub fn open_confirm(sh: &mut Shared, title: &str, text: String, ok: &str, danger: bool, on_ok: Act) {
    sh.menu = None;
    sh.dialog = Some(Dialog::Confirm {
        title: title.into(),
        text,
        ok: ok.into(),
        danger,
        on_ok: Box::new(on_ok),
    });
}

pub fn shared_update(sh: &mut Shared, act: Act, ctx: &mut Ctx) -> Task<Act> {
    match act {
        Act::CloseMenu => sh.menu = None,
        Act::CloseDialog => sh.dialog = None,
        Act::ConfirmOk => {
            if let Some(Dialog::Confirm { on_ok, .. }) = sh.dialog.take() {
                return shared_update(sh, *on_ok, ctx);
            }
        }
        Act::PlayNext(t) => {
            sh.menu = None;
            ctx.play_next(vec![*t]);
        }
        Act::Enqueue(t) => {
            sh.menu = None;
            ctx.enqueue(vec![*t]);
        }
        Act::SaveOffline(rows) => {
            sh.menu = None;
            crate::desktop::offline::save(ctx, rows);
        }
        Act::AddToPlaylist(ids) => return open_add_to_playlist(sh, ids, ctx),
        Act::Playlists(r) => {
            if let Some(Dialog::AddToPlaylist { playlists, .. }) = &mut sh.dialog {
                match r {
                    Ok(v) => *playlists = Some(fmt::items(&v)),
                    Err(e) => {
                        sh.dialog = None;
                        ctx.toast_err(e.message);
                    }
                }
            }
        }
        Act::NewPlaylistName(n) => {
            if let Some(Dialog::AddToPlaylist { name, .. }) = &mut sh.dialog {
                *name = n;
            }
        }
        Act::CreatePlaylist => {
            if let Some(Dialog::AddToPlaylist { name, busy, .. }) = &mut sh.dialog {
                if *busy {
                    return Task::none();
                }
                *busy = true;
                let name = name.trim().to_string();
                let name = if name.is_empty() { "New playlist".to_string() } else { name };
                let api = ctx.api.clone();
                return Task::perform(
                    async move { api.post("/playlists", json!({ "name": name })).await },
                    Act::PlaylistCreated,
                );
            }
        }
        Act::PlaylistCreated(Ok(r)) => {
            if let Some(Dialog::AddToPlaylist { busy, .. }) = &mut sh.dialog {
                *busy = false;
            }
            return Task::done(Act::PickPlaylist(fmt::s(&r, "id")));
        }
        Act::PlaylistCreated(Err(e)) => {
            if let Some(Dialog::AddToPlaylist { busy, .. }) = &mut sh.dialog {
                *busy = false;
            }
            ctx.toast_err(e.message);
        }
        Act::PickPlaylist(pid) => {
            if let Some(Dialog::AddToPlaylist { ids, busy, .. }) = &mut sh.dialog {
                if *busy {
                    return Task::none();
                }
                *busy = true;
                let ids = ids.clone();
                let api = ctx.api.clone();
                return Task::perform(
                    async move {
                        api.post(&format!("/playlists/{pid}/tracks"), json!({ "track_ids": ids }))
                            .await
                    },
                    Act::AddedToPlaylist,
                );
            }
        }
        Act::AddedToPlaylist(Ok(r)) => {
            sh.dialog = None;
            let mut msg = format!("Added {}", fmt::s(&r, "added"));
            if fmt::truthy(&r, "duplicates_skipped") {
                msg.push_str(&format!(" ({} already there)", fmt::s(&r, "duplicates_skipped")));
            }
            ctx.toast_ok(msg);
        }
        Act::AddedToPlaylist(Err(e)) => {
            if let Some(Dialog::AddToPlaylist { busy, .. }) = &mut sh.dialog {
                *busy = false;
            }
            ctx.toast_err(e.message);
        }
        Act::MoreLike(t) => {
            sh.menu = None;
            sh.dialog = Some(Dialog::MoreLike(Box::new(SimilarState {
                title: fmt::s(&t, "title"),
                source: String::new(),
                tracks: Vec::new(),
                loaded: false,
                error: String::new(),
                requesting: false,
            })));
            let mbid = fmt::s(&t, "mb_trackid");
            let url = format!(
                "/discover/similar-tracks?artist={}&title={}{}",
                urlencode(&fmt::s(&t, "artist")),
                urlencode(&fmt::s(&t, "title")),
                if mbid.is_empty() { String::new() } else { format!("&mbid={mbid}") }
            );
            let api = ctx.api.clone();
            return Task::perform(async move { api.get(&url).await }, Act::Similar);
        }
        Act::Similar(r) => {
            if let Some(Dialog::MoreLike(s)) = &mut sh.dialog {
                s.loaded = true;
                match r {
                    Ok(d) => {
                        s.source = fmt::s(&d, "source");
                        s.tracks = fmt::items(&d["tracks"]);
                    }
                    Err(e) => s.error = e.message,
                }
            }
        }
        Act::PlaySimilar(i) => {
            if let Some(Dialog::MoreLike(s)) = &sh.dialog {
                let owned: Vec<Value> = s
                    .tracks
                    .iter()
                    .filter(|t| fmt::truthy(t, "library"))
                    .map(|t| t["library"].clone())
                    .collect();
                let start = s.tracks[..i.min(s.tracks.len())]
                    .iter()
                    .filter(|t| fmt::truthy(t, "library"))
                    .count();
                if !owned.is_empty() {
                    ctx.play(owned, start);
                }
            }
        }
        Act::RequestSimilar(i) => {
            if let Some(Dialog::MoreLike(s)) = &sh.dialog {
                if let Some(t) = s.tracks.get(i) {
                    let body = req_body(t);
                    let api = ctx.api.clone();
                    return Task::perform(async move { api.post("/requests", body).await }, move |r| {
                        Act::Requested(i, r)
                    });
                }
            }
        }
        Act::Requested(i, Ok(r)) => {
            ctx.toast_ok(request_toast(&r));
            ctx.refresh_shell();
            if let Some(Dialog::MoreLike(s)) = &mut sh.dialog {
                if let Some(t) = s.tracks.get_mut(i) {
                    t["request_status"] = r["status"].clone();
                }
            }
        }
        Act::Requested(_, Err(e)) => ctx.toast_err(e.message),
        Act::RequestAllSimilar => {
            if let Some(Dialog::MoreLike(s)) = &mut sh.dialog {
                if s.requesting {
                    return Task::none();
                }
                s.requesting = true;
                let missing: Vec<(usize, Value)> = s
                    .tracks
                    .iter()
                    .enumerate()
                    .filter(|(_, t)| !fmt::truthy(t, "library") && !fmt::truthy(t, "request_status"))
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
                    |(done, err)| Act::RequestedAll(done, err),
                );
            }
        }
        Act::RequestedAll(done, err) => {
            if let Some(Dialog::MoreLike(s)) = &mut sh.dialog {
                s.requesting = false;
                for (i, r) in &done {
                    if let Some(t) = s.tracks.get_mut(*i) {
                        t["request_status"] = r["status"].clone();
                    }
                }
            }
            if let Some(e) = err {
                ctx.toast_err(e);
            }
            ctx.toast_ok(format!("{} song(s) requested", done.len()));
            ctx.refresh_shell();
        }
        Act::Lyrics(t) => {
            sh.menu = None;
            let id = fmt::i(&t, "id");
            sh.dialog = Some(Dialog::Lyrics {
                id,
                title: fmt::s(&t, "title"),
                result: None,
            });
            let api = ctx.api.clone();
            return Task::perform(
                async move { api.get(&format!("/library/tracks/{id}/lyrics")).await },
                Act::LyricsGot,
            );
        }
        Act::LyricsAgain => {
            if let Some(Dialog::Lyrics { id, result, .. }) = &mut sh.dialog {
                *result = None;
                let id = *id;
                let api = ctx.api.clone();
                return Task::perform(
                    async move { api.get(&format!("/library/tracks/{id}/lyrics?force=true")).await },
                    Act::LyricsGot,
                );
            }
        }
        Act::LyricsGot(r) => {
            if let Some(Dialog::Lyrics { result, .. }) = &mut sh.dialog {
                *result = Some(r);
            }
        }
        Act::Share(t) => {
            sh.menu = None;
            let id = fmt::i(&t, "id");
            let api = ctx.api.clone();
            return Task::perform(
                async move { api.post_empty(&format!("/library/tracks/{id}/share")).await },
                Act::ShareGot,
            );
        }
        Act::ShareGot(Ok(r)) => {
            ctx.copy(format!("{}{}", ctx.base(), fmt::s(&r, "path")));
            ctx.toast_ok("Link copied – anyone with it can play this song");
        }
        Act::ShareGot(Err(e)) => ctx.toast_err(e.message),
        Act::DownloadFile(t) => {
            sh.menu = None;
            let id = fmt::i(&t, "id");
            let ext = fmt::s(&t, "format").to_lowercase();
            let name = format!(
                "{} - {}{}",
                fmt::s(&t, "artist"),
                fmt::s(&t, "title"),
                if ext.is_empty() { String::new() } else { format!(".{ext}") }
            );
            let api = ctx.api.clone();
            return Task::perform(
                save_as(api, format!("/library/tracks/{id}/file"), safe_file_name(&name)),
                Act::Downloaded,
            );
        }
        Act::Downloaded(Ok(path)) => {
            if !path.is_empty() {
                ctx.toast_ok(format!("Saved to {path}"));
            }
        }
        Act::Downloaded(Err(e)) => ctx.toast_err(e),
        Act::GoAlbum(id) => {
            sh.menu = None;
            ctx.navigate(Page::LibraryAlbum(id));
        }
        Act::Report {
            kind,
            id,
            title,
            reason,
        } => open_report(sh, &kind, id, &title, &reason),
        Act::ReportReason(r) => {
            if let Some(Dialog::Report { reason, .. }) = &mut sh.dialog {
                *reason = r;
            }
        }
        Act::ReportNote(n) => {
            if let Some(Dialog::Report { note, .. }) = &mut sh.dialog {
                *note = n;
            }
        }
        Act::SendReport => {
            if let Some(Dialog::Report {
                kind,
                id,
                reason,
                note,
                busy,
                ..
            }) = &mut sh.dialog
            {
                if *busy {
                    return Task::none();
                }
                if reason.is_empty() {
                    ctx.toast_err("Pick what is wrong");
                    return Task::none();
                }
                let note = note.trim().to_string();
                if reason == "other" && note.is_empty() {
                    ctx.toast_err("Say what is wrong");
                    return Task::none();
                }
                *busy = true;
                let mut body = json!({ "kind": kind, "reason": reason, "note": note });
                if kind == "album" {
                    body["album_id"] = id.clone();
                } else {
                    body["track_id"] = id.clone();
                }
                let api = ctx.api.clone();
                return Task::perform(async move { api.post("/report", body).await }, Act::Reported);
            }
        }
        Act::Reported(Ok(res)) => {
            sh.dialog = None;
            sh.changed = true;
            ctx.refresh_shell();
            ctx.toast_ok(if fmt::s(&res, "status") == "replacing" {
                "Reported – looking for a better copy".to_string()
            } else if fmt::truthy(&res, "message") {
                fmt::s(&res, "message")
            } else {
                "Reported".to_string()
            });
        }
        Act::Reported(Err(e)) => {
            if let Some(Dialog::Report { busy, .. }) = &mut sh.dialog {
                *busy = false;
            }
            ctx.toast_err(e.message);
        }
        Act::FixSong(t) => {
            sh.menu = None;
            let or = |k: &str, fallback: String| if fmt::truthy(&t, k) { fmt::s(&t, k) } else { fallback };
            let fields = [
                fmt::s(&t, "title"),
                fmt::s(&t, "artist"),
                or("album", String::new()),
                or("albumartist", fmt::s(&t, "artist")),
                or("track", String::new()),
                or("year", String::new()),
            ];
            sh.dialog = Some(Dialog::Fix(Box::new(FixState {
                track: *t,
                fields,
                mv: true,
                identifying: false,
                msg: String::new(),
                results: Vec::new(),
                busy: false,
            })));
        }
        Act::FixField(i, v) => {
            if let Some(Dialog::Fix(f)) = &mut sh.dialog {
                if let Some(slot) = f.fields.get_mut(i) {
                    *slot = v;
                }
            }
        }
        Act::FixMove(on) => {
            if let Some(Dialog::Fix(f)) = &mut sh.dialog {
                f.mv = on;
            }
        }
        Act::Identify => {
            if let Some(Dialog::Fix(f)) = &mut sh.dialog {
                if f.identifying {
                    return Task::none();
                }
                f.identifying = true;
                f.msg = "Listening…".into();
                let id = fmt::i(&f.track, "id");
                let api = ctx.api.clone();
                return Task::perform(
                    async move { api.get(&format!("/library/tracks/{id}/identify")).await },
                    Act::Identified,
                );
            }
        }
        Act::Identified(r) => {
            if let Some(Dialog::Fix(f)) = &mut sh.dialog {
                f.identifying = false;
                match r {
                    Ok(v) => {
                        f.results = fmt::items(&v);
                        f.msg = if f.results.is_empty() {
                            "No match for this audio.".into()
                        } else {
                            "Pick what it really is:".into()
                        };
                    }
                    Err(e) => f.msg = e.message,
                }
            }
        }
        Act::UseIdentified(i) => {
            if let Some(Dialog::Fix(f)) = &mut sh.dialog {
                if let Some(r) = f.results.get(i).cloned() {
                    f.fields[0] = fmt::s(&r, "title");
                    f.fields[1] = fmt::s(&r, "artist");
                    if fmt::truthy(&r, "album") {
                        f.fields[2] = fmt::s(&r, "album");
                    }
                    ctx.toast_ok("Filled in – check and save");
                }
            }
        }
        Act::SaveFix => {
            if let Some(Dialog::Fix(f)) = &mut sh.dialog {
                if f.busy {
                    return Task::none();
                }
                f.busy = true;
                let mut values = serde_json::Map::new();
                for ((k, _), v) in FIX_FIELDS.iter().zip(f.fields.iter()) {
                    values.insert(k.to_string(), Value::String(v.clone()));
                }
                let body = json!({ "values": Value::Object(values), "move": f.mv });
                let id = fmt::i(&f.track, "id");
                let api = ctx.api.clone();
                return Task::perform(
                    async move { api.post(&format!("/library/tracks/{id}/fix"), body).await },
                    Act::Fixed,
                );
            }
        }
        Act::Fixed(Ok(r)) => {
            sh.dialog = None;
            sh.changed = true;
            if fmt::truthy(&r, "album_id") {
                sh.moved_to = Some(fmt::s(&r, "album_id"));
            }
            ctx.toast_ok("Song fixed");
        }
        Act::Fixed(Err(e)) => {
            if let Some(Dialog::Fix(f)) = &mut sh.dialog {
                f.busy = false;
            }
            ctx.toast_err(e.message);
        }
        Act::VerifyManual { ids, correct } => {
            sh.menu = None;
            let body = if correct {
                json!({ "track_ids": ids })
            } else {
                json!({ "track_ids": ids, "correct": false })
            };
            let api = ctx.api.clone();
            return Task::perform(async move { api.post("/library/verify-manual", body).await }, move |r| {
                Act::Verified(correct, r)
            });
        }
        Act::Verified(correct, Ok(_)) => {
            sh.changed = true;
            ctx.toast_ok(if correct {
                "Marked as confirmed – the audio check won’t flag it again"
            } else {
                "Will be checked again"
            });
        }
        Act::Verified(_, Err(e)) => ctx.toast_err(e.message),
        Act::DeleteSongAsk(t) => {
            let id = fmt::i(&t, "id");
            open_confirm(
                sh,
                "Delete song",
                format!("Permanently delete \"{}\" from disk?", fmt::s(&t, "title")),
                "Delete",
                true,
                Act::DeleteSong(id),
            );
        }
        Act::DeleteSong(id) => {
            sh.dialog = None;
            let api = ctx.api.clone();
            return Task::perform(
                async move { api.del(&format!("/library/tracks/{id}")).await },
                Act::Deleted,
            );
        }
        Act::Deleted(Ok(_)) => {
            sh.changed = true;
            ctx.toast_ok("Song deleted");
        }
        Act::Deleted(Err(e)) => ctx.toast_err(e.message),
        Act::SuspectQuality(t) => {
            let id = fmt::i(&t, "id");
            let api = ctx.api.clone();
            return Task::perform(
                async move { api.get(&format!("/library/tracks/{id}/info")).await },
                move |r| Act::QualityInfo(t.clone(), r),
            );
        }
        Act::QualityInfo(t, r) => {
            let codes: Vec<String> = r
                .ok()
                .map(|i| fmt::items(&i["quality"]["issues"]).iter().map(|x| fmt::s(x, "code")).collect())
                .unwrap_or_default();
            open_report(sh, "track", t["id"].clone(), &fmt::s(&t, "title"), reason_for(&codes));
        }
        Act::PlaySurround(id) => {
            sh.menu = None;
            let api = ctx.api.clone();
            return Task::perform(
                async move { api.get(&format!("/library/tracks/{id}")).await },
                Act::SurroundGot,
            );
        }
        Act::SurroundGot(Ok(t)) => ctx.play(vec![t], 0),
        Act::SurroundGot(Err(e)) => ctx.toast_err(e.message),
        Act::GetAtmos(path) => {
            sh.menu = None;
            ctx.toast("Looking for a Dolby Atmos version on Apple Music…");
            let api = ctx.api.clone();
            return Task::perform(async move { api.post_empty(&path).await }, Act::AtmosAsked);
        }
        Act::AtmosAsked(Ok(r)) => {
            sh.changed = true;
            ctx.refresh_shell();
            ctx.toast_ok(format!("Dolby Atmos version requested (#{})", fmt::s(&r, "request_id")));
        }
        // 404: Apple Music has no Dolby Atmos version; 409: already asked, or gamdl has no wrapper
        Act::AtmosAsked(Err(e)) => {
            if e.status == 404 {
                ctx.toast_warn(e.message);
            } else {
                ctx.toast_err(e.message);
            }
        }
        // who added the song: the first line of its menu while that menu is still open (nothing to say, or an
        // older server: no line)
        Act::AddedGot(id, Ok(v)) => {
            let text = crate::pages::library_types::added_of(&v, fmt::date);
            if let Some((_, items)) = sh.menu.as_mut().filter(|_| sh.menu_song == id && !text.is_empty()) {
                if !items.first().is_some_and(|i| i.info) {
                    items.insert(0, MenuItem::info(text));
                }
            }
        }
        Act::AddedGot(_, Err(_)) => {}
    }
    Task::none()
}

/// A file name without the characters file systems refuse.
pub fn safe_file_name(s: &str) -> String {
    s.chars()
        .map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') { '_' } else { c })
        .collect()
}

/// The shared menu or dialog, when one is open.
pub fn shared_overlay<'a>(sh: &'a Shared, ctx: &Ctx<'a>) -> Option<El<'a, Act>> {
    if let Some(d) = &sh.dialog {
        return Some(dialog_view(d, ctx));
    }
    if let Some((at, items)) = &sh.menu {
        return Some(pop_menu(*at, items.clone(), Act::CloseMenu, ctx.size));
    }
    None
}

fn dialog_view<'a>(d: &'a Dialog, ctx: &Ctx<'a>) -> El<'a, Act> {
    let p = ctx.look;
    match d {
        Dialog::Confirm {
            title,
            text,
            ok,
            danger,
            ..
        } => confirm(title, text, ok, *danger, Act::ConfirmOk, Act::CloseDialog),
        Dialog::AddToPlaylist {
            playlists, name, busy, ..
        } => {
            let list: El<'a, Act> = match playlists {
                None => skeleton_rows(3),
                Some(pls) => {
                    let mut col = column![];
                    // not someone else's playlist added from a link: only its owner adds songs to it
                    for pl in pls.iter().filter(|pl| crate::pages::playlists::can_add_to(pl)) {
                        let id = fmt::s(pl, "id");
                        let left: El<'a, Act> = if fmt::s(pl, "kind") == "liked" {
                            icon(Icon::Heart, 16.0, p.accent)
                        } else {
                            icon(Icon::List, 16.0, p.muted)
                        };
                        col = col.push(widgets::list_row(
                            Some(left),
                            fmt::s(pl, "name"),
                            String::new(),
                            vec![widgets::faint(fmt::s(pl, "total")).size(12).into()],
                            (!busy).then_some(Act::PickPlaylist(id)),
                        ));
                    }
                    if pls.is_empty() {
                        col = col.push(container(widgets::muted("No playlists yet").size(13)).padding(14));
                    }
                    widgets::card_flat(col).width(Length::Fill).into()
                }
            };
            let body = column![
                list,
                row![
                    text_input("…or a new playlist name", name)
                        .on_input(Act::NewPlaylistName)
                        .on_submit(Act::CreatePlaylist)
                        .padding([9, 12])
                        .size(14)
                        .style(theme::input),
                    btn_icon(Icon::Plus, "Create", Btn::Default, (!busy).then_some(Act::CreatePlaylist)),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            ]
            .spacing(12);
            modal("Add to playlist", body, None, false, Act::CloseDialog)
        }
        Dialog::MoreLike(s) => {
            let body: El<'a, Act> = if !s.loaded {
                widgets::muted("Finding similar songs…").size(14).into()
            } else if !s.error.is_empty() {
                widgets::error_box(&s.error)
            } else if s.tracks.is_empty() {
                empty(
                    Icon::Spark,
                    "Nothing similar found",
                    "Add a Last.fm API key in Settings for the best results.",
                )
            } else {
                let missing = s
                    .tracks
                    .iter()
                    .filter(|t| !fmt::truthy(t, "library") && !fmt::truthy(t, "request_status"))
                    .count();
                let mut head = row![small_muted(format!("from {}", s.source)), space().width(Length::Fill)]
                    .spacing(10)
                    .align_y(Alignment::Center);
                if missing > 0 {
                    head = head.push(btn_icon(
                        Icon::Download,
                        format!("Request {missing} missing"),
                        Btn::Default,
                        (!s.requesting).then_some(Act::RequestAllSimilar),
                    ));
                }
                let mut list = column![];
                for (i, v) in s.tracks.iter().enumerate() {
                    list = list.push(similar_row(v, i, ctx));
                }
                column![head, widgets::card_flat(list).width(Length::Fill)]
                    .spacing(12)
                    .into()
            };
            modal(format!("More like {}", s.title), body, None, true, Act::CloseDialog)
        }
        Dialog::Lyrics { title, result, .. } => {
            let body: El<'a, Act> = match result {
                None => widgets::muted("Looking for lyrics…").size(14).into(),
                Some(Err(e)) => widgets::error_box(&e.message),
                Some(Ok(r)) => {
                    let synced = fmt::truthy(r, "synced");
                    let raw = fmt::s(r, "lyrics");
                    let text = if synced { plain_lyrics(&raw) } else { raw };
                    if fmt::truthy(r, "instrumental") {
                        widgets::muted("Instrumental – no lyrics.").size(14).into()
                    } else if !text.is_empty() {
                        column![
                            widgets::faint(format!(
                                "{} lyrics · {}",
                                if synced { "Time-synced" } else { "Plain" },
                                if fmt::s(r, "source") == "file" {
                                    "saved next to the song"
                                } else {
                                    "from LRCLIB, saved next to the song"
                                }
                            ))
                            .size(12),
                            t(text).size(14).line_height(1.6),
                        ]
                        .spacing(10)
                        .into()
                    } else {
                        widgets::muted("No lyrics found on LRCLIB.").size(14).into()
                    }
                }
            };
            let footer = row![
                btn("Search again", Btn::Default, result.is_some().then_some(Act::LyricsAgain)),
                btn("Close", Btn::Primary, Some(Act::CloseDialog)),
            ]
            .spacing(10);
            modal(format!("Lyrics · {title}"), body, Some(footer.into()), false, Act::CloseDialog)
        }
        Dialog::Report {
            kind,
            title,
            reason,
            note,
            busy,
            ..
        } => {
            let what = if kind == "album" { "album" } else { "song" };
            let mut reasons = column![].spacing(8);
            for (k, l) in REASONS {
                reasons = reasons.push(
                    radio(l, k, Some(reason.as_str()), |v| Act::ReportReason(v.to_string()))
                        .size(16)
                        .text_size(13.5)
                        .spacing(8),
                );
            }
            let body = column![
                widgets::muted(format!(
                    "What is wrong with “{title}”? Encore stops using the source this {what} came from, moves the file{} to the trash for 7 days and gets the {what} again from somewhere else. Plays, likes and playlists stay with the {what}.",
                    if kind == "album" { "s" } else { "" }
                ))
                .size(13),
                reasons,
                widgets::field(
                    "Note",
                    text_input("Optional – e.g. the intro is missing", note)
                        .on_input(Act::ReportNote)
                        .padding([9, 12])
                        .size(14)
                        .style(theme::input)
                ),
            ]
            .spacing(14);
            let footer = row![
                btn("Cancel", Btn::Default, Some(Act::CloseDialog)),
                btn("Report", Btn::Primary, (!busy).then_some(Act::SendReport)),
            ]
            .spacing(10);
            modal("Report a problem", body, Some(footer.into()), false, Act::CloseDialog)
        }
        Dialog::Fix(f) => {
            let input = |i: usize| -> El<'a, Act> {
                widgets::field(
                    FIX_FIELDS[i].1,
                    text_input("", &f.fields[i])
                        .on_input(move |v| Act::FixField(i, v))
                        .padding([8, 11])
                        .size(14)
                        .style(theme::input),
                )
            };
            let mut results = column![];
            for (i, r) in f.results.iter().enumerate() {
                let mut sub = fmt::s(r, "artist");
                if fmt::truthy(r, "album") {
                    sub.push_str(&format!(" · {}", fmt::s(r, "album")));
                }
                results = results.push(widgets::list_row(
                    None,
                    fmt::s(r, "title"),
                    sub,
                    vec![badge(format!("{}%", fmt::s(r, "score")), Badge::Ok)],
                    Some(Act::UseIdentified(i)),
                ));
            }
            let mut body = column![
                widgets::muted("Change what this file is. Give a different version its own title (e.g. “… (Remix)”) or album so it stops counting as a duplicate. The file is moved to match and is never merged with another song.").size(13),
                row![input(0), input(1)].spacing(12),
                row![input(2), input(3)].spacing(12),
                row![input(4), input(5)].spacing(12),
                toggler(f.mv)
                    .label("Move the file to match (Artist/Album/NN Title)")
                    .text_size(13.5)
                    .spacing(10)
                    .on_toggle(Act::FixMove)
                    .style(theme::switch),
                row![
                    btn_icon(Icon::Shield, "Identify by its audio", Btn::Default, (!f.identifying).then_some(Act::Identify)),
                    small_muted(f.msg.clone()),
                ]
                .spacing(10)
                .align_y(Alignment::Center),
            ]
            .spacing(14);
            if !f.results.is_empty() {
                body = body.push(widgets::card_flat(results).width(Length::Fill));
            }
            let footer = row![
                btn("Cancel", Btn::Default, Some(Act::CloseDialog)),
                btn("Save", Btn::Primary, (!f.busy).then_some(Act::SaveFix)),
            ]
            .spacing(10);
            modal("Edit song details", body, Some(footer.into()), true, Act::CloseDialog)
        }
    }
}

/// One row of More like this (web foryou `track_row`): play it when it is in the library,
/// otherwise request it.
fn similar_row<'a>(v: &'a Value, i: usize, ctx: &Ctx<'a>) -> El<'a, Act> {
    let p = ctx.look;
    let owned = fmt::truthy(v, "library");
    let thumb: El<'a, Act> = match ctx.picture(&fmt::s(v, "cover")) {
        Some(h) => iced::widget::image(h)
            .width(40)
            .height(40)
            .content_fit(iced::ContentFit::Cover)
            .border_radius(6.0)
            .into(),
        None => container(icon(Icon::Music, 16.0, p.faint))
            .width(40)
            .height(40)
            .center(40)
            .style(theme::art)
            .into(),
    };
    let mut sub = tr(ctx, v, "artist");
    if fmt::truthy(v, "album") {
        sub.push_str(&format!(" · {}", tr(ctx, v, "album")));
    }
    let mut right: Vec<El<'a, Act>> = Vec::new();
    if fmt::truthy(v, "length") {
        right.push(small_muted(fmt::mmss(fmt::f(v, "length"))).into());
    }
    right.push(if owned {
        badge("In library", Badge::Ok)
    } else if fmt::truthy(v, "request_status") {
        status_badge(&fmt::s(v, "request_status"))
    } else {
        btn_icon(Icon::Plus, "Request", Btn::Default, Some(Act::RequestSimilar(i))).into()
    });
    widgets::list_row(
        Some(thumb),
        tr(ctx, v, "title"),
        sub,
        right,
        owned.then_some(Act::PlaySimilar(i)),
    )
}

// ---------------------------------------------------------------------------------------------
// The Library page
// ---------------------------------------------------------------------------------------------

/// A sort or filter option in a pick list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Opt {
    pub key: &'static str,
    pub label: &'static str,
    prefix: &'static str,
}

impl std::fmt::Display for Opt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}{}", self.prefix, self.label)
    }
}

/// `SORT_OPTS[tab]` (singles share the album options).
pub fn sort_opts(tab: &str) -> Option<&'static [(&'static str, &'static str)]> {
    match tab {
        "albums" | "singles" => Some(&[
            ("added", "Date added"),
            ("released", "Release date"),
            ("album", "Title A–Z"),
            ("artist", "Artist A–Z"),
            ("duration", "Length"),
            ("size", "Size"),
        ]),
        "artists" => Some(&[
            ("name", "Name A–Z"),
            ("added", "Date added"),
            ("albums", "Most albums"),
            ("tracks", "Most songs"),
        ]),
        "tracks" => Some(&[
            ("added", "Date added"),
            ("released", "Release date"),
            ("title", "Title A–Z"),
            ("artist", "Artist A–Z"),
            ("album", "Album A–Z"),
            ("duration", "Length"),
        ]),
        // the server sorts these one way only (no `dir`)
        "surround" => Some(&[
            ("added", "Date added"),
            ("name", "Title A–Z"),
            ("artist", "Artist A–Z"),
            ("year", "Year"),
        ]),
        _ => None,
    }
}

/// `DESC_FIRST.has(sort)`.
pub fn desc_first(sort: &str) -> bool {
    matches!(sort, "added" | "released" | "duration" | "size" | "albums" | "tracks")
}

const FILTERS: [(&str, &str); 4] = [
    ("", "All albums"),
    ("untagged", "Needs tagging"),
    ("tagged", "MusicBrainz tagged"),
    ("lossless", "Lossless only"),
];

/// The admin's bulk actions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdminAct {
    Scan,
    Autotag,
    Organize,
    Verify,
    Artwork,
}

impl AdminAct {
    fn path(self) -> &'static str {
        match self {
            AdminAct::Scan => "/library/scan",
            AdminAct::Autotag => "/library/autotag",
            AdminAct::Organize => "/library/organize",
            AdminAct::Verify => "/library/verify",
            AdminAct::Artwork => "/library/artwork",
        }
    }

    fn started(self) -> &'static str {
        match self {
            AdminAct::Scan => "Library scan started",
            AdminAct::Autotag => "Auto-tagging started",
            AdminAct::Organize => "Organizing…",
            AdminAct::Verify => "Verification started",
            AdminAct::Artwork => "Looking for missing covers (Cover Art Archive, Deezer, iTunes)…",
        }
    }
}

#[derive(Debug)]
pub enum LibDialog {
    Confirm {
        title: String,
        text: String,
        ok: String,
        danger: bool,
        on_ok: Box<Message>,
    },
    /// Delete… of the picked albums (0.36): the dry run's title and words; `can_go` false when the Trash's
    /// disk has no room (then only Close).
    Trash { title: String, text: String, can_go: bool },
}

/// A Move to Trash on its way (0.36): the selection in chunks of 100, one call after the other.
#[derive(Debug, Default)]
pub struct Moving {
    chunks: Vec<Vec<String>>,
    /// The chunk whose answer is awaited.
    next: usize,
    /// Albums asked for in all, and so far.
    n: usize,
    done: usize,
    moved: usize,
    first_reason: String,
}

#[derive(Debug, Default)]
pub struct State {
    pub tab: String,
    pub q: String,
    pub sort: String,
    pub dir: String,
    pub filter: String,
    /// The artist the list is filtered to (`#/library?artist=`), "" for the whole library.
    pub artist: String,
    pub admin: bool,
    /// Answers of older requests are dropped.
    seq: u64,
    /// The filter box's debounce.
    settle: u64,
    pub albums: Option<Vec<Value>>,
    pub total: usize,
    pub artists: Option<Vec<Value>>,
    pub tracks: Option<Vec<Value>>,
    pub dups: Option<Vec<Value>>,
    pub issues: Option<Vec<Value>>,
    /// The Surround tab's albums and `/surround/albums`' `stats`.
    pub surround: Option<Vec<Value>>,
    pub surround_stats: Value,
    pub error: String,
    /// Another page can be loaded.
    pub more: bool,
    loading_more: bool,
    offset: usize,
    /// The chosen sort per tab (the web's `encore.libSort`, kept for the process).
    saved: HashMap<String, (String, String)>,
    /// `/library/stats` (admins).
    pub stats: Option<Value>,
    was_busy: bool,
    /// The chosen copy to keep per duplicate group.
    keep: HashMap<String, i64>,
    pub dialog: Option<LibDialog>,
    pub shared: Shared,
    /// The side scrubber's sections (`/library/index`): (label, rail text, first row).
    index: Vec<(String, String, usize)>,
    index_seq: u64,
    /// The row a scrubber label asked for: pages load until it is there, then it scrolls in.
    jump: Option<usize>,
    /// The row that carries [`JUMP_ID`].
    target: Option<usize>,
    /// Rows the next page request asks for beyond its usual size (a jump far down).
    want: usize,
    /// The limit of the last songs request (a full answer means there may be more).
    songs_limit: usize,
    /// With a query: the one results view (true) or the selected tab filtered by it (false).
    pub results: bool,
    /// `/library/search` for `found_for`: the answer or why it failed; None while it runs.
    found: Option<Result<Value, String>>,
    found_for: String,
    /// The Albums tab's Type (0.36): a filter key of `/library/album-types` ("" = All types).
    pub album_type: String,
    /// The Type dropdown's entries (`/library/album-types`); empty: no dropdown (an older server).
    pub types: Vec<Value>,
    types_seq: u64,
    /// Select mode (0.36, admins): the grid's tiles pick albums instead of opening them.
    pub selecting: bool,
    pub sel: lt::Selection,
    /// A Move to Trash running (the bar shows how far it is).
    pub moving: Option<Moving>,
}

/// The first matches each section of the results view shows (the phone's and the web's 12).
pub const RESULTS_LIMIT: usize = 12;

/// The id of the row a scrubber jump scrolls to.
const JUMP_ID: &str = "lib-jump";

#[derive(Clone, Debug)]
pub enum Message {
    /// `/library/index` for the side scrubber.
    Index(u64, Result<Value, ApiError>),
    /// A scrubber label: show the row at this offset.
    Jump(usize),
    /// The rows up to the jump are drawn: scroll there.
    ScrollToJump,
    Tab(String),
    Query(String),
    /// The filter box settled (300 ms after the last keystroke).
    Settled(u64),
    Sort(Opt),
    Dir,
    Filter(Opt),
    LoadMore,
    Albums {
        seq: u64,
        append: bool,
        res: Result<Value, ApiError>,
    },
    Artists(u64, Result<Value, ApiError>),
    Tracks {
        seq: u64,
        append: bool,
        res: Result<Value, ApiError>,
    },
    Dups(u64, Result<Value, ApiError>),
    Issues(u64, Result<Value, ApiError>),
    Surround {
        seq: u64,
        append: bool,
        res: Result<Value, ApiError>,
    },
    OpenAlbum(String),
    PlayAlbum(String),
    AlbumTracks(String, Result<Value, ApiError>),
    /// (MusicBrainz id, name).
    OpenArtist(String, String),
    /// One artist of a song (its line links each one).
    OpenPage(Page),
    RemoveArtistAsk(String),
    RemoveArtist(String),
    ArtistRemoved(Result<Value, ApiError>),
    PlayTrack(usize),
    TrackMenu(usize),
    Shuffle,
    Shuffled(Result<Value, ApiError>),
    Admin(AdminAct),
    AdminGo(AdminAct),
    AdminDone(AdminAct, Result<Value, ApiError>),
    Poll,
    Stats(Result<Value, ApiError>),
    DupKeep(String, i64),
    DupIgnore(String),
    DupResolve(String),
    /// (group key, copies removed – None for "Not duplicates").
    DupDone(String, Option<usize>, Result<Value, ApiError>),
    DupResolveAll,
    DupResolveAllGo,
    DupAllDone(Result<Value, ApiError>),
    ClearArtist,
    ShareArtist,
    ArtistShared(Result<Value, ApiError>),
    CloseDialog,
    Shared(Act),
    /// `/library/search` answered for this query.
    Found(String, Result<Value, ApiError>),
    /// "Show all N" of a section: that tab, filtered by the query.
    ShowAll(String),
    /// Nothing in the library: Discover with the query.
    SearchEverywhere,
    /// A song of the results view: play the found songs from it.
    PlayFound(usize),
    FoundMenu(usize),
    /// The Type dropdown (0.36).
    TypePick(lt::TypeOpt),
    /// `/library/album-types` answered.
    Types(u64, Result<Value, ApiError>),
    /// Select (admins): pick albums to move to the Trash.
    SelectMode,
    /// Done: leave select mode.
    SelectDone,
    /// A tile in select mode: pick or unpick its album.
    Pick(String),
    /// Select all N: every album of the list as it is filtered.
    SelectAll,
    SelectAllGot(Result<Value, ApiError>),
    ClearSelection,
    /// Delete…: a dry run over the picked albums, then the dialog.
    DeleteSelected,
    DryRun(Result<Value, ApiError>),
    /// The dialog's Move to Trash: the real runs, 100 albums a call.
    MoveToTrash,
    TrashChunk(Result<Value, ApiError>),
}

/// Called when the app navigates here (with the route's parameters).
pub fn open(state: &mut State, tab: String, query: crate::pages::LibraryQuery, ctx: &mut Ctx) -> Task<Message> {
    open_inner(state, tab, String::new(), query, ctx)
}

/// The same page filtered to one artist (`#/library?artist=…`, the library_artist page).
pub fn open_with(state: &mut State, tab: String, artist: String, ctx: &mut Ctx) -> Task<Message> {
    open_inner(state, tab, artist, Default::default(), ctx)
}

/// The route's sort (when the tab has it) and direction win over the saved ones for this visit,
/// as the web's `paintSort` with the route's `sort`; they are saved only when changed here.
fn open_inner(
    state: &mut State,
    tab: String,
    artist: String,
    query: crate::pages::LibraryQuery,
    ctx: &mut Ctx,
) -> Task<Message> {
    state.admin = ctx.is_admin();
    let mut tab = if tab.is_empty() { "albums".to_string() } else { tab };
    if (!state.admin && matches!(tab.as_str(), "duplicates" | "issues"))
        || (tab == "surround" && !ctx.has_feature("surround"))
    {
        tab = "albums".into();
    }
    state.tab = tab;
    state.artist = artist;
    state.q.clear();
    state.filter.clear();
    state.stats = None;
    state.was_busy = false;
    state.dialog = None;
    state.shared.reset();
    state.saved = saved_sorts(ctx.config);
    apply_sort(state);
    if let Some(opts) = sort_opts(&state.tab) {
        if opts.iter().any(|o| o.0 == query.sort) {
            state.sort = query.sort.clone();
            state.dir = if desc_first(&state.sort) { "desc".into() } else { "asc".into() };
        }
        if matches!(query.dir.as_str(), "asc" | "desc") {
            state.dir = query.dir.clone();
        }
    }
    state.q = query.q;
    if FILTERS.iter().any(|f| f.0 == query.filter) {
        state.filter = query.filter;
    }
    state.album_type = query.album_type;
    state.selecting = false;
    state.sel.clear();
    state.moving = None;
    // a query in the route opens on the results view
    state.results = !state.q.trim().is_empty();
    state.found = None;
    state.found_for.clear();
    let mut tasks = vec![load(state, ctx, false), load_types(state, ctx)];
    if state.admin {
        tasks.push(poll_stats(ctx));
    }
    Task::batch(tasks)
}

/// `paintSort`: the sort and direction for the tab, from the saved choice or the defaults.
fn apply_sort(state: &mut State) {
    let Some(opts) = sort_opts(&state.tab) else {
        return;
    };
    let saved = state.saved.get(&state.tab).cloned();
    let cur = saved.as_ref().map(|(s, _)| s.clone()).unwrap_or_default();
    state.sort = if opts.iter().any(|o| o.0 == cur) { cur } else { opts[0].0.to_string() };
    state.dir = match &saved {
        Some((s, d)) if *s == state.sort && !d.is_empty() => d.clone(),
        _ if desc_first(&state.sort) => "desc".into(),
        _ => "asc".into(),
    };
}

fn save_sort(state: &mut State) {
    state
        .saved
        .insert(state.tab.clone(), (state.sort.clone(), state.dir.clone()));
}

/// The web's `encore.libSort`: `{tab: {sort, dir}}`, kept in the config.
const SORT_KEY: &str = "encore.libSort";

/// The saved sorts from the config.
fn saved_sorts(c: &crate::config::Config) -> HashMap<String, (String, String)> {
    c.local(SORT_KEY)
        .and_then(Value::as_object)
        .map(|o| {
            o.iter()
                .map(|(tab, v)| (tab.clone(), (fmt::s(v, "sort"), fmt::s(v, "dir"))))
                .collect()
        })
        .unwrap_or_default()
}

/// Keep the sorts in the config, as the web keeps them in localStorage.
fn persist_sorts(state: &State, ctx: &mut Ctx) {
    let v: serde_json::Map<String, Value> = state
        .saved
        .iter()
        .map(|(tab, (sort, dir))| (tab.clone(), json!({ "sort": sort, "dir": dir })))
        .collect();
    ctx.edit_config(move |c| {
        c.local.insert(SORT_KEY.into(), Value::Object(v));
    });
}

fn poll_stats(ctx: &Ctx) -> Task<Message> {
    let api = ctx.api.clone();
    Task::perform(async move { api.get("/library/stats").await }, Message::Stats)
}

/// `load(append)`: the list of the current tab, from the start or the next page.
/// Load the list (a new first page, or the next one) and, for a new list, the scrubber's sections.
fn load(state: &mut State, ctx: &Ctx, append: bool) -> Task<Message> {
    if append {
        return load_list(state, ctx, true);
    }
    if results_view(state) {
        return search(state, ctx);
    }
    state.jump = None;
    state.target = None;
    let list = load_list(state, ctx, false);
    Task::batch([list, load_index(state, ctx)])
}

/// `loadIndex()`: the scrubber's sections for the list as it is sorted (none while filtering
/// by text, and not for Duplicates or Wrong audio).
fn load_index(state: &mut State, ctx: &Ctx) -> Task<Message> {
    state.index_seq += 1;
    state.index.clear();
    if !matches!(state.tab.as_str(), "albums" | "singles" | "artists" | "tracks") || !state.q.is_empty() {
        return Task::none();
    }
    let seq = state.index_seq;
    let kind = if state.tab == "albums" { "albums&album_kind=albums".to_string() } else { state.tab.clone() };
    let album_type = if state.tab == "albums" { state.album_type.as_str() } else { "" };
    let path = format!(
        "/library/index?kind={kind}&sort={}&dir={}&filter={}&artist={}&type={}",
        state.sort,
        state.dir,
        state.filter,
        urlencode(&state.artist),
        urlencode(album_type)
    );
    let api = ctx.api.clone();
    Task::perform(async move { api.get(&path).await }, move |r| Message::Index(seq, r))
}

/// The rows of the current tab that are loaded.
fn loaded_rows(state: &State) -> usize {
    match state.tab.as_str() {
        "albums" | "singles" => state.albums.as_ref().map_or(0, Vec::len),
        "artists" => state.artists.as_ref().map_or(0, Vec::len),
        "tracks" => state.tracks.as_ref().map_or(0, Vec::len),
        "surround" => state.surround.as_ref().map_or(0, Vec::len),
        _ => 0,
    }
}

/// `jump(offset)`: load pages until the row is there, then scroll to it.
fn continue_jump(state: &mut State, ctx: &Ctx) -> Task<Message> {
    let Some(offset) = state.jump else {
        return Task::none();
    };
    let loaded = loaded_rows(state);
    if loaded <= offset && state.more {
        if state.loading_more {
            // the page on its way continues the jump
            return Task::none();
        }
        state.want = offset + 1 - loaded;
        return load(state, ctx, true);
    }
    state.jump = None;
    if loaded == 0 {
        return Task::none();
    }
    state.target = Some(offset.min(loaded - 1));
    Task::done(Message::ScrollToJump)
}

/// Scroll the main area so the jump row is at the top (16 px below the edge, as the web).
fn scroll_to_jump() -> Task<Message> {
    iced::advanced::widget::operate(RowTop::default()).then(|y: f32| {
        iced::widget::operation::scroll_to(
            crate::app::MAIN_SCROLL,
            iced::widget::operation::AbsoluteOffset {
                x: None,
                y: Some((y - 16.0).max(0.0)),
            },
        )
    })
}

/// Finds where the jump row starts inside the main scroll area's content.
#[derive(Default)]
struct RowTop {
    content_y: Option<f32>,
    row_y: Option<f32>,
}

impl iced::advanced::widget::Operation<f32> for RowTop {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn iced::advanced::widget::Operation<f32>)) {
        operate(self);
    }

    fn container(&mut self, id: Option<&iced::advanced::widget::Id>, bounds: iced::Rectangle) {
        if id == Some(&iced::advanced::widget::Id::new(JUMP_ID)) {
            self.row_y = Some(bounds.y);
        }
    }

    fn scrollable(
        &mut self,
        id: Option<&iced::advanced::widget::Id>,
        _bounds: iced::Rectangle,
        content_bounds: iced::Rectangle,
        _translation: iced::Vector,
        _state: &mut dyn iced::advanced::widget::operation::Scrollable,
    ) {
        if id == Some(&iced::advanced::widget::Id::new(crate::app::MAIN_SCROLL)) {
            self.content_y = Some(content_bounds.y);
        }
    }

    fn finish(&self) -> iced::advanced::widget::operation::Outcome<f32> {
        match (self.content_y, self.row_y) {
            (Some(c), Some(r)) => iced::advanced::widget::operation::Outcome::Some(r - c),
            _ => iced::advanced::widget::operation::Outcome::None,
        }
    }
}

/// The scrubber's sections from `/library/index` (`[{label, offset, short?}]`).
fn parse_index(v: &Value) -> Vec<(String, String, usize)> {
    v.as_array()
        .map(|a| {
            a.iter()
                .map(|s| {
                    let label = fmt::s(s, "label");
                    let short = fmt::s(s, "short");
                    let short = if short.is_empty() { label.clone() } else { short };
                    (label, short, fmt::f(s, "offset").max(0.0) as usize)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Wrap the jump target's row in a container with [`JUMP_ID`].
fn mark_row<'a>(state: &State, i: usize, el: El<'a, Message>) -> El<'a, Message> {
    if state.target == Some(i) {
        container(el).id(JUMP_ID).into()
    } else {
        el
    }
}

/// Whether the page shows the one results view: a query, typed (not a tab clicked since), on
/// the whole library (the artist page's filter keeps its tabs).
pub fn results_view(state: &State) -> bool {
    state.results && !state.q.trim().is_empty() && state.artist.is_empty()
}

/// `GET /api/library/search?q=&limit=12` for the results view (the newest query wins).
fn search(state: &mut State, ctx: &Ctx) -> Task<Message> {
    let q = state.q.trim().to_string();
    state.found = None;
    state.found_for = q.clone();
    let api = ctx.api.clone();
    let url = format!("/library/search?q={}&limit={RESULTS_LIMIT}", urlencode(&q));
    Task::perform(async move { api.get(&url).await }, move |r| Message::Found(q.clone(), r))
}

/// Keep the query (and the tab) in the route, so Back comes back to it.
fn keep_route(state: &State, ctx: &mut Ctx) {
    if state.artist.is_empty() {
        ctx.set_route(Page::Library {
            tab: state.tab.clone(),
            query: crate::pages::LibraryQuery {
                q: state.q.trim().to_string(),
                filter: state.filter.clone(),
                album_type: if state.tab == "albums" { state.album_type.clone() } else { String::new() },
                ..Default::default()
            },
        });
    }
}

/// The album kind a list asks for: the Albums tab with a query lists the singles too, as the
/// results view's count does (client-shapes §3.4).
fn album_kind(state: &State) -> &str {
    if state.tab == "albums" && !state.q.trim().is_empty() {
        ""
    } else {
        state.tab.as_str()
    }
}

/// The albums list's query (GET /api/library/albums): the tab's kind, or the Type when one is picked (Albums tab).
fn albums_query(state: &State) -> lt::AlbumsQuery {
    lt::AlbumsQuery {
        q: state.q.clone(),
        sort: state.sort.clone(),
        dir: state.dir.clone(),
        kind: album_kind(state).to_string(),
        filter: state.filter.clone(),
        artist: state.artist.clone(),
        album_type: if state.tab == "albums" { state.album_type.clone() } else { String::new() },
    }
}

/// The Type dropdown's counts (GET /api/library/album-types) for this artist and filter.
fn load_types(state: &mut State, ctx: &Ctx) -> Task<Message> {
    state.types_seq += 1;
    let seq = state.types_seq;
    let url = format!("/library/album-types?artist={}&filter={}", urlencode(&state.artist), urlencode(&state.filter));
    let api = ctx.api.clone();
    Task::perform(async move { api.get(&url).await }, move |r| Message::Types(seq, r))
}

/// Select mode can be on: an admin on the Albums or Singles & EPs tab (not the results view).
fn can_select(state: &State) -> bool {
    state.admin && matches!(state.tab.as_str(), "albums" | "singles") && !results_view(state)
}

/// Changing the tab, Type, filter, sort or search clears what was picked (select mode stays on).
fn clear_selection(state: &mut State) {
    state.sel.clear();
}

fn leave_select(state: &mut State) {
    state.selecting = false;
    state.sel.clear();
}

/// The next chunk of a Move to Trash, or – all done – the toast (with Open Trash), and the list, the Type counts
/// and the side index again.
fn trash_next(state: &mut State, ctx: &mut Ctx) -> Task<Message> {
    let Some(m) = state.moving.as_mut() else {
        return Task::none();
    };
    if let Some(chunk) = m.chunks.get(m.next).cloned() {
        let api = ctx.api.clone();
        return Task::perform(
            async move { api.post("/library/albums/trash", json!({ "ids": chunk, "dry_run": false })).await },
            Message::TrashChunk,
        );
    }
    let m = state.moving.take().unwrap_or_default();
    let reason = if m.first_reason.is_empty() && m.moved < m.n { "It is no longer in the library".to_string() } else { m.first_reason };
    ctx.toast_action(lt::moved_text(m.moved, m.n, &reason), lt::OPEN_TRASH, crate::player::Message::OpenTrash, lt::TOAST_MS);
    leave_select(state);
    Task::batch([load_types(state, ctx), load(state, ctx, false)])
}

fn load_list(state: &mut State, ctx: &Ctx, append: bool) -> Task<Message> {
    state.seq += 1;
    let seq = state.seq;
    if !append {
        state.offset = 0;
        state.more = false;
        state.error.clear();
        match state.tab.as_str() {
            "albums" | "singles" => state.albums = None,
            "artists" => state.artists = None,
            "tracks" => state.tracks = None,
            "duplicates" => state.dups = None,
            "issues" => state.issues = None,
            "surround" => state.surround = None,
            _ => {}
        }
    }
    state.loading_more = append;
    let want = std::mem::take(&mut state.want);
    let api = ctx.api.clone();
    match state.tab.as_str() {
        "albums" | "singles" => {
            let limit = if want > 0 { want.clamp(PAGE, 500) } else { PAGE };
            let url = albums_query(state).page(state.offset, limit);
            Task::perform(async move { api.get(&url).await }, move |res| Message::Albums { seq, append, res })
        }
        "artists" => {
            let url = format!(
                "/library/artists?q={}&sort={}&dir={}",
                urlencode(&state.q),
                state.sort,
                state.dir
            );
            Task::perform(async move { api.get(&url).await }, move |res| Message::Artists(seq, res))
        }
        "duplicates" => Task::perform(async move { api.get("/library/duplicates").await }, move |res| {
            Message::Dups(seq, res)
        }),
        "issues" => Task::perform(async move { api.get("/library/mismatches").await }, move |res| {
            Message::Issues(seq, res)
        }),
        "surround" => {
            let url = format!(
                "/surround/albums?q={}&sort={}&limit={PAGE}&offset={}",
                urlencode(&state.q),
                state.sort,
                state.offset
            );
            Task::perform(async move { api.get(&url).await }, move |res| Message::Surround { seq, append, res })
        }
        _ => {
            let limit = if want > 0 { want.clamp(SONGS_PAGE, 1000) } else { SONGS_PAGE };
            state.songs_limit = limit;
            let url = format!(
                "/library/tracks?q={}&sort={}&dir={}&limit={limit}&offset={}",
                urlencode(&state.q),
                state.sort,
                state.dir,
                if append { state.offset } else { 0 }
            );
            Task::perform(async move { api.get(&url).await }, move |res| Message::Tracks { seq, append, res })
        }
    }
}

/// Something of the admin's bulk work is running.
fn busy(s: &Value) -> bool {
    fmt::truthy(&s["scan"], "scanning")
        || fmt::truthy(&s["bulk"], "running")
        || fmt::truthy(&s["verify"], "running")
        || fmt::truthy(&s["artwork"], "running")
        || fmt::truthy(&s["organize"], "running")
}

fn confirm_dialog(state: &mut State, title: &str, text: impl Into<String>, ok: &str, danger: bool, on_ok: Message) {
    state.dialog = Some(LibDialog::Confirm {
        title: title.into(),
        text: text.into(),
        ok: ok.into(),
        danger,
        on_ok: Box::new(on_ok),
    });
}

pub fn update(state: &mut State, msg: Message, ctx: &mut Ctx) -> Task<Message> {
    match msg {
        Message::Index(seq, r) => {
            if seq == state.index_seq {
                state.index = r.map(|v| parse_index(&v)).unwrap_or_default();
            }
        }
        Message::Jump(offset) => {
            state.jump = Some(offset);
            return continue_jump(state, ctx);
        }
        Message::ScrollToJump => return scroll_to_jump(),
        Message::Tab(tab) => {
            // with a query, a tab shows that tab filtered by it
            if tab == state.tab && !results_view(state) {
                return Task::none();
            }
            state.results = false;
            state.tab = tab;
            if can_select(state) {
                clear_selection(state);
            } else {
                leave_select(state);
            }
            apply_sort(state);
            keep_route(state, ctx);
            return load(state, ctx, false);
        }
        Message::ShowAll(tab) => {
            state.results = false;
            state.tab = tab;
            apply_sort(state);
            keep_route(state, ctx);
            return load(state, ctx, false);
        }
        Message::SearchEverywhere => {
            let q = state.q.trim().to_string();
            ctx.navigate(Page::Search { q });
        }
        Message::Found(q, r) => {
            if q == state.found_for {
                state.found = Some(r.map_err(|e| e.message));
            }
        }
        Message::PlayFound(i) => {
            if let Some(Ok(v)) = &state.found {
                let songs = fmt::items(&v["songs"]);
                if i < songs.len() {
                    ctx.play(songs, i);
                }
            }
        }
        Message::FoundMenu(i) => {
            if let Some(Ok(v)) = &state.found {
                if let Some(t) = fmt::items(&v["songs"]).get(i).cloned() {
                    return track_menu(&mut state.shared, &t, ctx).map(Message::Shared);
                }
            }
        }
        Message::Query(q) => {
            state.q = q;
            // typing shows the results view again
            state.results = true;
            if can_select(state) {
                clear_selection(state);
            } else {
                leave_select(state);
            }
            state.settle += 1;
            let n = state.settle;
            return Task::perform(tokio::time::sleep(Duration::from_millis(300)), move |_| Message::Settled(n));
        }
        Message::Settled(n) => {
            if n == state.settle {
                keep_route(state, ctx);
                return load(state, ctx, false);
            }
        }
        Message::Sort(o) => {
            clear_selection(state);
            state.sort = o.key.to_string();
            state.dir = if desc_first(o.key) { "desc".into() } else { "asc".into() };
            save_sort(state);
            persist_sorts(state, ctx);
            return load(state, ctx, false);
        }
        Message::Dir => {
            clear_selection(state);
            state.dir = if state.dir == "asc" { "desc".into() } else { "asc".into() };
            save_sort(state);
            persist_sorts(state, ctx);
            return load(state, ctx, false);
        }
        Message::Filter(o) => {
            clear_selection(state);
            state.filter = o.key.to_string();
            return Task::batch([load_types(state, ctx), load(state, ctx, false)]);
        }
        Message::TypePick(o) => {
            clear_selection(state);
            state.album_type = o.key;
            keep_route(state, ctx);
            return load(state, ctx, false);
        }
        Message::Types(seq, r) => {
            if seq == state.types_seq {
                // an older server has no types: no dropdown
                state.types = r.map(|v| fmt::items(&v["types"])).unwrap_or_default();
            }
        }
        Message::SelectMode => {
            if can_select(state) {
                state.selecting = true;
                state.sel.clear();
            }
        }
        Message::SelectDone => {
            if state.moving.is_none() {
                leave_select(state);
            }
        }
        Message::Pick(id) => {
            if state.selecting && state.moving.is_none() {
                state.sel.toggle(&id);
            }
        }
        Message::SelectAll => {
            if state.selecting && state.moving.is_none() {
                let url = albums_query(state).ids();
                let api = ctx.api.clone();
                return Task::perform(async move { api.get(&url).await }, Message::SelectAllGot);
            }
        }
        Message::SelectAllGot(Ok(v)) => {
            if state.selecting {
                let ids: Vec<String> = fmt::items(&v["ids"])
                    .iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .filter(|x| !x.is_empty())
                    .collect();
                if state.sel.select_all(&ids) || fmt::truthy(&v, "capped") {
                    ctx.toast_warn(lt::AT_MOST);
                }
            }
        }
        Message::SelectAllGot(Err(e)) => ctx.toast_err(e.message),
        Message::ClearSelection => {
            if state.moving.is_none() {
                clear_selection(state);
            }
        }
        Message::DeleteSelected => {
            if !state.selecting || state.sel.is_empty() || state.moving.is_some() {
                return Task::none();
            }
            let ids = state.sel.ids().to_vec();
            let api = ctx.api.clone();
            return Task::perform(
                async move { api.post("/library/albums/trash", json!({ "ids": ids, "dry_run": true })).await },
                Message::DryRun,
            );
        }
        Message::DryRun(Ok(d)) => {
            let (title, text, can_go) = lt::trash_dialog_words(&d);
            state.dialog = Some(LibDialog::Trash { title, text, can_go });
        }
        Message::DryRun(Err(e)) => ctx.toast_err(e.message),
        Message::MoveToTrash => {
            state.dialog = None;
            if state.sel.is_empty() || state.moving.is_some() {
                return Task::none();
            }
            let ids = state.sel.ids().to_vec();
            state.moving = Some(Moving {
                chunks: lt::chunks(&ids),
                n: ids.len(),
                ..Default::default()
            });
            return trash_next(state, ctx);
        }
        Message::TrashChunk(r) => {
            let Some(m) = state.moving.as_mut() else {
                return Task::none();
            };
            match r {
                Ok(v) => {
                    m.moved += fmt::i(&v, "albums").max(0) as usize;
                    if m.first_reason.is_empty() {
                        m.first_reason = fmt::items(&v["failed"]).first().map(|f| fmt::s(f, "reason")).unwrap_or_default();
                    }
                }
                Err(e) => {
                    if m.first_reason.is_empty() {
                        m.first_reason = e.message;
                    }
                }
            }
            m.done += m.chunks.get(m.next).map_or(0, Vec::len);
            m.next += 1;
            return trash_next(state, ctx);
        }
        Message::LoadMore => {
            if state.more && !state.loading_more {
                return load(state, ctx, true);
            }
        }
        Message::Albums { seq, append, res } => {
            if seq != state.seq {
                return Task::none();
            }
            state.loading_more = false;
            match res {
                Ok(v) => {
                    let items = fmt::items(&v);
                    state.total = fmt::i(&v, "total").max(0) as usize;
                    let list = state.albums.get_or_insert_with(Vec::new);
                    if append {
                        list.extend(items);
                    } else {
                        *list = items;
                    }
                    state.offset = list.len();
                    state.more = state.offset < state.total;
                }
                Err(e) => {
                    state.jump = None;
                    if !append {
                        state.albums = Some(Vec::new());
                        state.error = e.message;
                    } else {
                        ctx.toast_err(e.message);
                    }
                }
            }
            return continue_jump(state, ctx);
        }
        Message::Artists(seq, res) => {
            if seq != state.seq {
                return Task::none();
            }
            match res {
                Ok(v) => state.artists = Some(fmt::items(&v)),
                Err(e) => {
                    state.artists = Some(Vec::new());
                    state.error = e.message;
                }
            }
            state.more = false;
            return continue_jump(state, ctx);
        }
        Message::Tracks { seq, append, res } => {
            if seq != state.seq {
                return Task::none();
            }
            state.loading_more = false;
            match res {
                Ok(v) => {
                    let rows = fmt::items(&v);
                    state.more = rows.len() >= state.songs_limit.max(1);
                    let list = state.tracks.get_or_insert_with(Vec::new);
                    if append {
                        list.extend(rows);
                    } else {
                        *list = rows;
                    }
                    state.offset = list.len();
                }
                Err(e) => {
                    state.jump = None;
                    if !append {
                        state.tracks = Some(Vec::new());
                        state.error = e.message;
                    } else {
                        ctx.toast_err(e.message);
                    }
                }
            }
            return continue_jump(state, ctx);
        }
        Message::Dups(seq, res) => {
            if seq != state.seq {
                return Task::none();
            }
            match res {
                Ok(v) => {
                    let groups = fmt::items(&v);
                    state.keep = groups
                        .iter()
                        .map(|g| (fmt::s(g, "key"), fmt::i(g, "keep")))
                        .collect();
                    state.dups = Some(groups);
                }
                Err(e) => {
                    state.dups = Some(Vec::new());
                    state.error = e.message;
                }
            }
        }
        Message::Issues(seq, res) => {
            if seq != state.seq {
                return Task::none();
            }
            match res {
                Ok(v) => state.issues = Some(fmt::items(&v)),
                Err(e) => {
                    state.issues = Some(Vec::new());
                    state.error = e.message;
                }
            }
        }
        Message::Surround { seq, append, res } => {
            if seq != state.seq {
                return Task::none();
            }
            state.loading_more = false;
            match res {
                Ok(v) => {
                    let items = fmt::items(&v["albums"]);
                    // no total: a full page means there may be more
                    state.more = items.len() >= PAGE;
                    state.surround_stats = v["stats"].clone();
                    let list = state.surround.get_or_insert_with(Vec::new);
                    if append {
                        list.extend(items);
                    } else {
                        *list = items;
                    }
                    state.offset = list.len();
                }
                Err(e) => {
                    if !append {
                        state.surround = Some(Vec::new());
                        state.error = e.message;
                    } else {
                        ctx.toast_err(e.message);
                    }
                }
            }
        }
        Message::OpenAlbum(id) => ctx.navigate(Page::LibraryAlbum(id)),
        Message::PlayAlbum(id) => {
            let api = ctx.api.clone();
            let path = format!("/library/albums/{id}");
            return Task::perform(async move { api.get(&path).await }, move |r| Message::AlbumTracks(id.clone(), r));
        }
        Message::AlbumTracks(id, Ok(a)) => {
            let tracks = with_album_id(&fmt::items(&a["tracks"]), &id);
            if tracks.is_empty() {
                ctx.toast_err("This album has no songs");
            } else {
                ctx.play(tracks, 0);
            }
        }
        Message::AlbumTracks(_, Err(e)) => ctx.toast_err(e.message),
        Message::OpenPage(p) => ctx.navigate(p),
        Message::OpenArtist(mbid, name) => {
            if !mbid.is_empty() {
                ctx.navigate(Page::Artist(mbid));
            } else {
                ctx.navigate(Page::ArtistName(name));
            }
        }
        Message::RemoveArtistAsk(name) => {
            let text = format!("Delete every album by “{name}” from disk and cancel their pending downloads? Use this for artists that were added by a wrong download.");
            confirm_dialog(state, "Remove artist", text, "Remove", true, Message::RemoveArtist(name));
        }
        Message::RemoveArtist(name) => {
            state.dialog = None;
            let api = ctx.api.clone();
            return Task::perform(
                async move { api.del(&format!("/library/artists?name={}", urlencode(&name))).await },
                Message::ArtistRemoved,
            );
        }
        Message::ArtistRemoved(Ok(r)) => {
            ctx.toast_ok(format!("Removed {} song(s)", fmt::s(&r, "removed")));
            return load(state, ctx, false);
        }
        Message::ArtistRemoved(Err(e)) => ctx.toast_err(e.message),
        Message::PlayTrack(i) => {
            if let Some(t) = state.tracks.as_ref().and_then(|l| l.get(i)) {
                // a single song: the library shuffle continues after it
                ctx.play(vec![t.clone()], 0);
            }
        }
        Message::TrackMenu(i) => {
            if let Some(t) = state.tracks.as_ref().and_then(|l| l.get(i)).cloned() {
                return track_menu(&mut state.shared, &t, ctx).map(Message::Shared);
            }
        }
        Message::Shuffle => {
            let exclude: Vec<i64> = ctx
                .player
                .status
                .queue
                .iter()
                .filter_map(crate::engine::tracks::encore_id)
                .collect();
            let api = ctx.api.clone();
            return Task::perform(
                async move {
                    api.post("/library/shuffle", json!({ "limit": 50, "exclude": exclude }))
                        .await
                },
                Message::Shuffled,
            );
        }
        Message::Shuffled(Ok(v)) => {
            let items = fmt::items(&v);
            if items.is_empty() {
                ctx.toast_err("Your library is empty");
            } else {
                ctx.play(items, 0);
            }
        }
        Message::Shuffled(Err(e)) => ctx.toast_err(e.message),
        Message::Admin(act) => match act {
            AdminAct::Scan | AdminAct::Artwork => return update(state, Message::AdminGo(act), ctx),
            AdminAct::Verify => {
                let has_key = state.stats.as_ref().is_some_and(|s| fmt::truthy(s, "acoustid"));
                if !has_key {
                    ctx.toast_err("Add a free AcoustID API key in Settings → Library → Verification & duplicates first");
                    return Task::none();
                }
                confirm_dialog(state, "Verify audio", "Every unchecked file is fingerprinted and compared against AcoustID/MusicBrainz, so wrong songs, remixes posing as originals, covers and mislabeled tracks get flagged. Takes ~0.4s per track.", "Start", false, Message::AdminGo(act));
            }
            AdminAct::Organize => confirm_dialog(state, "Organize files", "Every song is moved into folders from its tags using your naming template (Settings → Library → Library & tagging), e.g. Artist/Album (Year)/01 - Title. Missing tags are filled in first from the file name, AcoustID and Deezer. Playlists and likes keep working.", "Organize", false, Message::AdminGo(act)),
            AdminAct::Autotag => confirm_dialog(state, "Auto-tag library", "Every album without MusicBrainz tags will be matched automatically. Confident matches get retagged and renamed using your naming template; the rest are left for manual matching.", "Start", false, Message::AdminGo(act)),
        },
        Message::AdminGo(act) => {
            state.dialog = None;
            let api = ctx.api.clone();
            return Task::perform(async move { api.post_empty(act.path()).await }, move |r| Message::AdminDone(act, r));
        }
        Message::AdminDone(act, Ok(_)) => {
            ctx.toast_ok(act.started());
            state.was_busy = true;
            return poll_stats(ctx);
        }
        Message::AdminDone(_, Err(e)) => ctx.toast_err(e.message),
        Message::Poll => {
            if state.admin {
                return poll_stats(ctx);
            }
        }
        Message::Stats(Ok(s)) => {
            let now_busy = busy(&s);
            let finished = state.was_busy && !now_busy;
            state.was_busy = now_busy;
            state.stats = Some(s);
            if finished {
                return load(state, ctx, false);
            }
        }
        Message::Stats(Err(_)) => {}
        Message::DupKeep(key, id) => {
            state.keep.insert(key, id);
        }
        Message::DupIgnore(key) => {
            let api = ctx.api.clone();
            let k = key.clone();
            return Task::perform(
                async move { api.post("/library/duplicates/ignore", json!({ "key": k })).await },
                move |r| Message::DupDone(key.clone(), None, r),
            );
        }
        Message::DupResolve(key) => {
            let Some(g) = state
                .dups
                .as_ref()
                .and_then(|l| l.iter().find(|g| fmt::s(g, "key") == key))
            else {
                return Task::none();
            };
            let keep = state.keep.get(&key).copied().unwrap_or_else(|| fmt::i(g, "keep"));
            let del: Vec<i64> = fmt::items(&g["tracks"])
                .iter()
                .map(|t| fmt::i(t, "id"))
                .filter(|i| *i != keep)
                .collect();
            let n = del.len();
            let api = ctx.api.clone();
            let k = key.clone();
            return Task::perform(
                async move {
                    api.post(
                        "/library/duplicates/resolve",
                        json!({ "key": k, "keep": keep, "delete": del }),
                    )
                    .await
                },
                move |r| Message::DupDone(key.clone(), Some(n), r),
            );
        }
        Message::DupDone(key, n, Ok(_)) => {
            if let Some(n) = n {
                ctx.toast_ok(format!("Removed {n} copy/copies"));
            }
            if let Some(l) = &mut state.dups {
                l.retain(|g| fmt::s(g, "key") != key);
            }
        }
        Message::DupDone(_, _, Err(e)) => ctx.toast_err(e.message),
        Message::DupResolveAll => {
            let n = state.dups.as_ref().map_or(0, Vec::len);
            confirm_dialog(
                state,
                "Resolve all duplicates",
                format!("Delete the lower-quality copies in {n} groups? Files are removed from disk."),
                "Delete copies",
                true,
                Message::DupResolveAllGo,
            );
        }
        Message::DupResolveAllGo => {
            state.dialog = None;
            let api = ctx.api.clone();
            return Task::perform(
                async move { api.post_empty("/library/duplicates/resolve-all").await },
                Message::DupAllDone,
            );
        }
        Message::DupAllDone(Ok(r)) => {
            ctx.toast_ok(format!("Removed {} duplicate file(s)", fmt::s(&r, "deleted")));
            return load(state, ctx, false);
        }
        Message::DupAllDone(Err(e)) => ctx.toast_err(e.message),
        Message::ClearArtist => ctx.navigate(Page::library("")),
        Message::ShareArtist => {
            let name = state.artist.clone();
            let api = ctx.api.clone();
            return Task::perform(
                async move { api.post("/library/artists/share", json!({ "name": name })).await },
                Message::ArtistShared,
            );
        }
        Message::ArtistShared(Ok(r)) => {
            ctx.copy(format!("{}{}", ctx.base(), fmt::s(&r, "path")));
            ctx.toast_ok("Link copied – anyone with it can play this artist");
        }
        Message::ArtistShared(Err(e)) => ctx.toast_err(e.message),
        Message::CloseDialog => state.dialog = None,
        Message::Shared(a) => {
            let task = shared_update(&mut state.shared, a, ctx).map(Message::Shared);
            if state.shared.take_changed() {
                state.shared.moved_to = None;
                return Task::batch([task, load(state, ctx, false)]);
            }
            return task;
        }
    }
    Task::none()
}

/// The line under the title: what the list holds.
fn sub_line(state: &State) -> String {
    if results_view(state) {
        return String::new();
    }
    match state.tab.as_str() {
        "albums" | "singles" => {
            if state.albums.is_some() {
                format!("{} {}", num(state.total as f64), state.tab)
            } else {
                String::new()
            }
        }
        "artists" => state
            .artists
            .as_ref()
            .map(|a| format!("{} artists", num(a.len() as f64)))
            .unwrap_or_default(),
        "duplicates" => state
            .dups
            .as_ref()
            .map(|d| format!("{} duplicate group(s)", d.len()))
            .unwrap_or_default(),
        "issues" => state
            .issues
            .as_ref()
            .map(|d| format!("{} file(s) whose audio doesn't match their tags", d.len()))
            .unwrap_or_default(),
        "surround" if state.surround.is_some() => format!(
            "{} surround albums · {} in Dolby Atmos",
            num(fmt::f(&state.surround_stats, "albums")),
            num(fmt::f(&state.surround_stats, "atmos"))
        ),
        _ => String::new(),
    }
}

/// A `.btn.sm` with an icon, a label and badges after it (the admin's Verify / Auto-tag).
fn labeled_btn<'a>(i: Icon, label: &'a str, badges: Vec<El<'a, Message>>, on: Message) -> button::Button<'a, Message> {
    let p = theme::look();
    let mut r = row![icon(i, 15.0, p.text), t(label).size(13).font(theme::semibold())]
        .spacing(7)
        .align_y(Alignment::Center);
    for b in badges {
        r = r.push(b);
    }
    button(r)
        .padding([6, 11])
        .style(theme::btn(Btn::Default))
        .on_press(on)
}

/// The admin's progress card while something runs (web `poll`).
fn bulk_card<'a>(s: &Value) -> Option<El<'a, Message>> {
    let line = |title: &str, sub: String, done: f64, total: f64| -> El<'a, Message> {
        let pct = if total > 0.0 { (done / total * 100.0) as f32 } else { 0.0 };
        widgets::card(
            column![
                row![
                    t(title.to_string()).size(14).font(theme::bold()),
                    small_muted(sub)
                ]
                .spacing(10)
                .align_y(Alignment::Center),
                progress_bar(0.0..=100.0, pct).girth(6).style(theme::progress),
            ]
            .spacing(10),
        )
        .width(Length::Fill)
        .into()
    };
    let scan = &s["scan"];
    if fmt::truthy(scan, "scanning") {
        return Some(line(
            "Scanning library",
            format!("{} / {} files", fmt::s(scan, "progress"), fmt::s(scan, "total")),
            fmt::f(scan, "progress"),
            fmt::f(scan, "total"),
        ));
    }
    let aw = &s["artwork"];
    if fmt::truthy(aw, "running") {
        return Some(line(
            "Finding artwork",
            format!("{} / {} · {} found", fmt::s(aw, "done"), fmt::s(aw, "total"), fmt::s(aw, "found")),
            fmt::f(aw, "done"),
            fmt::f(aw, "total"),
        ));
    }
    let v = &s["verify"];
    if fmt::truthy(v, "running") {
        return Some(line(
            "Verifying audio fingerprints",
            format!(
                "{} / {} · {} ok · {} wrong · {} unknown · {}",
                fmt::s(v, "done"),
                fmt::s(v, "total"),
                fmt::s(v, "verified"),
                fmt::s(v, "mismatch"),
                fmt::s(v, "unknown"),
                fmt::s(v, "current")
            ),
            fmt::f(v, "done"),
            fmt::f(v, "total"),
        ));
    }
    let o = &s["organize"];
    if fmt::truthy(o, "running") {
        return Some(line(
            "Organizing files",
            format!(
                "{} / {} · {} moved · {} re-tagged · {}",
                fmt::s(o, "done"),
                fmt::s(o, "total"),
                fmt::s(o, "moved"),
                fmt::s(o, "fixed"),
                fmt::s(o, "current")
            ),
            fmt::f(o, "done"),
            fmt::f(o, "total"),
        ));
    }
    let b = &s["bulk"];
    if fmt::truthy(b, "running") {
        return Some(line(
            "Auto-tagging",
            format!(
                "{} / {} · {} tagged · {} skipped · {}",
                fmt::s(b, "done"),
                fmt::s(b, "total"),
                fmt::s(b, "tagged"),
                fmt::s(b, "skipped"),
                fmt::s(b, "current")
            ),
            fmt::f(b, "done"),
            fmt::f(b, "total"),
        ));
    }
    None
}

pub fn view<'a>(state: &'a State, ctx: &Ctx<'a>) -> El<'a, Message> {
    let p = ctx.look;
    let sub = sub_line(state);
    let mut title_col = column![h1("Library")].spacing(2);
    if !sub.is_empty() {
        title_col = title_col.push(small_muted(sub));
    }
    let shuffle = tooltip(
        btn_icon(Icon::Shuffle, "Shuffle", Btn::Primary, Some(Message::Shuffle)),
        widgets::tip_box("Play everything in random order – no song twice this session"),
        tooltip::Position::Bottom,
    )
    .gap(4);
    let mut head = row![title_col, shuffle, space().width(Length::Fill)]
        .spacing(14)
        .align_y(Alignment::Center);
    if state.admin {
        let stats = state.stats.as_ref();
        let mut verify_badges: Vec<El<'a, Message>> = Vec::new();
        let mut autotag_badges: Vec<El<'a, Message>> = Vec::new();
        if let Some(s) = stats {
            let vc = &s["verify_counts"];
            if fmt::truthy(vc, "unchecked") {
                verify_badges.push(badge(format!("{} unchecked", num(fmt::f(vc, "unchecked"))), Badge::Plain));
            }
            if fmt::truthy(vc, "mismatch") {
                verify_badges.push(badge(format!("{} wrong", fmt::s(vc, "mismatch")), Badge::Err));
            }
            if fmt::truthy(s, "untagged_albums") {
                autotag_badges.push(badge(fmt::s(s, "untagged_albums"), Badge::Warn));
            }
        }
        let admin_row = row![
            labeled_btn(Icon::Refresh, "Scan", vec![], Message::Admin(AdminAct::Scan)),
            labeled_btn(Icon::Tag, "Auto-tag untagged", autotag_badges, Message::Admin(AdminAct::Autotag)),
            tooltip(
                labeled_btn(Icon::Disc, "Organize files", vec![], Message::Admin(AdminAct::Organize)),
                widgets::tip_box("Move every file into Artist/Album/NN Title folders (your naming template), filling in missing tags first"),
                tooltip::Position::Bottom,
            )
            .gap(4),
            labeled_btn(Icon::Shield, "Verify audio", verify_badges, Message::Admin(AdminAct::Verify)),
            labeled_btn(Icon::Disc, "Find missing artwork", vec![], Message::Admin(AdminAct::Artwork)),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .wrap();
        head = head.push(admin_row);
    }

    let mut page = column![head].spacing(18).width(Length::Fill);
    if let Some(card) = state.stats.as_ref().and_then(bulk_card) {
        page = page.push(card);
    }

    // the tab row with its tools
    let mut tab_items: Vec<(&'a str, &'a str)> = vec![
        ("albums", "Albums"),
        ("singles", lt::SINGLES_TAB),
        ("artists", "Artists"),
        ("tracks", "Songs"),
    ];
    if ctx.has_feature("surround") {
        tab_items.push(("surround", "Surround"));
    }
    if state.admin {
        tab_items.extend([("duplicates", "Duplicates"), ("issues", "Wrong audio")]);
    }
    let results = results_view(state);
    // the results view has no tab selected; a tab clicked shows that tab filtered by the query
    let mut tools = row![tabs(&tab_items, if results { "" } else { state.tab.as_str() }, Message::Tab)]
        .spacing(10)
        .align_y(Alignment::Center);
    if results || !matches!(state.tab.as_str(), "duplicates" | "issues") {
        tools = tools.push(
            container(search_box("Search artists, albums, songs in your library…", &state.q, Message::Query, None))
                .width(Length::Fixed(400.0)),
        );
    }
    if let Some(opts) = sort_opts(&state.tab).filter(|_| !results) {
        let options: Vec<Opt> = opts
            .iter()
            .map(|(k, l)| Opt {
                key: k,
                label: l,
                prefix: "Sort: ",
            })
            .collect();
        let selected = options.iter().find(|o| o.key == state.sort).cloned();
        let mut sort = row![pick_list(options, selected, Message::Sort)
            .text_size(13)
            .padding([7, 10])
            .style(theme::pick)
            .menu_style(theme::pick_menu)]
        .spacing(6)
        .align_y(Alignment::Center);
        // the Surround list has one direction per sort
        if state.tab != "surround" {
            sort = sort.push(
                tooltip(
                    btn_sm(if state.dir == "asc" { "↑" } else { "↓" }, Btn::Default, Some(Message::Dir)),
                    widgets::tip_box("Ascending / descending"),
                    tooltip::Position::Bottom,
                )
                .gap(4),
            );
        }
        tools = tools.push(sort);
    }
    if !results && matches!(state.tab.as_str(), "albums" | "singles") {
        let options: Vec<Opt> = FILTERS
            .iter()
            .map(|(k, l)| Opt {
                key: k,
                label: l,
                prefix: "",
            })
            .collect();
        let selected = options.iter().find(|o| o.key == state.filter).cloned();
        tools = tools.push(
            pick_list(options, selected, Message::Filter)
                .text_size(13)
                .padding([7, 10])
                .style(theme::pick)
                .menu_style(theme::pick_menu),
        );
    }
    // the Type dropdown (0.36): the Albums tab of one type – live albums, compilations, One-offs …
    if !results && state.tab == "albums" && !state.types.is_empty() {
        let options = lt::type_options(&state.types, &state.album_type);
        let selected = options.iter().find(|o| o.key == state.album_type).cloned();
        tools = tools.push(
            tooltip(
                pick_list(options, selected, Message::TypePick)
                    .text_size(13)
                    .padding([7, 10])
                    .style(theme::pick)
                    .menu_style(theme::pick_menu),
                widgets::tip_box(lt::TYPE),
                tooltip::Position::Bottom,
            )
            .gap(4),
        );
    }
    // Select (admins): pick albums, Select all, Delete… to the Trash
    if can_select(state) && !state.selecting {
        tools = tools.push(
            tooltip(
                btn_icon(Icon::Check, lt::SELECT, Btn::Default, Some(Message::SelectMode)),
                widgets::tip_box("Pick albums to delete"),
                tooltip::Position::Bottom,
            )
            .gap(4),
        );
    }
    if !state.artist.is_empty() {
        tools = tools.push(
            button(
                row![
                    t(state.artist.clone()).size(12).font(theme::semibold()),
                    icon(Icon::X, 12.0, p.text)
                ]
                .spacing(6)
                .align_y(Alignment::Center),
            )
            .padding([3, 9])
            .style(theme::btn(Btn::Default))
            .on_press(Message::ClearArtist),
        );
        tools = tools.push(
            tooltip(
                btn_icon(Icon::External, "Share", Btn::Default, Some(Message::ShareArtist)),
                widgets::tip_box("Share link"),
                tooltip::Position::Bottom,
            )
            .gap(4),
        );
    }
    page = page.push(tools.wrap());

    if results {
        return page.push(results_body(state, ctx)).into();
    }
    if state.selecting && can_select(state) {
        page = page.push(select_bar(state));
    }
    page = page.push(body(state, ctx));
    if state.more {
        page = page.push(
            container(btn(
                "Load more",
                Btn::Default,
                (!state.loading_more).then_some(Message::LoadMore),
            ))
            .width(Length::Fill)
            .center_x(Length::Fill),
        );
    }
    page.into()
}

/// The list of the current tab.
fn body<'a>(state: &'a State, ctx: &Ctx<'a>) -> El<'a, Message> {
    let p = ctx.look;
    let cur = ctx.player.current_id();
    match state.tab.as_str() {
        "albums" | "singles" => match &state.albums {
            None => skeleton_tiles(12, widgets::tile::WIDTH),
            Some(list) if list.is_empty() => {
                if !state.error.is_empty() {
                    return widgets::error_box(&state.error);
                }
                if !state.q.trim().is_empty() {
                    return widgets::catalog::empty_text(Icon::Library, format!("No albums match “{}”", state.q.trim()), String::new());
                }
                if state.tab == "albums" && !state.album_type.is_empty() {
                    return empty(Icon::Library, lt::NO_ALBUMS_OF_TYPE, "");
                }
                empty(
                    Icon::Library,
                    "No albums found",
                    if state.filter == "untagged" { "Everything is tagged" } else { "" },
                )
            }
            Some(list) if state.selecting && can_select(state) => grid(
                list.iter()
                    .enumerate()
                    .map(|(i, a)| mark_row(state, i, pick_tile(a, ctx, state.sel.contains(&fmt::s(a, "id")))))
                    .collect(),
            ),
            Some(list) => grid(
                list.iter()
                    .enumerate()
                    .map(|(i, a)| mark_row(state, i, album_tile(a, ctx)))
                    .collect(),
            ),
        },
        "artists" => match &state.artists {
            None => skeleton_tiles(12, widgets::tile::WIDTH),
            Some(list) if list.is_empty() => {
                if !state.error.is_empty() {
                    return widgets::error_box(&state.error);
                }
                if !state.q.trim().is_empty() {
                    return widgets::catalog::empty_text(Icon::Users, format!("No artists match “{}”", state.q.trim()), String::new());
                }
                empty(Icon::Users, "No artists found", "")
            }
            Some(list) => grid(
                list.iter()
                    .enumerate()
                    .map(|(i, a)| mark_row(state, i, artist_tile(state, ctx, a)))
                    .collect(),
            ),
        },
        "duplicates" => match &state.dups {
            None => container(
                column![
                    widgets::muted("Comparing fingerprints, ISRCs, MusicBrainz ids and tags…").size(14)
                ]
                .align_x(Alignment::Center),
            )
            .width(Length::Fill)
            .padding([44, 20])
            .center_x(Length::Fill)
            .into(),
            Some(groups) if groups.is_empty() => {
                if !state.error.is_empty() {
                    return widgets::error_box(&state.error);
                }
                empty(
                    Icon::Check,
                    "No duplicates found",
                    "Remixes, live versions, covers and edits are kept apart – only true copies of the same recording are listed here. Run “Verify audio” to also catch copies with different tags.",
                )
            }
            Some(groups) => {
                let top = widgets::card(
                    row![
                        widgets::muted("The best copy (verified → lossless → bitrate → tagged) is preselected to keep. Playlists pointing at removed copies are moved to the kept one.").size(13).width(Length::Fill),
                        btn_icon(Icon::Trash, format!("Resolve all {}", groups.len()), Btn::Danger, Some(Message::DupResolveAll)),
                    ]
                    .spacing(16)
                    .align_y(Alignment::Center),
                )
                .width(Length::Fill);
                let mut col = column![top].spacing(16);
                for g in groups {
                    col = col.push(dup_card(g, state, p));
                }
                col.into()
            }
        },
        "surround" => match &state.surround {
            None => skeleton_tiles(12, widgets::tile::WIDTH),
            Some(list) if list.is_empty() => {
                if !state.error.is_empty() {
                    return widgets::error_box(&state.error);
                }
                if !state.q.trim().is_empty() {
                    return widgets::catalog::empty_text(Icon::Disc, format!("No surround albums match “{}”", state.q.trim()), String::new());
                }
                empty(
                    Icon::Disc,
                    "No surround albums yet",
                    "Dolby Atmos and 5.1 albums show here. Put them in the Surround folder, or use Get Dolby Atmos version on an album.",
                )
            }
            Some(list) => grid(list.iter().map(|a| album_tile(a, ctx)).collect()),
        },
        "issues" => match &state.issues {
            None => skeleton_rows(6),
            Some(rows) if rows.is_empty() => {
                if !state.error.is_empty() {
                    return widgets::error_box(&state.error);
                }
                empty(Icon::Shield, "Nothing flagged", "Run “Verify audio” to fingerprint your library.")
            }
            Some(rows) => {
                let mut col = column![];
                for v in rows {
                    let aid = fmt::s(v, "album_id");
                    let lines = column![
                        row![
                            t(format!("{} – {}", fmt::s(v, "artist"), fmt::s(v, "title")))
                                .size(14)
                                .font(theme::bold()),
                            widgets::muted(format!("· {}", fmt::s(v, "album"))).size(13),
                        ]
                        .spacing(6)
                        .align_y(Alignment::Center),
                        t(fmt::s(v, "verify_detail")).size(12).color(p.err),
                    ]
                    .spacing(3)
                    .width(Length::Fill);
                    col = col.push(column![
                        container(
                            row![
                                lines,
                                btn_icon(Icon::Tag, "Fix", Btn::Default, Some(Message::OpenAlbum(aid)))
                            ]
                            .spacing(14)
                            .align_y(Alignment::Center)
                        )
                        .padding([11, 16]),
                        widgets::divider(),
                    ]);
                }
                widgets::card_flat(col).width(Length::Fill).into()
            }
        },
        _ => match &state.tracks {
            None => skeleton_rows(10),
            Some(rows) if rows.is_empty() => {
                if !state.error.is_empty() {
                    return widgets::error_box(&state.error);
                }
                if !state.q.trim().is_empty() {
                    return widgets::catalog::empty_text(Icon::Music, format!("No songs match “{}”", state.q.trim()), String::new());
                }
                empty(Icon::Music, "No songs found", "")
            }
            Some(rows) => {
                let mut col = column![];
                for (i, v) in rows.iter().enumerate() {
                    let id = fmt::i(v, "id");
                    let tail = if fmt::truthy(v, "album") { format!(" · {}", tr(ctx, v, "album")) } else { String::new() };
                    col = col.push(mark_row(
                        state,
                        i,
                        TrackRow::new(tr(ctx, v, "title"))
                            .sub_links(artist_links(ctx, v), tail)
                            .cover(ctx.cover(&fmt::s(v, "album_id"), &v["cover_v"], 80))
                            .quality(format_text(v))
                            .badge_maybe(verify_badge(v))
                            .duration(Some(fmt::f(v, "duration")))
                            .playing(cur == Some(id))
                            .on_play(Message::PlayTrack(i))
                            .on_menu(Message::TrackMenu(i))
                            .view(),
                    ));
                }
                widgets::card_flat(col).width(Length::Fill).into()
            }
        },
    }
}

/// `libraryTile(a)`: an album of the library with its Untagged corner and the play button.
/// An artist of the library (a round tile; the admin can remove it).
fn artist_tile<'a>(state: &State, ctx: &Ctx<'a>, a: &Value) -> El<'a, Message> {
    let name = fmt::s(a, "name");
    let albums = fmt::i(a, "albums");
    let mut tile = Tile::new(tr(ctx, a, "name"), TileKind::Artist, Message::OpenArtist(fmt::s(a, "mbid"), name.clone()))
        .sub(format!(
            "{albums} album{} · {} tracks",
            if albums == 1 { "" } else { "s" },
            fmt::s(a, "tracks")
        ))
        .cover(ctx.cover(&fmt::s(a, "cover_album"), &a["cover_v"], 400));
    if state.admin {
        tile = tile.action(Icon::Trash, "Remove this artist from the library", Message::RemoveArtistAsk(name));
    }
    tile.view()
}

/// "Show all N" of a results section: the server's count when it sent one (§3.4), else "Show
/// all" when the section came back full (an older server).
pub fn show_all_label(count: Option<u64>, shown: usize) -> Option<String> {
    match count {
        Some(total) if total as usize > shown => Some(format!("Show all {total}")),
        Some(_) => None,
        None if shown >= RESULTS_LIMIT => Some("Show all".into()),
        None => None,
    }
}

/// The one results view: Artists, Albums (singles too), Songs, each with "Show all N"; with
/// nothing anywhere, "Search everywhere".
fn results_body<'a>(state: &'a State, ctx: &Ctx<'a>) -> El<'a, Message> {
    let q = state.q.trim();
    let v = match &state.found {
        None => return skeleton_tiles(8, widgets::tile::WIDTH),
        Some(Err(e)) => return widgets::error_box(e),
        Some(Ok(v)) => v,
    };
    let (artists, albums, songs) = (fmt::items(&v["artists"]), fmt::items(&v["albums"]), fmt::items(&v["songs"]));
    if artists.is_empty() && albums.is_empty() && songs.is_empty() {
        return column![
            widgets::catalog::empty_text(Icon::Search, format!("Nothing in your library matches “{q}”"), String::new()),
            container(btn_icon(Icon::Search, "Search everywhere", Btn::Primary, Some(Message::SearchEverywhere)))
                .width(Length::Fill)
                .center_x(Length::Fill),
        ]
        .spacing(4)
        .into();
    }
    let counts = &v["counts"];
    let head = |title: &'a str, key: &str, shown: usize, tab: &str| -> El<'a, Message> {
        let action = show_all_label(counts[key].as_u64(), shown).map(|label| El::from(btn_sm(label, Btn::Ghost, Some(Message::ShowAll(tab.to_string())))));
        widgets::section_head(title, action)
    };
    let mut col = column![].spacing(26).width(Length::Fill);
    if !artists.is_empty() {
        let tiles = artists.iter().map(|a| artist_tile(state, ctx, a)).collect();
        col = col.push(column![head("Artists", "artists", artists.len(), "artists"), grid(tiles)].spacing(14));
    }
    if !albums.is_empty() {
        let tiles = albums.iter().map(|a| album_tile(a, ctx)).collect();
        col = col.push(column![head("Albums", "albums", albums.len(), "albums"), grid(tiles)].spacing(14));
    }
    if !songs.is_empty() {
        let cur = ctx.player.current_id();
        let mut list = column![];
        for (i, v) in songs.iter().enumerate() {
            let id = fmt::i(v, "id");
            let tail = if fmt::truthy(v, "album") { format!(" · {}", tr(ctx, v, "album")) } else { String::new() };
            list = list.push(
                TrackRow::new(tr(ctx, v, "title"))
                    .sub_links(artist_links(ctx, v), tail)
                    .cover(ctx.cover(&fmt::s(v, "album_id"), &v["cover_v"], 80))
                    .quality(format_text(v))
                    .badge_maybe(verify_badge(v))
                    .duration(Some(fmt::f(v, "duration")))
                    .playing(cur == Some(id))
                    .on_play(Message::PlayFound(i))
                    .on_menu(Message::FoundMenu(i))
                    .view(),
            );
        }
        col = col.push(column![head("Songs", "songs", songs.len(), "tracks"), widgets::card_flat(list)].spacing(14));
    }
    col.into()
}

pub fn album_tile<'a>(a: &Value, ctx: &Ctx<'a>) -> El<'a, Message> {
    let id = fmt::s(a, "id");
    let title = if fmt::truthy(a, "album") { tr(ctx, a, "album") } else { "Unknown album".to_string() };
    let mut sub = tr(ctx, a, "albumartist");
    if fmt::truthy(a, "year") {
        sub.push_str(&format!(" · {}", fmt::s(a, "year")));
    }
    let mut tile = Tile::new(title, TileKind::Album, Message::OpenAlbum(id.clone()))
        .sub(sub)
        .cover(ctx.cover(&id, &a["cover_v"], 400))
        .play(Message::PlayAlbum(id));
    // a Surround album names its format in the corner: Dolby Atmos, 5.1, 7.1
    if is_surround(a) {
        tile = tile.corner(badge(surround_corner(a), Badge::Accent));
    } else if fmt::truthy(a, "untagged") {
        tile = tile.corner(
            tooltip(badge("Untagged", Badge::Warn), widgets::tip_box("Some tracks lack MusicBrainz tags"), tooltip::Position::Top)
                .gap(4)
                .into(),
        );
    }
    tile.view()
}

/// An album tile in select mode (0.36): a tap picks or unpicks it (no play button), a check mark on the picked.
fn pick_tile<'a>(a: &Value, ctx: &Ctx<'a>, picked: bool) -> El<'a, Message> {
    let id = fmt::s(a, "id");
    let title = if fmt::truthy(a, "album") { tr(ctx, a, "album") } else { "Unknown album".to_string() };
    let mut sub = tr(ctx, a, "albumartist");
    if fmt::truthy(a, "year") {
        sub.push_str(&format!(" · {}", fmt::s(a, "year")));
    }
    let mut tile = Tile::new(title, TileKind::Album, Message::Pick(id.clone()))
        .sub(sub)
        .cover(ctx.cover(&id, &a["cover_v"], 400));
    if picked {
        tile = tile.corner(
            container(icon(Icon::Check, 16.0, ctx.look.on_accent))
                .padding(5)
                .style(theme::accent_box)
                .into(),
        );
    }
    tile.view()
}

/// The bar above the grid in select mode: "N selected", Select all N, Clear selection, Delete…, Done; while
/// the albums move, how far it is.
fn select_bar<'a>(state: &'a State) -> El<'a, Message> {
    if let Some(m) = &state.moving {
        return row![small_muted(lt::moving_text(m.done, m.n))].align_y(Alignment::Center).into();
    }
    let n = state.sel.len();
    let some = n > 0;
    row![
        badge(lt::selected_text(n), Badge::Accent),
        btn_sm(lt::select_all_text(state.total), Btn::Default, Some(Message::SelectAll)),
        btn_sm(lt::CLEAR_SELECTION, Btn::Default, some.then_some(Message::ClearSelection)),
        btn_icon(Icon::Trash, lt::DELETE, Btn::Danger, some.then_some(Message::DeleteSelected)),
        btn_sm(lt::DONE, Btn::Primary, Some(Message::SelectDone)),
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .wrap()
    .into()
}

/// One duplicate group: the header with its buttons and a row per copy with the keep radio.
fn dup_card<'a>(g: &'a Value, state: &State, p: theme::Palette) -> El<'a, Message> {
    let key = fmt::s(g, "key");
    let tracks = g["tracks"].as_array().cloned().unwrap_or_default();
    let first = tracks.first().cloned().unwrap_or(Value::Null);
    let keep = state.keep.get(&key).copied().unwrap_or_else(|| fmt::i(g, "keep"));
    let head = container(
        row![
            row![
                t(format!("{} – {}", fmt::s(&first, "artist"), fmt::s(&first, "title")))
                    .size(14)
                    .font(theme::bold()),
                badge(fmt::s(g, "reason"), Badge::Accent),
            ]
            .spacing(8)
            .align_y(Alignment::Center)
            .width(Length::Fill),
            btn_sm("Not duplicates", Btn::Ghost, Some(Message::DupIgnore(key.clone()))),
            btn_sm("Delete unchecked", Btn::Danger, Some(Message::DupResolve(key.clone()))),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    )
    .padding([12, 16]);
    let mut col = column![head, widgets::divider()];
    for c in tracks {
        let id = fmt::i(&c, "id");
        let k = key.clone();
        let mut r = row![
            radio("", id, Some(keep), move |v| Message::DupKeep(k.clone(), v))
                .size(16)
                .spacing(0),
            column![
                row![
                    t(fmt::s(&c, "title")).size(14),
                    widgets::muted(format!("· {}", fmt::s(&c, "album"))).size(13),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
                widgets::faint(fmt::s(&c, "relpath")).size(11.5),
            ]
            .spacing(2)
            .width(Length::Fill),
            badge(format_text(&c), Badge::Plain),
        ]
        .spacing(12)
        .align_y(Alignment::Center);
        if let Some(vb) = verify_badge(&c) {
            r = r.push(vb);
        }
        r = r.push(small_muted(format!(
            "{} · {}",
            fmt::mmss(fmt::f(&c, "duration")),
            size_text(fmt::f(&c, "size"))
        )));
        col = col.push(column![container(r).padding([9, 16]), widgets::divider()]);
    }
    let _ = p;
    widgets::card_flat(col).width(Length::Fill).into()
}

/// A modal or ⋯ menu to lay over the whole window, when one is open.
pub fn overlay<'a>(state: &'a State, ctx: &Ctx<'a>) -> Option<El<'a, Message>> {
    if let Some(o) = shared_overlay(&state.shared, ctx) {
        return Some(o.map(Message::Shared));
    }
    match &state.dialog {
        Some(LibDialog::Confirm {
            title,
            text,
            ok,
            danger,
            on_ok,
        }) => Some(confirm(title, text, ok, *danger, (**on_ok).clone(), Message::CloseDialog)),
        Some(LibDialog::Trash { title, text, can_go }) => {
            let footer = if *can_go {
                row![
                    btn("Cancel", Btn::Default, Some(Message::CloseDialog)),
                    btn(lt::MOVE_TO_TRASH, Btn::Danger, Some(Message::MoveToTrash)),
                ]
                .spacing(10)
            } else {
                row![btn("Close", Btn::Default, Some(Message::CloseDialog))]
            };
            Some(modal(title.as_str(), widgets::muted(text.as_str()).size(14), Some(footer.into()), false, Message::CloseDialog))
        }
        None => scrubber(state, ctx),
    }
}

/// The side scrubber (web-rs/src/scrubber.rs): section labels on the right edge of the list,
/// over the main area; a press loads up to that section and scrolls to it. Shown for a long list
/// with two sections or more; with more sections than fit, every Nth label (18 px each).
fn scrubber<'a>(state: &'a State, ctx: &Ctx<'a>) -> Option<El<'a, Message>> {
    let n = state.index.len();
    let rows = loaded_rows(state)
        .max(state.total)
        .max(state.index.last().map_or(0, |s| s.2 + 1));
    if n < 2 || rows < 40 {
        return None;
    }
    let (top, bottom) = (96.0, theme::PLAYER_H + 16.0);
    let h = (ctx.size.height - top - bottom).max(36.0);
    let step = ((n as f32 * SCRUB_LABEL_PX / h).ceil() as usize).max(1);
    let mut rail = column![].align_x(Alignment::Center);
    for (i, (label, short, offset)) in state.index.iter().enumerate() {
        if i % step != 0 && i + 1 != n {
            continue;
        }
        let b = button(t(short.as_str()).size(10.5).color(ctx.look.muted))
            .padding([1, 5])
            .height(SCRUB_LABEL_PX)
            .style(theme::btn(Btn::Ghost))
            .on_press(Message::Jump(*offset));
        rail = rail.push(tooltip(b, widgets::tip_box(label.as_str()), tooltip::Position::Left).gap(6));
    }
    Some(
        container(container(rail).padding([6, 2]).style(theme::panel2))
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(iced::Padding {
                top,
                right: 18.0,
                bottom,
                left: 0.0,
            })
            .align_x(Alignment::End)
            .center_y(Length::Fill)
            .into(),
    )
}

/// Pixels a scrubber label needs.
const SCRUB_LABEL_PX: f32 = 18.0;

pub fn subscription(state: &State) -> Subscription<Message> {
    let mut subs = Vec::new();
    if state.admin {
        subs.push(iced::time::every(Duration::from_millis(2500)).map(|_| Message::Poll));
    }
    // select mode (0.36): Esc leaves, Ctrl+A selects all, Delete is Delete… (not while a dialog or menu is open)
    if state.selecting && state.moving.is_none() && state.dialog.is_none() && state.shared.menu.is_none() && state.shared.dialog.is_none() {
        subs.push(iced::event::listen_with(select_key));
    }
    Subscription::batch(subs)
}

/// The keys of select mode (those no text box took).
fn select_key(e: iced::Event, status: iced::event::Status, _: iced::window::Id) -> Option<Message> {
    use iced::keyboard::{key::Named, Event as Kb, Key};
    let iced::Event::Keyboard(Kb::KeyPressed { key, modifiers, .. }) = e else {
        return None;
    };
    match key.as_ref() {
        Key::Named(Named::Escape) => Some(Message::SelectDone),
        _ if status == iced::event::Status::Captured => None,
        Key::Named(Named::Delete) => Some(Message::DeleteSelected),
        Key::Character(c) if modifiers.command() && c.eq_ignore_ascii_case("a") => Some(Message::SelectAll),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_search_for_the_whole_library() {
        let mut st = State { tab: "albums".into(), q: "crazy gb".into(), results: true, ..Default::default() };
        assert!(results_view(&st));
        // with a query the Albums tab lists the singles too, as the count does
        assert_eq!(album_kind(&st), "");
        st.tab = "singles".into();
        assert_eq!(album_kind(&st), "singles");
        st.tab = "albums".into();
        // a tab clicked shows that tab filtered; no query, no results view; the artist page keeps its tabs
        st.results = false;
        assert!(!results_view(&st));
        st.results = true;
        st.q = "  ".into();
        assert!(!results_view(&st));
        assert_eq!(album_kind(&st), "albums");
        st.q = "x".into();
        st.artist = "Beta Band".into();
        assert!(!results_view(&st));
        // "Show all N" from the server's count, else when a section came back full (an older server)
        assert_eq!(show_all_label(Some(230), 12).as_deref(), Some("Show all 230"));
        assert_eq!(show_all_label(Some(4), 4), None);
        assert_eq!(show_all_label(None, RESULTS_LIMIT).as_deref(), Some("Show all"));
        assert_eq!(show_all_label(None, 3), None);
    }

    #[test]
    fn scrubber_sections_come_from_the_index() {
        let v = json!([{ "label": "A", "offset": 0 }, { "label": "March 2019", "short": "3/19", "offset": 40 }]);
        assert_eq!(
            parse_index(&v),
            vec![("A".to_string(), "A".to_string(), 0), ("March 2019".to_string(), "3/19".to_string(), 40)]
        );
        assert!(parse_index(&json!({ "detail": "x" })).is_empty());
    }

    #[test]
    fn numbers_and_durations_read_like_the_web() {
        assert_eq!(num(0.0), "0");
        assert_eq!(num(999.0), "999");
        assert_eq!(num(1234.0), "1,234");
        assert_eq!(num(1_234_567.0), "1,234,567");
        assert_eq!(long_dur(7500.0), "2 hr 5 min");
        assert_eq!(long_dur(720.0), "12 min");
        assert_eq!(long_dur(0.0), "0 min");
        assert_eq!(long_dur(90_000.0), "1 day 1 hr");
        assert_eq!(size_text(0.0), "0 B");
        assert_eq!(size_text(1536.0), "2 KB");
        assert_eq!(size_text(12.9 * 1024.0 * 1024.0), "12.9 MB");
    }

    #[test]
    fn format_and_verify_text() {
        assert_eq!(format_text(&json!({ "format": "mp3", "bitrate": 320 })), "MP3 320k");
        assert_eq!(format_text(&json!({ "format": "flac" })), "FLAC");
        assert!(verify_badge::<()>(&json!({ "verify_status": "manual" })).is_some());
        assert!(verify_badge::<()>(&json!({ "verify_status": "" })).is_none());
    }

    #[test]
    fn sort_defaults_follow_the_web() {
        let mut s = State {
            tab: "albums".into(),
            ..Default::default()
        };
        apply_sort(&mut s);
        assert_eq!((s.sort.as_str(), s.dir.as_str()), ("added", "desc"));
        s.sort = "album".into();
        s.dir = "desc".into();
        save_sort(&mut s);
        s.tab = "artists".into();
        apply_sort(&mut s);
        assert_eq!((s.sort.as_str(), s.dir.as_str()), ("name", "asc"));
        s.tab = "albums".into();
        apply_sort(&mut s);
        assert_eq!((s.sort.as_str(), s.dir.as_str()), ("album", "desc"), "the saved choice comes back");
        let mut c = crate::config::Config::default();
        c.local.insert(SORT_KEY.into(), json!({ "songs": { "sort": "plays", "dir": "asc" } }));
        assert_eq!(saved_sorts(&c).get("songs"), Some(&("plays".to_string(), "asc".to_string())));
        assert!(desc_first("size") && !desc_first("title"));
        assert!(sort_opts("duplicates").is_none());
    }

    #[test]
    fn lyrics_lose_their_time_tags() {
        // the space after a tag stays, as in the web (`replace` then `trim`)
        assert_eq!(plain_lyrics("[00:01.00]Hello\n\n\n\n[00:02.00] World\n"), "Hello\n\n World");
        assert_eq!(plain_lyrics("plain [unclosed"), "plain [unclosed");
    }

    #[test]
    fn request_bodies_and_reasons() {
        let b = req_body(&json!({ "title": "T", "artist": "A", "cover": "c", "length": 3, "isrc": "" }));
        assert_eq!(b["kind"], "track");
        assert_eq!(b["cover_url"], "c");
        assert!(b["mbid"].is_null() && b.get("isrc").is_none());
        assert_eq!(reason_for(&["upsampled".to_string()]), "fake_lossless");
        assert_eq!(reason_for(&["quiet".to_string()]), "loudness");
        assert_eq!(reason_for(&[]), "bad_audio");
        assert_eq!(request_toast(&json!({ "status": "pending" })), "Request sent – waiting for approval");
    }

    #[test]
    fn shuffle_keeps_every_song_and_album_ids_are_added() {
        let rows: Vec<Value> = (0..20).map(|i| json!({ "id": i })).collect();
        let mut s = shuffled(&rows);
        s.sort_by_key(|v| fmt::i(v, "id"));
        assert_eq!(s, rows);
        let w = with_album_id(&rows[..2], "abc");
        assert_eq!(w[1]["album_id"], "abc");
        assert_eq!(safe_file_name("a/b: c?.mp3"), "a_b_ c_.mp3");
    }

    #[test]
    fn the_track_menu_depends_on_the_role() {
        let api = crate::api::Api::new("http://s");
        let config = crate::config::Config::default();
        let admin = crate::api::Session {
            user: json!({ "username": "a", "role": "admin", "perms": {} }),
            info: Value::Null,
        };
        let covers = widgets::Covers::new();
        let ctx = Ctx::new(
            api.clone(),
            &config,
            &admin,
            Default::default(),
            &covers,
            theme::look(),
            iced::Size::new(1000.0, 700.0),
            Point::ORIGIN,
        );
        let mut sh = Shared::default();
        let _ = track_menu(&mut sh, &json!({ "id": 1, "title": "x", "album_id": "al" }), &ctx);
        let labels: Vec<String> = sh.menu.as_ref().unwrap().1.iter().map(|m| m.label.clone()).collect();
        assert!(labels.contains(&"Delete song from disk".to_string()));
        assert!(labels.contains(&"Edit song details…".to_string()));
        assert!(labels.contains(&"Go to album".to_string()));
        let user = crate::api::Session {
            user: json!({ "username": "u", "role": "user", "perms": {} }),
            info: Value::Null,
        };
        let ctx = Ctx::new(
            api,
            &config,
            &user,
            Default::default(),
            &covers,
            theme::look(),
            iced::Size::new(1000.0, 700.0),
            Point::ORIGIN,
        );
        let _ = track_menu(&mut sh, &json!({ "id": 1, "title": "x" }), &ctx);
        let labels: Vec<String> = sh.menu.as_ref().unwrap().1.iter().map(|m| m.label.clone()).collect();
        assert!(!labels.iter().any(|l| l.contains("Delete")));
        assert!(!labels.contains(&"Go to album".to_string()));
    }
    fn key(key: iced::keyboard::Key, modifiers: iced::keyboard::Modifiers) -> iced::Event {
        iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
            modified_key: key.clone(),
            key,
            physical_key: iced::keyboard::key::Physical::Unidentified(iced::keyboard::key::NativeCode::Unidentified),
            location: iced::keyboard::Location::Standard,
            modifiers,
            text: None,
            repeat: false,
        })
    }

    /// Select mode (0.36): Esc leaves, Ctrl+A is Select all, Delete is Delete… – the last two only when no
    /// text box took the key.
    #[test]
    fn select_mode_keys() {
        use iced::event::Status;
        use iced::keyboard::{key::Named, Key, Modifiers};
        let id = iced::window::Id::unique();
        let name = |e: Option<Message>| e.map(|m| format!("{m:?}")).unwrap_or_default();
        assert_eq!(name(select_key(key(Key::Named(Named::Escape), Modifiers::empty()), Status::Ignored, id)), "SelectDone");
        assert_eq!(name(select_key(key(Key::Named(Named::Escape), Modifiers::empty()), Status::Captured, id)), "SelectDone");
        assert_eq!(name(select_key(key(Key::Named(Named::Delete), Modifiers::empty()), Status::Ignored, id)), "DeleteSelected");
        assert_eq!(name(select_key(key(Key::Character("a".into()), Modifiers::COMMAND), Status::Ignored, id)), "SelectAll");
        assert_eq!(name(select_key(key(Key::Character("A".into()), Modifiers::COMMAND), Status::Ignored, id)), "SelectAll");
        assert!(select_key(key(Key::Character("a".into()), Modifiers::COMMAND), Status::Captured, id).is_none(), "the search box's own Ctrl+A");
        assert!(select_key(key(Key::Character("a".into()), Modifiers::empty()), Status::Ignored, id).is_none());
        assert!(select_key(key(Key::Named(Named::Delete), Modifiers::empty()), Status::Captured, id).is_none());
    }

    /// The Type dropdown and Select (0.36): a Type decides the Albums tab alone, picking clears the selection,
    /// select mode is for admins on the album tabs only, and the Singles tab says what it lists.
    #[test]
    fn types_and_select_mode() {
        let mut st = State { tab: "albums".into(), sort: "added".into(), dir: "desc".into(), album_type: "live".into(), ..Default::default() };
        assert_eq!(albums_query(&st).page(0, PAGE), "/library/albums?q=&sort=added&dir=desc&type=live&filter=&artist=&limit=120&offset=0");
        st.tab = "singles".into();
        assert_eq!(albums_query(&st).page(0, PAGE), "/library/albums?q=&sort=added&dir=desc&kind=singles&filter=&artist=&limit=120&offset=0", "the Type is the Albums tab's");
        assert!(!can_select(&st), "members don't select");
        st.admin = true;
        assert!(can_select(&st));
        st.tab = "tracks".into();
        assert!(!can_select(&st));
        st.tab = "albums".into();
        st.selecting = true;
        st.sel.toggle("a1");
        st.sel.toggle("a2");
        clear_selection(&mut st);
        assert!(st.selecting && st.sel.is_empty(), "the mode stays, the picks go");
        st.sel.toggle("a1");
        leave_select(&mut st);
        assert!(!st.selecting && st.sel.is_empty());
        // the results view of a search can't select
        st.q = "x".into();
        st.results = true;
        assert!(!can_select(&st));
        assert_eq!(lt::SINGLES_TAB, "Singles & EPs");
    }

    #[test]
    fn surround_songs_and_albums_are_told_apart() {
        assert!(is_surround(&json!({ "id": "s0123456789abcde", "class": "surround" })));
        assert!(is_surround(&json!({ "id": 1_000_000_007 })));
        assert!(!is_surround(&json!({ "id": 42, "surround_id": 1_000_000_007 })));
        assert!(!is_surround(&json!({ "id": "0123456789abcdef" })));
        assert_eq!(surround_label("Dolby Atmos (E-AC-3 5.1)"), "Dolby Atmos");
        assert_eq!(surround_label("E-AC-3 5.1"), "E-AC-3 5.1");
        assert_eq!(surround_corner(&json!({ "atmos": true, "quality": "Dolby Atmos (E-AC-3 5.1)" })), "Dolby Atmos");
        assert_eq!(surround_corner(&json!({ "atmos": false, "quality": "AC-3 5.1" })), "AC-3 5.1");
        let keys: Vec<&str> = sort_opts("surround").unwrap().iter().map(|o| o.0).collect();
        assert_eq!(keys, ["added", "name", "artist", "year"]);
    }

    #[test]
    fn surround_songs_have_their_own_menu() {
        let api = crate::api::Api::new("http://s");
        let config = crate::config::Config::default();
        let ctx_for = |role: &str, features: Value| crate::api::Session {
            user: json!({ "username": "a", "role": role, "perms": {} }),
            info: json!({ "features": features }),
        };
        let covers = widgets::Covers::new();
        let labels = |session: &crate::api::Session, t: Value| -> Vec<String> {
            let ctx = Ctx::new(
                api.clone(),
                &config,
                session,
                Default::default(),
                &covers,
                theme::look(),
                iced::Size::new(1000.0, 700.0),
                Point::ORIGIN,
            );
            let mut sh = Shared::default();
            let _ = track_menu(&mut sh, &t, &ctx);
            sh.menu.unwrap().1.iter().map(|m| m.label.clone()).collect()
        };
        let admin = ctx_for("admin", json!(["surround"]));
        // a Surround song: no likes, playlists, editing or deleting; a way to its stereo album
        let l = labels(
            &admin,
            json!({ "id": 1_000_000_001, "class": "surround", "album_id": "s1", "stereo_album_id": "ab" }),
        );
        assert!(l.contains(&"Go to the stereo version".to_string()));
        assert!(l.contains(&"Play next".to_string()));
        for gone in ["Add to playlist", "Edit song details…", "Delete song from disk", "Share link"] {
            assert!(!l.contains(&gone.to_string()), "{gone}");
        }
        // a stereo song with a surround version plays it; without one an admin can fetch it
        let l = labels(&admin, json!({ "id": 5, "surround_id": 1_000_000_001 }));
        assert!(l.contains(&"Play the surround version".to_string()));
        assert!(!l.contains(&"Get Dolby Atmos version".to_string()));
        let l = labels(&admin, json!({ "id": 5 }));
        assert!(l.contains(&"Get Dolby Atmos version".to_string()));
        // not for members, nor on a server without the feature
        assert!(!labels(&ctx_for("user", json!(["surround"])), json!({ "id": 5 }))
            .contains(&"Get Dolby Atmos version".to_string()));
        assert!(!labels(&ctx_for("admin", json!([])), json!({ "id": 5 })).contains(&"Get Dolby Atmos version".to_string()));
    }
}
