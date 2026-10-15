//! Port of web/js/scrubber.js: the library's side scrubber. Section labels from GET /api/library/index on
//! the right edge (desktop: on hover; touch: while scrolling, then drag), a bubble with the label under the
//! finger, and a jump to the section's first row through the page's own paging. Same DOM as the JS.

use crate::dom;
use crate::json::J;
use crate::ui::esc;
use serde_json::Value;
use std::cell::{Cell, RefCell};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use wasm_bindgen::JsCast;
use web_sys::Element;

/// Pixels a rail label needs; with more sections than fit, every Nth label is shown (all still jump).
const LABEL_PX: f64 = 18.0;

pub type JumpFn = Rc<dyn Fn(usize) -> Pin<Box<dyn Future<Output = ()>>>>;

pub struct Scrubber {
    el: Element,
    rail: Element,
    bubble: Element,
    sections: RefCell<Vec<(String, usize)>>,
    cur: Cell<i64>,
    picked: Cell<i64>,
    dragging: Cell<bool>,
    fade: RefCell<Option<dom::Timeout>>,
    last_track: Cell<f64>,
    /// the list's row `i` (list order)
    row: Box<dyn Fn(usize) -> Option<Element>>,
    /// whether the list is long enough to need a scrubber
    long: Box<dyn Fn() -> bool>,
    jump: JumpFn,
    busy: Cell<bool>,
    pending: Cell<Option<usize>>,
}

impl Scrubber {
    /// `scrubber(view, { rows, long, jump })`: mounts it in `view`; its listeners live as long as the page.
    pub fn mount(view: &Element, row: impl Fn(usize) -> Option<Element> + 'static, long: impl Fn() -> bool + 'static, jump: JumpFn) -> Rc<Scrubber> {
        let el = dom::create("div");
        el.set_class_name("scrubber");
        el.set_id("lib-scrub");
        dom::hide(&el, true);
        dom::set_html(&el, r##"<div class="scrub-rail"></div><div class="scrub-bubble"></div>"##);
        let _ = view.append_child(&el);
        let rail = dom::qs_in(&el, ".scrub-rail").expect("rail");
        let bubble = dom::qs_in(&el, ".scrub-bubble").expect("bubble");
        let s = Rc::new(Scrubber {
            el, rail: rail.clone(), bubble, sections: RefCell::new(vec![]), cur: Cell::new(-1), picked: Cell::new(-1), dragging: Cell::new(false),
            fade: RefCell::new(None), last_track: Cell::new(0.0), row: Box::new(row), long: Box::new(long), jump, busy: Cell::new(false), pending: Cell::new(None),
        });
        {
            let me = s.clone();
            dom::on(&rail, "pointerdown", move |e| {
                if me.el.has_attribute("hidden") { return; }
                dom::prevent(e);
                me.dragging.set(true);
                me.picked.set(-1);
                dom::add_class(&me.el, "drag");
                dom::add_class(&me.el, "on");
                if let Some(p) = e.dyn_ref::<web_sys::PointerEvent>() { let _ = me.rail.set_pointer_capture(p.pointer_id()); }
                me.pick(e);
            });
        }
        { let me = s.clone(); dom::on(&rail, "pointermove", move |e| if me.dragging.get() { me.pick(e) }); }
        { let me = s.clone(); dom::on(&rail, "pointerup", move |_| me.end()); }
        { let me = s.clone(); dom::on(&rail, "pointercancel", move |_| me.end()); }
        { let me = s.clone(); dom::on(&dom::window(), "scroll", move |_| me.on_scroll()); }
        { let me = s.clone(); dom::on(&dom::window(), "resize", move |_| me.refresh()); }
        s
    }

    /// `set(sections)`: `[{label, offset, short?}]` (the rail shows `short` when given, the bubble the label);
    /// anything else hides it.
    pub fn set(&self, next: &Value) {
        let arr = next.as_array().cloned().unwrap_or_default();
        let secs: Vec<(String, usize)> = arr.iter().map(|s| (s.s("label"), s.f("offset").max(0.0) as usize)).collect();
        dom::set_html(&self.rail, &arr.iter().enumerate().map(|(i, s)| {
            let short = s.s("short");
            format!(r##"<span data-s="{i}">{}</span>"##, esc(if short.is_empty() { s.s("label") } else { short }.as_str()))
        }).collect::<String>());
        *self.sections.borrow_mut() = secs;
        self.cur.set(-1);
        self.refresh();
        self.track();
    }

    /// `refresh()`: shown only for a long list with two sections or more.
    pub fn refresh(&self) {
        let n = self.sections.borrow().len();
        dom::hide(&self.el, n < 2 || !(self.long)());
        self.paint();
    }

    fn paint(&self) {
        if self.el.has_attribute("hidden") { return; }
        let n = self.sections.borrow().len();
        let h = (self.rail.client_height() as f64).max(1.0);
        let step = ((n as f64 * LABEL_PX / h).ceil() as usize).max(1);
        for (i, s) in dom::qsa_in(&self.rail, "[data-s]").iter().enumerate() {
            dom::toggle_class(s, "skip", i % step != 0 && i + 1 != n);
        }
    }

    /// The section the top of the screen is in.
    fn track(&self) {
        if self.el.has_attribute("hidden") || self.dragging.get() { return; }
        self.last_track.set(dom::now_ms());
        let mut at = 0usize;
        for (i, (_, off)) in self.sections.borrow().iter().enumerate() {
            match (self.row)(*off) {
                Some(r) if r.get_bounding_client_rect().top() <= 120.0 => at = i,
                _ => break,
            }
        }
        self.mark(at as i64);
    }

    fn mark(&self, i: i64) {
        if i == self.cur.get() { return; }
        if let Some(c) = dom::qs_in(&self.rail, ".cur") { dom::remove_class(&c, "cur"); }
        if let Some(c) = dom::qs_in(&self.rail, &format!(r##"[data-s="{i}"]"##)) { dom::add_class(&c, "cur"); }
        self.cur.set(i);
    }

    fn on_scroll(self: &Rc<Self>) {
        if self.el.has_attribute("hidden") { return; }
        dom::add_class(&self.el, "on");
        let me = Rc::downgrade(self);
        *self.fade.borrow_mut() = Some(dom::Timeout::new(1500, move || {
            let Some(me) = me.upgrade() else { return };
            if !me.dragging.get() { dom::remove_class(&me.el, "on"); }
            me.track();
        }));
        if dom::now_ms() - self.last_track.get() > 50.0 { self.track(); }
    }

    fn pick(self: &Rc<Self>, e: &web_sys::Event) {
        let Some(p) = e.dyn_ref::<web_sys::MouseEvent>() else { return };
        let n = self.sections.borrow().len();
        if n == 0 { return; }
        let r = self.rail.get_bounding_client_rect();
        let y = (p.client_y() as f64 - r.top()).max(0.0).min(r.height() - 1.0);
        let i = ((y / r.height() * n as f64).floor() as usize).min(n - 1);
        let top = y + self.rail.dyn_ref::<web_sys::HtmlElement>().map(|h| h.offset_top() as f64).unwrap_or(0.0);
        dom::set_style(&self.bubble, "top", &format!("{top}px"));
        if i as i64 == self.picked.get() { return; }
        self.picked.set(i as i64);
        let (label, offset) = self.sections.borrow()[i].clone();
        dom::set_text(&self.bubble, &label);
        self.mark(i as i64);
        if e.dyn_ref::<web_sys::PointerEvent>().is_some_and(|p| p.pointer_type() == "touch") {
            let _ = dom::window().navigator().vibrate_with_duration(4);   // a tick per section
        }
        // the latest pick wins
        self.pending.set(Some(offset));
        if self.busy.get() { return; }
        self.busy.set(true);
        let me = self.clone();
        dom::spawn(async move {
            while let Some(o) = me.pending.take() { (me.jump)(o).await; }
            me.busy.set(false);
        });
    }

    fn end(self: &Rc<Self>) {
        if !self.dragging.get() { return; }
        self.dragging.set(false);
        dom::remove_class(&self.el, "drag");
        self.on_scroll();
    }
}
