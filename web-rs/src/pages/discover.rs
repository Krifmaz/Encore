//! TODO: port of web/js/pages/discover.js – see docs/ui-rewrite/CONTRACT.md
#![allow(unused_variables, dead_code)]
use crate::app::{Ctx, PageFuture};
use serde_json::Value;
use std::rc::Rc;
use web_sys::Element;
#[allow(unused_imports)] use crate::json::J;
pub fn bind_request_buttons(_root: &Element) { todo!() }
pub fn render_search(view: Element, _ctx: Ctx) -> PageFuture {
    Box::pin(async move { view.set_inner_html(r#"<div class="empty"><h3>render_search: not ported yet</h3></div>"#); Ok(()) })
}
pub fn render_artist(view: Element, _ctx: Ctx) -> PageFuture {
    Box::pin(async move { view.set_inner_html(r#"<div class="empty"><h3>render_artist: not ported yet</h3></div>"#); Ok(()) })
}
pub fn render_artist_by_name(view: Element, _ctx: Ctx) -> PageFuture {
    Box::pin(async move { view.set_inner_html(r#"<div class="empty"><h3>render_artist_by_name: not ported yet</h3></div>"#); Ok(()) })
}
pub fn render_album(view: Element, _ctx: Ctx) -> PageFuture {
    Box::pin(async move { view.set_inner_html(r#"<div class="empty"><h3>render_album: not ported yet</h3></div>"#); Ok(()) })
}
