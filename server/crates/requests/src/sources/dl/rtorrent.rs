//! rTorrent over XML-RPC (its SCGI mount, e.g. ruTorrent's /RPC2 or /plugins/httprpc/action.php).
//! .torrent files are fetched by Encore and sent with load.raw_start so the info hash is known up
//! front; magnets go in with load.start. Downloads are found again by hash.
use encore_http::Ctx;
use super::{base, delete_inside, fetch_torrent, local, magnet_info, torrent_info, unreachable, Fetched, Poll};
use base64::Engine;
use regex::Regex;
use std::sync::LazyLock;

pub(crate) enum X {
    S(String),
    B64(Vec<u8>),
    I(i64),
}

/// XML-RPC element text: only &, < and > are escaped (quotes stay as they are, as rTorrent's commands
/// carry them).
fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}
fn unesc(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

pub(crate) fn body(method: &str, params: &[X]) -> String {
    let ps: String = params.iter().map(|p| match p {
        X::S(s) => format!("<param><value><string>{}</string></value></param>", esc(s)),
        X::B64(b) => format!("<param><value><base64>{}</base64></value></param>", base64::engine::general_purpose::STANDARD.encode(b)),
        X::I(n) => format!("<param><value><i8>{n}</i8></value></param>"),
    }).collect();
    format!(r#"<?xml version="1.0"?><methodCall><methodName>{method}</methodName><params>{ps}</params></methodCall>"#)
}

static FAULT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<name>faultString</name>\s*<value>\s*(?:<string>)?(.*?)(?:</string>)?\s*</value>").unwrap());
static VALUE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<params>\s*<param>\s*<value>(.*?)</value>").unwrap());
static TYPED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)^\s*<(i4|i8|int|string|boolean|double)>(.*)</(?:i4|i8|int|string|boolean|double)>\s*$").unwrap());

/// The single value of an XML-RPC answer as text ("" for an empty string), or the fault.
pub(crate) fn parse(xml: &str) -> anyhow::Result<String> {
    if xml.contains("<fault>") {
        let why = FAULT.captures(xml).map(|c| unesc(c[1].trim())).unwrap_or_else(|| "fault".into());
        anyhow::bail!("rTorrent: {why}");
    }
    let Some(c) = VALUE.captures(xml) else { anyhow::bail!("rTorrent sent no XML-RPC answer (check the URL)") };
    let inner = &c[1];
    if inner.trim() == "<string/>" {
        return Ok(String::new());
    }
    Ok(unesc(&TYPED.captures(inner).map(|t| t[2].to_string()).unwrap_or_else(|| inner.to_string())))
}

async fn call(st: &impl Ctx, method: &str, params: &[X]) -> anyhow::Result<String> {
    let mut rb = st.http().post(base(st, "rtorrent_url")).header("Content-Type", "text/xml").body(body(method, params)).timeout(std::time::Duration::from_secs(60));
    let user = st.cfg().str("rtorrent_user");
    if !user.is_empty() {
        rb = rb.basic_auth(&user, Some(st.cfg().str("rtorrent_password")));
    }
    let r = rb.send().await.map_err(|e| unreachable("rTorrent", e))?;
    match r.status().as_u16() {
        401 => anyhow::bail!("rTorrent login failed: wrong username or password"),
        s if !(200..300).contains(&s) => anyhow::bail!("rTorrent answered HTTP {s} (check the URL)"),
        _ => {}
    }
    parse(&r.text().await?)
}

fn s(v: &str) -> X {
    X::S(v.to_string())
}

pub async fn status(st: &impl Ctx) -> anyhow::Result<String> {
    let v = call(st, "system.client_version", &[]).await?;
    Ok(if v.is_empty() { String::new() } else { format!(" {v}") })
}

/// The load.* commands that set the folder and the ruTorrent label.
fn load_cmds(st: &impl Ctx) -> Vec<X> {
    let mut out = vec![];
    let dir = st.cfg().str("rtorrent_download_path");
    if !dir.trim().is_empty() {
        out.push(X::S(format!("d.directory.set=\"{}\"", dir.replace('"', ""))));
    }
    let label = st.cfg().str("rtorrent_label");
    if !label.trim().is_empty() {
        out.push(X::S(format!("d.custom1.set=\"{}\"", urlencoding::encode(label.trim()))));
    }
    out
}

pub async fn add(st: &impl Ctx, url: &str) -> anyhow::Result<String> {
    let (method, payload, hash) = match fetch_torrent(st, url).await? {
        Fetched::Magnet(m) => {
            let (hash, _) = magnet_info(&m).ok_or_else(|| anyhow::anyhow!("the magnet link has no info hash"))?;
            ("load.start", X::S(m), hash)
        }
        Fetched::File(b) => {
            let (hash, _) = torrent_info(&b).ok_or_else(|| anyhow::anyhow!("the indexer sent a broken torrent file"))?;
            ("load.raw_start", X::B64(b), hash)
        }
    };
    let mut params = vec![s(""), payload];
    params.extend(load_cmds(st));
    call(st, method, &params).await?;
    Ok(hash)
}

async fn int(st: &impl Ctx, method: &str, hash: &str) -> anyhow::Result<i64> {
    Ok(call(st, method, &[s(hash)]).await?.trim().parse().unwrap_or(0))
}

/// Where rTorrent keeps the download: base_path while open, else directory (+ name for single files).
async fn path(st: &impl Ctx, hash: &str) -> anyhow::Result<String> {
    let p = call(st, "d.base_path", &[s(hash)]).await?;
    if !p.is_empty() {
        return Ok(p);
    }
    let dir = call(st, "d.directory", &[s(hash)]).await?;
    if int(st, "d.is_multi_file", hash).await? == 1 {
        return Ok(dir);
    }
    Ok(format!("{}/{}", dir.trim_end_matches('/'), call(st, "d.name", &[s(hash)]).await?))
}

pub async fn poll(st: &impl Ctx, hash: &str) -> anyhow::Result<Poll> {
    let size = match call(st, "d.size_bytes", &[s(hash)]).await {
        Ok(v) => v.trim().parse::<f64>().unwrap_or(0.0),
        Err(e) if e.to_string().to_lowercase().contains("could not find info-hash") || e.to_string().contains("info-hash") => {
            return Ok(Poll::Failed("torrent disappeared from rTorrent".into()))
        }
        Err(e) => return Err(e),
    };
    if int(st, "d.complete", hash).await? == 1 {
        return Ok(Poll::Done(local(st, &path(st, hash).await?, "rtorrent_download_path")));
    }
    let msg = call(st, "d.message", &[s(hash)]).await.unwrap_or_default();
    let done = int(st, "d.completed_bytes", hash).await? as f64;
    // only some files selected: finished once no wanted chunk is left. rTorrent reports 0 wanted chunks for a
    // closed or stopped download too, so it must be open and started (and not checking its hashes)
    if size > 0.0 && done > 0.0 && int(st, "d.is_open", hash).await? == 1 && int(st, "d.state", hash).await? == 1
        && int(st, "d.wanted_chunks", hash).await? == 0 && int(st, "d.is_hash_checking", hash).await? == 0 {
        return Ok(Poll::Done(local(st, &path(st, hash).await?, "rtorrent_download_path")));
    }
    let rate = int(st, "d.down.rate", hash).await? as f64 / 1e6;
    let pct = if size > 0.0 { (done / size * 1000.0).round() / 10.0 } else { 0.0 };
    let text = if size == 0.0 { "rTorrent: waiting for the magnet's metadata".to_string() } else if !msg.trim().is_empty() && rate == 0.0 { format!("rTorrent: {}", msg.trim()) } else { format!("rTorrent: {rate:.1} MB/s") };
    Ok(Poll::Progress(pct, text))
}

/// d.erase keeps the files, so Encore deletes what's left (only inside the download folder).
pub async fn remove(st: &impl Ctx, hash: &str) {
    let p = path(st, hash).await.ok();
    let _ = call(st, "d.erase", &[s(hash)]).await;
    if let Some(p) = p.filter(|p| !p.is_empty()) {
        delete_inside(st, &local(st, &p, "rtorrent_download_path"), "rtorrent_download_path");
    }
}

/// The torrent's files (f.path, in rTorrent's order). None until a magnet's metadata is there.
pub async fn files(st: &impl Ctx, hash: &str) -> anyhow::Result<Option<Vec<String>>> {
    if call(st, "d.size_bytes", &[s(hash)]).await?.trim().parse::<f64>().unwrap_or(0.0) <= 0.0 {
        return Ok(None);
    }
    let n = int(st, "d.size_files", hash).await?.max(0);
    let mut out = vec![];
    for i in 0..n {
        out.push(call(st, "f.path", &[s(&format!("{hash}:f{i}"))]).await?);
    }
    Ok(if out.is_empty() { None } else { Some(out) })
}

/// Priority 0 (off) for every file but `keep`, then d.update_priorities so rTorrent applies it.
pub async fn select(st: &impl Ctx, hash: &str, keep: &[usize]) -> anyhow::Result<()> {
    let n = int(st, "d.size_files", hash).await?.max(0) as usize;
    for i in 0..n {
        call(st, "f.priority.set", &[s(&format!("{hash}:f{i}")), X::I(if keep.contains(&i) { 1 } else { 0 })]).await?;
    }
    call(st, "d.update_priorities", &[s(hash)]).await?;
    Ok(())
}
