//! TIDAL lossless through a HiFi API endpoint – the backend Monochrome (monochrome.tf) streams from.
//! Several URLs can be given, comma-separated, tried in order.
use crate::matcher::{compare, describe};
use crate::routes::catalog::similar;
use crate::state::AppState;
use base64::Engine;
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;

pub fn endpoints(st: &AppState) -> Vec<String> {
    st.cfg.str("hifi_api_url").split(',').map(|u| u.trim().trim_end_matches('/').to_string())
        .filter(|u| u.starts_with("http://") || u.starts_with("https://")).collect()
}

pub fn enabled(st: &AppState) -> bool {
    st.cfg.bool("hifi_enabled") && !endpoints(st).is_empty()
}

async fn get(st: &AppState, path: &str, q: &[(&str, String)]) -> anyhow::Result<Value> {
    let mut last = None;
    for base in endpoints(st) {
        match st.http.get(format!("{base}{path}")).query(q).send().await {
            Ok(r) if r.status().as_u16() == 200 => {
                let d: Value = r.json().await?;
                return Ok(if d.is_object() { d.get("data").cloned().unwrap_or(d) } else { json!({ "items": d }) });
            }
            Ok(r) => last = Some(format!("{base} answered {}", r.status().as_u16())),
            Err(e) => last = Some(format!("{base}: {e}")),
        }
    }
    anyhow::bail!("{}", last.unwrap_or("No HiFi API endpoint configured".into()))
}

/// Responses nest their lists differently between versions: find the list of objects with an id.
fn items(d: &Value) -> Vec<Value> {
    if let Some(a) = d.as_array() {
        return a.iter().map(|x| x.get("item").cloned().unwrap_or(x.clone())).filter(|x| x.get("id").is_some()).collect();
    }
    for k in ["items", "tracks", "albums", "data"] {
        if let Some(x) = d.get(k) {
            let got = items(x);
            if !got.is_empty() {
                return got;
            }
        }
    }
    vec![]
}

fn artist(x: &Value) -> String {
    if let Some(a) = x["artists"].as_array().filter(|a| !a.is_empty()) {
        return a.iter().filter_map(|a| a["name"].as_str()).collect::<Vec<_>>().join(", ");
    }
    x["artist"]["name"].as_str().or(x["artist"].as_str()).unwrap_or("").to_string()
}

fn title(x: &Value) -> String {
    let t = x["title"].as_str().unwrap_or("").to_string();
    match x["version"].as_str().filter(|v| !v.is_empty() && !t.to_lowercase().contains(&v.to_lowercase())) {
        Some(v) => format!("{t} ({v})"),
        None => t,
    }
}

pub fn cover(uuid: Option<&str>, size: u32) -> Value {
    uuid.map(|u| json!(format!("https://resources.tidal.com/images/{}/{size}x{size}.jpg", u.replace('-', "/")))).unwrap_or(Value::Null)
}

pub async fn search_tracks(st: &AppState, q: &str, isrc: Option<&str>) -> anyhow::Result<Vec<Value>> {
    let p = match isrc { Some(i) => vec![("i", i.to_string())], None => vec![("s", q.to_string())] };
    let d = get(st, "/search/", &p).await?;
    Ok(items(&d).iter().map(|x| json!({ "id": x["id"], "title": title(x), "artist": artist(x), "album": x["album"]["title"].as_str().unwrap_or(""),
        "duration": x["duration"], "isrc": x["isrc"], "quality": x["audioQuality"].as_str().unwrap_or(""), "cover": cover(x["album"]["cover"].as_str(), 320) })).collect())
}

pub async fn search_albums(st: &AppState, q: &str) -> anyhow::Result<Vec<Value>> {
    let d = get(st, "/search/", &[("al", q.to_string())]).await?;
    Ok(items(&d).iter().map(|x| json!({ "id": x["id"], "title": title(x), "artist": artist(x), "tracks": x["numberOfTracks"],
        "date": x["releaseDate"].as_str().unwrap_or(""), "quality": x["audioQuality"].as_str().unwrap_or(""), "cover": cover(x["cover"].as_str(), 320) })).collect())
}

pub async fn album_tracks(st: &AppState, id: &Value) -> anyhow::Result<Vec<Value>> {
    let d = get(st, "/album/", &[("id", id.to_string().trim_matches('"').to_string())]).await?;
    Ok(items(&d).iter().filter(|x| x["title"].is_string()).map(|x| json!({ "id": x["id"], "title": title(x), "artist": artist(x),
        "track": x["trackNumber"], "disc": x["volumeNumber"].as_i64().unwrap_or(1), "duration": x["duration"], "isrc": x["isrc"] })).collect())
}

pub async fn find_track(st: &AppState, a: &str, t: &str, isrc: Option<&str>, length: Option<f64>) -> anyhow::Result<Option<Value>> {
    let want = describe(t, a, "", length, isrc, None);
    let mut lists = vec![];
    if let Some(i) = isrc {
        lists.push(search_tracks(st, "", Some(i)).await?);
    }
    lists.push(search_tracks(st, &format!("{a} {t}"), None).await?);
    for res in lists {
        for x in res {
            let d = describe(x["title"].as_str().unwrap_or(""), x["artist"].as_str().unwrap_or(""), "", x["duration"].as_f64(), x["isrc"].as_str(), None);
            if compare(&want, &d, true).matched {
                return Ok(Some(x));
            }
        }
    }
    Ok(None)
}

pub async fn find_album(st: &AppState, a: &str, album: &str, n: Option<usize>) -> anyhow::Result<Option<Value>> {
    for x in search_albums(st, &format!("{a} {album}")).await? {
        if similar(x["title"].as_str().unwrap_or(""), album) >= 0.85 && (a.is_empty() || similar(x["artist"].as_str().unwrap_or(""), a) >= 0.6) {
            if let (Some(n), Some(t)) = (n, x["tracks"].as_i64()) {
                if (t - n as i64).unsigned_abs() as usize > (n / 5).max(1) {
                    continue;
                }
            }
            return Ok(Some(x));
        }
    }
    Ok(None)
}

pub fn safe_name(name: &str) -> String {
    name.chars().map(|c| if "\\/:*?\"<>|".contains(c) { '_' } else { c }).collect()
}

/// Fetch one track's lossless stream into folder/name.<ext>.
pub async fn download_track(st: &AppState, id: &Value, folder: &Path, name: &str) -> anyhow::Result<PathBuf> {
    let q = st.cfg.str("hifi_quality");
    let d = get(st, "/track/", &[("id", id.to_string().trim_matches('"').to_string()), ("quality", if q.is_empty() { "LOSSLESS".into() } else { q })]).await?;
    let info = match &d {
        Value::Array(a) => a.iter().find(|x| x.get("manifest").is_some()).cloned().unwrap_or_default(),
        _ => d.clone(),
    };
    std::fs::create_dir_all(folder)?;
    let Some(manifest) = info["manifest"].as_str() else {
        let url = info["OriginalTrackUrl"].as_str().or(info["url"].as_str()).ok_or_else(|| anyhow::anyhow!("no stream in the API response"))?;
        return fetch(st, url, folder, name, "flac").await;
    };
    let pad = "=".repeat((4 - manifest.len() % 4) % 4);
    let raw = String::from_utf8_lossy(&base64::engine::general_purpose::STANDARD.decode(format!("{manifest}{pad}"))?).to_string();
    let name = safe_name(name);
    if raw.contains("<MPD") {
        // hi-res: DASH manifest → ffmpeg assembles it
        let mpd = folder.join(format!("{name}.mpd"));
        std::fs::write(&mpd, &raw)?;
        let out = folder.join(format!("{name}.flac"));
        let o = tokio::process::Command::new("ffmpeg").args(["-loglevel", "error", "-y", "-protocol_whitelist", "file,http,https,tcp,tls,crypto", "-i"])
            .arg(&mpd).args(["-map", "0:a", "-c:a", "flac"]).arg(&out).output().await?;
        let _ = std::fs::remove_file(&mpd);
        if !o.status.success() {
            let e = String::from_utf8_lossy(&o.stderr);
            anyhow::bail!("ffmpeg: {}", &e[e.len().saturating_sub(200)..]);
        }
        return Ok(out);
    }
    let m: Value = serde_json::from_str(&raw)?;
    if m["encryptionType"].as_str().unwrap_or("NONE") != "NONE" {
        anyhow::bail!("this stream is encrypted – choose a lossless quality instead of MQA/Atmos");
    }
    let codec = m["codecs"].as_str().or(m["mimeType"].as_str()).unwrap_or("flac");
    let url = m["urls"][0].as_str().ok_or_else(|| anyhow::anyhow!("no stream url"))?;
    fetch(st, url, folder, &name, if codec.contains("flac") { "flac" } else { "m4a" }).await
}

async fn fetch(st: &AppState, url: &str, folder: &Path, name: &str, ext: &str) -> anyhow::Result<PathBuf> {
    let path = folder.join(format!("{}.{ext}", safe_name(name)));
    let r = st.http.get(url).timeout(std::time::Duration::from_secs(600)).send().await?;
    if r.status().as_u16() >= 400 {
        anyhow::bail!("stream answered {}", r.status().as_u16());
    }
    let mut f = tokio::fs::File::create(&path).await?;
    let mut s = r.bytes_stream();
    while let Some(c) = s.next().await {
        f.write_all(&c?).await?;
    }
    f.flush().await?;
    Ok(path)
}

pub async fn status(st: &AppState) -> Value {
    match search_tracks(st, "test", None).await {
        Ok(_) => json!({ "ok": true, "message": format!("{} endpoint(s) answering", endpoints(st).len()) }),
        Err(e) => json!({ "ok": false, "message": e.to_string() }),
    }
}
