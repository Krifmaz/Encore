//! GET /api/library/tracks/{id}/edges → {start_s, end_s}: where a song's sound starts and stops, with the
//! near-silence (below −50 dBFS) at either end left out. "Smart overlap" song transitions use it to skip the
//! silent lead-in of the next song and start it at the last audible moment of the one playing.
//! Measured once per file with ffmpeg's silencedetect and kept in the database (recomputed when the file changes).
use crate::auth::User;
use encore_db::{now, RowExt};
use encore_core::error::{err, ApiResult};
use encore_db::p;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::Json;
use serde_json::{json, Value};
use std::time::Duration;

/// Below this level counts as silence.
pub const SILENCE_DB: i32 = -50;
/// A quiet stretch shorter than this inside a song is not an edge (only stretches touching the start or the end count anyway).
const MIN_SILENCE_S: f64 = 0.2;
fn ensure_table(st: &AppState) -> ApiResult<()> {
    // a cache, not data: created on first use (an older server simply never reads it)
    st.db.execute_batch(
        "CREATE TABLE IF NOT EXISTS track_edges (track_id INTEGER PRIMARY KEY, mtime REAL, size INTEGER, start_s REAL NOT NULL, end_s REAL NOT NULL, computed_at REAL)",
    )?;
    Ok(())
}

pub async fn edges(State(st): State<AppState>, User(_): User, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    let t = st.db.one("SELECT id, path, duration, mtime, size FROM tracks WHERE id=?", p![id])?.ok_or_else(|| err(404, "Track not found"))?;
    ensure_table(&st)?;
    let (mtime, size, dur) = (t.f("mtime"), t.i("size"), t.f("duration"));
    if let Some(c) = st.db.one("SELECT start_s, end_s, mtime, size FROM track_edges WHERE track_id=?", p![id])? {
        if c.f("mtime") == mtime && c.i("size") == size {
            return Ok(Json(json!({ "start_s": c.f("start_s"), "end_s": c.f("end_s"), "cached": true })));
        }
    }
    let path = t.s("path");
    let (start_s, end_s) = {
        let _slot = st.web.track_edges.acquire().await.map_err(|_| err(503, "Busy"))?;
        analyse(&path, dur).await.ok_or_else(|| err(502, "Could not analyse this song"))?
    };
    st.db.execute(
        "INSERT INTO track_edges (track_id, mtime, size, start_s, end_s, computed_at) VALUES (?,?,?,?,?,?)
         ON CONFLICT(track_id) DO UPDATE SET mtime=excluded.mtime, size=excluded.size, start_s=excluded.start_s, end_s=excluded.end_s, computed_at=excluded.computed_at",
        p![id, mtime, size, start_s, end_s, now()],
    )?;
    Ok(Json(json!({ "start_s": start_s, "end_s": end_s, "cached": false })))
}

/// Run silencedetect over the whole file: (start_s, end_s), or None when ffmpeg is missing or fails.
pub async fn analyse(path: &str, duration: f64) -> Option<(f64, f64)> {
    // worked out ahead of the song, never what plays: ffmpeg at idle priority, like every background run
    let out = tokio::time::timeout(Duration::from_secs(120), encore_media::Ffmpeg::new().low_priority()
        .args(["-hide_banner", "-nostdin", "-v", "info", "-i"]).arg(path)
        .args(["-vn", "-af", &format!("silencedetect=noise={SILENCE_DB}dB:d={MIN_SILENCE_S}"), "-f", "null", "-"])
        .output()).await.ok()?.ok()?;
    if !out.status.success() {
        return None;
    }
    let log = String::from_utf8_lossy(&out.stderr);
    let dur = if duration > 0.0 { duration } else { parse_duration(&log).unwrap_or(0.0) };
    Some(parse_silence(&log, dur))
}

/// "Duration: 00:03:21.44" from ffmpeg's input description.
fn parse_duration(log: &str) -> Option<f64> {
    let at = log.find("Duration: ")? + 10;
    let hms = log[at..].split(',').next()?.trim();
    let mut secs = 0.0;
    for part in hms.split(':') {
        secs = secs * 60.0 + part.parse::<f64>().ok()?;
    }
    Some(secs)
}

/// From silencedetect's log: the end of a silence that starts at the very beginning (else 0), and the start of
/// a silence that runs to the very end (else the duration). A song that is silent throughout keeps 0..duration.
pub fn parse_silence(log: &str, duration: f64) -> (f64, f64) {
    let num = |line: &str, key: &str| -> Option<f64> {
        let at = line.find(key)? + key.len();
        line[at..].split_whitespace().next()?.trim_end_matches('|').parse().ok()
    };
    // (start, end) pairs; end None = silent until the file ends
    let mut spans: Vec<(f64, Option<f64>)> = vec![];
    for line in log.lines() {
        if let Some(s) = num(line, "silence_start: ") {
            spans.push((s.max(0.0), None));
        } else if let Some(e) = num(line, "silence_end: ") {
            match spans.last_mut() {
                Some(last) if last.1.is_none() => last.1 = Some(e),
                _ => spans.push((0.0, Some(e))),
            }
        }
    }
    // the end as ffmpeg decoded it (its last progress `time=`): often shorter than the tagged length (an
    // MP3's encoder delay and padding, a VBR estimate), and ffmpeg 5+ ends a trailing silence there
    let decoded = decoded_end(log);
    let end_of_file = |e: Option<f64>| e.is_none_or(|e| (duration > 0.0 && e >= duration - 0.05) || decoded.is_some_and(|d| d > 0.0 && e >= d - 0.05));
    let mut start = 0.0;
    let mut end = duration;
    if let Some(&(s, e)) = spans.first() {
        if s <= 0.05 && !end_of_file(e) {
            start = e.unwrap_or(0.0);
        }
    }
    if let Some(&(s, e)) = spans.last() {
        if end_of_file(e) && s > start {
            end = s;
        }
    }
    // NaN on either side counts as "not after the start" too
    if end.partial_cmp(&start) != Some(std::cmp::Ordering::Greater) {
        return (0.0, duration.max(0.0));
    }
    (round3(start), round3(end))
}

/// The last progress `time=HH:MM:SS.xx` in ffmpeg's log: how far it decoded.
fn decoded_end(log: &str) -> Option<f64> {
    let at = log.rfind("time=")? + 5;
    let hms = log[at..].split(|c: char| c.is_whitespace()).next()?;
    let mut secs = 0.0;
    for part in hms.split(':') {
        secs = secs * 60.0 + part.parse::<f64>().ok()?;
    }
    Some(secs)
}

fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

#[cfg(test)]
#[path = "track_edges_tests.rs"]
mod tests;
