use super::config::{self, WgConfig};
use super::engine::Engine;
use super::{dns, natpmp};
use base64::Engine as _;
use boringtun::x25519::{PublicKey, StaticSecret};
use serde_json::json;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn b64(k: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(k)
}

fn public(k: [u8; 32]) -> [u8; 32] {
    PublicKey::from(&StaticSecret::from(k)).to_bytes()
}

const PRIV: &str = "yAnz5TF+lXXJte14tji3zlMNq+hd2rYUIgJBgB3fBmk=";
const PUB: &str = "xTIBA5rboUvnH4htodjb6e697QjLERt1NAB4mZqp8Dg=";

fn proton_sample() -> String {
    format!(
        "[Interface]\r\n# Key for encore\r\n# Bouncing = 2\r\n# NetShield = 0\r\n# Moderate NAT = off\r\n# NAT-PMP (Port Forwarding) = on\r\n# VPN Accelerator = on\r\nPrivateKey = {PRIV}\r\nAddress = 10.2.0.2/32, 2a07:b944::2:2/128\r\nDNS = 10.2.0.1, 2a07:b944::2:1\r\n\r\n[Peer]\r\n# NL-FREE#1\r\nPublicKey = {PUB}\r\nAllowedIPs = 0.0.0.0/0, ::/0\r\nEndpoint = 190.2.146.180:51820\r\n"
    )
}

#[test]
fn parses_a_proton_config() {
    let c = config::parse(&proton_sample()).unwrap();
    assert_eq!(c.address, Ipv4Addr::new(10, 2, 0, 2));
    assert_eq!(c.prefix, 32);
    assert_eq!(c.dns_v4(), Some(Ipv4Addr::new(10, 2, 0, 1)));
    assert_eq!(c.dns.len(), 2);
    assert_eq!(b64(&c.private_key), PRIV);
    assert_eq!(b64(&c.public_key), PUB);
    assert_eq!(c.endpoint_host, "190.2.146.180");
    assert_eq!(c.endpoint_port, 51820);
    assert_eq!(c.allowed_ips, vec!["0.0.0.0/0", "::/0"]);
    assert_eq!(c.keepalive, 25, "keepalive defaults to 25 s");
    assert_eq!(c.preshared_key, None);
}

#[test]
fn parses_loose_formatting() {
    // lower-case keys and sections, odd spacing, a v6 address first, trailing comments, a host name
    let text = format!(
        "  [interface]\n privatekey={PRIV}   # mine\naddress =  fd00::2/128 ,10.64.1.9/16\n\n[PEER]\nPUBLICKEY  =  {PUB}\npresharedkey = {PUB}\nendpoint = nl.example.net:1637 ; comment\npersistentkeepalive = 15\n"
    );
    let c = config::parse(&text).unwrap();
    assert_eq!(c.address, Ipv4Addr::new(10, 64, 1, 9));
    assert_eq!(c.prefix, 16);
    assert!(c.dns.is_empty());
    assert_eq!(c.endpoint(), "nl.example.net:1637");
    assert_eq!(c.keepalive, 15);
    assert!(c.preshared_key.is_some());
    let v6 = config::parse(&format!("[Interface]\nPrivateKey={PRIV}\nAddress=10.0.0.2/32\n[Peer]\nPublicKey={PUB}\nEndpoint=[2001:db8::1]:51820\n")).unwrap();
    assert_eq!(v6.endpoint_host, "2001:db8::1");
    assert_eq!(v6.endpoint(), "[2001:db8::1]:51820");
    // a whole file in one environment variable line, with literal \n
    let env = proton_sample().replace("\r\n", "\\n");
    assert!(!env.contains('\n'));
    assert_eq!(config::parse(&env).unwrap(), config::parse(&proton_sample()).unwrap());
}

#[test]
fn says_which_key_is_missing() {
    let full = proton_sample();
    let without = |key: &str| full.lines().filter(|l| !l.starts_with(key)).collect::<Vec<_>>().join("\n");
    for (key, words) in [("PrivateKey", "PrivateKey"), ("Address", "Address"), ("PublicKey", "PublicKey"), ("Endpoint", "Endpoint")] {
        let e = config::parse(&without(key)).unwrap_err();
        assert!(e.contains(words), "{key}: {e}");
    }
    assert!(config::parse("").unwrap_err().contains("paste"));
    assert!(config::parse("PrivateKey = x").unwrap_err().contains("[Interface]"));
    assert!(config::parse(&full.replace(PUB, "not-a-key")).unwrap_err().contains("PublicKey"));
    assert!(config::parse(&full.replace("Address = 10.2.0.2/32, ", "Address = ")).unwrap_err().contains("IPv4"));
    assert!(config::parse(&full.replace(":51820", "")).unwrap_err().contains("Endpoint"));
}

#[test]
fn dns_query_and_answer() {
    let q = dns::query(0x1234, "server.slsknet.org").unwrap();
    assert_eq!(&q[..12], &[0x12, 0x34, 1, 0, 0, 1, 0, 0, 0, 0, 0, 0]);
    assert_eq!(&q[12..], b"\x06server\x07slsknet\x03org\x00\x00\x01\x00\x01");
    let ips = [Ipv4Addr::new(208, 76, 170, 59), Ipv4Addr::new(1, 2, 3, 4)];
    let a = dns::answer(&q, &ips);
    let (got, ttl) = dns::parse(&a, 0x1234).unwrap();
    assert_eq!(got, ips);
    assert_eq!(ttl, 3600);
    assert!(dns::parse(&a, 0x9999).is_err(), "another query's answer");
    let mut nx = a.clone();
    nx[3] = 0x83;
    assert!(dns::parse(&nx, 0x1234).unwrap_err().contains("no such host"));
    assert!(dns::parse(&a[..a.len() - 2], 0x1234).is_err(), "truncated");
    assert!(dns::parse(&dns::answer(&q, &[]), 0x1234).is_err(), "no A records");
    assert!(dns::query(1, "bad..name").is_err());
}

#[test]
fn natpmp_requests_and_answers() {
    assert_eq!(natpmp::address_request(), vec![0, 0]);
    let r = natpmp::map_request(natpmp::OP_TCP, 0, 1, 60);
    assert_eq!(r, vec![0, 2, 0, 0, 0, 0, 0, 1, 0, 0, 0, 60]);
    let a = natpmp::answer(&r, 51234, Ipv4Addr::new(203, 0, 113, 7));
    assert_eq!(natpmp::parse_map(&a, natpmp::OP_TCP).unwrap(), natpmp::Mapping { internal: 51234, external: 51234, lifetime: 60 });
    assert!(natpmp::parse_map(&a, natpmp::OP_UDP).is_err(), "answer for another opcode");
    let ip = natpmp::answer(&natpmp::address_request(), 0, Ipv4Addr::new(203, 0, 113, 7));
    assert_eq!(natpmp::parse_address(&ip).unwrap(), Ipv4Addr::new(203, 0, 113, 7));
    let mut refused = a.clone();
    refused[3] = 2;
    assert!(natpmp::parse_map(&refused, natpmp::OP_TCP).unwrap_err().contains("refused"));
    assert!(natpmp::parse_map(&a[..12], natpmp::OP_TCP).is_err(), "short");
}

#[test]
fn diagnostics_line() {
    assert_eq!(super::summary(&json!({ "state": "off" })), "VPN: off");
    assert_eq!(super::summary(&json!({ "state": "up", "forwarded_port": 51234, "public_ip": "1.2.3.4", "port_forwarding": true })), "VPN: up · forwarded port 51234 · public IP 1.2.3.4");
    assert_eq!(super::summary(&json!({ "state": "error", "error": "boom" })), "VPN: error – boom");
}

fn v4(a: SocketAddr) -> SocketAddrV4 {
    match a {
        SocketAddr::V4(a) => a,
        _ => panic!("v4 only"),
    }
}

/// Two WireGuard peers over loopback UDP. The far side is a second engine (its own smoltcp stack)
/// running a TCP echo server, a DNS stub and a NAT-PMP stub; the near side is the real Vpn API.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn end_to_end_over_loopback() {
    let _lock = super::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (near_key, far_key): ([u8; 32], [u8; 32]) = (rand::random(), rand::random());
    let far_udp = super::engine::udp_socket(([127, 0, 0, 1], 0).into()).unwrap();
    let far_addr = far_udp.local_addr().unwrap();
    let far_ip = Ipv4Addr::new(10, 2, 0, 1);
    let far_cfg = WgConfig { private_key: far_key, address: far_ip, prefix: 32, dns: vec![], public_key: public(near_key), preshared_key: None,
        endpoint_host: "127.0.0.1".into(), endpoint_port: 1, allowed_ips: vec![], keepalive: 0 };
    let far = Engine::new(far_cfg, false);
    tokio::spawn(far.clone().run(far_udp, None));

    // echo server on 10.2.0.1:7
    let echo = far.listen(7).unwrap();
    tokio::spawn(async move {
        while let Ok((s, _)) = echo.accept().await {
            tokio::spawn(async move {
                let (mut r, mut w) = tokio::io::split(s);
                let _ = tokio::io::copy(&mut r, &mut w).await;
                let _ = w.shutdown().await;
            });
        }
    });
    // DNS stub on 10.2.0.1:53, NAT-PMP stub on 10.2.0.1:5351
    let slsk = Ipv4Addr::new(208, 76, 170, 59);
    let dns_sock = far.udp_bind(53).unwrap();
    tokio::spawn(async move {
        while let Ok((q, from)) = dns_sock.recv_from().await {
            let _ = dns_sock.send_to(&dns::answer(&q, &[slsk]), v4(from));
        }
    });
    let pmp = far.udp_bind(natpmp::PORT).unwrap();
    tokio::spawn(async move {
        while let Ok((q, from)) = pmp.recv_from().await {
            let _ = pmp.send_to(&natpmp::answer(&q, 51234, Ipv4Addr::new(203, 0, 113, 7)), v4(from));
        }
    });

    // the near side: the config as a provider would hand it out, run as the current VPN
    let text = format!("[Interface]\nPrivateKey = {}\nAddress = 10.2.0.2/32, fd00::2/128\nDNS = 10.2.0.1\n\n[Peer]\nPublicKey = {}\nAllowedIPs = 0.0.0.0/0, ::/0\nEndpoint = {far_addr}\nPersistentKeepalive = 25\n", b64(&near_key), b64(&public(far_key)));
    let near = Engine::new(config::parse(&text).unwrap(), true);
    super::install(Some(near.clone()));
    let err = super::connect_tcp(SocketAddr::from((far_ip, 7))).await.unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::NotConnected, "not up yet");
    assert_eq!(err.to_string(), "VPN is not connected");
    let near_udp = super::engine::udp_socket(([127, 0, 0, 1], 0).into()).unwrap();
    super::spawn_engine(near.clone(), near_udp, Some(far_addr));
    tokio::time::timeout(Duration::from_secs(10), async {
        while !super::active() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("handshake within 10 s");

    // 1 MiB through the echo server and back, intact
    let data: Vec<u8> = (0..1 << 20).map(|_| rand::random::<u8>()).collect();
    let s = super::connect_tcp(SocketAddr::from((far_ip, 7))).await.unwrap();
    assert_eq!(s.peer_addr().unwrap(), SocketAddr::from((far_ip, 7)));
    let (mut r, mut w) = tokio::io::split(s);
    let sent = data.clone();
    let writer = tokio::spawn(async move {
        w.write_all(&sent).await.unwrap();
        w.shutdown().await.unwrap();
        w
    });
    let mut back = Vec::new();
    tokio::time::timeout(Duration::from_secs(60), r.read_to_end(&mut back)).await.expect("echo within 60 s").unwrap();
    let _w = writer.await.unwrap();
    assert_eq!(back.len(), data.len());
    assert!(back == data, "echoed bytes differ");

    // inbound: the far side connects to our listening port
    let l = super::listen_tcp(2234).await.unwrap();
    let dial = {
        let far = far.clone();
        tokio::spawn(async move {
            let mut c = far.connect(SocketAddr::from(([10, 2, 0, 2], 2234))).await.unwrap();
            c.write_all(b"hello").await.unwrap();
            let mut b = [0u8; 5];
            c.read_exact(&mut b).await.unwrap();
            b
        })
    };
    let (mut inc, from) = tokio::time::timeout(Duration::from_secs(10), l.accept()).await.expect("accept within 10 s").unwrap();
    assert_eq!(from.ip(), IpAddr::V4(far_ip));
    let mut b = [0u8; 5];
    inc.read_exact(&mut b).await.unwrap();
    assert_eq!(&b, b"hello");
    inc.write_all(b"world").await.unwrap();
    assert_eq!(&dial.await.unwrap(), b"world");
    // more connections at once than the listening pool holds: all are accepted
    let mut dials = vec![];
    for i in 0..20u8 {
        let far = far.clone();
        dials.push(tokio::spawn(async move {
            let mut c = far.connect(SocketAddr::from(([10, 2, 0, 2], 2234))).await.unwrap();
            c.write_all(&[i]).await.unwrap();
            let mut b = [0u8; 1];
            c.read_exact(&mut b).await.unwrap();
            assert_eq!(b[0], i + 1);
        }));
    }
    for _ in 0..20 {
        let (mut c, _) = tokio::time::timeout(Duration::from_secs(10), l.accept()).await.expect("accept within 10 s").unwrap();
        tokio::spawn(async move {
            let mut b = [0u8; 1];
            c.read_exact(&mut b).await.unwrap();
            c.write_all(&[b[0] + 1]).await.unwrap();
            let mut rest = vec![];
            let _ = c.read_to_end(&mut rest).await;
        });
    }
    for d in dials {
        d.await.unwrap();
    }
    // a second listener on the same port is refused while the first lives
    assert_eq!(super::listen_tcp(2234).await.unwrap_err().kind(), std::io::ErrorKind::AddrInUse);
    drop(l);
    drop(super::listen_tcp(2234).await.unwrap());

    // DNS over the tunnel
    assert_eq!(super::resolve("server.slsknet.org").await.unwrap(), vec![IpAddr::V4(slsk)]);
    assert_eq!(super::resolve("1.2.3.4").await.unwrap(), vec![IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4))]);

    // NAT-PMP: the loop maps the port once the tunnel is up
    tokio::time::timeout(Duration::from_secs(15), async {
        while super::forwarded_port().is_none() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("forwarded port within 15 s");
    assert_eq!(super::forwarded_port(), Some(51234));
    let st = super::status();
    assert_eq!(st["state"], "up");
    assert_eq!(st["forwarded_port"], 51234);
    assert_eq!(st["public_ip"], "203.0.113.7");
    assert_eq!(st["address"], "10.2.0.2");
    assert!(st["rx_bytes"].as_u64().unwrap() > 1 << 20 && st["tx_bytes"].as_u64().unwrap() > 1 << 20);
    assert!(st["last_handshake_secs_ago"].as_u64().unwrap() < 180);
    assert_eq!(super::summary(&st), "VPN: up · forwarded port 51234 · public IP 203.0.113.7");

    // stopped: everything says so
    near.shutdown();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!super::active());
    assert_eq!(super::connect_tcp(SocketAddr::from((far_ip, 7))).await.unwrap_err().kind(), std::io::ErrorKind::NotConnected);
    let err = inc.read(&mut b).await.unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::NotConnected);
    far.shutdown();
    super::install(None);
    assert_eq!(super::status()["state"], "off");
}

/// A handshaken tunnel over loopback UDP: (near, far). The near side is 10.2.0.2 with DNS 10.2.0.1,
/// as a provider config would have it; the far side is 10.2.0.1. Neither is installed as the
/// current VPN.
pub(crate) async fn tunnel() -> (Arc<Engine>, Arc<Engine>) {
    let (near_key, far_key): ([u8; 32], [u8; 32]) = (rand::random(), rand::random());
    let far_udp = super::engine::udp_socket(([127, 0, 0, 1], 0).into()).unwrap();
    let far_addr = far_udp.local_addr().unwrap();
    let far_cfg = WgConfig { private_key: far_key, address: Ipv4Addr::new(10, 2, 0, 1), prefix: 32, dns: vec![], public_key: public(near_key), preshared_key: None,
        endpoint_host: "127.0.0.1".into(), endpoint_port: 1, allowed_ips: vec![], keepalive: 0 };
    let far = Engine::new(far_cfg, false);
    tokio::spawn(far.clone().run(far_udp, None));
    let text = format!("[Interface]\nPrivateKey = {}\nAddress = 10.2.0.2/32\nDNS = 10.2.0.1\n\n[Peer]\nPublicKey = {}\nAllowedIPs = 0.0.0.0/0\nEndpoint = {far_addr}\n", b64(&near_key), b64(&public(far_key)));
    let near = Engine::new(config::parse(&text).unwrap(), false);
    let near_udp = super::engine::udp_socket(([127, 0, 0, 1], 0).into()).unwrap();
    super::spawn_engine(near.clone(), near_udp, Some(far_addr));
    tokio::time::timeout(Duration::from_secs(10), async {
        while !near.is_up() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("handshake within 10 s");
    (near, far)
}

/// A connection the far end resets reads as an error, not as a clean end of the stream; one the
/// far end closes (FIN) reads as the end.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reset_is_an_error_and_fin_is_the_end() {
    let (near, far) = tunnel().await;
    let l = near.listen(2300).unwrap();
    let to = SocketAddr::from(([10, 2, 0, 2], 2300));

    let mut c = far.connect(to).await.unwrap();
    let (mut inc, _) = tokio::time::timeout(Duration::from_secs(10), l.accept()).await.expect("accept within 10 s").unwrap();
    c.write_all(b"part").await.unwrap();
    let mut b = [0u8; 4];
    inc.read_exact(&mut b).await.unwrap();
    c.abort();
    let mut rest = vec![];
    let err = tokio::time::timeout(Duration::from_secs(10), inc.read_to_end(&mut rest)).await.expect("the reset arrives within 10 s").unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::ConnectionReset, "{err}");

    let mut c = far.connect(to).await.unwrap();
    let (mut inc, _) = tokio::time::timeout(Duration::from_secs(10), l.accept()).await.expect("accept within 10 s").unwrap();
    c.write_all(b"whole").await.unwrap();
    c.shutdown().await.unwrap();
    let mut all = vec![];
    tokio::time::timeout(Duration::from_secs(10), inc.read_to_end(&mut all)).await.expect("the FIN arrives within 10 s").unwrap();
    assert_eq!(all, b"whole");
    // still the end after the far side has gone away completely
    drop(c);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(inc.read(&mut b).await.unwrap(), 0);

    near.shutdown();
    far.shutdown();
}

/// No server behind the endpoint: "connecting", then the plain error after 30 s is covered by
/// Engine::state; here just that the state starts as connecting and nothing connects.
#[tokio::test]
async fn no_answer_is_not_up() {
    let text = format!("[Interface]\nPrivateKey = {PRIV}\nAddress = 10.2.0.2/32\nDNS = 10.2.0.1\n[Peer]\nPublicKey = {PUB}\nEndpoint = 127.0.0.1:9\n");
    let e = Engine::new(config::parse(&text).unwrap(), false);
    assert_eq!(e.state().0, "connecting");
    assert!(!e.is_up());
    e.info.lock().unwrap().started -= Duration::from_secs(31);
    assert_eq!(e.state(), ("error", Some(super::engine::NO_ANSWER.to_string())));
    e.shutdown();
    assert_eq!(e.state().0, "off");
}
