//! Lyrics: reading .lrc stamps, loading a song's lyrics (`/api/library/tracks/{id}/lyrics`)
//! and the line that follows the music, as web-rs/src/player/lyrics.rs does.

use crate::api::{Api, ApiError};
use crate::widgets::{self, t, El};
use crate::theme;
use iced::widget::text::Wrapping;
use iced::widget::{column, container, scrollable, space};
use iced::{Alignment, Length, Task};
use serde_json::Value;
use std::sync::Arc;

/// The id of the lyrics scroll area.
pub const SCROLL_ID: &str = "player-lyrics";
/// The id of the current line's box (the one kept in the middle).
pub const LINE_ID: &str = "player-lyrics-now";
/// Lyrics mode's sizes on the desktop: the current line, the others (the phone's 28 / 22 sp).
pub const CURRENT_PX: f32 = 30.0;
pub const LINE_PX: f32 = 24.0;

/// One synced line.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub t: f64,
    pub text: String,
}

/// What is known about the current song's lyrics.
#[derive(Clone, Debug, Default)]
pub struct Lyrics {
    /// The song they belong to (None = nothing loaded).
    pub id: Option<i64>,
    pub lines: Vec<Line>,
    pub plain: String,
    pub loading: bool,
    pub instrumental: bool,
    /// The line the music is at (−1 before the first).
    pub current: i64,
}

impl Lyrics {
    /// Start loading `id`'s lyrics (nothing when they are already here).
    pub fn load<M: Send + 'static>(
        &mut self,
        api: Arc<Api>,
        id: i64,
        done: impl Fn(i64, Result<Value, ApiError>) -> M + Send + 'static,
    ) -> Task<M> {
        if self.id == Some(id) {
            return Task::none();
        }
        *self = Lyrics {
            id: Some(id),
            loading: true,
            current: -1,
            ..Default::default()
        };
        Task::perform(
            async move { api.get(&format!("/library/tracks/{id}/lyrics")).await },
            move |r| done(id, r),
        )
    }

    /// Store the server's answer (ignored when the song changed meanwhile).
    pub fn loaded(&mut self, id: i64, r: Result<Value, ApiError>) {
        if self.id != Some(id) {
            return;
        }
        self.loading = false;
        match r {
            Ok(v) => {
                let synced = crate::fmt::truthy(&v, "synced");
                let text = crate::fmt::s(&v, "lyrics");
                self.lines = if synced { parse_lrc(&text) } else { Vec::new() };
                self.plain = if synced { String::new() } else { text };
                self.instrumental = crate::fmt::truthy(&v, "instrumental");
            }
            Err(_) => {
                self.lines.clear();
                self.plain.clear();
                self.instrumental = false;
            }
        }
        self.current = -1;
    }

    /// Lyrics (or LRCLIB's "Instrumental") are on screen for library song `song`: they can be reported.
    pub fn shown(&self, song: Option<i64>) -> bool {
        song.is_some() && self.id == song && !self.loading && (!self.lines.is_empty() || !self.plain.is_empty() || self.instrumental)
    }

    /// Nothing loaded (the song changed).
    pub fn clear(&mut self) {
        *self = Lyrics::default();
    }

    /// The line at `position` (the web looks 0.3 s ahead). Returns true when it changed.
    pub fn sync(&mut self, position: f64) -> bool {
        if self.lines.is_empty() {
            return false;
        }
        let i = line_at(&self.lines, position + 0.3);
        if i == self.current {
            return false;
        }
        self.current = i;
        true
    }

    /// Lyrics mode's column (p6 item 4, client-shapes §5.2): the synced lines at 24 px with the
    /// current one at 30 px and bold, the ones already sung dimmed, a click on a line seeks there;
    /// the plain text; or a line saying why there is nothing. `pad` is the room above the first
    /// and below the last line, so those can sit in the middle too. The current line carries
    /// [`LINE_ID`]: the player scrolls it into the middle (`player::lyrics_scroll`). `song` is the
    /// current song's library id (None for files).
    pub fn view<'a, M: Clone + 'a>(&'a self, song: Option<i64>, pad: f32, on_seek: impl Fn(f64) -> M + 'a) -> El<'a, M> {
        let p = theme::look();
        let note = |s: &'a str| -> El<'a, M> {
            container(widgets::muted(s).size(16))
                .padding([32, 8])
                .width(Length::Fill)
                .center_x(Length::Fill)
                .into()
        };
        if song.is_none() {
            return note("No lyrics for this song.");
        }
        if self.loading || self.id != song {
            return note("Looking for lyrics…");
        }
        if !self.lines.is_empty() {
            let mut col = column![space().height(pad)].spacing(14).width(Length::Fill).align_x(Alignment::Center);
            for (i, ln) in self.lines.iter().enumerate() {
                let on = i as i64 == self.current;
                let text = if ln.text.is_empty() { "♪" } else { ln.text.as_str() };
                let label = t(text)
                    .size(if on { CURRENT_PX } else { LINE_PX })
                    .font(if on { theme::bold() } else { iced::Font::DEFAULT })
                    .line_height(1.3)
                    .wrapping(Wrapping::WordOrGlyph)
                    .color(if on {
                        p.text
                    } else if (i as i64) < self.current {
                        p.faint
                    } else {
                        p.muted
                    })
                    .width(Length::Fill)
                    .center();
                let line = iced::widget::button(label)
                    .padding([4, 12])
                    .width(Length::Fill)
                    .style(theme::btn(theme::Btn::Row))
                    .on_press(on_seek(ln.t));
                col = col.push(if on { El::from(container(line).id(LINE_ID)) } else { El::from(line) });
            }
            col = col.push(space().height(pad));
            // the lines move by themselves: no scrollbar (the wheel still scrolls)
            return scrollable(container(col).width(Length::Fill))
                .id(SCROLL_ID)
                .direction(scrollable::Direction::Vertical(scrollable::Scrollbar::hidden()))
                .height(Length::Fill)
                .style(theme::scroll)
                .into();
        }
        if !self.plain.is_empty() {
            return scrollable(
                container(
                    t(self.plain.as_str())
                        .size(LINE_PX)
                        .line_height(1.5)
                        .wrapping(Wrapping::WordOrGlyph)
                        .width(Length::Fill)
                        .center(),
                )
                .padding([24, 12]),
            )
            .id(SCROLL_ID)
            .height(Length::Fill)
            .style(theme::scroll)
            .into();
        }
        note(if self.instrumental { "Instrumental" } else { "No lyrics found for this song." })
    }
}

/// Where the lyrics scroll area is and where it should go so the current line sits in the
/// middle: `(now, wanted)`, in pixels from the top of its content. Nothing when the lyrics
/// column is not on screen.
pub fn centre_task() -> Task<(f32, f32)> {
    iced::advanced::widget::operate(Centre::default())
}

/// Finds the scroll area, its offset and the current line's box.
#[derive(Default)]
struct Centre {
    /// (the content's top, the visible height, the offset now)
    area: Option<(f32, f32, f32)>,
    /// (the line's top, its height)
    line: Option<(f32, f32)>,
}

impl iced::advanced::widget::Operation<(f32, f32)> for Centre {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn iced::advanced::widget::Operation<(f32, f32)>)) {
        operate(self);
    }

    fn container(&mut self, id: Option<&iced::advanced::widget::Id>, bounds: iced::Rectangle) {
        if id == Some(&iced::advanced::widget::Id::new(LINE_ID)) {
            self.line = Some((bounds.y, bounds.height));
        }
    }

    fn scrollable(
        &mut self,
        id: Option<&iced::advanced::widget::Id>,
        bounds: iced::Rectangle,
        content_bounds: iced::Rectangle,
        translation: iced::Vector,
        _state: &mut dyn iced::advanced::widget::operation::Scrollable,
    ) {
        if id == Some(&iced::advanced::widget::Id::new(SCROLL_ID)) {
            self.area = Some((content_bounds.y, bounds.height, translation.y));
        }
    }

    fn finish(&self) -> iced::advanced::widget::operation::Outcome<(f32, f32)> {
        match (self.area, self.line) {
            (Some((top, height, now)), Some((y, h))) => {
                iced::advanced::widget::operation::Outcome::Some((now, centred_offset(y - top, h, height)))
            }
            _ => iced::advanced::widget::operation::Outcome::None,
        }
    }
}

/// The offset that puts a line (`line_y` from the top of the content, `line_h` high) in the
/// middle of a scroll area `visible` high.
pub fn centred_offset(line_y: f32, line_h: f32, visible: f32) -> f32 {
    (line_y + line_h / 2.0 - visible / 2.0).max(0.0)
}

/// The eased offset `elapsed` into a scroll from `from` to `to` that takes `total`.
pub fn eased(from: f32, to: f32, elapsed: std::time::Duration, total: std::time::Duration) -> f32 {
    let x = (elapsed.as_secs_f32() / total.as_secs_f32().max(1e-3)).clamp(0.0, 1.0);
    // ease out (cubic)
    let k = 1.0 - (1.0 - x).powi(3);
    from + (to - from) * k
}

/// The index of the last line at or before `p` (−1 before the first).
pub fn line_at(lines: &[Line], p: f64) -> i64 {
    let mut i: i64 = -1;
    while ((i + 1) as usize) < lines.len() && lines[(i + 1) as usize].t <= p {
        i += 1;
    }
    i
}

/// One `mm:ss.xx` stamp (the content of a bracket).
fn parse_stamp(s: &str) -> Option<f64> {
    let (m, rest) = s.split_once(':')?;
    let digits = |x: &str| !x.is_empty() && x.bytes().all(|c| c.is_ascii_digit());
    if !digits(m) {
        return None;
    }
    match rest.split_once('.') {
        Some((sec, frac)) => {
            if !digits(sec) || !digits(frac) {
                return None;
            }
        }
        None => {
            if !digits(rest) {
                return None;
            }
        }
    }
    let minutes: f64 = m.parse().ok()?;
    let seconds: f64 = rest.parse().ok()?;
    Some(minutes * 60.0 + seconds)
}

/// Every `[mm:ss.xx]` stamp of a line.
fn lrc_stamps(line: &str) -> Vec<f64> {
    let b = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'[' {
            if let Some(j) = b[i + 1..].iter().position(|&c| c == b']') {
                if let Some(t) = parse_stamp(&line[i + 1..i + 1 + j]) {
                    out.push(t);
                    i = i + 1 + j + 1;
                    continue;
                }
            }
        }
        i += 1;
    }
    out
}

/// The line without its `[...]` tags.
fn strip_brackets(line: &str) -> String {
    let mut out = String::new();
    let mut rest = line;
    loop {
        let Some(i) = rest.find('[') else {
            out.push_str(rest);
            break;
        };
        match rest[i + 1..].find(']') {
            Some(j) => {
                out.push_str(&rest[..i]);
                rest = &rest[i + 1 + j + 1..];
            }
            None => {
                out.push_str(rest);
                break;
            }
        }
    }
    out
}

/// An .lrc text as lines sorted by time (a line with several stamps appears once per stamp).
pub fn parse_lrc(text: &str) -> Vec<Line> {
    let mut out = Vec::new();
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let stamps = lrc_stamps(line);
        let words = strip_brackets(line).trim().to_string();
        for t in stamps {
            out.push(Line {
                t,
                text: words.clone(),
            });
        }
    }
    out.sort_by(|a, b| a.t.partial_cmp(&b.t).unwrap_or(std::cmp::Ordering::Equal));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lrc_lines_are_parsed_and_followed() {
        let lines = parse_lrc("[ti:Song]\n[00:12.50]First line\n[00:05]Early [00:30.1]and late\n\nno stamp\n[01:02.00]\r\n");
        let got: Vec<(f64, &str)> = lines.iter().map(|l| (l.t, l.text.as_str())).collect();
        assert_eq!(got, [(5.0, "Early and late"), (12.5, "First line"), (30.1, "Early and late"), (62.0, "")]);
        assert_eq!(line_at(&lines, 0.0), -1);
        assert_eq!(line_at(&lines, 12.6), 1);
        assert_eq!(line_at(&lines, 100.0), 3);
        let mut l = Lyrics {
            id: Some(1),
            lines,
            ..Default::default()
        };
        assert!(l.sync(12.3), "0.3 s ahead lights the line");
        assert_eq!(l.current, 1);
        assert!(!l.sync(12.4));
        assert!(parse_stamp("ab:12").is_none());
        assert_eq!(parse_stamp("2:03.5"), Some(123.5));
    }

    #[test]
    fn the_current_line_is_scrolled_into_the_middle() {
        // a line 300 px down the content, 40 px high, in an area 500 px high: 70 px of scroll
        assert_eq!(centred_offset(300.0, 40.0, 500.0), 70.0);
        // near the top the area stays at the top
        assert_eq!(centred_offset(10.0, 40.0, 500.0), 0.0);
        let total = std::time::Duration::from_millis(400);
        assert_eq!(eased(0.0, 100.0, std::time::Duration::ZERO, total), 0.0);
        assert_eq!(eased(0.0, 100.0, total, total), 100.0);
        let mid = eased(0.0, 100.0, std::time::Duration::from_millis(200), total);
        assert!(mid > 50.0 && mid < 100.0, "it slows down towards the end: {mid}");
    }

    #[test]
    /// "Report lyrics" shows only while the song's lyrics are on screen (not while loading, not for nothing found).
    fn only_lyrics_on_screen_can_be_reported() {
        let mut l = Lyrics::default();
        assert!(!l.shown(Some(7)));
        l.id = Some(7);
        l.loading = true;
        assert!(!l.shown(Some(7)));
        l.loaded(7, Ok(serde_json::json!({ "lyrics": "plain words", "synced": false })));
        assert!(l.shown(Some(7)) && !l.shown(Some(8)) && !l.shown(None));
        l.loaded(7, Ok(serde_json::json!({ "lyrics": null })));
        assert!(!l.shown(Some(7)));
    }

    #[test]
    fn answers_are_kept_only_for_the_song_asked() {
        let mut l = Lyrics {
            id: Some(7),
            loading: true,
            ..Default::default()
        };
        l.loaded(8, Ok(serde_json::json!({ "lyrics": "x", "synced": false })));
        assert!(l.loading, "another song's answer is ignored");
        l.loaded(7, Ok(serde_json::json!({ "lyrics": "plain words", "synced": false })));
        assert!(!l.loading && l.plain == "plain words" && l.lines.is_empty());
        l.loaded(7, Ok(serde_json::json!({ "lyrics": null, "instrumental": true })));
        assert!(l.instrumental && l.plain.is_empty());
    }
}
