//! The Soulseek connection runtime: one server link, a listener for peers, searches, downloads,
//! uploads of what we share, and a place in the distributed network so other users' searches reach
//! us. Everything slskd did for Encore, in-process.
//!
//! How a download goes: we open a "P" connection to the user and send QueueUpload(file). When they
//! have a free slot they send TransferRequest(upload) back; we answer "allowed"; they open an "F"
//! connection, send the transfer token, we send the byte offset to resume from, and the file streams
//! over. Uploads are the same dance from the other side. Peers that can't reach each other directly
//! ask the server to tell the other side to connect instead (ConnectToPeer + PierceFireWall).
use super::proto::{self as p, Dir, DistribMsg, FileEntry, InitMsg, PeerMsg, SearchResult, ServerMsg};
use super::shares::Shares;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeekExt, AsyncWrite, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot, watch, Notify, Semaphore};

pub const DEFAULT_SERVER: &str = "server.slsknet.org:2242";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(6);
const INDIRECT_TIMEOUT: Duration = Duration::from_secs(15);
const PEER_IDLE: Duration = Duration::from_secs(90);
const READ_TIMEOUT: Duration = Duration::from_secs(60);
/// Share lists are big and some clients build them on request.
const BROWSE_TIMEOUT: Duration = Duration::from_secs(45);
const MAX_RESULTS_PER_SEARCH: usize = 250;
const MAX_FILES_PER_ANSWER: usize = 100;

#[derive(Clone, Debug)]
pub struct Settings {
    pub username: String,
    pub password: String,
    pub server: String,
    pub listen_port: u16,
    pub download_dir: PathBuf,
    pub upload_slots: usize,
    pub share: bool,
    /// Every connection (server, peers, transfers, listener) goes through the built-in VPN.
    pub vpn: bool,
}

impl Settings {
    /// What a change of these means: a different login, so a restart.
    pub fn key(&self) -> String {
        format!("{}|{}|{}|{}|{}|{}|{}|{}", self.username, self.password, self.server, self.listen_port, self.download_dir.display(), self.upload_slots, self.share, self.vpn)
    }

    pub fn net(&self) -> Net {
        if self.vpn { Net::Vpn } else { Net::Direct }
    }
}

// ---- connections: direct or through the built-in VPN ---------------------------------------------

pub trait AsyncReadWrite: AsyncRead + AsyncWrite {}
impl<T: AsyncRead + AsyncWrite + ?Sized> AsyncReadWrite for T {}

/// One connection to the server or a peer, whichever way it was made.
pub type Conn = Box<dyn AsyncReadWrite + Send + Unpin>;

/// How this client reaches the network: the normal connection, or only through the VPN tunnel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Net {
    Direct,
    Vpn,
}

impl Net {
    pub async fn connect(self, addr: SocketAddr) -> std::io::Result<Conn> {
        match self {
            Net::Direct => {
                let s = TcpStream::connect(addr).await?;
                let _ = s.set_nodelay(true);
                Ok(Box::new(s))
            }
            Net::Vpn => Ok(Box::new(crate::vpn::connect_tcp(addr).await?)),
        }
    }

    /// `host:port`, where host may be a name; on the VPN the name is looked up through the tunnel too.
    pub async fn connect_host(self, hostport: &str) -> std::io::Result<Conn> {
        match self {
            Net::Direct => {
                let s = TcpStream::connect(hostport).await?;
                let _ = s.set_nodelay(true);
                Ok(Box::new(s))
            }
            Net::Vpn => {
                let (host, port) = split_host_port(hostport)
                    .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("bad server address {hostport}")))?;
                let ips = match host.parse::<IpAddr>() {
                    Ok(ip) => vec![ip],
                    Err(_) => crate::vpn::resolve(&host).await?,
                };
                let mut last = std::io::Error::new(std::io::ErrorKind::NotFound, format!("no address for {host}"));
                for ip in ips.into_iter().filter(|ip| ip.is_ipv4()) {
                    match crate::vpn::connect_tcp(SocketAddr::new(ip, port)).await {
                        Ok(s) => return Ok(Box::new(s)),
                        Err(e) => last = e,
                    }
                }
                Err(last)
            }
        }
    }

    pub async fn listen(self, port: u16) -> std::io::Result<Listener> {
        match self {
            Net::Direct => Ok(Listener::Direct(TcpListener::bind(SocketAddr::from(([0, 0, 0, 0], port))).await?)),
            Net::Vpn => Ok(Listener::Vpn(crate::vpn::listen_tcp(port).await?)),
        }
    }
}

pub enum Listener {
    Direct(TcpListener),
    Vpn(crate::vpn::VpnListener),
}

impl Listener {
    pub async fn accept(&self) -> std::io::Result<Conn> {
        match self {
            Listener::Direct(l) => {
                let (s, _) = l.accept().await?;
                let _ = s.set_nodelay(true);
                Ok(Box::new(s))
            }
            Listener::Vpn(l) => Ok(Box::new(l.accept().await?.0)),
        }
    }
}

/// "host:port" → (host, port); "[v6]:port" keeps the brackets off.
pub fn split_host_port(s: &str) -> Option<(String, u16)> {
    let (h, p) = s.trim().rsplit_once(':')?;
    let port = p.parse::<u16>().ok().filter(|p| *p > 0)?;
    let h = h.trim_start_matches('[').trim_end_matches(']');
    if h.is_empty() { None } else { Some((h.to_string(), port)) }
}

#[derive(Default, Clone)]
pub struct ConnState {
    pub connected: bool,
    pub logged_in: bool,
    pub error: String,
    pub ip: Option<Ipv4Addr>,
    pub parent: Option<String>,
    pub branch_level: i32,
    pub since: f64,
    pub supporter: bool,
}

struct Search {
    results: Vec<SearchResult>,
    started: Instant,
}

/// A live "P" connection: where to write; the reader runs in its own task.
#[derive(Clone)]
struct Peer {
    tx: mpsc::UnboundedSender<Vec<u8>>,
}

pub struct Download {
    pub id: u64,
    pub user: String,
    pub file: String,
    pub prog: Mutex<Prog>,
    cancelled: AtomicBool,
    wake: Notify,
    /// Fed by the TransferRequest handler once the uploader is ready.
    ready: Mutex<Option<oneshot::Sender<(u32, u64)>>>,
    requeue: AtomicBool,
}

/// The moving parts of a download (one lock, never held across an await).
#[derive(Default, Clone)]
pub struct Prog {
    pub size: u64,
    pub bytes: u64,
    pub state: String,
    pub place: Option<u32>,
    pub speed: f64,
    pub started: f64,
    pub error: String,
    pub token: Option<u32>,
}

pub struct Upload {
    pub id: u64,
    pub user: String,
    pub file: String,
    pub path: PathBuf,
    pub size: u64,
    pub bytes: u64,
    pub state: String,
    pub retry_at: Option<Instant>,
}

impl Download {
    fn set(&self, f: impl FnOnce(&mut Prog)) { f(&mut self.prog.lock().unwrap()) }
    fn state(&self) -> String { self.prog.lock().unwrap().state.clone() }
    fn size(&self) -> u64 { self.prog.lock().unwrap().size }
    fn done(&self) -> bool { self.prog.lock().unwrap().state.starts_with("Completed") }
}

struct Inner {
    settings: Settings,
    shares: RwLock<Arc<Shares>>,
    server_tx: Mutex<Option<mpsc::UnboundedSender<Vec<u8>>>>,
    state: Mutex<ConnState>,
    searches: Mutex<HashMap<u32, Search>>,
    addr_waiters: Mutex<HashMap<String, Vec<oneshot::Sender<Option<(Ipv4Addr, u16)>>>>>,
    indirect: Mutex<HashMap<u32, oneshot::Sender<Result<Conn, String>>>>,
    peers: Mutex<HashMap<String, Peer>>,
    downloads: Mutex<Vec<Arc<Download>>>,
    uploads: Mutex<Vec<Upload>>,
    transfer_waits: Mutex<HashMap<u32, oneshot::Sender<Result<(), String>>>>,
    file_waits: Mutex<HashMap<(String, u32), oneshot::Sender<Conn>>>,
    /// browse() callers waiting for a user's ShareFileList (it carries no token, so keyed by user)
    browse_waits: Mutex<HashMap<String, Vec<oneshot::Sender<Vec<Dir>>>>>,
    answers: Arc<Semaphore>,
    next_id: AtomicU64,
    upload_speed: Mutex<(u64, f64)>, // bytes, seconds of recent uploads
    stop: watch::Sender<bool>,
    parent_gen: AtomicU64,
}

#[derive(Clone)]
pub struct Client {
    inner: Arc<Inner>,
}

fn now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

fn token() -> u32 {
    rand::random::<u32>() | 1
}

/// Last path component of a Soulseek (backslash) path, and its folder's last component.
fn split_remote(name: &str) -> (String, String) {
    let n = name.replace('/', "\\");
    let mut parts = n.rsplit('\\');
    let base = parts.next().unwrap_or("").to_string();
    let parent = parts.next().unwrap_or("").to_string();
    (parent, base)
}

fn safe_component(s: &str) -> String {
    let s: String = s.chars().map(|c| if matches!(c, '/' | '\\' | ':' | '\0') { '_' } else { c }).collect();
    let s = s.trim().trim_start_matches('@').to_string();
    if s.is_empty() || s == "." || s == ".." { "_".into() } else { s }
}

// ---- framing -------------------------------------------------------------------------------------

async fn read_frame<R: AsyncReadExt + Unpin>(r: &mut R, code8: bool) -> anyhow::Result<Option<(u32, Vec<u8>)>> {
    let mut len = [0u8; 4];
    match r.read_exact(&mut len).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    let len = u32::from_le_bytes(len) as usize;
    let head = if code8 { 1 } else { 4 };
    if len < head || len > p::MAX_FRAME {
        anyhow::bail!("bad frame length {len}");
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).await?;
    let code = if code8 { buf[0] as u32 } else { u32::from_le_bytes(buf[0..4].try_into()?) };
    Ok(Some((code, buf[head..].to_vec())))
}

fn writer(mut w: WriteHalf<Conn>) -> mpsc::UnboundedSender<Vec<u8>> {
    let (tx, mut rx) = mpsc::unbounded_channel::<Vec<u8>>();
    tokio::spawn(async move {
        while let Some(b) = rx.recv().await {
            if w.write_all(&b).await.is_err() {
                break;
            }
        }
        let _ = w.shutdown().await;
    });
    tx
}

// ---- public API ----------------------------------------------------------------------------------

impl Client {
    pub fn start(settings: Settings, shares: Shares) -> Client {
        let (stop, _) = watch::channel(false);
        let inner = Arc::new(Inner {
            settings, shares: RwLock::new(Arc::new(shares)), server_tx: Mutex::new(None), state: Mutex::new(ConnState::default()),
            searches: Default::default(), addr_waiters: Default::default(), indirect: Default::default(), peers: Default::default(),
            downloads: Default::default(), uploads: Default::default(), transfer_waits: Default::default(), file_waits: Default::default(),
            browse_waits: Default::default(), answers: Arc::new(Semaphore::new(12)), next_id: AtomicU64::new(1), upload_speed: Mutex::new((0, 0.0)), stop, parent_gen: AtomicU64::new(0),
        });
        let c = Client { inner };
        tokio::spawn(c.clone().listen());
        tokio::spawn(c.clone().server_loop());
        tokio::spawn(c.clone().upload_pump());
        c
    }

    pub fn stop(&self) {
        self.inner.stop.send_replace(true); // stores even when no task has subscribed yet
        *self.inner.server_tx.lock().unwrap() = None;
        self.inner.peers.lock().unwrap().clear();
        self.inner.browse_waits.lock().unwrap().clear(); // waiters fail now instead of at their timeout
    }

    pub fn settings(&self) -> &Settings { &self.inner.settings }
    pub fn state(&self) -> ConnState { self.inner.state.lock().unwrap().clone() }
    pub fn logged_in(&self) -> bool { self.inner.state.lock().unwrap().logged_in }

    /// Swap in a fresh share index (after a library scan). The server learns the new counts.
    pub fn set_shares(&self, shares: Shares) {
        let (d, f) = (shares.dir_count() as u32, shares.file_count() as u32);
        *self.inner.shares.write().unwrap() = Arc::new(shares);
        if self.logged_in() {
            self.inner.send_server(p::shared_folders_files(d, f));
        }
    }

    pub fn status(&self) -> Value {
        let s = self.state();
        let sh = self.inner.shares.read().unwrap().clone();
        let (dl, ul) = (self.inner.downloads.lock().unwrap().len(), self.inner.uploads.lock().unwrap().len());
        json!({ "connected": s.connected, "logged_in": s.logged_in, "error": s.error, "username": self.inner.settings.username, "ip": s.ip.map(|i| i.to_string()),
            "listen_port": self.inner.settings.listen_port, "parent": s.parent, "branch_level": s.branch_level, "since": s.since, "supporter": s.supporter,
            "shares": { "dirs": sh.dir_count(), "files": sh.file_count(), "enabled": self.inner.settings.share }, "downloads": dl, "uploads": ul })
    }

    fn need_login(&self) -> anyhow::Result<()> {
        if !self.logged_in() {
            anyhow::bail!("Soulseek: not logged in ({})", { let e = self.state().error; if e.is_empty() { "connecting".into() } else { e } });
        }
        Ok(())
    }

    /// Send a search to the network; answers collect under the returned token for `search_results`.
    /// Entries live 300 s, so a page can poll one at leisure.
    pub fn search_start(&self, text: &str) -> anyhow::Result<u32> {
        self.need_login()?;
        let t = token();
        {
            let mut s = self.inner.searches.lock().unwrap();
            s.retain(|_, x| x.started.elapsed() < Duration::from_secs(300));
            s.insert(t, Search { results: vec![], started: Instant::now() });
        }
        if !self.inner.send_server(p::file_search(t, text)) {
            self.inner.searches.lock().unwrap().remove(&t);
            anyhow::bail!("not connected to the Soulseek server");
        }
        Ok(t)
    }

    /// What a search has gathered so far, and whether it is over: `secs` have passed or the result
    /// cap is reached. None once the entry has expired (or was never ours).
    pub fn search_results(&self, token: u32, secs: u64) -> Option<(Vec<SearchResult>, bool)> {
        let s = self.inner.searches.lock().unwrap();
        let x = s.get(&token)?;
        Some((x.results.clone(), x.started.elapsed() >= Duration::from_secs(secs) || x.results.len() >= MAX_RESULTS_PER_SEARCH))
    }

    /// Search the network for `secs` seconds; results in the order they came.
    pub async fn search(&self, text: &str, secs: u64) -> anyhow::Result<Vec<SearchResult>> {
        let t = self.search_start(text)?;
        // the entry goes away with this future, even when the HTTP request that started it is dropped
        struct Guard(Arc<Inner>, u32);
        impl Drop for Guard { fn drop(&mut self) { self.0.searches.lock().unwrap().remove(&self.1); } }
        let guard = Guard(self.inner.clone(), t);
        let deadline = Instant::now() + Duration::from_secs(secs.max(3));
        while Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(500)).await;
            if self.inner.searches.lock().unwrap().get(&t).map(|s| s.results.len() >= MAX_RESULTS_PER_SEARCH).unwrap_or(true) {
                break;
            }
        }
        let out = self.inner.searches.lock().unwrap().get_mut(&t).map(|s| std::mem::take(&mut s.results)).unwrap_or_default();
        drop(guard);
        Ok(out)
    }

    /// A user's whole share list (folders with files), as they send it.
    pub async fn browse(&self, user: &str) -> anyhow::Result<Vec<Dir>> {
        self.need_login()?;
        let tx = self.peer(user).await?;
        let (wtx, wrx) = oneshot::channel();
        {
            let mut m = self.inner.browse_waits.lock().unwrap();
            let ws = m.entry(user.to_string()).or_default();
            ws.retain(|w| !w.is_closed()); // callers that gave up (request dropped) leave dead senders behind
            ws.push(wtx);
        }
        if tx.send(p::get_share_file_list()).is_err() {
            self.inner.peers.lock().unwrap().remove(user);
            anyhow::bail!("lost the connection to {user}");
        }
        match tokio::time::timeout(BROWSE_TIMEOUT, wrx).await {
            Ok(Ok(dirs)) => Ok(dirs),
            Ok(Err(_)) => anyhow::bail!("the Soulseek client stopped"),
            Err(_) => {
                // our receiver is gone, so our sender reads as closed: drop it, keep anyone else's
                if let Some(ws) = self.inner.browse_waits.lock().unwrap().get_mut(user) {
                    ws.retain(|w| !w.is_closed());
                }
                anyhow::bail!("{user} did not send their file list within {} s", BROWSE_TIMEOUT.as_secs())
            }
        }
    }

    /// Queue files from one user. Each becomes a download that the worker polls through `downloads()`.
    pub fn enqueue(&self, user: &str, files: &[(String, u64)]) -> anyhow::Result<Vec<u64>> {
        if !self.logged_in() {
            anyhow::bail!("Soulseek: not logged in");
        }
        let mut ids = vec![];
        for (file, size) in files {
            // one live entry per (user, file): a second request for the same file rides the first
            if let Some(live) = self.inner.download_by_file(user, file) {
                ids.push(live.id);
                continue;
            }
            let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
            let d = Arc::new(Download {
                id, user: user.to_string(), file: file.clone(),
                prog: Mutex::new(Prog { size: *size, state: "Requested".into(), started: now(), ..Default::default() }),
                cancelled: AtomicBool::new(false), wake: Notify::new(), ready: Mutex::new(None), requeue: AtomicBool::new(false),
            });
            let mut list = self.inner.downloads.lock().unwrap();
            list.retain(|x| !(x.user == d.user && x.file == d.file)); // drop the finished one it replaces
            list.push(d.clone());
            drop(list);
            ids.push(id);
            tokio::spawn(self.clone().download_task(d));
        }
        Ok(ids)
    }

    pub fn downloads(&self) -> Vec<Value> {
        self.inner.downloads.lock().unwrap().iter().map(|d| {
            let (parent, base) = split_remote(&d.file);
            let p = d.prog.lock().unwrap().clone();
            let pct = if p.size > 0 { (p.bytes as f64 / p.size as f64 * 1000.0).round() / 10.0 } else { 0.0 };
            json!({ "id": d.id.to_string(), "username": d.user, "filename": d.file, "basename": base, "directory": parent, "size": p.size,
                "bytesTransferred": p.bytes, "percentComplete": pct, "state": p.state, "place": p.place, "averageSpeed": p.speed, "error": p.error, "started": p.started })
        }).collect()
    }

    pub fn uploads(&self) -> Vec<Value> {
        self.inner.uploads.lock().unwrap().iter().map(|u| json!({ "id": u.id.to_string(), "username": u.user, "filename": u.file, "size": u.size, "bytesTransferred": u.bytes, "state": u.state })).collect()
    }

    pub fn cancel(&self, user: &str, id: &str) {
        let found = {
            let mut list = self.inner.downloads.lock().unwrap();
            let d = list.iter().find(|d| d.user == user && d.id.to_string() == id).cloned();
            if let Some(d) = &d {
                d.cancelled.store(true, Ordering::Relaxed);
                d.wake.notify_waiters();
                list.retain(|x| x.id != d.id);
            }
            d
        };
        if let Some(d) = found {
            if d.state() != "InProgress" { let _ = std::fs::remove_file(self.part_path(&d)); } // receive() removes its own
        }
    }

    pub fn clear_completed(&self) {
        self.inner.downloads.lock().unwrap().retain(|d| !d.done());
    }
}

// ---- download / upload bookkeeping (short locks, never across awaits) ----------------------------

impl Inner {
    fn send_server(&self, b: Vec<u8>) -> bool {
        match self.server_tx.lock().unwrap().as_ref() { Some(tx) => tx.send(b).is_ok(), None => false }
    }

    fn download_by_file(&self, user: &str, file: &str) -> Option<Arc<Download>> {
        self.downloads.lock().unwrap().iter().find(|d| d.user == user && d.file == file && !d.done()).cloned()
    }

    fn active_uploads(&self) -> usize {
        // a slot is taken from the moment we ask the peer, not only once bytes flow
        self.uploads.lock().unwrap().iter().filter(|u| u.state == "InProgress" || u.state == "Requested").count()
    }

    fn free_slot(&self) -> bool {
        self.active_uploads() < self.settings.upload_slots.max(1)
    }

    fn queue_len(&self) -> u64 {
        self.uploads.lock().unwrap().iter().filter(|u| u.state == "Queued").count() as u64
    }

    fn avg_upload_speed(&self) -> u32 {
        let (b, s) = *self.upload_speed.lock().unwrap();
        if s > 0.0 { (b as f64 / s) as u32 } else { 0 }
    }

    fn search_result(&self, token: u32, files: Vec<FileEntry>) -> SearchResult {
        SearchResult { user: self.settings.username.clone(), token, files, free_slot: self.free_slot(), upload_speed: self.avg_upload_speed(), queue_length: self.queue_len() }
    }
}

// ---- server link ---------------------------------------------------------------------------------

impl Client {
    async fn server_loop(self) {
        let mut stop = self.inner.stop.subscribe();
        let mut backoff = 5u64;
        loop {
            if *stop.borrow() {
                return;
            }
            let res = tokio::select! {
                r = self.server_session() => r,
                _ = stop.changed() => return,
            };
            {
                let mut s = self.inner.state.lock().unwrap();
                s.connected = false;
                s.logged_in = false;
                s.parent = None;
                if let Err(e) = &res {
                    s.error = e.to_string();
                }
            }
            *self.inner.server_tx.lock().unwrap() = None;
            self.inner.addr_waiters.lock().unwrap().clear();
            let wait = match &res {
                Err(e) if e.to_string().contains("INVALIDPASS") || e.to_string().contains("INVALIDUSERNAME") => 300,
                Err(e) if e.to_string().contains("elsewhere") => 120,
                _ => { let w = backoff; backoff = (backoff * 2).min(300); w }
            };
            tracing::info!("soulseek: server link down ({}); retrying in {wait}s", res.err().map(|e| e.to_string()).unwrap_or_else(|| "closed".into()));
            tokio::select! { _ = tokio::time::sleep(Duration::from_secs(wait)) => {}, _ = stop.changed() => return }
            if backoff > 5 && wait >= 60 {
                backoff = 5;
            }
        }
    }

    async fn server_session(&self) -> anyhow::Result<()> {
        let st = &self.inner.settings;
        let stream = tokio::time::timeout(Duration::from_secs(15), st.net().connect_host(&st.server)).await
            .map_err(|_| anyhow::anyhow!("timed out connecting to {}", st.server))??;
        let (mut rd, wr) = tokio::io::split(stream);
        let tx = writer(wr);
        tx.send(p::login(&st.username, &st.password))?;
        {
            let mut s = self.inner.state.lock().unwrap();
            s.connected = true;
            s.error.clear();
        }
        *self.inner.server_tx.lock().unwrap() = Some(tx.clone());
        // keepalive from the side, so the read below is never cancelled half-way through a frame
        {
            let tx = tx.clone();
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    if tx.send(p::ping()).is_err() { break; }
                }
            });
        }
        loop {
            let (code, payload) = match tokio::time::timeout(Duration::from_secs(600), read_frame(&mut rd, false)).await {
                Ok(Ok(Some(f))) => f,
                Ok(Ok(None)) => anyhow::bail!("server closed the connection"),
                Ok(Err(e)) => return Err(e),
                Err(_) => anyhow::bail!("server silent for 10 minutes"),
            };
            let msg = match p::parse_server(code, &payload) { Ok(m) => m, Err(e) => { tracing::debug!("soulseek: bad server message {code}: {e}"); continue; } };
            match msg {
                ServerMsg::LoginOk { greet, ip, supporter } => {
                    tracing::info!("soulseek: logged in as {} ({})", st.username, greet.lines().next().unwrap_or(""));
                    {
                        let mut s = self.inner.state.lock().unwrap();
                        s.logged_in = true;
                        s.ip = Some(ip);
                        s.supporter = supporter;
                        s.since = now();
                        s.error.clear();
                    }
                    let sh = self.inner.shares.read().unwrap().clone();
                    for m in [p::set_wait_port(st.listen_port), p::set_status(true), p::shared_folders_files(sh.dir_count() as u32, sh.file_count() as u32),
                              p::have_no_parent(true), p::accept_children(false), p::check_privileges()] {
                        tx.send(m)?;
                    }
                }
                ServerMsg::LoginFailed { reason } => anyhow::bail!("login refused: {reason}"),
                ServerMsg::Relogged => anyhow::bail!("this account signed in elsewhere (is slskd still running with the same login?)"),
                ServerMsg::PeerAddress { user, ip, port } => {
                    let v = if ip.is_unspecified() || port == 0 { None } else { Some((ip, port)) };
                    if let Some(ws) = self.inner.addr_waiters.lock().unwrap().remove(&user) {
                        for w in ws {
                            let _ = w.send(v);
                        }
                    }
                }
                ServerMsg::ConnectToPeer { user, kind, ip, port, token } => {
                    tokio::spawn(self.clone().pierce(user, kind, ip, port, token));
                }
                ServerMsg::CantConnectToPeer { token, .. } => {
                    if let Some(w) = self.inner.indirect.lock().unwrap().remove(&token) {
                        let _ = w.send(Err("the server could not reach them".into()));
                    }
                }
                ServerMsg::EmbeddedSearch { user, token, query } => self.answer_search(user, token, query),
                ServerMsg::PossibleParents(list) => {
                    if self.inner.state.lock().unwrap().parent.is_none() {
                        tokio::spawn(self.clone().find_parent(list));
                    }
                }
                ServerMsg::ResetDistributed => {
                    self.inner.parent_gen.fetch_add(1, Ordering::Relaxed);
                    self.inner.state.lock().unwrap().parent = None;
                    let _ = tx.send(p::have_no_parent(true));
                }
                ServerMsg::ParentMinSpeed(_) | ServerMsg::ParentSpeedRatio(_) | ServerMsg::WishlistInterval(_) | ServerMsg::Other(_) => {}
            }
        }
    }

    /// The server says a peer wants us to connect to them (they couldn't reach us, or we asked).
    async fn pierce(self, user: String, kind: String, ip: Ipv4Addr, port: u16, token: u32) {
        let mut stream = match tokio::time::timeout(CONNECT_TIMEOUT, self.inner.settings.net().connect(SocketAddr::new(IpAddr::V4(ip), port))).await {
            Ok(Ok(s)) => s,
            _ => { self.inner.send_server(p::cant_connect_to_peer(token, &user)); return; }
        };
        if stream.write_all(&p::pierce_firewall(token)).await.is_err() {
            self.inner.send_server(p::cant_connect_to_peer(token, &user));
            return;
        }
        match kind.as_str() {
            "P" => self.run_peer(user, stream).await,
            "F" => self.file_stream(user, stream).await,
            _ => {}
        }
    }
}

// ---- listener and peer connections ---------------------------------------------------------------

/// Accept on `port` until `stop`, handing each connection to `take`. A restarted client races the
/// old one's listener going away, so binding is retried for 10 s before `give_up`. A listener on
/// the VPN dies with its tunnel (the tunnel restarts when its settings change): then this listens
/// again on the next tunnel, as soon as one is up.
pub(crate) async fn serve(net: Net, port: u16, stop: &mut watch::Receiver<bool>, mut take: impl FnMut(Conn), give_up: impl FnOnce(String)) {
    let mut listener = None;
    for i in 0..40 {
        match net.listen(port).await {
            Ok(l) => { listener = Some(l); break; }
            Err(e) => {
                if i == 39 || *stop.borrow() {
                    give_up(e.to_string());
                    return;
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        }
    }
    let mut listener = listener.expect("set above");
    loop {
        let r = tokio::select! {
            r = listener.accept() => r,
            _ = stop.changed() => return,
        };
        match r {
            Ok(c) => take(c),
            Err(e) if e.kind() == std::io::ErrorKind::NotConnected => {
                drop(listener);
                tracing::info!("soulseek: the VPN tunnel restarted – listening on port {port} again once it is up");
                let mut wait = Duration::from_millis(250);
                listener = loop {
                    tokio::select! {
                        _ = tokio::time::sleep(wait) => {}
                        _ = stop.changed() => return,
                    }
                    match net.listen(port).await {
                        Ok(l) => break l,
                        Err(_) => wait = (wait * 2).min(Duration::from_secs(5)),
                    }
                };
                tracing::info!("soulseek: listening on port {port} through the VPN again");
            }
            Err(_) => {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_millis(200)) => {}
                    _ = stop.changed() => return,
                }
            }
        }
    }
}

impl Client {
    async fn listen(self) {
        let port = self.inner.settings.listen_port;
        let mut stop = self.inner.stop.subscribe();
        let me = self.clone();
        let take = move |c: Conn| {
            tokio::spawn(me.clone().incoming(c));
        };
        let give_up = |e: String| {
            tracing::warn!("soulseek: can't listen on port {port}: {e} – only users who accept incoming connections will work");
            self.inner.state.lock().unwrap().error = format!("listen port {port} unavailable: {e}");
        };
        serve(self.inner.settings.net(), port, &mut stop, take, give_up).await;
    }

    async fn incoming(self, mut stream: Conn) {
        let init = match tokio::time::timeout(Duration::from_secs(10), read_frame(&mut stream, true)).await {
            Ok(Ok(Some((code, payload)))) => p::parse_init(code as u8, &payload).ok(),
            _ => None,
        };
        let Some(init) = init else { return };
        match init {
            InitMsg::PierceFireWall(token) => {
                if let Some(w) = self.inner.indirect.lock().unwrap().remove(&token) {
                    let _ = w.send(Ok(stream));
                }
            }
            InitMsg::PeerInit { user, kind, .. } => match kind.as_str() {
                "P" => self.run_peer(user, stream).await,
                "F" => self.file_stream(user, stream).await,
                _ => {} // "D": we don't take children
            },
        }
    }

    /// Ask the server where a user is.
    async fn peer_address(&self, user: &str) -> anyhow::Result<(Ipv4Addr, u16)> {
        let (tx, rx) = oneshot::channel();
        self.inner.addr_waiters.lock().unwrap().entry(user.to_string()).or_default().push(tx);
        if !self.inner.send_server(p::get_peer_address(user)) {
            anyhow::bail!("not connected to the Soulseek server");
        }
        match tokio::time::timeout(Duration::from_secs(8), rx).await {
            Ok(Ok(Some(a))) => Ok(a),
            Ok(Ok(None)) => anyhow::bail!("{user} is offline"),
            _ => anyhow::bail!("no address for {user}"),
        }
    }

    /// A fresh connection of `kind` to a user: direct, else through the server.
    async fn connect_peer(&self, user: &str, kind: &str) -> anyhow::Result<Conn> {
        let me = &self.inner.settings.username;
        let direct = async {
            let (ip, port) = self.peer_address(user).await?;
            let mut s = tokio::time::timeout(CONNECT_TIMEOUT, self.inner.settings.net().connect(SocketAddr::new(IpAddr::V4(ip), port))).await
                .map_err(|_| anyhow::anyhow!("connect timed out"))??;
            s.write_all(&p::peer_init(me, kind, token())).await?;
            anyhow::Ok(s)
        };
        let err = match direct.await { Ok(s) => return Ok(s), Err(e) => e };
        // indirect: the server asks them to connect to our listener with our token
        let t = token();
        let (tx, rx) = oneshot::channel();
        self.inner.indirect.lock().unwrap().insert(t, tx);
        if !self.inner.send_server(p::connect_to_peer(t, user, kind)) {
            self.inner.indirect.lock().unwrap().remove(&t);
            anyhow::bail!("not connected to the Soulseek server");
        }
        match tokio::time::timeout(INDIRECT_TIMEOUT, rx).await {
            Ok(Ok(Ok(s))) => Ok(s),
            Ok(Ok(Err(e))) => anyhow::bail!("{e} ({err})"),
            _ => {
                self.inner.indirect.lock().unwrap().remove(&t);
                anyhow::bail!("{user} unreachable ({err}; no answer to the indirect connection either – is port {} forwarded?)", self.inner.settings.listen_port)
            }
        }
    }

    /// The message channel to a user: the open "P" connection, or a new one.
    async fn peer(&self, user: &str) -> anyhow::Result<mpsc::UnboundedSender<Vec<u8>>> {
        if let Some(p) = self.inner.peers.lock().unwrap().get(user) {
            if !p.tx.is_closed() {
                return Ok(p.tx.clone());
            }
        }
        let stream = self.connect_peer(user, "P").await?;
        let (rd, wr) = tokio::io::split(stream);
        let tx = writer(wr);
        self.inner.peers.lock().unwrap().insert(user.to_string(), Peer { tx: tx.clone() });
        tokio::spawn(self.clone().peer_reader(user.to_string(), rd, tx.clone()));
        Ok(tx)
    }

    async fn run_peer(self, user: String, stream: Conn) {
        let (rd, wr) = tokio::io::split(stream);
        let tx = writer(wr);
        self.inner.peers.lock().unwrap().insert(user.clone(), Peer { tx: tx.clone() });
        self.peer_reader(user, rd, tx).await;
    }

    async fn peer_reader(self, user: String, mut rd: ReadHalf<Conn>, tx: mpsc::UnboundedSender<Vec<u8>>) {
        loop {
            let (code, payload) = match tokio::time::timeout(PEER_IDLE, read_frame(&mut rd, false)).await {
                Ok(Ok(Some(f))) => f,
                _ => break,
            };
            if *self.inner.stop.borrow() { break; }
            let msg = match p::parse_peer(code, &payload) { Ok(m) => m, Err(_) => continue };
            self.on_peer_msg(&user, &tx, msg).await;
        }
        let mut peers = self.inner.peers.lock().unwrap();
        if peers.get(&user).map(|p| p.tx.same_channel(&tx)).unwrap_or(false) {
            peers.remove(&user);
        }
    }

    async fn on_peer_msg(&self, user: &str, tx: &mpsc::UnboundedSender<Vec<u8>>, msg: PeerMsg) {
        let inner = &self.inner;
        match msg {
            PeerMsg::GetShareFileList => {
                let sh = inner.shares.read().unwrap().clone();
                let _ = tx.send(p::share_file_list(if inner.settings.share { sh.list() } else { &[] }));
            }
            PeerMsg::FileSearchRequest { token, query } => {
                if inner.settings.share {
                    let files = inner.shares.read().unwrap().search(&query, MAX_FILES_PER_ANSWER);
                    if !files.is_empty() {
                        let _ = tx.send(p::file_search_response(&inner.search_result(token, files)));
                    }
                }
            }
            PeerMsg::FileSearchResponse(r) => {
                let mut s = inner.searches.lock().unwrap();
                if let Some(x) = s.get_mut(&r.token) {
                    if x.results.len() < MAX_RESULTS_PER_SEARCH && !r.files.is_empty() {
                        x.results.push(r);
                    }
                }
            }
            PeerMsg::UserInfoRequest => {
                let active = inner.active_uploads() as u32;
                let _ = tx.send(p::user_info_reply("Encore music server – encore shares its library; be nice and share yours", active, inner.queue_len() as u32, inner.free_slot()));
            }
            PeerMsg::FolderContentsRequest { token, folder } => {
                let dirs = if inner.settings.share { inner.shares.read().unwrap().folder(&folder) } else { vec![] };
                let _ = tx.send(p::folder_contents_reply(token, &folder, &dirs));
            }
            PeerMsg::TransferRequest { direction: 1, token, file, size } => {
                // they're ready to send us a file we queued
                match inner.download_by_file(user, &file) {
                    Some(d) => {
                        let (ftx, frx) = oneshot::channel();
                        // tokens are the uploader's choice, so they're only unique per user
                        inner.file_waits.lock().unwrap().insert((user.to_string(), token), ftx);
                        d.set(|x| { x.token = Some(token); if size > 0 { x.size = size; } x.state = "Initializing".into(); });
                        let ready = d.ready.lock().unwrap().take();
                        if let Some(r) = ready {
                            let _ = r.send((token, size));
                        }
                        d.wake.notify_waiters();
                        let _ = tx.send(p::transfer_response(token, true, ""));
                        // the F connection lands in file_stream(); hand it to the task through the download
                        tokio::spawn(self.clone().await_file(d, token, frx));
                    }
                    None => { let _ = tx.send(p::transfer_response(token, false, "Cancelled")); }
                }
            }
            PeerMsg::TransferRequest { token, file, .. } => {
                // old-style download request: queue it, they'll get our TransferRequest when a slot frees
                let _ = tx.send(p::transfer_response(token, false, "Queued"));
                self.queue_upload(user, &file, tx);
            }
            PeerMsg::TransferResponse { token, allowed, reason } => {
                if let Some(w) = inner.transfer_waits.lock().unwrap().remove(&token) {
                    let _ = w.send(if allowed { Ok(()) } else { Err(reason) });
                }
            }
            PeerMsg::QueueUpload { file } => self.queue_upload(user, &file, tx),
            PeerMsg::PlaceInQueueRequest { file } => {
                let place = inner.uploads.lock().unwrap().iter().filter(|u| u.state != "InProgress").position(|u| u.user == user && u.file == file).map(|i| i as u32 + 1).unwrap_or(0);
                let _ = tx.send(p::place_in_queue_reply(&file, place));
            }
            PeerMsg::PlaceInQueueReply { file, place } => {
                if let Some(d) = inner.download_by_file(user, &file) {
                    d.set(|x| x.place = Some(place));
                }
            }
            PeerMsg::UploadFailed { file } => {
                if let Some(d) = inner.download_by_file(user, &file) {
                    d.requeue.store(true, Ordering::Relaxed);
                    d.wake.notify_waiters();
                }
            }
            PeerMsg::UploadDenied { file, reason } => {
                if let Some(d) = inner.download_by_file(user, &file) {
                    let fatal = !reason.eq_ignore_ascii_case("Queued");
                    if fatal {
                        d.set(|x| { x.state = "Completed, Rejected".into(); x.error = reason.clone(); });
                        d.cancelled.store(true, Ordering::Relaxed);
                        d.wake.notify_waiters();
                    }
                }
            }
            PeerMsg::ShareFileList { dirs, .. } => {
                if let Some(ws) = inner.browse_waits.lock().unwrap().remove(user) {
                    for w in ws {
                        let _ = w.send(dirs.clone());
                    }
                }
            }
            PeerMsg::UserInfoReply { .. } | PeerMsg::FolderContentsReply { .. } | PeerMsg::Other(_) => {}
        }
    }

    fn queue_upload(&self, user: &str, file: &str, tx: &mpsc::UnboundedSender<Vec<u8>>) {
        let inner = &self.inner;
        let resolved = if inner.settings.share { inner.shares.read().unwrap().resolve(file) } else { None };
        match resolved {
            Some((path, size)) if path.is_file() => {
                let mut ups = inner.uploads.lock().unwrap();
                if !ups.iter().any(|u| u.user == user && u.file == file) {
                    let id = inner.next_id.fetch_add(1, Ordering::Relaxed);
                    ups.push(Upload { id, user: user.to_string(), file: file.to_string(), path, size, bytes: 0, state: "Queued".into(), retry_at: None });
                }
            }
            _ => { let _ = tx.send(p::upload_denied(file, "File not shared.")); }
        }
    }

    /// Someone asked for our files (through our parent, or the server directly): answer them.
    fn answer_search(&self, user: String, token: u32, query: String) {
        if !self.inner.settings.share || user == self.inner.settings.username || query.trim().len() < 3 {
            return;
        }
        let files = self.inner.shares.read().unwrap().search(&query, MAX_FILES_PER_ANSWER);
        if files.is_empty() {
            return;
        }
        let c = self.clone();
        tokio::spawn(async move {
            let Ok(_permit) = c.inner.answers.clone().try_acquire_owned() else { return };
            if let Ok(tx) = c.peer(&user).await {
                let _ = tx.send(p::file_search_response(&c.inner.search_result(token, files)));
            }
        });
    }
}

// ---- downloads -----------------------------------------------------------------------------------

impl Client {
    /// Waits for the uploader's "F" connection and runs the transfer.
    async fn await_file(self, d: Arc<Download>, token: u32, frx: oneshot::Receiver<Conn>) {
        let stream = tokio::select! {
            r = frx => match r { Ok(s) => s, Err(_) => return },
            _ = tokio::time::sleep(Duration::from_secs(120)) => {
                self.inner.file_waits.lock().unwrap().remove(&(d.user.clone(), token));
                // only give up if no newer TransferRequest has taken over this download
                if d.prog.lock().unwrap().token == Some(token) {
                    d.set(|x| { if x.state == "Initializing" { x.state = "Queued, Remotely".into(); x.error = "the uploader never opened the file connection".into(); } });
                    d.requeue.store(true, Ordering::Relaxed);
                    d.wake.notify_waiters();
                }
                return;
            }
        };
        self.receive(d, stream).await;
    }

    /// An "F" connection from `user`: the first 4 bytes name the transfer.
    async fn file_stream(self, user: String, mut stream: Conn) {
        let mut t = [0u8; 4];
        if tokio::time::timeout(Duration::from_secs(30), stream.read_exact(&mut t)).await.is_err() {
            return;
        }
        let token = u32::from_le_bytes(t);
        if let Some(w) = self.inner.file_waits.lock().unwrap().remove(&(user, token)) {
            let _ = w.send(stream);
        }
    }

    fn local_target(&self, d: &Download) -> PathBuf {
        local_file_for(&self.inner.settings.download_dir, &d.file)
    }

    fn part_path(&self, d: &Download) -> PathBuf {
        part_file_for(&self.inner.settings.download_dir, &d.user, &d.file, d.size())
    }

    async fn receive(&self, d: Arc<Download>, mut stream: Conn) {
        let target = self.local_target(&d);
        let part = self.part_path(&d);
        if let Some(dir) = target.parent() {
            let _ = tokio::fs::create_dir_all(dir).await;
        }
        let mut file = match tokio::fs::OpenOptions::new().create(true).append(true).open(&part).await {
            Ok(f) => f,
            Err(e) => { d.set(|x| { x.state = "Completed, Errored".into(); x.error = format!("can't write {}: {e}", part.display()); }); return; }
        };
        let size = d.size();
        let mut have = file.metadata().await.map(|m| m.len()).unwrap_or(0);
        if size > 0 && have >= size {
            have = 0;
            let _ = file.set_len(0).await;
        }
        if stream.write_all(&have.to_le_bytes()).await.is_err() {
            d.set(|x| { x.state = "Completed, Errored".into(); x.error = "connection lost before the transfer started".into(); });
            return;
        }
        d.set(|x| { x.state = "InProgress".into(); x.bytes = have; });
        let mut buf = vec![0u8; 128 * 1024];
        let (mut got, t0) = (have, Instant::now());
        let mut last_tick = Instant::now();
        let outcome: Result<(), String> = loop {
            if d.cancelled.load(Ordering::Relaxed) {
                break Err("cancelled".into());
            }
            if size > 0 && got >= size {
                break Ok(());
            }
            let n = match tokio::time::timeout(READ_TIMEOUT, stream.read(&mut buf)).await {
                Ok(Ok(0)) => break if size == 0 { Ok(()) } else { Err(format!("connection closed at {}%", got * 100 / size.max(1))) },
                Ok(Ok(n)) => n,
                Ok(Err(e)) => break Err(e.to_string()),
                Err(_) => break Err("no data for 60 s".into()),
            };
            let n = if size > 0 { n.min((size - got) as usize) } else { n };
            if let Err(e) = file.write_all(&buf[..n]).await {
                break Err(format!("write failed: {e}"));
            }
            got += n as u64;
            if last_tick.elapsed() > Duration::from_millis(500) {
                let speed = (got - have) as f64 / t0.elapsed().as_secs_f64().max(0.001);
                d.set(|x| { x.bytes = got; x.speed = speed; });
                last_tick = Instant::now();
            }
        };
        let _ = file.flush().await;
        drop(file);
        match outcome {
            Ok(()) => {
                let speed = (got - have) as f64 / t0.elapsed().as_secs_f64().max(0.001);
                match tokio::fs::rename(&part, &target).await {
                    Ok(()) => d.set(|x| { x.bytes = got; x.size = got; x.speed = speed; x.state = "Completed, Succeeded".into(); }),
                    Err(e) => d.set(|x| { x.state = "Completed, Errored".into(); x.error = format!("can't move {} into place: {e}", part.display()); }),
                }
            }
            Err(e) if e == "cancelled" => {
                let _ = tokio::fs::remove_file(&part).await;
                d.set(|x| x.state = "Completed, Cancelled".into());
            }
            Err(e) => {
                // keep the .part for a resume; the task re-queues once
                d.set(|x| { x.bytes = got; x.error = e; });
                d.requeue.store(true, Ordering::Relaxed);
            }
        }
        d.wake.notify_waiters();
    }

    async fn download_task(self, d: Arc<Download>) {
        let inner = &self.inner;
        let mut stop = inner.stop.subscribe();
        let mut attempts = 0;
        'outer: loop {
            attempts += 1;
            if *stop.borrow() { break; }
            let tx = match self.peer(&d.user).await {
                Ok(tx) => tx,
                Err(e) => { d.set(|x| { x.state = "Completed, Errored".into(); x.error = e.to_string(); }); break; }
            };
            let (rtx, rrx) = oneshot::channel();
            *d.ready.lock().unwrap() = Some(rtx);
            d.requeue.store(false, Ordering::Relaxed);
            if tx.send(p::queue_upload(&d.file)).is_err() {
                inner.peers.lock().unwrap().remove(&d.user);
                if attempts < 3 { continue; }
                d.set(|x| { x.state = "Completed, Errored".into(); x.error = "lost the connection".into(); });
                break;
            }
            d.set(|x| x.state = "Queued, Remotely".into());
            // wait for the uploader's TransferRequest (then the F connection), asking our place now and then
            let mut rrx = rrx;
            let mut last_place = Instant::now();
            let mut transferring = false;
            loop {
                if d.cancelled.load(Ordering::Relaxed) || d.done() { break 'outer; }
                if *stop.borrow() { break 'outer; }
                if d.requeue.load(Ordering::Relaxed) { break; }
                if transferring {
                    // the transfer runs in await_file/receive; just watch the flags
                    let _ = tokio::time::timeout(Duration::from_secs(2), d.wake.notified()).await;
                    continue;
                }
                tokio::select! {
                    r = &mut rrx => {
                        if r.is_err() { break; } // handler dropped it: re-queue
                        transferring = true;
                    }
                    _ = d.wake.notified() => {}
                    _ = stop.changed() => { break 'outer; }
                    _ = tokio::time::sleep(Duration::from_secs(5)) => {
                        if last_place.elapsed() > Duration::from_secs(60) {
                            if let Some(p) = inner.peers.lock().unwrap().get(&d.user).cloned() { let _ = p.tx.send(p::place_in_queue_request(&d.file)); }
                            last_place = Instant::now();
                        }
                    }
                }
            }
            if attempts >= 3 {
                d.set(|x| { x.state = "Completed, Errored".into(); if x.error.is_empty() { x.error = "the upload failed three times".into(); } });
                break;
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
        // whatever is left of a failed or cancelled transfer is of no use to anyone else
        let st = d.state();
        if st == "Completed, Errored" || st == "Completed, Rejected" || (d.cancelled.load(Ordering::Relaxed) && st != "InProgress") {
            let _ = tokio::fs::remove_file(self.part_path(&d)).await;
        }
    }
}

// ---- uploads -------------------------------------------------------------------------------------

impl Client {
    async fn upload_pump(self) {
        let mut stop = self.inner.stop.subscribe();
        loop {
            tokio::select! { _ = tokio::time::sleep(Duration::from_secs(1)) => {}, _ = stop.changed() => return }
            if !self.logged_in() || !self.inner.free_slot() {
                continue;
            }
            let next = {
                let mut ups = self.inner.uploads.lock().unwrap();
                match ups.iter_mut().find(|u| u.state == "Queued" && u.retry_at.map(|t| t <= Instant::now()).unwrap_or(true)) {
                    Some(u) => { u.state = "Requested".into(); Some(u.id) }
                    None => None,
                }
            };
            if let Some(id) = next {
                tokio::spawn(self.clone().upload_task(id));
            }
        }
    }

    fn upload_set(&self, id: u64, f: impl FnOnce(&mut Upload)) {
        if let Some(u) = self.inner.uploads.lock().unwrap().iter_mut().find(|u| u.id == id) {
            f(u);
        }
    }

    async fn upload_task(self, id: u64) {
        let inner = &self.inner;
        let Some((user, file, path, size)) = inner.uploads.lock().unwrap().iter().find(|u| u.id == id).map(|u| (u.user.clone(), u.file.clone(), u.path.clone(), u.size)) else { return };
        let done = |why: &str| { tracing::debug!("soulseek: upload {file} to {user}: {why}"); inner.uploads.lock().unwrap().retain(|u| u.id != id); };
        let Ok(tx) = self.peer(&user).await else { return done("unreachable") };
        // the file may have been re-tagged since the share index was built
        let size = tokio::fs::metadata(&path).await.map(|m| m.len()).unwrap_or(size);
        let t = token();
        let (wtx, wrx) = oneshot::channel();
        inner.transfer_waits.lock().unwrap().insert(t, wtx);
        if tx.send(p::transfer_request_upload(t, &file, size)).is_err() {
            inner.transfer_waits.lock().unwrap().remove(&t);
            return done("connection lost");
        }
        match tokio::time::timeout(Duration::from_secs(60), wrx).await {
            Ok(Ok(Ok(()))) => {}
            Ok(Ok(Err(reason))) if reason.eq_ignore_ascii_case("Queued") => {
                self.upload_set(id, |u| { u.state = "Queued".into(); u.retry_at = Some(Instant::now() + Duration::from_secs(60)); });
                return;
            }
            Ok(Ok(Err(reason))) => return done(&reason),
            _ => { inner.transfer_waits.lock().unwrap().remove(&t); self.upload_set(id, |u| { u.state = "Queued".into(); u.retry_at = Some(Instant::now() + Duration::from_secs(120)); }); return; }
        }
        // from here on the peer is waiting for us: a failure must be told, or they wait it out
        let failed = |why: &str| { let _ = tx.send(p::upload_failed(&file)); done(why) };
        let Ok(mut stream) = self.connect_peer(&user, "F").await else { return failed("no file connection") };
        if stream.write_all(&t.to_le_bytes()).await.is_err() {
            return failed("connection lost");
        }
        let mut off = [0u8; 8];
        if tokio::time::timeout(Duration::from_secs(30), stream.read_exact(&mut off)).await.is_err() {
            return failed("no offset");
        }
        let offset = u64::from_le_bytes(off).min(size);
        let Ok(mut f) = tokio::fs::File::open(&path).await else { let _ = tx.send(p::upload_denied(&file, "File not shared.")); return done("file vanished") };
        if f.seek(std::io::SeekFrom::Start(offset)).await.is_err() {
            return failed("seek failed");
        }
        self.upload_set(id, |u| { u.state = "InProgress".into(); u.bytes = offset; });
        let mut buf = vec![0u8; 256 * 1024];
        let (mut sent, t0) = (offset, Instant::now());
        let ok = loop {
            let n = match f.read(&mut buf).await { Ok(0) => break true, Ok(n) => n, Err(_) => break false };
            if tokio::time::timeout(READ_TIMEOUT, stream.write_all(&buf[..n])).await.map(|r| r.is_err()).unwrap_or(true) {
                break false;
            }
            sent += n as u64;
            self.upload_set(id, |u| u.bytes = sent);
        };
        let _ = stream.shutdown().await;
        if !ok {
            let _ = tx.send(p::upload_failed(&file));
        }
        if ok && sent > offset {
            let mut s = inner.upload_speed.lock().unwrap();
            s.0 = s.0 / 2 + (sent - offset);
            s.1 = s.1 / 2.0 + t0.elapsed().as_secs_f64();
        }
        done(if ok { "done" } else { "interrupted" });
    }
}

// ---- distributed network -------------------------------------------------------------------------

impl Client {
    async fn find_parent(self, list: Vec<(String, Ipv4Addr, u16)>) {
        let gen = self.inner.parent_gen.load(Ordering::Relaxed);
        for (user, ip, port) in list.into_iter().take(10) {
            if self.inner.state.lock().unwrap().parent.is_some() || gen != self.inner.parent_gen.load(Ordering::Relaxed) {
                return;
            }
            let Ok(Ok(mut stream)) = tokio::time::timeout(CONNECT_TIMEOUT, self.inner.settings.net().connect(SocketAddr::new(IpAddr::V4(ip), port))).await else { continue };
            if stream.write_all(&p::peer_init(&self.inner.settings.username, "D", token())).await.is_err() {
                continue;
            }
            // the parent speaks first (branch level/root); otherwise it's not one
            let Ok(Ok(Some((code, payload)))) = tokio::time::timeout(Duration::from_secs(10), read_frame(&mut stream, true)).await else { continue };
            let Ok(first) = p::parse_distrib(code as u8, &payload) else { continue };
            {
                let mut s = self.inner.state.lock().unwrap();
                if s.parent.is_some() { return; }
                s.parent = Some(user.clone());
            }
            self.inner.send_server(p::have_no_parent(false));
            tracing::info!("soulseek: parent {user} in the distributed network");
            self.on_distrib(&user, first);
            loop {
                let msg = match tokio::time::timeout(Duration::from_secs(300), read_frame(&mut stream, true)).await {
                    Ok(Ok(Some((code, payload)))) => match p::parse_distrib(code as u8, &payload) { Ok(m) => m, Err(_) => continue },
                    _ => break,
                };
                if gen != self.inner.parent_gen.load(Ordering::Relaxed) || *self.inner.stop.borrow() {
                    break;
                }
                self.on_distrib(&user, msg);
            }
            {
                let mut s = self.inner.state.lock().unwrap();
                if s.parent.as_deref() == Some(user.as_str()) {
                    s.parent = None;
                    s.branch_level = 0;
                }
            }
            self.inner.send_server(p::have_no_parent(true));
            return;
        }
    }

    fn on_distrib(&self, _parent: &str, msg: DistribMsg) {
        match msg {
            DistribMsg::Search { user, token, query } => self.answer_search(user, token, query),
            DistribMsg::BranchLevel(l) => {
                self.inner.state.lock().unwrap().branch_level = l + 1;
                self.inner.send_server(p::branch_level((l + 1).max(0) as u32));
            }
            DistribMsg::BranchRoot(r) => { self.inner.send_server(p::branch_root(&r)); }
            DistribMsg::ChildDepth(_) | DistribMsg::Ping | DistribMsg::Other(_) => {}
        }
    }
}

/// One file in slskd's shape; `name` is the full remote path.
fn file_json(name: &str, f: &FileEntry) -> Value {
    json!({
        "filename": name, "size": f.size, "extension": f.ext,
        "bitRate": f.attr(p::ATTR_BITRATE), "length": f.attr(p::ATTR_DURATION), "sampleRate": f.attr(p::ATTR_SAMPLE_RATE), "bitDepth": f.attr(p::ATTR_BIT_DEPTH),
        "isVariableBitRate": f.attr(p::ATTR_VBR).map(|v| v == 1) })
}

/// Convert a search result to the JSON shape the slskd code paths consume.
pub fn result_json(r: &SearchResult) -> Value {
    json!({
        "username": r.user, "hasFreeUploadSlot": r.free_slot, "uploadSpeed": r.upload_speed, "queueLength": r.queue_length,
        "fileCount": r.files.len(),
        "files": r.files.iter().map(|f| file_json(&f.name, f)).collect::<Vec<_>>(),
    })
}

/// A browsed folder in the shape of slskd's GET /users/{u}/browse `directories[]`. Share lists
/// carry bare file names (Encore's own carry full paths); either way the JSON gets the full path,
/// which is what a download is queued by.
pub fn dir_json(d: &Dir) -> Value {
    let files: Vec<Value> = d.files.iter().map(|f| {
        if f.name.contains('\\') { file_json(&f.name, f) } else { file_json(&format!("{}\\{}", d.name.trim_end_matches('\\'), f.name), f) }
    }).collect();
    json!({ "name": d.name, "fileCount": files.len(), "files": files })
}

pub fn local_file_for(dir: &Path, remote: &str) -> PathBuf {
    let (parent, base) = split_remote(remote);
    dir.join(safe_component(&parent)).join(safe_component(&base))
}

/// Where a transfer in progress is written. Named after the uploader, the remote path and the size,
/// so a resume only ever continues the same file from the same user (and survives a restart).
pub fn part_file_for(dir: &Path, user: &str, remote: &str, size: u64) -> PathBuf {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in user.as_bytes().iter().chain(b"\0").chain(remote.as_bytes()).chain(b"\0").chain(size.to_string().as_bytes()) {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    let target = local_file_for(dir, remote);
    let ext = target.extension().map(|e| e.to_string_lossy().to_string()).unwrap_or_default();
    target.with_extension(format!("{ext}.{:08x}.part", (h >> 32) as u32 ^ h as u32))
}
