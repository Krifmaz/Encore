//! Settings → Diagnostics. Behaviour reference: web-rs/src/pages/diagnostics.rs: every connection,
//! folder and tool check from inside the container with the raw answers, the server block, where
//! settings come from (folded until opened), Backups (back up, upload, download, restore) and App crashes.

use super::help;
use crate::api::ApiError;
use crate::fmt;
use crate::pages::{Ctx, El};
use crate::theme::{self, Badge, Btn};
use crate::widgets::drop_zone::{self, DropZone, Outcome};
use crate::widgets::{self, badge, btn, btn_icon, btn_sm, modal, small_muted, t, Icon};
use iced::widget::{column, container, mouse_area, row, space};
use iced::{Alignment, Length, Task};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::PathBuf;

/// The [`State::open`] key of the folded "Where settings come from" list (the checks use `check-N`, crashes their ids).
const SOURCES: &str = "settings-sources";

const KIND: [(&str, &str); 3] = [("service", "Connections"), ("path", "Folders & permissions"), ("tool", "Tools in the container")];

#[derive(Clone, Debug)]
pub enum Dialog {
    Restore(String),
    ClearCrashes,
}

#[derive(Debug, Default)]
pub struct State {
    /// `/system/diagnostics`, None while running.
    data: Option<Value>,
    /// Encore's VPN for Soulseek as one more connection check (`/system/vpn`).
    vpn: Option<Value>,
    running: bool,
    error: String,
    backups: Option<Value>,
    backups_busy: bool,
    crashes: Option<Value>,
    /// Raw answers and full reports that are unfolded.
    open: HashSet<String>,
    dialog: Option<Dialog>,
    /// Backups dragged onto the window for *Upload a backup…*.
    drop: DropZone,
}

#[derive(Clone, Debug)]
pub enum Message {
    Run,
    Checks(Result<Value, ApiError>),
    Vpn(Result<Value, ApiError>),
    CopyReport,
    Toggle(String),
    Backups(Result<Value, ApiError>),
    BackupNow,
    BackedUp(Result<Value, ApiError>),
    PickBackup,
    BackupPicked(Option<PathBuf>),
    /// A file dragged over or dropped on the window: a dropped database backup is uploaded.
    Drop(drop_zone::Event),
    Uploaded(Result<Value, ApiError>),
    Download(String),
    Downloaded(Result<String, String>),
    AskRestore(String),
    Restore,
    Restored(Result<Value, ApiError>),
    Crashes(Result<Value, ApiError>),
    AskClearCrashes,
    ClearCrashes,
    Cleared(Result<Value, ApiError>),
    CloseDialog,
    /// Copy one failed check or one crash report in full.
    CopyText(String),
}

/// The VPN for Soulseek line: "Connected · port 51234 · public IP 1.2.3.4 · last handshake 12 s ago",
/// "Connecting…", the error or "Off".
pub fn vpn_text(v: &Value) -> String {
    if !v.is_object() {
        return "Off".into();
    }
    match v["state"].as_str() {
        Some("up") => {
            let mut parts = vec!["Connected".to_string()];
            if !v["forwarded_port"].is_null() && v.get("forwarded_port").is_some() {
                parts.push(format!("port {}", fmt::s(v, "forwarded_port")));
            } else if fmt::truthy(v, "port_forwarding") {
                parts.push(if fmt::truthy(v, "natpmp_error") {
                    format!("no forwarded port ({})", fmt::s(v, "natpmp_error"))
                } else {
                    "no forwarded port yet".to_string()
                });
            }
            if fmt::truthy(v, "public_ip") {
                parts.push(format!("public IP {}", fmt::s(v, "public_ip")));
            }
            if v.get("last_handshake_secs_ago").is_some_and(|x| !x.is_null()) {
                parts.push(format!("last handshake {} s ago", super::js_round(fmt::f(v, "last_handshake_secs_ago"))));
            }
            parts.join(" · ")
        }
        Some("connecting") => "Connecting…".into(),
        Some("error") => {
            if fmt::truthy(v, "error") {
                fmt::s(v, "error")
            } else {
                "The VPN could not connect".into()
            }
        }
        _ => "Off".into(),
    }
}

/// The VPN as a check row, like the web adds it to the list.
pub fn vpn_row(v: &Value) -> Value {
    json!({
        "kind": "service", "name": "VPN for Soulseek",
        "ok": !fmt::truthy(v, "enabled") || v["state"].as_str() == Some("up"),
        "request": if fmt::truthy(v, "endpoint") { format!("WireGuard {}", fmt::s(v, "endpoint")) } else { String::new() },
        "detail": vpn_text(v),
    })
}

fn run(ctx: &Ctx) -> Task<Message> {
    let api = ctx.api.clone();
    let api2 = ctx.api.clone();
    Task::batch([
        Task::perform(async move { api.get("/system/diagnostics").await }, Message::Checks),
        Task::perform(async move { api2.get("/system/vpn").await }, Message::Vpn),
    ])
}

fn load_backups(ctx: &Ctx) -> Task<Message> {
    let api = ctx.api.clone();
    Task::perform(async move { api.get("/system/backups").await }, Message::Backups)
}

fn load_crashes(ctx: &Ctx) -> Task<Message> {
    let api = ctx.api.clone();
    Task::perform(async move { api.get("/diagnostics/client-crash").await }, Message::Crashes)
}

pub fn open(state: &mut State, ctx: &mut Ctx) -> Task<Message> {
    state.data = None;
    state.vpn = None;
    state.running = true;
    state.error.clear();
    state.backups = None;
    state.crashes = None;
    state.open.clear();
    state.dialog = None;
    Task::batch([run(ctx), load_backups(ctx), load_crashes(ctx)])
}

/// `(label, value)` rows of the Server block (`machine` in the diagnostics answer).
pub fn server_rows(s: &Value) -> Vec<(&'static str, String)> {
    let folder = |v: &Value| -> String {
        if !v.is_object() {
            return "not set".into();
        }
        let mut t = if fmt::truthy(v, "fs") { fmt::s(v, "fs") } else { "unknown file system".to_string() };
        if v.get("network").is_some() {
            t.push_str(if fmt::truthy(v, "network") { " (network)" } else { " (local)" });
        }
        if fmt::truthy(v, "path") { format!("{} · {t}", fmt::s(v, "path")) } else { t }
    };
    let nullish = |k: &str| s.get(k).is_none_or(|v| v.is_null());
    let memory = if nullish("memory") {
        "unknown".to_string()
    } else {
        format!("{}{}", fmt::size(fmt::f(s, "memory")), if s["memory_source"].as_str() == Some("cgroup") { " (container limit)" } else { " (whole machine)" })
    };
    let readers = {
        let n = fmt::i(s, "db_readers");
        match s["db_readers_source"].as_str() {
            Some("env") => format!("{n} (ENCORE_DB_READERS)"),
            Some("network") => format!("{n} – off because the config folder is on a network file system (set ENCORE_DB_READERS to turn them on)"),
            _ => format!("{n} (default)"),
        }
    };
    vec![
        ("CPUs", if nullish("cpus") { "unknown".into() } else { fmt::s(s, "cpus") }),
        ("Memory", memory),
        ("Config folder", folder(&s["config"])),
        ("Music folder", folder(&s["music"])),
        ("Database", if nullish("database_bytes") { "unknown".into() } else { fmt::size(fmt::f(s, "database_bytes")) }),
        ("Translations", if fmt::truthy(s, "translations_on") { "on for at least one account".into() } else { "off for every account".into() }),
        ("Database read connections", readers),
    ]
}

/// The plain-text report the Copy report button puts on the clipboard.
pub fn report(last: &Value) -> String {
    let mut lines = vec![format!("Encore {} diagnostics (uid {})", fmt::s(last, "version"), fmt::s(last, "uid"))];
    for c in fmt::items(&last["checks"]) {
        let ok = fmt::truthy(&c, "ok");
        let status = if fmt::truthy(&c, "status") { format!(" → HTTP {}", fmt::s(&c, "status")) } else { String::new() };
        let ms = if c.get("ms").is_some_and(|m| !m.is_null()) { format!(" ({} ms)", fmt::s(&c, "ms")) } else { String::new() };
        let body = if fmt::truthy(&c, "body") && !ok {
            format!("\n     body: {}", fmt::s(&c, "body").chars().take(300).collect::<String>())
        } else {
            String::new()
        };
        lines.push(format!(
            "{} {} – {}{status}{ms}\n     {}{body}",
            if ok { "OK  " } else { "FAIL" },
            fmt::s(&c, "name"),
            fmt::s(&c, "request"),
            fmt::s(&c, "detail")
        ));
    }
    let folders = fmt::items(&last["download_folders"]["roots"]);
    if !folders.is_empty() {
        lines.push(String::new());
        lines.push("Download folders:".into());
        for r in &folders {
            let (what, detail, when) = folder_lines(r);
            lines.push(format!("{} {what}\n     {detail} ({when})", if fmt::truthy(r, "ok") { "OK  " } else { "FAIL" }));
        }
    }
    if last["machine"].is_object() {
        lines.push(String::new());
        lines.push("Server:".into());
        for (k, v) in server_rows(&last["machine"]) {
            lines.push(format!("  {k}: {v}"));
        }
    }
    lines.push(String::new());
    lines.push("Settings:".into());
    for s in fmt::items(&last["settings"]) {
        lines.push(format!(
            "  {} = {}  [{}]",
            fmt::s(&s, "key"),
            super::vs(&s["value"]),
            if s["source"].as_str() == Some("env") { format!("env {}", fmt::s(&s, "env")) } else { "saved".to_string() }
        ));
    }
    lines.join("\n")
}

/// A download folder in words, as the web's `folder_lines` (servers since 0.33, backlog item 17): (what
/// it is, the result with a note or the banner's sentence, when it was probed).
pub fn folder_lines(r: &Value) -> (String, String, String) {
    let what = format!("{} – {}", fmt::s(r, "label"), fmt::s(r, "path"));
    let result = if fmt::truthy(r, "result") { fmt::s(r, "result") } else { "Not checked yet".to_string() };
    let detail = if !fmt::truthy(r, "ok") && fmt::truthy(r, "text") {
        fmt::s(r, "text")
    } else if fmt::truthy(r, "note") {
        format!("{result} – {}", fmt::s(r, "note"))
    } else {
        result
    };
    let when = if fmt::f(r, "checked_at") > 0.0 {
        let since = if !fmt::truthy(r, "ok") && fmt::f(r, "since") > 0.0 { format!(" · not writable since {}", fmt::date(fmt::f(r, "since"))) } else { String::new() };
        format!("Checked {}{since}", fmt::date(fmt::f(r, "checked_at")))
    } else {
        "Not checked yet".to_string()
    };
    (what, detail, when)
}

/// What a download folder's item shows, as the web's: the badge (the result and its kind, Warn for a folder
/// with a note – a client's folder Encore only reads), the detail line only when it says more than the badge,
/// and the time only once the folder was probed.
pub fn folder_shown(r: &Value) -> (String, Badge, Option<String>, Option<String>) {
    let ok = fmt::truthy(r, "ok");
    let (_, detail, when) = folder_lines(r);
    let noted = ok && !matches!(fmt::s(r, "state").as_str(), "ok" | "unknown" | "");
    let result = if fmt::truthy(r, "result") { fmt::s(r, "result") } else { "Not checked yet".to_string() };
    let kind = if !ok { Badge::Err } else if noted { Badge::Warn } else { Badge::Ok };
    let detail = (detail != result).then_some(detail);
    (result, kind, detail, (fmt::f(r, "checked_at") > 0.0).then_some(when))
}

/// One download folder of the Download folders section: its probe's result and time.
fn folder_item<'a>(r: &Value) -> El<'a, Message> {
    let p = theme::look();
    let (result, kind, detail, when) = folder_shown(r);
    let mut body = column![
        row![t(fmt::s(r, "label")).size(14).font(theme::bold()), badge(result, kind)].spacing(8).align_y(Alignment::Center),
        widgets::faint(fmt::s(r, "path")).size(12).font(theme::mono()),
    ]
    .spacing(3)
    .width(Length::Fill);
    if let Some(d) = detail {
        body = body.push(t(d).size(12.5).color(if kind == Badge::Err { p.err } else { p.muted }));
    }
    if let Some(w) = when {
        body = body.push(widgets::faint(w).size(12));
    }
    let colour = match kind {
        Badge::Err => p.err,
        Badge::Warn => p.warn,
        _ => p.ok,
    };
    column![
        container(row![container(dot_in(colour)).padding([6, 0]), body].spacing(12).align_y(Alignment::Start)).padding([10, 14]),
        widgets::divider(),
    ]
    .into()
}

/// One check in full for Copy, as the web's dialog has it: the name, the request, what it said
/// and the whole raw answer (the report keeps 300 characters of it).
pub fn check_text(c: &Value) -> String {
    let detail = match &c["soulseek"] {
        s if s.is_object() => super::tools::session_text_elsewhere(&if fmt::truthy(c, "detail") { fmt::s(c, "detail") } else { super::tools::session_text(s) }),
        _ => fmt::s(c, "detail"),
    };
    [fmt::s(c, "name"), fmt::s(c, "request"), detail, fmt::s(c, "body")]
        .into_iter()
        .filter(|x| !x.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// One app crash report in full for Copy (the web's `crash_text`): what, where and when, then the
/// report.
pub fn crash_text(cr: &Value) -> String {
    let kind = fmt::s(cr, "kind");
    let label = crash_kind(&kind).map(str::to_string).unwrap_or_else(|| if kind.is_empty() { "Crash".to_string() } else { kind.clone() });
    let meta: Vec<String> = ["device", "app_version", "os", "user"].iter().filter(|k| fmt::truthy(cr, k)).map(|k| fmt::s(cr, k)).collect();
    let first = if fmt::truthy(cr, "first_line") { fmt::s(cr, "first_line") } else { "Crash".to_string() };
    let time = if fmt::f(cr, "time") > 0.0 { fmt::stamp(fmt::f(cr, "time")) } else { String::new() };
    let head = [first, label, meta.join(" · "), time].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join("\n");
    let text = fmt::s(cr, "text");
    if text.trim().is_empty() {
        head
    } else {
        format!("{head}\n\n{}", text.trim_end())
    }
}

/// App crash report kinds.
fn crash_kind(k: &str) -> Option<&'static str> {
    Some(match k {
        "crash" => "Crash",
        "native" => "Native crash",
        "anr" => "Not responding",
        "low_memory" => "Closed for memory",
        "signaled" => "Killed by the system",
        "web" => "Web page",
        _ => return None,
    })
}

pub fn update(state: &mut State, msg: Message, ctx: &mut Ctx) -> Task<Message> {
    match msg {
        Message::Run => {
            state.running = true;
            state.error.clear();
            return run(ctx);
        }
        Message::Checks(Ok(d)) => {
            state.running = false;
            state.data = Some(d);
        }
        Message::Checks(Err(e)) => {
            state.running = false;
            state.error = e.message;
        }
        Message::Vpn(Ok(v)) => state.vpn = Some(vpn_row(&v)),
        Message::Vpn(Err(_)) => state.vpn = None,
        Message::CopyReport => {
            if let Some(d) = &state.data {
                let mut all = d.clone();
                if let (Some(v), Some(a)) = (&state.vpn, all.get_mut("checks").and_then(|c| c.as_array_mut())) {
                    a.push(v.clone());
                }
                ctx.copy(report(&all));
                ctx.toast_ok("Copied to clipboard");
            }
        }
        Message::Toggle(k) => {
            if !state.open.remove(&k) {
                state.open.insert(k);
            }
        }
        Message::Backups(Ok(b)) => state.backups = Some(b),
        Message::Backups(Err(e)) => state.backups = Some(json!({ "error": e.message })),
        Message::BackupNow => {
            state.backups_busy = true;
            let api = ctx.api.clone();
            return Task::perform(async move { api.post_empty("/system/backups").await }, Message::BackedUp);
        }
        Message::BackedUp(r) => {
            state.backups_busy = false;
            match r {
                Ok(d) => {
                    ctx.toast_ok(format!("Backed up to backups/{}", fmt::s(&d["created"], "name")));
                    state.backups = Some(d);
                }
                Err(e) => ctx.toast_err(e.message),
            }
        }
        Message::PickBackup => {
            return Task::perform(
                async move {
                    rfd::AsyncFileDialog::new()
                        .set_title("Upload a backup")
                        .add_filter("Database backups", BACKUP_FILES.exts)
                        .pick_file()
                        .await
                        .map(|f| f.path().to_path_buf())
                },
                Message::BackupPicked,
            );
        }
        Message::Drop(ev) => {
            // like *Upload a backup…*: not while a backup is being made, uploaded or restored
            if state.backups_busy {
                state.drop.hovering = false;
                return Task::none();
            }
            match state.drop.handle(ev, &BACKUP_FILES) {
                Outcome::Take(path) => return Task::done(Message::BackupPicked(Some(path))),
                Outcome::Refused(m) => ctx.toast_err(m),
                Outcome::Nothing => {}
            }
        }
        Message::BackupPicked(None) => {}
        Message::BackupPicked(Some(path)) => {
            state.backups_busy = true;
            state.backups = None;
            let api = ctx.api.clone();
            return Task::perform(
                async move {
                    let part = crate::api::Api::file_part(&path).await?;
                    api.post_form("/system/backups/upload", reqwest::multipart::Form::new().part("file", part)).await
                },
                Message::Uploaded,
            );
        }
        Message::Uploaded(r) => {
            state.backups_busy = false;
            match r {
                Ok(d) => {
                    ctx.toast_ok(format!("Saved as backups/{}", fmt::s(&d["created"], "name")));
                    state.backups = Some(d);
                }
                Err(e) => {
                    ctx.toast_err(if e.message.is_empty() { "Upload failed".to_string() } else { e.message });
                    return load_backups(ctx);
                }
            }
        }
        // Through the API client: the browser has no session, so a plain link would end on a 401.
        Message::Download(name) => {
            let api = ctx.api.clone();
            let path = format!("/system/backups/{}", crate::pages::urlencode(&name));
            let file = crate::pages::library::safe_file_name(&name);
            return Task::perform(crate::pages::library::save_as(api, path, file), Message::Downloaded);
        }
        Message::Downloaded(Ok(path)) => {
            if !path.is_empty() {
                ctx.toast_ok(format!("Saved to {path}"));
            }
        }
        Message::Downloaded(Err(e)) => ctx.toast_err(e),
        Message::AskRestore(name) => state.dialog = Some(Dialog::Restore(name)),
        Message::Restore => {
            if let Some(Dialog::Restore(name)) = state.dialog.take() {
                state.backups_busy = true;
                let api = ctx.api.clone();
                return Task::perform(
                    async move { api.post_empty(&format!("/system/backups/{}/restore", crate::pages::urlencode(&name))).await },
                    Message::Restored,
                );
            }
        }
        Message::Restored(r) => {
            state.backups_busy = false;
            match r {
                Ok(_) => {
                    ctx.toast_ok("Backup restored – the server reopens its database; you may have to sign in again");
                    ctx.refresh_session();
                }
                Err(e) => ctx.toast_err(e.message),
            }
            return load_backups(ctx);
        }
        Message::Crashes(Ok(c)) => state.crashes = Some(c),
        Message::Crashes(Err(e)) => state.crashes = Some(json!({ "error": e.message })),
        Message::AskClearCrashes => state.dialog = Some(Dialog::ClearCrashes),
        Message::ClearCrashes => {
            state.dialog = None;
            let api = ctx.api.clone();
            return Task::perform(async move { api.del("/diagnostics/client-crash").await }, Message::Cleared);
        }
        Message::Cleared(r) => {
            if let Err(e) = r {
                ctx.toast_err(e.message);
            }
            return load_crashes(ctx);
        }
        Message::CloseDialog => state.dialog = None,
        Message::CopyText(text) => ctx.copy_text(text),
    }
    Task::none()
}

fn dot<'a>(ok: bool) -> El<'a, Message> {
    let p = theme::look();
    dot_in(if ok { p.ok } else { p.err })
}

/// A status dot in `colour`.
fn dot_in<'a>(colour: iced::Color) -> El<'a, Message> {
    container(space().width(8).height(8))
        .style(move |_| container::Style {
            background: Some(iced::Background::Color(colour)),
            border: iced::Border {
                color: iced::Color::TRANSPARENT,
                width: 0.0,
                radius: 99.0.into(),
            },
            ..container::Style::default()
        })
        .into()
}

/// A folded "Raw response" / "Full report" block.
fn details<'a>(state: &'a State, key: String, summary: &'static str, text: &'a str) -> El<'a, Message> {
    let open = state.open.contains(&key);
    let mut c = column![super::link_muted(if open { format!("▾ {summary}") } else { format!("▸ {summary}") }, Message::Toggle(key))].spacing(6);
    if open {
        c = c.push(
            container(t(text).size(12).font(theme::mono()))
                .padding(10)
                .width(Length::Fill)
                .style(theme::panel2),
        );
    }
    c.into()
}

fn check_item<'a>(state: &'a State, c: &'a Value, i: usize) -> El<'a, Message> {
    let p = theme::look();
    let ok = fmt::truthy(c, "ok");
    let mut head = row![t(fmt::s(c, "name")).size(14).font(theme::bold())].spacing(8).align_y(Alignment::Center);
    if fmt::truthy(c, "status") {
        head = head.push(badge(format!("HTTP {}", fmt::s(c, "status")), if ok { Badge::Ok } else { Badge::Err }));
    }
    if c.get("ms").is_some_and(|m| !m.is_null()) {
        head = head.push(widgets::faint(format!("{} ms", fmt::s(c, "ms"))).size(12));
    }
    if !ok {
        head = head.push(widgets::details::copy_btn(Message::CopyText(check_text(c))));
    }
    let mut body = column![head].spacing(3).width(Length::Fill);
    if fmt::truthy(c, "request") {
        body = body.push(widgets::faint(fmt::s(c, "request")).size(12).font(theme::mono()));
    }
    // the Soulseek session (servers since 0.29): the card's sentence and what is waiting
    let soulseek = &c["soulseek"];
    if soulseek.is_object() {
        use super::tools::{now, session_details, session_text, session_text_elsewhere};
        let detail = if fmt::truthy(c, "detail") { fmt::s(c, "detail") } else { session_text(soulseek) };
        body = body.push(t(session_text_elsewhere(&detail)).size(12.5).color(if ok { p.muted } else { p.err }));
        let more = session_details(soulseek, now());
        if !more.is_empty() {
            body = body.push(widgets::faint(more).size(12));
        }
    } else if fmt::truthy(c, "detail") {
        body = body.push(t(fmt::s(c, "detail")).size(12.5).color(if ok { p.muted } else { p.err }));
    }
    if let Some(raw) = c["body"].as_str().filter(|b| !b.is_empty()) {
        body = body.push(details(state, format!("check-{i}"), "Raw response", raw));
    }
    column![
        container(row![container(dot(ok)).padding([6, 0]), body].spacing(12).align_y(Alignment::Start)).padding([10, 14]),
        widgets::divider(),
    ]
    .into()
}

pub fn view<'a>(state: &'a State, ctx: &Ctx<'a>) -> El<'a, Message> {
    let p = ctx.look;
    let head = widgets::page_head(
        "Diagnostics",
        Some(help("Runs every connection, folder and tool check from inside the Encore container and shows the raw answers. Keys are hidden.").into()),
        vec![
            btn_icon(Icon::Copy, "Copy report", Btn::Default, state.data.as_ref().map(|_| Message::CopyReport)).into(),
            btn_icon(Icon::Refresh, if state.running { "Checking…" } else { "Run checks" }, Btn::Primary, (!state.running).then_some(Message::Run)).into(),
        ],
    );
    let mut page = column![head].spacing(16).width(Length::Fill);
    if !state.error.is_empty() {
        page = page.push(widgets::error_box(&state.error));
    }
    match &state.data {
        Some(d) => {
            let checks: &'a [Value] = d["checks"].as_array().map(|a| a.as_slice()).unwrap_or(&[]);
            let bad = checks.iter().chain(state.vpn.iter()).filter(|c| !fmt::truthy(c, "ok")).count();
            page = page.push(
                widgets::card(
                    row![
                        badge(
                            if bad > 0 { format!("{bad} problem{}", if bad > 1 { "s" } else { "" }) } else { "All good".to_string() },
                            if bad > 0 { Badge::Err } else { Badge::Ok }
                        ),
                        help(format!("Encore {} · running as uid {} · {}", fmt::s(d, "version"), fmt::s(d, "uid"), fmt::date(fmt::f(d, "time")))),
                    ]
                    .spacing(12)
                    .align_y(Alignment::Center),
                )
                .width(Length::Fill),
            );
            if d["machine"].is_object() {
                page = page.push(widgets::section_head("Server", Some(help("What Encore sees of the machine it runs on").into())));
                let mut rows = column![];
                for (k, v) in server_rows(&d["machine"]) {
                    rows = rows.push(
                        container(row![small_muted(k).width(220), t(v).size(13)].spacing(10).align_y(Alignment::Start)).padding([8, 14]),
                    );
                }
                page = page.push(widgets::card_flat(rows).width(Length::Fill));
            }
            let mut i = 0usize;
            for (k, title) in KIND {
                let mut group: Vec<&'a Value> = checks.iter().filter(|c| fmt::s(c, "kind") == k).collect();
                if k == "service" {
                    if let Some(v) = &state.vpn {
                        group.push(v);
                    }
                }
                if group.is_empty() {
                    continue;
                }
                page = page.push(widgets::section_head(title, None));
                let mut list = column![];
                for c in group {
                    list = list.push(check_item(state, c, i));
                    i += 1;
                }
                page = page.push(widgets::card_flat(list).width(Length::Fill));
            }
            let folders = fmt::items(&d["download_folders"]["roots"]);
            if !folders.is_empty() {
                page = page.push(widgets::section_head(
                    "Download folders",
                    Some(help("Probed every minute while downloads run: a file is written, flushed to the disk and removed. While nothing downloads they are looked at every 10 minutes without writing, so idle disks can spin down").into()),
                ));
                let mut list = column![];
                for r in &folders {
                    list = list.push(folder_item(r));
                }
                page = page.push(widgets::card_flat(list).width(Length::Fill));
            }
            // the settings list is long, so it starts folded: the whole heading toggles it, as on the web and Android
            let sources = fmt::items(&d["settings"]);
            let sources_open = state.open.contains(SOURCES);
            let toggle = format!("{} {}", if sources_open { "▾" } else { "▸" }, if sources_open { "Hide".to_string() } else { format!("Show {} settings", sources.len()) });
            page = page.push(
                mouse_area(widgets::section_head(
                    "Where settings come from",
                    Some(
                        row![help("Environment variables always win over values saved in Encore"), super::link_muted(toggle, Message::Toggle(SOURCES.into()))]
                            .spacing(10)
                            .align_y(Alignment::Center)
                            .into(),
                    ),
                ))
                .interaction(iced::mouse::Interaction::Pointer)
                .on_press(Message::Toggle(SOURCES.into())),
            );
            let mut rows = column![];
            for s in sources.iter().filter(|_| sources_open) {
                let source: El<'a, Message> = if s["source"].as_str() == Some("env") {
                    badge(format!("env {}", fmt::s(&s, "env")), Badge::Info)
                } else {
                    badge("saved in Encore", Badge::Plain)
                };
                rows = rows.push(
                    container(
                        row![
                            container(t(fmt::s(&s, "key")).size(12.5).font(theme::mono())).width(Length::FillPortion(2)),
                            container(source).width(Length::FillPortion(1)),
                            container(t(super::vs(&s["value"])).size(12.5).font(theme::mono()).color(p.muted)).width(Length::FillPortion(3)),
                        ]
                        .spacing(10)
                        .align_y(Alignment::Center),
                    )
                    .padding([7, 14]),
                );
            }
            if sources_open {
                page = page.push(widgets::card_flat(rows).width(Length::Fill));
            }
        }
        None => {
            if state.running {
                page = page.push(widgets::skeleton_rows(5));
            }
        }
    }

    // backups
    page = page.push(widgets::section_head(
        "Backups",
        Some(
            row![
                btn_icon(Icon::Download, "Upload a backup…", Btn::Default, (!state.backups_busy).then_some(Message::PickBackup)),
                btn_sm(if state.backups_busy { "Working…" } else { "Back up now" }, Btn::Primary, (!state.backups_busy).then_some(Message::BackupNow)),
            ]
            .spacing(8)
            .into(),
        ),
    ));
    let backups: El<'a, Message> = match &state.backups {
        None => widgets::skeleton_rows(2),
        Some(b) if fmt::truthy(b, "error") => help(fmt::s(b, "error")).into(),
        Some(b) => {
            let list = fmt::items(&b["backups"]);
            if list.is_empty() {
                help("No backups yet – press Back up now, or check the log: Encore takes one at every start when none exists.").into()
            } else {
                let mut c = column![];
                for bk in list {
                    let name = fmt::s(&bk, "name");
                    c = c.push(widgets::list_row(
                        None,
                        name.clone(),
                        format!("{} · {}", fmt::size(fmt::f(&bk, "size")), fmt::date(fmt::f(&bk, "time"))),
                        vec![
                            badge(fmt::s(&bk, "kind"), Badge::Plain),
                            btn_icon(Icon::Download, "Download", Btn::Default, Some(Message::Download(name.clone()))).into(),
                            btn_sm("Restore…", Btn::Default, (!state.backups_busy).then_some(Message::AskRestore(name))).into(),
                        ],
                        None::<Message>,
                    ));
                }
                c.into()
            }
        }
    };
    page = page.push(drop_zone::view(
        widgets::card(
            column![
                help("Encore keeps copies of its database here (daily, and before every upgrade). Download one now and then to keep it somewhere else."),
                backups
            ]
            .spacing(10)
            .width(Length::Fill),
        )
        .width(Length::Fill),
        &state.drop,
        "Drop a database backup (.db, .sqlite, .sqlite3) to upload it",
    ));

    // app crashes
    let crashes = state.crashes.as_ref().map(|c| fmt::items(&c["crashes"])).unwrap_or_default();
    page = page.push(widgets::section_head(
        "App crashes",
        Some(btn_sm("Clear", Btn::Default, (!crashes.is_empty()).then_some(Message::AskClearCrashes)).into()),
    ));
    let crash_list: El<'a, Message> = match &state.crashes {
        None => widgets::skeleton_rows(1),
        Some(c) if fmt::truthy(c, "error") => help(fmt::s(c, "error")).into(),
        Some(_) if crashes.is_empty() => help("No app crashes reported.").into(),
        Some(c) => {
            let list = c["crashes"].as_array().map(|a| a.as_slice()).unwrap_or(&[]);
            let mut col = column![];
            for (i, cr) in list.iter().enumerate() {
                let kind = fmt::s(cr, "kind");
                let label = crash_kind(&kind).map(str::to_string).unwrap_or_else(|| if kind.is_empty() { "Crash".to_string() } else { kind.clone() });
                let meta: Vec<String> = ["device", "app_version", "os", "user"].iter().filter(|k| fmt::truthy(cr, k)).map(|k| fmt::s(cr, k)).collect();
                let first = if fmt::truthy(cr, "first_line") { fmt::s(cr, "first_line") } else { "Crash".to_string() };
                let text = cr["text"].as_str().unwrap_or("");
                let mut body = column![
                    row![
                        t(first).size(14).font(theme::bold()),
                        badge(label, Badge::Plain),
                        widgets::details::copy_btn(Message::CopyText(crash_text(cr))),
                    ]
                    .spacing(8)
                    .align_y(Alignment::Center),
                    small_muted(format!("{} · {}", meta.join(" · "), fmt::date(fmt::f(cr, "time")))),
                ]
                .spacing(3)
                .width(Length::Fill);
                if !text.is_empty() {
                    body = body.push(details(state, format!("crash-{i}"), "Full report", text));
                }
                col = col.push(column![
                    container(row![container(dot(false)).padding([6, 0]), body].spacing(12).align_y(Alignment::Start)).padding([10, 14]),
                    widgets::divider(),
                ]);
            }
            col.into()
        }
    };
    page = page.push(
        widgets::card(
            column![
                help("The phone app sends a report the next time it opens after it crashed or the system closed it. The newest 50 are kept."),
                crash_list
            ]
            .spacing(10)
            .width(Length::Fill),
        )
        .width(Length::Fill),
    );
    page.into()
}

pub fn overlay<'a>(state: &'a State, _ctx: &Ctx<'a>) -> Option<El<'a, Message>> {
    Some(match state.dialog.as_ref()? {
        Dialog::Restore(name) => {
            let text = format!("Encore replaces its current database with {name}. A safety copy of the current data is taken first, then the server reopens the database: everything saved since that backup (library index, requests, playlists, settings and sign-ins) is replaced – you may have to sign in again.");
            modal(
                "Restore this backup?",
                widgets::muted(text).size(14),
                Some(
                    row![
                        btn("Cancel", Btn::Default, Some(Message::CloseDialog)),
                        btn("Restore", Btn::Danger, Some(Message::Restore)),
                    ]
                    .spacing(10)
                    .into(),
                ),
                false,
                Message::CloseDialog,
            )
        }
        Dialog::ClearCrashes => widgets::confirm(
            "Clear app crashes?",
            "All crash reports from the phone app are removed.",
            "Clear",
            true,
            Message::ClearCrashes,
            Message::CloseDialog,
        ),
    })
}

/// The database backups *Upload a backup…* takes, picked or dropped.
const BACKUP_FILES: drop_zone::Accept = drop_zone::Accept::one(&["db", "sqlite", "sqlite3"]);

/// The window's drag and drop, for *Upload a backup…*.
pub fn subscription(_state: &State) -> iced::Subscription<Message> {
    drop_zone::events().map(Message::Drop)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vpn_words_match_the_web() {
        assert_eq!(vpn_text(&Value::Null), "Off");
        assert_eq!(vpn_text(&json!({ "state": "connecting" })), "Connecting…");
        assert_eq!(vpn_text(&json!({ "state": "error" })), "The VPN could not connect");
        assert_eq!(
            vpn_text(&json!({ "state": "up", "forwarded_port": 51234, "public_ip": "1.2.3.4", "last_handshake_secs_ago": 12.4 })),
            "Connected · port 51234 · public IP 1.2.3.4 · last handshake 12 s ago"
        );
        assert_eq!(vpn_text(&json!({ "state": "up", "port_forwarding": true })), "Connected · no forwarded port yet");
    }

    #[test]
    fn the_report_and_server_rows() {
        let d = json!({ "version": "0.27.0", "uid": 1000, "checks": [{ "name": "MusicBrainz", "ok": false, "request": "GET x", "detail": "timeout", "status": 504, "ms": 30, "body": "bad" }],
            "machine": { "cpus": 4, "memory": 8e9, "memory_source": "cgroup", "config": { "path": "/config", "fs": "ext4", "network": false }, "database_bytes": 1e6, "db_readers": 3 },
            "settings": [{ "key": "app_name", "value": "Encore", "source": "env", "env": "ENCORE_APP_NAME" }] });
        let r = report(&d);
        assert!(r.starts_with("Encore 0.27.0 diagnostics (uid 1000)"));
        assert!(r.contains("FAIL MusicBrainz – GET x → HTTP 504 (30 ms)"));
        assert!(r.contains("body: bad"));
        assert!(r.contains("  CPUs: 4"));
        assert!(r.contains("  Memory: 8.0 GB (container limit)"));
        assert!(r.contains("  Config folder: /config · ext4 (local)"));
        assert!(r.contains("app_name = Encore  [env ENCORE_APP_NAME]"));
        let rows = server_rows(&json!({}));
        assert_eq!(rows[0].1, "unknown");
        assert_eq!(rows[2].1, "not set");
        assert_eq!(crash_kind("anr"), Some("Not responding"));
        // Copy takes one check whole, the raw answer included
        let long = "x".repeat(900);
        let c = json!({ "name": "MusicBrainz", "ok": false, "request": "GET x", "detail": "timeout", "body": long });
        assert_eq!(check_text(&c), format!("MusicBrainz\nGET x\ntimeout\n{long}"));
        let cr = json!({ "kind": "anr", "first_line": "ANR in app", "device": "Pixel", "app_version": "0.30", "time": 1_792_427_497.0, "text": "trace\n" });
        assert_eq!(crash_text(&cr), "ANR in app\nNot responding\nPixel · 0.30\n2026-10-19 16:31 UTC\n\ntrace");
    }

    /// Each download folder with its probe's result and time, as the web shows them.
    #[test]
    fn the_download_folders() {
        let d = json!({ "version": "0.33.0", "uid": 1000, "checks": [], "download_folders": { "ok": false, "roots": [
            { "key": "staging_path", "label": "Staging folder (yt-dlp, streamrip, links)", "path": "/downloads/encore", "state": "read_only", "result": "Read-only", "ok": false,
              "text": "The downloads folder /downloads/encore is read-only – check the disk (dmesg, fsck) or the mount", "checked_at": 1_792_427_497.0, "since": 1_792_427_437.0 },
            { "key": "qbit_download_path", "label": "Torrent downloads (qBittorrent)", "path": "/downloads/torrents", "state": "denied", "result": "Permission denied", "ok": true,
              "note": "Encore may not write here, which is fine", "checked_at": 1_792_427_497.0 },
            { "key": "sab_download_path", "label": "Usenet downloads (SABnzbd)", "path": "/downloads/usenet", "state": "unknown", "result": "Not checked yet", "ok": true, "checked_at": null } ] } });
        let roots = fmt::items(&d["download_folders"]["roots"]);
        assert_eq!(folder_lines(&roots[0]), ("Staging folder (yt-dlp, streamrip, links) – /downloads/encore".to_string(),
            "The downloads folder /downloads/encore is read-only – check the disk (dmesg, fsck) or the mount".to_string(),
            format!("Checked {} · not writable since {}", fmt::date(1_792_427_497.0), fmt::date(1_792_427_437.0))));
        assert_eq!(folder_lines(&roots[1]).1, "Permission denied – Encore may not write here, which is fine");
        assert_eq!(folder_lines(&roots[2]).2, "Not checked yet");
        // on the page the result is said once (the badge), a noted folder gets the warn colour, no time before
        // the first probe
        assert_eq!(folder_shown(&roots[1]).1, Badge::Warn);
        assert_eq!(folder_shown(&roots[2]), ("Not checked yet".to_string(), Badge::Ok, None, None));
        let fine = json!({ "state": "ok", "result": "Writable", "ok": true, "checked_at": 1_792_427_497.0 });
        assert_eq!(folder_shown(&fine), ("Writable".to_string(), Badge::Ok, None, Some(format!("Checked {}", fmt::date(1_792_427_497.0)))));
        assert_eq!(folder_shown(&roots[0]).2.as_deref(), Some("The downloads folder /downloads/encore is read-only – check the disk (dmesg, fsck) or the mount"));
        let r = report(&d);
        assert!(r.contains("Download folders:\nFAIL Staging folder (yt-dlp, streamrip, links) – /downloads/encore\n     The downloads folder /downloads/encore is read-only"), "{r}");
        assert!(r.contains("OK   Torrent downloads (qBittorrent) – /downloads/torrents"), "{r}");
    }
}
