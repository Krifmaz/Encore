//! The full view: Now playing, Lyrics and Up next, laid over the whole window.
//!
//! Lyrics is lyrics mode (p6 item 4, the web's player/full.rs, the phone's PlayerScreens.kt):
//! the cover becomes a 56 px thumbnail in a compact bar at the top (title and artist, play /
//! pause, a thin seek line) and the lyrics take the column, the current line in the middle.
//! "Back to the cover" (or a click on the compact bar: thumbnail, title, artist) goes back to Now playing; the mode is
//! remembered (`encore.lyricsMode`). Long titles scroll sideways (widgets::marquee).

use super::{bar, cover_handle, Message, Now, Pane, State};
use crate::fmt;
use crate::pages::{Ctx, El};
use crate::theme::{self, Btn};
use crate::widgets::marquee::marquee;
use crate::widgets::{self, icon, icon_btn, t, tip_box, Icon};
use iced::widget::{button, column, container, image, mouse_area, opaque, row, scrollable, slider, space, toggler, tooltip};
use iced::{Alignment, ContentFit, Length};

/// A row of Up next, from the engine's queue or the linked device's.
struct QueueRow {
    title: String,
    artist: String,
    cover: Option<String>,
}

fn queue_rows(state: &State) -> (Vec<QueueRow>, usize) {
    if let Some(l) = state.devices.linked.as_ref() {
        let idx = l.state["index"].as_u64().map(|i| i as usize).unwrap_or(0);
        let rows = fmt::items(&l.state["queue"])
            .into_iter()
            .map(|x| QueueRow {
                title: fmt::s(&x, "title"),
                artist: fmt::s(&x, "artist") + &fmt::feat_text(&x),
                cover: x["album_id"]
                    .as_str()
                    .filter(|a| !a.is_empty())
                    .map(|a| format!("/api/library/albums/{a}/cover")),
            })
            .collect();
        return (rows, idx);
    }
    let st = &state.status;
    (
        st.queue
            .iter()
            .map(|x| QueueRow {
                title: x.title.clone(),
                artist: x.artist.clone(),
                cover: x.cover.clone(),
            })
            .collect(),
        st.index.unwrap_or(0),
    )
}

fn art<'a>(ctx: &Ctx<'a>, cover: Option<&str>, size: f32) -> El<'a, Message> {
    let p = ctx.look;
    match cover.and_then(|c| cover_handle(ctx, c, 600)) {
        Some(h) => image(h)
            .width(size)
            .height(size)
            .content_fit(ContentFit::Cover)
            .border_radius(theme::RADIUS)
            .into(),
        None => container(icon(Icon::Music, size * 0.3, p.faint))
            .width(size)
            .height(size)
            .center(size)
            .style(theme::art)
            .into(),
    }
}

/// About how wide a line of text is (to share a line between the artist and the album).
fn text_width(s: &str, size: f32) -> f32 {
    s.chars().count() as f32 * size * 0.55
}

/// How the artist · album line shares `width`: the room each gets (None: no limit), so a long
/// artist name leaves room for the album and each scrolls in its own part.
pub fn line_shares(artist: &str, album: &str, size: f32, width: f32) -> (Option<f32>, Option<f32>) {
    let (a, b) = (text_width(artist, size), text_width(album, size));
    let room = (width - size * 1.5).max(0.0);
    if artist.is_empty() || album.is_empty() || a + b <= room {
        return (None, None);
    }
    let share = (a / (a + b)).clamp(0.3, 0.7);
    let first = (room * share).min(a);
    (Some(first), Some(room - first))
}

/// "artist · album": the artist opens the artist page, the album the album; each scrolls when
/// it is too long for its part of `width`.
fn artist_line<'a>(now: &Now, size: f32, width: f32, fade: iced::Color) -> El<'a, Message> {
    let p = theme::look();
    let (wa, wb) = line_shares(&now.artist, &now.album, size, width);
    let part = |s: &str, w: Option<f32>, msg: Option<Message>| -> El<'a, Message> {
        let words = container(marquee(s.to_string()).size(size).color(p.muted).fade(fade)).max_width(w.unwrap_or(width));
        match msg {
            Some(m) => button(words).padding(0).style(theme::btn(Btn::Row)).on_press(m).into(),
            None => words.into(),
        }
    };
    let mut line = row![].spacing(4).align_y(Alignment::Center);
    if !now.artist.is_empty() {
        line = line.push(part(&now.artist, wa, Some(Message::GoToArtist)));
    }
    if !now.album.is_empty() {
        if !now.artist.is_empty() {
            line = line.push(widgets::muted("·").size(size));
        }
        line = line.push(part(&now.album, wb, now.album_id.as_ref().map(|_| Message::ViewAlbum)));
    }
    line.into()
}

/// The left column: art, words, seek, controls, volume, actions.
fn main_column<'a>(state: &'a State, ctx: &Ctx<'a>, now: &Now, art_size: f32) -> El<'a, Message> {
    let p = ctx.look;
    let (shuffle_on, repeat) = state.modes();
    let lib = now.id;
    // the words take the column's width; a title or line too long for it scrolls sideways
    let words_w = (art_size + 160.0).min((ctx.size.width - 96.0).max(200.0));
    let mut meta = column![container(marquee(now.title.clone()).size(22).font(theme::bold()).center().fade(p.bg)).max_width(words_w)]
        .spacing(4)
        .align_x(Alignment::Center);
    meta = meta.push(artist_line(now, 14.0, words_w, p.bg));
    if let Some(q) = bar::quality(state) {
        meta = meta.push(q);
    }

    // in a group listen Shuffle shuffles the group's Up next once and repeat is off, with the reason
    let grouped = state.group.state.is_some();
    let controls = row![
        if grouped {
            super::group::view::shuffle(state, 18.0)
        } else {
            bar_mode(Icon::Shuffle, shuffle_on, None, Message::SetShuffle(!shuffle_on))
        },
        bar::transport(state, Some(now), true),
        if grouped {
            super::group::view::off(Icon::Refresh, super::group::view::REPEAT_OFF, 18.0)
        } else {
            bar_mode(
                Icon::Refresh,
                repeat != super::Repeat::Off,
                (repeat == super::Repeat::One).then_some("1"),
                Message::SetRepeat(repeat.next()),
            )
        },
    ]
    .spacing(18)
    .align_y(Alignment::Center);

    let mut actions = row![].spacing(6).align_y(Alignment::Center);
    if let Some(id) = lib {
        // Surround songs (0.35) have no likes
        if id < crate::pages::library::SURROUND_IDS {
            let liked = state.is_liked(Some(id));
            actions = actions.push(bar_mode(Icon::Heart, liked, None, Message::LikeTrack(id)));
        }
        actions = actions.push(icon_btn(Icon::External, "Share link", Some(Message::Share)));
    }
    actions = actions.push(bar_mode(
        Icon::Music,
        state.full == Some(Pane::Lyrics),
        None,
        Message::SetPane(if state.full == Some(Pane::Lyrics) { Pane::Now } else { Pane::Lyrics }),
    ));
    actions = actions.push(icon_btn(Icon::More, "More", Some(Message::Menu)));

    column![
        art(ctx, now.cover.as_deref(), art_size),
        meta,
        container(bar::seek(state, Some(now))).width(Length::Fixed(art_size + 80.0)),
        controls,
        container(bar::volume(state, Some(now), 160.0)).width(Length::Shrink),
        actions,
    ]
    .spacing(18)
    .align_x(Alignment::Center)
    .width(Length::Fill)
    .into()
}

fn bar_mode<'a>(i: Icon, on: bool, sup: Option<&'static str>, msg: Message) -> El<'a, Message> {
    let p = theme::look();
    let color = if on { p.accent } else { p.text };
    let mut inner = row![icon(i, 18.0, color)].spacing(1).align_y(Alignment::Start);
    if let Some(s) = sup {
        inner = inner.push(t(s).size(9).font(theme::bold()).color(color));
    }
    button(inner).padding(9).style(theme::btn(Btn::Icon)).on_press(msg).into()
}

/// Up next: the songs after the current one, with move up and remove (in a group listen the
/// group's queue, with who added each song).
fn queue_pane<'a>(state: &'a State, ctx: &Ctx<'a>) -> El<'a, Message> {
    if state.group.state.is_some() {
        return super::group::view::up_next(state, ctx);
    }
    let p = ctx.look;
    let linked = state.devices.linked.as_ref();
    let (rows, idx) = queue_rows(state);
    let mut head = row![widgets::h3(match linked {
        Some(l) => format!("Up next on {}", l.name),
        None => "Up next".to_string(),
    })]
    .spacing(12)
    .align_y(Alignment::Center);
    head = head.push(space().width(Length::Fill));
    // Clear: everything after the song playing ("Up next cleared", with Undo); not for another
    // device's queue
    if linked.is_none() && rows.len() > idx + 1 {
        head = head.push(widgets::btn_sm("Clear", Btn::Ghost, Some(Message::ClearUpNext)));
    }
    if linked.is_none() {
        head = head.push(
            iced::widget::tooltip(toggler(state.autoplay(ctx))
                    .label("Then shuffle library")
                    .text_size(13)
                    .spacing(8)
                    .on_toggle(Message::Autoplay)
                    .style(theme::switch), crate::widgets::tip_box("When the queue ends, keep playing a shuffle of the whole library"), iced::widget::tooltip::Position::Bottom).gap(4),
        );
    }
    let up_next: Vec<(usize, &QueueRow)> = rows.iter().enumerate().skip(idx + 1).take(200).collect();
    let list: El<'a, Message> = if up_next.is_empty() {
        widgets::muted(if linked.is_some() {
            "Nothing queued there."
        } else if state.autoplay(ctx) {
            "Nothing queued – a shuffle of your library plays next."
        } else {
            "Nothing queued."
        })
        .size(13)
        .into()
    } else {
        let mut col = column![].spacing(0);
        for (i, r) in up_next {
            let thumb: El<'a, Message> = match r.cover.as_deref().and_then(|c| cover_handle(ctx, c, 80)) {
                Some(h) => image(h)
                    .width(40)
                    .height(40)
                    .content_fit(ContentFit::Cover)
                    .border_radius(6.0)
                    .into(),
                None => container(icon(Icon::Music, 16.0, p.faint))
                    .width(40)
                    .height(40)
                    .center(40)
                    .style(theme::art)
                    .into(),
            };
            let mut line = row![
                thumb,
                column![
                    t(r.title.clone()).size(14).font(theme::semibold()),
                    widgets::small_muted(r.artist.clone())
                ]
                .spacing(2)
                .width(Length::Fill),
            ]
            .spacing(12)
            .align_y(Alignment::Center);
            if linked.is_none() {
                line = line.push(icon_btn(
                    Icon::Chevron,
                    "Move up",
                    (i > idx + 1).then_some(Message::MoveUp(i)),
                ));
                line = line.push(icon_btn(Icon::X, "Remove", Some(Message::RemoveAt(i))));
            }
            col = col.push(
                button(container(line).padding([8, 10]))
                    .padding(0)
                    .width(Length::Fill)
                    .style(theme::btn(Btn::Row))
                    .on_press(Message::Jump(i)),
            );
            col = col.push(widgets::divider());
        }
        scrollable(col).height(Length::Fill).style(theme::scroll).into()
    };
    column![head, list].spacing(12).width(Length::Fill).height(Length::Fill).into()
}

/// The widest the lyrics column gets.
const LYRICS_W: f32 = 760.0;

/// Lyrics mode: the compact bar (the 56 px cover, title and artist, play / pause, a thin seek
/// line), the lyrics, "Back to the cover".
fn lyrics_mode<'a>(state: &'a State, ctx: &Ctx<'a>, now: &Now) -> El<'a, Message> {
    let p = ctx.look;
    let thumb = tooltip(
        button(art(ctx, now.cover.as_deref(), 56.0))
            .padding(0)
            .style(theme::btn(Btn::Row))
            .on_press(Message::SetPane(Pane::Now)),
        tip_box("Back to the cover"),
        tooltip::Position::Bottom,
    )
    .gap(4);
    let width = LYRICS_W.min(ctx.size.width - 48.0);
    let words = column![
        marquee(now.title.clone()).size(17).font(theme::semibold()).width(Length::Fill).fade(p.bg),
        artist_line(now, 13.0, (width - 140.0).max(120.0), p.bg),
    ]
    .spacing(3)
    .width(Length::Fill);
    // a click on the title or the space around it goes back to the cover too (the artist link still opens the artist)
    let words = mouse_area(words).on_press(Message::SetPane(Pane::Now)).interaction(iced::mouse::Interaction::Pointer);
    let play = button(
        container(icon(if now.playing { Icon::Pause } else { Icon::Play }, 18.0, p.on_accent)).center(40.0),
    )
    .padding(0)
    .style(theme::btn(Btn::Play))
    .on_press(Message::Toggle);
    let compact = row![thumb, words, play].spacing(14).align_y(Alignment::Center);
    let head = column![compact, thin_seek(state, now)].spacing(8).width(Length::Fill);
    // room above the first line and below the last, so both can sit in the middle
    let pad = (ctx.size.height * 0.32).max(60.0);
    let lyrics = state.lyrics.view(now.id, pad, Message::SeekTo);
    let mut back = row![widgets::btn("Back to the cover", Btn::Default, Some(Message::SetPane(Pane::Now)))].spacing(10);
    // wrong words or timing: Report a problem with Wrong lyrics ticked (a song of the library, not a Surround one,
    // whose lyrics are its stereo song's – ids from 1 000 000 000, server surround::BASE_ID)
    if state.lyrics.shown(now.id) && now.id.is_some_and(|id| id < 1_000_000_000) {
        back = back.push(widgets::btn("Report lyrics", Btn::Ghost, Some(Message::ReportLyrics)));
    }
    let body = column![
        container(head).max_width(width),
        container(lyrics).max_width(width).height(Length::Fill),
        back,
    ]
    .spacing(14)
    .align_x(Alignment::Center)
    .width(Length::Fill)
    .height(Length::Fill);
    container(body).width(Length::Fill).height(Length::Fill).center_x(Length::Fill).into()
}

/// A seek line without the times (lyrics mode's compact bar); in a group listen only the
/// position for a member who may not control it.
fn thin_seek<'a>(state: &'a State, now: &Now) -> El<'a, Message> {
    let dur = now.duration.max(0.001);
    let pos = state.seek_drag.unwrap_or(now.position).clamp(0.0, dur);
    let denied = state.group.denied();
    let s = slider(0.0..=dur, pos, move |v| if denied { Message::Nothing } else { Message::SeekDrag(v) })
        .step(0.25)
        .height(12.0)
        .style(theme::slider_style);
    if denied {
        return tooltip(s, tip_box(&state.group.deny), tooltip::Position::Bottom).gap(4).into();
    }
    s.on_release(Message::SeekDone).into()
}

/// The whole view.
pub fn view<'a>(state: &'a State, ctx: &Ctx<'a>) -> El<'a, Message> {
    let p = ctx.look;
    let pane = state.full.unwrap_or_default();
    let now = state.now();
    let linked = state.devices.linked.as_ref();

    let content: El<'a, Message> = match now {
        None => {
            let name = linked.map(|l| l.name.clone()).unwrap_or_else(|| "this device".into());
            column![
                row![icon_btn(Icon::X, "Close", Some(Message::CloseFull)), space().width(Length::Fill)],
                container(
                    column![
                        icon(Icon::Devices, 40.0, p.faint),
                        widgets::h2(format!("Nothing playing on {name}")),
                        widgets::muted("Play anything – it starts there."),
                    ]
                    .spacing(10)
                    .align_x(Alignment::Center)
                )
                .width(Length::Fill)
                .height(Length::Fill)
                .center(Length::Fill),
            ]
            .into()
        }
        Some(now) => {
            let tabs = widgets::tabs(
                &[("now", "Now playing"), ("lyrics", "Lyrics"), ("queue", "Up next")],
                pane.id(),
                |id| Message::SetPane(Pane::by_id(&id)),
            );
            let mut top = row![icon_btn(Icon::X, "Close", Some(Message::CloseFull)), space().width(Length::Fill), tabs, space().width(Length::Fill)]
                .spacing(12)
                .align_y(Alignment::Center);
            if let Some(pill) = super::group::view::pill(state) {
                top = top.push(pill);
            }
            if ctx.has_feature("remote") {
                let label = match linked {
                    Some(l) => format!("Playing on {}", l.name),
                    None => "This device".to_string(),
                };
                top = top.push(iced::widget::tooltip(widgets::btn_icon(Icon::Devices, label, Btn::Ghost, Some(Message::Devices)), crate::widgets::tip_box("Choose where the music plays"), iced::widget::tooltip::Position::Bottom).gap(4));
            } else {
                top = top.push(space().width(40));
            }
            let wide = ctx.wide(900.0) && pane != Pane::Now;
            let base: f32 = if pane == Pane::Now { 340.0 } else { 260.0 };
            let art_size = base.min(ctx.size.height * 0.38);
            // a right-click on the song opens ⋯ at the pointer (the web's #f-more)
            let main = || iced::widget::mouse_area(main_column(state, ctx, &now, art_size)).on_right_press(Message::Menu);
            let body: El<'a, Message> = match pane {
                Pane::Now => container(main()).width(Length::Fill).height(Length::Fill).center(Length::Fill).into(),
                Pane::Lyrics => lyrics_mode(state, ctx, &now),
                Pane::Queue => {
                    let main = main();
                    let side = container(queue_pane(state, ctx)).width(Length::Fill).height(Length::Fill).padding([0, 12]);
                    if wide {
                        row![
                            container(main).width(Length::FillPortion(2)).center_y(Length::Fill),
                            container(side).width(Length::FillPortion(3))
                        ]
                        .spacing(24)
                        .height(Length::Fill)
                        .into()
                    } else {
                        scrollable(column![main, side].spacing(24))
                            .height(Length::Fill)
                            .style(theme::scroll)
                            .into()
                    }
                }
            };
            column![top, body].spacing(16).height(Length::Fill).into()
        }
    };
    opaque(
        container(content)
            .padding([18, 24])
            .width(Length::Fill)
            .height(Length::Fill)
            .style(theme::app_bg),
    )
}
