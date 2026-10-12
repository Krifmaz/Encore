//! Soulseek wire format: every message is `u32 length` + `code` + payload, little-endian throughout.
//! Server and peer messages use a u32 code; connection-init and distributed messages a u8 code.
//! Strings are `u32 length` + bytes. Shared-file lists and search results are zlib-compressed.
//! Layouts follow Nicotine+'s SLSKPROTOCOL document, which is what every live client speaks.
use std::io::{Read, Write};
use std::net::Ipv4Addr;

/// Client version numbers sent at login (what current clients send; the server only checks a minimum).
pub const VERSION: u32 = 160;
pub const MINOR_VERSION: u32 = 1;

/// File attribute keys in shared-file entries.
pub const ATTR_BITRATE: u32 = 0;
pub const ATTR_DURATION: u32 = 1;
pub const ATTR_VBR: u32 = 2;
pub const ATTR_SAMPLE_RATE: u32 = 4;
pub const ATTR_BIT_DEPTH: u32 = 5;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FileEntry {
    pub name: String,
    pub size: u64,
    pub ext: String,
    pub attrs: Vec<(u32, u32)>,
}

impl FileEntry {
    pub fn attr(&self, key: u32) -> Option<u32> {
        self.attrs.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Dir {
    pub name: String,
    pub files: Vec<FileEntry>,
}

// ---- primitives ----------------------------------------------------------------------------------

#[derive(Default)]
pub struct W(pub Vec<u8>);

impl W {
    pub fn u8(&mut self, v: u8) -> &mut Self { self.0.push(v); self }
    pub fn bool(&mut self, v: bool) -> &mut Self { self.u8(v as u8) }
    pub fn u16(&mut self, v: u16) -> &mut Self { self.0.extend_from_slice(&v.to_le_bytes()); self }
    pub fn u32(&mut self, v: u32) -> &mut Self { self.0.extend_from_slice(&v.to_le_bytes()); self }
    pub fn i32(&mut self, v: i32) -> &mut Self { self.0.extend_from_slice(&v.to_le_bytes()); self }
    pub fn u64(&mut self, v: u64) -> &mut Self { self.0.extend_from_slice(&v.to_le_bytes()); self }
    pub fn str(&mut self, s: &str) -> &mut Self { self.u32(s.len() as u32); self.0.extend_from_slice(s.as_bytes()); self }
    pub fn bytes(&mut self, b: &[u8]) -> &mut Self { self.0.extend_from_slice(b); self }
    /// IPs travel as a u32 whose little-endian bytes are the octets in reverse.
    pub fn ip(&mut self, ip: Ipv4Addr) -> &mut Self { self.u32(u32::from(ip)) }
    pub fn file(&mut self, f: &FileEntry) -> &mut Self {
        self.u8(1).str(&f.name).u64(f.size).str(&f.ext).u32(f.attrs.len() as u32);
        for (k, v) in &f.attrs {
            self.u32(*k).u32(*v);
        }
        self
    }
    pub fn dir(&mut self, d: &Dir) -> &mut Self {
        self.str(&d.name).u32(d.files.len() as u32);
        for f in &d.files {
            self.file(f);
        }
        self
    }
}

pub struct R<'a> { pub b: &'a [u8], pub pos: usize }

impl<'a> R<'a> {
    pub fn new(b: &'a [u8]) -> Self { R { b, pos: 0 } }
    pub fn left(&self) -> usize { self.b.len().saturating_sub(self.pos) }
    fn take(&mut self, n: usize) -> anyhow::Result<&'a [u8]> {
        if self.left() < n {
            anyhow::bail!("short message: need {n} bytes at {}, have {}", self.pos, self.left());
        }
        let s = &self.b[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    pub fn u8(&mut self) -> anyhow::Result<u8> { Ok(self.take(1)?[0]) }
    pub fn bool(&mut self) -> anyhow::Result<bool> { Ok(self.u8()? != 0) }
    pub fn u16(&mut self) -> anyhow::Result<u16> { Ok(u16::from_le_bytes(self.take(2)?.try_into()?)) }
    pub fn u32(&mut self) -> anyhow::Result<u32> { Ok(u32::from_le_bytes(self.take(4)?.try_into()?)) }
    pub fn i32(&mut self) -> anyhow::Result<i32> { Ok(i32::from_le_bytes(self.take(4)?.try_into()?)) }
    pub fn u64(&mut self) -> anyhow::Result<u64> { Ok(u64::from_le_bytes(self.take(8)?.try_into()?)) }
    pub fn str(&mut self) -> anyhow::Result<String> {
        let n = self.u32()? as usize;
        let b = self.take(n)?;
        // a few old clients still send Latin-1; never fail on it
        Ok(match std::str::from_utf8(b) { Ok(s) => s.to_string(), Err(_) => b.iter().map(|&c| c as char).collect() })
    }
    pub fn ip(&mut self) -> anyhow::Result<Ipv4Addr> { Ok(Ipv4Addr::from(self.u32()?)) }
    pub fn file(&mut self) -> anyhow::Result<FileEntry> {
        let _code = self.u8()?;
        let name = self.str()?;
        let size = self.u64()?;
        let ext = self.str()?;
        let n = self.u32()? as usize;
        let mut attrs = Vec::with_capacity(n.min(16));
        for _ in 0..n {
            attrs.push((self.u32()?, self.u32()?));
        }
        Ok(FileEntry { name, size, ext, attrs })
    }
    pub fn files(&mut self) -> anyhow::Result<Vec<FileEntry>> {
        let n = self.u32()? as usize;
        let mut v = Vec::with_capacity(n.min(1024));
        for _ in 0..n {
            v.push(self.file()?);
        }
        Ok(v)
    }
    pub fn dirs(&mut self) -> anyhow::Result<Vec<Dir>> {
        let n = self.u32()? as usize;
        let mut v = Vec::with_capacity(n.min(1024));
        for _ in 0..n {
            let name = self.str()?;
            let files = self.files()?;
            v.push(Dir { name, files });
        }
        Ok(v)
    }
}

pub fn zip(b: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    let _ = e.write_all(b);
    e.finish().unwrap_or_default()
}

pub fn unzip(b: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut out = Vec::new();
    flate2::read::ZlibDecoder::new(b).take(64 << 20).read_to_end(&mut out)?;
    Ok(out)
}

/// A complete frame: code + payload, prefixed with its length. `code` is written as u32 or u8.
pub fn frame32(code: u32, payload: &[u8]) -> Vec<u8> {
    let mut w = W(Vec::with_capacity(payload.len() + 8));
    w.u32(payload.len() as u32 + 4).u32(code).bytes(payload);
    w.0
}

pub fn frame8(code: u8, payload: &[u8]) -> Vec<u8> {
    let mut w = W(Vec::with_capacity(payload.len() + 5));
    w.u32(payload.len() as u32 + 1).u8(code).bytes(payload);
    w.0
}

/// Largest frame we accept from anyone (shared-file lists of big users run to a few MB).
pub const MAX_FRAME: usize = 32 << 20;

// ---- server messages -----------------------------------------------------------------------------

pub mod server {
    pub const LOGIN: u32 = 1;
    pub const SET_WAIT_PORT: u32 = 2;
    pub const GET_PEER_ADDRESS: u32 = 3;
    pub const WATCH_USER: u32 = 5;
    pub const GET_USER_STATUS: u32 = 7;
    pub const CONNECT_TO_PEER: u32 = 18;
    pub const FILE_SEARCH: u32 = 26;
    pub const SET_STATUS: u32 = 28;
    pub const PING: u32 = 32;
    pub const SHARED_FOLDERS_FILES: u32 = 35;
    pub const GET_USER_STATS: u32 = 36;
    pub const RELOGGED: u32 = 41;
    pub const HAVE_NO_PARENT: u32 = 71;
    pub const PARENT_MIN_SPEED: u32 = 83;
    pub const PARENT_SPEED_RATIO: u32 = 84;
    pub const CHECK_PRIVILEGES: u32 = 92;
    pub const EMBEDDED_MESSAGE: u32 = 93;
    pub const ACCEPT_CHILDREN: u32 = 100;
    pub const POSSIBLE_PARENTS: u32 = 102;
    pub const WISHLIST_INTERVAL: u32 = 104;
    pub const BRANCH_LEVEL: u32 = 126;
    pub const BRANCH_ROOT: u32 = 127;
    pub const CHILD_DEPTH: u32 = 129;
    pub const RESET_DISTRIBUTED: u32 = 130;
    pub const CANT_CONNECT_TO_PEER: u32 = 1001;
}

/// Messages we send to the server.
pub fn login(username: &str, password: &str) -> Vec<u8> {
    let hash = format!("{:x}", md5::compute(format!("{username}{password}")));
    let mut w = W::default();
    w.str(username).str(password).u32(VERSION).str(&hash).u32(MINOR_VERSION);
    frame32(server::LOGIN, &w.0)
}
pub fn set_wait_port(port: u16) -> Vec<u8> {
    let mut w = W::default();
    w.u32(port as u32).u32(0).u32(0);
    frame32(server::SET_WAIT_PORT, &w.0)
}
pub fn get_peer_address(user: &str) -> Vec<u8> { frame32(server::GET_PEER_ADDRESS, &W::default().str(user).0) }
pub fn watch_user(user: &str) -> Vec<u8> { frame32(server::WATCH_USER, &W::default().str(user).0) }
pub fn connect_to_peer(token: u32, user: &str, kind: &str) -> Vec<u8> {
    frame32(server::CONNECT_TO_PEER, &W::default().u32(token).str(user).str(kind).0)
}
pub fn file_search(token: u32, query: &str) -> Vec<u8> { frame32(server::FILE_SEARCH, &W::default().u32(token).str(query).0) }
pub fn set_status(online: bool) -> Vec<u8> { frame32(server::SET_STATUS, &W::default().i32(if online { 2 } else { 1 }).0) }
pub fn ping() -> Vec<u8> { frame32(server::PING, &[]) }
pub fn shared_folders_files(dirs: u32, files: u32) -> Vec<u8> { frame32(server::SHARED_FOLDERS_FILES, &W::default().u32(dirs).u32(files).0) }
pub fn have_no_parent(v: bool) -> Vec<u8> { frame32(server::HAVE_NO_PARENT, &W::default().bool(v).0) }
pub fn accept_children(v: bool) -> Vec<u8> { frame32(server::ACCEPT_CHILDREN, &W::default().bool(v).0) }
pub fn branch_level(level: u32) -> Vec<u8> { frame32(server::BRANCH_LEVEL, &W::default().u32(level).0) }
pub fn branch_root(user: &str) -> Vec<u8> { frame32(server::BRANCH_ROOT, &W::default().str(user).0) }
pub fn child_depth(depth: u32) -> Vec<u8> { frame32(server::CHILD_DEPTH, &W::default().u32(depth).0) }
pub fn check_privileges() -> Vec<u8> { frame32(server::CHECK_PRIVILEGES, &[]) }
/// We couldn't reach a peer the server asked us to connect to; the server passes it on.
pub fn cant_connect_to_peer(token: u32, user: &str) -> Vec<u8> { frame32(server::CANT_CONNECT_TO_PEER, &W::default().u32(token).str(user).0) }

/// Messages the server sends us (only the ones we act on).
#[derive(Debug, Clone, PartialEq)]
pub enum ServerMsg {
    LoginOk { greet: String, ip: Ipv4Addr, supporter: bool },
    LoginFailed { reason: String },
    PeerAddress { user: String, ip: Ipv4Addr, port: u16 },
    ConnectToPeer { user: String, kind: String, ip: Ipv4Addr, port: u16, token: u32 },
    Relogged,
    EmbeddedSearch { user: String, token: u32, query: String },
    PossibleParents(Vec<(String, Ipv4Addr, u16)>),
    ResetDistributed,
    CantConnectToPeer { token: u32, user: String },
    ParentMinSpeed(u32),
    ParentSpeedRatio(u32),
    WishlistInterval(u32),
    Other(u32),
}

pub fn parse_server(code: u32, payload: &[u8]) -> anyhow::Result<ServerMsg> {
    let mut r = R::new(payload);
    Ok(match code {
        server::LOGIN => {
            if r.bool()? {
                let greet = r.str()?;
                let ip = r.ip()?;
                let _hash = r.str().unwrap_or_default();
                let supporter = r.bool().unwrap_or(false);
                ServerMsg::LoginOk { greet, ip, supporter }
            } else {
                ServerMsg::LoginFailed { reason: r.str().unwrap_or_else(|_| "login refused".into()) }
            }
        }
        server::GET_PEER_ADDRESS => {
            let user = r.str()?;
            let ip = r.ip()?;
            let port = r.u32()? as u16;
            ServerMsg::PeerAddress { user, ip, port }
        }
        server::CONNECT_TO_PEER => {
            let user = r.str()?;
            let kind = r.str()?;
            let ip = r.ip()?;
            let port = r.u32()? as u16;
            let token = r.u32()?;
            ServerMsg::ConnectToPeer { user, kind, ip, port, token }
        }
        server::RELOGGED => ServerMsg::Relogged,
        server::EMBEDDED_MESSAGE => {
            let dcode = r.u8()?;
            if dcode == distrib::SEARCH {
                let _unknown = r.u32()?;
                let user = r.str()?;
                let token = r.u32()?;
                let query = r.str()?;
                ServerMsg::EmbeddedSearch { user, token, query }
            } else {
                ServerMsg::Other(code)
            }
        }
        server::POSSIBLE_PARENTS => {
            let n = r.u32()? as usize;
            let mut v = Vec::with_capacity(n.min(64));
            for _ in 0..n {
                let user = r.str()?;
                let ip = r.ip()?;
                let port = r.u32()? as u16;
                v.push((user, ip, port));
            }
            ServerMsg::PossibleParents(v)
        }
        server::RESET_DISTRIBUTED => ServerMsg::ResetDistributed,
        // the server sends only the token (the username is in our outgoing form of this message)
        server::CANT_CONNECT_TO_PEER => {
            let token = r.u32()?;
            let user = r.str().unwrap_or_default();
            ServerMsg::CantConnectToPeer { token, user }
        }
        // a user- or room-targeted search aimed straight at us (outside the distributed network)
        server::FILE_SEARCH => {
            let user = r.str()?;
            let token = r.u32()?;
            let query = r.str()?;
            ServerMsg::EmbeddedSearch { user, token, query }
        }
        server::PARENT_MIN_SPEED => ServerMsg::ParentMinSpeed(r.u32()?),
        server::PARENT_SPEED_RATIO => ServerMsg::ParentSpeedRatio(r.u32()?),
        server::WISHLIST_INTERVAL => ServerMsg::WishlistInterval(r.u32()?),
        other => ServerMsg::Other(other),
    })
}

// ---- connection init (u8 codes) ------------------------------------------------------------------

pub mod init {
    pub const PIERCE_FIREWALL: u8 = 0;
    pub const PEER_INIT: u8 = 1;
}

pub fn pierce_firewall(token: u32) -> Vec<u8> { frame8(init::PIERCE_FIREWALL, &W::default().u32(token).0) }
pub fn peer_init(user: &str, kind: &str, token: u32) -> Vec<u8> { frame8(init::PEER_INIT, &W::default().str(user).str(kind).u32(token).0) }

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InitMsg {
    PierceFireWall(u32),
    PeerInit { user: String, kind: String, token: u32 },
}

pub fn parse_init(code: u8, payload: &[u8]) -> anyhow::Result<InitMsg> {
    let mut r = R::new(payload);
    Ok(match code {
        init::PIERCE_FIREWALL => InitMsg::PierceFireWall(r.u32()?),
        init::PEER_INIT => {
            let user = r.str()?;
            let kind = r.str()?;
            let token = r.u32().unwrap_or(0);
            InitMsg::PeerInit { user, kind, token }
        }
        c => anyhow::bail!("unknown init code {c}"),
    })
}

// ---- peer messages (u32 codes) -------------------------------------------------------------------

pub mod peer {
    pub const GET_SHARE_FILE_LIST: u32 = 4;
    pub const SHARE_FILE_LIST: u32 = 5;
    pub const FILE_SEARCH_REQUEST: u32 = 8;
    pub const FILE_SEARCH_RESPONSE: u32 = 9;
    pub const USER_INFO_REQUEST: u32 = 15;
    pub const USER_INFO_REPLY: u32 = 16;
    pub const FOLDER_CONTENTS_REQUEST: u32 = 36;
    pub const FOLDER_CONTENTS_REPLY: u32 = 37;
    pub const TRANSFER_REQUEST: u32 = 40;
    pub const TRANSFER_RESPONSE: u32 = 41;
    pub const QUEUE_UPLOAD: u32 = 43;
    pub const PLACE_IN_QUEUE_REPLY: u32 = 44;
    pub const UPLOAD_FAILED: u32 = 46;
    pub const UPLOAD_DENIED: u32 = 50;
    pub const PLACE_IN_QUEUE_REQUEST: u32 = 51;
}

/// Our view of a search result from one user.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SearchResult {
    pub user: String,
    pub token: u32,
    pub files: Vec<FileEntry>,
    pub free_slot: bool,
    pub upload_speed: u32,
    pub queue_length: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PeerMsg {
    GetShareFileList,
    ShareFileList { dirs: Vec<Dir>, private: Vec<Dir> },
    FileSearchRequest { token: u32, query: String },
    FileSearchResponse(SearchResult),
    UserInfoRequest,
    UserInfoReply { description: String, uploads: u32, queue: u32, free_slot: bool },
    FolderContentsRequest { token: u32, folder: String },
    FolderContentsReply { token: u32, folder: String, dirs: Vec<Dir> },
    /// direction 1 = the sender wants to upload `file` to us (`size` given); 0 = the sender wants our file.
    TransferRequest { direction: u32, token: u32, file: String, size: u64 },
    TransferResponse { token: u32, allowed: bool, reason: String },
    QueueUpload { file: String },
    PlaceInQueueReply { file: String, place: u32 },
    UploadFailed { file: String },
    UploadDenied { file: String, reason: String },
    PlaceInQueueRequest { file: String },
    Other(u32),
}

pub fn get_share_file_list() -> Vec<u8> { frame32(peer::GET_SHARE_FILE_LIST, &[]) }
pub fn share_file_list(dirs: &[Dir]) -> Vec<u8> {
    let mut w = W::default();
    w.u32(dirs.len() as u32);
    for d in dirs {
        w.dir(d);
    }
    w.u32(0).u32(0); // unknown, no private folders
    frame32(peer::SHARE_FILE_LIST, &zip(&w.0))
}
pub fn file_search_request(token: u32, query: &str) -> Vec<u8> { frame32(peer::FILE_SEARCH_REQUEST, &W::default().u32(token).str(query).0) }
pub fn file_search_response(r: &SearchResult) -> Vec<u8> {
    let mut w = W::default();
    w.str(&r.user).u32(r.token).u32(r.files.len() as u32);
    for f in &r.files {
        w.file(f);
    }
    // queue length is u64 on the wire (older clients read it as u32 queue + u32 unknown), then the private-results count
    w.bool(r.free_slot).u32(r.upload_speed).u64(r.queue_length).u32(0);
    frame32(peer::FILE_SEARCH_RESPONSE, &zip(&w.0))
}
pub fn user_info_request() -> Vec<u8> { frame32(peer::USER_INFO_REQUEST, &[]) }
pub fn user_info_reply(description: &str, uploads: u32, queue: u32, free_slot: bool) -> Vec<u8> {
    frame32(peer::USER_INFO_REPLY, &W::default().str(description).bool(false).u32(uploads).u32(queue).bool(free_slot).u32(1).0) // 1 = uploads allowed to everyone
}
pub fn folder_contents_request(token: u32, folder: &str) -> Vec<u8> { frame32(peer::FOLDER_CONTENTS_REQUEST, &W::default().u32(token).str(folder).0) }
pub fn folder_contents_reply(token: u32, folder: &str, dirs: &[Dir]) -> Vec<u8> {
    let mut w = W::default();
    w.u32(token).str(folder).u32(dirs.len() as u32);
    for d in dirs {
        w.dir(d);
    }
    frame32(peer::FOLDER_CONTENTS_REPLY, &zip(&w.0))
}
pub fn transfer_request_upload(token: u32, file: &str, size: u64) -> Vec<u8> {
    frame32(peer::TRANSFER_REQUEST, &W::default().u32(1).u32(token).str(file).u64(size).0)
}
pub fn transfer_response(token: u32, allowed: bool, reason: &str) -> Vec<u8> {
    let mut w = W::default();
    w.u32(token).bool(allowed);
    if !allowed {
        w.str(reason);
    }
    frame32(peer::TRANSFER_RESPONSE, &w.0)
}
/// Reply to a legacy download request (direction 0) that we accept right away: carries the size.
pub fn transfer_response_size(token: u32, size: u64) -> Vec<u8> { frame32(peer::TRANSFER_RESPONSE, &W::default().u32(token).bool(true).u64(size).0) }
pub fn queue_upload(file: &str) -> Vec<u8> { frame32(peer::QUEUE_UPLOAD, &W::default().str(file).0) }
pub fn place_in_queue_reply(file: &str, place: u32) -> Vec<u8> { frame32(peer::PLACE_IN_QUEUE_REPLY, &W::default().str(file).u32(place).0) }
pub fn upload_failed(file: &str) -> Vec<u8> { frame32(peer::UPLOAD_FAILED, &W::default().str(file).0) }
pub fn upload_denied(file: &str, reason: &str) -> Vec<u8> { frame32(peer::UPLOAD_DENIED, &W::default().str(file).str(reason).0) }
pub fn place_in_queue_request(file: &str) -> Vec<u8> { frame32(peer::PLACE_IN_QUEUE_REQUEST, &W::default().str(file).0) }

pub fn parse_peer(code: u32, payload: &[u8]) -> anyhow::Result<PeerMsg> {
    let mut r = R::new(payload);
    Ok(match code {
        peer::GET_SHARE_FILE_LIST => PeerMsg::GetShareFileList,
        peer::SHARE_FILE_LIST => {
            let raw = unzip(payload)?;
            let mut r = R::new(&raw);
            let dirs = r.dirs()?;
            let _unknown = r.u32().unwrap_or(0);
            let private = if r.left() >= 4 { r.dirs().unwrap_or_default() } else { vec![] };
            PeerMsg::ShareFileList { dirs, private }
        }
        peer::FILE_SEARCH_REQUEST => {
            let token = r.u32()?;
            let query = r.str()?;
            PeerMsg::FileSearchRequest { token, query }
        }
        peer::FILE_SEARCH_RESPONSE => {
            let raw = unzip(payload)?;
            let mut r = R::new(&raw);
            let user = r.str()?;
            let token = r.u32()?;
            let files = r.files()?;
            let free_slot = r.bool().unwrap_or(false);
            let upload_speed = r.u32().unwrap_or(0);
            // queue length is u64 in today's clients; a few old ones send u32 and nothing after it
            let queue_length = if r.left() >= 8 { r.u64()? } else { r.u32().unwrap_or(0) as u64 };
            PeerMsg::FileSearchResponse(SearchResult { user, token, files, free_slot, upload_speed, queue_length })
        }
        peer::USER_INFO_REQUEST => PeerMsg::UserInfoRequest,
        peer::USER_INFO_REPLY => {
            let description = r.str()?;
            if r.bool()? {
                let _picture = r.str()?;
            }
            let uploads = r.u32()?;
            let queue = r.u32()?;
            let free_slot = r.bool().unwrap_or(false);
            PeerMsg::UserInfoReply { description, uploads, queue, free_slot }
        }
        peer::FOLDER_CONTENTS_REQUEST => {
            let token = r.u32()?;
            let folder = r.str()?;
            PeerMsg::FolderContentsRequest { token, folder }
        }
        peer::FOLDER_CONTENTS_REPLY => {
            let raw = unzip(payload)?;
            let mut r = R::new(&raw);
            let token = r.u32()?;
            let folder = r.str()?;
            let dirs = r.dirs()?;
            PeerMsg::FolderContentsReply { token, folder, dirs }
        }
        peer::TRANSFER_REQUEST => {
            let direction = r.u32()?;
            let token = r.u32()?;
            let file = r.str()?;
            let size = if direction == 1 { r.u64().unwrap_or(0) } else { 0 };
            PeerMsg::TransferRequest { direction, token, file, size }
        }
        peer::TRANSFER_RESPONSE => {
            let token = r.u32()?;
            let allowed = r.bool()?;
            let reason = if allowed { String::new() } else { r.str().unwrap_or_default() };
            PeerMsg::TransferResponse { token, allowed, reason }
        }
        peer::QUEUE_UPLOAD => PeerMsg::QueueUpload { file: r.str()? },
        peer::PLACE_IN_QUEUE_REPLY => {
            let file = r.str()?;
            let place = r.u32()?;
            PeerMsg::PlaceInQueueReply { file, place }
        }
        peer::UPLOAD_FAILED => PeerMsg::UploadFailed { file: r.str()? },
        peer::UPLOAD_DENIED => {
            let file = r.str()?;
            let reason = r.str().unwrap_or_default();
            PeerMsg::UploadDenied { file, reason }
        }
        peer::PLACE_IN_QUEUE_REQUEST => PeerMsg::PlaceInQueueRequest { file: r.str()? },
        other => PeerMsg::Other(other),
    })
}

// ---- distributed messages (u8 codes) -------------------------------------------------------------

pub mod distrib {
    pub const PING: u8 = 0;
    pub const SEARCH: u8 = 3;
    pub const BRANCH_LEVEL: u8 = 4;
    pub const BRANCH_ROOT: u8 = 5;
    pub const CHILD_DEPTH: u8 = 7;
    pub const EMBEDDED: u8 = 93;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DistribMsg {
    Ping,
    Search { user: String, token: u32, query: String },
    BranchLevel(i32),
    BranchRoot(String),
    ChildDepth(u32),
    Other(u8),
}

pub fn distrib_ping() -> Vec<u8> { frame8(distrib::PING, &[]) }
pub fn distrib_search(user: &str, token: u32, query: &str) -> Vec<u8> {
    frame8(distrib::SEARCH, &W::default().u32(0).str(user).u32(token).str(query).0)
}
pub fn distrib_branch_level(level: i32) -> Vec<u8> { frame8(distrib::BRANCH_LEVEL, &W::default().i32(level).0) }
pub fn distrib_branch_root(user: &str) -> Vec<u8> { frame8(distrib::BRANCH_ROOT, &W::default().str(user).0) }
pub fn distrib_child_depth(depth: u32) -> Vec<u8> { frame8(distrib::CHILD_DEPTH, &W::default().u32(depth).0) }

pub fn parse_distrib(code: u8, payload: &[u8]) -> anyhow::Result<DistribMsg> {
    let mut r = R::new(payload);
    Ok(match code {
        distrib::PING => DistribMsg::Ping,
        distrib::SEARCH => {
            let _unknown = r.u32()?;
            let user = r.str()?;
            let token = r.u32()?;
            let query = r.str()?;
            DistribMsg::Search { user, token, query }
        }
        distrib::BRANCH_LEVEL => DistribMsg::BranchLevel(r.i32()?),
        distrib::BRANCH_ROOT => DistribMsg::BranchRoot(r.str()?),
        distrib::CHILD_DEPTH => DistribMsg::ChildDepth(r.u32()?),
        // a search the server wrapped for a branch root. Roots forward the server's bytes as they
        // came, with a u32 code, so the inner code sits after three zero bytes; a few clients send it
        // as a plain u8 code instead.
        distrib::EMBEDDED => {
            let skip = if payload.len() >= 5 && payload[..3] == [0, 0, 0] { 3 } else { 0 };
            let inner = payload[skip];
            return parse_distrib(inner, &payload[skip + 1..]);
        }
        other => DistribMsg::Other(other),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split32(frame: &[u8]) -> (u32, Vec<u8>) {
        let len = u32::from_le_bytes(frame[0..4].try_into().unwrap()) as usize;
        assert_eq!(len + 4, frame.len());
        (u32::from_le_bytes(frame[4..8].try_into().unwrap()), frame[8..].to_vec())
    }
    fn split8(frame: &[u8]) -> (u8, Vec<u8>) {
        let len = u32::from_le_bytes(frame[0..4].try_into().unwrap()) as usize;
        assert_eq!(len + 4, frame.len());
        (frame[4], frame[5..].to_vec())
    }

    #[test]
    fn login_bytes() {
        // length, code 1, "ab", "cd", 160, md5("abcd"), 1
        let f = login("ab", "cd");
        let (code, p) = split32(&f);
        assert_eq!(code, 1);
        let mut r = R::new(&p);
        assert_eq!(r.str().unwrap(), "ab");
        assert_eq!(r.str().unwrap(), "cd");
        assert_eq!(r.u32().unwrap(), 160);
        assert_eq!(r.str().unwrap(), "e2fc714c4727ee9395f324cd2e7f331f");
        assert_eq!(r.u32().unwrap(), 1);
        assert_eq!(r.left(), 0);
    }

    #[test]
    fn ip_order() {
        // 1.2.3.4 travels as bytes 04 03 02 01
        let mut w = W::default();
        w.ip(Ipv4Addr::new(1, 2, 3, 4));
        assert_eq!(w.0, vec![4, 3, 2, 1]);
        assert_eq!(R::new(&w.0).ip().unwrap(), Ipv4Addr::new(1, 2, 3, 4));
    }

    #[test]
    fn server_roundtrips() {
        let mut w = W::default();
        w.bool(true).str("Welcome").ip(Ipv4Addr::new(10, 0, 0, 7)).str("hash").bool(false);
        assert_eq!(parse_server(server::LOGIN, &w.0).unwrap(), ServerMsg::LoginOk { greet: "Welcome".into(), ip: Ipv4Addr::new(10, 0, 0, 7), supporter: false });
        let mut w = W::default();
        w.bool(false).str("INVALIDPASS");
        assert_eq!(parse_server(server::LOGIN, &w.0).unwrap(), ServerMsg::LoginFailed { reason: "INVALIDPASS".into() });
        let mut w = W::default();
        w.str("bob").str("F").ip(Ipv4Addr::new(1, 1, 1, 1)).u32(2234).u32(77).bool(false).u32(0).u32(0);
        assert_eq!(parse_server(server::CONNECT_TO_PEER, &w.0).unwrap(), ServerMsg::ConnectToPeer { user: "bob".into(), kind: "F".into(), ip: Ipv4Addr::new(1, 1, 1, 1), port: 2234, token: 77 });
        let mut w = W::default();
        w.u8(3).u32(0).str("alice").u32(9).str("daft punk");
        assert_eq!(parse_server(server::EMBEDDED_MESSAGE, &w.0).unwrap(), ServerMsg::EmbeddedSearch { user: "alice".into(), token: 9, query: "daft punk".into() });
        let mut w = W::default();
        w.u32(1).str("p").ip(Ipv4Addr::new(2, 2, 2, 2)).u32(5);
        assert_eq!(parse_server(server::POSSIBLE_PARENTS, &w.0).unwrap(), ServerMsg::PossibleParents(vec![("p".into(), Ipv4Addr::new(2, 2, 2, 2), 5)]));
        let (code, p) = split32(&set_wait_port(2234));
        assert_eq!((code, p.len()), (2, 12));
        // the live server's CantConnectToPeer is just the token
        assert_eq!(parse_server(server::CANT_CONNECT_TO_PEER, &W::default().u32(7).0).unwrap(), ServerMsg::CantConnectToPeer { token: 7, user: String::new() });
        let mut w = W::default();
        w.str("carol").u32(3).str("boards of canada");
        assert_eq!(parse_server(server::FILE_SEARCH, &w.0).unwrap(), ServerMsg::EmbeddedSearch { user: "carol".into(), token: 3, query: "boards of canada".into() });
    }

    #[test]
    fn embedded_distrib_from_a_root() {
        // a branch root forwards the server's u32-coded frame: 93 00 00 00 | 03 | payload
        let mut w = W::default();
        w.u8(0).u8(0).u8(0).u8(distrib::SEARCH).u32(0).str("dan").u32(5).str("q");
        assert_eq!(parse_distrib(distrib::EMBEDDED, &w.0).unwrap(), DistribMsg::Search { user: "dan".into(), token: 5, query: "q".into() });
        let mut w = W::default();
        w.u8(distrib::SEARCH).u32(0).str("dan").u32(5).str("q");
        assert_eq!(parse_distrib(distrib::EMBEDDED, &w.0).unwrap(), DistribMsg::Search { user: "dan".into(), token: 5, query: "q".into() });
    }

    #[test]
    fn search_response_trailer_is_twelve_bytes() {
        let sr = SearchResult { user: "u".into(), token: 1, files: vec![], free_slot: false, upload_speed: 0, queue_length: 2 };
        let (_, p) = split32(&file_search_response(&sr));
        let raw = unzip(&p).unwrap();
        // user "u" (4+1), token 4, nfiles 4, then bool 1 + speed 4 + queue 8 + private count 4
        assert_eq!(raw.len(), 5 + 4 + 4 + 1 + 4 + 8 + 4);
    }

    #[test]
    fn init_and_distrib() {
        let (code, p) = split8(&peer_init("me", "P", 5));
        assert_eq!(parse_init(code, &p).unwrap(), InitMsg::PeerInit { user: "me".into(), kind: "P".into(), token: 5 });
        let (code, p) = split8(&pierce_firewall(9));
        assert_eq!(parse_init(code, &p).unwrap(), InitMsg::PierceFireWall(9));
        let (code, p) = split8(&distrib_search("u", 3, "q q"));
        assert_eq!(parse_distrib(code, &p).unwrap(), DistribMsg::Search { user: "u".into(), token: 3, query: "q q".into() });
        let (code, p) = split8(&distrib_branch_level(2));
        assert_eq!(parse_distrib(code, &p).unwrap(), DistribMsg::BranchLevel(2));
    }

    #[test]
    fn peer_roundtrips() {
        let f = FileEntry { name: "@@m\\A\\B\\01 - x.flac".into(), size: 12345678, ext: "flac".into(), attrs: vec![(ATTR_BITRATE, 900), (ATTR_DURATION, 240), (ATTR_SAMPLE_RATE, 44100), (ATTR_BIT_DEPTH, 16)] };
        let sr = SearchResult { user: "me".into(), token: 42, files: vec![f.clone()], free_slot: true, upload_speed: 123456, queue_length: 3 };
        let (code, p) = split32(&file_search_response(&sr));
        assert_eq!(parse_peer(code, &p).unwrap(), PeerMsg::FileSearchResponse(sr));
        let dirs = vec![Dir { name: "@@m\\A\\B".into(), files: vec![f.clone()] }, Dir { name: "@@m\\C".into(), files: vec![] }];
        let (code, p) = split32(&share_file_list(&dirs));
        assert_eq!(parse_peer(code, &p).unwrap(), PeerMsg::ShareFileList { dirs: dirs.clone(), private: vec![] });
        let (code, p) = split32(&folder_contents_reply(7, "@@m\\A\\B", &dirs[..1]));
        assert_eq!(parse_peer(code, &p).unwrap(), PeerMsg::FolderContentsReply { token: 7, folder: "@@m\\A\\B".into(), dirs: dirs[..1].to_vec() });
        let (code, p) = split32(&transfer_request_upload(11, "x", 99));
        assert_eq!(parse_peer(code, &p).unwrap(), PeerMsg::TransferRequest { direction: 1, token: 11, file: "x".into(), size: 99 });
        let (code, p) = split32(&transfer_response(11, false, "Queued"));
        assert_eq!(parse_peer(code, &p).unwrap(), PeerMsg::TransferResponse { token: 11, allowed: false, reason: "Queued".into() });
        let (code, p) = split32(&transfer_response(11, true, ""));
        assert_eq!(parse_peer(code, &p).unwrap(), PeerMsg::TransferResponse { token: 11, allowed: true, reason: "".into() });
        let (code, p) = split32(&queue_upload("f"));
        assert_eq!(parse_peer(code, &p).unwrap(), PeerMsg::QueueUpload { file: "f".into() });
        let (code, p) = split32(&upload_denied("f", "File not shared."));
        assert_eq!(parse_peer(code, &p).unwrap(), PeerMsg::UploadDenied { file: "f".into(), reason: "File not shared.".into() });
        let (code, p) = split32(&place_in_queue_reply("f", 4));
        assert_eq!(parse_peer(code, &p).unwrap(), PeerMsg::PlaceInQueueReply { file: "f".into(), place: 4 });
        let (code, p) = split32(&user_info_reply("Encore", 1, 2, true));
        assert_eq!(parse_peer(code, &p).unwrap(), PeerMsg::UserInfoReply { description: "Encore".into(), uploads: 1, queue: 2, free_slot: true });
    }

    #[test]
    fn old_u32_queue_length_still_parses() {
        let mut w = W::default();
        w.str("old").u32(1).u32(0).bool(false).u32(10).u32(7);
        let (code, p) = split32(&frame32(peer::FILE_SEARCH_RESPONSE, &zip(&w.0)));
        match parse_peer(code, &p).unwrap() { PeerMsg::FileSearchResponse(r) => assert_eq!((r.user.as_str(), r.queue_length), ("old", 7)), _ => panic!() }
    }

    #[test]
    fn latin1_strings_do_not_fail() {
        let mut w = W::default();
        w.u32(2).bytes(&[0xE9, 0x41]);
        assert_eq!(R::new(&w.0).str().unwrap(), "éA");
    }
}
