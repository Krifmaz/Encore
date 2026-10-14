//! Built-in VPN for Soulseek: a WireGuard tunnel in user space (no tun device, no NET_ADMIN, no
//! gluetun container) that only Soulseek uses – the Soulseek engine (slskd) reaches it through a
//! local SOCKS5 proxy (`socks`), and inbound peers arrive through `relay`. Everything else – Tidal,
//! Qobuz, the web UI, metadata – keeps the normal connection. With NAT-PMP (ProtonVPN) the VPN
//! forwards a port, which becomes Soulseek's listen port.
pub mod config;
pub mod dns;
pub mod engine;
pub mod natpmp;
pub mod relay;
pub mod socks;
#[cfg(test)]
pub(crate) mod tests;

use crate::state::AppState;
use engine::{not_connected, Engine};
pub use engine::{VpnListener, VpnStream};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// What the settings asked for last, and the tunnel that runs for it.
pub struct Vpn {
    key: u64,
    enabled: bool,
    /// the config doesn't parse (shown as the error)
    error: Option<String>,
    engine: Option<Arc<Engine>>,
}

static CURRENT: Mutex<Option<Vpn>> = Mutex::new(None);
static DNS_CACHE: Mutex<Option<HashMap<String, (Vec<IpAddr>, Instant)>>> = Mutex::new(None);

/// The settings the tunnel runs on. Not vpn_kill_switch: only Soulseek reads that (every 5 s), and
/// switching it must not tear down the tunnel and every transfer on it.
fn settings_key(st: &AppState) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (st.cfg.bool("vpn_enabled"), st.cfg.str("vpn_wireguard_config"), st.cfg.str("vpn_port_forwarding")).hash(&mut h);
    h.finish()
}

fn natpmp_wanted(st: &AppState) -> bool {
    st.cfg.str("vpn_port_forwarding").trim().to_lowercase() != "off"
}

fn engine() -> Option<Arc<Engine>> {
    CURRENT.lock().unwrap().as_ref().and_then(|v| v.engine.clone())
}

fn up_engine() -> io::Result<Arc<Engine>> {
    engine().filter(|e| e.is_up()).ok_or_else(not_connected)
}

/// vpn_enabled and a config that parses.
pub fn enabled(st: &AppState) -> bool {
    st.cfg.bool("vpn_enabled") && config::parse(&st.cfg.str("vpn_wireguard_config")).is_ok()
}

/// Enabled and a handshake within the last 180 s.
pub fn active() -> bool {
    engine().map(|e| e.is_up()).unwrap_or(false)
}

/// The port the VPN forwards to us (NAT-PMP), once mapped.
pub fn forwarded_port() -> Option<u16> {
    let e = engine()?;
    if !e.natpmp {
        return None;
    }
    let p = e.info.lock().unwrap().forwarded_port;
    p
}

/// Start, restart or stop the tunnel so it matches the settings. Cheap when nothing changed.
pub fn ensure(st: &AppState) {
    let key = settings_key(st);
    let mut cur = CURRENT.lock().unwrap();
    if cur.as_ref().map(|v| v.key == key).unwrap_or(false) {
        return;
    }
    if let Some(old) = cur.take() {
        if let Some(e) = old.engine {
            tracing::info!("vpn: stopping the WireGuard tunnel");
            e.shutdown();
        }
    }
    DNS_CACHE.lock().unwrap().take();
    if !st.cfg.bool("vpn_enabled") {
        *cur = Some(Vpn { key, enabled: false, error: None, engine: None });
        return;
    }
    let cfg = match config::parse(&st.cfg.str("vpn_wireguard_config")) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("vpn: {e}");
            *cur = Some(Vpn { key, enabled: true, error: Some(e), engine: None });
            return;
        }
    };
    let Ok(rt) = tokio::runtime::Handle::try_current() else {
        *cur = Some(Vpn { key, enabled: true, error: Some("internal: no async runtime".into()), engine: None });
        return;
    };
    let natpmp = natpmp_wanted(st);
    tracing::info!("vpn: starting the WireGuard tunnel to {} as {}", cfg.endpoint(), cfg.address);
    let e = Engine::new(cfg, natpmp);
    rt.spawn(start(e.clone()));
    *cur = Some(Vpn { key, enabled: true, error: None, engine: Some(e) });
}

/// Find the server with the normal resolver, open the UDP socket and run the tunnel.
async fn start(e: Arc<Engine>) {
    let peer = loop {
        if e.stopped() {
            return;
        }
        match tokio::net::lookup_host(e.cfg.endpoint()).await {
            Ok(addrs) => {
                let addrs: Vec<SocketAddr> = addrs.collect();
                if let Some(a) = addrs.iter().find(|a| a.is_ipv4()).or(addrs.first()) {
                    break *a;
                }
                e.set_error(Some(format!("Couldn't find the VPN server {}: no address", e.cfg.endpoint_host)));
            }
            Err(err) => e.set_error(Some(format!("Couldn't find the VPN server {}: {err}", e.cfg.endpoint_host))),
        }
        for _ in 0..15 {
            if e.stopped() {
                return;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    };
    let bind: SocketAddr = if peer.is_ipv4() { ([0, 0, 0, 0], 0).into() } else { (std::net::Ipv6Addr::UNSPECIFIED, 0).into() };
    let udp = match engine::udp_socket(bind) {
        Ok(u) => u,
        Err(err) => {
            e.set_error(Some(format!("Couldn't open a UDP socket for the VPN: {err}")));
            return;
        }
    };
    e.set_error(None);
    spawn_engine(e, udp, Some(peer));
}

/// Run an engine over an open socket (plus the port-forwarding loop when it is wanted).
pub(crate) fn spawn_engine(e: Arc<Engine>, udp: tokio::net::UdpSocket, peer: Option<SocketAddr>) {
    if e.natpmp && peer.is_some() {
        tokio::spawn(e.clone().natpmp_loop());
    }
    tokio::spawn(e.run(udp, peer));
}

/// Keeps the tunnel in step with the settings.
pub async fn supervisor(st: AppState) {
    loop {
        ensure(&st);
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

/// {enabled, state, endpoint, address, last_handshake_secs_ago, rx_bytes, tx_bytes, forwarded_port, public_ip, error}
pub fn status() -> Value {
    let cur = CURRENT.lock().unwrap();
    let off = json!({ "enabled": false, "state": "off", "endpoint": null, "address": null, "last_handshake_secs_ago": null, "rx_bytes": 0, "tx_bytes": 0,
        "forwarded_port": null, "public_ip": null, "port_forwarding": false, "natpmp_error": null, "error": null });
    let Some(v) = cur.as_ref() else { return off };
    if !v.enabled {
        return off;
    }
    let Some(e) = &v.engine else {
        let mut o = off;
        o["enabled"] = json!(true);
        o["state"] = json!("error");
        o["error"] = json!(v.error);
        return o;
    };
    let (state, error) = e.state();
    let i = e.info.lock().unwrap();
    json!({
        "enabled": true, "state": state, "endpoint": e.cfg.endpoint(), "address": e.cfg.address.to_string(),
        "last_handshake_secs_ago": i.last_handshake.map(|t| t.elapsed().as_secs()),
        "rx_bytes": i.rx_bytes, "tx_bytes": i.tx_bytes,
        "forwarded_port": if e.natpmp { i.forwarded_port } else { None },
        "public_ip": i.public_ip.map(|ip| ip.to_string()),
        "port_forwarding": e.natpmp,
        "natpmp_error": if e.natpmp { i.natpmp_error.clone() } else { None },
        "error": error,
    })
}

/// One line for the diagnostics card: "VPN: up · forwarded port 51234 · public IP 1.2.3.4" / "VPN: off" / "VPN: error – …".
pub fn summary(s: &Value) -> String {
    match s["state"].as_str().unwrap_or("off") {
        "up" => {
            let mut parts = vec!["VPN: up".to_string()];
            if let Some(p) = s["forwarded_port"].as_u64() {
                parts.push(format!("forwarded port {p}"));
            } else if s["port_forwarding"].as_bool().unwrap_or(false) {
                parts.push(match s["natpmp_error"].as_str() { Some(e) => format!("no forwarded port ({e})"), None => "no forwarded port yet".to_string() });
            }
            if let Some(ip) = s["public_ip"].as_str() {
                parts.push(format!("public IP {ip}"));
            }
            parts.join(" · ")
        }
        "connecting" => "VPN: connecting…".to_string(),
        "error" => format!("VPN: error – {}", s["error"].as_str().unwrap_or("unknown problem")),
        _ => "VPN: off".to_string(),
    }
}

/// Settings → "Test VPN": apply the settings, wait up to 10 s for a handshake, map the port once.
pub async fn test(st: &AppState) -> Value {
    ensure(st);
    for _ in 0..40 {
        match engine() {
            None => break,
            Some(e) if e.is_up() || e.info.lock().unwrap().error.is_some() => break,
            _ => {}
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let natpmp = match engine() {
        Some(e) if e.is_up() && e.natpmp => match e.natpmp_round().await {
            Ok(port) => json!({ "ok": true, "port": port }),
            Err(err) => json!({ "ok": false, "error": err }),
        },
        Some(e) if !e.natpmp => json!({ "ok": true, "off": true }),
        _ => Value::Null,
    };
    let mut s = status();
    s["natpmp"] = natpmp;
    s["message"] = json!(summary(&s));
    s
}

/// A TCP connection through the tunnel.
pub async fn connect_tcp(addr: SocketAddr) -> io::Result<VpnStream> {
    up_engine()?.connect(addr).await
}

/// Inbound TCP on the tunnel address, `port` (the forwarded one).
pub async fn listen_tcp(port: u16) -> io::Result<VpnListener> {
    up_engine()?.listen(port)
}

/// A records over the tunnel from the config's DNS server (5 s, 2 tries, cached 10 min).
pub async fn resolve(host: &str) -> io::Result<Vec<IpAddr>> {
    let host = host.trim().trim_end_matches('.').to_lowercase();
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Ok(vec![ip]);
    }
    if let Some((ips, at)) = DNS_CACHE.lock().unwrap().get_or_insert_with(HashMap::new).get(&host) {
        if at.elapsed() < Duration::from_secs(600) {
            return Ok(ips.clone());
        }
    }
    let e = up_engine()?;
    let server = SocketAddrV4::new(e.cfg.dns_v4().unwrap_or(Ipv4Addr::new(1, 1, 1, 1)), 53);
    let mut last = io::Error::new(io::ErrorKind::TimedOut, format!("no answer from the VPN's DNS server {}", server.ip()));
    for _ in 0..2 {
        let id: u16 = rand::random();
        let q = dns::query(id, &host).map_err(|m| io::Error::new(io::ErrorKind::InvalidInput, m))?;
        match e.udp_exchange(server, &q, Duration::from_secs(5)).await {
            Ok(a) => match dns::parse(&a, id) {
                Ok((ips, _ttl)) => {
                    let ips: Vec<IpAddr> = ips.into_iter().map(IpAddr::V4).collect();
                    DNS_CACHE.lock().unwrap().get_or_insert_with(HashMap::new).insert(host.clone(), (ips.clone(), Instant::now()));
                    return Ok(ips);
                }
                Err(m) => return Err(io::Error::new(io::ErrorKind::NotFound, format!("{host}: {m}"))),
            },
            Err(err) => last = err,
        }
    }
    Err(last)
}

/// Tests that install an engine as the current tunnel hold this, so they don't swap it under each other.
#[cfg(test)]
pub(crate) static TEST_LOCK: Mutex<()> = Mutex::new(());

/// Tests run their own engine as the current tunnel.
#[cfg(test)]
pub(crate) fn install(e: Option<Arc<Engine>>) {
    let enabled = e.is_some();
    *CURRENT.lock().unwrap() = Some(Vpn { key: 0, enabled, error: None, engine: e });
    DNS_CACHE.lock().unwrap().take();
}
