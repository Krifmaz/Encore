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
    /// Open the album that was played (skipped when none was).
    NavFirstAlbum,
    /// Soulseek, when the server has it.
    NavSoulseek,
    PlayFirstAlbum,
    Customize(bool),
    /// Open the first song's ⋯ menu on the album page, where a right-click would.
    RowMenu,
    /// Close the menu again.
    CloseRowMenu,
    /// Open the first request's details (skipped when the list is empty).
    RequestDetails,
    /// Close the details dialog.
    CloseDetails,
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
            Wait(8),
            Shot("playing"),
            Nav(Page::Search { q: String::new() }),
            Shot("search"),
            Nav(Page::ForYou),
            Shot("for-you"),
            Nav(Page::library("")),
            Shot("library"),
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
            Nav(Page::Settings(Section::Computer)),
            Shot("settings-computer"),
            Nav(Page::Settings(Section::Apps)),
            Shot("settings-apps"),
            Nav(Page::Home),
            Customize(true),
            Shot("player-customize"),
            Customize(false),
            Nav(Page::Offline),
            Shot("offline"),
            Exit,
        ]);
        Self {
            dir,
            login,
            steps,
            next: 0,
            wait: 0,
            first_album: None,
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
        Step::CloseRowMenu => {
            app.update(AppMsg::LibraryAlbum(crate::pages::library_album::Message::Shared(
                crate::pages::library::Act::CloseMenu,
            )))
        }
        Step::Customize(on) => {
            t.wait = 2;
            app.update(AppMsg::Player(crate::player::Message::Customize(on)))
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
