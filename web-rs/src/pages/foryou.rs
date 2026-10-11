//! TODO: port of web/js/pages/foryou.js – see docs/ui-rewrite/CONTRACT.md
#![allow(unused_variables, dead_code)]
use crate::app::{Ctx, PageFuture};
use serde_json::Value;
use std::rc::Rc;
use web_sys::Element;
#[allow(unused_imports)] use crate::json::J;
pub fn more_like_this(_artist: &str, _title: &str, _mbid: &str) { todo!() }
pub fn artist_card(_a: &Value) -> String { todo!() }
pub fn render(view: Element, _ctx: Ctx) -> PageFuture {
    Box::pin(async move { view.set_inner_html(r#"<div class="empty"><h3>render: not ported yet</h3></div>"#); Ok(()) })
}
pub fn render_tag(view: Element, _ctx: Ctx) -> PageFuture {
    Box::pin(async move { view.set_inner_html(r#"<div class="empty"><h3>render_tag: not ported yet</h3></div>"#); Ok(()) })
}
