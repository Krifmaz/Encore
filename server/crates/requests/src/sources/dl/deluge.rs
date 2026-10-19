//! Deluge through its Web UI's JSON-RPC (/json): auth.login gives a session cookie, the Web UI must be
//! connected to a daemon (the first host is used when it isn't). Downloads are found again by hash.
use encore_http::Ctx;
use super::{base, fetch_torrent, local, torrent_info, unreachable, Fetched, Poll};
use base64::Engine;
use serde_json::{json, Value};

fn endpoint(st: &impl Ctx) -> String {
    let b = base(st, "deluge_url");
    if b.ends_with("/json") { b } else { format!("{b}/json") }
}

/// One call. → (the answer, a new session cookie if one was set).
async fn raw(st: &impl Ctx, cookie: &str, method: &str, params: Value) -> anyhow::Result<(Value, Option<String>)> {
    let mut rb = st.http().post(endpoint(st)).json(&json!({ "method": method, "params": params, "id": 1 })).timeout(std::time::Duration::from_secs(60));
    if !cookie.is_empty() {
        rb = rb.header("Cookie", cookie);
    }
    let r = rb.send().await.map_err(|e| unreachable("Deluge", e))?;
    if !r.status().is_success() {
        anyhow::bail!("Deluge answered HTTP {} (check the URL)", r.status().as_u16());
    }
    let set = r.headers().get_all("set-cookie").iter().filter_map(|h| h.to_str().ok())
        .filter_map(|c| c.split(';').next().map(|kv| kv.trim().to_string()))
        .find(|kv| kv.starts_with("_session_id="));
    let v: Value = r.json().await.map_err(|_| anyhow::anyhow!("Deluge sent no JSON (check the URL)"))?;
    Ok((v, set))
}

async fn login(st: &impl Ctx) -> anyhow::Result<String> {
    if let Some(v) = st.cache_get("deluge_sid").filter(|v| v.is_string()) {
        return Ok(v.as_str().unwrap_or("").to_string());
    }
    let (v, cookie) = raw(st, "", "auth.login", json!([st.cfg().str("deluge_password")])).await?;
    if v["result"] != true {
        anyhow::bail!("Deluge login failed: wrong password");
    }
    let cookie = cookie.unwrap_or_default();
    // the Web UI talks to a daemon: connect it to the first one if it isn't yet
    let (c, _) = raw(st, &cookie, "web.connected", json!([])).await?;
    if c["result"] != true {
        let (h, _) = raw(st, &cookie, "web.get_hosts", json!([])).await?;
        let Some(host) = h["result"][0][0].as_str().map(String::from) else { anyhow::bail!("Deluge has no daemon to connect to (Connection Manager is empty)") };
        let (r, _) = raw(st, &cookie, "web.connect", json!([host])).await?;
        if !r["error"].is_null() {
            anyhow::bail!("Deluge couldn't connect to its daemon: {}", r["error"]["message"].as_str().unwrap_or("error"));
        }
    }
    st.cache_put("deluge_sid", json!(cookie), 1800);
    Ok(cookie)
}

async fn rpc(st: &impl Ctx, method: &str, params: Value) -> anyhow::Result<Value> {
    for attempt in 0..2 {
        let cookie = login(st).await?;
        let (v, _) = raw(st, &cookie, method, params.clone()).await?;
        if v["error"].is_null() {
            return Ok(v["result"].clone());
        }
        if v["error"]["code"].as_i64() == Some(1) && attempt == 0 {
            st.cache_put("deluge_sid", Value::Null, 1); // session expired: log in again
            continue;
        }
        anyhow::bail!("Deluge: {}", v["error"]["message"].as_str().unwrap_or("error"));
    }
    anyhow::bail!("Deluge: not authenticated")
}

pub async fn status(st: &impl Ctx) -> anyhow::Result<String> {
    login(st).await?;
    let v = rpc(st, "daemon.info", json!([])).await.unwrap_or(Value::Null);
    Ok(v.as_str().map(|s| format!(" {s}")).unwrap_or_default())
}

pub async fn add(st: &impl Ctx, url: &str) -> anyhow::Result<String> {
    let mut opts = json!({});
    let dir = st.cfg().str("deluge_download_path");
    if !dir.trim().is_empty() {
        opts["download_location"] = json!(dir);
    }
    // Encore fetches the release itself (Prowlarr links often redirect to a magnet, which the daemon's own
    // download doesn't follow reliably)
    let v = match fetch_torrent(st, url).await? {
        Fetched::Magnet(m) => rpc(st, "core.add_torrent_magnet", json!([m, opts])).await?,
        Fetched::File(b) => {
            let name = torrent_info(&b).map(|(_, n)| format!("{n}.torrent")).unwrap_or_else(|| "encore.torrent".into());
            rpc(st, "core.add_torrent_file", json!([name, base64::engine::general_purpose::STANDARD.encode(&b), opts])).await?
        }
    };
    let hash = v.as_str().filter(|h| !h.is_empty()).ok_or_else(|| anyhow::anyhow!("Deluge didn't accept the torrent"))?.to_string();
    let label = st.cfg().str("deluge_label").trim().to_lowercase();
    if !label.is_empty() {
        // the Label plugin is optional: ignore it when it's off
        let _ = rpc(st, "label.add", json!([label])).await;
        let _ = rpc(st, "label.set_torrent", json!([hash, label])).await;
    }
    Ok(hash)
}

pub async fn poll(st: &impl Ctx, hash: &str) -> anyhow::Result<Poll> {
    let t = rpc(st, "core.get_torrent_status", json!([hash, ["name", "progress", "state", "save_path", "download_payload_rate", "message"]])).await?;
    if t.as_object().map(|o| o.is_empty()).unwrap_or(true) {
        return Ok(Poll::Failed("torrent disappeared from Deluge".into()));
    }
    let state = t["state"].as_str().unwrap_or("");
    if state == "Error" {
        return Ok(Poll::Failed(format!("Deluge: {}", t["message"].as_str().unwrap_or("error"))));
    }
    let progress = t["progress"].as_f64().unwrap_or(0.0);
    if progress >= 100.0 && state != "Checking" && state != "Moving" && state != "Allocating" {
        let remote = format!("{}/{}", t["save_path"].as_str().unwrap_or("").trim_end_matches('/'), t["name"].as_str().unwrap_or(""));
        return Ok(Poll::Done(local(st, &remote, "deluge_download_path")));
    }
    let speed = t["download_payload_rate"].as_f64().unwrap_or(0.0) / 1e6;
    let msg = if state == "Downloading" { format!("Deluge: {speed:.1} MB/s") } else { format!("Deluge: {}", state.to_lowercase()) };
    Ok(Poll::Progress((progress * 10.0).round() / 10.0, msg))
}

pub async fn remove(st: &impl Ctx, hash: &str) {
    let _ = rpc(st, "core.remove_torrent", json!([hash, true])).await;
}

/// The torrent's files by Deluge's file index. None until a magnet's metadata is there.
pub async fn files(st: &impl Ctx, hash: &str) -> anyhow::Result<Option<Vec<String>>> {
    let t = rpc(st, "core.get_torrent_status", json!([hash, ["files"]])).await?;
    let mut list: Vec<(i64, String)> = t["files"].as_array().cloned().unwrap_or_default().iter().enumerate()
        .map(|(i, f)| (f["index"].as_i64().unwrap_or(i as i64), f["path"].as_str().unwrap_or("").to_string())).collect();
    list.sort_by_key(|x| x.0);
    Ok(if list.is_empty() { None } else { Some(list.into_iter().map(|x| x.1).collect()) })
}

/// Priority 0 (skip) for every file but `keep`, which get the normal priority (4).
pub async fn select(st: &impl Ctx, hash: &str, keep: &[usize]) -> anyhow::Result<()> {
    let n = files(st, hash).await?.map(|f| f.len()).unwrap_or(0);
    let prios: Vec<i64> = (0..n).map(|i| if keep.contains(&i) { 4 } else { 0 }).collect();
    rpc(st, "core.set_torrent_options", json!([[hash], { "file_priorities": prios }])).await?;
    Ok(())
}
