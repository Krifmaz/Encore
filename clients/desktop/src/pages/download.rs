//! Downloader: Paste a link or search a source and pick what to download. Behaviour reference:
//! web-rs/src/pages/download.rs (and activity.rs for the admin's transfers, torrents and log
//! under it; playlists.rs `import_modal` for "Import as playlist").

use crate::api::ApiError;
use crate::fmt;
use crate::pages::{urlencode, Ctx, El, Page};
use crate::theme::{self, Badge, Btn};
use crate::widgets::catalog;
use crate::widgets::request_list::{self, List};
use crate::widgets::{
    self, badge, btn, btn_icon, card, card_flat, empty, error_box, h1, modal, muted, section_head, small_muted, tabs,
    tip_box, Icon,
};
use iced::widget::{column, container, pick_list, progress_bar, row, text_input, toggler, tooltip};
use iced::{Alignment, Length, Subscription, Task};
use serde_json::{json, Value};
use std::time::Duration;

/// Links that only describe tracks (no downloadable audio): resolved track by track.
const METADATA_ONLY: [&str; 6] = [
    "open.spotify.com",
    "spotify.link",
    "music.apple.com",
    "music.amazon.",
    "amzn.to/",
    "listenbrainz.org",
];
/// Links streamrip rips as the exact release.
const STREAMRIP: [&str; 4] = ["qobuz.com", "tidal.com", "deezer.com", "deezer.page.link"];

/// The web's `METADATA_ONLY` test (`amazon.<tld>/music/player/` included).
pub fn metadata_only(url: &str) -> bool {
    let s = url.to_lowercase();
    if s.starts_with("spotify:") || METADATA_ONLY.iter().any(|m| s.contains(m)) {
        return true;
    }
    // amazon\.[a-z.]+/music/player/
    s.match_indices("amazon.").any(|(i, _)| {
        let rest = &s[i + "amazon.".len()..];
        let tld_len = rest.chars().take_while(|c| c.is_ascii_lowercase() || *c == '.').count();
        rest[tld_len..].starts_with("/music/player/")
    })
}

pub fn streamrip(url: &str) -> bool {
    let s = url.to_lowercase();
    STREAMRIP.iter().any(|m| s.contains(m))
}

/// The album-mode choice: `(key, label)`.
const ALBUM_MODES: [(&str, &str); 3] = [("song", "Just the song"), ("album", "Get the whole album"), ("", "Admin default")];

/// Preview of a link (`GET /api/discover/probe`).
#[derive(Debug, Default)]
enum Probe {
    #[default]
    None,
    Loading,
    Ok(Box<Value>),
    Err(String),
}

/// The "Import as playlist" dialog (the web's import modal, opened with the link and
/// "Download missing tracks" on).
#[derive(Debug)]
struct Import {
    url: String,
    name: String,
    follow: bool,
    request_missing: bool,
    album_mode: String,
    preview: Option<Result<Box<Value>, String>>,
    previewing: bool,
    importing: bool,
}

/// The import dialog's album choice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mode(&'static str, &'static str);

impl std::fmt::Display for Mode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.1)
    }
}

static IMPORT_MODES: [Mode; 3] = [
    Mode("", "Albums: admin default"),
    Mode("song", "Just the songs"),
    Mode("album", "Whole albums"),
];

/// The log filter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Level(&'static str, &'static str);

impl std::fmt::Display for Level {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.1)
    }
}

static LEVELS: [Level; 4] = [
    Level("", "All levels"),
    Level("info", "Info"),
    Level("warning", "Warnings"),
    Level("error", "Errors"),
];

/// The admin's part (the web's Activity page): transfers, torrents and the log.
#[derive(Debug, Default)]
struct Activity {
    /// `/api/system/downloads`; None until it answered.
    downloads: Option<Result<Box<Value>, String>>,
    /// `/api/system/activity`.
    log: Option<Vec<Value>>,
    level: String,
    clearing: bool,
}

#[derive(Debug, Default)]
pub struct State {
    url: String,
    /// "song" | "album" | "" (admin default); the web's `encore.albumMode`, kept in the config.
    album_mode: String,
    /// "Download" is looking the link up.
    busy: bool,
    /// "Exactly this link" is being sent.
    direct_busy: bool,
    probe: Probe,
    list: List,
    import: Option<Import>,
    admin: bool,
    activity: Activity,
}

#[derive(Clone, Debug)]
pub enum Message {
    Url(String),
    AlbumMode(String),
    /// The Download button (`POST /api/requests/link`).
    Download,
    Downloaded(Result<Value, ApiError>),
    /// "Exactly this link": a plain url request.
    Direct,
    DirectDone(Result<Value, ApiError>),
    /// Preview.
    Probe,
    Probed(Result<Value, ApiError>),
    ImportAsPlaylist,
    ImpName(String),
    ImpFollow(bool),
    ImpRequest(bool),
    ImpAlbum(Mode),
    ImpPreview,
    ImpPreviewed(Result<Value, ApiError>),
    ImpGo,
    ImpDone(Result<Value, ApiError>),
    CloseDialog,
    List(request_list::Msg),
    Tick,
    ActivityTick,
    Downloads(Result<Value, ApiError>),
    Log(Result<Value, ApiError>),
    Level(Level),
    ClearSlskd,
    ClearedSlskd(Result<Value, ApiError>),
    /// A line in full in the details dialog, with Copy: `(title, text)`.
    ShowText(String, String),
}

/// The 30 most recent url / track / album requests of this user.
fn keep(r: &Value) -> bool {
    matches!(r["kind"].as_str(), Some("url" | "track" | "album"))
}

fn load_activity(ctx: &Ctx) -> Task<Message> {
    let a = ctx.api.clone();
    let b = ctx.api.clone();
    Task::batch([
        Task::perform(async move { a.get("/system/downloads").await }, Message::Downloads),
        Task::perform(async move { b.get("/system/activity").await }, Message::Log),
    ])
}

/// The web's `encore.albumMode` key.
const ALBUM_MODE: &str = "encore.albumMode";

/// Called when the app navigates here.
pub fn open(state: &mut State, ctx: &mut Ctx) -> Task<Message> {
    state.album_mode = ctx
        .config
        .local(ALBUM_MODE)
        .and_then(Value::as_str)
        .unwrap_or("song")
        .to_string();
    state.admin = ctx.is_admin();
    state.busy = false;
    state.direct_busy = false;
    state.probe = Probe::None;
    state.import = None;
    state.list.reset();
    state.list.query = "mine=true&limit=100".into();
    state.list.keep = Some(keep);
    state.list.take = 30;
    state.list.admin = false;
    state.list.mine = true;
    state.activity = Activity::default();
    let mut tasks = vec![state.list.load(ctx).map(Message::List)];
    if state.admin {
        tasks.push(load_activity(ctx));
    }
    Task::batch(tasks)
}

/// The import dialog with the Downloader's link, "Download missing tracks" on and the album
/// mode of the page; the preview starts at once.
fn open_import(state: &mut State, ctx: &Ctx) -> Task<Message> {
    let url = state.url.trim().to_string();
    if url.is_empty() {
        return Task::none();
    }
    state.import = Some(Import {
        url,
        name: String::new(),
        follow: false,
        request_missing: true,
        album_mode: state.album_mode.clone(),
        preview: None,
        previewing: false,
        importing: false,
    });
    preview_import(state, ctx)
}

fn preview_import(state: &mut State, ctx: &Ctx) -> Task<Message> {
    let Some(imp) = state.import.as_mut() else {
        return Task::none();
    };
    imp.previewing = true;
    imp.preview = None;
    let body = json!({ "url": imp.url.trim() });
    let api = ctx.api.clone();
    Task::perform(async move { api.post("/playlists/preview", body).await }, Message::ImpPreviewed)
}

/// The web's `src_label(p)`: where the playlist comes from.
fn src_label(p: &Value) -> String {
    let s = fmt::s(p, "source");
    match s.as_str() {
        "ytmusic" => "YouTube Music".into(),
        "spotify" => "Spotify".into(),
        "deezer" => "Deezer".into(),
        "tidal" => "Tidal".into(),
        "apple" => "Apple Music".into(),
        "amazon" => "Amazon Music".into(),
        "qobuz" => "Qobuz".into(),
        "listenbrainz" => "ListenBrainz".into(),
        "text" => "pasted list".into(),
        "" => "link".into(),
        other => other.to_string(),
    }
}

pub fn update(state: &mut State, msg: Message, ctx: &mut Ctx) -> Task<Message> {
    match msg {
        Message::Url(u) => state.url = u,
        Message::AlbumMode(m) => {
            state.album_mode = m.clone();
            ctx.edit_config(move |c| {
                c.local.insert(ALBUM_MODE.into(), Value::String(m));
            });
        }
        Message::Download => {
            let url = state.url.trim().to_string();
            if url.is_empty() || state.busy {
                return Task::none();
            }
            state.busy = true;
            let mode = if state.album_mode.is_empty() {
                Value::Null
            } else {
                Value::String(state.album_mode.clone())
            };
            let api = ctx.api.clone();
            return Task::perform(
                async move { api.post("/requests/link", json!({ "url": url, "album_mode": mode })).await },
                Message::Downloaded,
            );
        }
        Message::Downloaded(r) => {
            state.busy = false;
            match r {
                Ok(r) => {
                    ctx.toast_ok(if fmt::s(&r, "mode") == "direct" {
                        "Downloading".to_string()
                    } else {
                        format!(
                            "{} song(s) requested{}{}",
                            fmt::s(&r, "requested"),
                            if fmt::truthy(&r, "already_requested") {
                                format!(", {} already requested", fmt::s(&r, "already_requested"))
                            } else {
                                String::new()
                            },
                            if state.album_mode == "album" { " – whole albums will be fetched" } else { "" }
                        )
                    });
                    state.url.clear();
                    state.probe = Probe::None;
                    return state.list.load(ctx).map(Message::List);
                }
                Err(e) => ctx.toast_err(e.message),
            }
        }
        Message::Direct => {
            let url = state.url.trim().to_string();
            if url.is_empty() || state.direct_busy {
                return Task::none();
            }
            if metadata_only(&url) {
                return open_import(state, ctx);
            }
            state.direct_busy = true;
            let title = match &state.probe {
                Probe::Ok(p) if fmt::truthy(p, "title") => p["title"].clone(),
                _ => Value::Null,
            };
            let api = ctx.api.clone();
            return Task::perform(
                async move {
                    api.post("/requests", json!({ "kind": "url", "url": url, "title": title }))
                        .await
                },
                Message::DirectDone,
            );
        }
        Message::DirectDone(r) => {
            state.direct_busy = false;
            match r {
                Ok(r) => catalog::request_toast(ctx, &r),
                Err(e) => ctx.toast_err(e.message),
            }
            state.url.clear();
            state.probe = Probe::None;
            return state.list.load(ctx).map(Message::List);
        }
        Message::Probe => {
            let url = state.url.trim().to_string();
            if url.is_empty() {
                return Task::none();
            }
            if metadata_only(&url) || streamrip(&url) {
                return open_import(state, ctx);
            }
            state.probe = Probe::Loading;
            let api = ctx.api.clone();
            let path = format!("/discover/probe?url={}", urlencode(&url));
            return Task::perform(async move { api.get(&path).await }, Message::Probed);
        }
        Message::Probed(r) => {
            state.probe = match r {
                Ok(p) => Probe::Ok(Box::new(p)),
                Err(e) => Probe::Err(e.message),
            }
        }
        Message::ImportAsPlaylist => return open_import(state, ctx),
        Message::ImpName(n) => {
            if let Some(i) = state.import.as_mut() {
                i.name = n;
            }
        }
        Message::ImpFollow(on) => {
            if let Some(i) = state.import.as_mut() {
                i.follow = on;
            }
        }
        Message::ImpRequest(on) => {
            if let Some(i) = state.import.as_mut() {
                i.request_missing = on;
            }
        }
        Message::ImpAlbum(m) => {
            if let Some(i) = state.import.as_mut() {
                i.album_mode = m.0.to_string();
            }
        }
        Message::ImpPreview => return preview_import(state, ctx),
        Message::ImpPreviewed(r) => {
            if let Some(i) = state.import.as_mut() {
                i.previewing = false;
                match r {
                    Ok(p) => {
                        if i.name.is_empty() {
                            i.name = fmt::s(&p, "name");
                        }
                        i.preview = Some(Ok(Box::new(p)));
                    }
                    Err(e) => i.preview = Some(Err(e.message)),
                }
            }
        }
        Message::ImpGo => {
            let Some(i) = state.import.as_mut() else {
                return Task::none();
            };
            if i.importing {
                return Task::none();
            }
            i.importing = true;
            let name = i.name.trim();
            let body = json!({
                "url": i.url.trim(),
                "into": Value::Null,
                "name": if name.is_empty() { Value::Null } else { Value::String(name.to_string()) },
                "follow": i.follow,
                "request_missing": i.request_missing,
                "album_mode": if i.album_mode.is_empty() { Value::Null } else { Value::String(i.album_mode.clone()) },
            });
            let api = ctx.api.clone();
            return Task::perform(async move { api.post("/playlists/import", body).await }, Message::ImpDone);
        }
        Message::ImpDone(r) => match r {
            Ok(r) => {
                ctx.toast_ok(format!(
                    "Imported {} track(s){}{}",
                    fmt::s(&r, "added"),
                    if fmt::truthy(&r, "duplicates_skipped") {
                        format!(", skipped {} duplicate(s)", fmt::s(&r, "duplicates_skipped"))
                    } else {
                        String::new()
                    },
                    if fmt::truthy(&r, "requested") {
                        format!(", requested {}", fmt::s(&r, "requested"))
                    } else {
                        String::new()
                    }
                ));
                state.import = None;
                state.url.clear();
                state.probe = Probe::None;
                ctx.navigate(Page::Playlist(fmt::i(&r, "id")));
            }
            Err(e) => {
                ctx.toast_err(e.message);
                if let Some(i) = state.import.as_mut() {
                    i.importing = false;
                }
            }
        },
        Message::CloseDialog => state.import = None,
        Message::List(m) => return request_list::update(&mut state.list, m, ctx).map(Message::List),
        Message::Tick => return state.list.load(ctx).map(Message::List),
        Message::ActivityTick => {
            if state.admin {
                return load_activity(ctx);
            }
        }
        Message::Downloads(r) => {
            state.activity.downloads = Some(match r {
                Ok(d) => Ok(Box::new(d)),
                Err(e) => Err(e.message),
            })
        }
        Message::Log(Ok(v)) => state.activity.log = Some(fmt::items(&v)),
        Message::Log(Err(e)) => {
            if state.activity.log.is_none() {
                log::warn!("activity log: {}", e.message);
            }
        }
        Message::Level(l) => state.activity.level = l.0.to_string(),
        Message::ShowText(title, text) => ctx.details(title, text),
        Message::ClearSlskd => {
            if state.activity.clearing {
                return Task::none();
            }
            state.activity.clearing = true;
            let api = ctx.api.clone();
            return Task::perform(
                async move { api.post_empty("/system/downloads/slskd/clear").await },
                Message::ClearedSlskd,
            );
        }
        Message::ClearedSlskd(r) => {
            state.activity.clearing = false;
            match r {
                Ok(_) => return load_activity(ctx),
                Err(e) => ctx.toast_err(e.message),
            }
        }
    }
    Task::none()
}

// ----- view -------------------------------------------------------------------------------

fn probe_view<'a>(state: &'a State, ctx: &Ctx<'a>) -> Option<El<'a, Message>> {
    match &state.probe {
        Probe::None => None,
        Probe::Loading => Some(small_muted("Looking the link up…").into()),
        Probe::Err(e) => Some(error_box(e)),
        Probe::Ok(p) => {
            let title = fmt::s(p, "title");
            let sub = [
                fmt::s(p, "extractor"),
                fmt::s(p, "uploader"),
                if fmt::truthy(p, "is_playlist") {
                    format!("{} items", fmt::s(p, "count"))
                } else if fmt::truthy(p, "duration") {
                    fmt::mmss(fmt::f(p, "duration"))
                } else {
                    String::new()
                },
            ]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" · ");
            Some(
                row![
                    catalog::art(ctx, &fmt::s(p, "thumbnail"), 64.0, false, Icon::Music),
                    column![
                        widgets::t(if title.is_empty() { "Untitled".to_string() } else { title })
                            .size(14)
                            .font(theme::semibold()),
                        small_muted(sub),
                    ]
                    .spacing(3)
                    .width(Length::Fill),
                ]
                .spacing(12)
                .align_y(Alignment::Center)
                .into(),
            )
        }
    }
}

pub fn view<'a>(state: &'a State, ctx: &Ctx<'a>) -> El<'a, Message> {
    let url = state.url.trim();
    let has_url = !url.is_empty();
    let meta = metadata_only(url);
    let direct_tip = if meta {
        "This service has no downloadable audio – use “Import as playlist”"
    } else if streamrip(url) {
        "Rip this exact release with streamrip"
    } else {
        "Skip matching and download exactly this link"
    };
    let input = row![
        text_input("https://…", &state.url)
            .on_input(Message::Url)
            .on_submit(Message::Download)
            .padding([9, 12])
            .size(14)
            .style(theme::input),
        btn("Preview", Btn::Default, has_url.then_some(Message::Probe)),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    let mode = row![
        small_muted("If a song is on an album:"),
        tabs(&ALBUM_MODES, &state.album_mode, Message::AlbumMode),
    ]
    .spacing(10)
    .align_y(Alignment::Center)
    .wrap();
    let download_label = if state.busy { "Looking up…" } else { "Download" };
    let actions = row![
        tooltip(
            btn_icon(
                Icon::Download,
                download_label,
                Btn::Primary,
                (has_url && !state.busy).then_some(Message::Download)
            ),
            tip_box("Works with any link: matched to the best source, or downloaded directly"),
            tooltip::Position::Bottom,
        )
        .gap(4),
        btn_icon(Icon::List, "Import as playlist", Btn::Default, has_url.then_some(Message::ImportAsPlaylist)),
        tooltip(
            btn_icon(
                Icon::Link,
                "Exactly this link",
                Btn::Ghost,
                (has_url && !meta && !state.direct_busy).then_some(Message::Direct)
            ),
            tip_box(direct_tip),
            tooltip::Position::Bottom,
        )
        .gap(4),
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .wrap();
    let mut form = column![input, mode, actions].spacing(14).width(Length::Fill);
    if let Some(p) = probe_view(state, ctx) {
        form = form.push(p);
    }
    // Retry failed for those who may retry (the server answers others "Ask an admin to retry")
    let mut recent_actions = row![].spacing(8).align_y(Alignment::Center);
    if request_list::may_fix(ctx.is_admin(), ctx.session.perm("fix")) {
        recent_actions = recent_actions
            .push(request_list::retry_failed_btn(&state.list, "Queue every failed download again").map(Message::List));
    }
    let recent_actions: El<'a, Message> = recent_actions.push(request_list::clear_btn("Clear…").map(Message::List)).into();
    let list = match request_list::view(ctx, &state.list) {
        Some(rows) => rows.map(Message::List),
        None => container(muted("Nothing yet – paste a link above.").size(13))
            .padding([24, 16])
            .width(Length::Fill)
            .center_x(Length::Fill)
            .into(),
    };
    let banner = request_list::storage_banner(ctx, &state.list).map(|b| b.map(Message::List));
    let mut page = column![
        column![
            h1("Downloader"),
            muted(
                "Paste a link from anywhere. Spotify, Apple Music, YouTube Music, Deezer, Tidal, Qobuz, Amazon Music songs, albums and playlists are matched song by song (exact ISRC when available) and fetched from the best source you have – lossless first. YouTube, SoundCloud, Bandcamp, Mixcloud and ~1,800 other sites download directly. Every file is fingerprint-verified and de-duplicated before it hits the library."
            )
            .size(14),
        ]
        .spacing(6),
        card(form).width(Length::Fill),
    ]
    .spacing(22)
    .width(Length::Fill);
    // imports paused: said above the downloads it holds back
    let mut recent = column![section_head("Recent downloads", Some(recent_actions))].spacing(10);
    if let Some(b) = banner {
        recent = recent.push(b);
    }
    page = page.push(recent.push(card_flat(list).width(Length::Fill)));
    if state.admin {
        page = page.push(activity_view(state, ctx));
    }
    page.into()
}

/// The web's Activity page, embedded for admins: transfers, torrents and the log.
fn activity_view<'a>(state: &'a State, ctx: &Ctx<'a>) -> El<'a, Message> {
    let a = &state.activity;
    let slskd: El<'a, Message> = match &a.downloads {
        None => container(widgets::skeleton_rows(3)).padding(16).into(),
        Some(Err(e)) => container(error_box(e)).padding(14).into(),
        Some(Ok(d)) => {
            if fmt::truthy(&d["errors"], "slskd") {
                container(error_box(d["errors"]["slskd"].as_str().unwrap_or("")))
                    .padding(14)
                    .into()
            } else {
                let files = fmt::items(&d["slskd"]);
                if files.is_empty() {
                    empty(Icon::Download, "No Soulseek transfers", "slskd may not be configured.")
                } else {
                    let mut col = column![];
                    for f in files.iter().take(200) {
                        col = col.push(slskd_row(f, ctx));
                    }
                    col.into()
                }
            }
        }
    };
    let torrents: El<'a, Message> = match &a.downloads {
        Some(Ok(d)) => {
            let tor = fmt::items(&d["torrents"]);
            if tor.is_empty() {
                empty(Icon::Download, "Nothing downloading from torrents or Usenet", "")
            } else {
                let mut col = column![];
                for q in &tor {
                    let mut lines = column![widgets::t(format!("{} – {}", fmt::s(q, "artist"), fmt::s(q, "title")))
                        .size(14)
                        .font(theme::semibold())]
                    .spacing(4);
                    if fmt::truthy(q, "message") {
                        // a click opens the whole message with Copy
                        lines = lines.push(
                            iced::widget::mouse_area(small_muted(fmt::s(q, "message")))
                                .on_press(Message::ShowText(
                                    format!("{} – {}", fmt::s(q, "artist"), fmt::s(q, "title")),
                                    fmt::s(q, "message"),
                                ))
                                .interaction(iced::mouse::Interaction::Pointer),
                        );
                    }
                    lines = lines.push(
                        container(
                            progress_bar(0.0..=100.0, fmt::f(q, "progress") as f32)
                                .girth(5)
                                .style(theme::progress),
                        )
                        .max_width(420),
                    );
                    col = col.push(column![container(lines).padding([11, 16]), widgets::divider()]);
                }
                col.into()
            }
        }
        _ => container(widgets::skeleton_rows(1)).padding(16).into(),
    };
    let level = LEVELS.iter().find(|l| l.0 == a.level).cloned();
    let level_pick = pick_list(&LEVELS[..], level, Message::Level)
        .text_size(13)
        .padding([7, 10])
        .style(theme::pick)
        .menu_style(theme::pick_menu);
    let log: El<'a, Message> = match &a.log {
        None => container(widgets::skeleton_rows(3)).padding(16).into(),
        Some(rows) => {
            let shown: Vec<&Value> = rows
                .iter()
                .filter(|l| a.level.is_empty() || l["level"].as_str() == Some(a.level.as_str()))
                .take(200)
                .collect();
            if shown.is_empty() {
                container(muted("No log entries").size(13))
                    .padding([24, 16])
                    .width(Length::Fill)
                    .center_x(Length::Fill)
                    .into()
            } else {
                let mut col = column![];
                for l in shown {
                    let level = fmt::s(l, "level");
                    let kind = match level.as_str() {
                        "error" => Badge::Err,
                        "warning" => Badge::Warn,
                        _ => Badge::Plain,
                    };
                    let ts = fmt::f(l, "ts");
                    // a click opens the line in full with Copy
                    let full = format!("{} · {}\n{}", level, fmt::stamp(ts), fmt::s(l, "message"));
                    col = col.push(column![
                        iced::widget::mouse_area(container(
                            row![
                                container(badge(level, kind)).width(78),
                                widgets::t(fmt::s(l, "message")).size(12.5).width(Length::Fill),
                                tooltip(
                                    widgets::faint(fmt::ago(ts)).size(12),
                                    container(widgets::t(fmt::date(ts)).size(12))
                                        .padding([5, 9])
                                        .style(theme::menu_pop),
                                    tooltip::Position::Left,
                                )
                                .gap(4),
                            ]
                            .spacing(12)
                            .align_y(Alignment::Center)
                        )
                        .padding([8, 16]))
                        .on_press(Message::ShowText("Log entry".into(), full))
                        .interaction(iced::mouse::Interaction::Pointer),
                        widgets::divider(),
                    ]);
                }
                col.into()
            }
        }
    };
    column![
        section_head(
            "Soulseek transfers",
            Some(
                btn_icon(
                    Icon::Trash,
                    "Clear completed",
                    Btn::Default,
                    (!a.clearing).then_some(Message::ClearSlskd)
                )
                .into()
            ),
        ),
        card_flat(slskd).width(Length::Fill),
        section_head("Torrents & Usenet", None),
        card_flat(torrents).width(Length::Fill),
        section_head("Log", Some(level_pick.into())),
        card_flat(log).width(Length::Fill),
    ]
    .spacing(12)
    .width(Length::Fill)
    .into()
}

/// One Soulseek transfer of the admin's table.
fn slskd_row<'a>(f: &Value, _ctx: &Ctx<'a>) -> El<'a, Message> {
    let state = fmt::s(f, "state");
    let kind = if state.contains("Succeeded") {
        Badge::Ok
    } else if ["Errored", "Rejected", "TimedOut", "Cancelled"].iter().any(|k| state.contains(k)) {
        Badge::Err
    } else {
        Badge::Info
    };
    let speed = if fmt::truthy(f, "speed") {
        format!(" · {}/s", fmt::size(fmt::f(f, "speed")))
    } else {
        String::new()
    };
    let pct = fmt::f(f, "progress");
    column![
        container(
            row![
                column![
                    widgets::t(fmt::s(f, "filename"))
                        .size(13)
                        .font(theme::semibold())
                        .wrapping(iced::widget::text::Wrapping::WordOrGlyph),
                    widgets::faint(format!("{} · {}", fmt::s(f, "directory"), fmt::size(fmt::f(f, "size"))))
                        .size(12)
                        .wrapping(iced::widget::text::Wrapping::WordOrGlyph),
                ]
                .spacing(2)
                .width(Length::Fill),
                small_muted(fmt::s(f, "username")).width(120),
                badge(state.clone(), kind),
                column![
                    progress_bar(0.0..=100.0, pct as f32).girth(5).style(theme::progress).length(160),
                    widgets::faint(format!("{}%{speed}", pct.round() as i64)).size(12),
                ]
                .spacing(4)
                .width(180),
            ]
            .spacing(14)
            .align_y(Alignment::Center)
        )
        .padding([9, 16]),
        widgets::divider(),
    ]
    .into()
}

/// The import dialog.
fn import_view<'a>(imp: &'a Import, _ctx: &Ctx<'a>) -> El<'a, Message> {
    let mode = IMPORT_MODES.iter().find(|m| m.0 == imp.album_mode).cloned();
    let mut body = column![
        column![
            widgets::t(imp.url.as_str()).size(13).wrapping(iced::widget::text::Wrapping::WordOrGlyph),
            widgets::faint(
                "Connect Spotify/Tidal on the Connections page to import private playlists and Liked Songs. Amazon Music playlists and albums import from their public share links."
            )
            .size(12),
        ]
        .spacing(6),
        row![
            container(
                text_input("Playlist name (optional)", &imp.name)
                    .on_input(Message::ImpName)
                    .padding([8, 12])
                    .size(14)
                    .style(theme::input)
            )
            .max_width(280),
            switch("Keep following (pull new tracks)", imp.follow, Message::ImpFollow),
            switch("Download missing tracks", imp.request_missing, Message::ImpRequest),
            tooltip(
                pick_list(&IMPORT_MODES[..], mode, Message::ImpAlbum)
                    .text_size(13)
                    .padding([7, 10])
                    .style(theme::pick)
                    .menu_style(theme::pick_menu),
                tip_box("When a song is part of an album"),
                tooltip::Position::Bottom,
            )
            .gap(4),
            btn_icon(
                Icon::Search,
                "Preview",
                Btn::Default,
                (!imp.previewing).then_some(Message::ImpPreview)
            ),
        ]
        .spacing(10)
        .align_y(Alignment::Center)
        .wrap(),
    ]
    .spacing(14)
    .width(Length::Fill);
    if imp.previewing {
        body = body.push(widgets::skeleton_rows(3));
    }
    match &imp.preview {
        Some(Err(e)) => body = body.push(error_box(e)),
        Some(Ok(pv)) => {
            let mut head = row![
                widgets::t(fmt::s(pv, "name")).size(14).font(theme::semibold()),
                badge(src_label(pv), Badge::Plain),
                badge(format!("{} tracks", fmt::s(pv, "count")), Badge::Plain),
                badge(format!("{} already in library", fmt::s(pv, "in_library")), Badge::Ok),
            ]
            .spacing(8)
            .align_y(Alignment::Center);
            if fmt::truthy(pv, "duplicates") {
                head = head.push(badge(
                    format!("{} duplicate(s) will be skipped", fmt::s(pv, "duplicates")),
                    Badge::Warn,
                ));
            }
            body = body.push(head.wrap());
            if fmt::truthy(pv, "warning") {
                body = body.push(error_box(pv["warning"].as_str().unwrap_or("")));
            }
            let mut list = column![];
            for t in fmt::items(&pv["tracks"]) {
                let mut words = format!("{} – {}", fmt::s(&t, "artist"), fmt::s(&t, "title"));
                if fmt::truthy(&t, "album") {
                    words.push_str(&format!(" · {}", fmt::s(&t, "album")));
                }
                let mut r = row![widgets::t(words).size(13).width(Length::Fill)]
                    .spacing(8)
                    .align_y(Alignment::Center);
                if fmt::truthy(&t, "isrc") {
                    r = r.push(badge("ISRC", Badge::Plain));
                }
                r = r.push(if fmt::truthy(&t, "track_id") {
                    badge("in library", Badge::Ok)
                } else {
                    badge("missing", Badge::Plain)
                });
                list = list.push(column![container(r).padding([7, 14]), widgets::divider()]);
            }
            body = body.push(card_flat(list).width(Length::Fill));
        }
        None => {}
    }
    let footer = row![
        btn("Cancel", Btn::Default, Some(Message::CloseDialog)),
        btn_icon(
            Icon::Download,
            if imp.importing { "Importing…" } else { "Import" },
            Btn::Primary,
            (!imp.importing).then_some(Message::ImpGo)
        ),
    ]
    .spacing(10);
    modal("Import playlist", body, Some(footer.into()), true, Message::CloseDialog)
}

/// A modal or ⋯ menu to lay over the whole window, when one is open.
pub fn overlay<'a>(state: &'a State, ctx: &Ctx<'a>) -> Option<El<'a, Message>> {
    if let Some(imp) = &state.import {
        return Some(import_view(imp, ctx));
    }
    request_list::overlay(&state.list, ctx).map(|e| e.map(Message::List))
}

pub fn subscription(state: &State) -> Subscription<Message> {
    let rows = iced::time::every(Duration::from_secs(4)).map(|_| Message::Tick);
    if state.admin {
        Subscription::batch([
            rows,
            iced::time::every(Duration::from_secs(5)).map(|_| Message::ActivityTick),
        ])
    } else {
        rows
    }
}

/// `.switch`: a toggler with its words.
fn switch<'a>(label: &'a str, on: bool, msg: fn(bool) -> Message) -> El<'a, Message> {
    toggler(on)
        .label(label)
        .text_size(13)
        .spacing(8)
        .on_toggle(msg)
        .style(theme::switch)
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_kinds_follow_the_web() {
        assert!(metadata_only("https://open.spotify.com/album/x"));
        assert!(metadata_only("spotify:track:abc"));
        assert!(metadata_only("https://music.apple.com/us/album/x"));
        assert!(metadata_only("https://music.amazon.de/albums/B0"));
        assert!(metadata_only("https://amazon.co.uk/music/player/albums/B0"));
        assert!(!metadata_only("https://amazon.com/dp/B0"));
        assert!(!metadata_only("https://music.youtube.com/watch?v=x"));
        assert!(streamrip("https://tidal.com/browse/album/1"));
        assert!(streamrip("https://deezer.page.link/abc"));
        assert!(!streamrip("https://bandcamp.com/x"));
        assert!(keep(&json!({ "kind": "url" })) && !keep(&json!({ "kind": "artist" })));
        assert_eq!(src_label(&json!({ "source": "ytmusic" })), "YouTube Music");
        assert_eq!(ALBUM_MODES[2].0, "");
    }
}
