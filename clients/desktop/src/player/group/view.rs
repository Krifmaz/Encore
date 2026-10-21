//! The group listen's windows and pieces, with the words of client-shapes §5.7: the group dialog
//! (the code, Copy invite link, the people, the host's settings, Leave / End for everyone), Start
//! a group listen, Join a group listen (a code or link), "Join <host>'s group listen?", the pill
//! "Group · 3" with "Paused here · Catch up" on the player bar, Up next with "added by Sam", the
//! Your devices panel's "Group listens" and Home's "Listening together now".

use super::{model, Dialog, Msg, Summary};
use crate::fmt;
use crate::pages::{Ctx, El};
use crate::player::{Message, State};
use crate::theme::{self, Badge, Btn};
use crate::widgets::{self, badge, btn, btn_icon, btn_sm, icon, icon_btn, modal, small_muted, t, tip_box, Icon};
use iced::widget::{button, column, container, image, radio, row, scrollable, text_input, toggler, tooltip};
use iced::{Alignment, ContentFit, Length};

fn g(m: Msg) -> Message {
    Message::Group(m)
}

/// A person's round picture: the account's avatar once loaded, else the first letter.
pub fn person<'a, M: 'a>(ctx: &Ctx<'a>, name: &str, avatar: &str, size: f32) -> El<'a, M> {
    match (!avatar.is_empty()).then(|| ctx.picture(avatar)).flatten() {
        Some(h) => image(h)
            .width(size)
            .height(size)
            .content_fit(ContentFit::Cover)
            .border_radius(size / 2.0)
            .into(),
        None => widgets::avatar(name, size),
    }
}

/// The group's windows, whichever is open.
pub fn overlay<'a>(st: &'a State, ctx: &Ctx<'a>) -> Option<El<'a, Message>> {
    let d = st.group.dialog.as_ref()?;
    Some(match d {
        Dialog::Sheet => sheet(st, ctx)?,
        Dialog::Start => start(st),
        Dialog::Join => join(st),
        Dialog::Invite { code, summary } => invite(st, code, summary.as_deref()),
        Dialog::ConfirmEnd => widgets::confirm(
            "End the group listen?",
            "The music stops for everyone in it.",
            "End for everyone",
            true,
            g(Msg::End),
            g(Msg::OpenSheet),
        ),
        Dialog::ConfirmClear => widgets::confirm(CLEAR_ASK, "", "Clear", true, g(Msg::Clear), g(Msg::Close)),
    })
}

/// The question before Up next's Clear in a group.
pub const CLEAR_ASK: &str = "Clear the group's Up next?";

fn switch_row<'a>(label: &'a str, help: &'a str, on: bool, f: impl Fn(bool) -> Message + 'a) -> El<'a, Message> {
    let mut col = column![toggler(on).label(label).text_size(14).spacing(10).on_toggle(f).style(theme::switch)].spacing(2);
    if !help.is_empty() {
        col = col.push(container(small_muted(help)).padding(widgets::pad(0.0, 0.0, 0.0, 46.0)));
    }
    col.into()
}

fn who_can_join<'a>(link_only: bool, f: impl Fn(bool) -> Message + Copy + 'a) -> El<'a, Message> {
    column![
        t("Who can join").size(14).font(theme::semibold()),
        radio("Anyone on this server can see and join", false, Some(link_only), f).size(16).text_size(14).spacing(8),
        radio("Only people with the link", true, Some(link_only), f).size(16).text_size(14).spacing(8),
    ]
    .spacing(8)
    .into()
}

/// The group: the code, the invite link, the people, the host's settings, Leave / End for
/// everyone. Two columns on a wide window (the code and the people; the settings), and the
/// buttons in the body, so they stay on screen when the people make it long.
fn sheet<'a>(st: &'a State, ctx: &Ctx<'a>) -> Option<El<'a, Message>> {
    let s = st.group.state.as_ref()?;
    let intro = small_muted(if s.you_host {
        "You host this group listen. Everyone hears the same songs at the same time, each on their own device.".to_string()
    } else {
        format!(
            "{} hosts this group listen. Everyone hears the same songs at the same time, each on their own device.",
            s.host_name
        )
    })
    .size(13);
    // the code, big enough to read out
    let mut left = column![container(
        column![
            small_muted("Code"),
            t(model::spaced(&s.code)).size(30).font(theme::mono()),
            btn_icon(Icon::Copy, "Copy invite link", Btn::Default, Some(g(Msg::CopyLink))),
        ]
        .spacing(8)
        .width(Length::Fill)
        .align_x(Alignment::Center),
    )
    .padding(14)
    .width(Length::Fill)
    .style(theme::card)]
    .spacing(14)
    .width(Length::Fill);
    if st.group.local_paused {
        left = left.push(
            row![
                t("Paused here").size(14).width(Length::Fill),
                btn_sm("Catch up", Btn::Primary, Some(g(Msg::CatchUp)))
            ]
            .align_y(Alignment::Center),
        );
    }
    left = left.push(t(format!("People ({})", s.count())).size(15).font(theme::semibold()));
    let mut people = column![].spacing(10);
    for m in &s.members {
        let mut name = row![t(m.name.clone()).size(14).font(theme::semibold())].spacing(6).align_y(Alignment::Center);
        if m.host {
            name = name.push(badge("Host", Badge::Accent));
        }
        if m.guest {
            name = name.push(badge("Guest", Badge::Plain));
        }
        if m.key == s.you_member {
            name = name.push(badge("You", Badge::Plain));
        }
        let mut words = column![name].spacing(2).width(Length::Fill);
        if !m.device_name.is_empty() {
            words = words.push(small_muted(m.device_name.clone()));
        }
        let mut line = row![person(ctx, &m.name, &m.avatar, 34.0), words].spacing(12).align_y(Alignment::Center);
        // only guests can be removed (accounts leave by themselves); the host and admins may
        if m.guest && (s.you_host || ctx.is_admin()) {
            line = line.push(btn_sm(
                "Remove from the group",
                Btn::Danger,
                Some(g(Msg::Remove(m.key.clone(), m.name.clone()))),
            ));
        }
        people = people.push(line);
    }
    left = left.push(people);
    let right: El<'a, Message> = if s.you_host {
        let mut set = column![
            t("Settings").size(15).font(theme::semibold()),
            switch_row(
                "Everyone can control",
                "Anyone in the group can play, pause, skip and change the queue.",
                s.everyone_controls,
                |on| g(Msg::Patch(serde_json::json!({ "everyone_controls": on })))
            ),
            who_can_join(s.link_only(), |link| g(Msg::Patch(
                serde_json::json!({ "visibility": if link { "link" } else { "anyone" } })
            ))),
        ]
        .spacing(12);
        if s.guests_possible {
            set = set.push(switch_row(
                "Guests can join with the link",
                "People without an account join on the web with the invite link. They listen; they can't add songs or control playback.",
                s.guests_allowed,
                |on| g(Msg::Patch(serde_json::json!({ "guests_allowed": on }))),
            ));
        }
        let limit = st.group.limit.unwrap_or(s.max_members);
        let floor = (s.count() as u32).max(2);
        set = set.push(
            column![
                row![
                    t(format!("Limit to {limit} people")).size(14).width(Length::Fill),
                    btn_sm("−", Btn::Default, (limit > floor).then_some(g(Msg::LimitDown))),
                    btn_sm("+", Btn::Default, (limit < s.server_max).then_some(g(Msg::LimitUp))),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
                small_muted(format!("At most {} on this server.", s.server_max)),
            ]
            .spacing(2),
        );
        container(set).padding(14).width(Length::Fill).style(theme::card).into()
    } else {
        small_muted(if s.everyone_controls {
            "Everyone can control playback.".to_string()
        } else {
            format!("{}. You can add songs with Add to queue.", st.group.deny)
        })
        .width(Length::Fill)
        .into()
    };
    let main: El<'a, Message> = if ctx.wide(1000.0) {
        row![left, right].spacing(18).into()
    } else {
        column![left, right].spacing(14).into()
    };
    let mut actions = row![iced::widget::space().width(Length::Fill)].spacing(10);
    if s.you_host {
        actions = actions.push(btn("End for everyone", Btn::Danger, Some(g(Msg::AskEnd))));
    }
    actions = actions.push(btn("Leave", Btn::Default, Some(g(Msg::Leave))));
    let body = column![intro, main, actions].spacing(16);
    Some(modal("Group listen", body, None, true, g(Msg::Close)))
}

/// "Start a group listen".
fn start<'a>(st: &'a State) -> El<'a, Message> {
    let gr = &st.group;
    let body = column![
        small_muted(
            "Everyone who joins hears the same songs at the same time, each on their own device. What plays here now becomes the group's queue."
        )
        .size(13),
        who_can_join(gr.start_link_only, |link| g(Msg::StartLinkOnly(link))),
        switch_row("Everyone can control", "Anyone in the group can play, pause, skip and change the queue.", gr.start_everyone, |on| g(Msg::StartEveryone(on))),
    ]
    .spacing(16);
    let footer = row![
        btn("Cancel", Btn::Default, Some(g(Msg::Close))),
        btn(if gr.busy { "Starting…" } else { "Start" }, Btn::Primary, (!gr.busy).then_some(g(Msg::Start))),
    ]
    .spacing(10);
    modal("Start a group listen", body, Some(footer.into()), false, g(Msg::Close))
}

/// "Join a group listen": a code, or an invite link pasted whole.
fn join<'a>(st: &'a State) -> El<'a, Message> {
    let gr = &st.group;
    let ok = model::code_from(&gr.join_text).is_some() && !gr.busy;
    let body = column![
        small_muted("Type the code the host gave you, or paste the invite link.").size(13),
        text_input("Code", &gr.join_text)
            .on_input(|t| g(Msg::JoinText(t)))
            .on_submit_maybe(ok.then_some(g(Msg::JoinSubmit)))
            .padding([9, 12])
            .size(15)
            .style(theme::input),
    ]
    .spacing(12);
    let footer = row![
        btn("Cancel", Btn::Default, Some(g(Msg::Close))),
        btn(if gr.busy { "Joining…" } else { "Join" }, Btn::Primary, ok.then_some(g(Msg::JoinSubmit))),
    ]
    .spacing(10);
    modal("Join a group listen", body, Some(footer.into()), false, g(Msg::Close))
}

/// "Join <host>'s group listen?" for a link that was looked up.
fn invite<'a>(st: &'a State, code: &'a str, summary: Option<&'a Summary>) -> El<'a, Message> {
    let gr = &st.group;
    let title = match summary {
        Some(s) => format!("Join {}'s group listen?", s.host_name),
        None => "Join this group listen?".to_string(),
    };
    let body = small_muted(format!(
        "{}What plays here stops, and you hear the group's songs with everyone else.",
        model::playing_now(summary)
    ))
    .size(13);
    let footer = row![
        btn("Cancel", Btn::Default, Some(g(Msg::Close))),
        btn(
            if gr.busy { "Joining…" } else { "Join" },
            Btn::Primary,
            (!gr.busy).then(|| g(Msg::JoinCode(code.to_string())))
        ),
    ]
    .spacing(10);
    modal(title, body, Some(footer.into()), false, g(Msg::Close))
}

/// The pill "Group · 3": opens the group dialog. Nothing outside a group.
pub fn pill<'a>(st: &'a State) -> Option<El<'a, Message>> {
    let s = st.group.state.as_ref()?;
    let p = theme::look();
    Some(
        button(
            row![icon(Icon::Users, 14.0, p.accent), t(model::pill(s)).size(12.5).font(theme::semibold())]
                .spacing(6)
                .align_y(Alignment::Center),
        )
        .padding([3, 10])
        .style(theme::btn(Btn::Default))
        .on_press(g(Msg::OpenSheet))
        .into(),
    )
}

/// Under the song on the player bar while in a group: the pill and whose group it is (scrolling
/// when long), or "Paused here" after a pause here only.
pub fn bar_line<'a>(st: &'a State) -> Option<El<'a, Message>> {
    use iced::widget::text::Wrapping;
    let s = st.group.state.as_ref()?;
    let p = theme::look();
    let mut r = row![pill(st)?].spacing(6).align_y(Alignment::Center);
    if st.group.local_paused {
        r = r.push(t("Paused here").size(12).wrapping(Wrapping::None));
    } else {
        r = r.push(
            crate::widgets::marquee::marquee(model::whose(s))
                .size(12)
                .color(p.muted)
                .width(Length::Fill)
                .fade(p.bg2),
        );
    }
    Some(r.into())
}

/// Above the player bar after a pause here only (the group plays on): "Paused here" and
/// "Catch up", as the pick-up offer sits there.
pub fn paused_strip<'a>(st: &'a State) -> Option<El<'a, Message>> {
    let s = st.group.state.as_ref().filter(|_| st.group.local_paused)?;
    let p = theme::look();
    Some(
        container(
            row![
                t("Paused here").size(13).font(theme::semibold()),
                t(model::whose(s)).size(13).color(p.muted),
                iced::widget::space().width(Length::Fill),
                btn_sm("Catch up", Btn::Primary, Some(g(Msg::CatchUp))),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        )
        .padding([6, 16])
        .width(Length::Fill)
        .style(theme::accent_box)
        .into(),
    )
}

/// A control this device may not use in a group: off, with the reason as its tooltip.
pub fn off<'a>(i: Icon, reason: &'a str, size: f32) -> El<'a, Message> {
    let p = theme::look();
    tooltip(
        // the faint colour (an svg's tint takes no alpha)
        button(icon(i, size, p.faint)).padding(8).style(theme::btn(Btn::Icon)),
        tip_box(reason),
        tooltip::Position::Top,
    )
    .gap(4)
    .into()
}

/// Repeat in a group: off, with the reason.
pub const REPEAT_OFF: &str = "Repeat is off during a group listen";

/// Shuffle in a group: the group's Up next in a random order, once, for those who control it
/// (the server makes the order, the same for everyone); off with the reason for anyone else.
pub fn shuffle<'a>(st: &'a State, size: f32) -> El<'a, Message> {
    let Some(s) = st.group.state.as_ref() else {
        return column![].into();
    };
    if !s.you_can_control {
        return off(Icon::Shuffle, st.group.deny.as_str(), size);
    }
    let p = theme::look();
    tooltip(
        button(icon(Icon::Shuffle, size, p.text)).padding(8).style(theme::btn(Btn::Icon)).on_press(g(Msg::Shuffle)),
        tip_box("Shuffle"),
        tooltip::Position::Top,
    )
    .gap(4)
    .into()
}

/// The Your devices panel's "Group listens": the group this computer is in, the others on the
/// server, "Start a group listen" and "Join with a code". Nothing on a server without them.
pub fn devices_section<'a>(st: &'a State, ctx: &Ctx<'a>) -> Option<El<'a, Message>> {
    let gr = &st.group;
    if !gr.shown() {
        return None;
    }
    let mut col = column![widgets::h3("Group listens")].spacing(10);
    let line = |host: &str, avatar: &str, title: &str, count: usize, note: &str, action: &'static str, msg: Message| -> El<'a, Message> {
        let sub = [title, note].iter().filter(|x| !x.is_empty()).cloned().collect::<Vec<_>>().join(" · ");
        let mut words = column![t(format!("{host} · {count} listening")).size(14).font(theme::semibold())].spacing(2);
        if !sub.is_empty() {
            words = words.push(small_muted(sub));
        }
        row![
            person(ctx, host, avatar, 34.0),
            words.width(Length::Fill),
            btn_sm(action, if action == "Join" { Btn::Primary } else { Btn::Default }, Some(msg)),
        ]
        .spacing(12)
        .align_y(Alignment::Center)
        .into()
    };
    let cur = gr.state.as_ref();
    if let Some(s) = cur {
        let title = s.current().map(|e| e.title()).unwrap_or_default();
        col = col.push(line(
            &s.host_name,
            &s.host_avatar,
            &title,
            s.count(),
            if s.you_host { "You host it" } else { "You're in it" },
            "Open",
            g(Msg::OpenSheet),
        ));
    }
    for x in gr.live.iter().filter(|x| cur.is_none_or(|s| s.id != x.id)) {
        col = col.push(line(
            &x.host_name,
            &x.host_avatar,
            &x.title,
            x.members_count,
            if x.mine { "You're in it on another device" } else { "" },
            "Join",
            g(Msg::JoinId(x.id.clone())),
        ));
    }
    if cur.is_none() && gr.live.is_empty() {
        col = col.push(small_muted("No group listens on this server right now."));
    }
    let mut actions = row![].spacing(8);
    if cur.is_none() {
        actions = actions.push(btn_icon(Icon::Users, "Start a group listen", Btn::Primary, Some(g(Msg::OpenStart))));
    }
    actions = actions.push(btn_sm("Join with a code", Btn::Default, Some(g(Msg::OpenJoin))));
    col = col.push(actions.wrap());
    Some(container(col).padding(14).width(Length::Fill).style(theme::card).into())
}

/// Up next in a group: the group's queue after the song that plays, with who added each song;
/// the controls follow what this device may do.
pub fn up_next<'a>(st: &'a State, ctx: &Ctx<'a>) -> El<'a, Message> {
    let p = ctx.look;
    let Some(s) = st.group.state.as_ref() else {
        return column![].into();
    };
    let ctl = s.you_can_control;
    let index = st.group.expected().map(|(i, _)| i).unwrap_or(0);
    let mut head = row![
        column![
            widgets::h3("Up next"),
            small_muted(if ctl {
                "Everyone in the group can add songs with Add to queue.".to_string()
            } else {
                format!("{}. You can add songs with Add to queue.", st.group.deny)
            })
        ]
        .spacing(2)
        .width(Length::Fill)
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    // Shuffle (two songs or more) and Clear of the group's Up next, for those who control it
    let next = s.queue.len().saturating_sub(index + 1);
    if ctl && next >= 2 {
        head = head.push(btn_sm("Shuffle", Btn::Ghost, Some(g(Msg::Shuffle))));
    }
    if ctl && next >= 1 {
        head = head.push(btn_sm("Clear", Btn::Ghost, Some(g(Msg::AskClear))));
    }
    let rest: Vec<(usize, &model::Entry)> = s.queue.iter().enumerate().skip(index + 1).take(300).collect();
    let list: El<'a, Message> = if rest.is_empty() {
        small_muted("Nothing queued. Anyone in the group can add songs with Add to queue.").size(13).into()
    } else {
        let mut col = column![].spacing(0);
        for (k, (i, e)) in rest.into_iter().enumerate() {
            let album = e.album_id();
            let thumb: El<'a, Message> = match (!album.is_empty()).then(|| ctx.cover(&album, &e.row["cover_v"], 80)).flatten() {
                Some(h) => image(h).width(40).height(40).content_fit(ContentFit::Cover).border_radius(6.0).into(),
                None => container(icon(Icon::Music, 16.0, p.faint)).width(40).height(40).center(40).style(theme::art).into(),
            };
            let sub = [e.artist(), fmt::mmss(e.duration())]
                .into_iter()
                .filter(|x| !x.is_empty() && x != "0:00")
                .collect::<Vec<_>>()
                .join(" · ");
            let mut words = column![t(e.title()).size(14).font(theme::semibold()), small_muted(sub)].spacing(2).width(Length::Fill);
            if !e.added_by.is_empty() {
                words = words.push(widgets::faint(format!("added by {}", e.added_by)).size(12));
            }
            let mut line = row![thumb, words].spacing(12).align_y(Alignment::Center);
            if ctl {
                line = line.push(icon_btn(Icon::Chevron, "Move up", (k > 0).then_some(g(Msg::MoveUp(i)))));
                line = line.push(icon_btn(Icon::X, "Remove", Some(g(Msg::RemoveEntry(i)))));
            }
            // a member who may not control: the rows only show (the reason is in the heading)
            let body = container(line).padding([8, 10]).width(Length::Fill);
            col = col.push(if ctl {
                El::from(
                    button(body)
                        .padding(0)
                        .width(Length::Fill)
                        .style(theme::btn(Btn::Row))
                        .on_press(g(Msg::Jump(i))),
                )
            } else {
                body.into()
            });
            col = col.push(widgets::divider());
        }
        scrollable(col).height(Length::Fill).style(theme::scroll).into()
    };
    column![head, list].spacing(12).width(Length::Fill).height(Length::Fill).into()
}

/// Home's "Listening together now": a tile per live group listen this account may see (the
/// song's cover with the host's picture, "Alex · 3 listening", Join; Open for the group this
/// computer is in). Nothing when there is none.
pub fn listening_row<'a, M: Clone + 'a>(
    ctx: &Ctx<'a>,
    live: &'a [Summary],
    current: Option<&str>,
    on: impl Fn(String) -> M,
) -> Option<El<'a, M>> {
    if live.is_empty() {
        return None;
    }
    let tiles: Vec<El<'a, M>> = live
        .iter()
        .map(|x| {
            let inside = current == Some(x.id.as_str());
            let song = [x.title.as_str(), x.artist.as_str()]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" – ");
            let sub = if song.is_empty() { (if x.playing { "Playing" } else { "Paused" }).to_string() } else { song };
            let cover = (!x.album_id.is_empty()).then(|| ctx.cover(&x.album_id, &x.cover_v, 400)).flatten();
            let tile = widgets::Tile::new(model::tile_line(x), widgets::TileKind::Album, on(x.id.clone()))
                .sub(sub)
                .cover(cover)
                .corner(person(ctx, &x.host_name, &x.host_avatar, 36.0))
                .view();
            column![
                tile,
                btn_sm(if inside { "Open" } else { "Join" }, if inside { Btn::Default } else { Btn::Primary }, Some(on(x.id.clone()))),
            ]
            .spacing(8)
            .into()
        })
        .collect();
    Some(
        column![widgets::section_head("Listening together now", None), widgets::grid(tiles)]
            .spacing(14)
            .into(),
    )
}

/// The note in Your devices while in a group (Link as remote, Play here and Send my music there
/// are off).
pub fn link_note<'a>(st: &'a State) -> Option<El<'a, Message>> {
    st.group.state.as_ref()?;
    let p = theme::look();
    Some(
        row![icon(Icon::Users, 14.0, p.muted), small_muted("Leave the group listen to control another device")]
            .spacing(6)
            .align_y(Alignment::Center)
            .into(),
    )
}
