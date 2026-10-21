//! yt-dlp (subprocess): YouTube, SoundCloud, Bandcamp, Mixcloud and ~1800 other sites.
use crate::Ctx;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use encore_core::audio::YTDLP_EXTS as AUDIO;

/// YouTube Music is a source now: the yt-dlp plugin is installed and switched on, and yt-dlp is on in its
/// settings.
pub fn enabled(st: &impl Ctx) -> bool {
    st.plugins().enabled("ytdlp") && st.cfg().bool("ytdlp_enabled")
}

/// The yt-dlp plugin is installed and switched on (a link or the last-resort step may use it).
pub fn plugin_on(st: &impl Ctx) -> bool {
    st.plugins().enabled("ytdlp")
}

pub fn bin(st: &impl Ctx) -> String {
    // the owner's own copy (ENCORE_YTDLP), else the newest that runs of the plugin's venv, the venv an Encore
    // before plugins kept (adopted), an older image's /opt/tools/bin and PATH (encore_system::tools)
    std::env::var("ENCORE_YTDLP").ok().filter(|v| !v.trim().is_empty()).unwrap_or_else(|| {
        encore_system::tools::path(st, "ytdlp").map(|p| p.to_string_lossy().into()).unwrap_or_else(|| "yt-dlp".into())
    })
}

/// yt-dlp's environment: Encore's own (its proxy, its locale) without Encore's settings and without forced
/// colours (encore_plugins::sandbox::program_env).
fn env() -> Vec<(String, String)> {
    encore_plugins::sandbox::program_env()
}

/// The command for a yt-dlp run, through the plugins' spawn builder (a session of its own without a terminal,
/// no new privileges, no other open files).
fn command(program: &str, args: &[String]) -> Command {
    let spawn = encore_plugins::sandbox::Spawn::new(&encore_plugins::sandbox::Sandbox::limits_only(), Path::new(program)).args(args.iter().cloned()).env(env());
    spawn.tokio_command()
}

/// A yt-dlp to run is there: the owner's own (ENCORE_YTDLP), or a copy of the plugin's, the adopted venv's,
/// an older image's or the system's that runs.
pub fn program_found(st: &impl Ctx) -> bool {
    std::env::var("ENCORE_YTDLP").ok().is_some_and(|v| !v.trim().is_empty()) || encore_system::tools::path(st, "ytdlp").is_some()
}

pub async fn version(st: &impl Ctx) -> Option<String> {
    let (st, exe) = (st.clone(), bin(st));
    let path = encore_core::which::which(&exe)?;
    tokio::task::spawn_blocking(move || st.system().tools.version_of(&path)).await.ok().flatten()
}

/// `--cookies <file>` from the YouTube sign-in in use (the hand-off session, the extension or pasted
/// cookies: see ytauth). yt-dlp writes the jar back when it exits, so it gets a throw-away copy.
fn cookie_args(st: &impl Ctx, dir: &Path) -> Vec<String> {
    let Some(c) = crate::ytauth::cookies_for_ytdlp(st).filter(|c| !c.trim().is_empty()) else { return vec![] };
    let p = dir.join(".cookies.txt");
    if crate::ytauth::write_private(&p, &c).is_ok() {
        vec!["--cookies".into(), p.to_string_lossy().into()]
    } else {
        vec![]
    }
}

/// Settings → test: do the saved cookies still sign in to YouTube Music?
pub async fn test_cookies(st: &impl Ctx) -> anyhow::Result<Value> {
    let ver = version(st).await;
    let tool = match &ver {
        Some(v) => format!("yt-dlp {v}"),
        None => "yt-dlp is not installed – install or update the yt-dlp plugin under Settings → Plugins".into(),
    };
    let source = crate::ytauth::active(st).map(|k| format!(" ({})", k.label().to_lowercase())).unwrap_or_default();
    let raw = crate::ytauth::cookies_for_ytdlp(st).unwrap_or_default();
    if raw.trim().is_empty() {
        return Ok(json!({ "ok": ver.is_some(), "message": format!("{tool} · no cookies saved – downloads still work, at 128 kbps without Premium") }));
    }
    let now = encore_db::now() as i64;
    let (mut pairs, mut expired, mut total) = (vec![], 0, 0);
    for line in raw.lines() {
        let line = line.strip_prefix("#HttpOnly_").unwrap_or(line);
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() < 7 || !f[0].contains("youtube.com") {
            continue;
        }
        total += 1;
        let exp: i64 = f[4].trim().parse().unwrap_or(0);
        if exp != 0 && exp < now {
            expired += 1;
            continue;
        }
        pairs.push(format!("{}={}", f[5], f[6].trim_end()));
    }
    if total == 0 {
        return Ok(json!({ "ok": false, "message": "No youtube.com cookies found – paste the whole Netscape cookies.txt exported from music.youtube.com" }));
    }
    if pairs.is_empty() {
        return Ok(json!({ "ok": false, "message": format!("All {total} youtube.com cookies have expired – export them again") }));
    }
    let html = st.http().get("https://music.youtube.com/").header("Cookie", pairs.join("; ")).header("Accept-Language", "en")
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0 Safari/537.36")
        .send().await?.text().await?;
    let logged_in = html.contains("\"LOGGED_IN\":true");
    let note = if expired > 0 { format!(" ({expired} expired cookies ignored)") } else { String::new() };
    Ok(json!({ "ok": logged_in && ver.is_some(), "message": if logged_in {
        format!("{tool} · cookies{source} work – signed in to YouTube Music{note}")
    } else {
        format!("{tool} · YouTube did not accept these cookies{source} (signed out or rotated){note}. Sign in again under YouTube sign-in, or export them again from a private window and close it right after.")
    } }))
}

pub struct Download {
    pub files: Vec<PathBuf>,
    pub entries: Vec<Value>,
}

/// Download a link (one video or a whole playlist) into `outdir`. `progress` gets 0–100.
/// The staging folder's steps say which folder failed ([`encore_core::fsx`]): a read-only or failing disk
/// is a storage problem the request waits out, not a failure of YouTube.
pub async fn download(st: &impl Ctx, url: &str, outdir: &Path, playlist: bool, progress: Option<&(dyn Fn(f64) + Send + Sync)>) -> anyhow::Result<Download> {
    use encore_core::fsx::{self, Step};
    fsx::mkdir_all(outdir, Step::Stage)?;
    let before: std::collections::HashSet<_> = fsx::read_dir(outdir, Step::Stage)?.flatten().map(|e| e.path()).collect();
    let fmt = st.cfg().str("ytdlp_format");
    let fmt = if fmt.is_empty() { "best".to_string() } else { fmt };
    let tmpl = outdir.join("%(playlist_index|)s%(playlist_index& - |)s%(artist,uploader|Unknown)s - %(track,title)s.%(ext)s");
    let mut args: Vec<String> = vec![
        "-f".into(), "bestaudio/best".into(), "-x".into(), "--audio-format".into(), fmt.clone(),
        "-o".into(), tmpl.to_string_lossy().into(), "--windows-filenames".into(), "--embed-metadata".into(),
        "--retries".into(), "5".into(), "--fragment-retries".into(), "5".into(), "--no-warnings".into(),
        "--newline".into(), "--progress-template".into(), "download:PROG %(progress.downloaded_bytes)s %(progress.total_bytes)s %(progress.total_bytes_estimate)s".into(),
        "--print".into(), "after_move:JSON %(.{title,track,artist,creator,uploader,album,duration,webpage_url,thumbnail,filepath})j".into(),
        if playlist { "--yes-playlist".into() } else { "--no-playlist".into() },
    ];
    if playlist {
        args.push("--ignore-errors".into());
    }
    if fmt == "mp3" || fmt == "m4a" {
        let q = st.cfg().str("ytdlp_quality");
        args.extend(["--audio-quality".into(), if q.is_empty() { "0".into() } else { q }]);
    }
    args.extend(js_runtime_args(st).await);
    // YouTube sometimes hides every audio format from one kind of client ("Requested format is not
    // available"): try again as other clients, without cookies, and on the other YouTube host
    let alt_host = if url.contains("music.youtube.com") { Some(url.replace("music.youtube.com", "www.youtube.com")) }
        else if url.contains("www.youtube.com/watch") || url.contains("://youtube.com/watch") { Some(url.replace("://youtube.com", "://www.youtube.com").replace("www.youtube.com", "music.youtube.com")) }
        else { None };
    let attempts: Vec<(String, Vec<String>, bool)> = [
        Some((url.to_string(), vec![], true)),
        Some((url.to_string(), vec!["--extractor-args".into(), "youtube:player_client=default,tv,web_music,mweb".into()], false)),
        alt_host.map(|u| (u, vec!["--extractor-args".into(), "youtube:player_client=default,tv,web_music,mweb".into()], false)),
    ].into_iter().flatten().collect();
    let mut last_err = String::new();
    let mut refreshed = false;
    let mut i = 0;
    while i < attempts.len() {
        let (u, extra, cookies) = &attempts[i];
        let mut a = args.clone();
        a.extend(extra.iter().cloned());
        if *cookies {
            a.extend(cookie_args(st, outdir));
        }
        a.push("--".into());
        a.push(u.clone());
        match run(st, &a, outdir, &before, progress).await {
            Ok(d) => return Ok(d),
            Err(e) => {
                last_err = e.to_string();
                // asked to sign in / prove it isn't a bot: Encore refreshes the server's own cookies, then the
                // same attempt runs once more
                if *cookies && !refreshed && auth_error(&last_err) {
                    refreshed = true;
                    if crate::ytauth::after_ytdlp_auth_error(st).await {
                        continue;
                    }
                }
                let retry = ["Requested format is not available", "Sign in to confirm", "Only images are available", "nsig", "n challenge", "This video is not available"]
                    .iter().any(|k| last_err.contains(k));
                if !retry || i + 1 == attempts.len() {
                    break;
                }
            }
        }
        i += 1;
    }
    anyhow::bail!("{last_err}")
}

/// yt-dlp's errors that mean the cookies are not (or no longer) signed in.
pub fn auth_error(e: &str) -> bool {
    ["Sign in to confirm", "confirm you’re not a bot", "confirm you're not a bot", "cookies are no longer valid", "This video is age-restricted", "Use --cookies", "login required"]
        .iter().any(|k| e.contains(k))
}

/// yt-dlp needs a JavaScript runtime for YouTube since late 2025. The Docker image ships Node.js.
async fn js_runtime_args(st: &impl Ctx) -> Vec<String> {
    let has_node = encore_core::which::which("node").is_some();
    let has_deno = encore_core::which::which("deno").is_some();
    let new_enough = version(st).await.is_some_and(|v| v.as_str() >= "2025.11");
    if has_node && !has_deno && new_enough { vec!["--js-runtimes".into(), "node".into()] } else { vec![] }
}

async fn run(st: &impl Ctx, args: &[String], outdir: &Path, before: &std::collections::HashSet<PathBuf>, progress: Option<&(dyn Fn(f64) + Send + Sync)>) -> anyhow::Result<Download> {
    use encore_core::fsx::{self, Step};
    let program = bin(st);
    // a program that isn't there says so; any other start failure names the program and the errno
    let mut child = match fsx::spawn(Step::Download, &program, || command(&program, args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()) {
        Ok(c) => c,
        Err(e) if e.source.kind() == std::io::ErrorKind::NotFound => anyhow::bail!("yt-dlp is not installed ({}) – install or update the yt-dlp plugin under Settings → Plugins", e.source),
        Err(e) => return Err(e.into()),
    };
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let stderr = child.stderr.take().unwrap();
    let err_task = tokio::spawn(async move {
        let mut s = String::new();
        let mut l = BufReader::new(stderr).lines();
        while let Ok(Some(x)) = l.next_line().await {
            s = x; // last error line is the useful one
        }
        s
    });
    let mut entries = vec![];
    let mut last = std::time::Instant::now() - std::time::Duration::from_secs(5);
    while let Some(line) = lines.next_line().await? {
        if let Some(p) = line.strip_prefix("PROG ") {
            let n: Vec<f64> = p.split_whitespace().map(|x| x.parse().unwrap_or(0.0)).collect();
            let total = if n.get(1).copied().unwrap_or(0.0) > 0.0 { n[1] } else { n.get(2).copied().unwrap_or(0.0) };
            if let (Some(cb), true) = (progress, total > 0.0 && last.elapsed().as_secs_f64() > 2.0) {
                last = std::time::Instant::now();
                cb((n[0] / total * 1000.0).round() / 10.0);
            }
        } else if let Some(j) = line.strip_prefix("JSON ") {
            if let Ok(e) = serde_json::from_str::<Value>(j) {
                entries.push(json!({ "title": e["track"].as_str().or(e["title"].as_str()), "artist": e["artist"].as_str().or(e["creator"].as_str()).or(e["uploader"].as_str()),
                    "album": e["album"], "duration": e["duration"], "webpage_url": e["webpage_url"], "thumbnail": e["thumbnail"] }));
            }
        }
    }
    let status = child.wait().await?;
    let err = err_task.await.unwrap_or_default();
    let _ = std::fs::remove_file(outdir.join(".cookies.txt"));
    let mut files = vec![];
    for e in fsx::read_dir(outdir, Step::Download)?.flatten() {
        let p = e.path();
        if before.contains(&p) {
            continue;
        }
        let ext = p.extension().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_default();
        if AUDIO.contains(&ext.as_str()) {
            files.push(p);
        } else if ["jpg", "jpeg", "webp", "png"].contains(&ext.as_str()) {
            let _ = std::fs::remove_file(&p);
        }
    }
    files.sort();
    if files.is_empty() {
        if status.success() {
            anyhow::bail!("yt-dlp finished without an audio file");
        }
        anyhow::bail!("{}", if err.is_empty() { "yt-dlp failed".into() } else { err.chars().take(300).collect::<String>() });
    }
    Ok(Download { files, entries })
}

async fn json_out(st: &impl Ctx, args: &[&str]) -> anyhow::Result<Value> {
    let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    let o = command(&bin(st), &args).stdin(Stdio::null()).output().await.map_err(|e| anyhow::anyhow!("yt-dlp is not installed ({e}) – install or update the yt-dlp plugin under Settings → Plugins"))?;
    if !o.status.success() {
        anyhow::bail!("{}", String::from_utf8_lossy(&o.stderr).lines().last().unwrap_or("yt-dlp failed").chars().take(300).collect::<String>());
    }
    Ok(serde_json::from_slice(&o.stdout)?)
}

pub async fn search(st: &impl Ctx, query: &str, n: usize) -> anyhow::Result<Vec<Value>> {
    let info = json_out(st, &["-J", "--flat-playlist", "--no-warnings", &format!("ytsearch{n}:{query}")]).await?;
    Ok(info["entries"].as_array().cloned().unwrap_or_default().iter().map(|e| json!({
        "url": e["url"].as_str().map(String::from).unwrap_or(format!("https://www.youtube.com/watch?v={}", e["id"].as_str().unwrap_or(""))),
        "title": e["title"], "duration": e["duration"], "channel": e["channel"].as_str().or(e["uploader"].as_str()) })).collect())
}

/// The YouTube video that best matches a track (prefers "Topic" auto-generated uploads).
pub async fn find_best(st: &impl Ctx, artist: &str, title: &str, length: Option<f64>) -> Option<String> {
    let results = search(st, &format!("{artist} - {title} audio"), 6).await.ok()?;
    let (tl, al) = (title.to_lowercase(), artist.to_lowercase());
    let mut best: Option<(f64, String)> = None;
    for r in results {
        let name = r["title"].as_str().unwrap_or("").to_lowercase();
        let ch = r["channel"].as_str().unwrap_or("");
        // another version than the one asked for (live, remix, sped up …): never picked
        if encore_library::matcher::unwanted_version(title, artist, &name).is_some() {
            continue;
        }
        let mut s = 0.0;
        if name.contains(&tl) { s += 20.0; }
        if name.contains(&al) || ch.to_lowercase().contains(&al) { s += 10.0; }
        if ch.ends_with("- Topic") { s += 15.0; }
        for bad in ["live", "cover", "karaoke", "remix", "reaction", "8d", "slowed", "sped up", "nightcore", "instrumental"] {
            if name.contains(bad) && !tl.contains(bad) { s -= 15.0; }
        }
        if let (Some(l), Some(d)) = (length, r["duration"].as_f64()) {
            let diff = (d - l).abs();
            s += if diff < 4.0 { 15.0 } else if diff < 12.0 { 5.0 } else { -20.0 };
        }
        if best.as_ref().is_none_or(|(b, _)| s > *b) {
            best = Some((s, r["url"].as_str().unwrap_or("").to_string()));
        }
    }
    best.map(|(_, u)| u)
}

pub async fn probe(st: &impl Ctx, url: &str) -> anyhow::Result<Value> {
    let info = json_out(st, &["-J", "--flat-playlist", "--no-warnings", "--", url]).await?;
    let entries = info["entries"].as_array();
    Ok(json!({ "title": info["title"], "uploader": info["uploader"].as_str().or(info["artist"].as_str()),
        "thumbnail": info["thumbnail"].as_str().map(String::from).or_else(|| info["thumbnails"].as_array().and_then(|t| t.last()).and_then(|t| t["url"].as_str()).map(String::from)),
        "is_playlist": entries.is_some(), "count": entries.map(|e| e.len()).unwrap_or(1), "extractor": info["extractor_key"], "duration": info["duration"] }))
}

/// Flat track list of a playlist/album link (used for link imports).
pub async fn playlist(st: &impl Ctx, url: &str) -> anyhow::Result<Value> {
    json_out(st, &["-J", "--flat-playlist", "--no-warnings", "--", url]).await
}
