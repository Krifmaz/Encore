//! The mini player: a small window that stays on top of the others, with the cover, the song,
//! previous / play-pause / next, the seek bar, a button back to Encore and ✕ (the classic
//! app's ui/mini.html). It follows `config.mini_player`: the tray, Settings → This computer and
//! the player's ⋯ menu change that, the app's tick opens or closes the window. Dragging its
//! background moves it.

use super::Msg;
use crate::app::{App, Message as AppMsg};
use crate::fmt;
use crate::player;
use crate::theme::{self, Btn};
use crate::widgets::{icon, t, El, Icon};
use iced::widget::{button, column, container, image, mouse_area, progress_bar, row, slider};
use iced::{window, Alignment, ContentFit, Length, Size, Task};
use std::sync::Mutex;

/// The window's size (the classic app's 380 × 132; wider is allowed, taller is not).
pub const SIZE: Size = Size::new(380.0, 132.0);

static ID: Mutex<Option<window::Id>> = Mutex::new(None);

fn slot() -> std::sync::MutexGuard<'static, Option<window::Id>> {
    ID.lock().unwrap_or_else(|e| e.into_inner())
}

/// Whether `id` is the mini player's window.
pub fn is(id: window::Id) -> bool {
    *slot() == Some(id)
}

/// The mini player's window, when open.
pub fn id() -> Option<window::Id> {
    *slot()
}

pub fn is_open() -> bool {
    slot().is_some()
}

/// The window went away (closed from outside).
pub fn forget(id: window::Id) {
    let mut s = slot();
    if *s == Some(id) {
        *s = None;
    }
}

/// The mini player window's settings: small, on top, no frame.
pub fn settings() -> window::Settings {
    window::Settings {
        size: SIZE,
        min_size: Some(Size::new(300.0, SIZE.height)),
        max_size: Some(Size::new(720.0, SIZE.height)),
        decorations: false,
        resizable: true,
        level: window::Level::AlwaysOnTop,
        icon: window::icon::from_file_data(include_bytes!("../../packaging/icons/64x64.png"), None)
            .ok(),
        exit_on_close_request: false,
        // no second task bar button on Windows; one app id with the main window on Linux
        #[cfg(target_os = "windows")]
        platform_specific: window::settings::PlatformSpecific {
            skip_taskbar: true,
            ..Default::default()
        },
        #[cfg(target_os = "linux")]
        platform_specific: window::settings::PlatformSpecific {
            application_id: "encore-desktop".into(),
            ..Default::default()
        },
        ..window::Settings::default()
    }
}

/// Open the window (nothing when it is open).
pub fn open() -> Task<AppMsg> {
    let mut s = slot();
    if s.is_some() {
        return Task::none();
    }
    let (id, task) = window::open(settings());
    *s = Some(id);
    log::info!("mini player open");
    task.discard()
}

/// Close the window (nothing when it is closed).
pub fn close() -> Task<AppMsg> {
    match slot().take() {
        Some(id) => window::close(id),
        None => Task::none(),
    }
}

/// Open or close the window as the config says (the app's tick and the tray).
pub fn sync(app: &App) -> Task<AppMsg> {
    let want = app.config.mini_player && app.window.is_some();
    match (want, is_open()) {
        (true, false) => open(),
        (false, true) => close(),
        _ => Task::none(),
    }
}

/// The cover of the song (a server cover through the app's picture cache, or the file of a
/// song saved on this computer).
fn cover(app: &App, url: Option<&str>) -> Option<image::Handle> {
    let url = url.filter(|u| !u.is_empty())?;
    let path = std::path::Path::new(url);
    if !url.starts_with("http") && path.is_absolute() {
        return path.is_file().then(|| image::Handle::from_path(path));
    }
    app.covers
        .get(&crate::widgets::cover::absolute(&app.api.base(), url))
}

fn small_btn<'a>(i: Icon, on: Option<Msg>) -> El<'a, Msg> {
    let p = theme::look();
    let color = if on.is_some() { p.text } else { p.faint };
    button(icon(i, 16.0, color))
        .padding(7)
        .style(theme::btn(Btn::Icon))
        .on_press_maybe(on)
        .into()
}

/// The window's content.
pub fn view(app: &App) -> El<'_, Msg> {
    let p = app.look;
    let now = app.player.now();
    let has = now.as_ref().is_some_and(|n| !n.title.is_empty());
    let art: El<'_, Msg> = match cover(app, now.as_ref().and_then(|n| n.cover.as_deref())) {
        Some(h) => image(h)
            .width(100)
            .height(100)
            .content_fit(ContentFit::Cover)
            .border_radius(10.0)
            .into(),
        None => container(icon(Icon::Music, 30.0, p.faint))
            .width(100)
            .height(100)
            .center(100)
            .style(theme::art)
            .into(),
    };
    let (title, artist) = match now.as_ref().filter(|_| has) {
        Some(n) => (n.title.clone(), n.artist.clone()),
        None => ("Nothing playing".to_string(), String::new()),
    };
    let head = row![
        column![
            t(title)
                .size(14)
                .font(theme::semibold())
                .wrapping(iced::widget::text::Wrapping::None),
            t(artist)
                .size(12)
                .color(p.muted)
                .wrapping(iced::widget::text::Wrapping::None),
        ]
        .spacing(1)
        .width(Length::Fill)
        .clip(true),
        small_btn(Icon::External, Some(Msg::MiniShowMain)),
        small_btn(Icon::X, Some(Msg::MiniClose)),
    ]
    .spacing(2)
    .align_y(Alignment::Start);

    let playing = now.as_ref().is_some_and(|n| n.playing);
    let on = |m: player::Message| has.then_some(Msg::Player(m));
    let play = button(
        container(icon(
            if playing { Icon::Pause } else { Icon::Play },
            16.0,
            p.on_accent,
        ))
        .width(30)
        .height(30)
        .center(30),
    )
    .padding(0)
    .style(theme::btn(Btn::Play))
    .on_press_maybe(on(player::Message::Toggle));
    let controls = row![
        small_btn(Icon::Prev, on(player::Message::Prev)),
        play,
        small_btn(Icon::Next, on(player::Message::Next)),
    ]
    .spacing(6)
    .align_y(Alignment::Center);

    let duration = now.as_ref().map(|n| n.duration).unwrap_or(0.0);
    let position = app
        .player
        .seek_drag
        .or(now.as_ref().map(|n| n.position))
        .unwrap_or(0.0);
    let position = if position.is_finite() {
        position.clamp(0.0, duration.max(0.0))
    } else {
        0.0
    };
    let seek: El<'_, Msg> = if has && duration > 0.0 {
        slider(0.0..=duration, position, |v| {
            Msg::Player(player::Message::SeekDrag(v))
        })
        .on_release(Msg::Player(player::Message::SeekDone))
        .step(0.5)
        .style(theme::slider_style)
        .into()
    } else {
        progress_bar(0.0..=1.0, 0.0)
            .girth(4)
            .style(theme::progress)
            .into()
    };
    let times = row![
        t(fmt::mmss(position)).size(11).color(p.muted),
        container(seek).width(Length::Fill),
        t(fmt::mmss(duration)).size(11).color(p.muted),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    let body = row![
        art,
        column![head, controls, times]
            .spacing(4)
            .width(Length::Fill),
    ]
    .spacing(12)
    .align_y(Alignment::Center);
    let frame = container(body)
        .padding(14)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(theme::player_bar);
    mouse_area(frame).on_press(Msg::MiniDrag).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_is_small_on_top_and_frameless() {
        let s = settings();
        assert_eq!(s.size, SIZE);
        assert!(!s.decorations && !s.exit_on_close_request);
        assert_eq!(s.level, window::Level::AlwaysOnTop);
        assert_eq!(s.max_size.map(|m| m.height), Some(SIZE.height));
        assert!(!is_open() && id().is_none());
    }

    #[test]
    fn the_view_builds_with_and_without_a_song() {
        let (mut app, _boot) = crate::app::App::boot(crate::app::Flags::default());
        let _ = view(&app);
        let mut st = crate::engine::Status {
            track: Some(crate::engine::tracks::from_file(
                "/nowhere/a.flac".into(),
                &serde_json::json!({ "id": "7", "title": "Song", "artist": "Band", "art": "/nowhere/a.jpg" }),
            )),
            state: crate::engine::PlayState::Playing,
            position: 10.0,
            duration: Some(100.0),
            ..Default::default()
        };
        app.player.status = std::sync::Arc::new(st.clone());
        let _ = view(&app);
        st.duration = None;
        st.position = f64::NAN;
        app.player.status = std::sync::Arc::new(st);
        let _ = view(&app);
    }
}
