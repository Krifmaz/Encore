//! The DOM the way the JavaScript used it: querySelector, innerHTML, closest, dataset, event listeners,
//! timers – plus the "page scope": listeners and timers that must die when the user navigates.

use std::any::Any;
use std::cell::RefCell;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{Element, Event, EventTarget, HtmlElement, HtmlInputElement};

pub use gloo_events::EventListener;
pub use gloo_timers::callback::{Interval, Timeout};

pub fn window() -> web_sys::Window { web_sys::window().expect("window") }
pub fn document() -> web_sys::Document { window().document().expect("document") }
pub fn body() -> HtmlElement { document().body().expect("body") }
pub fn location() -> web_sys::Location { window().location() }
pub fn local_storage() -> Option<web_sys::Storage> { window().local_storage().ok().flatten() }

pub fn ls_get(key: &str) -> Option<String> { local_storage().and_then(|s| s.get_item(key).ok().flatten()) }
pub fn ls_set(key: &str, value: &str) { if let Some(s) = local_storage() { let _ = s.set_item(key, value); } }
pub fn ls_remove(key: &str) { if let Some(s) = local_storage() { let _ = s.remove_item(key); } }

/// `document.querySelector(sel)`.
pub fn qs(sel: &str) -> Option<Element> { document().query_selector(sel).ok().flatten() }
/// `el.querySelector(sel)`.
pub fn qs_in(el: &Element, sel: &str) -> Option<Element> { el.query_selector(sel).ok().flatten() }
/// `[...el.querySelectorAll(sel)]`.
pub fn qsa_in(el: &Element, sel: &str) -> Vec<Element> {
    let Ok(list) = el.query_selector_all(sel) else { return vec![] };
    (0..list.length()).filter_map(|i| list.get(i)).filter_map(|n| n.dyn_into::<Element>().ok()).collect()
}
pub fn qsa(sel: &str) -> Vec<Element> {
    let Ok(list) = document().query_selector_all(sel) else { return vec![] };
    (0..list.length()).filter_map(|i| list.get(i)).filter_map(|n| n.dyn_into::<Element>().ok()).collect()
}
pub fn by_id(id: &str) -> Option<Element> { document().get_element_by_id(id) }

pub fn create(tag: &str) -> Element { document().create_element(tag).expect("create element") }
pub fn set_html(el: &Element, html: &str) { el.set_inner_html(html); }
pub fn html_el(el: &Element) -> Option<&HtmlElement> { el.dyn_ref::<HtmlElement>() }
pub fn set_text(el: &Element, text: &str) { el.set_text_content(Some(text)); }
pub fn text_of(el: &Element) -> String { el.text_content().unwrap_or_default() }

/// `e.target.closest(sel)`.
pub fn closest(e: &Event, sel: &str) -> Option<Element> {
    let t = e.target()?;
    let el = t.dyn_into::<Element>().ok()?;
    el.closest(sel).ok().flatten()
}
pub fn target_el(e: &Event) -> Option<Element> { e.target()?.dyn_into::<Element>().ok() }
/// `el.closest(sel)` on an element.
pub fn closest_of(el: &Element, sel: &str) -> Option<Element> { el.closest(sel).ok().flatten() }

/// `el.dataset.key` (missing → "").
pub fn data(el: &Element, key: &str) -> String { el.get_attribute(&format!("data-{}", kebab(key))).unwrap_or_default() }
pub fn data_opt(el: &Element, key: &str) -> Option<String> { el.get_attribute(&format!("data-{}", kebab(key))) }
pub fn set_data(el: &Element, key: &str, value: &str) { let _ = el.set_attribute(&format!("data-{}", kebab(key)), value); }
fn kebab(s: &str) -> String { let mut o = String::new(); for c in s.chars() { if c.is_ascii_uppercase() { o.push('-'); o.push(c.to_ascii_lowercase()); } else { o.push(c); } } o }

/// `input.value`.
pub fn value(el: &Element) -> String {
    if let Some(i) = el.dyn_ref::<HtmlInputElement>() { return i.value(); }
    if let Some(s) = el.dyn_ref::<web_sys::HtmlSelectElement>() { return s.value(); }
    if let Some(t) = el.dyn_ref::<web_sys::HtmlTextAreaElement>() { return t.value(); }
    String::new()
}
pub fn set_value(el: &Element, v: &str) {
    if let Some(i) = el.dyn_ref::<HtmlInputElement>() { i.set_value(v); }
    else if let Some(s) = el.dyn_ref::<web_sys::HtmlSelectElement>() { s.set_value(v); }
    else if let Some(t) = el.dyn_ref::<web_sys::HtmlTextAreaElement>() { t.set_value(v); }
}
/// `input.value` of `qs_in(root, sel)` ("" when missing).
pub fn value_of(root: &Element, sel: &str) -> String { qs_in(root, sel).map(|e| value(&e)).unwrap_or_default() }
pub fn checked(el: &Element) -> bool { el.dyn_ref::<HtmlInputElement>().map(|i| i.checked()).unwrap_or(false) }
pub fn set_checked(el: &Element, on: bool) { if let Some(i) = el.dyn_ref::<HtmlInputElement>() { i.set_checked(on); } }
pub fn checked_of(root: &Element, sel: &str) -> bool { qs_in(root, sel).map(|e| checked(&e)).unwrap_or(false) }
pub fn set_disabled(el: &Element, on: bool) {
    if on { let _ = el.set_attribute("disabled", ""); } else { let _ = el.remove_attribute("disabled"); }
}
pub fn focus(el: &Element) { if let Some(h) = el.dyn_ref::<HtmlElement>() { let _ = h.focus(); } }
pub fn click(el: &Element) { if let Some(h) = el.dyn_ref::<HtmlElement>() { h.click(); } }
pub fn add_class(el: &Element, c: &str) { let _ = el.class_list().add_1(c); }
pub fn remove_class(el: &Element, c: &str) { let _ = el.class_list().remove_1(c); }
pub fn toggle_class(el: &Element, c: &str, on: bool) { if on { add_class(el, c) } else { remove_class(el, c) } }
pub fn has_class(el: &Element, c: &str) -> bool { el.class_list().contains(c) }
pub fn set_style(el: &Element, prop: &str, v: &str) { if let Some(h) = el.dyn_ref::<HtmlElement>() { let _ = h.style().set_property(prop, v); } }
pub fn hide(el: &Element, hidden: bool) { if hidden { let _ = el.set_attribute("hidden", ""); } else { let _ = el.remove_attribute("hidden"); } }
pub fn remove(el: &Element) { el.remove(); }
pub fn is_connected(el: &Element) -> bool { el.is_connected() }
pub fn scroll_top() { window().scroll_to_with_x_and_y(0.0, 0.0); }
pub fn matches_media(q: &str) -> bool { window().match_media(q).ok().flatten().map(|m| m.matches()).unwrap_or(false) }
pub fn is_phone() -> bool { matches_media("(max-width: 760px)") }
pub fn now_ms() -> f64 { js_sys::Date::now() }
pub fn now_s() -> f64 { js_sys::Date::now() / 1000.0 }
pub fn prompt(msg: &str, default: &str) -> Option<String> { window().prompt_with_message_and_default(msg, default).ok().flatten() }
pub fn alert(msg: &str) { let _ = window().alert_with_message(msg); }
pub fn confirm(msg: &str) -> bool { window().confirm_with_message(msg).unwrap_or(false) }
pub fn reload() { let _ = location().reload(); }
pub fn hash() -> String { location().hash().unwrap_or_default() }
pub fn set_hash(h: &str) { let _ = location().set_hash(h); }
pub fn set_title(t: &str) { document().set_title(t); }
pub fn console_error(msg: &str) { web_sys::console::error_1(&JsValue::from_str(msg)); }
pub fn console_log(msg: &str) { web_sys::console::log_1(&JsValue::from_str(msg)); }

/// `window.dispatchEvent(new CustomEvent(name, { detail }))`.
pub fn dispatch(name: &str, detail: Option<JsValue>) {
    let init = web_sys::CustomEventInit::new();
    if let Some(d) = detail { init.set_detail(&d); }
    if let Ok(ev) = web_sys::CustomEvent::new_with_event_init_dict(name, &init) { let _ = window().dispatch_event(&ev); }
}

// ---- page scope: everything registered here is dropped on the next navigation (like the JS `cleanup`) ----
thread_local! {
    static PAGE: RefCell<Vec<Box<dyn Any>>> = const { RefCell::new(Vec::new()) };
    static APP: RefCell<Vec<Box<dyn Any>>> = const { RefCell::new(Vec::new()) };
}

/// Keep `x` (an EventListener, Interval, Timeout, …) alive until the next navigation.
pub fn keep(x: impl Any + 'static) { PAGE.with(|p| p.borrow_mut().push(Box::new(x))); }
/// Keep `x` for the whole session (shell, player, remote).
pub fn keep_forever(x: impl Any + 'static) { APP.with(|p| p.borrow_mut().push(Box::new(x))); }
/// The JS `onLeave`: only while the view is still on screen, otherwise clean up right away (drop).
pub fn keep_for(view: &Element, x: impl Any + 'static) { if view.is_connected() { keep(x) } else { drop(x) } }
pub fn clear_page() { PAGE.with(|p| p.borrow_mut().clear()); }

/// `el.addEventListener(ev, f)` for the current page.
pub fn on<F: FnMut(&Event) + 'static>(target: &EventTarget, ev: &str, f: F) {
    keep(EventListener::new(target, ev.to_string(), f));
}
/// `el.addEventListener(ev, f)` kept for the whole session.
pub fn on_forever<F: FnMut(&Event) + 'static>(target: &EventTarget, ev: &str, f: F) {
    keep_forever(EventListener::new(target, ev.to_string(), f));
}
/// A listener you keep yourself (dropping it removes it).
pub fn listener<F: FnMut(&Event) + 'static>(target: &EventTarget, ev: &str, f: F) -> EventListener {
    EventListener::new(target, ev.to_string(), f)
}
/// `window.addEventListener`.
pub fn on_window<F: FnMut(&Event) + 'static>(ev: &str, f: F) { on_forever(&window(), ev, f); }
pub fn on_document<F: FnMut(&Event) + 'static>(ev: &str, f: F) { on(&document(), ev, f); }
/// `el.querySelector(sel)?.addEventListener(ev, f)`.
pub fn on_sel<F: FnMut(&Event) + 'static>(root: &Element, sel: &str, ev: &str, f: F) {
    if let Some(el) = qs_in(root, sel) { on(&el, ev, f); }
}
/// `el.querySelectorAll(sel).forEach(x => x.addEventListener(ev, f))`; `f` gets the element it fired on.
pub fn on_each<F: Fn(&Element, &Event) + Clone + 'static>(root: &Element, sel: &str, ev: &str, f: F) {
    for el in qsa_in(root, sel) {
        let e2 = el.clone();
        let f = f.clone();
        on(&el, ev, move |e| f(&e2, e));
    }
}
/// `setTimeout(f, ms)` for the current page.
pub fn timeout<F: FnOnce() + 'static>(ms: u32, f: F) { keep(Timeout::new(ms, f)); }
/// `setInterval(f, ms)` tied to the view (dropped on navigation).
pub fn interval<F: FnMut() + 'static>(view: &Element, ms: u32, f: F) { keep_for(view, Interval::new(ms, f)); }
pub fn interval_forever<F: FnMut() + 'static>(ms: u32, f: F) { keep_forever(Interval::new(ms, f)); }
/// `await new Promise(r => setTimeout(r, ms))`.
pub async fn sleep(ms: u32) { gloo_timers::future::TimeoutFuture::new(ms).await; }

/// Run a future on the page (the JS `async` handler).
pub fn spawn<F: std::future::Future<Output = ()> + 'static>(f: F) { wasm_bindgen_futures::spawn_local(f); }

/// `e.key` for keyboard events.
pub fn key(e: &Event) -> String { e.dyn_ref::<web_sys::KeyboardEvent>().map(|k| k.key()).unwrap_or_default() }
pub fn prevent(e: &Event) { e.prevent_default(); }
pub fn stop(e: &Event) { e.stop_propagation(); }
