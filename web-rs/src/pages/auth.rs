//! TODO: port of web/js/pages/auth.js
use std::collections::HashMap;
use web_sys::Element;

pub async fn render(root: Element, _path: String, _query: HashMap<String, String>) {
    root.set_inner_html("<div class=\"auth-wrap\"><p>auth</p></div>");
}
