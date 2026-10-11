//! Lossless downloads from Qobuz / Tidal / Deezer through the external `streamrip` tool.
//! Encore only finds the right release (ISRC or version-aware search) and hands the link to streamrip,
//! which signs in with your own subscription.
use crate::matcher::{compare, describe};
use crate::state::AppState;
use regex::Regex;
use serde_json::Value;
use std::path::{Path, PathBuf};

const DEFAULT_CMD: &str = "{bin} --config-path {config} --folder {folder} --quality {quality} --no-db --no-progress url {url}";
const AUDIO: [&str; 8] = ["flac", "mp3", "m4a", "alac", "ogg", "opus", "wav", "aiff"];

fn which(name: &str) -> Option<String> {
    if name.contains('/') {
        return Path::new(name).is_file().then(|| name.to_string());
    }
    std::env::var("PATH").ok()?.split(':').map(|d| Path::new(d).join(name)).find(|p| p.is_file()).map(|p| p.to_string_lossy().into())
}

pub fn binary(st: &AppState) -> Option<String> {
    let custom = st.cfg.str("streamrip_bin");
    if !custom.is_empty() {
        return which(&custom);
    }
    // the Docker image puts it in /opt/tools/bin – found there even if PATH was overridden
    which("streamrip").or_else(|| which("rip")).or_else(|| which("/opt/tools/bin/rip"))
}

pub fn enabled(st: &AppState) -> bool {
    st.cfg.bool("streamrip_enabled") && binary(st).is_some()
}

pub fn config_path(st: &AppState) -> PathBuf {
    st.data_dir().join("streamrip").join("config.toml")
}

fn shq(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Minimal in-place TOML edit (keeps the user's comments and other settings).
pub fn set_toml(text: &str, section: &str, key: &str, val: &str) -> String {
    let sec = Regex::new(&format!(r"(?m)^\[{}\]\s*$", regex::escape(section))).unwrap();
    let Some(m) = sec.find(text) else { return format!("{text}\n[{section}]\n{key} = {val}\n") };
    let start = m.end();
    let end = Regex::new(r"(?m)^\[").unwrap().find(&text[start..]).map(|n| start + n.start()).unwrap_or(text.len());
    let body = &text[start..end];
    let line = Regex::new(&format!(r"(?m)^{}\s*=.*$", regex::escape(key))).unwrap();
    let body = if line.is_match(body) { line.replacen(body, 1, format!("{key} = {val}").as_str()).to_string() } else { format!("{}\n{key} = {val}\n\n", body.trim_end_matches('\n')) };
    format!("{}{}{}", &text[..start], body, &text[end..])
}

fn tstr(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Write the credentials entered in Encore's settings into streamrip's config file.
pub async fn apply_credentials(st: &AppState) -> anyhow::Result<PathBuf> {
    let path = config_path(st);
    std::fs::create_dir_all(path.parent().unwrap())?;
    let mut text = std::fs::read_to_string(&path).unwrap_or_default();
    if text.is_empty() {
        // let streamrip write its default config, then edit it
        if let Some(bin) = binary(st) {
            let _ = tokio::process::Command::new(bin).args(["--config-path", &path.to_string_lossy(), "config", "path"]).output().await;
            text = std::fs::read_to_string(&path).unwrap_or_default();
        }
    }
    let arl = st.cfg.str("streamrip_deezer_arl");
    if !arl.is_empty() {
        text = set_toml(&text, "deezer", "arl", &tstr(&arl));
    }
    let (qu, qt) = (st.cfg.str("streamrip_qobuz_user"), st.cfg.str("streamrip_qobuz_token"));
    if !qu.is_empty() && !qt.is_empty() {
        text = set_toml(&text, "qobuz", "user_id", &tstr(&qu));
        text = set_toml(&text, "qobuz", "auth_token", &tstr(&qt));
    }
    text = set_toml(&text, "database", "downloads_enabled", "false");
    std::fs::write(&path, text)?;
    Ok(path)
}

// ---- resolving a request to a service link ------------------------------------------------------

pub async fn deezer_isrc(st: &AppState, isrc: &str) -> Option<Value> {
    let d: Value = st.http.get(format!("https://api.deezer.com/track/isrc:{isrc}")).send().await.ok()?.json().await.ok()?;
    d["error"].is_null().then_some(d)
}

async fn deezer_search(st: &AppState, kind: &str, q: &str) -> Vec<Value> {
    let Ok(r) = st.http.get(format!("https://api.deezer.com/search/{kind}")).query(&[("q", q), ("limit", "15")]).send().await else { return vec![] };
    r.json::<Value>().await.ok().and_then(|d| d["data"].as_array().cloned()).unwrap_or_default()
}

/// Tidal developer-API token (client credentials), cached until it expires.
pub async fn tidal_token(st: &AppState) -> anyhow::Result<String> {
    if let Some(v) = st.cache_get("tidal_token") {
        return Ok(v.as_str().unwrap_or("").to_string());
    }
    let (cid, sec) = (st.cfg.str("tidal_client_id"), st.cfg.str("tidal_client_secret"));
    if cid.is_empty() || sec.is_empty() {
        anyhow::bail!("Tidal needs a Client ID/Secret from developer.tidal.com (Settings → Integrations)");
    }
    let r = st.http.post("https://auth.tidal.com/v1/oauth2/token").basic_auth(cid, Some(sec)).form(&[("grant_type", "client_credentials")]).send().await?;
    if !r.status().is_success() {
        anyhow::bail!("Tidal auth failed: {}", r.text().await.unwrap_or_default().chars().take(200).collect::<String>());
    }
    let d: Value = r.json().await?;
    let tok = d["access_token"].as_str().unwrap_or("").to_string();
    st.cache_put("tidal_token", serde_json::json!(tok), (d["expires_in"].as_u64().unwrap_or(3600)).saturating_sub(60));
    Ok(tok)
}

async fn tidal_by_isrc(st: &AppState, isrc: &str, albums: bool) -> anyhow::Result<Option<String>> {
    let tok = tidal_token(st).await?;
    let mut q = vec![("countryCode", "US"), ("filter[isrc]", isrc)];
    if albums {
        q.push(("include", "albums"));
    }
    let d: Value = st.http.get("https://openapi.tidal.com/v2/tracks").query(&q).bearer_auth(tok).header("Accept", "application/vnd.api+json").send().await?.json().await?;
    Ok(if albums {
        d["included"].as_array().and_then(|i| i.iter().find(|x| x["type"] == "albums")).and_then(|x| x["id"].as_str()).map(|id| format!("https://tidal.com/browse/album/{id}"))
    } else {
        d["data"].as_array().and_then(|a| a.first()).and_then(|x| x["id"].as_str()).map(|id| format!("https://tidal.com/browse/track/{id}"))
    })
}

pub const SERVICES: [&str; 3] = ["qobuz", "tidal", "deezer"];

/// The streamrip services to search, in the order they appear in the one Source order setting
/// (e.g. "tidal, deezer, slskd, ytdlp"). Older setups that just say "streamrip" keep their old list.
pub fn services(st: &AppState) -> Vec<String> {
    let order: Vec<String> = st.cfg.str("source_order").split(',').map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()).collect();
    let picked: Vec<String> = order.iter().filter(|s| SERVICES.contains(&s.as_str())).cloned().collect();
    if !picked.is_empty() {
        return picked;
    }
    let legacy: Vec<String> = st.cfg.str("streamrip_sources").split(',').map(|s| s.trim().to_lowercase()).filter(|s| SERVICES.contains(&s.as_str())).collect();
    if legacy.is_empty() { vec!["tidal".into(), "deezer".into()] } else { legacy }
}

pub async fn resolve_album(st: &AppState, artist: &str, album: &str, track_count: Option<usize>, first_isrc: Option<&str>) -> Vec<String> {
    let mut urls = vec![];
    let want = describe(album, artist, "", None, None, None);
    for svc in services(st) {
        if svc == "deezer" {
            if let Some(t) = match first_isrc { Some(i) => deezer_isrc(st, i).await, None => None } {
                if let Some(id) = t["album"]["id"].as_i64() {
                    urls.push(format!("https://www.deezer.com/album/{id}"));
                    continue;
                }
            }
            let mut res = deezer_search(st, "album", &format!("artist:\"{artist}\" album:\"{album}\"")).await;
            if res.is_empty() {
                res = deezer_search(st, "album", &format!("{artist} {album}")).await;
            }
            let mut best: Option<(f64, i64)> = None;
            for a in res {
                let v = compare(&want, &describe(a["title"].as_str().unwrap_or(""), a["artist"]["name"].as_str().unwrap_or(""), "", None, None, None), true);
                if !v.matched {
                    continue;
                }
                let s = v.score + if track_count.is_some_and(|n| a["nb_tracks"].as_u64() == Some(n as u64)) { 0.2 } else { 0.0 };
                if best.is_none_or(|(b, _)| s > b) {
                    best = Some((s, a["id"].as_i64().unwrap_or(0)));
                }
            }
            if let Some((_, id)) = best {
                urls.push(format!("https://www.deezer.com/album/{id}"));
            }
        } else if svc == "tidal" && first_isrc.is_some() && !st.cfg.str("tidal_client_id").is_empty() {
            match tidal_by_isrc(st, first_isrc.unwrap(), true).await {
                Ok(Some(u)) => urls.push(u),
                Ok(None) => {}
                Err(e) => st.db.log("warning", &format!("streamrip: tidal lookup failed: {e}")),
            }
        }
    }
    urls
}

pub async fn resolve_track(st: &AppState, artist: &str, title: &str, isrc: Option<&str>, length: Option<f64>) -> Vec<String> {
    let mut urls = vec![];
    let want = describe(title, artist, "", length, isrc, None);
    for svc in services(st) {
        if svc == "deezer" {
            let mut t = match isrc { Some(i) => deezer_isrc(st, i).await, None => None };
            if t.is_none() {
                for c in deezer_search(st, "track", &format!("artist:\"{artist}\" track:\"{title}\"")).await {
                    let mut ct = c["title"].as_str().unwrap_or("").to_string();
                    if let Some(v) = c["title_version"].as_str().filter(|v| !v.is_empty() && !ct.to_lowercase().contains(&v.to_lowercase())) {
                        ct = format!("{ct} {v}");
                    }
                    if compare(&want, &describe(&ct, c["artist"]["name"].as_str().unwrap_or(""), "", c["duration"].as_f64(), None, None), true).matched {
                        t = Some(c);
                        break;
                    }
                }
            }
            if let Some(id) = t.and_then(|t| t["id"].as_i64()) {
                urls.push(format!("https://www.deezer.com/track/{id}"));
            }
        } else if svc == "tidal" && isrc.is_some() && !st.cfg.str("tidal_client_id").is_empty() {
            match tidal_by_isrc(st, isrc.unwrap(), false).await {
                Ok(Some(u)) => urls.push(u),
                Ok(None) => {}
                Err(e) => st.db.log("warning", &format!("streamrip: tidal lookup failed: {e}")),
            }
        }
    }
    urls
}

// ---- running streamrip --------------------------------------------------------------------------

pub fn audio_files(dir: &Path, exts: &[&str]) -> Vec<PathBuf> {
    let mut f: Vec<PathBuf> = walkdir::WalkDir::new(dir).into_iter().flatten().filter(|e| e.file_type().is_file())
        .map(|e| e.into_path()).filter(|p| p.extension().is_some_and(|x| exts.contains(&x.to_string_lossy().to_lowercase().as_str()))).collect();
    f.sort();
    f
}

pub async fn download(st: &AppState, url: &str, folder: &Path) -> anyhow::Result<Vec<PathBuf>> {
    std::fs::create_dir_all(folder)?;
    if !config_path(st).exists() {
        apply_credentials(st).await?;
    }
    let tpl = st.cfg.str("streamrip_command");
    let tpl = if tpl.is_empty() { DEFAULT_CMD.to_string() } else { tpl };
    let cmd = tpl.replace("{bin}", &shq(&binary(st).unwrap_or("streamrip".into())))
        .replace("{config}", &shq(&config_path(st).to_string_lossy())).replace("{folder}", &shq(&folder.to_string_lossy()))
        .replace("{quality}", &st.cfg.int("streamrip_quality").to_string()).replace("{url}", &shq(url));
    let fut = tokio::process::Command::new("sh").args(["-c", &cmd]).env("NO_COLOR", "1").env("TERM", "dumb").kill_on_drop(true).output();
    let out = tokio::time::timeout(std::time::Duration::from_secs(3600), fut).await.map_err(|_| anyhow::anyhow!("streamrip timed out"))??;
    let files = audio_files(folder, &AUDIO);
    if files.is_empty() {
        let text = String::from_utf8_lossy(&[out.stdout, out.stderr].concat()).to_string();
        let boxes = Regex::new(r"[│╭╰╮╯─━┃]+").unwrap();
        let num = Regex::new(r"^\d+\s").unwrap();
        let errs = Regex::new(r"(?i)error|invalid|fail|not (found|available)|login|arl|token|subscription").unwrap();
        let lines: Vec<String> = text.lines().map(|l| boxes.replace_all(l, " ").trim().to_string()).filter(|l| !l.is_empty() && !num.is_match(l)).collect();
        let e: Vec<&String> = lines.iter().filter(|l| errs.is_match(l)).collect();
        let pick: Vec<&String> = if e.is_empty() { lines.iter().collect() } else { e };
        let tail: Vec<&str> = pick.iter().rev().take(3).rev().map(|s| s.as_str()).collect();
        anyhow::bail!("streamrip produced no files: {}", tail.join(" / ").chars().take(300).collect::<String>());
    }
    Ok(files)
}

// ---- Tidal account sign-in (device code) -------------------------------------------------------
// Tidal has no API key for downloads: streamrip signs in with your own account once, through a link you
// open in a browser. We run that flow here so it works from Settings instead of a terminal.

#[derive(Clone, Debug, Default)]
pub struct TidalLogin {
    /// "starting" | "waiting" | "done" | "failed"
    pub status: String,
    pub url: Option<String>,
    pub message: String,
}

static TIDAL_LOGIN: std::sync::Mutex<Option<TidalLogin>> = std::sync::Mutex::new(None);

fn tidal_state() -> TidalLogin {
    TIDAL_LOGIN.lock().map(|g| g.clone().unwrap_or_default()).unwrap_or_default()
}

fn tidal_set(f: impl FnOnce(&mut TidalLogin)) {
    if let Ok(mut g) = TIDAL_LOGIN.lock() {
        let mut s = g.take().unwrap_or_default();
        f(&mut s);
        *g = Some(s);
    }
}

/// streamrip's config has a Tidal access token (the account is signed in).
pub fn tidal_signed_in(st: &AppState) -> bool {
    let Ok(text) = std::fs::read_to_string(config_path(st)) else { return false };
    let Some(i) = text.find("[tidal]") else { return false };
    let body = &text[i..];
    let body = &body[..body[1..].find("\n[").map(|n| n + 1).unwrap_or(body.len())];
    body.lines().any(|l| {
        let l = l.trim();
        l.starts_with("access_token") && l.split_once('=').map(|(_, v)| v.trim().trim_matches('"').len() > 10).unwrap_or(false)
    })
}

/// Current sign-in state for the settings page.
pub fn tidal_login_status(st: &AppState) -> Value {
    let s = tidal_state();
    serde_json::json!({ "status": s.status, "url": s.url, "message": s.message, "signed_in": tidal_signed_in(st) })
}

/// Start the sign-in: run streamrip on a Tidal link with no token, catch the link.tidal.com URL it prints,
/// and watch for the token it writes. Returns once the URL is known (or it failed).
pub async fn tidal_login_start(st: &AppState) -> anyhow::Result<Value> {
    let Some(bin) = binary(st) else { anyhow::bail!("streamrip is not installed in this container") };
    let cfg = apply_credentials(st).await?;
    // a fresh sign-in: blank any old token so streamrip asks again
    if let Ok(text) = std::fs::read_to_string(&cfg) {
        let text = set_toml(&text, "tidal", "access_token", "\"\"");
        let text = set_toml(&text, "tidal", "refresh_token", "\"\"");
        std::fs::write(&cfg, text)?;
    }
    tidal_set(|s| { *s = TidalLogin { status: "starting".into(), url: None, message: "Starting streamrip…".into() }; });
    let mut child = tokio::process::Command::new(&bin)
        .args(["--config-path", &cfg.to_string_lossy(), "--no-db", "--no-progress", "url", "https://tidal.com/browse/track/1"])
        .env("NO_COLOR", "1").env("TERM", "dumb").env("COLUMNS", "400").env("BROWSER", "true")
        .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped())
        .spawn()?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let link = Regex::new(r"(https?://)?link\.tidal\.com/[A-Za-z0-9]+").unwrap();
    let st2 = st.clone();
    tokio::spawn(async move {
        use tokio::io::AsyncBufReadExt;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        if let Some(o) = stdout {
            let tx = tx.clone();
            tokio::spawn(async move {
                let mut lines = tokio::io::BufReader::new(o).lines();
                while let Ok(Some(l)) = lines.next_line().await { let _ = tx.send(l); }
            });
        }
        if let Some(e) = stderr {
            let tx = tx.clone();
            tokio::spawn(async move {
                let mut lines = tokio::io::BufReader::new(e).lines();
                while let Ok(Some(l)) = lines.next_line().await { let _ = tx.send(l); }
            });
        }
        drop(tx);
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(330);
        let mut last = String::new();
        loop {
            let line = tokio::select! {
                l = rx.recv() => l,
                _ = tokio::time::sleep_until(deadline) => {
                    tidal_set(|s| { s.status = "failed".into(); s.message = "Timed out – the link is only valid for 5 minutes. Try again.".into(); });
                    let _ = child.kill().await;
                    return;
                }
            };
            let Some(line) = line else { break };
            let text = line.trim().to_string();
            if let Some(m) = link.find(&text) {
                let u = m.as_str();
                let u = if u.starts_with("http") { u.to_string() } else { format!("https://{u}") };
                tidal_set(|s| { s.status = "waiting".into(); s.url = Some(u); s.message = "Open the link, approve in your Tidal account, then come back here.".into(); });
            } else if !text.is_empty() {
                last = text.chars().take(200).collect();
            }
            if tidal_signed_in(&st2) {
                tidal_set(|s| { s.status = "done".into(); s.message = "Signed in to Tidal – downloads from Tidal use this account now.".into(); });
                let _ = child.kill().await;
                return;
            }
        }
        let _ = child.wait().await;
        if tidal_signed_in(&st2) {
            tidal_set(|s| { s.status = "done".into(); s.message = "Signed in to Tidal – downloads from Tidal use this account now.".into(); });
        } else {
            tidal_set(|s| { s.status = "failed".into(); s.message = if last.is_empty() { "streamrip stopped before signing in".into() } else { format!("streamrip: {last}") }; });
        }
    });
    // wait for the link (streamrip needs a moment to start)
    for _ in 0..60 {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let s = tidal_state();
        if s.url.is_some() || s.status == "failed" || s.status == "done" {
            return Ok(tidal_login_status(st));
        }
    }
    tidal_set(|s| { if s.status == "starting" { s.status = "failed".into(); s.message = "streamrip did not show a sign-in link (check the Diagnostics page)".into(); } });
    Ok(tidal_login_status(st))
}
