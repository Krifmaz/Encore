//! The player bar at the bottom: the song, play / pause / next / previous, the seek bar and
//! times, the buttons chosen in "Customize the bar", ⋯, the volume; the pick-up offer above it;
//! the ⋯ menu, the Customize the bar dialog and the Add to playlist dialog.

use super::{cover_handle, signal, Message, Now, Pane, State};
use crate::config::{BAR_ACTIONS, BAR_DEFAULT};
use crate::fmt;
use crate::pages::library::SURROUND_IDS;
use crate::pages::{Ctx, El};
use crate::theme::{self, Badge, Btn};
use crate::widgets::marquee::marquee;
use crate::widgets::{self, badge, icon, icon_btn, list_row, modal, pop_menu, small_muted, t, tip_box, Icon, MenuItem};
use iced::widget::{button, column, container, image, mouse_area, row, slider, space, text_input, toggler, tooltip};
use iced::{Alignment, ContentFit, Length, Point};

/// A square icon button lit in the accent colour when `on` (the shuffle and repeat buttons).
fn mode_btn<'a>(i: Icon, tip: &'static str, on: bool, sup: Option<&'static str>, msg: Message) -> El<'a, Message> {
    let p = theme::look();
    let color = if on { p.accent } else { p.text };
    let mut inner = row![icon(i, 16.0, color)].spacing(1).align_y(Alignment::Start);
    if let Some(s) = sup {
        inner = inner.push(t(s).size(9).font(theme::bold()).color(color));
    }
    let b = button(inner).padding(8).style(theme::btn(Btn::Icon)).on_press(msg);
    tooltip(b, tip_box(tip), tooltip::Position::Top).gap(4).into()
}

/// The thumb of a song (its cover or the music note).
fn thumb<'a>(ctx: &Ctx<'a>, cover: Option<&str>, size: f32, fallback: Icon) -> El<'a, Message> {
    let p = ctx.look;
    match cover.and_then(|c| cover_handle(ctx, c, if size > 60.0 { 400 } else { 80 })) {
        Some(h) => image(h)
            .width(size)
            .height(size)
            .content_fit(ContentFit::Cover)
            .border_radius(8.0)
            .into(),
        None => container(icon(fallback, size * 0.42, p.faint))
            .width(size)
            .height(size)
            .center(size)
            .style(theme::art)
            .into(),
    }
}

/// Previous / play-pause / next. In a group listen, Previous and Next are the group's: off with
/// the reason for a member who may not control it (play / pause stays: a pause here only).
pub fn transport<'a>(state: &'a State, now: Option<&Now>, large: bool) -> El<'a, Message> {
    let p = theme::look();
    let has = now.is_some();
    let denied = state.group.denied();
    let playing = now.is_some_and(|n| n.playing);
    let size = if large { 56.0 } else { 40.0 };
    let play = button(
        container(icon(
            if playing { Icon::Pause } else { Icon::Play },
            if large { 24.0 } else { 18.0 },
            p.on_accent,
        ))
        .center(size),
    )
    .padding(0)
    .style(theme::btn(Btn::Play))
    .on_press_maybe(has.then_some(Message::Toggle));
    let skip = |i: Icon, label: &'static str, msg: Message| -> El<'a, Message> {
        if denied {
            super::group::view::off(i, &state.group.deny, 16.0)
        } else {
            icon_btn(i, label, has.then_some(msg))
        }
    };
    row![skip(Icon::Prev, "Previous", Message::Prev), play, skip(Icon::Next, "Next", Message::Next)]
    .spacing(if large { 14 } else { 6 })
    .align_y(Alignment::Center)
    .into()
}

/// The seek bar with the times. In a group listen a member who may not control it can't seek:
/// the bar only shows the position, with the reason as its tooltip.
pub fn seek<'a>(state: &'a State, now: Option<&Now>) -> El<'a, Message> {
    let dur = now.map(|n| n.duration).unwrap_or(0.0).max(0.001);
    let pos = match now {
        Some(n) => state.seek_drag.unwrap_or(n.position).clamp(0.0, dur),
        None => 0.0,
    };
    let denied = state.group.denied();
    let mut s = slider(0.0..=dur, pos, move |v| if denied { Message::Nothing } else { Message::SeekDrag(v) })
        .step(0.25)
        .style(theme::slider_style);
    if now.is_some() && !denied {
        s = s.on_release(Message::SeekDone);
    }
    let s: El<'a, Message> = if denied {
        tooltip(s, tip_box(&state.group.deny), tooltip::Position::Top).gap(4).into()
    } else {
        s.into()
    };
    row![
        widgets::faint(fmt::mmss(pos)).size(11).width(36).center(),
        s,
        widgets::faint(fmt::mmss(if now.is_some() { dur } else { 0.0 }))
            .size(11)
            .width(36)
            .center(),
    ]
    .spacing(10)
    .align_y(Alignment::Center)
    .into()
}

/// The volume icon and slider.
pub fn volume<'a>(state: &State, now: Option<&Now>, width: f32) -> El<'a, Message> {
    let p = theme::look();
    let v = state
        .vol_drag
        .unwrap_or_else(|| match now {
            Some(n) if n.remote => n.volume,
            _ => crate::engine::volume_level(),
        })
        .clamp(0.0, 1.0);
    row![
        icon(Icon::Volume, 16.0, p.muted),
        slider(0.0..=1.0, v, Message::Volume)
            .on_release(Message::VolumeDone)
            .step(0.01)
            .width(width)
            .style(theme::slider_style),
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .into()
}

/// The quality badge: the engine's path label; pressing it opens the Signal path.
pub fn quality<'a>(state: &State) -> Option<El<'a, Message>> {
    let label = signal::quality_label(&state.status, state.devices.linked.as_ref());
    if label.is_empty() {
        return None;
    }
    Some(
        tooltip(
            button(badge(label, Badge::Plain))
                .padding(0)
                .style(theme::btn(Btn::Row))
                .on_press(Message::Signal),
            tip_box("Signal path: what the file is, what Encore sends and what this device does with it"),
            tooltip::Position::Top,
        )
        .gap(4)
        .into(),
    )
}

/// The extra buttons on the right of the bar: the ones chosen in Customize the bar, then ⋯.
fn right_buttons<'a>(state: &'a State, ctx: &Ctx<'a>, now: Option<&Now>) -> Vec<El<'a, Message>> {
    let chosen = ctx.config.bar_buttons();
    let has = |id: &str| chosen.iter().any(|c| c == id);
    let lib = now.and_then(|n| n.id);
    let (shuffle_on, repeat) = state.modes();
    let mut out: Vec<El<'a, Message>> = Vec::new();
    if has("quality") && now.is_some() {
        if let Some(b) = quality(state) {
            out.push(b);
        }
    }
    let grouped = state.group.state.is_some();
    if grouped && now.is_some() && (has("shuffle") || has("repeat")) {
        if has("shuffle") {
            out.push(super::group::view::shuffle(state, 16.0));
        }
        if has("repeat") {
            out.push(super::group::view::off(Icon::Refresh, super::group::view::REPEAT_OFF, 16.0));
        }
    }
    if has("shuffle") && now.is_some() && !grouped {
        out.push(mode_btn(
            Icon::Shuffle,
            if shuffle_on { "Shuffle: on" } else { "Shuffle: off" },
            shuffle_on,
            None,
            Message::SetShuffle(!shuffle_on),
        ));
    }
    if has("repeat") && now.is_some() && !grouped {
        out.push(mode_btn(
            Icon::Refresh,
            repeat.title(),
            repeat != super::Repeat::Off,
            (repeat == super::Repeat::One).then_some("1"),
            Message::SetRepeat(repeat.next()),
        ));
    }
    // Surround songs (0.35) have no likes
    if has("like") && lib.is_some_and(|id| id < SURROUND_IDS) {
        let liked = state.is_liked(lib);
        out.push(mode_btn(
            Icon::Heart,
            if liked { "Liked – tap to remove" } else { "Like" },
            liked,
            None,
            Message::Like,
        ));
    }
    if has("album") && now.is_some_and(|n| n.album_id.is_some()) {
        out.push(icon_btn(Icon::Disc, "View album", Some(Message::ViewAlbum)));
    }
    if has("playlist") {
        if let Some(id) = lib {
            out.push(icon_btn(Icon::Plus, "Add to playlist", Some(Message::AddToPlaylist(vec![id]))));
        }
    }
    out.push(icon_btn(Icon::More, "More", Some(Message::Menu)));
    if has("lyrics") {
        out.push(icon_btn(Icon::Music, "Lyrics", Some(Message::OpenFull(Pane::Lyrics))));
    }
    if has("share") && lib.is_some() {
        out.push(icon_btn(Icon::External, "Share link", Some(Message::Share)));
    }
    if has("queue") {
        out.push(icon_btn(Icon::List, "Up next", Some(Message::OpenFull(Pane::Queue))));
    }
    if has("devices") && ctx.has_feature("remote") {
        out.push(icon_btn(
            Icon::Devices,
            "Play on or control your other devices",
            Some(Message::Devices),
        ));
    }
    out
}

/// "Continue <song> from 2:31 · was playing on <device>" with Play here and ×.
fn spot_bar<'a>(state: &'a State, ctx: &Ctx<'a>) -> Option<El<'a, Message>> {
    let o = state.spot.as_ref()?;
    let s = &o.spot;
    let (title, position) = if o.remote {
        (fmt::s(s, "title"), fmt::f(s, "position"))
    } else {
        let idx = s["index"].as_u64().unwrap_or(0) as usize;
        let row = fmt::items(&s["queue"]).get(idx).cloned().unwrap_or_default();
        (fmt::s(&row, "title"), fmt::f(s, "position"))
    };
    let tail = if o.remote {
        let device = fmt::s(s, "device_name");
        format!(
            " from {} · was playing on {}",
            fmt::mmss(position),
            if device.is_empty() { "another device".to_string() } else { device }
        )
    } else {
        format!(" from {} · where you left off", fmt::mmss(position))
    };
    let p = ctx.look;
    Some(
        container(
            row![
                t("Continue ").size(13),
                t(title).size(13).font(theme::bold()),
                t(tail).size(13).color(p.muted),
                space().width(Length::Fill),
                widgets::btn_sm("Play here", Btn::Primary, Some(Message::SpotPlay)),
                icon_btn(Icon::X, "Close", Some(Message::SpotDismiss)),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
        )
        .padding([6, 16])
        .width(Length::Fill)
        .style(theme::accent_box)
        .into(),
    )
}

/// `.player`: the 84px bar.
pub fn view<'a>(state: &'a State, ctx: &Ctx<'a>) -> El<'a, Message> {
    let p = ctx.look;
    let now = state.now();
    let linked = state.devices.linked.as_ref();

    // left: the song (press to open the full view)
    let left: El<'a, Message> = match (&now, linked) {
        (Some(n), _) => {
            let sub = [n.artist.as_str(), n.album.as_str()]
                .iter()
                .filter(|x| !x.is_empty())
                .cloned()
                .collect::<Vec<_>>()
                .join(" · ");
            // a title or line too long for the bar scrolls sideways (widgets::marquee)
            let mut info = column![marquee(n.title.clone()).size(14).font(theme::semibold()).width(Length::Fill).fade(p.bg2)]
                .spacing(2)
                .width(Length::Fill);
            if !sub.is_empty() {
                info = info.push(marquee(sub).size(12.5).color(p.muted).width(Length::Fill).fade(p.bg2));
            }
            if let Some(gl) = super::group::view::bar_line(state) {
                info = info.push(gl);
            }
            if let Some(l) = linked {
                info = info.push(
                    row![
                        icon(Icon::Link, 12.0, p.accent),
                        t(format!(
                            "{}Playing on {}",
                            if l.online { "" } else { "Offline · " },
                            l.name
                        ))
                        .size(12)
                        .color(p.accent)
                    ]
                    .spacing(4)
                    .align_y(Alignment::Center),
                );
            }
            let content = row![thumb(ctx, n.cover.as_deref(), 48.0, Icon::Music), info]
                .spacing(12)
                .width(Length::Fill)
                .align_y(Alignment::Center);
            tooltip(
                button(content)
                    .padding(0)
                    .width(Length::Fill)
                    .style(theme::btn(Btn::Row))
                    .on_press(Message::OpenFull(Pane::Now)),
                tip_box("Open player"),
                tooltip::Position::Top,
            )
            .gap(4)
            .into()
        }
        (None, Some(l)) => row![
            thumb(ctx, None, 48.0, Icon::Devices),
            column![
                t("Nothing playing").size(14).font(theme::semibold()).color(p.faint),
                small_muted("Play anything – it starts there"),
                row![
                    icon(Icon::Link, 12.0, p.accent),
                    t(format!("{}Playing on {}", if l.online { "" } else { "Offline · " }, l.name))
                        .size(12)
                        .color(p.accent)
                ]
                .spacing(4)
                .align_y(Alignment::Center),
            ]
            .spacing(2)
        ]
        .spacing(12)
        .align_y(Alignment::Center)
        .into(),
        (None, None) => row![
            thumb(ctx, None, 48.0, Icon::Music),
            column![
                t("Nothing playing").size(14).font(theme::semibold()).color(p.faint),
                row![
                    small_muted("Open an album in"),
                    button(t("Library").size(12).color(p.accent))
                        .padding(0)
                        .style(theme::btn(Btn::Row))
                        .on_press(Message::OpenLibrary),
                    small_muted("and press ▶ Play"),
                ]
                .spacing(4)
                .align_y(Alignment::Center),
            ]
            .spacing(2)
        ]
        .spacing(12)
        .align_y(Alignment::Center)
        .into(),
    };
    // a right-click on the song or the transport opens ⋯ at the pointer (the web's #p-more)
    let left = mouse_area(container(left).width(Length::FillPortion(3)).align_y(Alignment::Center))
        .on_right_press(Message::Menu);

    // centre: transport and seek
    let center = mouse_area(
        column![transport(state, now.as_ref(), false), seek(state, now.as_ref())]
            .spacing(2)
            .align_x(Alignment::Center)
            .width(Length::FillPortion(4)),
    )
    .on_right_press(Message::Menu);

    // right: the chosen buttons, ⋯, the volume, close
    let mut right = row![].spacing(4).align_y(Alignment::Center);
    if linked.is_some() {
        right = right.push(
            tooltip(
                widgets::btn_icon(Icon::Link, "Unlink", Btn::Default, Some(Message::Unlink)),
                tip_box("Stop being a remote and play on this device again"),
                tooltip::Position::Top,
            )
            .gap(4),
        );
    }
    if let Some(e) = &state.status.error {
        right = right.push(
            tooltip(badge("Engine error", Badge::Err), tip_box(e.as_str()), tooltip::Position::Top).gap(4),
        );
    }
    if now.is_some() {
        for b in right_buttons(state, ctx, now.as_ref()) {
            right = right.push(b);
        }
    } else if linked.is_none() && ctx.has_feature("remote") {
        right = right.push(icon_btn(
            Icon::Devices,
            "Play on or control your other devices",
            Some(Message::Devices),
        ));
    }
    right = right.push(space().width(6));
    right = right.push(volume(state, now.as_ref(), 100.0));
    // in a group listen the group plays on: Leave is in the group's dialog
    if now.as_ref().is_some_and(|n| !n.remote) && state.group.state.is_none() {
        right = right.push(icon_btn(Icon::X, "Close player", Some(Message::ClosePlayer)));
    }
    // the right side takes what its buttons need (the chosen ones, the badge and the volume
    // never get cut off); the song and the transport share the rest
    let right = container(right).align_x(Alignment::End);

    let mut col = column![widgets::divider()];
    if let Some(s) = spot_bar(state, ctx) {
        col = col.push(s);
        col = col.push(widgets::divider());
    }
    // a group listen paused here only: "Paused here" and "Catch up" (p6 item 8)
    if let Some(s) = super::group::view::paused_strip(state) {
        col = col.push(s);
        col = col.push(widgets::divider());
    }
    col.push(
        container(
            row![left, center, right]
                .spacing(16)
                .align_y(Alignment::Center)
                .height(Length::Fill),
        )
        .padding([0, 16])
        .height(theme::PLAYER_H - 1.0)
        .width(Length::Fill)
        .style(theme::player_bar),
    )
    .into()
}

/// "⋯" for the song in front of you.
pub fn more_menu<'a>(state: &'a State, ctx: &Ctx<'a>, at: Point) -> El<'a, Message> {
    let now = state.now();
    let lib = now.as_ref().and_then(|n| n.id);
    let linked = state.devices.linked.is_some();
    let mut items: Vec<MenuItem<Message>> = Vec::new();
    if let Some(n) = now.as_ref() {
        if !n.artist.is_empty() {
            items.push(MenuItem::new("Go to artist", Message::GoToArtist).icon(Icon::User));
        }
        if n.album_id.is_some() {
            items.push(MenuItem::new("View album", Message::ViewAlbum).icon(Icon::Disc));
        }
    }
    if let Some(id) = lib {
        // Surround songs (0.35) have no likes and are in no playlist
        if id < SURROUND_IDS {
            items.push(
                MenuItem::new(
                    if state.is_liked(Some(id)) { "Remove from Liked Songs" } else { "Like" },
                    Message::LikeTrack(id),
                )
                .icon(Icon::Heart),
            );
            items.push(MenuItem::new("Add to playlist", Message::AddToPlaylist(vec![id])).icon(Icon::Plus));
        }
        items.push(MenuItem::new("Lyrics", Message::OpenFull(Pane::Lyrics)).icon(Icon::Music));
        items.push(MenuItem::new("Share link", Message::Share).icon(Icon::External));
        items.push(MenuItem::new("Download this song", Message::DownloadSong).icon(Icon::Download));
        if !linked {
            items.push(MenuItem::new("Save for offline", Message::SaveOffline).icon(Icon::Download));
        }
    }
    items.push(MenuItem::new("Signal path", Message::Signal).icon(Icon::Activity));
    if !linked {
        items.push(MenuItem::new("Mini player", Message::OpenMini).icon(Icon::Music));
    }
    if lib.is_some() {
        items.push(MenuItem::new("Report a problem", Message::ReportSong).icon(Icon::Shield));
    }
    if ctx.has_feature("remote") {
        items.push(
            MenuItem::new(
                if linked { "Your devices…" } else { "Play on another device…" },
                Message::Devices,
            )
            .icon(Icon::Devices),
        );
    }
    // the group listen (p6 item 8): its dialog while in one, else start or join one
    if let Some(s) = state.group.state.as_ref() {
        items.push(MenuItem::new("Group listen", Message::Group(super::group::Msg::OpenSheet)).icon(Icon::Users));
        // the group's own pause (Pause pauses only here), for those who control it while it plays
        if super::group::model::offers_pause_for_everyone(s) {
            items.push(
                MenuItem::new(super::group::model::PAUSE_FOR_EVERYONE, Message::Group(super::group::Msg::PauseForEveryone))
                    .icon(Icon::Pause),
            );
        }
    } else if state.group.shown() {
        items.push(MenuItem::new("Start a group listen", Message::Group(super::group::Msg::OpenStart)).icon(Icon::Users));
        items.push(MenuItem::new("Join a group listen", Message::Group(super::group::Msg::OpenJoin)).icon(Icon::Users));
    }
    items.push(MenuItem::new("Customize the bar…", Message::Customize(true)).icon(Icon::Settings));
    // who added the song, once the server said (0.36)
    if let Some((_, text)) = state.menu_added.as_ref().filter(|(id, _)| lib == Some(*id)) {
        items.insert(0, MenuItem::info(text.clone()));
    }
    pop_menu(at, items, Message::CloseMenu, ctx.size)
}

/// "Customize the bar": tick the buttons the bar shows.
pub fn customize<'a>(_state: &'a State, ctx: &Ctx<'a>) -> El<'a, Message> {
    let p = ctx.look;
    let chosen = ctx.config.bar_buttons();
    let mut list = column![widgets::muted(
        "Buttons on the player bar. ⋯ and the volume always stay; everything is still in the ⋯ menu."
    )
    .size(13.5)]
    .spacing(10);
    for (id, label, ic) in BAR_ACTIONS {
        let on = chosen.iter().any(|c| c == id);
        let ic_el: El<'a, Message> = match Icon::by_name(ic) {
            Some(i) => icon(i, 18.0, p.faint),
            None => space().width(18).into(),
        };
        list = list.push(
            row![
                toggler(on)
                    .label(*label)
                    .text_size(14)
                    .spacing(10)
                    .on_toggle(move |_| Message::ToggleBarButton(id.to_string()))
                    .style(theme::switch),
                space().width(Length::Fill),
                ic_el,
            ]
            .spacing(10)
            .align_y(Alignment::Center),
        );
    }
    let same_as_default = chosen.len() == BAR_DEFAULT.len() && BAR_DEFAULT.iter().all(|d| chosen.iter().any(|x| x == d));
    let footer = row![
        widgets::btn("Reset", Btn::Default, (!same_as_default).then_some(Message::ResetBar)),
        widgets::btn("Done", Btn::Primary, Some(Message::Customize(false))),
    ]
    .spacing(10);
    modal("Customize the bar", list, Some(footer.into()), false, Message::Customize(false))
}

/// "Add to playlist": the playlists, or a new one.
pub fn add_to_playlist<'a>(state: &'a State, ctx: &Ctx<'a>) -> El<'a, Message> {
    let p = ctx.look;
    let body: El<'a, Message> = match state.playlists.as_ref() {
        None => widgets::skeleton_rows(3),
        Some(list) => {
            let mut col = column![].spacing(0);
            // not someone else's playlist added from a link: only its owner adds songs to it
            for pl in list.iter().filter(|pl| crate::pages::playlists::can_add_to(pl)) {
                let id = fmt::i(pl, "id");
                let left: El<'a, Message> = if fmt::s(pl, "kind") == "liked" {
                    icon(Icon::Heart, 16.0, p.accent)
                } else {
                    icon(Icon::List, 16.0, p.muted)
                };
                col = col.push(list_row(
                    Some(left),
                    fmt::s(pl, "name"),
                    String::new(),
                    vec![widgets::faint(fmt::s(pl, "total")).size(12).into()],
                    Some(Message::AddTo(id)),
                ));
            }
            let new_row = row![
                text_input("…or a new playlist name", &state.new_playlist)
                    .on_input(Message::NewPlaylistName)
                    .on_submit(Message::CreatePlaylist)
                    .padding([8, 10])
                    .size(14)
                    .style(theme::input),
                widgets::btn_icon(Icon::Plus, "Create", Btn::Default, Some(Message::CreatePlaylist)),
            ]
            .spacing(10)
            .align_y(Alignment::Center);
            column![widgets::card_flat(col), new_row].spacing(14).into()
        }
    };
    let body = mouse_area(body);
    modal("Add to playlist", body, None, false, Message::CloseDialog)
}
