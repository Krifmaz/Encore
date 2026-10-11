//! /api/remote: use any signed-in device as a remote for another (see what's playing, control it,
//! move playback between devices). Each open app checks in every couple of seconds with its state
//! and picks up commands; nothing is stored on disk.
use crate::auth::User;
use crate::db::{now, RowExt};
use crate::error::{err, ApiResult};
use crate::state::AppState;
use axum::extract::Path;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/remote/heartbeat", post(heartbeat))
        .route("/api/remote/devices", get(devices))
        .route("/api/remote/{device}/command", post(command))
}

struct Device {
    name: String,
    state: Value,
    seen: f64,
    commands: Vec<Value>,
}

/// user id → device id → device
static DEVICES: LazyLock<Mutex<HashMap<i64, HashMap<String, Device>>>> = LazyLock::new(Default::default);
const ALIVE: f64 = 15.0;
const ACTIONS: [&str; 9] = ["toggle", "play", "pause", "next", "prev", "seek", "volume", "play_queue", "stop"];

#[derive(Deserialize)]
struct Beat {
    id: String,
    name: String,
    #[serde(default)]
    state: Value,
}

async fn heartbeat(User(u): User, Json(b): Json<Beat>) -> ApiResult<Json<Value>> {
    if b.id.is_empty() || b.id.len() > 64 {
        return Err(err(400, "Bad device id"));
    }
    let t = now();
    let mut all = DEVICES.lock().unwrap();
    let mine = all.entry(u.i("id")).or_default();
    mine.retain(|_, d| t - d.seen < 600.0);
    let d = mine.entry(b.id).or_insert(Device { name: String::new(), state: Value::Null, seen: t, commands: vec![] });
    d.name = b.name.chars().take(60).collect();
    let mut state = b.state;
    if state.to_string().len() > 64_000 {
        state["queue"] = json!([]); // keep it small
    }
    d.state = state;
    d.seen = t;
    Ok(Json(json!({ "commands": std::mem::take(&mut d.commands) })))
}

async fn devices(User(u): User) -> Json<Value> {
    let t = now();
    let all = DEVICES.lock().unwrap();
    let list: Vec<Value> = all.get(&u.i("id")).map(|m| m.iter().filter(|(_, d)| t - d.seen < ALIVE)
        .map(|(id, d)| json!({ "id": id, "name": d.name, "state": d.state, "seen": d.seen })).collect()).unwrap_or_default();
    Json(json!(list))
}

async fn command(User(u): User, Path(device): Path<String>, Json(c): Json<Value>) -> ApiResult<Json<Value>> {
    if !ACTIONS.contains(&c["action"].as_str().unwrap_or("")) {
        return Err(err(400, "Unknown action"));
    }
    let mut all = DEVICES.lock().unwrap();
    let d = all.get_mut(&u.i("id")).and_then(|m| m.get_mut(&device)).filter(|d| now() - d.seen < ALIVE)
        .ok_or_else(|| err(404, "That device isn't online any more"))?;
    if d.commands.len() < 20 {
        d.commands.push(c);
    }
    Ok(Json(json!({ "ok": true })))
}
