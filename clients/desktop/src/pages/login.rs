//! The first page: pick a server (remembered ones, or type an address), sign in, and the
//! "Can't reach" state with Retry. Behaviour reference: web-rs/src/pages/auth.rs (the sign-in,
//! sign-up, forgot password and email sign-in link forms and their words) and the server picker of
//! the classic app (removed in 0.29). Only the pages an email links to (`#/reset`,
//! `#/magic`) stay in the browser.

use crate::api::ApiError;
use crate::fmt;
use crate::config::{normalize, Config};
use crate::pages::{Ctx, El, Page};
use crate::theme::{self, Badge, Btn};
use crate::widgets::{self, badge, btn, error_box, field, icon, icon_btn, logo, muted, small_muted, t, Icon};
use iced::widget::{button, column, container, row, scrollable, space, text_input};
use iced::{Alignment, Length, Subscription, Task};
use serde_json::{json, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Step {
    /// The server picker.
    #[default]
    Pick,
    /// The sign-in form for `server`.
    SignIn,
    /// `server` did not answer.
    Unreachable,
    /// Forgot password: `POST /auth/forgot {login}`.
    Forgot,
    /// Email me a sign-in link: `POST /auth/email-link {login}`.
    EmailLink,
    /// Create an account (with an invite when the server asks for one): `POST /auth/signup`.
    Signup,
}

#[derive(Debug, Default)]
pub struct State {
    pub step: Step,
    /// Remembered servers: (url, username).
    pub servers: Vec<(String, String)>,
    /// The address being typed.
    pub server_input: String,
    /// The server the form signs in to.
    pub server: String,
    /// Its `/api/auth/info` (app name, signup mode, email login).
    pub info: Value,
    pub username: String,
    pub password: String,
    /// The authenticator code field, shown after the server asked for it.
    pub code: Option<String>,
    /// "Username or email" of the Forgot password and sign-in link forms.
    pub login_key: String,
    /// The sign-up form's other fields.
    pub email: String,
    pub password2: String,
    pub invite: String,
    /// The server's answer after a reset or sign-in link was sent.
    pub notice: String,
    pub busy: bool,
    pub error: String,
    /// Why the server could not be reached.
    pub problem: String,
}

#[derive(Clone, Debug)]
pub enum Message {
    ServerInput(String),
    /// Try the typed address.
    Connect,
    /// Try a remembered server.
    Pick(String),
    Forget(String),
    /// `/api/auth/info` of `server` came back.
    Probed(String, Result<Value, ApiError>),
    Username(String),
    Password(String),
    Code(String),
    Submit,
    Done(Result<Value, ApiError>),
    ChangeServer,
    Retry,
    Offline,
    /// Show one of the forms (Forgot password, Email me a sign-in link, Sign up) or go back to
    /// the sign-in form.
    Show(Step),
    LoginKey(String),
    /// Send the reset link or the sign-in link (by the step that is shown).
    SendKey,
    Sent(Result<Value, ApiError>),
    Email(String),
    Password2(String),
    Invite(String),
    CreateAccount,
}

/// The state for the saved config: the sign-in form for the current server, or the picker.
pub fn new(config: &Config) -> State {
    let mut s = State {
        servers: servers_of(config),
        ..State::default()
    };
    if config.server.is_empty() {
        s.step = Step::Pick;
    } else {
        s.step = Step::SignIn;
        s.server = config.server.clone();
        s.username = config.current().map(|c| c.username.clone()).unwrap_or_default();
    }
    s
}

fn servers_of(config: &Config) -> Vec<(String, String)> {
    config
        .servers
        .iter()
        .map(|s| (s.url.clone(), s.username.clone()))
        .collect()
}

/// Show the "Can't reach" state for `server`.
pub fn unreachable(state: &mut State, server: &str, problem: &str) {
    state.step = Step::Unreachable;
    state.server = server.to_string();
    state.problem = problem.to_string();
    state.busy = false;
}

/// Show the sign-in form for `server` (the saved session ended).
pub fn sign_in(state: &mut State, server: &str, info: Value) {
    state.step = Step::SignIn;
    state.server = server.to_string();
    state.info = info;
    state.password.clear();
    state.code = None;
    state.error.clear();
    state.busy = false;
}

/// Fill the form (the screenshot tour's `--login user:pass`).
pub fn prefill(state: &mut State, username: &str, password: &str) {
    state.username = username.to_string();
    state.password = password.to_string();
}

pub fn open(state: &mut State, ctx: &mut Ctx) -> Task<Message> {
    state.servers = servers_of(ctx.config);
    Task::none()
}

fn probe(state: &mut State, url: String, ctx: &mut Ctx) -> Task<Message> {
    let url = normalize(&url);
    if url == "http://" || url == "https://" {
        state.error = "Type the server's address".into();
        return Task::none();
    }
    state.busy = true;
    state.error.clear();
    ctx.api.set_base(&url);
    if let Some(saved) = ctx.config.servers.iter().find(|s| s.url == url) {
        ctx.api.set_cookie(&saved.cookie);
        if state.username.is_empty() {
            state.username = saved.username.clone();
        }
    }
    let api = ctx.api.clone();
    Task::perform(async move { api.info().await }, move |r| Message::Probed(url, r))
}

pub fn update(state: &mut State, msg: Message, ctx: &mut Ctx) -> Task<Message> {
    match msg {
        Message::ServerInput(s) => {
            state.server_input = s;
            state.error.clear();
        }
        Message::Connect => {
            let url = state.server_input.clone();
            return probe(state, url, ctx);
        }
        Message::Pick(url) => return probe(state, url, ctx),
        Message::Forget(url) => {
            state.servers.retain(|(u, _)| *u != url);
            // A kept app token is a session on that server: it is ended there too.
            let token = ctx.config.servers.iter().find(|s| s.url == url).map(|s| s.token.clone()).unwrap_or_default();
            let u = url.clone();
            ctx.edit_config(move |c| c.forget(&u));
            if !token.is_empty() {
                let api = ctx.api.clone();
                return Task::future(async move { api.revoke_app_token(&url, &token).await }).discard();
            }
        }
        Message::Probed(url, Ok(info)) => {
            state.busy = false;
            state.server = url;
            state.info = info;
            state.error.clear();
            state.step = Step::SignIn;
        }
        Message::Probed(url, Err(e)) => {
            state.busy = false;
            if e.unreachable() {
                unreachable(state, &url, &e.message);
            } else {
                state.step = Step::Pick;
                state.error = e.message;
            }
        }
        Message::Username(s) => state.username = s,
        Message::Password(s) => state.password = s,
        Message::Code(s) => state.code = Some(s),
        Message::Submit => {
            if state.busy {
                return Task::none();
            }
            if state.username.trim().is_empty() || state.password.is_empty() {
                state.error = "Enter your username and password".into();
                return Task::none();
            }
            state.busy = true;
            state.error.clear();
            ctx.api.set_base(&state.server);
            let api = ctx.api.clone();
            let (u, p, c) = (state.username.clone(), state.password.clone(), state.code.clone());
            return Task::perform(
                async move { api.login(&u, &p, c.as_deref()).await },
                Message::Done,
            );
        }
        Message::Done(Ok(user)) => {
            state.busy = false;
            state.password.clear();
            state.code = None;
            ctx.signed_in(user);
        }
        Message::Done(Err(e)) => {
            state.busy = false;
            if e.unreachable() {
                let server = state.server.clone();
                unreachable(state, &server, &e.message);
            } else if e.flag("two_factor") && state.code.is_none() {
                // two-step sign-in: ask for the authenticator code, keep what was typed
                state.code = Some(String::new());
            } else {
                state.error = e.message;
            }
        }
        Message::ChangeServer => {
            state.step = Step::Pick;
            state.error.clear();
            state.server_input = state.server.clone();
        }
        Message::Retry => {
            let url = state.server.clone();
            return probe(state, url, ctx);
        }
        Message::Offline => ctx.navigate(Page::Offline),
        Message::Show(step) => {
            if step == Step::Signup && state.step != Step::Signup {
                state.password.clear();
                state.password2.clear();
            }
            state.step = step;
            state.error.clear();
            state.notice.clear();
            state.busy = false;
        }
        Message::LoginKey(s) => state.login_key = s,
        Message::SendKey => {
            if state.busy {
                return Task::none();
            }
            if state.login_key.trim().is_empty() {
                state.error = "Enter your username or email".into();
                return Task::none();
            }
            let path = if state.step == Step::EmailLink { "/auth/email-link" } else { "/auth/forgot" };
            state.busy = true;
            state.error.clear();
            ctx.api.set_base(&state.server);
            let api = ctx.api.clone();
            let body = json!({ "login": state.login_key.trim() });
            return Task::perform(async move { api.post(path, body).await }, Message::Sent);
        }
        Message::Sent(Ok(r)) => {
            state.busy = false;
            state.notice = fmt::s(&r, "message");
        }
        Message::Sent(Err(e)) => {
            state.busy = false;
            state.error = e.message;
        }
        Message::Email(s) => state.email = s,
        Message::Password2(s) => state.password2 = s,
        Message::Invite(s) => state.invite = s,
        Message::CreateAccount => {
            if state.busy {
                return Task::none();
            }
            if state.password != state.password2 {
                state.error = "Passwords do not match".into();
                return Task::none();
            }
            state.busy = true;
            state.error.clear();
            ctx.api.set_base(&state.server);
            let api = ctx.api.clone();
            let mut body = json!({
                "username": state.username.trim(),
                "email": state.email.trim(),
                "password": state.password,
            });
            if signup_mode(state) == "invite" {
                body["invite"] = json!(state.invite.trim());
            }
            // the answer is the new user, signed in, as after a sign-in
            return Task::perform(async move { api.post("/auth/signup", body).await }, Message::Done);
        }
    }
    Task::none()
}

/// `info.signup_mode`: "open", "invite" (the default) or "closed".
fn signup_mode(state: &State) -> &str {
    match state.info["signup_mode"].as_str() {
        Some(m) if !m.is_empty() => m,
        _ => "invite",
    }
}

/// A text field of the forms.
fn input<'a>(value: &'a str, on: fn(String) -> Message, submit: Message, secure: bool) -> El<'a, Message> {
    text_input("", value)
        .secure(secure)
        .on_input(on)
        .on_submit(submit)
        .padding([10, 12])
        .size(14)
        .style(theme::input)
        .into()
}

/// A small muted link-looking button.
fn link<'a>(label: &'a str, m: Message) -> El<'a, Message> {
    button(small_muted(label).size(12.5))
        .padding([4, 6])
        .style(theme::btn(Btn::Ghost))
        .on_press(m)
        .into()
}

/// The Forgot password and Email me a sign-in link forms (`forgot_form`, `email_link_form`).
fn key_form<'a>(state: &'a State) -> El<'a, Message> {
    let link_form = state.step == Step::EmailLink;
    let mut col = column![
        muted("Sign in to request music and manage the library")
            .size(14)
            .center()
            .width(Length::Fill),
        small_muted(state.server.clone()).center().width(Length::Fill),
    ]
    .spacing(14);
    if state.notice.is_empty() {
        col = col.push(small_muted(if link_form {
            "We'll email you a link that signs you in – no password needed."
        } else {
            "Enter your username or email. If your account has an email address, we'll send a reset link."
        }));
        col = col.push(field("Username or email", input(&state.login_key, Message::LoginKey, Message::SendKey, false)));
        col = col.push(error_box(&state.error));
        let label = match (link_form, state.busy) {
            (true, false) => "Email me a sign-in link",
            (false, false) => "Send reset link",
            (_, true) => "Sending…",
        };
        col = col.push(
            btn(label, Btn::Primary, (!state.busy).then_some(Message::SendKey))
                .width(Length::Fill)
                .padding(12),
        );
    } else {
        col = col.push(crate::pages::settings::result_line(true, state.notice.as_str()));
    }
    col = col.push(container(link("Back to sign in", Message::Show(Step::SignIn))).center_x(Length::Fill));
    col.into()
}

/// The sign-up form (`signup_form`).
fn signup_form<'a>(state: &'a State) -> El<'a, Message> {
    let mode = signup_mode(state);
    let mut col = column![
        muted("Create your account").size(14).center().width(Length::Fill),
        small_muted(state.server.clone()).center().width(Length::Fill),
    ]
    .spacing(14);
    if mode == "closed" {
        col = col.push(error_box("Sign-ups are currently closed."));
    } else {
        col = col.push(error_box(&state.error));
        if mode == "invite" {
            col = col.push(field(
                "Invite code",
                text_input("XXXX-XXXX-XXXX", &state.invite)
                    .on_input(Message::Invite)
                    .on_submit(Message::CreateAccount)
                    .padding([10, 12])
                    .size(14)
                    .font(theme::mono())
                    .style(theme::input),
            ));
        }
        col = col.push(field("Username", input(&state.username, Message::Username, Message::CreateAccount, false)));
        col = col.push(field("Email (optional)", input(&state.email, Message::Email, Message::CreateAccount, false)));
        col = col.push(field("Password", input(&state.password, Message::Password, Message::CreateAccount, true)));
        col = col.push(field(
            "Confirm password",
            input(&state.password2, Message::Password2, Message::CreateAccount, true),
        ));
        col = col.push(
            btn(
                if state.busy { "Creating…" } else { "Create account" },
                Btn::Primary,
                (!state.busy).then_some(Message::CreateAccount),
            )
            .width(Length::Fill)
            .padding(12),
        );
    }
    col = col.push(
        row![
            small_muted("Already have an account?").size(12.5),
            button(t("Sign in").size(12.5).color(theme::look().accent))
                .padding([4, 6])
                .style(theme::btn(Btn::Ghost))
                .on_press(Message::Show(Step::SignIn)),
        ]
        .align_y(Alignment::Center),
    );
    container(col).width(Length::Fill).into()
}

fn card<'a>(body: El<'a, Message>, app_name: String) -> El<'a, Message> {
    let brand = row![logo(30.0), t(app_name).size(18).font(theme::bold())]
        .spacing(10)
        .align_y(Alignment::Center);
    let inner = column![container(brand).center_x(Length::Fill), body]
        .spacing(16)
        .width(Length::Fill);
    container(
        scrollable(
            container(widgets::card(inner).padding(28).max_width(420.0))
                .width(Length::Fill)
                .center_x(Length::Fill)
                .padding([40, 20]),
        )
        .style(theme::scroll),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .center_y(Length::Fill)
    .into()
}

pub fn view<'a>(state: &'a State, ctx: &Ctx<'a>) -> El<'a, Message> {
    let p = ctx.look;
    let app_name = match state.info["app_name"].as_str() {
        Some(n) if !n.is_empty() => n.to_string(),
        _ => "Encore".to_string(),
    };
    let body: El<'a, Message> = match state.step {
        Step::Pick => {
            let mut list = column![].spacing(6);
            for (url, user) in &state.servers {
                let mut r = row![
                    icon(Icon::Devices, 18.0, p.muted),
                    column![
                        t(url.clone()).size(14).font(theme::semibold()),
                        small_muted(if user.is_empty() { "Not signed in".to_string() } else { user.clone() }),
                    ]
                    .spacing(1)
                    .width(Length::Fill),
                ]
                .spacing(12)
                .align_y(Alignment::Center);
                if !user.is_empty() {
                    r = r.push(badge("Saved", Badge::Accent));
                }
                list = list.push(
                    row![
                        button(container(r).padding([9, 12]))
                            .padding(0)
                            .width(Length::Fill)
                            .style(theme::btn(Btn::Row))
                            .on_press_maybe((!state.busy).then(|| Message::Pick(url.clone()))),
                        icon_btn(Icon::X, "Forget this server", Some(Message::Forget(url.clone()))),
                    ]
                    .spacing(4)
                    .align_y(Alignment::Center),
                );
            }
            let input = text_input("http://192.168.1.10:8080", &state.server_input)
                .on_input(Message::ServerInput)
                .on_submit(Message::Connect)
                .padding([10, 12])
                .size(14)
                .style(theme::input);
            let connect = btn(
                if state.busy { "Connecting…" } else { "Connect" },
                Btn::Primary,
                (!state.busy).then_some(Message::Connect),
            );
            let mut col = column![muted("Your Encore server").size(14).center().width(Length::Fill)].spacing(14);
            if !state.servers.is_empty() {
                col = col.push(list);
                col = col.push(small_muted("Or add another one").size(12.5));
            }
            col = col.push(field("Server address", input));
            col = col.push(error_box(&state.error));
            col = col.push(connect.width(Length::Fill));
            col = col.push(widgets::divider());
            col = col.push(
                widgets::btn_icon(Icon::Download, "Offline music", Btn::Default, Some(Message::Offline))
                    .width(Length::Fill),
            );
            col.into()
        }
        Step::SignIn => {
            let mut form = column![
                muted("Sign in to request music and manage the library")
                    .size(14)
                    .center()
                    .width(Length::Fill),
                small_muted(state.server.clone()).center().width(Length::Fill),
                error_box(&state.error),
                field(
                    "Username",
                    text_input("", &state.username)
                        .on_input(Message::Username)
                        .on_submit(Message::Submit)
                        .padding([10, 12])
                        .size(14)
                        .style(theme::input)
                ),
                field(
                    "Password",
                    text_input("", &state.password)
                        .secure(true)
                        .on_input(Message::Password)
                        .on_submit(Message::Submit)
                        .padding([10, 12])
                        .size(14)
                        .style(theme::input)
                ),
            ]
            .spacing(14);
            if let Some(code) = &state.code {
                form = form.push(field(
                    "Code from your authenticator app",
                    text_input("", code)
                        .on_input(Message::Code)
                        .on_submit(Message::Submit)
                        .padding([10, 12])
                        .size(14)
                        .style(theme::input),
                ));
            }
            form = form.push(
                btn(
                    if state.busy { "Signing in…" } else { "Sign in" },
                    Btn::Primary,
                    (!state.busy).then_some(Message::Submit),
                )
                .width(Length::Fill)
                .padding(12),
            );
            if crate::fmt::truthy(&state.info, "email_login") {
                form = form.push(
                    btn("Email me a sign-in link", Btn::Default, Some(Message::Show(Step::EmailLink)))
                        .width(Length::Fill)
                        .padding(12),
                );
            }
            let signup_mode = signup_mode(state);
            let foot_label = match signup_mode {
                "closed" => "Sign-ups are closed. Ask the admin for an account.",
                "invite" => "New here? Sign up with an invite",
                _ => "New here? Create an account",
            };
            let foot: El<'a, Message> = if signup_mode == "closed" {
                small_muted(foot_label).center().width(Length::Fill).into()
            } else {
                container(link(foot_label, Message::Show(Step::Signup)))
                    .center_x(Length::Fill)
                    .into()
            };
            form = form.push(
                row![
                    link("Forgot password?", Message::Show(Step::Forgot)),
                    space().width(Length::Fill),
                    link("Change server", Message::ChangeServer),
                ]
                .align_y(Alignment::Center),
            );
            form = form.push(foot);
            form.into()
        }
        Step::Forgot | Step::EmailLink => key_form(state),
        Step::Signup => {
            column![signup_form(state), container(link("Change server", Message::ChangeServer)).center_x(Length::Fill)]
                .spacing(6)
                .into()
        }
        Step::Unreachable => column![
            container(icon(Icon::Devices, 34.0, p.faint)).center_x(Length::Fill),
            t(format!("Can't reach {}", state.server))
                .size(16)
                .font(theme::semibold())
                .center()
                .width(Length::Fill),
            muted(state.problem.clone()).size(13.5).center().width(Length::Fill),
            btn(
                if state.busy { "Trying…" } else { "Retry" },
                Btn::Primary,
                (!state.busy).then_some(Message::Retry)
            )
            .width(Length::Fill),
            row![
                btn("Change server", Btn::Default, Some(Message::ChangeServer)).width(Length::Fill),
                btn("Offline music", Btn::Default, Some(Message::Offline)).width(Length::Fill),
            ]
            .spacing(10),
        ]
        .spacing(14)
        .into(),
    };
    card(body, app_name)
}

pub fn overlay<'a>(_state: &'a State, _ctx: &Ctx<'a>) -> Option<El<'a, Message>> {
    None
}

pub fn subscription(_state: &State) -> Subscription<Message> {
    Subscription::none()
}
