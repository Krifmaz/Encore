//! TODO: port of web/js/pages/playlists.js – see docs/ui-rewrite/CONTRACT.md
#![allow(unused_variables, dead_code)]
use crate::app::{Ctx, PageFuture};
use serde_json::Value;
use std::rc::Rc;
use web_sys::Element;
#[allow(unused_imports)] use crate::json::J;
pub fn import_modal(_url: &str, _into: Option<i64>) { todo!() }
pub fn album_choice(_title: &str, _go: Rc<dyn Fn(Option<String>)>) { todo!() }
pub fn add_to_playlist_modal(_track_ids: Vec<i64>) { todo!() }
pub fn render(view: Element, _ctx: Ctx) -> PageFuture {
    Box::pin(async move { view.set_inner_html(r#"<div class="empty"><h3>render: not ported yet</h3></div>"#); Ok(()) })
}
pub fn render_playlist(view: Element, _ctx: Ctx) -> PageFuture {
    Box::pin(async move { view.set_inner_html(r#"<div class="empty"><h3>render_playlist: not ported yet</h3></div>"#); Ok(()) })
}
