//! The live blocks inside the Server settings cards (web-rs/src/pages/settings.rs): Download
//! tools (yt-dlp, streamrip, Cadenza, slskd, the Translator), the Tidal sign-in and streamrip
//! config file, the Soulseek engine's status, the VPN for Soulseek, the server's Cadenza cache
//! and engine settings, and the YouTube sign-in card.

use super::{help, link_muted, result_line, result_line_copy, select, switch_maybe, vs, Opt};
use crate::api::ApiError;
use crate::fmt;
use crate::pages::{Ctx, El};
use crate::theme::{self, Badge, Btn};
use crate::widgets::text_area::text_area;
use crate::widgets::{self, badge, btn, btn_icon, btn_sm, modal, Icon};
use iced::widget::text_editor;
use iced::widget::{column, container, row};
use iced::{Alignment, Length, Task};
use serde_json::{json, Map, Value};
use std::collections::HashMap;

#[derive(Debug)]
pub enum Dialog {
    /// "Use the built-in engine" / "Remove slskd" / "Use the bundled version".
    ToolReset { id: String, ask: String, done: String },
    RipReset,
    YtSignOut,
    YtNewToken,
}

#[derive(Debug, Default)]
pub struct State {
    tools: Option<Vec<Value>>,
    busy: Option<String>,
    checking: bool,
    tidal: Value,
    tidal_busy: bool,
    /// The sign-in link the server is waiting on; polled every 3 s.
    tidal_waiting: bool,
    tidal_paste: bool,
    tidal_toml: text_editor::Content,
    rip_out: Option<(bool, String)>,
    slsk: Value,
    /// Reconnect now was pressed and has not answered yet.
    slsk_reconnecting: bool,
    vpn: Value,
    vpn_testing: bool,
    dsp: Value,
    /// The engine settings the server's Cadenza offers, as edited (number boxes as text).
    engine_edits: HashMap<String, String>,
    engine_values: Map<String, Value>,
    engine_loaded: bool,
    youtube: Value,
    yt_token: Option<String>,
    yt_busy: bool,
    dialog: Option<Dialog>,
}

#[derive(Clone, Debug)]
pub enum Message {
    Tools(Result<Value, ApiError>),
    Check,
    Checked(Result<Value, ApiError>),
    Update(String),
    Updated(String, bool, Result<Value, ApiError>),
    AskReset(String),
    Reset,
    ResetDone(String, Result<Value, ApiError>),
    Auto(String, bool),
    AutoSaved(Result<Value, ApiError>),
    Model(String),
    ModelSaved(Result<Value, ApiError>),
    Tidal(Result<Value, ApiError>),
    TidalSignIn,
    TidalStarted(Result<Value, ApiError>),
    TidalTick,
    TidalPaste(bool),
    TidalToml(text_editor::Action),
    TidalSaveTokens,
    TidalTokensSaved(Result<Value, ApiError>),
    TidalSignOut,
    TidalSignedOut(Result<Value, ApiError>),
    /// Copy a failed result's words.
    CopyText(String),
    AskRipReset,
    RipReset,
    RipResetDone(Result<Value, ApiError>),
    Slsk(Result<Value, ApiError>),
    SlskStart,
    SlskStarted(Result<Value, ApiError>),
    /// Reconnect now (POST /system/soulseek/reconnect): the session's schedule starts over.
    SlskReconnect,
    SlskReconnected(Result<Value, ApiError>),
    Vpn(Result<Value, ApiError>),
    /// Handled by the server page (the Soulseek card is saved first).
    VpnTest,
    VpnTested(Result<Value, ApiError>),
    Dsp(Result<Value, ApiError>),
    CacheClear,
    CacheCleared(Result<Value, ApiError>),
    EngineBool(String, bool),
    EngineSelect(String, String),
    EngineNum(String, String),
    Youtube(Result<Value, ApiError>),
    YtRefresh,
    YtRefreshed(Result<Value, ApiError>),
    AskYtSignOut,
    YtSignOut,
    YtSignedOut(Result<Value, ApiError>),
    YtExtension,
    YtToken(Result<Value, ApiError>),
    YtCopyToken,
    OpenUrl(String),
    /// Every 5 s while Downloads is open: the Soulseek and VPN states.
    Tick,
    CloseDialog,
}

fn get(ctx: &Ctx, path: &'static str, f: fn(Result<Value, ApiError>) -> Message) -> Task<Message> {
    let api = ctx.api.clone();
    Task::perform(async move { api.get(path).await }, f)
}

/// What Downloads (the Download tools card and the source cards' blocks) and Server processing (the
/// Cadenza card's blocks) need, loaded when one of them opens.
pub fn open(state: &mut State, ctx: &mut Ctx) -> Task<Message> {
    state.tools = None;
    state.busy = None;
    state.checking = false;
    state.tidal_waiting = false;
    state.tidal_paste = false;
    state.rip_out = None;
    state.engine_loaded = false;
    state.engine_edits.clear();
    state.yt_token = None;
    state.dialog = None;
    Task::batch([
        get(ctx, "/system/tools", Message::Tools),
        get(ctx, "/system/streamrip/tidal-login", Message::Tidal),
        get(ctx, "/soulseek/status", Message::Slsk),
        get(ctx, "/system/vpn", Message::Vpn),
        get(ctx, "/playback/dsp", Message::Dsp),
        get(ctx, "/youtube/auth", Message::Youtube),
    ])
}

fn tool_list(r: &Value) -> Vec<Value> {
    if r.is_array() { fmt::items(r) } else { fmt::items(&r["tools"]) }
}

/// `a || b`: the first truthy of the two, as text.
fn tool_or(t: &Value, a: &str, b: &str) -> String {
    if fmt::truthy(t, a) {
        fmt::s(t, a)
    } else if fmt::truthy(t, b) {
        fmt::s(t, b)
    } else {
        String::new()
    }
}

/// The reset link of a tool row `(link, question, toast)`.
pub fn tool_reset(t: &Value) -> (String, String, String) {
    match fmt::s(t, "id").as_str() {
        "cadenza" => ("Use the built-in engine".into(), "Go back to the engine built into Encore? The downloaded Cadenza is removed.".into(), "Back to the built-in engine".into()),
        "slskd" => ("Remove slskd".into(), "Remove slskd? Soulseek stays off until you install it again.".into(), "slskd removed".into()),
        "translator" => ("Remove the Translator".into(), "Remove the Translator and its model? Translations made so far are kept; new titles only get official translations until you install it again.".into(), "Translator removed".into()),
        _ => {
            let name = tool_or(t, "name", "id");
            ("Use the bundled version".into(), format!("Go back to the {name} version that came with Encore? The updated copy is removed."), format!("Back to the bundled {name}"))
        }
    }
}

/// slskd and the Translator are installed by Encore, not bundled: before that their button says Install.
fn tool_install(t: &Value) -> bool {
    matches!(fmt::s(t, "id").as_str(), "slskd" | "translator") && !fmt::truthy(t, "installed")
}

/// A wait in the server's words: "45 s", "3 m 12 s", "15 m", "2 h 5 m".
pub fn span(secs: f64) -> String {
    let s = secs.max(0.0).round() as i64;
    match (s / 3600, (s % 3600) / 60, s % 60) {
        (0, 0, s) => format!("{s} s"),
        (0, m, 0) => format!("{m} m"),
        (0, m, s) => format!("{m} m {s} s"),
        (h, 0, _) => format!("{h} h"),
        (h, m, _) => format!("{h} h {m} m"),
    }
}

/// `fmt::ago` against a given clock: "just now", "5 min ago", "2 h ago", "3 days ago".
fn ago_at(ts: f64, now: f64) -> String {
    let d = (now - ts).max(0.0);
    if d < 60.0 {
        "just now".into()
    } else if d < 3600.0 {
        format!("{} min ago", (d / 60.0) as u64)
    } else if d < 86_400.0 {
        format!("{} h ago", (d / 3600.0) as u64)
    } else {
        format!("{} days ago", (d / 86_400.0) as u64)
    }
}

/// Unix seconds now.
pub fn now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

/// `/soulseek/status` of a server since 0.29 with the engine running: it carries the session (`session`, the
/// same object as GET /system/soulseek), whose line says whether Soulseek is logged in, why not and when next.
pub fn has_session(r: &Value) -> bool {
    fmt::s(r, "state") == "running" && r["session"].is_object() && fmt::truthy(&r["session"], "state")
}

/// The session's colour (web-rs settings/apps.rs): ok (logged in), warn (rate limited, waiting for the next
/// attempt), err (stopped for a reason of the server's: rejected, banned, kicked again), "" (connecting, off).
pub fn session_tone(s: &Value) -> &'static str {
    match fmt::s(s, "state").as_str() {
        "logged_in" => if fmt::s(s, "reason") == "rate_limited" { "warn" } else { "ok" },
        "waiting" | "disconnected" => "warn",
        "stopped" if fmt::truthy(s, "reason") => "err",
        _ => "",
    }
}

/// The session's sentence: the server's `text`, so every app says the same ("Logged in as alice since 14:02",
/// "Disconnected – the login timed out (…). Next attempt at 14:20 (attempt 3).").
pub fn session_text(s: &Value) -> String {
    if fmt::truthy(s, "text") {
        return fmt::s(s, "text");
    }
    match fmt::s(s, "state").as_str() {
        "logged_in" => format!("Logged in as {}", if fmt::truthy(s, "username") { fmt::s(s, "username") } else { "?".into() }),
        "connecting" => "Connecting to the Soulseek server…".into(),
        "stopped" => if fmt::truthy(s, "message") { fmt::s(s, "message") } else { "Soulseek is off".into() },
        _ => {
            if fmt::truthy(s, "message") {
                format!("Not connected – {}", fmt::s(s, "message"))
            } else {
                "Not connected – connecting shortly".into()
            }
        }
    }
}

/// The sentence away from the card (the Soulseek page, Diagnostics): "fix them below" points at the card's
/// fields, which aren't there – it names the card instead, in the server's words (`text_elsewhere`).
pub fn session_text_elsewhere(text: &str) -> String {
    text.replace(" – fix them below", " – fix them in Settings → Downloads → Soulseek")
}

/// What goes under the sentence, on this computer's clock: "Next attempt in 3 m 12 s · 12 searches waiting
/// for the connection · last search 5 min ago".
pub fn session_details(s: &Value, now: f64) -> String {
    let mut parts: Vec<String> = vec![];
    let state = fmt::s(s, "state");
    let next = fmt::f(s, "next_attempt_at");
    if matches!(state.as_str(), "waiting" | "disconnected") && next > now {
        parts.push(format!("next attempt in {}", span(next - now)));
    }
    let resume = fmt::f(s, "next_search_at");
    if resume > now {
        parts.push(format!("searches go on in {}", span(resume - now)));
    }
    let n = fmt::i(s, "searches_waiting");
    if n > 0 {
        parts.push(match (n, state == "logged_in") {
            (1, true) => "1 search waiting for its turn".to_string(),
            (n, true) => format!("{n} searches waiting for their turn"),
            (1, false) => "1 search waiting for the connection".to_string(),
            (n, false) => format!("{n} searches waiting for the connection"),
        });
    }
    if fmt::truthy(s, "search_running") {
        parts.push("a search is running".into());
    }
    if fmt::f(s, "last_search_at") > 0.0 {
        parts.push(format!("last search {}", ago_at(fmt::f(s, "last_search_at"), now)));
    }
    let line = parts.join(" · ");
    // "Next attempt in 28 s · …"
    let mut c = line.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// Reconnect now: shown while the session is down and the engine runs; `Some(n)` when it has to wait n seconds
/// (attempts are at least 30 s apart), None when it can go now.
pub fn reconnect_wait(r: &Value) -> Option<Option<i64>> {
    let s = &r["session"];
    if fmt::s(r, "state") != "running" || !fmt::truthy(s, "can_reconnect") {
        return None;
    }
    let n = fmt::i(s, "reconnect_in");
    Some(if n > 0 { Some(n) } else { None })
}

/// "Running v0.24.1 · signed in as alice" / "Not installed" / "Waiting for the VPN" …
/// With a session (servers since 0.29): "Running v0.26.1"; the session's line goes under it.
pub fn soulseek_state_text(r: &Value) -> String {
    if !r.is_object() {
        return "Status unavailable".into();
    }
    let v = if fmt::truthy(r, "version") {
        let ver = fmt::s(r, "version");
        format!(" v{}", ver.strip_prefix('v').unwrap_or(&ver))
    } else {
        String::new()
    };
    let error = fmt::s(r, "error");
    let line = match fmt::s(r, "state").as_str() {
        "not_installed" => "Not installed".to_string(),
        "starting" => format!("Starting…{}", if fmt::truthy(r, "error") { format!(" · {error}") } else { String::new() }),
        "running" if has_session(r) => format!("Running{v}"),
        "running" => format!(
            "Running{v}{}",
            if fmt::truthy(r, "logged_in") {
                format!(" · signed in as {}", if fmt::truthy(r, "username") { fmt::s(r, "username") } else { "?".to_string() })
            } else if fmt::truthy(r, "error") {
                format!(" · {error}")
            } else {
                " · signing in…".to_string()
            }
        ),
        "crashed" => format!("Crashed{}", if fmt::truthy(r, "error") { format!(" – {error}") } else { String::new() }),
        "blocked" => format!("Can't start{}", if fmt::truthy(r, "error") { format!(" – {error}") } else { String::new() }),
        "waiting_vpn" => "Waiting for the VPN".to_string(),
        _ => {
            if r["enabled"] == json!(false) {
                "Off – switch on Enable Soulseek and save".to_string()
            } else {
                "Off".to_string()
            }
        }
    };
    if fmt::truthy(r, "hint") { format!("{line} · {}", fmt::s(r, "hint")) } else { line }
}

/// The Start now button belongs to a crash or a blocked start: the states a person can act on.
pub fn can_start_now(state: &str) -> bool {
    matches!(state, "crashed" | "blocked")
}

/// "Cache: 812 MB of 4096 MB · 23 songs · renders at 6.2× real time".
pub fn cadenza_cache_text(c: &Value) -> String {
    if !c.is_object() {
        return "Cache: not available on this server".into();
    }
    let mb = |v: f64| format!("{}", super::js_round(if v.is_finite() { v } else { 0.0 }));
    let n = fmt::f(c, "files");
    let f = fmt::f(c, "last_factor");
    format!(
        "Cache: {} MB of {} MB · {} {}{}",
        mb(fmt::f(c, "used_mb")),
        mb(fmt::f(c, "max_mb")),
        super::js_num(n),
        if n == 1.0 { "song" } else { "songs" },
        if f > 0.0 { format!(" · renders at {f:.1}× real time") } else { String::new() }
    )
}

/// "Engine: Cadenza 2.1.0 (updated)" / "Engine: Cadenza (built in)".
pub fn cadenza_engine_text(e: &Value) -> String {
    format!(
        "Engine: {}",
        if e.is_object() && fmt::s(e, "source") == "updated" && fmt::truthy(e, "version") {
            format!("Cadenza {} (updated)", fmt::s(e, "version"))
        } else {
            "Cadenza (built in)".to_string()
        }
    )
}

const ENGINE_TYPES: [&str; 3] = ["select", "bool", "number"];

fn engine_settings(dsp: &Value) -> Vec<Value> {
    fmt::items(&dsp["engine"]["settings"])
        .into_iter()
        .filter(|s| s.is_object() && fmt::truthy(s, "key") && ENGINE_TYPES.contains(&fmt::s(s, "type").as_str()))
        .collect()
}

fn engine_options(s: &Value) -> Vec<(Value, Value)> {
    fmt::items(&s["options"])
        .iter()
        .map(|o| (o.get(0).cloned().unwrap_or(Value::Null), o.get(1).cloned().unwrap_or(Value::Null)))
        .collect()
}

fn engine_default_text(s: &Value) -> String {
    match fmt::s(s, "type").as_str() {
        "bool" => (if fmt::truthy(s, "default") { "on" } else { "off" }).to_string(),
        "select" => engine_options(s).into_iter().find(|(v, _)| vs(v) == vs(&s["default"])).map(|(_, l)| vs(&l)).unwrap_or_else(|| vs(&s["default"])),
        _ => vs(&s["default"]),
    }
}

/// JavaScript `Number(v)`.
fn to_num(v: &Value) -> f64 {
    match v {
        Value::Null => 0.0,
        Value::Bool(b) => {
            if *b {
                1.0
            } else {
                0.0
            }
        }
        Value::Number(n) => n.as_f64().unwrap_or(f64::NAN),
        Value::String(t) => {
            let t = t.trim();
            if t.is_empty() { 0.0 } else { t.parse().unwrap_or(f64::NAN) }
        }
        _ => f64::NAN,
    }
}

/// The engine settings as the JSON text for `cadenza_engine_settings` (only values that differ
/// from the default; keys this engine does not have stay as saved), or None before they loaded.
pub fn engine_settings_value(state: &State) -> Option<String> {
    if !state.engine_loaded {
        return None;
    }
    let mut out = state.engine_values.clone();
    for s in engine_settings(&state.dsp) {
        let key = fmt::s(&s, "key");
        let current = state.engine_values.get(&key).cloned().unwrap_or_else(|| s["default"].clone());
        let v = match fmt::s(&s, "type").as_str() {
            "number" => {
                let raw = state.engine_edits.get(&key).cloned().unwrap_or_else(|| vs(&current));
                let n = if raw.trim().is_empty() { f64::NAN } else { to_num(&Value::String(raw)) };
                if !n.is_finite() || n == to_num(&s["default"]) {
                    out.remove(&key);
                    continue;
                }
                if n.fract() == 0.0 && n.abs() < 1e15 { Value::from(n as i64) } else { Value::from(n) }
            }
            _ => current,
        };
        if fmt::s(&s, "type") != "number" && v == s["default"] {
            out.remove(&key);
        } else {
            out.insert(key, v);
        }
    }
    Some(Value::Object(out).to_string())
}

/// `youtube_status` words: `(label, account, badge, when, lines, diagnostics)`.
fn source_words(key: &str) -> &'static str {
    match key {
        "handoff" => "Handed-over sign-in",
        "extension" => "Everyday browser (extension)",
        _ => "Pasted cookies",
    }
}

fn source_line(key: &str, s: &Value) -> String {
    let state = if !fmt::truthy(s, "has_cookies") {
        if key == "pasted" { "none".to_string() } else { "not connected".to_string() }
    } else if fmt::truthy(s, "signed_in") {
        format!("signed in{}", if fmt::truthy(s, "account") { format!(" as {}", fmt::s(s, "account")) } else { String::new() })
    } else {
        "signed out".to_string()
    };
    format!("{}: {state}", source_words(key))
}

pub fn update(state: &mut State, msg: Message, ctx: &mut Ctx) -> Task<Message> {
    match msg {
        Message::Tools(Ok(r)) => state.tools = Some(tool_list(&r)),
        Message::Tools(Err(_)) => state.tools = Some(Vec::new()),
        Message::Check => {
            state.checking = true;
            let api = ctx.api.clone();
            return Task::perform(async move { api.post_empty("/system/tools/check").await }, Message::Checked);
        }
        Message::Checked(r) => {
            state.checking = false;
            match r {
                Ok(r) => state.tools = Some(tool_list(&r)),
                Err(e) => ctx.toast_err(e.message),
            }
        }
        Message::Update(id) => {
            if state.busy.is_some() {
                return Task::none();
            }
            let tool = state.tools.as_ref().and_then(|l| l.iter().find(|t| fmt::s(t, "id") == id)).cloned().unwrap_or(json!({ "id": id }));
            let name = {
                let n = fmt::s(&tool, "name");
                if n.is_empty() { id.clone() } else { n }
            };
            let install = tool_install(&tool);
            state.busy = Some(id.clone());
            let api = ctx.api.clone();
            let i2 = id.clone();
            return Task::perform(async move { api.post_empty(&format!("/system/tools/{i2}/update")).await }, move |r| Message::Updated(name.clone(), install, r));
        }
        Message::Updated(name, install, r) => {
            state.busy = None;
            match r {
                Ok(r) => ctx.toast_ok(format!("{name} {} {}", if install { "installed" } else { "updated to" }, tool_or(&r, "installed", "latest"))),
                Err(e) => ctx.toast_err(e.message),
            }
            return get(ctx, "/system/tools", Message::Tools);
        }
        Message::AskReset(id) => {
            let tool = state.tools.as_ref().and_then(|l| l.iter().find(|t| fmt::s(t, "id") == id)).cloned().unwrap_or(json!({ "id": id }));
            let (_, ask, done) = tool_reset(&tool);
            state.dialog = Some(Dialog::ToolReset { id, ask, done });
        }
        Message::Reset => {
            if let Some(Dialog::ToolReset { id, done, .. }) = state.dialog.take() {
                let api = ctx.api.clone();
                return Task::perform(async move { api.post_empty(&format!("/system/tools/{id}/reset")).await }, move |r| Message::ResetDone(done.clone(), r));
            }
        }
        Message::ResetDone(done, r) => {
            match r {
                Ok(_) => ctx.toast_ok(done),
                Err(e) => ctx.toast_err(e.message),
            }
            return get(ctx, "/system/tools", Message::Tools);
        }
        Message::Auto(key, on) => {
            let api = ctx.api.clone();
            let mut body = Map::new();
            body.insert(key, Value::Bool(on));
            return Task::perform(async move { api.put("/admin/settings", Value::Object(body)).await }, Message::AutoSaved);
        }
        Message::AutoSaved(Ok(_)) => ctx.toast_ok("Saved"),
        Message::AutoSaved(Err(e)) => ctx.toast_err(e.message),
        Message::Model(v) => {
            let api = ctx.api.clone();
            return Task::perform(async move { api.put("/admin/settings", json!({ "translator_model": v })).await }, Message::ModelSaved);
        }
        Message::ModelSaved(Ok(_)) => {
            ctx.toast_ok("Saved – Install or Update the Translator to download this model");
            return get(ctx, "/system/tools", Message::Tools);
        }
        Message::ModelSaved(Err(e)) => ctx.toast_err(e.message),
        Message::Tidal(Ok(r)) => {
            state.tidal_waiting = fmt::s(&r, "status") == "waiting" && !fmt::s(&r, "url").is_empty();
            if fmt::s(&r, "status") != "waiting" {
                state.tidal_busy = false;
            }
            if fmt::s(&r, "message").contains("paste the tokens") {
                state.tidal_paste = true;
            }
            state.tidal = r;
        }
        Message::Tidal(Err(_)) => state.tidal = json!({ "unavailable": true }),
        Message::TidalSignIn => {
            state.tidal_busy = true;
            let api = ctx.api.clone();
            return Task::perform(async move { api.post_empty("/system/streamrip/tidal-login").await }, Message::TidalStarted);
        }
        Message::TidalStarted(Ok(r)) => {
            if fmt::s(&r, "status") == "waiting" && !fmt::s(&r, "url").is_empty() {
                ctx.open_url(fmt::s(&r, "url"));
                state.tidal_waiting = true;
            } else {
                state.tidal_busy = false;
            }
            state.tidal = r;
        }
        Message::TidalStarted(Err(e)) => {
            state.tidal_busy = false;
            state.tidal["status"] = json!("failed");
            state.tidal["message"] = json!(e.message);
        }
        Message::TidalTick => {
            if state.tidal_waiting {
                let before = fmt::s(&state.tidal, "url");
                let api = ctx.api.clone();
                return Task::perform(async move { api.get("/system/streamrip/tidal-login").await }, move |r| {
                    // a new step comes with a new link: open it too
                    if let Ok(v) = &r {
                        if fmt::s(v, "status") == "waiting" && fmt::s(v, "url") != before && !fmt::s(v, "url").is_empty() {
                            let _ = open::that_detached(fmt::s(v, "url"));
                        }
                    }
                    Message::Tidal(r)
                });
            }
        }
        Message::TidalPaste(on) => state.tidal_paste = on,
        Message::TidalToml(a) => state.tidal_toml.perform(a),
        Message::TidalSaveTokens => {
            let api = ctx.api.clone();
            let body = json!({ "toml": state.tidal_toml.text() });
            return Task::perform(async move { api.post("/system/streamrip/tidal-tokens", body).await }, Message::TidalTokensSaved);
        }
        Message::TidalTokensSaved(Ok(r)) => {
            state.tidal = r;
            state.tidal_toml = text_editor::Content::new();
            ctx.toast_ok("Tidal tokens saved");
        }
        Message::TidalTokensSaved(Err(e)) => ctx.toast_err(e.message),
        Message::TidalSignOut => {
            let api = ctx.api.clone();
            return Task::perform(async move { api.del("/system/streamrip/tidal-tokens").await }, Message::TidalSignedOut);
        }
        Message::TidalSignedOut(Ok(r)) => {
            state.tidal = r;
            ctx.toast_ok("Signed out of Tidal");
        }
        Message::TidalSignedOut(Err(e)) => ctx.toast_err(e.message),
        Message::CopyText(text) => ctx.copy_text(text),
        Message::AskRipReset => state.dialog = Some(Dialog::RipReset),
        Message::RipReset => {
            state.dialog = None;
            let api = ctx.api.clone();
            return Task::perform(async move { api.post_empty("/system/streamrip/reset-config").await }, Message::RipResetDone);
        }
        Message::RipResetDone(Ok(r)) => {
            let ok = fmt::truthy(&r, "ok");
            let msg = if fmt::truthy(&r, "message") { fmt::s(&r, "message") } else if ok { "Connected".into() } else { "Failed".into() };
            state.rip_out = Some((ok, msg));
            if ok {
                ctx.toast_ok("streamrip config reset")
            } else {
                ctx.toast_err("streamrip config still doesn't load")
            }
        }
        Message::RipResetDone(Err(e)) => state.rip_out = Some((false, e.message)),
        Message::Slsk(Ok(r)) => state.slsk = r,
        Message::Slsk(Err(_)) => state.slsk = Value::Null,
        Message::SlskStart => {
            let api = ctx.api.clone();
            return Task::perform(async move { api.post_empty("/soulseek/start").await }, Message::SlskStarted);
        }
        Message::SlskStarted(r) => {
            match r {
                Ok(_) => ctx.toast_ok("Starting Soulseek"),
                Err(e) => ctx.toast_err(e.message),
            }
            // the supervisor starts it on its next check: look again once it has had a moment
            let api = ctx.api.clone();
            return Task::perform(
                async move {
                    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
                    api.get("/soulseek/status").await
                },
                Message::Slsk,
            );
        }
        Message::SlskReconnect => {
            state.slsk_reconnecting = true;
            let api = ctx.api.clone();
            return Task::perform(async move { api.post_empty("/system/soulseek/reconnect").await }, Message::SlskReconnected);
        }
        Message::SlskReconnected(r) => {
            state.slsk_reconnecting = false;
            match r {
                Ok(_) => ctx.toast_ok("Connecting to Soulseek"),
                // 429 "Wait 12 s": the last attempt was less than 30 s ago
                Err(e) => ctx.toast_err(e.message),
            }
            // the attempt takes a few seconds: look again once it has had them
            let api = ctx.api.clone();
            return Task::batch([
                get(ctx, "/soulseek/status", Message::Slsk),
                Task::perform(
                    async move {
                        tokio::time::sleep(std::time::Duration::from_millis(3000)).await;
                        api.get("/soulseek/status").await
                    },
                    Message::Slsk,
                ),
            ]);
        }
        Message::Vpn(Ok(r)) => {
            if !state.vpn_testing {
                state.vpn = r;
            }
        }
        Message::Vpn(Err(_)) => {
            if !state.vpn_testing {
                state.vpn = Value::Null;
            }
        }
        Message::VpnTest => {} // the server page saves the card first, then sends the test
        Message::VpnTested(r) => {
            state.vpn_testing = false;
            match r {
                Ok(r) => {
                    let n = &r["natpmp"];
                    let mut v = r.clone();
                    if n.is_object() && n["ok"] == json!(false) && fmt::truthy(n, "error") {
                        v["_failed"] = json!(format!("Port forwarding failed: {}", fmt::s(n, "error")));
                    }
                    state.vpn = v;
                }
                Err(e) => state.vpn = json!({ "state": "error", "error": if e.message.is_empty() { "Test failed".to_string() } else { e.message } }),
            }
        }
        Message::Dsp(Ok(d)) => {
            if !state.engine_loaded {
                state.engine_values = d["engine"]["values"].as_object().cloned().unwrap_or_default();
                state.engine_loaded = true;
            }
            state.dsp = d;
        }
        Message::Dsp(Err(_)) => state.dsp = Value::Null,
        Message::CacheClear => {
            let api = ctx.api.clone();
            return Task::perform(async move { api.post_empty("/playback/dsp/cache/clear").await }, Message::CacheCleared);
        }
        Message::CacheCleared(r) => {
            match r {
                Ok(_) => ctx.toast_ok("Cache cleared"),
                Err(e) => ctx.toast_err(e.message),
            }
            return get(ctx, "/playback/dsp", Message::Dsp);
        }
        Message::EngineBool(k, on) => {
            state.engine_values.insert(k, Value::Bool(on));
        }
        Message::EngineSelect(k, v) => {
            let setting = engine_settings(&state.dsp).into_iter().find(|s| fmt::s(s, "key") == k);
            let value = setting
                .and_then(|s| engine_options(&s).into_iter().find(|(o, _)| vs(o) == v).map(|(o, _)| o))
                .unwrap_or(Value::String(v));
            state.engine_values.insert(k, value);
        }
        Message::EngineNum(k, text) => {
            state.engine_edits.insert(k, text);
        }
        Message::Youtube(Ok(r)) => state.youtube = r,
        Message::Youtube(Err(e)) => state.youtube = json!({ "_error": e.message }),
        Message::YtRefresh => {
            state.yt_busy = true;
            let api = ctx.api.clone();
            return Task::perform(async move { api.post_empty("/youtube/refresh").await }, Message::YtRefreshed);
        }
        Message::YtRefreshed(r) => {
            state.yt_busy = false;
            match r {
                Ok(r) => {
                    let ok = fmt::truthy(&r, "refreshed");
                    let nothing_renewed = r["rotated"].as_u64() == Some(0);
                    let trouble = fmt::s(&r["sources"][fmt::s(&r, "source")], "message");
                    let text = if !ok {
                        "YouTube signed out".to_string()
                    } else if !trouble.is_empty() {
                        trouble
                    } else if nothing_renewed {
                        "Still signed in – Google sent no renewed cookies, so nothing was refreshed".to_string()
                    } else {
                        "Refreshed – still signed in".to_string()
                    };
                    if ok { ctx.toast_ok(text) } else { ctx.toast_err(text) }
                    state.youtube = r;
                }
                Err(e) => ctx.toast_err(e.message),
            }
        }
        Message::AskYtSignOut => state.dialog = Some(Dialog::YtSignOut),
        Message::YtSignOut => {
            state.dialog = None;
            let api = ctx.api.clone();
            return Task::perform(async move { api.post("/youtube/signout", json!({ "source": "all" })).await }, Message::YtSignedOut);
        }
        Message::YtSignedOut(Ok(r)) => {
            state.youtube = r;
            ctx.toast_ok("Signed out");
        }
        Message::YtSignedOut(Err(e)) => ctx.toast_err(e.message),
        Message::YtExtension => {
            let had = fmt::truthy(&state.youtube["extension"], "token_at");
            if had && !matches!(state.dialog, Some(Dialog::YtNewToken)) {
                state.dialog = Some(Dialog::YtNewToken);
                return Task::none();
            }
            state.dialog = None;
            let api = ctx.api.clone();
            return Task::perform(async move { api.post_empty("/youtube/extension-token").await }, Message::YtToken);
        }
        Message::YtToken(Ok(r)) => {
            state.yt_token = Some(fmt::s(&r, "token"));
            return get(ctx, "/youtube/auth", Message::Youtube);
        }
        Message::YtToken(Err(e)) => ctx.toast_err(e.message),
        Message::YtCopyToken => {
            if let Some(t) = &state.yt_token {
                ctx.copy(t.clone());
                ctx.toast_ok("Copied to clipboard");
            }
        }
        Message::OpenUrl(u) => ctx.open_url(u),
        Message::Tick => {
            let mut tasks = vec![get(ctx, "/soulseek/status", Message::Slsk)];
            if !state.vpn_testing {
                tasks.push(get(ctx, "/system/vpn", Message::Vpn));
            }
            return Task::batch(tasks);
        }
        Message::CloseDialog => state.dialog = None,
    }
    Task::none()
}

/// The VPN test, started by the server page after the Soulseek card was saved.
pub fn start_vpn_test(state: &mut State, ctx: &Ctx) -> Task<Message> {
    state.vpn_testing = true;
    let api = ctx.api.clone();
    Task::perform(async move { api.post_empty("/system/vpn/test").await }, Message::VpnTested)
}

/// A save of the Soulseek card failed before the test could run.
pub fn vpn_test_failed(state: &mut State, message: String) {
    state.vpn_testing = false;
    state.vpn = json!({ "state": "error", "error": message });
}

// ----- views ------------------------------------------------------------------------------

fn flat<'a, M: 'a>(body: impl Into<El<'a, M>>) -> El<'a, M> {
    container(body).padding([12, 14]).width(Length::Fill).style(theme::card).into()
}

fn tool_row<'a>(state: &'a State, t: &'a Value) -> El<'a, Message> {
    let id = fmt::s(t, "id");
    let latest = if fmt::truthy(t, "latest") { fmt::s(t, "latest") } else { String::new() };
    let updatable = fmt::truthy(t, "updatable");
    let busy_any = state.busy.is_some();
    let action: El<'a, Message> = if state.busy.as_deref() == Some(id.as_str()) {
        btn_sm(if tool_install(t) { "Installing…" } else { "Updating…" }, Btn::Default, None::<Message>).into()
    } else if tool_install(t) && updatable {
        btn_icon(Icon::Download, if latest.is_empty() { "Install".to_string() } else { format!("Install {latest}") }, Btn::Primary, (!busy_any).then_some(Message::Update(id.clone()))).into()
    } else if fmt::truthy(t, "update_available") && updatable {
        btn_icon(Icon::Download, format!("Update to {latest}"), Btn::Primary, (!busy_any).then_some(Message::Update(id.clone()))).into()
    } else if fmt::truthy(t, "update_available") {
        badge(format!("{latest} available"), Badge::Warn)
    } else if fmt::truthy(t, "latest") {
        badge("Up to date", Badge::Ok)
    } else if updatable {
        widgets::faint("Not checked yet").size(12).into()
    } else {
        container(iced::widget::space()).into()
    };
    let updated = fmt::s(t, "source") == "updated" && id != "slskd" && id != "translator";
    let mut head = row![
        crate::widgets::t(tool_or(t, "name", "id")).size(14).font(theme::semibold()),
        widgets::small_muted(if fmt::truthy(t, "installed") { fmt::s(t, "installed") } else { "not installed".into() }).font(theme::mono()),
    ]
    .spacing(6)
    .align_y(Alignment::Center);
    if updated {
        head = head.push(widgets::small_muted("(updated)"));
    }
    let mut left = column![head].spacing(3).width(Length::Fill);
    if fmt::truthy(t, "note") {
        left = left.push(help(fmt::s(t, "note")));
    }
    if fmt::truthy(t, "error") {
        left = left.push(super::err_text(fmt::s(t, "error")));
    }
    if fmt::truthy(t, "resettable") && updatable && !busy_any {
        left = left.push(link_muted(tool_reset(t).0, Message::AskReset(id.clone())));
    }
    column![container(row![left, action].spacing(12).align_y(Alignment::Center)).padding([10, 14]), widgets::divider()].into()
}

/// Settings → Downloads → Download tools.
pub fn tools_card<'a>(state: &'a State, values: &'a Value) -> El<'a, Message> {
    let Some(tools) = &state.tools else {
        return widgets::card(widgets::skeleton_rows(3)).width(Length::Fill).into();
    };
    let meta = &values["_meta"];
    let mut list = column![];
    for t in tools {
        list = list.push(tool_row(state, t));
    }
    let toggle = |key: &'static str, label: &'static str| -> Option<El<'a, Message>> {
        values.get(key)?;
        let locked = meta[key]["source"].as_str() == Some("env");
        Some(switch_maybe(fmt::truthy(values, key), label, (!locked).then_some(move |on| Message::Auto(key.to_string(), on))))
    };
    let mut extra = column![].spacing(10);
    if values.get("translator_model").is_some() {
        let locked = meta["translator_model"]["source"].as_str() == Some("env");
        let opts = vec![
            Opt::new("small", "Small – Qwen3 4B Instruct (about 2.5 GB)"),
            Opt::new("best", "Best – Qwen3 8B (about 5 GB)"),
        ];
        let current = fmt::s(values, "translator_model");
        let pick: El<'a, Message> = if locked {
            help(opts.iter().find(|o| o.key == current).map(|o| o.label.clone()).unwrap_or(current.clone())).into()
        } else {
            select(opts, &current, Message::Model)
        };
        extra = extra.push(widgets::field("Translator model", pick));
        extra = extra.push(help("Small runs on most servers (about 4 GB of memory while it works); Best translates better and needs about 7 GB. It only runs while there is something to translate."));
    }
    for (k, l) in [
        ("tools_auto_ytdlp", "Update yt-dlp automatically (recommended – sites change often)"),
        ("tools_auto_streamrip", "Update streamrip automatically when a new release is out"),
        ("tools_auto_cadenza", "Update Cadenza automatically when a new release is out"),
        ("tools_auto_slskd", "Update slskd automatically when a new release is out"),
        ("tools_auto_translator", "Update the Translator (llama.cpp) automatically"),
        ("translate_background", "Translate the whole library in the background (titles on screen are always translated first)"),
    ] {
        if let Some(sw) = toggle(k, l) {
            extra = extra.push(sw);
        }
    }
    extra = extra.push(help("Every tool is checked daily; updates install by themselves only where the switch is on. Change the schedule under Scheduled tasks."));
    widgets::card(
        column![
            super::between(
                widgets::h2("Download tools").into(),
                btn_icon(Icon::Refresh, if state.checking { "Checking…" } else { "Check for updates" }, Btn::Default, (!state.checking && state.busy.is_none()).then_some(Message::Check)).into(),
            ),
            help("yt-dlp, streamrip, Cadenza, slskd and the Translator update in place – no new Encore image needed. Updates are kept in the config folder."),
            widgets::card_flat(list).width(Length::Fill),
            extra,
        ]
        .spacing(12)
        .width(Length::Fill),
    )
    .width(Length::Fill)
    .into()
}

/// The Tidal account block of the streamrip card.
pub fn tidal_block<'a>(state: &'a State) -> El<'a, Message> {
    let r = &state.tidal;
    if fmt::truthy(r, "unavailable") {
        return flat(column![crate::widgets::t("Tidal account").size(14).font(theme::bold()), help("Status unavailable")].spacing(4));
    }
    let partial = fmt::truthy(r, "partial");
    let signed = fmt::truthy(r, "signed_in") || partial;
    let state_text = if partial {
        "Signed in for 16/44.1 FLAC only – sign in again for hi-res (step 2 was not finished)."
    } else if fmt::truthy(r, "signed_in") {
        "Signed in – Tidal downloads use this account."
    } else {
        "Not signed in – Tidal needs a one-time sign-in with your own Tidal account (no API key)."
    };
    let mut right = row![].spacing(10).align_y(Alignment::Center);
    if signed {
        right = right.push(link_muted("Sign out of Tidal", Message::TidalSignOut));
    }
    right = right.push(btn_icon(
        Icon::Link,
        if signed { "Sign in again" } else { "Sign in to Tidal" },
        Btn::Default,
        (!state.tidal_busy).then_some(Message::TidalSignIn),
    ));
    let mut c = column![super::between(
        column![crate::widgets::t("Tidal account").size(14).font(theme::bold()), help(state_text)].spacing(2).into(),
        right.into(),
    )]
    .spacing(10);
    let status = fmt::s(r, "status");
    if status == "waiting" && !fmt::s(r, "url").is_empty() {
        let steps = fmt::f(r, "steps").max(1.0) as i64;
        let step = fmt::f(r, "step").max(1.0) as i64;
        let prefix = if steps > 1 { format!("Step {step} of {steps} · ") } else { String::new() };
        c = c.push(
            row![
                help(format!("{prefix}Open")),
                super::link(fmt::s(r, "url"), Message::OpenUrl(fmt::s(r, "url"))),
                help("on any device, approve in your Tidal account, then come back here. Waiting…"),
            ]
            .spacing(4)
            .align_y(Alignment::Center)
            .wrap(),
        );
    } else if status == "done" {
        c = c.push(result_line(true, fmt::s(r, "message")));
    } else if status == "failed" {
        let m = fmt::s(r, "message");
        c = c.push(result_line_copy(false, if m.is_empty() { "Sign-in failed" } else { m.as_str() }, Message::CopyText));
    }
    c = c.push(link_muted(if state.tidal_paste { "▾ Paste tokens instead" } else { "▸ Paste tokens instead" }, Message::TidalPaste(!state.tidal_paste)));
    if state.tidal_paste {
        c = c.push(help("On a computer with Tidal access: install streamrip (pip install streamrip), run rip url https://tidal.com/browse/track/1, approve the link it prints, then paste the [tidal] section of its config file here (rip config path shows where it is)."));
        c = c.push(text_area(&state.tidal_toml, "[tidal]\nuser_id = \"…\"\naccess_token = \"…\"", 6, Message::TidalToml));
        c = c.push(container(btn_icon(Icon::Check, "Save tokens", Btn::Default, Some(Message::TidalSaveTokens))));
    }
    flat(c)
}

/// streamrip's config file: a fresh one from its defaults mends a broken one.
pub fn streamrip_block<'a>(state: &'a State) -> El<'a, Message> {
    let mut c = column![super::between(
        column![
            crate::widgets::t("streamrip config file").size(14).font(theme::bold()),
            help("When requests fail with \"Error loading config\", write a fresh config.toml from streamrip's defaults. The old file is kept as a backup; the Tidal sign-in, Deezer ARL and Qobuz login are carried over."),
        ]
        .spacing(2)
        .into(),
        btn_icon(Icon::Refresh, "Reset streamrip config", Btn::Default, Some(Message::AskRipReset)).into(),
    )]
    .spacing(10);
    if let Some((ok, msg)) = &state.rip_out {
        c = c.push(result_line_copy(*ok, msg, Message::CopyText));
    }
    flat(c)
}

/// How Encore's Soulseek engine is doing.
pub fn soulseek_block<'a>(state: &'a State) -> El<'a, Message> {
    let r = &state.slsk;
    let st = fmt::s(r, "state");
    let p = theme::look();
    let red = matches!(st.as_str(), "crashed" | "blocked");
    let mut buttons = row![].spacing(6);
    if let Some(wait) = reconnect_wait(r) {
        // attempts are at least 30 s apart: the button waits that out
        let on = (wait.is_none() && !state.slsk_reconnecting).then_some(Message::SlskReconnect);
        let label = match wait {
            Some(n) => format!("Reconnect now (in {n} s)"),
            None => "Reconnect now".to_string(),
        };
        buttons = buttons.push(btn_sm(label, Btn::Default, on));
    }
    if can_start_now(&st) {
        buttons = buttons.push(btn_sm("Start now", Btn::Default, Some(Message::SlskStart)));
    }
    let mut c = column![super::between(crate::widgets::t("Status").size(14).font(theme::bold()).into(), buttons.into())].spacing(6);
    c = c.push(crate::widgets::t(soulseek_state_text(r)).size(12.5).color(if red { p.err } else { p.muted }));
    let session = &r["session"];
    if has_session(r) {
        let color = match session_tone(session) {
            "ok" => p.ok,
            "warn" => p.warn,
            "err" => p.err,
            _ => p.muted,
        };
        c = c.push(crate::widgets::t(session_text(session)).size(12.5).color(color));
        let details = session_details(session, now());
        if !details.is_empty() {
            c = c.push(crate::widgets::t(details).size(12).color(p.faint));
        }
    }
    if fmt::truthy(r, "warning") {
        c = c.push(super::warn_text(fmt::s(r, "warning")));
    }
    let logged_in = if has_session(r) { fmt::s(session, "state") == "logged_in" } else { fmt::truthy(r, "logged_in") };
    let unhealthy = red || (st == "running" && !logged_in);
    let log: Vec<String> = fmt::items(&r["log"]).iter().filter_map(|l| l.as_str().map(String::from)).collect();
    if unhealthy && !log.is_empty() {
        c = c.push(help(format!("Engine log (last {} lines)", log.len())));
        c = c.push(container(crate::widgets::t(log.join("\n")).size(12).font(theme::mono())).padding(10).width(Length::Fill).style(theme::panel2));
    }
    flat(c)
}

/// The VPN for Soulseek block: the WireGuard fields (given), the tunnel's state and Test VPN
/// (`on_test` is the page's message; None while a test runs). A failed test carries Copy
/// (`on_copy` gets its words).
pub fn vpn_block<'a, M: Clone + 'a>(
    state: &'a State,
    fields: El<'a, M>,
    on_test: Option<M>,
    on_copy: impl Fn(String) -> M,
) -> El<'a, M> {
    let v = &state.vpn;
    let p = theme::look();
    let text = if state.vpn_testing { "Testing…".to_string() } else if v.is_null() { "Status unavailable".to_string() } else { super::diagnostics::vpn_text(v) };
    let is_err = v["state"].as_str() == Some("error") && !state.vpn_testing;
    let mut status = row![crate::widgets::t(text.clone()).size(12.5).color(if is_err { p.err } else { p.muted })].spacing(6).align_y(Alignment::Center);
    if fmt::truthy(v, "_failed") {
        status = status.push(super::err_text(fmt::s(v, "_failed")));
    }
    if is_err || fmt::truthy(v, "_failed") {
        let all = [text, fmt::s(v, "_failed")].into_iter().filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" · ");
        status = status.push(crate::widgets::details::copy_btn(on_copy(all)));
    }
    flat(
        column![
            crate::widgets::t("VPN for Soulseek").size(14).font(theme::bold()),
            fields,
            super::between(
                status.wrap().into(),
                btn_icon(Icon::Refresh, "Test VPN", Btn::Default, on_test).into(),
            ),
        ]
        .spacing(12),
    )
}

impl State {
    /// A VPN test is under way (the page saved the card and is waiting for the answer).
    pub fn vpn_testing(&self) -> bool {
        self.vpn_testing
    }

    /// The page starts saving the Soulseek card before the test: the state line says so.
    pub fn vpn_testing_started(&mut self) {
        self.vpn_testing = true;
    }
}

/// Server processing: the pre-render cache and the engine's own settings.
pub fn cadenza_block<'a>(state: &'a State, values: &'a Value) -> El<'a, Message> {
    let d = &state.dsp;
    let cache = flat(
        column![
            super::between(
                column![
                    crate::widgets::t("Pre-render cache").size(14).font(theme::bold()),
                    help(if d.is_null() { "Status unavailable".to_string() } else { cadenza_cache_text(&d["cache"]) }),
                ]
                .spacing(2)
                .into(),
                btn_icon(Icon::Trash, "Clear cache", Btn::Default, Some(Message::CacheClear)).into(),
            ),
            help(cadenza_engine_text(&d["engine"])),
        ]
        .spacing(8),
    );
    let locked = values["_meta"]["cadenza_engine_settings"]["source"].as_str() == Some("env");
    let settings = engine_settings(d);
    let mut eng = column![crate::widgets::t("Engine settings").size(14).font(theme::bold())].spacing(10);
    if !state.engine_loaded {
        eng = eng.push(help(if d.is_null() { "Status unavailable" } else { "Checking…" }));
    } else if settings.is_empty() {
        eng = eng.push(help("This engine has no settings of its own."));
    } else {
        eng = eng.push(help(format!(
            "The settings of the engine in use. People’s own filter and dither choices under Settings → Playback still win for their streams.{}",
            if locked { " Set by an environment variable." } else { "" }
        )));
        for s in settings {
            let key = fmt::s(&s, "key");
            let label = if fmt::truthy(&s, "label") { fmt::s(&s, "label") } else { key.clone() };
            let hint = format!("{}Default: {}", if fmt::truthy(&s, "help") { format!("{} · ", fmt::s(&s, "help")) } else { String::new() }, engine_default_text(&s));
            let current = state.engine_values.get(&key).cloned().unwrap_or_else(|| s["default"].clone());
            let k2 = key.clone();
            let field: El<'a, Message> = match fmt::s(&s, "type").as_str() {
                "bool" => switch_maybe(super::truthy(&current), label, (!locked).then_some(move |on| Message::EngineBool(k2.clone(), on))),
                "select" => {
                    let opts: Vec<Opt> = engine_options(&s).into_iter().map(|(v, l)| Opt::new(vs(&v), vs(&l))).collect();
                    let pick = if opts.iter().any(|o| o.key == vs(&current)) { vs(&current) } else { vs(&s["default"]) };
                    if locked {
                        widgets::field(label, help(opts.iter().find(|o| o.key == pick).map(|o| o.label.clone()).unwrap_or(pick.clone())))
                    } else {
                        widgets::field(label, select(opts, &pick, move |v| Message::EngineSelect(k2.clone(), v)))
                    }
                }
                _ => {
                    let text = state.engine_edits.get(&key).cloned().unwrap_or_else(|| vs(&current));
                    if locked {
                        widgets::field(label, super::input_locked::<Message>(&text).width(160))
                    } else {
                        widgets::field(label, super::input("", &text, move |v| Message::EngineNum(k2.clone(), v)).width(160))
                    }
                }
            };
            eng = eng.push(column![field, help(hint)].spacing(3));
        }
        if !locked {
            eng = eng.push(help("Press Save on this card to keep the engine settings."));
        }
    }
    column![cache, flat(eng)].spacing(12).into()
}

/// Settings → YouTube sign-in: which cookies yt-dlp uses, Refresh now / Sign out, the extension token.
pub fn youtube_block<'a>(state: &'a State) -> El<'a, Message> {
    let r = &state.youtube;
    let mut c = column![].spacing(8);
    if fmt::truthy(r, "_error") {
        c = c.push(super::err_text(fmt::s(r, "_error")));
    } else if r.is_null() {
        c = c.push(help("Loading…"));
    } else {
        let source = fmt::s(r, "source");
        let has_source = fmt::truthy(r, "source");
        let sources = &r["sources"];
        let ok = has_source && fmt::truthy(&sources[&source], "signed_in");
        let b: El<'a, Message> = if !has_source {
            badge("No cookies", Badge::Plain)
        } else if ok {
            badge("Signed in", Badge::Ok)
        } else {
            badge("Signed out", Badge::Warn)
        };
        let label = if fmt::truthy(r, "source_label") { fmt::s(r, "source_label") } else { "Not signed in".to_string() };
        let mut left = column![crate::widgets::t(label).size(14).font(theme::bold())].spacing(2);
        if fmt::truthy(r, "account") {
            left = left.push(help(format!("Account: {}", fmt::s(r, "account"))));
        }
        c = c.push(super::between(left.into(), b));
        if has_source {
            let next = fmt::f(r, "next_refresh");
            c = c.push(help(format!(
                "Last refresh {}{}",
                super::ago(fmt::f(r, "last_refresh")),
                if next != 0.0 { format!(" · next refresh {}", super::until(next)) } else { String::new() }
            )));
        }
        let lines: Vec<String> = ["handoff", "extension", "pasted"].iter().map(|k| source_line(k, &sources[*k])).collect();
        c = c.push(help(lines.join("   ")));
        if fmt::truthy(r, "diagnostics") {
            c = c.push(widgets::faint(fmt::s(r, "diagnostics")).size(12));
        }
    }
    c = c.push(
        row![
            btn_icon(Icon::Link, "Connect the browser extension", Btn::Primary, Some(Message::YtExtension)),
            btn_icon(Icon::Refresh, if state.yt_busy { "Refreshing…" } else { "Refresh now" }, Btn::Default, (!state.yt_busy).then_some(Message::YtRefresh)),
            btn_icon(Icon::X, "Sign out", Btn::Danger, Some(Message::AskYtSignOut)),
        ]
        .spacing(8)
        .wrap(),
    );
    if let Some(token) = &state.yt_token {
        c = c.push(flat(
            column![
                crate::widgets::t("Connect the Encore extension").size(14).font(theme::bold()),
                help("1. Install it on a computer: download Encore-youtube-extension-<version>.zip from the releases page and unzip it. Chrome, Edge or Brave: Extensions → Developer mode → Load unpacked → the chrome folder, then Details → Allow in Incognito. Firefox: about:debugging → Load Temporary Add-on → firefox/manifest.json, then about:addons → Run in Private Windows → Allow."),
                help("2. Open its options and paste this Encore’s address and the token below."),
                help("3. Press Hand over a YouTube sign-in in the extension. A private window opens: sign in to Google there. Once YouTube Music shows you signed in, the extension sends the sign-in to Encore and closes the window without signing out."),
                row![
                    container(crate::widgets::t(token.as_str()).size(12.5).font(theme::mono())).padding(8).width(Length::Fill).style(theme::panel2),
                    btn_icon(Icon::Copy, "Copy", Btn::Default, Some(Message::YtCopyToken)),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
                widgets::faint("Shown once. It can only send YouTube cookies to Encore; making a new one stops the old one. After the hand-off Encore owns that sign-in and refreshes it itself every 4 hours, also while your computer is off.").size(12),
            ]
            .spacing(8),
        ));
    }
    flat(c)
}

pub fn overlay<'a>(state: &'a State) -> Option<El<'a, Message>> {
    Some(match state.dialog.as_ref()? {
        Dialog::ToolReset { ask, .. } => modal(
            "Download tools",
            widgets::muted(ask.as_str()).size(14),
            Some(
                row![
                    btn("Cancel", Btn::Default, Some(Message::CloseDialog)),
                    btn("Go back", Btn::Danger, Some(Message::Reset)),
                ]
                .spacing(10)
                .into(),
            ),
            false,
            Message::CloseDialog,
        ),
        Dialog::RipReset => widgets::confirm(
            "Reset streamrip config",
            "Replace streamrip's config.toml with a fresh one from its defaults? The current file is kept as a backup, and the Tidal sign-in, Deezer ARL and Qobuz login are carried over.",
            "Reset",
            false,
            Message::RipReset,
            Message::CloseDialog,
        ),
        Dialog::YtSignOut => widgets::confirm(
            "Sign out of YouTube",
            "Forget the handed-over sign-in and the extension’s cookies? Pasted cookies stay (clear them below).",
            "Sign out",
            true,
            Message::YtSignOut,
            Message::CloseDialog,
        ),
        Dialog::YtNewToken => widgets::confirm(
            "New extension token",
            "Make a new token? The extension stops working until you enter the new one in its options.",
            "Make a new token",
            false,
            Message::YtExtension,
            Message::CloseDialog,
        ),
    })
}

/// Whether the Tidal sign-in is being waited on (the page polls every 3 s then).
pub fn tidal_waiting(state: &State) -> bool {
    state.tidal_waiting
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_match_the_web() {
        assert_eq!(soulseek_state_text(&Value::Null), "Status unavailable");
        assert_eq!(soulseek_state_text(&json!({ "state": "running", "version": "v0.24.1", "logged_in": true, "username": "alice" })), "Running v0.24.1 · signed in as alice");
        assert_eq!(soulseek_state_text(&json!({ "state": "crashed", "error": "boom" })), "Crashed – boom");
        assert_eq!(soulseek_state_text(&json!({ "state": "off", "enabled": false })), "Off – switch on Enable Soulseek and save");
    }

    #[test]
    fn the_soulseek_session_under_the_engine() {
        let now = 1_760_000_000.0;
        let r = json!({ "state": "running", "version": "0.26.1", "logged_in": false,
            "session": { "state": "waiting", "reason": "timeout", "next_attempt_at": now + 192.0, "attempts": 2, "last_search_at": now - 300.0,
                "searches_waiting": 12, "search_running": false, "reconnect_in": 0, "can_reconnect": true,
                "text": "Disconnected – the login timed out (the Soulseek server did not answer in 5 s). Next attempt at 14:20 (attempt 3)." } });
        assert_eq!(soulseek_state_text(&r), "Running v0.26.1");
        assert_eq!(session_tone(&r["session"]), "warn");
        assert!(session_text(&r["session"]).starts_with("Disconnected – the login timed out"));
        assert_eq!(session_details(&r["session"], now), "Next attempt in 3 m 12 s · 12 searches waiting for the connection · last search 5 min ago");
        assert_eq!(reconnect_wait(&r), Some(None));
        let mut r2 = r.clone();
        r2["session"]["reconnect_in"] = json!(12);
        assert_eq!(reconnect_wait(&r2), Some(Some(12)));
        // logged in, with searches turned away for a minute
        let s = json!({ "state": "logged_in", "reason": "rate_limited", "next_search_at": now + 45.0, "search_running": true, "can_reconnect": false,
            "text": "Logged in as alice since 14:02. Rate limited by the Soulseek server – next search at 14:05" });
        assert_eq!(session_tone(&s), "warn");
        assert_eq!(session_details(&s, now), "Searches go on in 45 s · a search is running");
        // searches held back while logged in: one waits for its turn, more wait for theirs
        let one = json!({ "state": "logged_in", "searches_waiting": 1 });
        assert_eq!(session_details(&one, now), "1 search waiting for its turn");
        let three = json!({ "state": "logged_in", "searches_waiting": 3 });
        assert_eq!(session_details(&three, now), "3 searches waiting for their turn");
        assert_eq!(session_details(&json!({ "state": "connecting", "searches_waiting": 1 }), now), "1 search waiting for the connection");
        assert_eq!(reconnect_wait(&json!({ "state": "running", "session": s })), None);
        // rejected: red; away from the card the line names it (the server's words) instead of saying "below"
        let s = json!({ "state": "stopped", "reason": "rejected", "text": "Rejected: invalid username or password – fix them below" });
        assert_eq!(session_tone(&s), "err");
        assert_eq!(session_text_elsewhere(&session_text(&s)), "Rejected: invalid username or password – fix them in Settings → Downloads → Soulseek");
        // not installed: the engine line only
        let r = json!({ "state": "not_installed", "hint": "Install or update it under Download tools", "session": { "state": "stopped", "text": "Soulseek is not installed" } });
        assert!(!has_session(&r));
        assert_eq!(soulseek_state_text(&r), "Not installed · Install or update it under Download tools");
        assert_eq!(reconnect_wait(&r), None);
        assert_eq!(span(3900.0), "1 h 5 m");
        assert!(can_start_now("blocked") && !can_start_now("running"));
        assert_eq!(cadenza_cache_text(&Value::Null), "Cache: not available on this server");
        assert_eq!(cadenza_cache_text(&json!({ "used_mb": 812.4, "max_mb": 4096, "files": 23, "last_factor": 6.2 })), "Cache: 812 MB of 4096 MB · 23 songs · renders at 6.2× real time");
        assert_eq!(cadenza_engine_text(&json!({ "source": "updated", "version": "2.1.0" })), "Engine: Cadenza 2.1.0 (updated)");
        assert_eq!(cadenza_engine_text(&Value::Null), "Engine: Cadenza (built in)");
        assert_eq!(tool_reset(&json!({ "id": "cadenza" })).0, "Use the built-in engine");
        assert_eq!(tool_reset(&json!({ "id": "ytdlp", "name": "yt-dlp" })).2, "Back to the bundled yt-dlp");
        assert!(tool_install(&json!({ "id": "slskd" })) && !tool_install(&json!({ "id": "slskd", "installed": "1.0" })));
        assert_eq!(source_line("pasted", &json!({})), "Pasted cookies: none");
        assert_eq!(source_line("handoff", &json!({ "has_cookies": true, "signed_in": true, "account": "me" })), "Handed-over sign-in: signed in as me");
    }

    #[test]
    fn engine_settings_only_keep_what_differs() {
        let mut st = State {
            dsp: json!({ "engine": { "settings": [
                { "key": "taps", "type": "number", "default": 64 },
                { "key": "mode", "type": "select", "default": "a", "options": [["a", "A"], ["b", "B"]] },
                { "key": "fast", "type": "bool", "default": false } ] } }),
            engine_loaded: true,
            ..State::default()
        };
        st.engine_edits.insert("taps".into(), "128".into());
        st.engine_values.insert("mode".into(), json!("b"));
        st.engine_values.insert("fast".into(), json!(false));
        let v: Value = serde_json::from_str(&engine_settings_value(&st).unwrap()).unwrap();
        assert_eq!(v["taps"], 128);
        assert_eq!(v["mode"], "b");
        assert!(v.get("fast").is_none(), "the default is not saved");
        st.engine_edits.insert("taps".into(), "64".into());
        let v: Value = serde_json::from_str(&engine_settings_value(&st).unwrap()).unwrap();
        assert!(v.get("taps").is_none());
        assert_eq!(engine_default_text(&json!({ "type": "select", "default": "a", "options": [["a", "A"]] })), "A");
    }
}
