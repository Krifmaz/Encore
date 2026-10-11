//! Chromaprint fingerprints + AcoustID lookups: identify what a file actually contains,
//! and whether two files hold the same recording.
use crate::matcher::{compare, describe};
use crate::state::AppState;
use serde_json::{json, Value};
use std::path::Path;
use tokio::process::Command;

const API: &str = "https://api.acoustid.org/v2/lookup";

fn which(name: &str) -> bool {
    std::env::var("PATH").map(|p| p.split(':').any(|d| Path::new(d).join(name).is_file())).unwrap_or(false)
}

pub fn can_fingerprint() -> bool {
    which("fpcalc")
}

pub fn available(st: &AppState) -> bool {
    !st.cfg.str("acoustid_api_key").is_empty() && can_fingerprint()
}

pub async fn fingerprint(path: &Path) -> Option<(f64, String)> {
    let o = Command::new("fpcalc").args(["-json", "-length", "120"]).arg(path).output().await.ok()?;
    if !o.status.success() {
        return None;
    }
    let d: Value = serde_json::from_slice(&o.stdout).ok()?;
    Some((d["duration"].as_f64()?, d["fingerprint"].as_str()?.to_string()))
}

/// → [{acoustid, score, recordings:[{id,title,artist,length,release_groups,release_group_ids}]}], best first.
pub async fn lookup(st: &AppState, duration: f64, fp: &str) -> anyhow::Result<Vec<Value>> {
    {
        // AcoustID allows 3 requests/second
        static LAST: tokio::sync::Mutex<Option<std::time::Instant>> = tokio::sync::Mutex::const_new(None);
        let mut last = LAST.lock().await;
        if let Some(l) = *last {
            let gap = std::time::Duration::from_millis(340);
            if l.elapsed() < gap {
                tokio::time::sleep(gap - l.elapsed()).await;
            }
        }
        *last = Some(std::time::Instant::now());
    }
    let d: Value = st.http.post(API).form(&[("client", st.cfg.str("acoustid_api_key")), ("duration", (duration as i64).to_string()),
        ("fingerprint", fp.to_string()), ("meta", "recordings releasegroups compress".into())]).send().await?.json().await?;
    if d["status"] != "ok" {
        anyhow::bail!("{}", d["error"]["message"].as_str().unwrap_or("AcoustID error"));
    }
    let mut out: Vec<Value> = d["results"].as_array().cloned().unwrap_or_default().iter().map(|res| {
        let recs: Vec<Value> = res["recordings"].as_array().cloned().unwrap_or_default().iter().map(|rec| {
            let artist: String = rec["artists"].as_array().map(|a| a.iter().map(|x| format!("{}{}", x["name"].as_str().unwrap_or(""), x["joinphrase"].as_str().unwrap_or(""))).collect()).unwrap_or_default();
            let groups = rec["releasegroups"].as_array().cloned().unwrap_or_default();
            json!({ "id": rec["id"], "title": rec["title"].as_str().unwrap_or(""), "artist": artist, "length": rec["duration"],
                "release_groups": groups.iter().take(3).map(|g| g["title"].clone()).collect::<Vec<_>>(),
                "release_group_ids": groups.iter().filter_map(|g| g["id"].as_str()).collect::<Vec<_>>() })
        }).collect();
        json!({ "acoustid": res["id"], "score": res["score"].as_f64().unwrap_or(0.0), "recordings": recs })
    }).collect();
    out.sort_by(|a, b| b["score"].as_f64().partial_cmp(&a["score"].as_f64()).unwrap_or(std::cmp::Ordering::Equal));
    Ok(out)
}

/// → {status: verified|mismatch|unknown, acoustid, detail}
pub async fn check_file(st: &AppState, path: &Path, title: &str, artist: &str, length: Option<f64>, recording_id: Option<&str>) -> anyhow::Result<Value> {
    let Some((dur, fp)) = fingerprint(path).await else { return Ok(json!({ "status": "unknown", "acoustid": null, "detail": "fingerprinting failed" })) };
    let results: Vec<Value> = lookup(st, dur, &fp).await?.into_iter().filter(|r| r["score"].as_f64().unwrap_or(0.0) >= 0.6).collect();
    let Some(best) = results.first() else { return Ok(json!({ "status": "unknown", "acoustid": null, "detail": "audio not in AcoustID database" })) };
    let aid = best["acoustid"].clone();
    let recs: Vec<Value> = results.iter().flat_map(|r| r["recordings"].as_array().cloned().unwrap_or_default()).collect();
    if recs.is_empty() {
        return Ok(json!({ "status": "unknown", "acoustid": aid, "detail": "fingerprint known but not linked to MusicBrainz" }));
    }
    if let Some(rid) = recording_id.filter(|r| !r.is_empty()) {
        if recs.iter().any(|r| r["id"] == rid) {
            return Ok(json!({ "status": "verified", "acoustid": aid, "detail": "MusicBrainz recording confirmed by audio" }));
        }
    }
    let want = describe(title, artist, "", length, None, None);
    let d = |r: &Value| describe(r["title"].as_str().unwrap_or(""), r["artist"].as_str().unwrap_or(""), "", r["length"].as_f64(), None, None);
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

/// Mostly Latin letters? (false for Japanese, Korean, Cyrillic … titles)
fn latin(s: &str) -> bool {
    let letters: Vec<char> = s.chars().filter(|c| c.is_alphabetic()).collect();
    letters.is_empty() || letters.iter().filter(|c| (**c as u32) < 0x250).count() * 2 >= letters.len()
}

// ---- same recording? (before replacing a file with a "better" copy) -------------------------------

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
