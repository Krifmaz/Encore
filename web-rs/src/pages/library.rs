//! TODO: port of web/js/pages/library.js – see docs/ui-rewrite/CONTRACT.md
#![allow(unused_variables, dead_code)]
use crate::app::{Ctx, PageFuture};
use serde_json::Value;
use std::rc::Rc;
use web_sys::Element;
#[allow(unused_imports)] use crate::json::J;
pub fn play_album(_id: &str, _start: usize) { todo!() }
pub fn track_menu(_anchor: &Element, _t: &Value, _admin: bool, _on_deleted: Option<Rc<dyn Fn()>>) { todo!() }
pub fn fix_song_modal(_t: &Value) { todo!() }
pub fn render(view: Element, _ctx: Ctx) -> PageFuture {
    Box::pin(async move { view.set_inner_html(r#"<div class="empty"><h3>render: not ported yet</h3></div>"#); Ok(()) })
}
pub fn render_album(view: Element, _ctx: Ctx) -> PageFuture {
    Box::pin(async move { view.set_inner_html(r#"<div class="empty"><h3>render_album: not ported yet</h3></div>"#); Ok(()) })
}
