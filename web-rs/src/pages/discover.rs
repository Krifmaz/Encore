//! TODO: port of web/js/pages/discover.js
use crate::app::{Ctx, PageFuture};
use web_sys::Element;

pub fn render_search(view: Element, _ctx: Ctx) -> PageFuture {
    Box::pin(async move { view.set_inner_html("<div class=\"empty\"><h2>render_search</h2></div>"); Ok(()) })
}

pub fn render_artist(view: Element, _ctx: Ctx) -> PageFuture {
    Box::pin(async move { view.set_inner_html("<div class=\"empty\"><h2>render_artist</h2></div>"); Ok(()) })
}

pub fn render_artist_by_name(view: Element, _ctx: Ctx) -> PageFuture {
    Box::pin(async move { view.set_inner_html("<div class=\"empty\"><h2>render_artist_by_name</h2></div>"); Ok(()) })
}

pub fn render_album(view: Element, _ctx: Ctx) -> PageFuture {
    Box::pin(async move { view.set_inner_html("<div class=\"empty\"><h2>render_album</h2></div>"); Ok(()) })
}
