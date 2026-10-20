//! Playlist: One playlist: its songs, sync and download missing. Behaviour reference: web-rs/src/pages/playlists.rs render_playlist.
//!
//! Liked Songs is the playlist the server marks `kind: liked` (no rename, no delete, the heart
//! as its cover). "Download N missing" asks for the album choice first and toasts the counts
//! (`missing_report_text`).
//!
//! Share makes the playlist's public link (`POST /api/playlists/{id}/share`) and copies it; the
//! link always shows the playlist as it is now, Liked Songs too. While shared, a line under the
//! progress bar has Copy link and Stop sharing (`DELETE /api/playlists/{id}/share`).

use crate::api::ApiError;
use crate::fmt;
use crate::pages::playlists::{self, src_label, src_name, target_name, Import, ImportMsg};
use crate::pages::{Ctx, El, Page, Section};
use crate::theme::{self, Badge, Btn};
use crate::widgets::catalog;
use crate::widgets::{
    self, badge, btn, btn_icon, btn_sm, card, card_flat, confirm, empty, error_box, h1, icon_btn, modal, muted,
    pop_menu, skeleton_rows, small, small_muted, t, Icon, MenuItem, TrackRow,
};
use iced::widget::{column, container, progress_bar, row, text_input, toggler, tooltip};
use iced::{Alignment, Length, Point, Subscription, Task};
use serde_json::{json, Value};

/// A row "Download missing" would ask for: not in the library, and never requested or its
/// request is gone, failed, cancelled or denied. Rows whose request is waiting, running or done
/// are on their way already.
pub fn requestable(it: &Value) -> bool {
    !fmt::truthy(it, "track_id")
        && matches!(
            it["request_status"].as_str(),
            None | Some("failed") | Some("cancelled") | Some("denied")
        )
}

/// The "Download missing" toast from the route's `{requested, in_progress, failed: [{title, reason}]}`:
/// "Requested 120 · 600 already in progress · 64 couldn't be requested (why)".
pub fn missing_report_text(r: &Value) -> String {
    let mut s = format!("Requested {}", fmt::i(r, "requested"));
    if fmt::i(r, "in_progress") > 0 {
        s += &format!(" · {} already in progress", fmt::i(r, "in_progress"));
    }
    let failed = fmt::items(&r["failed"]);
    if !failed.is_empty() {
        let mut why: Vec<String> = Vec::new();
        for f in &failed {
            let reason = fmt::s(f, "reason");
            if !reason.is_empty() && !why.contains(&reason) {
                why.push(reason);
            }
        }
        let more = why.len().saturating_sub(3);
        why.truncate(3);
        let mut why = why.join("; ");
        if more > 0 {
            why += &format!("; {more} more");
        }
        s += &format!(" · {} couldn't be requested", failed.len());
        if !why.is_empty() {
            s += &format!(" ({why})");
        }
    }
    s
}

/// The Encore track rows of the items that are in the library, as the web's `playable`.
pub fn playable(items: &[Value]) -> Vec<Value> {
    items
        .iter()
        .filter(|i| fmt::truthy(i, "track_id"))
        .map(|i| {
            json!({
                "id": i["track_id"], "title": i["title"], "artist": i["artist"], "artists": i["artists"],
                "album": i["album"], "album_id": i["album_id"], "format": i["format"], "bitrate": i["bitrate"],
                "duration": i["duration"],
            })
        })
        .collect()
}

/// The toast after Share copied the link (the web's `share_playlist`).
pub const LINK_COPIED: &str = "Link copied – anyone with it can play this playlist";
/// The line beside the Shared mark.
pub const SHARED_HINT: &str = "Anyone with the link can play these songs";
/// The Stop sharing confirm: title (also its button) and text.
pub const STOP_TITLE: &str = "Stop sharing";
pub const STOP_TEXT: &str = "The link stops working for everyone who has it. Sharing again makes a new link.";
/// The toast once the link is gone.
pub const STOPPED: &str = "Sharing stopped";

/// The playlist's share path (`share.path`, "/s/<token>") when it is shared, else None.
pub fn shared_path(p: &Value) -> Option<String> {
    p["share"]["path"].as_str().filter(|s| !s.is_empty()).map(str::to_string)
}

/// Which request an `Acted` answer belongs to, for its toast.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    Missing,
    Dedupe,
    Rematch,
    Pull,
    Sync,
    Targets,
    Follow,
    Rename,
}

/// The toast of a finished action, as the web's `act(fn, msg)`.
pub fn act_text(a: Act, r: &Value) -> Option<String> {
    Some(match a {
        Act::Missing => missing_report_text(r),
        Act::Dedupe => {
            if fmt::truthy(r, "removed") {
                format!("Removed {} duplicate(s)", fmt::s(r, "removed"))
            } else {
                "No duplicates".into()
            }
        }
        Act::Rematch => format!("{} newly matched", fmt::s(r, "matched")),
        Act::Pull => format!("{} new track(s)", fmt::s(r, "added")),
        Act::Sync => {
            if fmt::truthy(r, "errors") {
                let log: Vec<String> = fmt::items(&r["log"])
                    .iter()
                    .map(|l| l.as_str().map(str::to_string).unwrap_or_else(|| l.to_string()))
                    .collect();
                format!("Synced with {} error(s): {}", fmt::s(r, "errors"), log.join("; "))
            } else {
                "Synced".into()
            }
        }
        Act::Targets => "Sync targets saved – syncing on the next pass (or press Sync now)".into(),
        Act::Follow | Act::Rename => return None,
    })
}

#[derive(Debug)]
pub enum Dialog {
    Import(Import),
    /// "Download missing songs": the album choice.
    AlbumChoice,
    /// The confirm's text.
    Delete(String),
    /// "Stop sharing": the link stops working.
    StopSharing,
}

#[derive(Debug, Default)]
pub struct State {
    pub id: i64,
    /// The playlist with its `items`; None while loading, Err(message) when it failed.
    pl: Option<Result<Box<Value>, String>>,
    /// The sync targets the user has connected (`/api/connections` keys).
    connected: Vec<String>,
    dialog: Option<Dialog>,
    /// The ⋯ menu: where, and the item id.
    menu: Option<(Point, i64)>,
    /// The name while it is being edited.
    rename: Option<String>,
    busy: bool,
}

#[derive(Clone, Debug)]
pub enum Message {
    Loaded(Result<Value, ApiError>),
    Connections(Result<Value, ApiError>),
    /// Save the songs that are in the library on this computer.
    SaveOffline,
    Play(usize),
    DownloadMissing,
    AlbumChoice(Option<String>),
    Dedupe,
    Rematch,
    Pull,
    Sync,
    AddFromLink,
    Import(ImportMsg),
    Follow(bool),
    Target(String, bool),
    Acted(Act, Result<Value, ApiError>),
    Rename,
    RenameInput(String),
    RenameSave,
    RenameCancel,
    Delete,
    DeleteConfirmed,
    Deleted(Result<Value, ApiError>),
    /// Make (or get) the public link and copy it.
    Share,
    ShareGot(Result<Value, ApiError>),
    /// Copy the link of a shared playlist again.
    CopyLink,
    /// Ask before the link stops working.
    StopSharing,
    StopSharingConfirmed,
    Unshared(Result<Value, ApiError>),
    RemoveItem(i64),
    ItemRemoved(i64, Result<Value, ApiError>),
    Menu(i64),
    CloseMenu,
    PlayNext(i64),
    Enqueue(i64),
    ViewAlbum(String),
    /// The "Connections" link of the sync card.
    OpenConnections,
    CloseDialog,
}

fn load(state: &State, ctx: &Ctx) -> Task<Message> {
    let api = ctx.api.clone();
    let path = format!("/playlists/{}", state.id);
    let api2 = ctx.api.clone();
    Task::batch([
        Task::perform(async move { api.get(&path).await }, Message::Loaded),
        Task::perform(async move { api2.get("/connections").await }, Message::Connections),
    ])
}

/// Called when the app navigates here (with the route's parameters).
pub fn open(state: &mut State, id: i64, ctx: &mut Ctx) -> Task<Message> {
    state.id = id;
    state.pl = None;
    state.dialog = None;
    state.menu = None;
    state.rename = None;
    state.busy = false;
    load(state, ctx)
}

impl State {
    fn playlist(&self) -> Option<&Value> {
        match &self.pl {
            Some(Ok(p)) => Some(p),
            _ => None,
        }
    }

    fn items(&self) -> Vec<Value> {
        self.playlist().map(|p| fmt::items(&p["items"])).unwrap_or_default()
    }

    fn item(&self, id: i64) -> Option<Value> {
        self.items().into_iter().find(|i| fmt::i(i, "id") == id)
    }
}

/// `POST` a playlist action and report back as `Acted`.
fn act(state: &mut State, ctx: &Ctx, a: Act, path: &str, body: Option<Value>) -> Task<Message> {
    state.busy = true;
    let api = ctx.api.clone();
    let path = format!("/playlists/{}/{path}", state.id);
    Task::perform(
        async move {
            match body {
                Some(b) => api.post(&path, b).await,
                None => api.post_empty(&path).await,
            }
        },
        move |r| Message::Acted(a, r),
    )
}

fn patch_task(state: &State, ctx: &Ctx, a: Act, body: Value) -> Task<Message> {
    let api = ctx.api.clone();
    let path = format!("/playlists/{}", state.id);
    Task::perform(async move { api.patch(&path, body).await }, move |r| Message::Acted(a, r))
}

pub fn update(state: &mut State, msg: Message, ctx: &mut Ctx) -> Task<Message> {
    match msg {
        Message::Loaded(Ok(v)) => state.pl = Some(Ok(Box::new(v))),
        Message::Loaded(Err(e)) => state.pl = Some(Err(e.message)),
        Message::Connections(Ok(v)) => {
            state.connected = v["connections"]
                .as_object()
                .map(|o| o.keys().filter(|k| target_name(k).is_some()).cloned().collect())
                .unwrap_or_default();
        }
        Message::Connections(Err(_)) => {}
        Message::OpenConnections => ctx.navigate(Page::Settings(Section::Connections)),
        Message::Play(index) => {
            let rows = playable(&state.items());
            if !rows.is_empty() {
                ctx.play(rows, index);
            }
        }
        Message::SaveOffline => {
            crate::desktop::offline::save(ctx, playable(&state.items()));
        }
        Message::DownloadMissing => state.dialog = Some(Dialog::AlbumChoice),
        Message::AlbumChoice(mode) => {
            state.dialog = None;
            return act(state, ctx, Act::Missing, "request-missing", Some(json!({ "album_mode": mode })));
        }
        Message::Dedupe => return act(state, ctx, Act::Dedupe, "dedupe", None),
        Message::Rematch => return act(state, ctx, Act::Rematch, "rematch", None),
        Message::Pull => return act(state, ctx, Act::Pull, "refresh", None),
        Message::Sync => return act(state, ctx, Act::Sync, "sync", None),
        Message::AddFromLink => state.dialog = Some(Dialog::Import(Import::new("", Some(state.id)))),
        Message::Import(m) => {
            if let Some(Dialog::Import(imp)) = state.dialog.as_mut() {
                let (task, done) = playlists::import_update(imp, m, ctx);
                if done.is_some() {
                    state.dialog = None;
                    return Task::batch([task.map(Message::Import), load(state, ctx)]);
                }
                return task.map(Message::Import);
            }
        }
        Message::Follow(on) => {
            if let Some(Ok(p)) = state.pl.as_mut() {
                p["follow"] = json!(on);
            }
            return patch_task(state, ctx, Act::Follow, json!({ "follow": on }));
        }
        Message::Target(name, on) => {
            let mut targets: Vec<String> = state
                .playlist()
                .map(|p| {
                    fmt::items(&p["sync_targets"])
                        .iter()
                        .filter_map(|t| t.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            targets.retain(|t| *t != name);
            if on {
                targets.push(name);
            }
            if let Some(Ok(p)) = state.pl.as_mut() {
                p["sync_targets"] = json!(targets);
            }
            return patch_task(state, ctx, Act::Targets, json!({ "sync_targets": targets }));
        }
        Message::Acted(a, r) => {
            state.busy = false;
            match r {
                Ok(v) => {
                    if let Some(text) = act_text(a, &v) {
                        ctx.toast_ok(text);
                    }
                    if matches!(a, Act::Follow | Act::Targets) {
                        // the answer is the summary; the items stay as they are
                        if let Some(Ok(p)) = state.pl.as_mut() {
                            for key in ["follow", "sync_targets", "name", "updated_at"] {
                                if let Some(x) = v.get(key) {
                                    p[key] = x.clone();
                                }
                            }
                        }
                    } else {
                        return load(state, ctx);
                    }
                }
                Err(e) => ctx.toast_err(e.message),
            }
        }
        Message::Rename => {
            if let Some(p) = state.playlist() {
                if fmt::s(p, "kind") != "liked" {
                    state.rename = Some(fmt::s(p, "name"));
                }
            }
        }
        Message::RenameInput(s) => {
            if state.rename.is_some() {
                state.rename = Some(s);
            }
        }
        Message::RenameCancel => state.rename = None,
        Message::RenameSave => {
            let Some(name) = state.rename.take() else {
                return Task::none();
            };
            let name = name.trim().to_string();
            let old = state.playlist().map(|p| fmt::s(p, "name")).unwrap_or_default();
            if name.is_empty() || name == old {
                return Task::none();
            }
            if let Some(Ok(p)) = state.pl.as_mut() {
                p["name"] = json!(name);
            }
            return patch_task(state, ctx, Act::Rename, json!({ "name": name }));
        }
        Message::Delete => {
            if let Some(p) = state.playlist() {
                state.dialog = Some(Dialog::Delete(format!(
                    "Delete “{}”? Songs stay in the library.",
                    fmt::s(p, "name")
                )));
            }
        }
        Message::DeleteConfirmed => {
            state.dialog = None;
            let api = ctx.api.clone();
            let path = format!("/playlists/{}", state.id);
            return Task::perform(async move { api.del(&path).await }, Message::Deleted);
        }
        Message::Deleted(Ok(_)) => ctx.navigate(Page::Playlists),
        Message::Deleted(Err(e)) => ctx.toast_err(e.message),
        Message::Share => {
            let api = ctx.api.clone();
            let path = format!("/playlists/{}/share", state.id);
            return Task::perform(async move { api.post_empty(&path).await }, Message::ShareGot);
        }
        Message::ShareGot(Ok(r)) => {
            ctx.copy(format!("{}{}", ctx.base(), fmt::s(&r, "path")));
            ctx.toast_ok(LINK_COPIED);
            // the Shared line appears
            return load(state, ctx);
        }
        Message::ShareGot(Err(e)) => ctx.toast_err(e.message),
        Message::CopyLink => {
            if let Some(path) = state.playlist().and_then(shared_path) {
                ctx.copy_text(format!("{}{path}", ctx.base()));
            }
        }
        Message::StopSharing => state.dialog = Some(Dialog::StopSharing),
        Message::StopSharingConfirmed => {
            state.dialog = None;
            let api = ctx.api.clone();
            let path = format!("/playlists/{}/share", state.id);
            return Task::perform(async move { api.del(&path).await }, Message::Unshared);
        }
        Message::Unshared(Ok(_)) => {
            ctx.toast_ok(STOPPED);
            return load(state, ctx);
        }
        Message::Unshared(Err(e)) => ctx.toast_err(e.message),
        Message::RemoveItem(iid) => {
            state.menu = None;
            let api = ctx.api.clone();
            let path = format!("/playlists/{}/items/{iid}", state.id);
            return Task::perform(async move { api.del(&path).await }, move |r| Message::ItemRemoved(iid, r));
        }
        Message::ItemRemoved(iid, Ok(_)) => {
            if let Some(Ok(p)) = state.pl.as_mut() {
                let had_track = p["items"]
                    .as_array()
                    .and_then(|a| a.iter().find(|i| fmt::i(i, "id") == iid))
                    .is_some_and(|i| fmt::truthy(i, "track_id"));
                if let Some(items) = p["items"].as_array_mut() {
                    items.retain(|i| fmt::i(i, "id") != iid);
                }
                p["total"] = json!(fmt::i(p, "total").saturating_sub(1).max(0));
                if had_track {
                    p["matched"] = json!(fmt::i(p, "matched").saturating_sub(1).max(0));
                }
            }
        }
        Message::ItemRemoved(_, Err(e)) => ctx.toast_err(e.message),
        Message::Menu(iid) => state.menu = Some((ctx.cursor, iid)),
        Message::CloseMenu => state.menu = None,
        Message::PlayNext(iid) => {
            state.menu = None;
            if let Some(it) = state.item(iid) {
                ctx.play_next(playable(&[it]));
            }
        }
        Message::Enqueue(iid) => {
            state.menu = None;
            if let Some(it) = state.item(iid) {
                ctx.enqueue(playable(&[it]));
            }
        }
        Message::ViewAlbum(id) => {
            state.menu = None;
            ctx.navigate(Page::LibraryAlbum(id));
        }
        Message::CloseDialog => state.dialog = None,
    }
    Task::none()
}

/// The status badges of one item: in library (+ verified / wrong audio), the request's state,
/// or missing.
fn status_badges<'a>(it: &Value) -> Vec<El<'a, Message>> {
    if fmt::truthy(it, "track_id") {
        let mut b = vec![badge("✓ in library", Badge::Ok)];
        match it["verify_status"].as_str() {
            Some("verified") => b.push(
                iced::widget::tooltip(badge("verified", Badge::Ok), crate::widgets::tip_box("Audio fingerprint confirmed"), iced::widget::tooltip::Position::Top).gap(4).into(),
            ),
            Some("mismatch") => b.push(badge("wrong audio", Badge::Err)),
            _ => {}
        }
        b
    } else if let Some(s) = it["request_status"].as_str() {
        vec![catalog::status_badge(s)]
    } else {
        vec![badge("missing", Badge::Plain)]
    }
}

/// The heart on the accent gradient that stands for Liked Songs.
fn heart_art<'a>(size: f32, ctx: &Ctx<'a>) -> El<'a, Message> {
    container(widgets::icon(Icon::Heart, size * 0.42, ctx.look.on_accent))
        .width(size)
        .height(size)
        .center(size)
        .style(theme::accent_box)
        .into()
}

fn header<'a>(state: &'a State, p: &'a Value, ctx: &Ctx<'a>) -> El<'a, Message> {
    let look = ctx.look;
    let liked = fmt::s(p, "kind") == "liked";
    let items = state.items();
    let missing = items.iter().filter(|i| requestable(i)).count();
    let (total, matched) = (fmt::f(p, "total"), fmt::f(p, "matched"));
    let has_matched = matched > 0.0;
    let cover: El<'a, Message> = if liked {
        heart_art(180.0, ctx)
    } else {
        catalog::art(ctx, &fmt::s(p, "cover_url"), 180.0, false, Icon::List)
    };
    let kicker = format!(
        "{} · {}",
        if liked { "Liked songs · synced everywhere" } else { "Playlist" },
        src_label(p)
    );
    let title: El<'a, Message> = match &state.rename {
        Some(name) => row![
            text_input("Playlist name", name)
                .on_input(Message::RenameInput)
                .on_submit(Message::RenameSave)
                .padding([8, 12])
                .size(20)
                .width(420)
                .style(theme::input),
            btn_sm("Save", Btn::Primary, Some(Message::RenameSave)),
            btn_sm("Cancel", Btn::Ghost, Some(Message::RenameCancel)),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .into(),
        None => {
            let mut r = row![h1(fmt::s(p, "name"))].spacing(8).align_y(Alignment::Center);
            if !liked {
                r = r.push(icon_btn(Icon::Edit, "Click to rename", Some(Message::Rename)));
            }
            r.into()
        }
    };
    let counts = row![
        muted(format!("{} tracks · ", fmt::s(p, "total"))).size(14),
        t(fmt::s(p, "matched")).size(14).font(theme::bold()).color(look.ok),
        muted(format!(" in library · {} missing", (total - matched).max(0.0) as i64)).size(14),
    ]
    .align_y(Alignment::Center);
    let pct = widgets::stat::fraction(matched, total) as f32 * 100.0;
    let progress = container(progress_bar(0.0..=100.0, pct).girth(8).style(theme::progress)).max_width(360);

    let mut actions = row![].spacing(8).align_y(Alignment::Center);
    if has_matched {
        actions = actions.push(btn_icon(Icon::Play, "Play", Btn::Primary, Some(Message::Play(0))));
        actions = actions.push(btn_icon(Icon::Download, "Save for offline", Btn::Default, Some(Message::SaveOffline)));
    }
    actions = actions.push(
        tooltip(
            btn_icon(Icon::External, "Share", Btn::Default, Some(Message::Share)),
            widgets::tip_box("Share link"),
            tooltip::Position::Bottom,
        )
        .gap(4),
    );
    if missing > 0 {
        actions = actions.push(btn_icon(
            Icon::Download,
            format!("Download {missing} missing"),
            if has_matched { Btn::Default } else { Btn::Primary },
            (!state.busy).then_some(Message::DownloadMissing),
        ));
    }
    actions = actions.push(btn_icon(Icon::Plus, "Add from link", Btn::Default, Some(Message::AddFromLink)));
    actions = actions.push(btn_icon(
        Icon::Check,
        "Remove duplicates",
        Btn::Default,
        (!state.busy).then_some(Message::Dedupe),
    ));
    actions = actions.push(btn_icon(
        Icon::Refresh,
        "Re-check library",
        Btn::Default,
        (!state.busy).then_some(Message::Rematch),
    ));
    if fmt::truthy(p, "source_url") {
        let src = fmt::s(p, "source");
        actions = actions.push(btn_icon(
            Icon::Refresh,
            format!("Pull from {}", src_name(&src).unwrap_or("source")),
            Btn::Default,
            (!state.busy).then_some(Message::Pull),
        ));
    }
    if !liked {
        actions = actions.push(icon_btn(Icon::Trash, "Delete playlist", Some(Message::Delete)));
    }
    let mut right = column![catalog::kicker(kicker), title, counts, progress].spacing(8).width(Length::Fill);
    if shared_path(p).is_some() {
        right = right.push(
            row![
                badge("Shared", Badge::Accent),
                small_muted(SHARED_HINT),
                btn_icon(Icon::Copy, "Copy link", Btn::Default, Some(Message::CopyLink)),
                btn_icon(Icon::X, STOP_TITLE, Btn::Ghost, Some(Message::StopSharing)),
            ]
            .spacing(8)
            .align_y(Alignment::Center)
            .wrap(),
        );
    }
    let right = right.push(container(actions.wrap()).padding([6, 0]));
    row![cover, right].spacing(24).align_y(Alignment::End).into()
}

/// The "Sync to" card: a switch per connected target, Follow source and Sync now.
fn sync_card<'a>(state: &'a State, p: &'a Value, _ctx: &Ctx<'a>) -> El<'a, Message> {
    let liked = fmt::s(p, "kind") == "liked";
    let source = fmt::s(p, "source");
    let targets: Vec<String> = fmt::items(&p["sync_targets"])
        .iter()
        .filter_map(|t| t.as_str().map(str::to_string))
        .collect();
    let mut left = row![t("Sync to").size(14).font(theme::bold())]
        .spacing(14)
        .align_y(Alignment::Center);
    if state.connected.is_empty() {
        left = left.push(
            row![
                small_muted("Connect Jellyfin, Plex or Navidrome on the"),
                catalog::muted_link("Connections", 12.0, Message::OpenConnections),
                small_muted("page."),
            ]
            .spacing(4)
            .align_y(Alignment::Center),
        );
    } else {
        for s in state.connected.iter().filter(|s| **s != source) {
            let name = s.clone();
            left = left.push(
                toggler(targets.contains(s))
                    .label(target_name(s).unwrap_or(""))
                    .text_size(13.5)
                    .spacing(8)
                    .on_toggle(move |on| Message::Target(name.clone(), on))
                    .style(theme::switch),
            );
        }
    }
    if liked {
        left = left.push(small("Liked songs also sync as favourites / loves automatically.").color(theme::look().faint));
    }
    let mut right = row![].spacing(12).align_y(Alignment::Center);
    if fmt::truthy(p, "source_url") {
        right = right.push(
            toggler(fmt::truthy(p, "follow"))
                .label("Follow source")
                .text_size(13.5)
                .spacing(8)
                .on_toggle(Message::Follow)
                .style(theme::switch),
        );
    }
    if !state.connected.is_empty() {
        right = right.push(btn_icon(
            Icon::Refresh,
            "Sync now",
            Btn::Default,
            (!state.busy).then_some(Message::Sync),
        ));
    }
    card(
        row![container(left.wrap()).width(Length::Fill), right]
            .spacing(12)
            .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .into()
}

fn track_rows<'a>(state: &'a State, items: &[Value], ctx: &Ctx<'a>) -> El<'a, Message> {
    let current = ctx.player.current_id();
    let mut play_index = 0usize;
    let mut rows: Vec<El<'a, Message>> = Vec::with_capacity(items.len());
    for (i, it) in items.iter().enumerate() {
        let iid = fmt::i(it, "id");
        let in_library = fmt::truthy(it, "track_id");
        let sub = [fmt::s(it, "artist"), fmt::s(it, "album")]
            .iter()
            .filter(|x| !x.is_empty())
            .cloned()
            .collect::<Vec<_>>()
            .join(" · ");
        let mut r = TrackRow::new(fmt::s(it, "title"))
            .index(i + 1)
            .sub(sub)
            .duration(Some(fmt::f(it, "duration")).filter(|d| *d > 0.0))
            .playing(in_library && current == Some(fmt::i(it, "track_id")))
            .extra(icon_btn(Icon::X, "Remove", Some(Message::RemoveItem(iid))))
            .on_menu(Message::Menu(iid));
        if in_library {
            r = r.quality(fmt::quality(it)).on_play(Message::Play(play_index));
            play_index += 1;
        }
        if let Some(m) = it["request_message"].as_str().filter(|m| !m.is_empty() && !in_library) {
            r = r.note(m.to_string());
        }
        for b in status_badges(it) {
            r = r.badge(b);
        }
        rows.push(r.view());
    }
    let _ = state;
    card_flat(column(rows)).width(Length::Fill).into()
}

pub fn view<'a>(state: &'a State, ctx: &Ctx<'a>) -> El<'a, Message> {
    match &state.pl {
        None => column![skeleton_rows(8)].into(),
        Some(Err(e)) => column![h1("Playlist"), error_box(e)].spacing(16).into(),
        Some(Ok(p)) => {
            let items = state.items();
            let mut col = column![header(state, p, ctx), sync_card(state, p, ctx)].spacing(18);
            if items.is_empty() {
                col = col.push(empty(
                    Icon::Music,
                    "Empty playlist",
                    "Add tracks from an album page or from a link.",
                ));
            } else {
                col = col.push(track_rows(state, &items, ctx));
            }
            col.width(Length::Fill).into()
        }
    }
}

/// A modal or ⋯ menu to lay over the whole window, when one is open.
pub fn overlay<'a>(state: &'a State, ctx: &Ctx<'a>) -> Option<El<'a, Message>> {
    if let Some((at, iid)) = state.menu {
        let it = state.item(iid)?;
        let mut items = Vec::new();
        if fmt::truthy(&it, "track_id") {
            items.push(MenuItem::new("Play next", Message::PlayNext(iid)).icon(Icon::Play));
            items.push(MenuItem::new("Add to queue", Message::Enqueue(iid)).icon(Icon::Plus));
        }
        if fmt::truthy(&it, "album_id") {
            items.push(MenuItem::new("View album", Message::ViewAlbum(fmt::s(&it, "album_id"))).icon(Icon::Disc));
        }
        items.push(MenuItem::new("Remove", Message::RemoveItem(iid)).icon(Icon::X).danger());
        return Some(pop_menu(at, items, Message::CloseMenu, ctx.size));
    }
    match state.dialog.as_ref()? {
        Dialog::Import(imp) => Some(playlists::import_dialog(imp, Message::Import, Message::CloseDialog)),
        Dialog::AlbumChoice => {
            let body = column![
                muted("When a song is part of an album:").size(14),
                btn_icon(
                    Icon::Music,
                    "Just the song",
                    Btn::Default,
                    Some(Message::AlbumChoice(Some("song".into())))
                )
                .width(Length::Fill),
                btn_icon(
                    Icon::Disc,
                    "Download the whole album (best quality available)",
                    Btn::Default,
                    Some(Message::AlbumChoice(Some("album".into())))
                )
                .width(Length::Fill),
                btn("Use the admin default", Btn::Ghost, Some(Message::AlbumChoice(None))).width(Length::Fill),
            ]
            .spacing(10);
            Some(modal("Download missing songs", body, None, false, Message::CloseDialog))
        }
        Dialog::Delete(text) => Some(confirm(
            "Delete playlist",
            text,
            "Delete",
            true,
            Message::DeleteConfirmed,
            Message::CloseDialog,
        )),
        Dialog::StopSharing => Some(confirm(
            STOP_TITLE,
            STOP_TEXT,
            STOP_TITLE,
            true,
            Message::StopSharingConfirmed,
            Message::CloseDialog,
        )),
    }
}

pub fn subscription(_state: &State) -> Subscription<Message> {
    Subscription::none()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_toast_counts_and_explains() {
        assert_eq!(
            missing_report_text(&json!({ "requested": 120, "in_progress": 0, "failed": [] })),
            "Requested 120"
        );
        assert_eq!(
            missing_report_text(&json!({ "requested": 120, "in_progress": 600, "failed": [] })),
            "Requested 120 · 600 already in progress"
        );
        let failed: Vec<Value> = (0..64)
            .map(|i| {
                json!({ "title": format!("Song {i}"), "reason": if i % 2 == 0 { "Track needs a MusicBrainz id or artist + title" } else { "Weekly request limit reached (50). Ask an admin to raise it." } })
            })
            .collect();
        assert_eq!(
            missing_report_text(&json!({ "requested": 120, "in_progress": 600, "failed": failed })),
            "Requested 120 · 600 already in progress · 64 couldn't be requested (Track needs a MusicBrainz id or artist + title; Weekly request limit reached (50). Ask an admin to raise it.)"
        );
        let failed: Vec<Value> = (0..5).map(|i| json!({ "title": "x", "reason": format!("reason {i}") })).collect();
        assert_eq!(
            missing_report_text(&json!({ "requested": 0, "failed": failed })),
            "Requested 0 · 5 couldn't be requested (reason 0; reason 1; reason 2; 2 more)"
        );
        assert_eq!(missing_report_text(&json!({ "requested": 3 })), "Requested 3");
    }

    #[test]
    fn only_rows_whose_request_is_over_or_gone_are_asked_for() {
        assert!(requestable(&json!({ "title": "a" })));
        assert!(requestable(&json!({ "request_id": 5 })), "the request row is gone");
        for s in ["failed", "cancelled", "denied"] {
            assert!(requestable(&json!({ "request_id": 5, "request_status": s })), "{s}");
        }
        for s in ["pending", "queued", "searching", "downloading", "completed"] {
            assert!(!requestable(&json!({ "request_id": 5, "request_status": s })), "{s}");
        }
        assert!(!requestable(&json!({ "track_id": 9 })));
    }

    #[test]
    fn playable_rows_and_action_toasts() {
        let items = vec![
            json!({ "id": 1, "track_id": 9, "title": "A", "artist": "B", "album": "C", "album_id": "x", "duration": 3 }),
            json!({ "id": 2, "title": "Missing" }),
        ];
        let p = playable(&items);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0]["id"], json!(9));
        assert_eq!(act_text(Act::Dedupe, &json!({ "removed": 0 })), Some("No duplicates".into()));
        assert_eq!(act_text(Act::Dedupe, &json!({ "removed": 2 })), Some("Removed 2 duplicate(s)".into()));
        assert_eq!(act_text(Act::Rematch, &json!({ "matched": 4 })), Some("4 newly matched".into()));
        assert_eq!(act_text(Act::Pull, &json!({ "added": 1 })), Some("1 new track(s)".into()));
        assert_eq!(act_text(Act::Sync, &json!({ "errors": 0 })), Some("Synced".into()));
        assert_eq!(
            act_text(Act::Sync, &json!({ "errors": 1, "log": ["plex: nope"] })),
            Some("Synced with 1 error(s): plex: nope".into())
        );
        assert_eq!(act_text(Act::Follow, &json!({})), None);
    }

    #[test]
    fn shared_path_reads_the_share() {
        assert_eq!(shared_path(&json!({ "share": { "token": "abc", "path": "/s/abc", "url": "http://x/s/abc" } })).as_deref(), Some("/s/abc"));
        assert_eq!(shared_path(&json!({ "share": null })), None);
        assert_eq!(shared_path(&json!({ "name": "Road trip" })), None);
        assert_eq!(shared_path(&json!({ "share": { "path": "" } })), None);
    }

    #[test]
    fn share_texts_match_the_web() {
        // web-rs/src/ui.rs share_playlist and pages/playlists.rs (the Shared line and the Stop sharing confirm)
        assert_eq!(LINK_COPIED, "Link copied – anyone with it can play this playlist");
        assert_eq!(SHARED_HINT, "Anyone with the link can play these songs");
        assert_eq!(STOP_TITLE, "Stop sharing");
        assert_eq!(STOP_TEXT, "The link stops working for everyone who has it. Sharing again makes a new link.");
        assert_eq!(STOPPED, "Sharing stopped");
    }
}
