//! Lyrics from LRCLIB (free, no key): saved next to each song as .lrc (time-synced) or .txt (plain),
//! which Jellyfin, Navidrome, Plexamp and most players pick up too.
use crate::db::{Row, RowExt};
use crate::p;
use crate::state::AppState;
use lofty::config::WriteOptions;
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::tag::{ItemKey, Tag};
use regex::Regex;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::LazyLock;

const API: &str = "https://lrclib.net/api";

/// (lyrics, synced?) from an existing .lrc/.txt next to the file.
pub fn sidecar(path: &Path) -> Option<(String, bool)> {
    for (ext, synced) in [("lrc", true), ("txt", false)] {
        if let Ok(t) = std::fs::read_to_string(path.with_extension(ext)) {
            return Some((t, synced));
        }
    }
    None
}

fn clean(s: &str) -> String {
    static RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\s*[\(\[](feat|ft|with|remaster)[^\)\]]*[\)\]]").unwrap());
    RE.replace_all(s, "").trim().to_string()
}

pub async fn lookup(st: &AppState, artist: &str, title: &str, album: &str, duration: Option<f64>) -> anyhow::Result<Option<Value>> {
    let mut q = vec![("artist_name", artist.to_string()), ("track_name", clean(title))];
    if !album.is_empty() {
        q.push(("album_name", album.into()));
    }
    if let Some(d) = duration.filter(|d| *d > 0.0) {
        q.push(("duration", (d.round() as i64).to_string()));
    }
    let r = st.http.get(format!("{API}/get")).query(&q).send().await?;
    if r.status().is_success() {
        let d: Value = r.json().await?;
        if d["syncedLyrics"].is_string() || d["plainLyrics"].is_string() || d["instrumental"].as_bool().unwrap_or(false) {
            return Ok(Some(d));
        }
    }
    let r = st.http.get(format!("{API}/search")).query(&[("artist_name", artist.to_string()), ("track_name", clean(title))]).send().await?;
    let mut hits: Vec<Value> = if r.status().is_success() { r.json::<Value>().await?.as_array().cloned().unwrap_or_default() } else { vec![] };
    if let Some(d) = duration.filter(|d| *d > 0.0) {
        hits.retain(|h| h["duration"].as_f64().is_none_or(|x| (x - d).abs() <= 3.0));
    }
    hits.sort_by_key(|h| (h["syncedLyrics"].is_null(), h["plainLyrics"].is_null()));
    Ok(hits.into_iter().next())
}

/// Unsynced lyrics inside the file, for players that don't read .lrc files.
fn embed(path: &Path, plain: &str) -> anyhow::Result<()> {
    let mut f = lofty::read_from_path(path)?;
    if f.primary_tag().is_none() {
        let t = f.primary_tag_type();
        f.insert_tag(Tag::new(t));
    }
    f.primary_tag_mut().unwrap().insert_text(ItemKey::Lyrics, plain.to_string());
    f.save_to_path(path, WriteOptions::default())?;
    Ok(())
}

/// {lyrics, synced, source}; downloads and saves them when missing.
pub async fn fetch_for_track(st: &AppState, t: &Row, force: bool) -> anyhow::Result<Value> {
    let path = Path::new(&t.s("path")).to_path_buf();
    if let (Some((text, synced)), false) = (sidecar(&path), force) {
        return Ok(json!({ "lyrics": text, "synced": synced, "source": "file" }));
    }
    let dur = Some(t.f("duration")).filter(|d| *d > 0.0);
    let Some(hit) = lookup(st, &t.s("artist"), &t.s("title"), &t.s("album"), dur).await? else {
        return Ok(json!({ "lyrics": null, "synced": false, "source": null }));
    };
    if hit["instrumental"].as_bool().unwrap_or(false) {
        return Ok(json!({ "lyrics": "", "synced": false, "source": "LRCLIB", "instrumental": true }));
    }
    let synced = hit["syncedLyrics"].is_string();
    let text = hit["syncedLyrics"].as_str().or(hit["plainLyrics"].as_str()).unwrap_or("").to_string();
    let plain = hit["plainLyrics"].as_str().unwrap_or("");
    if st.cfg.bool("lyrics_save_files") {
        if let Err(e) = std::fs::write(path.with_extension(if synced { "lrc" } else { "txt" }), &text) {
            st.db.log("warning", &format!("Could not save lyrics for {}: {e}", t.s("title")));
        }
    }
    if !plain.is_empty() && st.cfg.bool("lyrics_embed") && (synced || !st.cfg.bool("lyrics_save_files")) {
        if let Err(e) = embed(&path, plain) {
            st.db.log("warning", &format!("Could not embed lyrics in {}: {e}", path.display()));
        }
    }
    Ok(json!({ "lyrics": text, "synced": synced, "source": "LRCLIB" }))
}

/// Fetch lyrics for songs that have none yet (after imports, from the album menu, and on a schedule).
pub async fn fetch_missing(st: &AppState, ids: Option<&[i64]>, limit: usize) -> Value {
    let rows = match ids {
        Some(ids) if !ids.is_empty() => st.db.query(&format!("SELECT id, path, artist, title, album, duration FROM tracks WHERE id IN ({})", vec!["?"; ids.len()].join(",")),
            ids.iter().map(|i| json!(i)).collect()),
        _ => st.db.query("SELECT id, path, artist, title, album, duration FROM tracks ORDER BY added_at DESC LIMIT ?", p![limit * 5]),
    }.unwrap_or_default();
    let (mut done, mut found) = (0, 0);
    for t in rows {
        if sidecar(Path::new(&t.s("path"))).is_some() {
            continue;
        }
        if done >= limit {
            break;
        }
        done += 1;
        if fetch_for_track(st, &t, false).await.is_ok_and(|r| r["lyrics"].as_str().is_some_and(|l| !l.is_empty())) {
            found += 1;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await; // be gentle with the free service
    }
    json!({ "checked": done, "found": found })
}
