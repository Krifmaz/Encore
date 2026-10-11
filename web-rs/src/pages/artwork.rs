//! TODO: port of web/js/pages/artwork.js – see docs/ui-rewrite/CONTRACT.md
#![allow(unused_variables, dead_code)]
use crate::app::{Ctx, PageFuture};
use serde_json::Value;
use std::rc::Rc;
use web_sys::Element;
#[allow(unused_imports)] use crate::json::J;
pub fn artwork_modal(_album_id: &str, _a: &Value, _on_done: Rc<dyn Fn()>) { todo!() }
