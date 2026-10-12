//! Roon: Encore as a Roon extension. Finds the Roon Core on the LAN (SOOD multicast) or takes its address
//! from the settings, registers over Roon's MOO protocol on a WebSocket – you approve "Encore" once in
//! Roon → Settings → Extensions – then mirrors the zones (what plays where, transport state, volume), drives
//! them (play/pause/next/previous/seek/volume) and can start a search result playing in a zone through
//! Roon's Browse API. Nothing here is needed by the rest of Encore: `roon_enabled` off means this module sleeps.
use crate::state::AppState;
use anyhow::{anyhow, bail, Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;

pub const EXTENSION_ID: &str = "app.encore.roon";
/// Roon's SOOD service id – the Core answers queries that name it.
const SOOD_SERVICE_ID: &str = "00720724-5143-4a9b-abac-0e50cba674bb";
const SOOD_ADDR: &str = "239.255.90.90:9003";
const DEFAULT_PORT: u16 = 9330;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct Core {
    pub id: String,
    pub name: String,
    pub version: String,
    pub host: String,
    pub port: u16,
}

#[derive(Default)]
struct Status {
    /// off · searching · connecting · waiting_for_approval · connected · error
    phase: String,
    message: String,
    core: Option<Core>,
    since: Option<Instant>,
}

/// Everything the rest of the app reads: status, the live zones, and the session to send requests on.
struct Shared {
    status: Mutex<Status>,
    zones: Mutex<BTreeMap<String, Value>>,
    session: Mutex<Option<Session>>,
    images: Mutex<HashMap<String, (Instant, String, Vec<u8>)>>,
}
static SHARED: LazyLock<Shared> = LazyLock::new(|| Shared { status: Mutex::new(Status::default()), zones: Mutex::new(BTreeMap::new()), session: Mutex::new(None), images: Mutex::new(HashMap::new()) });

fn set_status(phase: &str, message: impl Into<String>, core: Option<Core>) {
    let mut s = SHARED.status.lock().unwrap();
    if s.phase != phase { s.since = Some(Instant::now()); }
    s.phase = phase.to_string();
    s.message = message.into();
    if core.is_some() || phase == "off" || phase == "searching" { s.core = core; }
}

// ---- MOO: Roon's little request/response protocol inside WebSocket frames ---------------------------

/// `MOO/1 <verb> <name>` + headers + optional body. Verbs: REQUEST (either way), COMPLETE (final answer),
/// CONTINUE (one of many answers to a subscription).
#[derive(Debug, Clone)]
struct Moo {
    verb: String,
    name: String,
    request_id: u32,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

impl Moo {
    fn json(&self) -> Value { serde_json::from_slice(&self.body).unwrap_or(Value::Null) }
    fn content_type(&self) -> String { self.headers.get("content-type").cloned().unwrap_or_default() }
}

fn encode(verb: &str, name: &str, id: u32, body: Option<(&str, &[u8])>) -> Vec<u8> {
    let mut out = format!("MOO/1 {verb} {name}\nRequest-Id: {id}\n").into_bytes();
    match body {
        Some((ct, b)) => {
            out.extend_from_slice(format!("Content-Length: {}\nContent-Type: {ct}\n\n", b.len()).as_bytes());
            out.extend_from_slice(b);
        }
        None => out.extend_from_slice(b"\n"),
    }
    out
}

fn decode(frame: &[u8]) -> Option<Moo> {
    let sep = frame.windows(2).position(|w| w == b"\n\n")?;
    let head = std::str::from_utf8(&frame[..sep]).ok()?;
    let body = frame[sep + 2..].to_vec();
    let mut lines = head.lines();
    let first = lines.next()?;
    let mut parts = first.splitn(3, ' ');
    let proto = parts.next()?;
    if proto != "MOO/1" { return None; }
    let verb = parts.next()?.to_string();
    let name = parts.next().unwrap_or("").to_string();
    let mut headers = HashMap::new();
    for l in lines {
        if let Some((k, v)) = l.split_once(':') { headers.insert(k.trim().to_lowercase(), v.trim().to_string()); }
    }
    let request_id = headers.get("request-id").and_then(|v| v.parse().ok()).unwrap_or(0);
    Some(Moo { verb, name, request_id, headers, body })
}

enum Waiter {
    Once(oneshot::Sender<Moo>),
    Many(mpsc::UnboundedSender<Moo>),
}

/// A connected Core: frames go out through `tx`; answers come back to whoever waits on the request id.
#[derive(Clone)]
struct Session {
    tx: mpsc::UnboundedSender<Vec<u8>>,
    waiters: Arc<Mutex<HashMap<u32, Waiter>>>,
    next_id: Arc<AtomicU32>,
}

impl Session {
    fn send_raw(&self, frame: Vec<u8>) -> Result<()> { self.tx.send(frame).map_err(|_| anyhow!("Roon connection closed")) }

    /// One request, one answer (COMPLETE).
    async fn request(&self, name: &str, body: Option<Value>) -> Result<Moo> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.waiters.lock().unwrap().insert(id, Waiter::Once(tx));
        let b = body.map(|v| v.to_string());
        self.send_raw(encode("REQUEST", name, id, b.as_deref().map(|s| ("application/json", s.as_bytes()))))?;
        let r = tokio::time::timeout(REQUEST_TIMEOUT, rx).await;
        self.waiters.lock().unwrap().remove(&id);
        let m = r.map_err(|_| anyhow!("Roon did not answer {name} in time"))?.map_err(|_| anyhow!("Roon connection closed"))?;
        if m.verb == "COMPLETE" && m.name != "Success" {
            bail!("Roon: {} – {}", m.name, m.json()["message"].as_str().unwrap_or(""));
        }
        Ok(m)
    }

    /// A subscription: every CONTINUE for this request id arrives on the returned channel.
    fn subscribe(&self, name: &str, body: Value) -> Result<mpsc::UnboundedReceiver<Moo>> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::unbounded_channel();
        self.waiters.lock().unwrap().insert(id, Waiter::Many(tx));
        let b = body.to_string();
        self.send_raw(encode("REQUEST", name, id, Some(("application/json", b.as_bytes()))))?;
        Ok(rx)
    }

    fn reply(&self, to: &Moo, name: &str, body: Option<Value>) {
        let b = body.map(|v| v.to_string());
        let _ = self.send_raw(encode("COMPLETE", name, to.request_id, b.as_deref().map(|s| ("application/json", s.as_bytes()))));
    }
}

// ---- SOOD discovery ---------------------------------------------------------------------------------

fn sood_prop(buf: &mut Vec<u8>, name: &str, value: Option<&str>) {
    buf.push(name.len() as u8);
    buf.extend_from_slice(name.as_bytes());
    match value {
        Some(v) => { buf.extend_from_slice(&(v.len() as u16).to_be_bytes()); buf.extend_from_slice(v.as_bytes()); }
        None => buf.extend_from_slice(&[0xff, 0xff]),
    }
}

fn sood_parse(pkt: &[u8]) -> Option<(char, HashMap<String, String>)> {
    if pkt.len() < 6 || &pkt[..4] != b"SOOD" || pkt[4] != 2 { return None; }
    let kind = pkt[5] as char;
    let mut props = HashMap::new();
    let mut i = 6;
    while i < pkt.len() {
        let nl = pkt[i] as usize; i += 1;
        if i + nl > pkt.len() { break; }
        let name = String::from_utf8_lossy(&pkt[i..i + nl]).to_string(); i += nl;
        if i + 2 > pkt.len() { break; }
        let vl = u16::from_be_bytes([pkt[i], pkt[i + 1]]) as usize; i += 2;
        if vl == 0xffff { continue; }
        if i + vl > pkt.len() { break; }
        props.insert(name, String::from_utf8_lossy(&pkt[i..i + vl]).to_string()); i += vl;
    }
    Some((kind, props))
}

/// Ask the LAN for Roon Cores (multicast + broadcast) and collect the answers for `wait`.
pub async fn discover(wait: Duration) -> Result<Vec<Core>> {
    let sock = tokio::net::UdpSocket::bind("0.0.0.0:0").await.context("UDP socket")?;
    sock.set_broadcast(true)?;
    let _ = sock.set_multicast_ttl_v4(2);
    let mut q = b"SOOD\x02Q".to_vec();
    sood_prop(&mut q, "query_service_id", Some(SOOD_SERVICE_ID));
    sood_prop(&mut q, "_tid", Some(&crate::auth::random_token(8)));
    let _ = sock.send_to(&q, SOOD_ADDR).await;
    let _ = sock.send_to(&q, "255.255.255.255:9003").await;
    let mut found: Vec<Core> = vec![];
    let deadline = Instant::now() + wait;
    let mut buf = [0u8; 2048];
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() { break; }
        let Ok(Ok((n, from))) = tokio::time::timeout(left, sock.recv_from(&mut buf)).await else { break };
        let Some(('R', props)) = sood_parse(&buf[..n]) else { continue };
        if props.get("service_id").map(|s| s.as_str()) != Some(SOOD_SERVICE_ID) { continue; }
        let port = props.get("http_port").and_then(|p| p.parse().ok()).unwrap_or(DEFAULT_PORT);
        let id = props.get("unique_id").cloned().unwrap_or_default();
        if found.iter().any(|c| c.id == id && !id.is_empty()) { continue; }
        found.push(Core { id, name: props.get("name").cloned().unwrap_or_else(|| "Roon Core".into()), version: props.get("display_version").cloned().unwrap_or_default(), host: from.ip().to_string(), port });
    }
    Ok(found)
}

/// The Core to talk to: the configured address, else the first one that answers on the LAN.
async fn find_core(st: &AppState) -> Result<Core> {
    let host = st.cfg.str("roon_host");
    let host = host.trim();
    if !host.is_empty() {
        let (h, p) = match host.rsplit_once(':') { Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) => (h.to_string(), p.parse().unwrap_or(DEFAULT_PORT)), _ => (host.to_string(), DEFAULT_PORT) };
        return Ok(Core { host: h.trim_start_matches("http://").trim_matches('/').to_string(), port: p, name: "Roon Core".into(), ..Default::default() });
    }
    let found = discover(Duration::from_secs(3)).await?;
    found.into_iter().next().ok_or_else(|| anyhow!("No Roon Core answered on the network. Running Encore in Docker? Multicast doesn't cross the bridge: enter the Core's address under Settings → Apps & sources → Roon."))
}

// ---- the session -----------------------------------------------------------------------------------

/// Zones as the UI wants them: a slimmed-down copy of Roon's zone objects.
fn slim_zone(z: &Value) -> Value {
    let np = &z["now_playing"];
    json!({
        "zone_id": z["zone_id"], "name": z["display_name"], "state": z["state"],
        "now_playing": if np.is_object() { json!({
            "line1": np["three_line"]["line1"], "line2": np["three_line"]["line2"], "line3": np["three_line"]["line3"],
            "image_key": np["image_key"], "length": np["length"], "seek_position": np["seek_position"],
        }) } else { Value::Null },
        "outputs": z["outputs"].as_array().map(|outs| outs.iter().map(|o| json!({ "output_id": o["output_id"], "name": o["display_name"], "volume": o["volume"] })).collect::<Vec<_>>()).unwrap_or_default(),
        "is_play_allowed": z["is_play_allowed"], "is_pause_allowed": z["is_pause_allowed"], "is_next_allowed": z["is_next_allowed"], "is_previous_allowed": z["is_previous_allowed"], "is_seek_allowed": z["is_seek_allowed"],
        "queue_items_remaining": z["queue_items_remaining"], "settings": z["settings"],
    })
}

fn apply_zone_changes(body: &Value) {
    let mut zones = SHARED.zones.lock().unwrap();
    for z in body["zones"].as_array().into_iter().flatten() { if let Some(id) = z["zone_id"].as_str() { zones.insert(id.to_string(), z.clone()); } }
    for z in body["zones_changed"].as_array().into_iter().flatten().chain(body["zones_added"].as_array().into_iter().flatten()) {
        if let Some(id) = z["zone_id"].as_str() { zones.insert(id.to_string(), z.clone()); }
    }
    for id in body["zones_removed"].as_array().into_iter().flatten() { if let Some(id) = id.as_str() { zones.remove(id); } }
    for s in body["zones_seek_changed"].as_array().into_iter().flatten() {
        if let Some(z) = s["zone_id"].as_str().and_then(|id| zones.get_mut(id)) {
            if let Some(np) = z.get_mut("now_playing").and_then(|n| n.as_object_mut()) { np.insert("seek_position".into(), s["seek_position"].clone()); }
            if let Some(o) = z.as_object_mut() { o.insert("queue_time_remaining".into(), s["queue_time_remaining"].clone()); }
        }
    }
}

async fn run_session(st: &AppState, core: Core) -> Result<()> {
    set_status("connecting", format!("Connecting to {} ({}:{})", core.name, core.host, core.port), Some(core.clone()));
    let (ws, _) = tokio::time::timeout(Duration::from_secs(10), tokio_tungstenite::connect_async(format!("ws://{}:{}/api", core.host, core.port))).await
        .map_err(|_| anyhow!("timed out connecting to {}:{}", core.host, core.port))?.context("WebSocket")?;
    let (mut sink, mut stream) = ws.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let session = Session { tx, waiters: Arc::new(Mutex::new(HashMap::new())), next_id: Arc::new(AtomicU32::new(1)) };
    let writer = tokio::spawn(async move {
        while let Some(frame) = rx.recv().await {
            if sink.send(Message::Binary(frame.into())).await.is_err() { break; }
        }
    });
    // the reader: answers to our requests go to their waiters; requests from the Core (ping) are answered here
    let s2 = session.clone();
    let (closed_tx, closed_rx) = oneshot::channel::<()>();
    let reader = tokio::spawn(async move {
        while let Some(Ok(msg)) = stream.next().await {
            let data = match msg { Message::Binary(b) => b.to_vec(), Message::Text(t) => t.as_bytes().to_vec(), Message::Close(_) => break, _ => continue };
            let Some(m) = decode(&data) else { continue };
            if m.verb == "REQUEST" {
                // the Core checks we're alive
                if m.name.ends_with("/ping") { s2.reply(&m, "Success", None); } else { s2.reply(&m, "InvalidRequest", Some(json!({ "message": "not provided" }))); }
                continue;
            }
            let waiter = { let mut w = s2.waiters.lock().unwrap(); match w.get(&m.request_id) { Some(Waiter::Once(_)) => w.remove(&m.request_id), Some(Waiter::Many(tx)) => Some(Waiter::Many(tx.clone())), None => None } };
            match waiter {
                Some(Waiter::Once(tx)) => { let _ = tx.send(m); }
                Some(Waiter::Many(tx)) => { let _ = tx.send(m); }
                None => {}
            }
        }
        let _ = closed_tx.send(());
    });
    *SHARED.session.lock().unwrap() = Some(session.clone());
    let result = drive(st, &session, core, closed_rx).await;
    *SHARED.session.lock().unwrap() = None;
    SHARED.zones.lock().unwrap().clear();
    reader.abort();
    writer.abort();
    result
}

/// Register, then follow the zones until the connection drops.
async fn drive(st: &AppState, s: &Session, mut core: Core, closed: oneshot::Receiver<()>) -> Result<()> {
    let info = s.request("com.roonlabs.registry:1/info", None).await?.json();
    core.id = info["core_id"].as_str().unwrap_or_default().to_string();
    core.name = info["display_name"].as_str().unwrap_or("Roon Core").to_string();
    core.version = info["display_version"].as_str().unwrap_or_default().to_string();
    let saved_token = if st.cfg.str("roon_core_id") == core.id { st.cfg.str("roon_token") } else { String::new() };
    let mut reg = json!({
        "extension_id": EXTENSION_ID, "display_name": st.cfg.str("app_name"), "display_version": crate::VERSION,
        "publisher": "Encore", "email": "encore@localhost", "website": "https://github.com/Krifmaz/Encore",
        "required_services": ["com.roonlabs.transport:2", "com.roonlabs.browse:1", "com.roonlabs.image:1"],
        "optional_services": [], "provided_services": ["com.roonlabs.ping:1"],
    });
    if !saved_token.is_empty() { reg["token"] = json!(saved_token); }
    set_status(if saved_token.is_empty() { "waiting_for_approval" } else { "connecting" },
        if saved_token.is_empty() { format!("Waiting for you to enable Encore in Roon → Settings → Extensions on {}", core.name) } else { format!("Registering with {}", core.name) }, Some(core.clone()));
    let mut reg_rx = s.subscribe("com.roonlabs.registry:1/register", reg)?;
    let registered = loop {
        let Some(m) = reg_rx.recv().await else { bail!("connection closed while registering") };
        match m.name.as_str() {
            "Registered" => break m.json(),
            "Unregistered" | "NotRegistered" => { set_status("waiting_for_approval", format!("Enable Encore in Roon → Settings → Extensions on {}", core.name), Some(core.clone())); }
            other if m.verb == "COMPLETE" => bail!("Roon refused the registration: {other} {}", m.json()["message"].as_str().unwrap_or("")),
            _ => {}
        }
    };
    if let Some(t) = registered["token"].as_str() {
        st.cfg.set_many(&json!({ "roon_token": t, "roon_core_id": core.id }));
    }
    set_status("connected", format!("Connected to {} {}", core.name, core.version), Some(core.clone()));
    tracing::info!("roon: registered with {} ({})", core.name, core.version);

    let mut zones_rx = s.subscribe("com.roonlabs.transport:2/subscribe_zones", json!({ "subscription_key": "encore-zones" }))?;
    let mut closed = closed;
    loop {
        tokio::select! {
            m = zones_rx.recv() => {
                let Some(m) = m else { bail!("zone subscription ended") };
                match m.name.as_str() {
                    "Subscribed" | "Changed" => apply_zone_changes(&m.json()),
                    "Unsubscribed" => bail!("Roon unsubscribed the zones"),
                    _ => {}
                }
            }
            m = reg_rx.recv() => {
                if let Some(m) = m { if m.name == "Unregistered" { bail!("Encore was disabled in Roon → Settings → Extensions"); } }
            }
            _ = &mut closed => bail!("connection closed"),
        }
    }
}

/// Keeps a session to the Core while Roon is enabled; reconnects with a growing pause.
pub async fn supervisor(st: AppState) {
    let mut backoff = 5u64;
    loop {
        if !st.cfg.bool("roon_enabled") {
            set_status("off", "Roon is off (Settings → Apps & sources → Roon)", None);
            tokio::time::sleep(Duration::from_secs(10)).await;
            backoff = 5;
            continue;
        }
        set_status("searching", "Looking for a Roon Core on the network…", None);
        let outcome = match find_core(&st).await {
            Ok(core) => run_session(&st, core).await,
            Err(e) => Err(e),
        };
        if let Err(e) = outcome {
            tracing::info!("roon: {e}");
            set_status("error", e.to_string(), None);
        }
        tokio::time::sleep(Duration::from_secs(backoff)).await;
        backoff = (backoff * 2).min(60);
    }
}

// ---- what the rest of Encore uses --------------------------------------------------------------------

pub fn status(st: &AppState) -> Value {
    let s = SHARED.status.lock().unwrap();
    json!({
        "enabled": st.cfg.bool("roon_enabled"), "phase": s.phase, "message": s.message,
        "core": s.core.as_ref().map(|c| json!({ "id": c.id, "name": c.name, "version": c.version, "host": c.host, "port": c.port })),
        "zones": SHARED.zones.lock().unwrap().len(), "since": s.since.map(|t| t.elapsed().as_secs()),
    })
}

pub fn connected() -> bool { SHARED.status.lock().unwrap().phase == "connected" }

pub fn zones() -> Vec<Value> {
    let zones = SHARED.zones.lock().unwrap();
    let mut v: Vec<Value> = zones.values().map(slim_zone).collect();
    v.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    v
}

fn session() -> Result<Session> { SHARED.session.lock().unwrap().clone().ok_or_else(|| anyhow!("Not connected to Roon")) }

/// Transport and volume for one zone. `value`: seconds for seek, 0..1 for volume.
pub async fn control(zone_id: &str, action: &str, value: Option<f64>) -> Result<()> {
    let s = session()?;
    match action {
        "play" | "pause" | "stop" | "next" => { s.request("com.roonlabs.transport:2/control", Some(json!({ "zone_or_output_id": zone_id, "control": action }))).await?; }
        "toggle" => { s.request("com.roonlabs.transport:2/control", Some(json!({ "zone_or_output_id": zone_id, "control": "playpause" }))).await?; }
        "prev" => { s.request("com.roonlabs.transport:2/control", Some(json!({ "zone_or_output_id": zone_id, "control": "previous" }))).await?; }
        "seek" => { s.request("com.roonlabs.transport:2/seek", Some(json!({ "zone_or_output_id": zone_id, "how": "absolute", "seconds": value.unwrap_or(0.0).max(0.0).round() as i64 }))).await?; }
        "volume" => {
            // 0..1 from the UI → the output's own range (Roon volumes are in dB or steps)
            let out = { let zones = SHARED.zones.lock().unwrap(); zones.get(zone_id).and_then(|z| z["outputs"].as_array().and_then(|o| o.iter().find(|o| o["volume"].is_object()).cloned())) }.ok_or_else(|| anyhow!("This zone has no volume control"))?;
            let (min, max) = (out["volume"]["min"].as_f64().unwrap_or(0.0), out["volume"]["max"].as_f64().unwrap_or(100.0));
            let v = min + (max - min) * value.unwrap_or(0.0).clamp(0.0, 1.0);
            s.request("com.roonlabs.transport:2/change_volume", Some(json!({ "output_id": out["output_id"], "how": "absolute", "value": v.round() }))).await?;
        }
        other => bail!("unknown action {other}"),
    }
    Ok(())
}

/// A cover from Roon, scaled; cached a few minutes.
pub async fn image(key: &str, size: u32) -> Result<(String, Vec<u8>)> {
    let cache_key = format!("{key}@{size}");
    if let Some((t, ct, bytes)) = SHARED.images.lock().unwrap().get(&cache_key) { if t.elapsed() < Duration::from_secs(600) { return Ok((ct.clone(), bytes.clone())); } }
    let s = session()?;
    let m = s.request("com.roonlabs.image:1/get_image", Some(json!({ "image_key": key, "scale": "fit", "width": size, "height": size, "format": "image/jpeg" }))).await?;
    let ct = { let c = m.content_type(); if c.is_empty() { "image/jpeg".to_string() } else { c } };
    let mut cache = SHARED.images.lock().unwrap();
    if cache.len() > 300 { cache.clear(); }
    cache.insert(cache_key, (Instant::now(), ct.clone(), m.body.clone()));
    Ok((ct, m.body))
}

/// Browse helper: one `browse` + `load` round trip, returning the list's items.
async fn browse_load(s: &Session, zone_id: &str, hierarchy: &str, item_key: Option<&str>, input: Option<&str>, pop_all: bool) -> Result<(Value, Vec<Value>)> {
    let mut b = json!({ "hierarchy": hierarchy, "zone_or_output_id": zone_id, "pop_all": pop_all });
    if let Some(k) = item_key { b["item_key"] = json!(k); }
    if let Some(i) = input { b["input"] = json!(i); }
    let r = s.request("com.roonlabs.browse:1/browse", Some(b)).await?.json();
    if r["action"] != "list" { return Ok((r, vec![])); }
    let l = s.request("com.roonlabs.browse:1/load", Some(json!({ "hierarchy": hierarchy, "zone_or_output_id": zone_id, "offset": 0, "count": 50 }))).await?.json();
    Ok((r, l["items"].as_array().cloned().unwrap_or_default()))
}

/// Search Roon's library for `query` and play the first album (or track) it finds in `zone_id`.
pub async fn play_search(zone_id: &str, query: &str) -> Result<String> {
    let s = session()?;
    let (_, cats) = browse_load(&s, zone_id, "search", None, Some(query), true).await?;
    // search results come as categories ("Albums", "Tracks", "Artists" …): albums first, else whatever has entries
    let pick = |want: &str| cats.iter().find(|c| c["title"].as_str().map(|t| t.eq_ignore_ascii_case(want)).unwrap_or(false) && c["hint"] == "list").cloned();
    let cat = pick("Albums").or_else(|| pick("Tracks")).or_else(|| cats.iter().find(|c| c["hint"] == "list").cloned()).ok_or_else(|| anyhow!("Roon found nothing for “{query}”"))?;
    let (_, items) = browse_load(&s, zone_id, "search", cat["item_key"].as_str(), None, false).await?;
    let first = items.iter().find(|i| i["hint"] == "list" || i["hint"] == "action_list").cloned().ok_or_else(|| anyhow!("Roon found nothing for “{query}”"))?;
    let what = format!("{}{}", first["title"].as_str().unwrap_or(""), first["subtitle"].as_str().map(|s| format!(" – {s}")).unwrap_or_default());
    // open it: an album shows "Play Album" (an action list), a track is an action list itself
    let mut key = first["item_key"].as_str().map(String::from);
    for _ in 0..3 {
        let Some(k) = key.clone() else { break };
        let (r, items) = browse_load(&s, zone_id, "search", Some(&k), None, false).await?;
        if r["action"] != "list" { return Ok(what); }
        if let Some(play) = items.iter().find(|i| i["hint"] == "action" && i["title"].as_str().map(|t| t.starts_with("Play Now") || t == "Play").unwrap_or(false)) {
            browse_load(&s, zone_id, "search", play["item_key"].as_str(), None, false).await?;
            return Ok(what);
        }
        key = items.iter().find(|i| i["hint"] == "action_list").or_else(|| items.iter().find(|i| i["hint"] == "action")).and_then(|i| i["item_key"].as_str()).map(String::from);
    }
    bail!("Roon showed “{what}” but no Play Now action")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moo_round_trip() {
        let f = encode("REQUEST", "com.roonlabs.registry:1/info", 7, Some(("application/json", br#"{"a":1}"#)));
        let m = decode(&f).unwrap();
        assert_eq!((m.verb.as_str(), m.name.as_str(), m.request_id), ("REQUEST", "com.roonlabs.registry:1/info", 7));
        assert_eq!(m.json()["a"], 1);
        let f = encode("COMPLETE", "Success", 3, None);
        let m = decode(&f).unwrap();
        assert_eq!((m.verb.as_str(), m.name.as_str(), m.request_id, m.body.len()), ("COMPLETE", "Success", 3, 0));
    }

    #[test]
    fn sood_round_trip() {
        let mut p = b"SOOD\x02R".to_vec();
        sood_prop(&mut p, "service_id", Some(SOOD_SERVICE_ID));
        sood_prop(&mut p, "http_port", Some("9330"));
        sood_prop(&mut p, "name", Some("Nucleus"));
        sood_prop(&mut p, "nothing", None);
        let (kind, props) = sood_parse(&p).unwrap();
        assert_eq!(kind, 'R');
        assert_eq!(props.get("http_port").map(String::as_str), Some("9330"));
        assert_eq!(props.get("name").map(String::as_str), Some("Nucleus"));
        assert!(!props.contains_key("nothing"));
    }

    #[test]
    fn zone_changes_apply() {
        apply_zone_changes(&json!({ "zones": [{ "zone_id": "z1", "display_name": "Kitchen", "state": "playing", "now_playing": { "seek_position": 1 } }] }));
        apply_zone_changes(&json!({ "zones_seek_changed": [{ "zone_id": "z1", "seek_position": 42 }] }));
        assert_eq!(SHARED.zones.lock().unwrap()["z1"]["now_playing"]["seek_position"], 42);
        apply_zone_changes(&json!({ "zones_removed": ["z1"] }));
        assert!(SHARED.zones.lock().unwrap().is_empty());
    }
}
