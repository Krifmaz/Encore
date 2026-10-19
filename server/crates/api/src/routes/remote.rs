//! /api/remote: use any signed-in device as a remote for another (see what's playing, control it,
//! move playback between devices). Each open app checks in every couple of seconds with its state
//! and picks up commands; nothing is stored on disk. Devices that keep an event stream open
//! (`/api/remote/events`) get commands and the other devices' state pushed the moment they change,
//! so a remote feels instant instead of waiting for the next check-in.
//! The one thing kept on disk is each user's last playback spot (`/api/remote/spot`), written from the
//! check-ins, so any device can pick up where another one left off.
use crate::enrich::enrich;
use crate::auth::User;
use encore_db::{now, RowExt};
use encore_core::error::{err, ApiResult};
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::stream::{self, Stream, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use encore_playback::hub::{announce, online, Device, ACTIONS, ALIVE};
use std::convert::Infallible;
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

#[derive(Deserialize)]
struct Beat {
    id: String,
    name: String,
    #[serde(default)]
    state: Value,
}

async fn heartbeat(State(st): State<AppState>, User(u): User, Json(b): Json<Beat>) -> ApiResult<Json<Value>> {
    if b.id.is_empty() || b.id.len() > 64 {
        return Err(err(400, "Bad device id"));
    }
    let t = now();
    let user = u.i("id");
    let name: String = b.name.chars().take(60).collect();
    st.playback.spots.record(&st.db, user, &b.id, &name, &b.state);
    let mut all = st.playback.devices.hubs();
    let hub = all.entry(user).or_default();
    hub.devices.retain(|_, d| t - d.seen < 600.0 || d.push.as_ref().is_some_and(|p| !p.is_closed()));
    let d = hub.devices.entry(b.id).or_insert_with(|| Device::unnamed(t));
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

async fn devices(State(st): State<AppState>, User(u): User) -> Json<Value> {
    let t = now();
    let mut v = {
        let all = st.playback.devices.hubs();
        json!(all.get(&u.i("id")).map(|h| online(h, t)).unwrap_or_default())
    };
    enrich(&st, Some(&u), &mut v).await;
    Json(v)
}

/// The user's last playback spot with its song, and whether the device that wrote it is online now
/// (and playing, going by its latest check-in) – or null.
async fn spot(State(st): State<AppState>, User(u): User) -> ApiResult<Json<Value>> {
    let mut v = encore_playback::spot::answer(&st.db, &st.playback.devices, u.i("id"))?;
    enrich(&st, Some(&u), &mut v).await;
    Ok(Json(v))
}

async fn clear_spot(State(st): State<AppState>, User(u): User) -> ApiResult<Json<Value>> {
    st.playback.spots.clear(&st.db, u.i("id"))?;
    Ok(Json(json!({ "ok": true })))
}

async fn command(State(st): State<AppState>, User(u): User, Path(device): Path<String>, Json(mut c): Json<Value>) -> ApiResult<Json<Value>> {
    if !ACTIONS.contains(&c["action"].as_str().unwrap_or("")) {
        return Err(err(400, "Unknown action"));
    }
    let mut all = st.playback.devices.hubs();
    let hub = all.get_mut(&u.i("id")).ok_or_else(|| err(404, "That device isn't online any more"))?;
    let d = hub.devices.get_mut(&device).filter(|d| now() - d.seen < ALIVE)
        .ok_or_else(|| err(404, "That device isn't online any more"))?;
    c["seq"] = json!(st.playback.devices.next_seq());
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
async fn events(State(st): State<AppState>, User(u): User, Query(q): Query<EventsQ>) -> ApiResult<Sse<impl Stream<Item = Result<Event, Infallible>>>> {
    if q.device.is_empty() || q.device.len() > 64 {
        return Err(err(400, "Bad device id"));
    }
    let (tx, mut rx) = mpsc::unbounded_channel::<Value>();
    let (watch, first) = {
        let t = now();
        let mut all = st.playback.devices.hubs();
        let hub = all.entry(u.i("id")).or_default();
        let d = hub.devices.entry(q.device.clone()).or_insert_with(|| Device::unnamed(t));
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
