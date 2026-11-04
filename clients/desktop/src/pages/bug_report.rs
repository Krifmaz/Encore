//! Bug report: One report: thread, status, export. Behaviour reference: web-rs/src/pages/bugreports.rs render_one.
//!
//! The report with its files, the replies, the reply box with pictures or text files (the same
//! picker as the New report dialog); admins set the status, export this report (saved through the
//! API client, since the browser has no session) and delete it.

use crate::api::ApiError;
use crate::fmt;
use crate::widgets::drop_zone;
use crate::pages::bug_reports::{form, kind_badge, pick_files, status_badge, status_label, status_table, Reply, ReplyMsg, ReplyOut};
pub use kit_iced::bug_report::meta_line;
use crate::pages::{Ctx, El, Page};
use crate::theme::{self, Badge, Btn};
use crate::widgets::catalog;
use crate::widgets::choice::select;
use crate::widgets::{
    self, badge, btn_icon, card, card_flat, confirm, error_box, h1, section_head, skeleton_rows, small,
    small_muted, t, Icon,
};
use iced::widget::{button, column, container, image, row, space};
use iced::{Alignment, ContentFit, Length, Subscription, Task};
use serde_json::{json, Value};

#[derive(Debug, Default)]
pub struct State {
    pub id: i64,
    /// `{report, messages}`; None while loading, Err(message) when it failed.
    data: Option<Result<Box<Value>, String>>,
    /// The reply box (the kit's: its text, files and Send).
    reply: Reply,
    /// The delete confirm is open.
    confirm_delete: bool,
}

#[derive(Clone, Debug)]
pub enum Message {
    Loaded(Result<Value, ApiError>),
    Back,
    OpenPage(Page),
    /// An attachment: its URL and file name.
    OpenFile(String, String),
    FileOpened(Result<(), String>),
    Status(String),
    StatusSet(String, Result<Value, ApiError>),
    Export,
    Exported(Result<String, String>),
    Delete,
    DeleteConfirmed,
    Deleted(Result<Value, ApiError>),
    /// The reply box: its text, files (picked or dropped on the window), Send and the server's answer.
    Reply(ReplyMsg),
    CloseDialog,
}

fn load(state: &State, ctx: &Ctx) -> Task<Message> {
    let api = ctx.api.clone();
    let path = format!("/bug-reports/{}", state.id);
    Task::perform(async move { api.get(&path).await }, Message::Loaded)
}

/// Fetch an attachment with the session, keep it in the temp folder and open it with the
/// system's app for its type.
async fn open_attachment(api: std::sync::Arc<crate::api::Api>, url: String, name: String) -> Result<(), String> {
    let bytes = api.get_bytes(&url).await.map_err(|e| e.message)?;
    // the file id (last path segment) keeps two files with the same name apart
    let fid = crate::pages::library::safe_file_name(url.rsplit('/').next().unwrap_or("file"));
    let dir = std::env::temp_dir().join("encore-bug-reports").join(fid);
    tokio::fs::create_dir_all(&dir).await.map_err(|e| e.to_string())?;
    let file = if name.trim().is_empty() { "file".to_string() } else { crate::pages::library::safe_file_name(&name) };
    let dest = dir.join(file);
    tokio::fs::write(&dest, bytes).await.map_err(|e| e.to_string())?;
    open::that_detached(&dest).map_err(|e| format!("Could not open the file: {e}"))
}

/// Called when the app navigates here (with the route's parameters).
pub fn open(state: &mut State, id: i64, ctx: &mut Ctx) -> Task<Message> {
    if state.id != id {
        state.reply = Reply::default();
    }
    state.id = id;
    state.data = None;
    state.reply.sending = false;
    state.confirm_delete = false;
    load(state, ctx)
}

pub fn update(state: &mut State, msg: Message, ctx: &mut Ctx) -> Task<Message> {
    match msg {
        Message::Loaded(Ok(v)) => {
            state.data = Some(Ok(Box::new(v)));
            // opening a report marks it read: the sidebar's badge follows
            ctx.refresh_shell();
        }
        Message::Loaded(Err(e)) => state.data = Some(Err(e.message)),
        Message::Back => ctx.navigate(Page::BugReports),
        Message::OpenPage(p) => ctx.navigate(p),
        // Attachments need the session, which the browser lacks: fetch, save to a temp file, open.
        Message::OpenFile(url, name) => {
            let api = ctx.api.clone();
            return Task::perform(open_attachment(api, url, name), Message::FileOpened);
        }
        Message::FileOpened(Ok(())) => {}
        Message::FileOpened(Err(e)) => ctx.toast_err(e),
        Message::Status(s) => {
            let api = ctx.api.clone();
            let path = format!("/bug-reports/{}/status", state.id);
            let body = json!({ "status": s });
            let s2 = s;
            return Task::perform(async move { api.post(&path, body).await }, move |r| {
                Message::StatusSet(s2, r)
            });
        }
        Message::StatusSet(s, Ok(_)) => {
            if let Some(Ok(d)) = state.data.as_mut() {
                d["report"]["status"] = json!(s);
            }
            ctx.toast_ok(format!("Status: {}", status_label(&s)));
            ctx.refresh_shell();
        }
        Message::StatusSet(_, Err(e)) => ctx.toast_err(e.message),
        Message::Export => {
            let api = ctx.api.clone();
            let path = format!("/bug-reports/export?ids={}&format=txt", state.id);
            let name = format!("bug-report-{}.txt", state.id);
            return Task::perform(crate::pages::library::save_as(api, path, name), Message::Exported);
        }
        Message::Exported(Ok(path)) => {
            if !path.is_empty() {
                ctx.toast_ok(format!("Saved to {path}"));
            }
        }
        Message::Exported(Err(e)) => ctx.toast_err(e),
        Message::Delete => state.confirm_delete = true,
        Message::DeleteConfirmed => {
            state.confirm_delete = false;
            let api = ctx.api.clone();
            let path = format!("/bug-reports/{}", state.id);
            return Task::perform(async move { api.del(&path).await }, Message::Deleted);
        }
        Message::Deleted(Ok(_)) => {
            ctx.toast_ok("Report deleted");
            ctx.refresh_shell();
            ctx.navigate(Page::BugReports);
        }
        Message::Deleted(Err(e)) => ctx.toast_err(e.message),
        Message::Reply(m) => {
            let mut tasks = Vec::new();
            for out in kit_iced::bug_report::reply_update(&mut state.reply, m) {
                match out {
                    ReplyOut::PickFiles => tasks.push(Task::perform(pick_files(), |f| Message::Reply(ReplyMsg::Picked(f)))),
                    ReplyOut::Error(e) => ctx.toast_err(e),
                    ReplyOut::Done(s) => ctx.toast_ok(s),
                    ReplyOut::Send(r) => {
                        let api = ctx.api.clone();
                        let path = format!("/bug-reports/{}/messages", state.id);
                        tasks.push(Task::perform(
                            async move {
                                let form = form(&r.fields(), &r.files).await?;
                                api.post_form(&path, form).await
                            },
                            |r: Result<Value, ApiError>| Message::Reply(ReplyMsg::Sent(r.map(|_| ()).map_err(|e| e.message))),
                        ));
                    }
                    ReplyOut::Reload => {
                        ctx.refresh_shell();
                        tasks.push(load(state, ctx));
                    }
                }
            }
            return Task::batch(tasks);
        }
        Message::CloseDialog => state.confirm_delete = false,
    }
    Task::none()
}

/// The thumbnails (pictures) and links (text files) of a report or a message.
fn files_view<'a>(files: &[Value], ctx: &Ctx<'a>) -> Option<El<'a, Message>> {
    if files.is_empty() {
        return None;
    }
    let look = ctx.look;
    let mut r = row![].spacing(8);
    for f in files {
        let (url, name, mime) = (fmt::s(f, "url"), fmt::s(f, "name"), fmt::s(f, "mime"));
        let inner: El<'a, Message> = if mime.starts_with("image/") {
            match ctx.picture(&url) {
                Some(h) => image(h)
                    .width(120)
                    .height(90)
                    .content_fit(ContentFit::Cover)
                    .border_radius(8.0)
                    .into(),
                None => container(widgets::icon(Icon::Disc, 20.0, look.faint))
                    .width(120)
                    .height(90)
                    .center(Length::Fill)
                    .style(theme::art)
                    .into(),
            }
        } else {
            container(
                row![
                    widgets::icon(Icon::External, 14.0, look.muted),
                    small(name.clone()),
                    small(fmt::size(fmt::f(f, "size"))).color(look.faint),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
            )
            .padding([7, 10])
            .style(theme::panel2)
            .into()
        };
        r = r.push(
            button(inner)
                .padding(0)
                .style(theme::btn(Btn::Row))
                .on_press(Message::OpenFile(url, name)),
        );
    }
    Some(r.wrap().into())
}

fn message_view<'a>(m: &Value, ctx: &Ctx<'a>) -> El<'a, Message> {
    let look = ctx.look;
    let mut head = row![t(fmt::s(m, "username")).size(14).font(theme::bold())]
        .spacing(8)
        .align_y(Alignment::Center);
    if fmt::truthy(m, "is_admin") {
        head = head.push(badge("Admin", Badge::Accent));
    }
    head = head.push(small(fmt::date(fmt::f(m, "created_at"))).color(look.faint));
    let mut col = column![head, t(fmt::s(m, "body")).size(14)].spacing(6);
    if let Some(files) = files_view(&fmt::items(&m["files"]), ctx) {
        col = col.push(files);
    }
    column![container(col).padding([12, 16]).width(Length::Fill), widgets::divider()].into()
}

pub fn view<'a>(state: &'a State, ctx: &Ctx<'a>) -> El<'a, Message> {
    let admin = ctx.is_admin();
    let back = catalog::muted_link("← Bug reports", 12.0, Message::Back);
    let body: El<'a, Message> = match &state.data {
        None => skeleton_rows(4),
        Some(Err(e)) => error_box(e),
        Some(Ok(d)) => {
            let r = &d["report"];
            let msgs = fmt::items(&d["messages"]);
            let status = fmt::s(r, "status");
            let status_el: El<'a, Message> = if admin {
                select(&status_table(), &status, Message::Status)
            } else {
                status_badge(&status)
            };
            let mut head = row![
                kind_badge(&fmt::s(r, "kind")),
                h1(fmt::s(r, "title")),
                status_el,
                space().width(Length::Fill),
            ]
            .spacing(10)
            .align_y(Alignment::Center);
            if admin {
                head = head.push(btn_icon(Icon::Download, "Export this report", Btn::Default, Some(Message::Export)));
                head = head.push(btn_icon(Icon::Trash, "Delete", Btn::Danger, Some(Message::Delete)));
            }
            let mut meta = row![small_muted(meta_line(r))].spacing(6).align_y(Alignment::Center);
            let page = fmt::s(r, "page");
            if let Some(p) = Page::from_hash(&page).filter(|_| page.starts_with("#/")) {
                meta = meta.push(catalog::muted_link("Open that page", 12.0, Message::OpenPage(p)));
            }
            let mut report = column![t(fmt::s(r, "body")).size(14)].spacing(10);
            if let Some(files) = files_view(&fmt::items(&r["files"]), ctx) {
                report = report.push(files);
            }
            let thread: El<'a, Message> = if msgs.is_empty() {
                container(small_muted("No replies yet."))
                    .padding(24)
                    .width(Length::Fill)
                    .center_x(Length::Fill)
                    .into()
            } else {
                column(msgs.iter().map(|m| message_view(m, ctx))).into()
            };
            let reply = kit_iced::bug_report::reply_view(&state.reply).map(Message::Reply);
            column![
                head,
                meta.wrap(),
                card(report).width(Length::Fill),
                section_head("Replies", None),
                card_flat(thread).width(Length::Fill),
                reply,
            ]
            .spacing(14)
            .into()
        }
    };
    column![back, body].spacing(10).width(Length::Fill).into()
}

/// A modal or ⋯ menu to lay over the whole window, when one is open.
pub fn overlay<'a>(state: &'a State, _ctx: &Ctx<'a>) -> Option<El<'a, Message>> {
    if !state.confirm_delete {
        return None;
    }
    Some(confirm(
        "Delete this report?",
        "The report, its replies and its files are removed for good.",
        "Delete",
        true,
        Message::DeleteConfirmed,
        Message::CloseDialog,
    ))
}

/// Files dragged onto the window go to the reply's attachments.
pub fn subscription(_state: &State) -> Subscription<Message> {
    drop_zone::events().map(|e| Message::Reply(ReplyMsg::Drop(e)))
}

