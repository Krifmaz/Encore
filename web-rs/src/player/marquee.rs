//! Long titles scroll: in the bar and the full-screen player (and lyrics mode's compact bar), a title, the line under
//! it or the artist line that doesn't fit scrolls sideways – a pause of about 1.5 s at the start, then about 30 px/s, a
//! gap, and round again – like the phone's (ui/components/Common.kt `scrollingTitle`). A line that fits stays still,
//! centred or left as before; with reduced motion asked for (`prefers-reduced-motion: reduce`) it keeps its "…".
//!
//! Lines are measured only after a view is really drawn (full::draw / the bar's draw returned true), when the full
//! player opens or changes pane, and when the window is resized – never per tick, so a play or a pause still swaps
//! only the button and no cover is drawn again. The movement itself is a CSS animation (app.css `.mq-run`) of the
//! line's inner span: `--marquee-dist` is the text's width plus the gap, after which a copy of the text (its `::after`)
//! stands exactly where the text started, so each round starts where the last one ended. A round is one animation
//! with a 1.5 s delay; at its end (`animationend`) the span switches to an identical second animation, which starts
//! the next round with the same pause.

use super::*;

/// The lines that may scroll: those marked `mq` (with an inner `mq-in` span) and the translation under the full
/// player's title (ui.rs draws it; its span is added here).
const LINES: &str = ".mq, .np-meta .tr-sub";
const SPEED_PX_S: f64 = 30.0;
/// The fade at each edge while a line scrolls (app.css: the line reaches this far left of its box, so a line resting
/// at the start shows its first letter in full).
const FADE_PX: f64 = 12.0;

thread_local! {
    static RESIZE: RefCell<Option<Timeout>> = const { RefCell::new(None) };
}

/// Once, at mount: a round that ended starts the next one; a resized window measures the lines again (and the
/// sung line of lyrics mode is put back in the middle: the lines wrap anew at another width).
pub(super) fn start_marquee() {
    dom::on_forever(&document(), "animationend", |e| {
        let Some(el) = e.target().and_then(|t| t.dyn_into::<Element>().ok()) else { return };
        if el.class_list().contains("mq-in") { dom::toggle_class(&el, "mq-alt", !el.class_list().contains("mq-alt")); }
    });
    dom::on_window("resize", |_| {
        let t = Timeout::new(150, || {
            measure_titles();
            super::lyrics::sync_lyrics(true);
        });
        RESIZE.with(|r| *r.borrow_mut() = Some(t));
    });
}

/// One line: its inner span, the text's own width, the room the line has, and the gap it scrolls with (when it does).
struct Line { el: Element, inner: Element, text_w: f64, room: f64, gap: Option<f64> }

/// The inner span that moves (made from the line's content when the line has none yet).
fn inner_of(el: &Element) -> Option<Element> {
    if let Some(i) = el.first_element_child().filter(|c| c.class_list().contains("mq-in")) { return Some(i); }
    let span = dom::create("span");
    span.set_class_name("mq-in");
    while let Some(c) = el.first_child() { span.append_child(&c).ok()?; }
    el.append_child(&span).ok()?;
    Some(span)
}

/// Measure every line on screen and start, keep or stop its scrolling. All the reading comes first, then all the
/// writing, so the page is laid out once.
pub(super) fn measure_titles() {
    let motion = !dom::matches_media("(prefers-reduced-motion: reduce)");
    let mut lines: Vec<Line> = vec![];
    for root in ["player", "np-full"] {
        let Some(r) = by_id(root) else { continue };
        for el in qsa_in(&r, LINES) {
            let Some(inner) = inner_of(&el) else { continue };
            dom::add_class(&el, "mq");
            // a line scrolling now has the fade's room to its left and the gap and the copy after its text
            let gap = el.class_list().contains("mq-run").then(|| num(&dom::data(&inner, "gap")));
            lines.push(Line { el, inner, text_w: 0.0, room: 0.0, gap });
        }
    }
    for l in lines.iter_mut() {
        let w = l.inner.get_bounding_client_rect().width();
        let room = l.el.client_width() as f64;
        (l.text_w, l.room) = match l.gap {
            Some(gap) => ((w - FADE_PX - gap) / 2.0, room - FADE_PX),
            None => (w, room),
        };
    }
    for l in lines {
        let fits = !motion || l.room <= 0.0 || l.text_w <= l.room + 0.5;
        match (l.gap, fits) {
            (Some(_), true) => stop(&l.el, &l.inner),
            (None, true) => {}
            (gap, false) => {
                let gap = gap.unwrap_or_else(|| (l.room / 3.0).max(24.0).round());
                let dist = l.text_w + gap;
                dom::set_data(&l.inner, "gap", &js_num(gap));
                let _ = l.inner.set_attribute("data-mq", &dom::text_of(&l.inner).replace('\n', " "));
                dom::set_style(&l.el, "--marquee-gap", &format!("{gap}px"));
                dom::set_style(&l.el, "--marquee-dist", &format!("{dist:.1}px"));
                dom::set_style(&l.el, "--marquee-time", &format!("{:.2}s", dist / SPEED_PX_S));
                dom::add_class(&l.el, "mq-run");
            }
        }
    }
}

fn stop(el: &Element, inner: &Element) {
    dom::remove_class(el, "mq-run");
    dom::remove_class(inner, "mq-alt");
    let _ = inner.remove_attribute("data-mq");
    let _ = inner.remove_attribute("data-gap");
}
