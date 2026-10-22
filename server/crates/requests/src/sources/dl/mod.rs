//! Download clients for torrent and Usenet releases found through Prowlarr. One `Client` per kind is
//! chosen in Settings (`torrent_client`, `nzb_client`); every download id carries its client's prefix
//! ("qbit:…", "tr:…", "sab:…"), so a download started before the setting changed still finishes.
use encore_http::Ctx;
use serde_json::Value;
use std::path::PathBuf;

pub mod blackhole;
pub mod deluge;
pub mod nzbget;
pub mod qbit;
pub mod rtorrent;
pub mod sab;
pub mod transmission;

#[derive(Debug)]
pub enum Poll {
    Progress(f64, String),
    /// Waiting in the client's own queue (its limit of downloads at once is reached): not a stall, the
    /// download starts when the client's turn comes.
    Queued(f64, String),
    /// The client works on the files it has – it checks, repairs, unpacks or moves them (a finished
    /// download, mostly): progress as [`Poll::Progress`], but no stall Download now may give up.
    Busy(f64, String),
    Done(PathBuf),
    Failed(String),
    /// The client doesn't list the download (yet): right after it was added the client may still be
    /// taking it in, later it is gone.
    Missing(String),
}

/// A finished download in Encore's category that Encore didn't add (by hand in the client, or another app
/// with Encore's category).
#[derive(Debug, Clone)]
pub struct Outside {
    /// the prefixed id ("qbit:<hash>", "sab:<nzo id>")
    pub id: String,
    pub name: String,
    /// its files as Encore sees them ([`local`])
    pub path: PathBuf,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Client {
    Qbit,
    Transmission,
    Deluge,
    Rtorrent,
    TorrentHole,
    Sab,
    NzbGet,
    NzbHole,
}

const ALL: [Client; 8] = [Client::Qbit, Client::Transmission, Client::Deluge, Client::Rtorrent, Client::TorrentHole, Client::Sab, Client::NzbGet, Client::NzbHole];

impl Client {
    /// The torrent client chosen in Settings (qBittorrent unless set).
    pub fn torrent(st: &impl Ctx) -> Client {
        match st.cfg().str("torrent_client").trim().to_lowercase().as_str() {
            "transmission" => Client::Transmission,
            "deluge" => Client::Deluge,
            "rtorrent" | "rutorrent" => Client::Rtorrent,
            "blackhole" => Client::TorrentHole,
            _ => Client::Qbit,
        }
    }
    /// The Usenet client chosen in Settings (SABnzbd unless set).
    pub fn usenet(st: &impl Ctx) -> Client {
        match st.cfg().str("nzb_client").trim().to_lowercase().as_str() {
            "nzbget" => Client::NzbGet,
            "blackhole" => Client::NzbHole,
            _ => Client::Sab,
        }
    }
    /// The catalog plugin of this client (encore_plugins' catalog: `qbittorrent`, `torrent-blackhole` …).
    pub fn plugin_id(self) -> &'static str {
        match self {
            Client::Qbit => "qbittorrent",
            Client::Transmission => "transmission",
            Client::Deluge => "deluge",
            Client::Rtorrent => "rtorrent",
            Client::TorrentHole => "torrent-blackhole",
            Client::Sab => "sabnzbd",
            Client::NzbGet => "nzbget",
            Client::NzbHole => "usenet-blackhole",
        }
    }
    /// The client a catalog plugin stands for.
    pub fn of_plugin(id: &str) -> Option<Client> {
        ALL.into_iter().find(|c| c.plugin_id() == id)
    }
    fn prefix(self) -> &'static str {
        match self {
            Client::Qbit => "qbit",
            Client::Transmission => "tr",
            Client::Deluge => "deluge",
            Client::Rtorrent => "rt",
            Client::TorrentHole => "tbh",
            Client::Sab => "sab",
            Client::NzbGet => "nzbget",
            Client::NzbHole => "nbh",
        }
    }
    /// The client a download id belongs to, and the client's own id. Ids without a known prefix are
    /// qBittorrent tags (the oldest format).
    pub fn for_id(id: &str) -> (Client, &str) {
        for c in ALL {
            if let Some(rest) = id.strip_prefix(c.prefix()).and_then(|r| r.strip_prefix(':')) {
                return (c, rest);
            }
        }
        (Client::Qbit, id)
    }
    pub fn name(self) -> &'static str {
        match self {
            Client::Qbit => "qBittorrent",
            Client::Transmission => "Transmission",
            Client::Deluge => "Deluge",
            Client::Rtorrent => "rTorrent",
            Client::TorrentHole => "Torrent blackhole",
            Client::Sab => "SABnzbd",
            Client::NzbGet => "NZBGet",
            Client::NzbHole => "Usenet blackhole",
        }
    }
    /// The setting holding the folder where Encore sees this client's finished downloads.
    pub fn download_path_key(self) -> &'static str {
        match self {
            Client::Qbit => "qbit_download_path",
            Client::Transmission => "transmission_download_path",
            Client::Deluge => "deluge_download_path",
            Client::Rtorrent => "rtorrent_download_path",
            Client::TorrentHole => "torrent_blackhole_completed",
            Client::Sab => "sab_download_path",
            Client::NzbGet => "nzbget_download_path",
            Client::NzbHole => "nzb_blackhole_completed",
        }
    }
    /// Set up enough to send it something.
    pub fn ready(self, st: &impl Ctx) -> bool {
        let has = |k: &str| !st.cfg().str(k).trim().is_empty();
        match self {
            Client::Qbit => has("qbit_url"),
            Client::Transmission => has("transmission_url"),
            Client::Deluge => has("deluge_url"),
            Client::Rtorrent => has("rtorrent_url"),
            Client::TorrentHole => has("torrent_blackhole_watch") && has("torrent_blackhole_completed"),
            Client::Sab => has("sab_url") && has("sab_api_key"),
            Client::NzbGet => has("nzbget_url"),
            Client::NzbHole => has("nzb_blackhole_watch") && has("nzb_blackhole_completed"),
        }
    }

    /// Hand a release (magnet, .torrent or .nzb URL) to the client. → the prefixed id used for polling.
    pub async fn add(self, st: &impl Ctx, url: &str, title: &str, rid: i64) -> anyhow::Result<String> {
        let id = match self {
            Client::Qbit => qbit::add(st, url, rid).await?,
            Client::Transmission => transmission::add(st, url).await?,
            Client::Deluge => deluge::add(st, url).await?,
            Client::Rtorrent => rtorrent::add(st, url).await?,
            Client::TorrentHole => blackhole::add_torrent(st, url, title, rid).await?,
            Client::Sab => sab::add(st, url, rid).await?,
            Client::NzbGet => nzbget::add(st, url, rid).await?,
            Client::NzbHole => blackhole::add_nzb(st, url, title, rid).await?,
        };
        Ok(format!("{}:{id}", self.prefix()))
    }

    /// `id` is the client's own id (without the prefix).
    pub async fn poll(self, st: &impl Ctx, id: &str) -> anyhow::Result<Poll> {
        match self {
            Client::Qbit => qbit::poll(st, id).await,
            Client::Transmission => transmission::poll(st, id).await,
            Client::Deluge => deluge::poll(st, id).await,
            Client::Rtorrent => rtorrent::poll(st, id).await,
            Client::TorrentHole => Ok(blackhole::poll(st, self, id)),
            Client::Sab => sab::poll(st, id).await,
            Client::NzbGet => nzbget::poll(st, id).await,
            Client::NzbHole => Ok(blackhole::poll(st, self, id)),
        }
    }

    /// Remove the download (and what's left of its files) from the client.
    pub async fn remove(self, st: &impl Ctx, id: &str) {
        match self {
            Client::Qbit => qbit::remove(st, id).await,
            Client::Transmission => transmission::remove(st, id).await,
            Client::Deluge => deluge::remove(st, id).await,
            Client::Rtorrent => rtorrent::remove(st, id).await,
            Client::TorrentHole | Client::NzbHole => blackhole::remove(st, self, id),
            Client::Sab => sab::remove(st, id).await,
            Client::NzbGet => nzbget::remove(st, id).await,
        }
    }

    /// The finished downloads in Encore's category that Encore didn't add (qBittorrent and SABnzbd; the other
    /// clients: none). Nothing without a category: every download of the client would be in it.
    pub async fn finished_outside(self, st: &impl Ctx) -> anyhow::Result<Vec<Outside>> {
        let with = |list: Vec<(String, String, PathBuf)>| list.into_iter().map(|(id, name, path)| Outside { id: format!("{}:{id}", self.prefix()), name, path }).collect();
        Ok(match self {
            Client::Qbit => with(qbit::finished_outside(st).await?),
            Client::Sab => with(sab::finished_outside(st).await?),
            _ => vec![],
        })
    }

    /// Mark a download [`Client::finished_outside`] found as taken care of, where the client can (a qBittorrent
    /// tag); `id` is the client's own id.
    pub async fn mark_imported(self, st: &impl Ctx, id: &str) {
        if self == Client::Qbit {
            qbit::mark_imported(st, id).await;
        }
    }

    /// Can download only some files of a release (torrent clients with an API; blackholes and Usenet
    /// can't).
    pub fn can_select(self) -> bool {
        matches!(self, Client::Qbit | Client::Transmission | Client::Deluge | Client::Rtorrent)
    }

    /// The files of a torrent (paths inside it, in the client's order). None while a magnet's metadata
    /// is still loading, and for clients that can't select files.
    pub async fn files(self, st: &impl Ctx, id: &str) -> anyhow::Result<Option<Vec<String>>> {
        match self {
            Client::Qbit => qbit::files(st, id).await,
            Client::Transmission => transmission::files(st, id).await,
            Client::Deluge => deluge::files(st, id).await,
            Client::Rtorrent => rtorrent::files(st, id).await,
            _ => Ok(None),
        }
    }

    /// Download only the files at `keep` (indexes into `files`); the rest are skipped.
    pub async fn select(self, st: &impl Ctx, id: &str, keep: &[usize]) -> anyhow::Result<()> {
        match self {
            Client::Qbit => qbit::select(st, id, keep).await,
            Client::Transmission => transmission::select(st, id, keep).await,
            Client::Deluge => deluge::select(st, id, keep).await,
            Client::Rtorrent => rtorrent::select(st, id, keep).await,
            _ => anyhow::bail!("{} downloads whole releases", self.name()),
        }
    }

    /// Test connection: Ok(extra words, e.g. " 4.0.5") when the client answers and accepts the login.
    pub async fn status(self, st: &impl Ctx) -> anyhow::Result<String> {
        match self {
            Client::Qbit => {
                st.cache_put("qbit_sid", Value::Null, 1); // always a fresh login when testing
                qbit::login(st).await.map(|_| String::new())
            }
            Client::Transmission => transmission::status(st).await,
            Client::Deluge => {
                st.cache_put("deluge_sid", Value::Null, 1);
                deluge::status(st).await
            }
            Client::Rtorrent => rtorrent::status(st).await,
            Client::TorrentHole | Client::NzbHole => blackhole::status(st, self),
            Client::Sab => sab::status(st).await,
            Client::NzbGet => nzbget::status(st).await,
        }
    }
}

pub(crate) fn base(st: &impl Ctx, key: &str) -> String {
    st.cfg().str(key).trim().trim_end_matches('/').to_string()
}

/// Where Encore finds a path the client reported: the same path when it is mounted the same way, else
/// under the folder Encore sees (`key`) – the download's own folder name, or with the folders above it
/// that the client added (a category folder: SABnzbd's ".../complete/music/encore-12" seen from its
/// "complete" folder), the shortest that is there. A Windows client's path ("D:\Usenet\complete\…") is
/// read the same way. When none is there: the folder name under it (what the request then names). A
/// reported path that can't be looked at because its storage fails (EIO, ENOTCONN, ESTALE …: a mount that
/// went away) is that path, not a guess under the folder: the import then waits for the storage.
pub(crate) fn local(st: &impl Ctx, remote: &str, key: &str) -> PathBuf {
    let p = PathBuf::from(remote);
    if !remote.is_empty() {
        match std::fs::metadata(&p) {
            Ok(_) => return p,
            Err(e) if e.raw_os_error().is_some_and(encore_core::fsx::is_storage) => return p,
            Err(_) => {}
        }
    }
    let root = PathBuf::from(st.cfg().str(key));
    let parts: Vec<&str> = remote.split(['/', '\\']).filter(|x| !x.is_empty() && *x != "." && *x != ".." && !x.ends_with(':')).collect();
    let under = |n: usize| parts[parts.len() - n..].iter().fold(root.clone(), |at, x| at.join(x));
    (1..=parts.len()).map(under).find(|at| at.exists()).unwrap_or_else(|| root.join(parts.last().copied().unwrap_or("")))
}

/// Delete a finished download's files, but only inside the client's download folder as Encore sees it.
pub(crate) fn delete_inside(st: &impl Ctx, path: &std::path::Path, key: &str) {
    let root = PathBuf::from(st.cfg().str(key));
    if st.cfg().str(key).trim().is_empty() || path == root || !path.starts_with(&root) || path.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        return;
    }
    if path.is_dir() {
        let _ = std::fs::remove_dir_all(path);
    } else if path.is_file() {
        let _ = std::fs::remove_file(path);
    }
}

pub(crate) fn unreachable(name: &str, e: reqwest::Error) -> anyhow::Error {
    anyhow::anyhow!("{name} unreachable: {}", http_error(e))
}

/// An HTTP error in words without the query of its address – where API keys travel (SABnzbd's, and the
/// one in Prowlarr's download links) – and with its causes ("connection refused").
pub fn http_error(e: reqwest::Error) -> String {
    let at = e.url().map(|u| {
        let mut u = u.clone();
        u.set_query(None);
        u.to_string()
    });
    let e = e.without_url();
    let mut words = e.to_string();
    let mut cause = std::error::Error::source(&e);
    while let Some(c) = cause {
        let c_words = c.to_string();
        if !words.contains(&c_words) {
            words.push_str(&format!(": {c_words}"));
        }
        cause = c.source();
    }
    match at {
        Some(at) => format!("{words} ({at})"),
        None => words,
    }
}

// ---- torrents: files and magnets -----------------------------------------------------------------

pub(crate) enum Fetched {
    Magnet(String),
    File(Vec<u8>),
}

/// A release URL as a magnet or the .torrent's bytes. Prowlarr's download links often redirect to a
/// magnet, which an HTTP client can't follow, so redirects are followed by hand.
pub(crate) async fn fetch_torrent(st: &impl Ctx, url: &str) -> anyhow::Result<Fetched> {
    if url.starts_with("magnet:") {
        return Ok(Fetched::Magnet(url.to_string()));
    }
    let http = st.http_no_redirects();
    let mut url = url.to_string();
    for _ in 0..6 {
        let r = http.get(&url).send().await.map_err(|e| anyhow::anyhow!("couldn't fetch the torrent: {}", http_error(e)))?;
        if r.status().is_redirection() {
            let loc = r.headers().get("location").and_then(|h| h.to_str().ok()).unwrap_or("").to_string();
            if loc.starts_with("magnet:") {
                return Ok(Fetched::Magnet(loc));
            }
            url = reqwest::Url::parse(&url)?.join(&loc)?.to_string();
            continue;
        }
        if !r.status().is_success() {
            anyhow::bail!("couldn't fetch the torrent (HTTP {})", r.status().as_u16());
        }
        let bytes = r.bytes().await.map_err(|e| anyhow::anyhow!("couldn't fetch the torrent: {}", http_error(e)))?.to_vec();
        if bytes.first() != Some(&b'd') {
            anyhow::bail!("the indexer didn't send a torrent file");
        }
        return Ok(Fetched::File(bytes));
    }
    anyhow::bail!("too many redirects fetching the torrent")
}

/// A release URL's .nzb file. Encore fetches it for the Usenet clients too: Prowlarr's links point at
/// Prowlarr, which a client on another network (or behind a VPN) may not reach.
pub(crate) async fn fetch_nzb(st: &impl Ctx, url: &str) -> anyhow::Result<Vec<u8>> {
    let r = st.http().get(url).timeout(std::time::Duration::from_secs(60)).send().await.map_err(|e| anyhow::anyhow!("couldn't fetch the NZB: {}", http_error(e)))?;
    if !r.status().is_success() {
        anyhow::bail!("couldn't fetch the NZB (HTTP {})", r.status().as_u16());
    }
    let data = r.bytes().await.map_err(|e| anyhow::anyhow!("couldn't fetch the NZB: {}", http_error(e)))?.to_vec();
    if !String::from_utf8_lossy(&data[..data.len().min(4096)]).contains("<nzb") {
        anyhow::bail!("the indexer didn't send an NZB file");
    }
    Ok(data)
}

/// End of the bencoded value starting at `i`.
fn bskip(d: &[u8], i: usize) -> Option<usize> {
    match *d.get(i)? {
        b'i' => d[i..].iter().position(|&c| c == b'e').map(|e| i + e + 1),
        b'l' | b'd' => {
            let mut j = i + 1;
            while *d.get(j)? != b'e' {
                j = bskip(d, j)?;
            }
            Some(j + 1)
        }
        b'0'..=b'9' => bstr(d, i).map(|(_, e)| e),
        _ => None,
    }
}
/// The bencoded string at `i`: (start of its bytes, end).
fn bstr(d: &[u8], i: usize) -> Option<(usize, usize)> {
    let c = d.get(i..)?.iter().position(|&c| c == b':')?;
    let n: usize = std::str::from_utf8(&d[i..i + c]).ok()?.parse().ok()?;
    let s = i + c + 1;
    (s + n <= d.len()).then_some((s, s + n))
}
/// The value span of `key` in the bencoded dict at `i`.
fn bget(d: &[u8], i: usize, key: &[u8]) -> Option<(usize, usize)> {
    if *d.get(i)? != b'd' {
        return None;
    }
    let mut j = i + 1;
    while *d.get(j)? != b'e' {
        let (ks, ke) = bstr(d, j)?;
        let end = bskip(d, ke)?;
        if &d[ks..ke] == key {
            return Some((ke, end));
        }
        j = end;
    }
    None
}

/// A .torrent's info hash (uppercase hex) and name.
pub(crate) fn torrent_info(d: &[u8]) -> Option<(String, String)> {
    use sha1::{Digest, Sha1};
    let (s, e) = bget(d, 0, b"info")?;
    let hash = hex::encode_upper(Sha1::digest(&d[s..e]));
    let name = bget(d, s, b"name").and_then(|(ns, _)| bstr(d, ns)).map(|(a, b)| String::from_utf8_lossy(&d[a..b]).to_string()).unwrap_or_default();
    Some((hash, name))
}

/// A magnet's info hash (uppercase hex, base32 hashes converted) and display name.
pub(crate) fn magnet_info(m: &str) -> Option<(String, String)> {
    let q = m.split_once('?')?.1;
    let mut hash = None;
    let mut name = String::new();
    for kv in q.split('&') {
        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
        if k == "xt" {
            if let Some(h) = v.to_lowercase().strip_prefix("urn:btih:") {
                if h.len() == 40 && h.chars().all(|c| c.is_ascii_hexdigit()) {
                    hash = Some(h.to_uppercase());
                } else if h.len() == 32 {
                    hash = base32(h).map(hex::encode_upper);
                }
            }
        } else if k == "dn" {
            name = urlencoding::decode(&v.replace('+', " ")).map(|s| s.to_string()).unwrap_or_default();
        }
    }
    Some((hash?, name))
}

fn base32(s: &str) -> Option<Vec<u8>> {
    let mut out = vec![];
    let (mut buf, mut bits) = (0u64, 0);
    for c in s.chars() {
        let v = match c.to_ascii_uppercase() {
            c @ 'A'..='Z' => c as u64 - 'A' as u64,
            c @ '2'..='7' => c as u64 - '2' as u64 + 26,
            _ => return None,
        };
        buf = (buf << 5) | v;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests;
