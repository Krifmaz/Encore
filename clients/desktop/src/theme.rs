//! The look: the web app's theme tokens (web-rs/static/css/app.css `:root` and the
//! `[data-theme=…]` blocks) and accents (web-rs/src/appearance.rs) as colours the widget
//! styles read. One look is active at a time ([`set`]); styles call [`look`].
//!
//! The look and every widget style come from the kit (`kit_iced::theme`, generated from the kit's
//! design tokens), so Encore's desktop app and every other kit app share them.

pub use kit_iced::theme::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_follow_the_css() {
        let p = palette("encore", "logo", "", "solid", true);
        assert_eq!(to_hex(p.bg), "#161617");
        assert_eq!(to_hex(p.text), "#f0ead6");
        assert_eq!(to_hex(p.accent), "#f0ead6");
        assert_eq!(to_hex(p.on_accent), "#16161f", "a light accent gets dark text");
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
    fn hex_round_trips() {
        assert_eq!(to_hex(hex("#8b5cf6")), "#8b5cf6");
        assert_eq!(to_hex(hex("fff")), "#ffffff");
        assert_eq!(to_hex(hex("nonsense")), "#808080");
    }
}
