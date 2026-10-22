//! Settings: the sections of the web's settings page (You, People, Server, System) plus This
//! computer, with the same ids and labels. The nav on the left is here; every section is a
//! module of its own with the page shape (`open`, `update`, `view`, `overlay`, `subscription`)
//! and this module routes to it. The helpers at the bottom are the small pieces every settings
//! page draws: switches, inputs, help lines, the Saved/Test result line, time words.

pub mod account;
pub mod appearance;
pub mod autodownload;
pub mod blocked;
pub mod connections;
pub mod contribute;
pub mod diagnostics;
pub mod fields;
pub mod invites;
pub mod language;
pub mod playback;
pub mod plugins;
pub mod server;
pub mod tasks;
pub mod this_computer;
pub mod tools;
pub mod users;
pub mod whats_new;

use crate::api::{Api, ApiResult};
use crate::pages::{Ctx, El, Page};
use crate::theme::{self, Btn};
use crate::widgets::{self, icon, t, Icon};
use iced::widget::text::IntoFragment;
use iced::widget::{button, column, container, row, space, text_input, toggler, Text};
use iced::{Alignment, Length, Subscription, Task};
use serde_json::Value;

/// One settings page.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Section {
    Account,
    Appearance,
    Language,
    Playback,
    Connections,
    Users,
    Invites,
    General,
    Library,
    Downloads,
    /// The plugins (0.32): sources, download clients, connections and tools, installed, updated and set up.
    Plugins,
    Integrations,
    /// Server processing (Cadenza).
    Processing,
    Accounts,
    Autodownload,
    Contribute,
    Blocked,
    Tasks,
    Diagnostics,
    /// The admin file manager (pages/files.rs, 0.33).
    Files,
    /// Desktop only: autostart, hotkeys, mini player, updates, offline songs.
    Computer,
    /// The changelog and the switch for the popup after updates.
    WhatsNew,
}

impl Section {
    pub const ALL: [Section; 22] = [
        Section::Account,
        Section::Appearance,
        Section::Language,
        Section::Playback,
        Section::Connections,
        Section::Users,
        Section::Invites,
        Section::General,
        Section::Library,
        Section::Downloads,
        Section::Plugins,
        Section::Integrations,
        Section::Processing,
        Section::Accounts,
        Section::Autodownload,
        Section::Contribute,
        Section::Blocked,
        Section::Tasks,
        Section::Diagnostics,
        Section::Files,
        Section::Computer,
        Section::WhatsNew,
    ];

    /// The web's page id (`#/settings?s=<id>`).
    pub fn id(self) -> &'static str {
        match self {
            Section::Account => "account",
            Section::Appearance => "appearance",
            Section::Language => "language",
            Section::Playback => "playback",
            Section::Connections => "connections",
            Section::Users => "users",
            Section::Invites => "invites",
            Section::General => "general",
            Section::Library => "library",
            Section::Downloads => "downloads",
            Section::Plugins => "plugins",
            Section::Integrations => "integrations",
            Section::Processing => "processing",
            Section::Accounts => "accounts",
            Section::Autodownload => "autodownload",
            Section::Contribute => "contribute",
            Section::Blocked => "blocked",
            Section::Tasks => "tasks",
            Section::Diagnostics => "diagnostics",
            Section::Files => "files",
            Section::Computer => "computer",
            Section::WhatsNew => "whats-new",
        }
    }

    /// The section of a web page id, including the old aliases (`apps`, the page Apps & sources was
    /// until R2, opens Downloads; `tagging` Library; `services` Integrations) and the ids of single
    /// cards (`verify` opens Library; `torznab`, a source card on Downloads until 0.32, opens Plugins
    /// with that plugin's card). A card id is also remembered, so the section opens with that card
    /// unfolded (see [`take_focus`]), the way the web's `#/settings?s=torznab` link does.
    pub fn by_id(id: &str) -> Option<Section> {
        if let Some(s) = Section::ALL.into_iter().find(|s| s.id() == id) {
            return Some(s);
        }
        if plugins::opens_plugins(id) {
            if let Some(p) = plugins::card_for(id) {
                *FOCUS.lock().unwrap_or_else(|e| e.into_inner()) = Some(p);
            }
            return Some(Section::Plugins);
        }
        match id {
            "apps" => Some(Section::Downloads),
            "tagging" => Some(Section::Library),
            "services" => Some(Section::Integrations),
            "about" | "api" | "keys" => Some(Section::General),
            _ => {
                let g = fields::group_of(id)?;
                let section = Section::ALL.into_iter().find(|s| s.id() == g.id)?;
                if let Some(c) = fields::card(id) {
                    *FOCUS.lock().unwrap_or_else(|e| e.into_inner()) = Some(c.id);
                }
                Some(section)
            }
        }
    }

    /// The nav entry's words.
    pub fn label(self) -> &'static str {
        match self {
            Section::Account => "Account",
            Section::Appearance => "Appearance",
            Section::Language => "Language",
            Section::Playback => "Playback",
            Section::Connections => "Connections",
            Section::Users => "Users & permissions",
            Section::Invites => "Invites",
            Section::General => "General",
            Section::Library => "Library",
            Section::Downloads => "Downloads",
            Section::Plugins => "Plugins",
            Section::Integrations => "Integrations",
            Section::Processing => "Server processing",
            Section::Accounts => "Email & notifications",
            Section::Autodownload => "Auto-download",
            Section::Contribute => "Contribute",
            Section::Blocked => "Blocked",
            Section::Tasks => "Scheduled tasks",
            Section::Diagnostics => "Diagnostics",
            Section::Files => "Files",
            Section::Computer => "This computer",
            Section::WhatsNew => "What's new",
        }
    }

    /// The nav group: You, People, Server, System, This computer, Encore.
    pub fn group(self) -> &'static str {
        match self {
            Section::Account | Section::Appearance | Section::Language | Section::Playback | Section::Connections => "You",
            Section::Users | Section::Invites => "People",
            Section::General
            | Section::Library
            | Section::Downloads
            | Section::Plugins
            | Section::Integrations
            | Section::Processing
            | Section::Accounts
            | Section::Autodownload
            | Section::Contribute
            | Section::Blocked
            | Section::Tasks => "Server",
            Section::Diagnostics | Section::Files => "System",
            Section::Computer => "This computer",
            Section::WhatsNew => "Encore",
        }
    }

    /// Only admins see People, Server and System.
    pub fn admin_only(self) -> bool {
        !matches!(self.group(), "You" | "This computer" | "Encore")
    }
}

/// The nav groups in order.
pub const GROUPS: [&str; 6] = ["You", "People", "Server", "System", "This computer", "Encore"];

/// The card a link asked for (`#/settings?s=slskd`), until its section opens.
static FOCUS: std::sync::Mutex<Option<&'static str>> = std::sync::Mutex::new(None);

/// The card the last link asked for, once (None when the link named a whole section).
pub fn take_focus() -> Option<&'static str> {
    FOCUS.lock().unwrap_or_else(|e| e.into_inner()).take()
}

#[derive(Debug, Default)]
pub struct State {
    pub section: Option<Section>,
    pub account: account::State,
    pub appearance: appearance::State,
    pub language: language::State,
    pub playback: playback::State,
    pub connections: connections::State,
    pub users: users::State,
    pub invites: invites::State,
    pub server: server::State,
    pub plugins: plugins::State,
    pub diagnostics: diagnostics::State,
    pub files: crate::pages::files::State,
    pub computer: this_computer::State,
    pub whats_new: whats_new::State,
}

#[derive(Clone, Debug)]
pub enum Message {
    Go(Section),
    Account(account::Message),
    Appearance(appearance::Message),
    Language(language::Message),
    Playback(playback::Message),
    Connections(connections::Message),
    Users(users::Message),
    Invites(invites::Message),
    Server(server::Message),
    Plugins(plugins::Message),
    Diagnostics(diagnostics::Message),
    Files(crate::pages::files::Message),
    Computer(this_computer::Message),
    WhatsNew(whats_new::Message),
}

pub fn open(state: &mut State, section: Section, ctx: &mut Ctx) -> Task<Message> {
    let section = if section.admin_only() && !ctx.is_admin() { Section::Account } else { section };
    state.section = Some(section);
    match section {
        Section::Account => account::open(&mut state.account, ctx).map(Message::Account),
        Section::Appearance => appearance::open(&mut state.appearance, ctx).map(Message::Appearance),
        Section::Language => language::open(&mut state.language, ctx).map(Message::Language),
        Section::Playback => playback::open(&mut state.playback, ctx).map(Message::Playback),
        Section::Connections => connections::open(&mut state.connections, ctx).map(Message::Connections),
        Section::Users => users::open(&mut state.users, ctx).map(Message::Users),
        Section::Invites => invites::open(&mut state.invites, ctx).map(Message::Invites),
        Section::General
        | Section::Library
        | Section::Downloads
        | Section::Integrations
        | Section::Processing
        | Section::Accounts
        | Section::Autodownload
        | Section::Contribute
        | Section::Blocked
        | Section::Tasks => server::open(&mut state.server, section, ctx).map(Message::Server),
        Section::Plugins => plugins::open(&mut state.plugins, ctx).map(Message::Plugins),
        Section::Diagnostics => diagnostics::open(&mut state.diagnostics, ctx).map(Message::Diagnostics),
        Section::Files => crate::pages::files::open(&mut state.files, ctx).map(Message::Files),
        Section::Computer => this_computer::open(&mut state.computer, ctx).map(Message::Computer),
        Section::WhatsNew => whats_new::open(&mut state.whats_new, ctx).map(Message::WhatsNew),
    }
}

pub fn update(state: &mut State, msg: Message, ctx: &mut Ctx) -> Task<Message> {
    match msg {
        Message::Go(s) => {
            ctx.navigate(Page::Settings(s));
            Task::none()
        }
        Message::Account(m) => account::update(&mut state.account, m, ctx).map(Message::Account),
        Message::Appearance(m) => appearance::update(&mut state.appearance, m, ctx).map(Message::Appearance),
        Message::Language(m) => language::update(&mut state.language, m, ctx).map(Message::Language),
        Message::Playback(m) => playback::update(&mut state.playback, m, ctx).map(Message::Playback),
        Message::Connections(m) => connections::update(&mut state.connections, m, ctx).map(Message::Connections),
        Message::Users(m) => users::update(&mut state.users, m, ctx).map(Message::Users),
        Message::Invites(m) => invites::update(&mut state.invites, m, ctx).map(Message::Invites),
        Message::Server(m) => server::update(&mut state.server, m, ctx).map(Message::Server),
        Message::Plugins(m) => plugins::update(&mut state.plugins, m, ctx).map(Message::Plugins),
        Message::Diagnostics(m) => diagnostics::update(&mut state.diagnostics, m, ctx).map(Message::Diagnostics),
        Message::Files(m) => crate::pages::files::update(&mut state.files, m, ctx).map(Message::Files),
        Message::Computer(m) => this_computer::update(&mut state.computer, m, ctx).map(Message::Computer),
        Message::WhatsNew(m) => whats_new::update(&mut state.whats_new, m, ctx).map(Message::WhatsNew),
    }
}

/// The settings nav (left) and the section (right).
pub fn view<'a>(state: &'a State, ctx: &Ctx<'a>) -> El<'a, Message> {
    let current = state.section.unwrap_or(Section::Account);
    let p = ctx.look;
    let mut nav = column![].spacing(2).width(200);
    for group in GROUPS {
        let entries: Vec<Section> = Section::ALL
            .into_iter()
            .filter(|s| s.group() == group && (!s.admin_only() || ctx.is_admin()))
            .collect();
        if entries.is_empty() {
            continue;
        }
        nav = nav.push(
            container(t(group).size(11).color(p.faint).font(theme::bold()))
                .padding(widgets::pad(14.0, 12.0, 6.0, 12.0)),
        );
        for s in entries {
            let on = s == current;
            nav = nav.push(
                button(t(s.label()).size(14))
                    .width(Length::Fill)
                    .padding([8, 12])
                    .style(theme::btn(if on { Btn::NavOn } else { Btn::Nav }))
                    .on_press(Message::Go(s)),
            );
        }
    }
    let body: El<'a, Message> = match current {
        Section::Account => account::view(&state.account, ctx).map(Message::Account),
        Section::Appearance => appearance::view(&state.appearance, ctx).map(Message::Appearance),
        Section::Language => language::view(&state.language, ctx).map(Message::Language),
        Section::Playback => playback::view(&state.playback, ctx).map(Message::Playback),
        Section::Connections => connections::view(&state.connections, ctx).map(Message::Connections),
        Section::Users => users::view(&state.users, ctx).map(Message::Users),
        Section::Invites => invites::view(&state.invites, ctx).map(Message::Invites),
        Section::General
        | Section::Library
        | Section::Downloads
        | Section::Integrations
        | Section::Processing
        | Section::Accounts
        | Section::Autodownload
        | Section::Contribute
        | Section::Blocked
        | Section::Tasks => server::view(&state.server, ctx).map(Message::Server),
        Section::Plugins => plugins::view(&state.plugins, ctx).map(Message::Plugins),
        Section::Diagnostics => diagnostics::view(&state.diagnostics, ctx).map(Message::Diagnostics),
        Section::Files => crate::pages::files::view(&state.files, ctx).map(Message::Files),
        Section::Computer => this_computer::view(&state.computer, ctx).map(Message::Computer),
        Section::WhatsNew => whats_new::view(&state.whats_new, ctx).map(Message::WhatsNew),
    };
    column![
        widgets::h1("Settings"),
        row![nav, space().width(24), container(body).width(Length::Fill)].align_y(Alignment::Start),
    ]
    .spacing(18)
    .width(Length::Fill)
    .into()
}

pub fn overlay<'a>(state: &'a State, ctx: &Ctx<'a>) -> Option<El<'a, Message>> {
    match state.section.unwrap_or(Section::Account) {
        Section::Account => account::overlay(&state.account, ctx).map(|e| e.map(Message::Account)),
        Section::Appearance => None,
        Section::Language => None,
        Section::Playback => playback::overlay(&state.playback, ctx).map(|e| e.map(Message::Playback)),
        Section::Connections => connections::overlay(&state.connections, ctx).map(|e| e.map(Message::Connections)),
        Section::Users => users::overlay(&state.users, ctx).map(|e| e.map(Message::Users)),
        Section::Invites => invites::overlay(&state.invites, ctx).map(|e| e.map(Message::Invites)),
        Section::General
        | Section::Library
        | Section::Downloads
        | Section::Integrations
        | Section::Processing
        | Section::Accounts
        | Section::Autodownload
        | Section::Contribute
        | Section::Blocked
        | Section::Tasks => server::overlay(&state.server, ctx).map(|e| e.map(Message::Server)),
        Section::Plugins => plugins::overlay(&state.plugins).map(|e| e.map(Message::Plugins)),
        Section::Diagnostics => diagnostics::overlay(&state.diagnostics, ctx).map(|e| e.map(Message::Diagnostics)),
        Section::Files => crate::pages::files::overlay(&state.files, ctx).map(|e| e.map(Message::Files)),
        Section::Computer => this_computer::overlay(&state.computer, ctx).map(|e| e.map(Message::Computer)),
        Section::WhatsNew => None,
    }
}

pub fn subscription(state: &State) -> Subscription<Message> {
    match state.section.unwrap_or(Section::Account) {
        Section::Playback => playback::subscription(&state.playback).map(Message::Playback),
        Section::Connections => connections::subscription(&state.connections).map(Message::Connections),
        Section::General
        | Section::Library
        | Section::Downloads
        | Section::Integrations
        | Section::Processing
        | Section::Accounts
        | Section::Autodownload
        | Section::Contribute
        | Section::Blocked
        | Section::Tasks => server::subscription(&state.server).map(Message::Server),
        Section::Plugins => plugins::subscription(&state.plugins).map(Message::Plugins),
        Section::Files => crate::pages::files::subscription(&state.files).map(Message::Files),
        Section::Computer => this_computer::subscription(&state.computer).map(Message::Computer),
        _ => Subscription::none(),
    }
}

// ----- PATCH -----------------------------------------------------------------------------

/// `PATCH <base>/api<path>` (users, tasks and connection flags are patched on the web).
pub async fn patch(api: &Api, path: &str, body: Value) -> ApiResult {
    api.patch(path, body).await
}

// ----- the pieces every settings page draws ----------------------------------------------

/// A switch with its words on the right.
pub fn switch<'a, M: Clone + 'a>(on: bool, label: impl IntoFragment<'a>, f: impl Fn(bool) -> M + 'a) -> El<'a, M> {
    toggler(on)
        .label(label)
        .text_size(14)
        .spacing(10)
        .on_toggle(f)
        .style(theme::switch)
        .into()
}

/// A switch that can be read-only (`None`: set by an environment variable, or not yours).
pub fn switch_maybe<'a, M: Clone + 'a, F: Fn(bool) -> M + 'a>(on: bool, label: impl IntoFragment<'a>, f: Option<F>) -> El<'a, M> {
    let dim = f.is_none();
    let p = theme::look();
    let w = toggler(on)
        .label(label)
        .text_size(14)
        .spacing(10)
        .on_toggle_maybe(f)
        .style(theme::switch);
    if dim {
        container(w).style(move |_| container::Style {
            text_color: Some(p.muted),
            ..container::Style::default()
        })
        .into()
    } else {
        w.into()
    }
}

/// One option of a [`select`] built at run time (device lists, filters the engine offers).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Opt {
    pub key: String,
    pub label: String,
}

impl Opt {
    pub fn new(key: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
        }
    }
}

impl std::fmt::Display for Opt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label)
    }
}

/// `select.input` over run-time options; `selected` is a key (an unknown one shows nothing).
pub fn select<'a, M: Clone + 'a>(options: Vec<Opt>, selected: &str, on_pick: impl Fn(String) -> M + 'a) -> El<'a, M> {
    let current = options.iter().find(|o| o.key == selected).cloned();
    iced::widget::pick_list(options, current, move |o: Opt| on_pick(o.key))
        .text_size(13)
        .padding([7, 10])
        .style(theme::pick)
        .menu_style(theme::pick_menu)
        .into()
}

/// [`select`] over a `(key, label)` table.
pub fn select_table<'a, M: Clone + 'a>(table: &[(&str, &str)], selected: &str, on_pick: impl Fn(String) -> M + 'a) -> El<'a, M> {
    select(table.iter().map(|(k, l)| Opt::new(*k, *l)).collect(), selected, on_pick)
}

/// `.input`: a one-line text box (add `.secure(true)`, `.on_submit(..)`, `.width(..)`).
pub fn input<'a, M: Clone + 'a>(placeholder: &str, value: &str, on_input: impl Fn(String) -> M + 'a) -> text_input::TextInput<'a, M> {
    text_input(placeholder, value)
        .on_input(on_input)
        .padding([9, 12])
        .size(14)
        .style(theme::input)
}

/// A read-only `.input` (set by an environment variable).
pub fn input_locked<'a, M: Clone + 'a>(value: &str) -> text_input::TextInput<'a, M> {
    text_input("", value).padding([9, 12]).size(14).style(theme::input)
}

/// `.small.muted`: a help line under a field or a heading.
pub fn help<'a>(s: impl IntoFragment<'a>) -> Text<'a> {
    t(s).size(12.5).color(theme::look().muted)
}

/// A small line in the warning colour.
pub fn warn_text<'a>(s: impl IntoFragment<'a>) -> Text<'a> {
    t(s).size(12.5).color(theme::look().warn)
}

/// A small line in the error colour.
pub fn err_text<'a>(s: impl IntoFragment<'a>) -> Text<'a> {
    t(s).size(12.5).color(theme::look().err)
}

/// A titled block inside a card: `h3` over its lines.
pub fn block<'a, M: 'a>(title: &'a str, lines: Vec<El<'a, M>>) -> El<'a, M> {
    let mut c = column![widgets::h3(title)].spacing(8);
    for l in lines {
        c = c.push(l);
    }
    c.width(Length::Fill).into()
}

/// `.card.stack` with an `h2`: the frame of most settings pages.
pub fn card<'a, M: 'a>(title: &'a str, lines: Vec<El<'a, M>>) -> El<'a, M> {
    let mut c = column![widgets::h2(title)].spacing(14);
    for l in lines {
        c = c.push(l);
    }
    widgets::card(c.width(Length::Fill)).width(Length::Fill).into()
}

/// `.test-result`: a check or a cross with the message, green or red.
pub fn result_line<'a, M: 'a>(ok: bool, message: impl IntoFragment<'a>) -> El<'a, M> {
    let p = theme::look();
    let color = if ok { p.ok } else { p.err };
    row![icon(if ok { Icon::Check } else { Icon::X }, 15.0, color), t(message).size(13).color(color)]
        .spacing(6)
        .align_y(Alignment::Center)
        .into()
}

/// A test result like [`result_line`]; a failure carries Copy (`on_copy` gets the whole text).
pub fn result_line_copy<'a, M: Clone + 'a>(ok: bool, message: &str, on_copy: impl Fn(String) -> M) -> El<'a, M> {
    if ok {
        return result_line(ok, message.to_string());
    }
    let color = theme::look().err;
    row![
        icon(Icon::X, 15.0, color),
        container(t(message.to_string()).size(13).color(color)).width(Length::Fill),
        crate::widgets::details::copy_btn(on_copy(message.to_string())),
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .into()
}

/// A link-looking button in the accent colour (`<a href>` on the web).
pub fn link<'a, M: Clone + 'a>(label: impl IntoFragment<'a>, on: M) -> El<'a, M> {
    button(t(label).size(13).color(theme::look().accent))
        .padding([2, 4])
        .style(theme::btn(Btn::Ghost))
        .on_press(on)
        .into()
}

/// A small muted link-looking button (the `clear`, `Sign out of Tidal`, reset links).
pub fn link_muted<'a, M: Clone + 'a>(label: impl IntoFragment<'a>, on: M) -> El<'a, M> {
    button(t(label).size(12.5).color(theme::look().muted))
        .padding([2, 4])
        .style(theme::btn(Btn::Ghost))
        .on_press(on)
        .into()
}

/// The left words and the right value or button of a row.
pub fn between<'a, M: 'a>(left: El<'a, M>, right: El<'a, M>) -> El<'a, M> {
    row![left, space().width(Length::Fill), right]
        .spacing(12)
        .align_y(Alignment::Center)
        .width(Length::Fill)
        .into()
}

/// Two fields side by side (the web's `.form-grid`); `right` None leaves the row half empty.
pub fn two<'a, M: 'a>(left: El<'a, M>, right: Option<El<'a, M>>) -> El<'a, M> {
    let mut r = row![container(left).width(Length::FillPortion(1))].spacing(16).align_y(Alignment::Start);
    r = r.push(match right {
        Some(x) => container(x).width(Length::FillPortion(1)),
        None => container(space()).width(Length::FillPortion(1)),
    });
    r.width(Length::Fill).into()
}

// ----- words and numbers --------------------------------------------------------------------

/// Seconds since 1970.
pub fn now_s() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// `Math.round`.
pub fn js_round(x: f64) -> i64 {
    (x + 0.5).floor() as i64
}

/// The web's settings `ago(t)`: "never", "just now", "12 min ago", "3 h ago", "2 days ago".
pub fn ago(t: f64) -> String {
    ago_from(now_s(), t)
}

/// [`ago`] against a given clock (tests, rows drawn with one `now`).
pub fn ago_from(now: f64, t: f64) -> String {
    if t == 0.0 {
        return "never".into();
    }
    let s = now - t;
    if s < 90.0 {
        "just now".into()
    } else if s < 5400.0 {
        format!("{} min ago", js_round(s / 60.0))
    } else if s < 129_600.0 {
        format!("{} h ago", js_round(s / 3600.0))
    } else {
        format!("{} days ago", js_round(s / 86_400.0))
    }
}

/// The web's `until(t)`: "any moment", "in 12 min", "in 3 h", "in 2 days" ("" for 0).
pub fn until(t: f64) -> String {
    until_from(now_s(), t)
}

/// [`until`] against a given clock.
pub fn until_from(now: f64, t: f64) -> String {
    if t == 0.0 {
        return String::new();
    }
    let s = t - now;
    if s <= 60.0 {
        "any moment".into()
    } else if s < 5400.0 {
        format!("in {} min", js_round(s / 60.0))
    } else if s < 129_600.0 {
        format!("in {} h", js_round(s / 3600.0))
    } else {
        format!("in {} days", js_round(s / 86_400.0))
    }
}

/// `${n}` of a JavaScript number: `15`, `2.5`, `0.05`.
pub fn js_num(n: f64) -> String {
    if !n.is_finite() {
        return "0".into();
    }
    if n.fract() == 0.0 && n.abs() < 1e15 {
        return format!("{}", n as i64);
    }
    let s = format!("{n:.6}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// JavaScript `+value` of an input's text as JSON: a number, `0` for "", `null` for NaN.
pub fn js_plus(s: &str) -> Value {
    let s = s.trim();
    if s.is_empty() {
        return Value::from(0);
    }
    match s.parse::<f64>() {
        Ok(f) if f.is_finite() => {
            if f.fract() == 0.0 && f.abs() < 9.0e15 {
                Value::from(f as i64)
            } else {
                Value::from(f)
            }
        }
        _ => Value::Null,
    }
}

/// A JSON value as the text an input shows: strings as they are, numbers and booleans
/// printed, null and missing as "".
pub fn vs(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Number(n) => n.as_f64().map(js_num).unwrap_or_else(|| n.to_string()),
        Value::Bool(b) => b.to_string(),
        other => other.to_string(),
    }
}

/// JavaScript truthiness of a value.
pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(_) => true,
    }
}

/// `_meta.<key>.source == "env"`: the setting is locked by an environment variable.
pub fn env_locked(values: &Value, key: &str) -> bool {
    values["_meta"][key]["source"].as_str() == Some("env")
}

/// Runs `f` with a page context against a server nobody answers (tests of `update`: a task that would call the
/// server is only made, never run).
#[cfg(test)]
pub fn with_test_ctx(f: impl FnOnce(&mut Ctx)) {
    let api = crate::api::Api::new("http://s");
    let config = crate::config::Config::default();
    let session = crate::api::Session { user: serde_json::json!({ "username": "a", "role": "owner", "perms": {} }), info: Value::Null };
    let covers = crate::widgets::Covers::new();
    let mut ctx = Ctx::new(api, &config, &session, Default::default(), &covers, crate::theme::look(), iced::Size::new(1000.0, 700.0), iced::Point::ORIGIN);
    f(&mut ctx);
}

/// The note under a value an environment variable sets ("🔒 Set by ENCORE_… in your compose/env"), or None when
/// none does. The Server pages' fields, the Users page and the Invites page use it alike (as the web does).
pub fn env_note(values: &Value, key: &str) -> Option<String> {
    env_locked(values, key).then(|| format!("🔒 Set by {} in your compose/env – change it there (environment always wins)", crate::fmt::s(&values["_meta"][key], "env")))
}

/// The saved value the server masks (`•••• (saved)`).
pub fn is_masked(v: &str) -> bool {
    v.starts_with("••••")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_resolve_like_the_web_router() {
        assert_eq!(Section::by_id("playback"), Some(Section::Playback));
        // Apps & sources became four pages (R2); its old names and its cards open the page that holds them
        assert_eq!(Section::by_id("downloads"), Some(Section::Downloads));
        assert_eq!(Section::by_id("apps"), Some(Section::Downloads));
        assert_eq!(Section::by_id("tagging"), Some(Section::Library));
        assert_eq!(Section::by_id("services"), Some(Section::Integrations));
        assert_eq!(Section::by_id("library"), Some(Section::Library));
        assert_eq!(Section::by_id("verify"), Some(Section::Library));
        assert_eq!(Section::by_id("roon"), Some(Section::Integrations));
        assert_eq!(Section::by_id("cadenza"), Some(Section::Processing));
        assert_eq!(Section::by_id("processing"), Some(Section::Processing));
        // the source cards moved to Plugins (0.32): their links open the plugin's card there
        assert_eq!(Section::by_id("torznab"), Some(Section::Plugins));
        assert_eq!(Section::by_id("plugins"), Some(Section::Plugins));
        assert_eq!(Section::by_id("youtube"), Some(Section::Plugins));
        assert_eq!(Section::by_id("tools"), Some(Section::Plugins), "the Download tools card");
        assert_eq!(Section::by_id("pipeline"), Some(Section::Downloads));
        assert_eq!(Section::by_id("releases"), Some(Section::Downloads));
        assert_eq!(Section::by_id("sync"), Some(Section::Tasks));
        assert_eq!(Section::by_id("about"), Some(Section::General));
        assert_eq!(Section::by_id("nowhere"), None);
        assert_eq!(Section::by_id("slskd"), Some(Section::Plugins));
        let server: Vec<&str> = Section::ALL.into_iter().filter(|s| s.group() == "Server").map(Section::label).collect();
        assert_eq!(server[..6], ["General", "Library", "Downloads", "Plugins", "Integrations", "Server processing"]);
        assert!(Section::Plugins.admin_only());
        // (other tests parse settings links at the same time, so only "something" is certain)
        assert!(take_focus().is_some());
        assert!(Section::Users.admin_only() && !Section::Computer.admin_only());
    }

    #[test]
    fn time_and_number_words_match_the_web() {
        let now = 1_000_000.0;
        assert_eq!(ago_from(now, 0.0), "never");
        assert_eq!(ago_from(now, now - 30.0), "just now");
        assert_eq!(ago_from(now, now - 600.0), "10 min ago");
        assert_eq!(ago_from(now, now - 7200.0), "2 h ago");
        assert_eq!(ago_from(now, now - 3.0 * 86_400.0), "3 days ago");
        assert_eq!(until_from(now, now + 10.0), "any moment");
        assert_eq!(until_from(now, now + 1800.0), "in 30 min");
        assert_eq!(until_from(now, now + 2.0 * 86_400.0), "in 2 days");
        assert_eq!(js_num(15.0), "15");
        assert_eq!(js_num(2.5), "2.5");
        assert_eq!(js_num(0.05), "0.05");
        assert_eq!(js_plus(""), Value::from(0));
        assert_eq!(js_plus("12"), Value::from(12));
        assert_eq!(js_plus("2.5"), Value::from(2.5));
        assert_eq!(js_plus("x"), Value::Null);
        assert_eq!(vs(&serde_json::json!(1.0)), "1");
        assert_eq!(vs(&Value::Null), "");
        let v = serde_json::json!({ "_meta": { "a": { "source": "env" }, "b": { "source": "ui" } } });
        assert!(env_locked(&v, "a") && !env_locked(&v, "b"));
        assert!(is_masked("•••• (saved)") && !is_masked("abc"));
    }
}
