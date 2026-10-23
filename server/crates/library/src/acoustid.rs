//! Chromaprint fingerprints + AcoustID lookups: identify what a file actually contains,
//! and whether two files hold the same recording.
use crate::matcher::{compare, describe};
use crate::Ctx;
use serde_json::{json, Value};
use std::path::Path;
use tokio::process::Command;

/// fpcalc (Chromaprint) is on PATH.
pub fn can_fingerprint() -> bool {
    encore_core::which::which("fpcalc").is_some()
}

/// Fingerprint checks can run: an AcoustID key is set and fpcalc is there.
pub fn available(st: &impl Ctx) -> bool {
    !st.cfg().str("acoustid_api_key").is_empty() && can_fingerprint()
}

/// The first 120 s of `path` fingerprinted: (its duration, the compressed fingerprint).
pub async fn fingerprint(path: &Path) -> Option<(f64, String)> {
    let o = Command::new("fpcalc").args(["-json", "-length", "120"]).arg(path).output().await.ok()?;
    if !o.status.success() {
        return None;
    }
    let d: Value = serde_json::from_slice(&o.stdout).ok()?;
    Some((d["duration"].as_f64()?, d["fingerprint"].as_str()?.to_string()))
}

use encore_http::acoustid::lookup;

/// Whether `path` would be fingerprinted (an AcoustID key and fpcalc, or the tests' stand-in for it).
pub fn can_check(st: &impl Ctx, path: &Path) -> bool {
    available(st) || faked(path)
}

/// The words of a check that was not made because the title asks for a re-pitched copy ([`check_file`]).
pub fn skipped_detail(words: &str) -> String {
    let mut w = words.chars();
    let words = w.next().map(|c| c.to_uppercase().chain(w).collect::<String>()).unwrap_or_default();
    format!("{words} version – the fingerprint can't judge re-pitched audio; checked by length and title only")
}

/// → {status: verified|mismatch|unknown, acoustid, detail}. A title that asks for a nightcore, sped up,
/// slowed … copy ([`crate::matcher::altered_words`]) is not fingerprinted: its audio is a re-pitched copy,
/// which AcoustID knows as the original or as something unrelated ("unknown", `skipped`: the words).
pub async fn check_file(st: &impl Ctx, path: &Path, title: &str, artist: &str, length: Option<f64>, recording_id: Option<&str>) -> anyhow::Result<Value> {
    if let Some(words) = crate::matcher::altered_words(title) {
        return Ok(json!({ "status": "unknown", "acoustid": null, "skipped": words, "detail": skipped_detail(&words) }));
    }
    let (aid, recs, fp, dur) = match heard(path) {
        Some(recs) => (json!("test"), recs, String::new(), 0.0),
        None => {
            let Some((dur, fp)) = fingerprint(path).await else { return Ok(json!({ "status": "unknown", "acoustid": null, "detail": "fingerprinting failed" })) };
            let results: Vec<Value> = lookup(st, dur, &fp).await?.into_iter().filter(|r| r["score"].as_f64().unwrap_or(0.0) >= 0.6).collect();
            let Some(best) = results.first() else { return Ok(json!({ "status": "unknown", "acoustid": null, "detail": "audio not in AcoustID database" })) };
            let aid = best["acoustid"].clone();
            let recs: Vec<Value> = results.iter().flat_map(|r| r["recordings"].as_array().cloned().unwrap_or_default()).collect();
            (aid, recs, fp, dur)
        }
    };
    if recs.is_empty() {
        return Ok(json!({ "status": "unknown", "acoustid": aid, "detail": "fingerprint known but not linked to MusicBrainz" }));
    }
    if let Some(rid) = recording_id.filter(|r| !r.is_empty()) {
        if recs.iter().any(|r| r["id"] == rid) {
            // the fingerprint goes along so contribute.rs can send it to AcoustID (only this sure case: the
            // audio confirmed the recording the file was asked for / tagged as, no title guess)
            return Ok(json!({ "status": "verified", "acoustid": aid, "detail": "MusicBrainz recording confirmed by audio",
                "submit": { "mbid": rid, "fingerprint": fp, "duration": dur } }));
        }
    }
    let want = describe(title, artist, length, None, None);
    let d = |r: &Value| describe(r["title"].as_str().unwrap_or(""), r["artist"].as_str().unwrap_or(""), r["length"].as_f64(), None, None);
    for r in &recs {
        if compare(&want, &d(r), false).matched {
            return Ok(json!({ "status": "verified", "acoustid": aid, "detail": format!("audio matches “{} – {}”", r["artist"].as_str().unwrap_or(""), r["title"].as_str().unwrap_or("")) }));
        }
    }
    let top = &recs[0];
    let v = compare(&want, &d(top), false);
    let top_title = top["title"].as_str().unwrap_or("");
    // Can't judge, so don't reject: titles in different scripts ("ちゅきちゅきポコポコポン" vs
    // "CHUKICHUKI POKOPOKO-PON") or an instrumental/karaoke whose fingerprint AcoustID files under the
    // vocal version (the backing track is the same audio to Chromaprint).
    let lc = title.to_lowercase();
    let vocal_less = ["instrumental", "karaoke", "off vocal", "inst.", "(inst)"].iter().any(|k| lc.contains(k));
    if latin(title) != latin(top_title) || (vocal_less && v.reason.to_lowercase().contains("instrumental") || vocal_less && v.reason.to_lowercase().contains("karaoke")) {
        return Ok(json!({ "status": "unknown", "acoustid": aid, "actual": top,
            "detail": format!("AcoustID knows this audio as “{} – {}” – can't compare reliably, kept", top["artist"].as_str().unwrap_or(""), top_title) }));
    }
    Ok(json!({ "status": "mismatch", "acoustid": aid, "actual": top,
        "detail": format!("audio is “{} – {}” ({})", top["artist"].as_str().unwrap_or(""), top["title"].as_str().unwrap_or(""), v.reason) }))
}

/// What the tests' stand-in heard in `path` (as AcoustID's recordings), None outside tests.
fn heard(_path: &Path) -> Option<Vec<Value>> {
    #[cfg(any(test, feature = "test-support"))]
    if let Some((artist, title)) = fake::heard(_path) {
        return Some(vec![json!({ "id": "00000000-0000-0000-0000-000000000000", "title": title, "artist": artist, "length": null })]);
    }
    None
}

/// The tests' stand-in covers `path` (never outside tests).
pub fn faked(_path: &Path) -> bool {
    #[cfg(any(test, feature = "test-support"))]
    if fake::heard(_path).is_some() {
        return true;
    }
    false
}

/// A stand-in for fpcalc and AcoustID in tests: every file under a folder is heard as one song.
#[cfg(any(test, feature = "test-support"))]
pub mod fake {
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    /// (folder, artist, title) the stand-in fingerprints say files under the folder sound like.
    static HEARD: Mutex<Vec<(PathBuf, String, String)>> = Mutex::new(Vec::new());

    /// Files under `dir` sound like `artist – title` to the fingerprint check.
    pub fn hear(dir: impl AsRef<Path>, artist: &str, title: &str) {
        let mut h = HEARD.lock().unwrap_or_else(|p| p.into_inner());
        h.retain(|(d, _, _)| d != dir.as_ref());
        h.push((dir.as_ref().to_path_buf(), artist.to_string(), title.to_string()));
    }

    /// Files under `dir` are fingerprinted for real again.
    pub fn clear(dir: impl AsRef<Path>) {
        HEARD.lock().unwrap_or_else(|p| p.into_inner()).retain(|(d, _, _)| d != dir.as_ref());
    }

    /// (artist, title) heard in `path`: the deepest folder registered that holds it.
    pub fn heard(path: &Path) -> Option<(String, String)> {
        let h = HEARD.lock().unwrap_or_else(|p| p.into_inner());
        h.iter().filter(|(d, _, _)| path.starts_with(d)).max_by_key(|(d, _, _)| d.as_os_str().len()).map(|(_, a, t)| (a.clone(), t.clone()))
    }
}

/// Mostly Latin letters? (false for Japanese, Korean, Cyrillic … titles)
fn latin(s: &str) -> bool {
    let letters: Vec<char> = s.chars().filter(|c| c.is_alphabetic()).collect();
    letters.is_empty() || letters.iter().filter(|c| (**c as u32) < 0x250).count() * 2 >= letters.len()
}

// ---- same recording? (before replacing a file with a "better" copy) -------------------------------

/// The raw fingerprint of the first 90 s of `path` (for comparing two files).
async fn raw_fp(path: &Path) -> Option<Vec<u32>> {
    let o = Command::new("fpcalc").args(["-raw", "-json", "-length", "90"]).arg(path).output().await.ok()?;
    if !o.status.success() {
        return None;
    }
    let d: Value = serde_json::from_slice(&o.stdout).ok()?;
    Some(d["fingerprint"].as_array()?.iter().filter_map(|x| x.as_u64().map(|n| n as u32).or(x.as_i64().map(|n| n as u32))).collect())
}

/// 0..1 – share of matching fingerprint bits at the best alignment (same recording ≳ 0.8, different < 0.65).
pub fn similarity(a: &[u32], b: &[u32]) -> f64 {
    let mut best: f64 = 0.0;
    for off in -80i64..=80 {
        let start = 0.max(-off) as usize;
        let end = (a.len() as i64).min(b.len() as i64 - off);
        if end - start as i64 <= 20 {
            continue;
        }
        let end = end as usize;
        let diff: u32 = (start..end).map(|i| (a[i] ^ b[(i as i64 + off) as usize]).count_ones()).sum();
        best = best.max(1.0 - diff as f64 / (32.0 * (end - start) as f64));
    }
    best
}

/// Some(true/false) when both files could be fingerprinted, None when we can't tell.
pub async fn same_audio(a: &Path, b: &Path) -> Option<bool> {
    let (fa, fb) = tokio::join!(raw_fp(a), raw_fp(b));
    Some(similarity(&fa?, &fb?) >= 0.75)
}

// ---- contributing fingerprints ------------------------------------------------------------------

/// Queue a fingerprint after a verification (acoustid::check_file's `submit`: only present when the audio
/// confirmed the recording). Never queues the same fingerprint + recording twice.
pub fn queue_acoustid(st: &impl Ctx, track_id: Option<i64>, res: &Value) {
    let s = &res["submit"];
    let (Some(mbid), Some(fp), Some(dur)) = (s["mbid"].as_str(), s["fingerprint"].as_str(), s["duration"].as_f64()) else { return };
    if res["status"] != "verified" || !encore_core::ids::is_mbid(mbid) || fp.is_empty() || dur < 1.0 {
        return;
    }
    let hash = encore_core::ids::sha256_hex(fp);
    if let Some(t) = track_id {
        // the same song verified again (a library re-check): already queued or sent for this recording
        if st.db().one("SELECT 1 FROM contrib_acoustid WHERE track_id=? AND mbid=?", encore_db::p![t, mbid]).ok().flatten().is_some() {
            return;
        }
    }
    let _ = st.db().execute("INSERT OR IGNORE INTO contrib_acoustid (track_id, fp_hash, fingerprint, duration, mbid, status, created_at) VALUES (?,?,?,?,?,'queued',?)",
        encore_db::p![track_id, hash, fp, dur.round() as i64, mbid, encore_db::now()]);
}

#[cfg(test)]
mod tests {
    #[test]
    fn scripts() {
        assert!(super::latin("CHUKICHUKI POKOPOKO-PON"));
        assert!(!super::latin("ちゅきちゅきポコポコポン"));
        assert!(super::latin("Café del Mar"));
    }

    #[test]
    fn similarity() {
        let a: Vec<u32> = (0..200u32).map(|i| i.wrapping_mul(2654435761)).collect();
        let mut b = vec![0u32; 10];
        b.extend(&a);
        assert!(super::similarity(&a, &b) > 0.99);
        let c: Vec<u32> = (0..200u32).map(|i| i.wrapping_mul(40503).rotate_left(7)).collect();
        assert!(super::similarity(&a, &c) < 0.7);
    }
}
