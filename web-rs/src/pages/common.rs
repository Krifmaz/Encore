//! TODO: port of web/js/pages/common.js – see docs/ui-rewrite/CONTRACT.md
#![allow(unused_variables, dead_code)]
use crate::app::{Ctx, PageFuture};
use serde_json::Value;
use std::rc::Rc;
use web_sys::Element;
#[allow(unused_imports)] use crate::json::J;

pub async fn submit_request(_body: Value, _btn: Option<Element>) -> Option<Value> { todo!() }
pub fn album_tile(_a: &Value, _href: &str, _actions: &str, _sub: Option<&str>) -> String { todo!() }
pub fn library_tile(_a: &Value) -> String { todo!() }
pub fn bind_tiles(_root: &Element) { todo!() }
pub fn request_row(_r: &Value, _admin: bool) -> String { todo!() }
pub fn bind_request_actions(_root: &Element, _reload: Rc<dyn Fn()>) { todo!() }
pub fn clear_menu(_anchor: &Element, _reload: Rc<dyn Fn()>, _mine: bool) { todo!() }
