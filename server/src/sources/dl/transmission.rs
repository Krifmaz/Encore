//! Transmission (RPC at /transmission/rpc). Every call needs the X-Transmission-Session-Id the
//! server hands out with a 409; downloads are found again by their info hash.
use super::{base, fetch_torrent, local, unreachable, Fetched, Poll};
use base64::Engine;
use crate::state::AppState;
use serde_json::{json, Value};

fn endpoint(st: &AppState) -> String {
    let b = base(st, "transmission_url");
    if b.ends_with("/rpc") { b } else { format!("{b}/transmission/rpc") }
}

async fn rpc(st: &AppState, method: &str, args: Value) -> anyhow::Result<Value> {
    let url = endpoint(st);
    let user = st.cfg.str("transmission_user");
    for _ in 0..3 {
        let sid = st.cache_get("transmission_sid").and_then(|v| v.as_str().map(String::from)).unwrap_or_default();
        let mut rb = st.http.post(&url).json(&json!({ "method": method, "arguments": args })).timeout(std::time::Duration::from_secs(60));
        if !sid.is_empty() {
            rb = rb.header("X-Transmission-Session-Id", sid);
        }
        if !user.is_empty() {
            rb = rb.basic_auth(&user, Some(st.cfg.str("transmission_password")));
        }
        let r = rb.send().await.map_err(|e| unreachable("Transmission", e))?;
        match r.status().as_u16() {
            409 => {
                let id = r.headers().get("x-transmission-session-id").and_then(|h| h.to_str().ok()).unwrap_or("").to_string();
                if id.is_empty() {
                    anyhow::bail!("Transmission answered 409 without a session id");
                }
                st.cache_put("transmission_sid", json!(id), 3600);
                continue;
            }
            401 => anyhow::bail!("Transmission login failed: wrong username or password"),
            403 => anyhow::bail!("Transmission refused the request (HTTP 403) – add Encore's address to rpc-whitelist"),
            s if !(200..300).contains(&s) => anyhow::bail!("Transmission answered HTTP {s} (check the URL)"),
            _ => {}
        }
        let v: Value = r.json().await.map_err(|_| anyhow::anyhow!("Transmission sent no JSON (check the URL)"))?;
        if v["result"] != "success" {
            anyhow::bail!("Transmission: {}", v["result"].as_str().unwrap_or("error"));
        }
        return Ok(v["arguments"].clone());
    }
    anyhow::bail!("Transmission kept asking for a new session id")
}

pub async fn status(st: &AppState) -> anyhow::Result<String> {
    let v = rpc(st, "session-get", json!({ "fields": ["version"] })).await?;
    Ok(v["version"].as_str().map(|s| format!(" {}", s.split_whitespace().next().unwrap_or(s))).unwrap_or_default())
}

/// Encore fetches the release itself (Prowlarr links often redirect to a magnet, which Transmission's own
/// download can't follow): a magnet goes in as `filename`, a .torrent file as `metainfo`.
pub async fn add(st: &AppState, url: &str) -> anyhow::Result<String> {
    let mut args = match fetch_torrent(url).await? {
        Fetched::Magnet(m) => json!({ "filename": m }),
        Fetched::File(b) => json!({ "metainfo": base64::engine::general_purpose::STANDARD.encode(b) }),
    };
    let dir = st.cfg.str("transmission_download_path");
    if !dir.trim().is_empty() {
        args["download-dir"] = json!(dir);
    }
    let label = st.cfg.str("transmission_label");
    if !label.trim().is_empty() {
        args["labels"] = json!([label]);
    }
    let v = rpc(st, "torrent-add", args).await?;
    let t = if v["torrent-added"].is_object() { &v["torrent-added"] } else { &v["torrent-duplicate"] };
    let hash = t["hashString"].as_str().filter(|h| !h.is_empty()).ok_or_else(|| anyhow::anyhow!("Transmission didn't accept the torrent"))?;
    Ok(hash.to_string())
}

const FIELDS: [&str; 9] = ["hashString", "name", "percentDone", "status", "error", "errorString", "rateDownload", "downloadDir", "peersSendingToUs"];

pub async fn poll(st: &AppState, hash: &str) -> anyhow::Result<Poll> {
    let v = rpc(st, "torrent-get", json!({ "ids": [hash], "fields": FIELDS })).await?;
    let Some(t) = v["torrents"].as_array().and_then(|a| a.first()).cloned() else { return Ok(Poll::Failed("torrent disappeared from Transmission".into())) };
    // error 3 = local error (disk, permissions); 1 and 2 are tracker warnings/errors that don't stop it
    if t["error"].as_i64() == Some(3) {
        return Ok(Poll::Failed(format!("Transmission: {}", t["errorString"].as_str().unwrap_or("local error"))));
    }
    let done = t["percentDone"].as_f64().unwrap_or(0.0);
    let status = t["status"].as_i64().unwrap_or(0); // 0 stopped, 1/2 checking, 3/4 download, 5/6 seed
    if done >= 1.0 && status != 1 && status != 2 {
        let remote = format!("{}/{}", t["downloadDir"].as_str().unwrap_or("").trim_end_matches('/'), t["name"].as_str().unwrap_or(""));
        return Ok(Poll::Done(local(st, &remote, "transmission_download_path")));
    }
    let speed = t["rateDownload"].as_f64().unwrap_or(0.0) / 1e6;
    let msg = if status == 1 || status == 2 {
        "Transmission: checking files".to_string()
    } else if t["peersSendingToUs"].as_i64().unwrap_or(0) == 0 {
        "Transmission: waiting for peers".to_string()
    } else {
        format!("Transmission: {speed:.1} MB/s")
    };
    Ok(Poll::Progress((done * 1000.0).round() / 10.0, msg))
}

pub async fn remove(st: &AppState, hash: &str) {
    let _ = rpc(st, "torrent-remove", json!({ "ids": [hash], "delete-local-data": true })).await;
}
