//! The number tiles and thin meters of the Storage and Leaderboard pages (`.stats .stat`,
//! `.progress` with a coloured part) and the number words those pages share with the web
//! (`fmt.num`, `fmt.long_dur`).

use super::{small_muted, t, El};
use crate::theme;
use iced::widget::{column, container, row, space};
use iced::{Alignment, Background, Border, Color, Length};

/// `.stat`: a big value over a small label.
pub fn stat<'a, M: 'a>(value: impl Into<String>, label: impl Into<String>) -> El<'a, M> {
    container(
        column![
            t(value.into()).size(22).font(theme::bold()),
            small_muted(label.into()).size(12.5)
        ]
        .spacing(4),
    )
    .padding([14, 18])
    .width(Length::Fixed(190.0))
    .style(theme::card)
    .into()
}

/// `.stats`: the tiles in a wrapping row.
pub fn stats<'a, M: 'a>(tiles: Vec<El<'a, M>>) -> El<'a, M> {
    row(tiles).spacing(12).wrap().into()
}

/// `bar(part, whole, color)`: a thin meter filled to `part / whole` in `color`.
pub fn bar<'a, M: 'a>(part: f64, whole: f64, color: Color) -> El<'a, M> {
    let p = theme::look();
    let pct = fraction(part, whole);
    let filled = (pct * 1000.0).round() as u16;
    let rest = 1000u16.saturating_sub(filled);
    let track = move |_: &iced::Theme| container::Style {
        background: Some(Background::Color(p.border)),
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: 99.0.into(),
        },
        ..container::Style::default()
    };
    let fill = move |_: &iced::Theme| container::Style {
        background: Some(Background::Color(color)),
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: 99.0.into(),
        },
        ..container::Style::default()
    };
    let mut r = row![].height(8);
    if filled > 0 {
        r = r.push(container(space()).width(Length::FillPortion(filled)).height(8).style(fill));
    }
    if rest > 0 {
        r = r.push(space().width(Length::FillPortion(rest)));
    }
    container(r).width(Length::Fill).height(8).style(track).into()
}

/// `part / whole` clamped to 0..=1; 0 when `whole` is 0 or not a number.
pub fn fraction(part: f64, whole: f64) -> f64 {
    if whole <= 0.0 || whole.is_nan() || part.is_nan() {
        return 0.0;
    }
    (part / whole).clamp(0.0, 1.0)
}

/// A row with words on the left and a bold value on the right (`.row.between`).
pub fn between<'a, M: 'a>(left: El<'a, M>, right: El<'a, M>) -> El<'a, M> {
    row![left, space().width(Length::Fill), right]
        .spacing(10)
        .align_y(Alignment::Center)
        .width(Length::Fill)
        .into()
}

/// `fmt.num(n)`: `(n || 0).toLocaleString()` with English thousands separators (`12,345`).
pub fn num(n: f64) -> String {
    let n = if n.is_nan() { 0.0 } else { n };
    let whole = n.trunc().abs() as u64;
    let mut s = whole.to_string();
    let mut out = String::new();
    while s.len() > 3 {
        let tail = s.split_off(s.len() - 3);
        out = if out.is_empty() { tail } else { format!("{tail},{out}") };
    }
    out = if out.is_empty() { s } else { format!("{s},{out}") };
    let frac = n.fract().abs();
    if frac > 0.0 {
        let f = format!("{frac:.1}");
        out.push_str(f.trim_start_matches('0'));
    }
    if n < 0.0 {
        out.insert(0, '-');
    }
    out
}

/// `fmt.long_dur(seconds)`: `2 days 3 hr 5 min`, `48 min`.
pub fn long_dur(seconds: f64) -> String {
    let mut m = (seconds.max(0.0) / 60.0).round() as i64;
    let mut parts: Vec<String> = Vec::new();
    for (n, one, many) in [(525_600, "yr", "yr"), (43_200, "mo", "mo"), (1440, "day", "days"), (60, "hr", "hr")] {
        let v = m / n;
        if v > 0 {
            parts.push(format!("{v} {}", if v == 1 { one } else { many }));
            m -= v * n;
        }
    }
    if m > 0 || parts.is_empty() {
        parts.push(format!("{m} min"));
    }
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_read_like_the_web() {
        assert_eq!(num(0.0), "0");
        assert_eq!(num(999.0), "999");
        assert_eq!(num(1000.0), "1,000");
        assert_eq!(num(1_234_567.0), "1,234,567");
        assert_eq!(num(2.5), "2.5");
        assert_eq!(num(-1500.0), "-1,500");
        assert_eq!(long_dur(0.0), "0 min");
        assert_eq!(long_dur(2_880.0), "48 min");
        assert_eq!(long_dur(7_500.0), "2 hr 5 min");
        assert_eq!(long_dur(90_000.0), "1 day 1 hr");
        assert_eq!(long_dur(3.0 * 86_400.0), "3 days");
    }

    #[test]
    fn fractions_are_safe() {
        assert_eq!(fraction(50.0, 100.0), 0.5);
        assert_eq!(fraction(5.0, 0.0), 0.0);
        assert_eq!(fraction(500.0, 100.0), 1.0);
        assert_eq!(fraction(f64::NAN, 1.0), 0.0);
    }
}
