//! The Server group of Settings: General, Library, Downloads, Integrations, Server processing,
//! Email & notifications and Scheduled tasks are the admin settings form of web-rs/src/pages/settings.rs
//! (`show`), drawn from the field table in `fields.rs`: one card per section with its fields, secrets
//! with Show saved / Copy, Save and Test, the live blocks of `tools.rs` (the Download tools card on
//! Downloads), the API key card and About. Auto-download, Contribute and Blocked are pages of their own
//! and are routed to their modules.

use super::fields::{self, Card, Field};
use super::{autodownload, blocked, contribute, tasks, tools, Section};
use super::{env_locked, help, input, input_locked, is_masked, link_muted, result_line_copy, select_table, switch_maybe, vs};
use crate::api::ApiError;
use crate::fmt;
use crate::pages::{Ctx, El};
use crate::theme::{self, Btn};
use crate::widgets::text_area::text_area;
use crate::widgets::{self, btn, btn_icon, btn_sm, icon, icon_btn, modal, t, Icon};
use iced::widget::text_editor;
use iced::widget::{button, checkbox, column, container, row};
use iced::{Alignment, Length, Subscription, Task};
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};
use std::time::Duration;

#[derive(Clone, Copy, Debug)]
pub enum Dialog {
    RegenKey,
}

#[derive(Debug, Default)]
pub struct State {
    section: Option<Section>,
    /// `/api/admin/settings` (with `_meta`).
    values: Option<Value>,
    error: String,
    /// Text typed into inputs, by key (absent = the saved value shows).
    edits: HashMap<String, String>,
    /// Secrets shown in clear.
    shown: HashSet<String>,
    /// Secrets fetched from the server to show (emptied again when hidden).
    revealed: HashSet<String>,
    bools: HashMap<String, bool>,
    checks: HashMap<String, Vec<String>>,
    areas: HashMap<String, text_editor::Content>,
    open: HashSet<&'static str>,
    /// Test and save results under a card.
    flash: HashMap<&'static str, Vec<(bool, String)>>,
    busy: HashSet<&'static str>,
    /// The card a link asked for: unfolded instead of the first one.
    focus: Option<&'static str>,
    api_key: Option<String>,
    key_shown: bool,
    dialog: Option<Dialog>,
    pub tools: tools::State,
    pub tasks: tasks::State,
    pub autodownload: autodownload::State,
    pub contribute: contribute::State,
    pub blocked: blocked::State,
}

#[derive(Clone, Debug)]
pub enum Message {
    Values(Result<Value, ApiError>),
    Toggle(&'static str),
    Edit(String, String),
    Bool(String, bool),
    Check(String, String, bool),
    Area(String, text_editor::Action),
    Reveal(String),
    Revealed(String, Result<Value, ApiError>),
    Hide(String),
    Copy(String),
    Copied(Result<Value, ApiError>),
    Clear(String),
    Save(&'static str),
    Saved(&'static str, Result<Value, ApiError>),
    Test(&'static str),
    Tested(&'static str, Result<(Value, Result<Value, ApiError>), ApiError>),
    VpnSaved(Result<Value, ApiError>),
    ApiKey(Result<Value, ApiError>),
    ShowKey(bool),
    CopyKey,
    /// Copy a failed test's words.
    CopyText(String),
    AskRegen,
    Regen,
    Regenerated(Result<Value, ApiError>),
    OpenReleases,
    Tools(tools::Message),
    Tasks(tasks::Message),
    Autodownload(autodownload::Message),
    Contribute(contribute::Message),
    Blocked(blocked::Message),
    TidalTick,
    CloseDialog,
}

/// The field group a section shows (None for the pages of their own).
fn group_id(s: Section) -> Option<&'static str> {
    match s {
        Section::General => Some("general"),
        Section::Library => Some("library"),
        Section::Downloads => Some("downloads"),
        Section::Integrations => Some("integrations"),
        Section::Processing => Some("processing"),
        Section::Accounts => Some("accounts"),
        Section::Tasks => Some("tasks"),
        _ => None,
    }
}

fn load_values(ctx: &Ctx) -> Task<Message> {
    let api = ctx.api.clone();
    Task::perform(async move { api.get("/admin/settings").await }, Message::Values)
}

pub fn open(state: &mut State, section: Section, ctx: &mut Ctx) -> Task<Message> {
    state.section = Some(section);
    state.error.clear();
    state.edits.clear();
    state.shown.clear();
    state.revealed.clear();
    state.bools.clear();
    state.checks.clear();
    state.areas.clear();
    state.flash.clear();
    state.busy.clear();
    state.open.clear();
    state.dialog = None;
    match section {
        Section::Autodownload => return autodownload::open(&mut state.autodownload, ctx).map(Message::Autodownload),
        Section::Contribute => return contribute::open(&mut state.contribute, ctx).map(Message::Contribute),
        Section::Blocked => return blocked::open(&mut state.blocked, ctx).map(Message::Blocked),
        _ => {}
    }
    state.values = None;
    let group = group_id(section).and_then(fields::group);
    state.focus = super::take_focus().filter(|f| group.is_some_and(|g| g.sections().any(|s| s == *f)));
    let mut tasks = vec![load_values(ctx)];
    match section {
        Section::Tasks => tasks.push(tasks::open(&mut state.tasks, ctx).map(Message::Tasks)),
        // the Download tools card and the blocks of the source cards; Cadenza's blocks need its state too
        Section::Downloads | Section::Processing => tasks.push(tools::open(&mut state.tools, ctx).map(Message::Tools)),
        Section::General if ctx.session.is_owner() => {
            let api = ctx.api.clone();
            tasks.push(Task::perform(async move { api.get("/system/api-key").await }, Message::ApiKey));
        }
        _ => {}
    }
    Task::batch(tasks)
}

/// The card that starts unfolded: the one a link asked for, else the first this server has
/// (the web skips cards whose settings the server does not know).
fn first_open(state: &State) -> Option<&'static str> {
    if state.focus.is_some() {
        return state.focus;
    }
    let values = state.values.as_ref()?;
    let group = state.section.and_then(group_id).and_then(fields::group)?;
    group
        .sections()
        .find(|id| fields::card(id).is_some_and(|c| c.fields.iter().any(|f| values.get(f.key).is_some())))
}

/// After the values arrived: a text box per textarea field the server has.
fn make_areas(state: &mut State, values: &Value) {
    for c in fields::SECTIONS {
        for f in c.fields.iter().filter(|f| f.kind == "textarea" && values.get(f.key).is_some()) {
            state.areas.entry(f.key.to_string()).or_default();
        }
    }
}

/// The value an input shows: what was typed, else the saved value.
fn current(state: &State, values: &Value, key: &str) -> String {
    state.edits.get(key).cloned().unwrap_or_else(|| vs(&values[key]))
}

/// The `when` rule: shown only while the select it names has one of the values.
fn visible(state: &State, values: &Value, f: &Field) -> bool {
    match f.when {
        None => true,
        Some((k, vals)) => vals.contains(&current(state, values, k).as_str()),
    }
}

/// The ticked options of a `checks` field.
fn checked(state: &State, values: &Value, key: &str) -> Vec<String> {
    match state.checks.get(key) {
        Some(c) => c.clone(),
        None => vs(&values[key]).split(',').map(fields::type_key).filter(|s| !s.is_empty()).collect(),
    }
}

/// The body of a card's Save, as the web builds it: every field the server has that the
/// environment does not lock; secrets only when typed (an empty box keeps the saved value).
pub fn save_body(state: &State, values: &Value, card: &Card) -> Map<String, Value> {
    let mut body = Map::new();
    for f in card.fields.iter().filter(|f| values.get(f.key).is_some()) {
        if env_locked(values, f.key) {
            continue;
        }
        match f.kind {
            "checks" => {
                let on = checked(state, values, f.key);
                let joined: Vec<&str> = f.options.iter().filter(|(v, _)| on.contains(&fields::type_key(v))).map(|(v, _)| *v).collect();
                body.insert(f.key.into(), Value::String(joined.join(",")));
            }
            "bool" => {
                let on = state.bools.get(f.key).copied().unwrap_or_else(|| super::truthy(&values[f.key]));
                body.insert(f.key.into(), Value::Bool(on));
            }
            "textarea" => {
                let text = state.areas.get(f.key).map(|c| c.text()).unwrap_or_default();
                if f.is_secret() && (text.trim().is_empty() || is_masked(&text)) {
                    continue;
                }
                body.insert(f.key.into(), Value::String(text.trim().to_string()));
            }
            "password" => {
                let text = state.edits.get(f.key).cloned().unwrap_or_default();
                if text.trim().is_empty() || is_masked(&text) || state.revealed.contains(f.key) {
                    continue;
                }
                body.insert(f.key.into(), Value::String(text.trim().to_string()));
            }
            _ => {
                let text = current(state, values, f.key);
                if is_masked(&text) {
                    continue;
                }
                body.insert(f.key.into(), Value::String(text));
            }
        }
    }
    if card.id == "cadenza" && !env_locked(values, "cadenza_engine_settings") {
        if let Some(e) = tools::engine_settings_value(&state.tools) {
            body.insert("cadenza_engine_settings".into(), Value::String(e));
        }
    }
    body
}

/// Forget the typed values of a card (its saved values show again).
fn settle(state: &mut State, card: &Card) {
    for f in card.fields {
        state.edits.remove(f.key);
        state.bools.remove(f.key);
        state.checks.remove(f.key);
        state.shown.remove(f.key);
        state.revealed.remove(f.key);
        if f.kind == "textarea" {
            if let Some(c) = state.areas.get_mut(f.key) {
                *c = text_editor::Content::new();
            }
        }
    }
}

/// `{ok, message}` of a test answer as words.
fn test_words(r: &Value) -> (bool, String) {
    let ok = fmt::truthy(r, "ok");
    let m = if fmt::truthy(r, "message") {
        fmt::s(r, "message")
    } else if ok {
        "Connected".into()
    } else {
        "Failed".into()
    };
    (ok, m)
}

fn take_values(state: &mut State, mut v: Value) -> Vec<String> {
    let warnings: Vec<String> = fmt::items(&v["_warnings"]).iter().map(|w| fmt::s(w, "message")).filter(|m| !m.is_empty()).collect();
    if let Some(o) = v.as_object_mut() {
        o.remove("_warnings");
    }
    make_areas(state, &v);
    state.values = Some(v);
    warnings
}

pub fn update(state: &mut State, msg: Message, ctx: &mut Ctx) -> Task<Message> {
    match msg {
        Message::Values(Ok(v)) => {
            take_values(state, v);
            if state.open.is_empty() {
                if let Some(id) = first_open(state) {
                    state.open.insert(id);
                }
            }
        }
        Message::Values(Err(e)) => state.error = e.message,
        Message::Toggle(id) => {
            if !state.open.remove(id) {
                state.open.insert(id);
            }
        }
        Message::Edit(k, v) => {
            state.revealed.remove(&k);
            state.edits.insert(k, v);
        }
        Message::Bool(k, on) => {
            state.bools.insert(k, on);
        }
        Message::Check(k, option, on) => {
            let values = state.values.clone().unwrap_or(Value::Null);
            let mut list = checked(state, &values, &k);
            let key = fields::type_key(&option);
            list.retain(|x| *x != key);
            if on {
                list.push(key);
            }
            state.checks.insert(k, list);
        }
        Message::Area(k, a) => {
            if let Some(c) = state.areas.get_mut(&k) {
                c.perform(a);
            }
        }
        Message::Reveal(k) => {
            if state.edits.get(&k).is_some_and(|v| !v.is_empty()) {
                state.shown.insert(k);
                return Task::none();
            }
            let api = ctx.api.clone();
            let key = k.clone();
            return Task::perform(
                async move { api.get(&format!("/admin/settings/secret/{}", crate::pages::urlencode(&key))).await },
                move |r| Message::Revealed(k.clone(), r),
            );
        }
        Message::Revealed(k, Ok(r)) => {
            let value = fmt::s(&r, "value");
            if let Some(c) = state.areas.get_mut(&k) {
                *c = text_editor::Content::with_text(&value);
            } else {
                state.edits.insert(k.clone(), value);
            }
            state.revealed.insert(k.clone());
            state.shown.insert(k);
        }
        Message::Revealed(_, Err(e)) => ctx.toast_err(e.message),
        Message::Hide(k) => {
            state.shown.remove(&k);
            if state.revealed.remove(&k) {
                state.edits.remove(&k);
                if let Some(c) = state.areas.get_mut(&k) {
                    *c = text_editor::Content::new();
                }
            }
        }
        Message::Copy(k) => {
            let typed = state.edits.get(&k).cloned().or_else(|| state.areas.get(&k).map(|c| c.text())).unwrap_or_default();
            if !typed.trim().is_empty() {
                ctx.copy(typed);
                ctx.toast_ok("Copied to clipboard");
                return Task::none();
            }
            let api = ctx.api.clone();
            return Task::perform(
                async move { api.get(&format!("/admin/settings/secret/{}", crate::pages::urlencode(&k))).await },
                Message::Copied,
            );
        }
        Message::Copied(Ok(r)) => {
            ctx.copy(fmt::s(&r, "value"));
            ctx.toast_ok("Copied to clipboard");
        }
        Message::Copied(Err(e)) => ctx.toast_err(e.message),
        Message::Clear(k) => {
            let api = ctx.api.clone();
            let mut body = Map::new();
            body.insert(k, Value::String(String::new()));
            return Task::perform(async move { api.put("/admin/settings", Value::Object(body)).await }, |r| Message::Saved("", r));
        }
        Message::Save(id) => {
            let (Some(values), Some(card)) = (&state.values, fields::card(id)) else { return Task::none() };
            let body = save_body(state, values, card);
            state.busy.insert(id);
            state.flash.remove(id);
            let api = ctx.api.clone();
            return Task::perform(async move { api.put("/admin/settings", Value::Object(body)).await }, move |r| Message::Saved(id, r));
        }
        Message::Saved(id, Ok(v)) => {
            state.busy.remove(id);
            let warnings = take_values(state, v);
            if id.is_empty() {
                ctx.toast_ok("Cleared");
            } else {
                if let Some(card) = fields::card(id) {
                    settle(state, card);
                }
                ctx.toast_ok("Settings saved");
                if !warnings.is_empty() {
                    state.flash.insert(id, warnings.into_iter().map(|w| (false, w)).collect());
                }
            }
        }
        Message::Saved(id, Err(e)) => {
            state.busy.remove(id);
            ctx.toast_err(e.message);
        }
        Message::Test(id) => {
            let (Some(values), Some(card)) = (&state.values, fields::card(id)) else { return Task::none() };
            let Some(test) = card.test else { return Task::none() };
            let body = save_body(state, values, card);
            state.busy.insert(id);
            state.open.insert(id);
            state.flash.insert(id, vec![(true, "Testing…".into())]);
            let api = ctx.api.clone();
            return Task::perform(
                async move {
                    let saved = api.put("/admin/settings", Value::Object(body)).await?;
                    let tested = api.post_empty(&format!("/system/test/{test}")).await;
                    Ok((saved, tested))
                },
                move |r| Message::Tested(id, r),
            );
        }
        Message::Tested(id, Ok((saved, tested))) => {
            state.busy.remove(id);
            let mut lines: Vec<(bool, String)> = take_values(state, saved).into_iter().map(|w| (false, w)).collect();
            if let Some(card) = fields::card(id) {
                settle(state, card);
            }
            match tested {
                Ok(r) => lines.push(test_words(&r)),
                Err(e) => lines.push((false, if e.message.is_empty() { "Test failed".into() } else { e.message })),
            }
            state.flash.insert(id, lines);
        }
        Message::Tested(id, Err(e)) => {
            state.busy.remove(id);
            state.flash.insert(id, vec![(false, if e.message.is_empty() { "Test failed".into() } else { e.message })]);
        }
        Message::VpnSaved(Ok(v)) => {
            take_values(state, v);
            if let Some(card) = fields::card("slskd") {
                settle(state, card);
            }
            return tools::start_vpn_test(&mut state.tools, ctx).map(Message::Tools);
        }
        Message::VpnSaved(Err(e)) => tools::vpn_test_failed(&mut state.tools, e.message),
        Message::ApiKey(Ok(r)) => state.api_key = Some(fmt::s(&r, "api_key")),
        Message::ApiKey(Err(_)) => state.api_key = None,
        Message::ShowKey(on) => state.key_shown = on,
        Message::CopyText(text) => ctx.copy_text(text),
        Message::CopyKey => {
            if let Some(k) = &state.api_key {
                ctx.copy(k.clone());
                ctx.toast_ok("Copied to clipboard");
            }
        }
        Message::AskRegen => state.dialog = Some(Dialog::RegenKey),
        Message::Regen => {
            state.dialog = None;
            let api = ctx.api.clone();
            return Task::perform(async move { api.post_empty("/admin/settings/api-key").await }, Message::Regenerated);
        }
        Message::Regenerated(Ok(r)) => {
            state.api_key = Some(fmt::s(&r, "api_key"));
            ctx.toast_ok("New API key generated");
        }
        Message::Regenerated(Err(e)) => ctx.toast_err(e.message),
        Message::OpenReleases => ctx.open_url("https://github.com/Krifmaz/Encore/releases"),
        Message::Tools(tools::Message::VpnTest) => {
            // save the Soulseek card first, then run the test
            let (Some(values), Some(card)) = (&state.values, fields::card("slskd")) else { return Task::none() };
            let body = save_body(state, values, card);
            state.tools.vpn_testing_started();
            let api = ctx.api.clone();
            return Task::perform(async move { api.put("/admin/settings", Value::Object(body)).await }, Message::VpnSaved);
        }
        Message::Tools(m) => return tools::update(&mut state.tools, m, ctx).map(Message::Tools),
        Message::Tasks(m) => return tasks::update(&mut state.tasks, m, ctx).map(Message::Tasks),
        Message::Autodownload(m) => return autodownload::update(&mut state.autodownload, m, ctx).map(Message::Autodownload),
        Message::Contribute(m) => return contribute::update(&mut state.contribute, m, ctx).map(Message::Contribute),
        Message::Blocked(m) => return blocked::update(&mut state.blocked, m, ctx).map(Message::Blocked),
        Message::TidalTick => return tools::update(&mut state.tools, tools::Message::TidalTick, ctx).map(Message::Tools),
        Message::CloseDialog => state.dialog = None,
    }
    Task::none()
}

// ----- views -------------------------------------------------------------------------------

/// The note under a field about where its value comes from.
fn source_note<'a>(f: &Field, values: &Value) -> Option<El<'a, Message>> {
    let m = &values["_meta"][f.key];
    let set = super::truthy(&values[f.key]);
    if m["source"].as_str() == Some("env") {
        return Some(help(format!("🔒 Set by {} in your compose/env – change it there (environment always wins)", fmt::s(m, "env"))).into());
    }
    if f.is_secret() && set {
        return Some(
            row![help("Saved ✓ (hidden) ·"), link_muted("clear", Message::Clear(f.key.to_string()))]
                .spacing(4)
                .align_y(Alignment::Center)
                .into(),
        );
    }
    if f.is_secret() {
        let env = if fmt::truthy(m, "env") { format!(" – or set {} in your compose file", fmt::s(m, "env")) } else { String::new() };
        return Some(help(format!("Not set{env}")).into());
    }
    if m["source"].as_str() == Some("ui") {
        return None;
    }
    if fmt::truthy(m, "env") {
        return Some(widgets::faint(format!("Default · env var {}", fmt::s(m, "env"))).size(12).into());
    }
    None
}

fn field_view<'a>(state: &'a State, values: &'a Value, f: &'static Field, reveal: bool) -> El<'a, Message> {
    let key = f.key.to_string();
    let locked = env_locked(values, f.key);
    let v = &values[f.key];
    let mut lines: Vec<El<'a, Message>> = Vec::new();
    match f.kind {
        "bool" => {
            let on = state.bools.get(f.key).copied().unwrap_or_else(|| super::truthy(v));
            let k = key.clone();
            lines.push(switch_maybe(on, f.label, (!locked).then_some(move |x| Message::Bool(k.clone(), x))));
        }
        "checks" => {
            let on = checked(state, values, f.key);
            let mut boxes = row![].spacing(14);
            for (val, lab) in f.options {
                let is = on.contains(&fields::type_key(val));
                let (k, o) = (key.clone(), val.to_string());
                boxes = boxes.push(checkbox(is).label(*lab).size(16).text_size(13).spacing(6).on_toggle_maybe((!locked).then_some(move |x| Message::Check(k.clone(), o.clone(), x))));
            }
            lines.push(widgets::small_muted(f.label).font(theme::semibold()).into());
            lines.push(boxes.wrap().into());
        }
        "select" => {
            let cur = current(state, values, f.key);
            let k = key.clone();
            let picker: El<'a, Message> = if locked {
                help(f.options.iter().find(|(x, _)| *x == cur).map(|(_, l)| *l).unwrap_or(cur.as_str()).to_string()).into()
            } else {
                select_table(f.options, &cur, move |x| Message::Edit(k.clone(), x))
            };
            lines.push(widgets::field(f.label, picker));
        }
        "textarea" => {
            lines.push(widgets::small_muted(f.label).font(theme::semibold()).into());
            if let Some(c) = state.areas.get(f.key) {
                let k = key.clone();
                let placeholder = if super::truthy(v) { "Saved – paste to replace" } else { "" };
                lines.push(text_area(c, placeholder, 5, move |a| Message::Area(k.clone(), a)));
            }
            if reveal && super::truthy(v) && f.key != "vpn_wireguard_config" && !locked {
                let shown = state.shown.contains(f.key);
                lines.push(
                    row![
                        btn_sm(if shown { "Hide" } else { "Show saved" }, Btn::Default, Some(if shown { Message::Hide(key.clone()) } else { Message::Reveal(key.clone()) })),
                        btn_icon(Icon::Copy, "Copy", Btn::Default, Some(Message::Copy(key.clone()))),
                    ]
                    .spacing(8)
                    .into(),
                );
            }
        }
        "password" => {
            let text = state.edits.get(f.key).cloned().unwrap_or_default();
            let placeholder = if locked {
                "set by environment"
            } else if super::truthy(v) {
                "saved – paste a new value to replace"
            } else {
                "not set"
            };
            let shown = state.shown.contains(f.key);
            let k = key.clone();
            let mut r = row![].spacing(6).align_y(Alignment::Center);
            if locked {
                r = r.push(input_locked::<Message>("").width(Length::Fill));
            } else {
                r = r.push(input(placeholder, &text, move |x| Message::Edit(k.clone(), x)).secure(!shown).width(Length::Fill));
            }
            if reveal && super::truthy(v) {
                r = r.push(icon_btn(if shown { Icon::X } else { Icon::Search }, if shown { "Hide" } else { "Show" }, Some(if shown { Message::Hide(key.clone()) } else { Message::Reveal(key.clone()) })));
                r = r.push(icon_btn(Icon::Copy, "Copy", Some(Message::Copy(key.clone()))));
            }
            lines.push(widgets::field(f.label, r));
        }
        _ => {
            let text = current(state, values, f.key);
            let k = key.clone();
            let b: El<'a, Message> = if locked {
                input_locked::<Message>(&text).width(Length::Fill).into()
            } else {
                input("", &text, move |x| Message::Edit(k.clone(), x)).width(Length::Fill).into()
            };
            lines.push(widgets::field(f.label, b));
        }
    }
    if let Some(h) = f.help {
        lines.push(help(h).into());
    }
    if let Some(n) = source_note(f, values) {
        lines.push(n);
    }
    column(lines).spacing(4).width(Length::Fill).into()
}

/// The fields of a card two per row (`.form-grid`), the spanning ones on a row of their own.
fn grid<'a>(state: &'a State, values: &'a Value, fields: &[&'static Field], reveal: bool) -> El<'a, Message> {
    let mut rows = column![].spacing(14);
    let mut pending: Option<El<'a, Message>> = None;
    for f in fields {
        let el = field_view(state, values, f, reveal);
        if f.spans() {
            if let Some(p) = pending.take() {
                rows = rows.push(super::two(p, None));
            }
            rows = rows.push(el);
        } else if let Some(p) = pending.take() {
            rows = rows.push(super::two(p, Some(el)));
        } else {
            pending = Some(el);
        }
    }
    if let Some(p) = pending.take() {
        rows = rows.push(super::two(p, None));
    }
    rows.width(Length::Fill).into()
}

fn card_view<'a>(state: &'a State, values: &'a Value, card: &'static Card, ctx: &Ctx<'a>) -> El<'a, Message> {
    let p = ctx.look;
    let fields: Vec<&'static Field> = card.fields.iter().filter(|f| values.get(f.key).is_some() && visible(state, values, f)).collect();
    let open = state.open.contains(card.id);
    let busy = state.busy.contains(card.id);
    let reveal = ctx.has_feature("reveal") && ctx.session.is_owner();
    let mut head_words = column![widgets::h2(card.title)].spacing(4);
    if !card.desc.is_empty() {
        head_words = head_words.push(help(card.desc));
    }
    let header = button(row![head_words.width(Length::Fill), container(icon(if open { Icon::Chevron } else { Icon::More }, 18.0, p.muted)).padding([4, 0])].spacing(10).align_y(Alignment::Start))
        .padding(0)
        .width(Length::Fill)
        .style(theme::btn(Btn::Row))
        .on_press(Message::Toggle(card.id));
    let mut head = row![header].spacing(10).align_y(Alignment::Start);
    if card.test.is_some() {
        head = head.push(btn_icon(Icon::Refresh, card.test_label.unwrap_or("Test connection"), Btn::Default, (!busy).then_some(Message::Test(card.id))));
    }
    let mut body = column![head].spacing(14).width(Length::Fill);
    if open {
        if let Some(lines) = state.flash.get(card.id) {
            for (ok, m) in lines {
                body = body.push(result_line_copy(*ok, m, Message::CopyText));
            }
        }
        if card.id == "youtube" {
            body = body.push(tools::youtube_block(&state.tools).map(Message::Tools));
        }
        if card.id == "slskd" {
            let (vpn, rest): (Vec<&'static Field>, Vec<&'static Field>) = fields.iter().copied().partition(|f| f.key.starts_with("vpn_"));
            body = body.push(grid(state, values, &rest, reveal));
            if !vpn.is_empty() {
                body = body.push(tools::vpn_block(
                    &state.tools,
                    grid(state, values, &vpn, reveal),
                    (!state.tools.vpn_testing()).then_some(Message::Tools(tools::Message::VpnTest)),
                    Message::CopyText,
                ));
            }
            body = body.push(tools::soulseek_block(&state.tools).map(Message::Tools));
        } else {
            body = body.push(grid(state, values, &fields, reveal));
        }
        if card.id == "streamrip" {
            body = body.push(tools::tidal_block(&state.tools).map(Message::Tools));
            body = body.push(tools::streamrip_block(&state.tools).map(Message::Tools));
        }
        if card.id == "cadenza" {
            body = body.push(tools::cadenza_block(&state.tools, values).map(Message::Tools));
        }
        body = body.push(
            container(btn_icon(Icon::Check, if busy { "Saving…" } else { "Save" }, Btn::Primary, (!busy).then_some(Message::Save(card.id))))
                .width(Length::Fill)
                .align_x(Alignment::End),
        );
    }
    widgets::card(body).width(Length::Fill).into()
}

fn api_key_card<'a>(state: &'a State) -> Option<El<'a, Message>> {
    let key = state.api_key.as_ref()?;
    let shown = if state.key_shown { key.clone() } else { "•".repeat(key.chars().count().min(32)) };
    Some(super::card(
        "Encore API key",
        vec![
            help("Send it in an X-Api-Key header to automate Encore (scripts, Home Assistant…). It acts as the owner account – keep it private.").into(),
            row![
                container(t(shown).size(13).font(theme::mono())).padding(8).width(Length::Fill).style(theme::panel2),
                icon_btn(if state.key_shown { Icon::X } else { Icon::Search }, if state.key_shown { "Hide" } else { "Show" }, Some(Message::ShowKey(!state.key_shown))),
                icon_btn(Icon::Copy, "Copy", Some(Message::CopyKey)),
                btn_icon(Icon::Refresh, "Regenerate", Btn::Danger, Some(Message::AskRegen)),
            ]
            .spacing(8)
            .align_y(Alignment::Center)
            .into(),
        ],
    ))
}

fn pipeline_help<'a>() -> El<'a, Message> {
    let steps = [
        ("Library check", "if the album/track is already in your library (by MusicBrainz id or name) it's marked available instantly."),
        ("streamrip", "lossless from Qobuz, Tidal or Deezer with your own accounts, matched by ISRC."),
        ("Torrents & Usenet", "Prowlarr searches your indexers; the best album release goes to your torrent or Usenet client and is imported when it finishes. If it stalls past the timeout, the next release or source is tried."),
        ("Soulseek", "Encore searches the network, results are ranked by track count, format, bitrate, folder name and the uploader's free slots/speed. Stalled or failed users are skipped automatically."),
        ("yt-dlp", "each track from the MusicBrainz tracklist is matched on YouTube (preferring official \"Topic\" uploads with matching length)."),
        ("Import", "files are matched to the MusicBrainz release, tagged (incl. MBIDs, ISRCs, cover art), renamed with your template, and your media servers are refreshed."),
    ];
    let mut lines: Vec<El<'a, Message>> = Vec::new();
    for (i, (name, what)) in steps.iter().enumerate() {
        lines.push(
            row![
                widgets::muted(format!("{}.", i + 1)).size(13).width(22),
                t(*name).size(13).font(theme::semibold()),
                widgets::muted(format!("– {what}")).size(13).width(Length::Fill),
            ]
            .spacing(6)
            .align_y(Alignment::Start)
            .into(),
        );
    }
    let mut c = column![widgets::h3("How requests are fulfilled")].spacing(8);
    for l in lines {
        c = c.push(l);
    }
    widgets::card(c.width(Length::Fill)).width(Length::Fill).into()
}

pub fn view<'a>(state: &'a State, ctx: &Ctx<'a>) -> El<'a, Message> {
    let section = state.section.unwrap_or(Section::General);
    match section {
        Section::Autodownload => return autodownload::view(&state.autodownload, ctx).map(Message::Autodownload),
        Section::Contribute => return contribute::view(&state.contribute, ctx).map(Message::Contribute),
        Section::Blocked => return blocked::view(&state.blocked, ctx).map(Message::Blocked),
        _ => {}
    }
    if !state.error.is_empty() {
        return widgets::error_box(&state.error);
    }
    let Some(values) = &state.values else {
        return widgets::skeleton_rows(6);
    };
    let Some(group) = group_id(section).and_then(fields::group) else {
        return widgets::skeleton_rows(2);
    };
    let mut page = column![].spacing(16).width(Length::Fill);
    if section == Section::Tasks {
        page = page.push(tasks::view(&state.tasks, ctx).map(Message::Tasks));
    }
    if section == Section::Downloads {
        page = page.push(tools::tools_card(&state.tools, values).map(Message::Tools));
    }
    for (label, ids) in group.parts {
        let cards: Vec<&'static Card> = ids
            .iter()
            .filter_map(|id| fields::card(id))
            .filter(|c| c.fields.iter().any(|f| values.get(f.key).is_some()))
            .collect();
        if cards.is_empty() {
            continue;
        }
        if !label.is_empty() {
            page = page.push(container(t(*label).size(11).color(ctx.look.faint).font(theme::bold())).padding(widgets::pad(10.0, 4.0, 0.0, 4.0)));
        }
        for c in cards {
            page = page.push(card_view(state, values, c, ctx));
        }
    }
    if section == Section::Downloads {
        page = page.push(pipeline_help());
    }
    if section == Section::General {
        if let Some(k) = api_key_card(state) {
            page = page.push(k);
        }
        page = page.push(super::account::about_card(ctx, Message::OpenReleases));
    }
    page.into()
}

pub fn overlay<'a>(state: &'a State, ctx: &Ctx<'a>) -> Option<El<'a, Message>> {
    match state.section.unwrap_or(Section::General) {
        Section::Autodownload => return None,
        Section::Contribute => return None,
        Section::Blocked => return blocked::overlay(&state.blocked, ctx).map(|e| e.map(Message::Blocked)),
        _ => {}
    }
    if let Some(d) = tools::overlay(&state.tools) {
        return Some(d.map(Message::Tools));
    }
    match state.dialog? {
        Dialog::RegenKey => Some(modal(
            "Encore API key",
            widgets::muted("Make a new API key? Scripts using the old one stop working.").size(14),
            Some(
                row![
                    btn("Cancel", Btn::Default, Some(Message::CloseDialog)),
                    btn("Regenerate", Btn::Danger, Some(Message::Regen)),
                ]
                .spacing(10)
                .into(),
            ),
            false,
            Message::CloseDialog,
        )),
    }
}

pub fn subscription(state: &State) -> Subscription<Message> {
    // the Soulseek and VPN states and the Tidal sign-in are on Downloads
    if state.section != Some(Section::Downloads) {
        return Subscription::none();
    }
    let mut subs = vec![iced::time::every(Duration::from_secs(5)).map(|_| Message::Tools(tools::Message::Tick))];
    if tools::tidal_waiting(&state.tools) {
        subs.push(iced::time::every(Duration::from_secs(3)).map(|_| Message::TidalTick));
    }
    Subscription::batch(subs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_save_body_follows_the_web_rules() {
        let values = json!({
            "app_name": "Encore", "base_url": "http://x", "auto_approve": 1, "github_token": "•••• (saved)", "signup_mode": "invite",
            "_meta": { "base_url": { "source": "env", "env": "ENCORE_BASE_URL" } }
        });
        let mut st = State::default();
        st.edits.insert("app_name".into(), "Mine".into());
        st.bools.insert("auto_approve".into(), false);
        let card = fields::card("general").unwrap();
        let b = save_body(&st, &values, card);
        assert_eq!(b["app_name"], "Mine");
        assert_eq!(b["auto_approve"], false);
        assert_eq!(b["signup_mode"], "invite");
        assert!(b.get("base_url").is_none(), "locked by the environment");
        assert!(b.get("github_token").is_none(), "nothing typed keeps the saved secret");
        assert!(b.get("session_days").is_none(), "the server does not have it");
        st.edits.insert("github_token".into(), " tok ".into());
        assert_eq!(save_body(&st, &values, card)["github_token"], "tok");
        // checks join the ticked options in the table's spelling
        let values = json!({ "release_types": "album,ep", "release_skip": "" });
        let mut st = State::default();
        st.checks.insert("release_skip".into(), vec!["live".into(), "djmix".into()]);
        let b = save_body(&st, &values, fields::card("releases").unwrap());
        assert_eq!(b["release_types"], "Album,EP");
        assert_eq!(b["release_skip"], "Live,DJ-mix");
    }

    #[test]
    fn when_rules_follow_the_select() {
        let values = json!({ "torrent_client": "qbittorrent", "qbit_url": "", "transmission_url": "" });
        let mut st = State::default();
        let qbit = fields::card("torznab").unwrap().fields.iter().find(|f| f.key == "qbit_url").unwrap();
        let tr = fields::card("torznab").unwrap().fields.iter().find(|f| f.key == "transmission_url").unwrap();
        assert!(visible(&st, &values, qbit) && !visible(&st, &values, tr));
        st.edits.insert("torrent_client".into(), "transmission".into());
        assert!(!visible(&st, &values, qbit) && visible(&st, &values, tr));
        assert_eq!(test_words(&json!({ "ok": true })), (true, "Connected".to_string()));
        assert_eq!(test_words(&json!({ "ok": false, "message": "no" })), (false, "no".to_string()));
        assert_eq!(group_id(Section::Tasks), Some("tasks"));
        assert_eq!(group_id(Section::Blocked), None);
        assert_eq!(group_id(Section::Processing), Some("processing"));
        // the first card the server has starts open, or the one a link asked for
        let mut st = State {
            section: Some(Section::Downloads),
            values: Some(json!({ "release_types": "Album" })),
            ..State::default()
        };
        assert_eq!(first_open(&st), Some("releases"));
        st.focus = Some("slskd");
        assert_eq!(first_open(&st), Some("slskd"));
    }
}
