//! Inbound Soulseek connections on the VPN's forwarded port: listen on the tunnel and pipe every
//! accepted connection to the Soulseek engine (slskd), which listens on 127.0.0.1 at the same port.
use super::{Vpn, VpnListener};
use std::io;
use std::net::SocketAddr;
use std::time::Duration;

impl Vpn {
    /// Keep a relay for `port` running (None stops it). Cheap when nothing changed.
    pub fn relay(&self, port: Option<u16>) {
        let mut cur = self.0.relay.lock().unwrap();
        if let (Some(p), Some((q, h))) = (port, cur.as_ref()) {
            if p == *q && !h.is_finished() {
                return;
            }
        }
        if let Some((_, h)) = cur.take() {
            h.abort();
        }
        if let Some(p) = port.filter(|p| *p > 0) {
            tracing::info!("vpn: relaying the forwarded port {p} to the Soulseek engine");
            *cur = Some((p, tokio::spawn(run(self.clone(), p))));
        }
    }
}

/// Listen on the tunnel (again after the tunnel restarts) and pipe to 127.0.0.1:port.
async fn run(vpn: Vpn, port: u16) {
    let to = SocketAddr::from(([127, 0, 0, 1], port));
    loop {
        match vpn.listen_tcp(port).await {
            Ok(l) => {
                let e = pipe_accepted(&l, to).await;
                tracing::debug!("vpn relay on {port}: {e}");
            }
            Err(e) => tracing::debug!("vpn relay: can't listen on {port} yet: {e}"),
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

/// Accept until the listener fails (the tunnel went away); each connection is copied both ways to
/// `to` in its own task. Returns the error that ended it.
pub(crate) async fn pipe_accepted(l: &VpnListener, to: SocketAddr) -> io::Error {
    loop {
        let (mut inc, _) = match l.accept().await {
            Ok(x) => x,
            Err(e) => return e,
        };
        tokio::spawn(async move {
            let Ok(mut out) = tokio::net::TcpStream::connect(to).await else { return };
            let _ = out.set_nodelay(true);
            let _ = tokio::io::copy_bidirectional(&mut inc, &mut out).await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// A peer on the far side of a loopback tunnel connects to the forwarded port and reaches a
    /// local TCP server (standing in for slskd) through the relay, both ways.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn forwards_tunnel_connections_to_localhost() {
        let (near, far) = super::super::tests::tunnel().await;
        let local = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let to = local.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = local.accept().await else { return };
                tokio::spawn(async move {
                    let mut b = [0u8; 5];
                    if s.read_exact(&mut b).await.is_ok() {
                        b.reverse();
                        let _ = s.write_all(&b).await;
                    }
                });
            }
        });
        let l = near.listen(2400).unwrap();
        let relay = tokio::spawn(async move { pipe_accepted(&l, to).await });
        for _ in 0..2 {
            let mut c = far.connect(SocketAddr::from(([10, 2, 0, 2], 2400))).await.unwrap();
            c.write_all(b"hello").await.unwrap();
            let mut b = [0u8; 5];
            tokio::time::timeout(Duration::from_secs(10), c.read_exact(&mut b)).await.expect("answer within 10 s").unwrap();
            assert_eq!(&b, b"olleh");
        }
        near.shutdown();
        // the tunnel going away ends the relay loop with an error (run() then listens again)
        tokio::time::timeout(Duration::from_secs(10), relay).await.expect("relay ends when the tunnel stops").unwrap();
        far.shutdown();
    }
}
