//! A one-line text that scrolls sideways when it does not fit (p6 item 11, the phone's
//! `scrollingTitle`, the web's marquee): it waits about 1.5 s at the start, moves at about
//! 30 px/s, leaves a gap and comes round again, with a fade at both edges. A text that fits stays
//! still (left or centred). With reduced motion it is cut with "…" instead.
//!
//! The movement asks the window for frames itself (`Shell::request_redraw_at`), only while the
//! text overflows and is inside the viewport, and only 20 times a second while it moves (1.5 px
//! a frame; once at the end of the pause): no app message, no `view` per frame. A window that is
//! hidden or minimised gets no frames, so nothing runs then.
//!
//! Reduced motion: the app has no setting of its own and Iced does not report the system's, so
//! the system setting is read once in the background: GNOME's `enable-animations`, macOS's
//! `reduceMotion`, Windows's "Show animations" (`MinAnimate`); `ENCORE_REDUCE_MOTION=1` forces it.

use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer;
use iced::advanced::text::{self as atext, paragraph};
use iced::advanced::widget::{tree, Tree, Widget};
use iced::advanced::{mouse, Clipboard, Shell};
use iced::alignment::Vertical;
use iced::widget::text::{LineHeight, Shaping, Wrapping};
use iced::{gradient, window, Background, Color, Element, Event, Font, Length, Pixels, Point, Radians, Rectangle, Size};
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant};

/// The wait at the start of each round.
pub const PAUSE: Duration = Duration::from_millis(1500);
/// Pixels per second.
pub const SPEED: f32 = 30.0;
/// The frame interval while it moves (the whole window is drawn again each time, so not more).
const FRAME: Duration = Duration::from_millis(50);
/// The width of the fade at each edge.
const FADE: f32 = 18.0;

/// The gap between the end of the text and its next round.
pub fn gap(visible: f32) -> f32 {
    (visible / 3.0).max(40.0)
}

/// Where the text is, `elapsed` into its rounds: (offset in px, how long until it moves again;
/// None while it moves). `travel` is the text's width plus the gap: one round.
pub fn offset_at(elapsed: Duration, travel: f32) -> (f32, Option<Duration>) {
    let moving = Duration::from_secs_f32((travel / SPEED).max(0.0));
    let round = PAUSE + moving;
    if round.is_zero() {
        return (0.0, None);
    }
    let into = Duration::from_secs_f64(elapsed.as_secs_f64() % round.as_secs_f64());
    if into < PAUSE {
        (0.0, Some(PAUSE - into))
    } else {
        (((into - PAUSE).as_secs_f32() * SPEED).min(travel), None)
    }
}

/// 0 = not known yet (moves), 1 = motion on, 2 = reduced motion.
static MOTION: AtomicU8 = AtomicU8::new(0);
static ASKED: std::sync::Once = std::sync::Once::new();

/// Whether titles may move (the system's reduced-motion setting, read once in the background;
/// on until it is known).
pub fn motion_on() -> bool {
    ASKED.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("encore-motion".into())
            .spawn(|| MOTION.store(if reduced_motion() { 2 } else { 1 }, Ordering::Relaxed));
    });
    MOTION.load(Ordering::Relaxed) != 2
}

fn reduced_motion() -> bool {
    if let Ok(v) = std::env::var("ENCORE_REDUCE_MOTION") {
        return matches!(v.trim(), "1" | "true" | "yes");
    }
    system_reduced_motion().unwrap_or(false)
}

/// What a command prints, trimmed (None when it is not there or fails).
fn output(cmd: &str, args: &[&str]) -> Option<String> {
    let mut c = std::process::Command::new(cmd);
    c.args(args).stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    let out = c.output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[cfg(target_os = "linux")]
fn system_reduced_motion() -> Option<bool> {
    let v = output("gsettings", &["get", "org.gnome.desktop.interface", "enable-animations"])?;
    Some(v == "false")
}

#[cfg(target_os = "macos")]
fn system_reduced_motion() -> Option<bool> {
    let v = output("defaults", &["read", "com.apple.universalaccess", "reduceMotion"])?;
    Some(v == "1")
}

#[cfg(windows)]
fn system_reduced_motion() -> Option<bool> {
    // "Show animations in Windows" off also turns window minimise animations off
    let v = output("reg", &["query", r"HKCU\Control Panel\Desktop\WindowMetrics", "/v", "MinAnimate"])?;
    let last = v.lines().find(|l| l.contains("MinAnimate"))?.split_whitespace().last()?.to_string();
    Some(last == "0")
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn system_reduced_motion() -> Option<bool> {
    None
}

/// The widget.
pub struct Marquee {
    content: String,
    size: f32,
    font: Font,
    color: Option<Color>,
    width: Length,
    center: bool,
    fade: Option<Color>,
}

/// A line of text that scrolls when it is too long (Shrink width: as wide as the text, at most
/// the space there is).
pub fn marquee(content: impl Into<String>) -> Marquee {
    Marquee {
        content: content.into(),
        size: 14.0,
        font: Font::DEFAULT,
        color: None,
        width: Length::Shrink,
        center: false,
        fade: None,
    }
}

impl Marquee {
    pub fn size(mut self, size: impl Into<Pixels>) -> Self {
        self.size = size.into().0;
        self
    }

    pub fn font(mut self, font: Font) -> Self {
        self.font = font;
        self
    }

    pub fn color(mut self, color: Color) -> Self {
        self.color = Some(color);
        self
    }

    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    /// Centre a text that fits (it still starts at the left edge when it scrolls).
    pub fn center(mut self) -> Self {
        self.center = true;
        self.width = Length::Fill;
        self
    }

    /// The colour behind the text: the edges fade into it while the text scrolls.
    pub fn fade(mut self, background: Color) -> Self {
        self.fade = Some(background);
        self
    }
}

struct State<P: atext::Paragraph> {
    text: paragraph::Plain<P>,
    dots: paragraph::Plain<P>,
    /// The text the rounds started for (a new text starts at the beginning).
    shown: String,
    started: Option<Instant>,
    offset: f32,
}

impl<P: atext::Paragraph> Default for State<P> {
    fn default() -> Self {
        Self {
            text: paragraph::Plain::default(),
            dots: paragraph::Plain::default(),
            shown: String::new(),
            started: None,
            offset: 0.0,
        }
    }
}

impl Marquee {
    fn spec<'b>(&self, content: &'b str) -> atext::Text<&'b str, Font> {
        atext::Text {
            content,
            bounds: Size::INFINITE,
            size: Pixels(self.size),
            line_height: LineHeight::default(),
            font: self.font,
            align_x: atext::Alignment::Left,
            align_y: Vertical::Top,
            shaping: Shaping::Advanced,
            wrapping: Wrapping::None,
        }
    }
}

impl<M, Theme, Renderer> Widget<M, Theme, Renderer> for Marquee
where
    Renderer: atext::Renderer<Font = Font>,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State<Renderer::Paragraph>>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::<Renderer::Paragraph>::default())
    }

    fn size(&self) -> Size<Length> {
        Size::new(self.width, Length::Shrink)
    }

    fn layout(&mut self, tree: &mut Tree, _renderer: &Renderer, limits: &layout::Limits) -> layout::Node {
        let st = tree.state.downcast_mut::<State<Renderer::Paragraph>>();
        let _ = st.text.update(self.spec(&self.content));
        let _ = st.dots.update(self.spec("…"));
        if st.shown != self.content {
            st.shown = self.content.clone();
            st.started = None;
            st.offset = 0.0;
        }
        let natural = st.text.min_bounds();
        layout::Node::new(limits.resolve(self.width, Length::Shrink, natural))
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _renderer: &Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, M>,
        viewport: &Rectangle,
    ) {
        let Event::Window(window::Event::RedrawRequested(now)) = event else {
            return;
        };
        let st = tree.state.downcast_mut::<State<Renderer::Paragraph>>();
        let bounds = layout.bounds();
        let width = st.text.min_bounds().width;
        let moves = width - bounds.width > 0.5 && motion_on() && viewport.intersects(&bounds);
        if !moves {
            st.started = None;
            st.offset = 0.0;
            return;
        }
        let started = *st.started.get_or_insert(*now);
        let (offset, wait) = offset_at(now.saturating_duration_since(started), width + gap(bounds.width));
        st.offset = offset;
        shell.request_redraw_at(*now + wait.unwrap_or(FRAME));
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        _theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let st = tree.state.downcast_ref::<State<Renderer::Paragraph>>();
        let bounds = layout.bounds();
        let Some(clip) = bounds.intersection(viewport) else {
            return;
        };
        let color = self.color.unwrap_or(style.text_color);
        let natural = st.text.min_bounds();
        // the line's text sits at the top of the bounds (the bounds are the text's height)
        let y = bounds.y;
        if natural.width - bounds.width <= 0.5 {
            let x = if self.center { bounds.x + (bounds.width - natural.width) / 2.0 } else { bounds.x };
            renderer.fill_paragraph(st.text.raw(), Point::new(x, y), color, clip);
            return;
        }
        if !motion_on() {
            // reduced motion: cut, with "…" at the end
            let dots = st.dots.min_bounds().width;
            let cut = Rectangle { width: (bounds.width - dots).max(0.0), ..bounds };
            if let Some(cut) = cut.intersection(&clip) {
                renderer.with_layer(cut, |r| r.fill_paragraph(st.text.raw(), Point::new(bounds.x, y), color, cut));
            }
            renderer.fill_paragraph(st.dots.raw(), Point::new(bounds.x + bounds.width - dots, y), color, clip);
            return;
        }
        let travel = natural.width + gap(bounds.width);
        renderer.with_layer(clip, |r| {
            let x = bounds.x - st.offset;
            r.fill_paragraph(st.text.raw(), Point::new(x, y), color, clip);
            r.fill_paragraph(st.text.raw(), Point::new(x + travel, y), color, clip);
        });
        // the fades go in a layer of their own: within one layer quads are drawn under text
        if let Some(bg) = self.fade {
            let clear = Color { a: 0.0, ..bg };
            let edge = |at: f32, from: Color, to: Color| {
                (
                    renderer::Quad { bounds: Rectangle { x: at, width: FADE, ..bounds }, ..renderer::Quad::default() },
                    Background::Gradient(gradient::Linear::new(Radians(std::f32::consts::FRAC_PI_2)).add_stop(0.0, from).add_stop(1.0, to).into()),
                )
            };
            renderer.with_layer(clip, |r| {
                if st.offset > 0.0 {
                    let (q, b) = edge(bounds.x, bg, clear);
                    r.fill_quad(q, b);
                }
                let (q, b) = edge(bounds.x + bounds.width - FADE, clear, bg);
                r.fill_quad(q, b);
            });
        }
    }
}

impl<'a, M: 'a, Theme: 'a, Renderer> From<Marquee> for Element<'a, M, Theme, Renderer>
where
    Renderer: atext::Renderer<Font = Font> + 'a,
{
    fn from(m: Marquee) -> Self {
        Element::new(m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_waits_then_moves_round_again() {
        let travel = 90.0; // 3 s of movement at 30 px/s
        assert_eq!(offset_at(Duration::ZERO, travel), (0.0, Some(PAUSE)));
        let (o, w) = offset_at(Duration::from_millis(1000), travel);
        assert_eq!((o, w), (0.0, Some(Duration::from_millis(500))));
        let (o, w) = offset_at(Duration::from_millis(2500), travel);
        assert!((o - 30.0).abs() < 0.01 && w.is_none(), "1 s after the pause: 30 px");
        // a round is the pause plus the travel: 4.5 s, then it starts over with the pause
        let (o, w) = offset_at(Duration::from_millis(4600), travel);
        assert_eq!(o, 0.0);
        assert!(w.is_some());
        assert_eq!(gap(60.0), 40.0);
        assert_eq!(gap(300.0), 100.0);
    }
}
