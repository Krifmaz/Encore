//! Lossless downloads from Qobuz / Tidal / Deezer through the external `streamrip` tool.
//! Encore only finds the right release (ISRC or version-aware search) and hands the link to streamrip,
//! which signs in with your own subscription.
use crate::Ctx;
use encore_core::audio::STREAMRIP_EXTS as AUDIO;
use encore_library::matcher::{compare, describe};
use regex::Regex;
use serde_json::Value;
use std::path::{Path, PathBuf};

const DEFAULT_CMD: &str = "{bin} --config-path {config} --folder {folder} --quality {quality} --no-db --no-progress url {url}";

pub fn binary(st: &impl Ctx) -> Option<String> {
    let custom = st.cfg().str("streamrip_bin");
    if !custom.is_empty() {
        return encore_core::which::which(&custom).map(|p| p.to_string_lossy().into_owned());
    }
    // the updated copy in the config dir when it's newer, else the image's /opt/tools/bin, else PATH
    encore_system::tools::path(st, "streamrip").map(|p| p.to_string_lossy().into())
}

pub fn enabled(st: &impl Ctx) -> bool {
    st.cfg().bool("streamrip_enabled") && binary(st).is_some()
}

pub fn config_path(st: &impl Ctx) -> PathBuf {
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

// ---- the config file ----------------------------------------------------------------------------
// streamrip reads one TOML file. Encore lets streamrip write the defaults (it does so for a path that
// has no file yet), checks that the file loads with `rip --config-path X config path`, and when it does
// not – a half-written file, the format of an older streamrip, a key it does not know – moves the file
// aside and writes a fresh one, carrying the sign-ins over. The credentials from Encore's settings go
// in before every run, not only when the file is new.

pub const NOT_INSTALLED_MSG: &str = "streamrip is not installed in this container (use the default Docker image or set its path)";
/// Deezer runs need the ARL cookie; streamrip would ask for it on a terminal, which a server has none of.
pub const ARL_MSG: &str = "Deezer needs an ARL cookie – Settings → streamrip";
pub const CONFIG_BROKEN_MSG: &str = "streamrip can't load its config – press Reset streamrip config in Settings → streamrip";
/// The line streamrip prints when its config does not load (the text after the colon says why).
const CONFIG_ERROR: &str = "Error loading config";
/// How much of a run's output the request's log keeps.
const LOG_CAP: usize = 20_000;
/// How much of it the request's message shows.
const MESSAGE_CAP: usize = 300;
/// Time limit for a config check or regeneration (a Python start-up, normally a second).
const CHECK_SECS: u64 = 90;

/// The body of a `[section]` (without the header), or "" when there is none.
pub fn section_body<'a>(text: &'a str, name: &str) -> &'a str {
    let Some(m) = Regex::new(&format!(r"(?m)^\s*\[{}\]\s*$", regex::escape(name))).unwrap().find(text) else { return "" };
    let body = &text[m.end()..];
    let end = Regex::new(r"(?m)^\s*\[").unwrap().find(body).map(|n| n.start()).unwrap_or(body.len());
    &body[..end]
}

/// One `key = value` of a section as plain text ("" when missing).
pub fn toml_value(text: &str, section: &str, key: &str) -> String {
    let line = Regex::new(&format!(r"(?m)^\s*{}\s*=\s*(.*)$", regex::escape(key))).unwrap();
    line.captures(section_body(text, section)).map(|c| toml_scalar(&c[1])).unwrap_or_default()
}

/// streamrip 2.x names the Qobuz login `email_or_userid` / `password_or_token` (with `use_auth_token`);
/// 1.x had `user_id` / `auth_token`. The file streamrip wrote says which it is.
fn qobuz_v2(text: &str) -> bool {
    let q = section_body(text, "qobuz");
    q.contains("email_or_userid") || !q.contains("auth_token")
}

/// Write the credentials entered in Encore's settings into streamrip's config file. Keys the settings
/// leave empty stay as they are in the file, so a login typed into config.toml by hand is kept.
pub fn apply_credentials(st: &impl Ctx) -> anyhow::Result<PathBuf> {
    let path = config_path(st);
    std::fs::create_dir_all(path.parent().unwrap())?;
    let mut text = std::fs::read_to_string(&path).unwrap_or_default();
    let arl = st.cfg().str("streamrip_deezer_arl");
    if !arl.is_empty() {
        text = set_toml(&text, "deezer", "arl", &tstr(&arl));
    }
    let (qu, qt) = (st.cfg().str("streamrip_qobuz_user"), st.cfg().str("streamrip_qobuz_token"));
    if !qu.is_empty() && !qt.is_empty() {
        if qobuz_v2(&text) {
            text = set_toml(&text, "qobuz", "use_auth_token", "true");
            text = set_toml(&text, "qobuz", "email_or_userid", &tstr(&qu));
            text = set_toml(&text, "qobuz", "password_or_token", &tstr(&qt));
        } else {
            text = set_toml(&text, "qobuz", "user_id", &tstr(&qu));
            text = set_toml(&text, "qobuz", "auth_token", &tstr(&qt));
        }
    }
    text = set_toml(&text, "database", "downloads_enabled", "false");
    std::fs::write(&path, text)?;
    Ok(path)
}

/// Run streamrip for a check: stdin closed (it can never wait for a key), no colours, a time limit.
/// Ok((exited 0, combined output)).
async fn run_rip(bin: &str, args: &[&str], secs: u64) -> anyhow::Result<(bool, String)> {
    let fut = tokio::process::Command::new(bin).args(args).stdin(std::process::Stdio::null())
        .env("NO_COLOR", "1").env("TERM", "dumb").env("PYTHONUNBUFFERED", "1").kill_on_drop(true).output();
    let out = tokio::time::timeout(std::time::Duration::from_secs(secs), fut).await.map_err(|_| anyhow::anyhow!("streamrip did not answer within {secs} s"))??;
    Ok((out.status.success(), String::from_utf8_lossy(&[out.stdout, out.stderr].concat()).to_string()))
}

/// Does the config at `path` load? `rip --config-path X config path` says so: a non-zero exit or the
/// fork's "Error loading config" line means no. Err carries the complaint, one line.
pub async fn validate_config(bin: &str, path: &Path) -> Result<(), String> {
    let p = path.to_string_lossy().to_string();
    match run_rip(bin, &["--config-path", &p, "config", "path"], CHECK_SECS).await {
        Err(e) => Err(e.to_string()),
        Ok((true, text)) if !text.contains(CONFIG_ERROR) => Ok(()),
        Ok((_, text)) => Err(summary(&text, MESSAGE_CAP)),
    }
}

/// Move the config aside (config.toml.bak-<date>) and let streamrip write its defaults. Returns the backup.
async fn regenerate(bin: &str, path: &Path) -> anyhow::Result<Option<PathBuf>> {
    let backup = if path.exists() {
        let b = path.with_file_name(format!("config.toml.bak-{}", encore_db::backup::stamp(encore_db::now())));
        std::fs::rename(path, &b)?;
        Some(b)
    } else {
        None
    };
    let p = path.to_string_lossy().to_string();
    // streamrip writes its default config for a path that has no file yet; `config reset` is the fallback
    let _ = run_rip(bin, &["--config-path", &p, "config", "path"], CHECK_SECS).await;
    if !path.is_file() {
        let _ = run_rip(bin, &["--config-path", &p, "config", "reset", "-y"], CHECK_SECS).await;
    }
    if !path.is_file() {
        anyhow::bail!("streamrip did not write a default config at {}", path.display());
    }
    Ok(backup)
}

/// The sign-ins an old config held, written into a fresh one: the [tidal] tokens (and the user's own
/// client and hires_client), the Deezer ARL, the Qobuz pair under whichever key names the fresh file uses.
fn carry_over(old: &str, fresh: &str) -> String {
    let mut text = fresh.to_string();
    let t = parse_tidal_toml(old);
    for k in TIDAL_TOKEN_KEYS.iter().chain(["hires_client", "client_id", "client_secret"].iter()) {
        if let Some(v) = t.get(*k).filter(|v| !v.is_empty()) {
            text = set_toml(&text, "tidal", k, &tidal_toml_value(k, v));
        }
    }
    let arl = toml_value(old, "deezer", "arl");
    if !arl.is_empty() {
        text = set_toml(&text, "deezer", "arl", &tstr(&arl));
    }
    let user = [toml_value(old, "qobuz", "email_or_userid"), toml_value(old, "qobuz", "user_id")].into_iter().find(|v| !v.is_empty());
    let token = [toml_value(old, "qobuz", "password_or_token"), toml_value(old, "qobuz", "auth_token")].into_iter().find(|v| !v.is_empty());
    if let (Some(u), Some(k)) = (user, token) {
        if qobuz_v2(&text) {
            text = set_toml(&text, "qobuz", "use_auth_token", "true");
            text = set_toml(&text, "qobuz", "email_or_userid", &tstr(&u));
            text = set_toml(&text, "qobuz", "password_or_token", &tstr(&k));
        } else {
            text = set_toml(&text, "qobuz", "user_id", &tstr(&u));
            text = set_toml(&text, "qobuz", "auth_token", &tstr(&k));
        }
    }
    text
}

/// What `check_config` found and did.
#[derive(Debug, Default)]
pub struct ConfigCheck {
    pub path: PathBuf,
    /// A fresh file was written (the old one, if any, is `backup`).
    pub regenerated: bool,
    pub backup: Option<PathBuf>,
    /// Why the config still does not load (or why it could not be checked), when it does not.
    pub problem: Option<String>,
}

impl ConfigCheck {
    pub fn json(&self) -> Value {
        serde_json::json!({ "ok": self.problem.is_none(), "path": self.path.to_string_lossy(), "regenerated": self.regenerated,
            "backup": self.backup.as_ref().map(|b| b.to_string_lossy().to_string()), "message": self.message() })
    }

    pub fn message(&self) -> String {
        let where_ = self.path.display();
        match (&self.problem, self.regenerated) {
            (Some(p), true) => format!("streamrip wrote a fresh config at {where_}, but it still doesn't load: {p}"),
            (Some(p), false) => format!("streamrip can't load its config at {where_}: {p}"),
            (None, true) => match &self.backup {
                Some(b) => format!("streamrip config written fresh at {where_} – the old file is kept as {} and its Tidal sign-in, Deezer ARL and Qobuz login were carried over", b.display()),
                None => format!("streamrip config written fresh at {where_}"),
            },
            (None, false) => format!("streamrip config loads ({where_})"),
        }
    }
}

/// Make sure streamrip's config loads: a missing file is written from streamrip's defaults; one that
/// fails the check (or `force`) is backed up, regenerated and gets its sign-ins carried over. The
/// credentials from Encore's settings are written in every time.
pub async fn check_config(st: &impl Ctx, force: bool) -> anyhow::Result<ConfigCheck> {
    let path = config_path(st);
    std::fs::create_dir_all(path.parent().unwrap())?;
    let mut out = ConfigCheck { path: path.clone(), ..Default::default() };
    let Some(bin) = binary(st) else {
        // nothing can run without the program; the credentials still land in the file for when it is there
        apply_credentials(st)?;
        out.problem = Some(NOT_INSTALLED_MSG.into());
        return Ok(out);
    };
    let why = if !path.is_file() {
        Some("no config file yet".to_string())
    } else if force {
        Some("reset asked for".to_string())
    } else {
        validate_config(&bin, &path).await.err()
    };
    if let Some(why) = why {
        let old = std::fs::read_to_string(&path).unwrap_or_default();
        out.backup = regenerate(&bin, &path).await?;
        out.regenerated = true;
        if !old.is_empty() {
            let fresh = std::fs::read_to_string(&path)?;
            std::fs::write(&path, carry_over(&old, &fresh))?;
        }
        match &out.backup {
            Some(b) => st.db().log("warning", &format!("streamrip: config regenerated ({why}); the old file is kept as {}", b.display())),
            None => st.db().log("info", &format!("streamrip: config written at {} ({why})", path.display())),
        }
        out.problem = validate_config(&bin, &path).await.err();
    }
    apply_credentials(st)?;
    Ok(out)
}

/// The config file, made to load when it can be (see `check_config`); its path.
pub async fn ensure_config(st: &impl Ctx) -> anyhow::Result<PathBuf> {
    check_config(st, false).await.map(|c| c.path)
}

/// "Reset streamrip config": a fresh file whatever the state of the old one, sign-ins carried over.
pub async fn reset_config(st: &impl Ctx) -> anyhow::Result<ConfigCheck> {
    check_config(st, true).await
}

/// Diagnostics: does streamrip load its config? Read-only – nothing is written or regenerated here.
pub async fn config_loads_check(st: &impl Ctx) -> Value {
    let path = config_path(st);
    let bin = binary(st).unwrap_or("streamrip".into());
    let (ok, detail) = match binary(st) {
        None => (false, NOT_INSTALLED_MSG.to_string()),
        Some(_) if !path.is_file() => (false, format!("No config file at {} yet – it is written on the first download, or press Reset streamrip config in Settings → streamrip", path.display())),
        Some(bin) => match validate_config(&bin, &path).await {
            Ok(()) => (true, format!("OK – {}", path.display())),
            Err(e) => (false, format!("{e} – press Reset streamrip config in Settings → streamrip")),
        },
    };
    serde_json::json!({ "name": "streamrip config loads", "kind": "tool", "request": format!("{bin} --config-path {} config path", path.display()), "ok": ok, "detail": detail })
}

/// The useful part of streamrip's output, one line: box drawing stripped, numbered progress lines
/// dropped, the last three lines that name an error (each with the line that continues it), capped.
fn summary(text: &str, max: usize) -> String {
    let boxes = Regex::new(r"[│╭╰╮╯─━┃]+").unwrap();
    let num = Regex::new(r"^\d+\s").unwrap();
    let errs = Regex::new(r"(?i)error|invalid|fail|not (found|available)|login|arl|token|subscription|traceback|exception").unwrap();
    let lines: Vec<String> = text.lines().map(|l| boxes.replace_all(l, " ").split_whitespace().collect::<Vec<_>>().join(" ")).filter(|l| !l.is_empty() && !num.is_match(l)).collect();
    let mut groups: Vec<String> = vec![];
    let mut i = 0;
    while i < lines.len() {
        if errs.is_match(&lines[i]) {
            let mut g = lines[i].clone();
            if i + 1 < lines.len() && !errs.is_match(&lines[i + 1]) {
                g.push(' ');
                g.push_str(&lines[i + 1]);
                i += 1;
            }
            groups.push(g);
        }
        i += 1;
    }
    let pick: Vec<String> = if groups.is_empty() { lines.iter().rev().take(3).rev().cloned().collect() } else { groups.into_iter().rev().take(3).rev().collect() };
    cap(&pick.join(" / "), max)
}

fn cap(s: &str, max: usize) -> String {
    if s.chars().count() <= max { s.to_string() } else { s.chars().take(max.saturating_sub(1)).collect::<String>() + "…" }
}

/// streamrip stopped to ask for the Deezer ARL (a prompt that reads EOF on a server).
fn asks_for_arl(text: &str) -> bool {
    let arl = Regex::new(r"(?i)\barl\b|Finding-Your-Deezer-ARL").unwrap();
    let prompt = Regex::new(r"(?i)enter your|paste|copy the arl|finding-your-deezer-arl|prompt|cookie|eoferror|eof when reading").unwrap();
    arl.is_match(text) && prompt.is_match(text)
}

/// The one-line reason a run produced nothing: the Deezer prompt and a config that does not load get
/// plain words; anything else the error lines of the output, first 300 characters.
pub fn failure_message(text: &str) -> String {
    if asks_for_arl(text) {
        return ARL_MSG.to_string();
    }
    if text.contains(CONFIG_ERROR) {
        return format!("{CONFIG_BROKEN_MSG} – {}", summary(text, MESSAGE_CAP));
    }
    format!("streamrip produced no files: {}", summary(text, MESSAGE_CAP))
}

/// A run that produced no files: the message for the request, the whole output for its log.
#[derive(Debug)]
pub struct RipError {
    pub message: String,
    pub log: String,
}

impl std::fmt::Display for RipError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for RipError {}

/// The output of a failed run, when the error came from one (for the request's log).
pub fn run_log(e: &anyhow::Error) -> Option<&str> {
    e.downcast_ref::<RipError>().map(|r| r.log.as_str()).filter(|l| !l.is_empty())
}

// ---- resolving a request to a service link ------------------------------------------------------

pub async fn deezer_isrc(st: &impl Ctx, isrc: &str) -> Option<Value> {
    let d: Value = encore_http::deezer::track_by_isrc(st, isrc).send().await.ok()?.json().await.ok()?;
    d["error"].is_null().then_some(d)
}

async fn deezer_search(st: &impl Ctx, kind: &str, q: &str) -> Vec<Value> {
    let Ok(r) = encore_http::deezer::search(st, kind, q, 15).send().await else { return vec![] };
    r.json::<Value>().await.ok().and_then(|d| d["data"].as_array().cloned()).unwrap_or_default()
}

/// Tidal developer-API token (client credentials), cached until it expires.
pub async fn tidal_token(st: &impl Ctx) -> anyhow::Result<String> {
    if let Some(v) = st.cache_get("tidal_token") {
        return Ok(v.as_str().unwrap_or("").to_string());
    }
    let (cid, sec) = (st.cfg().str("tidal_client_id"), st.cfg().str("tidal_client_secret"));
    if cid.is_empty() || sec.is_empty() {
        anyhow::bail!("Tidal needs a Client ID/Secret from developer.tidal.com (Settings → Integrations)");
    }
    let r = st.http().post("https://auth.tidal.com/v1/oauth2/token").basic_auth(cid, Some(sec)).form(&[("grant_type", "client_credentials")]).send().await?;
    if !r.status().is_success() {
        anyhow::bail!("Tidal auth failed: {}", r.text().await.unwrap_or_default().chars().take(200).collect::<String>());
    }
    let d: Value = r.json().await?;
    let tok = d["access_token"].as_str().unwrap_or("").to_string();
    st.cache_put("tidal_token", serde_json::json!(tok), (d["expires_in"].as_u64().unwrap_or(3600)).saturating_sub(60));
    Ok(tok)
}

async fn tidal_by_isrc(st: &impl Ctx, isrc: &str, albums: bool) -> anyhow::Result<Option<String>> {
    let tok = tidal_token(st).await?;
    let mut q = vec![("countryCode", "US"), ("filter[isrc]", isrc)];
    if albums {
        q.push(("include", "albums"));
    }
    let d: Value = encore_http::tidal::get(st, &tok, "/tracks").query(&q).send().await?.json().await?;
    Ok(if albums {
        d["included"].as_array().and_then(|i| i.iter().find(|x| x["type"] == "albums")).and_then(|x| x["id"].as_str()).map(|id| format!("https://tidal.com/browse/album/{id}"))
    } else {
        d["data"].as_array().and_then(|a| a.first()).and_then(|x| x["id"].as_str()).map(|id| format!("https://tidal.com/browse/track/{id}"))
    })
}

pub const SERVICES: [&str; 3] = ["qobuz", "tidal", "deezer"];

/// The streamrip services to search, in the order they appear in the one Source order setting
/// (e.g. "tidal, deezer, slskd, ytdlp"). Older setups that just say "streamrip" keep their old list.
pub fn services(st: &impl Ctx) -> Vec<String> {
    let order: Vec<String> = st.cfg().str("source_order").split(',').map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()).collect();
    let picked: Vec<String> = order.iter().filter(|s| SERVICES.contains(&s.as_str())).cloned().collect();
    if !picked.is_empty() {
        return picked;
    }
    let legacy: Vec<String> = st.cfg().str("streamrip_sources").split(',').map(|s| s.trim().to_lowercase()).filter(|s| SERVICES.contains(&s.as_str())).collect();
    if legacy.is_empty() { vec!["tidal".into(), "deezer".into()] } else { legacy }
}

pub async fn resolve_album(st: &impl Ctx, artist: &str, album: &str, track_count: Option<usize>, first_isrc: Option<&str>) -> Vec<String> {
    let mut urls = vec![];
    let want = describe(album, artist, None, None, None);
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
                let v = compare(&want, &describe(a["title"].as_str().unwrap_or(""), a["artist"]["name"].as_str().unwrap_or(""), None, None, None), true);
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
        } else if svc == "tidal" && first_isrc.is_some() && !st.cfg().str("tidal_client_id").is_empty() {
            match tidal_by_isrc(st, first_isrc.unwrap(), true).await {
                Ok(Some(u)) => urls.push(u),
                Ok(None) => {}
                Err(e) => st.db().log("warning", &format!("streamrip: tidal lookup failed: {e}")),
            }
        }
    }
    urls
}

pub async fn resolve_track(st: &impl Ctx, artist: &str, title: &str, isrc: Option<&str>, length: Option<f64>) -> Vec<String> {
    let mut urls = vec![];
    let want = describe(title, artist, length, isrc, None);
    for svc in services(st) {
        if svc == "deezer" {
            let mut t = match isrc { Some(i) => deezer_isrc(st, i).await, None => None };
            if t.is_none() {
                for c in deezer_search(st, "track", &format!("artist:\"{artist}\" track:\"{title}\"")).await {
                    let mut ct = c["title"].as_str().unwrap_or("").to_string();
                    if let Some(v) = c["title_version"].as_str().filter(|v| !v.is_empty() && !ct.to_lowercase().contains(&v.to_lowercase())) {
                        ct = format!("{ct} {v}");
                    }
                    if compare(&want, &describe(&ct, c["artist"]["name"].as_str().unwrap_or(""), c["duration"].as_f64(), None, None), true).matched {
                        t = Some(c);
                        break;
                    }
                }
            }
            if let Some(id) = t.and_then(|t| t["id"].as_i64()) {
                urls.push(format!("https://www.deezer.com/track/{id}"));
            }
        } else if svc == "tidal" && isrc.is_some() && !st.cfg().str("tidal_client_id").is_empty() {
            match tidal_by_isrc(st, isrc.unwrap(), false).await {
                Ok(Some(u)) => urls.push(u),
                Ok(None) => {}
                Err(e) => st.db().log("warning", &format!("streamrip: tidal lookup failed: {e}")),
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

pub async fn download(st: &impl Ctx, url: &str, folder: &Path) -> anyhow::Result<Vec<PathBuf>> {
    std::fs::create_dir_all(folder)?;
    let Some(bin) = binary(st) else { anyhow::bail!("{NOT_INSTALLED_MSG}") };
    // the config is checked (and mended) and the credentials written before every run, not only the first
    let check = check_config(st, false).await?;
    if let Some(p) = check.problem {
        anyhow::bail!("{CONFIG_BROKEN_MSG} – {p}");
    }
    if url.contains("deezer.com") && toml_value(&std::fs::read_to_string(&check.path).unwrap_or_default(), "deezer", "arl").is_empty() {
        anyhow::bail!("{ARL_MSG}");
    }
    let tpl = st.cfg().str("streamrip_command");
    let tpl = if tpl.is_empty() { DEFAULT_CMD.to_string() } else { tpl };
    let cmd = tpl.replace("{bin}", &shq(&bin))
        .replace("{config}", &shq(&check.path.to_string_lossy())).replace("{folder}", &shq(&folder.to_string_lossy()))
        .replace("{quality}", &st.cfg().int("streamrip_quality").to_string()).replace("{url}", &shq(url));
    // stdin closed: a prompt (the Deezer ARL, say) reads EOF at once instead of waiting forever
    let fut = tokio::process::Command::new("sh").args(["-c", &cmd]).stdin(std::process::Stdio::null())
        .env("NO_COLOR", "1").env("TERM", "dumb").env("PYTHONUNBUFFERED", "1").kill_on_drop(true).output();
    let out = tokio::time::timeout(std::time::Duration::from_secs(3600), fut).await.map_err(|_| anyhow::anyhow!("streamrip timed out"))??;
    let files = audio_files(folder, &AUDIO);
    if files.is_empty() {
        let text = String::from_utf8_lossy(&[out.stdout, out.stderr].concat()).to_string();
        return Err(RipError { message: failure_message(&text), log: cap(text.trim(), LOG_CAP) }.into());
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

/// The Tidal sign-in in progress (Settings → streamrip → Sign in to Tidal), kept in the server's state.
/// Clones share it.
#[derive(Clone, Default)]
pub struct TidalLogins(std::sync::Arc<std::sync::Mutex<Option<TidalLogin>>>);

fn tidal_state(st: &impl Ctx) -> TidalLogin {
    st.requests().tidal.0.lock().map(|g| g.clone().unwrap_or_default()).unwrap_or_default()
}

fn tidal_set(st: &impl Ctx, f: impl FnOnce(&mut TidalLogin)) {
    if let Ok(mut g) = st.requests().tidal.0.lock() {
        let mut s = g.take().unwrap_or_default();
        f(&mut s);
        *g = Some(s);
    }
}

/// The `[tidal]` section of a TOML text (without the header), or "" when there is none.
fn tidal_section(text: &str) -> &str {
    section_body(text, "tidal")
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
fn tidal_config(st: &impl Ctx) -> std::collections::BTreeMap<String, String> {
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

/// Write `[tidal]` values into streamrip's config (made sound through ensure_config first).
async fn tidal_write(st: &impl Ctx, values: &[(String, String)]) -> anyhow::Result<()> {
    let path = ensure_config(st).await?;
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
pub fn tidal_signed_in(st: &impl Ctx) -> bool {
    let cfg = tidal_config(st);
    let has = |k: &str| cfg.get(k).map(|v| v.len() > 10).unwrap_or(false);
    has("access_token") && (!tidal_hires_client(&cfg) || has("hires_access_token"))
}

/// Signed in on the default client only: hi-res is wanted but step 2 was never done.
fn tidal_partial(st: &impl Ctx) -> bool {
    let cfg = tidal_config(st);
    let has = |k: &str| cfg.get(k).map(|v| v.len() > 10).unwrap_or(false);
    has("access_token") && tidal_hires_client(&cfg) && !has("hires_access_token")
}

/// Current sign-in state for the settings page.
pub fn tidal_login_status(st: &impl Ctx) -> Value {
    let s = tidal_state(st);
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
async fn tidal_auth_post(st: &impl Ctx, path: &str, form: &[(&str, &str)], basic: Option<(&str, &str)>) -> Result<(u16, Value), String> {
    let mut req = st.http().post(format!("{TIDAL_AUTH}/{path}"))
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
async fn tidal_lane_flow(st: &impl Ctx, cid: &str, sec: &str, step: u8, steps: u8) -> Result<Value, String> {
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
    tidal_set(st, |s| { s.status = "waiting".into(); s.url = Some(url.clone()); s.message = message.into(); s.step = step; s.steps = steps; });
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
pub async fn tidal_login_start(st: &impl Ctx) -> anyhow::Result<Value> {
    ensure_config(st).await?;
    let cfg = tidal_config(st);
    let lanes = tidal_lanes(&cfg);
    let steps = lanes.len() as u8;
    // the old tokens stay until lane 1 is approved: a refused link (Tidal blocks the address) must not
    // throw away a working, pasted-in sign-in
    tidal_set(st, |s| { *s = TidalLogin { status: "starting".into(), url: None, message: "Asking Tidal for a sign-in link…".into(), step: 1, steps }; });
    let st2 = st.clone();
    tokio::spawn(async move {
        let mut values: Vec<(String, String)> = vec![];
        for (i, (cid, sec, prefix)) in lanes.iter().enumerate() {
            let step = i as u8 + 1;
            match tidal_lane_flow(&st2, cid, sec, step, steps).await {
                Ok(t) => {
                    let expiry = t["expires_in"].as_f64().unwrap_or(604800.0) + encore_core::time::now();
                    if i == 0 {
                        values.push(("user_id".into(), tstr(t["user"]["userId"].to_string().trim_matches('"'))));
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
                        tidal_set(&st2, |s| { s.status = "failed".into(); s.url = None; s.message = format!("Could not write streamrip's config: {e}"); });
                        return;
                    }
                    values.clear();
                }
                Err(e) => {
                    tidal_set(&st2, |s| { s.status = "failed".into(); s.url = None; s.message = e; });
                    return;
                }
            }
        }
        let hires = lanes.iter().any(|l| l.2 == "hires_");
        if let Err(e) = tidal_write(&st2, &[("hires_client".into(), if hires { "true".into() } else { "false".into() })]).await {
            tidal_set(&st2, |s| { s.status = "failed".into(); s.url = None; s.message = format!("Could not write streamrip's config: {e}"); });
            return;
        }
        st2.db().log("info", &format!("streamrip: signed in to Tidal ({} client{})", lanes.len(), if lanes.len() == 1 { "" } else { "s" }));
        tidal_set(&st2, |s| { s.status = "done".into(); s.url = None; s.message = TIDAL_DONE_MSG.into(); s.step = steps; });
    });
    // wait for the link
    for _ in 0..60 {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let s = tidal_state(st);
        if s.url.is_some() || s.status == "failed" || s.status == "done" {
            return Ok(tidal_login_status(st));
        }
    }
    tidal_set(st, |s| { if s.status == "starting" { s.status = "failed".into(); s.message = "Tidal did not answer within 30 seconds. Try again.".into(); } });
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
pub async fn tidal_tokens_save(st: &impl Ctx, toml: &str) -> anyhow::Result<Value> {
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
    tidal_set(st, |s| { *s = TidalLogin { status: "done".into(), url: None, message: "Tokens saved – downloads from Tidal use this account now.".into(), step: 0, steps: 0 }; });
    st.db().log("info", "streamrip: Tidal tokens pasted in from another computer");
    Ok(tidal_login_status(st))
}

/// "Sign out of Tidal": blank every token field.
pub async fn tidal_sign_out(st: &impl Ctx) -> anyhow::Result<Value> {
    let values: Vec<(String, String)> = TIDAL_TOKEN_KEYS.iter().map(|k| (k.to_string(), "\"\"".to_string())).collect();
    tidal_write(st, &values).await?;
    tidal_set(st, |s| { *s = TidalLogin::default(); });
    Ok(tidal_login_status(st))
}

/// Diagnostics: can this server talk to auth.tidal.com at all? A JSON answer (even an error) means yes.
pub async fn tidal_probe(st: &impl Ctx) -> String {
    match tidal_auth_post(st, "device_authorization", &[("client_id", "encore-probe"), ("scope", TIDAL_SCOPE)], None).await {
        Ok(_) => "Tidal reachable".into(),
        Err(e) if e.starts_with("Could not reach") => e,
        Err(_) => "Tidal blocks this server's address – see Sign in to Tidal".into(),
    }
}

// ---- the picker's catalogue searches: the accounts streamrip signs in with --------------------------

/// The Deezer ARL is set, in Encore's settings or by hand in streamrip's config.toml.
pub fn deezer_arl_set(st: &impl Ctx) -> bool {
    !st.cfg().str("streamrip_deezer_arl").is_empty() || !toml_value(&std::fs::read_to_string(config_path(st)).unwrap_or_default(), "deezer", "arl").is_empty()
}

/// The Qobuz login streamrip has, as (user id or email, token or md5 password, is a token): Encore's settings
/// first, else config.toml under either naming (`user_id`/`auth_token`, or `email_or_userid`/`password_or_token`
/// with `use_auth_token`). None when there is no login.
pub fn qobuz_login(st: &impl Ctx) -> Option<(String, String, bool)> {
    let (u, t) = (st.cfg().str("streamrip_qobuz_user"), st.cfg().str("streamrip_qobuz_token"));
    if !u.is_empty() && !t.is_empty() {
        return Some((u, t, true));
    }
    let text = std::fs::read_to_string(config_path(st)).unwrap_or_default();
    let v = |k: &str| toml_value(&text, "qobuz", k);
    let (user, token) = (v("user_id"), v("auth_token"));
    if !user.is_empty() && !token.is_empty() {
        return Some((user, token, true));
    }
    let (user, secret) = (v("email_or_userid"), v("password_or_token"));
    if user.is_empty() || secret.is_empty() {
        return None;
    }
    Some((user, secret, v("use_auth_token").trim() != "false"))
}

pub const QOBUZ_APP_ID_MSG: &str = "Qobuz's web player has changed and its app id can't be read – run one Qobuz download so streamrip stores the id in config.toml";

/// The app id Qobuz's API wants in `X-App-Id`: the one streamrip stored in config.toml after its first run,
/// else read from the web player's bundle the way streamrip's own client does (kept for a day).
pub async fn qobuz_app_id(st: &impl Ctx) -> anyhow::Result<String> {
    let stored = toml_value(&std::fs::read_to_string(config_path(st)).unwrap_or_default(), "qobuz", "app_id");
    if !stored.is_empty() {
        return Ok(stored);
    }
    if let Some(v) = st.cache_get("qobuz_app_id") {
        return Ok(v.as_str().unwrap_or("").to_string());
    }
    let player = st.urls().qobuz_player.clone();
    let login = st.http().get(format!("{player}/login")).send().await?.text().await?;
    let bundle = Regex::new(r#"<script src="(/resources/\d+\.\d+\.\d+-[a-z]\d{3}/bundle\.js)"></script>"#).unwrap().captures(&login)
        .ok_or_else(|| anyhow::anyhow!("{QOBUZ_APP_ID_MSG}"))?[1].to_string();
    let js = st.http().get(format!("{player}{bundle}")).send().await?.text().await?;
    let id = Regex::new(r#"production:\{api:\{appId:"(\d{9})",appSecret:"\w{32}"#).unwrap().captures(&js).ok_or_else(|| anyhow::anyhow!("{QOBUZ_APP_ID_MSG}"))?[1].to_string();
    st.cache_put("qobuz_app_id", Value::String(id.clone()), 24 * 3600);
    Ok(id)
}

/// The `X-User-Auth-Token` for Qobuz's API: the stored token, or the token Qobuz hands out for an
/// email + md5 password login (kept for twelve hours). None without a login.
pub async fn qobuz_user_token(st: &impl Ctx, app_id: &str) -> anyhow::Result<Option<String>> {
    let Some((user, secret, is_token)) = qobuz_login(st) else { return Ok(None) };
    if is_token {
        return Ok(Some(secret));
    }
    if let Some(v) = st.cache_get("qobuz_user_token") {
        return Ok(v.as_str().map(String::from));
    }
    // the login goes in the query string (streamrip's way): a failed call must not echo it (reqwest's
    // message would carry the whole URL, e-mail and password hash included)
    let r = st.http().get(format!("{}/user/login", st.urls().qobuz_api)).query(&[("email", user.as_str()), ("password", secret.as_str()), ("app_id", app_id)]).header("X-App-Id", app_id).send().await
        .map_err(|e| anyhow::anyhow!("Qobuz login: {}", http_error_without_url(e)))?;
    let code = r.status().as_u16();
    let d: Value = r.json().await.unwrap_or(Value::Null);
    let tok = d["user_auth_token"].as_str().filter(|t| !t.is_empty()).map(String::from);
    match (code, tok) {
        (200, Some(t)) => {
            st.cache_put("qobuz_user_token", Value::String(t.clone()), 12 * 3600);
            Ok(Some(t))
        }
        (401, _) => anyhow::bail!("Qobuz refused the login – check the email and password in streamrip's config.toml"),
        _ => anyhow::bail!("Qobuz login failed (HTTP {code}): {}", d["message"].as_str().unwrap_or("no message")),
    }
}

/// Qobuz's catalogue search (`kind` album or track) with the app id and login streamrip uses: the answer as
/// Qobuz gives it, an error in words when Qobuz turns the call down.
pub async fn qobuz_search(st: &impl Ctx, kind: &str, q: &str) -> anyhow::Result<Value> {
    let app_id = qobuz_app_id(st).await?;
    let token = qobuz_user_token(st, &app_id).await?;
    let mut req = st.http().get(format!("{}/{kind}/search", st.urls().qobuz_api)).query(&[("query", q), ("limit", "25"), ("offset", "0")]).header("X-App-Id", &app_id);
    if let Some(t) = &token {
        req = req.header("X-User-Auth-Token", t);
    }
    let r = req.send().await?;
    let code = r.status().as_u16();
    if code >= 400 {
        let d: Value = r.json().await.unwrap_or(Value::Null);
        let msg = d["message"].as_str().unwrap_or("no message");
        anyhow::bail!(match code {
            401 => "Qobuz rejected the login – check the user id and token in Settings → streamrip".to_string(),
            400 => format!("Qobuz rejected the app id ({msg}) – run one Qobuz download so streamrip stores a fresh one"),
            _ => format!("Qobuz API {code}: {msg}"),
        });
    }
    Ok(r.json().await?)
}

/// A request error as words, without the URL reqwest appends to its message, but with its cause
/// ("connection refused", "dns error …").
fn http_error_without_url(e: reqwest::Error) -> String {
    let e = e.without_url();
    let mut s = e.to_string();
    let mut cause = std::error::Error::source(&e);
    while let Some(c) = cause {
        s.push_str(": ");
        s.push_str(&c.to_string());
        cause = std::error::Error::source(c);
    }
    s
}

pub const TIDAL_LAPSED_MSG: &str = "Tidal sign-in has lapsed – sign in again in Settings → streamrip";

/// The signed-in Tidal account's token for Encore's own API calls (the picker's Tidal search), as
/// (access_token, country_code). A lapsed token is refreshed through the OAuth client it came from and
/// written back, so streamrip's next run finds it fresh. None when not signed in.
pub async fn tidal_user_token(st: &impl Ctx) -> anyhow::Result<Option<(String, String)>> {
    let cfg = tidal_config(st);
    let get = |k: &str| cfg.get(k).cloned().unwrap_or_default();
    let token = get("access_token");
    if token.len() <= 10 {
        return Ok(None);
    }
    let cc = get("country_code");
    let cc = if cc.is_empty() { "US".to_string() } else { cc };
    let expiry = get("token_expiry").trim().parse::<f64>().unwrap_or(0.0);
    if expiry <= 0.0 || expiry > encore_core::time::now() + 60.0 {
        return Ok(Some((token, cc)));
    }
    let refresh = get("refresh_token");
    if refresh.is_empty() {
        anyhow::bail!("{TIDAL_LAPSED_MSG}");
    }
    let cid = { let c = get("token_client_id"); if c.is_empty() { TIDAL_DEFAULT_CLIENT.0.to_string() } else { c } };
    let secret = if cid == TIDAL_DEFAULT_CLIENT.0 { TIDAL_DEFAULT_CLIENT.1.to_string() }
        else if cid == TIDAL_HIRES_CLIENT.0 { TIDAL_HIRES_CLIENT.1.to_string() }
        else if cid == get("client_id") { get("client_secret") } else { String::new() };
    let form = [("client_id", cid.as_str()), ("refresh_token", refresh.as_str()), ("grant_type", "refresh_token"), ("scope", TIDAL_SCOPE)];
    let (code, d) = tidal_auth_post(st, "token", &form, Some((cid.as_str(), secret.as_str()))).await.map_err(|e| anyhow::anyhow!("{e}"))?;
    let fresh = d["access_token"].as_str().unwrap_or("").to_string();
    if code != 200 || fresh.len() <= 10 {
        anyhow::bail!("{TIDAL_LAPSED_MSG}");
    }
    let mut values = vec![("access_token".to_string(), tstr(&fresh)), ("token_expiry".to_string(), toml_float(encore_core::time::now() + d["expires_in"].as_f64().unwrap_or(3600.0)))];
    if let Some(r) = d["refresh_token"].as_str().filter(|r| !r.is_empty()) {
        values.push(("refresh_token".to_string(), tstr(r)));
    }
    tidal_write(st, &values).await?;
    Ok(Some((fresh, cc)))
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

    #[test]
    fn failure_messages_read_plainly() {
        // the owner's case: the complaint continues on the next line, which the old picker lost
        let owner = "Error loading config from /config/streamrip/config.toml: \nQobuzConfig.__init__() got an unexpected keyword argument 'user_id'\nTry running rip config reset\n";
        let m = failure_message(owner);
        assert!(m.starts_with("streamrip can't load its config – press Reset streamrip config in Settings → streamrip – "), "{m}");
        assert!(m.contains("unexpected keyword argument 'user_id'"), "{m}");
        assert_eq!(ARL_MSG, "Deezer needs an ARL cookie – Settings → streamrip");
        assert_eq!(failure_message("Copy the ARL cookie from your browser and paste it here\nEnter your ARL:"), ARL_MSG);
        assert_eq!(failure_message("Finding-Your-Deezer-ARL-Cookie\nEOFError: EOF when reading a line"), ARL_MSG);
        // an unrelated failure keeps streamrip's own words, boxes and progress lines stripped
        let other = "╭──────╮\n│ Searching │\n╰──────╯\n12 downloading\nNo results found for the query\n";
        assert_eq!(failure_message(other), "streamrip produced no files: Searching / No results found for the query");
        // 300 characters at most
        let long = format!("Error: {}", "x".repeat(400));
        let m = failure_message(&long);
        assert_eq!(m.chars().count(), "streamrip produced no files: ".len() + 300, "{m}");
        assert!(m.ends_with('…'));
        // "early" is not the ARL
        assert!(failure_message("Login failed early: token expired").starts_with("streamrip produced no files: Login failed early"));
        assert!(!asks_for_arl("an early prompt"));
        assert_eq!(toml_value("[deezer]\nquality = 2\narl = \"abc\" # mine\n[tidal]\narl = \"no\"", "deezer", "arl"), "abc");
        assert_eq!(toml_value("[tidal]\nx = 1", "deezer", "arl"), "");
        assert!(qobuz_v2("[qobuz]\nemail_or_userid = \"\"\nauth_token = \"x\""));
        assert!(!qobuz_v2("[qobuz]\nuser_id = \"\"\nauth_token = \"\""));
        assert!(qobuz_v2(""));
    }
}
