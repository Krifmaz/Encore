//! Verify the whole library by audio, and find duplicate copies of the same song.
use crate::db::{now, Row, RowExt};
use crate::matcher::{describe, key};
use crate::state::AppState;
use crate::tagger::quality;
use crate::{acoustid, jobs, p};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;

pub async fn verify_track(st: &AppState, t: &Row) -> anyhow::Result<Value> {
    let dur = Some(t.f("duration")).filter(|d| *d > 0.0);
    let res = acoustid::check_file(st, Path::new(&t.s("path")), &t.s("title"), &t.s("artist"), dur, Some(&t.s("mb_trackid"))).await?;
    st.db.execute("UPDATE tracks SET verify_status=?, verify_detail=?, acoustid_id=COALESCE(?, acoustid_id), verified_at=? WHERE id=?",
        p![res["status"], res["detail"], res["acoustid"], now(), t.i("id")])?;
    Ok(res)
}

pub async fn verify_library(st: AppState, only_new: bool) {
    // songs confirmed by hand ("manual") are never second-guessed by the fingerprint check
    let w = if only_new { "WHERE verify_status IS NULL" } else { "WHERE verify_status IS NOT 'manual'" };
    let rows = st.db.query(&format!("SELECT id, path, title, artist, duration, mb_trackid FROM tracks {w} ORDER BY id"), p![]).unwrap_or_default();
    if !jobs::start("verify", json!({ "done": 0, "total": rows.len(), "verified": 0, "mismatch": 0, "unknown": 0, "current": "" })) {
        return;
    }
    for t in rows {
        jobs::set("verify", json!({ "current": format!("{} – {}", t.s("artist"), t.s("title")) }));
        match verify_track(&st, &t).await {
            Ok(r) => jobs::add("verify", r["status"].as_str().unwrap_or("unknown"), 1),
            Err(e) => {
                jobs::add("verify", "unknown", 1);
                let msg = e.to_string();
                let _ = st.db.execute("UPDATE tracks SET verify_status='error', verify_detail=? WHERE id=?", p![msg.chars().take(200).collect::<String>(), t.i("id")]);
                if msg.contains("invalid API key") || msg.to_lowercase().contains("client") {
                    st.db.log("error", &format!("AcoustID: {msg}"));
                    break;
                }
            }
        }
        jobs::add("verify", "done", 1);
    }
    let j = jobs::get("verify");
    st.db.log("info", &format!("Verification finished: {} verified, {} mismatched, {} unknown", j["verified"], j["mismatch"], j["unknown"]));
    jobs::set("verify", json!({ "running": false, "current": "" }));
}

/// Groups of files that hold the same song, best copy first.
pub fn find_duplicates(st: &AppState) -> Vec<Value> {
    let rows = st.db.query("SELECT id, path, title, artist, album, albumartist, album_id, duration, format, bitrate, size, isrc, mb_trackid, acoustid_id, verify_status FROM tracks", p![]).unwrap_or_default();
    let ids: Vec<i64> = rows.iter().map(|r| r.i("id")).collect();
    let mut parent: HashMap<i64, i64> = ids.iter().map(|i| (*i, *i)).collect();
    let mut reason: HashMap<i64, String> = HashMap::new();
    fn find(p: &mut HashMap<i64, i64>, mut x: i64) -> i64 {
        while p[&x] != x {
            let gp = p[&p[&x]];
            p.insert(x, gp);
            x = gp;
        }
        x
    }
    let mut union = |p: &mut HashMap<i64, i64>, a: i64, b: i64, why: &str| {
        let (ra, rb) = (find(p, a), find(p, b));
        if ra != rb {
            p.insert(rb, ra);
            reason.entry(ra).or_insert(why.to_string());
        }
    };
    for (col, why) in [("acoustid_id", "identical audio (AcoustID)"), ("mb_trackid", "same MusicBrainz recording"), ("isrc", "same ISRC")] {
        let mut seen: HashMap<String, i64> = HashMap::new();
        for r in &rows {
            let k = r.s(col);
            if k.is_empty() {
                continue;
            }
            match seen.get(&k) { Some(first) => union(&mut parent, *first, r.i("id"), why), None => { seen.insert(k, r.i("id")); } }
        }
    }
    // same artist + base title + version, lengths within 3 s
    let mut buckets: HashMap<String, Vec<&Row>> = HashMap::new();
    for r in &rows {
        let d = describe(&r.s("title"), &r.s("artist"), "", Some(r.f("duration")), None, None);
        if !d.base.is_empty() {
            buckets.entry(key(&d)).or_default().push(r);
        }
    }
    for items in buckets.values_mut() {
        items.sort_by(|a, b| a.f("duration").partial_cmp(&b.f("duration")).unwrap_or(std::cmp::Ordering::Equal));
        for w in items.windows(2) {
            let (a, b) = (w[0].f("duration"), w[1].f("duration"));
            if a > 0.0 && b > 0.0 && (a - b).abs() <= 3.0 {
                union(&mut parent, w[0].i("id"), w[1].i("id"), "same song & version (tags + length)");
            }
        }
    }
    let mut groups: HashMap<i64, Vec<Row>> = HashMap::new();
    for r in &rows {
        let root = find(&mut parent, r.i("id"));
        groups.entry(root).or_default().push(r.clone());
    }
    let ignored: std::collections::HashSet<String> = st.db.query("SELECT key FROM dup_ignore", p![]).unwrap_or_default().into_iter().map(|r| r.s("key")).collect();
    let lib = format!("{}/", st.cfg.str("library_path").trim_end_matches('/'));
    let mut out = vec![];
    for (root, mut items) in groups {
        if items.len() < 2 {
            continue;
        }
        let mut byid: Vec<i64> = items.iter().map(|i| i.i("id")).collect();
        byid.sort();
        let gkey = byid.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",");
        if ignored.contains(&gkey) {
            continue;
        }
        items.sort_by(|a, b| (quality(b), -(b.s("path").len() as i64)).cmp(&(quality(a), -(a.s("path").len() as i64))));
        for i in items.iter_mut() {
            let p = i.s("path");
            i.insert("relpath".into(), json!(p.strip_prefix(&lib).unwrap_or(&p)));
        }
        out.push(json!({ "key": gkey, "reason": reason.get(&root).cloned().unwrap_or_default(), "keep": items[0].i("id"), "tracks": items }));
    }
    out.sort_by_key(|g| g["tracks"][0]["artist"].as_str().unwrap_or("").to_lowercase());
    out
}
