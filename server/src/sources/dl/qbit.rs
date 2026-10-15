//! qBittorrent (Web API v2). Downloads are found again by their tag "encore-<request id>".
use super::{base, local, Poll};
use crate::state::AppState;
use serde_json::{json, Value};

/// Log in to qBittorrent's Web API. Returns the session cookie as "name=value": older versions call
/// it SID, 5.1+ QBT_SID_<port>. Empty when qBittorrent skips auth for this network.
pub async fn login(st: &AppState) -> anyhow::Result<String> {
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
    let sid = login(st).await?;
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

pub async fn add(st: &AppState, url: &str, rid: i64) -> anyhow::Result<String> {
    let tag = format!("encore-{rid}");
    let form = reqwest::multipart::Form::new().text("urls", url.to_string()).text("category", st.cfg.str("qbit_category"))
        .text("tags", tag.clone()).text("savepath", st.cfg.str("qbit_download_path"));
    let body = qbit(st, "torrents/add", Some(form), &[]).await?.text().await.unwrap_or_default();
    if body.contains("Fails") {
        anyhow::bail!("qBittorrent didn't accept the torrent");
    }
    Ok(tag)
}

pub async fn poll(st: &AppState, tag: &str) -> anyhow::Result<Poll> {
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

pub async fn remove(st: &AppState, tag: &str) {
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

async fn hash(st: &AppState, tag: &str) -> anyhow::Result<Option<String>> {
    let v: Value = qbit(st, "torrents/info", None, &[("tag", tag.into())]).await?.json().await?;
    Ok(v.as_array().and_then(|a| a.first()).and_then(|t| t["hash"].as_str()).map(String::from))
}

/// The torrent's files in qBittorrent's order (its file index). None until the metadata is there.
pub async fn files(st: &AppState, tag: &str) -> anyhow::Result<Option<Vec<String>>> {
    let Some(h) = hash(st, tag).await? else { return Ok(None) };
    let v: Value = qbit(st, "torrents/files", None, &[("hash", h)]).await?.json().await?;
    let mut list: Vec<(i64, String)> = v.as_array().cloned().unwrap_or_default().iter().enumerate()
        .map(|(i, f)| (f["index"].as_i64().unwrap_or(i as i64), f["name"].as_str().unwrap_or("").to_string())).collect();
    list.sort_by_key(|x| x.0);
    Ok(if list.is_empty() { None } else { Some(list.into_iter().map(|x| x.1).collect()) })
}

/// Priority 0 (don't download) for every file but `keep`.
pub async fn select(st: &AppState, tag: &str, keep: &[usize]) -> anyhow::Result<()> {
    let Some(h) = hash(st, tag).await? else { anyhow::bail!("torrent disappeared from qBittorrent") };
    let n = files(st, tag).await?.map(|f| f.len()).unwrap_or(0);
    let skip: Vec<String> = (0..n).filter(|i| !keep.contains(i)).map(|i| i.to_string()).collect();
    let want: Vec<String> = keep.iter().map(|i| i.to_string()).collect();
    for (ids, prio) in [(skip, "0"), (want, "1")] {
        if ids.is_empty() {
            continue;
        }
        let form = reqwest::multipart::Form::new().text("hash", h.clone()).text("id", ids.join("|")).text("priority", prio);
        qbit(st, "torrents/filePrio", Some(form), &[]).await?;
    }
    Ok(())
}
