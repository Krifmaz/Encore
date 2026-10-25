//! The window: `App` state (session, page, history, toasts, player, link, config), the
//! top-level `Message` with one variant per module, `update` routing to each module, `view`
//! composing the shell (sidebar, main, player bar) around the page, and `subscription`
//! merging the modules'. Page agents never edit this file beyond what their stub needs.

use crate::api::{Api, ApiError, Session};
use crate::config::Config;
use crate::engine;
use crate::fmt;
use crate::link;
use crate::pages::{self, Ctx, Effect, El, Page};
use crate::player;
use crate::theme::{self, Palette};
use crate::widgets::details::{self, Details};
use crate::widgets::{self, Covers, Toast, ToastKind};
use iced::widget::{column, container, operation, row, scrollable, stack};
use iced::{window, Length, Point, Size, Subscription, Task};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// Command-line flags.
#[derive(Clone, Debug, Default)]
pub struct Flags {
    /// `--server <url>`: open this server (remembered on sign-in).
    pub server: Option<String>,
    /// `--minimized`: start minimized (in the tray once package G lands).
    pub minimized: bool,
    /// `--screenshot-tour <dir>` with `--login user:pass`.
    pub tour: Option<(PathBuf, Option<(String, String)>)>,
}

/// The id of the main scroll area, so navigation can scroll to the top.
pub const MAIN_SCROLL: &str = "main-scroll";

/// Counts the sidebar shows.
#[derive(Debug, Default)]
pub struct Shell {
    /// Pending requests (admins).
    pub pending: i64,
    /// Bug reports with something new.
    pub bugs: i64,
    /// `/api/library/storage`.
    pub storage: Value,
}

#[derive(Clone, Copy, Debug)]
pub enum ShellKind {
    Pending,
    Bugs,
    Storage,
}

#[derive(Clone, Debug)]
pub enum Message {
    Tick,
    /// The offline banner's Retry, and every 30 s in offline mode: ask the server again.
    CheckServer,
    /// The answer to [`Message::CheckServer`]: true when the server is back.
    ServerBack(bool),
    /// The address in use was chosen again ([`Api::choose_route`]): the local network address while it answers.
    Route(crate::route::Route),
    Nav(Page),
    Back,
    SignOut,
    /// The saved session was checked (`/api/auth/me` + `/api/auth/info`).
    Session(Result<Session, ApiError>),
    Shell(ShellKind, Result<Value, ApiError>),
    Cover(String, Option<Vec<u8>>),
    DismissToast(u64),
    /// Copy an error toast's text.
    CopyToast(u64),
    /// A toast's action ("Undo").
    ToastAction(u64),
    /// The details dialog (an error or a request in full, with Copy).
    Details(details::Msg),
    /// The details dialog's action was sent (Keep anyway, Download anyway): its toast when it
    /// worked.
    DetailsActed(String, Result<Value, ApiError>),
    /// A request's Play: the library song or album it brought in.
    DetailsPlay(Result<Value, ApiError>),
    /// A copy was checked against the clipboard: whether it took.
    Copied(bool),
    WindowOpened(window::Id),
    CloseRequested(window::Id),
    WindowClosed(window::Id),
    Resized(Size),
    CursorMoved(Point),
    SystemTheme(iced::theme::Mode),
    Player(player::Message),
    Link(link::Message),
    Desktop(crate::desktop::Message),
    /// The mini player, a seek from the OS media controls, the tray's update line.
    DesktopUi(crate::desktop::Msg),
    Tour(crate::tour::Message),
    /// The What's new popup after an update.
    WhatsNew(crate::whats_new::Message),
    Login(pages::login::Message),
    Home(pages::home::Message),
    Search(pages::search::Message),
    ForYou(pages::for_you::Message),
    Tag(pages::tag::Message),
    Artist(pages::artist::Message),
    Album(pages::album::Message),
    Library(pages::library::Message),
    LibraryAlbum(pages::library_album::Message),
    LibraryArtist(pages::library_artist::Message),
    Requests(pages::requests::Message),
    Playlists(pages::playlists::Message),
    Playlist(pages::playlist::Message),
    Download(pages::download::Message),
    Soulseek(pages::soulseek::Message),
    Leaderboard(pages::leaderboard::Message),
    Storage(pages::storage::Message),
    BugReports(pages::bug_reports::Message),
    BugReport(pages::bug_report::Message),
    Settings(pages::settings::Message),
    Offline(pages::offline::Message),
    Nothing,
}

pub struct App {
    pub api: Arc<Api>,
    pub config: Config,
    pub session: Session,
    pub page: Page,
    pub history: Vec<Page>,
    pub toasts: Vec<Toast>,
    next_toast: u64,
    /// The details dialog when one is open (over the page and its own dialogs).
    pub details: Option<Details>,
    pub covers: Covers,
    pub player: player::State,
    pub link: link::State,
    pub shell: Shell,
    pub window: Option<window::Id>,
    pub size: Size,
    pub cursor: Point,
    pub look: Palette,
    iced_theme: iced::Theme,
    system_dark: bool,
    /// The saved session is being checked; the window shows the logo meanwhile.
    pub booting: bool,
    pub tour: Option<crate::tour::State>,
    pub whats_new: crate::whats_new::State,
    ticks: u64,
    /// The server address in use when it was last chosen, so a switch is told once ([`Message::Route`]).
    route_via: crate::route::Via,
    // one state per page
    pub login: pages::login::State,
    pub home: pages::home::State,
    pub search: pages::search::State,
    pub for_you: pages::for_you::State,
    pub tag: pages::tag::State,
    pub artist: pages::artist::State,
    pub album: pages::album::State,
    pub library: pages::library::State,
    pub library_album: pages::library_album::State,
    pub library_artist: pages::library_artist::State,
    pub requests: pages::requests::State,
    pub playlists: pages::playlists::State,
    pub playlist: pages::playlist::State,
    pub download: pages::download::State,
    pub soulseek: pages::soulseek::State,
    pub leaderboard: pages::leaderboard::State,
    pub storage: pages::storage::State,
    pub bug_reports: pages::bug_reports::State,
    pub bug_report: pages::bug_report::State,
    pub settings: pages::settings::State,
    pub offline: pages::offline::State,
}

/// A `Ctx` over the app's fields (disjoint borrows, so a page state can be `&mut` beside it).
macro_rules! ctx {
    ($app:ident) => {
        Ctx::new(
            $app.api.clone(),
            &$app.config,
            &$app.session,
            $app.player.snapshot(),
            &$app.covers,
            $app.look,
            $app.size,
            $app.cursor,
        )
        .with_previous($app.history.last())
    };
}

/// Route a page message: `update` the page, map its task, apply its effects.
macro_rules! page_update {
    ($app:ident, $msg:expr, $field:ident, $module:ident, $variant:ident) => {{
        let mut ctx = ctx!($app);
        let task = pages::$module::update(&mut $app.$field, $msg, &mut ctx).map(Message::$variant);
        let fx = ctx.take_effects();
        Task::batch([task, $app.apply(fx)])
    }};
}

impl App {
    pub fn boot(flags: Flags) -> (App, Task<Message>) {
        let mut config = Config::load();
        if let Some(s) = &flags.server {
            config.remember(s);
        }
        let api = Api::new(&config.server);
        if let Some(cur) = config.current() {
            api.set_cookie(&cur.cookie);
        }
        let w = &config.window;
        // --minimized starts in the tray where one is expected (else minimized, as before).
        let in_tray = flags.minimized && flags.tour.is_none() && crate::desktop::tray::likely();
        let (id, open) = window::open(crate::desktop::window_state::settings(w, in_tray));
        let login = pages::login::new(&config);
        let mut app = App {
            api,
            session: Session::default(),
            page: Page::Login,
            history: Vec::new(),
            toasts: Vec::new(),
            next_toast: 1,
            details: None,
            covers: Covers::new(),
            player: player::State::default(),
            link: link::State::default(),
            shell: Shell::default(),
            window: Some(id),
            size: Size::new(w.width, w.height),
            cursor: Point::ORIGIN,
            look: theme::look(),
            iced_theme: theme::iced_theme(&theme::look()),
            system_dark: true,
            booting: false,
            tour: flags.tour.map(|(dir, login)| crate::tour::State::new(dir, login)),
            whats_new: Default::default(),
            ticks: 0,
            route_via: crate::route::Via::Home,
            login,
            home: Default::default(),
            search: Default::default(),
            for_you: Default::default(),
            tag: Default::default(),
            artist: Default::default(),
            album: Default::default(),
            library: Default::default(),
            library_album: Default::default(),
            library_artist: Default::default(),
            requests: Default::default(),
            playlists: Default::default(),
            playlist: Default::default(),
            download: Default::default(),
            soulseek: Default::default(),
            leaderboard: Default::default(),
            storage: Default::default(),
            bug_reports: Default::default(),
            bug_report: Default::default(),
            settings: Default::default(),
            offline: Default::default(),
            config,
        };
        app.apply_look();
        let mut tasks = vec![
            open.map(Message::WindowOpened),
            iced::system::theme().map(Message::SystemTheme),
        ];
        if !app.config.server.is_empty() {
            app.booting = true;
            let api = app.api.clone();
            tasks.push(Task::perform(async move { api.session().await }, Message::Session));
        }
        if flags.minimized && !in_tray {
            tasks.push(window::minimize(id, true));
        }
        (app, Task::batch(tasks))
    }

    pub fn title(&self, _: window::Id) -> String {
        match self.player.status.track.as_ref() {
            Some(t) if self.player.status.state == engine::PlayState::Playing && !t.title.is_empty() => {
                format!("{} – Encore", t.title)
            }
            _ => "Encore".into(),
        }
    }

    pub fn theme(&self, _: window::Id) -> iced::Theme {
        self.iced_theme.clone()
    }

    /// Recompute the look from the config and the system theme.
    fn apply_look(&mut self) {
        let a = &self.config.appearance;
        let p = theme::palette(&a.theme, &a.accent, &a.custom, &a.background, self.system_dark);
        theme::set(p);
        self.look = p;
        self.iced_theme = theme::iced_theme(&p);
    }

    fn ctx(&self) -> Ctx<'_> {
        ctx!(self)
    }

    pub fn toast(&mut self, text: impl Into<String>, kind: ToastKind) {
        self.toast_with(text, kind, None);
    }

    /// A toast with one action ("Up next cleared · Undo"): it stays `ms`, its button sends `then`
    /// to the player.
    pub fn toast_action(&mut self, text: impl Into<String>, label: &str, then: player::Message, ms: u64) {
        self.toast_with(text, ToastKind::Ok, Some((label.to_string(), then, std::time::Duration::from_millis(ms))));
    }

    fn toast_with(&mut self, text: impl Into<String>, kind: ToastKind, action: Option<(String, player::Message, std::time::Duration)>) {
        let text = text.into();
        // an error stays until it is dismissed: the same error again (a page that polls) does not
        // pile up a second copy
        if kind == ToastKind::Err && self.toasts.iter().any(|t| t.stays() && t.text == text) {
            return;
        }
        let id = self.next_toast;
        self.next_toast += 1;
        self.toasts.push(Toast {
            id,
            text,
            kind,
            at: std::time::Instant::now(),
            action,
        });
        // at most three errors stay: a new one pushes out the oldest
        while self.toasts.iter().filter(|t| t.stays()).count() > Toast::STAYING {
            if let Some(i) = self.toasts.iter().position(Toast::stays) {
                self.toasts.remove(i);
            }
        }
        if self.toasts.len() > 5 {
            self.toasts.remove(0);
        }
    }

    /// The web's `copy_text`: write to the clipboard, read it back, and say whether it took.
    fn copy_checked(text: String) -> Task<Message> {
        iced::clipboard::write::<Message>(text.clone())
            .chain(iced::clipboard::read().map(move |got| Message::Copied(details::copied(&text, got.as_deref()))))
    }

    /// Play Encore track rows through the engine.
    pub fn play(&mut self, tracks: Vec<Value>, index: usize) -> Task<Message> {
        let auth = link::auth(&self.config, &self.api);
        let list = engine::tracks::many(&self.api.base(), &auth, &tracks);
        if let Err(e) = engine::play(list, index, 0.0) {
            self.toast(e, ToastKind::Err);
        }
        self.group_flush()
    }

    /// A Shuffle button: the rows in a random order; in a group listen the server shuffles them
    /// (player::group::forward_shuffle).
    pub fn play_shuffled(&mut self, tracks: Vec<Value>) -> Task<Message> {
        if player::group::active() {
            let auth = link::auth(&self.config, &self.api);
            let list = engine::tracks::many(&self.api.base(), &auth, &tracks);
            if player::group::forward_shuffle(&list) {
                return self.group_flush();
            }
        }
        self.play(pages::library::shuffled(&tracks), 0)
    }

    /// In a group listen a play goes to the group (player::group): sent at once.
    fn group_flush(&mut self) -> Task<Message> {
        if player::group::has_pending() {
            self.update(Message::Player(player::Message::Group(player::group::Msg::Flush)))
        } else {
            Task::none()
        }
    }

    fn queue(&mut self, tracks: Vec<Value>, next: bool) -> Task<Message> {
        let auth = link::auth(&self.config, &self.api);
        let list = engine::tracks::many(&self.api.base(), &auth, &tracks);
        let r = if next { engine::play_next(list) } else { engine::enqueue(list) };
        match r {
            // the web's words, whatever the number of songs
            Ok(()) => self.toast(if next { "Plays next" } else { "Added to queue" }, ToastKind::Ok),
            Err(e) => self.toast(e, ToastKind::Err),
        }
        self.group_flush()
    }

    /// The sidebar's counts: storage, pending requests (admins), bug reports.
    fn load_shell(&self) -> Task<Message> {
        self.load_shell_kinds(&[ShellKind::Storage, ShellKind::Bugs, ShellKind::Pending])
    }

    /// Some of the sidebar's counts (the pending badge only for admins).
    fn load_shell_kinds(&self, kinds: &[ShellKind]) -> Task<Message> {
        let mut tasks = Vec::new();
        for &kind in kinds {
            let path = match kind {
                ShellKind::Storage => "/library/storage",
                ShellKind::Bugs => "/bug-reports/unread",
                ShellKind::Pending if self.session.is_admin() => "/requests?status=pending&limit=500",
                ShellKind::Pending => continue,
            };
            let api = self.api.clone();
            tasks.push(Task::perform(async move { api.get(path).await }, move |r| Message::Shell(kind, r)));
        }
        Task::batch(tasks)
    }

    /// The web's `appearance.adoptAccount`: where the server keeps preferences, the account's
    /// look (`prefs.appearance`) wins over this computer's; keys it lacks take the defaults.
    fn adopt_appearance(&mut self) {
        if !self.session.has_feature("theme") {
            return;
        }
        let a = &self.session.user["prefs"]["appearance"];
        if !a.is_object() {
            return;
        }
        let mut next = crate::config::Appearance::default();
        for (key, field) in [
            ("theme", &mut next.theme),
            ("accent", &mut next.accent),
            ("custom", &mut next.custom),
            ("background", &mut next.background),
        ] {
            if let Some(v) = a[key].as_str().filter(|v| !v.is_empty()) {
                *field = v.to_string();
            }
        }
        if next != self.config.appearance {
            self.config.appearance = next;
            self.config.save();
            self.apply_look();
        }
    }

    /// Signed in: ask whether the What's new popup has something to show (not during the screenshot tour).
    fn whats_new_check(&self) -> Task<Message> {
        if self.tour.is_some() {
            return Task::none();
        }
        crate::whats_new::check(&self.ctx()).map(Message::WhatsNew)
    }

    /// Go to `page`; `push` keeps the current one in the history.
    pub fn navigate(&mut self, page: Page, push: bool) -> Task<Message> {
        if page == Page::Login {
            self.login = pages::login::new(&self.config);
        }
        if push && self.page != page && self.page != Page::Login {
            self.history.push(self.page.clone());
            if self.history.len() > 60 {
                self.history.remove(0);
            }
        }
        self.page = page.clone();
        self.details = None;
        let mut ctx = ctx!(self);
        let task = match page {
            Page::Login => pages::login::open(&mut self.login, &mut ctx).map(Message::Login),
            Page::Home => pages::home::open(&mut self.home, &mut ctx).map(Message::Home),
            Page::Search { q } => pages::search::open(&mut self.search, q, &mut ctx).map(Message::Search),
            Page::ForYou => pages::for_you::open(&mut self.for_you, &mut ctx).map(Message::ForYou),
            Page::Tag(name) => pages::tag::open(&mut self.tag, name, &mut ctx).map(Message::Tag),
            Page::Artist(id) | Page::ArtistName(id) => {
                pages::artist::open(&mut self.artist, id, &mut ctx).map(Message::Artist)
            }
            Page::Album(id) => pages::album::open(&mut self.album, id, &mut ctx).map(Message::Album),
            Page::Library { tab, query } => {
                pages::library::open(&mut self.library, tab, query, &mut ctx).map(Message::Library)
            }
            Page::LibraryAlbum(id) => {
                pages::library_album::open(&mut self.library_album, id, &mut ctx).map(Message::LibraryAlbum)
            }
            Page::LibraryArtist(name) => {
                pages::library_artist::open(&mut self.library_artist, name, &mut ctx).map(Message::LibraryArtist)
            }
            Page::Requests => pages::requests::open(&mut self.requests, &mut ctx).map(Message::Requests),
            Page::Playlists => pages::playlists::open(&mut self.playlists, &mut ctx).map(Message::Playlists),
            Page::Playlist(id) => pages::playlist::open(&mut self.playlist, id, &mut ctx).map(Message::Playlist),
            Page::Download => pages::download::open(&mut self.download, &mut ctx).map(Message::Download),
            Page::Soulseek => pages::soulseek::open(&mut self.soulseek, &mut ctx).map(Message::Soulseek),
            Page::Leaderboard => pages::leaderboard::open(&mut self.leaderboard, &mut ctx).map(Message::Leaderboard),
            Page::Storage => pages::storage::open(&mut self.storage, &mut ctx).map(Message::Storage),
            Page::BugReports => pages::bug_reports::open(&mut self.bug_reports, &mut ctx).map(Message::BugReports),
            Page::BugReport(id) => pages::bug_report::open(&mut self.bug_report, id, &mut ctx).map(Message::BugReport),
            Page::Settings(s) => pages::settings::open(&mut self.settings, s, &mut ctx).map(Message::Settings),
            Page::Offline => pages::offline::open(&mut self.offline, &mut ctx).map(Message::Offline),
        };
        let fx = ctx.take_effects();
        let top = operation::snap_to(MAIN_SCROLL, scrollable::RelativeOffset::START);
        Task::batch([task, self.apply(fx), top])
    }

    /// After a sign-in: remember the server with its cookie, load the session, connect the link.
    fn after_login(&mut self, user: Value) -> Task<Message> {
        let base = self.api.base();
        let cookie = self.api.session_cookie().unwrap_or_default();
        let name = fmt::s(&user, "username");
        // Every sign-in gets a new app token: the kept one may be revoked (password change,
        // Sign out everywhere) or belong to another account; it is ended on the server.
        let old_token = {
            let s = self.config.remember(&base);
            s.cookie = cookie;
            s.username = name;
            std::mem::take(&mut s.token)
        };
        self.config.save();
        let revoke = {
            let (api, base) = (self.api.clone(), base.clone());
            Task::perform(async move { api.revoke_app_token(&base, &old_token).await }, |_| Message::Nothing)
        };
        self.session = Session {
            user,
            info: self.login.info.clone(),
        };
        self.adopt_appearance();
        self.covers.clear();
        self.history.clear();
        self.booting = false;
        let api = self.api.clone();
        Task::batch([
            self.navigate(Page::Home, false),
            link::start(self.api.clone(), &self.config).map(Message::Link),
            self.load_shell(),
            Task::perform(async move { api.session().await }, Message::Session),
            revoke,
            self.whats_new_check(),
        ])
    }

    /// Picks the server address to use ([`Api::choose_route`]).
    fn choose_route(&self) -> Task<Message> {
        let api = self.api.clone();
        Task::perform(async move { api.choose_route().await }, Message::Route)
    }

    /// Asks the server whether it is back ([`Api::check_back`], which ends offline mode).
    fn check_server(&self) -> Task<Message> {
        let api = self.api.clone();
        Task::perform(async move { api.check_back().await }, Message::ServerBack)
    }

    /// Offline mode's banner above the page: the server can't be reached, the pages show what
    /// this computer kept and the saved songs play; Retry asks again.
    fn offline_banner(&self) -> Option<El<'_, Message>> {
        if !self.api.is_offline() {
            return None;
        }
        let p = theme::look();
        Some(
            container(
                row![
                    widgets::t("Offline – showing what this computer saved. Songs saved for offline play.")
                        .size(14)
                        .color(p.warn)
                        .width(Length::Fill),
                    widgets::btn("Offline music", theme::Btn::Default, Some(Message::Nav(Page::Offline))),
                    widgets::btn("Retry", theme::Btn::Primary, Some(Message::CheckServer)),
                ]
                .spacing(12)
                .align_y(iced::Alignment::Center),
            )
            .padding([10, 16])
            .width(Length::Fill)
            .style(theme::warn_box)
            .into(),
        )
    }

    fn sign_out(&mut self) -> Task<Message> {
        // the answers kept for offline mode go with the session (before its cookie is cleared)
        self.api.forget_kept();
        let server = self.config.server.clone();
        let token = match self.config.server_mut(&server) {
            Some(s) => {
                s.cookie.clear();
                std::mem::take(&mut s.token)
            }
            None => String::new(),
        };
        self.config.save();
        // out of a group listen first, while the session still counts
        let leave = player::group::sign_out(&mut self.player, &server);
        // The app token is a session of its own: the cookie logout leaves it, so it is ended too.
        let api = self.api.clone();
        let logout = Task::perform(
            async move {
                if let Some((path, body)) = leave {
                    let _ = api.post(&path, body).await;
                }
                let _ = api.revoke_app_token(&server, &token).await;
                api.logout().await
            },
            |_| Message::Nothing,
        );
        link::stop(&mut self.link);
        engine::send(engine::Command::Stop);
        self.session = Session::default();
        self.covers.clear();
        self.history.clear();
        self.shell = Shell::default();
        self.whats_new = Default::default();
        Task::batch([logout, self.navigate(Page::Login, false)])
    }

    /// Apply what a page asked for.
    pub fn apply(&mut self, effects: Vec<Effect>) -> Task<Message> {
        let mut tasks = Vec::new();
        for e in effects {
            match e {
                Effect::Navigate(p) => tasks.push(self.navigate(p, true)),
                Effect::Replace(p) => tasks.push(self.navigate(p, false)),
                Effect::Route(p) => {
                    // the same page, its route changed: nothing to open again
                    if std::mem::discriminant(&p) == std::mem::discriminant(&self.page) {
                        self.page = p;
                    }
                }
                Effect::Toast(t, k) => self.toast(t, k),
                Effect::ToastAction { text, label, then, ms } => self.toast_action(text, &label, then, ms),
                Effect::Play { tracks, index } => tasks.push(self.play(tracks, index)),
                Effect::PlayShuffled(tracks) => tasks.push(self.play_shuffled(tracks)),
                Effect::PlayNext(t) => tasks.push(self.queue(t, true)),
                Effect::Enqueue(t) => tasks.push(self.queue(t, false)),
                Effect::Clipboard(s) => tasks.push(iced::clipboard::write(s)),
                Effect::CopyText(s) => tasks.push(Self::copy_checked(s)),
                Effect::Details { title, text, lead, act, links } => self.details = Some(Details::new(title, text, lead, act).with_links(*links)),
                Effect::OpenUrl(u) => {
                    if let Err(e) = open::that_detached(&u) {
                        self.toast(format!("Could not open the browser: {e}"), ToastKind::Err);
                    }
                }
                Effect::Config(f) => {
                    f(&mut self.config);
                    self.config.save();
                    self.apply_look();
                }
                Effect::SignedIn(user) => tasks.push(self.after_login(user)),
                Effect::SignOut => tasks.push(self.sign_out()),
                Effect::RefreshSession => {
                    let api = self.api.clone();
                    tasks.push(Task::perform(async move { api.session().await }, Message::Session));
                }
                Effect::RefreshShell => {
                    if self.session.signed_in() {
                        tasks.push(self.load_shell());
                    }
                }
                Effect::Player(m) => tasks.push(self.update(Message::Player(m))),
                Effect::ForgetCover(u) => self.covers.forget(&u),
            }
        }
        Task::batch(tasks)
    }

    pub fn update(&mut self, msg: Message) -> Task<Message> {
        match msg {
            Message::Tick => {
                self.ticks += 1;
                self.toasts.retain(|t| !t.expired());
                let mut tasks: Vec<Task<Message>> = self
                    .covers
                    .take_wants()
                    .into_iter()
                    .map(|u| widgets::cover::load(self.api.clone(), u).map(|(u, b)| Message::Cover(u, b)))
                    .collect();
                // as the web: pending requests every 30 s, the bug badge every minute, storage
                // every 5 minutes
                if self.session.signed_in() {
                    let mut kinds = Vec::new();
                    if self.ticks.is_multiple_of(60) {
                        kinds.push(ShellKind::Pending);
                    }
                    if self.ticks.is_multiple_of(120) {
                        kinds.push(ShellKind::Bugs);
                    }
                    if self.ticks.is_multiple_of(600) {
                        kinds.push(ShellKind::Storage);
                    }
                    if !kinds.is_empty() {
                        tasks.push(self.load_shell_kinds(&kinds));
                    }
                    tasks.push(link::on_tick(&mut self.link, self.api.clone(), &mut self.config).map(Message::Link));
                    // offline mode: the server is asked again every 30 s
                    if self.api.is_offline() && self.ticks.is_multiple_of(60) {
                        tasks.push(self.check_server());
                    }
                    // every minute: home network or not (the local network address answers or it doesn't)
                    if !self.api.is_offline() && self.ticks.is_multiple_of(120) {
                        tasks.push(self.choose_route());
                    }
                }
                tasks.push(crate::desktop::on_tick(self));
                Task::batch(tasks)
            }
            Message::CheckServer => self.check_server(),
            Message::Route(r) => {
                // a switch shows once; the same address again says nothing
                if r.via != self.route_via {
                    self.toast(format!("Connected through {}", r.label()), ToastKind::Ok);
                    self.route_via = r.via;
                }
                Task::none()
            }
            Message::ServerBack(false) => Task::none(),
            Message::ServerBack(true) => {
                // the open page and the shell's badges load again from the server
                self.toast("Back online", ToastKind::Ok);
                Task::batch([self.navigate(self.page.clone(), false), self.load_shell()])
            }
            Message::Nav(p) => self.navigate(p, true),
            Message::Back => match self.history.pop() {
                Some(p) => self.navigate(p, false),
                None => Task::none(),
            },
            Message::SignOut => self.sign_out(),
            Message::Session(Ok(s)) => {
                let first = !self.session.signed_in();
                self.session = s;
                self.adopt_appearance();
                self.booting = false;
                if first {
                    Task::batch([
                        self.choose_route(),
                        self.navigate(Page::Home, false),
                        link::start(self.api.clone(), &self.config).map(Message::Link),
                        self.load_shell(),
                        self.whats_new_check(),
                    ])
                } else {
                    Task::none()
                }
            }
            Message::Session(Err(e)) => {
                self.booting = false;
                if self.session.signed_in() {
                    if e.status == 401 {
                        self.toast("Your session expired – please sign in again", ToastKind::Err);
                        return self.sign_out();
                    }
                    return Task::none();
                }
                let server = self.config.server.clone();
                self.page = Page::Login;
                if e.unreachable() {
                    pages::login::unreachable(&mut self.login, &server, &e.message);
                } else {
                    pages::login::sign_in(&mut self.login, &server, Value::Null);
                    let api = self.api.clone();
                    return Task::perform(async move { api.info().await }, move |r| {
                        Message::Login(pages::login::Message::Probed(server.clone(), r))
                    });
                }
                Task::none()
            }
            Message::Shell(kind, Ok(v)) => {
                match kind {
                    ShellKind::Pending => self.shell.pending = fmt::items(&v).len() as i64,
                    ShellKind::Bugs => self.shell.bugs = fmt::i(&v, "count"),
                    ShellKind::Storage => self.shell.storage = v,
                }
                Task::none()
            }
            Message::Shell(_, Err(_)) => Task::none(),
            Message::Cover(u, b) => {
                self.covers.insert(u, b);
                Task::none()
            }
            Message::DismissToast(id) => {
                self.toasts.retain(|t| t.id != id);
                Task::none()
            }
            Message::CopyToast(id) => match self.toasts.iter().find(|t| t.id == id) {
                Some(t) => Self::copy_checked(t.text.clone()),
                None => Task::none(),
            },
            Message::ToastAction(id) => {
                let then = self.toasts.iter().find(|t| t.id == id).and_then(|t| t.action.as_ref()).map(|a| a.1.clone());
                self.toasts.retain(|t| t.id != id);
                match then {
                    Some(m) => self.update(Message::Player(m)),
                    None => Task::none(),
                }
            }
            Message::Details(m) => {
                if matches!(m, details::Msg::Close) {
                    self.details = None;
                    return Task::none();
                }
                match self.details.as_mut().and_then(|d| d.update(m)) {
                    Some(details::Out::Copy(text)) => Self::copy_checked(text),
                    // the dialog closes; the page's list reloads once the server answered
                    Some(details::Out::Act(a)) => {
                        self.details = None;
                        let api = self.api.clone();
                        let done = a.done.clone();
                        Task::perform(async move { api.post(&a.path, a.body).await }, move |r| {
                            Message::DetailsActed(done.clone(), r)
                        })
                    }
                    Some(details::Out::Open(page)) => {
                        self.details = None;
                        self.navigate(page, true)
                    }
                    Some(details::Out::Play(path)) => {
                        self.details = None;
                        let api = self.api.clone();
                        Task::perform(async move { api.get(&path).await }, Message::DetailsPlay)
                    }
                    None => Task::none(),
                }
            }
            // a request's Play: the song, or the album from its first song
            Message::DetailsPlay(r) => match r {
                Ok(v) => {
                    let tracks = crate::widgets::request_list::playable(&v);
                    if tracks.is_empty() {
                        self.toast("Nothing to play", ToastKind::Err);
                        Task::none()
                    } else {
                        self.play(tracks, 0)
                    }
                }
                Err(e) => {
                    self.toast(e.message, ToastKind::Err);
                    Task::none()
                }
            },
            Message::DetailsActed(done, r) => {
                match r {
                    Ok(_) => self.toast(done, ToastKind::Ok),
                    Err(e) => self.toast(e.message, ToastKind::Err),
                }
                // the list shows the change at once (or, after an error, what the server offers now)
                let reload = crate::widgets::request_list::Msg::Reload;
                match self.page {
                    Page::Requests => self.update(Message::Requests(pages::requests::Message::List(reload))),
                    Page::Download => self.update(Message::Download(pages::download::Message::List(reload))),
                    _ => Task::none(),
                }
            }
            Message::Copied(ok) => {
                if ok {
                    self.toast(details::COPIED, ToastKind::Ok);
                } else {
                    self.toast(details::COPY_FAILED, ToastKind::Err);
                }
                Task::none()
            }
            Message::WindowOpened(id) => {
                self.window = Some(id);
                crate::desktop::on_window_opened(self, id)
            }
            Message::CloseRequested(id) => {
                // The mini player just closes; the main window's size and place are saved.
                if let Some(t) = crate::desktop::on_close_requested(self, id) {
                    return t;
                }
                engine::shutdown();
                iced::exit()
            }
            Message::WindowClosed(id) => {
                if self.window == Some(id) {
                    self.window = None;
                }
                Task::none()
            }
            Message::Resized(s) => {
                self.size = s;
                Task::none()
            }
            Message::CursorMoved(p) => {
                self.cursor = p;
                Task::none()
            }
            Message::SystemTheme(mode) => {
                let dark = !matches!(mode, iced::theme::Mode::Light);
                if dark != self.system_dark {
                    self.system_dark = dark;
                    self.apply_look();
                }
                Task::none()
            }
            Message::Player(m) => {
                let mut ctx = ctx!(self);
                let task = player::update(&mut self.player, m, &mut ctx).map(Message::Player);
                let fx = ctx.take_effects();
                Task::batch([task, self.apply(fx)])
            }
            Message::Link(m) => {
                // Refused a token with a 401: the session itself is gone, which the session
                // check turns into "please sign in again".
                let refused = matches!(&m, link::Message::Token(_, Err(e)) if e.status == 401);
                let task = link::update(&mut self.link, m, &mut self.config).map(Message::Link);
                if refused && self.session.signed_in() {
                    let api = self.api.clone();
                    return Task::batch([task, Task::perform(async move { api.session().await }, Message::Session)]);
                }
                task
            }
            Message::Desktop(m) => crate::desktop::handle(self, m),
            Message::DesktopUi(m) => crate::desktop::update(self, m),
            Message::Tour(m) => crate::tour::update(self, m),
            Message::WhatsNew(m) => {
                let mut ctx = ctx!(self);
                let task = crate::whats_new::update(&mut self.whats_new, m, &mut ctx).map(Message::WhatsNew);
                let fx = ctx.take_effects();
                Task::batch([task, self.apply(fx)])
            }
            Message::Login(m) => page_update!(self, m, login, login, Login),
            Message::Home(m) => page_update!(self, m, home, home, Home),
            Message::Search(m) => page_update!(self, m, search, search, Search),
            Message::ForYou(m) => page_update!(self, m, for_you, for_you, ForYou),
            Message::Tag(m) => page_update!(self, m, tag, tag, Tag),
            Message::Artist(m) => page_update!(self, m, artist, artist, Artist),
            Message::Album(m) => page_update!(self, m, album, album, Album),
            Message::Library(m) => page_update!(self, m, library, library, Library),
            Message::LibraryAlbum(m) => page_update!(self, m, library_album, library_album, LibraryAlbum),
            Message::LibraryArtist(m) => page_update!(self, m, library_artist, library_artist, LibraryArtist),
            Message::Requests(m) => page_update!(self, m, requests, requests, Requests),
            Message::Playlists(m) => page_update!(self, m, playlists, playlists, Playlists),
            Message::Playlist(m) => page_update!(self, m, playlist, playlist, Playlist),
            Message::Download(m) => page_update!(self, m, download, download, Download),
            Message::Soulseek(m) => page_update!(self, m, soulseek, soulseek, Soulseek),
            Message::Leaderboard(m) => page_update!(self, m, leaderboard, leaderboard, Leaderboard),
            Message::Storage(m) => page_update!(self, m, storage, storage, Storage),
            Message::BugReports(m) => page_update!(self, m, bug_reports, bug_reports, BugReports),
            Message::BugReport(m) => page_update!(self, m, bug_report, bug_report, BugReport),
            Message::Settings(m) => page_update!(self, m, settings, settings, Settings),
            Message::Offline(m) => page_update!(self, m, offline, offline, Offline),
            Message::Nothing => Task::none(),
        }
    }

    /// The current page's content.
    fn page_view<'a>(&'a self, ctx: &Ctx<'a>) -> El<'a, Message> {
        match &self.page {
            Page::Login => pages::login::view(&self.login, ctx).map(Message::Login),
            Page::Home => pages::home::view(&self.home, ctx).map(Message::Home),
            Page::Search { .. } => pages::search::view(&self.search, ctx).map(Message::Search),
            Page::ForYou => pages::for_you::view(&self.for_you, ctx).map(Message::ForYou),
            Page::Tag(_) => pages::tag::view(&self.tag, ctx).map(Message::Tag),
            Page::Artist(_) | Page::ArtistName(_) => pages::artist::view(&self.artist, ctx).map(Message::Artist),
            Page::Album(_) => pages::album::view(&self.album, ctx).map(Message::Album),
            Page::Library { .. } => pages::library::view(&self.library, ctx).map(Message::Library),
            Page::LibraryAlbum(_) => pages::library_album::view(&self.library_album, ctx).map(Message::LibraryAlbum),
            Page::LibraryArtist(_) => {
                pages::library_artist::view(&self.library_artist, ctx).map(Message::LibraryArtist)
            }
            Page::Requests => pages::requests::view(&self.requests, ctx).map(Message::Requests),
            Page::Playlists => pages::playlists::view(&self.playlists, ctx).map(Message::Playlists),
            Page::Playlist(_) => pages::playlist::view(&self.playlist, ctx).map(Message::Playlist),
            Page::Download => pages::download::view(&self.download, ctx).map(Message::Download),
            Page::Soulseek => pages::soulseek::view(&self.soulseek, ctx).map(Message::Soulseek),
            Page::Leaderboard => pages::leaderboard::view(&self.leaderboard, ctx).map(Message::Leaderboard),
            Page::Storage => pages::storage::view(&self.storage, ctx).map(Message::Storage),
            Page::BugReports => pages::bug_reports::view(&self.bug_reports, ctx).map(Message::BugReports),
            Page::BugReport(_) => pages::bug_report::view(&self.bug_report, ctx).map(Message::BugReport),
            Page::Settings(_) => pages::settings::view(&self.settings, ctx).map(Message::Settings),
            Page::Offline => pages::offline::view(&self.offline, ctx).map(Message::Offline),
        }
    }

    /// The current page's modal or menu, and the player's.
    fn overlay_view<'a>(&'a self, ctx: &Ctx<'a>) -> Option<El<'a, Message>> {
        if let Some(o) = player::overlay(&self.player, ctx) {
            return Some(o.map(Message::Player));
        }
        match &self.page {
            Page::Login => pages::login::overlay(&self.login, ctx).map(|e| e.map(Message::Login)),
            Page::Home => pages::home::overlay(&self.home, ctx).map(|e| e.map(Message::Home)),
            Page::Search { .. } => pages::search::overlay(&self.search, ctx).map(|e| e.map(Message::Search)),
            Page::ForYou => pages::for_you::overlay(&self.for_you, ctx).map(|e| e.map(Message::ForYou)),
            Page::Tag(_) => pages::tag::overlay(&self.tag, ctx).map(|e| e.map(Message::Tag)),
            Page::Artist(_) | Page::ArtistName(_) => {
                pages::artist::overlay(&self.artist, ctx).map(|e| e.map(Message::Artist))
            }
            Page::Album(_) => pages::album::overlay(&self.album, ctx).map(|e| e.map(Message::Album)),
            Page::Library { .. } => pages::library::overlay(&self.library, ctx).map(|e| e.map(Message::Library)),
            Page::LibraryAlbum(_) => {
                pages::library_album::overlay(&self.library_album, ctx).map(|e| e.map(Message::LibraryAlbum))
            }
            Page::LibraryArtist(_) => {
                pages::library_artist::overlay(&self.library_artist, ctx).map(|e| e.map(Message::LibraryArtist))
            }
            Page::Requests => pages::requests::overlay(&self.requests, ctx).map(|e| e.map(Message::Requests)),
            Page::Playlists => pages::playlists::overlay(&self.playlists, ctx).map(|e| e.map(Message::Playlists)),
            Page::Playlist(_) => pages::playlist::overlay(&self.playlist, ctx).map(|e| e.map(Message::Playlist)),
            Page::Download => pages::download::overlay(&self.download, ctx).map(|e| e.map(Message::Download)),
            Page::Soulseek => pages::soulseek::overlay(&self.soulseek, ctx).map(|e| e.map(Message::Soulseek)),
            Page::Leaderboard => {
                pages::leaderboard::overlay(&self.leaderboard, ctx).map(|e| e.map(Message::Leaderboard))
            }
            Page::Storage => pages::storage::overlay(&self.storage, ctx).map(|e| e.map(Message::Storage)),
            Page::BugReports => {
                pages::bug_reports::overlay(&self.bug_reports, ctx).map(|e| e.map(Message::BugReports))
            }
            Page::BugReport(_) => pages::bug_report::overlay(&self.bug_report, ctx).map(|e| e.map(Message::BugReport)),
            Page::Settings(_) => pages::settings::overlay(&self.settings, ctx).map(|e| e.map(Message::Settings)),
            Page::Offline => pages::offline::overlay(&self.offline, ctx).map(|e| e.map(Message::Offline)),
        }
    }

    pub fn view(&self, id: window::Id) -> El<'_, Message> {
        if let Some(v) = crate::desktop::window_view(self, id) {
            return v.map(Message::DesktopUi);
        }
        let ctx = self.ctx();
        let body: El<'_, Message> = if self.booting {
            container(widgets::logo(64.0))
                .width(Length::Fill)
                .height(Length::Fill)
                .center(Length::Fill)
                .into()
        } else if matches!(self.page, Page::Login | Page::Offline) && !self.session.signed_in() {
            self.page_view(&ctx)
        } else {
            let content_width = 1180.0_f32.max(self.size.width * 0.68).min(self.size.width - theme::SIDEBAR - 64.0);
            let page = container(container(self.page_view(&ctx)).max_width(content_width).width(Length::Fill))
                .padding([26, 32])
                .width(Length::Fill)
                .center_x(Length::Fill);
            let main = scrollable(page)
                .id(MAIN_SCROLL)
                .height(Length::Fill)
                .width(Length::Fill)
                .style(theme::scroll);
            let bar = player::bar(&self.player, &ctx).map(Message::Player);
            // offline mode: the banner over the page (not over the sidebar)
            let main: El<'_, Message> = match self.offline_banner() {
                Some(b) => column![container(b).padding([8, 16]), main].into(),
                None => main.into(),
            };
            column![
                row![widgets::sidebar::view(self), main].height(Length::Fill),
                bar
            ]
            .into()
        };
        let mut layers = stack![container(body).width(Length::Fill).height(Length::Fill).style(theme::app_bg)];
        if let Some(o) = self.overlay_view(&ctx) {
            layers = layers.push(o);
        }
        if let Some(w) = crate::whats_new::view(&self.whats_new) {
            layers = layers.push(w.map(Message::WhatsNew));
        }
        if let Some(d) = &self.details {
            layers = layers.push(details::view(d).map(Message::Details));
        }
        if !self.toasts.is_empty() {
            layers = layers.push(widgets::toast::view(&self.toasts, Message::DismissToast, Message::CopyToast, Message::ToastAction));
        }
        layers.into()
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let mut subs = vec![
            iced::time::every(Duration::from_millis(500)).map(|_| Message::Tick),
            window::close_requests().map(Message::CloseRequested),
            window::close_events().map(Message::WindowClosed),
            window::resize_events().map(|(_, s)| Message::Resized(s)),
            iced::event::listen_with(|e, _, _| match e {
                iced::Event::Mouse(iced::mouse::Event::CursorMoved { position }) => Some(Message::CursorMoved(position)),
                // the mouse's back button goes to the previous page
                iced::Event::Mouse(iced::mouse::Event::ButtonPressed(iced::mouse::Button::Back)) => Some(Message::Back),
                _ => None,
            }),
            iced::system::theme_changes().map(Message::SystemTheme),
            crate::desktop::events().map(Message::Desktop),
            crate::desktop::ui_events().map(Message::DesktopUi),
            player::subscription(&self.player).map(Message::Player),
        ];
        subs.push(match &self.page {
            Page::Login => pages::login::subscription(&self.login).map(Message::Login),
            Page::Home => pages::home::subscription(&self.home).map(Message::Home),
            Page::Search { .. } => pages::search::subscription(&self.search).map(Message::Search),
            Page::ForYou => pages::for_you::subscription(&self.for_you).map(Message::ForYou),
            Page::Tag(_) => pages::tag::subscription(&self.tag).map(Message::Tag),
            Page::Artist(_) | Page::ArtistName(_) => pages::artist::subscription(&self.artist).map(Message::Artist),
            Page::Album(_) => pages::album::subscription(&self.album).map(Message::Album),
            Page::Library { .. } => pages::library::subscription(&self.library).map(Message::Library),
            Page::LibraryAlbum(_) => pages::library_album::subscription(&self.library_album).map(Message::LibraryAlbum),
            Page::LibraryArtist(_) => {
                pages::library_artist::subscription(&self.library_artist).map(Message::LibraryArtist)
            }
            Page::Requests => pages::requests::subscription(&self.requests).map(Message::Requests),
            Page::Playlists => pages::playlists::subscription(&self.playlists).map(Message::Playlists),
            Page::Playlist(_) => pages::playlist::subscription(&self.playlist).map(Message::Playlist),
            Page::Download => pages::download::subscription(&self.download).map(Message::Download),
            Page::Soulseek => pages::soulseek::subscription(&self.soulseek).map(Message::Soulseek),
            Page::Leaderboard => pages::leaderboard::subscription(&self.leaderboard).map(Message::Leaderboard),
            Page::Storage => pages::storage::subscription(&self.storage).map(Message::Storage),
            Page::BugReports => pages::bug_reports::subscription(&self.bug_reports).map(Message::BugReports),
            Page::BugReport(_) => pages::bug_report::subscription(&self.bug_report).map(Message::BugReport),
            Page::Settings(_) => pages::settings::subscription(&self.settings).map(Message::Settings),
            Page::Offline => pages::offline::subscription(&self.offline).map(Message::Offline),
        });
        if self.tour.is_some() {
            subs.push(crate::tour::subscription().map(Message::Tour));
        }
        if crate::whats_new::open(&self.whats_new) && self.details.is_none() {
            // Escape closes the What's new popup
            subs.push(iced::event::listen_with(|e, _, _| match e {
                iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
                    key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape),
                    ..
                }) => Some(Message::WhatsNew(crate::whats_new::Message::Close)),
                _ => None,
            }));
        }
        if self.details.is_some() {
            // Escape closes the details dialog
            subs.push(iced::event::listen_with(|e, _, _| match e {
                iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
                    key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape),
                    ..
                }) => Some(Message::Details(details::Msg::Close)),
                _ => None,
            }));
        }
        Subscription::batch(subs)
    }
}
