//! Songs saved for listening without the server: audio + cover in the app's data folder, listed in index.json.
//! The offline page plays them through Tauri's asset protocol.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager, Runtime};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Song {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    /// URL the offline page can play (asset protocol).
    pub file: String,
    /// Cover URL for the page, or "".
    pub art: String,
}

/// What the web app sends with "Save for offline" (see web/js/player.js saveOffline).
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Request {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub ext: String,
    pub url: String,
    pub art: String,
}

impl Default for Request {
    fn default() -> Self {
        Self { id: String::new(), title: String::new(), artist: String::new(), album: String::new(), ext: "audio".into(), url: String::new(), art: String::new() }
    }
}

pub fn dir<R: Runtime>(app: &AppHandle<R>) -> PathBuf {
    let d = app.path().app_data_dir().unwrap_or_else(|_| std::env::temp_dir().join("encore")).join("offline");
    let _ = std::fs::create_dir_all(&d);
    d
}

fn index_path<R: Runtime>(app: &AppHandle<R>) -> PathBuf {
    dir(app).join("index.json")
}

pub fn list<R: Runtime>(app: &AppHandle<R>) -> Vec<Song> {
    std::fs::read(index_path(app)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

pub fn ids<R: Runtime>(app: &AppHandle<R>) -> Vec<String> {
    list(app).into_iter().map(|s| s.id).collect()
}

fn write<R: Runtime>(app: &AppHandle<R>, songs: &[Song]) {
    if let Ok(b) = serde_json::to_vec(songs) {
        let _ = std::fs::write(index_path(app), b);
    }
}

pub fn remove<R: Runtime>(app: &AppHandle<R>, id: &str) {
    let d = dir(app);
    let mut songs = list(app);
    songs.retain(|s| s.id != id);
    write(app, &songs);
    if let Ok(rd) = std::fs::read_dir(&d) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with(&format!("{id}.")) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

/// The audio file of a saved song (`<id>.<ext>` next to index.json), for the desktop's Cadenza playback.
#[cfg_attr(mobile, allow(dead_code))]
pub fn audio_path<R: Runtime>(app: &AppHandle<R>, id: &str) -> Option<PathBuf> {
    let id = clean(id, |c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if id.is_empty() {
        return None;
    }
    let rd = std::fs::read_dir(dir(app)).ok()?;
    rd.flatten().map(|e| e.path()).find(|p| is_audio_of(p, &id))
}

/// `<id>.<ext>` that isn't the cover, the index or an unfinished download.
fn is_audio_of(p: &Path, id: &str) -> bool {
    let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let Some(ext) = name.strip_prefix(&format!("{id}.")) else { return false };
    !ext.is_empty() && !ext.contains('.') && !matches!(ext, "jpg" | "part" | "json")
}

fn clean(s: &str, allow: fn(char) -> bool) -> String {
    s.chars().filter(|c| allow(*c)).collect()
}

/// The URL the webview can load a file from (Tauri's asset protocol; the scope is set in tauri.conf.json).
pub fn asset_url(path: &Path) -> String {
    use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
    // encodeURIComponent's set
    const SET: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'_').remove(b'.').remove(b'!').remove(b'~').remove(b'*').remove(b'\'').remove(b'(').remove(b')');
    let enc = utf8_percent_encode(&path.to_string_lossy(), SET).to_string();
    if cfg!(any(windows, target_os = "android")) { format!("http://asset.localhost/{enc}") } else { format!("asset://localhost/{enc}") }
}

/// Download `url` to `out` (through a .part file). `cookie` is the server's cookie header, so the request is signed in.
pub async fn download(http: &reqwest::Client, url: &str, cookie: &str, out: &Path) -> Result<(), String> {
    if out.exists() {
        return Ok(());
    }
    let mut req = http.get(url).timeout(std::time::Duration::from_secs(600));
    if !cookie.is_empty() {
        req = req.header(reqwest::header::COOKIE, cookie);
    }
    let r = req.send().await.map_err(|e| e.to_string())?;
    if !r.status().is_success() {
        return Err(format!("answered {}", r.status()));
    }
    let tmp = out.with_extension("part");
    let bytes = r.bytes().await.map_err(|e| e.to_string())?;
    tokio::fs::write(&tmp, &bytes).await.map_err(|e| e.to_string())?;
    tokio::fs::rename(&tmp, out).await.map_err(|e| e.to_string())
}

/// Save one song (audio, then cover if there is one) and put it in the index. Returns the title for the toast.
pub async fn save<R: Runtime>(app: &AppHandle<R>, http: &reqwest::Client, cookie: &str, req: Request) -> Result<String, String> {
    let id = clean(&req.id, |c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if id.is_empty() || req.url.is_empty() {
        return Err("nothing to save".into());
    }
    let ext = { let e = clean(&req.ext.to_lowercase(), |c| c.is_ascii_alphanumeric()); if e.is_empty() { "audio".to_string() } else { e } };
    let d = dir(app);
    let audio = d.join(format!("{id}.{ext}"));
    download(http, &req.url, cookie, &audio).await?;
    let art = d.join(format!("{id}.jpg"));
    if !req.art.is_empty() && !art.exists() {
        let _ = download(http, &req.art, cookie, &art).await;
    }
    let song = Song {
        id: id.clone(), title: req.title.clone(), artist: req.artist, album: req.album,
        file: asset_url(&audio), art: if art.exists() { asset_url(&art) } else { String::new() },
    };
    let mut songs = list(app);
    songs.retain(|s| s.id != id);
    songs.push(song);
    write(app, &songs);
    Ok(req.title)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_audio_file_is_not_the_cover_or_a_half_download() {
        assert!(is_audio_of(Path::new("/x/42.flac"), "42"));
        assert!(is_audio_of(Path::new("/x/42.audio"), "42"));
        assert!(!is_audio_of(Path::new("/x/42.jpg"), "42"));
        assert!(!is_audio_of(Path::new("/x/42.part"), "42"));
        assert!(!is_audio_of(Path::new("/x/420.flac"), "42"));
        assert!(!is_audio_of(Path::new("/x/index.json"), "42"));
    }
}
