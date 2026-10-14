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
    // the updated copy in the config dir when it's newer, else the image's /opt/tools/bin, else PATH
    crate::tools::path(st, "streamrip").map(|p| p.to_string_lossy().into())
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
// open in a browser. Encore runs that device flow itself (streamrip's own copy dies when Tidal answers the
// server with an HTML block page) and writes the tokens into streamrip's config the way streamrip expects:
// one "lane" per OAuth client – the default client (16/44.1 FLAC) in the plain fields and, when
// `hires_client` is on, the hi-res client in the `hires_*` fields.

const TIDAL_AUTH: &str = "https://auth.tidal.com/v1/oauth2";
const TIDAL_SCOPE: &str = "r_usr+w_usr+w_sub";
const TIDAL_DEFAULT_CLIENT: (&str, &str) = ("4N3n6Q1x95LL5K7p", "oKOXfJW371cX6xaZ0PyhgGNBdNLlBZd4AKKYougMjik=");
const TIDAL_HIRES_CLIENT: (&str, &str) = ("fX2JxdmntZWK0ixT", "1Nm5AfDAjxrgJFJbKNWLeAyKGVGmINuXPPLHVXAvxAg=");
const TIDAL_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36";
/// The [tidal] keys a pasted config may carry, in the order they are written.
const TIDAL_TOKEN_KEYS: [&str; 10] = ["user_id", "country_code", "access_token", "refresh_token", "token_expiry", "token_client_id",
    "hires_access_token", "hires_refresh_token", "hires_token_expiry", "hires_token_client_id"];
const TIDAL_HIRES_KEYS: [&str; 4] = ["hires_access_token", "hires_refresh_token", "hires_token_expiry", "hires_token_client_id"];
const TIDAL_STEP1_MSG: &str = "Step 1 of 2: open the link, approve in your Tidal account, then come back here.";
const TIDAL_STEP2_MSG: &str = "Step 2 of 2: open the new link, approve in your Tidal account, then come back here.";
const TIDAL_ONE_STEP_MSG: &str = "Open the link, approve in your Tidal account, then come back here.";
const TIDAL_DONE_MSG: &str = "Signed in to Tidal – downloads from Tidal use this account now.";
const TIDAL_PARTIAL_MSG: &str = "Signed in for 16/44.1 FLAC only – sign in again for hi-res (step 2 was not finished).";

#[derive(Clone, Debug, Default)]
pub struct TidalLogin {
    /// "starting" | "waiting" | "done" | "failed"
    pub status: String,
    pub url: Option<String>,
    pub message: String,
    /// 1-based lane being approved, and how many lanes there are (1 or 2).
    pub step: u8,
    pub steps: u8,
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

/// The `[tidal]` section of a TOML text (without the header), or "" when there is none.
fn tidal_section(text: &str) -> &str {
    let Some(m) = Regex::new(r"(?m)^\s*\[tidal\]\s*$").unwrap().find(text) else { return "" };
    let body = &text[m.end()..];
    let end = Regex::new(r"(?m)^\s*\[").unwrap().find(body).map(|n| n.start()).unwrap_or(body.len());
    &body[..end]
}

/// One TOML scalar as text: quotes stripped, inline comments dropped.
fn toml_scalar(raw: &str) -> String {
    let v = raw.trim();
    if let Some(q) = v.strip_prefix('"').or_else(|| v.strip_prefix('\'')) {
        let close = if v.starts_with('"') { '"' } else { '\'' };
        return q.split(close).next().unwrap_or("").replace("\\\"", "\"").replace("\\\\", "\\");
    }
    v.split('#').next().unwrap_or("").trim().to_string()
}

/// `key = value` pairs of a `[tidal]` section. Accepts a whole config.toml or just the section, with or
/// without its `[tidal]` header, quoted or bare values.
pub fn parse_tidal_toml(text: &str) -> std::collections::BTreeMap<String, String> {
    // a whole file (any section header present) is narrowed to [tidal]; a bare key list is taken as is
    let body = if Regex::new(r"(?m)^\s*\[").unwrap().is_match(text) { tidal_section(text).to_string() } else { text.to_string() };
    let line = Regex::new(r"(?m)^\s*([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.*)$").unwrap();
    let mut out = std::collections::BTreeMap::new();
    for c in line.captures_iter(&body) {
        let key = c[1].to_string();
        if TIDAL_TOKEN_KEYS.contains(&key.as_str()) || matches!(key.as_str(), "hires_client" | "client_id" | "client_secret" | "quality") {
            out.insert(key, toml_scalar(&c[2]));
        }
    }
    out
}

/// What streamrip's config currently says about Tidal.
fn tidal_config(st: &AppState) -> std::collections::BTreeMap<String, String> {
    std::fs::read_to_string(config_path(st)).map(|t| parse_tidal_toml(&t)).unwrap_or_default()
}

fn tidal_hires_client(cfg: &std::collections::BTreeMap<String, String>) -> bool {
    cfg.get("hires_client").map(|v| v.trim() != "false").unwrap_or(true)
}

/// The OAuth clients to sign in: the one in the config if set, else streamrip's default and (with
/// `hires_client`) hi-res clients. Each lane is (client_id, client_secret, key prefix).
fn tidal_lanes(cfg: &std::collections::BTreeMap<String, String>) -> Vec<(String, String, &'static str)> {
    let (cid, sec) = (cfg.get("client_id").cloned().unwrap_or_default(), cfg.get("client_secret").cloned().unwrap_or_default());
    if !cid.is_empty() && !sec.is_empty() {
        return vec![(cid, sec, "")];
    }
    let mut lanes = vec![(TIDAL_DEFAULT_CLIENT.0.to_string(), TIDAL_DEFAULT_CLIENT.1.to_string(), "")];
    if tidal_hires_client(cfg) {
        lanes.push((TIDAL_HIRES_CLIENT.0.to_string(), TIDAL_HIRES_CLIENT.1.to_string(), "hires_"));
    }
    lanes
}

/// A TOML float with one decimal, the way streamrip stores `token_expiry`.
fn toml_float(v: f64) -> String {
    format!("{v:.1}")
}

fn now_secs() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

/// Write `[tidal]` values into streamrip's config (creating the file through apply_credentials first).
async fn tidal_write(st: &AppState, values: &[(String, String)]) -> anyhow::Result<()> {
    let path = apply_credentials(st).await?;
    let mut text = std::fs::read_to_string(&path).unwrap_or_default();
    for (k, v) in values {
        text = set_toml(&text, "tidal", k, v);
    }
    std::fs::write(&path, text)?;
    Ok(())
}

/// The TOML value for a `[tidal]` key: bare for the float/bool keys, quoted for everything else.
fn tidal_toml_value(key: &str, val: &str) -> String {
    if key == "hires_client" {
        return if val == "true" { "true".into() } else { "false".into() };
    }
    if key.ends_with("token_expiry") {
        return match val.trim().parse::<f64>() { Ok(f) => toml_float(f), Err(_) => tstr(val) };
    }
    tstr(val)
}

/// streamrip's config has a Tidal access token (the account is signed in).
pub fn tidal_signed_in(st: &AppState) -> bool {
    let cfg = tidal_config(st);
    let has = |k: &str| cfg.get(k).map(|v| v.len() > 10).unwrap_or(false);
    has("access_token") && (!tidal_hires_client(&cfg) || has("hires_access_token"))
}

/// Signed in on the default client only: hi-res is wanted but step 2 was never done.
fn tidal_partial(st: &AppState) -> bool {
    let cfg = tidal_config(st);
    let has = |k: &str| cfg.get(k).map(|v| v.len() > 10).unwrap_or(false);
    has("access_token") && tidal_hires_client(&cfg) && !has("hires_access_token")
}

/// Current sign-in state for the settings page.
pub fn tidal_login_status(st: &AppState) -> Value {
    let s = tidal_state();
    let mut out = serde_json::json!({ "status": s.status, "url": s.url, "message": s.message, "signed_in": tidal_signed_in(st) });
    if s.steps > 0 {
        out["step"] = serde_json::json!(s.step);
        out["steps"] = serde_json::json!(s.steps);
    }
    if tidal_partial(st) && s.status != "waiting" && s.status != "starting" {
        out["partial"] = serde_json::json!(true);
        if s.status != "failed" {
            out["message"] = serde_json::json!(TIDAL_PARTIAL_MSG);
        }
    }
    out
}

/// The message for a Tidal answer that is not JSON (an HTML block page) or a 403/429/5xx.
pub fn tidal_blocked_message(code: u16, body: &str) -> String {
    let mut m = format!("Tidal refused this server's address (HTTP {code}, an HTML page instead of an answer). Tidal blocks many hosting and VPN addresses. Sign in on your own computer instead and paste the tokens below, or give the container a different route to the internet.");
    if let Some(c) = Regex::new(r"(?is)<title[^>]*>(.*?)</title>").unwrap().captures(body) {
        let title: String = c[1].split_whitespace().collect::<Vec<_>>().join(" ");
        let title = title.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#39;", "'");
        if !title.is_empty() {
            m.push_str(" – ");
            m.extend(title.chars().take(120));
        }
    }
    m
}

/// POST a form to auth.tidal.com with browser-like headers; Ok((status, json)) or Err(plain message).
async fn tidal_auth_post(st: &AppState, path: &str, form: &[(&str, &str)], basic: Option<(&str, &str)>) -> Result<(u16, Value), String> {
    let mut req = st.http.post(format!("{TIDAL_AUTH}/{path}"))
        .header("User-Agent", TIDAL_UA).header("Accept", "application/json").header("Accept-Language", "en-US,en;q=0.9").form(form);
    if let Some((u, p)) = basic {
        req = req.basic_auth(u, Some(p));
    }
    let r = req.send().await.map_err(|e| format!("Could not reach Tidal: {e}"))?;
    let code = r.status().as_u16();
    let text = r.text().await.unwrap_or_default();
    // a JSON object of any status is an answer (pending/invalid-code JSON is handled by the caller);
    // anything else (an HTML block page, an empty body) means the address is refused
    match serde_json::from_str::<Value>(&text) {
        Ok(v) if v.is_object() => Ok((code, v)),
        _ => Err(tidal_blocked_message(code, &text)),
    }
}

/// One lane's device flow: get the link, show it, poll until approved. Returns the token answer.
async fn tidal_lane_flow(st: &AppState, cid: &str, sec: &str, step: u8, steps: u8) -> Result<Value, String> {
    let (_, d) = tidal_auth_post(st, "device_authorization", &[("client_id", cid), ("scope", TIDAL_SCOPE)], None).await?;
    let Some(code) = d["deviceCode"].as_str().map(str::to_string) else {
        return Err(format!("Tidal did not hand out a sign-in code: {}", d["error_description"].as_str().or(d["error"].as_str()).unwrap_or("no deviceCode in the answer")));
    };
    let Some(url) = d["verificationUriComplete"].as_str().map(|u| if u.starts_with("http") { u.to_string() } else { format!("https://{u}") }) else {
        return Err("Tidal did not hand out a sign-in link".into());
    };
    let expires = d["expiresIn"].as_f64().unwrap_or(300.0).max(30.0);
    let interval = d["interval"].as_f64().unwrap_or(2.0).max(2.0);
    let message = if steps == 1 { TIDAL_ONE_STEP_MSG } else if step == 1 { TIDAL_STEP1_MSG } else { TIDAL_STEP2_MSG };
    tidal_set(|s| { s.status = "waiting".into(); s.url = Some(url.clone()); s.message = message.into(); s.step = step; s.steps = steps; });
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs_f64(expires);
    loop {
        tokio::time::sleep(std::time::Duration::from_secs_f64(interval)).await;
        if tokio::time::Instant::now() >= deadline {
            return Err("Timed out – the link is only valid for 5 minutes. Try again.".into());
        }
        let form = [("client_id", cid), ("device_code", code.as_str()), ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"), ("scope", TIDAL_SCOPE)];
        let (http, t) = tidal_auth_post(st, "token", &form, Some((cid, sec))).await?;
        if t["access_token"].is_string() {
            return Ok(t);
        }
        let pending = t["sub_status"].as_i64() == Some(1002) || t["error"].as_str() == Some("authorization_pending") || t["error"].as_str() == Some("slow_down");
        if http == 400 && pending {
            continue;
        }
        let why = t["error_description"].as_str().or(t["error"].as_str()).map(str::to_string).unwrap_or_else(|| format!("HTTP {http}"));
        return Err(format!("Tidal did not approve the sign-in: {why}"));
    }
}

/// Start the sign-in: the device flow runs in the background, lane by lane, and writes the tokens into
/// streamrip's config. Returns once the first link is known (or it failed).
pub async fn tidal_login_start(st: &AppState) -> anyhow::Result<Value> {
    apply_credentials(st).await?;
    let cfg = tidal_config(st);
    let lanes = tidal_lanes(&cfg);
    let steps = lanes.len() as u8;
    // the old tokens stay until lane 1 is approved: a refused link (Tidal blocks the address) must not
    // throw away a working, pasted-in sign-in
    tidal_set(|s| { *s = TidalLogin { status: "starting".into(), url: None, message: "Asking Tidal for a sign-in link…".into(), step: 1, steps }; });
    let st2 = st.clone();
    tokio::spawn(async move {
        let mut values: Vec<(String, String)> = vec![];
        for (i, (cid, sec, prefix)) in lanes.iter().enumerate() {
            let step = i as u8 + 1;
            match tidal_lane_flow(&st2, cid, sec, step, steps).await {
                Ok(t) => {
                    let expiry = t["expires_in"].as_f64().unwrap_or(604800.0) + now_secs();
                    if i == 0 {
                        values.push(("user_id".into(), tstr(&t["user"]["userId"].to_string().trim_matches('"').to_string())));
                        values.push(("country_code".into(), tstr(t["user"]["countryCode"].as_str().unwrap_or(""))));
                        // the new default lane replaces the whole old sign-in: blank the hi-res lane with it so a
                        // stale hi-res token from another account is never mixed in (lane 2 fills it again)
                        for k in TIDAL_HIRES_KEYS {
                            values.push((k.to_string(), "\"\"".to_string()));
                        }
                    }
                    values.push((format!("{prefix}access_token"), tstr(t["access_token"].as_str().unwrap_or(""))));
                    values.push((format!("{prefix}refresh_token"), tstr(t["refresh_token"].as_str().unwrap_or(""))));
                    values.push((format!("{prefix}token_expiry"), toml_float(expiry)));
                    values.push((format!("{prefix}token_client_id"), tstr(cid)));
                    // the first lane is saved at once, so a stopped step 2 still leaves a working 16/44.1 sign-in
                    if let Err(e) = tidal_write(&st2, &values).await {
                        tidal_set(|s| { s.status = "failed".into(); s.url = None; s.message = format!("Could not write streamrip's config: {e}"); });
                        return;
                    }
                    values.clear();
                }
                Err(e) => {
                    tidal_set(|s| { s.status = "failed".into(); s.url = None; s.message = e; });
                    return;
                }
            }
        }
        let hires = lanes.iter().any(|l| l.2 == "hires_");
        if let Err(e) = tidal_write(&st2, &[("hires_client".into(), if hires { "true".into() } else { "false".into() })]).await {
            tidal_set(|s| { s.status = "failed".into(); s.url = None; s.message = format!("Could not write streamrip's config: {e}"); });
            return;
        }
        st2.db.log("info", &format!("streamrip: signed in to Tidal ({} client{})", lanes.len(), if lanes.len() == 1 { "" } else { "s" }));
        tidal_set(|s| { s.status = "done".into(); s.url = None; s.message = TIDAL_DONE_MSG.into(); s.step = steps; });
    });
    // wait for the link
    for _ in 0..60 {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let s = tidal_state();
        if s.url.is_some() || s.status == "failed" || s.status == "done" {
            return Ok(tidal_login_status(st));
        }
    }
    tidal_set(|s| { if s.status == "starting" { s.status = "failed".into(); s.message = "Tidal did not answer within 30 seconds. Try again.".into(); } });
    Ok(tidal_login_status(st))
}

pub const TIDAL_NO_TOKEN_MSG: &str = "No access_token found in the pasted text. Paste the [tidal] section of streamrip's config.toml, including its access_token line.";

/// Does a pasted `[tidal]` section (or whole config.toml) carry a default-lane access_token? Anything
/// else about the paste that goes wrong is a server-side problem, not a bad paste.
pub fn tidal_toml_has_token(toml: &str) -> bool {
    parse_tidal_toml(toml).get("access_token").map(|v| v.len() > 10).unwrap_or(false)
}

/// "Paste tokens instead": a `[tidal]` section (or whole config.toml) from a streamrip run on another
/// computer. Writes the token fields and reports the sign-in state like the status route.
pub async fn tidal_tokens_save(st: &AppState, toml: &str) -> anyhow::Result<Value> {
    let got = parse_tidal_toml(toml);
    if !tidal_toml_has_token(toml) {
        anyhow::bail!("{TIDAL_NO_TOKEN_MSG}");
    }
    let mut values: Vec<(String, String)> = TIDAL_TOKEN_KEYS.iter().filter_map(|k| got.get(*k).map(|v| (k.to_string(), tidal_toml_value(k, v)))).collect();
    // a pasted default-lane token with no client id came from streamrip's default client
    if !got.contains_key("token_client_id") {
        values.push(("token_client_id".into(), tstr(TIDAL_DEFAULT_CLIENT.0)));
    }
    let hires = got.get("hires_access_token").map(|v| v.len() > 10).unwrap_or(false);
    if hires && !got.contains_key("hires_token_client_id") {
        values.push(("hires_token_client_id".into(), tstr(TIDAL_HIRES_CLIENT.0)));
    }
    if let Some(h) = got.get("hires_client") {
        values.push(("hires_client".into(), tidal_toml_value("hires_client", h)));
    } else {
        values.push(("hires_client".into(), if hires { "true".into() } else { "false".into() }));
    }
    tidal_write(st, &values).await?;
    tidal_set(|s| { *s = TidalLogin { status: "done".into(), url: None, message: "Tokens saved – downloads from Tidal use this account now.".into(), step: 0, steps: 0 }; });
    st.db.log("info", "streamrip: Tidal tokens pasted in from another computer");
    Ok(tidal_login_status(st))
}

/// "Sign out of Tidal": blank every token field.
pub async fn tidal_sign_out(st: &AppState) -> anyhow::Result<Value> {
    let values: Vec<(String, String)> = TIDAL_TOKEN_KEYS.iter().map(|k| (k.to_string(), "\"\"".to_string())).collect();
    tidal_write(st, &values).await?;
    tidal_set(|s| { *s = TidalLogin::default(); });
    Ok(tidal_login_status(st))
}

/// Diagnostics: can this server talk to auth.tidal.com at all? A JSON answer (even an error) means yes.
pub async fn tidal_probe(st: &AppState) -> String {
    match tidal_auth_post(st, "device_authorization", &[("client_id", "encore-probe"), ("scope", TIDAL_SCOPE)], None).await {
        Ok(_) => "Tidal reachable".into(),
        Err(e) if e.starts_with("Could not reach") => e,
        Err(_) => "Tidal blocks this server's address – see Sign in to Tidal".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECTION: &str = r#"
user_id = "12345678"
country_code = "US"
access_token = "eyJhbGciOiJIUzI1NiJ9.default-token-abcdef"
refresh_token = "refresh-default"
token_expiry = 1760000000.5
token_client_id = "4N3n6Q1x95LL5K7p"
hires_access_token = "eyJhbGciOiJIUzI1NiJ9.hires-token-abcdef"
hires_refresh_token = "refresh-hires"
hires_token_expiry = "1760000001.0"
hires_token_client_id = "fX2JxdmntZWK0ixT"
"#;

    #[test]
    fn parses_quoted_and_unquoted_section() {
        let got = parse_tidal_toml(SECTION);
        assert_eq!(got["user_id"], "12345678");
        assert_eq!(got["token_expiry"], "1760000000.5");
        assert_eq!(got["hires_token_expiry"], "1760000001.0");
        assert_eq!(got["hires_access_token"], "eyJhbGciOiJIUzI1NiJ9.hires-token-abcdef");
        assert_eq!(got["hires_token_client_id"], "fX2JxdmntZWK0ixT");
        let bare = parse_tidal_toml("access_token = abc-def-ghi-jkl # comment\ncountry_code = 'NL'\nuser_id = 42");
        assert_eq!(bare["access_token"], "abc-def-ghi-jkl");
        assert_eq!(bare["country_code"], "NL");
        assert_eq!(bare["user_id"], "42");
    }

    #[test]
    fn parses_whole_file_and_only_its_tidal_section() {
        let file = format!("[downloads]\nfolder = \"/x\"\n\n[qobuz]\nuser_id = \"q\"\nauth_token = \"zzz\"\n\n[tidal]\nquality = 3\nhires_client = true{SECTION}\n[deezer]\narl = \"d\"\nuser_id = \"dz\"\n");
        let got = parse_tidal_toml(&file);
        assert_eq!(got["user_id"], "12345678");
        assert_eq!(got["hires_client"], "true");
        assert_eq!(got["refresh_token"], "refresh-default");
        assert!(!got.contains_key("arl"));
        assert!(!got.contains_key("auth_token"));
        // a pasted section that still has its header is fine too
        assert_eq!(parse_tidal_toml(&format!("[tidal]{SECTION}"))["country_code"], "US");
        assert!(parse_tidal_toml("[qobuz]\nuser_id = \"q\"\n").is_empty());
    }

    #[test]
    fn set_toml_round_trips_both_lanes() {
        let mut text = "[downloads]\nfolder = \"/x\"\n\n[tidal]\nquality = 3\naccess_token = \"\"\ntoken_expiry = \"\"\nhires_client = false\n\n[deezer]\narl = \"d\"\n".to_string();
        let (e1, e2) = (toml_float(1760000000.0), toml_float(1760000001.75));
        let lanes: Vec<(&str, &str)> = vec![("user_id", "\"12345678\""), ("country_code", "\"US\""), ("access_token", "\"default-token-abcdef\""), ("refresh_token", "\"r1\""),
            ("token_expiry", &e1), ("token_client_id", "\"4N3n6Q1x95LL5K7p\""), ("hires_access_token", "\"hires-token-abcdef\""),
            ("hires_refresh_token", "\"r2\""), ("hires_token_expiry", &e2), ("hires_token_client_id", "\"fX2JxdmntZWK0ixT\""), ("hires_client", "true")];
        for (k, v) in &lanes {
            text = set_toml(&text, "tidal", k, v);
        }
        let got = parse_tidal_toml(&text);
        assert_eq!(got["access_token"], "default-token-abcdef");
        assert_eq!(got["token_expiry"], "1760000000.0");
        assert_eq!(got["hires_token_expiry"], "1760000001.8");
        assert_eq!(got["token_client_id"], "4N3n6Q1x95LL5K7p");
        assert_eq!(got["hires_token_client_id"], "fX2JxdmntZWK0ixT");
        assert_eq!(got["hires_client"], "true");
        assert_eq!(got["quality"], "3");
        // the other sections are untouched and nothing leaked into them
        assert!(text.contains("[downloads]\nfolder = \"/x\""));
        assert!(text.trim_end().ends_with("[deezer]\narl = \"d\""));
        assert_eq!(text.matches("access_token").count(), 2, "one plain, one hires_ – no duplicate lines");
        assert_eq!(tidal_toml_value("token_expiry", "1760000000"), "1760000000.0");
        assert_eq!(tidal_toml_value("hires_client", "yes"), "false");
        assert_eq!(tidal_toml_value("access_token", "a\"b"), "\"a\\\"b\"");
    }

    #[test]
    fn lanes_follow_the_config() {
        let two = tidal_lanes(&parse_tidal_toml("quality = 3"));
        assert_eq!(two.len(), 2);
        assert_eq!(two[0].0, TIDAL_DEFAULT_CLIENT.0);
        assert_eq!(two[1].2, "hires_");
        assert_eq!(tidal_lanes(&parse_tidal_toml("hires_client = false")).len(), 1);
        let own = tidal_lanes(&parse_tidal_toml("client_id = \"me\"\nclient_secret = \"s\"\nhires_client = true"));
        assert_eq!(own.len(), 1);
        assert_eq!(own[0].0, "me");
    }

    #[test]
    fn html_403_message() {
        let html = "<html><head>\n<title>  Access denied | auth.tidal.com used\n Cloudflare to restrict access</title></head><body>blocked</body></html>";
        let m = tidal_blocked_message(403, html);
        assert!(m.starts_with("Tidal refused this server's address (HTTP 403, an HTML page instead of an answer). Tidal blocks many hosting and VPN addresses. Sign in on your own computer instead and paste the tokens below, or give the container a different route to the internet."));
        assert!(m.ends_with(" – Access denied | auth.tidal.com used Cloudflare to restrict access"), "{m}");
        assert!(!m.contains('!'));
        let plain = tidal_blocked_message(502, "bad gateway");
        assert!(plain.contains("HTTP 502"));
        assert!(!plain.contains(" – "));
        let long = format!("<title>{}</title>", "x".repeat(300));
        assert_eq!(tidal_blocked_message(403, &long).split(" – ").nth(1).unwrap().len(), 120);
    }

    #[test]
    fn partial_status_when_hires_token_missing() {
        let has = |cfg: &std::collections::BTreeMap<String, String>, k: &str| cfg.get(k).map(|v| v.len() > 10).unwrap_or(false);
        let partial = |text: &str| { let c = parse_tidal_toml(text); has(&c, "access_token") && tidal_hires_client(&c) && !has(&c, "hires_access_token") };
        let signed = |text: &str| { let c = parse_tidal_toml(text); has(&c, "access_token") && (!tidal_hires_client(&c) || has(&c, "hires_access_token")) };
        let only_default = "hires_client = true\naccess_token = \"default-token-abcdef\"\nhires_access_token = \"\"";
        assert!(partial(only_default));
        assert!(!signed(only_default));
        assert!(signed(SECTION));
        assert!(!partial(SECTION));
        assert!(signed("hires_client = false\naccess_token = \"default-token-abcdef\""));
        assert!(!partial("access_token = \"\""));
        assert_eq!(TIDAL_PARTIAL_MSG, "Signed in for 16/44.1 FLAC only – sign in again for hi-res (step 2 was not finished).");
    }

    #[test]
    fn pasted_text_needs_an_access_token() {
        assert!(tidal_toml_has_token(SECTION));
        assert!(tidal_toml_has_token(&format!("[tidal]{SECTION}")));
        assert!(!tidal_toml_has_token(""));
        assert!(!tidal_toml_has_token("user_id = \"12345678\"\naccess_token = \"\""));
        assert!(!tidal_toml_has_token("[qobuz]\nuser_id = \"q\"\n"));
        assert!(!tidal_toml_has_token("hires_access_token = \"eyJhbGciOiJIUzI1NiJ9.hires-token-abcdef\""));
        assert!(TIDAL_NO_TOKEN_MSG.starts_with("No access_token found"));
    }

    #[test]
    fn a_new_default_lane_blanks_the_hires_lane() {
        // the write for lane 1 carries blank hires_ fields, so a stale hi-res token never outlives a new sign-in
        let mut text = "[tidal]\nquality = 3\nhires_client = true".to_string();
        for (k, v) in [("access_token", "\"old-default-token-abcdef\""), ("hires_access_token", "\"old-hires-token-abcdef\""), ("hires_token_client_id", "\"fX2JxdmntZWK0ixT\"")] {
            text = set_toml(&text, "tidal", k, v);
        }
        let mut values: Vec<(String, String)> = vec![("access_token".into(), "\"new-default-token-abcdef\"".into())];
        for k in TIDAL_HIRES_KEYS {
            values.push((k.to_string(), "\"\"".to_string()));
        }
        for (k, v) in &values {
            text = set_toml(&text, "tidal", k, v);
        }
        let got = parse_tidal_toml(&text);
        assert_eq!(got["access_token"], "new-default-token-abcdef");
        assert_eq!(got["hires_access_token"], "");
        assert_eq!(got["hires_token_client_id"], "");
        assert_eq!(TIDAL_HIRES_KEYS.len(), 4);
        assert!(TIDAL_HIRES_KEYS.iter().all(|k| TIDAL_TOKEN_KEYS.contains(k) && k.starts_with("hires_")));
    }
}
