//! TODO: port of web/js/pages/playlists.js
use crate::app::{Ctx, PageFuture};
use web_sys::Element;

pub fn render(view: Element, _ctx: Ctx) -> PageFuture {
    Box::pin(async move { view.set_inner_html("<div class=\"empty\"><h2>render</h2></div>"); Ok(()) })
}

pub fn render_playlist(view: Element, _ctx: Ctx) -> PageFuture {
    Box::pin(async move { view.set_inner_html("<div class=\"empty\"><h2>render_playlist</h2></div>"); Ok(()) })
}
