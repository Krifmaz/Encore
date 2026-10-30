//! Settings → Users & permissions. Behaviour reference: web-rs/src/pages/admin.rs `render_users`.
//! The users table (role, requests, weekly limit, auto-approve, last seen, permissions, password
//! reset, disable, delete), Add user, Permissions for everyone and what new members start with.

use super::{env_locked, env_note, help, input, input_locked, js_plus, patch, select_table, switch, switch_maybe};
use crate::api::ApiError;
use crate::fmt;
use crate::pages::{Ctx, El, Page, Section};
use crate::theme::{self, Badge, Btn};
use crate::widgets::{self, badge, btn, btn_icon, btn_sm, icon_btn, modal, small_muted, t, Icon};
use iced::widget::{column, container, row, space};
use iced::{Alignment, Length, Task};
use serde_json::{json, Map, Value};
use std::collections::HashMap;

/// What new members start with: `(settings key, label)`.
pub const NEW: [(&str, &str); 6] = [
    ("new_user_auto_approve", "Requests skip the approval queue"),
    ("new_user_auto_approve_playlists", "Playlist downloads skip the approval queue"),
    ("new_user_pick", "Pick the exact version to download"),
    ("new_user_fix", "Fix their own downloads"),
    ("new_user_storage", "See server storage"),
    ("new_user_auto_download", "Auto-download (likes and followed playlists)"),
];

const ROLES: [(&str, &str); 2] = [("user", "User"), ("admin", "Admin")];

#[derive(Debug)]
pub enum Dialog {
    Add {
        username: String,
        email: String,
        password: String,
        role: String,
        busy: bool,
    },
    Perms {
        id: String,
        name: String,
        perms: Vec<(String, String, bool)>,
    },
    Password {
        id: String,
        name: String,
        password: String,
    },
    Delete {
        id: String,
        name: String,
    },
}

#[derive(Debug, Default)]
pub struct State {
    users: Option<Vec<Value>>,
    perms: Option<Value>,
    settings: Value,
    /// The weekly limit boxes while typed in.
    limits: HashMap<String, String>,
    default_limit: String,
    dialog: Option<Dialog>,
    error: String,
}

#[derive(Clone, Debug)]
pub enum Message {
    Users(Result<Value, ApiError>),
    Perms(Result<Value, ApiError>),
    Settings(Result<Value, ApiError>),
    Role(String, String),
    LimitInput(String, String),
    LimitCommit(String),
    AutoApprove(String, bool),
    /// Disable (`true`) or enable a user.
    Disabled(String, bool),
    AskDelete(String, String),
    Delete,
    OpenPerms(String, String),
    PermToggle(String, bool),
    SavePerms,
    OpenPassword(String, String),
    PasswordInput(String),
    SavePassword,
    Bulk(String, bool),
    NewDefault(&'static str, bool),
    DefaultLimitInput(String),
    DefaultLimitCommit,
    OpenAdd,
    AddUsername(String),
    AddEmail(String),
    AddPassword(String),
    AddRole(String),
    AddSubmit,
    Patched(Result<Value, ApiError>),
    BulkDone(Result<Value, ApiError>),
    DefaultsSaved(Result<Value, ApiError>),
    Created(Result<Value, ApiError>),
    Deleted(Result<Value, ApiError>),
    GoInvites,
    CloseDialog,
}

fn load_users(ctx: &Ctx) -> Task<Message> {
    let api = ctx.api.clone();
    Task::perform(async move { api.get("/admin/users").await }, Message::Users)
}

fn load_perms(ctx: &Ctx) -> Task<Message> {
    let api = ctx.api.clone();
    Task::perform(async move { api.get("/admin/permissions").await }, Message::Perms)
}

pub fn open(state: &mut State, ctx: &mut Ctx) -> Task<Message> {
    state.users = None;
    state.perms = None;
    state.dialog = None;
    state.error.clear();
    state.limits.clear();
    let api = ctx.api.clone();
    Task::batch([
        load_users(ctx),
        load_perms(ctx),
        Task::perform(async move { api.get("/admin/settings").await }, Message::Settings),
    ])
}

/// `PATCH /admin/users/{id}` with one field; the table reloads after.
fn patch_user(ctx: &Ctx, id: String, body: Value) -> Task<Message> {
    let api = ctx.api.clone();
    Task::perform(async move { patch(&api, &format!("/admin/users/{id}"), body).await }, Message::Patched)
}

fn obj(k: &str, v: Value) -> Value {
    let mut m = Map::new();
    m.insert(k.to_string(), v);
    Value::Object(m)
}

/// `parseInt(s, 10) || 0`.
pub fn parse_int(s: &str) -> i64 {
    let t = s.trim_start();
    let (neg, rest) = match t.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    let n = digits.parse::<i64>().unwrap_or(0);
    if neg { -n } else { n }
}

pub fn update(state: &mut State, msg: Message, ctx: &mut Ctx) -> Task<Message> {
    match msg {
        Message::Users(Ok(v)) => {
            let list = fmt::items(&v);
            state.limits = list
                .iter()
                .map(|u| (fmt::s(u, "id"), if fmt::truthy(u, "request_limit") { fmt::s(u, "request_limit") } else { String::new() }))
                .collect();
            state.users = Some(list);
        }
        Message::Users(Err(e)) => state.error = e.message,
        Message::Perms(Ok(v)) => state.perms = Some(v),
        Message::Perms(Err(_)) => state.perms = Some(json!({ "perms": [] })),
        Message::Settings(Ok(v)) => {
            state.default_limit = if v["default_request_limit"].is_null() { "0".into() } else { fmt::s(&v, "default_request_limit") };
            state.settings = v;
        }
        Message::Settings(Err(_)) => state.settings = Value::Null,
        Message::Role(id, role) => return patch_user(ctx, id, obj("role", json!(role))),
        Message::LimitInput(id, s) => {
            state.limits.insert(id, s);
        }
        Message::LimitCommit(id) => {
            let v = state.limits.get(&id).cloned().unwrap_or_default();
            return patch_user(ctx, id, obj("request_limit", json!(parse_int(&v))));
        }
        Message::AutoApprove(id, on) => return patch_user(ctx, id, obj("auto_approve", json!(on))),
        Message::Disabled(id, d) => return patch_user(ctx, id, obj("disabled", json!(d))),
        Message::AskDelete(id, name) => state.dialog = Some(Dialog::Delete { id, name }),
        Message::Delete => {
            if let Some(Dialog::Delete { id, .. }) = state.dialog.take() {
                let api = ctx.api.clone();
                return Task::perform(async move { api.del(&format!("/admin/users/{id}")).await }, Message::Deleted);
            }
        }
        Message::OpenPerms(id, name) => {
            let user = state.users.as_ref().and_then(|l| l.iter().find(|u| fmt::s(u, "id") == id)).cloned().unwrap_or(Value::Null);
            let perms = state
                .perms
                .as_ref()
                .map(|d| fmt::items(&d["perms"]))
                .unwrap_or_default()
                .iter()
                .map(|p| {
                    let key = fmt::s(p, "key");
                    let on = fmt::truthy(&user["perms"], &key);
                    (key, fmt::s(p, "label"), on)
                })
                .collect();
            state.dialog = Some(Dialog::Perms { id, name, perms });
        }
        Message::PermToggle(key, on) => {
            if let Some(Dialog::Perms { perms, .. }) = state.dialog.as_mut() {
                if let Some(p) = perms.iter_mut().find(|(k, _, _)| *k == key) {
                    p.2 = on;
                }
            }
        }
        Message::SavePerms => {
            if let Some(Dialog::Perms { id, perms, .. }) = state.dialog.take() {
                let mut m = Map::new();
                for (k, _, on) in perms {
                    m.insert(k, Value::Bool(on));
                }
                return Task::batch([patch_user(ctx, id, json!({ "perms": Value::Object(m) })), load_perms(ctx)]);
            }
        }
        Message::OpenPassword(id, name) => {
            state.dialog = Some(Dialog::Password {
                id,
                name,
                password: String::new(),
            })
        }
        Message::PasswordInput(s) => {
            if let Some(Dialog::Password { password, .. }) = state.dialog.as_mut() {
                *password = s;
            }
        }
        Message::SavePassword => {
            if let Some(Dialog::Password { id, password, .. }) = state.dialog.take() {
                return patch_user(ctx, id, json!({ "password": password }));
            }
        }
        Message::Bulk(perm, on) => {
            let api = ctx.api.clone();
            let body = json!({ "perm": perm, "value": on });
            return Task::perform(async move { api.post("/admin/users/bulk", body).await }, Message::BulkDone);
        }
        // set by an environment variable: never sent (the environment would win at the next start)
        Message::NewDefault(key, _) if env_locked(&state.settings, key) => {}
        Message::DefaultLimitCommit if env_locked(&state.settings, "default_request_limit") => {}
        Message::NewDefault(key, on) => {
            state.settings[key] = json!(on);
            let api = ctx.api.clone();
            let body = obj(key, json!(on));
            return Task::perform(async move { api.put("/admin/settings", body).await }, Message::DefaultsSaved);
        }
        Message::DefaultLimitInput(s) => state.default_limit = s,
        Message::DefaultLimitCommit => {
            let api = ctx.api.clone();
            let body = obj("default_request_limit", js_plus(&state.default_limit));
            return Task::perform(async move { api.put("/admin/settings", body).await }, Message::DefaultsSaved);
        }
        Message::OpenAdd => {
            state.dialog = Some(Dialog::Add {
                username: String::new(),
                email: String::new(),
                password: String::new(),
                role: "user".into(),
                busy: false,
            })
        }
        Message::AddUsername(s) => {
            if let Some(Dialog::Add { username, .. }) = state.dialog.as_mut() {
                *username = s;
            }
        }
        Message::AddEmail(s) => {
            if let Some(Dialog::Add { email, .. }) = state.dialog.as_mut() {
                *email = s;
            }
        }
        Message::AddPassword(s) => {
            if let Some(Dialog::Add { password, .. }) = state.dialog.as_mut() {
                *password = s;
            }
        }
        Message::AddRole(r) => {
            if let Some(Dialog::Add { role, .. }) = state.dialog.as_mut() {
                *role = r;
            }
        }
        Message::AddSubmit => {
            if let Some(Dialog::Add { username, email, password, role, busy }) = state.dialog.as_mut() {
                if username.trim().chars().count() < 3 {
                    ctx.toast_err("The username needs at least 3 characters");
                    return Task::none();
                }
                if password.chars().count() < 8 {
                    ctx.toast_err("The password needs at least 8 characters");
                    return Task::none();
                }
                *busy = true;
                let api = ctx.api.clone();
                let body = json!({ "username": username.trim(), "email": email.trim(), "password": password, "role": role });
                return Task::perform(async move { api.post("/admin/users", body).await }, Message::Created);
            }
        }
        Message::Patched(Ok(_)) => {
            ctx.toast_ok("Saved");
            return load_users(ctx);
        }
        Message::Patched(Err(e)) => {
            ctx.toast_err(e.message);
            return load_users(ctx);
        }
        Message::BulkDone(Ok(_)) => {
            ctx.toast_ok("Updated for everyone");
            return Task::batch([load_perms(ctx), load_users(ctx)]);
        }
        Message::BulkDone(Err(e)) => ctx.toast_err(e.message),
        Message::DefaultsSaved(Ok(_)) => ctx.toast_ok("Saved for new members"),
        Message::DefaultsSaved(Err(e)) => ctx.toast_err(e.message),
        Message::Created(Ok(_)) => {
            state.dialog = None;
            ctx.toast_ok("User created");
            return load_users(ctx);
        }
        Message::Created(Err(e)) => {
            if let Some(Dialog::Add { busy, .. }) = state.dialog.as_mut() {
                *busy = false;
            }
            ctx.toast_err(e.message);
        }
        Message::Deleted(Ok(_)) => return load_users(ctx),
        Message::Deleted(Err(e)) => ctx.toast_err(e.message),
        Message::GoInvites => ctx.navigate(Page::Settings(Section::Invites)),
        Message::CloseDialog => state.dialog = None,
    }
    Task::none()
}

const COL_ROLE: f32 = 120.0;
const COL_REQ: f32 = 72.0;
const COL_LIMIT: f32 = 90.0;
const COL_AUTO: f32 = 96.0;
const COL_SEEN: f32 = 100.0;

fn user_row<'a>(state: &'a State, u: &'a Value, ctx: &Ctx<'a>) -> El<'a, Message> {
    let p = ctx.look;
    let id = fmt::s(u, "id");
    let is_me = fmt::i(u, "id") == ctx.session.user_id();
    let role = fmt::s(u, "role");
    let disabled = fmt::truthy(u, "disabled");
    let name = fmt::s(u, "username");
    let mut meta: Vec<String> = Vec::new();
    if fmt::truthy(u, "email") {
        meta.push(fmt::s(u, "email"));
    }
    if fmt::truthy(u, "invite_code") {
        meta.push(format!("invite {}", fmt::s(u, "invite_code")));
    }
    meta.push(format!("joined {}", fmt::ago(fmt::f(u, "created_at"))));
    let mut name_row = row![t(name.clone()).size(14).font(theme::semibold())].spacing(6).align_y(Alignment::Center);
    if is_me {
        name_row = name_row.push(badge("you", Badge::Plain));
    }
    if disabled {
        name_row = name_row.push(badge("disabled", Badge::Err));
    }
    let user_cell = row![
        crate::widgets::sidebar::user_avatar(u, |p| ctx.picture(p), 32.0),
        column![name_row, widgets::faint(meta.join(" · ")).size(12)].spacing(2).width(Length::Fill),
    ]
    .spacing(10)
    .align_y(Alignment::Center)
    .width(Length::Fill);
    let role_cell: El<'a, Message> = if role == "owner" {
        badge("Owner", Badge::Accent)
    } else if is_me {
        t(if role == "admin" { "Admin" } else { "User" }).size(13).color(p.muted).into()
    } else {
        let id2 = id.clone();
        select_table(&ROLES, &role, move |r| Message::Role(id2.clone(), r))
    };
    let limit = state.limits.get(&id).cloned().unwrap_or_default();
    let (id_a, id_b, id_c) = (id.clone(), id.clone(), id.clone());
    let limit_cell = input("∞", &limit, move |s| Message::LimitInput(id_a.clone(), s)).on_submit(Message::LimitCommit(id_b)).width(COL_LIMIT - 8.0);
    let auto_cell = iced::widget::toggler(fmt::truthy(u, "auto_approve"))
        .on_toggle(move |on| Message::AutoApprove(id_c.clone(), on))
        .style(theme::switch);
    let mut actions = row![].spacing(4).align_y(Alignment::Center);
    if !matches!(role.as_str(), "admin" | "owner") {
        actions = actions.push(icon_btn(Icon::Settings, "Permissions", Some(Message::OpenPerms(id.clone(), name.clone()))));
    }
    actions = actions.push(icon_btn(Icon::Shield, "Reset password", Some(Message::OpenPassword(id.clone(), name.clone()))));
    if !is_me {
        actions = actions.push(btn_sm(if disabled { "Enable" } else { "Disable" }, Btn::Default, Some(Message::Disabled(id.clone(), !disabled))));
        actions = actions.push(icon_btn(Icon::Trash, "Delete", Some(Message::AskDelete(id, name))));
    }
    let r = row![
        user_cell,
        container(role_cell).width(COL_ROLE),
        container(t(fmt::s(u, "request_count")).size(13)).width(COL_REQ),
        container(limit_cell).width(COL_LIMIT),
        container(auto_cell).width(COL_AUTO),
        container(small_muted(fmt::ago(fmt::f(u, "last_login")))).width(COL_SEEN),
        actions,
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    column![container(r).padding([10, 14]), widgets::divider()].into()
}

fn header<'a>() -> El<'a, Message> {
    let h = |s: &'a str| small_muted(s).font(theme::semibold());
    container(
        row![
            container(h("User")).width(Length::Fill),
            container(h("Role")).width(COL_ROLE),
            container(h("Requests")).width(COL_REQ),
            container(h("Weekly limit")).width(COL_LIMIT),
            container(h("Auto-approve")).width(COL_AUTO),
            container(h("Last seen")).width(COL_SEEN),
            container(space()).width(150),
        ]
        .spacing(10),
    )
    .padding([8, 14])
    .into()
}

pub fn view<'a>(state: &'a State, ctx: &Ctx<'a>) -> El<'a, Message> {
    if !state.error.is_empty() {
        return widgets::error_box(&state.error);
    }
    let head = widgets::page_head(
        "Users",
        Some(help("Manage who can access your library and what they can request.").into()),
        vec![
            btn_icon(Icon::Ticket, "Invites", Btn::Default, Some(Message::GoInvites)).into(),
            btn_icon(Icon::Plus, "Add user", Btn::Primary, Some(Message::OpenAdd)).into(),
        ],
    );
    let table: El<'a, Message> = match &state.users {
        None => widgets::skeleton_rows(4),
        Some(list) => {
            let mut c = column![header(), widgets::divider()];
            for u in list {
                c = c.push(user_row(state, u, ctx));
            }
            c.into()
        }
    };
    // Permissions for everyone
    let bulk: El<'a, Message> = match &state.perms {
        None => widgets::skeleton_rows(2),
        Some(d) => {
            let mut c = column![];
            for p in fmt::items(&d["perms"]) {
                let key = fmt::s(&p, "key");
                let (k1, k2) = (key.clone(), key.clone());
                c = c.push(widgets::list_row(
                    None,
                    fmt::s(&p, "label"),
                    format!("{} of {} user(s) have it", fmt::s(&p, "on"), fmt::s(&p, "of")),
                    vec![
                        btn_sm("All on", Btn::Default, Some(Message::Bulk(k1, true))).into(),
                        btn_sm("All off", Btn::Default, Some(Message::Bulk(k2, false))).into(),
                    ],
                    None::<Message>,
                ));
            }
            c.into()
        }
    };
    // New members start with
    let defaults = new_defaults(state);
    column![
        head,
        widgets::card_flat(table).width(Length::Fill),
        widgets::section_head("Permissions for everyone", Some(help("applies to every non-admin user; fine-tune one user with the gear in their row").into())),
        widgets::card_flat(bulk).width(Length::Fill),
        widgets::section_head("New members start with", Some(help("copied onto every new account (sign-up, invite or added here); change one person later with their gear").into())),
        widgets::card(defaults).width(Length::Fill),
    ]
    .spacing(16)
    .width(Length::Fill)
    .into()
}

/// "New members start with": a switch per default permission and the weekly request limit. A value an
/// environment variable sets is shown locked, with the same note as the Server pages' fields.
fn new_defaults<'a>(state: &'a State) -> iced::widget::Column<'a, Message> {
    let mut defaults = column![].spacing(10);
    if state.settings.is_null() {
        return defaults;
    }
    let v = &state.settings;
    for (k, l) in NEW {
        let on = fmt::truthy(v, k);
        defaults = match env_note(v, k) {
            Some(note) => defaults.push(column![switch_maybe(on, l, None::<fn(bool) -> Message>), help(note)].spacing(4)),
            None => defaults.push(switch(on, l, move |on| Message::NewDefault(k, on))),
        };
    }
    let limit: El<'a, Message> = match env_note(v, "default_request_limit") {
        Some(note) => column![input_locked(&state.default_limit).width(120), help(note)].spacing(4).into(),
        None => input("0", &state.default_limit, Message::DefaultLimitInput).on_submit(Message::DefaultLimitCommit).width(120).into(),
    };
    defaults.push(widgets::field("Weekly request limit (0 = unlimited)", limit))
}

pub fn overlay<'a>(state: &'a State, _ctx: &Ctx<'a>) -> Option<El<'a, Message>> {
    let d = state.dialog.as_ref()?;
    Some(match d {
        Dialog::Add { username, email, password, role, busy } => modal(
            "Add user",
            column![
                widgets::field("Username", input("", username, Message::AddUsername).width(Length::Fill)),
                widgets::field("Email (optional)", input("", email, Message::AddEmail).width(Length::Fill)),
                widgets::field("Password", input("", password, Message::AddPassword).secure(true).on_submit(Message::AddSubmit).width(Length::Fill)),
                widgets::field("Role", select_table(&ROLES, role, Message::AddRole)),
            ]
            .spacing(12),
            Some(
                row![
                    btn("Cancel", Btn::Default, Some(Message::CloseDialog)),
                    btn(if *busy { "Creating…" } else { "Create" }, Btn::Primary, (!busy).then_some(Message::AddSubmit)),
                ]
                .spacing(10)
                .into(),
            ),
            false,
            Message::CloseDialog,
        ),
        Dialog::Perms { name, perms, .. } => {
            let mut c = column![].spacing(10);
            for (key, label, on) in perms {
                let k = key.clone();
                c = c.push(switch(*on, label.as_str(), move |v| Message::PermToggle(k.clone(), v)));
            }
            modal(
                format!("Permissions – {name}"),
                c,
                Some(
                    row![
                        btn("Cancel", Btn::Default, Some(Message::CloseDialog)),
                        btn("Save", Btn::Primary, Some(Message::SavePerms)),
                    ]
                    .spacing(10)
                    .into(),
                ),
                false,
                Message::CloseDialog,
            )
        }
        Dialog::Password { name, password, .. } => modal(
            format!("Reset password for {name}"),
            column![
                widgets::field("New password", input("", password, Message::PasswordInput).secure(true).on_submit(Message::SavePassword).width(Length::Fill)),
                help("All of their sessions will be signed out."),
            ]
            .spacing(10),
            Some(
                row![
                    btn("Cancel", Btn::Default, Some(Message::CloseDialog)),
                    btn("Set password", Btn::Primary, (password.chars().count() >= 8).then_some(Message::SavePassword)),
                ]
                .spacing(10)
                .into(),
            ),
            false,
            Message::CloseDialog,
        ),
        Dialog::Delete { name, .. } => {
            let text = format!("Delete {name}? Their requests stay in history.");
            modal(
                "Delete user",
                widgets::muted(text).size(14),
                Some(
                    row![
                        btn("Cancel", Btn::Default, Some(Message::CloseDialog)),
                        btn("Delete", Btn::Danger, Some(Message::Delete)),
                    ]
                    .spacing(10)
                    .into(),
                ),
                false,
                Message::CloseDialog,
            )
        }
    })
}

#[cfg(test)]
fn role_options() -> Vec<super::Opt> {
    ROLES.iter().map(|(k, l)| super::Opt::new(*k, *l)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_set_by_the_environment_are_locked() {
        let v = json!({ "new_user_pick": true, "new_user_fix": false, "default_request_limit": 5,
            "_meta": { "new_user_pick": { "source": "env", "env": "ENCORE_NEW_USER_PICK" }, "new_user_fix": { "source": "ui", "env": "ENCORE_NEW_USER_FIX" },
                       "default_request_limit": { "source": "env", "env": "ENCORE_REQUEST_LIMIT" } } });
        assert_eq!(env_note(&v, "new_user_pick").as_deref(), Some("🔒 Set by ENCORE_NEW_USER_PICK in your compose/env – change it there (environment always wins)"));
        assert!(env_note(&v, "new_user_fix").is_none(), "set in the UI: no note");
        assert!(env_note(&v, "default_request_limit").is_some_and(|n| n.contains("ENCORE_REQUEST_LIMIT")));
        super::super::with_test_ctx(|ctx| {
            let mut state = State::default();
            let _ = update(&mut state, Message::Settings(Ok(v.clone())), ctx);
            let _ = new_defaults(&state);
            // a locked switch and the locked limit send nothing; an editable switch is saved
            assert_eq!(update(&mut state, Message::NewDefault("new_user_pick", false), ctx).units(), 0);
            assert_eq!(state.settings["new_user_pick"], true);
            state.default_limit = "9".into();
            assert_eq!(update(&mut state, Message::DefaultLimitCommit, ctx).units(), 0);
            assert_eq!(update(&mut state, Message::NewDefault("new_user_fix", true), ctx).units(), 1);
        });
    }

    #[test]
    fn numbers_parse_like_javascript() {
        assert_eq!(parse_int("12"), 12);
        assert_eq!(parse_int(" 7 songs"), 7);
        assert_eq!(parse_int(""), 0);
        assert_eq!(parse_int("-3"), -3);
        assert_eq!(parse_int("abc"), 0);
        assert_eq!(role_options().len(), 2);
        assert_eq!(NEW.len(), 6);
    }
}
