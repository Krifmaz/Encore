//! qBittorrent (Web API v2). Encore fetches the release itself (Prowlarr's links are its own proxy links,
//! which qBittorrent behind a VPN or on another network often can't reach) and hands over the magnet or
//! the .torrent file; the download is found again by its info hash (lower case). Downloads added by older
//! versions, by URL, are found by their tag "encore-<request id>".
use encore_http::Ctx;
use super::{base, fetch_torrent, local, magnet_info, torrent_info, Fetched, Poll};
use serde_json::{json, Value};

/// Log in to qBittorrent's Web API. Returns the session cookie as "name=value": older versions call
/// it SID, 5.1+ QBT_SID_<port>. Empty when qBittorrent skips auth for this network.
pub async fn login(st: &impl Ctx) -> anyhow::Result<String> {
    if let Some(v) = st.cache_get("qbit_sid").filter(|v| v.is_string()) {
        return Ok(v.as_str().unwrap_or("").to_string());
    }
    let url = base(st, "qbit_url");
    let r = st.http().post(format!("{url}/api/v2/auth/login")).header("Referer", &url).header("Origin", &url)
        .form(&[("username", st.cfg().str("qbit_user")), ("password", st.cfg().str("qbit_password"))]).send().await
        .map_err(|e| super::unreachable("qBittorrent", e))?;
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

/// One call. A form is built by `form` (it is built again when the call is repeated). A session qBittorrent
/// no longer knows (it restarted, or the session expired) is answered with 403: Encore logs in again and
/// repeats the call once.
async fn qbit(st: &impl Ctx, path: &str, form: Option<&(dyn Fn() -> reqwest::multipart::Form + Sync)>, query: &[(&str, String)]) -> anyhow::Result<reqwest::Response> {
    let url = format!("{}/api/v2/{path}", base(st, "qbit_url"));
    for again in [false, true] {
        let sid = login(st).await?;
        let rb = match form { Some(f) => st.http().post(&url).multipart(f()), None => st.http().get(&url).query(query) };
        let rb = if sid.is_empty() { rb } else { rb.header("Cookie", sid) };
        let r = rb.header("Referer", base(st, "qbit_url")).send().await.map_err(|e| super::unreachable("qBittorrent", e))?;
        if r.status().as_u16() == 403 {
            st.cache_put("qbit_sid", Value::Null, 1);
            if !again {
                continue;
            }
            anyhow::bail!("qBittorrent refused the session again after a new login (HTTP 403)");
        }
        if !r.status().is_success() {
            anyhow::bail!("qBittorrent answered HTTP {} to {path}", r.status().as_u16());
        }
        return Ok(r);
    }
    unreachable!("the loop returns or bails")
}

/// An id's lookup in torrents/info: by info hash, or by tag for the ids of older versions.
fn by(id: &str) -> (&'static str, String) {
    if id.starts_with("encore-") { ("tag", id.to_string()) } else { ("hashes", id.to_string()) }
}

/// The torrent behind an id, when qBittorrent lists it.
async fn lookup(st: &impl Ctx, id: &str) -> anyhow::Result<Option<Value>> {
    let (k, v) = by(id);
    let list: Value = qbit(st, "torrents/info", None, &[(k, v)]).await?.json().await?;
    Ok(list.as_array().and_then(|a| a.first()).cloned())
}

/// The form of torrents/add for one release: started at once at the top of qBittorrent's queue, in
/// Encore's category and folder, with the torrent's own folder layout (whatever qBittorrent's default
/// layout is), and tagged with the request.
fn add_form(st: &impl Ctx, tag: &str, what: &Fetched) -> reqwest::multipart::Form {
    let form = reqwest::multipart::Form::new();
    let form = match what {
        Fetched::Magnet(m) => form.text("urls", m.clone()),
        Fetched::File(b) => {
            let part = || reqwest::multipart::Part::bytes(b.clone()).file_name("encore.torrent");
            form.part("torrents", part().mime_str("application/x-bittorrent").unwrap_or_else(|_| part()))
        }
    };
    let mut form = form.text("category", st.cfg().str("qbit_category")).text("tags", tag.to_string())
        // 5.x says stopped, 4.x paused; neither waits for "metadata received"
        .text("stopped", "false").text("paused", "false").text("stopCondition", "None")
        .text("addToTopOfQueue", "true").text("contentLayout", "Original");
    let dir = st.cfg().str("qbit_download_path");
    if !dir.trim().is_empty() {
        form = form.text("savepath", dir);
    }
    form
}

/// Hand a release to qBittorrent. → its info hash. A release Encore can't fetch is handed over as its link
/// (qBittorrent fetches it, as before), found again by the tag.
pub async fn add(st: &impl Ctx, url: &str, rid: i64) -> anyhow::Result<String> {
    let tag = format!("encore-{rid}");
    let fetched = fetch_torrent(st, url).await;
    let hash = match &fetched {
        Ok(Fetched::Magnet(m)) => magnet_info(m).map(|(h, _)| h),
        Ok(Fetched::File(b)) => torrent_info(b).map(|(h, _)| h),
        Err(_) => None,
    }.map(|h| h.to_lowercase());
    let what = match fetched {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!("qBittorrent: Encore couldn't fetch the release itself ({e:#}); qBittorrent fetches the link");
            Fetched::Magnet(url.to_string())
        }
    };
    let form = || add_form(st, &tag, &what);
    let body = qbit(st, "torrents/add", Some(&form), &[]).await?.text().await.unwrap_or_default();
    let Some(hash) = hash else {
        if body.contains("Fails") {
            anyhow::bail!("qBittorrent didn't accept the torrent");
        }
        return Ok(tag);
    };
    if body.contains("Fails") {
        // qBittorrent has it already: this request's own earlier attempt (Retry) is taken over; another
        // request's download is left to it (taking it over would narrow its files to one song and delete it
        // after this import), and so is anything added outside Encore
        let tags = |t: &Value| t["tags"].as_str().unwrap_or("").split(',').map(|x| x.trim().to_string()).collect::<Vec<_>>();
        match lookup(st, &hash).await? {
            Some(t) if tags(&t).contains(&tag) => {}
            Some(t) if tags(&t).iter().any(|x| x.starts_with("encore-")) => {
                anyhow::bail!("qBittorrent is already downloading this release for another request – pick another release")
            }
            Some(_) => anyhow::bail!("qBittorrent already has this torrent (added outside Encore)"),
            None => anyhow::bail!("qBittorrent didn't accept the torrent"),
        }
    }
    Ok(hash)
}

pub async fn poll(st: &impl Ctx, id: &str) -> anyhow::Result<Poll> {
    let Some(t) = lookup(st, id).await? else { return Ok(Poll::Missing("torrent disappeared from qBittorrent".into())) };
    let state = t["state"].as_str().unwrap_or("");
    let progress = t["progress"].as_f64().unwrap_or(0.0);
    let pct = (progress * 1000.0).round() / 10.0;
    if state == "error" || state == "missingFiles" {
        return Ok(Poll::Failed(format!("qBittorrent: {state}")));
    }
    // finished once qBittorrent is done with the files: not while it checks them, or moves them out of
    // its folder for incomplete downloads (the path still points there)
    let busy = state.starts_with("checking") || state == "moving" || state == "allocating";
    if progress >= 1.0 && !busy {
        return Ok(Poll::Done(local(st, t["content_path"].as_str().unwrap_or(""), "qbit_download_path")));
    }
    if state == "queuedDL" {
        return Ok(Poll::Queued(pct, "qBittorrent: queued – its limit of downloads at once is reached".into()));
    }
    if state == "stoppedDL" || state == "pausedDL" {
        // stopped (a stop condition, or "don't start the download"): started again; 5.x calls it start,
        // 4.x resume
        let hash = t["hash"].as_str().unwrap_or("").to_string();
        for path in ["torrents/start", "torrents/resume"] {
            let f = || reqwest::multipart::Form::new().text("hashes", hash.clone());
            if qbit(st, path, Some(&f), &[]).await.is_ok() {
                break;
            }
        }
        return Ok(Poll::Progress(pct, "qBittorrent: starting the download (it was stopped)".into()));
    }
    let speed = t["dlspeed"].as_f64().unwrap_or(0.0) / 1e6;
    let msg = if state == "moving" {
        "qBittorrent: moving the files".to_string()
    } else if state.contains("stalled") || state.starts_with("meta") {
        format!("qBittorrent: waiting for peers ({state})")
    } else {
        format!("qBittorrent: {speed:.1} MB/s")
    };
    Ok(Poll::Progress(pct, msg))
}

pub async fn remove(st: &impl Ctx, id: &str) {
    let (k, v) = by(id);
    if let Ok(r) = qbit(st, "torrents/info", None, &[(k, v)]).await {
        if let Ok(v) = r.json::<Value>().await {
            let hashes: Vec<&str> = v.as_array().map(|a| a.iter().filter_map(|t| t["hash"].as_str()).collect()).unwrap_or_default();
            if !hashes.is_empty() {
                let f = || reqwest::multipart::Form::new().text("hashes", hashes.join("|")).text("deleteFiles", "true");
                let _ = qbit(st, "torrents/delete", Some(&f), &[]).await;
            }
        }
    }
}

async fn hash(st: &impl Ctx, id: &str) -> anyhow::Result<Option<String>> {
    Ok(lookup(st, id).await?.and_then(|t| t["hash"].as_str().map(String::from)))
}

/// The torrent's files in qBittorrent's order (its file index). None until the metadata is there.
pub async fn files(st: &impl Ctx, id: &str) -> anyhow::Result<Option<Vec<String>>> {
    let Some(h) = hash(st, id).await? else { return Ok(None) };
    let v: Value = qbit(st, "torrents/files", None, &[("hash", h)]).await?.json().await?;
    let mut list: Vec<(i64, String)> = v.as_array().cloned().unwrap_or_default().iter().enumerate()
        .map(|(i, f)| (f["index"].as_i64().unwrap_or(i as i64), f["name"].as_str().unwrap_or("").to_string())).collect();
    list.sort_by_key(|x| x.0);
    Ok(if list.is_empty() { None } else { Some(list.into_iter().map(|x| x.1).collect()) })
}

/// Priority 0 (don't download) for every file but `keep`.
pub async fn select(st: &impl Ctx, id: &str, keep: &[usize]) -> anyhow::Result<()> {
    let Some(h) = hash(st, id).await? else { anyhow::bail!("torrent disappeared from qBittorrent") };
    let n = files(st, id).await?.map(|f| f.len()).unwrap_or(0);
    let skip: Vec<String> = (0..n).filter(|i| !keep.contains(i)).map(|i| i.to_string()).collect();
    let want: Vec<String> = keep.iter().map(|i| i.to_string()).collect();
    for (ids, prio) in [(skip, "0"), (want, "1")] {
        if ids.is_empty() {
            continue;
        }
        let f = || reqwest::multipart::Form::new().text("hash", h.clone()).text("id", ids.join("|")).text("priority", prio);
        qbit(st, "torrents/filePrio", Some(&f), &[]).await?;
    }
    Ok(())
}
