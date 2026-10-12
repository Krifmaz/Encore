//! Integration test: two built-in clients on 127.0.0.1 against a fake Soulseek server – alice shares
//! one file, bob finds it and downloads it directly, through a server relay, and resuming a .part file.
use super::client::{local_file_for, part_file_for, Client, Settings};
use super::proto as p;
use super::shares::{self, Item, Shares};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{tcp::OwnedReadHalf, TcpListener, TcpStream};
use tokio::sync::mpsc;

const FILE: &str = "Daft Punk/Discovery/01 - One More Time.flac";
/// Not a multiple of either side's buffer, so the last chunk is a partial one.
const SIZE: usize = 600 * 1024 + 123;
const PART: usize = 100 * 1024;

// ---- fake server ---------------------------------------------------------------------------------

struct User { tx: mpsc::UnboundedSender<Vec<u8>>, port: u16 }

#[derive(Default)]
struct ServerState {
    users: HashMap<String, User>,
    /// users whose GetPeerAddress answer is 0.0.0.0:0, so direct connections to them fail
    hidden: HashSet<String>,
    /// every ConnectToPeer relayed: (from, to, kind)
    relays: Vec<(String, String, String)>,
}

#[derive(Clone)]
struct FakeServer { addr: String, st: Arc<Mutex<ServerState>> }

impl FakeServer {
    async fn start() -> FakeServer {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let s = FakeServer { addr: l.local_addr().unwrap().to_string(), st: Default::default() };
        let st = s.st.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = l.accept().await {
                tokio::spawn(serve(stream, st.clone()));
            }
        });
        s
    }
    fn hide(&self, user: &str) { self.st.lock().unwrap().hidden.insert(user.to_string()); }
    fn port_of(&self, user: &str) -> Option<u16> { self.st.lock().unwrap().users.get(user).map(|u| u.port).filter(|p| *p > 0) }
    fn relays(&self) -> Vec<(String, String, String)> { self.st.lock().unwrap().relays.clone() }
}

/// Same framing as `client::read_frame` for u32 codes.
async fn read_frame32(r: &mut OwnedReadHalf) -> Option<(u32, Vec<u8>)> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len).await.ok()?;
    let len = u32::from_le_bytes(len) as usize;
    if !(4..=p::MAX_FRAME).contains(&len) {
        return None;
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).await.ok()?;
    Some((u32::from_le_bytes(buf[0..4].try_into().unwrap()), buf[4..].to_vec()))
}

/// One client's server connection: just enough of the real server for two users to find each other.
async fn serve(stream: TcpStream, st: Arc<Mutex<ServerState>>) {
    let (mut rd, mut wr) = stream.into_split();
    let (tx, mut rx) = mpsc::unbounded_channel::<Vec<u8>>();
    tokio::spawn(async move { while let Some(b) = rx.recv().await { if wr.write_all(&b).await.is_err() { break } } });
    let mut me = String::new();
    while let Some((code, payload)) = read_frame32(&mut rd).await {
        let mut r = p::R::new(&payload);
        match code {
            p::server::LOGIN => {
                me = r.str().unwrap();
                let _ = tx.send(p::frame32(code, &p::W::default().bool(true).str("Welcome to the fake server").ip(Ipv4Addr::LOCALHOST).str("").bool(false).0));
                st.lock().unwrap().users.insert(me.clone(), User { tx: tx.clone(), port: 0 });
            }
            p::server::SET_WAIT_PORT => {
                let port = r.u32().unwrap() as u16;
                if let Some(u) = st.lock().unwrap().users.get_mut(&me) { u.port = port; }
            }
            p::server::GET_PEER_ADDRESS => {
                let user = r.str().unwrap();
                let s = st.lock().unwrap();
                let (ip, port) = match s.users.get(&user) {
                    Some(u) if !s.hidden.contains(&user) => (Ipv4Addr::LOCALHOST, u.port),
                    _ => (Ipv4Addr::UNSPECIFIED, 0),
                };
                let _ = tx.send(p::frame32(code, &p::W::default().str(&user).ip(ip).u32(port as u32).0));
            }
            p::server::FILE_SEARCH => {
                let token = r.u32().unwrap();
                let query = r.str().unwrap();
                // what the real server sends a branch root: the distributed Search inside EmbeddedMessage
                let f = p::frame32(p::server::EMBEDDED_MESSAGE, &p::W::default().u8(p::distrib::SEARCH).u32(0).str(&me).u32(token).str(&query).0);
                for (name, u) in &st.lock().unwrap().users {
                    if *name != me { let _ = u.tx.send(f.clone()); }
                }
            }
            p::server::CONNECT_TO_PEER => {
                let token = r.u32().unwrap();
                let target = r.str().unwrap();
                let kind = r.str().unwrap();
                let mut g = st.lock().unwrap();
                let s = &mut *g;
                let my_port = s.users.get(&me).map(|u| u.port).unwrap_or(0);
                match s.users.get(&target) {
                    Some(u) => {
                        let _ = u.tx.send(p::frame32(code, &p::W::default().str(&me).str(&kind).ip(Ipv4Addr::LOCALHOST).u32(my_port as u32).u32(token).u8(0).u32(0).u32(0).0));
                        s.relays.push((me.clone(), target, kind));
                    }
                    None => { let _ = tx.send(p::frame32(p::server::CANT_CONNECT_TO_PEER, &p::W::default().u32(token).0)); } // the live server sends only the token
                }
            }
            _ => {} // ping, status, share counts, distributed bookkeeping: nothing to do
        }
    }
    let mut s = st.lock().unwrap();
    if s.users.get(&me).map(|u| u.tx.same_channel(&tx)).unwrap_or(false) {
        s.users.remove(&me);
    }
}

// ---- harness -------------------------------------------------------------------------------------

fn noise(n: usize) -> Vec<u8> {
    let mut x = 0x9E37_79B9_7F4A_7C15u64;
    (0..n).map(|_| { x ^= x << 13; x ^= x >> 7; x ^= x << 17; (x >> 24) as u8 }).collect()
}

/// Two distinct free ports: bound at the same time so the kernel can't hand out one twice.
fn free_ports() -> (u16, u16) {
    let a = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let b = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    (a.local_addr().unwrap().port(), b.local_addr().unwrap().port())
}

fn settings(user: &str, server: &str, port: u16, dl: &Path, share: bool) -> Settings {
    Settings { username: user.into(), password: "pw".into(), server: server.into(), listen_port: port, download_dir: dl.to_path_buf(), upload_slots: 1, share }
}

async fn wait_for(what: &str, secs: u64, mut ok: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while !ok() {
        assert!(Instant::now() < deadline, "timed out after {secs} s waiting for {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

struct Env {
    root: PathBuf,
    server: FakeServer,
    a: Client,
    b: Client,
    b_dl: PathBuf,
    bytes: Vec<u8>,
    /// the file's name as the network sees it
    remote: String,
}

impl Env {
    /// alice shares one file, bob has nothing; both logged in to a fresh fake server.
    async fn new(name: &str, hide_alice: bool) -> Env {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let root = std::env::temp_dir().join(format!("encore-slsk-{name}-{}-{nanos}", std::process::id()));
        let (a_lib, a_dl, b_lib, b_dl) = (root.join("a-lib"), root.join("a-dl"), root.join("b-lib"), root.join("b-dl"));
        let path = a_lib.join(FILE);
        for d in [path.parent().unwrap(), a_dl.as_path(), b_lib.as_path(), b_dl.as_path()] {
            std::fs::create_dir_all(d).unwrap();
        }
        let bytes = noise(SIZE);
        std::fs::write(&path, &bytes).unwrap();
        let shares = Shares::build(&a_lib, [Item { path: path.to_string_lossy().into_owned(), size: bytes.len() as u64, bitrate: 900, duration: 320 }]);
        let server = FakeServer::start().await;
        if hide_alice {
            server.hide("alice");
        }
        let (pa, pb) = free_ports();
        let a = Client::start(settings("alice", &server.addr, pa, &a_dl, true), shares);
        let b = Client::start(settings("bob", &server.addr, pb, &b_dl, false), Shares::build(&b_lib, std::iter::empty()));
        wait_for("both clients to log in", 5, || a.logged_in() && b.logged_in()).await;
        wait_for("the server to learn both listen ports", 5, || server.port_of("alice") == Some(pa) && server.port_of("bob") == Some(pb)).await;
        let remote = format!("{}\\{}", shares::ROOT, FILE.replace('/', "\\"));
        Env { root, server, a, b, b_dl, bytes, remote }
    }

    fn target(&self) -> PathBuf { local_file_for(&self.b_dl, &self.remote) }

    /// bob's download `id` once it has succeeded; fails fast on any other final state.
    async fn wait_download(&self, id: u64) -> Value {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let d = self.b.downloads().into_iter().find(|d| d["id"] == id.to_string()).unwrap_or(Value::Null);
            let state = d["state"].as_str().unwrap_or("(gone)");
            if state == "Completed, Succeeded" {
                return d;
            }
            assert!(!state.starts_with("Completed") && Instant::now() < deadline, "download {id} did not succeed: state={state:?} error={}", d["error"]);
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    fn assert_file(&self) {
        let target = self.target();
        assert_eq!(target, self.b_dl.join("Discovery").join("01 - One More Time.flac"));
        let got = std::fs::read(&target).unwrap_or_else(|e| panic!("{}: {e}", target.display()));
        assert!(got == self.bytes, "downloaded file differs ({} vs {} bytes)", got.len(), self.bytes.len());
    }

    /// Finished uploads are dropped from alice's list; then both clients go down.
    async fn finish(self) {
        wait_for("alice's finished upload to be dropped", 5, || self.a.uploads().is_empty()).await;
        self.a.stop();
        self.b.stop();
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        self.a.stop();
        self.b.stop();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

// ---- tests ---------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn search_and_download_direct() {
    let env = Env::new("direct", false).await;
    let results = env.b.search("one more time", 3).await.unwrap();
    assert_eq!(results.len(), 1, "{results:?}");
    let r = &results[0];
    assert_eq!(r.user, "alice");
    assert!(r.free_slot, "alice has an idle upload slot");
    assert_eq!(r.files.len(), 1, "{:?}", r.files);
    let f = &r.files[0];
    assert!(f.name.ends_with("\\01 - One More Time.flac"), "{}", f.name);
    assert_eq!(f.name, env.remote);
    assert_eq!(f.size, env.bytes.len() as u64);
    let ids = env.b.enqueue(&r.user, &[(f.name.clone(), f.size)]).unwrap();
    let d = env.wait_download(ids[0]).await;
    assert_eq!(d["bytesTransferred"], env.bytes.len() as u64);
    assert_eq!(d["percentComplete"], 100.0);
    env.assert_file();
    assert!(env.server.relays().is_empty(), "everything went direct: {:?}", env.server.relays());
    env.finish().await;
}

/// bob can't learn alice's address, so his "P" connection has to go the long way: ConnectToPeer to
/// the server, which tells alice to connect to bob's listener and pierce with the token.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn download_through_server_relay() {
    let env = Env::new("relay", true).await;
    // no search first: alice's answer would open the "P" connection bob then reuses, and the relay
    // would never be needed – queue the file by its known name instead
    let ids = env.b.enqueue("alice", &[(env.remote.clone(), env.bytes.len() as u64)]).unwrap();
    env.wait_download(ids[0]).await;
    env.assert_file();
    let relays = env.server.relays();
    assert!(relays.contains(&("bob".into(), "alice".into(), "P".into())), "{relays:?}");
    env.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resume_after_partial_file() {
    let env = Env::new("resume", false).await;
    let target = env.target();
    // receive() resumes a .part named after the uploader, path and size, from whatever is already there
    let part = part_file_for(&env.b_dl, "alice", &env.remote, env.bytes.len() as u64);
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(&part, &env.bytes[..PART]).unwrap();
    let results = env.b.search("one more time", 3).await.unwrap();
    assert_eq!(results.len(), 1, "{results:?}");
    let f = &results[0].files[0];
    let ids = env.b.enqueue(&results[0].user, &[(f.name.clone(), f.size)]).unwrap();
    let d = env.wait_download(ids[0]).await;
    assert_eq!(d["bytesTransferred"], env.bytes.len() as u64);
    assert_eq!(d["size"], env.bytes.len() as u64);
    assert!(!part.exists(), "the .part file is renamed into place");
    env.assert_file();
    env.finish().await;
}
