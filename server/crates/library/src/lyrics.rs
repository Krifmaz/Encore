//! Lyrics from LRCLIB (free, no key): saved next to each song as .lrc (time-synced) or .txt (plain),
//! which Jellyfin, Navidrome, Plexamp and most players pick up too.
use encore_db::{Row, RowExt};
use encore_db::p;
use crate::Ctx;
use lofty::config::WriteOptions;
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::tag::{ItemKey, Tag};
use serde_json::{json, Value};
use std::path::Path;

/// (lyrics, synced?) from an existing .lrc/.txt next to the file.
pub fn sidecar(path: &Path) -> Option<(String, bool)> {
    for (ext, synced) in [("lrc", true), ("txt", false)] {
        if let Ok(t) = std::fs::read_to_string(path.with_extension(ext)) {
            return Some((t, synced));
        }
    }
    None
}

use encore_http::lrclib::lookup;

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
pub async fn fetch_for_track(st: &impl Ctx, t: &Row, force: bool) -> anyhow::Result<Value> {
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
    if st.cfg().bool("lyrics_save_files") {
        if let Err(e) = std::fs::write(path.with_extension(if synced { "lrc" } else { "txt" }), &text) {
            st.db().log("warning", &format!("Could not save lyrics for {}: {e}", t.s("title")));
        }
    }
    if !plain.is_empty() && st.cfg().bool("lyrics_embed") && (synced || !st.cfg().bool("lyrics_save_files")) {
        if let Err(e) = embed(&path, plain) {
            st.db().log("warning", &format!("Could not embed lyrics in {}: {e}", path.display()));
        }
    }
    Ok(json!({ "lyrics": text, "synced": synced, "source": "LRCLIB" }))
}

/// Fetch lyrics for songs that have none yet (after imports, from the album menu, and on a schedule).
pub async fn fetch_missing(st: &impl Ctx, ids: Option<&[i64]>, limit: usize) -> Value {
    let rows = match ids {
        Some(ids) if !ids.is_empty() => st.db().query(&format!("SELECT id, path, artist, title, album, duration FROM tracks WHERE id IN ({})", vec!["?"; ids.len()].join(",")),
            ids.iter().map(|i| json!(i)).collect()),
        _ => st.db().query("SELECT id, path, artist, title, album, duration FROM tracks ORDER BY added_at DESC LIMIT ?", p![limit * 5]),
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
