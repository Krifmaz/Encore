//! Soulseek: Search, browse users and transfers. Behaviour reference: web-rs/src/pages/soulseek.rs.
//! Search the network, browse what a user shares, download files or whole folders, watch
//! transfers. Everything goes through /api/soulseek; a download becomes an ordinary request the
//! worker tags and imports.

use crate::api::ApiError;
use crate::fmt;
use crate::pages::{urlencode, Ctx, El};
use crate::theme::{self, Badge, Btn};
use crate::widgets::picker::{count_tabs, settings_page};
use crate::widgets::{
    self, badge, btn_icon, card_flat, empty, error_box, h1, h2, icon, icon_btn, modal, muted, search_box,
    small_muted, t, Icon,
};
use iced::widget::{button, checkbox, column, container, progress_bar, row, space};
use iced::{Alignment, Background, Border, Length, Subscription, Task};
use serde_json::{json, Map, Value};
use std::collections::HashSet;
use std::time::Duration;

const LOSSLESS: [&str; 7] = ["flac", "alac", "wav", "aiff", "aif", "ape", "wv"];
/// Result cards shown per search; the network can answer with far more.
const MAX_USERS: usize = 150;
/// Folders listed in the browse dialog before asking for a filter.
const MAX_FOLDERS: usize = 400;

/// slskd transfer states: the badge and its short label; anything else is prettified.
fn states(state: &str) -> Option<(Badge, &'static str)> {
    Some(match state {
        "Requested" => (Badge::Plain, "Requested"),
        "Queued, Remotely" => (Badge::Info, "In their queue"),
        "Queued, Locally" => (Badge::Info, "Queued"),
        "Initializing" => (Badge::Accent, "Starting"),
        "InProgress" => (Badge::Accent, "Downloading"),
        "Completed, Succeeded" => (Badge::Ok, "Done"),
        "Completed, Errored" => (Badge::Err, "Failed"),
        "Completed, Cancelled" => (Badge::Plain, "Cancelled"),
        "Completed, TimedOut" => (Badge::Err, "Timed out"),
        "Completed, Rejected" => (Badge::Err, "Rejected"),
        "Completed, Aborted" => (Badge::Err, "Aborted"),
        _ => return None,
    })
}

// ---- small helpers -------------------------------------------------------------------------

fn arr<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v[key].as_array().map(Vec::as_slice).unwrap_or(&[])
}

/// Last component of a Soulseek path (backslashes from Windows clients, slashes from others).
pub fn base_name(p: &str) -> String {
    p.split(['\\', '/']).rfind(|s| !s.is_empty()).unwrap_or("").to_string()
}

/// The file's extension, lower case, from `extension` or its name.
fn ext_of(f: &Value) -> String {
    let e = if fmt::truthy(f, "extension") {
        fmt::s(f, "extension")
    } else {
        let name = if fmt::truthy(f, "name") { fmt::s(f, "name") } else { base_name(&fmt::s(f, "filename")) };
        name.rsplit('.').next().unwrap_or("").to_string()
    };
    e.strip_prefix('.').unwrap_or(&e).to_lowercase()
}

fn is_lossless(f: &Value) -> bool {
    LOSSLESS.contains(&ext_of(f).as_str())
}

/// "user\nfilename": the key of a ticked file.
fn file_key(username: &str, f: &Value) -> String {
    format!("{username}\n{}", fmt::s(f, "filename"))
}

fn is_finished(t: &Value) -> bool {
    fmt::s(t, "state").starts_with("Completed")
}

fn is_active(t: &Value) -> bool {
    !is_finished(t)
}

/// `1 file`, `3 files`, with thousands separators.
pub fn plural(n: usize, w: &str) -> String {
    format!("{} {w}{}", num(n), if n == 1 { "" } else { "s" })
}

/// `toLocaleString`: 1,234.
pub fn num(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

pub fn speed_text(b: f64) -> String {
    if b >= 1e6 {
        format!("{:.1} MB/s", b / 1e6)
    } else {
        format!("{} kB/s", (b / 1e3).round() as i64)
    }
}

/// `Math.max(0, Math.min(100, Math.round(t.progress ?? (t.size ? bytes / size * 100 : 0))))`.
pub fn pct(t: &Value) -> i64 {
    let p = if !t["progress"].is_null() {
        fmt::f(t, "progress")
    } else if fmt::truthy(t, "size") {
        fmt::f(t, "bytes") / fmt::f(t, "size") * 100.0
    } else {
        0.0
    };
    p.round().clamp(0.0, 100.0) as i64
}

/// Free slot first, then the fastest uploaders.
pub fn sort_users(mut users: Vec<Value>) -> Vec<Value> {
    let slot = |u: &Value| i32::from(fmt::truthy(u, "hasFreeUploadSlot"));
    users.sort_by(|a, b| {
        slot(b).cmp(&slot(a)).then_with(|| {
            fmt::f(b, "uploadSpeed")
                .partial_cmp(&fmt::f(a, "uploadSpeed"))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    });
    users
}

/// A number as JavaScript prints it (44.1, 96).
fn js_num(x: f64) -> String {
    if x.fract() == 0.0 {
        format!("{}", x as i64)
    } else {
        let s = format!("{x}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// Same text as the picker's quality badge: "FLAC 24-bit/96kHz", "MP3 320k".
pub fn quality(f: &Value) -> String {
    let ext = ext_of(f);
    let up = if ext.is_empty() { "?".to_string() } else { ext.to_uppercase() };
    if LOSSLESS.contains(&ext.as_str()) {
        let bits = if fmt::truthy(f, "bitDepth") { format!(" {}-bit", fmt::s(f, "bitDepth")) } else { String::new() };
        let khz = if fmt::truthy(f, "sampleRate") && fmt::f(f, "sampleRate") != 44100.0 {
            format!("/{}kHz", js_num(fmt::f(f, "sampleRate") / 1000.0))
        } else {
            String::new()
        };
        return format!("{up}{bits}{khz}");
    }
    if fmt::truthy(f, "bitRate") {
        format!("{up} {}k", fmt::s(f, "bitRate"))
    } else {
        up
    }
}

/// `.replace(/([a-z])([A-Z])/g, '$1 $2').replace(/,\s*/g, ' · ')`.
pub fn prettify(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut o = String::with_capacity(s.len() + 8);
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == ',' {
            o.push_str(" · ");
            i += 1;
            while i < chars.len() && chars[i].is_whitespace() {
                i += 1;
            }
            continue;
        }
        o.push(c);
        if c.is_ascii_lowercase() && chars.get(i + 1).is_some_and(|n| n.is_ascii_uppercase()) {
            o.push(' ');
        }
        i += 1;
    }
    o
}

/// Humanised slskd state as a badge; uploads say "Uploading" instead of "Downloading".
pub fn state_words(state: &str, upload: bool) -> (Badge, String) {
    let (kind, mut label) = match states(state) {
        Some((k, l)) => (k, l.to_string()),
        None => (Badge::Plain, prettify(if state.is_empty() { "Unknown" } else { state })),
    };
    if upload && state == "InProgress" {
        label = "Uploading".into();
    }
    (kind, label)
}

fn state_badge<'a>(state: &str, upload: bool) -> El<'a, Message> {
    let (kind, label) = state_words(state, upload);
    badge(label, kind)
}

// ---- state ---------------------------------------------------------------------------------

#[derive(Debug)]
struct Search {
    id: String,
    seconds: String,
    q: String,
}

#[derive(Debug, Default)]
enum SearchStatus {
    #[default]
    Idle,
    Starting,
    /// One Soulseek search runs at a time: this one waits for another (30 s at most).
    Waiting,
    Running {
        n: usize,
    },
    Done {
        n: usize,
        files: usize,
        q: String,
    },
    Err(String),
}

/// The browse dialog: everything a user shares, with a filter over the folders.
#[derive(Debug)]
struct Browse {
    username: String,
    /// None while the file list loads; Err(message) when it failed.
    folders: Option<Result<Vec<Value>, String>>,
    filter: String,
    open: HashSet<usize>,
    checked: HashSet<String>,
    /// Folders whose download is being sent.
    sending: HashSet<usize>,
}

#[derive(Debug, Default)]
pub struct State {
    tab: String,
    /// `/api/soulseek/status`.
    status: Value,
    status_err: String,
    status_known: bool,
    enabled: bool,
    /// 403: the account may not pick downloads.
    forbidden: bool,
    q: String,
    search: Option<Search>,
    polling: bool,
    s_status: SearchStatus,
    /// The search generation: answers of an older search are dropped.
    gen: u64,
    results: Vec<Value>,
    open_users: HashSet<String>,
    /// "user\nfilename" of every ticked file in the results.
    checked: HashSet<String>,
    /// "user\nfolder" of the folders whose download is being sent.
    sending: HashSet<String>,
    transfers: Value,
    transfers_err: String,
    cancelling: HashSet<String>,
    clearing: bool,
    browse: Option<Browse>,
    ticks: u64,
}

#[derive(Clone, Debug)]
pub enum Message {
    Tab(String),
    Status(Result<Value, ApiError>),
    /// Every 2 s while the page is open.
    Tick,
    /// Every second while a search runs.
    PollTick,
    Query(String),
    Search,
    Started(u64, Result<Value, ApiError>),
    /// A second into a start that hasn't answered: GET /system/soulseek, to say whether it waits for another search.
    TurnCheck(u64, Result<Value, ApiError>),
    Polled(u64, Result<Value, ApiError>),
    ToggleUser(String),
    ToggleFile(String, bool),
    /// Download a folder of a result (`selected`: only the ticked files).
    Download { user: usize, folder: usize, selected: bool },
    /// `(the sending key, the keys to untick, in the browse dialog, the answer)`.
    Downloaded(String, Vec<String>, bool, Result<Value, ApiError>),
    Browse(String),
    Browsed(String, Result<Value, ApiError>),
    BrowseFilter(String),
    BrowseFolder(usize),
    BrowseFile(String, bool),
    BrowseDownload { folder: usize, selected: bool },
    CloseDialog,
    Transfers(Result<Value, ApiError>),
    Cancel(usize),
    Cancelled(String, Result<Value, ApiError>),
    ClearFinished,
    Cleared(Result<Value, ApiError>),
    GoSettings,
}

fn load_status(ctx: &Ctx) -> Task<Message> {
    let api = ctx.api.clone();
    Task::perform(async move { api.get("/soulseek/status").await }, Message::Status)
}

fn load_transfers(ctx: &Ctx) -> Task<Message> {
    let api = ctx.api.clone();
    Task::perform(async move { api.get("/soulseek/transfers").await }, Message::Transfers)
}

/// Called when the app navigates here.
pub fn open(state: &mut State, ctx: &mut Ctx) -> Task<Message> {
    state.tab = "search".into();
    state.status_known = false;
    state.enabled = true;
    state.forbidden = false;
    state.status_err.clear();
    state.gen += 1;
    state.search = None;
    state.polling = false;
    state.s_status = SearchStatus::Idle;
    state.results.clear();
    state.open_users.clear();
    state.checked.clear();
    state.sending.clear();
    state.transfers = json!({ "downloads": [], "uploads": [] });
    state.transfers_err.clear();
    state.cancelling.clear();
    state.clearing = false;
    state.browse = None;
    state.ticks = 0;
    Task::batch([load_status(ctx), load_transfers(ctx)])
}

fn poll(state: &mut State, ctx: &Ctx) -> Task<Message> {
    let Some(s) = &state.search else {
        return Task::none();
    };
    if state.polling {
        return Task::none();
    }
    state.polling = true;
    let api = ctx.api.clone();
    let url = format!("/soulseek/search/{}", urlencode(&s.id));
    let gen = state.gen;
    Task::perform(async move { api.get(&url).await }, move |r| Message::Polled(gen, r))
}

/// `POST /api/soulseek/download`: the worker fetches, tags and imports the files like a pick.
/// JSON.stringify drops undefined: only keys the objects have go out.
pub fn download_body(username: &str, folder: &Value, files: &[Value]) -> Value {
    let mut body = Map::new();
    body.insert("username".into(), json!(username));
    if folder.get("directory").is_some() {
        body.insert("directory".into(), folder["directory"].clone());
    }
    body.insert(
        "files".into(),
        Value::Array(
            files
                .iter()
                .map(|f| {
                    let mut m = Map::new();
                    if f.get("filename").is_some() {
                        m.insert("filename".into(), f["filename"].clone());
                    }
                    if f.get("size").is_some() {
                        m.insert("size".into(), f["size"].clone());
                    }
                    Value::Object(m)
                })
                .collect(),
        ),
    );
    body.insert("kind".into(), json!(if files.len() > 1 { "album" } else { "track" }));
    Value::Object(body)
}

fn send_download(
    ctx: &Ctx,
    username: &str,
    folder: &Value,
    files: Vec<Value>,
    key: String,
    selected: bool,
    browse: bool,
) -> Task<Message> {
    let body = download_body(username, folder, &files);
    let unticks: Vec<String> = if selected { files.iter().map(|f| file_key(username, f)).collect() } else { Vec::new() };
    let api = ctx.api.clone();
    Task::perform(
        async move { api.post("/soulseek/download", body).await },
        move |r| Message::Downloaded(key.clone(), unticks.clone(), browse, r),
    )
}

pub fn update(state: &mut State, msg: Message, ctx: &mut Ctx) -> Task<Message> {
    match msg {
        Message::Tab(tab) => {
            state.tab = tab;
            if state.tab == "transfers" && state.enabled {
                return load_transfers(ctx);
            }
        }
        Message::Status(Ok(s)) => {
            state.enabled = fmt::truthy(&s, "enabled");
            state.forbidden = false;
            state.status = s;
            state.status_err.clear();
            state.status_known = true;
        }
        Message::Status(Err(e)) => {
            // 400 "not enabled", 403 may not pick downloads
            state.enabled = !matches!(e.status, 400 | 403);
            state.forbidden = e.status == 403;
            state.status = Value::Null;
            state.status_err = e.message;
            state.status_known = true;
        }
        Message::Tick => {
            state.ticks += 1;
            let mut tasks = Vec::new();
            // every 30 s, every 10 s while Soulseek isn't logged in (the line follows the reconnects)
            let down = super::settings::tools::has_session(&state.status) && fmt::s(&state.status["session"], "state") != "logged_in";
            if state.ticks.is_multiple_of(15) || (down && state.ticks.is_multiple_of(5)) {
                tasks.push(load_status(ctx));
            }
            // 2 s on the tab, 10 s for the badge
            if state.enabled && (state.tab == "transfers" || state.ticks.is_multiple_of(5)) {
                tasks.push(load_transfers(ctx));
            }
            return Task::batch(tasks);
        }
        Message::PollTick => return poll(state, ctx),
        Message::Query(q) => state.q = q,
        Message::Search => {
            let q = state.q.trim().to_string();
            if q.is_empty() {
                return Task::none();
            }
            state.gen += 1;
            state.search = None;
            state.polling = false;
            state.results.clear();
            state.checked.clear();
            state.open_users.clear();
            state.s_status = SearchStatus::Starting;
            let api = ctx.api.clone();
            let api2 = ctx.api.clone();
            let gen = state.gen;
            return Task::batch([
                Task::perform(async move { api.post("/soulseek/search", json!({ "q": q })).await }, move |r| Message::Started(gen, r)),
                // a start that takes over a second waits for another search (one runs at a time)
                Task::perform(
                    async move {
                        tokio::time::sleep(Duration::from_secs(1)).await;
                        api2.get("/system/soulseek").await
                    },
                    move |r| Message::TurnCheck(gen, r),
                ),
            ]);
        }
        Message::TurnCheck(gen, r) => {
            if gen == state.gen && matches!(state.s_status, SearchStatus::Starting) && r.is_ok_and(|v| fmt::truthy(&v, "search_running")) {
                state.s_status = SearchStatus::Waiting;
            }
        }
        Message::Started(gen, r) => {
            if gen != state.gen {
                return Task::none();
            }
            match r {
                Ok(s) => {
                    state.search = Some(Search {
                        id: fmt::s(&s, "id"),
                        seconds: fmt::s(&s, "seconds"),
                        q: state.q.trim().to_string(),
                    });
                    state.s_status = SearchStatus::Running { n: 0 };
                    return poll(state, ctx);
                }
                Err(e) => {
                    let down = e.status == 409;
                    state.s_status = SearchStatus::Err(e.message);
                    // 409: the session is down – the line at the top says why and when the next attempt is
                    if down {
                        return load_status(ctx);
                    }
                }
            }
        }
        Message::Polled(gen, r) => {
            if gen != state.gen {
                return Task::none();
            }
            state.polling = false;
            match r {
                Ok(r) => {
                    state.results = sort_users(fmt::items(&r["results"]));
                    let n = state.results.len();
                    let files: usize = state
                        .results
                        .iter()
                        .map(|u| arr(u, "folders").iter().map(|f| arr(f, "files").len()).sum::<usize>())
                        .sum();
                    if fmt::truthy(&r, "done") {
                        let q = if fmt::truthy(&r, "q") {
                            fmt::s(&r, "q")
                        } else {
                            state.search.as_ref().map(|s| s.q.clone()).unwrap_or_default()
                        };
                        state.s_status = SearchStatus::Done { n, files, q };
                        state.search = None;
                        if state.open_users.is_empty() {
                            if let Some(first) = state.results.first() {
                                state.open_users.insert(fmt::s(first, "username"));
                            }
                        }
                    } else {
                        state.s_status = SearchStatus::Running { n };
                    }
                }
                Err(e) => {
                    state.s_status = SearchStatus::Err(if e.status == 404 {
                        "That search has expired – run it again.".into()
                    } else {
                        e.message
                    });
                    state.search = None;
                }
            }
        }
        Message::ToggleUser(u) => {
            if !state.open_users.remove(&u) {
                state.open_users.insert(u);
            }
        }
        Message::ToggleFile(key, on) => {
            if on {
                state.checked.insert(key);
            } else {
                state.checked.remove(&key);
            }
        }
        Message::Download { user, folder, selected } => {
            let Some(u) = state.results.get(user) else {
                return Task::none();
            };
            let Some(f) = arr(u, "folders").get(folder) else {
                return Task::none();
            };
            let username = fmt::s(u, "username");
            let files: Vec<Value> = if selected {
                arr(f, "files")
                    .iter()
                    .filter(|x| state.checked.contains(&file_key(&username, x)))
                    .cloned()
                    .collect()
            } else {
                arr(f, "files").to_vec()
            };
            let key = format!("{username}\n{}", fmt::s(f, "directory"));
            if files.is_empty() || state.sending.contains(&key) {
                return Task::none();
            }
            let f = f.clone();
            state.sending.insert(key.clone());
            return send_download(ctx, &username, &f, files, key, selected, false);
        }
        Message::Downloaded(key, unticks, browse, r) => {
            if browse {
                if let Some(b) = state.browse.as_mut() {
                    b.sending.retain(|j| j.to_string() != key);
                    if r.is_ok() {
                        for k in &unticks {
                            b.checked.remove(k);
                        }
                    }
                }
            } else {
                state.sending.remove(&key);
                if r.is_ok() {
                    for k in &unticks {
                        state.checked.remove(k);
                    }
                }
            }
            match r {
                Ok(_) => ctx.toast_ok("Added to your requests"),
                Err(e) => ctx.toast_err(e.message),
            }
        }
        Message::Browse(username) => {
            state.browse = Some(Browse {
                username: username.clone(),
                folders: None,
                filter: String::new(),
                open: HashSet::new(),
                checked: HashSet::new(),
                sending: HashSet::new(),
            });
            let api = ctx.api.clone();
            let url = format!("/soulseek/browse/{}", urlencode(&username));
            return Task::perform(async move { api.get(&url).await }, move |r| Message::Browsed(username.clone(), r));
        }
        Message::Browsed(username, r) => {
            if let Some(b) = state.browse.as_mut().filter(|b| b.username == username) {
                b.folders = Some(match r {
                    Ok(d) => Ok(fmt::items(&d["folders"])),
                    Err(e) => Err(e.message),
                });
            }
        }
        Message::BrowseFilter(f) => {
            if let Some(b) = state.browse.as_mut() {
                b.filter = f;
            }
        }
        Message::BrowseFolder(j) => {
            if let Some(b) = state.browse.as_mut() {
                if !b.open.remove(&j) {
                    b.open.insert(j);
                }
            }
        }
        Message::BrowseFile(key, on) => {
            if let Some(b) = state.browse.as_mut() {
                if on {
                    b.checked.insert(key);
                } else {
                    b.checked.remove(&key);
                }
            }
        }
        Message::BrowseDownload { folder, selected } => {
            let Some(b) = state.browse.as_mut() else {
                return Task::none();
            };
            let Some(Ok(folders)) = &b.folders else {
                return Task::none();
            };
            let Some(f) = folders.get(folder) else {
                return Task::none();
            };
            let files: Vec<Value> = if selected {
                arr(f, "files")
                    .iter()
                    .filter(|x| b.checked.contains(&file_key(&b.username, x)))
                    .cloned()
                    .collect()
            } else {
                arr(f, "files").to_vec()
            };
            if files.is_empty() || b.sending.contains(&folder) {
                return Task::none();
            }
            b.sending.insert(folder);
            let (username, f) = (b.username.clone(), f.clone());
            return send_download(ctx, &username, &f, files, folder.to_string(), selected, true);
        }
        Message::CloseDialog => state.browse = None,
        Message::Transfers(Ok(t)) => {
            state.transfers = t;
            state.transfers_err.clear();
        }
        Message::Transfers(Err(e)) => {
            if arr(&state.transfers, "downloads").is_empty() {
                state.transfers_err = e.message;
            }
        }
        Message::Cancel(i) => {
            let Some(x) = arr(&state.transfers, "downloads").get(i).cloned() else {
                return Task::none();
            };
            let id = fmt::s(&x, "id");
            if !state.cancelling.insert(id.clone()) {
                return Task::none();
            }
            let api = ctx.api.clone();
            let url = format!(
                "/soulseek/transfers/{}/{}",
                urlencode(&fmt::s(&x, "username")),
                urlencode(&id)
            );
            return Task::perform(async move { api.del(&url).await }, move |r| Message::Cancelled(id.clone(), r));
        }
        Message::Cancelled(id, r) => {
            state.cancelling.remove(&id);
            match r {
                Ok(_) => return load_transfers(ctx),
                Err(e) => ctx.toast_err(e.message),
            }
        }
        Message::ClearFinished => {
            if state.clearing {
                return Task::none();
            }
            state.clearing = true;
            let api = ctx.api.clone();
            return Task::perform(
                async move { api.post_empty("/soulseek/transfers/clear").await },
                Message::Cleared,
            );
        }
        Message::Cleared(r) => {
            state.clearing = false;
            match r {
                Ok(_) => return load_transfers(ctx),
                Err(e) => ctx.toast_err(e.message),
            }
        }
        Message::GoSettings => ctx.navigate(settings_page("#/settings?s=slskd")),
    }
    Task::none()
}

// ---- view ----------------------------------------------------------------------------------

/// A small accent link.
fn link<'a>(label: &'a str, on: Message) -> El<'a, Message> {
    let p = theme::look();
    button(t(label).size(12.5).color(p.accent))
        .padding(0)
        .style(theme::btn(Btn::Row))
        .on_press(on)
        .into()
}

/// `.dot`: the status light.
fn dot<'a>(color: iced::Color) -> El<'a, Message> {
    container(space())
        .width(8)
        .height(8)
        .style(move |_| container::Style {
            background: Some(Background::Color(color)),
            border: Border {
                radius: 99.0.into(),
                ..Border::default()
            },
            ..container::Style::default()
        })
        .into()
}

/// The parts of the status line: `(dot colour, words, warn-coloured words, with the settings link)`.
pub fn status_parts(s: &Value, error: &str) -> (&'static str, Vec<(String, bool)>, bool) {
    if !error.is_empty() {
        return ("err", vec![(error.to_string(), false)], true);
    }
    let state = fmt::s(s, "state");
    let dash = |key: &str| if fmt::truthy(s, key) { format!(" – {}", fmt::s(s, key)) } else { String::new() };
    if fmt::truthy(s, "configured") && state == "not_installed" {
        return ("err", vec![(format!("Soulseek is not installed{}", dash("hint")), true)], true);
    }
    if !fmt::truthy(s, "enabled") {
        return ("", vec![("Soulseek is not enabled – turn it on in".into(), false)], true);
    }
    if state == "waiting_vpn" {
        let w = if fmt::truthy(s, "error") { fmt::s(s, "error") } else { "Waiting for the VPN".into() };
        return ("", vec![(w, true)], true);
    }
    if state == "crashed" {
        return ("err", vec![(format!("Soulseek stopped{}", dash("error")), true)], true);
    }
    if state == "blocked" {
        return ("err", vec![(format!("Soulseek can't start{}", dash("error")), true)], true);
    }
    if state == "starting" {
        let mut parts = vec![("Soulseek is starting…".to_string(), false)];
        if fmt::truthy(s, "error") {
            parts.push((fmt::s(s, "error"), true));
        }
        return ("", parts, false);
    }
    // the session (servers since 0.29): Encore's own view of the login – why it is down and when the next attempt is
    use super::settings::tools::{has_session, session_text, session_text_elsewhere};
    let session = &s["session"];
    let with_session = has_session(s);
    let logged_in = if with_session { fmt::s(session, "state") == "logged_in" } else { fmt::truthy(s, "logged_in") };
    if with_session && !logged_in {
        let (dot, warn) = match fmt::s(session, "state").as_str() {
            "connecting" => ("", false),
            "stopped" if fmt::truthy(session, "reason") => ("err", true),
            "stopped" => ("", true),
            _ => ("warn", true),
        };
        return (dot, vec![(session_text_elsewhere(&session_text(session)), warn)], true);
    }
    if !logged_in {
        return ("err", vec![(format!("Not signed in{}", dash("error")), true)], true);
    }
    let name = if fmt::truthy(s, "username") {
        fmt::s(s, "username")
    } else if with_session && fmt::truthy(session, "username") {
        fmt::s(session, "username")
    } else {
        "?".into()
    };
    let mut parts = vec![(format!("Signed in as {name}"), false)];
    parts.push((
        if fmt::truthy(&s["shares"], "files") {
            format!("sharing {}", plural(fmt::i(&s["shares"], "files").max(0) as usize, "file"))
        } else {
            "sharing nothing".to_string()
        },
        false,
    ));
    if !s["scanning"].is_null() {
        parts.push((format!("scanning shares {}%", fmt::s(s, "scanning")), false));
    }
    if fmt::truthy(s, "port") {
        parts.push((format!("port {}", fmt::s(s, "port")), false));
    }
    if fmt::truthy(s, "error") {
        parts.push((fmt::s(s, "error"), true));
    }
    // searches turned away for a while: "Rate limited by the Soulseek server – next search at 14:05"
    if with_session && fmt::s(session, "reason") == "rate_limited" {
        let text = session_text(session);
        parts.push((text.find("Rate limited").map(|i| text[i..].to_string()).unwrap_or_else(|| "Rate limited by the Soulseek server".into()), true));
    }
    if fmt::truthy(s, "warning") {
        parts.push((fmt::s(s, "warning"), true));
    }
    ("ok", parts, false)
}

/// "Signed in as X · sharing N files · port P" – or why it is not working, with the settings link.
fn status_line<'a>(state: &'a State, ctx: &Ctx<'a>) -> El<'a, Message> {
    let p = ctx.look;
    if !state.status_known {
        return small_muted("Checking Soulseek…").into();
    }
    let (kind, parts, with_settings) = status_parts(&state.status, &state.status_err);
    let color = match kind {
        "ok" => p.ok,
        "warn" => p.warn,
        "err" => p.err,
        _ => p.faint,
    };
    let mut r = row![dot(color)].spacing(6).align_y(Alignment::Center);
    let n = parts.len();
    for (i, (words, warn)) in parts.into_iter().enumerate() {
        let mut txt = t(words).size(12.5);
        txt = if warn { txt.color(p.warn) } else { txt.color(p.muted) };
        r = r.push(txt);
        if i + 1 < n || with_settings {
            r = r.push(small_muted("·"));
        }
    }
    if with_settings {
        r = r.push(link("Settings → Soulseek", Message::GoSettings));
    }
    r.wrap().into()
}

fn user_badges<'a>(u: &Value) -> Vec<El<'a, Message>> {
    let mut v = vec![if fmt::truthy(u, "hasFreeUploadSlot") {
        badge("Free slot", Badge::Ok)
    } else {
        badge(format!("{} queued", num(fmt::i(u, "queueLength").max(0) as usize)), Badge::Warn)
    }];
    if fmt::truthy(u, "uploadSpeed") {
        v.push(badge(speed_text(fmt::f(u, "uploadSpeed")), Badge::Plain));
    }
    v
}

/// One file: a tick box, the name, the quality badge, length and size.
fn file_row<'a>(f: &Value, checked: bool, on_toggle: impl Fn(bool) -> Message + 'a) -> El<'a, Message> {
    let name = if fmt::truthy(f, "name") { fmt::s(f, "name") } else { base_name(&fmt::s(f, "filename")) };
    let mut r = row![
        checkbox(checked).on_toggle(on_toggle).size(16),
        t(name)
            .size(13)
            .wrapping(iced::widget::text::Wrapping::WordOrGlyph)
            .width(Length::Fill),
        badge(quality(f), if is_lossless(f) { Badge::Ok } else { Badge::Plain }),
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    if fmt::truthy(f, "length") {
        r = r.push(small_muted(fmt::mmss(fmt::f(f, "length"))).width(44));
    }
    r = r.push(small_muted(fmt::size(fmt::f(f, "size"))).width(64));
    container(r).padding([3, 0]).into()
}

/// One folder: name, full path, download buttons, file rows. Collapsible in the browse dialog
/// (`toggle` set), open in results.
#[allow(clippy::too_many_arguments)]
fn folder_block<'a>(
    ctx: &Ctx<'a>,
    username: &str,
    f: &Value,
    checked: &HashSet<String>,
    sending: bool,
    toggle: Option<Message>,
    is_open: bool,
    on_file: impl Fn(String, bool) -> Message + Clone + 'a,
    on_download: impl Fn(bool) -> Message,
) -> El<'a, Message> {
    let p = ctx.look;
    let files = arr(f, "files");
    let size: f64 = files.iter().map(|x| fmt::f(x, "size")).sum();
    let directory = fmt::s(f, "directory");
    let bn = base_name(&directory);
    let n_sel = files.iter().filter(|x| checked.contains(&file_key(username, x))).count();
    let name_row = row![
        t(if bn.is_empty() { directory.clone() } else { bn })
            .size(14)
            .font(theme::semibold())
            .wrapping(iced::widget::text::Wrapping::WordOrGlyph),
        widgets::faint(format!("· {} · {}", plural(files.len(), "file"), fmt::size(size))).size(12),
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .wrap();
    let lines = column![
        name_row,
        widgets::faint(directory)
            .size(11.5)
            .wrapping(iced::widget::text::Wrapping::WordOrGlyph)
    ]
    .spacing(2)
    .width(Length::Fill);
    let mut head = row![].spacing(10).align_y(Alignment::Center);
    match &toggle {
        Some(m) => {
            head = head.push(
                button(
                    row![icon(Icon::Chevron, 14.0, p.muted), lines]
                        .spacing(8)
                        .align_y(Alignment::Center),
                )
                .padding(0)
                .width(Length::Fill)
                .style(theme::btn(Btn::Row))
                .on_press(m.clone()),
            );
        }
        None => head = head.push(lines),
    }
    let sel_label = if n_sel > 0 { format!("Download selected ({n_sel})") } else { "Download selected".to_string() };
    head = head.push(
        row![
            btn_icon(
                Icon::Check,
                sel_label,
                Btn::Default,
                (n_sel > 0 && !sending).then(|| on_download(true))
            ),
            btn_icon(
                Icon::Download,
                "Download folder",
                Btn::Primary,
                (!sending).then(|| on_download(false))
            ),
        ]
        .spacing(6),
    );
    let mut col = column![container(head).padding([10, 14])].spacing(0);
    if is_open {
        let mut list = column![].spacing(0);
        for x in files {
            let key = file_key(username, x);
            let on_file = on_file.clone();
            list = list.push(file_row(x, checked.contains(&key), move |on| on_file(key.clone(), on)));
        }
        col = col.push(container(list).padding(widgets::pad(0.0, 14.0, 10.0, 14.0)));
    }
    container(col).style(theme::panel2).into()
}

/// One search result: a collapsible card per user with its folders.
fn user_card<'a>(ctx: &Ctx<'a>, state: &'a State, i: usize, u: &'a Value) -> El<'a, Message> {
    let p = ctx.look;
    let username = fmt::s(u, "username");
    let folders = arr(u, "folders");
    let files: usize = folders.iter().map(|f| arr(f, "files").len()).sum();
    let open = state.open_users.contains(&username);
    let mut head = row![
        icon(Icon::Chevron, 14.0, p.muted),
        t(username.clone()).size(14).font(theme::bold()),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    for b in user_badges(u) {
        head = head.push(b);
    }
    head = head.push(widgets::faint(format!("{} · {}", plural(folders.len(), "folder"), plural(files, "file"))).size(12));
    let toggle = button(container(head.wrap()).padding([10, 14]))
        .padding(0)
        .width(Length::Fill)
        .style(theme::btn(Btn::Row))
        .on_press(Message::ToggleUser(username.clone()));
    let header = row![
        toggle,
        container(btn_icon(Icon::User, "Browse", Btn::Default, Some(Message::Browse(username.clone())))).padding([0, 10]),
    ]
    .align_y(Alignment::Center);
    let mut col = column![header].spacing(0);
    if open {
        let mut body = column![].spacing(10);
        for (j, f) in folders.iter().enumerate() {
            let key = format!("{username}\n{}", fmt::s(f, "directory"));
            body = body.push(folder_block(
                ctx,
                &username,
                f,
                &state.checked,
                state.sending.contains(&key),
                None,
                true,
                Message::ToggleFile,
                move |selected| Message::Download {
                    user: i,
                    folder: j,
                    selected,
                },
            ));
        }
        col = col.push(container(body).padding(widgets::pad(0.0, 14.0, 14.0, 14.0)));
    }
    card_flat(col).width(Length::Fill).into()
}

fn search_view<'a>(state: &'a State, ctx: &Ctx<'a>) -> El<'a, Message> {
    let p = ctx.look;
    if state.status_known && !state.enabled {
        return if state.forbidden {
            empty(
                Icon::Users,
                "Soulseek is for people who may pick downloads",
                "Ask an admin to allow manual download picking for your account.",
            )
        } else {
            widgets::empty::empty_with(
                Icon::Users,
                "Soulseek is unavailable",
                "",
                Some(
                    row![
                        muted("An admin can turn it on under").size(13),
                        link("Settings → Soulseek", Message::GoSettings),
                    ]
                    .spacing(4)
                    .align_y(Alignment::Center)
                    .into(),
                ),
            )
        };
    }
    let form = row![
        search_box(
            "Artist, album, song – the way the uploader probably named it",
            &state.q,
            Message::Query,
            Some(Message::Search)
        ),
        btn_icon(Icon::Search, "Search", Btn::Primary, Some(Message::Search)),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    let seconds = state.search.as_ref().map(|s| s.seconds.clone()).filter(|s| !s.is_empty());
    let status: Option<El<'a, Message>> = match &state.s_status {
        SearchStatus::Idle => None,
        SearchStatus::Starting => Some(small_muted("Starting the search…").into()),
        SearchStatus::Waiting => Some(small_muted("Waiting for another Soulseek search…").into()),
        SearchStatus::Running { n } => Some(
            small_muted(format!(
                "Searching for up to {} s… {} so far",
                seconds.unwrap_or_else(|| "?".into()),
                plural(*n, "user")
            ))
            .into(),
        ),
        SearchStatus::Done { n, files, q } if *n > 0 => {
            Some(small_muted(format!("{} · {} for “{q}”", plural(*n, "user"), plural(*files, "file"))).into())
        }
        SearchStatus::Done { .. } => None,
        SearchStatus::Err(e) => Some(t(e.as_str()).size(12).color(p.err).into()),
    };
    let mut col = column![form].spacing(10).width(Length::Fill);
    if let Some(s) = status {
        col = col.push(s);
    }
    if matches!(state.s_status, SearchStatus::Done { n: 0, .. }) {
        col = col.push(empty(
            Icon::Search,
            "Nothing found",
            "Try fewer words, or just the artist and album – Soulseek matches file and folder names.",
        ));
    }
    let shown = &state.results[..state.results.len().min(MAX_USERS)];
    for (i, u) in shown.iter().enumerate() {
        col = col.push(user_card(ctx, state, i, u));
    }
    if state.results.len() > shown.len() {
        col = col.push(small_muted(format!(
            "Showing the first {MAX_USERS} of {} – free slots and fast uploaders come first.",
            plural(state.results.len(), "user")
        )));
    }
    col.into()
}

/// A column heading of the transfer tables.
fn th<'a>(label: &'a str, width: Option<f32>) -> El<'a, Message> {
    let txt = widgets::faint(label).size(11.5).font(theme::semibold());
    match width {
        Some(w) => txt.width(w).into(),
        None => txt.width(Length::Fill).into(),
    }
}

fn transfers_view<'a>(state: &'a State, ctx: &Ctx<'a>) -> El<'a, Message> {
    let p = ctx.look;
    let tr = &state.transfers;
    let (dl, ul) = (arr(tr, "downloads"), arr(tr, "uploads"));
    let any_finished = dl.iter().any(is_finished);
    let downloads: El<'a, Message> = if !state.transfers_err.is_empty() {
        container(error_box(&state.transfers_err)).padding(14).into()
    } else if dl.is_empty() {
        empty(
            Icon::Download,
            "No downloads",
            "Files you download from Soulseek show up here while they transfer.",
        )
    } else {
        let mut col = column![container(
            row![
                th("User", Some(120.0)),
                th("File", None),
                th("State", Some(110.0)),
                th("Progress", Some(160.0)),
                th("Speed / size", Some(120.0)),
                space().width(32),
            ]
            .spacing(14)
        )
        .padding([8, 16])];
        col = col.push(widgets::divider());
        for (i, x) in dl.iter().enumerate() {
            let active = is_active(x);
            let pc = pct(x);
            let mut file = column![t(if fmt::truthy(x, "basename") {
                fmt::s(x, "basename")
            } else {
                base_name(&fmt::s(x, "filename"))
            })
            .size(13)
            .font(theme::semibold())
            .wrapping(iced::widget::text::Wrapping::WordOrGlyph)]
            .spacing(2);
            file = file.push(
                widgets::faint(fmt::s(x, "directory"))
                    .size(11.5)
                    .wrapping(iced::widget::text::Wrapping::WordOrGlyph),
            );
            if fmt::truthy(x, "error") {
                file = file.push(t(fmt::s(x, "error")).size(12).color(p.err));
            }
            let mut speed = column![].spacing(2);
            if active && fmt::truthy(x, "speed") {
                speed = speed.push(small_muted(speed_text(fmt::f(x, "speed"))));
            }
            speed = speed.push(small_muted(format!(
                "{} / {}",
                fmt::size(fmt::f(x, "bytes")),
                fmt::size(fmt::f(x, "size"))
            )));
            let id = fmt::s(x, "id");
            let action = if active { "Cancel" } else { "Remove" };
            col = col.push(column![
                container(
                    row![
                        small_muted(fmt::s(x, "username"))
                            .width(120)
                            .wrapping(iced::widget::text::Wrapping::WordOrGlyph),
                        file.width(Length::Fill),
                        container(state_badge(&fmt::s(x, "state"), false)).width(110),
                        column![
                            progress_bar(0.0..=100.0, pc as f32).girth(5).style(theme::progress).length(160),
                            widgets::faint(format!("{pc}%")).size(11.5),
                        ]
                        .spacing(4)
                        .width(160),
                        speed.width(120),
                        icon_btn(
                            Icon::X,
                            action,
                            (!state.cancelling.contains(&id)).then_some(Message::Cancel(i))
                        ),
                    ]
                    .spacing(14)
                    .align_y(Alignment::Center)
                )
                .padding([9, 16]),
                widgets::divider(),
            ]);
        }
        col.into()
    };
    let uploads: El<'a, Message> = if ul.is_empty() {
        empty(Icon::Users, "No uploads", "Other people fetching from your shares show up here.")
    } else {
        let mut col = column![container(
            row![
                th("User", Some(120.0)),
                th("File", None),
                th("State", Some(110.0)),
                th("Progress", Some(160.0)),
            ]
            .spacing(14)
        )
        .padding([8, 16])];
        col = col.push(widgets::divider());
        for x in ul {
            let pc = pct(x);
            col = col.push(column![
                container(
                    row![
                        small_muted(fmt::s(x, "username"))
                            .width(120)
                            .wrapping(iced::widget::text::Wrapping::WordOrGlyph),
                        column![
                            t(if fmt::truthy(x, "basename") {
                                fmt::s(x, "basename")
                            } else {
                                base_name(&fmt::s(x, "filename"))
                            })
                            .size(13)
                            .font(theme::semibold())
                            .wrapping(iced::widget::text::Wrapping::WordOrGlyph),
                            widgets::faint(fmt::size(fmt::f(x, "size"))).size(11.5),
                        ]
                        .spacing(2)
                        .width(Length::Fill),
                        container(state_badge(&fmt::s(x, "state"), true)).width(110),
                        column![
                            progress_bar(0.0..=100.0, pc as f32).girth(5).style(theme::progress).length(160),
                            widgets::faint(format!(
                                "{} / {}",
                                fmt::size(fmt::f(x, "bytes")),
                                fmt::size(fmt::f(x, "size"))
                            ))
                            .size(11.5),
                        ]
                        .spacing(4)
                        .width(160),
                    ]
                    .spacing(14)
                    .align_y(Alignment::Center)
                )
                .padding([9, 16]),
                widgets::divider(),
            ]);
        }
        col.into()
    };
    let dl_head = row![
        h2("Downloads"),
        small_muted(dl.len().to_string()).color(p.faint),
        space().width(Length::Fill),
        btn_icon(
            Icon::Trash,
            "Clear finished",
            Btn::Default,
            (any_finished && !state.clearing).then_some(Message::ClearFinished)
        ),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    let ul_head = row![h2("Uploads"), small_muted(ul.len().to_string()).color(p.faint)]
        .spacing(8)
        .align_y(Alignment::Center);
    column![
        dl_head,
        card_flat(downloads).width(Length::Fill),
        ul_head,
        card_flat(uploads).width(Length::Fill),
    ]
    .spacing(12)
    .width(Length::Fill)
    .into()
}

pub fn view<'a>(state: &'a State, ctx: &Ctx<'a>) -> El<'a, Message> {
    let active = arr(&state.transfers, "downloads").iter().filter(|x| is_active(x)).count();
    let tabs = count_tabs(
        &[
            ("search".into(), "Search".into(), String::new()),
            (
                "transfers".into(),
                "Transfers".into(),
                if active > 0 { active.to_string() } else { String::new() },
            ),
        ],
        &state.tab,
        Message::Tab,
    );
    let head = row![
        column![h1("Soulseek"), status_line(state, ctx)].spacing(4),
        space().width(Length::Fill),
        tabs,
    ]
    .spacing(12)
    .align_y(Alignment::Center);
    let body = if state.tab == "transfers" {
        transfers_view(state, ctx)
    } else {
        search_view(state, ctx)
    };
    column![head, body].spacing(16).width(Length::Fill).into()
}

/// The browse dialog.
fn browse_view<'a>(b: &'a Browse, ctx: &Ctx<'a>) -> El<'a, Message> {
    let title = format!("{} · shared files", b.username);
    let body: El<'a, Message> = match &b.folders {
        None => column![
            small_muted(format!(
                "Asking {} for their file list – this can take a while if they are slow to answer.",
                b.username
            )),
            widgets::skeleton_rows(4),
        ]
        .spacing(14)
        .into(),
        Some(Err(e)) => error_box(e),
        Some(Ok(folders)) => {
            let total: usize = folders.iter().map(|f| arr(f, "files").len()).sum();
            let needle = b.filter.trim().to_lowercase();
            let hit = |s: &str| s.to_lowercase().contains(&needle);
            let shown: Vec<(usize, &Value)> = folders
                .iter()
                .enumerate()
                .filter(|(_, f)| {
                    needle.is_empty()
                        || hit(&fmt::s(f, "directory"))
                        || arr(f, "files").iter().any(|x| hit(&fmt::s(x, "name")))
                })
                .collect();
            // a narrow filter: show the files right away
            let open_all = !needle.is_empty() && shown.len() <= 6;
            let mut col = column![row![
                search_box("Filter folders and files…", &b.filter, Message::BrowseFilter, None),
                small_muted(format!("{} · {}", plural(folders.len(), "folder"), plural(total, "file"))),
            ]
            .spacing(12)
            .align_y(Alignment::Center)]
            .spacing(10);
            if shown.is_empty() {
                col = col.push(empty(
                    Icon::Search,
                    if !needle.is_empty() { "No folders match" } else { "Nothing shared" },
                    "",
                ));
            } else {
                for (j, f) in shown.iter().take(MAX_FOLDERS) {
                    let j = *j;
                    col = col.push(folder_block(
                        ctx,
                        &b.username,
                        f,
                        &b.checked,
                        b.sending.contains(&j),
                        Some(Message::BrowseFolder(j)),
                        open_all || b.open.contains(&j),
                        Message::BrowseFile,
                        move |selected| Message::BrowseDownload { folder: j, selected },
                    ));
                }
                if shown.len() > MAX_FOLDERS {
                    col = col.push(small_muted(format!(
                        "Showing {MAX_FOLDERS} of {} – narrow the filter to see the rest.",
                        plural(shown.len(), "folder")
                    )));
                }
            }
            col.into()
        }
    };
    modal(title, body, None, true, Message::CloseDialog)
}

/// A modal or ⋯ menu to lay over the whole window, when one is open.
pub fn overlay<'a>(state: &'a State, ctx: &Ctx<'a>) -> Option<El<'a, Message>> {
    state.browse.as_ref().map(|b| browse_view(b, ctx))
}

pub fn subscription(state: &State) -> Subscription<Message> {
    let tick = iced::time::every(Duration::from_secs(2)).map(|_| Message::Tick);
    if state.search.is_some() {
        Subscription::batch([
            tick,
            iced::time::every(Duration::from_secs(1)).map(|_| Message::PollTick),
        ])
    } else {
        tick
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_numbers_and_quality_read_like_the_web() {
        assert_eq!(base_name("@@music\\Daft Punk\\Discovery\\01 - One.flac"), "01 - One.flac");
        assert_eq!(base_name("a/b/c/"), "c");
        assert_eq!(num(1_234_567), "1,234,567");
        assert_eq!(num(999), "999");
        assert_eq!(plural(1, "file"), "1 file");
        assert_eq!(plural(2500, "user"), "2,500 users");
        assert_eq!(speed_text(1_500_000.0), "1.5 MB/s");
        assert_eq!(speed_text(320_000.0), "320 kB/s");
        assert_eq!(quality(&json!({ "filename": "x\\a.flac", "bitDepth": 24, "sampleRate": 96000 })), "FLAC 24-bit/96kHz");
        assert_eq!(quality(&json!({ "extension": "flac", "sampleRate": 44100 })), "FLAC");
        assert_eq!(quality(&json!({ "extension": "flac", "sampleRate": 88200 })), "FLAC/88.2kHz");
        assert_eq!(quality(&json!({ "name": "a.mp3", "bitRate": 320 })), "MP3 320k");
        assert_eq!(quality(&json!({ "name": "noext" })), "NOEXT");
        assert_eq!(prettify("Completed, Succeeded"), "Completed · Succeeded");
        assert_eq!(prettify("QueuedRemotely"), "Queued Remotely");
        assert_eq!(state_words("InProgress", true), (Badge::Accent, "Uploading".into()));
        assert_eq!(state_words("Queued, Remotely", false), (Badge::Info, "In their queue".into()));
        assert_eq!(state_words("", false), (Badge::Plain, "Unknown".into()));
    }

    #[test]
    fn progress_sorting_and_bodies() {
        assert_eq!(pct(&json!({ "progress": 42.4 })), 42);
        assert_eq!(pct(&json!({ "bytes": 50, "size": 200 })), 25);
        assert_eq!(pct(&json!({ "progress": 140 })), 100);
        assert_eq!(pct(&json!({})), 0);
        let users = sort_users(vec![
            json!({ "username": "slow", "uploadSpeed": 10 }),
            json!({ "username": "fast", "uploadSpeed": 900 }),
            json!({ "username": "slot", "hasFreeUploadSlot": true, "uploadSpeed": 1 }),
        ]);
        let names: Vec<String> = users.iter().map(|u| fmt::s(u, "username")).collect();
        assert_eq!(names, ["slot", "fast", "slow"]);
        let folder = json!({ "directory": "d", "files": [] });
        let files = vec![json!({ "filename": "d\\a.flac", "size": 3, "bitRate": 900 }), json!({ "filename": "d\\b.flac" })];
        let body = download_body("alice", &folder, &files);
        assert_eq!(body["kind"], "album");
        assert_eq!(body["files"][0], json!({ "filename": "d\\a.flac", "size": 3 }));
        assert_eq!(body["files"][1], json!({ "filename": "d\\b.flac" }));
        assert_eq!(download_body("alice", &folder, &files[..1])["kind"], "track");
        assert!(is_finished(&json!({ "state": "Completed, Succeeded" })) && is_active(&json!({ "state": "InProgress" })));
    }

    #[test]
    fn status_words() {
        let (dot, parts, link) = status_parts(&Value::Null, "boom");
        assert_eq!((dot, link), ("err", true));
        assert_eq!(parts[0].0, "boom");
        let s = json!({ "enabled": true, "state": "running", "logged_in": true, "username": "me", "shares": { "files": 1200 }, "port": 2234 });
        let (dot, parts, link) = status_parts(&s, "");
        assert_eq!((dot, link), ("ok", false));
        let words: Vec<String> = parts.iter().map(|p| p.0.clone()).collect();
        assert_eq!(words, ["Signed in as me", "sharing 1,200 files", "port 2234"]);
        let (dot, parts, _) = status_parts(&json!({ "enabled": true, "state": "crashed", "error": "x" }), "");
        assert_eq!(dot, "err");
        assert_eq!(parts[0], ("Soulseek stopped – x".to_string(), true));
        let (_, parts, link) = status_parts(&json!({ "enabled": false }), "");
        assert!(link && parts[0].0.starts_with("Soulseek is not enabled"));
        // the session (0.29): why it isn't logged in and when the next attempt is, linking to the settings
        let waiting = json!({ "enabled": true, "state": "running", "logged_in": false,
            "session": { "state": "waiting", "reason": "timeout", "text": "Disconnected – the login timed out (the Soulseek server did not answer in 5 s). Next attempt at 14:20 (attempt 3)." } });
        let (dot, parts, link) = status_parts(&waiting, "");
        assert_eq!((dot, link), ("warn", true));
        assert!(parts[0].1 && parts[0].0.starts_with("Disconnected – the login timed out"));
        let rejected = json!({ "enabled": true, "state": "running", "session": { "state": "stopped", "reason": "rejected", "text": "Rejected: invalid username or password – fix them below" } });
        let (dot, parts, _) = status_parts(&rejected, "");
        assert_eq!((dot, parts[0].0.as_str()), ("err", "Rejected: invalid username or password"));
        let limited = json!({ "enabled": true, "state": "running", "logged_in": true, "username": "me", "shares": { "files": 0 },
            "session": { "state": "logged_in", "reason": "rate_limited", "text": "Logged in as me since 14:02. Rate limited by the Soulseek server – next search at 14:05" } });
        let (dot, parts, _) = status_parts(&limited, "");
        assert_eq!(dot, "ok");
        assert_eq!(parts.last().unwrap(), &("Rate limited by the Soulseek server – next search at 14:05".to_string(), true));
    }
}
