//! Settings → Appearance. Behaviour reference: web-rs/src/pages/settings.rs `paint_appearance` and
//! web-rs/src/appearance.rs. Presets, theme, accent colour (with any hex colour), background and
//! the Customize the bar button. Saved in the app's config, and on the account where the server
//! keeps preferences (`theme` feature).

use super::{block, help};
use crate::config::Appearance;
use crate::pages::{Ctx, El};
use crate::player;
use crate::theme::{self, Badge, Btn, ACCENTS, PRESETS, THEMES};
use crate::widgets::{self, badge, btn_icon, seg, t, tip_box, Icon};
use iced::widget::{button, column, container, row, space, tooltip};
use iced::{Alignment, Background, Border, Color, Length, Task};
use serde_json::json;

#[derive(Debug, Default)]
pub struct State {
    /// The hex box's text (the saved custom colour until typed over).
    hex: String,
    hex_error: String,
}

#[derive(Clone, Debug)]
pub enum Message {
    Preset(&'static str),
    Theme(String),
    Accent(&'static str),
    HexInput(String),
    HexSubmit,
    Background(String),
    Customize,
    /// The account has the look too (or the server said no: this device keeps it either way).
    Saved,
}

pub fn open(state: &mut State, ctx: &mut Ctx) -> Task<Message> {
    state.hex = ctx.config.appearance.custom.clone();
    state.hex_error.clear();
    Task::none()
}

/// The web's `appearance.save`: this device and, where the server keeps preferences, the account.
fn save(a: Appearance, ctx: &mut Ctx) -> Task<Message> {
    let copy = a.clone();
    ctx.edit_config(move |c| c.appearance = copy);
    if !ctx.has_feature("theme") {
        return Task::none();
    }
    let api = ctx.api.clone();
    let body = json!({ "appearance": { "theme": a.theme, "accent": a.accent, "custom": a.custom, "background": a.background } });
    Task::perform(
        async move {
            let _ = api.put("/auth/prefs", body).await;
        },
        |_| Message::Saved,
    )
}

/// `#abc` or `#aabbcc` (case does not matter) as `#aabbcc`; None for anything else.
pub fn normalize_hex(raw: &str) -> Option<String> {
    let v = raw.trim();
    let digits = v.strip_prefix('#').unwrap_or(v);
    if !(digits.len() == 3 || digits.len() == 6) || !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let full: String = if digits.len() == 3 { digits.chars().flat_map(|c| [c, c]).collect() } else { digits.to_string() };
    Some(format!("#{}", full.to_lowercase()))
}

pub fn update(state: &mut State, msg: Message, ctx: &mut Ctx) -> Task<Message> {
    let mut a = ctx.config.appearance.clone();
    match msg {
        Message::Preset(id) => {
            let Some(x) = PRESETS.iter().find(|x| x.0 == id) else { return Task::none() };
            a.theme = x.2.into();
            a.accent = x.3.into();
            if !x.4.is_empty() {
                a.custom = x.4.into();
                state.hex = a.custom.clone();
            }
            a.background = x.5.into();
            save(a, ctx)
        }
        Message::Theme(theme) => {
            a.theme = theme;
            save(a, ctx)
        }
        Message::Accent(id) => {
            a.accent = id.into();
            save(a, ctx)
        }
        Message::HexInput(s) => {
            state.hex = s;
            state.hex_error.clear();
            Task::none()
        }
        Message::HexSubmit => match normalize_hex(&state.hex) {
            Some(hex) => {
                state.hex = hex.clone();
                a.accent = "custom".into();
                a.custom = hex;
                save(a, ctx)
            }
            None => {
                ctx.toast_err("Use a hex colour like #ff7a00");
                Task::none()
            }
        },
        Message::Background(bg) => {
            a.background = bg;
            save(a, ctx)
        }
        Message::Customize => {
            ctx.player_msg(player::Message::Customize(true));
            Task::none()
        }
        Message::Saved => Task::none(),
    }
}

/// A 26px round swatch in the gradient `a → b`; `on` draws a ring around it.
fn swatch<'a>(a: Color, b: Color, on: bool, size: f32, tip: &'a str, msg: Message) -> El<'a, Message> {
    let p = theme::look();
    let w = button(space().width(size).height(size))
        .padding(0)
        .style(move |_, status| button::Style {
            background: Some(Background::Gradient(iced::Gradient::Linear(
                iced::gradient::Linear::new(iced::Radians(2.356)).add_stop(0.0, a).add_stop(1.0, b),
            ))),
            text_color: p.text,
            border: Border {
                color: if on {
                    p.text
                } else if matches!(status, button::Status::Hovered) {
                    p.muted
                } else {
                    Color::TRANSPARENT
                },
                width: 2.0,
                radius: 99.0.into(),
            },
            shadow: Default::default(),
            snap: true,
        })
        .on_press(msg);
    tooltip(w, tip_box(tip), tooltip::Position::Bottom).gap(4).into()
}

/// A preset's little picture: the accent, its partner and the theme's page colour.
fn preset_art<'a>(theme_id: &str, accent_id: &str, custom: &str) -> El<'a, Message> {
    let bg = match theme_id {
        "encore" => "#161617",
        "oled" => "#000000",
        "light" => "#f6f6fb",
        _ => "#0b0b12",
    };
    let (a, b) = match ACCENTS.iter().find(|x| x.0 == accent_id) {
        Some(x) => (theme::hex(x.2), theme::hex(x.3)),
        None => {
            let c = theme::hex(custom);
            (c, c)
        }
    };
    let strip = |c: Color, w: f32| {
        container(space().width(w).height(28)).style(move |_| container::Style {
            background: Some(Background::Color(c)),
            ..container::Style::default()
        })
    };
    container(row![strip(a, 10.0), strip(b, 6.0), strip(theme::hex(bg), 28.0)])
        .style(|_| container::Style {
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: 7.0.into(),
            },
            ..container::Style::default()
        })
        .clip(true)
        .into()
}

pub fn view<'a>(state: &'a State, ctx: &Ctx<'a>) -> El<'a, Message> {
    let a = &ctx.config.appearance;
    let saved = if ctx.has_feature("theme") {
        "Saved to your account, so it follows you to every device."
    } else {
        "Saved on this device."
    };

    // presets
    let mut presets = row![].spacing(10).align_y(Alignment::Center);
    for (id, name, theme_id, accent, custom, background) in PRESETS {
        let on = a.theme == theme_id && a.accent == accent && a.background == background && (accent != "custom" || a.custom == custom);
        let label = row![preset_art(theme_id, accent, custom), t(name).size(13).font(theme::semibold())]
            .spacing(10)
            .align_y(Alignment::Center);
        presets = presets.push(
            button(label)
                .padding([6, 10])
                .style(theme::btn(if on { Btn::NavOn } else { Btn::Default }))
                .on_press(Message::Preset(id)),
        );
    }

    // accents
    let mut swatches = row![].spacing(10).align_y(Alignment::Center);
    for (id, name, ca, cb) in ACCENTS {
        swatches = swatches.push(swatch(theme::hex(ca), theme::hex(cb), a.accent == *id, 28.0, name, Message::Accent(id)));
    }
    let custom_colour = theme::hex(if a.custom.is_empty() { "#f0ead6" } else { &a.custom });
    let mut custom = row![
        help("Any colour:"),
        swatch(custom_colour, theme::partner(custom_colour), a.accent == "custom", 28.0, "Pick any colour", Message::HexSubmit),
        super::input("#ff7a00", &state.hex, Message::HexInput).on_submit(Message::HexSubmit).width(120),
        widgets::btn_sm("Use", Btn::Default, Some(Message::HexSubmit)),
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    if a.accent == "custom" {
        custom = custom.push(badge("in use", Badge::Accent));
    }

    let themes: Vec<(&str, &str)> = THEMES.iter().map(|(id, name)| (*id, if *id == "system" { "Match device" } else { name })).collect();
    let example = row![
        btn_icon(Icon::Check, "Example button", Btn::Primary, Some(Message::Customize)),
        badge("Example badge", Badge::Accent),
    ]
    .spacing(12)
    .align_y(Alignment::Center);

    let card = widgets::card(
        column![
            widgets::h2("Appearance"),
            help(saved),
            block("Presets", vec![presets.wrap().into()]),
            block("Theme", vec![seg(&themes, &a.theme, Message::Theme)]),
            block("Accent colour", vec![swatches.wrap().into(), custom.wrap().into()]),
            block(
                "Background",
                vec![seg(&[("fade", "Faded colour"), ("solid", "Solid")], &a.background, Message::Background)]
            ),
            example,
            block(
                "Player bar",
                vec![
                    help("Pick which buttons sit on the bar at the bottom (Like, View album, Add to playlist, Lyrics, Share, Up next, Other devices, the quality badge). Saved on this device.").into(),
                    container(btn_icon(Icon::Settings, "Customize the bar…", Btn::Default, Some(Message::Customize))).into(),
                ]
            ),
        ]
        .spacing(16)
        .width(Length::Fill),
    )
    .width(Length::Fill);
    container(card).max_width(820).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_colours_are_checked_like_the_web() {
        assert_eq!(normalize_hex("#FF7A00").as_deref(), Some("#ff7a00"));
        assert_eq!(normalize_hex("abc").as_deref(), Some("#aabbcc"));
        assert_eq!(normalize_hex(" #123456 ").as_deref(), Some("#123456"));
        assert!(normalize_hex("#12345").is_none());
        assert!(normalize_hex("red").is_none());
    }
}
