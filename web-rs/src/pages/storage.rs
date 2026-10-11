//! TODO: port of web/js/pages/storage.js
use crate::app::{Ctx, PageFuture};
use web_sys::Element;

pub fn render(view: Element, _ctx: Ctx) -> PageFuture {
    Box::pin(async move { view.set_inner_html("<div class=\"empty\"><h2>storage</h2></div>"); Ok(()) })
}

/// The small storage meter in the sidebar.
pub fn sidebar_meter(_el: Element) {}
