//! yt-dlp (subprocess): YouTube, SoundCloud, Bandcamp, Mixcloud and ~1800 other sites.
use crate::state::AppState;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

pub const AUDIO: [&str; 8] = ["mp3", "m4a", "opus", "ogg", "flac", "webm", "wav", "aac"];

pub fn enabled(st: &AppState) -> bool {
    st.cfg.bool("ytdlp_enabled")
}

pub fn bin() -> String {
    std::env::var("ENCORE_YTDLP").unwrap_or_else(|_| "yt-dlp".into())
}

pub async fn version() -> Option<String> {
    let o = Command::new(bin()).arg("--version").output().await.ok()?;
    Some(String::from_utf8_lossy(&o.stdout).trim().to_string()).filter(|s| !s.is_empty())
}

fn cookie_args(st: &AppState, dir: &Path) -> Vec<String> {
    let c = st.cfg.str("ytdlp_cookies");
    if c.is_empty() {
        return vec![];
    }
    let p = dir.join(".cookies.txt");
    if std::fs::write(&p, c).is_ok() {
        vec!["--cookies".into(), p.to_string_lossy().into()]
    } else {
        vec![]
    }
}

pub struct Download {
    pub files: Vec<PathBuf>,
    pub entries: Vec<Value>,
}

/// Download a link (one video or a whole playlist) into `outdir`. `progress` gets 0–100.
pub async fn download(st: &AppState, url: &str, outdir: &Path, playlist: bool, progress: Option<&(dyn Fn(f64) + Send + Sync)>) -> anyhow::Result<Download> {
    std::fs::create_dir_all(outdir)?;
    let before: std::collections::HashSet<_> = std::fs::read_dir(outdir)?.flatten().map(|e| e.path()).collect();
    let fmt = st.cfg.str("ytdlp_format");
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
        let q = st.cfg.str("ytdlp_quality");
        args.extend(["--audio-quality".into(), if q.is_empty() { "0".into() } else { q }]);
    }
    args.extend(cookie_args(st, outdir));
    args.push("--".into());
    args.push(url.into());
    let mut child = Command::new(bin()).args(&args).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true).spawn()
        .map_err(|e| anyhow::anyhow!("yt-dlp is not installed ({e})"))?;
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
    for e in std::fs::read_dir(outdir)?.flatten() {
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
    if files.is_empty() && !status.success() {
        anyhow::bail!("{}", if err.is_empty() { "yt-dlp failed".into() } else { err.chars().take(300).collect::<String>() });
    }
    Ok(Download { files, entries })
}

async fn json_out(args: &[&str]) -> anyhow::Result<Value> {
    let o = Command::new(bin()).args(args).kill_on_drop(true).output().await.map_err(|e| anyhow::anyhow!("yt-dlp is not installed ({e})"))?;
    if !o.status.success() {
        anyhow::bail!("{}", String::from_utf8_lossy(&o.stderr).lines().last().unwrap_or("yt-dlp failed").chars().take(300).collect::<String>());
    }
    Ok(serde_json::from_slice(&o.stdout)?)
}

pub async fn search(query: &str, n: usize) -> anyhow::Result<Vec<Value>> {
    let info = json_out(&["-J", "--flat-playlist", "--no-warnings", &format!("ytsearch{n}:{query}")]).await?;
    Ok(info["entries"].as_array().cloned().unwrap_or_default().iter().map(|e| json!({
        "url": e["url"].as_str().map(String::from).unwrap_or(format!("https://www.youtube.com/watch?v={}", e["id"].as_str().unwrap_or(""))),
        "title": e["title"], "duration": e["duration"], "channel": e["channel"].as_str().or(e["uploader"].as_str()) })).collect())
}

/// The YouTube video that best matches a track (prefers "Topic" auto-generated uploads).
pub async fn find_best(artist: &str, title: &str, length: Option<f64>) -> Option<String> {
    let results = search(&format!("{artist} - {title} audio"), 6).await.ok()?;
    let (tl, al) = (title.to_lowercase(), artist.to_lowercase());
    let mut best: Option<(f64, String)> = None;
    for r in results {
        let name = r["title"].as_str().unwrap_or("").to_lowercase();
        let ch = r["channel"].as_str().unwrap_or("");
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

pub async fn probe(url: &str) -> anyhow::Result<Value> {
    let info = json_out(&["-J", "--flat-playlist", "--no-warnings", "--", url]).await?;
    let entries = info["entries"].as_array();
    Ok(json!({ "title": info["title"], "uploader": info["uploader"].as_str().or(info["artist"].as_str()),
        "thumbnail": info["thumbnail"].as_str().map(String::from).or_else(|| info["thumbnails"].as_array().and_then(|t| t.last()).and_then(|t| t["url"].as_str()).map(String::from)),
        "is_playlist": entries.is_some(), "count": entries.map(|e| e.len()).unwrap_or(1), "extractor": info["extractor_key"], "duration": info["duration"] }))
}

/// Flat track list of a playlist/album link (used for link imports).
pub async fn playlist(url: &str) -> anyhow::Result<Value> {
    json_out(&["-J", "--flat-playlist", "--no-warnings", "--", url]).await
}
