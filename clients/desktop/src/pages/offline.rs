//! Offline music: the songs saved on this computer (desktop/offline.rs), playing through Cadenza
//! without the server. Behaviour reference: the classic app's offline page
//! (clients/app/ui/offline.html) and its words. Reachable from the sign-in page when the server
//! can't be reached (then it has its own Retry, Change server and player, since the app shows no
//! sidebar or player bar there) and from Settings → This computer.

use crate::desktop::offline::{self, Song};
use crate::engine::PlayState;
use crate::fmt;
use crate::pages::{Ctx, El, Page};
use crate::player;
use crate::theme::{self, Badge, Btn};
use crate::widgets::{
    self, btn, btn_icon, card, card_flat, empty, icon, icon_btn, small_muted, t, Icon, TrackRow,
};
use cadenza::decode::Source;
use iced::widget::{button, column, container, image, progress_bar, row, scrollable, slider};
use iced::{Alignment, Background, Border, ContentFit, Length, Subscription, Task};
use std::time::Duration;

#[derive(Debug, Default)]
pub struct State {
    /// Newest first.
    songs: Vec<Song>,
    /// The store's revision the list was read at.
    revision: u64,
    /// "Remove all" asks first.
    confirm: bool,
    /// The signed-out player's seek bar while it is held.
    drag: Option<f64>,
}

#[derive(Clone, Debug)]
pub enum Message {
    /// Once a second: reload when the store changed.
    Poll,
    Play(usize),
    PlayAll,
    Shuffle,
    Remove(usize),
    AskRemoveAll,
    RemoveAll,
    CloseDialog,
    /// Signed out: try the server again (signed in again when it answers).
    Retry,
    /// Signed out: the server picker.
    ChangeServer,
    /// Signed out: the transport of this page's own player.
    Player(player::Message),
    /// Signed out: its seek bar held at a place, and let go.
    Seek(f64),
    SeekDone,
}

/// The songs, newest first.
fn load(state: &mut State) {
    state.revision = offline::revision();
    let mut songs = offline::list();
    songs.sort_by(|a, b| b.saved_at.total_cmp(&a.saved_at));
    state.songs = songs;
}

/// Called when the app navigates here (with the route's parameters).
pub fn open(state: &mut State, _ctx: &mut Ctx) -> Task<Message> {
    state.confirm = false;
    load(state);
    Task::none()
}

/// A random order (xorshift seeded by the clock; good enough for a shuffle).
pub fn shuffle<T>(v: &mut [T]) {
    let mut x = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9E37_79B9_7F4A_7C15)
        | 1;
    for i in (1..v.len()).rev() {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        v.swap(i, (x % (i as u64 + 1)) as usize);
    }
}

pub fn update(state: &mut State, msg: Message, ctx: &mut Ctx) -> Task<Message> {
    match msg {
        Message::Poll => {
            if offline::revision() != state.revision {
                load(state);
            }
        }
        Message::Play(i) => {
            if let Err(e) = offline::play(&state.songs, i) {
                ctx.toast_err(e);
            }
        }
        Message::PlayAll => {
            if let Err(e) = offline::play(&state.songs, 0) {
                ctx.toast_err(e);
            }
        }
        Message::Shuffle => {
            let mut list = state.songs.clone();
            shuffle(&mut list);
            if let Err(e) = offline::play(&list, 0) {
                ctx.toast_err(e);
            }
        }
        Message::Remove(i) => {
            if let Some(s) = state.songs.get(i).cloned() {
                match offline::remove(&s) {
                    Ok(()) => ctx.toast_ok(format!("Removed from this computer: {}", s.title)),
                    Err(e) => ctx.toast_err(format!("Couldn't remove it: {e}")),
                }
                load(state);
            }
        }
        Message::AskRemoveAll => state.confirm = true,
        Message::CloseDialog => state.confirm = false,
        Message::RemoveAll => {
            state.confirm = false;
            match offline::remove_all() {
                Ok(()) => ctx.toast_ok("Removed every song from this computer"),
                Err(e) => ctx.toast_err(format!("Couldn't remove them: {e}")),
            }
            load(state);
        }
        Message::Retry => {
            ctx.toast("Trying the server again…");
            ctx.refresh_session();
        }
        Message::ChangeServer => ctx.navigate(Page::Login),
        Message::Player(m) => ctx.player_msg(m),
        Message::Seek(v) => state.drag = Some(v),
        Message::SeekDone => {
            if let Some(v) = state.drag.take() {
                ctx.player_msg(player::Message::SeekTo(v));
            }
        }
    }
    Task::none()
}

/// "12 song(s) saved on this computer · 1.2 GB".
pub fn summary(songs: &[Song]) -> String {
    if songs.is_empty() {
        return "No songs saved on this computer yet.".into();
    }
    format!(
        "{} song(s) saved on this computer · {}",
        songs.len(),
        fmt::size(offline::total_bytes(songs) as f64)
    )
}

/// A plain square of `c` for songs without a cover (kept, so the picture is uploaded once).
fn blank(c: iced::Color) -> image::Handle {
    static BLANK: std::sync::Mutex<Option<([u8; 4], image::Handle)>> = std::sync::Mutex::new(None);
    let px = c.into_rgba8();
    let mut b = BLANK.lock().unwrap_or_else(|e| e.into_inner());
    match b.as_ref() {
        Some((k, h)) if *k == px => h.clone(),
        _ => {
            let h = image::Handle::from_rgba(1, 1, px.to_vec());
            *b = Some((px, h.clone()));
            h
        }
    }
}

/// Whether the engine plays this saved song now.
fn is_playing(song: &Song, ctx: &Ctx) -> bool {
    ctx.player.track().is_some_and(|t| match &t.source {
        Source::Path { path } => *path == offline::audio_path(song),
        _ => false,
    })
}

fn banner<'a>(ctx: &Ctx<'a>) -> El<'a, Message> {
    let p = ctx.look;
    container(
        row![
            icon(Icon::X, 18.0, p.warn),
            t("Not connected to your Encore server – playing music saved on this computer.")
                .size(14)
                .color(p.warn)
                .width(Length::Fill),
            btn("Retry", Btn::Primary, Some(Message::Retry)),
        ]
        .spacing(12)
        .align_y(Alignment::Center),
    )
    .padding([12, 16])
    .width(Length::Fill)
    .style(move |_| container::Style {
        background: Some(Background::Color(p.warn.scale_alpha(0.12))),
        border: Border {
            color: p.warn.scale_alpha(0.4),
            width: 1.0,
            radius: theme::RADIUS.into(),
        },
        ..container::Style::default()
    })
    .into()
}

/// The page's own player while signed out (the app has no player bar then).
fn player_card<'a>(drag: Option<f64>, ctx: &Ctx<'a>) -> Option<El<'a, Message>> {
    let p = ctx.look;
    let st = &ctx.player.status;
    let tr = st.track.as_ref()?;
    let playing = st.state == PlayState::Playing;
    let duration = st.duration.or(tr.duration).unwrap_or(0.0);
    let position = drag.unwrap_or(st.position);
    let position = if position.is_finite() {
        position.clamp(0.0, duration.max(0.0))
    } else {
        0.0
    };
    let art: El<'a, Message> = match tr
        .cover
        .as_deref()
        .filter(|c| std::path::Path::new(c).is_file())
    {
        Some(c) => image(image::Handle::from_path(c))
            .width(48)
            .height(48)
            .content_fit(ContentFit::Cover)
            .border_radius(8.0)
            .into(),
        None => container(icon(Icon::Music, 20.0, p.faint))
            .width(48)
            .height(48)
            .center(48)
            .style(theme::art)
            .into(),
    };
    let pl = |m: player::Message| Some(Message::Player(m));
    let play = button(
        container(icon(
            if playing { Icon::Pause } else { Icon::Play },
            18.0,
            p.on_accent,
        ))
        .width(36)
        .height(36)
        .center(36),
    )
    .padding(0)
    .style(theme::btn(Btn::Play))
    .on_press(Message::Player(player::Message::Toggle));
    let seek: El<'a, Message> = if duration > 0.0 {
        slider(0.0..=duration, position, Message::Seek)
            .on_release(Message::SeekDone)
            .step(0.5)
            .style(theme::slider_style)
            .into()
    } else {
        progress_bar(0.0..=1.0, 0.0)
            .girth(4)
            .style(theme::progress)
            .into()
    };
    Some(
        card(
            column![
                row![
                    art,
                    column![
                        t(tr.title.clone()).size(14).font(theme::semibold()),
                        small_muted(tr.artist.clone()),
                    ]
                    .spacing(2)
                    .width(Length::Fill),
                    icon_btn(Icon::Prev, "Previous", pl(player::Message::Prev)),
                    play,
                    icon_btn(Icon::Next, "Next", pl(player::Message::Next)),
                ]
                .spacing(12)
                .align_y(Alignment::Center),
                row![
                    small_muted(fmt::mmss(position)),
                    container(seek).width(Length::Fill),
                    small_muted(fmt::mmss(duration)),
                ]
                .spacing(10)
                .align_y(Alignment::Center),
            ]
            .spacing(10),
        )
        .width(Length::Fill)
        .into(),
    )
}

pub fn view<'a>(state: &'a State, ctx: &Ctx<'a>) -> El<'a, Message> {
    let signed_in = ctx.session.signed_in();
    let mut page = column![].spacing(18).width(Length::Fill);
    if !signed_in {
        page = page.push(banner(ctx));
    }
    let has = !state.songs.is_empty();
    let actions: Vec<El<'a, Message>> = vec![
        btn_icon(
            Icon::Play,
            "Play all",
            Btn::Primary,
            has.then_some(Message::PlayAll),
        )
        .into(),
        btn_icon(
            Icon::Shuffle,
            "Shuffle",
            Btn::Default,
            has.then_some(Message::Shuffle),
        )
        .into(),
    ];
    page = page.push(widgets::page_head(
        if signed_in {
            "Offline music"
        } else {
            "Offline"
        },
        Some(small_muted(summary(&state.songs)).into()),
        actions,
    ));
    if !signed_in {
        if let Some(pc) = player_card(state.drag, ctx) {
            page = page.push(pc);
        }
    }
    if let Some(pr) = offline::progress() {
        page = page.push(
            card(
                column![
                    t(format!(
                        "Saving {} of {}…",
                        (pr.done + 1).min(pr.total),
                        pr.total
                    ))
                    .size(14)
                    .font(theme::semibold()),
                    small_muted(pr.current.clone()),
                    progress_bar(0.0..=pr.total as f32, pr.done as f32)
                        .girth(6)
                        .style(theme::progress),
                ]
                .spacing(8),
            )
            .width(Length::Fill),
        );
    }
    if has {
        // Songs without a cover get a plain square, so the rows line up.
        let blank = blank(ctx.look.panel2);
        let mut list = column![];
        for (i, s) in state.songs.iter().enumerate() {
            let sub = if s.album.is_empty() {
                s.artist.clone()
            } else {
                format!("{} · {}", s.artist, s.album)
            };
            let art = offline::art_path(s)
                .map(image::Handle::from_path)
                .unwrap_or_else(|| blank.clone());
            let mut row = TrackRow::new(s.title.clone())
                .sub(sub)
                .cover(Some(art))
                .quality(s.quality.clone())
                .duration((s.duration > 0.0).then_some(s.duration))
                .playing(is_playing(s, ctx))
                .on_play(Message::Play(i))
                .extra(widgets::badge(fmt::size(s.bytes as f64), Badge::Plain))
                .extra(icon_btn(
                    Icon::Trash,
                    "Remove from this computer",
                    Some(Message::Remove(i)),
                ));
            if signed_in && !s.source_server.is_empty() && s.source_server != ctx.base() {
                row = row.note(format!("From {}", s.source_server));
            }
            list = list.push(row.view());
        }
        page = page.push(card_flat(list).width(Length::Fill));
        page = page.push(
            row![btn("Remove all", Btn::Danger, Some(Message::AskRemoveAll))]
                .spacing(10)
                .align_y(Alignment::Center),
        );
    } else {
        page = page.push(empty(
            Icon::Download,
            "Nothing saved yet",
            "When you're connected, use ⋯ → “Save for offline” on songs, albums or playlists.",
        ));
    }
    if !signed_in {
        page = page.push(row![btn(
            "Change server",
            Btn::Default,
            Some(Message::ChangeServer)
        )]);
        // Signed out the app draws the page alone: it scrolls by itself.
        return scrollable(
            container(container(page).max_width(1000).width(Length::Fill))
                .padding([26, 32])
                .center_x(Length::Fill),
        )
        .height(Length::Fill)
        .style(theme::scroll)
        .into();
    }
    page.into()
}

/// A modal or ⋯ menu to lay over the whole window, when one is open.
pub fn overlay<'a>(state: &'a State, _ctx: &Ctx<'a>) -> Option<El<'a, Message>> {
    state.confirm.then(|| {
        widgets::confirm(
            "Remove all offline songs?",
            "Every song saved on this computer is deleted from it. Your library on the server stays as it is.",
            "Remove all",
            true,
            Message::RemoveAll,
            Message::CloseDialog,
        )
    })
}

pub fn subscription(_state: &State) -> Subscription<Message> {
    iced::time::every(Duration::from_secs(1)).map(|_| Message::Poll)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_summary_counts_songs_and_space() {
        assert_eq!(summary(&[]), "No songs saved on this computer yet.");
        let s = Song {
            bytes: 30_000_000,
            ..Song::default()
        };
        assert_eq!(
            summary(&[s.clone(), s]),
            "2 song(s) saved on this computer · 60.0 MB"
        );
    }

    #[test]
    fn a_shuffle_keeps_every_song() {
        let mut v: Vec<u32> = (0..50).collect();
        shuffle(&mut v);
        let mut sorted = v.clone();
        sorted.sort();
        assert_eq!(sorted, (0..50).collect::<Vec<_>>());
        let mut one = [1];
        shuffle(&mut one);
        assert_eq!(one, [1]);
    }
}
