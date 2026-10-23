//! Downloads for plugins: streamed to a file through `<name>.part` with a size cap and the sha256 worked out
//! on the way, over https only (also after a redirect), so a plugin zip, a program or a model never sits in
//! memory and a download that grows past its cap stops at once.
use encore_core::fsx::{self, Step};
use futures_util::StreamExt;
use sha2::Digest;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::AsyncWriteExt;

/// A finished download.
#[derive(Debug, Clone, PartialEq)]
pub struct Fetched {
    /// where the download went
    pub path: PathBuf,
    /// its size in bytes
    pub size: u64,
    /// lower-case hex
    pub sha256: String,
}

/// The client plugin downloads go through: redirects are followed only to https addresses (at most 10), and
/// plain http only when `allow_http` (tests talking to a stand-in on 127.0.0.1).
pub fn client(user_agent: &str, allow_http: bool) -> reqwest::Client {
    let policy = reqwest::redirect::Policy::custom(move |a| {
        if a.previous().len() >= 10 {
            a.error("too many redirects")
        } else if a.url().scheme() != "https" && !(allow_http && a.url().scheme() == "http") {
            a.error("a redirect to an address that is not https")
        } else {
            a.follow()
        }
    });
    reqwest::Client::builder()
        .user_agent(user_agent)
        .redirect(policy)
        .connect_timeout(Duration::from_secs(30))
        .build()
        .unwrap_or_default()
}

/// The address checked: https (http only when allowed), a host, no user name or password in it.
pub fn check_url(url: &str, allow_http: bool) -> anyhow::Result<reqwest::Url> {
    let u = reqwest::Url::parse(url.trim()).map_err(|_| anyhow::anyhow!("not a web address: {url}"))?;
    match u.scheme() {
        "https" => {}
        "http" if allow_http => {}
        _ => anyhow::bail!("only https addresses are fetched ({url})"),
    }
    if u.host_str().is_none_or(str::is_empty) {
        anyhow::bail!("the address has no host: {url}");
    }
    if !u.username().is_empty() || u.password().is_some() {
        anyhow::bail!("the address carries a user name or password, which Encore does not send: {url}");
    }
    Ok(u)
}

/// A short form of an address for messages (scheme, host and path; no query string, which may carry a key).
pub fn shown(url: &str) -> String {
    match reqwest::Url::parse(url) {
        Ok(u) => format!("{}://{}{}", u.scheme(), u.host_str().unwrap_or(""), u.path()),
        Err(_) => url.split(['?', '#']).next().unwrap_or(url).to_string(),
    }
}

/// Download `url` to `dest` (through `dest.part`, renamed when whole): at most `max` bytes, within `limit`.
/// A download that fails or grows too large leaves nothing behind.
pub async fn to_file(http: &reqwest::Client, url: &str, dest: &Path, max: u64, limit: Duration, allow_http: bool) -> anyhow::Result<Fetched> {
    let u = check_url(url, allow_http)?;
    let what = shown(url);
    if let Some(d) = dest.parent() {
        fsx::mkdir_all(d, Step::Plugin)?;
    }
    let r = http.get(u).timeout(limit).send().await.map_err(|e| anyhow::anyhow!("downloading {what} failed: {}", plain(e)))?;
    if !r.status().is_success() {
        anyhow::bail!("downloading {what} failed: the server answered HTTP {}", r.status().as_u16());
    }
    if r.content_length().is_some_and(|n| n > max) {
        anyhow::bail!("{what} is larger than {} MB", max >> 20);
    }
    let part = PathBuf::from(format!("{}.part", dest.display()));
    let file = fsx::create(&part, Step::Plugin)?;
    let mut f = tokio::fs::File::from_std(file);
    let mut hash = sha2::Sha256::new();
    let mut size: u64 = 0;
    let res: anyhow::Result<()> = async {
        let mut body = r.bytes_stream();
        while let Some(chunk) = body.next().await {
            let chunk = chunk.map_err(|e| anyhow::anyhow!("downloading {what} failed: {}", plain(e)))?;
            size += chunk.len() as u64;
            if size > max {
                anyhow::bail!("{what} is larger than {} MB", max >> 20);
            }
            hash.update(&chunk);
            f.write_all(&chunk).await.map_err(fsx::at(Step::Plugin, "write", &part))?;
        }
        f.flush().await.map_err(fsx::at(Step::Plugin, "write", &part))?;
        f.sync_all().await.map_err(fsx::at(Step::Plugin, "fsync", &part))?;
        Ok(())
    }
    .await;
    drop(f);
    if let Err(e) = res {
        let _ = std::fs::remove_file(&part);
        return Err(e);
    }
    fsx::rename(&part, dest, Step::Plugin)?;
    Ok(Fetched { path: dest.to_path_buf(), size, sha256: hex::encode(hash.finalize()) })
}

/// A small text (a sums file, a manifest): at most `max` bytes.
pub async fn text(http: &reqwest::Client, url: &str, max: usize, allow_http: bool) -> anyhow::Result<String> {
    let u = check_url(url, allow_http)?;
    let what = shown(url);
    let r = http.get(u).timeout(Duration::from_secs(60)).send().await.map_err(|e| anyhow::anyhow!("fetching {what} failed: {}", plain(e)))?;
    if !r.status().is_success() {
        anyhow::bail!("fetching {what} failed: the server answered HTTP {}", r.status().as_u16());
    }
    let mut out = Vec::new();
    let mut body = r.bytes_stream();
    while let Some(chunk) = body.next().await {
        let chunk = chunk.map_err(|e| anyhow::anyhow!("fetching {what} failed: {}", plain(e)))?;
        if out.len() + chunk.len() > max {
            anyhow::bail!("{what} is larger than {} KB", max / 1024);
        }
        out.extend_from_slice(&chunk);
    }
    String::from_utf8(out).map_err(|_| anyhow::anyhow!("{what} is not text"))
}

/// A JSON answer (an API: GitHub, PyPI, Hugging Face) → (HTTP status, body; null when it is not JSON).
pub async fn json(http: &reqwest::Client, url: &str, headers: &[(&str, String)], allow_http: bool) -> anyhow::Result<(u16, serde_json::Value)> {
    let u = check_url(url, allow_http)?;
    let mut req = http.get(u).timeout(Duration::from_secs(30));
    for (k, v) in headers {
        req = req.header(*k, v);
    }
    let r = req.send().await.map_err(|e| anyhow::anyhow!("{} did not answer: {}", shown(url), plain(e)))?;
    let code = r.status().as_u16();
    let text = r.text().await.unwrap_or_default();
    if text.len() > 16 << 20 {
        anyhow::bail!("{} answered with more than 16 MB", shown(url));
    }
    Ok((code, serde_json::from_str(&text).unwrap_or(serde_json::Value::Null)))
}

/// reqwest's error without the address it repeats (the message names it already).
fn plain(e: reqwest::Error) -> String {
    let e = e.without_url();
    let mut s = e.to_string();
    let mut src = std::error::Error::source(&e);
    while let Some(c) = src {
        let t = c.to_string();
        if !s.contains(&t) {
            s = format!("{s}: {t}");
        }
        src = c.source();
    }
    s
}

/// The sha256 of a file (lower-case hex).
pub fn sha256_file(path: &Path) -> anyhow::Result<String> {
    use std::io::Read;
    let mut f = fsx::open(path, Step::Plugin)?;
    let mut hash = sha2::Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = f.read(&mut buf).map_err(fsx::at(Step::Plugin, "read", path))?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    Ok(hex::encode(hash.finalize()))
}
