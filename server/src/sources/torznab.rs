//! Torrent and Usenet releases: search every indexer through Prowlarr, download with the chosen torrent
//! client (qBittorrent, Transmission, Deluge, rTorrent or a blackhole folder) or Usenet client (SABnzbd,
//! NZBGet or a blackhole folder), then import the finished folder. Replaces Lidarr. The clients are in
//! sources/dl.
pub use super::dl::Poll;
use super::dl::Client;
use crate::state::AppState;
use regex::Regex;
use serde_json::{json, Value};
use std::sync::LazyLock;

/// The chosen torrent client is set up (the name is from when qBittorrent was the only one).
pub fn qbit_ready(st: &AppState) -> bool {
    Client::torrent(st).ready(st)
}
/// The chosen Usenet client is set up.
pub fn sab_ready(st: &AppState) -> bool {
    Client::usenet(st).ready(st)
}
pub fn enabled(st: &AppState) -> bool {
    st.cfg.bool("torznab_enabled") && !st.cfg.str("prowlarr_url").is_empty() && !st.cfg.str("prowlarr_api_key").is_empty() && (qbit_ready(st) || sab_ready(st))
}

fn base(st: &AppState, key: &str) -> String {
    st.cfg.str(key).trim_end_matches('/').to_string()
}

// ---- Prowlarr ------------------------------------------------------------------------------------

pub async fn search(st: &AppState, query: &str) -> anyhow::Result<Vec<Value>> {
    let r = st.http.get(format!("{}/api/v1/search", base(st, "prowlarr_url")))
        .query(&[("query", query), ("type", "search"), ("categories", "3000"), ("limit", "100")])
        .header("X-Api-Key", st.cfg.str("prowlarr_api_key")).timeout(std::time::Duration::from_secs(90)).send().await
        .map_err(|e| anyhow::anyhow!("Prowlarr unreachable: {e}"))?;
    if !r.status().is_success() {
        anyhow::bail!("Prowlarr answered {}{}", r.status().as_u16(), if r.status().as_u16() == 401 { " (check the API key)" } else { "" });
    }
    Ok(r.json::<Value>().await?.as_array().cloned().unwrap_or_default())
}

pub async fn status(st: &AppState) -> Value {
    let p = st.http.get(format!("{}/api/v1/indexer", base(st, "prowlarr_url"))).header("X-Api-Key", st.cfg.str("prowlarr_api_key")).send().await;
    let indexers = match p {
        Ok(r) if r.status().is_success() => r.json::<Value>().await.ok().and_then(|v| v.as_array().map(|a| a.iter().filter(|i| i["enable"].as_bool().unwrap_or(true)).count())),
        _ => None,
    };
    let Some(n) = indexers else { return json!({ "ok": false, "message": "Prowlarr not reachable or wrong API key" }) };
    let (mut ok, mut parts) = (n > 0, vec![format!("{n} indexer(s) in Prowlarr")]);
    for c in [Client::torrent(st), Client::usenet(st)] {
        if !c.ready(st) {
            parts.push(format!("no {}", c.name()));
            continue;
        }
        match c.status(st).await {
            Ok(extra) => parts.push(format!("{} ok{extra}", c.name())),
            Err(e) => {
                ok = false;
                parts.push(e.to_string());
            }
        }
    }
    json!({ "ok": ok, "message": parts.join(" · ") })
}

static NON_ALNUM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^a-z0-9]+").unwrap());
static LOSSLESS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(flac|lossless|alac|24[\s-]?bit|16[\s-]?bit|wav|hi-?res)\b").unwrap());
static MP3_320: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(320|v0)\b").unwrap());
static BAD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(live|remix(es)?|karaoke|instrumental|8d|nightcore|tribute|cover)\b").unwrap());
static BIG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(discography|collection|anthology|complete works|box ?set)\b").unwrap());

fn norm(s: &str) -> String {
    NON_ALNUM.replace_all(&s.to_lowercase().replace('&', " and "), " ").split_whitespace().collect::<Vec<_>>().join(" ")
}
fn contains(hay: &str, needle: &str) -> f64 {
    let n = norm(needle);
    let w: Vec<&str> = n.split_whitespace().collect();
    if w.is_empty() {
        return 1.0;
    }
    let h = format!(" {} ", norm(hay));
    w.iter().filter(|x| h.contains(&format!(" {x} "))).count() as f64 / w.len() as f64
}

/// Best releases for an album, best first.
pub fn rank_album(st: &AppState, results: &[Value], artist: &str, album: &str, lossless_only: bool) -> Vec<Value> {
    let min_seed = st.cfg.int("torznab_min_seeders");
    let mut out = vec![];
    for r in results {
        let title = r["title"].as_str().unwrap_or("");
        let proto = r["protocol"].as_str().unwrap_or("torrent");
        if (proto == "torrent" && !qbit_ready(st)) || (proto == "usenet" && !sab_ready(st)) {
            continue;
        }
        let (a, b) = (contains(title, album), contains(title, artist));
        if a < 0.8 || b < 0.5 {
            continue;
        }
        let size = r["size"].as_f64().unwrap_or(0.0) / 1e6;
        if size > 0.0 && !(15.0..=4000.0).contains(&size) {
            continue;
        }
        if proto == "torrent" && r["seeders"].as_i64().unwrap_or(0) < min_seed {
            continue;
        }
        let lossless = LOSSLESS.is_match(title);
        if lossless_only && !lossless {
            continue;
        }
        let mut score = a * 30.0 + b * 15.0;
        score += if lossless { 30.0 } else if MP3_320.is_match(title) { 12.0 } else { 0.0 };
        if BAD.is_match(title) && !BAD.is_match(album) {
            score -= 20.0;
        }
        if BIG.is_match(title) {
            score -= 25.0;
        }
        score += if proto == "usenet" { 8.0 } else { (r["seeders"].as_f64().unwrap_or(0.0) + 1.0).log2().min(10.0) };
        let url = r["magnetUrl"].as_str().filter(|m| !m.is_empty()).or(r["downloadUrl"].as_str()).unwrap_or("").to_string();
        if url.is_empty() {
            continue;
        }
        out.push(json!({ "title": title, "indexer": r["indexer"], "protocol": proto, "size": r["size"], "seeders": r["seeders"],
            "url": url, "lossless": lossless, "score": (score * 10.0).round() / 10.0 }));
    }
    out.sort_by(|x, y| y["score"].as_f64().partial_cmp(&x["score"].as_f64()).unwrap_or(std::cmp::Ordering::Equal));
    out.truncate(5);
    out
}

// ---- one download, through the chosen client ----------------------------------------------------

/// The client a release of this protocol goes to.
pub fn client_for(st: &AppState, proto: &str) -> Client {
    if proto == "usenet" { Client::usenet(st) } else { Client::torrent(st) }
}

/// "qBittorrent", "Transmission", "SABnzbd" … for messages.
pub fn client_name(st: &AppState, proto: &str) -> &'static str {
    client_for(st, proto).name()
}

/// Hand a release to the matching download client. → the id used for polling.
pub async fn add(st: &AppState, cand: &Value, rid: i64) -> anyhow::Result<String> {
    let url = cand["url"].as_str().unwrap_or("");
    let c = client_for(st, cand["protocol"].as_str().unwrap_or("torrent"));
    c.add(st, url, cand["title"].as_str().unwrap_or(""), rid).await
}

pub async fn poll(st: &AppState, id: &str) -> anyhow::Result<Poll> {
    let (c, own) = Client::for_id(id);
    c.poll(st, own).await
}

/// Remove the download (and what's left of its files) from the client.
pub async fn remove(st: &AppState, id: &str) {
    let (c, own) = Client::for_id(id);
    c.remove(st, own).await
}
