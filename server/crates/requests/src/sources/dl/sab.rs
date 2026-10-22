//! SABnzbd (HTTP API with an API key). Downloads are found again by their nzo id.
use encore_http::Ctx;
use super::{base, http_error, local, Poll};
use serde_json::Value;

/// SABnzbd's own words for what went wrong, as Encore says them: the key in the wrong field and the host
/// name check are the common ones, and SABnzbd names neither in a way that says what to do.
fn explain(st: &impl Ctx, said: &str) -> String {
    let s = said.trim();
    let l = s.to_lowercase();
    if l.contains("hostname verification") {
        let host = reqwest::Url::parse(&base(st, "sab_url")).ok().and_then(|u| u.host_str().map(String::from)).unwrap_or_else(|| "the name in the SABnzbd URL".into());
        return format!("SABnzbd refused the host name Encore uses ({host}) – add {host} to SABnzbd → Config → Special → host_whitelist, or use its IP address in the SABnzbd URL");
    }
    if l.contains("api key incorrect") {
        return "SABnzbd: wrong API key – use the API Key from SABnzbd → Config → General, not the NZB Key".into();
    }
    if l.contains("api key required") {
        return "SABnzbd: no API key – paste the API Key from SABnzbd → Config → General".into();
    }
    format!("SABnzbd: {}", s.chars().take(200).collect::<String>())
}

/// One API call. Errors never carry the address with its query (the API key is in it).
async fn sab(st: &impl Ctx, q: &[(&str, &str)]) -> anyhow::Result<Value> {
    let mut q: Vec<(&str, String)> = q.iter().map(|(k, v)| (*k, v.to_string())).collect();
    q.push(("apikey", st.cfg().str("sab_api_key")));
    q.push(("output", "json".into()));
    let r = st.http().get(format!("{}/api", base(st, "sab_url"))).query(&q).send().await.map_err(|e| anyhow::anyhow!("SABnzbd unreachable: {}", http_error(e)))?;
    answer(st, r).await
}

/// Test connection: the version (which SABnzbd tells anyone), then the queue, which needs the API key
/// itself – the NZB Key is enough to add downloads but not to follow them.
pub async fn status(st: &impl Ctx) -> anyhow::Result<String> {
    let v = sab(st, &[("mode", "version")]).await?;
    sab(st, &[("mode", "queue"), ("limit", "1")]).await?;
    Ok(v["version"].as_str().map(|s| format!(" {s}")).unwrap_or_default())
}

/// The answer of a call with the API key, in [`sab`]'s words.
async fn answer(st: &impl Ctx, r: reqwest::Response) -> anyhow::Result<Value> {
    let code = r.status();
    let text = r.text().await.map_err(|e| anyhow::anyhow!("SABnzbd unreachable: {}", http_error(e)))?;
    if text.to_lowercase().contains("hostname verification") {
        anyhow::bail!(explain(st, &text));
    }
    if !code.is_success() {
        anyhow::bail!("SABnzbd answered HTTP {}{}", code.as_u16(), if code.as_u16() == 404 { " (check the URL)" } else { "" });
    }
    let v: Value = serde_json::from_str(&text).map_err(|_| anyhow::anyhow!("SABnzbd sent no JSON (check the URL)"))?;
    if v["status"] == false {
        anyhow::bail!(explain(st, v["error"].as_str().unwrap_or("error")));
    }
    Ok(v)
}

/// Post-processing for Encore's downloads: repair, unpack and delete the archives (SABnzbd's "pp" 3),
/// whatever the category says – Encore imports audio files, never archives.
const UNPACK: &str = "3";

/// Hand an NZB to SABnzbd: Encore fetches it and uploads it (Prowlarr's link may not be reachable from
/// SABnzbd); SABnzbd fetches the link itself only when Encore can't.
pub async fn add(st: &impl Ctx, url: &str, rid: i64) -> anyhow::Result<String> {
    let name = format!("encore-{rid}");
    let cat = st.cfg().str("sab_category");
    let v = match super::fetch_nzb(st, url).await {
        Ok(nzb) => {
            let q = [("mode", "addfile".to_string()), ("nzbname", name.clone()), ("cat", cat), ("pp", UNPACK.into()), ("apikey", st.cfg().str("sab_api_key")), ("output", "json".into())];
            let part = || reqwest::multipart::Part::bytes(nzb.clone()).file_name(format!("{name}.nzb"));
            let form = reqwest::multipart::Form::new().part("name", part().mime_str("application/x-nzb").unwrap_or_else(|_| part()));
            let r = st.http().post(format!("{}/api", base(st, "sab_url"))).query(&q).multipart(form).send().await
                .map_err(|e| anyhow::anyhow!("SABnzbd unreachable: {}", http_error(e)))?;
            answer(st, r).await?
        }
        Err(e) => {
            tracing::warn!("SABnzbd: Encore couldn't fetch the NZB itself ({e:#}); SABnzbd fetches the link");
            sab(st, &[("mode", "addurl"), ("name", url), ("nzbname", &name), ("cat", &cat), ("pp", UNPACK)]).await?
        }
    };
    let id = v["nzo_ids"][0].as_str().ok_or_else(|| anyhow::anyhow!("SABnzbd didn't accept the NZB"))?;
    Ok(id.to_string())
}

pub async fn poll(st: &impl Ctx, nzo: &str) -> anyhow::Result<Poll> {
    let q = sab(st, &[("mode", "queue"), ("nzo_ids", nzo)]).await?;
    if let Some(s) = q["queue"]["slots"].as_array().and_then(|a| a.first()) {
        let pct = s["percentage"].as_str().and_then(|p| p.parse().ok()).unwrap_or(0.0);
        let status = s["status"].as_str().unwrap_or("");
        // the whole queue paused (SABnzbd's own pause when the disk runs low, a schedule, or by hand):
        // nothing moves, so the stall time runs and the request says why
        let paused = q["queue"]["paused"].as_bool().unwrap_or(false) || q["queue"]["paused"].as_str() == Some("True");
        if paused {
            return Ok(Poll::Progress(pct, "SABnzbd: its queue is paused".into()));
        }
        if status == "Queued" {
            // behind SABnzbd's other downloads: no stall
            return Ok(Poll::Queued(pct, "SABnzbd: queued behind its other downloads".into()));
        }
        return Ok(Poll::Progress(pct, format!("SABnzbd: {status} {}", s["timeleft"].as_str().unwrap_or(""))));
    }
    // set to archive finished jobs (SABnzbd 4.2+), they leave the history for its archive
    let mut slot = None;
    for archive in ["0", "1"] {
        let h = sab(st, &[("mode", "history"), ("nzo_ids", nzo), ("archive", archive)]).await?;
        slot = h["history"]["slots"].as_array().and_then(|a| a.first()).cloned();
        if slot.is_some() {
            break;
        }
    }
    let Some(s) = slot else { return Ok(Poll::Failed("download disappeared from SABnzbd".into())) };
    Ok(match s["status"].as_str().unwrap_or("") {
        "Completed" => Poll::Done(local(st, s["storage"].as_str().unwrap_or(""), "sab_download_path")),
        "Failed" => Poll::Failed(format!("SABnzbd: {}", s["fail_message"].as_str().unwrap_or("failed"))),
        // downloaded, and SABnzbd verifies, repairs, unpacks or moves it
        other => Poll::Busy(99.0, format!("SABnzbd: {other}")),
    })
}

/// Completed jobs in Encore's category of SABnzbd's history (the latest 100) that Encore didn't add (Encore
/// names its jobs "encore-<request>"): (nzo id, name, folder as Encore sees it).
pub async fn finished_outside(st: &impl Ctx) -> anyhow::Result<Vec<(String, String, std::path::PathBuf)>> {
    let cat = st.cfg().str("sab_category").trim().to_string();
    if cat.is_empty() {
        return Ok(vec![]);
    }
    let h = sab(st, &[("mode", "history"), ("category", &cat), ("limit", "100")]).await?;
    Ok(h["history"]["slots"].as_array().cloned().unwrap_or_default().iter().filter(|s| {
        let name = s["name"].as_str().unwrap_or("");
        s["status"] == "Completed" && !name.starts_with("encore-") && s["category"].as_str().is_none_or(|c| c.eq_ignore_ascii_case(&cat))
    }).filter_map(|s| {
        let nzo = s["nzo_id"].as_str()?.to_string();
        Some((nzo, s["name"].as_str().unwrap_or("").to_string(), local(st, s["storage"].as_str().unwrap_or(""), "sab_download_path")))
    }).collect())
}

pub async fn remove(st: &impl Ctx, nzo: &str) {
    let _ = sab(st, &[("mode", "queue"), ("name", "delete"), ("value", nzo), ("del_files", "1")]).await;
    let _ = sab(st, &[("mode", "history"), ("name", "delete"), ("value", nzo), ("del_files", "1"), ("archive", "0")]).await;
}
