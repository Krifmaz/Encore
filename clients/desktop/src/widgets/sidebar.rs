//! `.sidebar`: the logo, the nav (the same entries and order as web-rs/src/app.rs), the storage
//! box, Bug reports under it, and the user foot. App-level: it sends `app::Message`.

use super::{avatar, badge, icon, icon_btn, logo, small, small_muted, t, El, Icon};
use crate::app::{App, Message};
use crate::pages::{Page, Section};
use crate::theme::{self, Badge, Btn};
use iced::widget::{button, column, container, progress_bar, row, scrollable, space};
use iced::{Alignment, Background, Border, Color, ContentFit, Length};
use serde_json::Value;

/// `/api/users/{id}/avatar?v=<avatar_at>` when the user uploaded a picture.
pub fn avatar_path(u: &Value) -> Option<String> {
    crate::fmt::truthy(u, "avatar_at").then(|| {
        format!(
            "/api/users/{}/avatar?v={}",
            crate::fmt::s(u, "id"),
            crate::fmt::f(u, "avatar_at").round() as i64
        )
    })
}

/// The web's `avatar_html`: the uploaded picture once it is loaded (`picture` looks a server path
/// up in the covers cache, which fetches it), else the initial in the accent circle.
pub fn user_avatar<'a, M: 'a>(
    u: &Value,
    picture: impl Fn(&str) -> Option<iced::widget::image::Handle>,
    size: f32,
) -> El<'a, M> {
    match avatar_path(u).and_then(|p| picture(&p)) {
        Some(h) => iced::widget::image(h)
            .width(size)
            .height(size)
            .content_fit(ContentFit::Cover)
            .border_radius(size / 2.0)
            .into(),
        None => avatar(crate::fmt::s(u, "username").as_str(), size),
    }
}

fn nav_button<'a>(app: &App, page: Page, i: Icon, label: &'a str, count: Option<i64>) -> El<'a, Message> {
    let p = theme::look();
    let on = app.page.nav_key() == page.nav_key();
    let mark = container(space())
        .width(3)
        .height(18)
        .style(move |_| container::Style {
            background: Some(Background::Color(if on { p.accent } else { Color::TRANSPARENT })),
            border: Border {
                radius: 2.0.into(),
                ..Border::default()
            },
            ..container::Style::default()
        });
    let mut r = row![
        mark,
        icon(i, 18.0, if on { p.text } else { p.muted }),
        t(label).size(14).font(theme::semibold()),
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    if let Some(n) = count.filter(|n| *n > 0) {
        r = r.push(space().width(Length::Fill));
        r = r.push(badge(n.to_string(), Badge::Warn));
    }
    button(r)
        .padding([8, 9])
        .width(Length::Fill)
        .style(theme::btn(if on { Btn::NavOn } else { Btn::Nav }))
        .on_press(Message::Nav(page))
        .into()
}

/// The storage box under the nav (`#storage-mini`).
fn storage_box(app: &App) -> Option<El<'_, Message>> {
    let s = &app.shell.storage;
    if !s.is_object() {
        return None;
    }
    let lib = crate::fmt::f(&s["library"], "size");
    let body: El<'_, Message> = if s["disk"].is_object() {
        let disk = &s["disk"];
        let (used, total, free) = (
            crate::fmt::f(disk, "used"),
            crate::fmt::f(disk, "total"),
            crate::fmt::f(disk, "free"),
        );
        let pct = if total > 0.0 { (used / total) as f32 } else { 0.0 };
        let bar = progress_bar(0.0..=1.0, pct)
            .girth(6)
            .style(if pct > 0.9 { theme::progress_err } else { theme::progress });
        column![
            row![
                small("Storage"),
                space().width(Length::Fill),
                small_muted(format!("{} free", crate::fmt::size(free)))
            ]
            .align_y(Alignment::Center),
            bar,
            small_muted(format!("Music {} · {} disk", crate::fmt::size(lib), crate::fmt::size(total))).size(11),
        ]
        .spacing(5)
        .into()
    } else {
        small(format!("Library {}", crate::fmt::size(lib))).into()
    };
    Some(
        button(container(body).padding([8, 10]))
            .padding(0)
            .width(Length::Fill)
            .style(theme::btn(Btn::Nav))
            .on_press(Message::Nav(Page::Storage))
            .into(),
    )
}

pub fn view(app: &App) -> El<'_, Message> {
    let p = theme::look();
    let admin = app.session.is_admin();
    let brand = button(
        row![logo(30.0), t(app.session.app_name()).size(18).font(theme::bold())]
            .spacing(10)
            .align_y(Alignment::Center),
    )
    .padding([4, 10])
    .style(theme::btn(Btn::Row))
    .on_press(Message::Nav(Page::Home));

    let mut nav = column![].spacing(2);
    nav = nav.push(nav_button(app, Page::Home, Icon::Home, "Home", None));
    nav = nav.push(nav_button(app, Page::Search { q: String::new() }, Icon::Search, "Discover", None));
    nav = nav.push(nav_button(app, Page::ForYou, Icon::Spark, "For you", None));
    nav = nav.push(nav_button(app, Page::library(""), Icon::Library, "Library", None));
    nav = nav.push(nav_button(app, Page::Requests, Icon::Inbox, "Requests", admin.then_some(app.shell.pending)));
    nav = nav.push(nav_button(app, Page::Playlists, Icon::List, "Playlists", None));
    nav = nav.push(nav_button(app, Page::Download, Icon::Download, "Downloader", None));
    if app.session.has_feature("soulseek") {
        nav = nav.push(nav_button(app, Page::Soulseek, Icon::Users, "Soulseek", None));
    }
    nav = nav.push(nav_button(app, Page::Leaderboard, Icon::Activity, "Leaderboard", None));

    let mut under = column![].spacing(2);
    if let Some(s) = storage_box(app) {
        under = under.push(s);
    }
    under = under.push(nav_button(app, Page::BugReports, Icon::Bug, "Bug reports", Some(app.shell.bugs)));

    let name = app.session.username();
    let base = app.api.base();
    let me = user_avatar(&app.session.user, |path| app.covers.get(&super::cover::absolute(&base, path)), 32.0);
    let account = button(
        row![
            me,
            column![
                t(name).size(14).font(theme::semibold()),
                small_muted(app.session.role_label()).size(12)
            ]
            .spacing(1)
            .width(Length::Fill),
        ]
        .spacing(10)
        .align_y(Alignment::Center),
    )
    .padding([4, 6])
    .width(Length::Fill)
    .style(theme::btn(Btn::Row))
    .on_press(Message::Nav(Page::Settings(Section::Account)));
    let foot = column![
        super::divider(),
        row![
            account,
            icon_btn(Icon::Settings, "Settings", Some(Message::Nav(Page::Settings(Section::Account)))),
            icon_btn(Icon::Logout, "Sign out", Some(Message::SignOut)),
        ]
        .spacing(4)
        .align_y(Alignment::Center)
        .padding(super::pad(8.0, 4.0, 0.0, 4.0)),
    ]
    .spacing(0);

    let body = column![
        container(brand).padding(super::pad(0.0, 0.0, 14.0, 0.0)),
        scrollable(nav).height(Length::Fill).style(theme::scroll),
        under,
        foot,
    ]
    .spacing(10);
    let _ = p;
    row![
        container(body)
            .padding([18, 12])
            .width(theme::SIDEBAR - 1.0)
            .height(Length::Fill)
            .style(theme::sidebar),
        iced::widget::rule::vertical(1).style(theme::rule_style),
    ]
    .into()
}
