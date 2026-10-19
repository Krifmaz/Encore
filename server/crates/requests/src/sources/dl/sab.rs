//! SABnzbd (HTTP API with an API key). Downloads are found again by their nzo id.
use encore_http::Ctx;
use super::{base, local, Poll};
use serde_json::Value;

async fn sab(st: &impl Ctx, q: &[(&str, &str)]) -> anyhow::Result<Value> {
    let mut q: Vec<(&str, String)> = q.iter().map(|(k, v)| (*k, v.to_string())).collect();
    q.push(("apikey", st.cfg().str("sab_api_key")));
    q.push(("output", "json".into()));
    let r = st.http().get(format!("{}/api", base(st, "sab_url"))).query(&q).send().await.map_err(|e| anyhow::anyhow!("SABnzbd unreachable: {e}"))?;
    let v: Value = r.error_for_status()?.json().await?;
    if v["status"] == false {
        anyhow::bail!("SABnzbd: {}", v["error"].as_str().unwrap_or("error"));
    }
    Ok(v)
}

pub async fn status(st: &impl Ctx) -> anyhow::Result<String> {
    sab(st, &[("mode", "version")]).await.map(|_| String::new())
}

pub async fn add(st: &impl Ctx, url: &str, rid: i64) -> anyhow::Result<String> {
    let name = format!("encore-{rid}");
    let v = sab(st, &[("mode", "addurl"), ("name", url), ("nzbname", &name), ("cat", &st.cfg().str("sab_category"))]).await?;
    let id = v["nzo_ids"][0].as_str().ok_or_else(|| anyhow::anyhow!("SABnzbd didn't accept the NZB"))?;
    Ok(id.to_string())
}

pub async fn poll(st: &impl Ctx, nzo: &str) -> anyhow::Result<Poll> {
    let q = sab(st, &[("mode", "queue"), ("nzo_ids", nzo)]).await?;
    if let Some(s) = q["queue"]["slots"].as_array().and_then(|a| a.first()) {
        let pct = s["percentage"].as_str().and_then(|p| p.parse().ok()).unwrap_or(0.0);
        return Ok(Poll::Progress(pct, format!("SABnzbd: {} {}", s["status"].as_str().unwrap_or(""), s["timeleft"].as_str().unwrap_or(""))));
    }
    let h = sab(st, &[("mode", "history"), ("nzo_ids", nzo)]).await?;
    let Some(s) = h["history"]["slots"].as_array().and_then(|a| a.first()).cloned() else { return Ok(Poll::Failed("download disappeared from SABnzbd".into())) };
    Ok(match s["status"].as_str().unwrap_or("") {
        "Completed" => Poll::Done(local(st, s["storage"].as_str().unwrap_or(""), "sab_download_path")),
        "Failed" => Poll::Failed(format!("SABnzbd: {}", s["fail_message"].as_str().unwrap_or("failed"))),
        other => Poll::Progress(99.0, format!("SABnzbd: {other}")),
    })
}

pub async fn remove(st: &impl Ctx, nzo: &str) {
    let _ = sab(st, &[("mode", "queue"), ("name", "delete"), ("value", nzo), ("del_files", "1")]).await;
    let _ = sab(st, &[("mode", "history"), ("name", "delete"), ("value", nzo), ("del_files", "1")]).await;
}
