//! Settings → Account. Behaviour reference: web-rs/src/pages/account.rs. Profile and picture,
//! username, password, email, the apps card, two-step sign-in, active sessions and (for
//! members) the About card.

use super::{block, card, help, input, link, warn_text};
use crate::api::{ApiError, VERSION};
use crate::fmt;
use crate::pages::{Ctx, El};
use crate::theme::{self, Badge, Btn};
use crate::widgets::drop_zone::{self, DropZone, Outcome};
use crate::widgets::{self, badge, btn, btn_icon, btn_sm, icon_btn, modal, small_muted, t, Icon};
use iced::widget::{column, container, image, row};
use iced::{Alignment, ContentFit, Length, Task};
use serde_json::{json, Value};
use std::path::PathBuf;

/// The two-step sign-in card's inner state.
#[derive(Debug, Default)]
pub enum TwoFa {
    #[default]
    Idle,
    /// `/auth/2fa/setup` answered: the QR picture, the key and the code box.
    Setup {
        qr: Option<image::Handle>,
        secret: String,
        code: String,
    },
    /// Turning it off asks for the password.
    Off {
        password: String,
    },
}

#[derive(Clone, Copy, Debug)]
pub enum Dialog {
    LogoutAll,
    ChangeServer,
}

#[derive(Debug, Default)]
pub struct State {
    me: Value,
    sessions: Option<Vec<Value>>,
    /// The username editor, with its text, while open.
    rename: Option<String>,
    pw_open: bool,
    pw_current: String,
    pw_new: String,
    email: String,
    twofa: TwoFa,
    dialog: Option<Dialog>,
    busy: bool,
    error: String,
    /// Pictures dragged onto the window for *Change picture*.
    drop: DropZone,
}

/// What a finished request was for.
#[derive(Clone, Copy, Debug)]
pub enum Act {
    Name,
    Email,
    Password,
    Avatar,
    AvatarRemoved,
    TotpSetup,
    TotpOn,
    TotpOff,
    LogoutAll,
    Revoke,
}

#[derive(Clone, Debug)]
pub enum Message {
    Me(Result<Value, ApiError>),
    Sessions(Result<Value, ApiError>),
    EditName(bool),
    NameInput(String),
    SaveName,
    PwToggle,
    PwCurrent(String),
    PwNew(String),
    PwSubmit,
    EmailInput(String),
    EmailSubmit,
    PickAvatar,
    AvatarPicked(Option<PathBuf>),
    /// A file dragged over or dropped on the window: a dropped picture becomes the new one.
    Drop(drop_zone::Event),
    RemoveAvatar,
    TwoFaSetup,
    TwoFaCode(String),
    TwoFaEnable,
    TwoFaOffStart,
    TwoFaPassword(String),
    TwoFaDisable,
    TwoFaCancel,
    Ask(Dialog),
    CloseDialog,
    LogoutAll,
    ChangeServer,
    Revoke(String),
    OpenReleases,
    Done(Act, Result<Value, ApiError>),
}

fn load_me(ctx: &Ctx) -> Task<Message> {
    let api = ctx.api.clone();
    Task::perform(async move { api.me().await }, Message::Me)
}

fn load_sessions(ctx: &Ctx) -> Task<Message> {
    let api = ctx.api.clone();
    Task::perform(async move { api.get("/auth/sessions").await }, Message::Sessions)
}

pub fn open(state: &mut State, ctx: &mut Ctx) -> Task<Message> {
    state.me = ctx.session.user.clone();
    state.email = fmt::s(&state.me, "email");
    state.sessions = None;
    state.rename = None;
    state.pw_open = false;
    state.pw_current.clear();
    state.pw_new.clear();
    state.twofa = TwoFa::Idle;
    state.dialog = None;
    state.busy = false;
    state.error.clear();
    Task::batch([load_me(ctx), load_sessions(ctx)])
}

/// "Encore app · Pixel 9 Pro · last used 2h ago" for the native app's sign-ins, the browser otherwise.
pub fn session_title(s: &Value) -> String {
    if fmt::s(s, "kind") == "app" {
        let seen = if fmt::f(s, "last_seen_at") > 0.0 { fmt::f(s, "last_seen_at") } else { fmt::f(s, "created_at") };
        let last = format!("last used {}", fmt::ago(seen));
        ["Encore app", fmt::s(s, "device_name").as_str(), last.as_str()]
            .iter()
            .filter(|p| !p.is_empty())
            .copied()
            .collect::<Vec<_>>()
            .join(" · ")
    } else {
        let d = fmt::s(s, "device_name");
        if !d.is_empty() {
            return d;
        }
        let ua = fmt::s(s, "user_agent");
        if ua.is_empty() { "Unknown device".into() } else { ua }
    }
}

/// `data:image/png;base64,…` without the prefix: the picture's bytes.
pub fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut buf = 0u32;
    let mut bits = 0u32;
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' | b'\n' | b'\r' | b' ' => continue,
            _ => return None,
        } as u32;
        buf = (buf << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Some(out)
}

pub fn update(state: &mut State, msg: Message, ctx: &mut Ctx) -> Task<Message> {
    match msg {
        Message::Me(Ok(me)) => {
            state.email = fmt::s(&me, "email");
            state.me = me;
        }
        Message::Me(Err(e)) => state.error = e.message,
        Message::Sessions(Ok(v)) => state.sessions = Some(fmt::items(&v)),
        Message::Sessions(Err(_)) => state.sessions = Some(Vec::new()),
        Message::EditName(on) => state.rename = on.then(|| fmt::s(&state.me, "username")),
        Message::NameInput(s) => {
            if let Some(n) = state.rename.as_mut() {
                *n = s;
            }
        }
        Message::SaveName => {
            let Some(name) = state.rename.clone() else { return Task::none() };
            let api = ctx.api.clone();
            return Task::perform(
                async move { api.put("/auth/profile", json!({ "username": name.trim() })).await },
                |r| Message::Done(Act::Name, r),
            );
        }
        Message::PwToggle => {
            state.pw_open = !state.pw_open;
            state.pw_current.clear();
            state.pw_new.clear();
        }
        Message::PwCurrent(s) => state.pw_current = s,
        Message::PwNew(s) => state.pw_new = s,
        Message::PwSubmit => {
            if state.pw_new.chars().count() < 8 {
                ctx.toast_err("The new password needs at least 8 characters");
                return Task::none();
            }
            let api = ctx.api.clone();
            let body = json!({ "current": state.pw_current, "new": state.pw_new });
            return Task::perform(async move { api.post("/auth/password", body).await }, |r| Message::Done(Act::Password, r));
        }
        Message::EmailInput(s) => state.email = s,
        Message::EmailSubmit => {
            let api = ctx.api.clone();
            let body = json!({ "email": state.email });
            return Task::perform(async move { api.put("/auth/profile", body).await }, |r| Message::Done(Act::Email, r));
        }
        Message::PickAvatar => {
            return Task::perform(
                async move {
                    rfd::AsyncFileDialog::new()
                        .set_title("Change picture")
                        .add_filter("Pictures", AVATAR_FILES.exts)
                        .pick_file()
                        .await
                        .map(|f| f.path().to_path_buf())
                },
                Message::AvatarPicked,
            );
        }
        Message::Drop(ev) => {
            // only while *Change picture* can be pressed, like the button
            if state.busy || !ctx.has_feature("avatar") {
                state.drop.hovering = false;
                return Task::none();
            }
            match state.drop.handle(ev, &AVATAR_FILES) {
                Outcome::Take(path) => return Task::done(Message::AvatarPicked(Some(path))),
                Outcome::Refused(m) => ctx.toast_err(m),
                Outcome::Nothing => {}
            }
        }
        Message::AvatarPicked(None) => {}
        Message::AvatarPicked(Some(path)) => {
            state.busy = true;
            let api = ctx.api.clone();
            return Task::perform(
                async move {
                    let part = crate::api::Api::file_part(&path).await?;
                    api.post_form("/auth/avatar", reqwest::multipart::Form::new().part("file", part)).await
                },
                |r| Message::Done(Act::Avatar, r),
            );
        }
        Message::RemoveAvatar => {
            let api = ctx.api.clone();
            return Task::perform(async move { api.del("/auth/avatar").await }, |r| Message::Done(Act::AvatarRemoved, r));
        }
        Message::TwoFaSetup => {
            let api = ctx.api.clone();
            return Task::perform(async move { api.post_empty("/auth/2fa/setup").await }, |r| Message::Done(Act::TotpSetup, r));
        }
        Message::TwoFaCode(s) => {
            if let TwoFa::Setup { code, .. } = &mut state.twofa {
                *code = s;
            }
        }
        Message::TwoFaEnable => {
            if let TwoFa::Setup { code, .. } = &state.twofa {
                let api = ctx.api.clone();
                let body = json!({ "code": code.trim() });
                return Task::perform(async move { api.post("/auth/2fa/enable", body).await }, |r| Message::Done(Act::TotpOn, r));
            }
        }
        Message::TwoFaOffStart => state.twofa = TwoFa::Off { password: String::new() },
        Message::TwoFaPassword(s) => {
            if let TwoFa::Off { password } = &mut state.twofa {
                *password = s;
            }
        }
        Message::TwoFaDisable => {
            if let TwoFa::Off { password } = &state.twofa {
                let api = ctx.api.clone();
                let body = json!({ "password": password });
                return Task::perform(async move { api.post("/auth/2fa/disable", body).await }, |r| Message::Done(Act::TotpOff, r));
            }
        }
        Message::TwoFaCancel => state.twofa = TwoFa::Idle,
        Message::Ask(d) => state.dialog = Some(d),
        Message::CloseDialog => state.dialog = None,
        Message::LogoutAll => {
            state.dialog = None;
            let api = ctx.api.clone();
            return Task::perform(async move { api.post_empty("/auth/logout-all").await }, |r| Message::Done(Act::LogoutAll, r));
        }
        Message::ChangeServer => {
            state.dialog = None;
            ctx.sign_out();
        }
        Message::Revoke(sid) => {
            let api = ctx.api.clone();
            return Task::perform(async move { api.del(&format!("/auth/sessions/{sid}")).await }, |r| Message::Done(Act::Revoke, r));
        }
        Message::OpenReleases => ctx.open_url("https://github.com/Krifmaz/Encore/releases"),
        Message::Done(act, Err(e)) => {
            state.busy = false;
            if matches!(act, Act::Avatar) && e.message.is_empty() {
                ctx.toast_err("Upload failed");
            } else {
                ctx.toast_err(e.message);
            }
        }
        Message::Done(act, Ok(r)) => {
            state.busy = false;
            match act {
                Act::Name => {
                    ctx.toast_ok(format!("You're now {}", fmt::s(&r, "username")));
                    state.rename = None;
                    ctx.refresh_session();
                    return load_me(ctx);
                }
                Act::Email => ctx.toast_ok("Email saved"),
                Act::Password => {
                    ctx.toast_ok("Password updated – other sessions signed out");
                    state.pw_open = false;
                    state.pw_current.clear();
                    state.pw_new.clear();
                    return load_sessions(ctx);
                }
                Act::Avatar => {
                    ctx.toast_ok("Picture updated");
                    ctx.forget_cover(avatar_url(&ctx.base(), &state.me));
                    ctx.refresh_session();
                    return load_me(ctx);
                }
                Act::AvatarRemoved => {
                    ctx.refresh_session();
                    return load_me(ctx);
                }
                Act::TotpSetup => {
                    let qr = r["qr"].as_str().and_then(base64_decode).map(image::Handle::from_bytes);
                    state.twofa = TwoFa::Setup {
                        qr,
                        secret: fmt::s(&r, "secret"),
                        code: String::new(),
                    };
                }
                Act::TotpOn => {
                    ctx.toast_ok("Two-step sign-in is on");
                    state.twofa = TwoFa::Idle;
                    ctx.refresh_session();
                    return load_me(ctx);
                }
                Act::TotpOff => {
                    ctx.toast_ok("Two-step sign-in is off");
                    state.twofa = TwoFa::Idle;
                    ctx.refresh_session();
                    return load_me(ctx);
                }
                Act::LogoutAll => ctx.sign_out(),
                Act::Revoke => return load_sessions(ctx),
            }
        }
    }
    Task::none()
}

/// The pictures *Change picture* takes, picked or dropped.
const AVATAR_FILES: drop_zone::Accept = drop_zone::Accept::one(&["png", "jpg", "jpeg", "webp", "gif"]);

/// The window's drag and drop, for *Change picture*.
pub fn subscription(_state: &State) -> iced::Subscription<Message> {
    drop_zone::events().map(Message::Drop)
}

/// `/api/users/{id}/avatar?v=<avatar_at>`, the picture the server keeps.
fn avatar_url(base: &str, me: &Value) -> String {
    format!("{base}/api/users/{}/avatar?v={}", fmt::s(me, "id"), fmt::f(me, "avatar_at").round() as i64)
}

fn text_box<'a>(placeholder: &str, value: &str, on_input: impl Fn(String) -> Message + 'a, secure: bool) -> El<'a, Message> {
    input(placeholder, value, on_input).secure(secure).width(Length::Fill).into()
}

pub fn view<'a>(state: &'a State, ctx: &Ctx<'a>) -> El<'a, Message> {
    let p = ctx.look;
    let me = &state.me;
    let has = |f: &str| ctx.has_feature(f);
    let admin = ctx.is_admin();
    let name = fmt::s(me, "username");

    // ----- the profile card -------------------------------------------------------------
    let picture: El<'a, Message> = match (fmt::truthy(me, "avatar_at"), ctx.picture(&format!("/api/users/{}/avatar?v={}", fmt::s(me, "id"), fmt::f(me, "avatar_at").round() as i64))) {
        (true, Some(h)) => image(h).width(52).height(52).content_fit(ContentFit::Cover).border_radius(26.0).into(),
        _ => widgets::avatar(&name, 52.0),
    };
    let mut title = row![widgets::h2(name.clone())].spacing(6).align_y(Alignment::Center);
    if has("rename") {
        title = title.push(icon_btn(Icon::Edit, "Change username", Some(Message::EditName(state.rename.is_none()))));
    }
    let role = if fmt::s(me, "role") == "owner" {
        "Owner"
    } else if admin {
        "Administrator"
    } else {
        "Member"
    };
    let head = row![
        picture,
        column![title, small_muted(format!("{role} · joined {}", fmt::ago(fmt::f(me, "created_at"))))].spacing(2),
    ]
    .spacing(14)
    .align_y(Alignment::Center);
    let mut profile = vec![head.into()];
    if let Some(n) = &state.rename {
        let mut form = column![
            row![
                text_box("", n, Message::NameInput, false),
                btn("Save", Btn::Primary, Some(Message::SaveName)),
                btn("Cancel", Btn::Ghost, Some(Message::EditName(false))),
            ]
            .spacing(10)
            .align_y(Alignment::Center),
            help("What you sign in with and what others see. 3–32 letters, numbers, . _ -"),
        ]
        .spacing(8);
        if !fmt::truthy(me, "email") {
            form = form.push(warn_text("⚠ There's no email on this account, so your username is the only way you'll be able to sign in – make it one you'll remember. Add an email below to be able to reset your password."));
        }
        profile.push(form.into());
    }
    if has("avatar") {
        let mut r = row![btn_icon(Icon::Download, "Change picture", Btn::Default, (!state.busy).then_some(Message::PickAvatar))]
            .spacing(8)
            .align_y(Alignment::Center);
        if fmt::truthy(me, "avatar_at") {
            r = r.push(btn_sm("Remove", Btn::Ghost, Some(Message::RemoveAvatar)));
        }
        profile.push(drop_zone::view(r, &state.drop, "Drop a picture to make it yours"));
    }
    let mut requests = row![
        help("Requests this week:"),
        t(fmt::s(me, "requests_this_week")).size(12.5).font(theme::bold()),
    ]
    .spacing(4)
    .align_y(Alignment::Center);
    if fmt::truthy(me, "request_limit") && !admin {
        requests = requests.push(help(format!("of {}", fmt::s(me, "request_limit"))));
    }
    if fmt::truthy(me, "auto_approve") || admin {
        requests = requests.push(help("·"));
        requests = requests.push(badge("auto-approved", Badge::Ok));
    }
    profile.push(requests.into());
    let profile_card = widgets::card(column(profile).spacing(14).width(Length::Fill)).width(Length::Fill);

    // ----- password -----------------------------------------------------------------------
    let mut pw = vec![super::between(
        widgets::h3("Password").into(),
        btn_sm(if state.pw_open { "Cancel" } else { "Change password" }, Btn::Default, Some(Message::PwToggle)).into(),
    )];
    if state.pw_open {
        pw.push(widgets::field("Current password", text_box("", &state.pw_current, Message::PwCurrent, true)));
        pw.push(widgets::field(
            "New password",
            input("", &state.pw_new, Message::PwNew).secure(true).on_submit(Message::PwSubmit).width(Length::Fill),
        ));
        pw.push(container(btn("Update password", Btn::Primary, Some(Message::PwSubmit))).into());
    }
    let pw_card = widgets::card(column(pw).spacing(12).width(Length::Fill)).width(Length::Fill);

    // ----- email --------------------------------------------------------------------------
    let email_card = widgets::card(
        column![
            widgets::h3("Email"),
            help("Used only to send you a link if you forget your password."),
            row![
                input("you@example.com", &state.email, Message::EmailInput)
                    .on_submit(Message::EmailSubmit)
                    .width(Length::Fill),
                btn("Save", Btn::Primary, Some(Message::EmailSubmit)),
            ]
            .spacing(10)
            .align_y(Alignment::Center),
        ]
        .spacing(10)
        .width(Length::Fill),
    )
    .width(Length::Fill);

    // ----- apps ---------------------------------------------------------------------------
    let apps_card = widgets::card(
        column![
            widgets::h3("Apps"),
            row![
                help("Android app and desktop apps for Windows, macOS and Linux:"),
                link("download from the Releases page", Message::OpenReleases),
            ]
            .spacing(4)
            .align_y(Alignment::Center),
            help("On a phone you can also use your browser's Add to Home screen."),
            container(btn("Change server…", Btn::Default, Some(Message::Ask(Dialog::ChangeServer)))),
        ]
        .spacing(10)
        .width(Length::Fill),
    )
    .width(Length::Fill);

    let mut page = column![profile_card, pw_card, email_card, apps_card].spacing(16).width(Length::Fill);

    // ----- two-step sign-in ---------------------------------------------------------------
    if has("totp") {
        let on = fmt::truthy(me, "totp_enabled");
        let mut lines: Vec<El<'a, Message>> = vec![help(if on {
            "✅ On – signing in asks for a code from your authenticator app."
        } else {
            "Ask for a 6-digit code from an authenticator app (Google Authenticator, 1Password, Aegis…) when signing in."
        })
        .into()];
        match &state.twofa {
            TwoFa::Idle => lines.push(
                container(if on {
                    btn("Turn off", Btn::Danger, Some(Message::TwoFaOffStart))
                } else {
                    btn("Set up", Btn::Primary, Some(Message::TwoFaSetup))
                })
                .into(),
            ),
            TwoFa::Setup { qr, secret, code } => {
                let mut r = row![].spacing(18).align_y(Alignment::Start);
                if let Some(h) = qr {
                    r = r.push(
                        container(image(h.clone()).width(164).height(164))
                            .padding(8)
                            .style(|_| container::Style {
                                background: Some(iced::Background::Color(iced::Color::WHITE)),
                                border: iced::Border {
                                    color: iced::Color::TRANSPARENT,
                                    width: 0.0,
                                    radius: 12.0.into(),
                                },
                                ..container::Style::default()
                            }),
                    );
                }
                r = r.push(
                    column![
                        help("Scan the code with your authenticator app, or enter this key:"),
                        t(secret.clone()).size(13).font(theme::mono()),
                        row![
                            input("6-digit code", code, Message::TwoFaCode).on_submit(Message::TwoFaEnable).width(170),
                            btn("Turn on", Btn::Primary, Some(Message::TwoFaEnable)),
                            btn("Cancel", Btn::Ghost, Some(Message::TwoFaCancel)),
                        ]
                        .spacing(8)
                        .align_y(Alignment::Center),
                    ]
                    .spacing(10)
                    .width(Length::Fill),
                );
                lines.push(r.into());
            }
            TwoFa::Off { password } => lines.push(
                row![
                    input("Your password", password, Message::TwoFaPassword)
                        .secure(true)
                        .on_submit(Message::TwoFaDisable)
                        .width(240),
                    btn("Turn off two-step sign-in", Btn::Danger, Some(Message::TwoFaDisable)),
                    btn("Cancel", Btn::Ghost, Some(Message::TwoFaCancel)),
                ]
                .spacing(8)
                .align_y(Alignment::Center)
                .into(),
            ),
        }
        page = page.push(widgets::card(block("Two-step sign-in", lines)).width(Length::Fill));
    }

    // ----- active sessions ----------------------------------------------------------------
    let logout_all: Option<El<'a, Message>> = has("logout_all").then(|| {
        btn_icon(Icon::Logout, "Sign out everywhere", Btn::Danger, Some(Message::Ask(Dialog::LogoutAll))).into()
    });
    page = page.push(widgets::section_head("Active sessions", logout_all));
    let sessions: El<'a, Message> = match &state.sessions {
        None => widgets::skeleton_rows(3),
        Some(list) if list.is_empty() => help("No sessions.").into(),
        Some(list) => {
            let mut c = column![].spacing(0);
            for s in list {
                let right: El<'a, Message> = if fmt::truthy(s, "current") {
                    badge("This device", Badge::Ok)
                } else {
                    btn_icon(Icon::Logout, "Revoke", Btn::Danger, Some(Message::Revoke(fmt::s(s, "id")))).into()
                };
                c = c.push(widgets::list_row(
                    None,
                    session_title(s),
                    format!("{} · signed in {}", fmt::s(s, "ip"), fmt::ago(fmt::f(s, "created_at"))),
                    vec![right],
                    None::<Message>,
                ));
            }
            c.into()
        }
    };
    page = page.push(widgets::card_flat(sessions).width(Length::Fill));

    // ----- about (members only; admins have it under General) -----------------------------
    if !admin {
        page = page.push(about_card(ctx, Message::OpenReleases));
    }
    if !state.error.is_empty() {
        page = page.push(widgets::error_box(&state.error));
    }
    let _ = p;
    container(page).max_width(760).into()
}

/// Version, build and app downloads – the bottom of General (and of Account for members).
pub fn about_card<'a, M: Clone + 'a>(ctx: &Ctx<'a>, open_releases: M) -> El<'a, M> {
    let info = &ctx.session.info;
    let version = {
        let v = fmt::s(info, "version");
        if v.is_empty() { "?".to_string() } else { v }
    };
    let build = {
        let b = fmt::s(info, "build");
        if b.is_empty() { "dev".to_string() } else { b }
    };
    let cell = |label: &'static str, value: El<'a, M>| column![small_muted(label), value].spacing(2);
    card(
        "About Encore",
        vec![
            row![
                cell("Version", t(format!("v{version}")).size(20).font(theme::bold()).into()),
                cell("Server", t("Encore").into()),
                cell("Build", t(build).font(theme::mono()).into()),
                cell("This app", t(format!("Encore desktop {VERSION}")).into()),
            ]
            .spacing(24)
            .align_y(Alignment::Start)
            .into(),
            row![
                help("Apps for Android, Windows, macOS and Linux:"),
                link("github.com/Krifmaz/Encore/releases", open_releases),
            ]
            .spacing(4)
            .align_y(Alignment::Center)
            .into(),
        ],
    )
}

pub fn overlay<'a>(state: &'a State, _ctx: &Ctx<'a>) -> Option<El<'a, Message>> {
    match state.dialog? {
        Dialog::LogoutAll => Some(widgets::confirm(
            "Sign out everywhere?",
            "Every phone, computer and app signed in to your account is signed out – including this one.",
            "Sign out everywhere",
            true,
            Message::LogoutAll,
            Message::CloseDialog,
        )),
        Dialog::ChangeServer => Some(modal(
            "Change server",
            widgets::muted("Sign out of this server and pick another one? Your saved servers stay in the list.").size(14),
            Some(
                row![
                    btn("Cancel", Btn::Default, Some(Message::CloseDialog)),
                    btn("Change server", Btn::Primary, Some(Message::ChangeServer)),
                ]
                .spacing(10)
                .into(),
            ),
            false,
            Message::CloseDialog,
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_titles_read_like_the_web() {
        let app = json!({ "kind": "app", "device_name": "Pixel 9 Pro", "created_at": 1.0, "last_seen_at": 0 });
        assert!(session_title(&app).starts_with("Encore app · Pixel 9 Pro · last used "));
        assert_eq!(session_title(&json!({ "kind": "web", "device_name": "Chrome on Linux" })), "Chrome on Linux");
        assert_eq!(session_title(&json!({ "kind": "web" })), "Unknown device");
    }

    #[test]
    fn base64_decodes_pictures() {
        assert_eq!(base64_decode("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(base64_decode("aGVsbG8gd29ybGQ").unwrap(), b"hello world");
        assert_eq!(base64_decode("iVBORw0K").unwrap(), [0x89, b'P', b'N', b'G', b'\r', b'\n']);
        assert!(base64_decode("not base64!").is_none());
    }
}
