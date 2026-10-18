//! A song in a list: number or cover, title and artist, badges, duration, the ⋯ button;
//! the playing one in the accent colour. Right-click opens the same menu as ⋯. Package A
//! (home + library) owns this file in stage 2.

use super::{badge, icon, small_muted, t, El, Icon};
use crate::theme::{self, Badge, Btn};
use iced::widget::{button, column, container, image, mouse_area, row, space};
use iced::{Alignment, ContentFit, Length};

pub struct TrackRow<'a, M> {
    index: Option<String>,
    title: String,
    sub: String,
    /// Faint small lines under the title (the admin's file path, the source line).
    notes: Vec<String>,
    cover: Option<image::Handle>,
    duration: Option<f64>,
    quality: String,
    badges: Vec<El<'a, M>>,
    /// Anything else on the right, before the heart and the ⋯ (a button).
    extras: Vec<El<'a, M>>,
    playing: bool,
    liked: Option<(bool, M)>,
    on_play: Option<M>,
    on_menu: Option<M>,
}

impl<'a, M: Clone + 'a> TrackRow<'a, M> {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            index: None,
            title: title.into(),
            sub: String::new(),
            notes: Vec::new(),
            cover: None,
            duration: None,
            quality: String::new(),
            badges: Vec::new(),
            extras: Vec::new(),
            playing: false,
            liked: None,
            on_play: None,
            on_menu: None,
        }
    }

    /// The track number shown in front (album pages).
    pub fn index(mut self, i: usize) -> Self {
        self.index = Some(i.to_string());
        self
    }

    /// Any text in the number column (`2-7` for disc 2, track 7).
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.index = Some(label.into());
        self
    }

    /// `artist · album`.
    pub fn sub(mut self, sub: impl Into<String>) -> Self {
        self.sub = sub.into();
        self
    }

    /// A faint line under the title and the sub line (skipped when empty).
    pub fn note(mut self, note: impl Into<String>) -> Self {
        let n = note.into();
        if !n.is_empty() {
            self.notes.push(n);
        }
        self
    }

    /// A small cover in front (song lists across albums).
    pub fn cover(mut self, h: Option<image::Handle>) -> Self {
        self.cover = h;
        self
    }

    pub fn duration(mut self, seconds: Option<f64>) -> Self {
        self.duration = seconds;
        self
    }

    /// `FLAC`, `MP3 320` (see [`crate::fmt::quality`]).
    pub fn quality(mut self, q: impl Into<String>) -> Self {
        self.quality = q.into();
        self
    }

    pub fn badge(mut self, el: El<'a, M>) -> Self {
        self.badges.push(el);
        self
    }

    /// `Some(badge)` adds it, `None` adds nothing.
    pub fn badge_maybe(mut self, el: Option<El<'a, M>>) -> Self {
        if let Some(e) = el {
            self.badges.push(e);
        }
        self
    }

    /// Something on the right before the heart and the ⋯ (a small button).
    pub fn extra(mut self, el: El<'a, M>) -> Self {
        self.extras.push(el);
        self
    }

    pub fn playing(mut self, on: bool) -> Self {
        self.playing = on;
        self
    }

    /// Shows the heart; `on` toggles it.
    pub fn liked(mut self, liked: bool, on: M) -> Self {
        self.liked = Some((liked, on));
        self
    }

    /// Pressing the row (or its play button) plays it.
    pub fn on_play(mut self, m: M) -> Self {
        self.on_play = Some(m);
        self
    }

    /// The ⋯ button and the right-click.
    pub fn on_menu(mut self, m: M) -> Self {
        self.on_menu = Some(m);
        self
    }

    pub fn view(self) -> El<'a, M> {
        let p = theme::look();
        let color = if self.playing { p.accent } else { p.text };
        let mut r = row![].spacing(14).align_y(Alignment::Center);
        if let Some(h) = self.cover {
            r = r.push(
                image(h)
                    .width(40)
                    .height(40)
                    .content_fit(ContentFit::Cover)
                    .border_radius(6.0),
            );
        } else if let Some(i) = self.index {
            r = r.push(
                container(if self.playing {
                    icon(Icon::Play, 14.0, p.accent)
                } else {
                    small_muted(i).into()
                })
                .width(30)
                .align_x(Alignment::Center),
            );
        }
        let mut lines = column![t(self.title).size(14).font(theme::semibold()).color(color)].spacing(2);
        if !self.sub.is_empty() {
            lines = lines.push(small_muted(self.sub).size(12.5));
        }
        for n in self.notes {
            lines = lines.push(super::faint(n).size(11.5));
        }
        r = r.push(lines.width(Length::Fill));
        for b in self.badges {
            r = r.push(b);
        }
        if !self.quality.is_empty() {
            r = r.push(badge(self.quality, Badge::Plain));
        }
        for e in self.extras {
            r = r.push(e);
        }
        if let Some(d) = self.duration {
            r = r.push(small_muted(crate::fmt::mmss(d)).width(40).align_x(iced::alignment::Horizontal::Right));
        }
        if let Some((liked, on)) = self.liked {
            r = r.push(
                iced::widget::tooltip(
                    button(icon(Icon::Heart, 16.0, if liked { p.accent } else { p.muted }))
                        .padding(6)
                        .style(theme::btn(Btn::Icon))
                        .on_press(on),
                    super::tip_box("Like (syncs to Finamp/Plexamp)"),
                    iced::widget::tooltip::Position::Top,
                )
                .gap(4),
            );
        }
        if let Some(m) = &self.on_menu {
            r = r.push(
                button(icon(Icon::More, 16.0, p.muted))
                    .padding(6)
                    .style(theme::btn(Btn::Icon))
                    .on_press(m.clone()),
            );
        } else {
            r = r.push(space().width(28));
        }
        let inner = button(container(r).padding([9, 14]))
            .padding(0)
            .width(Length::Fill)
            .style(theme::btn(Btn::Row))
            .on_press_maybe(self.on_play);
        let el: El<'a, M> = match self.on_menu {
            Some(m) => mouse_area(inner).on_right_press(m).into(),
            None => inner.into(),
        };
        column![el, super::divider()].into()
    }
}
