//! The web app's icon set (web-rs/src/ui.rs `icon()`), the same names and paths, as SVG handles
//! tinted to the theme. Generated from the web table; keep the names in sync.

use super::El;
use crate::theme;
use iced::widget::svg::{self, Handle};
use iced::Color;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Icon {
    Home,
    Search,
    Library,
    Inbox,
    Download,
    Activity,
    Users,
    Ticket,
    Settings,
    Play,
    Pause,
    Next,
    Prev,
    Plus,
    Check,
    X,
    Menu,
    Logout,
    Tag,
    Edit,
    Trash,
    Refresh,
    Link,
    Music,
    Disc,
    User,
    Sun,
    Moon,
    Copy,
    Volume,
    External,
    Shield,
    Heart,
    Spark,
    Chevron,
    More,
    Shuffle,
    List,
    Devices,
    Bug,
}

impl Icon {
    pub const ALL: [Icon; 40] = [
        Icon::Home,
        Icon::Search,
        Icon::Library,
        Icon::Inbox,
        Icon::Download,
        Icon::Activity,
        Icon::Users,
        Icon::Ticket,
        Icon::Settings,
        Icon::Play,
        Icon::Pause,
        Icon::Next,
        Icon::Prev,
        Icon::Plus,
        Icon::Check,
        Icon::X,
        Icon::Menu,
        Icon::Logout,
        Icon::Tag,
        Icon::Edit,
        Icon::Trash,
        Icon::Refresh,
        Icon::Link,
        Icon::Music,
        Icon::Disc,
        Icon::User,
        Icon::Sun,
        Icon::Moon,
        Icon::Copy,
        Icon::Volume,
        Icon::External,
        Icon::Shield,
        Icon::Heart,
        Icon::Spark,
        Icon::Chevron,
        Icon::More,
        Icon::Shuffle,
        Icon::List,
        Icon::Devices,
        Icon::Bug,
    ];

    /// The web's name (`icon("home")`).
    pub fn name(self) -> &'static str {
        match self {
            Icon::Home => "home",
            Icon::Search => "search",
            Icon::Library => "library",
            Icon::Inbox => "inbox",
            Icon::Download => "download",
            Icon::Activity => "activity",
            Icon::Users => "users",
            Icon::Ticket => "ticket",
            Icon::Settings => "settings",
            Icon::Play => "play",
            Icon::Pause => "pause",
            Icon::Next => "next",
            Icon::Prev => "prev",
            Icon::Plus => "plus",
            Icon::Check => "check",
            Icon::X => "x",
            Icon::Menu => "menu",
            Icon::Logout => "logout",
            Icon::Tag => "tag",
            Icon::Edit => "edit",
            Icon::Trash => "trash",
            Icon::Refresh => "refresh",
            Icon::Link => "link",
            Icon::Music => "music",
            Icon::Disc => "disc",
            Icon::User => "user",
            Icon::Sun => "sun",
            Icon::Moon => "moon",
            Icon::Copy => "copy",
            Icon::Volume => "volume",
            Icon::External => "external",
            Icon::Shield => "shield",
            Icon::Heart => "heart",
            Icon::Spark => "spark",
            Icon::Chevron => "chevron",
            Icon::More => "more",
            Icon::Shuffle => "shuffle",
            Icon::List => "list",
            Icon::Devices => "devices",
            Icon::Bug => "bug",
        }
    }

    /// The icon of a web name, for tables that carry icon names (the bar actions).
    pub fn by_name(name: &str) -> Option<Icon> {
        Icon::ALL.into_iter().find(|i| i.name() == name)
    }

    /// Filled shapes (the transport) are drawn with `fill`, the rest with a 2px stroke.
    fn filled(self) -> bool {
        matches!(self, Icon::Play | Icon::Pause | Icon::Next | Icon::Prev)
    }

    fn body(self) -> &'static str {
        match self {
            Icon::Home => r##"<path d="M3 10.5 12 3l9 7.5V20a1 1 0 0 1-1 1h-5v-6H9v6H4a1 1 0 0 1-1-1z"/>"##,
            Icon::Search => r##"<circle cx="11" cy="11" r="7"/><path d="m21 21-4.3-4.3"/>"##,
            Icon::Library => r##"<path d="M4 4v16M9 4v16M14 4l6 16"/>"##,
            Icon::Inbox => r##"<path d="M22 12h-6l-2 3h-4l-2-3H2"/><path d="M5.45 5.11 2 12v6a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-6l-3.45-6.89A2 2 0 0 0 16.76 4H7.24a2 2 0 0 0-1.79 1.11z"/>"##,
            Icon::Download => r##"<path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><path d="m7 10 5 5 5-5M12 15V3"/>"##,
            Icon::Activity => r##"<path d="M22 12h-4l-3 9L9 3l-3 9H2"/>"##,
            Icon::Users => r##"<path d="M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2"/><circle cx="9" cy="7" r="4"/><path d="M22 21v-2a4 4 0 0 0-3-3.87M16 3.13a4 4 0 0 1 0 7.75"/>"##,
            Icon::Ticket => r##"<path d="M2 9a3 3 0 0 0 0 6v2a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-2a3 3 0 0 0 0-6V7a2 2 0 0 0-2-2H4a2 2 0 0 0-2 2z"/><path d="M13 5v2M13 17v2M13 11v2"/>"##,
            Icon::Settings => r##"<circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"/>"##,
            Icon::Play => r##"<path d="M7 4.5v15a1 1 0 0 0 1.5.86l12-7.5a1 1 0 0 0 0-1.72l-12-7.5A1 1 0 0 0 7 4.5z"/>"##,
            Icon::Pause => r##"<rect x="6" y="4" width="4" height="16" rx="1"/><rect x="14" y="4" width="4" height="16" rx="1"/>"##,
            Icon::Next => r##"<path d="M5 5.5v13a1 1 0 0 0 1.5.86L16 13.5V18a1 1 0 0 0 2 0V6a1 1 0 0 0-2 0v4.5L6.5 4.64A1 1 0 0 0 5 5.5z"/>"##,
            Icon::Prev => r##"<path d="M19 5.5v13a1 1 0 0 1-1.5.86L8 13.5V18a1 1 0 0 1-2 0V6a1 1 0 0 1 2 0v4.5l9.5-5.86A1 1 0 0 1 19 5.5z"/>"##,
            Icon::Plus => r##"<path d="M12 5v14M5 12h14"/>"##,
            Icon::Check => r##"<path d="M20 6 9 17l-5-5"/>"##,
            Icon::X => r##"<path d="M18 6 6 18M6 6l12 12"/>"##,
            Icon::Menu => r##"<path d="M3 6h18M3 12h18M3 18h18"/>"##,
            Icon::Logout => r##"<path d="M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4M16 17l5-5-5-5M21 12H9"/>"##,
            Icon::Tag => r##"<path d="M20.59 13.41 13.42 20.58a2 2 0 0 1-2.83 0L2 12V2h10l8.59 8.59a2 2 0 0 1 0 2.82z"/><circle cx="7" cy="7" r="1.5"/>"##,
            Icon::Edit => r##"<path d="M12 20h9M16.5 3.5a2.12 2.12 0 1 1 3 3L7 19l-4 1 1-4z"/>"##,
            Icon::Trash => r##"<path d="M3 6h18M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6"/>"##,
            Icon::Refresh => r##"<path d="M21 12a9 9 0 1 1-2.64-6.36L21 8M21 3v5h-5"/>"##,
            Icon::Link => r##"<path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71"/><path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71"/>"##,
            Icon::Music => r##"<path d="M9 18V5l12-2v13"/><circle cx="6" cy="18" r="3"/><circle cx="18" cy="16" r="3"/>"##,
            Icon::Disc => r##"<circle cx="12" cy="12" r="10"/><circle cx="12" cy="12" r="3"/>"##,
            Icon::User => r##"<path d="M20 21v-2a4 4 0 0 0-4-4H8a4 4 0 0 0-4 4v2"/><circle cx="12" cy="7" r="4"/>"##,
            Icon::Sun => r##"<circle cx="12" cy="12" r="4"/><path d="M12 2v2M12 20v2M4.93 4.93l1.41 1.41M17.66 17.66l1.41 1.41M2 12h2M20 12h2M6.34 17.66l-1.41 1.41M19.07 4.93l-1.41 1.41"/>"##,
            Icon::Moon => r##"<path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z"/>"##,
            Icon::Copy => r##"<rect x="9" y="9" width="13" height="13" rx="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/>"##,
            Icon::Volume => r##"<path d="M11 5 6 9H2v6h4l5 4zM15.54 8.46a5 5 0 0 1 0 7.07M19.07 4.93a10 10 0 0 1 0 14.14"/>"##,
            Icon::External => r##"<path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6M15 3h6v6M10 14 21 3"/>"##,
            Icon::Shield => r##"<path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z"/>"##,
            Icon::Heart => r##"<path d="M20.84 4.61a5.5 5.5 0 0 0-7.78 0L12 5.67l-1.06-1.06a5.5 5.5 0 0 0-7.78 7.78l1.06 1.06L12 21.23l7.78-7.78 1.06-1.06a5.5 5.5 0 0 0 0-7.78z"/>"##,
            Icon::Spark => r##"<path d="M12 3l1.9 5.6L19.5 10l-5.6 1.9L12 17.5l-1.9-5.6L4.5 10l5.6-1.4z"/><path d="M19 15l.8 2.2L22 18l-2.2.8L19 21l-.8-2.2L16 18l2.2-.8z"/>"##,
            Icon::Chevron => r##"<polyline points="6 9 12 15 18 9"/>"##,
            Icon::More => r##"<circle cx="12" cy="5" r="1.6"/><circle cx="12" cy="12" r="1.6"/><circle cx="12" cy="19" r="1.6"/>"##,
            Icon::Shuffle => r##"<path d="M16 3h5v5M4 20 21 3M21 16v5h-5M15 15l6 6M4 4l5 5"/>"##,
            Icon::List => r##"<path d="M8 6h13M8 12h13M8 18h13M3 6h.01M3 12h.01M3 18h.01"/>"##,
            Icon::Devices => r##"<rect x="2" y="4" width="13" height="10" rx="2"/><path d="M6 18h5M8.5 14v4"/><rect x="17" y="8" width="5" height="12" rx="1.5"/><path d="M19.5 17h.01"/>"##,
            Icon::Bug => r##"<path d="M9 7V6a3 3 0 0 1 6 0v1"/><path d="M12 20c-3.3 0-6-2.7-6-6v-3a4 4 0 0 1 4-4h4a4 4 0 0 1 4 4v3c0 3.3-2.7 6-6 6z"/><path d="M12 20v-9M6 13H2M22 13h-4M3 21c0-2.1 1.7-3.9 3.8-4M20.2 17c2.1.1 3.8 1.9 3.8 4M3 5c0 2.1 1.6 3.8 3.5 4M21 5c0 2.1-1.6 3.8-3.5 4M8 2l1.9 1.9M16 2l-1.9 1.9"/>"##,
        }
    }

    /// The SVG document.
    pub fn svg(self) -> String {
        if self.filled() {
            format!(r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="#000">{}</svg>"##, self.body())
        } else {
            format!(
                r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#000" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">{}</svg>"##,
                self.body()
            )
        }
    }
}

fn handles() -> &'static HashMap<Icon, Handle> {
    static H: OnceLock<HashMap<Icon, Handle>> = OnceLock::new();
    H.get_or_init(|| {
        Icon::ALL
            .into_iter()
            .map(|i| (i, Handle::from_memory(i.svg().into_bytes())))
            .collect()
    })
}

/// The handle of an icon (shared, cheap to clone).
pub fn handle(i: Icon) -> Handle {
    handles()[&i].clone()
}

/// An icon of `size` px in `color`.
pub fn icon<'a, M: 'a>(i: Icon, size: f32, color: Color) -> El<'a, M> {
    svg::Svg::new(handle(i))
        .width(size)
        .height(size)
        .style(move |_, _| svg::Style { color: Some(color) })
        .into()
}

const LOGO_PATHS: [&str; 3] = [
    "M249.205 114.329 L510.699 32.377 C510.699 32.377 525.463 25.913 538.636 31.22 C551.81 36.528 551.358 49.709 551.358 49.709 L550.464 99.763 C550.464 99.763 550.049 113.161 538.636 121.587 C527.224 130.012 514.95 133.407 514.95 133.407 L308.537 197.811 C298.175 201.165 275.923 206.119 273.697 229.869 C273.697 229.869 275.227 403.645 275.227 443.582 C275.227 493.942 241.622 582.872 154.358 582.872 C65.76 582.872 47.709 513.389 47.709 470.406 C47.709 412.557 102.003 373.129 140.138 369.574 C178.273 366.019 204.127 385.087 204.127 385.087 L201.373 166.271 C199.204 125.931 244.035 116.052 249.205 114.329 Z",
    "M345.928 420.623 C345.928 420.623 518.078 368.274 535.733 363.243 C553.388 358.212 564.789 368.142 564.274 383.837 L564.274 419.323 C564.274 449.986 539.212 459.963 525.178 464.079 L348.687 519.256 C329.357 525.979 320.186 517.423 320.186 498.97 L320.186 446.803 C320.186 430.002 326.687 425.907 345.928 420.623 Z",
    "M339.143 254.402 L494.664 206.043 C507.578 202.394 520.795 206.043 520.795 221.613 L520.795 239.527 C520.795 274.769 508.753 302.967 450.793 319.539 L341.142 356.25 C330.333 359.74 319.414 354.699 320.186 340.976 L319.414 282.394 C319.414 269.641 322.93 259.403 339.143 254.402 Z",
];

/// The Encore logo in the accent gradient (web-rs/src/appearance.rs `logo()`), cached per pair of colours.
pub fn logo_handle(a: Color, b: Color) -> Handle {
    static CACHE: Mutex<Option<HashMap<String, Handle>>> = Mutex::new(None);
    let key = format!("{}{}", theme::to_hex(a), theme::to_hex(b));
    let mut g = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let map = g.get_or_insert_with(HashMap::new);
    if let Some(h) = map.get(&key) {
        return h.clone();
    }
    let mut s = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="16 15 580 580"><defs><linearGradient id="g" x1="71" y1="351" x2="551" y2="257" gradientUnits="userSpaceOnUse"><stop offset="0" stop-color="{}"/><stop offset="1" stop-color="{}"/></linearGradient></defs>"#,
        theme::to_hex(a),
        theme::to_hex(b)
    );
    for d in LOGO_PATHS {
        s.push_str(&format!(r##"<path d="{d}" fill="url(#g)"/>"##));
    }
    s.push_str("</svg>");
    let h = Handle::from_memory(s.into_bytes());
    map.insert(key, h.clone());
    h
}

/// The logo at `size` px in the active look's accent colours.
pub fn logo<'a, M: 'a>(size: f32) -> El<'a, M> {
    let p = theme::look();
    svg::Svg::new(logo_handle(p.accent, p.accent2))
        .width(size)
        .height(size)
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_web_icon_has_a_handle_and_a_name() {
        for i in Icon::ALL {
            assert_eq!(Icon::by_name(i.name()), Some(i));
            assert!(i.svg().contains("<svg"));
            let _ = handle(i);
        }
        assert_eq!(Icon::by_name("devices"), Some(Icon::Devices));
        assert_eq!(Icon::by_name("nothing"), None);
    }
}
