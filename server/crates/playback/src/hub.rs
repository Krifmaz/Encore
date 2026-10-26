//! The devices hub behind /api/remote: every signed-in device of a user (what it plays, the commands
//! waiting for it, its open event stream) and the channel that tells the user's other devices about
//! changes. Nothing here is stored on disk. The handlers are the server's (routes/remote.rs); a sibling
//! module (group listen) may use the types here directly.
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use tokio::sync::{broadcast, mpsc};

/// A device that checked in this long ago (seconds) or less is online.
pub const ALIVE: f64 = 15.0;
/// "shuffle" {value: bool} shuffles the rest of the queue (false puts the order back); "repeat" {value: "off"|"all"|"one"}.
/// A device's check-in state carries `shuffle` and `repeat` the same way. "prefs" {norm: {mode, target, clip}} sets the
/// device's own volume normalization (0.31; its state reports them as `norm`, client-shapes §5.1).
/// "volume" {value: 0..1} is the app player's volume; "system_volume" {value: 0..1} is the device's own
/// output volume (Android's media volume, the computer's speaker volume), which a device that can reach it
/// reports as `system_volume` in its state (0 while muted; left out or null when it cannot).
pub const ACTIONS: [&str; 16] = ["toggle", "play", "pause", "next", "prev", "seek", "volume", "system_volume", "play_queue", "enqueue", "play_next", "stop", "engine", "shuffle", "repeat", "prefs"];

/// One player device of a user, as its check-ins left it.
pub struct Device {
    /// the name it shows as
    pub name: String,
    /// its last reported state (song, position, volume, …)
    pub state: Value,
    /// when it last checked in, Unix seconds
    pub seen: f64,
    /// commands waiting for the next check-in (also pushed over the event stream when there is one)
    pub commands: Vec<Value>,
    /// the open event stream of this device, if any
    pub push: Option<mpsc::UnboundedSender<Value>>,
}

impl Device {
    /// A device known by its id only (an event stream opened before its first check-in).
    pub fn unnamed(seen: f64) -> Device {
        Device { name: String::new(), state: Value::Null, seen, commands: vec![], push: None }
    }
}

/// One user's devices plus the channel that tells their open event streams about changes.
pub struct Hub {
    /// the devices by id
    pub devices: HashMap<String, Device>,
    /// tells the user's open event streams that the device list changed
    pub watch: broadcast::Sender<Value>,
}

impl Default for Hub {
    fn default() -> Self {
        Hub { devices: HashMap::new(), watch: broadcast::channel(64).0 }
    }
}

/// The Cadenza engine built into the desktop app checks in like a device, but it is not one: it is that
/// computer's own output path (0.27 makes it a Playback switch). Until then it is kept out of every device
/// list, so nobody can send music to it or link it. A standalone Cadenza on the network stays listed.
pub fn is_embedded_engine(id: &str, d: &Device) -> bool {
    id.starts_with("cadenza-desktop-") || d.state["engine"]["embedded"].as_bool() == Some(true)
}

/// The devices of a hub that are online and named, in a stable order.
pub fn online(hub: &Hub, t: f64) -> Vec<Value> {
    let mut list: Vec<Value> = hub.devices.iter().filter(|(id, d)| t - d.seen < ALIVE && !d.name.is_empty() && !is_embedded_engine(id, d))
        .map(|(id, d)| json!({ "id": id, "name": d.name, "state": d.state, "seen": d.seen })).collect();
    // a stable order, so the list doesn't jump around between refreshes
    list.sort_by(|a, b| (a["name"].as_str(), a["id"].as_str()).cmp(&(b["name"].as_str(), b["id"].as_str())));
    list
}

/// Tell every open event stream of this user what the devices look like now.
pub fn announce(hub: &Hub, t: f64) {
    if hub.watch.receiver_count() > 0 {
        let _ = hub.watch.send(json!({ "type": "devices", "devices": online(hub, t) }));
    }
}

/// Every user's hub, and the numbers commands get. The server keeps one (in [`crate::Playback`]); clones
/// share it.
#[derive(Clone)]
pub struct Devices {
    /// user id → hub
    hubs: Arc<Mutex<HashMap<i64, Hub>>>,
    /// every command gets a number, so a device that receives one twice (pushed and then on its check-in) runs it once
    seq: Arc<AtomicU64>,
}

impl Default for Devices {
    fn default() -> Self {
        Devices { hubs: Default::default(), seq: Arc::new(AtomicU64::new(1)) }
    }
}

impl Devices {
    /// Every user's hub, locked.
    pub fn hubs(&self) -> MutexGuard<'_, HashMap<i64, Hub>> {
        self.hubs.lock().unwrap()
    }

    /// The next command number.
    pub fn next_seq(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use encore_db::now;

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
