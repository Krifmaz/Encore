//! SOCKS5 on 127.0.0.1 for the Soulseek engine (slskd): every connection it opens to the Soulseek
//! server and to peers goes out through the VPN tunnel. A random username and password (RFC 1929)
//! keep other local programs off it; CONNECT only (BIND and UDP are refused), to IPv4 addresses or
//! domain names (looked up through the VPN's DNS server, so no lookup leaks either).
//!
//! One address can have a door ([`Vpn::set_door`]): Encore lets slskd reach the Soulseek server only
//! while one of its own connection attempts is under way, so slskd's own reconnect loop is refused here
//! ("connection refused", on loopback, in a moment) and never knocks on the server.
use super::{Vpn, VpnStream};
use std::future::Future;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use subtle::ConstantTimeEq;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::TcpListener;

/// A door on the connections to one address: asked once per connection, `Some(pass)` lets it through
/// (the pass is kept as long as the connection lasts), `None` turns it away.
pub type Door = Arc<dyn Fn() -> Option<Pass> + Send + Sync>;
/// What a door hands out for one connection; dropping it says the connection is over.
pub type Pass = Box<dyn Send + Sync>;

/// A stream that keeps its door's pass until it is dropped.
pub struct Guarded<T> {
    inner: T,
    _pass: Option<Pass>,
}

impl<T: AsyncRead + Unpin> AsyncRead for Guarded<T> {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_read(cx, buf)
    }
}

impl<T: AsyncWrite + Unpin> AsyncWrite for Guarded<T> {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

/// Where a client wants to go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    V4(SocketAddrV4),
    Domain(String, u16),
}

// RFC 1928 reply codes
const REP_OK: u8 = 0;
const REP_FAILURE: u8 = 1;
const REP_NET_UNREACHABLE: u8 = 3;
const REP_HOST_UNREACHABLE: u8 = 4;
const REP_REFUSED: u8 = 5;
const REP_TTL: u8 = 6;
const REP_COMMAND: u8 = 7;
const REP_ADDRESS_TYPE: u8 = 8;

impl Vpn {
    /// Put a door on the connections to `addr` (None takes it away).
    pub fn set_door(&self, door: Option<(SocketAddrV4, Door)>) {
        *self.0.door.lock().unwrap() = door;
    }

    /// Whether a connection to `addr` may go out now: Ok(None) with no door there, Ok(Some(pass)) when
    /// the door lets it through, Err (connection refused) when the door is shut.
    pub fn through_door(&self, addr: SocketAddr) -> io::Result<Option<Pass>> {
        let door = self.0.door.lock().unwrap().as_ref().filter(|(a, _)| SocketAddr::V4(*a) == addr).map(|(_, d)| d.clone());
        match door {
            None => Ok(None),
            Some(d) => d().map(Some).ok_or_else(|| io::Error::new(io::ErrorKind::ConnectionRefused, "Encore is not connecting to this server now")),
        }
    }

    /// The proxy for slskd.yml, once [`Vpn::socks`] has run: (port, username, password).
    pub fn socks_endpoint(&self) -> Option<(u16, String, String)> {
        self.0.socks.lock().unwrap().clone()
    }

    /// Start the proxy once (later calls return the same endpoint). It stays up for the life of the
    /// server; while the VPN is down every CONNECT is answered "network unreachable", never sent around it.
    pub async fn socks(&self) -> io::Result<(u16, String, String)> {
        if let Some(e) = self.socks_endpoint() {
            return Ok(e);
        }
        let l = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0))).await?;
        let port = l.local_addr()?.port();
        let mut cur = self.0.socks.lock().unwrap();
        if let Some(e) = cur.clone() {
            return Ok(e); // another caller won the race; this listener just closes
        }
        let ep = (port, encore_core::ids::random_alnum(24), encore_core::ids::random_alnum(32));
        *cur = Some(ep.clone());
        let (user, pass) = (ep.1.clone(), ep.2.clone());
        tokio::spawn(accept_loop(self.clone(), l, user, pass));
        Ok(ep)
    }
}

async fn accept_loop(vpn: Vpn, l: TcpListener, user: String, pass: String) {
    loop {
        match l.accept().await {
            Ok((s, _)) => {
                let (user, pass, vpn) = (user.clone(), pass.clone(), vpn.clone());
                tokio::spawn(async move {
                    let _ = s.set_nodelay(true);
                    if let Err(e) = serve(s, &user, &pass, |t| through_vpn(vpn.clone(), t)).await {
                        tracing::debug!("vpn socks: {e}");
                    }
                });
            }
            Err(e) => {
                tracing::warn!("vpn socks: accept failed: {e}");
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
        }
    }
}

/// The real connector: the tunnel, names resolved by the VPN's DNS server, through the door when the
/// address has one.
async fn through_vpn(vpn: Vpn, t: Target) -> io::Result<Guarded<VpnStream>> {
    let addr = match t {
        Target::V4(a) => SocketAddr::V4(a),
        Target::Domain(host, port) => {
            let ips = vpn.resolve(&host).await?;
            let ip = ips.into_iter().find_map(|ip| match ip { IpAddr::V4(v) => Some(v), _ => None })
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("{host} has no IPv4 address")))?;
            SocketAddr::from((ip, port))
        }
    };
    let pass = vpn.through_door(addr)?;
    Ok(Guarded { inner: vpn.connect_tcp(addr).await?, _pass: pass })
}

fn bad(m: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, m.to_string())
}

fn reply_code(e: &io::Error) -> u8 {
    match e.kind() {
        io::ErrorKind::ConnectionRefused => REP_REFUSED,
        io::ErrorKind::TimedOut => REP_TTL,
        io::ErrorKind::NotConnected | io::ErrorKind::Unsupported => REP_NET_UNREACHABLE,
        io::ErrorKind::NotFound | io::ErrorKind::HostUnreachable => REP_HOST_UNREACHABLE,
        _ => REP_FAILURE,
    }
}

/// A reply in one write: Soulseek.NET reads each part of the answer with a single read call.
async fn reply<S: AsyncWrite + Unpin>(s: &mut S, code: u8) -> io::Result<()> {
    s.write_all(&[5, code, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
    s.flush().await
}

/// One client, start to end: greeting, username/password, CONNECT through `connect`, then bytes
/// both ways until either side closes.
pub async fn serve<S, C, F, T>(mut s: S, user: &str, pass: &str, connect: C) -> io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
    C: FnOnce(Target) -> F,
    F: Future<Output = io::Result<T>>,
    T: AsyncRead + AsyncWrite + Unpin,
{
    // greeting: VER NMETHODS METHODS…
    let mut head = [0u8; 2];
    s.read_exact(&mut head).await?;
    if head[0] != 5 {
        return Err(bad("not SOCKS5"));
    }
    let mut methods = vec![0u8; head[1] as usize];
    s.read_exact(&mut methods).await?;
    if !methods.contains(&2) {
        s.write_all(&[5, 0xFF]).await?;
        return Err(bad("the client offers no username/password login"));
    }
    s.write_all(&[5, 2]).await?;
    // RFC 1929: VER ULEN UNAME PLEN PASSWD
    let mut v = [0u8; 2];
    s.read_exact(&mut v).await?;
    if v[0] != 1 {
        return Err(bad("bad login version"));
    }
    let mut u = vec![0u8; v[1] as usize];
    s.read_exact(&mut u).await?;
    let mut pl = [0u8; 1];
    s.read_exact(&mut pl).await?;
    let mut p = vec![0u8; pl[0] as usize];
    s.read_exact(&mut p).await?;
    let ok = bool::from(u.as_slice().ct_eq(user.as_bytes())) & bool::from(p.as_slice().ct_eq(pass.as_bytes()));
    if !ok {
        s.write_all(&[1, 1]).await?;
        return Err(bad("wrong proxy username or password"));
    }
    s.write_all(&[1, 0]).await?;
    // request: VER CMD RSV ATYP DST.ADDR DST.PORT
    let mut req = [0u8; 4];
    s.read_exact(&mut req).await?;
    if req[0] != 5 {
        return Err(bad("not SOCKS5"));
    }
    let target = match req[3] {
        1 => {
            let mut a = [0u8; 6];
            s.read_exact(&mut a).await?;
            Some(Target::V4(SocketAddrV4::new(Ipv4Addr::new(a[0], a[1], a[2], a[3]), u16::from_be_bytes([a[4], a[5]]))))
        }
        3 => {
            let mut n = [0u8; 1];
            s.read_exact(&mut n).await?;
            let mut name = vec![0u8; n[0] as usize + 2];
            s.read_exact(&mut name).await?;
            let port = u16::from_be_bytes([name[name.len() - 2], name[name.len() - 1]]);
            name.truncate(n[0] as usize);
            Some(Target::Domain(String::from_utf8_lossy(&name).to_string(), port))
        }
        4 => {
            let mut a = [0u8; 18];
            s.read_exact(&mut a).await?;
            None // the tunnel carries IPv4 only
        }
        _ => return Err(bad("unknown address type")),
    };
    if req[1] != 1 {
        reply(&mut s, REP_COMMAND).await?;
        return Err(bad("only CONNECT is allowed"));
    }
    let Some(target) = target else {
        reply(&mut s, REP_ADDRESS_TYPE).await?;
        return Err(bad("IPv6 is not carried by the tunnel"));
    };
    let mut out = match connect(target.clone()).await {
        Ok(o) => o,
        Err(e) => {
            reply(&mut s, reply_code(&e)).await?;
            return Err(io::Error::new(e.kind(), format!("{target:?}: {e}")));
        }
    };
    reply(&mut s, REP_OK).await?;
    match tokio::io::copy_bidirectional(&mut s, &mut out).await {
        Ok(_) => Ok(()),
        // a peer that goes away mid-transfer is normal on Soulseek
        Err(e) if matches!(e.kind(), io::ErrorKind::ConnectionReset | io::ErrorKind::BrokenPipe | io::ErrorKind::UnexpectedEof) => Ok(()),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::duplex;

    /// A client over an in-memory pipe against `serve` with a fake connector that hands back the
    /// other end of a second pipe, so what the client sends arrives "at the peer".
    #[tokio::test]
    async fn handshake_login_and_connect() {
        let (mut client, server) = duplex(4096);
        let (peer_near, mut peer_far) = duplex(4096);
        let seen = std::sync::Arc::new(std::sync::Mutex::new(None));
        let seen2 = seen.clone();
        let task = tokio::spawn(async move {
            serve(server, "user", "pass", move |t| async move {
                *seen2.lock().unwrap() = Some(t);
                Ok(peer_near)
            })
            .await
        });
        client.write_all(&[5, 2, 0, 2]).await.unwrap();
        let mut b = [0u8; 2];
        client.read_exact(&mut b).await.unwrap();
        assert_eq!(b, [5, 2], "username/password chosen over no auth");
        client.write_all(&[1, 4, b'u', b's', b'e', b'r', 4, b'p', b'a', b's', b's']).await.unwrap();
        client.read_exact(&mut b).await.unwrap();
        assert_eq!(b, [1, 0]);
        client.write_all(&[5, 1, 0, 1, 208, 76, 170, 59, 0x08, 0xDF]).await.unwrap(); // 208.76.170.59:2271
        let mut r = [0u8; 10];
        client.read_exact(&mut r).await.unwrap();
        assert_eq!(r[..2], [5, 0]);
        assert_eq!(*seen.lock().unwrap(), Some(Target::V4("208.76.170.59:2271".parse().unwrap())));
        client.write_all(b"hello").await.unwrap();
        let mut got = [0u8; 5];
        peer_far.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"hello");
        peer_far.write_all(b"back").await.unwrap();
        let mut got = [0u8; 4];
        client.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"back");
        drop(client);
        drop(peer_far);
        task.await.unwrap().unwrap();
    }

    async fn login(client: &mut tokio::io::DuplexStream, user: &[u8], pass: &[u8]) -> [u8; 2] {
        client.write_all(&[5, 1, 2]).await.unwrap();
        let mut b = [0u8; 2];
        client.read_exact(&mut b).await.unwrap();
        let mut m = vec![1, user.len() as u8];
        m.extend_from_slice(user);
        m.push(pass.len() as u8);
        m.extend_from_slice(pass);
        client.write_all(&m).await.unwrap();
        client.read_exact(&mut b).await.unwrap();
        b
    }

    fn never(_: Target) -> std::future::Ready<io::Result<tokio::io::DuplexStream>> {
        panic!("must not connect")
    }

    #[tokio::test]
    async fn refuses_wrong_password_and_no_auth() {
        let (mut client, server) = duplex(1024);
        let task = tokio::spawn(async move { serve(server, "user", "pass", never).await });
        assert_eq!(login(&mut client, b"user", b"nope").await, [1, 1]);
        assert!(task.await.unwrap().is_err());

        let (mut client, server) = duplex(1024);
        let task = tokio::spawn(async move { serve(server, "user", "pass", never).await });
        client.write_all(&[5, 1, 0]).await.unwrap();
        let mut b = [0u8; 2];
        client.read_exact(&mut b).await.unwrap();
        assert_eq!(b, [5, 0xFF], "no anonymous access");
        assert!(task.await.unwrap().is_err());
    }

    #[tokio::test]
    async fn refuses_bind_udp_and_ipv6() {
        for (req, code) in [(vec![5u8, 2, 0, 1, 1, 2, 3, 4, 0, 80], REP_COMMAND), (vec![5, 3, 0, 1, 1, 2, 3, 4, 0, 80], REP_COMMAND),
                            ([vec![5u8, 1, 0, 4], vec![0; 16], vec![0, 80]].concat(), REP_ADDRESS_TYPE)] {
            let (mut client, server) = duplex(1024);
            let task = tokio::spawn(async move { serve(server, "u", "p", never).await });
            assert_eq!(login(&mut client, b"u", b"p").await, [1, 0]);
            client.write_all(&req).await.unwrap();
            let mut r = [0u8; 10];
            client.read_exact(&mut r).await.unwrap();
            assert_eq!(r[1], code, "{req:?}");
            assert!(task.await.unwrap().is_err());
        }
    }

    #[tokio::test]
    async fn domain_names_and_connect_errors() {
        let (mut client, server) = duplex(1024);
        let task = tokio::spawn(async move {
            serve(server, "u", "p", |t| async move {
                assert_eq!(t, Target::Domain("vps.slsknet.org".into(), 2271));
                Err::<tokio::io::DuplexStream, _>(io::Error::new(io::ErrorKind::NotConnected, "the VPN is not connected"))
            })
            .await
        });
        assert_eq!(login(&mut client, b"u", b"p").await, [1, 0]);
        let mut req = vec![5, 1, 0, 3, 15];
        req.extend_from_slice(b"vps.slsknet.org");
        req.extend_from_slice(&2271u16.to_be_bytes());
        client.write_all(&req).await.unwrap();
        let mut r = [0u8; 10];
        client.read_exact(&mut r).await.unwrap();
        assert_eq!(r[1], REP_NET_UNREACHABLE, "a down tunnel is unreachable, never a direct connection");
        assert!(task.await.unwrap().is_err());
    }

    /// A door on the Soulseek server's address: shut, a CONNECT there is refused before the tunnel is
    /// even asked; open, it goes on (unreachable here: no tunnel) and the door counts it; other
    /// addresses pass by it.
    #[tokio::test]
    async fn the_door_on_one_address() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let vpn = Vpn::default();
        vpn.install(None);
        let asked = Arc::new(AtomicUsize::new(0));
        let open = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (a, o) = (asked.clone(), open.clone());
        vpn.set_door(Some(("208.76.170.59:2271".parse().unwrap(), Arc::new(move || {
            a.fetch_add(1, Ordering::SeqCst);
            o.load(Ordering::SeqCst).then(|| Box::new(()) as Pass)
        }))));
        assert!(vpn.through_door("1.2.3.4:2271".parse().unwrap()).unwrap().is_none(), "no door there");
        assert_eq!(asked.load(Ordering::SeqCst), 0);
        let e = vpn.through_door("208.76.170.59:2271".parse().unwrap()).err().unwrap();
        assert_eq!(e.kind(), io::ErrorKind::ConnectionRefused);
        let (port, user, pass) = vpn.socks().await.unwrap();
        let connect = |target: [u8; 6]| {
            let (user, pass) = (user.clone(), pass.clone());
            async move {
                let mut c = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
                c.write_all(&[5, 1, 2]).await.unwrap();
                let mut b = [0u8; 2];
                c.read_exact(&mut b).await.unwrap();
                let mut m = vec![1, user.len() as u8];
                m.extend_from_slice(user.as_bytes());
                m.push(pass.len() as u8);
                m.extend_from_slice(pass.as_bytes());
                c.write_all(&m).await.unwrap();
                c.read_exact(&mut b).await.unwrap();
                c.write_all(&[&[5u8, 1, 0, 1][..], &target].concat()).await.unwrap();
                let mut r = [0u8; 10];
                c.read_exact(&mut r).await.unwrap();
                r[1]
            }
        };
        let server = [208, 76, 170, 59, 0x08, 0xDF];
        assert_eq!(connect(server).await, REP_REFUSED, "a shut door: refused");
        open.store(true, Ordering::SeqCst);
        assert_eq!(connect(server).await, REP_NET_UNREACHABLE, "an open door: on to the tunnel (down here)");
        assert_eq!(connect([1, 2, 3, 4, 0x08, 0xDF]).await, REP_NET_UNREACHABLE, "a peer: no door");
        assert_eq!(asked.load(Ordering::SeqCst), 3);
        vpn.set_door(None);
        assert!(vpn.through_door("208.76.170.59:2271".parse().unwrap()).unwrap().is_none());
    }

    /// The real server: started once, a random login, refuses CONNECT while no VPN is up.
    #[tokio::test]
    async fn real_server_without_a_tunnel_is_unreachable() {
        let vpn = Vpn::default();
        vpn.install(None);
        let (port, user, pass) = vpn.socks().await.unwrap();
        assert_eq!(vpn.socks().await.unwrap().0, port, "one server");
        assert_eq!(vpn.socks_endpoint().unwrap().0, port);
        assert!(user.len() >= 16 && pass.len() >= 16);
        let mut c = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        c.write_all(&[5, 1, 2]).await.unwrap();
        let mut b = [0u8; 2];
        c.read_exact(&mut b).await.unwrap();
        let mut m = vec![1, user.len() as u8];
        m.extend_from_slice(user.as_bytes());
        m.push(pass.len() as u8);
        m.extend_from_slice(pass.as_bytes());
        c.write_all(&m).await.unwrap();
        c.read_exact(&mut b).await.unwrap();
        assert_eq!(b, [1, 0]);
        c.write_all(&[5, 1, 0, 1, 1, 1, 1, 1, 0, 80]).await.unwrap();
        let mut r = [0u8; 10];
        c.read_exact(&mut r).await.unwrap();
        assert_eq!(r[1], REP_NET_UNREACHABLE);
    }
}
