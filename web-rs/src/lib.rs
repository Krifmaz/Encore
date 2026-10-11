//! Encore's web app, in Rust (WebAssembly). A 1:1 port of web/js: every module here mirrors the JavaScript
//! file of the same name – same HTML, same CSS classes, same text, same API calls, same localStorage keys –
//! so users see no difference. Pages build HTML strings and wire events exactly like the JS did; dom.rs,
//! json.rs, api.rs and ui.rs are the small toolkit that makes that natural in Rust.

pub mod api;
pub mod app;
pub mod appearance;
pub mod dom;
pub mod json;
pub mod pages;
pub mod player;
pub mod remote;
pub mod ui;

use wasm_bindgen::prelude::*;

#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
    ui::init();
    appearance::init();
    app::init();
}
