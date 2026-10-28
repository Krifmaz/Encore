//! A three-part row that keeps its middle part in the middle of the window: the player bar's
//! song (left), transport and seek (centre) and buttons (right).
//!
//! A plain `row!` gives the right side what its buttons need and splits the rest between the
//! song and the transport, so the more buttons the right side has (an "Engine error" badge, the
//! quality badge, devices, ...) the further left the play button sits. Here both sides are
//! always the same width, at least as wide as the right side's buttons, and the centre takes
//! what is left between them, so the play button and the seek bar stay centred in the window.
//! Only when the window is too narrow for that does the song give up its room first (see
//! [`split`]).

use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{tree, Operation, Tree, Widget};
use iced::advanced::{mouse, overlay, renderer, Clipboard, Shell};
use iced::{Element, Event, Length, Point, Rectangle, Size, Vector};

/// The centre's share of the bar's width while there is room for it (the old 3 : 4 : 3 split).
const CENTER_SHARE: f32 = 0.4;
/// The centre never gets narrower than this while the song can still give up room for it.
const CENTER_MIN: f32 = 260.0;

/// The widths of the left side and the centre for a bar `width` wide whose right side needs
/// `right` (its buttons at their natural width), with `spacing` between the parts.
///
/// Normally both sides are `max(right, 30 % of the bar)` and the centre is what is left, so it
/// sits exactly in the middle. If that leaves the centre narrower than [`CENTER_MIN`], the
/// centre keeps [`CENTER_MIN`] (or whatever fits next to the right side) and the left side takes
/// the rest, down to nothing: the right side's buttons are never cut off.
pub fn split(width: f32, right: f32, spacing: f32) -> (f32, f32) {
    // what the three parts share once the two gaps are taken out
    let room = (width - 2.0 * spacing).max(0.0);
    let side = right.max(room * (1.0 - CENTER_SHARE) / 2.0);
    let center = room - 2.0 * side;
    if center >= CENTER_MIN {
        return (side, center);
    }
    // not enough room to stay symmetric: the centre keeps its minimum next to the right side,
    // and the song gets whatever is left of the bar
    let center = CENTER_MIN.min((room - right).max(0.0));
    let left = (room - right - center).max(0.0);
    (left, center)
}

/// The bar row: `left`, `center` and `right`, the centre kept in the middle (see the module docs).
pub struct CenteredBar<'a, Message, Theme, Renderer> {
    /// Left, centre and right, in that order (the order `layout` and the tree rely on).
    children: [Element<'a, Message, Theme, Renderer>; 3],
    /// The gap between the parts.
    spacing: f32,
}

/// A [`CenteredBar`] of `left`, `center` and `right` with `spacing` between them.
pub fn centered_bar<'a, Message, Theme, Renderer>(
    left: impl Into<Element<'a, Message, Theme, Renderer>>,
    center: impl Into<Element<'a, Message, Theme, Renderer>>,
    right: impl Into<Element<'a, Message, Theme, Renderer>>,
    spacing: f32,
) -> CenteredBar<'a, Message, Theme, Renderer> {
    CenteredBar { children: [left.into(), center.into(), right.into()], spacing }
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer> for CenteredBar<'_, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    fn children(&self) -> Vec<Tree> {
        self.children.iter().map(Tree::new).collect()
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(&self.children);
    }

    fn tag(&self) -> tree::Tag {
        tree::Tag::stateless()
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fill)
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) -> layout::Node {
        let max = limits.max();
        let [left, center, right] = &mut self.children;
        let [lt, ct, rt] = &mut tree.children[..] else { unreachable!("a centered bar always has three children") };
        // the right side first, at its natural width: that decides how wide both sides are
        let r = right.as_widget_mut().layout(rt, renderer, &layout::Limits::new(Size::ZERO, max));
        let rw = r.size().width.min(max.width);
        let (lw, cw) = split(max.width, rw, self.spacing);
        // left and centre get exactly their widths (both are Fill inside)
        let exact = |w: f32| layout::Limits::new(Size::new(w, 0.0), Size::new(w, max.height));
        let l = left.as_widget_mut().layout(lt, renderer, &exact(lw));
        let c = center.as_widget_mut().layout(ct, renderer, &exact(cw));
        // each part centred vertically; the right side pinned to the right edge
        let y = |n: &layout::Node| ((max.height - n.size().height) / 2.0).max(0.0);
        let (ly, cy, ry) = (y(&l), y(&c), y(&r));
        layout::Node::with_children(
            max,
            vec![
                l.move_to(Point::new(0.0, ly)),
                c.move_to(Point::new(lw + self.spacing, cy)),
                r.move_to(Point::new(max.width - rw, ry)),
            ],
        )
    }

    fn operate(&mut self, tree: &mut Tree, layout: Layout<'_>, renderer: &Renderer, operation: &mut dyn Operation) {
        operation.container(None, layout.bounds());
        operation.traverse(&mut |operation| {
            for ((child, state), layout) in self.children.iter_mut().zip(&mut tree.children).zip(layout.children()) {
                child.as_widget_mut().operate(state, layout, renderer, operation);
            }
        });
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        for ((child, state), layout) in self.children.iter_mut().zip(&mut tree.children).zip(layout.children()) {
            child.as_widget_mut().update(state, event, layout, cursor, renderer, clipboard, shell, viewport);
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.children
            .iter()
            .zip(&tree.children)
            .zip(layout.children())
            .map(|((child, state), layout)| child.as_widget().mouse_interaction(state, layout, cursor, viewport, renderer))
            .max()
            .unwrap_or_default()
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        for ((child, state), layout) in self.children.iter().zip(&tree.children).zip(layout.children()) {
            child.as_widget().draw(state, renderer, theme, style, layout, cursor, viewport);
        }
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        overlay::from_children(&mut self.children, tree, layout, renderer, viewport, translation)
    }
}

impl<'a, Message: 'a, Theme: 'a, Renderer> From<CenteredBar<'a, Message, Theme, Renderer>> for Element<'a, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer + 'a,
{
    fn from(bar: CenteredBar<'a, Message, Theme, Renderer>) -> Self {
        Element::new(bar)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A wide window: both sides equal, so the centre's middle is the window's middle.
    #[test]
    fn wide_bar_keeps_the_centre_in_the_middle() {
        let (w, sp) = (1900.0, 16.0);
        let (left, center) = split(w, 700.0, sp);
        assert_eq!(left, 700.0);
        let mid = left + sp + center / 2.0;
        assert!((mid - w / 2.0).abs() < 0.01, "centre's middle {mid} is off the window's {}", w / 2.0);
    }

    /// Few buttons: the sides are 30 % each and the centre 40 %, the old 3 : 4 : 3 split.
    #[test]
    fn few_buttons_keep_the_old_proportions() {
        let (left, center) = split(1032.0, 200.0, 16.0);
        assert!((left - 300.0).abs() < 0.01 && (center - 400.0).abs() < 0.01, "{left} {center}");
    }

    /// Narrow window: the centre keeps its minimum and the song gives up room, right never cut.
    #[test]
    fn narrow_bar_takes_room_from_the_song() {
        let (w, sp, right) = (1100.0, 16.0, 700.0);
        let (left, center) = split(w, right, sp);
        assert_eq!(center, CENTER_MIN);
        assert!((left + center + right + 2.0 * sp - w).abs() < 0.01);
    }

    /// Too narrow even for the right side: nothing goes negative.
    #[test]
    fn tiny_bar_never_goes_negative() {
        let (left, center) = split(500.0, 700.0, 16.0);
        assert_eq!((left, center), (0.0, 0.0));
    }
}
