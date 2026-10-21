//! `--screenshot-tour <dir> [--login user:pass]`: sign in, open every page in order, save one
//! PNG per page with `iced::window::screenshot`, play the first album for a few seconds and
//! exit. Used under Xvfb to check the app without a person at the screen.

use crate::app::{App, Message as AppMsg};
use crate::pages::{Page, Section};
use iced::{Subscription, Task};
use serde_json::Value;
use std::path::PathBuf;
use std::time::Duration;

/// The tick between steps.
const TICK: Duration = Duration::from_millis(600);

#[derive(Clone, Debug)]
pub enum Message {
    Step,
    Taken(String, iced::window::Screenshot),
    /// The first album (with its tracks) for the playing step.
    Album(Result<Value, crate::api::ApiError>),
}

#[derive(Clone, Debug)]
enum Step {
    /// Let the page settle for this many ticks.
    Wait(u32),
    Shot(&'static str),
    SignIn,
    Nav(Page),
    /// Follow a web link (`#/settings?s=torznab`) when the step runs: a settings link to a card unfolds that card
    /// in the section it opens.
    NavHash(&'static str),
    /// Open the album that was played (skipped when none was).
    NavFirstAlbum,
    /// Soulseek, when the server has it.
    NavSoulseek,
    PlayFirstAlbum,
    Customize(bool),
    /// Open one of the file manager's roots (Settings → Files).
    FilesRoot(&'static str),
    /// Open the first song's ⋯ menu on the album page, where a right-click would.
    RowMenu,
    /// Close the menu again.
    CloseRowMenu,
    /// Open the first request's details (skipped when the list is empty).
    RequestDetails,
    /// Close the details dialog.
    CloseDetails,
    /// Open the picker for the first request (skipped when the list is empty).
    RequestPick,
    /// Close the picker.
    ClosePick,
    /// The full player on this pane (None: closed again).
    Full(Option<crate::player::Pane>),
    /// A message to the player (the Signal path dialog and closing it).
    Player(crate::player::Message),
    /// Volume normalization on (Album, −14 LUFS) and Cadenza on at the source rate, so the
    /// Signal path shows the gain.
    Normalize,
    /// Scroll the page to its end (Settings → Playback's Sound block).
    ScrollToEnd,
    /// The Library's search, Discover's search and the artist page for the played album's artist
    /// (0.31: one library search, Discover asking the library, the artist page's sections;
    /// skipped when no album played).
    NavLibrarySearch,
    NavDiscoverSearch,
    NavArtist,
    Exit,
}

#[derive(Debug)]
pub struct State {
    dir: PathBuf,
    login: Option<(String, String)>,
    steps: Vec<Step>,
    next: usize,
    wait: u32,
    first_album: Option<String>,
    /// The played album's artist (the searches' query).
    artist: String,
    pub saved: Vec<String>,
}

impl State {
    pub fn new(dir: PathBuf, login: Option<(String, String)>) -> Self {
        use Step::*;
        let mut steps = vec![Wait(4), Shot("login")];
        if login.is_some() {
            steps.extend([SignIn, Wait(6)]);
        }
        steps.extend([
            Shot("home"),
            PlayFirstAlbum,
            Wait(6),
            Normalize,
            Wait(3),
            Shot("playing"),
            Player(crate::player::Message::Signal),
            Wait(2),
            Shot("signal-path"),
            Player(crate::player::Message::CloseDialog),
            Full(Some(crate::player::Pane::Now)),
            Shot("player-full"),
            Full(Some(crate::player::Pane::Lyrics)),
            Wait(2),
            Shot("player-lyrics"),
            Full(Some(crate::player::Pane::Now)),
            Full(None),
            Nav(Page::Search { q: String::new() }),
            Shot("search"),
            Nav(Page::ForYou),
            Shot("for-you"),
            Nav(Page::library("")),
            Shot("library"),
            NavLibrarySearch,
            Shot("library-search"),
            NavDiscoverSearch,
            Wait(4),
            Shot("search-library"),
            NavArtist,
            Wait(6),
            Shot("artist"),
            NavFirstAlbum,
            Shot("library-album"),
            RowMenu,
            Shot("row-menu"),
            CloseRowMenu,
            Nav(Page::Requests),
            Shot("requests"),
            RequestDetails,
            Shot("request-details"),
            CloseDetails,
            RequestPick,
            Shot("request-picker"),
            ClosePick,
            Nav(Page::Playlists),
            Shot("playlists"),
            Nav(Page::Download),
            Shot("download"),
            NavSoulseek,
            Shot("soulseek"),
            Nav(Page::Leaderboard),
            Shot("leaderboard"),
            Nav(Page::Storage),
            Shot("storage"),
            Nav(Page::BugReports),
            Shot("bug-reports"),
            Nav(Page::Settings(Section::Account)),
            Shot("settings-account"),
            Nav(Page::Settings(Section::Appearance)),
            Shot("settings-appearance"),
            Nav(Page::Settings(Section::Playback)),
            Shot("settings-playback"),
            ScrollToEnd,
            Shot("settings-playback-end"),
            Nav(Page::Settings(Section::Computer)),
            Shot("settings-computer"),
            Nav(Page::Settings(Section::Downloads)),
            Shot("settings-downloads"),
            // 0.32, the plugins: installed (with their status) and available; then an old link to a source card,
            // which opens that plugin's card (Torrents & Usenet, when it is installed)
            Nav(Page::Settings(Section::Plugins)),
            Wait(3),
            Shot("settings-plugins"),
            NavHash("#/settings?s=torznab"),
            Wait(3),
            Shot("settings-plugins-card"),
            ScrollToEnd,
            Shot("settings-plugins-end"),
            // 0.33, the file manager: the roots, then the library's folder
            Nav(Page::Settings(Section::Files)),
            Wait(2),
            Shot("settings-files"),
            FilesRoot("library"),
            Shot("settings-files-library"),
            Nav(Page::Home),
            Customize(true),
            Shot("player-customize"),
            Customize(false),
            Nav(Page::Offline),
            Shot("offline"),
        ]);
        // 0.31, group listen. By default a group listen started from what plays (the start
        // dialog, the group dialog, the bar's pill, Up next with who added each song, Your
        // devices and Home's row). With ENCORE_TOUR_GROUP_JOIN=<invite link> this computer
        // joins that group instead, as a member who may not control it (the invite question,
        // the controls that are off, a pause here only and Catch up). Left again at the end.
        {
            use crate::player::group::Msg as G;
            use crate::player::Message as P;
            match std::env::var("ENCORE_TOUR_GROUP_JOIN").ok().filter(|l| !l.trim().is_empty()) {
                Some(link) => {
                    let code = crate::player::group::model::code_from(&link).unwrap_or_default();
                    steps.extend([
                        Nav(Page::Home),
                        Wait(4),
                        Shot("group-home-join"),
                        Player(P::Group(G::OpenJoin)),
                        Player(P::Group(G::JoinText(link))),
                        Shot("group-join"),
                        Player(P::Group(G::JoinSubmit)),
                        Wait(3),
                        Shot("group-invite"),
                        Player(P::Group(G::JoinCode(code))),
                        Wait(8),
                        Shot("group-member-bar"),
                        Player(P::Toggle),
                        Wait(3),
                        Shot("group-paused-here"),
                        Full(Some(crate::player::Pane::Queue)),
                        Wait(2),
                        Shot("group-member-up-next"),
                        Full(None),
                        Player(P::Group(G::OpenSheet)),
                        Shot("group-member-dialog"),
                        Player(P::Group(G::Close)),
                        Player(P::Group(G::CatchUp)),
                        Wait(6),
                        Shot("group-caught-up"),
                        Wait(10),
                        Player(P::Group(G::Leave)),
                        Wait(2),
                    ]);
                }
                None => steps.extend([
                    Nav(Page::Home),
                    PlayFirstAlbum,
                    Wait(6),
                    Player(P::Group(G::OpenStart)),
                    Shot("group-start"),
                    Player(P::Group(G::Start)),
                    Wait(6),
                    Shot("group-dialog"),
                    Player(P::Group(G::Close)),
                    Wait(4),
                    Shot("group-bar"),
                    Full(Some(crate::player::Pane::Queue)),
                    Wait(2),
                    Shot("group-up-next"),
                    Full(None),
                    Player(P::Devices),
                    Wait(4),
                    Shot("group-devices"),
                    Player(P::CloseDialog),
                    Nav(Page::Home),
                    Wait(4),
                    Shot("group-home"),
                    Player(P::Group(G::Leave)),
                    Wait(2),
                ]),
            }
        }
        steps.push(Exit);
        Self {
            dir,
            login,
            steps,
            next: 0,
            wait: 0,
            first_album: None,
            artist: String::new(),
            saved: Vec::new(),
        }
    }
}

/// One step per tick.
pub fn subscription() -> Subscription<Message> {
    iced::time::every(TICK).map(|_| Message::Step)
}

pub fn update(app: &mut App, msg: Message) -> Task<AppMsg> {
    match msg {
        Message::Step => step(app),
        Message::Taken(name, shot) => {
            let Some(t) = app.tour.as_mut() else {
                return Task::none();
            };
            let _ = std::fs::create_dir_all(&t.dir);
            let path = t.dir.join(format!("{name}.png"));
            let (w, h) = (shot.size.width, shot.size.height);
            match image::RgbaImage::from_raw(w, h, shot.rgba.to_vec()) {
                Some(img) => match img.save(&path) {
                    Ok(()) => {
                        log::info!("tour: saved {} ({w}x{h})", path.display());
                        t.saved.push(name);
                    }
                    Err(e) => log::error!("tour: could not save {}: {e}", path.display()),
                },
                None => log::error!("tour: bad screenshot size {w}x{h} for {name}"),
            }
            Task::none()
        }
        Message::Album(Ok(album)) => {
            let id = crate::fmt::s(&album, "id");
            let tracks = crate::fmt::items(&album["tracks"]);
            if let Some(t) = app.tour.as_mut() {
                t.first_album = (!id.is_empty()).then_some(id);
                let first = tracks.first().cloned().unwrap_or_default();
                t.artist = [crate::fmt::s(&album, "albumartist"), crate::fmt::s(&first, "albumartist"), crate::fmt::s(&first, "artist")]
                    .into_iter()
                    .find(|a| !a.is_empty())
                    .unwrap_or_default();
            }
            if tracks.is_empty() {
                log::warn!("tour: the first album has no tracks");
                return Task::none();
            }
            log::info!("tour: playing {} ({} songs)", crate::fmt::s(&album, "album"), tracks.len());
            app.play(tracks, 0)
        }
        Message::Album(Err(e)) => {
            log::warn!("tour: no album to play: {}", e.message);
            Task::none()
        }
    }
}

fn step(app: &mut App) -> Task<AppMsg> {
    let Some(t) = app.tour.as_mut() else {
        return Task::none();
    };
    if t.wait > 0 {
        t.wait -= 1;
        return Task::none();
    }
    let Some(step) = t.steps.get(t.next).cloned() else {
        return iced::exit();
    };
    t.next += 1;
    log::info!("tour: {step:?}");
    match step {
        Step::Wait(n) => {
            t.wait = n;
            Task::none()
        }
        Step::Shot(name) => {
            t.wait = 1;
            let Some(id) = app.window else {
                log::error!("tour: no window to shoot");
                return Task::none();
            };
            iced::window::screenshot(id).map(move |shot| AppMsg::Tour(Message::Taken(name.to_string(), shot)))
        }
        Step::SignIn => {
            let Some((u, p)) = t.login.clone() else {
                return Task::none();
            };
            crate::pages::login::prefill(&mut app.login, &u, &p);
            app.update(AppMsg::Login(crate::pages::login::Message::Submit))
        }
        Step::Nav(page) => {
            t.wait = 3;
            app.update(AppMsg::Nav(page))
        }
        Step::NavHash(hash) => {
            t.wait = 3;
            app.update(AppMsg::Nav(Page::from_hash(hash).unwrap_or(Page::Home)))
        }
        Step::NavFirstAlbum => match t.first_album.clone() {
            Some(id) => {
                t.wait = 3;
                app.update(AppMsg::Nav(Page::LibraryAlbum(id)))
            }
            None => {
                // nothing to open: skip its picture too
                t.next += 1;
                Task::none()
            }
        },
        Step::NavSoulseek => {
            if app.session.has_feature("soulseek") {
                t.wait = 3;
                app.update(AppMsg::Nav(Page::Soulseek))
            } else {
                t.next += 1;
                Task::none()
            }
        }
        Step::PlayFirstAlbum => {
            let api = app.api.clone();
            Task::perform(
                async move {
                    let recent = api.get("/library/recent?limit=1").await?;
                    let id = crate::fmt::items(&recent)
                        .first()
                        .map(|a| crate::fmt::s(a, "id"))
                        .unwrap_or_default();
                    if id.is_empty() {
                        return Err(crate::api::ApiError::new(404, "The library has no albums"));
                    }
                    api.get(&format!("/library/albums/{id}")).await
                },
                |r| AppMsg::Tour(Message::Album(r)),
            )
        }
        Step::RowMenu => {
            if !matches!(app.page, Page::LibraryAlbum(_)) {
                // no album was opened: skip the picture too
                t.next += 2;
                return Task::none();
            }
            t.wait = 1;
            // where the pointer would be over the first song
            app.cursor = iced::Point::new(760.0, 430.0);
            app.update(AppMsg::LibraryAlbum(crate::pages::library_album::Message::TrackMenu(0)))
        }
        Step::RequestDetails => match app.requests.first_request() {
            Some(id) => {
                t.wait = 4;
                app.update(AppMsg::Requests(crate::pages::requests::Message::List(
                    crate::widgets::request_list::Msg::Details(id),
                )))
            }
            None => {
                t.next += 1;
                Task::none()
            }
        },
        Step::CloseDetails => app.update(AppMsg::Details(crate::widgets::details::Msg::Close)),
        Step::RequestPick => match app.requests.first_request_row() {
            Some(r) => {
                t.wait = 4;
                app.update(AppMsg::Requests(crate::pages::requests::Message::List(
                    crate::widgets::request_list::Msg::Pick(r),
                )))
            }
            None => {
                t.next += 1;
                Task::none()
            }
        },
        Step::ClosePick => app.update(AppMsg::Requests(crate::pages::requests::Message::List(
            crate::widgets::request_list::Msg::Picker(crate::widgets::picker::Message::Close),
        ))),
        Step::Full(pane) => {
            t.wait = 2;
            match pane {
                Some(p) => app.update(AppMsg::Player(crate::player::Message::SetPane(p))),
                None => app.update(AppMsg::Player(crate::player::Message::CloseFull)),
            }
        }
        Step::Normalize => {
            use crate::engine::normalize::{set_prefs, Mode, Prefs};
            set_prefs(Prefs { mode: Mode::Album, target: -14, clip: true });
            let mut s = crate::engine::settings::settings();
            s.dsp.bit_perfect = false;
            s.output.rate = cadenza::config::RateSetting::Source;
            if let Err(e) = crate::engine::settings::apply(s) {
                log::warn!("tour: Cadenza did not take the settings: {e}");
            }
            Task::none()
        }
        Step::ScrollToEnd => {
            t.wait = 1;
            iced::widget::operation::snap_to(
                crate::app::MAIN_SCROLL,
                iced::widget::scrollable::RelativeOffset { x: 0.0, y: 1.0 },
            )
        }
        Step::Player(m) => {
            t.wait = 1;
            app.update(AppMsg::Player(m))
        }
        Step::NavLibrarySearch | Step::NavDiscoverSearch | Step::NavArtist => {
            if t.artist.is_empty() {
                t.next += 1;
                return Task::none();
            }
            t.wait = 4;
            let q = t.artist.clone();
            let page = match step {
                Step::NavLibrarySearch => Page::Library {
                    tab: String::new(),
                    query: crate::pages::LibraryQuery { q, ..Default::default() },
                },
                Step::NavDiscoverSearch => Page::Search { q },
                _ => Page::ArtistName(q),
            };
            app.update(AppMsg::Nav(page))
        }
        Step::CloseRowMenu => {
            app.update(AppMsg::LibraryAlbum(crate::pages::library_album::Message::Shared(
                crate::pages::library::Act::CloseMenu,
            )))
        }
        Step::Customize(on) => {
            t.wait = 2;
            app.update(AppMsg::Player(crate::player::Message::Customize(on)))
        }
        Step::FilesRoot(id) => {
            t.wait = 3;
            app.update(AppMsg::Settings(crate::pages::settings::Message::Files(
                crate::pages::files::Message::OpenRoot(id.to_string()),
            )))
        }
        Step::Exit => {
            let saved = t.saved.len();
            log::info!("tour: done, {saved} pictures in {}", t.dir.display());
            let st = crate::engine::status();
            log::info!(
                "tour: engine state {:?}, position {:.1}s, output {}, error {:?}",
                st.state,
                st.position,
                st.output,
                st.error
            );
            crate::engine::shutdown();
            iced::exit()
        }
    }
}
