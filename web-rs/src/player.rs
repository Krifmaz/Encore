//! TODO: port of web/js/player.js – see docs/ui-rewrite/CONTRACT.md
#![allow(unused_variables, dead_code)]
use serde_json::Value;
use std::rc::Rc;
#[allow(unused_imports)] use crate::json::J;

#[derive(Clone)]
pub struct Link { pub id: String, pub name: String, pub state: Value, pub online: bool, pub send: Rc<dyn Fn(Value)> }
/// Dropping it unsubscribes.
pub struct OnChange(pub Box<dyn std::any::Any>);
pub async fn shuffle_library() -> usize { todo!() }
pub fn on_change(_f: impl Fn(Option<Value>) + 'static) -> OnChange { todo!() }
pub fn current() -> Option<Value> { None }
pub fn linked() -> Option<Link> { None }
pub fn set_link(_l: Option<Link>) { todo!() }
pub fn link_state(_s: Value, _online: bool) { todo!() }
pub fn snapshot() -> Value { serde_json::json!({ "playing": false }) }
pub fn set_volume(_v: f64) { todo!() }
pub fn command(_c: Value) { todo!() }
pub fn queue() -> (Vec<Value>, usize) { (vec![], 0) }
pub fn play_queue(_tracks: Vec<Value>, _start: usize, _context: &str) { todo!() }
pub fn enqueue(_tracks: Vec<Value>) { todo!() }
pub fn play_next(_tracks: Vec<Value>) { todo!() }
pub fn preview(_t: Value) { todo!() }
pub fn can_save_offline() -> bool { crate::app::native_has("saveOffline") }
pub fn save_offline(_tracks: Vec<Value>) { todo!() }
pub fn seek_to(_sec: f64) { todo!() }
pub fn toggle() { todo!() }
pub fn next(_auto: bool) { todo!() }
pub fn prev() { todo!() }
pub fn mount() {}
pub fn set_full(_on: bool) { todo!() }
