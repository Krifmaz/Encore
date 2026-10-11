//! Torrent and Usenet releases: search every indexer through Prowlarr, download with qBittorrent (torrents)
//! or SABnzbd (Usenet), then import the finished folder. Replaces Lidarr.
use crate::state::AppState;
use regex::Regex;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::LazyLock;

pub fn qbit_ready(st: &AppState) -> bool {
    !st.cfg.str("qbit_url").is_empty()
}
pub fn sab_ready(st: &AppState) -> bool {
    !st.cfg.str("sab_url").is_empty() && !st.cfg.str("sab_api_key").is_empty()
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
    st.cache_put("qbit_sid", Value::Null, 1); // always a fresh login when testing
    let (mut ok, q) = (n > 0, if qbit_ready(st) { qbit_login(st).await.map(|_| "qBittorrent ok".to_string()) } else { Ok("no qBittorrent".into()) });
    let s = if sab_ready(st) { sab(st, &[("mode", "version")]).await.map(|_| "SABnzbd ok".to_string()) } else { Ok("no SABnzbd".into()) };
    ok &= q.is_ok() && s.is_ok();
    let txt = |r: anyhow::Result<String>| r.unwrap_or_else(|e| e.to_string());
    json!({ "ok": ok, "message": format!("{n} indexer(s) in Prowlarr · {} · {}", txt(q), txt(s)) })
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

// ---- qBittorrent ---------------------------------------------------------------------------------

/// Log in to qBittorrent's Web API. Returns the session cookie as "name=value": older versions call
/// it SID, 5.1+ QBT_SID_<port>. Empty when qBittorrent skips auth for this network.
async fn qbit_login(st: &AppState) -> anyhow::Result<String> {
    if let Some(v) = st.cache_get("qbit_sid").filter(|v| v.is_string()) {
        return Ok(v.as_str().unwrap_or("").to_string());
    }
    let url = base(st, "qbit_url");
    let r = st.http.post(format!("{url}/api/v2/auth/login")).header("Referer", &url).header("Origin", &url)
        .form(&[("username", st.cfg.str("qbit_user")), ("password", st.cfg.str("qbit_password"))]).send().await
        .map_err(|e| anyhow::anyhow!("qBittorrent unreachable: {e}"))?;
    let status = r.status().as_u16();
    let cookie = r.headers().get_all("set-cookie").iter().filter_map(|h| h.to_str().ok())
        .filter_map(|c| c.split(';').next().map(|kv| kv.trim().to_string()))
        .find(|kv| kv.starts_with("SID=") || kv.starts_with("QBT_SID"));
    let body = r.text().await.unwrap_or_default();
    let cookie = match cookie {
        Some(c) => c,
        None if (200..300).contains(&status) && (body.trim() == "Ok." || body.trim().is_empty()) => String::new(),
        None if status == 403 => anyhow::bail!("qBittorrent refused the login (HTTP 403) – too many failed attempts ban the IP for a while; also check Web UI → \"Enable Host header validation\" / CSRF settings"),
        None if body.trim() == "Fails." => anyhow::bail!("qBittorrent login failed: wrong username or password"),
        None => anyhow::bail!("qBittorrent login failed (HTTP {status}: {})", body.trim().chars().take(120).collect::<String>()),
    };
    st.cache_put("qbit_sid", json!(cookie), 1800);
    Ok(cookie)
}

async fn qbit(st: &AppState, path: &str, form: Option<reqwest::multipart::Form>, query: &[(&str, String)]) -> anyhow::Result<reqwest::Response> {
    let sid = qbit_login(st).await?;
    let url = format!("{}/api/v2/{path}", base(st, "qbit_url"));
    let rb = match form { Some(f) => st.http.post(&url).multipart(f), None => st.http.get(&url).query(query) };
    let rb = if sid.is_empty() { rb } else { rb.header("Cookie", sid) };
    let r = rb.header("Referer", base(st, "qbit_url")).send().await?;
    if r.status().as_u16() == 403 {
        st.cache_put("qbit_sid", Value::Null, 1);
        anyhow::bail!("qBittorrent refused the session, retrying next time");
    }
    Ok(r.error_for_status()?)
}

// ---- SABnzbd -------------------------------------------------------------------------------------

async fn sab(st: &AppState, q: &[(&str, &str)]) -> anyhow::Result<Value> {
    let mut q: Vec<(&str, String)> = q.iter().map(|(k, v)| (*k, v.to_string())).collect();
    q.push(("apikey", st.cfg.str("sab_api_key")));
    q.push(("output", "json".into()));
    let r = st.http.get(format!("{}/api", base(st, "sab_url"))).query(&q).send().await.map_err(|e| anyhow::anyhow!("SABnzbd unreachable: {e}"))?;
    let v: Value = r.error_for_status()?.json().await?;
    if v["status"] == false {
        anyhow::bail!("SABnzbd: {}", v["error"].as_str().unwrap_or("error"));
    }
    Ok(v)
}

// ---- one download, either client -----------------------------------------------------------------

/// Hand a release to the matching download client. → the id used for polling.
pub async fn add(st: &AppState, cand: &Value, rid: i64) -> anyhow::Result<String> {
    let url = cand["url"].as_str().unwrap_or("");
    if cand["protocol"] == "usenet" {
        let name = format!("encore-{rid}");
        let v = sab(st, &[("mode", "addurl"), ("name", url), ("nzbname", &name), ("cat", &st.cfg.str("sab_category"))]).await?;
        let id = v["nzo_ids"][0].as_str().ok_or_else(|| anyhow::anyhow!("SABnzbd didn't accept the NZB"))?;
        return Ok(format!("sab:{id}"));
    }
    let tag = format!("encore-{rid}");
    let form = reqwest::multipart::Form::new().text("urls", url.to_string()).text("category", st.cfg.str("qbit_category"))
        .text("tags", tag.clone()).text("savepath", st.cfg.str("qbit_download_path"));
    let body = qbit(st, "torrents/add", Some(form), &[]).await?.text().await.unwrap_or_default();
    if body.contains("Fails") {
        anyhow::bail!("qBittorrent didn't accept the torrent");
    }
    Ok(format!("qbit:{tag}"))
}

pub enum Poll {
    Progress(f64, String),
    Done(PathBuf),
    Failed(String),
}

fn local(st: &AppState, remote: &str, key: &str) -> PathBuf {
    let p = PathBuf::from(remote);
    if p.exists() {
        return p;
    }
    // client sees a different mount: same name under the folder Encore sees
    PathBuf::from(st.cfg.str(key)).join(p.file_name().unwrap_or_default())
}

pub async fn poll(st: &AppState, id: &str) -> anyhow::Result<Poll> {
    if let Some(nzo) = id.strip_prefix("sab:") {
        let q = sab(st, &[("mode", "queue"), ("nzo_ids", nzo)]).await?;
        if let Some(s) = q["queue"]["slots"].as_array().and_then(|a| a.first()) {
            let pct = s["percentage"].as_str().and_then(|p| p.parse().ok()).unwrap_or(0.0);
            return Ok(Poll::Progress(pct, format!("SABnzbd: {} {}", s["status"].as_str().unwrap_or(""), s["timeleft"].as_str().unwrap_or(""))));
        }
        let h = sab(st, &[("mode", "history"), ("nzo_ids", nzo)]).await?;
        let Some(s) = h["history"]["slots"].as_array().and_then(|a| a.first()).cloned() else { return Ok(Poll::Failed("download disappeared from SABnzbd".into())) };
        return Ok(match s["status"].as_str().unwrap_or("") {
            "Completed" => Poll::Done(local(st, s["storage"].as_str().unwrap_or(""), "sab_download_path")),
            "Failed" => Poll::Failed(format!("SABnzbd: {}", s["fail_message"].as_str().unwrap_or("failed"))),
            other => Poll::Progress(99.0, format!("SABnzbd: {other}")),
        });
    }
    let tag = id.trim_start_matches("qbit:");
    let v: Value = qbit(st, "torrents/info", None, &[("tag", tag.into())]).await?.json().await?;
    let Some(t) = v.as_array().and_then(|a| a.first()).cloned() else { return Ok(Poll::Failed("torrent disappeared from qBittorrent".into())) };
    let state = t["state"].as_str().unwrap_or("");
    let progress = t["progress"].as_f64().unwrap_or(0.0);
    if state == "error" || state == "missingFiles" {
        return Ok(Poll::Failed(format!("qBittorrent: {state}")));
    }
    if progress >= 1.0 && !state.starts_with("checking") {
        return Ok(Poll::Done(local(st, t["content_path"].as_str().unwrap_or(""), "qbit_download_path")));
    }
    let speed = t["dlspeed"].as_f64().unwrap_or(0.0) / 1e6;
    let msg = if state.contains("stalled") || state.starts_with("meta") { format!("qBittorrent: waiting for peers ({state})") } else { format!("qBittorrent: {speed:.1} MB/s") };
    Ok(Poll::Progress((progress * 1000.0).round() / 10.0, msg))
}

/// Remove the download (and what's left of its files) from the client.
pub async fn remove(st: &AppState, id: &str) {
    if let Some(nzo) = id.strip_prefix("sab:") {
        let _ = sab(st, &[("mode", "queue"), ("name", "delete"), ("value", nzo), ("del_files", "1")]).await;
        let _ = sab(st, &[("mode", "history"), ("name", "delete"), ("value", nzo), ("del_files", "1")]).await;
        return;
    }
    let tag = id.trim_start_matches("qbit:");
    if let Ok(r) = qbit(st, "torrents/info", None, &[("tag", tag.into())]).await {
        if let Ok(v) = r.json::<Value>().await {
            let hashes: Vec<&str> = v.as_array().map(|a| a.iter().filter_map(|t| t["hash"].as_str()).collect()).unwrap_or_default();
            if !hashes.is_empty() {
                let form = reqwest::multipart::Form::new().text("hashes", hashes.join("|")).text("deleteFiles", "true");
                let _ = qbit(st, "torrents/delete", Some(form), &[]).await;
            }
        }
    }
}
