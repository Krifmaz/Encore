//! /api/remote: use any signed-in device as a remote for another (see what's playing, control it,
//! move playback between devices). Each open app checks in every couple of seconds with its state
//! and picks up commands; nothing is stored on disk. Devices that keep an event stream open
//! (`/api/remote/events`) get commands and the other devices' state pushed the moment they change,
//! so a remote feels instant instead of waiting for the next check-in.
//! The one thing kept on disk is each user's last playback spot (`/api/remote/spot`), written from the
//! check-ins, so any device can pick up where another one left off.
use crate::auth::User;
use crate::db::{now, RowExt};
use crate::error::{err, ApiResult};
use crate::p;
use crate::state::AppState;
use axum::extract::{Path, Query, State};
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
        .route("/api/remote/spot", get(spot).delete(clear_spot))
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
/// "shuffle" {value: bool} shuffles the rest of the queue (false puts the order back); "repeat" {value: "off"|"all"|"one"}.
/// A device's check-in state carries `shuffle` and `repeat` the same way.
const ACTIONS: [&str; 14] = ["toggle", "play", "pause", "next", "prev", "seek", "volume", "play_queue", "enqueue", "play_next", "stop", "engine", "shuffle", "repeat"];

/// The last spot written per user, so check-ins only touch the database every SPOT_EVERY seconds
/// (and at once when the song changes or playback stops or starts).
struct Last {
    at: f64,
    track: i64,
    playing: bool,
    device: String,
}
static SPOT_LAST: LazyLock<Mutex<HashMap<i64, Last>>> = LazyLock::new(Default::default);
const SPOT_EVERY: f64 = 10.0;
const SPOT_QUEUE_MAX: usize = 300;

/// Whether a check-in should write the spot. A playing device writes on its first check-in, on a song
/// change and every SPOT_EVERY seconds. Another playing device takes over once the last writer has
/// stopped or gone quiet for SPOT_EVERY seconds, so two devices playing at once still write at most
/// once per SPOT_EVERY between them. A paused device only writes when it is the one that wrote last and
/// it just paused or changed song, so an app that opens paused on an old song doesn't overwrite what
/// another device is playing.
fn spot_due(last: Option<&Last>, device: &str, track: i64, playing: bool, t: f64) -> bool {
    match last {
        None => playing,
        Some(l) if playing && l.device != device => !l.playing || t - l.at >= SPOT_EVERY,
        Some(l) if playing => l.track != track || !l.playing || t - l.at >= SPOT_EVERY,
        Some(l) => l.device == device && (l.playing || l.track != track),
    }
}

/// Library track ids of a check-in's queue (numbers or objects with a numeric `id`), at most
/// SPOT_QUEUE_MAX, and where the current song is in them.
fn spot_queue(state: &Value, track: i64) -> (Vec<i64>, usize) {
    let mut ids: Vec<i64> = state["queue"].as_array().map(|q| q.iter()
        .filter_map(|x| x.as_i64().or_else(|| x["id"].as_i64())).take(SPOT_QUEUE_MAX).collect()).unwrap_or_default();
    let at = state["index"].as_u64().map(|i| i as usize).filter(|&i| ids.get(i) == Some(&track))
        .or_else(|| ids.iter().position(|&x| x == track));
    match at {
        Some(i) => (ids, i),
        None => {
            ids = vec![track];
            (ids, 0)
        }
    }
}

/// Writes the user's spot from a check-in when it is due (see `spot_due`).
fn record_spot(st: &AppState, user: i64, device: &str, name: &str, state: &Value) {
    // a remote for another device isn't playing anything itself; only library songs can be picked up elsewhere
    if state["remote_of"].as_str().is_some_and(|r| !r.is_empty()) {
        return;
    }
    let Some(track) = state["id"].as_i64() else { return };
    let playing = state["playing"].as_bool().unwrap_or(false);
    let t = now();
    {
        let mut last = SPOT_LAST.lock().unwrap();
        if !spot_due(last.get(&user), device, track, playing, t) {
            return;
        }
        last.insert(user, Last { at: t, track, playing, device: device.to_string() });
    }
    let (queue, idx) = spot_queue(state, track);
    let position = state["position"].as_f64().filter(|p| p.is_finite() && *p >= 0.0).unwrap_or(0.0);
    let r = st.db.execute(
        "INSERT INTO playback_spot (user_id, track_id, position, queue, idx, device_id, device_name, playing, updated_at) VALUES (?,?,?,?,?,?,?,?,?)
         ON CONFLICT(user_id) DO UPDATE SET track_id=excluded.track_id, position=excluded.position, queue=excluded.queue, idx=excluded.idx,
         device_id=excluded.device_id, device_name=excluded.device_name, playing=excluded.playing, updated_at=excluded.updated_at",
        p![user, track, position, json!(queue).to_string(), idx, device, name, playing, t],
    );
    if let Err(e) = r {
        tracing::warn!("could not save the playback spot: {e}");
        SPOT_LAST.lock().unwrap().remove(&user);
    }
}

#[derive(Deserialize)]
struct Beat {
    id: String,
    name: String,
    #[serde(default)]
    state: Value,
}

/// The Cadenza engine built into the desktop app checks in like a device, but it is not one: it is that
/// computer's own output path (0.27 makes it a Playback switch). Until then it is kept out of every device
/// list, so nobody can send music to it or link it. A standalone Cadenza on the network stays listed.
fn is_embedded_engine(id: &str, d: &Device) -> bool {
    id.starts_with("cadenza-desktop-") || d.state["engine"]["embedded"].as_bool() == Some(true)
}

fn online(hub: &Hub, t: f64) -> Vec<Value> {
    let mut list: Vec<Value> = hub.devices.iter().filter(|(id, d)| t - d.seen < ALIVE && !d.name.is_empty() && !is_embedded_engine(id, d))
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

async fn heartbeat(State(st): State<AppState>, User(u): User, Json(b): Json<Beat>) -> ApiResult<Json<Value>> {
    if b.id.is_empty() || b.id.len() > 64 {
        return Err(err(400, "Bad device id"));
    }
    let t = now();
    let user = u.i("id");
    let name: String = b.name.chars().take(60).collect();
    record_spot(&st, user, &b.id, &name, &b.state);
    let mut all = HUBS.lock().unwrap();
    let hub = all.entry(user).or_default();
    hub.devices.retain(|_, d| t - d.seen < 600.0 || d.push.as_ref().is_some_and(|p| !p.is_closed()));
    let d = hub.devices.entry(b.id).or_insert(Device { name: String::new(), state: Value::Null, seen: t, commands: vec![], push: None });
    d.name = name;
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

/// The user's last playback spot with its song, and whether the device that wrote it is online now
/// (and playing, going by its latest check-in) – or null.
async fn spot(State(st): State<AppState>, User(u): User) -> ApiResult<Json<Value>> {
    let user = u.i("id");
    let Some(mut row) = st.db.one(
        "SELECT s.track_id, s.position, s.queue, s.idx, s.device_id, s.device_name, s.playing, s.updated_at,
                t.title, t.artist, t.album, t.album_id, t.duration
         FROM playback_spot s JOIN tracks t ON t.id = s.track_id WHERE s.user_id = ?",
        p![user],
    )? else {
        return Ok(Json(Value::Null));
    };
    let queue: Value = serde_json::from_str(&row.s("queue")).unwrap_or_else(|_| json!([]));
    let device = row.s("device_id");
    let t = now();
    let live = {
        let all = HUBS.lock().unwrap();
        all.get(&user).and_then(|h| h.devices.get(&device)).filter(|d| t - d.seen < ALIVE)
            .map(|d| d.state["playing"].as_bool().unwrap_or(false))
    };
    let index = row.i("idx");
    let playing = row.b("playing");
    row.remove("idx");
    row.insert("index".into(), json!(index));
    row.insert("queue".into(), queue);
    row.insert("playing".into(), json!(playing));
    row.insert("online".into(), json!(live.is_some()));
    row.insert("device_playing".into(), json!(live.unwrap_or(false)));
    Ok(Json(Value::Object(row)))
}

async fn clear_spot(State(st): State<AppState>, User(u): User) -> ApiResult<Json<Value>> {
    let user = u.i("id");
    st.db.execute("DELETE FROM playback_spot WHERE user_id = ?", p![user])?;
    SPOT_LAST.lock().unwrap().remove(&user);
    Ok(Json(json!({ "ok": true })))
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
#[path = "remote_tests.rs"]
mod spot_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spot_is_written_on_changes_and_otherwise_every_ten_seconds() {
        let l = Last { at: 100.0, track: 1, playing: true, device: "a".into() };
        assert!(spot_due(None, "a", 1, true, 0.0));
        assert!(!spot_due(None, "a", 1, false, 0.0)); // an app that opens paused doesn't claim the spot
        assert!(!spot_due(Some(&l), "a", 1, true, 105.0));
        assert!(spot_due(Some(&l), "a", 1, true, 110.0));
        assert!(spot_due(Some(&l), "a", 2, true, 101.0)); // song change
        assert!(spot_due(Some(&l), "a", 1, false, 101.0)); // pause
        assert!(!spot_due(Some(&l), "b", 1, true, 101.0)); // another device plays while "a" still does: waits its turn
        assert!(!spot_due(Some(&l), "b", 7, true, 101.0)); // even with a different song
        assert!(spot_due(Some(&l), "b", 1, true, 110.0)); // "a" has gone quiet
        assert!(!spot_due(Some(&l), "b", 7, false, 101.0)); // another device sits paused
        let paused = Last { playing: false, ..l };
        assert!(!spot_due(Some(&paused), "a", 1, false, 500.0));
        assert!(spot_due(Some(&paused), "a", 2, false, 101.0)); // skipped while paused
        assert!(spot_due(Some(&paused), "a", 1, true, 101.0)); // resumed
        assert!(spot_due(Some(&paused), "b", 1, true, 101.0)); // another device plays after "a" stopped
    }

    #[test]
    fn spot_queue_keeps_library_ids_and_finds_the_current_song() {
        let s = json!({ "index": 1, "queue": [{ "id": 5 }, { "id": 6 }, { "title": "stream" }, 7] });
        assert_eq!(spot_queue(&s, 6), (vec![5, 6, 7], 1));
        // an index from before non-library songs were dropped: found by id instead
        let s = json!({ "index": 3, "queue": [{ "id": 5 }, { "title": "x" }, { "id": 6 }, { "id": 7 }] });
        assert_eq!(spot_queue(&s, 7), (vec![5, 6, 7], 2));
        assert_eq!(spot_queue(&json!({}), 9), (vec![9], 0));
        let big: Vec<i64> = (1..=400).collect();
        let (q, i) = spot_queue(&json!({ "queue": big, "index": 0 }), 1);
        assert_eq!((q.len(), i), (300, 0));
    }

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

    #[test]
    fn the_desktop_apps_built_in_engine_is_not_a_device() {
        let mut hub = Hub::default();
        let t = now();
        hub.devices.insert("cadenza-desktop-0123456789abcdef".into(), Device { name: "Cadenza on PC".into(), state: json!({}), seen: t, commands: vec![], push: None });
        hub.devices.insert("x".into(), Device { name: "Cadenza embedded".into(), state: json!({ "engine": { "embedded": true } }), seen: t, commands: vec![], push: None });
        hub.devices.insert("lan".into(), Device { name: "Cadenza on the NAS".into(), state: json!({ "engine": { "embedded": false } }), seen: t, commands: vec![], push: None });
        hub.devices.insert("p".into(), Device { name: "Phone".into(), state: json!({}), seen: t, commands: vec![], push: None });
        let names: Vec<_> = online(&hub, t).iter().map(|d| d["name"].as_str().unwrap().to_string()).collect();
        assert_eq!(names, vec!["Cadenza on the NAS", "Phone"]);
    }
}
