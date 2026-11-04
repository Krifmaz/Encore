//! Playlist: One playlist: its songs, sync and download missing. Behaviour reference: web-rs/src/pages/playlists.rs render_playlist.
//!
//! Liked Songs is the playlist the server marks `kind: liked` (no rename, no delete, the heart
//! as its cover). "Download N missing" asks for the album choice first and toasts the counts
//! (`missing_report_text`).
//!
//! Share makes the playlist's public link (`POST /api/playlists/{id}/share`) and copies it; the
//! link always shows the playlist as it is now, Liked Songs too. While shared, a line under the
//! progress bar has Copy link and Stop sharing (`DELETE /api/playlists/{id}/share`), and how many
//! people added it to their playlists ("added by 3 people", never who).
//!
//! Someone else's playlist added from its share link (`kind: linked`) is read-only: its owner's
//! songs as they are now, Play and Save for offline, Save a copy (`POST /api/playlists/{id}/copy`)
//! and Remove from my playlists (`DELETE /api/playlists/{id}`). No rename, sync, share, Add from
//! link, Download missing or per-song Remove. Once the link ended, only the "stopped sharing" line
//! and Remove.
//!
//! Collaborative (0.37): the owner's Collaborative switch (`PATCH {collaborative}`) and Invite window
//! (by user name, the invite link, members with Remove). Someone else's collaborative playlist the
//! user joined (`collab.role: collaborator`) works like their own for its songs (add, remove,
//! reorder, de-duplicate), without rename, share, sync, follow or Download missing; its Delete is
//! Leave. docs/contracts/collaborative-playlists.md.
//!
//! 🔍 in the header opens a search above the songs that filters them by title, artist and album
//! as you type (× clears it, 🔍 again closes it); a song found still plays the whole playlist from
//! that song.

use crate::api::ApiError;
use crate::fmt;
use crate::pages::playlists::{
    self, added_by_text, found_text, is_linked, item_haystack, linked_active, linked_hint, linked_kicker, linked_owner,
    linked_stopped, query_matches, remove_text, src_label, src_name, target_name, Import, ImportMsg, MergeInto,
    LINKED_EMPTY, REMOVED_FROM_MINE, REMOVE_FROM_MINE, SAVED_COPY, SAVE_COPY, SONG_SEARCH_PLACEHOLDER,
};
use crate::pages::{Ctx, El, Page, Section};
use crate::theme::{self, Badge, Btn};
use crate::widgets::catalog;
use crate::widgets::{
    self, badge, btn, btn_icon, btn_sm, card, card_flat, confirm, empty, error_box, h1, icon_btn, modal, muted,
    pop_menu, search_box_clearable, skeleton_rows, small, small_muted, t, Icon, MenuItem, TrackRow,
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

/// Why a missing row can't be requested (the server's `cant_request`: no MusicBrainz id and no
/// artist and title to search by), or `None` when it can be, or it is in the library. The web's
/// `cant_request` in web-rs/src/pages/playlists.rs.
pub fn cant_request(it: &Value) -> Option<&str> {
    if fmt::truthy(it, "track_id") {
        return None;
    }
    it["cant_request"].as_str().filter(|w| !w.is_empty())
}

/// The "can't be requested" list (the web's `cant_request_list`): one "Artist - Title" line per
/// song, the title (or the artist) alone when the other is empty, the album in brackets when known.
pub fn cant_request_list(rows: &[&Value]) -> String {
    rows.iter()
        .map(|r| {
            let (artist, title, album) = (fmt::s(r, "artist"), fmt::s(r, "title"), fmt::s(r, "album"));
            let mut line = if artist.is_empty() {
                title
            } else if title.is_empty() {
                artist
            } else {
                format!("{artist} - {title}")
            };
            if !album.is_empty() {
                line += &format!(" ({album})");
            }
            line
        })
        .collect::<Vec<_>>()
        .join("\n")
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

/// The hint beside the Shared mark, with how many people added the playlist when any did:
/// "Anyone with the link can play these songs · added by 3 people".
pub fn shared_hint(p: &Value) -> String {
    let added = added_by_text(fmt::i(&p["share"], "added_by"));
    if added.is_empty() {
        SHARED_HINT.to_string()
    } else {
        format!("{SHARED_HINT} · {added}")
    }
}

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
    /// "Remove from my playlists" (someone else's playlist): the confirm's text.
    RemoveLinked(String),
    /// Collaborative: invite by user name, the invite link, who's in it.
    Members(Members),
    /// "Leave playlist" (someone else's collaborative playlist): the confirm's text.
    Leave(String),
    /// Merge into…: the songs go into another playlist or Liked Songs.
    MergeInto(MergeInto),
}

/// The members window: the server's `/collaborators` answer, the user name being typed, its error.
#[derive(Debug, Default)]
pub struct Members {
    data: Option<Value>,
    username: String,
    error: Option<String>,
    busy: bool,
}

/// What a member may do, under the name of someone else's collaborative playlist.
pub const MEMBER_HINT: &str = "You can add, remove and reorder songs";
pub const LEAVE_TEXT: &str = "You stop seeing it and can't change it any more. The owner can invite you again.";

/// Someone else's collaborative playlist the user joined.
pub fn is_member(p: &Value) -> bool {
    p["collab"]["role"].as_str() == Some("collaborator")
}

/// "1 member" / "3 members" ("" for none).
pub fn members_text(n: i64) -> String {
    match n {
        n if n <= 0 => String::new(),
        1 => "1 member".to_string(),
        n => format!("{n} members"),
    }
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
    /// Who added the open menu's song (0.36, `/library/tracks/{id}/added`): (song id, words), its first line.
    added: Option<(i64, String)>,
    /// The name while it is being edited.
    rename: Option<String>,
    busy: bool,
    /// The song search: None while closed, else its text ("" shows every song).
    search: Option<String>,
}

/// The id of the song search box, to focus it when it opens.
const SEARCH_ID: &str = "playlist-search";
/// The header's 🔍, as its tooltip says.
const SEARCH_TIP: &str = "Search in this playlist";

#[derive(Clone, Debug)]
pub enum Message {
    Loaded(Result<Value, ApiError>),
    /// One artist of a song (its line links each one).
    OpenPage(Page),
    Connections(Result<Value, ApiError>),
    /// Save the songs that are in the library on this computer.
    SaveOffline,
    Play(usize),
    /// Shuffle: the songs in a random order (in a group listen the server's, the same for
    /// everyone).
    Shuffle,
    DownloadMissing,
    /// "N can't be requested": the songs "Download missing" skips, as a list to copy.
    CantRequest,
    AlbumChoice(Option<String>),
    Dedupe,
    Rematch,
    Pull,
    Sync,
    AddFromLink,
    Import(ImportMsg),
    /// Merge into…: the window, the playlists it lists, the choice, Go and the answer.
    OpenMergeInto,
    MergeTargets(Result<Value, ApiError>),
    MergeIntoPick(Option<i64>),
    MergeIntoDelete(bool),
    MergeIntoGo,
    MergedInto(Result<Value, ApiError>),
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
    /// Someone else's playlist: an editable copy of it as it is now.
    SaveCopy,
    CopySaved(Result<Value, ApiError>),
    /// Someone else's playlist: ask, then take it out of my playlists.
    RemoveLinked,
    RemoveLinkedConfirmed,
    RemovedLinked(Result<Value, ApiError>),
    RemoveItem(i64),
    ItemRemoved(i64, Result<Value, ApiError>),
    Menu(i64),
    /// `/library/tracks/{id}/added` for the open menu's song.
    Added(i64, Result<Value, ApiError>),
    /// 🔍: open the song search (focused), or close it and forget the search.
    ToggleSearch,
    /// The search box's text changed.
    SearchInput(String),
    /// × in the search box.
    SearchClear,
    CloseMenu,
    PlayNext(i64),
    Enqueue(i64),
    ViewAlbum(String),
    /// The "Connections" link of the sync card.
    OpenConnections,
    CloseDialog,
    /// Collaborative on or off (the owner).
    Collaborative(bool),
    CollaborativeSet(bool, Result<Value, ApiError>),
    OpenMembers,
    MembersLoaded(Result<Value, ApiError>),
    InviteName(String),
    Invite,
    Invited(Result<Value, ApiError>),
    MakeInviteLink,
    CopyInviteLink,
    DropInviteLink,
    RemoveMember(i64),
    /// After a change in the members window: load it (and the page) again.
    MembersChanged(Result<Value, ApiError>),
    Leave,
    LeaveConfirmed,
    Left(Result<Value, ApiError>),
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
    // another playlist starts without a search
    state.search = None;
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
        Message::ToggleSearch => {
            if state.search.take().is_none() {
                state.search = Some(String::new());
                return iced::widget::operation::focus(SEARCH_ID);
            }
        }
        Message::SearchInput(q) => state.search = Some(q),
        Message::SearchClear => {
            state.search = Some(String::new());
            return iced::widget::operation::focus(SEARCH_ID);
        }
        Message::OpenPage(p) => ctx.navigate(p),
        Message::Play(index) => {
            let rows = playable(&state.items());
            if !rows.is_empty() {
                ctx.play(rows, index);
            }
        }
        Message::Shuffle => {
            let rows = playable(&state.items());
            if !rows.is_empty() {
                ctx.play_shuffled(rows);
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
        Message::OpenMergeInto => {
            state.dialog = Some(Dialog::MergeInto(MergeInto::default()));
            let api = ctx.api.clone();
            return Task::perform(async move { api.get("/playlists").await }, Message::MergeTargets);
        }
        Message::MergeTargets(r) => {
            if let Some(Dialog::MergeInto(m)) = state.dialog.as_mut() {
                match r {
                    Ok(v) => m.targets = playlists::targets(&fmt::items(&v), &[state.id], None),
                    Err(e) => ctx.toast_err(e.message),
                }
            }
        }
        Message::MergeIntoPick(into) => {
            if let Some(Dialog::MergeInto(m)) = state.dialog.as_mut() {
                m.into = into;
            }
        }
        Message::MergeIntoDelete(on) => {
            if let Some(Dialog::MergeInto(m)) = state.dialog.as_mut() {
                m.delete_source = on;
            }
        }
        Message::MergeIntoGo => {
            if let Some(Dialog::MergeInto(m)) = state.dialog.as_mut() {
                let Some(into) = m.into.filter(|_| !m.busy) else { return Task::none() };
                m.busy = true;
                let body = json!({ "ids": [state.id], "into": into, "delete_sources": m.delete_source });
                let api = ctx.api.clone();
                return Task::perform(async move { api.post("/playlists/merge", body).await }, Message::MergedInto);
            }
        }
        Message::MergedInto(Ok(r)) => {
            state.dialog = None;
            ctx.toast_ok(playlists::merged_into_text(&r));
            ctx.navigate(Page::Playlist(fmt::i(&r, "id")));
        }
        Message::MergedInto(Err(e)) => {
            if let Some(Dialog::MergeInto(m)) = state.dialog.as_mut() {
                m.busy = false;
            }
            ctx.toast_err(e.message);
        }
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
        Message::CantRequest => {
            if let Some(p) = state.playlist() {
                let rows: Vec<&Value> = p["items"].as_array().map(|a| a.iter().filter(|i| cant_request(i).is_some()).collect()).unwrap_or_default();
                ctx.details(format!("{} songs can't be requested", rows.len()), cant_request_list(&rows));
            }
        }
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
        Message::SaveCopy => {
            if state.busy {
                return Task::none();
            }
            state.busy = true;
            let api = ctx.api.clone();
            let path = format!("/playlists/{}/copy", state.id);
            return Task::perform(async move { api.post_empty(&path).await }, Message::CopySaved);
        }
        Message::CopySaved(r) => {
            state.busy = false;
            match r {
                Ok(v) => {
                    ctx.toast_ok(SAVED_COPY);
                    ctx.navigate(Page::Playlist(fmt::i(&v, "id")));
                }
                Err(e) => ctx.toast_err(e.message),
            }
        }
        Message::RemoveLinked => {
            if let Some(p) = state.playlist() {
                state.dialog = Some(Dialog::RemoveLinked(remove_text(&fmt::s(p, "name"))));
            }
        }
        Message::RemoveLinkedConfirmed => {
            state.dialog = None;
            let api = ctx.api.clone();
            let path = format!("/playlists/{}", state.id);
            return Task::perform(async move { api.del(&path).await }, Message::RemovedLinked);
        }
        Message::RemovedLinked(Ok(_)) => {
            ctx.toast_ok(REMOVED_FROM_MINE);
            ctx.navigate(Page::Playlists);
        }
        Message::RemovedLinked(Err(e)) => ctx.toast_err(e.message),
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
        Message::Menu(iid) => {
            state.menu = Some((ctx.cursor, iid));
            state.added = None;
            let tid = state.item(iid).map(|it| fmt::i(&it, "track_id")).unwrap_or(0);
            if tid > 0 {
                let api = ctx.api.clone();
                return Task::perform(async move { api.get(&format!("/library/tracks/{tid}/added")).await }, move |r| {
                    Message::Added(tid, r)
                });
            }
        }
        Message::Added(tid, r) => {
            if let Ok(v) = r {
                state.added = Some((tid, crate::pages::library_types::added_of(&v, fmt::date)));
            }
        }
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
        Message::Collaborative(on) => {
            if let Some(Ok(p)) = state.pl.as_mut() {
                p["collaborative"] = json!(on as i64);
            }
            let api = ctx.api.clone();
            let path = format!("/playlists/{}", state.id);
            return Task::perform(async move { api.patch(&path, json!({ "collaborative": on })).await }, move |r| {
                Message::CollaborativeSet(on, r)
            });
        }
        Message::CollaborativeSet(on, r) => match r {
            Ok(_) => {
                ctx.toast_ok(if on {
                    "Collaborative: people you invite can change the songs"
                } else {
                    "Collaborative off: members can't see it until it's on again"
                });
                if on {
                    state.dialog = Some(Dialog::Members(Members::default()));
                    return Task::batch([load(state, ctx), load_members(state, ctx)]);
                }
                return load(state, ctx);
            }
            Err(e) => {
                ctx.toast_err(e.message);
                return load(state, ctx);
            }
        },
        Message::OpenMembers => {
            state.dialog = Some(Dialog::Members(Members::default()));
            return load_members(state, ctx);
        }
        Message::MembersLoaded(r) => {
            if let Some(Dialog::Members(m)) = state.dialog.as_mut() {
                m.busy = false;
                match r {
                    Ok(v) => m.data = Some(v),
                    Err(e) => m.error = Some(e.message),
                }
            }
        }
        Message::InviteName(s) => {
            if let Some(Dialog::Members(m)) = state.dialog.as_mut() {
                m.username = s;
            }
        }
        Message::Invite => {
            if let Some(Dialog::Members(m)) = state.dialog.as_mut() {
                let name = m.username.trim().to_string();
                if name.is_empty() || m.busy {
                    return Task::none();
                }
                m.busy = true;
                m.error = None;
                let api = ctx.api.clone();
                let path = format!("/playlists/{}/collaborators", state.id);
                return Task::perform(async move { api.post(&path, json!({ "username": name })).await }, Message::Invited);
            }
        }
        Message::Invited(r) => {
            if let Some(Dialog::Members(m)) = state.dialog.as_mut() {
                m.busy = false;
                match r {
                    Ok(v) => {
                        ctx.toast_ok(format!("Invited {}", m.username.trim()));
                        m.username.clear();
                        m.data = Some(v);
                        return load(state, ctx);
                    }
                    Err(e) => m.error = Some(e.message),
                }
            }
        }
        Message::MakeInviteLink => {
            let api = ctx.api.clone();
            let path = format!("/playlists/{}/invite-link", state.id);
            return Task::perform(async move { api.post_empty(&path).await }, Message::MembersChanged);
        }
        Message::CopyInviteLink => {
            if let Some(Dialog::Members(m)) = state.dialog.as_ref() {
                if let Some(link) = m.data.as_ref().map(|d| &d["invite_link"]).filter(|l| l.is_object()) {
                    let url = fmt::s(link, "url");
                    ctx.copy_text(if url.starts_with("http") { url } else { format!("{}{}", ctx.base(), fmt::s(link, "path")) });
                }
            }
        }
        Message::DropInviteLink => {
            let api = ctx.api.clone();
            let path = format!("/playlists/{}/invite-link", state.id);
            return Task::perform(async move { api.del(&path).await }, Message::MembersChanged);
        }
        Message::RemoveMember(uid) => {
            let api = ctx.api.clone();
            let path = format!("/playlists/{}/collaborators/{uid}", state.id);
            return Task::perform(async move { api.del(&path).await }, Message::MembersChanged);
        }
        Message::MembersChanged(r) => {
            if let Err(e) = r {
                ctx.toast_err(e.message);
            }
            return Task::batch([load(state, ctx), load_members(state, ctx)]);
        }
        Message::Leave => {
            if let Some(p) = state.playlist() {
                state.dialog = Some(Dialog::Leave(format!("Leave “{}”? {LEAVE_TEXT}", fmt::s(p, "name"))));
            }
        }
        Message::LeaveConfirmed => {
            state.dialog = None;
            let api = ctx.api.clone();
            let path = format!("/playlists/{}", state.id);
            return Task::perform(async move { api.del(&path).await }, Message::Left);
        }
        Message::Left(Ok(_)) => {
            ctx.toast_ok("You left the playlist");
            ctx.navigate(Page::Playlists);
        }
        Message::Left(Err(e)) => ctx.toast_err(e.message),
    }
    Task::none()
}

fn load_members(state: &State, ctx: &Ctx) -> Task<Message> {
    let api = ctx.api.clone();
    let path = format!("/playlists/{}/collaborators", state.id);
    Task::perform(async move { api.get(&path).await }, Message::MembersLoaded)
}

/// The line under the header: the owner's Collaborative switch with Invite, or a member's badge, owner and
/// Members. None for Liked Songs.
fn collab_line<'a>(p: &'a Value) -> Option<El<'a, Message>> {
    if fmt::s(p, "kind") != "playlist" {
        return None;
    }
    if is_member(p) {
        return Some(
            row![
                badge("Collaborative", Badge::Accent),
                small_muted(format!("by {} · {MEMBER_HINT}", fmt::s(&p["collab"], "owner"))),
                btn_icon(Icon::User, "Members", Btn::Default, Some(Message::OpenMembers)),
            ]
            .spacing(8)
            .align_y(Alignment::Center)
            .wrap()
            .into(),
        );
    }
    let on = fmt::truthy(p, "collaborative");
    let mut r = row![tooltip(
        toggler(on).label("Collaborative").text_size(13.5).spacing(8).on_toggle(Message::Collaborative).style(theme::switch),
        widgets::tip_box("Let people you invite add, remove and reorder songs"),
        tooltip::Position::Bottom,
    )
    .gap(4)]
    .spacing(10)
    .align_y(Alignment::Center);
    if on {
        r = r.push(small_muted(members_text(fmt::i(&p["collab"], "members"))));
        r = r.push(btn_icon(Icon::Plus, "Invite", Btn::Default, Some(Message::OpenMembers)));
    }
    Some(r.wrap().into())
}

/// The members window's body: invite by name, the invite link and the list for the owner; who's in it and
/// Leave for a member.
fn members_dialog<'a>(m: &'a Members) -> El<'a, Message> {
    let Some(d) = &m.data else {
        return match &m.error {
            Some(e) => error_box(e),
            None => muted("Loading…").size(14).into(),
        };
    };
    let owner = fmt::s(d, "role") == "owner";
    let mut col = column![].spacing(12);
    if owner {
        col = col.push(small_muted("People you invite can add, remove and reorder songs. Only you can rename, share or delete the playlist."));
        col = col.push(
            row![
                text_input("User name on this server", &m.username)
                    .on_input(Message::InviteName)
                    .on_submit(Message::Invite)
                    .padding([9, 12])
                    .size(14)
                    .style(theme::input),
                btn_icon(Icon::Plus, "Invite", Btn::Primary, (!m.busy && !m.username.trim().is_empty()).then_some(Message::Invite)),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        );
        if let Some(e) = &m.error {
            col = col.push(error_box(e));
        }
        let link = &d["invite_link"];
        col = col.push(if link.is_object() {
            row![
                small_muted(fmt::s(link, "url")),
                btn_icon(Icon::Copy, "Copy link", Btn::Default, Some(Message::CopyInviteLink)),
                btn_sm("Turn off link", Btn::Ghost, Some(Message::DropInviteLink)),
            ]
            .spacing(8)
            .align_y(Alignment::Center)
            .wrap()
        } else {
            row![
                btn_icon(Icon::Link, "Create invite link", Btn::Default, Some(Message::MakeInviteLink)),
                small_muted("Anyone on this server with the link can join."),
            ]
            .spacing(8)
            .align_y(Alignment::Center)
            .wrap()
        });
    } else {
        col = col.push(small_muted(format!("{MEMBER_HINT}. Only {} can rename, share or delete it.", fmt::s(d, "owner"))));
    }
    col = col.push(t("Members").size(14).font(theme::bold()));
    col = col.push(small_muted(format!("Owner: {}", fmt::s(d, "owner"))));
    let members = fmt::items(&d["members"]);
    if members.is_empty() {
        col = col.push(small("Nobody yet.").color(theme::look().faint));
    }
    for p in members {
        let mut r = row![t(fmt::s(&p, "username")).size(14)].spacing(8).align_y(Alignment::Center);
        if fmt::s(&p, "status") == "invited" {
            r = r.push(badge("invited", Badge::Plain));
        }
        let mut line = row![container(r).width(Length::Fill)].align_y(Alignment::Center);
        if owner {
            line = line.push(icon_btn(Icon::X, "Remove", Some(Message::RemoveMember(fmt::i(&p, "user_id")))));
        }
        col = col.push(line);
    }
    if !owner {
        col = col.push(btn_icon(Icon::X, "Leave playlist", Btn::Ghost, Some(Message::Leave)));
    }
    col.into()
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
    } else if let Some(why) = cant_request(it) {
        // "Download missing" skips it: say so, with the reason on hover
        vec![iced::widget::tooltip(badge("can't request", Badge::Err), crate::widgets::tip_box(why.to_string()), iced::widget::tooltip::Position::Top).gap(4).into()]
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
    // someone else's collaborative playlist: its songs are the user's to change, the rest is the owner's
    let member = is_member(p);
    let items = state.items();
    let missing = items.iter().filter(|i| requestable(i)).count();
    let (total, matched) = (fmt::f(p, "total"), fmt::f(p, "matched"));
    let has_matched = matched > 0.0;
    let cover: El<'a, Message> = if liked {
        heart_art(180.0, ctx)
    } else {
        catalog::art(ctx, &fmt::s(p, "cover_url"), 180.0, false, Icon::List)
    };
    let kicker = if member {
        format!("Collaborative playlist · by {}", fmt::s(&p["collab"], "owner"))
    } else {
        format!("{} · {}", if liked { "Liked songs · synced everywhere" } else { "Playlist" }, src_label(p))
    };
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
            if !liked && !member {
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
        actions = actions.push(btn_icon(Icon::Shuffle, "Shuffle", Btn::Default, Some(Message::Shuffle)));
        actions = actions.push(btn_icon(Icon::Download, "Save for offline", Btn::Default, Some(Message::SaveOffline)));
    }
    // 🔍 whenever there are songs to search
    if !items.is_empty() {
        actions = actions.push(icon_btn(Icon::Search, SEARCH_TIP, Some(Message::ToggleSearch)));
    }
    if !member {
        actions = actions.push(
            tooltip(
                btn_icon(Icon::External, "Share", Btn::Default, Some(Message::Share)),
                widgets::tip_box("Share link"),
                tooltip::Position::Bottom,
            )
            .gap(4),
        );
    }
    if missing > 0 && !member {
        actions = actions.push(btn_icon(
            Icon::Download,
            format!("Download {missing} missing"),
            if has_matched { Btn::Default } else { Btn::Primary },
            (!state.busy).then_some(Message::DownloadMissing),
        ));
    }
    // the songs "Download missing" can't ask for, listed to copy and fix by hand
    let cant = items.iter().filter(|i| cant_request(i).is_some()).count();
    if cant > 0 && !member {
        actions = actions.push(btn_icon(Icon::List, format!("{cant} can't be requested"), Btn::Default, Some(Message::CantRequest)));
    }
    actions = actions.push(btn_icon(Icon::Plus, "Add from link", Btn::Default, Some(Message::AddFromLink)));
    if !liked && !member && !items.is_empty() {
        actions = actions.push(btn_icon(Icon::List, "Merge into…", Btn::Default, Some(Message::OpenMergeInto)));
    }
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
    if fmt::truthy(p, "source_url") && !member {
        let src = fmt::s(p, "source");
        actions = actions.push(btn_icon(
            Icon::Refresh,
            format!("Pull from {}", src_name(&src).unwrap_or("source")),
            Btn::Default,
            (!state.busy).then_some(Message::Pull),
        ));
    }
    if member {
        actions = actions.push(btn_icon(Icon::X, "Leave", Btn::Ghost, Some(Message::Leave)));
    } else if !liked {
        actions = actions.push(icon_btn(Icon::Trash, "Delete playlist", Some(Message::Delete)));
    }
    let mut right = column![catalog::kicker(kicker), title, counts, progress].spacing(8).width(Length::Fill);
    if shared_path(p).is_some() {
        right = right.push(
            row![
                badge("Shared", Badge::Accent),
                small_muted(shared_hint(p)),
                btn_icon(Icon::Copy, "Copy link", Btn::Default, Some(Message::CopyLink)),
                btn_icon(Icon::X, STOP_TITLE, Btn::Ghost, Some(Message::StopSharing)),
            ]
            .spacing(8)
            .align_y(Alignment::Center)
            .wrap(),
        );
    }
    if let Some(line) = collab_line(p) {
        right = right.push(line);
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

/// The head of someone else's playlist: its picture, "Playlist · by anna", the name (no rename),
/// "12 tracks" and the owner's hint, then Play, Save for offline, Save a copy and Remove from my
/// playlists. Once the link ended: the "stopped sharing" line and Remove only.
fn linked_header<'a>(state: &'a State, p: &'a Value, ctx: &Ctx<'a>) -> El<'a, Message> {
    let owner = linked_owner(p);
    let active = linked_active(p);
    let cover = catalog::art(ctx, if active { p["cover_url"].as_str().unwrap_or("") } else { "" }, 180.0, false, Icon::List);
    let mut right = column![catalog::kicker(linked_kicker(p)), h1(fmt::s(p, "name"))].spacing(8).width(Length::Fill);
    let mut actions = row![].spacing(8).align_y(Alignment::Center);
    if active {
        right = right.push(muted(format!("{} tracks", fmt::i(p, "total"))).size(14));
        right = right.push(small_muted(linked_hint(&owner)));
        if !playable(&state.items()).is_empty() {
            actions = actions.push(btn_icon(Icon::Play, "Play", Btn::Primary, Some(Message::Play(0))));
            actions = actions.push(btn_icon(Icon::Shuffle, "Shuffle", Btn::Default, Some(Message::Shuffle)));
            actions = actions.push(btn_icon(Icon::Download, "Save for offline", Btn::Default, Some(Message::SaveOffline)));
        }
        if !state.items().is_empty() {
            actions = actions.push(icon_btn(Icon::Search, SEARCH_TIP, Some(Message::ToggleSearch)));
        }
        actions = actions.push(btn_icon(Icon::Copy, SAVE_COPY, Btn::Default, (!state.busy).then_some(Message::SaveCopy)));
    } else {
        right = right.push(muted(linked_stopped(&owner)).size(14));
    }
    actions = actions.push(btn_icon(Icon::X, REMOVE_FROM_MINE, Btn::Ghost, Some(Message::RemoveLinked)));
    let right = right.push(container(actions.wrap()).padding([6, 0]));
    row![cover, right].spacing(24).align_y(Alignment::End).into()
}

/// The song search above the rows while it is open: the box, and the found line under it
/// ("12 of 5263 songs", or that nothing matches). None while it is closed.
fn search_bar<'a>(state: &'a State, items: &[Value]) -> Option<El<'a, Message>> {
    let q = state.search.as_deref()?;
    let shown = items.iter().filter(|it| query_matches(&item_haystack(it), q)).count();
    let found = found_text(shown, items.len(), q, ("song", "songs"));
    let mut col = column![container(search_box_clearable(
        SEARCH_ID,
        SONG_SEARCH_PLACEHOLDER,
        q,
        Message::SearchInput,
        Message::SearchClear
    ))
    .max_width(420)]
    .spacing(8);
    if !found.is_empty() {
        col = col.push(small_muted(found));
    }
    Some(col.into())
}

fn track_rows<'a>(state: &'a State, items: &[Value], ctx: &Ctx<'a>) -> El<'a, Message> {
    // someone else's playlist: every song is in the library, and only its owner takes songs out
    let read_only = state.playlist().is_some_and(is_linked);
    let current = ctx.player.current_id();
    let query = state.search.as_deref().unwrap_or("");
    let mut play_index = 0usize;
    let mut rows: Vec<El<'a, Message>> = Vec::with_capacity(items.len());
    for (i, it) in items.iter().enumerate() {
        // a row the search hides still counts for the play index, so a found song plays the
        // whole playlist from its own place
        if !query_matches(&item_haystack(it), query) {
            if fmt::truthy(it, "track_id") {
                play_index += 1;
            }
            continue;
        }
        let iid = fmt::i(it, "id");
        let in_library = fmt::truthy(it, "track_id");
        let tail = if fmt::truthy(it, "album") { format!(" · {}", fmt::s(it, "album")) } else { String::new() };
        let artists = crate::widgets::catalog::credit_parts(it, &fmt::s(it, "artist")).into_iter().map(|(x, p)| (x, p.map(Message::OpenPage))).collect();
        let mut r = TrackRow::new(fmt::s(it, "title"))
            .index(i + 1)
            .sub_links(artists, tail)
            .duration(Some(fmt::f(it, "duration")).filter(|d| *d > 0.0))
            .playing(in_library && current == Some(fmt::i(it, "track_id")))
            .on_menu(Message::Menu(iid));
        if !read_only {
            r = r.extra(icon_btn(Icon::X, "Remove", Some(Message::RemoveItem(iid))));
        }
        if in_library {
            r = r.quality(fmt::quality(it)).on_play(Message::Play(play_index));
            play_index += 1;
        }
        if let Some(m) = it["request_message"].as_str().filter(|m| !m.is_empty() && !in_library) {
            r = r.note(m.to_string());
        }
        if !read_only {
            for b in status_badges(it) {
                r = r.badge(b);
            }
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
        Some(Ok(p)) if is_linked(p) => {
            let items = state.items();
            let mut col = column![linked_header(state, p, ctx)].spacing(18);
            if !linked_active(p) {
                // the link ended: the head says so, there is nothing to list
            } else if items.is_empty() {
                col = col.push(empty(Icon::Music, LINKED_EMPTY, ""));
            } else {
                if let Some(bar) = search_bar(state, &items) {
                    col = col.push(bar);
                }
                col = col.push(track_rows(state, &items, ctx));
            }
            col.width(Length::Fill).into()
        }
        Some(Ok(p)) => {
            let items = state.items();
            let mut col = column![header(state, p, ctx)].spacing(18);
            if !is_member(p) {
                col = col.push(sync_card(state, p, ctx));
            }
            if items.is_empty() {
                col = col.push(empty(
                    Icon::Music,
                    "Empty playlist",
                    "Add tracks from an album page or from a link.",
                ));
            } else {
                if let Some(bar) = search_bar(state, &items) {
                    col = col.push(bar);
                }
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
        // who added the song (0.36): its first line once the server said
        if let Some((tid, text)) = &state.added {
            if *tid == fmt::i(&it, "track_id") && !text.is_empty() {
                items.push(MenuItem::info(text.clone()));
            }
        }
        if fmt::truthy(&it, "track_id") {
            items.push(MenuItem::new("Play next", Message::PlayNext(iid)).icon(Icon::Play));
            items.push(MenuItem::new("Add to queue", Message::Enqueue(iid)).icon(Icon::Plus));
        }
        if fmt::truthy(&it, "album_id") {
            items.push(MenuItem::new("View album", Message::ViewAlbum(fmt::s(&it, "album_id"))).icon(Icon::Disc));
        }
        if !state.playlist().is_some_and(is_linked) {
            items.push(MenuItem::new("Remove", Message::RemoveItem(iid)).icon(Icon::X).danger());
        }
        return Some(pop_menu(at, items, Message::CloseMenu, ctx.size));
    }
    match state.dialog.as_ref()? {
        Dialog::Import(imp) => Some(playlists::import_dialog(imp, Message::Import, Message::CloseDialog)),
        Dialog::MergeInto(m) => Some(playlists::merge_into_dialog(
            m,
            &state.playlist().map(|p| fmt::s(p, "name")).unwrap_or_default(),
            Message::MergeIntoPick,
            Message::MergeIntoDelete,
            Message::MergeIntoGo,
            Message::CloseDialog,
        )),
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
        Dialog::Members(m) => Some(modal(
            if is_member(state.playlist()?) { "Members" } else { "Invite collaborators" },
            members_dialog(m),
            Some(btn("Done", Btn::Default, Some(Message::CloseDialog)).into()),
            false,
            Message::CloseDialog,
        )),
        Dialog::Leave(text) => Some(confirm(
            "Leave playlist",
            text,
            "Leave",
            true,
            Message::LeaveConfirmed,
            Message::CloseDialog,
        )),
        Dialog::RemoveLinked(text) => Some(confirm(
            REMOVE_FROM_MINE,
            text,
            "Remove",
            true,
            Message::RemoveLinkedConfirmed,
            Message::CloseDialog,
        )),
    }
}

/// A .txt list dragged onto the window while *Add tracks from a link* is open.
pub fn subscription(state: &State) -> Subscription<Message> {
    match state.dialog {
        Some(Dialog::Import(_)) => playlists::import_events().map(Message::Import),
        _ => Subscription::none(),
    }
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
    fn the_owner_line_counts_who_added_it() {
        let shared = |n: i64| json!({ "share": { "token": "abc", "path": "/s/abc", "url": "u", "added_by": n } });
        assert_eq!(shared_hint(&shared(0)), "Anyone with the link can play these songs");
        assert_eq!(shared_hint(&shared(1)), "Anyone with the link can play these songs · added by 1 person");
        assert_eq!(shared_hint(&shared(3)), "Anyone with the link can play these songs · added by 3 people");
        assert_eq!(
            shared_hint(&json!({ "share": { "path": "/s/abc" } })),
            "Anyone with the link can play these songs",
            "an older server has no added_by"
        );
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

    /// The rows the server marks `cant_request` are the ones listed (in the library: never), as
    /// "Artist - Title (Album)", the title alone when there's no artist.
    #[test]
    fn the_songs_that_cant_be_requested_are_listed() {
        let why = "Track needs a MusicBrainz id or artist + title";
        assert_eq!(cant_request(&json!({ "title": "Song", "cant_request": why })), Some(why));
        assert_eq!(cant_request(&json!({ "title": "Song", "track_id": 4, "cant_request": why })), None);
        assert_eq!(cant_request(&json!({ "title": "Song", "artist": "A" })), None);
        let rows = [json!({ "title": "Lonely" }), json!({ "title": "Song", "artist": "Band", "album": "LP" }), json!({ "artist": "Only artist" })];
        let refs: Vec<&Value> = rows.iter().collect();
        assert_eq!(cant_request_list(&refs), "Lonely\nBand - Song (LP)\nOnly artist");
    }
}