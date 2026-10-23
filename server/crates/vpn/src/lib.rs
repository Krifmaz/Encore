//! Built-in VPN for Soulseek: a WireGuard tunnel in user space (no tun device, no NET_ADMIN, no
//! gluetun container) that only Soulseek uses – the Soulseek engine (slskd) reaches it through a
//! local SOCKS5 proxy (`socks`), and inbound peers arrive through `relay`. Everything else – Tidal,
//! Qobuz, the web UI, metadata – keeps the normal connection. With NAT-PMP (ProtonVPN) the VPN
//! forwards a port, which becomes Soulseek's listen port.
//!
//! [`Vpn`] is the one entry point: the server keeps one in its state, its supervisor keeps the tunnel in
//! step with the settings, and slskd's supervisor asks it for the proxy, the relay and the forwarded port.
pub mod config;
pub mod dns;
pub mod engine;
pub mod natpmp;
pub mod relay;
pub mod socks;
#[cfg(test)]
pub(crate) mod tests;

use encore_config::Config;
use engine::{not_connected, Engine};
pub use engine::{VpnListener, VpnStream};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::task::JoinHandle;

/// What the settings asked for last, and the tunnel that runs for it.
struct Tunnel {
    /// a hash of the settings this tunnel was made for, so an unchanged config is left alone
    key: u64,
    /// the tunnel is turned on in the settings
    enabled: bool,
    /// the config doesn't parse (shown as the error)
    error: Option<String>,
    /// the running tunnel, when there is one
    engine: Option<Arc<Engine>>,
}

/// host name → (its addresses, when looked up)
type DnsCache = HashMap<String, (Vec<IpAddr>, Instant)>;

/// The VPN of one server: the tunnel that runs, its DNS answers, the relay of the forwarded port and the
/// SOCKS5 proxy for slskd. Clones share it.
#[derive(Clone, Default)]
pub struct Vpn(Arc<Inner>);

/// The shared state behind [`Vpn`].
#[derive(Default)]
struct Inner {
    /// the tunnel for the current settings
    current: Mutex<Option<Tunnel>>,
    /// names looked up through the tunnel
    dns: Mutex<DnsCache>,
    /// the relay that runs: (port, its task)
    relay: Mutex<Option<(u16, JoinHandle<()>)>>,
    /// the SOCKS5 proxy once started: (port, username, password)
    socks: Mutex<Option<(u16, String, String)>>,
    /// the door on connections to one address (the Soulseek server), see [`socks::Door`]
    door: Mutex<Option<(SocketAddrV4, socks::Door)>>,
}

/// The settings the tunnel runs on. Not vpn_kill_switch: only Soulseek reads that (every 5 s), and
/// switching it must not tear down the tunnel and every transfer on it.
fn settings_key(cfg: &Config) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (cfg.bool("vpn_enabled"), cfg.str("vpn_wireguard_config"), cfg.str("vpn_port_forwarding")).hash(&mut h);
    h.finish()
}

/// Port forwarding (NAT-PMP) is wanted unless the setting says "off".
fn natpmp_wanted(cfg: &Config) -> bool {
    cfg.str("vpn_port_forwarding").trim().to_lowercase() != "off"
}

/// vpn_enabled and a config that parses.
pub fn enabled(cfg: &Config) -> bool {
    cfg.bool("vpn_enabled") && config::parse(&cfg.str("vpn_wireguard_config")).is_ok()
}

impl Vpn {
    /// The running tunnel, up or not.
    fn engine(&self) -> Option<Arc<Engine>> {
        self.0.current.lock().unwrap().as_ref().and_then(|v| v.engine.clone())
    }

    /// The tunnel, but only when it is up; otherwise a "not connected" error.
    fn up_engine(&self) -> io::Result<Arc<Engine>> {
        self.engine().filter(|e| e.is_up()).ok_or_else(not_connected)
    }

    /// Enabled and a handshake within the last 180 s.
    pub fn active(&self) -> bool {
        self.engine().map(|e| e.is_up()).unwrap_or(false)
    }

    /// The port the VPN forwards to us (NAT-PMP), once mapped.
    pub fn forwarded_port(&self) -> Option<u16> {
        let e = self.engine()?;
        if !e.natpmp {
            return None;
        }
        let p = e.info.lock().unwrap().forwarded_port;
        p
    }

    /// Start, restart or stop the tunnel so it matches the settings. Cheap when nothing changed.
    pub fn ensure(&self, cfg: &Config) {
        let key = settings_key(cfg);
        let mut cur = self.0.current.lock().unwrap();
        if cur.as_ref().map(|v| v.key == key).unwrap_or(false) {
            return;
        }
        if let Some(old) = cur.take() {
            if let Some(e) = old.engine {
                tracing::info!("vpn: stopping the WireGuard tunnel");
                e.shutdown();
            }
        }
        self.0.dns.lock().unwrap().clear();
        if !cfg.bool("vpn_enabled") {
            *cur = Some(Tunnel { key, enabled: false, error: None, engine: None });
            return;
        }
        let wg = match config::parse(&cfg.str("vpn_wireguard_config")) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("vpn: {e}");
                *cur = Some(Tunnel { key, enabled: true, error: Some(e), engine: None });
                return;
            }
        };
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            *cur = Some(Tunnel { key, enabled: true, error: Some("internal: no async runtime".into()), engine: None });
            return;
        };
        let natpmp = natpmp_wanted(cfg);
        tracing::info!("vpn: starting the WireGuard tunnel to {} as {}", wg.endpoint(), wg.address);
        let e = Engine::new(wg, natpmp);
        rt.spawn(start(e.clone()));
        *cur = Some(Tunnel { key, enabled: true, error: None, engine: Some(e) });
    }

    /// Keeps the tunnel in step with the settings (the server's VPN supervisor).
    pub async fn supervise(self, cfg: Config) {
        loop {
            self.ensure(&cfg);
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }

    /// The supervisor of the tunnel, for the server's list of background loops.
    pub fn supervisor(&self, cfg: Config) -> Arc<dyn encore_jobs::Supervisor> {
        let vpn = self.clone();
        encore_jobs::supervisor::looping("vpn", move || vpn.clone().supervise(cfg.clone()))
    }
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

impl Vpn {
    /// {enabled, state, endpoint, address, last_handshake_secs_ago, rx_bytes, tx_bytes, forwarded_port, public_ip, error}
    pub fn status(&self) -> Value {
        let cur = self.0.current.lock().unwrap();
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

    /// Settings → "Test VPN": apply the settings, wait up to 10 s for a handshake, map the port once.
    pub async fn test(&self, cfg: &Config) -> Value {
        self.ensure(cfg);
        for _ in 0..40 {
            match self.engine() {
                None => break,
                Some(e) if e.is_up() || e.info.lock().unwrap().error.is_some() => break,
                _ => {}
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        let natpmp = match self.engine() {
            Some(e) if e.is_up() && e.natpmp => match e.natpmp_round().await {
                Ok(port) => json!({ "ok": true, "port": port }),
                Err(err) => json!({ "ok": false, "error": err }),
            },
            Some(e) if !e.natpmp => json!({ "ok": true, "off": true }),
            _ => Value::Null,
        };
        let mut s = self.status();
        s["natpmp"] = natpmp;
        s["message"] = json!(summary(&s));
        s
    }

    /// A TCP connection through the tunnel.
    pub async fn connect_tcp(&self, addr: SocketAddr) -> io::Result<VpnStream> {
        self.up_engine()?.connect(addr).await
    }

    /// Inbound TCP on the tunnel address, `port` (the forwarded one).
    pub async fn listen_tcp(&self, port: u16) -> io::Result<VpnListener> {
        self.up_engine()?.listen(port)
    }

    /// A records over the tunnel from the config's DNS server (5 s, 2 tries, cached 10 min).
    pub async fn resolve(&self, host: &str) -> io::Result<Vec<IpAddr>> {
        let host = host.trim().trim_end_matches('.').to_lowercase();
        if let Ok(ip) = host.parse::<IpAddr>() {
            return Ok(vec![ip]);
        }
        if let Some((ips, at)) = self.0.dns.lock().unwrap().get(&host) {
            if at.elapsed() < Duration::from_secs(600) {
                return Ok(ips.clone());
            }
        }
        let e = self.up_engine()?;
        let server = SocketAddrV4::new(e.cfg.dns_v4().unwrap_or(Ipv4Addr::new(1, 1, 1, 1)), 53);
        let mut last = io::Error::new(io::ErrorKind::TimedOut, format!("no answer from the VPN's DNS server {}", server.ip()));
        for _ in 0..2 {
            let id: u16 = rand::random();
            let q = dns::query(id, &host).map_err(|m| io::Error::new(io::ErrorKind::InvalidInput, m))?;
            match e.udp_exchange(server, &q, Duration::from_secs(5)).await {
                Ok(a) => match dns::parse(&a, id) {
                    Ok((ips, _ttl)) => {
                        let ips: Vec<IpAddr> = ips.into_iter().map(IpAddr::V4).collect();
                        self.0.dns.lock().unwrap().insert(host.clone(), (ips.clone(), Instant::now()));
                        return Ok(ips);
                    }
                    Err(m) => return Err(io::Error::new(io::ErrorKind::NotFound, format!("{host}: {m}"))),
                },
                Err(err) => last = err,
            }
        }
        Err(last)
    }

    /// Tests run their own engine as the current tunnel.
    #[cfg(any(test, feature = "test-support"))]
    pub fn install(&self, e: Option<Arc<Engine>>) {
        let enabled = e.is_some();
        *self.0.current.lock().unwrap() = Some(Tunnel { key: 0, enabled, error: None, engine: e });
        self.0.dns.lock().unwrap().clear();
    }
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

