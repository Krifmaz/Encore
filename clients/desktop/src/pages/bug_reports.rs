//! Bug reports: Reports and their threads. Behaviour reference: web-rs/src/pages/bugreports.rs render.
//!
//! The list with its filters (status tabs, kind chips, search, Everyone / Mine for admins),
//! Export (admins; saved through the API client, the browser has no session) and the New report dialog: kind, title,
//! description, pictures or text files picked with the OS dialog, sent as multipart to
//! `POST /api/bug-reports`. The file picker and the dialog are shared with the report page.

use crate::api::{Api, ApiError};
use crate::fmt;
use crate::pages::{urlencode, Ctx, El, Page};
use crate::theme::{self, Badge, Btn};
use crate::widgets::drop_zone::{self, DropZone, Outcome};
use crate::widgets::text_area::text_area;
use crate::widgets::{
    self, badge, btn, btn_icon, card_flat, error_box, field, h1, modal, muted, seg, skeleton_rows, small, small_muted,
    t, tabs, Icon,
};
use iced::widget::{button, column, container, image, row, space, text_editor, text_input};
use iced::{Alignment, Length, Subscription, Task};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::Duration;

/// The three kinds, as the server stores them and as the user reads them.
/// Every kind: the list's chips and the labels; `crash` is a crash log the phone app sends (the Crash logs section).
pub const KINDS: [(&str, &str); 4] = [("bug", "Bug"), ("not_bug", "Not a bug"), ("howto", "How do I…"), ("crash", "Crash log")];
/// The kinds a new report can pick (a crash log comes from the app).
pub const PICK_KINDS: [(&str, &str); 3] = [("bug", "Bug"), ("not_bug", "Not a bug"), ("howto", "How do I…")];
/// The five statuses: key, label, badge.
pub const STATUSES: [(&str, &str, Badge); 5] = [
    ("new", "New", Badge::Warn),
    ("looking", "Looking into it", Badge::Info),
    ("fixed", "Fixed", Badge::Ok),
    ("wontfix", "Won't fix", Badge::Plain),
    ("answered", "Answered", Badge::Accent),
];
pub const MAX_FILES: usize = 6;
pub const MAX_BYTES: u64 = 8 * 1024 * 1024;

pub fn kind_label(k: &str) -> &'static str {
    KINDS.iter().find(|(key, _)| *key == k).map(|(_, l)| *l).unwrap_or("Bug")
}

pub fn kind_badge<'a, M: 'a>(k: &str) -> El<'a, M> {
    let kind = match k {
        "bug" => Badge::Err,
        "howto" => Badge::Info,
        "crash" => Badge::Warn,
        _ => Badge::Plain,
    };
    badge(kind_label(k), kind)
}

pub fn status_label(s: &str) -> &'static str {
    STATUSES.iter().find(|(key, _, _)| *key == s).map(|(_, l, _)| *l).unwrap_or("New")
}

pub fn status_badge<'a, M: 'a>(s: &str) -> El<'a, M> {
    let kind = STATUSES.iter().find(|(key, _, _)| *key == s).map(|(_, _, b)| *b).unwrap_or(Badge::Warn);
    badge(status_label(s), kind)
}

/// `(key, label)` pairs for a status select.
pub fn status_table() -> Vec<(&'static str, &'static str)> {
    STATUSES.iter().map(|(k, l, _)| (*k, *l)).collect()
}

/// The server's version as the app knows it (`/api/auth/info`), "" when it has none.
pub fn app_version(ctx: &Ctx) -> String {
    fmt::s(&ctx.session.info, "version")
}

/// What stands for the browser line on the web: the app's user agent and the window size.
pub fn device(ctx: &Ctx) -> String {
    format!(
        "{} · {}x{}",
        crate::api::user_agent(),
        ctx.size.width.round() as u32,
        ctx.size.height.round() as u32
    )
}

/// "No replies", "1 reply", "3 replies" + " · 2 files".
pub fn count_words(messages: i64, files: i64) -> String {
    let mut s = match messages {
        0 => "No replies".to_string(),
        1 => "1 reply".to_string(),
        n => format!("{n} replies"),
    };
    if files > 0 {
        s += &format!(" · {files} file{}", if files == 1 { "" } else { "s" });
    }
    s
}

// ---- the file picker behind "Add images" (the New report dialog and the reply box) ---------

/// A picked file: its path and size.
pub type Picked = (PathBuf, u64);

#[derive(Debug, Default)]
pub struct Picker {
    pub files: Vec<PathBuf>,
    /// Files dragged onto the window: they join the picked ones.
    pub drop: DropZone,
}

/// The files *Add images* takes, picked or dropped (images, .txt and .log files, several at once).
pub const ATTACH_FILES: drop_zone::Accept = drop_zone::Accept::many(&["png", "jpg", "jpeg", "webp", "gif", "txt", "log"]);

impl Picker {
    /// Add picked files within the limits; the rule that stopped one goes to a toast.
    pub fn add(&mut self, picked: Vec<Picked>, ctx: &mut Ctx) {
        for (path, size) in picked {
            if self.files.len() >= MAX_FILES {
                ctx.toast_err(format!("At most {MAX_FILES} files per report"));
                break;
            }
            if size > MAX_BYTES {
                let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                ctx.toast_err(format!("{name} is over 8 MB"));
                continue;
            }
            if !self.files.contains(&path) {
                self.files.push(path);
            }
        }
    }

    /// A file dragged over or dropped on the window: a dropped one is added like a picked one
    /// (same types, same limits); one that isn't taken goes to a toast.
    pub fn dropped(&mut self, ev: drop_zone::Event, ctx: &mut Ctx) {
        match self.drop.handle(ev, &ATTACH_FILES) {
            Outcome::Take(path) => {
                let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                self.add(vec![(path, size)], ctx);
            }
            Outcome::Refused(m) => ctx.toast_err(m),
            Outcome::Nothing => {}
        }
    }

    pub fn remove(&mut self, i: usize) {
        if i < self.files.len() {
            self.files.remove(i);
        }
    }

    /// The thumbnails (pictures) or names (text files), each with its × button.
    pub fn view<'a, M: Clone + 'a>(&'a self, on_remove: impl Fn(usize) -> M + 'a) -> El<'a, M> {
        let p = theme::look();
        let mut r = row![].spacing(8);
        for (i, f) in self.files.iter().enumerate() {
            let name = f.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let inner: El<'a, M> = if is_picture(f) {
                image(image::Handle::from_path(f))
                    .width(56)
                    .height(56)
                    .content_fit(iced::ContentFit::Cover)
                    .border_radius(6.0)
                    .into()
            } else {
                container(small(name.clone()).color(p.muted))
                    .padding([6, 10])
                    .height(56)
                    .max_width(160)
                    .center_y(56)
                    .style(theme::panel2)
                    .into()
            };
            r = r.push(
                row![
                    inner,
                    button(widgets::icon(Icon::X, 12.0, p.muted))
                        .padding(4)
                        .style(theme::btn(Btn::Icon))
                        .on_press(on_remove(i)),
                ]
                .spacing(2)
                .align_y(Alignment::Start),
            );
        }
        r.wrap().into()
    }

    /// The multipart parts of the picked files, each under the field name `file`.
    pub async fn parts(paths: Vec<PathBuf>) -> Result<Vec<reqwest::multipart::Part>, ApiError> {
        let mut out = Vec::with_capacity(paths.len());
        for p in &paths {
            out.push(Api::file_part(p).await?);
        }
        Ok(out)
    }
}

/// Whether the file is a picture by its extension.
pub fn is_picture(path: &std::path::Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref(),
        Some("png" | "jpg" | "jpeg" | "webp" | "gif")
    )
}

/// The OS file dialog for pictures, .txt and .log files; the picked files with their sizes.
pub async fn pick_files() -> Vec<Picked> {
    let Some(chosen) = rfd::AsyncFileDialog::new()
        .add_filter("Images and text files", ATTACH_FILES.exts)
        .pick_files()
        .await
    else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(chosen.len());
    for f in chosen {
        let path = f.path().to_path_buf();
        let size = tokio::fs::metadata(&path).await.map(|m| m.len()).unwrap_or(0);
        out.push((path, size));
    }
    out
}

// ---- the New report dialog -------------------------------------------------------------

#[derive(Debug)]
pub struct NewReport {
    pub kind: String,
    /// The page the person came from (`#/library/album/…`), sent along.
    pub page: String,
    pub title: String,
    pub body: text_editor::Content,
    pub picker: Picker,
    pub sending: bool,
}

impl Default for NewReport {
    fn default() -> Self {
        Self {
            kind: "bug".into(),
            page: String::new(),
            title: String::new(),
            body: text_editor::Content::new(),
            picker: Picker::default(),
            sending: false,
        }
    }
}

#[derive(Clone, Debug)]
pub enum NewMsg {
    Kind(String),
    Title(String),
    Body(text_editor::Action),
    AddFiles,
    Picked(Vec<Picked>),
    /// A file dragged over or dropped on the window: it joins the attachments.
    Drop(drop_zone::Event),
    RemoveFile(usize),
    Send,
    Sent(Result<Value, ApiError>),
}

/// Drive the dialog. `Some(id)` comes back when the report was sent: the report to show.
pub fn new_update(d: &mut NewReport, msg: NewMsg, ctx: &mut Ctx) -> (Task<NewMsg>, Option<i64>) {
    match msg {
        NewMsg::Kind(k) => d.kind = k,
        NewMsg::Title(s) => d.title = s,
        NewMsg::Body(a) => d.body.perform(a),
        NewMsg::AddFiles => return (Task::perform(pick_files(), NewMsg::Picked), None),
        NewMsg::Picked(files) => d.picker.add(files, ctx),
        NewMsg::Drop(ev) => d.picker.dropped(ev, ctx),
        NewMsg::RemoveFile(i) => d.picker.remove(i),
        NewMsg::Send => {
            if d.sending {
                return (Task::none(), None);
            }
            let title = d.title.trim().to_string();
            let text = d.body.text().trim().to_string();
            if title.is_empty() {
                ctx.toast_err("Give the report a title");
                return (Task::none(), None);
            }
            if text.is_empty() {
                ctx.toast_err("Say what happened");
                return (Task::none(), None);
            }
            d.sending = true;
            let api = ctx.api.clone();
            let kind = d.kind.clone();
            let version = app_version(ctx);
            let device = device(ctx);
            let paths = d.picker.files.clone();
            let page = d.page.clone();
            return (
                Task::perform(
                    async move {
                        let mut form = reqwest::multipart::Form::new()
                            .text("kind", kind)
                            .text("title", title)
                            .text("body", text)
                            .text("page", page)
                            .text("app_version", version)
                            .text("device", device);
                        for part in Picker::parts(paths).await? {
                            form = form.part("file", part);
                        }
                        api.post_form("/bug-reports", form).await
                    },
                    NewMsg::Sent,
                ),
                None,
            );
        }
        NewMsg::Sent(Ok(r)) => {
            d.sending = false;
            ctx.toast_ok("Report sent. Thanks.");
            ctx.refresh_shell();
            return (Task::none(), Some(fmt::i(&r, "id")));
        }
        NewMsg::Sent(Err(e)) => {
            d.sending = false;
            ctx.toast_err(e.message);
        }
    }
    (Task::none(), None)
}

/// The whole modal in the page's message type.
pub fn new_dialog<'a, M: Clone + 'a>(
    d: &'a NewReport,
    ctx: &Ctx<'a>,
    wrap: impl Fn(NewMsg) -> M + 'a,
    on_close: M,
) -> El<'a, M> {
    let send = wrap(NewMsg::Send);
    let version = app_version(ctx);
    let body = column![
        seg(&PICK_KINDS, &d.kind, NewMsg::Kind),
        field(
            "Title",
            text_input("A short name for it", &d.title)
                .on_input(NewMsg::Title)
                .padding([9, 12])
                .size(14)
                .style(theme::input)
        ),
        field(
            "Description",
            text_area(&d.body, "What happened, what you expected, how to repeat it", 6, NewMsg::Body)
        ),
        drop_zone::view(
            column![
                row![
                    btn_icon(Icon::Plus, "Add images", Btn::Default, Some(NewMsg::AddFiles)),
                    small_muted(format!("Up to {MAX_FILES} files, 8 MB each. Images, .txt and .log files (pick or drop them).")),
                ]
                .spacing(10)
                .align_y(Alignment::Center)
                .wrap(),
                d.picker.view(NewMsg::RemoveFile),
            ]
            .spacing(12),
            &d.picker.drop,
            "Drop images, .txt or .log files to attach them",
        ),
        small_muted(format!(
            "Sent along: {}the app version ({}) and this computer.",
            if d.page.is_empty() { String::new() } else { format!("the page you came from ({}), ", d.page) },
            if version.is_empty() { "unknown" } else { &version }
        )),
    ]
    .spacing(12);
    let footer = row![
        btn("Cancel", Btn::Default, Some(on_close.clone())),
        btn(
            if d.sending { "Sending…" } else { "Send" },
            Btn::Primary,
            (!d.sending).then_some(send)
        ),
    ]
    .spacing(10);
    modal("New report", El::from(body).map(wrap), Some(footer.into()), false, on_close)
}

// ---- the list filter ---------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct Filter {
    /// "all" or a status key.
    pub status: String,
    /// "" or a kind key.
    pub kind: String,
    pub q: String,
    /// Admins: only my own reports.
    pub mine: bool,
}

impl Default for Filter {
    fn default() -> Self {
        Self {
            status: "all".into(),
            kind: String::new(),
            q: String::new(),
            mine: false,
        }
    }
}

impl Filter {
    /// The web's `encore.bugFilter` (`{status, kind, q, mine}`), as kept in the config.
    pub fn from_json(v: &Value) -> Filter {
        let d = Filter::default();
        Filter {
            status: v["status"].as_str().filter(|s| !s.is_empty()).map(str::to_string).unwrap_or(d.status),
            kind: fmt::s(v, "kind"),
            q: fmt::s(v, "q"),
            mine: v["mine"].as_bool().unwrap_or(false),
        }
    }

    pub fn to_json(&self) -> Value {
        json!({ "status": self.status, "kind": self.kind, "q": self.q, "mine": self.mine })
    }

    /// `status=&kind=` for the list and the export link (only what is set).
    pub fn params(&self) -> Vec<String> {
        let mut p = Vec::new();
        if self.status != "all" && !self.status.is_empty() {
            p.push(format!("status={}", urlencode(&self.status)));
        }
        if !self.kind.is_empty() {
            p.push(format!("kind={}", urlencode(&self.kind)));
        }
        p
    }

    pub fn active(&self) -> bool {
        (self.status != "all" && !self.status.is_empty()) || !self.kind.is_empty() || !self.q.trim().is_empty()
    }

    /// The query string of the list request.
    pub fn list_query(&self, admin: bool) -> String {
        let mut p = self.params();
        if !self.q.trim().is_empty() {
            p.push(format!("q={}", urlencode(self.q.trim())));
        }
        if admin && self.mine {
            p.push("mine=1".into());
        }
        if p.is_empty() {
            String::new()
        } else {
            format!("?{}", p.join("&"))
        }
    }

    /// `/api/bug-reports/export?...` under the server's base.
    pub fn export_url(&self, base: &str) -> String {
        let mut p = self.params();
        // a plain text file (the zip with the pictures: without format=txt)
        p.push("format=txt".into());
        format!("{base}/api/bug-reports/export?{}", p.join("&"))
    }
}

// ---- the page ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct State {
    pub filter: Filter,
    /// None while loading; Err(message) when the server said no.
    rows: Option<Result<Vec<Value>, String>>,
    dialog: Option<NewReport>,
    /// The search box's debounce: only the latest keystroke's timer loads.
    typed: u64,
}

#[derive(Clone, Debug)]
pub enum Message {
    Loaded(Result<Value, ApiError>),
    Status(String),
    Kind(String),
    Query(String),
    /// The 250 ms after a keystroke passed.
    Typed(u64),
    Scope(String),
    Export,
    Exported(Result<String, String>),
    NewReport,
    New(NewMsg),
    Open(i64),
    Refresh,
    CloseDialog,
}

/// The web's `encore.bugFilter` key.
const FILTER_KEY: &str = "encore.bugFilter";

/// Keep the filter in the config (the web keeps it in localStorage) and load the list.
fn reload(state: &State, ctx: &mut Ctx) -> Task<Message> {
    let v = state.filter.to_json();
    ctx.edit_config(move |c| {
        c.local.insert(FILTER_KEY.into(), v);
    });
    load(state, ctx)
}

fn load(state: &State, ctx: &Ctx) -> Task<Message> {
    let api = ctx.api.clone();
    let path = format!("/bug-reports{}", state.filter.list_query(ctx.is_admin()));
    Task::perform(async move { api.get(&path).await }, Message::Loaded)
}

/// Called when the app navigates here (with the route's parameters).
pub fn open(state: &mut State, ctx: &mut Ctx) -> Task<Message> {
    state.rows = None;
    state.dialog = None;
    if let Some(v) = ctx.config.local(FILTER_KEY) {
        state.filter = Filter::from_json(v);
    }
    if !ctx.is_admin() {
        state.filter.mine = false;
    }
    load(state, ctx)
}

pub fn update(state: &mut State, msg: Message, ctx: &mut Ctx) -> Task<Message> {
    match msg {
        Message::Loaded(Ok(v)) => state.rows = Some(Ok(fmt::items(&v["reports"]))),
        Message::Loaded(Err(e)) => state.rows = Some(Err(e.message)),
        Message::Status(s) => {
            state.filter.status = s;
            return reload(state, ctx);
        }
        Message::Kind(k) => {
            state.filter.kind = if state.filter.kind == k { String::new() } else { k };
            return reload(state, ctx);
        }
        Message::Query(q) => {
            state.filter.q = q;
            state.typed += 1;
            let n = state.typed;
            return Task::perform(
                async move {
                    tokio::time::sleep(Duration::from_millis(250)).await;
                    n
                },
                Message::Typed,
            );
        }
        Message::Typed(n) => {
            if n == state.typed {
                return reload(state, ctx);
            }
        }
        Message::Scope(v) => {
            state.filter.mine = v == "mine";
            return reload(state, ctx);
        }
        Message::Export => {
            let api = ctx.api.clone();
            let url = state.filter.export_url(&ctx.base());
            return Task::perform(
                crate::pages::library::save_as(api, url, "encore-bug-reports.txt".into()),
                Message::Exported,
            );
        }
        Message::Exported(Ok(path)) => {
            if !path.is_empty() {
                ctx.toast_ok(format!("Saved to {path}"));
            }
        }
        Message::Exported(Err(e)) => ctx.toast_err(e),
        Message::NewReport => {
            state.dialog = Some(NewReport {
                page: ctx.previous.map(Page::hash).unwrap_or_default(),
                ..NewReport::default()
            })
        }
        Message::New(m) => {
            if let Some(d) = state.dialog.as_mut() {
                let (task, sent) = new_update(d, m, ctx);
                if let Some(id) = sent {
                    state.dialog = None;
                    ctx.navigate(Page::BugReport(id));
                }
                return task.map(Message::New);
            }
        }
        Message::Open(id) => ctx.navigate(Page::BugReport(id)),
        Message::Refresh => return load(state, ctx),
        Message::CloseDialog => state.dialog = None,
    }
    Task::none()
}

fn report_row<'a>(r: &Value, ctx: &Ctx<'a>) -> El<'a, Message> {
    let look = ctx.look;
    let id = fmt::i(r, "id");
    let unread = fmt::truthy(r, "unread");
    let when = fmt::ago(if fmt::f(r, "updated_at") > 0.0 { fmt::f(r, "updated_at") } else { fmt::f(r, "created_at") });
    let dot: El<'a, Message> = container(space())
        .width(8)
        .height(8)
        .style(move |_| container::Style {
            background: Some(iced::Background::Color(if unread { look.warn } else { iced::Color::TRANSPARENT })),
            border: iced::Border {
                color: iced::Color::TRANSPARENT,
                width: 0.0,
                radius: 99.0.into(),
            },
            ..container::Style::default()
        })
        .into();
    let title = t(fmt::s(r, "title")).size(14).font(theme::semibold());
    let line = row![
        dot,
        kind_badge(&fmt::s(r, "kind")),
        column![
            title,
            small(format!("by {} · {when}", fmt::s(r, "username"))).color(look.faint),
        ]
        .spacing(2)
        .width(Length::Fill),
        status_badge(&fmt::s(r, "status")),
        small_muted(count_words(fmt::i(r, "message_count"), fmt::i(r, "file_count"))),
    ]
    .spacing(12)
    .align_y(Alignment::Center);
    let b = button(container(line).padding([11, 16]))
        .padding(0)
        .width(Length::Fill)
        .style(theme::btn(Btn::Row))
        .on_press(Message::Open(id));
    column![b, widgets::divider()].into()
}

pub fn view<'a>(state: &'a State, ctx: &Ctx<'a>) -> El<'a, Message> {
    let admin = ctx.is_admin();
    let f = &state.filter;
    let mut actions = row![].spacing(8).align_y(Alignment::Center);
    if admin {
        actions = actions.push(seg(
            &[("all", "Everyone"), ("mine", "Mine")],
            if f.mine { "mine" } else { "all" },
            Message::Scope,
        ));
        actions = actions.push(btn_icon(Icon::Download, "Export", Btn::Default, Some(Message::Export)));
    }
    actions = actions.push(btn_icon(Icon::Plus, "New report", Btn::Primary, Some(Message::NewReport)));
    let head = row![h1("Bug reports").width(Length::Fill), actions]
        .spacing(12)
        .align_y(Alignment::Center);
    // the tab row reads "Looking" (the badge and the export keep the long label)
    let status_tabs: Vec<(&'a str, &'a str)> = std::iter::once(("all", "All"))
        .chain(STATUSES.iter().map(|(k, l, _)| (*k, if *k == "looking" { "Looking" } else { *l })))
        .collect();
    let mut chips = row![].spacing(6).align_y(Alignment::Center);
    for (k, l) in KINDS {
        let on = f.kind == k;
        chips = chips.push(widgets::catalog::chip(l.to_string(), on, Message::Kind(k.to_string())));
    }
    let filters = row![
        chips,
        space().width(Length::Fill),
        container(
            text_input("Search reports", &f.q)
                .on_input(Message::Query)
                .padding([7, 12])
                .size(14)
                .style(theme::input)
        )
        .max_width(260),
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    let body: El<'a, Message> = match &state.rows {
        None => skeleton_rows(5),
        Some(Err(e)) => error_box(e),
        Some(Ok(rows)) if rows.is_empty() => {
            let words = if f.active() {
                "Nothing matches these filters."
            } else {
                "No reports yet. New report writes the first one."
            };
            container(
                column![widgets::icon(Icon::Bug, 34.0, ctx.look.faint), muted(words).size(13)]
                    .spacing(10)
                    .align_x(Alignment::Center),
            )
            .width(Length::Fill)
            .padding([44, 20])
            .center_x(Length::Fill)
            .into()
        }
        Some(Ok(rows)) => column(rows.iter().map(|r| report_row(r, ctx)).collect::<Vec<_>>()).into(),
    };
    column![
        head,
        tabs(&status_tabs, &f.status, Message::Status),
        filters,
        card_flat(body).width(Length::Fill),
    ]
    .spacing(16)
    .width(Length::Fill)
    .into()
}

/// A modal or ⋯ menu to lay over the whole window, when one is open.
pub fn overlay<'a>(state: &'a State, ctx: &Ctx<'a>) -> Option<El<'a, Message>> {
    let d = state.dialog.as_ref()?;
    Some(new_dialog(d, ctx, Message::New, Message::CloseDialog))
}

/// The list refreshes once a minute while it is open (a reply may have come in); files dragged onto
/// the window go to the New report dialog while it is open.
pub fn subscription(state: &State) -> Subscription<Message> {
    let refresh = iced::time::every(Duration::from_secs(60)).map(|_| Message::Refresh);
    if state.dialog.is_some() {
        Subscription::batch([refresh, drop_zone::events().map(|e| Message::New(NewMsg::Drop(e)))])
    } else {
        refresh
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_match_the_web() {
        assert_eq!(kind_label("howto"), "How do I…");
        assert_eq!(kind_label("nope"), "Bug");
        assert_eq!(status_label("wontfix"), "Won't fix");
        assert_eq!(status_label("?"), "New");
        assert_eq!(count_words(0, 0), "No replies");
        assert_eq!(count_words(1, 1), "1 reply · 1 file");
        assert_eq!(count_words(3, 2), "3 replies · 2 files");
        assert_eq!(status_table().len(), 5);
    }

    #[test]
    fn the_filter_builds_the_same_queries() {
        let mut f = Filter::default();
        assert!(!f.active());
        assert_eq!(Filter::from_json(&f.to_json()), f);
        assert_eq!(Filter::from_json(&serde_json::Value::Null), f, "nothing saved: the default");
        assert_eq!(f.list_query(true), "");
        assert_eq!(f.export_url("http://s"), "http://s/api/bug-reports/export?format=txt");
        f.status = "looking".into();
        f.kind = "bug".into();
        f.q = " hi ".into();
        f.mine = true;
        assert!(f.active());
        assert_eq!(f.list_query(true), "?status=looking&kind=bug&q=hi&mine=1");
        assert_eq!(f.list_query(false), "?status=looking&kind=bug&q=hi");
        assert_eq!(f.export_url("http://s"), "http://s/api/bug-reports/export?status=looking&kind=bug&format=txt");
    }

    #[test]
    fn pictures_by_extension() {
        assert!(is_picture(std::path::Path::new("/a/b.PNG")));
        assert!(!is_picture(std::path::Path::new("/a/b.log")));
    }
}
