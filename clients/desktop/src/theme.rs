//! The look: the web app's theme tokens (web-rs/static/css/app.css `:root` and the
//! `[data-theme=…]` blocks) and accents (web-rs/src/appearance.rs) as colours the widget
//! styles read. One look is active at a time ([`set`]); styles call [`look`].

use iced::border::Radius;
use iced::widget::{
    button, container, overlay::menu, pick_list, progress_bar, rule, scrollable, slider,
    text_input, toggler,
};
use iced::{Background, Border, Color, Font, Shadow, Theme, Vector};
use std::sync::RwLock;

/// `--radius`.
pub const RADIUS: f32 = 14.0;
/// `--sidebar`.
pub const SIDEBAR: f32 = 236.0;
/// `--player-h`.
pub const PLAYER_H: f32 = 84.0;

/// The colour tokens of one theme with one accent.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub bg: Color,
    pub bg2: Color,
    pub panel: Color,
    pub panel2: Color,
    pub border: Color,
    pub text: Color,
    pub muted: Color,
    pub faint: Color,
    pub accent: Color,
    pub accent2: Color,
    pub on_accent: Color,
    pub ok: Color,
    pub warn: Color,
    pub err: Color,
    pub info: Color,
    pub dark: bool,
    /// `data-bg="fade"`: a soft wash of the accent behind the page.
    pub fade: bool,
}

/// `(id, name)` of the themes the Appearance page offers, in its order.
pub const THEMES: [(&str, &str); 5] = [
    ("encore", "Encore"),
    ("dark", "Dark"),
    ("oled", "OLED black"),
    ("light", "Light"),
    ("system", "Follow the system"),
];

/// `(id, name, colour, second colour)` – web-rs/src/appearance.rs ACCENTS.
pub const ACCENTS: [(&str, &str, &str, &str); 10] = [
    ("logo", "Logo", "#f0ead6", "#d6c49a"),
    ("violet", "Violet", "#8b5cf6", "#ec4899"),
    ("encore", "Encore", "#f2a7e4", "#9fd2fb"),
    ("lilac", "Lilac", "#b48cf2", "#7cc4fa"),
    ("pink", "Pink", "#ec4899", "#f97316"),
    ("sky", "Sky", "#0ea5e9", "#8b5cf6"),
    ("teal", "Teal", "#14b8a6", "#0ea5e9"),
    ("green", "Green", "#22c55e", "#14b8a6"),
    ("amber", "Amber", "#f59e0b", "#ef4444"),
    ("red", "Red", "#ef4444", "#ec4899"),
];

/// `(id, name, theme, accent, custom colour, background)` – web-rs PRESETS.
pub const PRESETS: [(&str, &str, &str, &str, &str, &str); 6] = [
    ("encore", "Encore", "encore", "logo", "", "solid"),
    ("default", "Violet dark", "dark", "violet", "", "fade"),
    ("oled", "OLED black", "oled", "violet", "", "solid"),
    ("pastel", "Pastel", "dark", "encore", "", "fade"),
    ("light", "Daylight", "light", "sky", "", "fade"),
    ("mono", "Mono", "oled", "custom", "#e5e5e5", "solid"),
];

/// `#rrggbb` (or `#rgb`) to a colour; grey when it does not parse.
pub fn hex(s: &str) -> Color {
    let h = s.trim().trim_start_matches('#');
    let h: String = if h.len() == 3 {
        h.chars().flat_map(|c| [c, c]).collect()
    } else {
        h.to_string()
    };
    match u32::from_str_radix(&h, 16) {
        Ok(n) if h.len() == 6 => Color::from_rgb8((n >> 16) as u8, (n >> 8) as u8, n as u8),
        _ => Color::from_rgb8(128, 128, 128),
    }
}

/// A colour back to `#rrggbb`.
pub fn to_hex(c: Color) -> String {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", b(c.r), b(c.g), b(c.b))
}

fn luminance(c: Color) -> f32 {
    let f = |v: f32| {
        if v <= 0.03928 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * f(c.r) + 0.7152 * f(c.g) + 0.0722 * f(c.b)
}

/// The second colour for a custom accent: the hue turned a little, as the web does.
pub fn partner(c: Color) -> Color {
    let (r, g, b) = (c.r, c.g, c.b);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    let (mut h, mut s) = (0.0, 0.0);
    if max != min {
        let d = max - min;
        s = if l > 0.5 {
            d / (2.0 - max - min)
        } else {
            d / (max + min)
        };
        h = if max == r {
            (g - b) / d + if g < b { 6.0 } else { 0.0 }
        } else if max == g {
            (b - r) / d + 2.0
        } else {
            (r - g) / d + 4.0
        } / 6.0;
    }
    h = (h + 0.1) % 1.0;
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    let f = |t: f32| {
        let t = (t + 1.0) % 1.0;
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    };
    Color::from_rgb(f(h + 1.0 / 3.0), f(h), f(h - 1.0 / 3.0))
}

/// Mix `a` and `b`: `t` = 0 is all `a`.
pub fn mix(a: Color, b: Color, t: f32) -> Color {
    Color::from_rgba(
        a.r + (b.r - a.r) * t,
        a.g + (b.g - a.g) * t,
        a.b + (b.b - a.b) * t,
        a.a + (b.a - a.a) * t,
    )
}

/// The tokens for a theme id (`encore`, `dark`, `oled`, `light`, `system`), an accent id (or
/// `custom` with `custom` as `#rrggbb`) and a background (`solid` or `fade`). `system_dark`
/// decides what `system` means.
pub fn palette(theme: &str, accent: &str, custom: &str, background: &str, system_dark: bool) -> Palette {
    let theme = if theme == "system" {
        if system_dark { "dark" } else { "light" }
    } else {
        theme
    };
    let dark = theme != "light";
    let (bg, bg2, panel, panel2, border, text, muted, faint) = match theme {
        "encore" => ("#161617", "#1c1c1e", "#222224", "#2a2a2d", "#353538", "#f0ead6", "#aaa493", "#77725f"),
        "oled" => ("#000000", "#000000", "#0b0b0d", "#17171b", "#232329", "#ececf4", "#9a9aa8", "#66666f"),
        "light" => ("#f6f6fb", "#ffffff", "#ffffff", "#f0f0f7", "#e2e2ee", "#16161f", "#5d5d75", "#8d8da5"),
        _ => ("#0b0b12", "#12121c", "#161624", "#1d1d2e", "#2a2a40", "#ececf4", "#9a9ab3", "#6b6b85"),
    };
    let (a, b) = match ACCENTS.iter().find(|x| x.0 == accent) {
        Some(x) => (hex(x.2), hex(x.3)),
        None => {
            let a = hex(if custom.trim().is_empty() { "#f0ead6" } else { custom });
            (a, partner(a))
        }
    };
    let on_accent = if (luminance(a) + luminance(b)) / 2.0 > 0.45 {
        hex("#16161f")
    } else {
        Color::WHITE
    };
    Palette {
        bg: hex(bg),
        bg2: hex(bg2),
        panel: hex(panel),
        panel2: hex(panel2),
        border: hex(border),
        text: hex(text),
        muted: hex(muted),
        faint: hex(faint),
        accent: a,
        accent2: b,
        on_accent,
        ok: hex("#22c55e"),
        warn: hex("#f59e0b"),
        err: hex("#ef4444"),
        info: hex("#38bdf8"),
        dark,
        fade: background == "fade",
    }
}

static LOOK: RwLock<Option<Palette>> = RwLock::new(None);

/// Make `p` the look every style reads.
pub fn set(p: Palette) {
    *LOOK.write().unwrap_or_else(|e| e.into_inner()) = Some(p);
}

/// The active look (the default Encore look before [`set`] was called).
pub fn look() -> Palette {
    (*LOOK.read().unwrap_or_else(|e| e.into_inner()))
        .unwrap_or_else(|| palette("encore", "logo", "", "solid", true))
}

/// The iced theme built from the look, so built-in widgets (menus, defaults) match.
pub fn iced_theme(p: &Palette) -> Theme {
    Theme::custom(
        "Encore".to_string(),
        iced::theme::Palette {
            background: p.bg,
            text: p.text,
            primary: p.accent,
            success: p.ok,
            warning: p.warn,
            danger: p.err,
        },
    )
}

/// Bold text (`font-weight: 700`).
pub fn bold() -> Font {
    Font {
        weight: iced::font::Weight::Bold,
        ..Font::DEFAULT
    }
}

/// Semibold text (`font-weight: 600`, buttons and titles).
pub fn semibold() -> Font {
    Font {
        weight: iced::font::Weight::Semibold,
        ..Font::DEFAULT
    }
}

/// Monospace (`.mono`).
pub fn mono() -> Font {
    Font::MONOSPACE
}

fn no_shadow() -> Shadow {
    Shadow::default()
}

/// The soft drop shadow (`--shadow`) of cards that float: modals, menus, toasts.
pub fn shadow(p: &Palette) -> Shadow {
    if p.dark && p.bg == Color::BLACK {
        return no_shadow();
    }
    Shadow {
        color: if p.dark {
            Color::from_rgba(0.0, 0.0, 0.0, 0.35)
        } else {
            Color::from_rgba8(20, 20, 50, 0.08)
        },
        offset: Vector::new(0.0, 10.0),
        blur_radius: 30.0,
    }
}

// ----- buttons -------------------------------------------------------------------------

/// The button looks of app.css: `.btn`, `.btn.primary`, `.btn.ghost`, `.btn.danger`,
/// `.btn.icon` (ghost, square), the sidebar `.nav a`, `.tabs button`, `.seg button`, and the
/// invisible one for rows and tiles (`Row`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Btn {
    Primary,
    Default,
    Ghost,
    Danger,
    Icon,
    Nav,
    NavOn,
    Tab,
    TabOn,
    Seg,
    SegOn,
    Row,
    /// The round play button of the player bar and tile overlays.
    Play,
}

/// `button(...).style(theme::btn(Btn::Primary))`.
pub fn btn(kind: Btn) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| button_style(kind, status)
}

fn button_style(kind: Btn, status: button::Status) -> button::Style {
    use button::Status::{Active, Disabled, Hovered, Pressed};
    let p = look();
    let hovered = matches!(status, Hovered | Pressed);
    let radius: Radius = match kind {
        Btn::Nav | Btn::NavOn => 10.0.into(),
        Btn::Tab | Btn::TabOn | Btn::Row => 8.0.into(),
        Btn::Seg | Btn::SegOn => 0.0.into(),
        Btn::Play => 99.0.into(),
        _ => 10.0.into(),
    };
    let border = |c: Color| Border {
        color: c,
        width: 1.0,
        radius,
    };
    let none = Border {
        color: Color::TRANSPARENT,
        width: 0.0,
        radius,
    };
    let mut style = match kind {
        Btn::Primary => button::Style {
            background: Some(Background::Gradient(iced::Gradient::Linear(
                iced::gradient::Linear::new(iced::Radians(2.356))
                    .add_stop(0.0, p.accent)
                    .add_stop(1.0, p.accent2),
            ))),
            text_color: p.on_accent,
            border: none,
            shadow: no_shadow(),
            snap: true,
        },
        Btn::Default => button::Style {
            background: Some(Background::Color(p.panel2)),
            text_color: p.text,
            border: border(if hovered { p.accent } else { p.border }),
            shadow: no_shadow(),
            snap: true,
        },
        Btn::Danger => button::Style {
            background: Some(Background::Color(p.panel2)),
            text_color: p.err,
            border: border(if hovered { p.err } else { p.border }),
            shadow: no_shadow(),
            snap: true,
        },
        Btn::Ghost | Btn::Icon => button::Style {
            background: hovered.then_some(Background::Color(p.panel2)),
            text_color: p.text,
            border: none,
            shadow: no_shadow(),
            snap: true,
        },
        Btn::Nav => button::Style {
            background: hovered.then_some(Background::Color(p.panel2)),
            text_color: if hovered { p.text } else { p.muted },
            border: none,
            shadow: no_shadow(),
            snap: true,
        },
        Btn::NavOn => button::Style {
            background: Some(Background::Color(p.panel2)),
            text_color: p.text,
            border: none,
            shadow: no_shadow(),
            snap: true,
        },
        Btn::Tab | Btn::Seg => button::Style {
            background: None,
            text_color: if hovered { p.text } else { p.muted },
            border: none,
            shadow: no_shadow(),
            snap: true,
        },
        Btn::TabOn | Btn::SegOn => button::Style {
            background: Some(Background::Color(p.panel2)),
            text_color: p.text,
            border: none,
            shadow: no_shadow(),
            snap: true,
        },
        Btn::Row => button::Style {
            background: hovered.then_some(Background::Color(p.panel2)),
            text_color: p.text,
            border: none,
            shadow: no_shadow(),
            snap: true,
        },
        Btn::Play => button::Style {
            background: Some(Background::Color(p.accent)),
            text_color: p.on_accent,
            border: none,
            shadow: no_shadow(),
            snap: true,
        },
    };
    if let Disabled = status {
        style.text_color = style.text_color.scale_alpha(0.5);
        if let Some(Background::Color(c)) = style.background {
            style.background = Some(Background::Color(c.scale_alpha(0.6)));
        }
    }
    if let Active = status {
        if kind == Btn::Primary && !p.dark {
            style.shadow = no_shadow();
        }
    }
    style
}

// ----- inputs ---------------------------------------------------------------------------

/// `.input`: panel background, border, accent ring when focused.
pub fn input(_: &Theme, status: text_input::Status) -> text_input::Style {
    let p = look();
    let focused = matches!(status, text_input::Status::Focused { .. });
    text_input::Style {
        background: Background::Color(p.panel2),
        border: Border {
            color: if focused { p.accent } else { p.border },
            width: 1.0,
            radius: 10.0.into(),
        },
        icon: p.faint,
        placeholder: p.faint,
        value: p.text,
        selection: p.accent.scale_alpha(0.35),
    }
}

/// `.switch`.
pub fn switch(_: &Theme, status: toggler::Status) -> toggler::Style {
    let p = look();
    let on = matches!(
        status,
        toggler::Status::Active { is_toggled: true } | toggler::Status::Hovered { is_toggled: true }
    );
    toggler::Style {
        background: Background::Color(if on { p.accent } else { p.border }),
        background_border_width: 0.0,
        background_border_color: Color::TRANSPARENT,
        foreground: Background::Color(Color::WHITE),
        foreground_border_width: 0.0,
        foreground_border_color: Color::TRANSPARENT,
        text_color: Some(p.text),
        border_radius: None,
        padding_ratio: 0.14,
    }
}

/// `select.input`.
pub fn pick(_: &Theme, status: pick_list::Status) -> pick_list::Style {
    let p = look();
    let hovered = matches!(status, pick_list::Status::Hovered | pick_list::Status::Opened { .. });
    pick_list::Style {
        text_color: p.text,
        placeholder_color: p.faint,
        handle_color: p.muted,
        background: Background::Color(p.panel2),
        border: Border {
            color: if hovered { p.accent } else { p.border },
            width: 1.0,
            radius: 10.0.into(),
        },
    }
}

/// The dropdown of a pick list (`.menu-pop` look).
pub fn pick_menu(_: &Theme) -> menu::Style {
    let p = look();
    menu::Style {
        background: Background::Color(p.panel),
        border: Border {
            color: p.border,
            width: 1.0,
            radius: 12.0.into(),
        },
        text_color: p.text,
        selected_text_color: p.text,
        selected_background: Background::Color(p.panel2),
        shadow: shadow(&p),
    }
}

/// The seek and volume sliders.
pub fn slider_style(_: &Theme, status: slider::Status) -> slider::Style {
    let p = look();
    let active = matches!(status, slider::Status::Hovered | slider::Status::Dragged);
    slider::Style {
        rail: slider::Rail {
            backgrounds: (Background::Color(p.accent), Background::Color(p.border)),
            width: 5.0,
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: 99.0.into(),
            },
        },
        handle: slider::Handle {
            shape: slider::HandleShape::Circle {
                radius: if active { 7.5 } else { 6.5 },
            },
            background: Background::Color(p.text),
            border_width: 0.0,
            border_color: Color::TRANSPARENT,
        },
    }
}

/// `.progress`.
pub fn progress(_: &Theme) -> progress_bar::Style {
    let p = look();
    progress_bar::Style {
        background: Background::Color(p.border),
        bar: Background::Color(p.accent),
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: 99.0.into(),
        },
    }
}

/// A progress bar in the error colour (a nearly full disk).
pub fn progress_err(_: &Theme) -> progress_bar::Style {
    let p = look();
    progress_bar::Style {
        background: Background::Color(p.border),
        bar: Background::Color(p.err),
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: 99.0.into(),
        },
    }
}

/// A 1px `--border` line.
pub fn rule_style(_: &Theme) -> rule::Style {
    rule::Style {
        color: look().border,
        radius: 0.0.into(),
        fill_mode: rule::FillMode::Full,
        snap: true,
    }
}

/// Thin scrollbars in the border colour.
pub fn scroll(_: &Theme, status: scrollable::Status) -> scrollable::Style {
    let p = look();
    let hovered = !matches!(status, scrollable::Status::Active { .. });
    let rail = scrollable::Rail {
        background: None,
        border: Border::default(),
        scroller: scrollable::Scroller {
            background: Background::Color(if hovered { p.faint } else { p.border }),
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: 99.0.into(),
            },
        },
    };
    scrollable::Style {
        container: container::Style::default(),
        vertical_rail: rail,
        horizontal_rail: rail,
        gap: None,
        auto_scroll: scrollable::AutoScroll {
            background: Background::Color(p.panel2),
            border: Border::default(),
            shadow: no_shadow(),
            icon: p.text,
        },
    }
}

// ----- containers -----------------------------------------------------------------------

fn boxed(bg: Color, border: Color, radius: f32) -> container::Style {
    container::Style {
        text_color: None,
        background: Some(Background::Color(bg)),
        border: Border {
            color: border,
            width: 1.0,
            radius: radius.into(),
        },
        shadow: no_shadow(),
        snap: true,
    }
}

/// The page background (`body`).
pub fn app_bg(_: &Theme) -> container::Style {
    let p = look();
    container::Style {
        text_color: Some(p.text),
        background: Some(Background::Color(p.bg)),
        ..container::Style::default()
    }
}

/// `.sidebar`: `--bg-2` with a right border.
pub fn sidebar(_: &Theme) -> container::Style {
    let p = look();
    container::Style {
        text_color: Some(p.text),
        background: Some(Background::Color(p.bg2)),
        border: Border {
            color: p.border,
            width: 0.0,
            radius: 0.0.into(),
        },
        ..container::Style::default()
    }
}

/// `.player`: the bar along the bottom.
pub fn player_bar(_: &Theme) -> container::Style {
    let p = look();
    container::Style {
        text_color: Some(p.text),
        background: Some(Background::Color(p.bg2)),
        ..container::Style::default()
    }
}

/// `.card`.
pub fn card(_: &Theme) -> container::Style {
    let p = look();
    boxed(p.panel, p.border, RADIUS)
}

/// A card in `--panel-2` (nested boxes, hover rows).
pub fn panel2(_: &Theme) -> container::Style {
    let p = look();
    boxed(p.panel2, p.border, 12.0)
}

/// A square frame around covers (`.thumb`, `.tile .art`).
pub fn art(_: &Theme) -> container::Style {
    let p = look();
    boxed(p.panel2, Color::TRANSPARENT, 10.0)
}

/// A rounded box in the accent gradient (logo tile, avatar).
pub fn accent_box(_: &Theme) -> container::Style {
    let p = look();
    container::Style {
        text_color: Some(p.on_accent),
        background: Some(Background::Gradient(iced::Gradient::Linear(
            iced::gradient::Linear::new(iced::Radians(2.356))
                .add_stop(0.0, p.accent)
                .add_stop(1.0, p.accent2),
        ))),
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: 10.0.into(),
        },
        ..container::Style::default()
    }
}

/// A round avatar in the accent gradient.
pub fn avatar(t: &Theme) -> container::Style {
    let mut s = accent_box(t);
    s.border.radius = 99.0.into();
    s
}

/// `.modal-bg`: the dimmed backdrop.
pub fn modal_bg(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(Color::from_rgba8(5, 5, 10, 0.6))),
        ..container::Style::default()
    }
}

/// `.modal`: the dialog card.
pub fn modal(_: &Theme) -> container::Style {
    let p = look();
    let mut s = boxed(p.panel, p.border, 18.0);
    s.shadow = shadow(&p);
    s
}

/// `.menu-pop`: the ⋯ menu.
pub fn menu_pop(_: &Theme) -> container::Style {
    let p = look();
    let mut s = boxed(p.panel, p.border, 12.0);
    s.shadow = shadow(&p);
    s
}

/// `.toast` with its coloured left edge painted by the toast widget.
pub fn toast(_: &Theme) -> container::Style {
    let p = look();
    let mut s = boxed(p.panel, p.border, 12.0);
    s.shadow = shadow(&p);
    s
}

/// `.error-box`.
pub fn error_box(_: &Theme) -> container::Style {
    let p = look();
    container::Style {
        text_color: Some(if p.dark { hex("#fca5a5") } else { hex("#b91c1c") }),
        background: Some(Background::Color(p.err.scale_alpha(0.12))),
        border: Border {
            color: p.err.scale_alpha(0.35),
            width: 1.0,
            radius: 10.0.into(),
        },
        ..container::Style::default()
    }
}

/// `.banner.warn`: an amber box for something that needs attention but is not an error (imports
/// paused while the library cannot be written).
pub fn warn_box(_: &Theme) -> container::Style {
    let p = look();
    container::Style {
        text_color: Some(p.text),
        background: Some(Background::Color(p.warn.scale_alpha(0.12))),
        border: Border {
            color: p.warn.scale_alpha(0.4),
            width: 1.0,
            radius: 10.0.into(),
        },
        ..container::Style::default()
    }
}

/// `.tabs` and `.seg`: the frame around tab buttons.
pub fn tabs_frame(_: &Theme) -> container::Style {
    let p = look();
    boxed(p.panel, p.border, 12.0)
}

/// `.skeleton`: a grey placeholder block.
pub fn skeleton(_: &Theme) -> container::Style {
    let p = look();
    boxed(p.panel2, Color::TRANSPARENT, 8.0)
}

/// The badge styles of app.css: `.badge`, `.badge.ok`, `.warn`, `.err`, `.info`, `.accent`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Badge {
    Plain,
    Ok,
    Warn,
    Err,
    Info,
    Accent,
}

/// The colours of a badge: (text, border, background).
pub fn badge_colors(kind: Badge) -> (Color, Color, Color) {
    let p = look();
    let tinted = |c: Color| (c, c.scale_alpha(0.35), c.scale_alpha(0.10));
    match kind {
        Badge::Plain => (p.muted, p.border, p.panel2),
        Badge::Ok => tinted(p.ok),
        Badge::Warn => tinted(p.warn),
        Badge::Err => tinted(p.err),
        Badge::Info => tinted(p.info),
        Badge::Accent => (mix(p.accent, p.text, 0.45), p.accent.scale_alpha(0.4), p.accent.scale_alpha(0.14)),
    }
}

/// `.badge.<kind>` as a container style.
pub fn badge(kind: Badge) -> impl Fn(&Theme) -> container::Style {
    move |_| {
        let (text, border, bg) = badge_colors(kind);
        container::Style {
            text_color: Some(text),
            background: Some(Background::Color(bg)),
            border: Border {
                color: border,
                width: 1.0,
                radius: 99.0.into(),
            },
            ..container::Style::default()
        }
    }
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
