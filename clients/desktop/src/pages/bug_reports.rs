//! Bug reports: Reports and their threads. Behaviour reference: web-rs/src/pages/bugreports.rs render.
//!
//! The list with its filters (status tabs, kind chips, search, Everyone / Mine for admins),
//! Export (admins; saved through the API client, the browser has no session) and the New report dialog: kind, title,
//! description, pictures or text files picked with the OS dialog, sent as multipart to
//! `POST /api/bug-reports`. The file picker and the dialog are shared with the report page.

use crate::api::{Api, ApiError};
use crate::fmt;
use crate::pages::{urlencode, Ctx, El, Page};
use crate::theme::{self, Btn};
use crate::widgets::drop_zone;
use crate::widgets::{
    self, btn_icon, card_flat, error_box, h1, muted, seg, skeleton_rows, small, small_muted,
    t, tabs, Icon,
};
use iced::widget::{button, column, container, row, space, text_input};
use iced::{Alignment, Length, Subscription, Task};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::Duration;

// The words, the file picker and the New report dialog are the kit's (kit_iced::bug_report);
// this page keeps what needs the session: the app version, the device line, the file dialog and the request.
pub use kit_iced::bug_report::{
    count_words, kind_badge, status_badge, status_label, status_table, NewMsg, NewOut, NewReport, Picked, Reply, ReplyMsg, ReplyOut,
    ATTACH_FILES, FILE_FIELD, FILE_FILTER, KINDS, STATUSES,
};

/// The server's version as the app knows it (`/api/auth/info`), "" when it has none.
pub fn app_version(ctx: &Ctx) -> String {
    fmt::s(&ctx.session.info, "version")
}

/// What stands for the browser line on the web: the app's user agent and the window size.
pub fn device(ctx: &Ctx) -> String {
    kit_iced::bug_report::device_line(&crate::api::user_agent(), ctx.size.width, ctx.size.height)
}

/// The multipart form of a report or a reply: its text fields, then one `file` part per file.
pub async fn form(fields: &[(&'static str, &str)], paths: &[PathBuf]) -> Result<reqwest::multipart::Form, ApiError> {
    let mut form = reqwest::multipart::Form::new();
    for (name, value) in fields {
        form = form.text(*name, value.to_string());
    }
    for p in paths {
        form = form.part(FILE_FIELD, Api::file_part(p).await?);
    }
    Ok(form)
}

/// The OS file dialog for pictures, .txt and .log files; the picked files with their sizes.
pub async fn pick_files() -> Vec<Picked> {
    let Some(chosen) = rfd::AsyncFileDialog::new().add_filter(FILE_FILTER, ATTACH_FILES.exts).pick_files().await else {
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

/// The server's answer to a new report, as the dialog takes it: the report's id or the error's words.
pub fn sent(r: Result<Value, ApiError>) -> NewMsg {
    NewMsg::Sent(r.map(|v| fmt::i(&v, "id")).map_err(|e| e.message))
}

/// Drive the dialog. `Some(id)` comes back when the report was sent: the report to show.
pub fn new_update(d: &mut NewReport, msg: NewMsg, ctx: &mut Ctx) -> (Task<NewMsg>, Option<i64>) {
    let mut tasks = Vec::new();
    let mut shown = None;
    for out in kit_iced::bug_report::new_update(d, msg, &app_version(ctx), &device(ctx)) {
        match out {
            NewOut::PickFiles => tasks.push(Task::perform(pick_files(), NewMsg::Picked)),
            NewOut::Error(e) => ctx.toast_err(e),
            NewOut::Done(s) => ctx.toast_ok(s),
            NewOut::Send(r) => {
                let api = ctx.api.clone();
                tasks.push(Task::perform(
                    async move {
                        let form = form(&r.fields(), &r.files).await?;
                        api.post_form("/bug-reports", form).await
                    },
                    sent,
                ));
            }
            NewOut::Show(id) => {
                ctx.refresh_shell();
                shown = Some(id);
            }
        }
    }
    (Task::batch(tasks), shown)
}

/// The whole modal in the page's message type.
pub fn new_dialog<'a, M: Clone + 'a>(
    d: &'a NewReport,
    ctx: &Ctx<'a>,
    wrap: impl Fn(NewMsg) -> M + 'a,
    on_close: M,
) -> El<'a, M> {
    kit_iced::bug_report::new_dialog(d, &app_version(ctx), wrap, on_close)
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
}
