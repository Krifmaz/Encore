//! A list of requests as Requests and the Downloader show it (web-rs/src/pages/common.rs
//! `request_row`, `bind_request_actions`, `clear_menu`, `retry_failed`): the rows with their
//! action buttons, the ⋯ menu, the Clear menu, the picker, and the requests they send. A page
//! holds a [`List`], routes `Msg` here and draws [`view`] and [`overlay`]. Package C owns this file.
//!
//! Also what a request keeps when the library can't be written (servers since 0.29): the
//! `waiting` status, Import again for a request whose downloaded files are kept, the storage
//! banner ([`storage_banner`], `GET /api/requests/storage`) with Check again for admins, and a
//! request's details in full with Copy ([`request_text`]).
//!
//! And what a request keeps when the audio check turns a download away (servers since R2): the
//! `rejected` downloads, with the reason in full on the row and in the details, and the row's
//! `anyway` – **Keep anyway** (import the kept file without the check) or **Download anyway**
//! (the file is gone: ask the same source for it again without the check), on the row and in
//! the details dialog ([`anyway`]).
//!
//! And **Download now** (Oct 21, the owner: "for the que can i get a download now button"): the
//! server says on each row whether it offers the button (`can_download_now`) and what it does
//! (`download_now.title`, the tooltip) – go first in the source's line, try a wait now, switch a
//! Soulseek download queued at a user to one with a free upload slot, or give up a torrent with no
//! progress for the next source; never past a source's limit or a ban. The answer's words are the
//! toast. Admins get **Download all now** on the Requests page ([`download_all_now_btn`]), and the
//! details name the line, the Soulseek queue, a download with no progress and the press
//! ([`download_now_facts`]).

use super::{
    badge, btn_sm, confirm, icon, icon_btn, picker, pop_menu, small_muted, t, tip_box, El, Icon, MenuItem,
};
use crate::api::ApiError;
use crate::fmt;
use super::details::Links;
use crate::pages::{Ctx, Page};
use crate::theme::{self, Badge, Btn};
use iced::widget::{button, column, container, image, progress_bar, row, tooltip};
use iced::{Alignment, ContentFit, Length, Point, Task};
use serde_json::{json, Map, Value};
use std::collections::HashSet;

/// The web's `active` set: a request the worker is on right now.
pub fn is_active(status: &str) -> bool {
    matches!(status, "searching" | "downloading" | "importing" | "monitoring")
}

/// Statuses the Cancel button applies to (the server's ACTIVE list and pending).
pub fn can_cancel(status: &str) -> bool {
    matches!(
        status,
        "pending" | "queued" | "searching" | "downloading" | "monitoring" | "importing" | "waiting"
    )
}

/// Import again (`POST /api/requests/{id}/import-again`): a failed or waiting request whose
/// downloaded files are kept (or a link's request that kept the files it could not import)
/// imports them without downloading anything again. It takes Retry's place on a failed row.
pub fn can_import_again(r: &Value) -> bool {
    fmt::truthy(r, "has_files") && matches!(r["status"].as_str(), Some("failed" | "waiting" | "completed"))
}

/// An import that hasn't moved for 15 minutes (or whose task is gone: `import_stalled` of the
/// list): the row offers Import again, which stops it and imports the kept files at once.
pub fn stalled_import(r: &Value) -> bool {
    r["status"].as_str() == Some("importing") && r["import_stalled"].as_bool() == Some(true)
}

/// The tooltip of Import again on a stalled import (the server's words, as the web and the phone).
pub const STALLED_IMPORT: &str = "The import hasn't moved for 15 minutes";

/// `body` with whose requests a bulk action takes (Clear, Clear waiting, Retry failed, Download
/// all now): `mine`, and `user` when the admin picked one person in the User list.
pub fn whose_body(mut body: Value, mine: bool, user: Option<i64>) -> Value {
    body["mine"] = json!(mine);
    if let Some(u) = user {
        body["user"] = json!(u);
    }
    body
}

/// Retry on a row (the phone's `canRetry`, the web's row): a playlist's songs anyone may queue
/// again; a failed or cancelled request only admins and members with Fix their own downloads (the
/// server answers others "Ask an admin to retry"). Import again takes its place when the
/// downloaded files are kept.
pub fn can_retry(r: &Value, admin: bool, fix: bool) -> bool {
    if can_import_again(r) {
        return false;
    }
    r["kind"].as_str() == Some("playlist")
        || (matches!(r["status"].as_str(), Some("failed" | "cancelled")) && may_fix(admin, fix))
}

/// What Cancel and Remove from list say first when a request keeps downloaded files (the web's
/// words): they go with it, and Import again is gone.
pub const KEPT_FILES_LOST: &str = "Its downloaded files have not been imported yet and can't be imported after this.";

/// Where a request's title goes in its details: the library album once it is there (a song's
/// album), else the catalogue's album page for an album asked for by its release group, else
/// Discover searching for it; None for an artist (its name is the link), a link or a playlist.
pub fn title_page(r: &Value) -> Option<Page> {
    let kind = fmt::s(r, "kind");
    if kind != "album" && kind != "track" {
        return None;
    }
    if fmt::truthy(r, "library_album_id") {
        return Some(Page::LibraryAlbum(fmt::s(r, "library_album_id")));
    }
    if kind == "album" && fmt::truthy(r, "mbid") {
        return Some(Page::Album(fmt::s(r, "mbid")));
    }
    let title = asked_title(r);
    if title.is_empty() {
        return None;
    }
    let q = [fmt::s(r, "artist"), title].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" ");
    Some(Page::Search { q })
}

/// The artist a request's details link to (None for a link or a playlist, which name none).
pub fn artist_page(r: &Value) -> Option<(String, Page)> {
    let artist = fmt::s(r, "artist");
    (!artist.is_empty() && !matches!(fmt::s(r, "kind").as_str(), "url" | "playlist"))
        .then(|| (artist.clone(), Page::ArtistName(artist)))
}

/// What Play in a request's details plays: a finished song request's library song, a finished
/// album request's library album; None while it isn't in the library.
pub fn play_path(r: &Value) -> Option<String> {
    if fmt::s(r, "status") != "completed" {
        return None;
    }
    let track = r["library_track_id"].as_i64().unwrap_or(0);
    if track > 0 {
        return Some(format!("/library/tracks/{track}"));
    }
    (fmt::s(r, "kind") == "album" && fmt::truthy(r, "library_album_id")).then(|| format!("/library/albums/{}", fmt::s(r, "library_album_id")))
}

/// The album or song a request asked for, without its artist.
fn asked_title(r: &Value) -> String {
    if fmt::s(r, "kind") == "album" { or(r, "title", "album") } else { fmt::s(r, "title") }
}

/// A request's details links (the owner on Oct 23: "from the download i should be able to click
/// on the artist song or album or play it if its downloaded").
pub fn links(r: &Value) -> Links {
    let title = if fmt::s(r, "kind") == "artist" { artist_page(r) } else { title_page(r).map(|p| (asked_title(r), p)) };
    let artist = if fmt::s(r, "kind") == "artist" { None } else { artist_page(r) };
    Links { title, artist, play: play_path(r) }
}

/// The songs a Play answer holds: an album's (`tracks`, each with the album's id) or the one song.
pub fn playable(v: &Value) -> Vec<Value> {
    if v.get("tracks").is_some() {
        let id = fmt::s(v, "id");
        return fmt::items(&v["tracks"])
            .into_iter()
            .map(|mut t| {
                if !fmt::truthy(&t, "album_id") && !id.is_empty() {
                    t["album_id"] = Value::String(id.clone());
                }
                t
            })
            .collect();
    }
    if v.get("id").is_some_and(|i| !i.is_null()) { vec![v.clone()] } else { vec![] }
}

/// The candidate being fetched, as the web's details say it: "from alice: Illuminate EP (FLAC,
/// 4 files)".
pub fn candidate_line(c: &Value) -> String {
    if !c.is_object() || (!fmt::truthy(c, "username") && !fmt::truthy(c, "directory")) {
        return String::new();
    }
    let mut extra = vec![];
    if fmt::truthy(c, "format") {
        extra.push(fmt::s(c, "format").to_uppercase());
    }
    let n = fmt::i(c, "files");
    if n > 0 {
        extra.push(format!("{n} file{}", if n == 1 { "" } else { "s" }));
    }
    let mut what = fmt::s(c, "directory");
    if !extra.is_empty() {
        what = format!("{what} ({})", extra.join(", ")).trim().to_string();
    }
    if fmt::truthy(c, "username") {
        format!("from {}: {what}", fmt::s(c, "username"))
    } else {
        what
    }
}

/// What a request asked for in one line, the details dialog's title (the web's `request_what`):
/// "Illuminate EP – Duumu", "Duumu – full discography", the link.
pub fn request_what(r: &Value) -> String {
    let kind = fmt::s(r, "kind");
    let title = match kind.as_str() {
        "album" => or(r, "title", "album"),
        "url" => or(r, "title", "url"),
        "artist" => String::new(),
        _ => fmt::s(r, "title"),
    };
    let artist = fmt::s(r, "artist");
    match (title.is_empty(), artist.is_empty()) {
        (false, false) => format!("{title} – {artist}"),
        (false, true) => title,
        (true, false) if kind == "artist" => format!("{artist} – full discography"),
        (true, false) => artist,
        _ => String::new(),
    }
}

/// What a request offers after the audio check turned a download away (the row's `anyway`):
/// Keep anyway or Download anyway, the rejected download it acts on, and the reason in full.
#[derive(Clone, Debug, PartialEq)]
pub struct Anyway {
    /// Keep anyway (the kept file is there); false: Download anyway (it is gone).
    pub keep: bool,
    /// `rejected[].id`, sent as `{"rejected": id}`.
    pub id: i64,
    /// "The audio sounds like Diase – Sun, not Moonlight Shadow".
    pub text: String,
}

impl Anyway {
    /// The button's words.
    pub fn label(&self) -> &'static str {
        if self.keep {
            "Keep anyway"
        } else {
            "Download anyway"
        }
    }

    /// The button's tooltip.
    pub fn tip(&self) -> &'static str {
        if self.keep {
            "Import the download the audio check turned away, without the check"
        } else {
            "Download it again from the same place, without the audio check"
        }
    }

    /// `POST /api/requests/{id}/keep-anyway` or `…/download-anyway`.
    pub fn path(&self, request: i64) -> String {
        format!("/requests/{request}/{}", if self.keep { "keep-anyway" } else { "download-anyway" })
    }

    /// `{"rejected": id}`, so the server acts on the download the page showed (`{}`: its newest).
    pub fn body(&self) -> Value {
        if self.id > 0 {
            json!({ "rejected": self.id })
        } else {
            json!({})
        }
    }

    /// The toast once the server took it.
    pub fn done(&self) -> &'static str {
        if self.keep {
            "It will be imported without the audio check"
        } else {
            "Downloading it again without the audio check"
        }
    }

    /// The details dialog's button.
    pub fn act(&self, request: i64) -> super::details::Act {
        super::details::Act {
            label: self.label().into(),
            path: self.path(request),
            body: self.body(),
            done: self.done().into(),
        }
    }
}

/// The row's `anyway` (null, or from an older server none: nothing to offer).
pub fn anyway(r: &Value) -> Option<Anyway> {
    let a = &r["anyway"];
    let keep = match a["action"].as_str()? {
        "keep" => true,
        "download" => false,
        _ => return None,
    };
    let mut text = fmt::s(a, "text");
    if text.trim().is_empty() {
        // the server's sentence; else the rejected download's own
        text = rejected_of(r, &a["id"])
            .map(|x| if fmt::truthy(x, "text") { fmt::s(x, "text") } else { fmt::s(x, "reason") })
            .unwrap_or_default();
    }
    Some(Anyway { keep, id: fmt::i(a, "id"), text: text.trim().to_string() })
}

/// The rejected download with this id.
fn rejected_of<'r>(r: &'r Value, id: &Value) -> Option<&'r Value> {
    r["rejected"].as_array().and_then(|l| l.iter().find(|x| &x["id"] == id))
}

/// Who may press Keep anyway and Download anyway (the server's rule, as for Retry): admins, and
/// a member with the `fix` permission on their own requests. Others see the reason without the
/// button (the server would only answer "Ask an admin to keep it anyway"). The same people get
/// Retry, Retry failed and the picker's "Don't check the audio".
pub fn may_fix(admin: bool, fix: bool) -> bool {
    admin || fix
}

/// Where a rejected download came from, in one line: "Deezer · via streamrip ·
/// https://www.deezer.com/track/1 · 01. Moonlight Shadow.flac" (an album: "… and 9 more").
pub fn rejected_from(x: &Value) -> String {
    let n = fmt::i(x, "count");
    let files = if n > 1 && fmt::truthy(x, "file") {
        format!("{} and {} more", fmt::s(x, "file"), n - 1)
    } else {
        fmt::s(x, "file")
    };
    [fmt::s(x, "via"), fmt::s(x, "detail"), fmt::s(x, "url"), files]
        .into_iter()
        .filter(|p| !p.trim().is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
}

/// The top of a request's details when the audio check turned a download away: why, in full,
/// and where it came from.
pub fn anyway_lead(r: &Value, a: &Anyway) -> super::details::Lead {
    super::details::Lead {
        text: a.text.clone(),
        from: rejected_of(r, &json!(a.id)).map(rejected_from).unwrap_or_default(),
    }
}

/// The reason in full on a row's line of its own (warn colour, wrapped: never cut).
pub fn anyway_reason<'a, M: 'a>(a: &Anyway) -> El<'a, M> {
    t(a.text.clone())
        .size(12)
        .color(theme::look().warn)
        .wrapping(iced::widget::text::Wrapping::WordOrGlyph)
        .into()
}

/// The row's Keep anyway / Download anyway button with its tooltip (`on` None while busy).
pub fn anyway_btn<'a, M: Clone + 'a>(a: &Anyway, on: Option<M>) -> El<'a, M> {
    let p = theme::look();
    tooltip(
        button(
            row![
                icon(if a.keep { Icon::Check } else { Icon::Download }, 14.0, p.text),
                t(a.label()).size(13).font(theme::semibold()),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
        )
        .padding([6, 11])
        .style(theme::btn(Btn::Default))
        .on_press_maybe(on),
        tip_box(a.tip()),
        tooltip::Position::Bottom,
    )
    .gap(4)
    .into()
}

/// Whether a row shows its message: not when it is the reason the offer shows in full on a line
/// of its own (a hand pick turned away has it as its message: it shows once).
pub fn shows_message(r: &Value, offer: Option<&Anyway>) -> bool {
    fmt::truthy(r, "message") && !offer.is_some_and(|a| a.text == fmt::s(r, "message").trim())
}

/// The lines of the details for the downloads the audio check turned away, oldest first: a
/// heading, then for each why in full, where from, the check's own words, when, and whether the
/// files are kept (the web's and the phone's words, so a copied text reads the same everywhere).
pub fn rejected_lines(r: &Value) -> Vec<String> {
    let list = r["rejected"].as_array().map(Vec::as_slice).unwrap_or(&[]);
    let mut out = vec![];
    if !list.is_empty() {
        out.push(String::new());
        out.push("Turned away by the audio check (oldest first):".into());
    }
    for x in list {
        let why = if fmt::truthy(x, "text") { fmt::s(x, "text") } else { fmt::s(x, "reason") };
        out.push(if why.trim().is_empty() { "The audio sounds like another song".to_string() } else { why });
        let mut sub = |k: &str, v: String| {
            if !v.trim().is_empty() {
                out.push(format!("  {k}: {v}"));
            }
        };
        sub("From", rejected_from(x));
        if fmt::truthy(x, "text") && fmt::truthy(x, "reason") {
            sub("Check", fmt::s(x, "reason"));
        }
        if fmt::f(x, "at") > 0.0 {
            sub("When", fmt::stamp(fmt::f(x, "at")));
        }
        sub(
            "Files",
            if !fmt::truthy(x, "has_file") {
                "gone".into()
            } else if fmt::truthy(x, "path") {
                format!("kept in {}", fmt::s(x, "path"))
            } else {
                // (other users than admins get no server folder)
                "kept on the server".into()
            },
        );
    }
    out
}

/// Download now: the server offers the button on this row (`can_download_now`; an older server
/// sends no such field: no button).
pub fn can_download_now(r: &Value) -> bool {
    r["can_download_now"].as_bool() == Some(true)
}

/// The server has Download now (its rows carry `can_download_now`, an older server's don't):
/// admins get Download all now.
pub fn has_download_now(rows: &[Value]) -> bool {
    rows.iter().any(|r| r.get("can_download_now").is_some_and(|v| !v.is_null()))
}

/// What Download now does on this row, in the server's words ("Go first in line for Deezer"): the
/// button's tooltip. "" when it isn't offered.
pub fn download_now_title(r: &Value) -> String {
    if can_download_now(r) {
        fmt::s(&r["download_now"], "title").trim().to_string()
    } else {
        String::new()
    }
}

/// What the toast says when the server answered a press without words.
pub const STARTING_NOW: &str = "Starting now";

/// Download all now with nothing to do (the server's words).
pub const NOTHING_WAITING: &str = "Nothing is waiting – every request is downloading or done";

/// The tooltip of the admin's Download all now.
pub const DOWNLOAD_ALL_NOW_HELP: &str =
    "Wake every waiting request and move requests up in their lines – never more at once than each source allows";

/// The toast after Download now: (good news, the server's words). `ok` false means nothing could
/// be sooner ("Soulseek asked Encore to wait – next attempt at 14:20"): a warning. None when such
/// an answer came without words.
pub fn download_now_toast(v: &Value) -> Option<(bool, String)> {
    let ok = v["ok"].as_bool() == Some(true);
    let text = fmt::s(v, "text").trim().to_string();
    if text.is_empty() {
        return ok.then(|| (true, STARTING_NOW.to_string()));
    }
    Some((ok, text))
}

/// The toast after Download all now: (good news, the server's words); a warning when nothing
/// was waiting.
pub fn download_all_now_toast(v: &Value) -> (bool, String) {
    let ok = v["ok"].as_bool() == Some(true);
    let text = fmt::s(v, "text").trim().to_string();
    if !text.is_empty() {
        return (ok, text);
    }
    (ok, if ok { STARTING_NOW } else { NOTHING_WAITING }.to_string())
}

/// The Download now lines of the details and Copy, as `(key, value)` – the web's and the phone's
/// words: "In line: 2nd in line for Deezer (1 download at a time)", "Soulseek queue: 12th in
/// alice's upload queue (since …)", "No progress since: …", "Download now: asked by carol at …"
/// (times are UTC, as the rest of the details).
pub fn download_now_facts(r: &Value) -> Vec<(&'static str, String)> {
    let mut out = vec![];
    let l = &r["line"];
    let text = fmt::s(l, "text").trim().to_string();
    if !text.is_empty() {
        let limit = fmt::s(l, "limit_text").trim().to_string();
        out.push(("In line", if limit.is_empty() { text } else { format!("{text} ({limit})") }));
    }
    let q = &r["remote_queue"];
    let text = fmt::s(q, "text").trim().to_string();
    if !text.is_empty() {
        let since = fmt::f(q, "since");
        out.push((
            "Soulseek queue",
            if since > 0.0 { format!("{text} (since {})", fmt::stamp(since)) } else { text },
        ));
    }
    if fmt::f(r, "stalled_since") > 0.0 {
        out.push(("No progress since", fmt::stamp(fmt::f(r, "stalled_since"))));
    }
    let p = &r["pressed"];
    let (by, at) = (fmt::s(p, "by").trim().to_string(), fmt::f(p, "at"));
    if !by.is_empty() || at > 0.0 {
        let mut said = "asked".to_string();
        if !by.is_empty() {
            said.push_str(&format!(" by {by}"));
        }
        if at > 0.0 {
            said.push_str(&format!(" at {}", fmt::stamp(at)));
        }
        out.push(("Download now", said));
    }
    out
}

/// The words for what a waiting request waits for (`wait_reason`), as the web says them.
pub fn wait_words(reason: &str) -> &'static str {
    match reason {
        "soulseek" => "Soulseek isn't connected yet",
        "storage_import" => "the downloaded files are kept and the import waits for the storage",
        "storage_download" => "the download waits for the staging folder",
        "waiting_disk" => "the downloads folder can't be written now",
        "plugin_install" => "its source's plugin is still being installed",
        _ => "",
    }
}

/// The status of a request waiting for a downloads folder (`wait_reason` "waiting_disk", servers
/// since 0.33), as the web's `WAITING_DISK`.
pub const WAITING_DISK: &str = "Waiting for the downloads folder";

/// A request row's badge: its status, and for a request waiting for a downloads folder that it
/// is (the web's `request_badge`).
pub fn row_status(r: &Value) -> (Badge, String) {
    let status = fmt::s(r, "status");
    if status == "waiting" && fmt::s(r, "wait_reason") == "waiting_disk" {
        return (Badge::Warn, WAITING_DISK.to_string());
    }
    super::catalog::status_words(&status)
}

/// The banner's sentence for the download folders (`downloads` of `GET /api/requests/storage`,
/// servers since 0.33): for admins, while a folder downloads are saved in can't be written.
pub fn downloads_text(s: &Value, admin: bool) -> Option<String> {
    let d = &s["downloads"];
    (admin && d["ok"].as_bool() == Some(false) && fmt::truthy(d, "text")).then(|| fmt::s(d, "text"))
}

/// The toast after Check again, from the answer and the state before it: (whether it is good news, the words).
/// Imports resumed only when they were paused; a downloads folder that can be written again says that, as the
/// web's banner does; a check that still runs (a folder that doesn't answer) says so.
pub fn check_toast(after: &Value, before: &Value) -> (bool, &'static str) {
    if after["ok"].as_bool() == Some(false) {
        (false, "Imports are still paused")
    } else if after["downloads"]["ok"].as_bool() == Some(false) {
        (false, "A downloads folder still can't be written")
    } else if after["downloads"]["checking"].as_bool() == Some(true) {
        (false, "Still checking the downloads folders")
    } else if before["ok"].as_bool() == Some(false) {
        (true, "Imports resumed")
    } else {
        (true, "The downloads folders can be written")
    }
}

/// Imports are paused or a downloads folder fails: the storage state is asked for at every load.
fn storage_failing(s: &Value) -> bool {
    s["ok"].as_bool() == Some(false) || s["downloads"]["ok"].as_bool() == Some(false)
}

/// A request in plain text for the details dialog and its Copy, laid out as the web's
/// `request_text` – what a bug report needs: title, artist, status and message, every attempt in
/// the log with its source and whole error, the failed step with its operation, path and errno,
/// streamrip's output, the request id and the server's version (and this app's). Times are UTC.
pub fn request_text(r: &Value, server_version: &str) -> String {
    let mut out: Vec<String> = vec![format!("Encore request #{}", fmt::s(r, "id"))];
    let mut add = |k: &str, v: String| {
        if !v.trim().is_empty() {
            out.push(format!("{k}: {v}"));
        }
    };
    let kind = fmt::s(r, "kind");
    add("Title", if kind == "album" { or(r, "title", "album") } else { fmt::s(r, "title") });
    add("Artist", fmt::s(r, "artist"));
    if kind == "track" && fmt::s(r, "album") != fmt::s(r, "title") {
        add("Album", fmt::s(r, "album"));
    }
    add("Link", fmt::s(r, "url"));
    add("Kind", kind);
    let status = fmt::s(r, "status");
    let (_, label) = super::catalog::status_words(&status);
    let why = if status == "waiting" { wait_words(&fmt::s(r, "wait_reason")) } else { "" };
    add("Status", if why.is_empty() { label } else { format!("{label} – {why}") });
    add("Message", fmt::s(r, "message"));
    if status == "waiting" && fmt::f(r, "next_try_at") > 0.0 {
        add("Next try", fmt::stamp(fmt::f(r, "next_try_at")));
    }
    // Download now: the line, the Soulseek queue, no progress, the press
    for (k, v) in download_now_facts(r) {
        add(k, v);
    }
    add("Attempts", fmt::s(r, "attempts"));
    add("Source", fmt::s(r, "source"));
    if fmt::truthy(r, "picked") {
        add("Picked by hand", "yes".into());
    }
    add("Downloading", candidate_line(&r["candidate"]));
    let e = &r["error"];
    if e.is_object() {
        add("Error", if fmt::truthy(e, "text") { fmt::s(e, "text") } else { fmt::s(e, "error") });
        add("Step", fmt::s(e, "step"));
        add("Operation", fmt::s(e, "op"));
        add("Path", fmt::s(e, "path"));
        add("Other path", fmt::s(e, "to"));
        add("Errno", fmt::s(e, "errno"));
        if fmt::truthy(e, "text") {
            add("System error", fmt::s(e, "error"));
        }
        if fmt::truthy(e, "storage") {
            add("Storage problem", "yes – not the source".into());
        }
    }
    if fmt::truthy(r, "has_files") {
        add(
            "Downloaded files kept in",
            // (other users than admins get no server folder)
            if fmt::truthy(r, "kept_path") { fmt::s(r, "kept_path") } else { "the server".into() },
        );
    }
    if fmt::truthy(r, "skip_check") {
        add("Audio check", "off for this request".into());
    }
    // the downloads the audio check turned away (R2), oldest first
    out.extend(rejected_lines(r));
    let log: Vec<String> = r["log"]
        .as_array()
        .map(|a| a.iter().map(|l| l.as_str().map(str::to_string).unwrap_or_else(|| l.to_string())).collect())
        .unwrap_or_default();
    let log: Vec<String> = log.into_iter().filter(|l| !l.trim().is_empty()).collect();
    if !log.is_empty() {
        out.push(String::new());
        out.push("Log (every attempt, oldest first):".into());
        out.extend(log);
    }
    let o = &r["output"];
    if o.is_object() && fmt::truthy(o, "text") {
        let head: Vec<String> = [
            fmt::s(o, "url"),
            (if fmt::truthy(o, "ok") { "produced files" } else { "failed" }).to_string(),
            if fmt::f(o, "at") > 0.0 { fmt::stamp(fmt::f(o, "at")) } else { String::new() },
        ]
        .into_iter()
        .filter(|x| !x.is_empty())
        .collect();
        out.push(String::new());
        let source = if fmt::truthy(o, "source") { fmt::s(o, "source") } else { "streamrip".to_string() };
        out.push(format!("{source} output ({}):", head.join(" · ")));
        out.push(fmt::s(o, "text").trim_end().to_string());
    }
    out.push(String::new());
    let mut add = |k: &str, v: String| {
        if !v.trim().is_empty() {
            out.push(format!("{k}: {v}"));
        }
    };
    add("Requested by", fmt::s(r, "username"));
    if fmt::f(r, "created_at") > 0.0 {
        add("Requested", fmt::stamp(fmt::f(r, "created_at")));
    }
    if fmt::f(r, "updated_at") > 0.0 {
        add("Updated", fmt::stamp(fmt::f(r, "updated_at")));
    }
    add("Request id", fmt::s(r, "id"));
    add("Server version", if server_version.is_empty() { "unknown".into() } else { server_version.to_string() });
    out.join("\n")
}

/// The storage banner's sentence (`GET /api/requests/storage`), or None when imports go on. The
/// server sends it finished on its own clock; the words here are for a server that sends none.
pub fn storage_text(s: &Value, admin: bool) -> Option<String> {
    if !s.is_object() || s["ok"].as_bool() != Some(false) {
        return None;
    }
    if fmt::truthy(s, "text") {
        return Some(fmt::s(s, "text"));
    }
    Some(if admin && fmt::truthy(s, "path") {
        format!(
            "Imports are paused: {} is not writable – {}: {}.",
            fmt::s(s, "path"),
            fmt::s(s, "op"),
            fmt::s(s, "error")
        )
    } else {
        "Imports are paused because of a storage problem on the server. Your downloads are kept and will be added when it is fixed.".to_string()
    })
}

/// `a.x || a.y`, as a string.
fn or(v: &Value, a: &str, b: &str) -> String {
    if fmt::truthy(v, a) {
        fmt::s(v, a)
    } else {
        fmt::s(v, b)
    }
}

/// The first line of a row: what was requested.
pub fn what(r: &Value) -> String {
    match fmt::s(r, "kind").as_str() {
        "album" => format!("{} · {}", or(r, "title", "album"), fmt::s(r, "artist")),
        "artist" => format!("{} · full discography", fmt::s(r, "artist")),
        "url" => or(r, "title", "url"),
        "playlist" => format!("{} · playlist import", fmt::s(r, "title")),
        _ => format!("{} · {}", fmt::s(r, "title"), fmt::s(r, "artist")),
    }
}

/// The picker's options for a request row: `{ request_id, kind, mbid, artist, title, album,
/// length }` with the fields the request has.
pub fn pick_opts(r: &Value) -> Value {
    let mut o = Map::new();
    if let Some(id) = r.get("id") {
        o.insert("request_id".into(), id.clone());
    }
    for k in ["kind", "mbid", "artist", "title", "album", "length"] {
        if let Some(v) = r.get(k) {
            o.insert(k.into(), v.clone());
        }
    }
    Value::Object(o)
}

/// Parents with their child requests (a child whose parent is not in the list is a parent).
pub fn tree(rows: &[Value]) -> Vec<(&Value, Vec<&Value>)> {
    rows.iter()
        .filter(|r| !fmt::truthy(r, "parent_id") || !rows.iter().any(|p| p["id"] == r["parent_id"]))
        .map(|r| (r, rows.iter().filter(|c| c["parent_id"] == r["id"]).collect()))
        .collect()
}

/// The question before deleting a request's files (`kept`: its downloaded files are kept for
/// Import again).
pub fn delete_files_text(status: &str, kept: bool) -> &'static str {
    if status == "completed" {
        "Permanently delete the songs this request downloaded from the library?"
    } else if kept {
        "Delete the downloaded files kept for this request? They can't be imported afterwards."
    } else {
        "Delete the leftover/partial files of this download?"
    }
}

/// The entries of the Clear menu of a download list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Clear {
    Completed,
    Failed,
    FailedFiles,
    CompletedFiles,
}

impl Clear {
    pub fn statuses(self) -> &'static [&'static str] {
        match self {
            Clear::Completed | Clear::CompletedFiles => &["completed"],
            Clear::Failed | Clear::FailedFiles => &["failed", "cancelled", "denied"],
        }
    }

    pub fn files(self) -> bool {
        matches!(self, Clear::FailedFiles | Clear::CompletedFiles)
    }

    /// The words of the confirm question.
    pub fn label(self) -> &'static str {
        match self {
            Clear::FailedFiles => "Failed & cancelled",
            Clear::CompletedFiles => "Completed downloads",
            _ => "",
        }
    }

    /// `POST /api/requests/clear` (`user`: one person's, from the admin's User list).
    pub fn body(self, mine: bool, user: Option<i64>) -> Value {
        whose_body(json!({ "statuses": self.statuses(), "files": self.files() }), mine, user)
    }
}

/// `clearMenu`: Clear completed, Clear failed & cancelled, and the admin's file-deleting pair.
pub fn clear_menu<M: Clone>(admin: bool, on: impl Fn(Clear) -> M) -> Vec<MenuItem<M>> {
    let mut items = vec![
        MenuItem::new("Clear completed", on(Clear::Completed)).icon(Icon::Check),
        MenuItem::new("Clear failed & cancelled", on(Clear::Failed)).icon(Icon::X),
    ];
    if admin {
        items.push(
            MenuItem::new("Clear failed & cancelled + delete their files", on(Clear::FailedFiles))
                .icon(Icon::Trash)
                .danger(),
        );
        items.push(
            MenuItem::new("Clear completed + delete the songs", on(Clear::CompletedFiles))
                .icon(Icon::Trash)
                .danger(),
        );
    }
    items
}

/// The ⋯ menu of a row: Details, and for a row that stopped moving Remove from list and the
/// admin's files entry.
pub fn row_menu<M: Clone>(
    admin: bool,
    status: &str,
    kept: bool,
    on_details: M,
    on_remove: impl Fn(bool) -> M,
) -> Vec<MenuItem<M>> {
    let mut items = vec![MenuItem::new("Details", on_details).icon(Icon::List)];
    if is_active(status) {
        return items;
    }
    items.push(MenuItem::new("Remove from list", on_remove(false)).icon(Icon::X));
    if admin {
        items.push(
            MenuItem::new(
                if status == "completed" || kept { "Delete downloaded files" } else { "Delete leftover files" },
                on_remove(true),
            )
            .icon(Icon::Trash)
            .danger(),
        );
    }
    items
}

/// "{removed} cleared · {n} file(s) deleted".
pub fn cleared_toast(r: &Value, files: bool) -> String {
    format!(
        "{} cleared{}",
        fmt::s(r, "removed"),
        if files { format!(" · {} file(s) deleted", fmt::s(r, "deleted_files")) } else { String::new() }
    )
}

/// The question before Clear waiting (`n` counted by the server's dry run).
pub fn waiting_question(n: i64) -> String {
    if n == 1 {
        "Cancel 1 waiting request? Downloads and imports already running keep going.".to_string()
    } else {
        format!("Cancel {n} waiting requests? Downloads and imports already running keep going.")
    }
}

/// What Clear waiting did, when the server's own words are missing.
pub fn waiting_done(n: i64) -> String {
    match n {
        0 => "Nothing is waiting".to_string(),
        1 => "Cancelled 1 waiting request".to_string(),
        n => format!("Cancelled {n} waiting requests"),
    }
}

/// The server's words of a Clear waiting answer (`{count, text}`), else ours.
pub fn waiting_text(r: &Value) -> String {
    match fmt::s(r, "text") {
        t if t.is_empty() => waiting_done(fmt::i(r, "count")),
        t => t,
    }
}

/// The words after Retry failed, and whether they are a warning.
pub fn retry_failed_toast(r: &Value) -> (String, bool) {
    let (n, skipped) = (fmt::i(r, "retried"), fmt::i(r, "skipped"));
    let msg = match (n, skipped) {
        (0, 0) => "No failed requests to retry".to_string(),
        (n, 0) => format!("Retrying {n} request{}", if n == 1 { "" } else { "s" }),
        (n, s) => format!(
            "Retrying {n} · {s} couldn't be retried{}",
            r["why"].as_str().map(|w| format!(" ({w})")).unwrap_or_default()
        ),
    };
    (msg, skipped > 0)
}

// ----- the list ---------------------------------------------------------------------------

#[derive(Debug)]
enum Menu {
    Row { id: i64, status: String, kept: bool },
    Clear,
}

#[derive(Clone, Debug)]
enum Then {
    /// Remove from list with the files (Delete downloaded files)
    Remove(i64),
    /// Remove from list, the kept files going with it
    RemoveKept(i64),
    /// Cancel, the kept files going with it
    Cancel(i64),
    Clear(Clear),
    /// Clear waiting: every request still waiting is cancelled.
    ClearWaiting,
}

#[derive(Debug)]
struct Confirm {
    title: &'static str,
    text: String,
    ok: &'static str,
    then: Then,
}

/// The rows and everything that acts on them.
#[derive(Debug, Default)]
pub struct List {
    /// What follows `/api/requests?`.
    pub query: String,
    /// Keep only these rows (the Downloader's url / track / album) …
    pub keep: Option<fn(&Value) -> bool>,
    /// … and at most this many (0 = all).
    pub take: usize,
    /// Rows show the admin's extras (who asked, Approve / Deny).
    pub admin: bool,
    /// Clear and Retry failed act on the user's own requests only.
    pub mine: bool,
    /// … or on this person's (the admin's User list on the Requests page; None: everyone's).
    pub user: Option<i64>,
    /// None while the first load runs; Err(message) when it failed.
    pub rows: Option<Result<Vec<Value>, String>>,
    /// Parents whose child requests are shown.
    pub expanded: HashSet<i64>,
    busy: HashSet<i64>,
    menu: Option<(Point, Menu)>,
    confirm: Option<Confirm>,
    picker: Option<picker::State>,
    gen: u64,
    retrying: bool,
    /// `GET /api/requests/storage`: whether imports are paused (null until it answered, and from a
    /// server without it).
    storage: Value,
    /// Check again is running.
    checking: bool,
    /// Loads since the page opened (the storage state goes with every eighth).
    storage_loads: u64,
    /// Download all now is running.
    downloading_all: bool,
    /// Clear waiting is counting or cancelling.
    clearing_waiting: bool,
}

#[derive(Clone, Debug)]
pub enum Msg {
    Loaded(u64, Result<Value, ApiError>),
    /// Show or hide a parent's child requests.
    Toggle(i64),
    /// approve, deny, retry or cancel one request.
    Act(i64, &'static str),
    Acted(i64, Result<Value, ApiError>),
    Pick(Value),
    Picker(picker::Message),
    Menu(i64, String),
    ClearMenu,
    CloseMenu,
    /// Remove a request, with its files when `true` (asks first).
    Remove(i64, bool),
    Clear(Clear),
    ConfirmOk,
    CloseDialog,
    Removed(bool, Result<Value, ApiError>),
    Cleared(bool, Result<Value, ApiError>),
    RetryFailed,
    RetriedFailed(Result<Value, ApiError>),
    /// Keep anyway or Download anyway on a request the audio check turned a download away for.
    Anyway(i64),
    Anywayed(i64, &'static str, Result<Value, ApiError>),
    /// Open a request's details (the whole message, the log, the failed step) with Copy.
    Details(i64),
    /// A click on a finished request's cover: play what it brought into the library (the
    /// details' Play, without opening them).
    Play(i64),
    /// The song or album [`Msg::Play`] asked the server for.
    PlayLoaded(Result<Value, ApiError>),
    /// Load the rows now (the details dialog's Keep anyway or Download anyway was sent).
    Reload,
    Storage(Result<Value, ApiError>),
    /// Check again (admins): probe the folders now.
    CheckStorage,
    Checked(Result<Value, ApiError>),
    /// Download now on a request the server offers it for.
    DownloadNow(i64),
    DownloadedNow(i64, Result<Value, ApiError>),
    /// Download all now (admins): wake every waiting request, move requests up in their lines.
    DownloadAllNow,
    DownloadedAllNow(Result<Value, ApiError>),
    /// Clear waiting: the server counts first (a dry run), then it is asked.
    ClearWaiting,
    WaitingCounted(Result<Value, ApiError>),
    ClearedWaiting(Result<Value, ApiError>),
}

impl List {
    /// Forget what is shown (the page is opened again).
    pub fn reset(&mut self) {
        self.rows = None;
        self.expanded.clear();
        self.busy.clear();
        self.menu = None;
        self.confirm = None;
        self.picker = None;
        self.retrying = false;
        self.storage = Value::Null;
        self.checking = false;
        self.storage_loads = 0;
        self.downloading_all = false;
        self.clearing_waiting = false;
    }

    pub fn rows(&self) -> &[Value] {
        match &self.rows {
            Some(Ok(r)) => r,
            _ => &[],
        }
    }

    /// Fetch the rows again (the first time, and every few seconds while the page is open), and
    /// beside them whether imports are paused: the first time, then every eighth time (about
    /// every 30 s, as the web) – and every time while they are paused, so the banner goes as soon
    /// as the rows show the import done.
    pub fn load(&mut self, ctx: &Ctx) -> Task<Msg> {
        self.gen += 1;
        let gen = self.gen;
        let api = ctx.api.clone();
        let url = format!("/requests?{}", self.query);
        let rows = Task::perform(async move { api.get(&url).await }, move |r| Msg::Loaded(gen, r));
        self.storage_loads += 1;
        if self.storage_loads % 8 != 1 && !storage_failing(&self.storage) {
            return rows;
        }
        let api = ctx.api.clone();
        Task::batch([rows, Task::perform(async move { api.get("/requests/storage").await }, Msg::Storage)])
    }

    fn status_of(&self, id: i64) -> String {
        self.rows()
            .iter()
            .find(|r| fmt::i(r, "id") == id)
            .map(|r| fmt::s(r, "status"))
            .unwrap_or_default()
    }

    fn row(&self, id: i64) -> Option<&Value> {
        self.rows().iter().find(|r| fmt::i(r, "id") == id)
    }

    fn kept(&self, id: i64) -> bool {
        self.rows().iter().any(|r| fmt::i(r, "id") == id && fmt::truthy(r, "has_files"))
    }
}

pub fn update(l: &mut List, msg: Msg, ctx: &mut Ctx) -> Task<Msg> {
    match msg {
        Msg::Loaded(gen, r) => {
            if gen != l.gen {
                return Task::none();
            }
            match r {
                Ok(v) => {
                    let mut rows = fmt::items(&v);
                    if let Some(keep) = l.keep {
                        rows.retain(keep);
                    }
                    if l.take > 0 {
                        rows.truncate(l.take);
                    }
                    l.rows = Some(Ok(rows));
                }
                Err(e) => {
                    if l.rows.is_none() {
                        l.rows = Some(Err(e.message));
                    } else {
                        log::warn!("requests: {}", e.message);
                    }
                }
            }
        }
        Msg::Toggle(id) => {
            if !l.expanded.remove(&id) {
                l.expanded.insert(id);
            }
        }
        Msg::Act(id, act) => {
            // the kept files go with a cancelled request: it asks first
            if act == "cancel" && l.kept(id) && !l.busy.contains(&id) {
                l.confirm = Some(Confirm { title: "Cancel this request?", text: KEPT_FILES_LOST.into(), ok: "Cancel request", then: Then::Cancel(id) });
                return Task::none();
            }
            if !l.busy.insert(id) {
                return Task::none();
            }
            let api = ctx.api.clone();
            let url = format!("/requests/{id}/{act}");
            return Task::perform(async move { api.post_empty(&url).await }, move |r| Msg::Acted(id, r));
        }
        Msg::Acted(id, r) => {
            l.busy.remove(&id);
            match r {
                Ok(_) => return l.load(ctx),
                Err(e) => ctx.toast_err(e.message),
            }
        }
        Msg::Anyway(id) => {
            let Some(a) = l.row(id).and_then(anyway) else {
                return Task::none();
            };
            if !l.busy.insert(id) {
                return Task::none();
            }
            let api = ctx.api.clone();
            let (path, body, done) = (a.path(id), a.body(), a.done());
            return Task::perform(async move { api.post(&path, body).await }, move |r| Msg::Anywayed(id, done, r));
        }
        Msg::Anywayed(id, done, r) => {
            l.busy.remove(&id);
            match r {
                Ok(_) => {
                    ctx.toast_ok(done);
                    return l.load(ctx);
                }
                // the server's words ("The rejected download is gone – Download anyway asks for it
                // again", "It is being imported now" …); the row then shows what it offers now
                Err(e) => {
                    ctx.toast_err(e.message);
                    return l.load(ctx);
                }
            }
        }
        Msg::Pick(r) => {
            let (st, task) = picker::open(pick_opts(&r), ctx);
            l.picker = Some(st);
            return task.map(Msg::Picker);
        }
        Msg::Picker(m) => {
            let Some(p) = l.picker.as_mut() else {
                return Task::none();
            };
            let mut tasks = vec![picker::update(p, m, ctx).map(Msg::Picker)];
            // `onDone: reload` – the list refreshes after a pick
            if p.picked.take().is_some() {
                tasks.push(l.load(ctx));
            }
            if l.picker.as_ref().is_some_and(|p| p.closed) {
                l.picker = None;
            }
            return Task::batch(tasks);
        }
        Msg::Menu(id, status) => {
            let kept = l.kept(id);
            l.menu = Some((ctx.cursor, Menu::Row { id, status, kept }));
        }
        Msg::ClearMenu => l.menu = Some((ctx.cursor, Menu::Clear)),
        Msg::CloseMenu => l.menu = None,
        Msg::Remove(id, files) => {
            l.menu = None;
            if files {
                l.confirm = Some(Confirm {
                    title: "Delete files",
                    text: delete_files_text(&l.status_of(id), l.kept(id)).to_string(),
                    ok: "Delete",
                    then: Then::Remove(id),
                });
                return Task::none();
            }
            if l.kept(id) {
                l.confirm = Some(Confirm { title: "Remove this request?", text: KEPT_FILES_LOST.into(), ok: "Remove", then: Then::RemoveKept(id) });
                return Task::none();
            }
            let api = ctx.api.clone();
            let url = format!("/requests/{id}");
            return Task::perform(async move { api.del(&url).await }, |r| Msg::Removed(false, r));
        }
        Msg::Clear(c) => {
            l.menu = None;
            if c.files() {
                l.confirm = Some(Confirm {
                    title: "Delete files",
                    text: format!("{}: this permanently deletes the files from disk.", c.label()),
                    ok: "Delete",
                    then: Then::Clear(c),
                });
                return Task::none();
            }
            let api = ctx.api.clone();
            let body = c.body(l.mine, l.user);
            return Task::perform(
                async move { api.post("/requests/clear", body).await },
                |r| Msg::Cleared(false, r),
            );
        }
        Msg::ConfirmOk => {
            let Some(c) = l.confirm.take() else {
                return Task::none();
            };
            let api = ctx.api.clone();
            return match c.then {
                Then::Remove(id) => {
                    let url = format!("/requests/{id}?files=true");
                    Task::perform(async move { api.del(&url).await }, |r| Msg::Removed(true, r))
                }
                Then::RemoveKept(id) => {
                    let url = format!("/requests/{id}");
                    Task::perform(async move { api.del(&url).await }, |r| Msg::Removed(false, r))
                }
                Then::Cancel(id) => {
                    if !l.busy.insert(id) {
                        return Task::none();
                    }
                    let url = format!("/requests/{id}/cancel");
                    Task::perform(async move { api.post_empty(&url).await }, move |r| Msg::Acted(id, r))
                }
                Then::Clear(k) => {
                    let body = k.body(l.mine, l.user);
                    Task::perform(
                        async move { api.post("/requests/clear", body).await },
                        |r| Msg::Cleared(true, r),
                    )
                }
                Then::ClearWaiting => {
                    l.clearing_waiting = true;
                    let body = whose_body(json!({}), l.mine, l.user);
                    Task::perform(
                        async move { api.post("/requests/clear-waiting", body).await },
                        Msg::ClearedWaiting,
                    )
                }
            };
        }
        // Clear waiting: admins everyone's (or the person picked), members their own (the server
        // decides); counted first for the question
        Msg::ClearWaiting => {
            if l.clearing_waiting {
                return Task::none();
            }
            l.clearing_waiting = true;
            let api = ctx.api.clone();
            let body = whose_body(json!({ "dry_run": true }), l.mine, l.user);
            return Task::perform(
                async move { api.post("/requests/clear-waiting", body).await },
                Msg::WaitingCounted,
            );
        }
        Msg::WaitingCounted(r) => {
            l.clearing_waiting = false;
            match r {
                Ok(v) if fmt::i(&v, "count") > 0 => {
                    l.confirm = Some(Confirm {
                        title: "Clear waiting",
                        text: waiting_question(fmt::i(&v, "count")),
                        ok: "Clear waiting",
                        then: Then::ClearWaiting,
                    });
                }
                Ok(v) => ctx.toast(waiting_text(&v)),
                Err(e) => ctx.toast_err(e.message),
            }
        }
        Msg::ClearedWaiting(r) => {
            l.clearing_waiting = false;
            match r {
                Ok(v) => {
                    ctx.toast_ok(waiting_text(&v));
                    return l.load(ctx);
                }
                Err(e) => ctx.toast_err(e.message),
            }
        }
        Msg::CloseDialog => l.confirm = None,
        Msg::Removed(files, r) => match r {
            Ok(v) => {
                if files {
                    ctx.toast_ok(format!("{} file(s) deleted", fmt::s(&v, "deleted_files")));
                }
                return l.load(ctx);
            }
            Err(e) => ctx.toast_err(e.message),
        },
        Msg::Cleared(files, r) => match r {
            Ok(v) => {
                ctx.toast_ok(cleared_toast(&v, files));
                return l.load(ctx);
            }
            Err(e) => ctx.toast_err(e.message),
        },
        Msg::RetryFailed => {
            if l.retrying {
                return Task::none();
            }
            l.retrying = true;
            let api = ctx.api.clone();
            let body = whose_body(json!({}), l.mine, l.user);
            return Task::perform(
                async move { api.post("/requests/retry-failed", body).await },
                Msg::RetriedFailed,
            );
        }
        Msg::RetriedFailed(r) => {
            l.retrying = false;
            match r {
                Ok(v) => {
                    let (msg, warn) = retry_failed_toast(&v);
                    if warn {
                        ctx.toast_warn(msg);
                    } else {
                        ctx.toast_ok(msg);
                    }
                    return l.load(ctx);
                }
                Err(e) => ctx.toast_err(e.message),
            }
        }
        Msg::Play(id) => {
            l.menu = None;
            // the row as the list has it now; a request no longer in the library plays nothing
            let Some(path) = l.row(id).and_then(play_path) else { return Task::none() };
            let api = ctx.api.clone();
            return Task::perform(async move { api.get(&path).await }, Msg::PlayLoaded);
        }
        Msg::PlayLoaded(r) => match r {
            Ok(v) => {
                // the song, or the album from its first song
                let tracks = playable(&v);
                if tracks.is_empty() {
                    ctx.toast_err("Nothing to play");
                } else {
                    ctx.play(tracks, 0);
                }
            }
            Err(e) => ctx.toast_err(e.message),
        },
        Msg::Details(id) => {
            l.menu = None;
            if let Some(r) = l.row(id) {
                let title = request_what(r);
                let text = request_text(r, &fmt::s(&ctx.session.info, "version"));
                // a download the audio check turned away: why, in full, at the top, and Keep anyway /
                // Download anyway beside Copy for whoever may press it
                let offer = anyway(r);
                let lead = offer.as_ref().map(|a| anyway_lead(r, a));
                let act = offer
                    .filter(|_| may_fix(ctx.is_admin(), ctx.session.perm("fix")))
                    .map(|a| a.act(id));
                let links = links(r);
                ctx.request_details(if title.is_empty() { "Details".to_string() } else { title }, text, lead, act, links);
            }
        }
        Msg::Reload => return l.load(ctx),
        Msg::Storage(Ok(v)) => l.storage = v,
        // an older server has no storage state (no banner); a failed look keeps the last one
        Msg::Storage(Err(_)) => {}
        Msg::CheckStorage => {
            if l.checking {
                return Task::none();
            }
            l.checking = true;
            let api = ctx.api.clone();
            return Task::perform(async move { api.post_empty("/requests/storage/check").await }, Msg::Checked);
        }
        Msg::Checked(r) => {
            l.checking = false;
            match r {
                // when the folders can be written again the waiting imports resume: show them move
                Ok(v) => {
                    match check_toast(&v, &l.storage) {
                        (true, words) => ctx.toast_ok(words),
                        (false, words) => ctx.toast_warn(words),
                    }
                    l.storage = v;
                    return l.load(ctx);
                }
                Err(e) => ctx.toast_err(e.message),
            }
        }
        Msg::DownloadNow(id) => {
            if !l.row(id).is_some_and(can_download_now) || !l.busy.insert(id) {
                return Task::none();
            }
            let api = ctx.api.clone();
            let url = format!("/requests/{id}/download-now");
            return Task::perform(async move { api.post_empty(&url).await }, move |r| Msg::DownloadedNow(id, r));
        }
        Msg::DownloadedNow(id, r) => {
            l.busy.remove(&id);
            match r {
                Ok(v) => match download_now_toast(&v) {
                    Some((true, words)) => ctx.toast_ok(words),
                    Some((false, words)) => ctx.toast_warn(words),
                    None => {}
                },
                // the server's words ("It is searching now", "Not your request" …); the row then
                // shows what it offers now
                Err(e) => ctx.toast_err(e.message),
            }
            return l.load(ctx);
        }
        Msg::DownloadAllNow => {
            if l.downloading_all {
                return Task::none();
            }
            l.downloading_all = true;
            let api = ctx.api.clone();
            let body = whose_body(json!({}), l.mine, l.user);
            return Task::perform(async move { api.post("/requests/download-now", body).await }, Msg::DownloadedAllNow);
        }
        Msg::DownloadedAllNow(r) => {
            l.downloading_all = false;
            match r {
                Ok(v) => match download_all_now_toast(&v) {
                    (true, words) => ctx.toast_ok(words),
                    (false, words) => ctx.toast_warn(words),
                },
                Err(e) => ctx.toast_err(e.message),
            }
            return l.load(ctx);
        }
    }
    Task::none()
}

// ----- view -------------------------------------------------------------------------------

/// `requestRow(r, { admin })`: the thumb, what was asked, the status line, the buttons. The thumb
/// of a finished request plays it; the words open the details.
pub fn request_row<'a>(ctx: &Ctx<'a>, l: &List, r: &Value) -> El<'a, Msg> {
    let p = ctx.look;
    let id = fmt::i(r, "id");
    let kind = fmt::s(r, "kind");
    let status = fmt::s(r, "status");
    let active = is_active(&status);
    let busy = l.busy.contains(&id);
    let thumb: El<'a, Msg> = match ctx.picture(&fmt::s(r, "cover_url")) {
        Some(h) => image(h)
            .width(40)
            .height(40)
            .content_fit(ContentFit::Cover)
            .border_radius(6.0)
            .into(),
        None => container(icon(Icon::Download, 16.0, p.faint))
            .width(40)
            .height(40)
            .center(40)
            .style(theme::art)
            .into(),
    };
    let mut line = row![badge(kind.clone(), Badge::Plain)].spacing(6).align_y(Alignment::Center);
    let attempts = fmt::i(r, "attempts");
    if attempts > 1 {
        line = line.push(
            tooltip(
                super::faint(format!("{attempts} attempts ·")).size(12),
                tip_box_owned(format!("Requested or retried {attempts} times")),
                tooltip::Position::Bottom,
            )
            .gap(4),
        );
    }
    // a download the audio check turned away: its reason in full on a line of its own (a hand
    // pick turned away has it as its message: it shows once)
    let offer = anyway(r);
    let message = shows_message(r, offer.as_ref());
    if fmt::truthy(r, "source") {
        // (the dot goes with the message that follows)
        line = line.push(super::faint(format!("via {}{}", fmt::s(r, "source"), if message { " ·" } else { "" })).size(12));
    }
    if message {
        line = line.push(small_muted(fmt::s(r, "message")));
    }
    if l.admin && fmt::truthy(r, "username") {
        line = line.push(super::faint(format!("· by {}", fmt::s(r, "username"))).size(12));
    }
    line = line.push(super::faint(format!("· {}", fmt::ago(fmt::f(r, "created_at")))).size(12));
    let mut lines = column![t(what(r)).size(14).font(theme::semibold()), line.wrap()].spacing(3);
    // a request with kept files says where they are (the files wait for Import again or the next
    // try) – to admins: the server's folders are theirs (the server sends members none)
    if l.admin && fmt::truthy(r, "has_files") && fmt::truthy(r, "kept_path") {
        lines = lines.push(
            super::faint(format!("Downloaded files kept in {}", fmt::s(r, "kept_path")))
                .size(12)
                .wrapping(iced::widget::text::Wrapping::WordOrGlyph),
        );
    }
    if let Some(a) = offer.as_ref().filter(|a| !a.text.is_empty()) {
        lines = lines.push(anyway_reason(a));
    }
    if active && fmt::truthy(r, "progress") {
        lines = lines.push(
            container(
                progress_bar(0.0..=100.0, fmt::f(r, "progress") as f32)
                    .girth(5)
                    .style(theme::progress),
            )
            .max_width(420),
        );
    }
    let (badge_kind, badge_words) = row_status(r);
    let mut right = row![super::badge(badge_words, badge_kind)].spacing(6).align_y(Alignment::Center);
    if l.admin && status == "pending" {
        right = right.push(
            button(
                row![icon(Icon::Check, 14.0, p.on_accent), t("Approve").size(13).font(theme::semibold())]
                    .spacing(6)
                    .align_y(Alignment::Center),
            )
            .padding([6, 11])
            .style(theme::btn(Btn::Primary))
            .on_press_maybe((!busy).then_some(Msg::Act(id, "approve"))),
        );
        right = right.push(btn_sm("Deny", Btn::Danger, (!busy).then_some(Msg::Act(id, "deny"))));
    }
    if matches!(kind.as_str(), "album" | "track") && status != "completed" {
        right = right.push(
            tooltip(
                button(
                    row![icon(Icon::Search, 14.0, p.text), t("Pick").size(13).font(theme::semibold())]
                        .spacing(6)
                        .align_y(Alignment::Center),
                )
                .padding([6, 11])
                .style(theme::btn(Btn::Default))
                .on_press_maybe((!busy).then_some(Msg::Pick(r.clone()))),
                tip_box("Search the sources and pick the exact download"),
                tooltip::Position::Bottom,
            )
            .gap(4),
        );
    }
    if let Some(a) = offer.as_ref().filter(|_| may_fix(ctx.is_admin(), ctx.session.perm("fix"))) {
        // the audio check said it is another song: take it anyway (members need `fix`, as for Retry)
        right = right.push(anyway_btn(a, (!busy).then_some(Msg::Anyway(id))));
    }
    if can_import_again(r) {
        // the downloaded files are kept: import them, nothing is downloaded again
        right = right.push(
            tooltip(
                button(
                    row![icon(Icon::Refresh, 14.0, p.text), t("Import again").size(13).font(theme::semibold())]
                        .spacing(6)
                        .align_y(Alignment::Center),
                )
                .padding([6, 11])
                .style(theme::btn(Btn::Default))
                .on_press_maybe((!busy).then_some(Msg::Act(id, "import-again"))),
                tip_box("Import the downloaded files again – nothing is downloaded again"),
                tooltip::Position::Bottom,
            )
            .gap(4),
        );
    } else if stalled_import(r) {
        // an import that hasn't moved for 15 minutes: stopped, and the kept files imported at once
        right = right.push(
            tooltip(
                button(
                    row![icon(Icon::Refresh, 14.0, p.text), t("Import again").size(13).font(theme::semibold())]
                        .spacing(6)
                        .align_y(Alignment::Center),
                )
                .padding([6, 11])
                .style(theme::btn(Btn::Default))
                .on_press_maybe((!busy).then_some(Msg::Act(id, "import-again"))),
                tip_box(STALLED_IMPORT),
                tooltip::Position::Bottom,
            )
            .gap(4),
        );
    } else if can_retry(r, ctx.is_admin(), ctx.session.perm("fix")) {
        right = right.push(icon_btn(Icon::Refresh, "Retry", (!busy).then_some(Msg::Act(id, "retry"))));
    }
    if can_download_now(r) {
        // the server says what it does (go first in line, try the wait now, switch Soulseek users …)
        right = right.push(download_now_btn(download_now_title(r), (!busy).then_some(Msg::DownloadNow(id))));
    }
    if can_cancel(&status) {
        right = right.push(icon_btn(Icon::X, "Cancel", (!busy).then_some(Msg::Act(id, "cancel"))));
    }
    right = right.push(icon_btn(Icon::More, "More", Some(Msg::Menu(id, status.clone()))));
    // a click on the words opens the details (the whole message, the log, Copy)
    let words = iced::widget::mouse_area(lines.width(Length::Fill))
        .on_press(Msg::Details(id))
        .interaction(iced::mouse::Interaction::Pointer);
    // a click on a finished request's cover plays it; the words beside it still open the details
    let thumb: El<'a, Msg> = if play_path(r).is_some() {
        tooltip(
            iced::widget::mouse_area(thumb).on_press(Msg::Play(id)).interaction(iced::mouse::Interaction::Pointer),
            tip_box("Play"),
            tooltip::Position::Bottom,
        )
        .gap(4)
        .into()
    } else {
        thumb
    };
    let body = container(row![thumb, words, right].spacing(14).align_y(Alignment::Center)).padding([11, 16]);
    // a right-click opens the row's ⋯ menu at the pointer
    let body = iced::widget::mouse_area(body).on_right_press(Msg::Menu(id, status));
    column![body, super::divider()].into()
}

/// The banner over a request list while imports are paused (`GET /api/requests/storage`): for
/// admins the folder, the operation and the error with Check again; for everyone else that the
/// downloads are kept. Admins also read when a folder downloads are saved in can't be written
/// ([`downloads_text`]). None while imports go on and every folder can be written.
pub fn storage_banner<'a>(ctx: &Ctx<'a>, l: &List) -> Option<El<'a, Msg>> {
    let admin = ctx.is_admin();
    let texts: Vec<String> = [storage_text(&l.storage, admin), downloads_text(&l.storage, admin)].into_iter().flatten().collect();
    if texts.is_empty() {
        return None;
    }
    let text = texts.join("\n");
    let p = ctx.look;
    let mut r = row![
        icon(Icon::Download, 18.0, p.warn),
        t(text).size(13.5).width(Length::Fill).wrapping(iced::widget::text::Wrapping::WordOrGlyph),
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    if admin {
        r = r.push(btn_sm(
            if l.checking { "Checking…" } else { "Check again" },
            Btn::Default,
            (!l.checking).then_some(Msg::CheckStorage),
        ));
    }
    Some(container(r).padding([12, 16]).width(Length::Fill).style(theme::warn_box).into())
}

/// The row's Download now, with what it does as its tooltip (`on` None while busy).
pub fn download_now_btn<'a, M: Clone + 'a>(title: String, on: Option<M>) -> El<'a, M> {
    let p = theme::look();
    let b = button(
        row![icon(Icon::Download, 14.0, p.text), t("Download now").size(13).font(theme::semibold())]
            .spacing(6)
            .align_y(Alignment::Center),
    )
    .padding([6, 11])
    .style(theme::btn(Btn::Default))
    .on_press_maybe(on);
    if title.is_empty() {
        return b.into();
    }
    tooltip(b, tip_box_owned(title), tooltip::Position::Bottom).gap(4).into()
}

/// A tooltip box with an owned text.
fn tip_box_owned<'a, M: 'a>(tip: String) -> El<'a, M> {
    container(t(tip).size(12))
        .padding([5, 9])
        .style(theme::menu_pop)
        .into()
}

/// The rows: parents, and under an expanded parent its children. None when the list loaded
/// and is empty (the page shows its own words); the caller wraps the rows in a card.
pub fn view<'a>(ctx: &Ctx<'a>, l: &'a List) -> Option<El<'a, Msg>> {
    let p = ctx.look;
    Some(match &l.rows {
        None => container(super::skeleton_rows(5)).padding(16).into(),
        Some(Err(e)) => container(super::error_box(e)).padding(14).into(),
        Some(Ok(rows)) if rows.is_empty() => return None,
        Some(Ok(rows)) => {
            let mut col = column![];
            for (parent, kids) in tree(rows) {
                col = col.push(request_row(ctx, l, parent));
                if !kids.is_empty() {
                    let id = fmt::i(parent, "id");
                    let open = l.expanded.contains(&id);
                    let available = kids.iter().filter(|k| fmt::s(k, "status") == "completed").count();
                    col = col.push(
                        container(
                            button(
                                row![
                                    icon(if open { Icon::Chevron } else { Icon::Plus }, 12.0, p.muted),
                                    small_muted(format!(
                                        "{} child request(s) · {available} available",
                                        kids.len()
                                    )),
                                ]
                                .spacing(6)
                                .align_y(Alignment::Center),
                            )
                            .padding([4, 8])
                            .style(theme::btn(Btn::Row))
                            .on_press(Msg::Toggle(id)),
                        )
                        .padding(super::pad(0.0, 16.0, 10.0, 70.0)),
                    );
                    if open {
                        let mut inner = column![];
                        for k in kids {
                            inner = inner.push(request_row(ctx, l, k));
                        }
                        col = col.push(container(inner).padding(super::pad(0.0, 0.0, 6.0, 48.0)));
                    }
                }
            }
            col.into()
        }
    })
}

/// "Retry failed" (every list has it, for those who may retry: [`may_fix`]).
pub fn retry_failed_btn<'a>(l: &List, tip: &'a str) -> El<'a, Msg> {
    tooltip(
        super::btn_icon(
            Icon::Refresh,
            "Retry failed",
            Btn::Default,
            (!l.retrying).then_some(Msg::RetryFailed),
        ),
        tip_box(tip),
        tooltip::Position::Bottom,
    )
    .gap(4)
    .into()
}

/// The admin's "Download all now" (the Requests page, beside Retry failed, on a server with
/// Download now: [`has_download_now`]).
pub fn download_all_now_btn<'a>(l: &List) -> El<'a, Msg> {
    tooltip(
        super::btn_icon(
            Icon::Download,
            "Download all now",
            Btn::Default,
            (!l.downloading_all).then_some(Msg::DownloadAllNow),
        ),
        tip_box(DOWNLOAD_ALL_NOW_HELP),
        tooltip::Position::Bottom,
    )
    .gap(4)
    .into()
}

/// "Clear waiting": cancel every request still waiting (pending, queued, waiting, searching);
/// downloads and imports already running keep going.
pub fn clear_waiting_btn<'a>(l: &List) -> El<'a, Msg> {
    tooltip(
        super::btn_icon(Icon::X, "Clear waiting", Btn::Default, (!l.clearing_waiting).then_some(Msg::ClearWaiting)),
        tip_box("Cancel every request that is still waiting"),
        tooltip::Position::Bottom,
    )
    .gap(4)
    .into()
}

/// The Clear button that opens the Clear menu ("Clear finished" or "Clear…").
pub fn clear_btn<'a>(label: &'a str) -> El<'a, Msg> {
    super::btn_icon(Icon::Trash, label, Btn::Default, Some(Msg::ClearMenu)).into()
}

/// The picker, a confirm or a menu, when one is open.
pub fn overlay<'a>(l: &'a List, ctx: &Ctx<'a>) -> Option<El<'a, Msg>> {
    if let Some(p) = &l.picker {
        return Some(picker::view(p, ctx).map(Msg::Picker));
    }
    if let Some(c) = &l.confirm {
        return Some(confirm(c.title, &c.text, c.ok, true, Msg::ConfirmOk, Msg::CloseDialog));
    }
    let (at, menu) = l.menu.as_ref()?;
    let admin = ctx.is_admin();
    let items = match menu {
        Menu::Row { id, status, kept } => {
            let id = *id;
            row_menu(admin, status, *kept, Msg::Details(id), move |files| Msg::Remove(id, files))
        }
        Menu::Clear => clear_menu(admin, Msg::Clear),
    };
    Some(pop_menu(*at, items, Msg::CloseMenu, ctx.size))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_details_link_the_album_the_artist_and_play_what_is_downloaded() {
        // the Love Buzz card of the Oct 23 screenshot: queued, asked for by its release group
        let queued = json!({ "kind": "album", "artist": "Nirvana", "title": "Love Buzz", "mbid": "rg-1", "status": "queued" });
        let l = links(&queued);
        assert_eq!(l.title, Some(("Love Buzz".into(), Page::Album("rg-1".into()))));
        assert_eq!(l.artist, Some(("Nirvana".into(), Page::ArtistName("Nirvana".into()))));
        assert_eq!(l.play, None);
        let done = json!({ "kind": "album", "artist": "Nirvana", "title": "Love Buzz", "status": "completed", "library_album_id": "abc", "library_track_id": null });
        assert_eq!(links(&done).title, Some(("Love Buzz".into(), Page::LibraryAlbum("abc".into()))));
        assert_eq!(links(&done).play.as_deref(), Some("/library/albums/abc"));
        let song = json!({ "kind": "track", "artist": "Nirvana", "title": "Blew", "status": "failed" });
        assert_eq!(title_page(&song), Some(Page::Search { q: "Nirvana Blew".into() }));
        let song_done = json!({ "kind": "track", "artist": "Nirvana", "title": "Blew", "status": "completed", "library_album_id": "x", "library_track_id": 12 });
        assert_eq!(links(&song_done).play.as_deref(), Some("/library/tracks/12"));
        assert_eq!(play_path(&json!({ "kind": "track", "status": "completed", "library_track_id": null })), None);
        let link = json!({ "kind": "url", "url": "https://example.com/a", "artist": "x", "status": "completed" });
        assert_eq!(links(&link), Links::default());
        // an album answer plays its songs with the album's id; a song answer plays the song
        assert_eq!(playable(&json!({ "id": "abc", "tracks": [{ "id": 1 }] })), vec![json!({ "id": 1, "album_id": "abc" })]);
        assert_eq!(playable(&json!({ "id": 12, "title": "Blew" })).len(), 1);
    }

    #[test]
    fn clear_waiting_asks_with_the_count_and_says_what_it_did() {
        assert_eq!(waiting_question(612), "Cancel 612 waiting requests? Downloads and imports already running keep going.");
        assert_eq!(waiting_question(1), "Cancel 1 waiting request? Downloads and imports already running keep going.");
        // the server's words first
        assert_eq!(waiting_text(&json!({ "count": 612, "text": "Cancelled 612 waiting requests" })), "Cancelled 612 waiting requests");
        assert_eq!(waiting_text(&json!({ "count": 0, "text": "Nothing is waiting" })), "Nothing is waiting");
        // ours when it has none
        assert_eq!(waiting_text(&json!({ "count": 3 })), "Cancelled 3 waiting requests");
        assert_eq!(waiting_text(&json!({ "count": 0 })), "Nothing is waiting");
    }

    #[test]
    fn words_follow_the_web() {
        let r = json!({ "kind": "album", "album": "Discovery", "artist": "Daft Punk" });
        assert_eq!(what(&r), "Discovery · Daft Punk");
        assert_eq!(what(&json!({ "kind": "artist", "artist": "Daft Punk" })), "Daft Punk · full discography");
        assert_eq!(what(&json!({ "kind": "url", "url": "https://x" })), "https://x");
        assert_eq!(what(&json!({ "kind": "playlist", "title": "Mix" })), "Mix · playlist import");
        assert_eq!(what(&json!({ "kind": "track", "title": "One", "artist": "A" })), "One · A");
        assert!(is_active("downloading") && !is_active("queued"));
        assert!(can_cancel("pending") && !can_cancel("completed"));
        assert_eq!(delete_files_text("completed", false), "Permanently delete the songs this request downloaded from the library?");
        assert_eq!(delete_files_text("failed", true), "Delete the downloaded files kept for this request? They can't be imported afterwards.");
        assert_eq!(delete_files_text("failed", false), "Delete the leftover/partial files of this download?");
    }

    #[test]
    fn pick_options_carry_only_what_the_row_has() {
        let r = json!({ "id": 7, "kind": "track", "artist": "A", "title": "T", "album": null, "status": "failed" });
        let o = pick_opts(&r);
        assert_eq!(o["request_id"], 7);
        assert_eq!(o["kind"], "track");
        assert!(o.get("album").is_some(), "a null field is still a field");
        assert!(o.get("mbid").is_none());
        assert!(o.get("status").is_none());
    }

    #[test]
    fn clear_and_retry_words() {
        assert_eq!(Clear::Failed.statuses(), &["failed", "cancelled", "denied"]);
        assert!(Clear::CompletedFiles.files() && !Clear::Completed.files());
        assert_eq!(Clear::FailedFiles.body(true, None), json!({ "statuses": ["failed", "cancelled", "denied"], "files": true, "mine": true }));
        // one person's, from the admin's User list
        assert_eq!(Clear::Completed.body(false, Some(7)), json!({ "statuses": ["completed"], "files": false, "mine": false, "user": 7 }));
        assert_eq!(whose_body(json!({ "dry_run": true }), false, None), json!({ "dry_run": true, "mine": false }));
        assert_eq!(whose_body(json!({}), false, Some(12)), json!({ "mine": false, "user": 12 }));
        // an import that hasn't moved for 15 minutes offers Import again (with why); one that moves doesn't
        let stalled = json!({ "status": "importing", "import_stalled": true, "has_files": true });
        assert!(stalled_import(&stalled) && !can_import_again(&stalled));
        assert!(!stalled_import(&json!({ "status": "importing", "import_stalled": false })));
        assert!(!stalled_import(&json!({ "status": "importing" })), "an older server says nothing");
        assert!(!stalled_import(&json!({ "status": "failed", "import_stalled": true })));
        assert_eq!(STALLED_IMPORT, "The import hasn't moved for 15 minutes");
        assert_eq!(clear_menu::<()>(true, |_| ()).len(), 4);
        assert_eq!(clear_menu::<()>(false, |_| ()).len(), 2);
        assert_eq!(row_menu::<()>(false, "failed", false, (), |_| ()).len(), 2);
        assert_eq!(row_menu::<()>(true, "completed", false, (), |_| ())[0].label, "Details");
        assert_eq!(row_menu::<()>(true, "completed", false, (), |_| ())[2].label, "Delete downloaded files");
        assert_eq!(row_menu::<()>(true, "failed", false, (), |_| ())[2].label, "Delete leftover files");
        assert_eq!(row_menu::<()>(true, "failed", true, (), |_| ())[2].label, "Delete downloaded files");
        // a request the worker is on: only its details
        assert_eq!(row_menu::<()>(true, "downloading", false, (), |_| ()).len(), 1);
        assert_eq!(cleared_toast(&json!({ "removed": 3, "deleted_files": 5 }), true), "3 cleared · 5 file(s) deleted");
        assert_eq!(cleared_toast(&json!({ "removed": 3 }), false), "3 cleared");
        assert_eq!(retry_failed_toast(&json!({ "retried": 0, "skipped": 0 })), ("No failed requests to retry".to_string(), false));
        assert_eq!(retry_failed_toast(&json!({ "retried": 1, "skipped": 0 })), ("Retrying 1 request".to_string(), false));
        assert_eq!(
            retry_failed_toast(&json!({ "retried": 2, "skipped": 1, "why": "quota" })),
            ("Retrying 2 · 1 couldn't be retried (quota)".to_string(), true)
        );
    }

    /// Retry is for those the server lets retry (the phone's `canRetry`): a member without Fix
    /// their own downloads gets no button that only answers "Ask an admin to retry".
    #[test]
    fn retry_is_for_those_who_may_retry() {
        let failed = json!({ "kind": "track", "status": "failed" });
        assert!(can_retry(&failed, true, false), "an admin");
        assert!(can_retry(&failed, false, true), "a member with fix");
        assert!(!can_retry(&failed, false, false), "a member without fix");
        assert!(can_retry(&json!({ "kind": "album", "status": "cancelled" }), false, true));
        assert!(!can_retry(&json!({ "kind": "track", "status": "completed" }), true, true));
        assert!(!can_retry(&json!({ "kind": "track", "status": "downloading" }), true, true));
        // a playlist's songs anyone may queue again
        assert!(can_retry(&json!({ "kind": "playlist", "status": "completed" }), false, false));
        // the kept files: Import again in Retry's place
        assert!(!can_retry(&json!({ "kind": "track", "status": "failed", "has_files": true }), true, true));
    }

    #[test]
    fn a_waiting_request_and_its_kept_files() {
        assert!(can_cancel("waiting") && !is_active("waiting"));
        assert_eq!(super::super::catalog::status_words("waiting").1, "Waiting");
        let failed = json!({ "status": "failed", "has_files": true });
        assert!(can_import_again(&failed));
        assert!(can_import_again(&json!({ "status": "waiting", "has_files": true })));
        assert!(!can_import_again(&json!({ "status": "failed", "has_files": false })));
        assert!(can_import_again(&json!({ "status": "completed", "has_files": true })), "a link's files that could not be imported");
        assert!(!can_import_again(&json!({ "status": "cancelled", "has_files": true })));
        assert!(!can_import_again(&json!({ "status": "failed" })), "an older server sends no has_files");
        // Keep anyway's import waits for the queue's next look, not for the storage: the server sends
        // no reason (R2), so the details read plain "Waiting"
        let kept = json!({ "id": 7, "kind": "track", "title": "One", "status": "waiting", "wait_reason": null,
                           "message": "Waiting to import the kept download", "has_files": true });
        let text = request_text(&kept, "");
        assert!(text.contains("Status: Waiting\n") && text.contains("Message: Waiting to import the kept download\n"), "{text}");
    }

    #[test]
    fn the_copied_request_holds_everything_for_a_report() {
        let r = json!({
            "id": 12, "kind": "album", "title": "Illuminate EP", "artist": "Duumu", "status": "waiting",
            "message": "Waiting: library storage error – import failed: mkdir /music/Duumu/Illuminate EP: Input/output error (os error 5). Next try at 14:20",
            "wait_reason": "storage_import", "next_try_at": 1_792_427_497.2, "has_files": true,
            "kept_path": "/downloads/encore/request-12/streamrip", "attempts": 3, "source": "streamrip", "username": "caz",
            "created_at": 1_792_420_000.0, "picked": true,
            "error": { "step": "import", "op": "mkdir", "path": "/music/Duumu/Illuminate EP", "to": null, "errno": 5,
                       "error": "Input/output error (os error 5)", "storage": true,
                       "text": "import failed: mkdir /music/Duumu/Illuminate EP: Input/output error (os error 5) – storage problem, not the source" },
            "output": { "source": "streamrip", "url": "https://www.deezer.com/album/1", "text": "Downloading Illuminate EP\n4 tracks done\n", "ok": true, "at": 1_792_427_000.1 },
            "log": ["slskd: error: Soulseek isn't connected yet", "attempt 2 failed: streamrip: Input/output error (os error 5)"],
            "candidate": null,
        });
        let text = request_text(&r, "0.29.0");
        assert!(text.starts_with("Encore request #12\nTitle: Illuminate EP\nArtist: Duumu\nKind: album\n"), "{text}");
        for want in [
            "Status: Waiting – the downloaded files are kept and the import waits for the storage",
            "Message: Waiting: library storage error – import failed: mkdir /music/Duumu/Illuminate EP: Input/output error (os error 5). Next try at 14:20",
            "Next try: 2026-10-19 16:31 UTC",
            "Attempts: 3",
            "Source: streamrip",
            "Picked by hand: yes",
            "Error: import failed: mkdir /music/Duumu/Illuminate EP: Input/output error (os error 5) – storage problem, not the source",
            "Step: import",
            "Operation: mkdir",
            "Path: /music/Duumu/Illuminate EP",
            "Errno: 5",
            "System error: Input/output error (os error 5)",
            "Storage problem: yes – not the source",
            "Downloaded files kept in: /downloads/encore/request-12/streamrip",
            "\nLog (every attempt, oldest first):\nslskd: error: Soulseek isn't connected yet\nattempt 2 failed: streamrip: Input/output error (os error 5)\n",
            "\nstreamrip output (https://www.deezer.com/album/1 · produced files · 2026-10-19 16:23 UTC):\nDownloading Illuminate EP\n4 tracks done\n",
            "Requested by: caz",
            "Request id: 12",
            "Server version: 0.29.0",
        ] {
            assert!(text.contains(want), "missing {want:?} in\n{text}");
        }
        assert!(!text.contains("Other path"), "an empty field is left out");
        assert_eq!(request_what(&r), "Illuminate EP – Duumu");
        assert_eq!(request_what(&json!({ "kind": "artist", "artist": "Duumu" })), "Duumu – full discography");
        // an older server's row still reads
        let old = request_text(&json!({ "id": 3, "kind": "track", "title": "One", "artist": "A", "status": "failed", "message": "All sources exhausted (x)", "attempts": 1 }), "");
        assert!(old.contains("Status: Failed") && old.contains("Attempts: 1") && old.contains("Server version: unknown"), "{old}");
        assert!(!old.contains("Error:") && !old.contains("Log ("), "{old}");
        assert!(text.ends_with("Server version: 0.29.0"), "the same last line as the web's and the phone's: {text}");
        assert_eq!(wait_words("soulseek"), "Soulseek isn't connected yet");
        assert_eq!(wait_words("plugin_install"), "its source's plugin is still being installed");
    }

    /// A song the audio check turned away (the server's answer, wf/r2-a.md).
    fn turned_away() -> Value {
        json!({
            "id": 12, "kind": "track", "title": "Moonlight Shadow (nightcore)", "artist": "Machine Time", "status": "failed",
            "message": "All sources exhausted (ytdlp: YouTube match was the wrong audio (audio is \"diase – sun\"))",
            "attempts": 10, "source": "ytdlp",
            "rejected": [
                { "id": 1, "source": "streamrip", "via": "Deezer", "detail": "via streamrip", "url": "https://www.deezer.com/track/3135556",
                  "file": "01. Moonlight Shadow.flac", "count": 1, "check": "audio", "reason": "audio is “Diase – Sun” (different title)",
                  "heard": "Diase – Sun", "text": "The audio sounds like Diase – Sun, not Moonlight Shadow", "at": 1_792_427_497.2,
                  "has_file": false, "path": null },
                { "id": 2, "source": "ytdlp", "via": "YouTube Music", "detail": null, "url": "https://music.youtube.com/watch?v=x",
                  "file": "Moonlight Shadow.opus", "count": 3, "check": "audio", "reason": "audio is “Diase – Sun” (different title)",
                  "heard": "Diase – Sun", "text": "The audio sounds like Diase – Sun, not Moonlight Shadow (nightcore)", "at": 1_792_427_557.0,
                  "has_file": true, "path": "/downloads/encore/rejected-12/2" }
            ],
            "anyway": { "action": "keep", "id": 2, "text": "The audio sounds like Diase – Sun, not Moonlight Shadow (nightcore)" },
            "skip_check": false,
        })
    }

    #[test]
    fn keep_anyway_and_download_anyway() {
        let r = turned_away();
        let a = anyway(&r).unwrap();
        assert_eq!(a, Anyway { keep: true, id: 2, text: "The audio sounds like Diase – Sun, not Moonlight Shadow (nightcore)".into() });
        assert_eq!(a.label(), "Keep anyway");
        assert_eq!(a.tip(), "Import the download the audio check turned away, without the check");
        assert_eq!(a.path(12), "/requests/12/keep-anyway");
        assert_eq!(a.body(), json!({ "rejected": 2 }));
        assert_eq!(a.done(), "It will be imported without the audio check");
        let act = a.act(12);
        assert_eq!((act.label.as_str(), act.path.as_str()), ("Keep anyway", "/requests/12/keep-anyway"));
        // the top of the details: why in full, and where it came from
        let lead = anyway_lead(&r, &a);
        assert_eq!(lead.text, "The audio sounds like Diase – Sun, not Moonlight Shadow (nightcore)");
        assert_eq!(lead.from, "YouTube Music · https://music.youtube.com/watch?v=x · Moonlight Shadow.opus and 2 more");
        // the kept file is gone: Download anyway
        let mut r2 = r;
        r2["anyway"] = json!({ "action": "download", "id": 1, "text": "The audio sounds like Diase – Sun, not Moonlight Shadow" });
        let d = anyway(&r2).unwrap();
        assert!(!d.keep);
        assert_eq!(d.label(), "Download anyway");
        assert_eq!(d.tip(), "Download it again from the same place, without the audio check");
        assert_eq!(d.path(12), "/requests/12/download-anyway");
        assert_eq!(d.done(), "Downloading it again without the audio check");
        // no text on the offer: the rejected download's own, else the check's words
        r2["anyway"] = json!({ "action": "download", "id": 1 });
        assert_eq!(anyway(&r2).unwrap().text, "The audio sounds like Diase – Sun, not Moonlight Shadow");
        let t = anyway(&json!({ "anyway": { "action": "keep", "id": 4 }, "rejected": [{ "id": 4, "reason": "audio is “X – Y”" }] }));
        assert_eq!(t.map(|a| a.text).as_deref(), Some("audio is “X – Y”"));
        // no id: the server takes its newest
        assert_eq!(anyway(&json!({ "anyway": { "action": "keep" } })).unwrap().body(), json!({}));
        // nothing offered, an unknown action, an older server
        r2["anyway"] = Value::Null;
        assert_eq!(anyway(&r2), None);
        r2["anyway"] = json!({ "action": "later", "id": 1 });
        assert_eq!(anyway(&r2), None);
        assert_eq!(anyway(&json!({ "id": 3, "status": "failed" })), None);
        // who may press it: admins, and members with `fix` (as Retry)
        assert!(may_fix(true, false) && may_fix(false, true) && !may_fix(false, false));
        // a hand pick turned away has the reason as its message: the row shows it once, on its own line
        let mut pick = turned_away();
        pick["message"] = json!("The audio sounds like Diase – Sun, not Moonlight Shadow (nightcore)");
        assert!(!shows_message(&pick, anyway(&pick).as_ref()));
        assert!(shows_message(&turned_away(), anyway(&turned_away()).as_ref()), "another message stays");
        assert!(shows_message(&json!({ "message": "Searching Soulseek" }), None));
        assert!(!shows_message(&json!({ "message": "" }), None));
    }

    #[test]
    fn the_details_name_every_download_the_audio_check_turned_away() {
        let mut r = turned_away();
        r["skip_check"] = json!(true);
        let text = request_text(&r, "0.29.0");
        let want = "Audio check: off for this request

Turned away by the audio check (oldest first):
The audio sounds like Diase – Sun, not Moonlight Shadow
  From: Deezer · via streamrip · https://www.deezer.com/track/3135556 · 01. Moonlight Shadow.flac
  Check: audio is “Diase – Sun” (different title)
  When: 2026-10-19 16:31 UTC
  Files: gone
The audio sounds like Diase – Sun, not Moonlight Shadow (nightcore)
  From: YouTube Music · https://music.youtube.com/watch?v=x · Moonlight Shadow.opus and 2 more
  Check: audio is “Diase – Sun” (different title)
  When: 2026-10-19 16:32 UTC
  Files: kept in /downloads/encore/rejected-12/2
";
        assert!(text.contains(want), "missing\n{want}\nin\n{text}");
        // a member gets no server folder
        let mut m = turned_away();
        m["rejected"][1]["path"] = Value::Null;
        assert!(request_text(&m, "").contains("  Files: kept on the server\n"));
        assert!(!request_text(&m, "").contains("Audio check:"));
        // nothing turned away, an older server: none of it
        let old = request_text(&json!({ "id": 3, "kind": "track", "title": "One", "status": "failed" }), "");
        assert!(!old.contains("Turned away") && !old.contains("Audio check"), "{old}");
        // no words at all
        let bare = rejected_lines(&json!({ "rejected": [{ "id": 1, "has_file": true }] }));
        assert_eq!(
            bare,
            vec![
                String::new(),
                "Turned away by the audio check (oldest first):".into(),
                "The audio sounds like another song".into(),
                "  Files: kept on the server".into()
            ]
        );
    }

    #[test]
    fn the_storage_banner_words() {
        assert_eq!(storage_text(&Value::Null, true), None, "an older server: no banner");
        assert_eq!(storage_text(&json!({ "ok": true, "text": null }), true), None);
        let admin = json!({ "ok": false, "path": "/music", "op": "mkdir", "error": "Input/output error (os error 5)",
            "text": "Imports are paused: /music is not writable – mkdir /music/.encore-probe-12-3: Input/output error (os error 5). Checked 14:10, next check 14:20" });
        assert_eq!(storage_text(&admin, true).unwrap(), fmt::s(&admin, "text"));
        let mut bare = admin;
        bare["text"] = Value::Null;
        assert_eq!(storage_text(&bare, true).unwrap(), "Imports are paused: /music is not writable – mkdir: Input/output error (os error 5).");
        assert_eq!(
            storage_text(&json!({ "ok": false, "since": 1.0 }), false).unwrap(),
            "Imports are paused because of a storage problem on the server. Your downloads are kept and will be added when it is fixed."
        );
        assert_eq!(
            candidate_line(&json!({ "username": "alice", "format": "flac", "directory": "Illuminate EP", "files": 4 })),
            "from alice: Illuminate EP (FLAC, 4 files)"
        );
        assert_eq!(candidate_line(&Value::Null), "");
    }

    /// A request waiting for a downloads folder (servers since 0.33) says so on its badge and in its
    /// details, and admins read the folder's sentence on the banner.
    #[test]
    fn a_request_waiting_for_the_downloads_folder() {
        let mut r = json!({ "id": 21, "kind": "album", "status": "waiting", "title": "Illuminate EP", "artist": "Duumu", "album": "Illuminate EP",
            "message": "Waiting for the downloads folder – /downloads/torrents is missing", "wait_reason": "waiting_disk", "next_try_at": null });
        assert_eq!(row_status(&r), (Badge::Warn, "Waiting for the downloads folder".to_string()));
        let text = request_text(&r, "0.33.0");
        assert!(text.contains("Status: Waiting – the downloads folder can't be written now"), "{text}");
        assert!(!text.contains("Next try"), "{text}");
        r["wait_reason"] = json!("soulseek");
        assert_eq!(row_status(&r), (Badge::Warn, "Waiting".to_string()));
        let s = json!({ "ok": true, "downloads": { "ok": false, "text": "The downloads folder /downloads is read-only – check the disk (dmesg, fsck) or the mount" } });
        assert_eq!(downloads_text(&s, true).as_deref(), Some("The downloads folder /downloads is read-only – check the disk (dmesg, fsck) or the mount"));
        assert_eq!(downloads_text(&s, false), None, "members get no banner for it");
        assert_eq!(downloads_text(&json!({ "ok": true, "downloads": { "ok": true } }), true), None);
        assert_eq!(downloads_text(&json!({ "ok": true }), true), None, "an older server");
        assert!(storage_failing(&s) && !storage_failing(&json!({ "ok": true, "downloads": { "ok": true } })));
        // Check again on the downloads sentence: imports were never paused, so they don't "resume"
        let fine = json!({ "ok": true, "downloads": { "ok": true } });
        assert_eq!(check_toast(&fine, &s), (true, "The downloads folders can be written"));
        assert_eq!(check_toast(&fine, &json!({ "ok": false, "downloads": { "ok": true } })), (true, "Imports resumed"));
        assert_eq!(check_toast(&s, &s), (false, "A downloads folder still can't be written"));
        assert_eq!(check_toast(&json!({ "ok": false }), &s), (false, "Imports are still paused"));
        assert_eq!(check_toast(&json!({ "ok": true, "downloads": { "ok": true, "checking": true } }), &s), (false, "Still checking the downloads folders"));
    }

    #[test]
    fn children_hang_under_their_parent() {
        let rows = vec![
            json!({ "id": 1, "status": "completed" }),
            json!({ "id": 2, "parent_id": 1, "status": "completed" }),
            json!({ "id": 3, "parent_id": 99, "status": "failed" }),
        ];
        let t = tree(&rows);
        assert_eq!(t.len(), 2, "an orphan is a parent");
        assert_eq!(t[0].1.len(), 1);
        assert!(t[1].1.is_empty());
    }

    /// A request in the streamrip line that carol asked to download now (download-now.md §3.2).
    fn in_line() -> Value {
        json!({
            "id": 12, "kind": "album", "status": "queued", "title": "Illuminate EP", "artist": "Duumu", "album": "Illuminate EP",
            "source": "streamrip", "message": "2nd in line for Deezer", "attempts": 1, "username": "carol",
            "can_download_now": true,
            "download_now": { "act": "line", "title": "Go first in line for Deezer", "until": null },
            "line": { "source": "streamrip", "label": "Deezer", "position": 2, "waiting": 5, "running": 1, "limit": 1,
                      "limit_text": "1 download at a time", "first": false, "text": "2nd in line for Deezer" },
            "remote_queue": null, "stalled_since": null,
            "pressed": { "by": "carol", "at": 1_792_500_300.0 },
        })
    }

    #[test]
    fn download_now_only_where_the_server_offers_it() {
        let r = in_line();
        assert!(can_download_now(&r));
        assert_eq!(download_now_title(&r), "Go first in line for Deezer");
        // false: no button, no tooltip
        let no = json!({ "id": 14, "status": "searching", "can_download_now": false, "download_now": null });
        assert!(!can_download_now(&no));
        assert_eq!(download_now_title(&no), "");
        assert_eq!(download_now_title(&json!({ "can_download_now": false, "download_now": { "title": "Go first in line for Deezer" } })), "");
        // an older server sends no such field: no button, and no Download all now
        let old = json!({ "id": 15, "status": "queued" });
        assert!(!can_download_now(&old));
        assert!(!has_download_now(std::slice::from_ref(&old)));
        assert!(!has_download_now(&[]));
        assert!(!has_download_now(&[json!({ "id": 16, "can_download_now": null })]));
        // a newer server: Download all now, even when no row offers its own button
        assert!(has_download_now(&[old, no]));
        // odd values: no button
        assert!(!can_download_now(&json!({ "can_download_now": "yes" })));
        // the row's message is the server's line text
        assert_eq!(fmt::s(&r, "message"), "2nd in line for Deezer");
    }

    #[test]
    fn download_now_toasts_are_the_servers_words() {
        assert_eq!(
            download_now_toast(&json!({ "ok": true, "action": "line", "text": "Next in line for Deezer (1 download at a time)", "status": "queued" })),
            Some((true, "Next in line for Deezer (1 download at a time)".to_string()))
        );
        assert_eq!(
            download_now_toast(&json!({ "ok": false, "action": "later", "at": 1_792_501_200, "status": "waiting",
                "text": "Another Soulseek client logged in with this username – Encore tries again at 14:35" })),
            Some((false, "Another Soulseek client logged in with this username – Encore tries again at 14:35".to_string()))
        );
        assert_eq!(
            download_now_toast(&json!({ "ok": false, "action": "kept", "text": "No one with a free upload slot has it – it stays 12th in alice's queue" })),
            Some((false, "No one with a free upload slot has it – it stays 12th in alice's queue".to_string()))
        );
        // without words: a press that started something says so; one that changed nothing says nothing
        assert_eq!(download_now_toast(&json!({ "ok": true, "action": "now" })), Some((true, "Starting now".to_string())));
        assert_eq!(download_now_toast(&json!({ "ok": false, "action": "later", "text": " " })), None);
        // Download all now
        let text = "Woke 3 waiting requests; 8 stay in line: Deezer 1 download at a time (6 in line), YouTube 2 downloads at a time (2 in line). Soulseek asked Encore to wait – next attempt at 14:20";
        assert_eq!(download_all_now_toast(&json!({ "ok": true, "pressed": 8, "woken": 3, "later": 1, "lines": [], "text": text })), (true, text.to_string()));
        assert_eq!(
            download_all_now_toast(&json!({ "ok": false, "pressed": 0, "woken": 0, "later": 0, "lines": [], "text": "Nothing is waiting – every request is downloading or done" })),
            (false, "Nothing is waiting – every request is downloading or done".to_string())
        );
        assert_eq!(download_all_now_toast(&json!({})), (false, NOTHING_WAITING.to_string()));
    }

    #[test]
    fn request_text_has_the_line_the_queue_and_the_press() {
        let text = request_text(&in_line(), "0.34.0");
        let want = "\nMessage: 2nd in line for Deezer\nIn line: 2nd in line for Deezer (1 download at a time)\nDownload now: asked by carol at 2026-10-20 12:45 UTC\nAttempts: 1\n";
        assert!(text.contains(want), "missing {want:?} in\n{text}");
        // a Soulseek download queued at alice
        let remote = json!({ "id": 13, "kind": "album", "status": "downloading", "can_download_now": true,
            "remote_queue": { "user": "alice", "place": 12, "since": 1_792_500_000.0, "text": "12th in alice's upload queue" } });
        assert_eq!(download_now_facts(&remote), vec![("Soulseek queue", "12th in alice's upload queue (since 2026-10-20 12:40 UTC)".to_string())]);
        // its place unknown, no time
        let unknown = json!({ "remote_queue": { "user": "alice", "place": null, "text": "in alice's upload queue" } });
        assert_eq!(download_now_facts(&unknown), vec![("Soulseek queue", "in alice's upload queue".to_string())]);
        // a torrent with no progress
        let stalled = json!({ "id": 18, "kind": "track", "status": "downloading", "source": "torznab", "stalled_since": 1_792_499_000 });
        assert!(request_text(&stalled, "").contains("\nNo progress since: 2026-10-20 12:23 UTC\n"));
        // what is known of a press, and a line without its limit's words
        let bare = json!({ "line": { "text": "Next in line for Soulseek" }, "pressed": { "by": "", "at": 1_792_500_300.0 } });
        assert_eq!(
            download_now_facts(&bare),
            vec![("In line", "Next in line for Soulseek".to_string()), ("Download now", "asked at 2026-10-20 12:45 UTC".to_string())]
        );
        // nothing of it from an older server
        let old = request_text(&json!({ "id": 3, "kind": "track", "title": "One", "status": "queued", "attempts": 1 }), "");
        assert!(!old.contains("In line") && !old.contains("Download now") && !old.contains("No progress"), "{old}");
        assert!(download_now_facts(&json!({ "line": null, "remote_queue": null, "pressed": null, "stalled_since": null })).is_empty());
    }
}
