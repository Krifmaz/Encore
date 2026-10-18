//! The full view: Now playing, Lyrics and Up next, laid over the whole window.

use super::{bar, cover_handle, Message, Now, Pane, State};
use crate::fmt;
use crate::pages::{Ctx, El};
use crate::theme::{self, Btn};
use crate::widgets::{self, icon, icon_btn, t, Icon};
use iced::widget::{button, column, container, image, opaque, row, scrollable, space, toggler};
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
                artist: fmt::s(&x, "artist"),
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

/// The left column: art, words, seek, controls, volume, actions.
fn main_column<'a>(state: &'a State, ctx: &Ctx<'a>, now: &Now, art_size: f32) -> El<'a, Message> {
    let p = ctx.look;
    let (shuffle_on, repeat) = state.modes();
    let lib = now.id;
    let mut meta = column![t(now.title.clone()).size(22).font(theme::bold()).center()]
        .spacing(4)
        .align_x(Alignment::Center);
    let mut line = row![].spacing(4).align_y(Alignment::Center);
    if !now.artist.is_empty() {
        line = line.push(
            button(t(now.artist.clone()).size(14).color(p.muted))
                .padding(0)
                .style(theme::btn(Btn::Row))
                .on_press(Message::GoToArtist),
        );
    }
    if !now.album.is_empty() {
        if !now.artist.is_empty() {
            line = line.push(widgets::muted("·").size(14));
        }
        let album = t(now.album.clone()).size(14).color(p.muted);
        line = line.push(match now.album_id {
            Some(_) => button(album)
                .padding(0)
                .style(theme::btn(Btn::Row))
                .on_press(Message::ViewAlbum)
                .into(),
            None => El::from(album),
        });
    }
    meta = meta.push(line);
    if let Some(q) = bar::quality(state) {
        meta = meta.push(q);
    }

    let controls = row![
        bar_mode(Icon::Shuffle, shuffle_on, None, Message::SetShuffle(!shuffle_on)),
        bar::transport(Some(now), true),
        bar_mode(
            Icon::Refresh,
            repeat != super::Repeat::Off,
            (repeat == super::Repeat::One).then_some("1"),
            Message::SetRepeat(repeat.next())
        ),
    ]
    .spacing(18)
    .align_y(Alignment::Center);

    let mut actions = row![].spacing(6).align_y(Alignment::Center);
    if let Some(id) = lib {
        let liked = state.is_liked(Some(id));
        actions = actions.push(bar_mode(Icon::Heart, liked, None, Message::LikeTrack(id)));
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

/// Up next: the songs after the current one, with move up and remove.
fn queue_pane<'a>(state: &'a State, ctx: &Ctx<'a>) -> El<'a, Message> {
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
            let main = iced::widget::mouse_area(main_column(state, ctx, &now, art_size)).on_right_press(Message::Menu);
            let body: El<'a, Message> = match pane {
                Pane::Now => container(main).width(Length::Fill).height(Length::Fill).center(Length::Fill).into(),
                Pane::Lyrics | Pane::Queue => {
                    let side: El<'a, Message> = match pane {
                        Pane::Lyrics => state.lyrics.view(now.id, Message::SeekTo),
                        _ => queue_pane(state, ctx),
                    };
                    let side = container(side).width(Length::Fill).height(Length::Fill).padding([0, 12]);
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
