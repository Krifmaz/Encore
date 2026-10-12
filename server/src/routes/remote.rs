//! /api/remote: use any signed-in device as a remote for another (see what's playing, control it,
//! move playback between devices). Each open app checks in every couple of seconds with its state
//! and picks up commands; nothing is stored on disk. Devices that keep an event stream open
//! (`/api/remote/events`) get commands and the other devices' state pushed the moment they change,
//! so a remote feels instant instead of waiting for the next check-in.
use crate::auth::User;
use crate::db::{now, RowExt};
use crate::error::{err, ApiResult};
use crate::state::AppState;
use axum::extract::{Path, Query};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::stream::{self, Stream, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;
use tokio::sync::{broadcast, mpsc};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/remote/heartbeat", post(heartbeat))
        .route("/api/remote/devices", get(devices))
        .route("/api/remote/events", get(events))
        .route("/api/remote/{device}/command", post(command))
}

struct Device {
    name: String,
    state: Value,
    seen: f64,
    /// commands waiting for the next check-in (also pushed over the event stream when there is one)
    commands: Vec<Value>,
    /// the open event stream of this device, if any
    push: Option<mpsc::UnboundedSender<Value>>,
}

/// One user's devices plus the channel that tells their open event streams about changes.
struct Hub {
    devices: HashMap<String, Device>,
    watch: broadcast::Sender<Value>,
}

impl Default for Hub {
    fn default() -> Self {
        Hub { devices: HashMap::new(), watch: broadcast::channel(64).0 }
    }
}

/// user id → hub
static HUBS: LazyLock<Mutex<HashMap<i64, Hub>>> = LazyLock::new(Default::default);
/// every command gets a number, so a device that receives one twice (pushed and then on its check-in) runs it once
static SEQ: AtomicU64 = AtomicU64::new(1);
const ALIVE: f64 = 15.0;
const ACTIONS: [&str; 12] = ["toggle", "play", "pause", "next", "prev", "seek", "volume", "play_queue", "enqueue", "play_next", "stop", "engine"];

#[derive(Deserialize)]
struct Beat {
    id: String,
    name: String,
    #[serde(default)]
    state: Value,
}

fn online(hub: &Hub, t: f64) -> Vec<Value> {
    let mut list: Vec<Value> = hub.devices.iter().filter(|(_, d)| t - d.seen < ALIVE && !d.name.is_empty())
        .map(|(id, d)| json!({ "id": id, "name": d.name, "state": d.state, "seen": d.seen })).collect();
    // a stable order, so the list doesn't jump around between refreshes
    list.sort_by(|a, b| (a["name"].as_str(), a["id"].as_str()).cmp(&(b["name"].as_str(), b["id"].as_str())));
    list
}

/// Tell every open event stream of this user what the devices look like now.
fn announce(hub: &Hub, t: f64) {
    if hub.watch.receiver_count() > 0 {
        let _ = hub.watch.send(json!({ "type": "devices", "devices": online(hub, t) }));
    }
}

async fn heartbeat(User(u): User, Json(b): Json<Beat>) -> ApiResult<Json<Value>> {
    if b.id.is_empty() || b.id.len() > 64 {
        return Err(err(400, "Bad device id"));
    }
    let t = now();
    let mut all = HUBS.lock().unwrap();
    let hub = all.entry(u.i("id")).or_default();
    hub.devices.retain(|_, d| t - d.seen < 600.0 || d.push.as_ref().is_some_and(|p| !p.is_closed()));
    let d = hub.devices.entry(b.id).or_insert(Device { name: String::new(), state: Value::Null, seen: t, commands: vec![], push: None });
    d.name = b.name.chars().take(60).collect();
    let mut state = b.state;
    if state.to_string().len() > 64_000 {
        state["queue"] = json!([]); // keep it small
    }
    let changed = d.state != state;
    d.state = state;
    d.seen = t;
    let commands = std::mem::take(&mut d.commands);
    if changed {
        announce(hub, t);
    }
    Ok(Json(json!({ "commands": commands })))
}

async fn devices(User(u): User) -> Json<Value> {
    let t = now();
    let all = HUBS.lock().unwrap();
    Json(json!(all.get(&u.i("id")).map(|h| online(h, t)).unwrap_or_default()))
}

async fn command(User(u): User, Path(device): Path<String>, Json(mut c): Json<Value>) -> ApiResult<Json<Value>> {
    if !ACTIONS.contains(&c["action"].as_str().unwrap_or("")) {
        return Err(err(400, "Unknown action"));
    }
    let mut all = HUBS.lock().unwrap();
    let hub = all.get_mut(&u.i("id")).ok_or_else(|| err(404, "That device isn't online any more"))?;
    let d = hub.devices.get_mut(&device).filter(|d| now() - d.seen < ALIVE)
        .ok_or_else(|| err(404, "That device isn't online any more"))?;
    c["seq"] = json!(SEQ.fetch_add(1, Ordering::Relaxed));
    let mut pushed = false;
    if let Some(p) = &d.push {
        pushed = p.send(json!({ "type": "command", "command": c })).is_ok();
        if !pushed {
            d.push = None;
        }
    }
    // kept for the next check-in too: the stream may be open but the device gone (the "seq" makes it run once)
    if d.commands.len() < 20 {
        d.commands.push(c);
    }
    Ok(Json(json!({ "ok": true, "pushed": pushed })))
}

#[derive(Deserialize)]
struct EventsQ {
    device: String,
}

/// Server-sent events for one device: its commands as they are sent, and every change on the user's
/// other devices. The heartbeat keeps running beside it (liveness, and the fallback when the stream drops).
async fn events(User(u): User, Query(q): Query<EventsQ>) -> ApiResult<Sse<impl Stream<Item = Result<Event, Infallible>>>> {
    if q.device.is_empty() || q.device.len() > 64 {
        return Err(err(400, "Bad device id"));
    }
    let (tx, mut rx) = mpsc::unbounded_channel::<Value>();
    let (watch, first) = {
        let t = now();
        let mut all = HUBS.lock().unwrap();
        let hub = all.entry(u.i("id")).or_default();
        let d = hub.devices.entry(q.device.clone()).or_insert(Device { name: String::new(), state: Value::Null, seen: t, commands: vec![], push: None });
        d.push = Some(tx);
        (hub.watch.subscribe(), json!({ "type": "devices", "devices": online(hub, t) }))
    };
    let mine = stream::poll_fn(move |cx| rx.poll_recv(cx));
    let others = stream::unfold(watch, |mut w| async move {
        loop {
            match w.recv().await {
                Ok(v) => return Some((v, w)),
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    });
    let all = stream::once(async move { first }).chain(stream::select(mine, others))
        .map(|v| Ok(Event::default().event(v["type"].as_str().unwrap_or("message")).data(v.to_string())));
    Ok(Sse::new(all).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)).text("ping")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn online_hides_devices_that_never_checked_in_and_sorts_by_name() {
        let mut hub = Hub::default();
        let t = now();
        hub.devices.insert("b".into(), Device { name: "Phone".into(), state: json!({}), seen: t, commands: vec![], push: None });
        hub.devices.insert("a".into(), Device { name: "Desktop".into(), state: json!({}), seen: t, commands: vec![], push: None });
        hub.devices.insert("ghost".into(), Device { name: String::new(), state: Value::Null, seen: t, commands: vec![], push: None });
        hub.devices.insert("old".into(), Device { name: "Old".into(), state: json!({}), seen: t - 100.0, commands: vec![], push: None });
        let names: Vec<_> = online(&hub, t).iter().map(|d| d["name"].as_str().unwrap().to_string()).collect();
        assert_eq!(names, vec!["Desktop", "Phone"]);
    }
}
