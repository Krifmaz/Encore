//! Lyrics: reading .lrc stamps, loading a song's lyrics (`/api/library/tracks/{id}/lyrics`)
//! and the line that follows the music, as web-rs/src/player/lyrics.rs does.

use crate::api::{Api, ApiError};
use crate::widgets::{self, t, El};
use crate::theme;
use iced::widget::{column, container, scrollable};
use iced::{Alignment, Length, Task};
use serde_json::Value;
use std::sync::Arc;

/// The id of the lyrics scroll area.
pub const SCROLL_ID: &str = "player-lyrics";

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

    /// Where the scroll area should sit so the current line is in the middle (0…1).
    pub fn scroll_offset(&self) -> f32 {
        if self.lines.len() < 2 || self.current < 0 {
            return 0.0;
        }
        (self.current as f32 / (self.lines.len() - 1) as f32).clamp(0.0, 1.0)
    }

    /// The pane: the synced lines (the current one lit), the plain text, or a line saying why
    /// there is nothing. `song` is the current song's library id (None for files).
    pub fn view<'a, M: Clone + 'a>(&'a self, song: Option<i64>, on_seek: impl Fn(f64) -> M + 'a) -> El<'a, M> {
        let p = theme::look();
        let note = |s: &'a str| -> El<'a, M> {
            container(widgets::muted(s).size(14))
                .padding([24, 8])
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
            let mut col = column![].spacing(10).width(Length::Fill).align_x(Alignment::Center);
            for (i, ln) in self.lines.iter().enumerate() {
                let on = i as i64 == self.current;
                let text = if ln.text.is_empty() { "♪" } else { ln.text.as_str() };
                let label = t(text)
                    .size(if on { 22 } else { 18 })
                    .font(if on { theme::bold() } else { iced::Font::DEFAULT })
                    .color(if on {
                        p.text
                    } else if (i as i64) < self.current {
                        p.faint
                    } else {
                        p.muted
                    })
                    .center();
                col = col.push(
                    iced::widget::button(label)
                        .padding([4, 8])
                        .style(theme::btn(theme::Btn::Row))
                        .on_press(on_seek(ln.t)),
                );
            }
            return scrollable(container(col).padding([16, 8]).width(Length::Fill))
                .id(SCROLL_ID)
                .height(Length::Fill)
                .style(theme::scroll)
                .into();
        }
        if !self.plain.is_empty() {
            return scrollable(container(t(self.plain.as_str()).size(17).line_height(1.6)).padding([16, 12]))
                .id(SCROLL_ID)
                .height(Length::Fill)
                .style(theme::scroll)
                .into();
        }
        note(if self.instrumental { "Instrumental" } else { "No lyrics found for this song." })
    }
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
        assert!((l.scroll_offset() - 1.0 / 3.0).abs() < 1e-6);
        assert!(parse_stamp("ab:12").is_none());
        assert_eq!(parse_stamp("2:03.5"), Some(123.5));
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
