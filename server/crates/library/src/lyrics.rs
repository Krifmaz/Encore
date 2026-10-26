//! Lyrics from LRCLIB (free, no key): saved next to each song as .lrc (time-synced) or .txt (plain),
//! which Jellyfin, Navidrome, Plexamp and most players pick up too.
//!
//! A lyrics report (⋯ → Report a problem → Wrong lyrics / Lyrics out of sync) never touches the audio:
//! [`reject_current`] remembers the lyrics shown now in `lyrics_reject`, deletes the file next to the
//! song, and every later lookup skips LRCLIB versions with that text ([`Rejects::screen`]), so another
//! version (time-synced first) is picked – or none, rather than the wrong ones again.
//!
//! Lyrics whose words are right but run a little early or late are nudged instead ([`set_shift`]): the
//! lyrics view's Earlier / Later buttons save one shift per song in `lyrics_shift`, and every answer of
//! `GET /api/library/tracks/{id}/lyrics` carries it as `shift_ms` ([`with_shift`]), together with an
//! `[offset:…]` tag the .lrc file itself may have, so every device shows the lines at the same moment.
use encore_db::{Row, RowExt};
use encore_db::p;
use crate::Ctx;
use lofty::config::WriteOptions;
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::tag::{ItemKey, Tag};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
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

use encore_http::lrclib::lookup_with;

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
    // versions a lyrics report rejected for this song are skipped
    let rejects = Rejects::of(st, t.i("id"));
    let picky = !rejects.is_empty();
    let Some(hit) = lookup_with(st, &t.s("artist"), &t.s("title"), &t.s("album"), dur, |h| rejects.screen(h), picky).await? else {
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

// ---- lyrics reports ------------------------------------------------------------------------------

/// What a lyrics report says is wrong with the lyrics a song shows now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Complaint {
    /// The words belong to another song (or another version): no version with these words again.
    Wrong,
    /// The words are right but the timing is off: not this timing again; other time-synced
    /// versions, or the same words without timing, may be shown.
    OutOfSync,
}

impl Complaint {
    /// The complaint of a report reason ("wrong_lyrics", "lyrics_sync"); None for the reasons about the audio.
    pub fn of_reason(reason: &str) -> Option<Self> {
        match reason {
            "wrong_lyrics" => Some(Complaint::Wrong),
            "lyrics_sync" => Some(Complaint::OutOfSync),
            _ => None,
        }
    }
}

/// The words of lyrics, for comparing versions: the `[mm:ss.xx]` (and other `[…]`) tags taken out,
/// lower case, letters and digits only, one space between words.
pub fn lyric_words(text: &str) -> String {
    let mut out = String::new();
    for line in text.lines() {
        let mut depth = 0;
        for c in line.chars() {
            match c {
                '[' => depth += 1,
                ']' if depth > 0 => depth -= 1,
                _ if depth > 0 => {}
                c if c.is_alphanumeric() => out.extend(c.to_lowercase()),
                _ => out.push(' '),
            }
        }
        out.push(' ');
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Time-synced lyrics as compared: line endings and the spaces around each line evened out, empty lines dropped.
fn synced_norm(text: &str) -> String {
    text.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n")
}

/// SHA-256 (hex) of a text: the key a rejected version is remembered by.
fn digest(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}

/// The lyrics versions rejected for one song (the `lyrics_reject` rows), by kind.
#[derive(Default, Debug)]
pub struct Rejects {
    /// words no version may have (Wrong lyrics)
    words: HashSet<String>,
    /// exact time-synced texts not to use (Lyrics out of sync)
    synced: HashSet<String>,
    /// words not to show without timing (Lyrics out of sync on plain lyrics)
    plain: HashSet<String>,
    /// LRCLIB's "instrumental" was reported as wrong
    instrumental: bool,
}

impl Rejects {
    /// What reports rejected for track `id`.
    pub fn of(st: &impl Ctx, id: i64) -> Self {
        let mut r = Rejects::default();
        for row in st.db().query("SELECT kind, key FROM lyrics_reject WHERE track_id=?", p![id]).unwrap_or_default() {
            match row.s("kind").as_str() {
                "words" => { r.words.insert(row.s("key")); }
                "synced" => { r.synced.insert(row.s("key")); }
                "plain" => { r.plain.insert(row.s("key")); }
                "instrumental" => r.instrumental = true,
                _ => {}
            }
        }
        r
    }

    /// Nothing rejected: lookups behave as they always did.
    pub fn is_empty(&self) -> bool {
        self.words.is_empty() && self.synced.is_empty() && self.plain.is_empty() && !self.instrumental
    }

    /// An LRCLIB version with the rejected parts taken out (its time-synced or plain text set to null),
    /// or None when nothing of it may be shown.
    pub fn screen(&self, mut hit: Value) -> Option<Value> {
        if hit["instrumental"].as_bool().unwrap_or(false) {
            return (!self.instrumental).then_some(hit);
        }
        if let Some(s) = hit["syncedLyrics"].as_str() {
            if self.words.contains(&digest(&lyric_words(s))) || self.synced.contains(&digest(&synced_norm(s))) {
                hit["syncedLyrics"] = Value::Null;
            }
        }
        if let Some(s) = hit["plainLyrics"].as_str() {
            let w = digest(&lyric_words(s));
            if self.words.contains(&w) || self.plain.contains(&w) {
                hit["plainLyrics"] = Value::Null;
            }
        }
        (hit["syncedLyrics"].is_string() || hit["plainLyrics"].is_string()).then_some(hit)
    }
}

/// The `lyrics_reject` rows (kind, key) for lyrics shown now (`shown`: text and time-synced?, None when
/// LRCLIB's "instrumental" was shown) and a complaint about them.
pub fn reject_rows(shown: Option<(&str, bool)>, complaint: Complaint) -> Vec<(&'static str, String)> {
    match (shown, complaint) {
        (None, _) => vec![("instrumental", String::new())],
        (Some((text, _)), Complaint::Wrong) => vec![("words", digest(&lyric_words(text)))],
        (Some((text, true)), Complaint::OutOfSync) => vec![("synced", digest(&synced_norm(text)))],
        // plain lyrics have no timing to be off: these words come back only with time-synced lines
        (Some((text, false)), Complaint::OutOfSync) => vec![("plain", digest(&lyric_words(text)))],
    }
}

/// A lyrics report on track `t`: the lyrics shown now (the file next to the song, else what LRCLIB
/// answers now) are rejected for good, the .lrc/.txt files go (and the words Encore embedded in the
/// file's tags), and other lyrics are looked up at once. The audio itself is never moved or replaced.
/// → the new answer ({lyrics, synced, source}, `lyrics` null when no other version was found)
pub async fn reject_current(st: &impl Ctx, t: &Row, complaint: Complaint, report_id: i64) -> anyhow::Result<Value> {
    let path = Path::new(&t.s("path")).to_path_buf();
    let shown: Option<(String, bool)> = match sidecar(&path) {
        Some(s) => Some(s),
        // nothing saved (saving lyrics files is off, or LRCLIB said instrumental): what it answers now
        None => {
            let now = fetch_for_track(st, t, false).await?;
            if now["instrumental"].as_bool().unwrap_or(false) {
                None
            } else if let Some(l) = now["lyrics"].as_str().filter(|l| !l.trim().is_empty()) {
                Some((l.to_string(), now["synced"].as_bool().unwrap_or(false)))
            } else {
                // no lyrics shown at all: nothing to reject, just look again
                return fetch_for_track(st, t, true).await;
            }
        }
    };
    // the nudge was made on the lyrics that go now
    let _ = st.db().execute("DELETE FROM lyrics_shift WHERE track_id=?", p![t.i("id")]);
    for (kind, key) in reject_rows(shown.as_ref().map(|(l, s)| (l.as_str(), *s)), complaint) {
        st.db().execute("INSERT OR IGNORE INTO lyrics_reject (track_id, kind, key, report_id, created_at) VALUES (?,?,?,?,?)",
            p![t.i("id"), kind, key, report_id, encore_db::now()])?;
    }
    for ext in ["lrc", "txt"] {
        let f = path.with_extension(ext);
        if f.is_file() {
            if let Err(e) = std::fs::remove_file(&f) {
                st.db().log("warning", &format!("Could not delete the lyrics file {}: {e}", f.display()));
            }
        }
    }
    // Encore embeds plain lyrics when asked to; the rejected ones go from the tags too (the audio is untouched)
    if st.cfg().bool("lyrics_embed") && path.is_file() {
        if let Err(e) = unembed(&path) {
            st.db().log("warning", &format!("Could not remove the embedded lyrics from {}: {e}", path.display()));
        }
    }
    fetch_for_track(st, t, true).await
}

/// Take the lyrics out of a file's tags (nothing happens when it has none).
fn unembed(path: &Path) -> anyhow::Result<()> {
    let mut f = lofty::read_from_path(path)?;
    let Some(tag) = f.primary_tag_mut() else { return Ok(()) };
    if tag.get_string(ItemKey::Lyrics).is_none() {
        return Ok(());
    }
    tag.remove_key(ItemKey::Lyrics);
    f.save_to_path(path, WriteOptions::default())?;
    Ok(())
}

// ---- timing nudges ------------------------------------------------------------------------------

/// The largest nudge either way, ms: lines more than this far off are other lyrics, not a late start.
pub const SHIFT_MAX_MS: i64 = 30_000;

/// The shift an `[offset:±N]` tag of a .lrc asks for, in ms (0 without one). The LRC convention: a positive
/// offset shows the lines *sooner*, so the shift (how much *later* they are shown) is its negative.
pub fn lrc_offset_shift(text: &str) -> i64 {
    for line in text.lines().map(str::trim) {
        let Some(rest) = line.strip_prefix('[') else { continue };
        let Some((key, value)) = rest.split_once(':') else { continue };
        if !key.trim().eq_ignore_ascii_case("offset") {
            continue;
        }
        // "[offset:+250]" → "+250"; anything that isn't a whole number counts as no tag
        let value = value.trim_end_matches(']').trim().trim_start_matches('+');
        if let Ok(ms) = value.parse::<i64>() {
            return (-ms).clamp(-SHIFT_MAX_MS, SHIFT_MAX_MS);
        }
    }
    0
}

/// The key a nudge is saved under: the time-synced text it was made on, normalised like a rejected timing.
fn shift_key(synced_text: &str) -> String {
    digest(&synced_norm(synced_text))
}

/// The saved nudge of track `id` for the time-synced lyrics `synced_text` (ms, 0 when there is none or it was
/// made on other lyrics).
pub fn shift_of(st: &impl Ctx, id: i64, synced_text: &str) -> i64 {
    st.db().one("SELECT shift_ms, lyrics_key FROM lyrics_shift WHERE track_id=?", p![id]).ok().flatten()
        .filter(|r| r.s("lyrics_key") == shift_key(synced_text))
        .map(|r| r.i("shift_ms"))
        .unwrap_or(0)
}

/// A lyrics answer ({lyrics, synced, source, …}) for track `id` with `shift_ms` added: how much later every
/// time-synced line is to be shown (the song's saved nudge plus the file's own `[offset:]` tag; 0 for plain
/// lyrics or none).
pub fn with_shift(st: &impl Ctx, id: i64, mut answer: Value) -> Value {
    let shift = match (answer["synced"].as_bool().unwrap_or(false), answer["lyrics"].as_str()) {
        (true, Some(text)) => (shift_of(st, id, text) + lrc_offset_shift(text)).clamp(-SHIFT_MAX_MS, SHIFT_MAX_MS),
        _ => 0,
    };
    answer["shift_ms"] = json!(shift);
    answer
}

/// Save the nudge of track `id`'s time-synced lyrics `synced_text` (`shift_ms` later, clamped to
/// ±[`SHIFT_MAX_MS`]; 0 takes it away). → the saved nudge, ms.
pub fn set_shift(st: &impl Ctx, id: i64, synced_text: &str, shift_ms: i64) -> anyhow::Result<i64> {
    let shift = shift_ms.clamp(-SHIFT_MAX_MS, SHIFT_MAX_MS);
    if shift == 0 {
        st.db().execute("DELETE FROM lyrics_shift WHERE track_id=?", p![id])?;
    } else {
        st.db().execute("INSERT INTO lyrics_shift (track_id, shift_ms, lyrics_key, updated_at) VALUES (?,?,?,?) \
            ON CONFLICT(track_id) DO UPDATE SET shift_ms=excluded.shift_ms, lyrics_key=excluded.lyrics_key, updated_at=excluded.updated_at",
            p![id, shift, shift_key(synced_text), encore_db::now()])?;
    }
    Ok(shift)
}

/// The lyrics a dismissed report rejected may be shown again.
pub fn forget_rejects(st: &impl Ctx, report_id: i64) {
    let _ = st.db().execute("DELETE FROM lyrics_reject WHERE report_id=?", p![report_id]);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tags, case, punctuation and line breaks don't make lyrics different.
    #[test]
    fn words_ignore_timing_case_and_punctuation() {
        assert_eq!(lyric_words("[00:01.00] Hello, World!\r\n[00:02.50]  it's me"), "hello world it s me");
        assert_eq!(lyric_words("[ar:Someone]\nHello world\nit's me"), "hello world it s me");
        assert_eq!(lyric_words(""), "");
    }

    /// Wrong lyrics: no version with those words, timed or not. Out of sync: that timing goes, the
    /// same words without timing and other timings stay.
    #[test]
    fn rejects_screen_the_versions_a_report_named() {
        let bad = "[00:01.00] la la\n[00:02.00] la";
        let hit = json!({ "id": 1, "syncedLyrics": bad, "plainLyrics": "La la\nla" });
        let other_timing = json!({ "id": 2, "syncedLyrics": "[00:03.00] la la\n[00:04.00] la" });
        let other_song = json!({ "id": 3, "plainLyrics": "something else" });
        let set = |rows: Vec<(&'static str, String)>| {
            let mut r = Rejects::default();
            for (k, v) in rows {
                match k { "words" => { r.words.insert(v); } "synced" => { r.synced.insert(v); } "plain" => { r.plain.insert(v); } _ => r.instrumental = true }
            }
            r
        };
        let wrong = set(reject_rows(Some((bad, true)), Complaint::Wrong));
        assert!(wrong.screen(hit.clone()).is_none());
        assert!(wrong.screen(other_timing.clone()).is_none());
        assert_eq!(wrong.screen(other_song.clone()).unwrap()["id"], 3);

        let sync = set(reject_rows(Some((bad, true)), Complaint::OutOfSync));
        let kept = sync.screen(hit.clone()).unwrap();
        assert!(kept["syncedLyrics"].is_null() && kept["plainLyrics"].is_string());
        assert!(sync.screen(other_timing.clone()).unwrap()["syncedLyrics"].is_string());

        // plain lyrics reported out of sync: only a time-synced version of them may come back
        let plain = set(reject_rows(Some(("La la\nla", false)), Complaint::OutOfSync));
        assert!(plain.screen(json!({ "plainLyrics": "la la la" })).is_none());
        assert!(plain.screen(other_timing).is_some());

        let instr = set(reject_rows(None, Complaint::Wrong));
        assert!(instr.screen(json!({ "instrumental": true })).is_none());
        assert!(instr.screen(other_song).is_some());
        assert!(Rejects::default().is_empty() && !instr.is_empty());
    }

    /// An `[offset:]` tag shows lines sooner when positive: the shift is its negative; no tag, no shift.
    #[test]
    fn lrc_offset_tags() {
        assert_eq!(lrc_offset_shift("[ar:x]\n[offset:+250]\n[00:01.00] a"), -250);
        assert_eq!(lrc_offset_shift("[OFFSET: -1200]\n[00:01.00] a"), 1200);
        assert_eq!(lrc_offset_shift("[00:01.00] a"), 0);
        assert_eq!(lrc_offset_shift("[offset:soon]"), 0);
        assert_eq!(lrc_offset_shift("[offset:-999999]"), SHIFT_MAX_MS);
    }

    /// A nudge is keyed by the lyrics it was made on, whatever their line endings and blank lines.
    #[test]
    fn shift_keys_ignore_layout() {
        assert_eq!(shift_key("[00:01.00] a\r\n\r\n[00:02.00] b "), shift_key("[00:01.00] a\n[00:02.00] b"));
        assert_ne!(shift_key("[00:01.00] a"), shift_key("[00:01.50] a"));
    }

    /// Only the two lyrics reasons are complaints about lyrics.
    #[test]
    fn complaints_of_reasons() {
        assert_eq!(Complaint::of_reason("wrong_lyrics"), Some(Complaint::Wrong));
        assert_eq!(Complaint::of_reason("lyrics_sync"), Some(Complaint::OutOfSync));
        assert_eq!(Complaint::of_reason("wrong_song"), None);
    }
}
