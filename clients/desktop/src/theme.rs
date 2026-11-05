//! The look: the web app's theme tokens (web-rs/static/css/app.css `:root` and the
//! `[data-theme=…]` blocks) and accents (web-rs/src/appearance.rs) as colours the widget
//! styles read. One look is active at a time ([`set`]); styles call [`look`].
//!
//! The look and every widget style come from the kit (`kit_iced::theme`, generated from the kit's
//! design tokens), so Encore's desktop app and every other kit app share them.

//!
//! Encore keeps two things of its own on top: the names of two accents (its saved settings and the
//! web call the kit's `cream` and `pastel` accents `logo` "Logo" and `encore` "Encore"), which
//! [`ACCENTS`], [`PRESETS`] and [`palette`] translate, and the height of its player bar
//! ([`PLAYER_H`]).

pub use kit_iced::theme::*;

use std::sync::LazyLock;

/// The height of the player bar along the bottom of the window; toasts and full-height pages keep
/// clear of it.
pub const PLAYER_H: f32 = 84.0;

/// Encore's ids and names for the kit's accents: `(kit id, Encore id, Encore name)`.
const OWN_ACCENTS: [(&str, &str, &str); 2] =
    [("cream", "logo", "Logo"), ("pastel", "encore", "Encore")];

/// Encore's id and name for a kit accent (the kit's own for every other accent).
fn own_accent(id: &'static str, name: &'static str) -> (&'static str, &'static str) {
    OWN_ACCENTS
        .iter()
        .find(|x| x.0 == id)
        .map_or((id, name), |x| (x.1, x.2))
}

/// The kit's id for an accent id Encore saved (`logo` → `cream`, `encore` → `pastel`).
pub fn kit_accent(id: &str) -> &str {
    OWN_ACCENTS.iter().find(|x| x.1 == id).map_or(id, |x| x.0)
}

/// An accent: `(id, name, colour, partner)`.
pub type Accent = (&'static str, &'static str, &'static str, &'static str);

/// A preset: `(id, name, theme, accent, custom, background)`.
pub type Preset = (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
);

/// Every accent as `(id, name, colour, partner)`, with Encore's ids and names.
pub static ACCENTS: LazyLock<Vec<Accent>> = LazyLock::new(|| {
    kit_iced::theme::ACCENTS
        .iter()
        .map(|&(id, name, a, b)| {
            let (id, name) = own_accent(id, name);
            (id, name, a, b)
        })
        .collect()
});

/// The presets as `(id, name, theme, accent, custom, background)`, their accents in Encore's ids.
pub static PRESETS: LazyLock<Vec<Preset>> = LazyLock::new(|| {
    kit_iced::theme::PRESETS
        .iter()
        .map(|&(id, name, theme, accent, custom, background)| {
            (
                id,
                name,
                theme,
                own_accent(accent, "").0,
                custom,
                background,
            )
        })
        .collect()
});

/// The kit's [`kit_iced::theme::palette`] for a look Encore saved (its accent ids translated).
pub fn palette(
    theme: &str,
    accent: &str,
    custom: &str,
    background: &str,
    system_dark: bool,
) -> Palette {
    kit_iced::theme::palette(theme, kit_accent(accent), custom, background, system_dark)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_follow_the_css() {
        let p = palette("encore", "logo", "", "solid", true);
        assert_eq!(to_hex(p.bg), "#161617");
        assert_eq!(to_hex(p.text), "#f0ead6");
        assert_eq!(to_hex(p.accent), "#f0ead6");
        assert_eq!(
            to_hex(p.on_accent),
            "#16161f",
            "a light accent gets dark text"
        );
        let d = palette("dark", "violet", "", "fade", true);
        assert_eq!(to_hex(d.bg), "#0b0b12");
        assert!(d.fade && d.dark);
        assert_eq!(to_hex(d.on_accent), "#ffffff");
        let l = palette("system", "sky", "", "solid", false);
        assert!(!l.dark);
        assert_eq!(to_hex(l.bg), "#f6f6fb");
        let o = palette("oled", "custom", "#e5e5e5", "solid", true);
        assert_eq!(to_hex(o.bg), "#000000");
        assert_eq!(to_hex(o.accent), "#e5e5e5");
    }

    #[test]
    fn encores_accent_ids_and_names_stay() {
        assert!(ACCENTS.contains(&("logo", "Logo", "#f0ead6", "#d6c49a")));
        assert!(ACCENTS.contains(&("encore", "Encore", "#f2a7e4", "#9fd2fb")));
        assert!(ACCENTS.iter().any(|a| a.0 == "violet"));
        assert_eq!(
            PRESETS[0],
            ("encore", "Encore", "encore", "logo", "", "solid")
        );
        assert!(PRESETS.contains(&("pastel", "Pastel", "dark", "encore", "", "fade")));
        assert_eq!(
            (kit_accent("logo"), kit_accent("encore"), kit_accent("teal")),
            ("cream", "pastel", "teal")
        );
        assert_eq!(
            to_hex(palette("dark", "encore", "", "fade", true).accent),
            "#f2a7e4"
        );
    }

    #[test]
    fn hex_round_trips() {
        assert_eq!(to_hex(hex("#8b5cf6")), "#8b5cf6");
        assert_eq!(to_hex(hex("fff")), "#ffffff");
        assert_eq!(to_hex(hex("nonsense")), "#808080");
    }
}
