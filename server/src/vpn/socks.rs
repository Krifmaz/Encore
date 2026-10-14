//! SOCKS5 on 127.0.0.1 for the Soulseek engine (slskd): every connection it opens to the Soulseek
//! server and to peers goes out through the VPN tunnel. A random username and password (RFC 1929)
//! keep other local programs off it; CONNECT only (BIND and UDP are refused), to IPv4 addresses or
//! domain names (looked up through the VPN's DNS server, so no lookup leaks either).
use super::VpnStream;
use std::future::Future;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::Mutex;
use subtle::ConstantTimeEq;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;

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

/// The running server: (port, username, password).
static ENDPOINT: Mutex<Option<(u16, String, String)>> = Mutex::new(None);

/// The proxy for slskd.yml, once `start` has run.
pub fn socks_endpoint() -> Option<(u16, String, String)> {
    ENDPOINT.lock().unwrap().clone()
}

fn random_secret(n: usize) -> String {
    use rand::Rng;
    const ABC: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut r = rand::rng();
    (0..n).map(|_| ABC[r.random_range(0..ABC.len())] as char).collect()
}

/// Start the proxy once (later calls return the same endpoint). It stays up for the life of the
/// server; while the VPN is down every CONNECT is answered "network unreachable", never sent around it.
pub async fn start() -> io::Result<(u16, String, String)> {
    if let Some(e) = socks_endpoint() {
        return Ok(e);
    }
    let l = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0))).await?;
    let port = l.local_addr()?.port();
    let mut cur = ENDPOINT.lock().unwrap();
    if let Some(e) = cur.clone() {
        return Ok(e); // another caller won the race; this listener just closes
    }
    let ep = (port, random_secret(24), random_secret(32));
    *cur = Some(ep.clone());
    let (user, pass) = (ep.1.clone(), ep.2.clone());
    tokio::spawn(accept_loop(l, user, pass));
    Ok(ep)
}

async fn accept_loop(l: TcpListener, user: String, pass: String) {
    loop {
        match l.accept().await {
            Ok((s, _)) => {
                let (user, pass) = (user.clone(), pass.clone());
                tokio::spawn(async move {
                    let _ = s.set_nodelay(true);
                    if let Err(e) = serve(s, &user, &pass, through_vpn).await {
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

/// The real connector: the tunnel, names resolved by the VPN's DNS server.
async fn through_vpn(t: Target) -> io::Result<VpnStream> {
    let addr = match t {
        Target::V4(a) => SocketAddr::V4(a),
        Target::Domain(host, port) => {
            let ips = super::resolve(&host).await?;
            let ip = ips.into_iter().find_map(|ip| match ip { IpAddr::V4(v) => Some(v), _ => None })
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("{host} has no IPv4 address")))?;
            SocketAddr::from((ip, port))
        }
    };
    super::connect_tcp(addr).await
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
        let seen = std::sync::Arc::new(Mutex::new(None));
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

    /// The real server: started once, a random login, refuses CONNECT while no VPN is up.
    #[tokio::test]
    async fn real_server_without_a_tunnel_is_unreachable() {
        let _lock = super::super::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        super::super::install(None);
        let (port, user, pass) = start().await.unwrap();
        assert_eq!(start().await.unwrap().0, port, "one server");
        assert_eq!(socks_endpoint().unwrap().0, port);
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
