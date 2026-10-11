//! TODO: port of web/js/pages/auth.js – see docs/ui-rewrite/CONTRACT.md
#![allow(unused_variables, dead_code)]
use std::collections::HashMap;
use web_sys::Element;

pub async fn render(root: Element, _path: String, _query: HashMap<String, String>) {
    root.set_inner_html(r#"<div class="auth-wrap"><p>auth: not ported yet</p></div>"#);
}
