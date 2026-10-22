//! `.tile`: an album, artist or playlist card – the cover, a title, a line under it and a
//! play button on hover. Tiles go in a wrapping [`grid`]. Package A (home + library) owns
//! this file in stage 2.

use super::{icon, small_muted, t, El, Icon};
use crate::theme::{self, Btn};
use iced::widget::{button, column, container, hover, image, row, space, stack, tooltip};
use iced::{Alignment, ContentFit, Length};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TileKind {
    Album,
    /// Round picture, centred text.
    Artist,
    Playlist,
}

/// Build with [`Tile::new`] and the setters, then [`Tile::view`].
pub struct Tile<'a, M> {
    title: String,
    sub: String,
    cover: Option<image::Handle>,
    /// A picture built by the page (a collage) instead of one cover.
    art: Option<El<'a, M>>,
    kind: TileKind,
    on_open: M,
    on_play: Option<M>,
    /// An extra hover button (an icon, its tooltip and the message).
    action: Option<(Icon, &'a str, M)>,
    width: f32,
    corner: Option<El<'a, M>>,
    center: bool,
}

/// The default tile width (the web's grid cell at a laptop size).
pub const WIDTH: f32 = 168.0;

impl<'a, M: Clone + 'a> Tile<'a, M> {
    pub fn new(title: impl Into<String>, kind: TileKind, on_open: M) -> Self {
        Self {
            title: title.into(),
            sub: String::new(),
            cover: None,
            art: None,
            kind,
            on_open,
            on_play: None,
            action: None,
            width: WIDTH,
            corner: None,
            center: kind == TileKind::Artist,
        }
    }

    /// The line under the title (artist, year, song count).
    pub fn sub(mut self, sub: impl Into<String>) -> Self {
        self.sub = sub.into();
        self
    }

    pub fn cover(mut self, cover: Option<image::Handle>) -> Self {
        self.cover = cover;
        self
    }

    /// A picture of the page's own making (a collage of covers) in place of the cover.
    pub fn art(mut self, art: El<'a, M>) -> Self {
        self.art = Some(art);
        self
    }

    /// Shows the play button on hover.
    pub fn play(mut self, on: M) -> Self {
        self.on_play = Some(on);
        self
    }

    /// Another button on hover, next to the play button (the admin's "Remove this artist").
    pub fn action(mut self, i: Icon, tip: &'a str, on: M) -> Self {
        self.action = Some((i, tip, on));
        self
    }

    pub fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    /// A badge in the top left corner (`.corner`).
    pub fn corner(mut self, el: El<'a, M>) -> Self {
        self.corner = Some(el);
        self
    }

    pub fn view(self) -> El<'a, M> {
        let p = theme::look();
        let w = self.width;
        let round = self.kind == TileKind::Artist;
        let radius = if round { w / 2.0 } else { 10.0 };
        let art: El<'a, M> = match (self.art, self.cover) {
            (Some(a), _) => container(a)
                .width(w)
                .height(w)
                .clip(true)
                .style(move |th| {
                    let mut s = theme::art(th);
                    s.border.radius = radius.into();
                    s
                })
                .into(),
            (None, Some(h)) => image(h)
                .width(w)
                .height(w)
                .content_fit(ContentFit::Cover)
                .border_radius(radius)
                .into(),
            (None, None) => {
                let i = match self.kind {
                    TileKind::Album => Icon::Disc,
                    TileKind::Artist => Icon::User,
                    TileKind::Playlist => Icon::List,
                };
                container(icon(i, 34.0, p.faint))
                    .width(w)
                    .height(w)
                    .center(w)
                    .style(move |th| {
                        let mut s = theme::art(th);
                        s.border.radius = radius.into();
                        s
                    })
                    .into()
            }
        };
        let mut layers = stack![art];
        if let Some(c) = self.corner {
            layers = layers.push(container(c).padding(8));
        }
        let mut hover_row = row![space().width(Length::Fill)].spacing(6).align_y(Alignment::End);
        if let Some((i, tip, on)) = self.action {
            let b = button(icon(i, 16.0, p.text))
                .padding(7)
                .style(theme::btn(Btn::Default))
                .on_press(on);
            if tip.is_empty() {
                hover_row = hover_row.push(b);
            } else {
                hover_row = hover_row.push(tooltip(b, super::tip_box(tip), tooltip::Position::Top).gap(4));
            }
        }
        if let Some(pl) = self.on_play {
            hover_row = hover_row.push(
                button(container(icon(Icon::Play, 18.0, p.on_accent)).center(40))
                    .padding(0)
                    .style(theme::btn(Btn::Play))
                    .on_press(pl),
            );
        }
        let overlay = container(hover_row).padding(10).width(w).height(w).align_y(Alignment::End);
        let art_with_hover: El<'a, M> = hover(layers, overlay);
        let title = t(self.title).size(14).font(theme::semibold()).width(w);
        let title = if self.center { title.center() } else { title };
        let mut lines = column![title].spacing(2);
        if !self.sub.is_empty() {
            let sub = small_muted(self.sub).size(13).width(w);
            lines = lines.push(if self.center { sub.center() } else { sub });
        }
        let body = column![art_with_hover, lines].spacing(9).width(w);
        button(body)
            .padding(0)
            .style(theme::btn(Btn::Row))
            .on_press(self.on_open)
            .into()
    }
}

/// A wrapping grid of tiles with the web's 16px gaps.
pub fn grid<'a, M: 'a>(tiles: Vec<El<'a, M>>) -> El<'a, M> {
    iced::widget::row(tiles).spacing(16).wrap().into()
}

/// One row of tiles that scrolls sideways (Discover's For you picks), the web's `.grid.rail`.
pub fn rail<'a, M: 'a>(tiles: Vec<El<'a, M>>) -> El<'a, M> {
    use iced::widget::scrollable::{Direction, Scrollbar};
    let strip = iced::widget::row(tiles).spacing(16).padding(iced::Padding { bottom: 12.0, ..iced::Padding::ZERO });
    iced::widget::scrollable(strip)
        .direction(Direction::Horizontal(Scrollbar::default()))
        .width(Length::Fill)
        .style(theme::scroll)
        .into()
}

/// Up to four pictures in a 2 × 2 collage of `size` px (the web's `.collage`); fewer than four
/// fall back to the first one, none to the icon.
pub fn collage<'a, M: 'a>(pictures: Vec<Option<image::Handle>>, size: f32, fallback: Icon) -> El<'a, M> {
    let p = theme::look();
    let cell = |h: Option<image::Handle>, s: f32| -> El<'a, M> {
        match h {
            Some(h) => image(h).width(s).height(s).content_fit(ContentFit::Cover).into(),
            None => container(space()).width(s).height(s).style(theme::art).into(),
        }
    };
    if pictures.len() >= 4 {
        let half = size / 2.0;
        let mut it = pictures.into_iter();
        let (a, b, c, d) = (it.next().flatten(), it.next().flatten(), it.next().flatten(), it.next().flatten());
        return column![row![cell(a, half), cell(b, half)], row![cell(c, half), cell(d, half)]].into();
    }
    match pictures.into_iter().next().flatten() {
        Some(h) => cell(Some(h), size),
        None => container(icon(fallback, 34.0, p.faint))
            .width(size)
            .height(size)
            .center(size)
            .into(),
    }
}
