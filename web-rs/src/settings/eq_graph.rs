//! The equalizer as a graph (Settings → Playback → Sound effects), from Cadenza's Sound page: each band's curve
//! faint, the sum bold and filled to 0 dB, a numbered handle per band. Drag a handle (left/right = frequency, up/down
//! = gain), scroll on a bell or notch for its width (Q), double-click the empty graph for a bell. The curves are
//! cadenza-eq's own response, so the graph shows exactly what plays.

use crate::dom;
use crate::json::J;
use crate::num::js_num;
use serde_json::{json, Value};
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::JsCast;

const W: f64 = 600.0;
const H: f64 = 220.0;
/// The curves are drawn at this rate (the player runs the EQ at the device's; they only differ near its top).
const RATE: u32 = 48_000;
const FMIN: f64 = 20.0;
const FMAX: f64 = 20_000.0;
const COLORS: [&str; 8] = ["#f97316", "#eab308", "#22c55e", "#06b6d4", "#3b82f6", "#a855f7", "#ec4899", "#ef4444"];
/// Bands a graph takes (the same cap as the table).
pub const MAX: usize = 10;

fn fx(f: f64) -> f64 { (f / FMIN).ln() / (FMAX / FMIN).ln() * W }
fn xf(x: f64) -> f64 { FMIN * (FMAX / FMIN).powf((x / W).clamp(0.0, 1.0)) }
fn gy(g: f64, range: f64) -> f64 { H / 2.0 - g / range * (H / 2.0 - 14.0) }
fn yg(y: f64, range: f64) -> f64 { (H / 2.0 - y) / (H / 2.0 - 14.0) * range }
fn has_gain(b: &Value) -> bool { matches!(b.s("type").as_str(), "peaking" | "lowshelf" | "highshelf") }
/// `1.2k`, `850`, `20k`.
fn hz_label(f: f64) -> String {
    if f >= 1000.0 {
        let k = f / 1000.0;
        let s = if f >= 10_000.0 { format!("{k:.1}") } else { format!("{k:.2}") };
        format!("{}k", s.trim_end_matches('0').trim_end_matches('.'))
    } else {
        format!("{}", f.round())
    }
}
/// ±12 dB, or ±24 when a band goes past 12.
fn range_of(bands: &[Value]) -> f64 { if bands.iter().any(|b| b.f("gain").abs() > 12.0) { 24.0 } else { 12.0 } }
fn response(bands: &[cadenza_eq::Band], f: f64) -> f64 { cadenza_eq::Equalizer::new(RATE, 1, bands).response_db(f) }
fn polyline(points: impl Iterator<Item = (f64, f64)>) -> String {
    points.map(|(x, y)| format!("{:.1},{:.1}", x, y.clamp(-50.0, H + 50.0))).collect::<Vec<_>>().join(" ")
}
/// Where band `b`'s handle sits.
fn handle(b: &Value, range: f64) -> (f64, f64) {
    let g = if has_gain(b) { b.f("gain") } else { crate::playback::cadenza_band(b).map(|c| response(&[c], c.freq)).unwrap_or(0.0).clamp(-range, range) };
    (fx(b.f("freq").max(FMIN)), gy(g, range))
}

/// The inside of the graph's `<svg>`. `on`: the effects are on (off draws a flat line). `sel`: the band dragged.
pub fn inner(bands: &[Value], on: bool, sel: Option<usize>) -> String {
    let range = range_of(bands);
    let mut s = String::new();
    for f in [20.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10000.0, 20000.0] {
        let x = fx(f);
        s += &format!(r#"<line x1="{x:.1}" y1="0" x2="{x:.1}" y2="{H}" stroke="var(--border)" stroke-width="1"/>"#);
        let (tx, anchor) = if f >= 20000.0 { (x - 3.0, "end") } else { (x + 3.0, "start") };
        s += &format!(r#"<text x="{tx:.1}" y="{}" fill="var(--faint)" font-size="11" text-anchor="{anchor}">{}</text>"#, H - 4.0, hz_label(f));
    }
    for g in [-range, -range / 2.0, 0.0, range / 2.0, range] {
        let y = gy(g, range);
        s += &format!(r#"<line x1="0" y1="{y:.1}" x2="{W}" y2="{y:.1}" stroke="var(--border)" stroke-width="{}"/>"#, if g == 0.0 { 1.6 } else { 1.0 });
        s += &format!(r#"<text x="4" y="{:.1}" fill="var(--faint)" font-size="11">{}{}</text>"#, y - 3.0, if g > 0.0 { "+" } else { "" }, js_num(g));
    }
    let steps = 240;
    let xs = |n: usize| (0..=n).map(move |k| k as f64 / n as f64 * W);
    let mapped: Vec<Option<cadenza_eq::Band>> = bands.iter().map(crate::playback::cadenza_band).collect();
    if on {
        for (i, c) in mapped.iter().enumerate() {
            let Some(c) = c else { continue };
            let pts = polyline(xs(steps).map(|x| (x, gy(response(&[*c], xf(x)), range))));
            let a = if sel == Some(i) { 0.9 } else { 0.35 };
            s += &format!(r#"<polyline points="{pts}" fill="none" stroke="{}" stroke-opacity="{a}" stroke-width="1.2"/>"#, COLORS[i % COLORS.len()]);
        }
    }
    let all: Vec<cadenza_eq::Band> = if on { mapped.iter().flatten().copied().collect() } else { vec![] };
    let eq = cadenza_eq::Equalizer::new(RATE, 1, &all);
    let total = polyline(xs(steps * 2).map(|x| (x, gy(eq.response_db(xf(x)), range))));
    let zero = gy(0.0, range);
    s += &format!(r#"<polygon points="0,{zero:.1} {total} {W},{zero:.1}" fill="var(--accent)" fill-opacity="0.18"/>"#);
    s += &format!(r#"<polyline points="{total}" fill="none" stroke="var(--accent)" stroke-width="2.4"/>"#);
    for (i, b) in bands.iter().enumerate() {
        let (x, y) = handle(b, range);
        let r = if sel == Some(i) { 10.0 } else { 8.0 };
        s += &format!(r#"<g data-band="{i}" style="cursor:grab"><circle cx="{x:.1}" cy="{y:.1}" r="{}" fill="var(--bg-2)"/><circle cx="{x:.1}" cy="{y:.1}" r="{r}" fill="{}" fill-opacity="{}"/><text x="{x:.1}" y="{:.1}" fill="var(--bg)" font-size="11" font-weight="700" text-anchor="middle">{}</text></g>"#,
            r + 2.0, COLORS[i % COLORS.len()], if on { 0.95 } else { 0.4 }, y + 4.0, i + 1);
    }
    s
}

/// The graph's `<svg>`, to place above the band table.
pub fn html(bands: &[Value], on: bool) -> String {
    format!(r#"<svg id="pb-eq-graph" viewBox="0 0 {W} {H}" role="img" aria-label="Equalizer curve" style="width:100%;height:auto;display:block;background:var(--bg-2);border:1px solid var(--border);border-radius:12px;touch-action:none;user-select:none">{}</svg>"#,
        inner(bands, on, None))
}

/// A pointer position as graph coordinates (the viewBox's).
fn at(svg: &web_sys::Element, e: &web_sys::MouseEvent) -> (f64, f64) {
    let r = svg.get_bounding_client_rect();
    let x = (e.client_x() as f64 - r.left()) / r.width().max(1.0) * W;
    let y = (e.client_y() as f64 - r.top()) / r.height().max(1.0) * H;
    (x, y)
}

/// Make the graph under `root` live. `bands` are the bands on screen; `commit` saves a new list (as Custom).
pub fn bind(root: &web_sys::Element, bands: Vec<Value>, on: bool, commit: impl Fn(Vec<Value>) + 'static) {
    let Some(svg) = dom::qs_in(root, "#pb-eq-graph") else { return };
    if !on { return; }
    let commit = Rc::new(commit);
    let live = Rc::new(RefCell::new(bands));
    let drag: Rc<RefCell<Option<usize>>> = Rc::new(RefCell::new(None));
    {
        let (svg2, drag) = (svg.clone(), drag.clone());
        dom::on(&svg, "pointerdown", move |e| {
            let Some(pe) = e.dyn_ref::<web_sys::PointerEvent>() else { return };
            let Some(i) = e.target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()).and_then(|t| dom::closest_of(&t, "[data-band]"))
                .and_then(|g| dom::data(&g, "band").parse::<usize>().ok()) else { return };
            dom::prevent(e);
            let _ = svg2.set_pointer_capture(pe.pointer_id());
            *drag.borrow_mut() = Some(i);
        });
    }
    {
        let (svg2, drag, live) = (svg.clone(), drag.clone(), live.clone());
        dom::on(&svg, "pointermove", move |e| {
            let Some(i) = *drag.borrow() else { return };
            let Some(me) = e.dyn_ref::<web_sys::MouseEvent>() else { return };
            let (x, y) = at(&svg2, me);
            let mut list = live.borrow_mut();
            let range = range_of(&list);
            let Some(b) = list.get_mut(i) else { return };
            b["freq"] = json!(xf(x).round());
            if has_gain(b) { b["gain"] = crate::playback::jnum((yg(y, range).clamp(-range, range) * 2.0).round() / 2.0); }
            svg2.set_inner_html(&inner(&list, true, Some(i)));
        });
    }
    for ev in ["pointerup", "pointercancel"] {
        let (drag, live, commit) = (drag.clone(), live.clone(), commit.clone());
        dom::on(&svg, ev, move |_| {
            if drag.borrow_mut().take().is_some() { commit(live.borrow().clone()); }
        });
    }
    {
        let (svg2, live, commit) = (svg.clone(), live.clone(), commit.clone());
        dom::on(&svg, "dblclick", move |e| {
            let Some(me) = e.dyn_ref::<web_sys::MouseEvent>() else { return };
            let on_band = e.target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()).and_then(|t| dom::closest_of(&t, "[data-band]")).is_some();
            let mut list = live.borrow().clone();
            if on_band || list.len() >= MAX { return; }
            let (x, y) = at(&svg2, me);
            let range = range_of(&list);
            list.push(json!({ "type": "peaking", "freq": xf(x).round(), "gain": crate::playback::jnum((yg(y, range).clamp(-range, range) * 2.0).round() / 2.0), "q": 1 }));
            commit(list);
        });
    }
    {
        let (live, commit) = (live.clone(), commit.clone());
        dom::on(&svg, "wheel", move |e| {
            let Some(we) = e.dyn_ref::<web_sys::WheelEvent>() else { return };
            let Some(i) = e.target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()).and_then(|t| dom::closest_of(&t, "[data-band]"))
                .and_then(|g| dom::data(&g, "band").parse::<usize>().ok()) else { return };
            let mut list = live.borrow().clone();
            let Some(b) = list.get_mut(i) else { return };
            // Q only means a width on bells and notches (shelves have none, the cuts' Q is a resonance in dB)
            if !matches!(b.s("type").as_str(), "peaking" | "notch") { return; }
            dom::prevent(e);
            let f = if we.delta_y() < 0.0 { 1.1 } else { 1.0 / 1.1 };
            b["q"] = crate::playback::jnum(((b.f("q") * f).clamp(0.1, 18.0) * 100.0).round() / 100.0);
            commit(list);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_graph_draws_each_band_and_the_sum() {
        let bands = vec![json!({ "type": "peaking", "freq": 1000, "gain": 6, "q": 1 }), json!({ "type": "highpass", "freq": 80, "gain": 0, "q": 0.7 })];
        let s = inner(&bands, true, Some(0));
        assert_eq!(s.matches("data-band=").count(), 2);
        // one faint curve per band, plus the sum
        assert_eq!(s.matches("<polyline").count(), 3);
        // the bell's handle sits at +6 dB
        let (_, y) = handle(&bands[0], 12.0);
        assert!((y - gy(6.0, 12.0)).abs() < 1e-9);
        assert_eq!(hz_label(1250.0), "1.25k");
        assert_eq!(range_of(&[json!({ "gain": 18 })]), 24.0);
        // off: only the flat sum
        assert_eq!(inner(&bands, false, None).matches("<polyline").count(), 1);
    }
}
