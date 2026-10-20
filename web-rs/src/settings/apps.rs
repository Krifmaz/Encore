//! Port of web/js/pages/settings.js (the Apps & sources extras, on Downloads, Integrations and Server processing
//! since R2) – see docs/archive/ui-rewrite/CONTRACT.md
//! What the Server cards show besides their fields, drawn by this web app and not by the schema: the Download
//! tools card, Tidal sign-in and the streamrip config reset (streamrip), the VPN sub-block and the engine's
//! status (Soulseek), the pre-render cache and the engine's own settings (Cadenza), and How requests are fulfilled.

use super::diagnostics;
use super::fields::Group;
use super::{test_result, Hub};
use crate::api;
use crate::dom;
use crate::json::{js_string, json, truthy, J};
use crate::ui::{esc, escv, icon, toast};
use serde_json::{Map, Value};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use web_sys::Element;

/// What a card holds between its result line and Save: its fields, with the extras this web app draws for
/// some cards (YouTube sign-in above the fields; Tidal sign-in and the streamrip config under streamrip's; the
/// pre-render cache and engine settings under Cadenza's; the engine's status under Soulseek's).
pub fn card_body(id: &str, fields_html: &str) -> String {
    format!("{yt}{fields_html}{tidal}{cadenza}\n      {slsk}",
        yt = if id == "youtube" { crate::pages::youtube::youtube_block() } else { String::new() },
        tidal = if id == "streamrip" { tidal_block() + &streamrip_config_block() } else { String::new() },
        cadenza = if id == "cadenza" { cadenza_cache_block() + &engine_settings_block() } else { String::new() },
        slsk = if id == "slskd" { soulseek_status_block() } else { String::new() })
}

/// Wires the extras of a card (see [`card_body`]; the VPN block is part of Soulseek's fields).
pub fn bind_card(hub: &Rc<Hub>, g: &Rc<Group>, card: &Element) {
    match g.id.as_str() {
        "streamrip" => { bind_tidal(&hub.view, card); bind_streamrip_config(card); }
        "cadenza" => bind_cadenza_cache(hub, card),
        "slskd" => { bind_vpn(hub, g.clone(), card); bind_soulseek(hub, card); }
        "youtube" => crate::pages::youtube::bind_youtube(card),
        _ => {}
    }
}

// Settings → Soulseek → VPN for Soulseek: the WireGuard fields, the tunnel's state (polled every 5 s while the card is open) and Test VPN.
pub fn vpn_block(inner: &str) -> String {
    format!(r##"<div class="card flat mt" id="vpn-block" style="padding:12px 14px">
    <b>VPN for Soulseek</b>
    <div class="form-grid mt">{inner}</div>
    <div class="row between wrap mt" style="gap:8px">
      <div class="small muted" id="vpn-state" style="min-width:0">Checking…</div>
      <button class="btn sm" id="vpn-test" type="button">{refresh}Test VPN</button>
    </div>
  </div>"##, refresh = icon("refresh"))
}

thread_local! {
    /// Bumped on every paint of the Soulseek card, so the 5 s poll of an earlier paint stops.
    static VPN_POLL: Cell<u32> = const { Cell::new(0) };
}

pub fn bind_vpn(hub: &Rc<Hub>, g: Rc<Group>, card: &Element) {
    let (Some(state), Some(btn)) = (dom::qs_in(card, "#vpn-state"), dom::qs_in(card, "#vpn-test")) else { return };
    let refresh: Rc<dyn Fn()> = {
        let (state, btn) = (state.clone(), btn.clone());
        Rc::new(move || {
            let (state, btn) = (state.clone(), btn.clone());
            dom::spawn(async move {
                match api::get("/system/vpn").await {
                    Ok(r) => { if !btn.has_attribute("disabled") { dom::set_html(&state, &diagnostics::vpn_state_html(&r)); } }
                    Err(_) => dom::set_text(&state, "Status unavailable"),
                }
            });
        })
    };
    refresh();
    let gen = VPN_POLL.with(|g| { g.set(g.get().wrapping_add(1)); g.get() });
    {
        let (card, btn, refresh) = (card.clone(), btn.clone(), refresh.clone());
        dom::interval(&hub.view, 5000, move || {
            if VPN_POLL.with(|g| g.get()) != gen || !dom::is_connected(&card) { return; }
            if card.has_attribute("open") && !btn.has_attribute("disabled") { refresh(); }
        });
    }
    let (hub2, card2, btn2) = (hub.clone(), card.clone(), btn.clone());
    dom::on(&btn, "click", move |e| {
        dom::prevent(e);
        let (hub, g, card, btn, state) = (hub2.clone(), g.clone(), card2.clone(), btn2.clone(), state.clone());
        dom::set_disabled(&btn, true);
        dom::set_html(&state, r##"<span class="spinner" style="width:10px;height:10px;border-width:2px"></span> Testing…"##);
        dom::spawn(async move {
            let r = match hub.save(&g, true, &card).await {
                Ok(()) => api::post_empty("/system/vpn/test").await,
                Err(e) => Err(e),
            };
            match r {
                Ok(r) => {
                    let n = r.g("natpmp");
                    let failed = if n.is_object() && n.g("ok") == &Value::Bool(false) && n.truthy("error") {
                        format!(r##" · <span style="color:var(--err)">Port forwarding failed: {}</span>"##, esc(&n.s("error")))
                    } else { String::new() };
                    dom::set_html(&state, &format!("{}{failed}", diagnostics::vpn_state_html(&r)));
                }
                Err(err) => {
                    let m = if err.message.is_empty() { "Test failed".to_string() } else { err.message.clone() };
                    dom::set_html(&state, &format!(r##"<span style="color:var(--err)">{}</span>"##, esc(&m)));
                }
            }
            dom::set_disabled(&btn, false);
        });
    });
}

// Server processing: how full the Cadenza pre-render cache is, how fast it renders, and a button to empty it.
pub fn cadenza_cache_block() -> String {
    format!(r##"<div class="card flat mt" id="cadenza-cache" style="padding:12px 14px">
    <div class="row between wrap" style="gap:8px">
      <div style="min-width:0"><b>Pre-render cache</b><div class="small muted" id="cadenza-cache-state">Checking…</div></div>
      <button class="btn sm" id="cadenza-cache-clear" type="button">{}Clear cache</button>
    </div>
    <div class="small muted mt" id="cadenza-engine">Engine: …</div>
  </div>"##, icon("trash"))
}

/// "Cache: 812 MB of 4096 MB · 23 songs · renders at 6.2× real time" (the factor only once a render has finished).
pub fn cadenza_cache_text(c: &Value) -> String {
    if !c.is_object() { return "Cache: not available on this server".into(); }
    let mb = |v: f64| { let r = crate::playback::js_round(if v.is_finite() { v } else { 0.0 }); format!("{}", r as i64) };
    let n = c.f("files");
    let n = if n.is_finite() { n } else { 0.0 };
    let f = c.f("last_factor");
    format!("Cache: {} MB of {} MB · {} {}{}", mb(c.f("used_mb")), mb(c.f("max_mb")), n, if n == 1.0 { "song" } else { "songs" },
        if f > 0.0 { format!(" · renders at {}× real time", crate::player::to_fixed1(f)) } else { String::new() })
}

/// "Engine: Cadenza 2.1.0 (updated)" / "Engine: Cadenza (built in)" – `engine` of GET /playback/dsp.
pub fn cadenza_engine_text(e: &Value) -> String {
    format!("Engine: {}", if e.is_object() && e.s("source") == "updated" && e.truthy("version") { format!("Cadenza {} (updated)", e.s("version")) } else { "Cadenza (built in)".to_string() })
}

// Server processing → Engine settings: drawn only from the list the active engine gives (engine.settings in
// GET /playback/dsp: `cadenza render --describe`, or the built-in engine's own), saved to cadenza_engine_settings.
struct EngineSettings {
    settings: Vec<Value>,
    values: Value,
}

thread_local! {
    /// { settings, values } of the engine the block was drawn for
    static ENGINE: RefCell<Option<EngineSettings>> = const { RefCell::new(None) };
}

pub fn engine_settings_block() -> String {
    r##"<div class="card flat mt" id="cadenza-engine-settings" style="padding:12px 14px">
    <b>Engine settings</b>
    <div class="small muted" id="cadenza-engine-hint">Checking…</div>
    <div class="form-grid mt" id="cadenza-engine-fields" hidden></div>
  </div>"##.to_string()
}

const ENGINE_TYPES: &[&str] = &["select", "bool", "number"];

/// JavaScript `Number(v)`.
fn to_num(v: &Value) -> f64 {
    match v {
        Value::Null => 0.0,
        Value::Bool(b) => if *b { 1.0 } else { 0.0 },
        Value::Number(n) => n.as_f64().unwrap_or(f64::NAN),
        Value::String(t) => { let t = t.trim(); if t.is_empty() { 0.0 } else { t.parse().unwrap_or(f64::NAN) } }
        _ => f64::NAN,
    }
}

/// `Array.isArray(s.options) ? s.options : []`, each as (value, label).
fn engine_options(s: &Value) -> Vec<(Value, Value)> {
    s.a("options").iter().map(|o| (o.get(0).cloned().unwrap_or(Value::Null), o.get(1).cloned().unwrap_or(Value::Null))).collect()
}

fn engine_default_text(s: &Value) -> String {
    match s.s("type").as_str() {
        "bool" => (if s.truthy("default") { "on" } else { "off" }).to_string(),
        "select" => engine_options(s).into_iter().find(|(v, _)| js_string(v) == s.s("default")).map(|(_, l)| js_string(&l)).unwrap_or_else(|| s.s("default")),
        _ => s.s("default"),
    }
}

/// One engine setting as a form field (data-ek = its key). A saved select value the engine no longer offers shows the default.
fn engine_field(s: &Value, v: &Value, locked: bool) -> String {
    let dis = if locked { "disabled" } else { "" };
    let k = esc(&s.s("key"));
    let label = esc(&if s.truthy("label") { s.s("label") } else { s.s("key") });
    let help = format!("<small>{}Default: {}</small>", if s.truthy("help") { format!("{} · ", esc(&s.s("help"))) } else { String::new() }, esc(&engine_default_text(s)));
    match s.s("type").as_str() {
        "bool" => format!(r##"<div class="field" style="justify-content:center"><label class="switch"><input type="checkbox" data-ek="{k}" {} {dis}><i></i>{label}</label>{help}</div>"##,
            if truthy(v) { "checked" } else { "" }),
        "select" => {
            let opts = engine_options(s);
            let pick = if opts.iter().any(|(o, _)| js_string(o) == js_string(v)) { v.clone() } else { s.g("default").clone() };
            let options: String = opts.iter().map(|(val, lab)| format!(r##"<option value="{}" {}>{}</option>"##,
                escv(val), if js_string(val) == js_string(&pick) { "selected" } else { "" }, escv(lab))).collect();
            format!(r##"<label class="field"><span>{label}</span><select class="input" data-ek="{k}" {dis}>{options}</select>{help}</label>"##)
        }
        _ => {
            let attr = |n: &str| -> String { let x = s.g(n); if !x.is_null() && to_num(x).is_finite() { format!(r##" {n}="{}""##, escv(x)) } else { String::new() } };
            format!(r##"<label class="field"><span>{label}</span><input class="input" type="number" data-ek="{k}"{}{}{} value="{}" autocomplete="off" {dis}>{help}</label>"##,
                attr("min"), attr("max"), attr("step"), escv(v))
        }
    }
}

fn paint_engine_settings(card: &Element, engine: &Value, locked: bool) {
    let (Some(hint), Some(bx)) = (dom::qs_in(card, "#cadenza-engine-hint"), dom::qs_in(card, "#cadenza-engine-fields")) else { return };
    if !dom::data(&bx, "loaded").is_empty() { return; }
    let settings: Vec<Value> = if engine.is_object() { engine.a("settings") } else { vec![] }
        .into_iter().filter(|s| s.is_object() && s.truthy("key") && ENGINE_TYPES.contains(&s.s("type").as_str())).collect();
    let values = if engine.is_object() && engine.g("values").is_object() { engine.g("values").clone() } else { json!({}) };
    dom::set_data(&bx, "loaded", "1");
    let html: String = settings.iter().map(|s| engine_field(s, values.get(s.s("key").as_str()).unwrap_or(s.g("default")), locked)).collect();
    let empty = settings.is_empty();
    ENGINE.with(|e| *e.borrow_mut() = Some(EngineSettings { settings, values }));
    if empty { dom::set_text(&hint, "This engine has no settings of its own."); return; }
    dom::set_text(&hint, &format!("The settings of the engine in use. People’s own filter and dither choices under Settings → Playback still win for their streams.{}", if locked { " Set by an environment variable." } else { "" }));
    dom::set_html(&bx, &html);
    dom::hide(&bx, false);
}

/// After a save: the engine settings just saved are what the block compares with from now on.
pub fn remember_engine_values(saved: &str) {
    ENGINE.with(|en| if let Some(en) = en.borrow_mut().as_mut() { en.values = serde_json::from_str(saved).unwrap_or_else(|_| json!({})); });
}

/// What one engine setting's field holds, as JSON (None: a number field left empty or not a number).
fn engine_field_value(s: &Value, raw: &str, checked: bool) -> Option<Value> {
    match s.s("type").as_str() {
        "bool" => Some(Value::Bool(checked)),
        "number" => {
            let n = if raw.trim().is_empty() { f64::NAN } else { to_num(&Value::String(raw.to_string())) };
            if !n.is_finite() { return None; }
            Some(if n.fract() == 0.0 && n.abs() < 1e15 { Value::from(n as i64) } else { Value::from(n) })
        }
        _ => Some(engine_options(s).into_iter().find(|(o, _)| js_string(o) == raw).map(|(o, _)| o).unwrap_or_else(|| Value::String(raw.to_string()))),
    }
}

/// The engine settings after the form: `saved` with each field's value (`fields`: key → value, None for an empty
/// number), a value equal to the setting's default left out; keys this engine doesn't have stay as saved.
fn engine_values_after(settings: &[Value], saved: &Value, fields: &[(String, Option<Value>)]) -> Map<String, Value> {
    let mut out = saved.as_object().cloned().unwrap_or_default();
    for (key, v) in fields {
        let Some(s) = settings.iter().find(|x| &x.s("key") == key) else { continue };
        match v {
            None => { out.remove(key); }
            Some(v) if s.s("type") == "number" && to_num(v) == to_num(s.g("default")) => { out.remove(key); }
            Some(v) if s.s("type") != "number" && v == s.g("default") => { out.remove(key); }
            Some(v) => { out.insert(key.clone(), v.clone()); }
        }
    }
    out
}

/// The values the engine runs with: each of its settings' value (its default when not set; numbers as
/// numbers, so `3` and `3.0` and `"3"` are one value), and the keys it doesn't have as they are.
fn engine_effective(settings: &[Value], values: &Value) -> Map<String, Value> {
    let mut out = values.as_object().cloned().unwrap_or_default();
    for s in settings {
        let key = s.s("key");
        let v = values.get(key.as_str()).filter(|v| !v.is_null()).unwrap_or(s.g("default"));
        let v = match s.s("type").as_str() {
            "number" => { let n = to_num(v); if n.is_finite() { Value::from(n) } else { Value::from(to_num(s.g("default"))) } }
            "bool" => Value::Bool(truthy(v)),
            _ => Value::String(js_string(v)),
        };
        out.insert(key, v);
    }
    out
}

/// Whether the form changes what the engine runs with (`saved` → `after`).
fn engine_changed(settings: &[Value], saved: &Value, after: &Map<String, Value>) -> bool {
    engine_effective(settings, saved) != engine_effective(settings, &Value::Object(after.clone()))
}

/// The engine settings as the JSON text for cadenza_engine_settings (only values that differ from the default;
/// keys this engine doesn't have stay as saved), or None when the block isn't drawn or nothing in it changed:
/// Save and Test send cadenza_engine_settings only when the admin changed an engine setting.
pub fn engine_settings_value(root: &Element) -> Option<String> {
    let bx = dom::qs_in(root, "#cadenza-engine-fields")?;
    if dom::data(&bx, "loaded").is_empty() { return None; }
    ENGINE.with(|e| {
        let e = e.borrow();
        let en = e.as_ref()?;
        let fields: Vec<(String, Option<Value>)> = dom::qsa_in(&bx, "[data-ek]").iter().filter_map(|el| {
            let key = dom::data(el, "ek");
            let s = en.settings.iter().find(|x| x.s("key") == key)?;
            Some((key, engine_field_value(s, &dom::value(el), dom::checked(el))))
        }).collect();
        let out = engine_values_after(&en.settings, &en.values, &fields);
        engine_changed(&en.settings, &en.values, &out).then(|| Value::Object(out).to_string())
    })
}

pub fn bind_cadenza_cache(hub: &Rc<Hub>, card: &Element) {
    let (Some(state), Some(btn)) = (dom::qs_in(card, "#cadenza-cache-state"), dom::qs_in(card, "#cadenza-cache-clear")) else { return };
    let line = dom::qs_in(card, "#cadenza-engine");
    let locked = hub.values.borrow().o("_meta").o("cadenza_engine_settings").s("source") == "env";
    let refresh: Rc<dyn Fn()> = {
        let (state, card) = (state.clone(), card.clone());
        Rc::new(move || {
            let (state, card, line) = (state.clone(), card.clone(), line.clone());
            dom::spawn(async move {
                match api::get("/playback/dsp").await {
                    Ok(d) => {
                        dom::set_text(&state, &cadenza_cache_text(d.g("cache")));
                        if let Some(l) = &line { dom::set_text(l, &cadenza_engine_text(d.g("engine"))); }
                        paint_engine_settings(&card, d.g("engine"), locked);
                    }
                    Err(_) => dom::set_text(&state, "Status unavailable"),
                }
            });
        })
    };
    refresh();
    let btn2 = btn.clone();
    dom::on(&btn, "click", move |e| {
        dom::prevent(e);
        let (btn, refresh) = (btn2.clone(), refresh.clone());
        dom::set_disabled(&btn, true);
        dom::spawn(async move {
            match api::post_empty("/playback/dsp/cache/clear").await { Ok(_) => toast("Cache cleared", "ok"), Err(err) => toast(&err.message, "err") }
            dom::set_disabled(&btn, false);
            refresh();
        });
    });
}

// Settings → Soulseek: how Encore's Soulseek engine is doing (GET /soulseek/status), asked again every 5 s while
// the card is open. The listen port is hidden while the VPN is on (the VPN's forwarded port is used). Once the
// engine runs, the session's own line (`session`: the same object as GET /system/soulseek) says whether it is
// logged in, why not and when the next attempt is, with Reconnect now.
pub fn soulseek_status_block() -> String {
    format!(r##"<div class="card flat mt" id="slsk-engine" style="padding:12px 14px">
    <div class="row between" style="gap:10px"><b>Status</b><div class="row" style="gap:6px"><button class="btn sm" id="slsk-reconnect" type="button" hidden>{}<span id="slsk-reconnect-label">Reconnect now</span></button><button class="btn sm" id="slsk-start" hidden>Start now</button></div></div>
    <div class="small muted" id="slsk-engine-state">Checking…</div>
  </div>"##, icon("refresh"))
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

/// How long ago, in the desktop app's words (its settings/tools.rs `ago_at`): "just now", "5 min ago", "2 h ago",
/// "3 days ago".
fn ago_at(ts: f64, now: f64) -> String {
    let d = (now - ts).max(0.0);
    if d < 60.0 { "just now".into() }
    else if d < 3600.0 { format!("{} min ago", (d / 60.0).floor() as i64) }
    else if d < 86400.0 { format!("{} h ago", (d / 3600.0).floor() as i64) }
    else { format!("{} days ago", (d / 86400.0).floor() as i64) }
}

/// The Soulseek session's colour: ok (logged in), warn (rate limited, waiting for the next attempt), err (stopped
/// for a reason of the server's: rejected, banned, kicked again), "" (connecting, or off).
pub fn session_tone(s: &Value) -> &'static str {
    match s.s("state").as_str() {
        "logged_in" => if s.s("reason") == "rate_limited" { "warn" } else { "ok" },
        "waiting" | "disconnected" => "warn",
        "stopped" if s.truthy("reason") => "err",
        _ => "",
    }
}

/// The session's sentence: the server's `text`, so every app says the same ("Logged in as alice since 14:02",
/// "Disconnected – the login timed out (…). Next attempt at 14:20 (attempt 3).", "Rejected: invalid username or
/// password – fix them below").
pub fn session_text(s: &Value) -> String {
    if s.truthy("text") { return s.s("text"); }
    match s.s("state").as_str() {
        "logged_in" => format!("Logged in as {}", if s.truthy("username") { s.s("username") } else { "?".into() }),
        "connecting" => "Connecting to the Soulseek server…".into(),
        "stopped" => if s.truthy("message") { s.s("message") } else { "Soulseek is off".into() },
        _ => if s.truthy("message") { format!("Not connected – {}", s.s("message")) } else { "Not connected – connecting shortly".into() },
    }
}

/// The sentence away from the card (the Soulseek page, Diagnostics): "fix them below" points at the card's
/// fields, which aren't there – the line links to the card instead. (The server's Diagnostics line already
/// says "fix them in Settings → Downloads → Soulseek" – "Settings → Soulseek" before R2 – for the apps
/// without a link; the link says it here.)
pub fn session_text_elsewhere(text: &str) -> String {
    text.replace(" – fix them below", "").replace(" – fix them in Settings → Downloads → Soulseek", "").replace(" – fix them in Settings → Soulseek", "")
}

/// What goes under the sentence, in the browser's own clock: "Next attempt in 3 m 12 s · 12 searches waiting for
/// the connection · last search 5 min ago".
pub fn session_details(s: &Value, now: f64) -> String {
    let mut parts: Vec<String> = vec![];
    let state = s.s("state");
    let next = s.f("next_attempt_at");
    if matches!(state.as_str(), "waiting" | "disconnected") && next > now { parts.push(format!("next attempt in {}", span(next - now))); }
    let resume = s.f("next_search_at");
    if resume > now { parts.push(format!("searches go on in {}", span(resume - now))); }
    let n = s.i("searches_waiting");
    if n > 0 {
        let what = if n == 1 { "1 search".to_string() } else { format!("{n} searches") };
        let turn = if n == 1 { "its" } else { "their" };
        parts.push(if state == "logged_in" { format!("{what} waiting for {turn} turn") } else { format!("{what} waiting for the connection") });
    }
    if s.truthy("search_running") { parts.push("a search is running".into()); }
    if s.f("last_search_at") > 0.0 { parts.push(format!("last search {}", ago_at(s.f("last_search_at"), now))); }
    capitalized(&parts.join(" · "))
}

/// The line with its first letter in upper case ("Next attempt in 28 s · …").
fn capitalized(s: &str) -> String {
    let mut c = s.chars();
    match c.next() { Some(f) => f.to_uppercase().collect::<String>() + c.as_str(), None => String::new() }
}

/// The session as HTML: a dot and the sentence, the details under it in faint type.
pub fn session_html(s: &Value, now: f64) -> String {
    let tone = session_tone(s);
    let color = match tone { "ok" => "var(--text)", "warn" => "var(--warn)", "err" => "var(--err)", _ => "var(--muted)" };
    // connecting: a spinner where the dot goes
    let mark = if s.s("state") == "connecting" { r##"<span class="spinner" style="width:10px;height:10px;border-width:2px;vertical-align:middle;margin-right:6px"></span>"##.to_string() }
        else { format!(r##"<span class="dot {tone}" style="vertical-align:middle;margin-right:6px"></span>"##) };
    let details = session_details(s, now);
    format!(r##"<div class="slsk-session" style="margin-top:6px;color:{color}">{mark}{}</div>{}"##,
        esc(&session_text(s)),
        if details.is_empty() { String::new() } else { format!(r##"<div class="faint" style="margin-top:2px">{}</div>"##, esc(&details)) })
}

/// The Reconnect now button's words, as in the desktop app: "Reconnect now (in 12 s)" while it has to wait.
pub fn reconnect_label(wait: Option<i64>) -> String {
    match wait { Some(n) => format!("Reconnect now (in {n} s)"), None => "Reconnect now".into() }
}

/// Reconnect now: shown while the session is down and the engine runs; `Some(n)` when it has to wait n seconds
/// (attempts are at least 30 s apart), None when it can go now.
pub fn reconnect_wait(r: &Value) -> Option<Option<i64>> {
    let s = r.g("session");
    if r.s("state") != "running" || !s.truthy("can_reconnect") { return None; }
    let n = s.i("reconnect_in");
    Some(if n > 0 { Some(n) } else { None })
}

/// "Running v0.24.1 · signed in as alice" / "Not installed · Install or update it under Download tools" / "Waiting for the VPN" …
/// With a session (servers since 0.29): "Running v0.26.1" and the session's line under it.
pub fn soulseek_state_html(r: &Value, now: f64) -> String {
    if !r.is_object() { return "Status unavailable".into(); }
    let red = |t: &str| format!(r##"<span style="color:var(--err)">{}</span>"##, esc(t));
    let v = if r.truthy("version") { let ver = r.s("version"); format!(" v{}", esc(ver.strip_prefix('v').unwrap_or(&ver))) } else { String::new() };
    let error = r.s("error");
    let session = r.g("session");
    let with_session = r.s("state") == "running" && session.is_object() && session.truthy("state");
    let line = match r.s("state").as_str() {
        "not_installed" => "Not installed".to_string(),
        "starting" => format!("Starting…{}", if r.truthy("error") { format!(" · {}", red(&error)) } else { String::new() }),
        "running" if with_session => format!("Running{v}"),
        "running" => format!("Running{v}{}", if r.truthy("logged_in") { format!(" · signed in as {}", esc(&if r.truthy("username") { r.s("username") } else { "?".to_string() })) }
            else if r.truthy("error") { format!(" · {}", red(&error)) } else { " · signing in…".to_string() }),
        "crashed" => red(&format!("Crashed{}", if r.truthy("error") { format!(" – {error}") } else { String::new() })),
        "blocked" => red(&format!("Can't start{}", if r.truthy("error") { format!(" – {error}") } else { String::new() })),
        "waiting_vpn" => "Waiting for the VPN".to_string(),
        _ => if r.g("enabled") == &Value::Bool(false) { "Off – switch on Enable Soulseek and save".to_string() } else { "Off".to_string() },
    };
    let line = if r.truthy("hint") { format!("{line} · {}", esc(&r.s("hint"))) } else { line };
    let line = if with_session { line + &session_html(session, now) } else { line };
    // slskd runs, but downloads go to the fallback folder: say so, with the fix
    let line = if r.truthy("warning") { format!(r##"{line}<div style="color:var(--warn);margin-top:4px">{}</div>"##, esc(&r.s("warning"))) } else { line };
    // admins get the engine's last lines whenever something is wrong, so the cause can be read right here
    let logged_in = if with_session { session.s("state") == "logged_in" } else { r.truthy("logged_in") };
    let unhealthy = matches!(r.s("state").as_str(), "crashed" | "blocked") || (r.s("state") == "running" && !logged_in);
    let log: Vec<String> = r.g("log").as_array().map(|a| a.iter().filter_map(|l| l.as_str().map(String::from)).collect()).unwrap_or_default();
    if unhealthy && !log.is_empty() {
        format!(r##"{line}<details class="small" style="margin-top:6px"><summary class="muted">Engine log (last {} lines)</summary><pre class="mono" style="white-space:pre-wrap;max-height:240px;overflow:auto;margin:6px 0 0">{}</pre></details>"##,
            log.len(), esc(&log.join("\n")))
    } else {
        line
    }
}

/// The Start now button belongs to a crash or a blocked start: the states a person can act on.
pub fn can_start_now(state: &str) -> bool {
    matches!(state, "crashed" | "blocked")
}

thread_local! {
    /// Bumped on every paint of the Soulseek card, so the 5 s status poll of an earlier paint stops.
    static SLSK_POLL: Cell<u32> = const { Cell::new(0) };
}

pub fn bind_soulseek(hub: &Rc<Hub>, card: &Element) {
    let vpn = dom::qs_in(card, r#"[data-k="vpn_enabled"]"#);
    let field = dom::qs_in(card, r#"[data-k="soulseek_port"]"#).and_then(|p| dom::closest_of(&p, ".field"));
    if let (Some(vpn), Some(field)) = (vpn, field) {
        dom::hide(&field, dom::checked(&vpn));
        let vpn2 = vpn.clone();
        dom::on(&vpn, "change", move |_| dom::hide(&field, dom::checked(&vpn2)));
    }
    let Some(state) = dom::qs_in(card, "#slsk-engine-state") else { return };
    let start = dom::qs_in(card, "#slsk-start");
    let reconnect = dom::qs_in(card, "#slsk-reconnect");
    // the status as it came, and drawn: the engine line, the session, and the two buttons
    let paint: Rc<dyn Fn(&Value)> = {
        let (state, start, reconnect) = (state.clone(), start.clone(), reconnect.clone());
        Rc::new(move |r: &Value| {
            dom::set_html(&state, &soulseek_state_html(r, dom::now_s()));
            if let Some(b) = &start { dom::hide(b, !can_start_now(&r.s("state"))); }
            if let Some(b) = &reconnect {
                let wait = reconnect_wait(r);
                dom::hide(b, wait.is_none());
                if !b.has_attribute("data-busy") {
                    let n = wait.flatten();
                    if let Some(l) = dom::qs_in(b, "#slsk-reconnect-label") { dom::set_text(&l, &reconnect_label(n)); }
                    dom::set_disabled(b, n.is_some());
                    let _ = b.set_attribute("title", &match n { Some(n) => format!("Wait {n} s – attempts are at least 30 s apart"), None => "Connect to the Soulseek server now".into() });
                }
            }
        })
    };
    let refresh: Rc<dyn Fn()> = {
        let (state, paint) = (state.clone(), paint.clone());
        Rc::new(move || {
            let (state, paint) = (state.clone(), paint.clone());
            dom::spawn(async move {
                match api::get("/soulseek/status").await {
                    Ok(r) => paint(&r),
                    Err(_) => dom::set_text(&state, "Status unavailable"),
                }
            });
        })
    };
    refresh();
    if let Some(b) = &start {
        let (refresh2, b2) = (refresh.clone(), b.clone());
        dom::on(b, "click", move |_| {
            let (refresh, b) = (refresh2.clone(), b2.clone());
            dom::set_disabled(&b, true);
            dom::spawn(async move {
                match api::post_empty("/soulseek/start").await {
                    Ok(_) => toast("Starting Soulseek", "ok"),
                    Err(err) => toast(&err.message, "err"),
                }
                dom::set_disabled(&b, false);
                // the supervisor starts it on its next check: look again once it has had a moment
                dom::timeout(1500, move || refresh());
            });
        });
    }
    if let Some(b) = &reconnect {
        let (refresh2, b2) = (refresh.clone(), b.clone());
        dom::on(b, "click", move |e| {
            dom::prevent(e);
            let (refresh, b) = (refresh2.clone(), b2.clone());
            dom::set_disabled(&b, true);
            let _ = b.set_attribute("data-busy", "1");
            dom::spawn(async move {
                match api::post_empty("/system/soulseek/reconnect").await {
                    Ok(_) => toast("Connecting to Soulseek", "ok"),
                    // 429 "Wait 12 s": the last attempt was less than 30 s ago
                    Err(err) => toast(&err.message, "err"),
                }
                let _ = b.remove_attribute("data-busy");
                dom::set_disabled(&b, false);
                refresh();
                // the attempt takes a few seconds: look again once it has had them
                dom::timeout(3000, move || refresh());
            });
        });
    }
    let gen = SLSK_POLL.with(|g| { g.set(g.get().wrapping_add(1)); g.get() });
    let card = card.clone();
    dom::interval(&hub.view, 5000, move || {
        if SLSK_POLL.with(|g| g.get()) != gen || !dom::is_connected(&card) { return; }
        if card.has_attribute("open") { refresh(); }
    });
}

// Settings → Downloads → Download tools: yt-dlp, streamrip, Cadenza (server processing) and slskd
// (Soulseek engine) update in place – kept in the config folder, so no new Encore image is needed.
const TOOL_SPIN: &str = r##"<span class="spinner" style="width:10px;height:10px;border-width:2px"></span>"##;
fn tool_list(r: &Value) -> Vec<Value> { if r.is_array() { r.items() } else { r.a("tools") } }
fn tool_updatable(t: &Value) -> bool { t.truthy("updatable") }
/// `a || b`: the first truthy of the two, as text.
fn tool_or(t: &Value, a: &str, b: &str) -> String { if t.truthy(a) { t.s(a) } else if t.truthy(b) { t.s(b) } else { String::new() } }

/// The reset link of a row (link, confirm question, toast): Cadenza goes back to the engine built into Encore,
/// slskd is removed, the rest go back to the bundled copy.
fn tool_reset(t: &Value) -> (String, String, String) {
    match t.s("id").as_str() {
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
fn tool_install(t: &Value) -> bool { (t.s("id") == "slskd" || t.s("id") == "translator") && !t.truthy("installed") }

fn tool_row(t: &Value, busy: Option<&str>) -> String {
    let id = t.s("id");
    let latest = if t.truthy("latest") { t.s("latest") } else { String::new() };
    let action = if busy == Some(id.as_str()) {
        format!(r##"<button class="btn sm" type="button" disabled>{TOOL_SPIN} {}</button>"##, if tool_install(t) { "Installing…" } else { "Updating…" })
    } else if tool_install(t) && tool_updatable(t) {
        format!(r##"<button class="btn sm primary" type="button" data-tool-update="{}" {}>{}Install{}</button>"##,
            esc(&id), if busy.is_some() { "disabled" } else { "" }, icon("download"), if latest.is_empty() { String::new() } else { format!(" {}", esc(&latest)) })
    } else if t.truthy("update_available") && tool_updatable(t) {
        format!(r##"<button class="btn sm primary" type="button" data-tool-update="{}" {}>{}Update to {}</button>"##,
            esc(&id), if busy.is_some() { "disabled" } else { "" }, icon("download"), esc(&latest))
    } else if t.truthy("update_available") {
        format!(r##"<span class="badge warn">{} available</span>"##, esc(&latest))
    } else if t.truthy("latest") {
        format!(r##"<span class="badge ok">{} Up to date</span>"##, icon("check"))
    } else if tool_updatable(t) {
        r##"<span class="small faint">Not checked yet</span>"##.to_string()
    } else { String::new() };
    let updated = t.s("source") == "updated" && id != "slskd" && id != "translator";
    format!(r##"<div class="list-item" style="align-items:flex-start;flex-wrap:wrap;gap:12px" data-tool="{eid}">
      <div class="grow" style="min-width:200px"><div style="font-weight:650">{name} <span class="small muted mono">{ver}</span>{upd}</div>
        {note}
        {err}
        {reset}</div>
      <div class="row" style="gap:8px;align-items:center">{action}</div></div>"##,
        eid = esc(&id),
        name = esc(&tool_or(t, "name", "id")),
        ver = if t.truthy("installed") { esc(&t.s("installed")) } else { "not installed".to_string() },
        upd = if updated { r##" <span class="small muted">(updated)</span>"## } else { "" },
        note = if t.truthy("note") { format!(r##"<div class="small muted">{}</div>"##, esc(&t.s("note"))) } else { String::new() },
        err = if t.truthy("error") { format!(r##"<div class="small" style="color:var(--err)">{}</div>"##, esc(&t.s("error"))) } else { String::new() },
        reset = if t.truthy("resettable") && tool_updatable(t) {
            format!(r##"<a href="#" class="small muted" data-tool-reset="{}" {}>{}</a>"##, esc(&id), if busy.is_some() { "hidden" } else { "" }, esc(&tool_reset(t).0))
        } else { String::new() })
}

/// What the Download tools card shows: the tool list, the tool being updated, and whether a check runs.
struct Tools {
    list: Element,
    check: Element,
    tools: RefCell<Vec<Value>>,
    busy: RefCell<Option<String>>,
    checking: Cell<bool>,
}

impl Tools {
    fn paint(&self) {
        let busy = self.busy.borrow().clone();
        let rows: String = self.tools.borrow().iter().map(|t| tool_row(t, busy.as_deref())).collect();
        dom::set_html(&self.list, &rows);
        dom::set_disabled(&self.check, self.checking.get() || busy.is_some());
        dom::set_html(&self.check, &if self.checking.get() { format!("{TOOL_SPIN} Checking…") } else { format!("{}Check for updates", icon("refresh")) });
    }
    async fn reload(&self) { if let Ok(r) = api::get("/system/tools").await { *self.tools.borrow_mut() = tool_list(&r); } }
    fn name_of(&self, id: &str) -> String {
        self.tools.borrow().iter().find(|t| t.s("id") == id).map(|t| t.s("name")).filter(|n| !n.is_empty()).unwrap_or_else(|| id.to_string())
    }
    fn tool(&self, id: &str) -> Value {
        self.tools.borrow().iter().find(|t| t.s("id") == id).cloned().unwrap_or_else(|| json!({ "id": id }))
    }
}

pub async fn render_tools(host: Element, hub: Rc<Hub>) {
    let tools = match api::get("/system/tools").await { Ok(r) => tool_list(&r), Err(_) => { dom::set_html(&host, ""); return; } };
    let values = hub.values.borrow().clone();
    let meta = values.o("_meta");
    let toggle = |key: &str, label: &str| -> String {
        if !values.has(key) { return String::new(); }
        let locked = meta.o(key).s("source") == "env";
        format!(r##"<label class="switch"><input type="checkbox" data-tool-auto="{key}" {} {}><i></i>{}</label>"##,
            if values.truthy(key) { "checked" } else { "" }, if locked { "disabled" } else { "" }, esc(label))
    };
    let toggles = toggle("tools_auto_ytdlp", "Update yt-dlp automatically (recommended – sites change often)")
        + &toggle("tools_auto_streamrip", "Update streamrip automatically when a new release is out")
        + &toggle("tools_auto_cadenza", "Update Cadenza automatically when a new release is out")
        + &toggle("tools_auto_slskd", "Update slskd automatically when a new release is out")
        + &toggle("tools_auto_translator", "Update the Translator (llama.cpp) automatically")
        + &toggle("translate_background", "Translate the whole library in the background (titles on screen are always translated first)");
    // the Translator's model: changing it downloads the other one on the next Install / Update
    let model_pick = if values.has("translator_model") {
        let opts: String = [("small", "Small – Qwen3 4B Instruct (about 2.5 GB)"), ("best", "Best – Qwen3 8B (about 5 GB)")].iter()
            .map(|(v, l)| format!(r##"<option value="{v}" {}>{l}</option>"##, if values.s("translator_model") == *v { "selected" } else { "" })).collect();
        format!(r##"<label class="stack" style="gap:4px;max-width:420px"><span class="small muted">Translator model</span>
    <select class="input" id="tools-model" {}>{opts}</select>
    <span class="small muted">Small runs on most servers (about 4 GB of memory while it works); Best translates better and needs about 7 GB. It only runs while there is something to translate.</span></label>"##,
            if meta.o("translator_model").s("source") == "env" { "disabled" } else { "" })
    } else { String::new() };
    dom::set_html(&host, &format!(r##"<div class="card" id="tools-card" style="margin-bottom:12px">
    <div class="row between" style="margin-bottom:8px;gap:10px"><h2>Download tools</h2><button class="btn sm" type="button" id="tools-check">{refresh}Check for updates</button></div>
    <p class="muted small" style="margin:0 0 14px">yt-dlp, streamrip, Cadenza, slskd and the Translator update in place – no new Encore image needed. Updates are kept in the config folder.</p>
    <div class="list" id="tools-list"></div>
    <div class="stack mt" style="gap:8px">{model_pick}{toggles}<div class="small muted">Every tool is checked daily; updates install by themselves only where the switch is on. Change the schedule under <a href="#/settings?s=tasks">Scheduled tasks</a>.</div></div>
  </div>"##, refresh = icon("refresh")));
    let (Some(list), Some(check)) = (dom::qs_in(&host, "#tools-list"), dom::qs_in(&host, "#tools-check")) else { return };
    let st = Rc::new(Tools { list: list.clone(), check: check.clone(), tools: RefCell::new(tools), busy: RefCell::new(None), checking: Cell::new(false) });
    st.paint();
    {
        let st = st.clone();
        dom::on(&check, "click", move |_| {
            let st = st.clone();
            st.checking.set(true); st.paint();
            dom::spawn(async move {
                match api::post_empty("/system/tools/check").await {
                    Ok(r) => *st.tools.borrow_mut() = tool_list(&r),
                    Err(e) => toast(&e.message, "err"),
                }
                st.checking.set(false); st.paint();
            });
        });
    }
    {
        let st = st.clone();
        dom::on(&list, "click", move |e| {
            let idle = st.busy.borrow().is_none();
            if let Some(up) = dom::closest(e, "[data-tool-update]") {
                if !idle { return; }
                let id = dom::data(&up, "toolUpdate");
                let (name, install) = (st.name_of(&id), tool_install(&st.tool(&id)));
                *st.busy.borrow_mut() = Some(id.clone()); st.paint();
                let st = st.clone();
                dom::spawn(async move {
                    match api::post_empty(&format!("/system/tools/{id}/update")).await {
                        Ok(r) => toast(&format!("{name} {} {}", if install { "installed" } else { "updated to" }, tool_or(&r, "installed", "latest")), "ok"),
                        Err(err) => toast(&err.message, "err"),
                    }
                    *st.busy.borrow_mut() = None;
                    st.reload().await; st.paint();
                });
            } else if let Some(reset) = dom::closest(e, "[data-tool-reset]") {
                if !idle { return; }
                dom::prevent(e);
                let id = dom::data(&reset, "toolReset");
                let (_, ask, done) = tool_reset(&st.tool(&id));
                if !dom::confirm(&ask) { return; }
                let st = st.clone();
                dom::spawn(async move {
                    match api::post_empty(&format!("/system/tools/{id}/reset")).await {
                        Ok(_) => toast(&done, "ok"),
                        Err(err) => toast(&err.message, "err"),
                    }
                    st.reload().await; st.paint();
                });
            }
        });
    }
    if let Some(sel) = dom::qs_in(&host, "#tools-model") {
        let (sel2, hub, st) = (sel.clone(), hub.clone(), st.clone());
        dom::on(&sel, "change", move |_| {
            let (v, hub, st) = (dom::value(&sel2), hub.clone(), st.clone());
            dom::spawn(async move {
                match api::put("/admin/settings", json!({ "translator_model": v })).await {
                    Ok(v) => { *hub.values.borrow_mut() = v; toast("Saved – Install or Update the Translator to download this model", "ok"); st.reload().await; st.paint(); }
                    Err(err) => toast(&err.message, "err"),
                }
            });
        });
    }
    for b in dom::qsa_in(&host, "[data-tool-auto]") {
        let (b2, hub) = (b.clone(), hub.clone());
        dom::on(&b, "change", move |_| {
            let (b, hub) = (b2.clone(), hub.clone());
            let (key, on) = (dom::data(&b, "toolAuto"), dom::checked(&b));
            dom::spawn(async move {
                let mut body = Map::new();
                body.insert(key, Value::Bool(on));
                match api::put("/admin/settings", Value::Object(body)).await {
                    Ok(v) => { *hub.values.borrow_mut() = v; toast("Saved", "ok"); }
                    Err(err) => { dom::set_checked(&b, !dom::checked(&b)); toast(&err.message, "err"); }
                }
            });
        });
    }
}

// Tidal downloads need your Tidal account, not an API key: Encore signs in once through a link
// (device code, one step per streamrip "lane"). This block shows the link and waits for the approval;
// when Tidal blocks the server's address the tokens can be pasted from a computer that has access.
pub fn tidal_block() -> String {
    format!(r##"<div class="card flat mt" id="tidal-login" style="padding:12px 14px">
    <div class="row between wrap" style="gap:8px">
      <div style="min-width:0"><b>Tidal account</b><div class="small muted" id="tidal-state">Checking…</div></div>
      <div class="row" style="gap:10px;align-items:center"><a href="#" class="small muted" id="tidal-signout" hidden>Sign out of Tidal</a><button class="btn sm" id="tidal-signin" type="button">{link}Sign in to Tidal</button></div>
    </div>
    <div class="small mt" id="tidal-out" hidden></div>
    <details class="mt" id="tidal-paste">
      <summary class="small" style="cursor:pointer">Paste tokens instead</summary>
      <div class="small muted mt">On a computer with Tidal access: install streamrip (<code>pip install streamrip</code>), run <code>rip url https://tidal.com/browse/track/1</code>, approve the link it prints, then paste the [tidal] section of its config file here (<code>rip config path</code> shows where it is).</div>
      <textarea id="tidal-toml" class="mt" rows="6" spellcheck="false" style="width:100%;font-family:monospace" placeholder="[tidal]&#10;user_id = &quot;…&quot;&#10;access_token = &quot;…&quot;"></textarea>
      <div class="row wrap mt" style="gap:8px"><button class="btn sm" id="tidal-save-tokens" type="button">{check}Save tokens</button></div>
    </details>
  </div>"##, link = icon("link"), check = icon("check"))
}

const TIDAL_SPINNER: &str = r##"<span class="spinner" style="width:10px;height:10px;border-width:2px"></span>"##;

pub fn bind_tidal(view: &Element, card: &Element) {
    let (Some(state), Some(out), Some(btn)) = (dom::qs_in(card, "#tidal-state"), dom::qs_in(card, "#tidal-out"), dom::qs_in(card, "#tidal-signin")) else { return };
    let (Some(paste), Some(toml), Some(save), Some(signout)) = (dom::qs_in(card, "#tidal-paste"), dom::qs_in(card, "#tidal-toml"), dom::qs_in(card, "#tidal-save-tokens"), dom::qs_in(card, "#tidal-signout")) else { return };
    let paint: Rc<dyn Fn(&Value)> = {
        let (state, out, btn, paste, signout) = (state.clone(), out.clone(), btn.clone(), paste.clone(), signout.clone());
        Rc::new(move |r: &Value| {
            // partial (default lane only) and signed_in never come together: the server reports one or the other
            let partial = r.truthy("partial");
            let signed = r.truthy("signed_in") || partial;
            dom::set_text(&state, if partial { "Signed in for 16/44.1 FLAC only – sign in again for hi-res (step 2 was not finished)." }
                else if r.truthy("signed_in") { "Signed in – Tidal downloads use this account." }
                else { "Not signed in – Tidal needs a one-time sign-in with your own Tidal account (no API key)." });
            dom::set_html(&btn, &format!("{}{}", icon("link"), if signed { "Sign in again" } else { "Sign in to Tidal" }));
            dom::hide(&signout, !signed);
            let status = r.s("status");
            if status == "waiting" && !r.s("url").is_empty() {
                let steps = r.num("steps").filter(|n| *n >= 1.0).unwrap_or(1.0) as i64;
                let step = r.num("step").filter(|n| *n >= 1.0).unwrap_or(1.0) as i64;
                let prefix = if steps > 1 { format!("Step {step} of {steps} · ") } else { String::new() };
                dom::hide(&out, false);
                dom::set_html(&out, &format!(r##"{prefix}Open <a href="{u}" target="_blank" rel="noopener" style="color:var(--accent)">{u}</a> on any device, approve in your Tidal account, then come back here. {TIDAL_SPINNER} Waiting…"##, u = esc(&r.s("url"))));
            } else if status == "done" {
                dom::hide(&out, false);
                dom::set_html(&out, &format!(r##"<span class="badge ok">{} {}</span>"##, icon("check"), esc(&r.s("message"))));
            } else if status == "failed" {
                dom::hide(&out, false);
                let m = r.s("message");
                dom::set_html(&out, &super::test_result(&json!({ "ok": false, "message": if m.is_empty() { "Sign-in failed".to_string() } else { m.clone() } })));
                if m.contains("paste the tokens") { let _ = paste.set_attribute("open", ""); }
            }
        })
    };
    {
        let (paint, state) = (paint.clone(), state.clone());
        dom::spawn(async move {
            match api::get("/system/streamrip/tidal-login").await {
                Ok(r) => paint(&r),
                Err(_) => dom::set_text(&state, "Status unavailable"),
            }
        });
    }
    // Each step has its own link: open the new one when the server moves to step 2.
    let opened: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));
    let open_link: Rc<dyn Fn(&Value)> = {
        let opened = opened.clone();
        Rc::new(move |r: &Value| {
            let url = r.s("url");
            if r.s("status") == "waiting" && !url.is_empty() && *opened.borrow() != url {
                *opened.borrow_mut() = url.clone();
                let _ = dom::window().open_with_url_and_target_and_features(&url, "_blank", "noopener");
            }
        })
    };
    // one poll at a time: a new click stops the previous poller
    let polling: Rc<Cell<u32>> = Rc::new(Cell::new(0));
    {
        let (view, card, out2, btn2, paint, open_link, polling) = (view.clone(), card.clone(), out.clone(), btn.clone(), paint.clone(), open_link.clone(), polling.clone());
        dom::on(&btn, "click", move |_| {
            dom::set_disabled(&btn2, true);
            dom::hide(&out2, false);
            dom::set_html(&out2, &format!("{TIDAL_SPINNER} Starting sign-in…"));
            let (view, card, out, btn, paint, open_link, polling) = (view.clone(), card.clone(), out2.clone(), btn2.clone(), paint.clone(), open_link.clone(), polling.clone());
            dom::spawn(async move {
                match api::post_empty("/system/streamrip/tidal-login").await {
                    Ok(r) => {
                        paint(&r);
                        if r.s("status") == "waiting" {
                            open_link(&r);
                            // poll every 3 s until the sign-in finishes or the card is gone
                            let gen = polling.get() + 1;
                            polling.set(gen);
                            let (card, btn, paint, open_link, polling) = (card.clone(), btn.clone(), paint.clone(), open_link.clone(), polling.clone());
                            dom::interval(&view, 3000, move || {
                                if polling.get() != gen || !dom::is_connected(&card) { return; }
                                let (btn, paint, open_link, polling) = (btn.clone(), paint.clone(), open_link.clone(), polling.clone());
                                dom::spawn(async move {
                                    if polling.get() != gen { return; }
                                    if let Ok(r) = api::get("/system/streamrip/tidal-login").await {
                                        if polling.get() != gen { return; }
                                        paint(&r);
                                        open_link(&r);
                                        if r.s("status") != "waiting" { polling.set(0); dom::set_disabled(&btn, false); }
                                    }
                                });
                            });
                        } else { dom::set_disabled(&btn, false); }
                    }
                    Err(e) => { dom::set_html(&out, &super::test_result(&json!({ "ok": false, "message": e.message }))); dom::set_disabled(&btn, false); }
                }
            });
        });
    }
    {
        let (save2, toml, paint) = (save.clone(), toml.clone(), paint.clone());
        dom::on(&save, "click", move |_| {
            dom::set_disabled(&save2, true);
            let (save, toml, paint) = (save2.clone(), toml.clone(), paint.clone());
            dom::spawn(async move {
                match api::post("/system/streamrip/tidal-tokens", json!({ "toml": dom::value(&toml) })).await {
                    Ok(r) => { paint(&r); dom::set_value(&toml, ""); toast("Tidal tokens saved", "ok"); }
                    Err(e) => toast(&e.message, "err"),
                }
                dom::set_disabled(&save, false);
            });
        });
    }
    {
        let (out, paint) = (out.clone(), paint.clone());
        dom::on(&signout, "click", move |e| {
            e.prevent_default();
            let (out, paint) = (out.clone(), paint.clone());
            dom::spawn(async move {
                match api::del("/system/streamrip/tidal-tokens").await {
                    Ok(r) => { paint(&r); dom::hide(&out, true); toast("Signed out of Tidal", "ok"); }
                    Err(err) => toast(&err.message, "err"),
                }
            });
        });
    }
}

// streamrip's config file: when it does not load (a half-written file, the format of an older streamrip),
// a fresh one from streamrip's defaults mends it; the server keeps the old file and carries the sign-ins over.
pub fn streamrip_config_block() -> String {
    format!(r##"<div class="card flat mt" id="rip-config" style="padding:12px 14px">
    <div class="row between wrap" style="gap:8px">
      <div style="min-width:0"><b>streamrip config file</b><div class="small muted">When requests fail with "Error loading config", write a fresh config.toml from streamrip's defaults. The old file is kept as a backup; the Tidal sign-in, Deezer ARL and Qobuz login are carried over.</div></div>
      <button class="btn sm" id="rip-reset" type="button">{refresh}Reset streamrip config</button>
    </div>
    <div class="small mt" id="rip-out" hidden></div>
  </div>"##, refresh = icon("refresh"))
}

pub fn bind_streamrip_config(card: &Element) {
    let (Some(btn), Some(out)) = (dom::qs_in(card, "#rip-reset"), dom::qs_in(card, "#rip-out")) else { return };
    let btn2 = btn.clone();
    dom::on(&btn, "click", move |_| {
        let (btn, out) = (btn2.clone(), out.clone());
        dom::spawn(async move {
            if !crate::ui::confirm_dialog("Reset streamrip config", "Replace streamrip's config.toml with a fresh one from its defaults? The current file is kept as a backup, and the Tidal sign-in, Deezer ARL and Qobuz login are carried over.", "Reset", false).await { return; }
            dom::set_disabled(&btn, true);
            dom::hide(&out, false);
            match api::post_empty("/system/streamrip/reset-config").await {
                Ok(r) => {
                    dom::set_html(&out, &test_result(&r));
                    let ok = r.truthy("ok");
                    toast(if ok { "streamrip config reset" } else { "streamrip config still doesn't load" }, if ok { "ok" } else { "err" });
                }
                Err(e) => dom::set_html(&out, &super::test_result(&json!({ "ok": false, "message": e.message }))),
            }
            dom::set_disabled(&btn, false);
        });
    });
}

pub fn pipeline_help() -> String {
    r##"<div class="card mt"><h3>How requests are fulfilled</h3>
    <ol class="muted" style="line-height:1.9;padding-left:18px;margin:10px 0 0">
      <li><b>Library check</b> – if the album/track is already in your library (by MusicBrainz id or name) it's marked available instantly.</li>
      <li><b>streamrip</b> – lossless from Qobuz, Tidal or Deezer with your own accounts, matched by ISRC.</li>
      <li><b>Torrents & Usenet</b> – Prowlarr searches your indexers; the best album release goes to your torrent or Usenet client and is imported when it finishes. If it stalls past the timeout, the next release or source is tried.</li>
      <li><b>Soulseek</b> – Encore searches the network, results are ranked by track count, format, bitrate, folder name and the uploader's free slots/speed. Stalled or failed users are skipped automatically.</li>
      <li><b>yt-dlp</b> – each track from the MusicBrainz tracklist is matched on YouTube (preferring official "Topic" uploads with matching length).</li>
      <li><b>Import</b> – files are matched to the MusicBrainz release, tagged (incl. MBIDs, ISRCs, cover art), renamed with your template, and your media servers are refreshed.</li>
    </ol></div>"##.to_string()
}

#[cfg(test)]
mod soulseek_tests {
    use super::*;
    use serde_json::json;

    /// GET /soulseek/status of a 0.29 server: the engine runs, the session is down and waits for its next attempt.
    fn waiting(now: f64) -> Value {
        json!({ "enabled": true, "state": "running", "version": "0.26.1", "logged_in": false, "error": "", "log": ["Failed to reconnect: The wait timed out after 5000 milliseconds"],
            "session": { "state": "waiting", "reason": "timeout", "message": "The wait timed out after 5000 milliseconds", "since": now - 20.0,
                "next_attempt_at": now + 192.0, "attempts": 2, "last_search_at": now - 300.0, "searches_waiting": 12, "search_running": false,
                "username": null, "next_search_at": null, "reconnect_in": 0, "can_reconnect": true,
                "text": "Disconnected – the login timed out (the Soulseek server did not answer in 5 s). Next attempt at 14:20 (attempt 3)." } })
    }

    #[test]
    fn waits_read_like_the_server_log() {
        assert_eq!(span(45.0), "45 s");
        assert_eq!(span(120.0), "2 m");
        assert_eq!(span(192.4), "3 m 12 s");
        assert_eq!(span(3600.0 + 300.0), "1 h 5 m");
        assert_eq!(span(-3.0), "0 s");
    }

    #[test]
    fn the_card_shows_the_session_under_the_engine() {
        let now = 1_760_000_000.0;
        let r = waiting(now);
        let html = soulseek_state_html(&r, now);
        assert!(html.starts_with("Running v0.26.1<div class=\"slsk-session\""), "{html}");
        assert!(html.contains("Disconnected – the login timed out (the Soulseek server did not answer in 5 s). Next attempt at 14:20 (attempt 3)."), "{html}");
        assert!(html.contains("dot warn"), "{html}");
        // the same words as the desktop app's card
        assert!(html.contains("Next attempt in 3 m 12 s · 12 searches waiting for the connection · last search 5 min ago"), "{html}");
        // down: the engine log is there to read
        assert!(html.contains("Engine log (last 1 lines)"), "{html}");
        assert_eq!(reconnect_wait(&r), Some(None));
        let mut r2 = r.clone();
        r2["session"]["reconnect_in"] = json!(12);
        assert_eq!(reconnect_wait(&r2), Some(Some(12)));
        assert_eq!(reconnect_label(Some(12)), "Reconnect now (in 12 s)");
        assert_eq!(reconnect_label(None), "Reconnect now");
        assert_eq!((ago_at(now - 7300.0, now), ago_at(now - 3.0 * 86400.0, now)), ("2 h ago".to_string(), "3 days ago".to_string()));
    }

    #[test]
    fn logged_in_rejected_and_rate_limited() {
        let now = 1_760_000_000.0;
        let mut r = waiting(now);
        r["session"] = json!({ "state": "logged_in", "reason": null, "username": "alice", "since": now - 600.0, "searches_waiting": 0, "search_running": true,
            "can_reconnect": false, "reconnect_in": 0, "last_search_at": now - 2.0, "text": "Logged in as alice since 14:02" });
        let html = soulseek_state_html(&r, now);
        assert!(html.contains("dot ok") && html.contains("Logged in as alice since 14:02"), "{html}");
        assert!(html.contains("A search is running · last search just now"), "{html}");
        assert!(!html.contains("Engine log"), "logged in: no log – {html}");
        assert_eq!(reconnect_wait(&r), None);
        // searches turned away for a minute
        r["session"]["reason"] = json!("rate_limited");
        r["session"]["next_search_at"] = json!(now + 45.0);
        assert_eq!(session_tone(&r["session"]), "warn");
        assert!(session_details(&r["session"], now).starts_with("Searches go on in 45 s"));
        // the server refused the login: red, and Reconnect now stays (after fixing the password)
        r["session"] = json!({ "state": "stopped", "reason": "rejected", "can_reconnect": true, "reconnect_in": 0,
            "text": "Rejected: invalid username or password – fix them below" });
        assert_eq!(session_tone(&r["session"]), "err");
        assert!(soulseek_state_html(&r, now).contains("Rejected: invalid username or password – fix them below"));
        assert_eq!(session_text_elsewhere(&session_text(&r["session"])), "Rejected: invalid username or password");
        // the server's Diagnostics line names the card itself (for the apps without a link); here the link does
        assert_eq!(session_text_elsewhere("Rejected: invalid username or password – fix them in Settings → Downloads → Soulseek"), "Rejected: invalid username or password");
        assert_eq!(session_text_elsewhere("Rejected: invalid username or password – fix them in Settings → Soulseek"), "Rejected: invalid username or password", "an older server");
    }

    #[test]
    fn without_a_running_engine_or_a_session_the_card_reads_as_before() {
        let now = 1_760_000_000.0;
        // not installed: the engine line only (the session would only repeat it)
        let r = json!({ "enabled": true, "configured": true, "state": "not_installed", "hint": "Install or update it under Download tools",
            "session": { "state": "stopped", "message": "Soulseek is not installed – install it under Settings → Downloads → Download tools (slskd – Soulseek engine)", "can_reconnect": false,
                "text": "Soulseek is not installed – install it under Settings → Downloads → Download tools (slskd – Soulseek engine)" } });
        assert_eq!(soulseek_state_html(&r, now), "Not installed · Install or update it under Download tools");
        assert_eq!(reconnect_wait(&r), None);
        // a server before 0.29: no session
        let old = json!({ "state": "running", "version": "v0.24.1", "logged_in": true, "username": "alice" });
        assert_eq!(soulseek_state_html(&old, now), "Running v0.24.1 · signed in as alice");
        let old = json!({ "state": "running", "version": "v0.24.1", "logged_in": false, "error": "" });
        assert_eq!(soulseek_state_html(&old, now), "Running v0.24.1 · signing in…");
        // a session without `text` still says something
        assert_eq!(session_text(&json!({ "state": "connecting" })), "Connecting to the Soulseek server…");
        assert_eq!(session_text(&json!({ "state": "waiting" })), "Not connected – connecting shortly");
    }
}

#[cfg(test)]
mod session_words_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn searches_waiting_for_their_turn_read_right_for_one_and_many() {
        let now = 1_760_000_000.0;
        let s = |n: i64| json!({ "state": "logged_in", "searches_waiting": n });
        assert_eq!(session_details(&s(1), now), "1 search waiting for its turn");
        assert_eq!(session_details(&s(3), now), "3 searches waiting for their turn");
        let down = json!({ "state": "waiting", "searches_waiting": 1 });
        assert_eq!(session_details(&down, now), "1 search waiting for the connection");
    }
}

#[cfg(test)]
mod engine_settings_tests {
    use super::*;
    use serde_json::json;

    fn settings() -> Vec<Value> {
        vec![
            json!({ "key": "filter", "type": "select", "default": "poly-sinc-long", "options": [["poly-sinc-long", "Long"], ["poly-sinc-short", "Short"]] }),
            json!({ "key": "headroom_db", "type": "number", "default": -3, "min": -12, "max": 0, "step": 0.5 }),
            json!({ "key": "dither", "type": "bool", "default": true }),
        ]
    }

    fn f(k: &str, v: Option<Value>) -> (String, Option<Value>) { (k.to_string(), v) }

    #[test]
    fn an_untouched_form_changes_nothing() {
        let s = settings();
        // nothing saved, the form shows the defaults
        let saved = json!({});
        let form = [f("filter", Some(json!("poly-sinc-long"))), f("headroom_db", Some(json!(-3))), f("dither", Some(json!(true)))];
        let after = engine_values_after(&s, &saved, &form);
        assert!(after.is_empty(), "{after:?}");
        assert!(!engine_changed(&s, &saved, &after));
        // saved values, shown as they are; a saved default and a number saved as text are the same values
        let saved = json!({ "filter": "poly-sinc-short", "headroom_db": "-6", "dither": true, "other_engine_key": 1 });
        let form = [f("filter", Some(json!("poly-sinc-short"))), f("headroom_db", Some(json!(-6))), f("dither", Some(json!(true)))];
        let after = engine_values_after(&s, &saved, &form);
        assert!(!engine_changed(&s, &saved, &after), "{after:?}");
        assert_eq!(after.get("other_engine_key"), Some(&json!(1)), "keys this engine doesn't have stay");
    }

    #[test]
    fn a_changed_field_is_sent() {
        let s = settings();
        let saved = json!({ "headroom_db": -6 });
        let form = [f("filter", Some(json!("poly-sinc-long"))), f("headroom_db", Some(json!(-4.5))), f("dither", Some(json!(true)))];
        let after = engine_values_after(&s, &saved, &form);
        assert!(engine_changed(&s, &saved, &after));
        assert_eq!(Value::Object(after), json!({ "headroom_db": -4.5 }));
        // back to the default: changed, and left out
        let form = [f("headroom_db", Some(json!(-3)))];
        let after = engine_values_after(&s, &saved, &form);
        assert!(engine_changed(&s, &saved, &after));
        assert!(after.is_empty());
        // a switch turned off
        let after = engine_values_after(&s, &json!({}), &[f("dither", Some(json!(false)))]);
        assert!(engine_changed(&s, &json!({}), &after));
        // an emptied number field falls back to the default: a change only when something else was saved
        let after = engine_values_after(&s, &json!({}), &[f("headroom_db", None)]);
        assert!(!engine_changed(&s, &json!({}), &after));
        let after = engine_values_after(&s, &json!({ "headroom_db": -9 }), &[f("headroom_db", None)]);
        assert!(engine_changed(&s, &json!({ "headroom_db": -9 }), &after));
    }

    #[test]
    fn field_values_read_like_the_form() {
        let s = settings();
        assert_eq!(engine_field_value(&s[1], "-4.5", false), Some(json!(-4.5)));
        assert_eq!(engine_field_value(&s[1], "-4", false), Some(json!(-4)));
        assert_eq!(engine_field_value(&s[1], " ", false), None);
        assert_eq!(engine_field_value(&s[1], "abc", false), None);
        assert_eq!(engine_field_value(&s[2], "", true), Some(json!(true)));
        assert_eq!(engine_field_value(&s[0], "poly-sinc-short", false), Some(json!("poly-sinc-short")));
    }
}
