//! NZBGet over JSON-RPC (/jsonrpc, basic auth with ControlUsername/ControlPassword). NZBs are fetched by
//! Encore and added as their content (by URL when Encore can't fetch them); downloads are found again by
//! their NZBID.
use encore_http::Ctx;
use super::{base, delete_inside, local, unreachable, Poll};
use base64::Engine;
use serde_json::{json, Value};

fn endpoint(st: &impl Ctx) -> String {
    let b = base(st, "nzbget_url");
    if b.ends_with("/jsonrpc") { b } else { format!("{b}/jsonrpc") }
}

async fn rpc(st: &impl Ctx, method: &str, params: Value) -> anyhow::Result<Value> {
    let mut rb = st.http().post(endpoint(st)).json(&json!({ "version": "1.1", "method": method, "params": params, "id": 1 })).timeout(std::time::Duration::from_secs(60));
    let user = st.cfg().str("nzbget_user");
    if !user.is_empty() {
        rb = rb.basic_auth(&user, Some(st.cfg().str("nzbget_password")));
    }
    let r = rb.send().await.map_err(|e| unreachable("NZBGet", e))?;
    match r.status().as_u16() {
        401 => anyhow::bail!("NZBGet login failed: wrong username or password"),
        s if !(200..300).contains(&s) => anyhow::bail!("NZBGet answered HTTP {s} (check the URL)"),
        _ => {}
    }
    let v: Value = r.json().await.map_err(|_| anyhow::anyhow!("NZBGet sent no JSON (check the URL)"))?;
    if !v["error"].is_null() {
        anyhow::bail!("NZBGet: {}", v["error"]["message"].as_str().unwrap_or("error"));
    }
    Ok(v["result"].clone())
}

pub async fn status(st: &impl Ctx) -> anyhow::Result<String> {
    let v = rpc(st, "version", json!([])).await?;
    Ok(v.as_str().map(|s| format!(" {s}")).unwrap_or_default())
}

pub async fn add(st: &impl Ctx, url: &str, rid: i64) -> anyhow::Result<String> {
    // append(NZBFilename, NZBContent (a URL works since v13), Category, Priority, AddToTop, AddPaused, DupeKey, DupeScore, DupeMode, PPParameters).
    // Every release for a request has the same name: with DupeMode SCORE, NZBGet's duplicate check would
    // turn the next release away as a duplicate of the one Encore rejected – FORCE adds it anyway.
    // Encore fetches the NZB (Prowlarr's link may not be reachable from NZBGet); NZBGet fetches the link
    // itself only when Encore can't. Unpacked whatever the category says: Encore imports audio files,
    // never archives
    let content = match super::fetch_nzb(st, url).await {
        Ok(nzb) => base64::engine::general_purpose::STANDARD.encode(nzb),
        Err(e) => {
            tracing::warn!("NZBGet: Encore couldn't fetch the NZB itself ({e:#}); NZBGet fetches the link");
            url.to_string()
        }
    };
    let v = rpc(st, "append", json!([format!("encore-{rid}.nzb"), content, st.cfg().str("nzbget_category"), 0, false, false, "", 0, "FORCE", [{ "Name": "*Unpack:", "Value": "yes" }]])).await?;
    match v.as_i64() {
        Some(id) if id > 0 => Ok(id.to_string()),
        _ => anyhow::bail!("NZBGet didn't accept the NZB"),
    }
}

fn find(list: &Value, id: &str) -> Option<Value> {
    list.as_array()?.iter().find(|g| g["NZBID"].as_i64().is_some_and(|n| n.to_string() == id)).cloned()
}

/// Where NZBGet put a finished download (FinalDir when a script moved it).
fn dest(h: &Value) -> String {
    h["FinalDir"].as_str().filter(|d| !d.is_empty()).or(h["DestDir"].as_str()).unwrap_or("").to_string()
}

pub async fn poll(st: &impl Ctx, id: &str) -> anyhow::Result<Poll> {
    if let Some(g) = find(&rpc(st, "listgroups", json!([0])).await?, id) {
        let total = g["FileSizeMB"].as_f64().unwrap_or(0.0);
        let left = g["RemainingSizeMB"].as_f64().unwrap_or(0.0);
        let pct = if total > 0.0 { ((total - left) / total * 1000.0).round() / 10.0 } else { 0.0 };
        let status = g["Status"].as_str().unwrap_or("").to_lowercase().replace('_', " ");
        if status == "queued" {
            // behind NZBGet's other downloads: no stall
            return Ok(Poll::Queued(pct, "NZBGet: queued behind its other downloads".into()));
        }
        return Ok(Poll::Progress(pct, format!("NZBGet: {status}")));
    }
    let Some(h) = find(&rpc(st, "history", json!([false])).await?, id) else { return Ok(Poll::Failed("download disappeared from NZBGet".into())) };
    let status = h["Status"].as_str().unwrap_or("");
    // SUCCESS/ALL, SUCCESS/UNPACK …; WARNING/SCRIPT means only a post-processing script complained
    if status.starts_with("SUCCESS") || status == "WARNING/SCRIPT" {
        return Ok(Poll::Done(local(st, &dest(&h), "nzbget_download_path")));
    }
    Ok(Poll::Failed(format!("NZBGet: {}", status.to_lowercase())))
}

pub async fn remove(st: &impl Ctx, id: &str) {
    let Ok(n) = id.parse::<i64>() else { return };
    let finished = rpc(st, "history", json!([false])).await.ok().and_then(|l| find(&l, id)).map(|h| dest(&h));
    let _ = rpc(st, "editqueue", json!(["GroupFinalDelete", "", [n]])).await;
    // gone for good: HistoryDelete only hides it, and NZBGet keeps hidden ones for its duplicate check
    let _ = rpc(st, "editqueue", json!(["HistoryFinalDelete", "", [n]])).await;
    if let Some(d) = finished.filter(|d| !d.is_empty()) {
        delete_inside(st, &local(st, &d, "nzbget_download_path"), "nzbget_download_path");
    }
}
