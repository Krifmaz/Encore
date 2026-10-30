//! Settings → Invites. Behaviour reference: web-rs/src/pages/admin.rs `render_invites`: the sign-up
//! mode, the invites table (code, uses, role, expires, used by, Copy link, delete) and New invite.

use super::{env_locked, env_note, help, input, js_plus, select_table};
use crate::api::ApiError;
use crate::fmt;
use crate::pages::{Ctx, El};
use crate::theme::{self, Badge, Btn};
use crate::widgets::{self, badge, btn, btn_icon, icon_btn, modal, small_muted, t, tabs_maybe, Icon};
use iced::widget::{column, container, row};
use iced::{Alignment, Length, Task};
use serde_json::{json, Value};

const MODES: [(&str, &str); 3] = [("invite", "Invite only"), ("open", "Open"), ("closed", "Closed")];
const ROLES: [(&str, &str); 2] = [("user", "User"), ("admin", "Admin")];

#[derive(Debug)]
pub struct NewInvite {
    max_uses: String,
    expires_days: String,
    role: String,
    note: String,
    busy: bool,
    /// The invite the server made, once created.
    made: Option<Value>,
}

#[derive(Debug, Default)]
pub struct State {
    invites: Option<Vec<Value>>,
    settings: Value,
    dialog: Option<NewInvite>,
    error: String,
}

#[derive(Clone, Debug)]
pub enum Message {
    Invites(Result<Value, ApiError>),
    Settings(Result<Value, ApiError>),
    Mode(String),
    ModeSaved(String, Result<Value, ApiError>),
    Copy(String),
    Delete(String),
    Deleted(Result<Value, ApiError>),
    OpenNew,
    MaxUses(String),
    ExpiresDays(String),
    Role(String),
    Note(String),
    Create,
    Created(Result<Value, ApiError>),
    CloseDialog,
}

fn load(ctx: &Ctx) -> Task<Message> {
    let api = ctx.api.clone();
    Task::perform(async move { api.get("/admin/invites").await }, Message::Invites)
}

pub fn open(state: &mut State, ctx: &mut Ctx) -> Task<Message> {
    state.invites = None;
    state.dialog = None;
    state.error.clear();
    let api = ctx.api.clone();
    Task::batch([load(ctx), Task::perform(async move { api.get("/admin/settings").await }, Message::Settings)])
}

/// `<base>/#/signup?invite=<code>`: the link that signs a friend up with the invite filled in.
pub fn invite_link(base: &str, code: &str) -> String {
    format!("{}/#/signup?invite={}", base.trim_end_matches('/'), crate::pages::urlencode(code))
}

fn base(state: &State, ctx: &Ctx) -> String {
    let b = fmt::s(&state.settings, "base_url");
    if b.is_empty() { ctx.base() } else { b }
}

/// Used up or past its date.
pub fn dead(i: &Value, now: f64) -> bool {
    fmt::f(i, "uses") >= fmt::f(i, "max_uses") || (fmt::truthy(i, "expires_at") && fmt::f(i, "expires_at") < now)
}

pub fn update(state: &mut State, msg: Message, ctx: &mut Ctx) -> Task<Message> {
    match msg {
        Message::Invites(Ok(v)) => state.invites = Some(fmt::items(&v)),
        Message::Invites(Err(e)) => state.error = e.message,
        Message::Settings(Ok(v)) => state.settings = v,
        Message::Settings(Err(_)) => {}
        // set by an environment variable: never sent (the environment would win at the next start)
        Message::Mode(_) if env_locked(&state.settings, "signup_mode") => {}
        Message::Mode(m) => {
            let api = ctx.api.clone();
            let body = json!({ "signup_mode": m });
            return Task::perform(async move { api.put("/admin/settings", body).await }, move |r| Message::ModeSaved(m, r));
        }
        Message::ModeSaved(m, Ok(_)) => {
            let label = MODES.iter().find(|(k, _)| *k == m).map(|(_, l)| *l).unwrap_or("");
            state.settings["signup_mode"] = json!(m);
            ctx.toast_ok(format!("Sign-ups: {label}"));
        }
        Message::ModeSaved(_, Err(e)) => ctx.toast_err(e.message),
        Message::Copy(code) => {
            ctx.copy(invite_link(&base(state, ctx), &code));
            ctx.toast_ok("Copied to clipboard");
        }
        Message::Delete(id) => {
            let api = ctx.api.clone();
            return Task::perform(async move { api.del(&format!("/admin/invites/{id}")).await }, Message::Deleted);
        }
        Message::Deleted(Ok(_)) => return load(ctx),
        Message::Deleted(Err(e)) => ctx.toast_err(e.message),
        Message::OpenNew => {
            state.dialog = Some(NewInvite {
                max_uses: "1".into(),
                expires_days: "7".into(),
                role: "user".into(),
                note: String::new(),
                busy: false,
                made: None,
            })
        }
        Message::MaxUses(s) => {
            if let Some(d) = state.dialog.as_mut() {
                d.max_uses = s;
            }
        }
        Message::ExpiresDays(s) => {
            if let Some(d) = state.dialog.as_mut() {
                d.expires_days = s;
            }
        }
        Message::Role(r) => {
            if let Some(d) = state.dialog.as_mut() {
                d.role = r;
            }
        }
        Message::Note(s) => {
            if let Some(d) = state.dialog.as_mut() {
                d.note = s;
            }
        }
        Message::Create => {
            if let Some(d) = state.dialog.as_mut() {
                d.busy = true;
                let max_uses = match js_plus(&d.max_uses) {
                    v if super::truthy(&v) => v,
                    _ => json!(1),
                };
                let expires_days = match js_plus(&d.expires_days) {
                    v if super::truthy(&v) => v,
                    _ => Value::Null,
                };
                let note = if d.note.trim().is_empty() { Value::Null } else { json!(d.note.trim()) };
                let body = json!({ "max_uses": max_uses, "expires_days": expires_days, "role": d.role, "note": note });
                let api = ctx.api.clone();
                return Task::perform(async move { api.post("/admin/invites", body).await }, Message::Created);
            }
        }
        Message::Created(Ok(inv)) => {
            if let Some(d) = state.dialog.as_mut() {
                d.busy = false;
                d.made = Some(inv);
            }
            return load(ctx);
        }
        Message::Created(Err(e)) => {
            if let Some(d) = state.dialog.as_mut() {
                d.busy = false;
            }
            ctx.toast_err(e.message);
        }
        Message::CloseDialog => state.dialog = None,
    }
    Task::none()
}

const COL_USES: f32 = 70.0;
const COL_ROLE: f32 = 80.0;
const COL_EXP: f32 = 110.0;
const COL_BY: f32 = 150.0;

fn invite_row<'a>(i: &'a Value, now: f64, p: crate::theme::Palette) -> El<'a, Message> {
    let is_dead = dead(i, now);
    let code = fmt::s(i, "code");
    let mut code_col = column![t(code.clone()).size(14).font(theme::mono()).color(if is_dead { p.faint } else { p.text })].spacing(2);
    if fmt::truthy(i, "note") {
        code_col = code_col.push(widgets::faint(fmt::s(i, "note")).size(12));
    }
    let role: El<'a, Message> = if matches!(fmt::s(i, "role").as_str(), "admin" | "owner") { badge("admin", Badge::Warn) } else { t("user").size(13).into() };
    let expires = if fmt::truthy(i, "expires_at") {
        if fmt::f(i, "expires_at") < now { "expired".to_string() } else { fmt::date(fmt::f(i, "expires_at")) }
    } else {
        "never".to_string()
    };
    let used_by = i["used_by"].as_str().filter(|s| !s.is_empty()).unwrap_or("—").to_string();
    let mut actions = row![].spacing(4).align_y(Alignment::Center);
    if !is_dead {
        actions = actions.push(btn_icon(Icon::Copy, "Copy link", Btn::Default, Some(Message::Copy(code))));
    }
    actions = actions.push(icon_btn(Icon::Trash, "Delete", Some(Message::Delete(fmt::s(i, "id")))));
    let r = row![
        container(code_col).width(Length::Fill),
        container(t(format!("{} / {}", fmt::s(i, "uses"), fmt::s(i, "max_uses"))).size(13)).width(COL_USES),
        container(role).width(COL_ROLE),
        container(small_muted(expires)).width(COL_EXP),
        container(small_muted(used_by)).width(COL_BY),
        actions,
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    column![container(r).padding([10, 14]), widgets::divider()].into()
}

/// The Sign-up mode card's content: its title and the three modes, locked (with the Server pages' note) when an
/// environment variable sets the mode.
fn signup_mode<'a>(settings: &Value) -> El<'a, Message> {
    let locked = env_locked(settings, "signup_mode");
    let mut title = column![t("Sign-up mode").size(14).font(theme::bold()), help("Who is allowed to create an account on the login page.")].spacing(2);
    if let Some(note) = env_note(settings, "signup_mode") {
        title = title.push(help(note));
    }
    super::between(title.into(), tabs_maybe(&MODES, &fmt::s(settings, "signup_mode"), (!locked).then_some(Message::Mode)))
}

pub fn view<'a>(state: &'a State, ctx: &Ctx<'a>) -> El<'a, Message> {
    if !state.error.is_empty() {
        return widgets::error_box(&state.error);
    }
    let head = widgets::page_head(
        "Invites",
        Some(help("Share an invite link so friends can create an account.").into()),
        vec![btn_icon(Icon::Plus, "New invite", Btn::Primary, Some(Message::OpenNew)).into()],
    );
    let mode = widgets::card(signup_mode(&state.settings)).width(Length::Fill);
    let now = super::now_s();
    let table: El<'a, Message> = match &state.invites {
        None => widgets::skeleton_rows(3),
        Some(list) if list.is_empty() => widgets::empty(Icon::Ticket, "No invites yet", "Create one and send the link to a friend."),
        Some(list) => {
            let h = |s: &'a str| small_muted(s).font(theme::semibold());
            let mut c = column![
                container(
                    row![
                        container(h("Code")).width(Length::Fill),
                        container(h("Uses")).width(COL_USES),
                        container(h("Role")).width(COL_ROLE),
                        container(h("Expires")).width(COL_EXP),
                        container(h("Used by")).width(COL_BY),
                        container(iced::widget::space()).width(130),
                    ]
                    .spacing(10)
                )
                .padding([8, 14]),
                widgets::divider()
            ];
            for i in list {
                c = c.push(invite_row(i, now, ctx.look));
            }
            c.into()
        }
    };
    column![head, mode, widgets::card_flat(table).width(Length::Fill)].spacing(16).width(Length::Fill).into()
}

pub fn overlay<'a>(state: &'a State, ctx: &Ctx<'a>) -> Option<El<'a, Message>> {
    let d = state.dialog.as_ref()?;
    let body: El<'a, Message> = match &d.made {
        None => column![
            super::two(
                widgets::field("Max uses", input("1", &d.max_uses, Message::MaxUses).width(Length::Fill)),
                Some(column![
                    widgets::field("Expires after (days)", input("7", &d.expires_days, Message::ExpiresDays).width(Length::Fill)),
                    help("0 = never"),
                ]
                .spacing(4)
                .into()),
            ),
            widgets::field("Role", select_table(&ROLES, &d.role, Message::Role)),
            widgets::field("Note", input("e.g. for Sam", &d.note, Message::Note).width(Length::Fill)),
        ]
        .spacing(12)
        .into(),
        Some(inv) => {
            let code = fmt::s(inv, "code");
            let link = invite_link(&base(state, ctx), &code);
            column![
                widgets::muted("Send this link – it signs them up with the invite filled in:").size(14),
                container(t(link).size(13).font(theme::mono())).padding(10).width(Length::Fill).style(theme::panel2),
                row![
                    btn_icon(Icon::Copy, "Copy link", Btn::Primary, Some(Message::Copy(code.clone()))),
                    widgets::muted("Code:").size(14),
                    t(code).size(14).font(theme::mono()),
                ]
                .spacing(10)
                .align_y(Alignment::Center),
            ]
            .spacing(12)
            .into()
        }
    };
    let footer = if d.made.is_some() {
        row![btn("Close", Btn::Default, Some(Message::CloseDialog))]
    } else {
        row![
            btn("Close", Btn::Default, Some(Message::CloseDialog)),
            btn(if d.busy { "Creating…" } else { "Create invite" }, Btn::Primary, (!d.busy).then_some(Message::Create)),
        ]
    }
    .spacing(10);
    Some(modal("New invite", body, Some(footer.into()), false, Message::CloseDialog))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mode_set_by_the_environment_is_locked() {
        let s = json!({ "signup_mode": "open", "_meta": { "signup_mode": { "source": "env", "env": "ENCORE_SIGNUP_MODE" } } });
        assert!(env_locked(&s, "signup_mode"));
        assert_eq!(env_note(&s, "signup_mode").as_deref(), Some("🔒 Set by ENCORE_SIGNUP_MODE in your compose/env – change it there (environment always wins)"));
        let _ = signup_mode(&s);
        let ui = json!({ "signup_mode": "invite", "_meta": { "signup_mode": { "source": "ui", "env": "ENCORE_SIGNUP_MODE" } } });
        assert!(!env_locked(&ui, "signup_mode") && env_note(&ui, "signup_mode").is_none());
        assert!(!env_locked(&json!({ "signup_mode": "invite" }), "signup_mode"), "without _meta: editable");
        // a press on a locked mode sends nothing; an editable one is saved
        super::super::with_test_ctx(|ctx| {
            let mut state = State { settings: s.clone(), ..Default::default() };
            assert_eq!(update(&mut state, Message::Mode("closed".into()), ctx).units(), 0, "nothing was sent");
            assert_eq!(state.settings["signup_mode"], "open");
            let mut state = State { settings: ui.clone(), ..Default::default() };
            assert_eq!(update(&mut state, Message::Mode("closed".into()), ctx).units(), 1);
        });
    }

    #[test]
    fn links_and_liveness() {
        assert_eq!(invite_link("http://e:8080/", "AB-CD"), "http://e:8080/#/signup?invite=AB%2DCD");
        let now = 1000.0;
        assert!(dead(&json!({ "uses": 1, "max_uses": 1 }), now));
        assert!(dead(&json!({ "uses": 0, "max_uses": 1, "expires_at": 900 }), now));
        assert!(!dead(&json!({ "uses": 0, "max_uses": 1, "expires_at": 1100 }), now));
    }
}
