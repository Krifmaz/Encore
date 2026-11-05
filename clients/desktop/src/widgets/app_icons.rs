//! Encore's own icons and logo on top of the kit's set: the music icons (a song, an album, the
//! transport, shuffle, the library, the volume) are `kit_iced::widgets::Icon::App` icons drawn like
//! the kit's, so every kit widget that takes an `Icon` takes them too. The logo is Encore's mark in
//! the accent gradient (web-rs/src/appearance.rs `logo()`).

use super::{El, Icon};
use kit_iced::widgets::{AppIcon, Logo};

/// A song (a cover's stand-in, the Songs rows).
pub const MUSIC: Icon = Icon::App(AppIcon::stroke(
    "music",
    r#"<path d="M9 18V5l12-2v13"/><circle cx="6" cy="18" r="3"/><circle cx="18" cy="16" r="3"/>"#,
));
/// An album (a cover's stand-in, the album pages).
pub const DISC: Icon = Icon::App(AppIcon::stroke(
    "disc",
    r#"<circle cx="12" cy="12" r="10"/><circle cx="12" cy="12" r="3"/>"#,
));
/// Next song (filled, like Play and Pause).
pub const NEXT: Icon = Icon::App(AppIcon::filled(
    "next",
    r#"<path d="M5 5.5v13a1 1 0 0 0 1.5.86L16 13.5V18a1 1 0 0 0 2 0V6a1 1 0 0 0-2 0v4.5L6.5 4.64A1 1 0 0 0 5 5.5z"/>"#,
));
/// Previous song (filled, like Play and Pause).
pub const PREV: Icon = Icon::App(AppIcon::filled(
    "prev",
    r#"<path d="M19 5.5v13a1 1 0 0 1-1.5.86L8 13.5V18a1 1 0 0 1-2 0V6a1 1 0 0 1 2 0v4.5l9.5-5.86A1 1 0 0 1 19 5.5z"/>"#,
));
/// The library.
pub const LIBRARY: Icon = Icon::App(AppIcon::stroke(
    "library",
    r#"<path d="M4 4v16M9 4v16M14 4l6 16"/>"#,
));
/// The volume.
pub const VOLUME: Icon = Icon::App(AppIcon::stroke(
    "volume",
    r#"<path d="M11 5 6 9H2v6h4l5 4zM15.54 8.46a5 5 0 0 1 0 7.07M19.07 4.93a10 10 0 0 1 0 14.14"/>"#,
));
/// Shuffle.
pub const SHUFFLE: Icon = Icon::App(AppIcon::stroke(
    "shuffle",
    r#"<path d="M16 3h5v5M4 20 21 3M21 16v5h-5M15 15l6 6M4 4l5 5"/>"#,
));

/// Encore's own icons, for lookups by name.
pub const ALL: [Icon; 7] = [MUSIC, DISC, NEXT, PREV, LIBRARY, VOLUME, SHUFFLE];

/// The icon of a web name: the kit's, then Encore's own (the player bar's action table names them).
pub fn by_name(name: &str) -> Option<Icon> {
    Icon::by_name(name).or_else(|| ALL.into_iter().find(|i| i.name() == name))
}

/// Encore's mark: three shapes in a 580×580 box, the gradient running left to right across them.
pub const LOGO: Logo = Logo {
    view_box: "16 15 580 580",
    paths: &[
        "M249.205 114.329 L510.699 32.377 C510.699 32.377 525.463 25.913 538.636 31.22 C551.81 36.528 551.358 49.709 551.358 49.709 L550.464 99.763 C550.464 99.763 550.049 113.161 538.636 121.587 C527.224 130.012 514.95 133.407 514.95 133.407 L308.537 197.811 C298.175 201.165 275.923 206.119 273.697 229.869 C273.697 229.869 275.227 403.645 275.227 443.582 C275.227 493.942 241.622 582.872 154.358 582.872 C65.76 582.872 47.709 513.389 47.709 470.406 C47.709 412.557 102.003 373.129 140.138 369.574 C178.273 366.019 204.127 385.087 204.127 385.087 L201.373 166.271 C199.204 125.931 244.035 116.052 249.205 114.329 Z",
        "M345.928 420.623 C345.928 420.623 518.078 368.274 535.733 363.243 C553.388 358.212 564.789 368.142 564.274 383.837 L564.274 419.323 C564.274 449.986 539.212 459.963 525.178 464.079 L348.687 519.256 C329.357 525.979 320.186 517.423 320.186 498.97 L320.186 446.803 C320.186 430.002 326.687 425.907 345.928 420.623 Z",
        "M339.143 254.402 L494.664 206.043 C507.578 202.394 520.795 206.043 520.795 221.613 L520.795 239.527 C520.795 274.769 508.753 302.967 450.793 319.539 L341.142 356.25 C330.333 359.74 319.414 354.699 320.186 340.976 L319.414 282.394 C319.414 269.641 322.93 259.403 339.143 254.402 Z",
    ],
    gradient: (71.0, 351.0, 551.0, 257.0),
};

/// Encore's logo at `size` px in the active look's accent colours.
pub fn logo<'a, M: 'a>(size: f32) -> El<'a, M> {
    kit_iced::widgets::logo(&LOGO, size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encores_icons_are_found_by_name_beside_the_kits() {
        for i in ALL {
            assert_eq!(by_name(i.name()), Some(i));
            assert!(i.svg().contains("<svg"));
        }
        assert_eq!(by_name("home"), Some(Icon::Home));
        assert_eq!(by_name("nothing"), None);
        // the transport is filled like Play, the rest stroked
        assert!(!NEXT.svg().contains("stroke-width") && SHUFFLE.svg().contains("stroke-width"));
    }

    #[test]
    fn the_logo_is_encores_mark() {
        let s = LOGO.svg(iced::Color::WHITE, iced::Color::BLACK);
        assert!(s.contains(r#"viewBox="16 15 580 580""#) && s.contains(r#"x1="71""#));
        let _ = logo::<()>(30.0);
    }
}
