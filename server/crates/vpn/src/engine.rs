//! The tunnel itself: one tokio task owns the UDP socket to the VPN server and the WireGuard state
//! (boringtun), and drives a small TCP/IP stack (smoltcp) whose "network card" is two queues of IP
//! packets. Streams, listeners and UDP sockets share that stack behind a mutex: they read and write
//! the smoltcp socket buffers directly, leave their waker with the socket, and nudge the task, which
//! then moves the packets. smoltcp wakes readers when data arrives and writers when the window opens.
use super::config::WgConfig;
use super::natpmp;
use boringtun::noise::errors::WireGuardError;
use boringtun::noise::{Tunn, TunnResult};
use boringtun::x25519::{PublicKey, StaticSecret};
use smoltcp::iface::{Config, Interface, PollIngressSingleResult, SocketHandle, SocketSet};
use smoltcp::phy::{Device, DeviceCapabilities, Medium};
use smoltcp::socket::{tcp, udp, Socket};
use smoltcp::storage::PacketMetadata;
use smoltcp::time::Instant as SInstant;
use smoltcp::wire::{HardwareAddress, IpAddress, IpCidr, IpEndpoint};
use std::collections::{HashMap, VecDeque};
use std::future::poll_fn;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, Notify};

/// WireGuard's usual MTU: 1500 minus the outer IPv4/IPv6 + UDP + WireGuard headers.
pub const MTU: usize = 1420;
/// Per TCP socket, each way – room for Soulseek file transfers over a high-latency tunnel.
pub const TCP_BUF: usize = 256 * 1024;
/// At most this many TCP sockets at once (streams, closing orphans and listening pools).
pub const MAX_SOCKETS: usize = 512;
/// Listening sockets kept ready per port (each takes one incoming connection).
pub const LISTEN_POOL: usize = 8;
/// How often the task runs even without traffic (TCP retransmits, WireGuard timers).
const TICK: Duration = Duration::from_millis(25);
/// A tunnel counts as up while its last handshake is younger than this.
pub const HANDSHAKE_FRESH: Duration = Duration::from_secs(180);
/// The error shown when the server never answers the handshake.
pub const NO_ANSWER: &str = "No answer from the VPN server – check the Endpoint and that this WireGuard config is still active in your VPN account";

/// The error for anything tried while the tunnel is down.
pub fn not_connected() -> io::Error {
    io::Error::new(io::ErrorKind::NotConnected, "VPN is not connected")
}

/// The UDP socket to the VPN server, with roomy kernel buffers: a full TCP window arrives as a
/// burst of a few hundred datagrams, and the default buffer (~200 KiB) drops part of it.
pub fn udp_socket(bind: SocketAddr) -> io::Result<UdpSocket> {
    let s = socket2::Socket::new(socket2::Domain::for_address(bind), socket2::Type::DGRAM, Some(socket2::Protocol::UDP))?;
    let _ = s.set_recv_buffer_size(4 << 20);
    let _ = s.set_send_buffer_size(4 << 20);
    s.set_nonblocking(true)?;
    s.bind(&bind.into())?;
    UdpSocket::from_std(s.into())
}

// ---- the virtual network card -------------------------------------------------------------------

/// smoltcp's device: IP packets waiting to go into the stack and out of it.
#[derive(Default)]
struct Queues {
    /// packets from the tunnel, decrypted, for the stack to read
    rx: VecDeque<Vec<u8>>,
    /// packets from the stack, to be encrypted and sent
    tx: VecDeque<Vec<u8>>,
}

/// smoltcp receive token: one inbound packet.
struct RxTok(Vec<u8>);
/// smoltcp transmit token: pushes the packet onto the outbound queue.
struct TxTok<'a>(&'a mut VecDeque<Vec<u8>>);

impl smoltcp::phy::RxToken for RxTok {
    fn consume<R, F: FnOnce(&[u8]) -> R>(self, f: F) -> R {
        f(&self.0)
    }
}

impl smoltcp::phy::TxToken for TxTok<'_> {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R {
        let mut b = vec![0u8; len];
        let r = f(&mut b);
        self.0.push_back(b);
        r
    }
}

impl Device for Queues {
    type RxToken<'a> = RxTok;
    type TxToken<'a> = TxTok<'a>;
    fn receive(&mut self, _: SInstant) -> Option<(RxTok, TxTok<'_>)> {
        let p = self.rx.pop_front()?;
        Some((RxTok(p), TxTok(&mut self.tx)))
    }
    fn transmit(&mut self, _: SInstant) -> Option<TxTok<'_>> {
        Some(TxTok(&mut self.tx))
    }
    fn capabilities(&self) -> DeviceCapabilities {
        let mut c = DeviceCapabilities::default();
        c.medium = Medium::Ip;
        c.max_transmission_unit = MTU;
        c
    }
}

// ---- the stack ----------------------------------------------------------------------------------

/// Listening on one port: a pool of sockets waiting for a connection each.
struct Listener {
    /// tells this listener apart from a later one on the same port
    id: u64,
    /// the listening sockets, topped up as connections arrive
    pool: Vec<SocketHandle>,
    /// where accepted connections go: (socket, the far end)
    tx: mpsc::UnboundedSender<(SocketHandle, SocketAddr)>,
}

/// The userspace TCP/IP stack and everything the engine tracks in it.
struct Stack {
    /// the smoltcp interface on the tunnel address
    iface: Interface,
    /// the packet queues the interface reads and writes
    dev: Queues,
    /// every socket: TCP and UDP
    sockets: SocketSet<'static>,
    /// TCP sockets in the set (streams, orphans and listening pools)
    tcp_count: usize,
    /// the listeners by local port
    listeners: HashMap<u16, Listener>,
    /// streams dropped by their owner: closing gracefully, removed once closed (or after 30 s)
    orphans: Vec<(SocketHandle, Instant)>,
    /// TCP sockets that got the far end's FIN: once Closed they read as a clean end, the others
    /// (reset, timed out) as an error
    fin: std::collections::HashSet<SocketHandle>,
    /// the last ephemeral port handed out (49152..65535)
    next_port: u16,
    /// the last listener id handed out
    next_listener: u64,
    /// the engine has stopped: every call fails as not connected
    dead: bool,
}

/// A TCP socket with roomy buffers, no Nagle and CUBIC congestion control.
fn new_tcp() -> tcp::Socket<'static> {
    let mut s = tcp::Socket::new(tcp::SocketBuffer::new(vec![0u8; TCP_BUF]), tcp::SocketBuffer::new(vec![0u8; TCP_BUF]));
    s.set_nagle_enabled(false);
    // don't blast the whole 256 KiB window into a lossy internet path at once
    s.set_congestion_control(tcp::CongestionControl::Cubic);
    s.set_keep_alive(Some(smoltcp::time::Duration::from_secs(60)));
    s.set_timeout(Some(smoltcp::time::Duration::from_secs(180)));
    s
}

/// A smoltcp endpoint as a std address.
fn to_sockaddr(e: IpEndpoint) -> SocketAddr {
    SocketAddr::new(IpAddr::from(e.addr), e.port)
}

impl Stack {
    /// Every local port in use: listeners, TCP sockets and UDP sockets.
    fn local_ports(&self) -> std::collections::HashSet<u16> {
        let mut used: std::collections::HashSet<u16> = self.listeners.keys().copied().collect();
        for (_, s) in self.sockets.iter() {
            match s {
                Socket::Tcp(t) => {
                    if let Some(e) = t.local_endpoint() {
                        used.insert(e.port);
                    }
                }
                Socket::Udp(u) => {
                    used.insert(u.endpoint().port);
                }
                #[allow(unreachable_patterns)]
                _ => {}
            }
        }
        used
    }

    /// The next free ephemeral port (49152..65535), wrapping around.
    fn alloc_port(&mut self) -> u16 {
        let used = self.local_ports();
        for _ in 0..16384 {
            self.next_port = if self.next_port < 49152 || self.next_port == 65535 { 49152 } else { self.next_port + 1 };
            if !used.contains(&self.next_port) {
                return self.next_port;
            }
        }
        self.next_port
    }

    /// Take a TCP socket out of the set and the counts.
    fn remove_tcp(&mut self, h: SocketHandle) {
        self.fin.remove(&h);
        self.sockets.remove(h);
        self.tcp_count = self.tcp_count.saturating_sub(1);
    }

    /// A new socket listening on `port`, unless the socket limit is reached.
    fn listen_socket(&mut self, port: u16) -> Option<SocketHandle> {
        if self.tcp_count >= MAX_SOCKETS {
            return None;
        }
        let mut s = new_tcp();
        s.listen(port).ok()?;
        self.tcp_count += 1;
        Some(self.sockets.add(s))
    }

    /// Always LISTEN_POOL sockets ready for a new SYN on every listening port (half-open ones don't
    /// count). Runs after every inbound packet, so a burst of SYNs never meets an empty pool.
    fn refill_listeners(&mut self) {
        let ports: Vec<u16> = self.listeners.keys().copied().collect();
        for port in ports {
            let ready = self.listeners[&port].pool.iter().filter(|h| self.sockets.get::<tcp::Socket>(**h).state() == tcp::State::Listen).count();
            for _ in ready..LISTEN_POOL {
                match self.listen_socket(port) {
                    Some(h) => self.listeners.get_mut(&port).unwrap().pool.push(h),
                    None => break,
                }
            }
        }
    }

    /// Run the TCP/IP stack: inbound packets one by one (refilling listening pools in between),
    /// then timers and outbound.
    fn poll(&mut self, now: SInstant) {
        loop {
            let Stack { iface, dev, sockets, .. } = self;
            if iface.poll_ingress_single(now, dev, sockets) == PollIngressSingleResult::None {
                break;
            }
            if !self.listeners.is_empty() {
                self.refill_listeners();
            }
        }
        let Stack { iface, dev, sockets, .. } = self;
        iface.poll(now, dev, sockets);
        self.housekeeping();
    }

    /// Hand connected sockets from the listening pools to their listeners and refill the pools;
    /// drop orphans that have finished closing.
    fn housekeeping(&mut self) {
        // FIN is only ever seen through these states, and none of them ends before the next poll
        let Stack { sockets, fin, .. } = self;
        for (h, s) in sockets.iter() {
            if let Socket::Tcp(t) = s {
                if matches!(t.state(), tcp::State::CloseWait | tcp::State::LastAck | tcp::State::Closing | tcp::State::TimeWait) {
                    fin.insert(h);
                }
            }
        }
        let ports: Vec<u16> = self.listeners.keys().copied().collect();
        for port in ports {
            let pool = std::mem::take(&mut self.listeners.get_mut(&port).unwrap().pool);
            let mut keep = Vec::with_capacity(LISTEN_POOL);
            for h in pool {
                let s = self.sockets.get_mut::<tcp::Socket>(h);
                match s.state() {
                    tcp::State::Listen | tcp::State::SynReceived => keep.push(h),
                    tcp::State::Closed => {
                        // a half-open connection that was reset: listen again
                        if s.listen(port).is_ok() { keep.push(h) } else { self.remove_tcp(h) }
                    }
                    _ => {
                        let peer = s.remote_endpoint().map(to_sockaddr).unwrap_or_else(|| SocketAddr::from(([0, 0, 0, 0], 0)));
                        if self.listeners[&port].tx.send((h, peer)).is_err() {
                            self.sockets.get_mut::<tcp::Socket>(h).abort();
                            self.orphans.push((h, Instant::now()));
                        }
                    }
                }
            }
            self.listeners.get_mut(&port).unwrap().pool = keep;
        }
        self.refill_listeners();
        let orphans = std::mem::take(&mut self.orphans);
        for (h, since) in orphans {
            let s = self.sockets.get_mut::<tcp::Socket>(h);
            match s.state() {
                tcp::State::Closed | tcp::State::TimeWait => self.remove_tcp(h),
                _ => {
                    if since.elapsed() > Duration::from_secs(30) {
                        s.abort();
                    }
                    self.orphans.push((h, since));
                }
            }
        }
    }
}

// ---- status shared with the API -----------------------------------------------------------------

/// The tunnel's status for the API.
pub struct Info {
    /// when the engine started
    pub started: Instant,
    /// the last completed handshake
    pub last_handshake: Option<Instant>,
    /// bytes received through the tunnel
    pub rx_bytes: u64,
    /// bytes sent through the tunnel
    pub tx_bytes: u64,
    /// the resolved server address
    pub peer: Option<SocketAddr>,
    /// a problem that keeps the tunnel from working (endpoint not found, …)
    pub error: Option<String>,
    /// the public port the VPN server forwards to us (NAT-PMP)
    pub forwarded_port: Option<u16>,
    /// the public IPv4 address the VPN server reports (NAT-PMP)
    pub public_ip: Option<Ipv4Addr>,
    /// the last NAT-PMP problem, cleared on success
    pub natpmp_error: Option<String>,
}

/// One WireGuard tunnel with a TCP/IP stack on top: connections, listeners and UDP sockets.
pub struct Engine {
    /// the config it was started with
    pub cfg: WgConfig,
    /// port forwarding (NAT-PMP) is wanted
    pub natpmp: bool,
    /// the TCP/IP stack, shared with the streams
    stack: Mutex<Stack>,
    /// wakes the run loop when there is something to send
    notify: Notify,
    /// set by [`Engine::shutdown`]
    stop: AtomicBool,
    /// time zero for the stack's clock
    epoch: Instant,
    /// status for the API
    pub info: Mutex<Info>,
}

impl Engine {
    /// A new engine for `cfg`; nothing runs until [`Engine::run`].
    pub fn new(cfg: WgConfig, natpmp: bool) -> Arc<Engine> {
        let epoch = Instant::now();
        let mut dev = Queues::default();
        let mut config = Config::new(HardwareAddress::Ip);
        config.random_seed = rand::random();
        let mut iface = Interface::new(config, &mut dev, SInstant::from_micros(0));
        iface.update_ip_addrs(|a| {
            let _ = a.push(IpCidr::new(IpAddress::Ipv4(cfg.address), 32));
        });
        // everything goes into the tunnel: the "gateway" only has to exist for the route lookup
        let _ = iface.routes_mut().add_default_ipv4_route(cfg.dns_v4().unwrap_or(cfg.address));
        let stack = Stack { iface, dev, sockets: SocketSet::new(vec![]), tcp_count: 0, listeners: HashMap::new(), orphans: vec![], fin: Default::default(), next_port: 49152 + rand::random::<u16>() % 16000, next_listener: 0, dead: false };
        let info = Info { started: epoch, last_handshake: None, rx_bytes: 0, tx_bytes: 0, peer: None, error: None, forwarded_port: None, public_ip: None, natpmp_error: None };
        Arc::new(Engine { cfg, natpmp, stack: Mutex::new(stack), notify: Notify::new(), stop: AtomicBool::new(false), epoch, info: Mutex::new(info) })
    }

    /// The stack's clock: time since the engine was made.
    fn now(&self) -> SInstant {
        SInstant::from_micros(self.epoch.elapsed().as_micros() as i64)
    }

    /// The engine was told to stop.
    pub fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    /// Stop the run loop; every socket then fails.
    pub fn shutdown(&self) {
        self.stop.store(true, Ordering::Relaxed);
        self.notify.notify_one();
    }

    /// How long ago the last handshake completed.
    pub fn handshake_age(&self) -> Option<Duration> {
        self.info.lock().unwrap().last_handshake.map(|t| t.elapsed())
    }

    /// Handshake within the last 180 s.
    pub fn is_up(&self) -> bool {
        !self.stopped() && self.handshake_age().map(|a| a < HANDSHAKE_FRESH).unwrap_or(false)
    }

    /// "off" | "connecting" | "up" | "error", with the error message.
    pub fn state(&self) -> (&'static str, Option<String>) {
        if self.stopped() {
            return ("off", None);
        }
        if self.is_up() {
            return ("up", None);
        }
        let i = self.info.lock().unwrap();
        if let Some(e) = &i.error {
            return ("error", Some(e.clone()));
        }
        let (since, limit) = match i.last_handshake { Some(t) => (t, HANDSHAKE_FRESH + Duration::from_secs(30)), None => (i.started, Duration::from_secs(30)) };
        if since.elapsed() > limit { ("error", Some(NO_ANSWER.to_string())) } else { ("connecting", None) }
    }

    /// Set or clear the problem shown in the status.
    pub fn set_error(&self, e: Option<String>) {
        self.info.lock().unwrap().error = e;
    }

    /// Wake the run loop so it sends what is queued.
    fn kick(&self) {
        self.notify.notify_one();
    }

    /// Run the tunnel over `udp` until shut down. `peer` is the VPN server; None learns it from the
    /// first valid packet (the far side in tests).
    pub async fn run(self: Arc<Self>, udp: UdpSocket, mut peer: Option<SocketAddr>) {
        let secret = StaticSecret::from(self.cfg.private_key);
        let public = PublicKey::from(self.cfg.public_key);
        let keepalive = if self.cfg.keepalive > 0 && peer.is_some() { Some(self.cfg.keepalive) } else { None };
        let new_tunn = || Tunn::new(secret.clone(), public, self.cfg.preshared_key, keepalive, rand::random::<u32>() >> 8, None);
        let mut tunn = new_tunn();
        let mut buf = vec![0u8; 65536];
        let mut out = vec![0u8; 65536 + 64];
        let mut send: Vec<Vec<u8>> = vec![];
        if let Some(p) = peer {
            self.info.lock().unwrap().peer = Some(p);
            if let TunnResult::WriteToNetwork(d) = tunn.format_handshake_initiation(&mut out, false) {
                send.push(d.to_vec());
            }
        }
        let mut last_timers = Instant::now();
        while !self.stopped() {
            // 1. timers: retransmit handshakes, keepalives, rekey
            if last_timers.elapsed() >= Duration::from_millis(250) {
                last_timers = Instant::now();
                match tunn.update_timers(&mut out) {
                    TunnResult::WriteToNetwork(d) => send.push(d.to_vec()),
                    TunnResult::Err(WireGuardError::ConnectionExpired) if peer.is_some() => {
                        // gave up on this handshake round (90 s): start over, forever
                        tunn = new_tunn();
                        if let TunnResult::WriteToNetwork(d) = tunn.format_handshake_initiation(&mut out, false) {
                            send.push(d.to_vec());
                        }
                    }
                    _ => {}
                }
                // keep the handshake fresh even when only keepalives flow (they don't trigger a rekey)
                if peer.is_some() && tunn.time_since_last_handshake().map(|d| d > Duration::from_secs(150)).unwrap_or(false) {
                    if let TunnResult::WriteToNetwork(d) = tunn.format_handshake_initiation(&mut out, false) {
                        send.push(d.to_vec());
                    }
                }
            }
            // 2. the stack: run TCP/IP, then encrypt what it wants to send
            let delay = {
                let mut s = self.stack.lock().unwrap();
                let now = self.now();
                s.poll(now);
                while let Some(p) = s.dev.tx.pop_front() {
                    if peer.is_none() {
                        continue;
                    }
                    if let TunnResult::WriteToNetwork(d) = tunn.encapsulate(&p, &mut out) {
                        send.push(d.to_vec());
                    }
                }
                let Stack { iface, sockets, .. } = &mut *s;
                iface.poll_delay(self.now(), sockets).map(|d| Duration::from_micros(d.total_micros())).unwrap_or(TICK).clamp(Duration::from_millis(1), TICK)
            };
            if let Some(p) = peer {
                let mut sent = 0u64;
                for d in send.drain(..) {
                    if udp.send_to(&d, p).await.is_ok() {
                        sent += d.len() as u64;
                    }
                }
                self.info.lock().unwrap().tx_bytes += sent;
            }
            send.clear();
            // 3. wait for a datagram, a nudge from a socket, or the tick
            let first = tokio::select! {
                r = udp.recv_from(&mut buf) => r.ok(),
                _ = self.notify.notified() => None,
                _ = tokio::time::sleep(delay) => None,
            };
            let mut inbound: Vec<Vec<u8>> = vec![];
            let mut received = 0u64;
            let mut next = first;
            let mut batch = 0;
            while let Some((n, from)) = next {
                received += n as u64;
                let mut ok = false;
                match tunn.decapsulate(Some(from.ip()), &buf[..n], &mut out) {
                    TunnResult::WriteToNetwork(d) => {
                        ok = true;
                        send.push(d.to_vec());
                        // flush what was queued while the handshake was going on
                        while let TunnResult::WriteToNetwork(d) = tunn.decapsulate(None, &[], &mut out) {
                            send.push(d.to_vec());
                        }
                    }
                    TunnResult::WriteToTunnelV4(p, _) => {
                        ok = true;
                        inbound.push(p.to_vec());
                    }
                    TunnResult::WriteToTunnelV6(..) | TunnResult::Done => ok = true,
                    TunnResult::Err(e) => tracing::debug!("vpn: dropped a packet: {e:?}"),
                }
                if ok && peer.is_none() {
                    peer = Some(from);
                    self.info.lock().unwrap().peer = Some(from);
                }
                batch += 1;
                next = if batch < 256 { udp.try_recv_from(&mut buf).ok() } else { None };
            }
            {
                let mut i = self.info.lock().unwrap();
                i.rx_bytes += received;
                if let Some(d) = tunn.time_since_last_handshake() {
                    let t = Instant::now().checked_sub(d);
                    if t.is_some() && i.last_handshake.is_none() {
                        tracing::info!("vpn: WireGuard handshake with {} done – the tunnel is up", self.cfg.endpoint());
                    }
                    if t.is_some() {
                        i.last_handshake = t;
                        i.error = None;
                    }
                }
            }
            if !inbound.is_empty() {
                self.stack.lock().unwrap().dev.rx.extend(inbound);
            }
        }
        // shut down: every socket fails, every waiter wakes
        let mut s = self.stack.lock().unwrap();
        s.dead = true;
        s.listeners.clear();
        for (_, sock) in s.sockets.iter_mut() {
            match sock {
                Socket::Tcp(t) => t.abort(),
                Socket::Udp(u) => u.close(),
                #[allow(unreachable_patterns)]
                _ => {}
            }
        }
        s.dev.rx.clear();
        s.dev.tx.clear();
    }

    // ---- TCP ------------------------------------------------------------------------------------

    /// Open a TCP connection to `addr` (IPv4 only) through the tunnel.
    pub async fn connect(self: &Arc<Self>, addr: SocketAddr) -> io::Result<VpnStream> {
        let SocketAddr::V4(v4) = addr else { return Err(io::Error::new(io::ErrorKind::Unsupported, "The VPN tunnel carries IPv4 only")) };
        let handle = {
            let mut s = self.stack.lock().unwrap();
            if s.dead {
                return Err(not_connected());
            }
            if s.tcp_count >= MAX_SOCKETS {
                return Err(io::Error::other(format!("Too many connections through the VPN (at most {MAX_SOCKETS})")));
            }
            let port = s.alloc_port();
            let mut sock = new_tcp();
            sock.connect(s.iface.context(), IpEndpoint::from(v4), port).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, format!("can't connect to {addr}: {e:?}")))?;
            s.tcp_count += 1;
            s.sockets.add(sock)
        };
        self.kick();
        let stream = VpnStream { engine: self.clone(), handle, peer: addr };
        let established = poll_fn(|cx| {
            let mut s = self.stack.lock().unwrap();
            if s.dead {
                return Poll::Ready(Err(not_connected()));
            }
            let sock = s.sockets.get_mut::<tcp::Socket>(handle);
            match sock.state() {
                tcp::State::SynSent | tcp::State::SynReceived => {
                    sock.register_send_waker(cx.waker());
                    Poll::Pending
                }
                tcp::State::Closed | tcp::State::Listen => Poll::Ready(Err(io::Error::new(io::ErrorKind::ConnectionRefused, format!("{addr} refused the connection")))),
                _ => Poll::Ready(Ok(())),
            }
        });
        match tokio::time::timeout(Duration::from_secs(20), established).await {
            Ok(Ok(())) => Ok(stream),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(io::Error::new(io::ErrorKind::TimedOut, format!("connecting to {addr} through the VPN timed out"))),
        }
    }

    /// Listen for TCP connections on `port` of the tunnel address.
    pub fn listen(self: &Arc<Self>, port: u16) -> io::Result<VpnListener> {
        if port == 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "listen port 0"));
        }
        let (tx, rx) = mpsc::unbounded_channel();
        let id = {
            let mut s = self.stack.lock().unwrap();
            if s.dead {
                return Err(not_connected());
            }
            if s.listeners.get(&port).map(|l| !l.tx.is_closed()).unwrap_or(false) {
                return Err(io::Error::new(io::ErrorKind::AddrInUse, format!("port {port} is already listening on the VPN")));
            }
            if let Some(old) = s.listeners.remove(&port) {
                for h in old.pool {
                    s.remove_tcp(h);
                }
            }
            let mut pool = vec![];
            for _ in 0..LISTEN_POOL {
                match s.listen_socket(port) {
                    Some(h) => pool.push(h),
                    None => break,
                }
            }
            if pool.is_empty() {
                return Err(io::Error::other(format!("Too many connections through the VPN (at most {MAX_SOCKETS})")));
            }
            s.next_listener += 1;
            let id = s.next_listener;
            s.listeners.insert(port, Listener { id, pool, tx });
            id
        };
        self.kick();
        Ok(VpnListener { engine: self.clone(), id, port, rx: tokio::sync::Mutex::new(rx) })
    }

    // ---- UDP ------------------------------------------------------------------------------------

    /// Bind a UDP socket on `port` of the tunnel address (0 picks a free one).
    pub fn udp_bind(self: &Arc<Self>, port: u16) -> io::Result<VpnUdp> {
        let mut s = self.stack.lock().unwrap();
        if s.dead {
            return Err(not_connected());
        }
        let port = if port == 0 { s.alloc_port() } else { port };
        let buf = || udp::PacketBuffer::new(vec![PacketMetadata::EMPTY; 32], vec![0u8; 32 * 1500]);
        let mut sock = udp::Socket::new(buf(), buf());
        sock.bind(port).map_err(|e| io::Error::new(io::ErrorKind::AddrInUse, format!("udp port {port}: {e:?}")))?;
        let handle = s.sockets.add(sock);
        Ok(VpnUdp { engine: self.clone(), handle })
    }

    /// One request, one answer from `to` within `wait`.
    pub async fn udp_exchange(self: &Arc<Self>, to: SocketAddrV4, payload: &[u8], wait: Duration) -> io::Result<Vec<u8>> {
        let u = self.udp_bind(0)?;
        u.send_to(payload, to)?;
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            match tokio::time::timeout_at(deadline, u.recv_from()).await {
                Ok(Ok((d, from))) if from == SocketAddr::V4(to) => return Ok(d),
                Ok(Ok(_)) => continue,
                Ok(Err(e)) => return Err(e),
                Err(_) => return Err(io::Error::new(io::ErrorKind::TimedOut, format!("no answer from {to} through the VPN"))),
            }
        }
    }

    // ---- NAT-PMP --------------------------------------------------------------------------------

    /// One round of port forwarding: UDP + TCP mapping and the public address. Updates the status.
    pub async fn natpmp_round(self: &Arc<Self>) -> Result<u16, String> {
        let res = self.natpmp_try().await;
        let mut i = self.info.lock().unwrap();
        match &res {
            Ok((port, ip)) => {
                if i.forwarded_port != Some(*port) {
                    tracing::info!("vpn: NAT-PMP forwarded port is now {port}");
                }
                i.forwarded_port = Some(*port);
                if ip.is_some() {
                    i.public_ip = *ip;
                }
                i.natpmp_error = None;
            }
            Err(e) => {
                if i.natpmp_error.as_deref() != Some(e.as_str()) {
                    tracing::warn!("vpn: NAT-PMP port forwarding failed: {e}");
                }
                i.natpmp_error = Some(e.clone());
            }
        }
        res.map(|(p, _)| p)
    }

    /// One NAT-PMP round: map UDP and TCP, then ask for the public address. Gives the forwarded port and the address if it answered.
    async fn natpmp_try(self: &Arc<Self>) -> Result<(u16, Option<Ipv4Addr>), String> {
        let gw = self.cfg.dns_v4().ok_or("Port forwarding needs the DNS line of the WireGuard config (the NAT-PMP gateway)")?;
        let to = SocketAddrV4::new(gw, natpmp::PORT);
        let ask = |req: Vec<u8>| async move {
            let mut last = String::new();
            for _ in 0..2 {
                match self.udp_exchange(to, &req, Duration::from_secs(3)).await {
                    Ok(d) => return Ok(d),
                    Err(e) => last = e.to_string(),
                }
            }
            Err(format!("NAT-PMP gateway {gw}: {last}"))
        };
        natpmp::parse_map(&ask(natpmp::map_request(natpmp::OP_UDP, 0, 1, 60)).await?, natpmp::OP_UDP)?;
        let tcp = natpmp::parse_map(&ask(natpmp::map_request(natpmp::OP_TCP, 0, 1, 60)).await?, natpmp::OP_TCP)?;
        if tcp.external == 0 {
            return Err("the VPN server forwarded no port".into());
        }
        let ip = match ask(natpmp::address_request()).await {
            Ok(d) => natpmp::parse_address(&d).ok(),
            Err(_) => None,
        };
        Ok((tcp.external, ip))
    }

    /// Every 45 s once the tunnel is up (10 s after a failure), until the engine stops.
    pub async fn natpmp_loop(self: Arc<Self>) {
        while !self.stopped() {
            let wait = if !self.is_up() {
                1
            } else if self.natpmp_round().await.is_ok() {
                45
            } else {
                10
            };
            for _ in 0..wait {
                if self.stopped() {
                    return;
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }
}

// ---- streams --------------------------------------------------------------------------------------

/// A TCP connection through the tunnel.
pub struct VpnStream {
    /// the engine the socket lives in
    engine: Arc<Engine>,
    /// the socket in the stack
    handle: SocketHandle,
    /// the far end
    peer: SocketAddr,
}

impl VpnStream {
    /// Like TcpStream::peer_addr, for callers that log who they talk to.
    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        Ok(self.peer)
    }
}

#[cfg(test)]
impl VpnStream {
    /// Drop the connection with a reset, like a peer that went away mid-transfer.
    pub fn abort(&self) {
        let mut s = self.engine.stack.lock().unwrap();
        if !s.dead {
            s.sockets.get_mut::<tcp::Socket>(self.handle).abort();
        }
        drop(s);
        self.engine.kick();
    }
}

impl std::fmt::Debug for VpnStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "VpnStream({})", self.peer)
    }
}

impl Drop for VpnStream {
    fn drop(&mut self) {
        let mut s = self.engine.stack.lock().unwrap();
        if s.dead {
            s.remove_tcp(self.handle);
            return;
        }
        s.sockets.get_mut::<tcp::Socket>(self.handle).close();
        s.orphans.push((self.handle, Instant::now()));
        drop(s);
        self.engine.kick();
    }
}

impl AsyncRead for VpnStream {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        if buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        let mut s = self.engine.stack.lock().unwrap();
        if s.dead {
            return Poll::Ready(Err(not_connected()));
        }
        let fin = s.fin.contains(&self.handle);
        let sock = s.sockets.get_mut::<tcp::Socket>(self.handle);
        if sock.can_recv() {
            let n = sock.recv_slice(buf.initialize_unfilled()).map_err(|e| io::Error::other(format!("{e:?}")))?;
            buf.advance(n);
            drop(s);
            self.engine.kick(); // the window opened
            return Poll::Ready(Ok(()));
        }
        if !sock.may_recv() {
            // the far end closed (FIN): the end of the stream; reset or timed out: an error, like TcpStream
            return match sock.state() {
                tcp::State::TimeWait | tcp::State::CloseWait | tcp::State::LastAck | tcp::State::Closing => Poll::Ready(Ok(())),
                tcp::State::Closed if fin => Poll::Ready(Ok(())),
                tcp::State::Closed => Poll::Ready(Err(io::Error::new(io::ErrorKind::ConnectionReset, "the connection through the VPN was reset"))),
                _ => {
                    sock.register_recv_waker(cx.waker());
                    Poll::Pending
                }
            };
        }
        sock.register_recv_waker(cx.waker());
        Poll::Pending
    }
}

impl AsyncWrite for VpnStream {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, data: &[u8]) -> Poll<io::Result<usize>> {
        if data.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let mut s = self.engine.stack.lock().unwrap();
        if s.dead {
            return Poll::Ready(Err(not_connected()));
        }
        let sock = s.sockets.get_mut::<tcp::Socket>(self.handle);
        match sock.state() {
            tcp::State::SynSent | tcp::State::SynReceived => {
                sock.register_send_waker(cx.waker());
                return Poll::Pending;
            }
            _ => {}
        }
        if !sock.may_send() {
            return Poll::Ready(Err(io::Error::new(io::ErrorKind::BrokenPipe, "the connection through the VPN is closed")));
        }
        if !sock.can_send() {
            sock.register_send_waker(cx.waker());
            return Poll::Pending;
        }
        let n = sock.send_slice(data).map_err(|e| io::Error::other(format!("{e:?}")))?;
        drop(s);
        self.engine.kick();
        Poll::Ready(Ok(n))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        let mut s = self.engine.stack.lock().unwrap();
        if !s.dead {
            s.sockets.get_mut::<tcp::Socket>(self.handle).close();
        }
        drop(s);
        self.engine.kick();
        Poll::Ready(Ok(()))
    }
}

/// Inbound TCP on the tunnel address.
pub struct VpnListener {
    /// the engine the sockets live in
    engine: Arc<Engine>,
    /// this listener's id, so a drop doesn't remove a newer one on the same port
    id: u64,
    /// the local port
    port: u16,
    /// accepted connections from the run loop
    rx: tokio::sync::Mutex<mpsc::UnboundedReceiver<(SocketHandle, SocketAddr)>>,
}

impl VpnListener {
    /// Wait for the next incoming connection.
    pub async fn accept(&self) -> io::Result<(VpnStream, SocketAddr)> {
        let (handle, peer) = self.rx.lock().await.recv().await.ok_or_else(not_connected)?;
        Ok((VpnStream { engine: self.engine.clone(), handle, peer }, peer))
    }
}

impl std::fmt::Debug for VpnListener {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "VpnListener({})", self.port)
    }
}

impl Drop for VpnListener {
    fn drop(&mut self) {
        let mut s = self.engine.stack.lock().unwrap();
        // only our own registration (a newer listener on the port keeps its pool)
        let mine = s.listeners.get(&self.port).map(|l| l.id == self.id).unwrap_or(false);
        if mine {
            if let Some(l) = s.listeners.remove(&self.port) {
                for h in l.pool {
                    s.remove_tcp(h);
                }
            }
        }
        // connections accepted by the stack but never picked up
        if let Ok(mut rx) = self.rx.try_lock() {
            while let Ok((h, _)) = rx.try_recv() {
                if s.dead {
                    s.remove_tcp(h);
                } else {
                    s.sockets.get_mut::<tcp::Socket>(h).abort();
                    s.orphans.push((h, Instant::now()));
                }
            }
        }
    }
}

/// A UDP socket on the tunnel (DNS, NAT-PMP).
pub struct VpnUdp {
    /// the engine the socket lives in
    engine: Arc<Engine>,
    /// the socket in the stack
    handle: SocketHandle,
}

impl VpnUdp {
    /// Send `data` to `to`.
    pub fn send_to(&self, data: &[u8], to: SocketAddrV4) -> io::Result<()> {
        let mut s = self.engine.stack.lock().unwrap();
        if s.dead {
            return Err(not_connected());
        }
        s.sockets.get_mut::<udp::Socket>(self.handle).send_slice(data, IpEndpoint::from(to)).map_err(|e| io::Error::other(format!("{e:?}")))?;
        drop(s);
        self.engine.kick();
        Ok(())
    }

    /// Wait for the next datagram and its sender.
    pub async fn recv_from(&self) -> io::Result<(Vec<u8>, SocketAddr)> {
        poll_fn(|cx| {
            let mut s = self.engine.stack.lock().unwrap();
            if s.dead {
                return Poll::Ready(Err(not_connected()));
            }
            let sock = s.sockets.get_mut::<udp::Socket>(self.handle);
            match sock.recv() {
                Ok((d, meta)) => Poll::Ready(Ok((d.to_vec(), to_sockaddr(meta.endpoint)))),
                Err(_) => {
                    sock.register_recv_waker(cx.waker());
                    Poll::Pending
                }
            }
        })
        .await
    }
}

impl Drop for VpnUdp {
    fn drop(&mut self) {
        let mut s = self.engine.stack.lock().unwrap();
        s.sockets.remove(self.handle);
    }
}
