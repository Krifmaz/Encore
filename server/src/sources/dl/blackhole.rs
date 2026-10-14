//! Blackhole: Encore drops the .torrent / .magnet / .nzb into a watch folder that the download client
//! picks up, then waits for a folder or file of the same name in the completed folder. The download
//! counts as finished once no partial files are left and neither its size nor any file in it changed for
//! a few minutes. Set the client to move finished downloads into the completed folder (its "move
//! completed downloads to" option) rather than downloading into it: many clients write files at full
//! size from the start and don't mark unfinished ones.
use super::{fetch_torrent, magnet_info, torrent_info, Client, Fetched, Poll};
use crate::state::AppState;
use serde_json::json;
use std::path::{Path, PathBuf};

fn keys(c: Client) -> (&'static str, &'static str) {
    if c == Client::NzbHole { ("nzb_blackhole_watch", "nzb_blackhole_completed") } else { ("torrent_blackhole_watch", "torrent_blackhole_completed") }
}

/// A file name that is safe on every OS.
pub(crate) fn safe_name(s: &str) -> String {
    let n: String = s.chars().map(|c| if c.is_control() || "/\\:*?\"<>|".contains(c) { '_' } else { c }).collect();
    n.trim().trim_matches('.').trim().chars().take(150).collect()
}

fn norm(s: &str) -> String {
    s.to_lowercase().chars().map(|c| if c.is_alphanumeric() { c } else { ' ' }).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ")
}

fn write(dir: &str, file: &str, data: &[u8]) -> anyhow::Result<()> {
    let dir = PathBuf::from(dir);
    std::fs::create_dir_all(&dir).map_err(|e| anyhow::anyhow!("can't use the watch folder {}: {e}", dir.display()))?;
    // write under a temporary name first so the client never picks up half a file
    let tmp = dir.join(format!(".{file}.encore-tmp"));
    std::fs::write(&tmp, data).map_err(|e| anyhow::anyhow!("can't write to the watch folder {}: {e}", dir.display()))?;
    std::fs::rename(&tmp, dir.join(file))?;
    Ok(())
}

pub async fn add_torrent(st: &AppState, url: &str, title: &str, rid: i64) -> anyhow::Result<String> {
    let fallback = if safe_name(title).is_empty() { format!("encore-{rid}") } else { safe_name(title) };
    let watch = st.cfg.str("torrent_blackhole_watch");
    match fetch_torrent(url).await? {
        Fetched::Magnet(m) => {
            let name = magnet_info(&m).map(|(_, n)| safe_name(&n)).filter(|n| !n.is_empty()).unwrap_or(fallback);
            write(&watch, &format!("{name}.magnet"), m.as_bytes())?;
            Ok(name)
        }
        Fetched::File(b) => {
            let name = torrent_info(&b).map(|(_, n)| safe_name(&n)).filter(|n| !n.is_empty()).unwrap_or(fallback);
            write(&watch, &format!("{name}.torrent"), &b)?;
            Ok(name)
        }
    }
}

pub async fn add_nzb(st: &AppState, url: &str, title: &str, rid: i64) -> anyhow::Result<String> {
    let name = if safe_name(title).is_empty() { format!("encore-{rid}") } else { safe_name(title) };
    let r = st.http.get(url).timeout(std::time::Duration::from_secs(60)).send().await.map_err(|e| anyhow::anyhow!("couldn't fetch the NZB: {e}"))?;
    if !r.status().is_success() {
        anyhow::bail!("couldn't fetch the NZB (HTTP {})", r.status().as_u16());
    }
    let data = r.bytes().await?;
    if !String::from_utf8_lossy(&data[..data.len().min(4096)]).contains("<nzb") {
        anyhow::bail!("the indexer didn't send an NZB file");
    }
    write(&st.cfg.str("nzb_blackhole_watch"), &format!("{name}.nzb"), &data)?;
    Ok(name)
}

/// The entry in the completed folder that belongs to `name` (same name, ignoring case, punctuation and
/// a file extension).
pub(crate) fn find(completed: &Path, name: &str) -> Option<PathBuf> {
    let want = norm(name);
    if want.is_empty() {
        return None;
    }
    let entries: Vec<PathBuf> = std::fs::read_dir(completed).ok()?.flatten().map(|e| e.path()).collect();
    let stem = |p: &Path| p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let full = |p: &Path| p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    entries.iter().find(|p| norm(&full(p)) == want).or_else(|| entries.iter().find(|p| p.is_file() && norm(&stem(p)) == want)).cloned()
}

const PARTIAL: [&str; 6] = ["part", "!qb", "!ut", "tmp", "partial", "crdownload"];

/// How long (seconds) a download must stay unchanged before it counts as finished.
#[cfg(not(test))]
const QUIET_SECS: f64 = 300.0;
#[cfg(test)]
const QUIET_SECS: f64 = 0.0;

/// Total size, whether any partial file is still there, and when a file in it last changed (Unix time).
fn measure(p: &Path) -> (u64, bool, f64) {
    if p.is_file() {
        let partial = p.extension().map(|e| PARTIAL.contains(&e.to_string_lossy().to_lowercase().as_str())).unwrap_or(false);
        let m = p.metadata().ok();
        let changed = m.as_ref().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs_f64()).unwrap_or(0.0);
        return (m.map(|m| m.len()).unwrap_or(0), partial, changed);
    }
    let (mut size, mut partial, mut changed) = (0, false, 0.0f64);
    for e in walkdir::WalkDir::new(p).into_iter().flatten().filter(|e| e.file_type().is_file()) {
        let (s, part, c) = measure(e.path());
        size += s;
        partial |= part;
        changed = changed.max(c);
    }
    (size, partial, changed)
}

pub fn poll(st: &AppState, c: Client, name: &str) -> Poll {
    let (_, done_key) = keys(c);
    let waiting = Poll::Progress(0.0, format!("{}: waiting for the download to show up in {}", c.name(), st.cfg.str(done_key)));
    let Some(p) = find(Path::new(&st.cfg.str(done_key)), name) else { return waiting };
    let (size, partial, changed) = measure(&p);
    let key = format!("blackhole_size:{}:{name}", c.name());
    let t = crate::db::now();
    // when the size last changed, and how many times it did (the progress rises with it, so a download
    // that is still growing never looks stalled)
    let last = st.cache_get(&key);
    let (since, steps) = match &last {
        Some(v) if v["size"].as_u64() == Some(size) => (v["since"].as_f64().unwrap_or(t), v["steps"].as_u64().unwrap_or(0)),
        Some(v) => (t, v["steps"].as_u64().unwrap_or(0) + 1),
        None => (t, 0),
    };
    st.cache_put(&key, json!({ "size": size, "since": since, "steps": steps }), 86400);
    let held = last.as_ref().is_some_and(|v| v["size"].as_u64() == Some(size));
    if size > 0 && !partial && held && t - since.max(changed) >= QUIET_SECS {
        return Poll::Done(p);
    }
    let pct = 50.0 + 49.0 * steps as f64 / (steps as f64 + 200.0);
    Poll::Progress(pct, format!("{}: {} MB arrived, waiting until it stops changing", c.name(), size / 1_000_000))
}

pub fn remove(st: &AppState, c: Client, name: &str) {
    let (watch_key, done_key) = keys(c);
    for ext in ["torrent", "magnet", "nzb"] {
        let _ = std::fs::remove_file(Path::new(&st.cfg.str(watch_key)).join(format!("{name}.{ext}")));
    }
    if let Some(p) = find(Path::new(&st.cfg.str(done_key)), name) {
        super::delete_inside(st, &p, done_key);
    }
}

pub fn status(st: &AppState, c: Client) -> anyhow::Result<String> {
    let (watch_key, done_key) = keys(c);
    let watch = PathBuf::from(st.cfg.str(watch_key));
    std::fs::create_dir_all(&watch).map_err(|e| anyhow::anyhow!("can't use the watch folder {}: {e}", watch.display()))?;
    let probe = watch.join(".encore-write-test");
    std::fs::write(&probe, b"").map_err(|e| anyhow::anyhow!("can't write to the watch folder {}: {e}", watch.display()))?;
    let _ = std::fs::remove_file(&probe);
    let done = PathBuf::from(st.cfg.str(done_key));
    if !done.is_dir() {
        anyhow::bail!("the completed folder {} doesn't exist", done.display());
    }
    Ok(" (folders ready)".into())
}
