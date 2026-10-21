//! The parametric EQ as a graph, from Cadenza's Sound page: each band's curve faint, the sum
//! bold and filled to 0 dB, a numbered handle per band. Drag a handle (left/right = frequency,
//! up/down = gain), scroll on it for the width (Q), double-click the empty graph for a bell.
//! The page owns the bands; the graph only reports what the pointer did, as a [`GraphEvent`].

use crate::theme;
use cadenza_dsp::eq::{Band, BandKind, Equalizer};
use iced::mouse;
use iced::widget::canvas::{self, Frame, Geometry, Path, Stroke, Text};
use iced::{Color, Element, Length, Point, Rectangle, Renderer, Size, Theme};
use std::time::{Duration, Instant};

/// The curves are drawn at this rate (the engine runs the EQ at the source's rate; the
/// curves only differ near its top).
const RATE: u32 = 48_000;
const FMIN: f64 = 20.0;
const FMAX: f64 = 20_000.0;
const COLORS: [&str; 8] = ["#f97316", "#eab308", "#22c55e", "#06b6d4", "#3b82f6", "#a855f7", "#ec4899", "#ef4444"];

/// Band `i`'s colour (its handle, its curve, its box on the page).
pub fn color(i: usize) -> Color {
    theme::hex(COLORS[i % COLORS.len()])
}

/// Peaking and the shelves have a gain; the cuts and the notch do not.
pub fn has_gain(k: BandKind) -> bool {
    matches!(k, BandKind::Peaking | BandKind::LowShelf | BandKind::HighShelf)
}

/// `1.2k`, `850`, `20k`.
pub fn hz_label(f: f64) -> String {
    if f >= 1000.0 {
        let k = f / 1000.0;
        let s = if f >= 10_000.0 { format!("{k:.1}") } else { format!("{k:.2}") };
        format!("{}k", s.trim_end_matches('0').trim_end_matches('.'))
    } else {
        format!("{}", f.round())
    }
}

/// What the pointer did on the graph.
#[derive(Clone, Copy, Debug)]
pub enum GraphEvent {
    Select(usize),
    /// `(band, frequency, gain)` while a handle is dragged.
    Drag(usize, f64, f64),
    DragEnd,
    /// `(band, wheel delta)`: up widens the Q, down narrows it.
    Wheel(usize, f32),
    /// Double-click on the empty graph: a bell at `(frequency, gain)`.
    Add(f64, f64),
}

/// The graph, `height` pixels tall. `range` is the gain shown above and below 0 dB.
pub fn eq_graph<'a, M: 'a>(bands: Vec<Band>, on: bool, range: f64, sel: Option<usize>, height: f32, on_event: impl Fn(GraphEvent) -> M + 'a) -> Element<'a, M> {
    canvas::Canvas::new(Graph { bands, on, range, sel, on_event: Box::new(on_event) }).width(Length::Fill).height(height).into()
}

struct Graph<'a, M> {
    bands: Vec<Band>,
    on: bool,
    range: f64,
    sel: Option<usize>,
    on_event: Box<dyn Fn(GraphEvent) -> M + 'a>,
}

#[derive(Default)]
struct GraphState {
    drag: Option<usize>,
    last_click: Option<Instant>,
}

impl<M> Graph<'_, M> {
    fn fx(&self, f: f64, w: f32) -> f32 {
        ((f / FMIN).ln() / (FMAX / FMIN).ln()) as f32 * w
    }
    fn xf(&self, x: f32, w: f32) -> f64 {
        FMIN * (FMAX / FMIN).powf((x / w).clamp(0.0, 1.0) as f64)
    }
    fn gy(&self, g: f64, h: f32) -> f32 {
        h / 2.0 - (g / self.range) as f32 * (h / 2.0 - 14.0)
    }
    fn yg(&self, y: f32, h: f32) -> f64 {
        ((h / 2.0 - y) / (h / 2.0 - 14.0)) as f64 * self.range
    }
    fn band_db(&self, b: &Band, f: f64) -> f64 {
        Equalizer::new(RATE, 1, &[Band { enabled: true, ..*b }]).response_db(f)
    }
    /// Where band `i`'s handle sits.
    fn handle(&self, i: usize, size: Size) -> Point {
        let b = &self.bands[i];
        let g = if has_gain(b.kind) { b.gain_db } else { self.band_db(b, b.freq).clamp(-self.range, self.range) };
        Point::new(self.fx(b.freq, size.width), self.gy(g, size.height))
    }
    fn hit(&self, p: Point, size: Size) -> Option<usize> {
        (0..self.bands.len()).rev().find(|&i| self.handle(i, size).distance(p) <= 12.0)
    }
}

impl<M> canvas::Program<M> for Graph<'_, M> {
    type State = GraphState;

    fn update(&self, state: &mut GraphState, event: &iced::Event, bounds: Rectangle, cursor: mouse::Cursor) -> Option<canvas::Action<M>> {
        let size = bounds.size();
        let pos = cursor.position_in(bounds);
        let publish = |e: GraphEvent| Some(canvas::Action::publish((self.on_event)(e)).and_capture());
        match event {
            iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let p = pos?;
                if let Some(i) = self.hit(p, size) {
                    state.drag = Some(i);
                    return publish(GraphEvent::Select(i));
                }
                // double-click on the empty graph: a bell there
                let now = Instant::now();
                let double = state.last_click.is_some_and(|t| now.duration_since(t) < Duration::from_millis(400));
                state.last_click = Some(now);
                if double {
                    state.last_click = None;
                    let g = self.yg(p.y, size.height).clamp(-self.range, self.range);
                    return publish(GraphEvent::Add(self.xf(p.x, size.width), g));
                }
                None
            }
            iced::Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                let i = state.drag?;
                let p = cursor.position_from(bounds.position())?;
                let g = self.yg(p.y, size.height).clamp(-self.range, self.range);
                publish(GraphEvent::Drag(i, self.xf(p.x, size.width), g))
            }
            iced::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                state.drag.take()?;
                publish(GraphEvent::DragEnd)
            }
            iced::Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                let i = self.hit(pos?, size)?;
                let (mouse::ScrollDelta::Lines { y, .. } | mouse::ScrollDelta::Pixels { y, .. }) = delta;
                publish(GraphEvent::Wheel(i, *y))
            }
            _ => None,
        }
    }

    fn draw(&self, _: &GraphState, renderer: &Renderer, _: &Theme, bounds: Rectangle, _: mouse::Cursor) -> Vec<Geometry> {
        let l = theme::look();
        let mut frame = Frame::new(renderer, bounds.size());
        let (w, h) = (bounds.width, bounds.height);
        let grid = Stroke::default().with_color(l.border).with_width(1.0);
        for f in [20.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10000.0, 20000.0] {
            let x = self.fx(f, w);
            frame.stroke(&Path::line(Point::new(x, 0.0), Point::new(x, h)), grid);
            frame.fill_text(Text {
                content: hz_label(f),
                position: Point::new(if f >= 20000.0 { x - 26.0 } else { x + 3.0 }, h - 16.0),
                color: l.faint,
                size: 11.0.into(),
                ..Default::default()
            });
        }
        let r = self.range;
        for g in [-r, -r / 2.0, 0.0, r / 2.0, r] {
            let y = self.gy(g, h);
            frame.stroke(&Path::line(Point::new(0.0, y), Point::new(w, y)), Stroke::default().with_color(l.border).with_width(if g == 0.0 { 1.6 } else { 1.0 }));
            frame.fill_text(Text {
                content: format!("{}{}", if g > 0.0 { "+" } else { "" }, g),
                position: Point::new(4.0, y - 14.0),
                color: l.faint,
                size: 11.0.into(),
                ..Default::default()
            });
        }
        // each band's own curve, faint; the sum bold, filled to 0 dB
        let steps = 240;
        if self.on {
            for (i, b) in self.bands.iter().enumerate().filter(|(_, b)| b.enabled) {
                let path = Path::new(|p| {
                    for k in 0..=steps {
                        let x = k as f32 / steps as f32 * w;
                        let y = self.gy(self.band_db(b, self.xf(x, w)), h).clamp(-50.0, h + 50.0);
                        if k == 0 { p.move_to(Point::new(x, y)) } else { p.line_to(Point::new(x, y)) }
                    }
                });
                let a = if self.sel == Some(i) { 0.9 } else { 0.35 };
                frame.stroke(&path, Stroke::default().with_color(color(i).scale_alpha(a)).with_width(1.2));
            }
        }
        let eq = Equalizer::new(RATE, 1, &self.bands);
        let total: Vec<Point> = (0..=steps * 2)
            .map(|k| {
                let x = k as f32 / (steps * 2) as f32 * w;
                let g = if self.on { eq.response_db(self.xf(x, w)) } else { 0.0 };
                Point::new(x, self.gy(g, h).clamp(-50.0, h + 50.0))
            })
            .collect();
        let zero = self.gy(0.0, h);
        let fill = Path::new(|p| {
            p.move_to(Point::new(0.0, zero));
            for pt in &total {
                p.line_to(*pt);
            }
            p.line_to(Point::new(w, zero));
            p.close();
        });
        frame.fill(&fill, l.accent.scale_alpha(0.18));
        let line = Path::new(|p| {
            for (k, pt) in total.iter().enumerate() {
                if k == 0 { p.move_to(*pt) } else { p.line_to(*pt) }
            }
        });
        frame.stroke(&line, Stroke::default().with_color(l.accent).with_width(2.4));
        for i in 0..self.bands.len() {
            let c = self.handle(i, bounds.size());
            let r = if self.sel == Some(i) { 10.0 } else { 8.0 };
            frame.fill(&Path::circle(c, r + 2.0), l.bg2);
            frame.fill(&Path::circle(c, r), color(i).scale_alpha(if self.bands[i].enabled { 0.95 } else { 0.3 }));
            frame.fill_text(Text {
                content: format!("{}", i + 1),
                position: Point::new(c.x - 3.5, c.y - 7.0),
                color: l.bg,
                size: 11.0.into(),
                font: theme::bold(),
                ..Default::default()
            });
        }
        vec![frame.into_geometry()]
    }

    fn mouse_interaction(&self, state: &GraphState, bounds: Rectangle, cursor: mouse::Cursor) -> mouse::Interaction {
        if state.drag.is_some() {
            return mouse::Interaction::Grabbing;
        }
        match cursor.position_in(bounds) {
            Some(p) if self.hit(p, bounds.size()).is_some() => mouse::Interaction::Grab,
            Some(_) => mouse::Interaction::Crosshair,
            None => mouse::Interaction::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hz_labels_read_like_cadenzas() {
        assert_eq!(hz_label(20.0), "20");
        assert_eq!(hz_label(1000.0), "1k");
        assert_eq!(hz_label(1250.0), "1.25k");
        assert_eq!(hz_label(20000.0), "20k");
    }
}
