//! A list of requests as Requests and the Downloader show it (web-rs/src/pages/common.rs
//! `request_row`, `bind_request_actions`, `clear_menu`, `retry_failed`): the rows with their
//! action buttons, the ⋯ menu, the Clear menu, the picker, and the requests they send. A page
//! holds a [`List`], routes `Msg` here and draws [`view`] and [`overlay`]. Package C owns this file.

use super::catalog::status_badge;
use super::{
    badge, btn_sm, confirm, icon, icon_btn, picker, pop_menu, small_muted, t, tip_box, El, Icon, MenuItem,
};
use crate::api::ApiError;
use crate::fmt;
use crate::pages::Ctx;
use crate::theme::{self, Badge, Btn};
use iced::widget::{button, column, container, image, progress_bar, row, tooltip};
use iced::{Alignment, ContentFit, Length, Point, Task};
use serde_json::{json, Map, Value};
use std::collections::HashSet;

/// The web's `active` set: a request the worker is on right now.
pub fn is_active(status: &str) -> bool {
    matches!(status, "searching" | "downloading" | "importing" | "monitoring")
}

/// Statuses the Cancel button applies to.
pub fn can_cancel(status: &str) -> bool {
    matches!(
        status,
        "pending" | "queued" | "searching" | "downloading" | "monitoring" | "importing"
    )
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

/// The question before deleting a request's files.
pub fn delete_files_text(status: &str) -> &'static str {
    if status == "completed" {
        "Permanently delete the songs this request downloaded from the library?"
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

    /// `POST /api/requests/clear`.
    pub fn body(self, mine: bool) -> Value {
        json!({ "statuses": self.statuses(), "files": self.files(), "mine": mine })
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

/// The ⋯ menu of a finished row.
pub fn row_menu<M: Clone>(admin: bool, status: &str, on_remove: impl Fn(bool) -> M) -> Vec<MenuItem<M>> {
    let mut items = vec![MenuItem::new("Remove from list", on_remove(false)).icon(Icon::X)];
    if admin {
        items.push(
            MenuItem::new(
                if status == "completed" { "Delete downloaded files" } else { "Delete leftover files" },
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
    Row { id: i64, status: String },
    Clear,
}

#[derive(Clone, Debug)]
enum Then {
    Remove(i64),
    Clear(Clear),
}

#[derive(Debug)]
struct Confirm {
    text: String,
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
    }

    pub fn rows(&self) -> &[Value] {
        match &self.rows {
            Some(Ok(r)) => r,
            _ => &[],
        }
    }

    /// Fetch the rows again (the first time, and every few seconds while the page is open).
    pub fn load(&mut self, ctx: &Ctx) -> Task<Msg> {
        self.gen += 1;
        let gen = self.gen;
        let api = ctx.api.clone();
        let url = format!("/requests?{}", self.query);
        Task::perform(async move { api.get(&url).await }, move |r| Msg::Loaded(gen, r))
    }

    fn status_of(&self, id: i64) -> String {
        self.rows()
            .iter()
            .find(|r| fmt::i(r, "id") == id)
            .map(|r| fmt::s(r, "status"))
            .unwrap_or_default()
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
        Msg::Menu(id, status) => l.menu = Some((ctx.cursor, Menu::Row { id, status })),
        Msg::ClearMenu => l.menu = Some((ctx.cursor, Menu::Clear)),
        Msg::CloseMenu => l.menu = None,
        Msg::Remove(id, files) => {
            l.menu = None;
            if files {
                l.confirm = Some(Confirm {
                    text: delete_files_text(&l.status_of(id)).to_string(),
                    then: Then::Remove(id),
                });
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
                    text: format!("{}: this permanently deletes the files from disk.", c.label()),
                    then: Then::Clear(c),
                });
                return Task::none();
            }
            let api = ctx.api.clone();
            let body = c.body(l.mine);
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
                Then::Clear(k) => {
                    let body = k.body(l.mine);
                    Task::perform(
                        async move { api.post("/requests/clear", body).await },
                        |r| Msg::Cleared(true, r),
                    )
                }
            };
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
            let body = json!({ "mine": l.mine });
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
    }
    Task::none()
}

// ----- view -------------------------------------------------------------------------------

/// `requestRow(r, { admin })`: the thumb, what was asked, the status line, the buttons.
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
    if fmt::truthy(r, "source") {
        line = line.push(super::faint(format!("via {} ·", fmt::s(r, "source"))).size(12));
    }
    if fmt::truthy(r, "message") {
        line = line.push(small_muted(fmt::s(r, "message")));
    }
    if l.admin && fmt::truthy(r, "username") {
        line = line.push(super::faint(format!("· by {}", fmt::s(r, "username"))).size(12));
    }
    line = line.push(super::faint(format!("· {}", fmt::ago(fmt::f(r, "created_at")))).size(12));
    let mut lines = column![t(what(r)).size(14).font(theme::semibold()), line.wrap()].spacing(3);
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
    let mut right = row![status_badge(&status)].spacing(6).align_y(Alignment::Center);
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
    if matches!(status.as_str(), "failed" | "cancelled") || kind == "playlist" {
        right = right.push(icon_btn(Icon::Refresh, "Retry", (!busy).then_some(Msg::Act(id, "retry"))));
    }
    if can_cancel(&status) {
        right = right.push(icon_btn(Icon::X, "Cancel", (!busy).then_some(Msg::Act(id, "cancel"))));
    }
    if !active {
        right = right.push(icon_btn(Icon::More, "More", Some(Msg::Menu(id, status.clone()))));
    }
    let body = container(
        row![thumb, lines.width(Length::Fill), right]
            .spacing(14)
            .align_y(Alignment::Center),
    )
    .padding([11, 16]);
    // a right-click opens the row's ⋯ menu at the pointer, where it has one
    let body: El<'a, Msg> = if active {
        body.into()
    } else {
        iced::widget::mouse_area(body).on_right_press(Msg::Menu(id, status.clone())).into()
    };
    column![body, super::divider()].into()
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

/// "Retry failed" (every list has it).
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
        return Some(confirm("Delete files", &c.text, "Delete", true, Msg::ConfirmOk, Msg::CloseDialog));
    }
    let (at, menu) = l.menu.as_ref()?;
    let admin = ctx.is_admin();
    let items = match menu {
        Menu::Row { id, status } => {
            let id = *id;
            row_menu(admin, status, move |files| Msg::Remove(id, files))
        }
        Menu::Clear => clear_menu(admin, Msg::Clear),
    };
    Some(pop_menu(*at, items, Msg::CloseMenu, ctx.size))
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(delete_files_text("completed"), "Permanently delete the songs this request downloaded from the library?");
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
        assert_eq!(Clear::FailedFiles.body(true), json!({ "statuses": ["failed", "cancelled", "denied"], "files": true, "mine": true }));
        assert_eq!(clear_menu::<()>(true, |_| ()).len(), 4);
        assert_eq!(clear_menu::<()>(false, |_| ()).len(), 2);
        assert_eq!(row_menu::<()>(false, "failed", |_| ()).len(), 1);
        assert_eq!(row_menu::<()>(true, "completed", |_| ())[1].label, "Delete downloaded files");
        assert_eq!(cleared_toast(&json!({ "removed": 3, "deleted_files": 5 }), true), "3 cleared · 5 file(s) deleted");
        assert_eq!(cleared_toast(&json!({ "removed": 3 }), false), "3 cleared");
        assert_eq!(retry_failed_toast(&json!({ "retried": 0, "skipped": 0 })), ("No failed requests to retry".to_string(), false));
        assert_eq!(retry_failed_toast(&json!({ "retried": 1, "skipped": 0 })), ("Retrying 1 request".to_string(), false));
        assert_eq!(
            retry_failed_toast(&json!({ "retried": 2, "skipped": 1, "why": "quota" })),
            ("Retrying 2 · 1 couldn't be retried (quota)".to_string(), true)
        );
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
}
