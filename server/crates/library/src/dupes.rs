//! Verify the whole library by audio, and find duplicate copies of the same song.
use encore_db::{now, Row, RowExt};
use crate::matcher::{describe, key};
use crate::Ctx;
use crate::tagger::quality;
use crate::acoustid;
use encore_db::p;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;

/// Fingerprint-check one song and save the verdict (verified, mismatch or unknown) on its row.
pub async fn verify_track(st: &impl Ctx, t: &Row) -> anyhow::Result<Value> {
    let dur = Some(t.f("duration")).filter(|d| *d > 0.0);
    let res = acoustid::check_file(st, Path::new(&t.s("path")), &t.s("title"), &t.s("artist"), dur, Some(&t.s("mb_trackid"))).await?;
    st.db().execute("UPDATE tracks SET verify_status=?, verify_detail=?, acoustid_id=COALESCE(?, acoustid_id), verified_at=? WHERE id=?",
        p![res["status"], res["detail"], res["acoustid"], now(), t.i("id")])?;
    crate::acoustid::queue_acoustid(st, Some(t.i("id")), &res);
    Ok(res)
}

/// The Verify job: fingerprint-check every song (`only_new`: only those never checked); a song confirmed by hand is skipped.
pub async fn verify_library(st: impl Ctx, only_new: bool) {
    // songs confirmed by hand ("manual") are never second-guessed by the fingerprint check
    let w = if only_new { "WHERE verify_status IS NULL" } else { "WHERE verify_status IS NOT 'manual'" };
    let rows = st.db().query(&format!("SELECT id, path, title, artist, duration, mb_trackid FROM tracks {w} ORDER BY id"), p![]).unwrap_or_default();
    if !st.jobs().start("verify", json!({ "done": 0, "total": rows.len(), "verified": 0, "mismatch": 0, "unknown": 0, "current": "" })) {
        return;
    }
    for t in rows {
        st.jobs().set("verify", json!({ "current": format!("{} – {}", t.s("artist"), t.s("title")) }));
        match verify_track(&st, &t).await {
            Ok(r) => st.jobs().add("verify", r["status"].as_str().unwrap_or("unknown"), 1),
            Err(e) => {
                st.jobs().add("verify", "unknown", 1);
                let msg = e.to_string();
                let _ = st.db().execute("UPDATE tracks SET verify_status='error', verify_detail=? WHERE id=?", p![msg.chars().take(200).collect::<String>(), t.i("id")]);
                if msg.contains("invalid API key") || msg.to_lowercase().contains("client") {
                    st.db().log("error", &format!("AcoustID: {msg}"));
                    break;
                }
            }
        }
        st.jobs().add("verify", "done", 1);
    }
    let j = st.jobs().get("verify");
    st.db().log("info", &format!("Verification finished: {} verified, {} mismatched, {} unknown", j["verified"], j["mismatch"], j["unknown"]));
    st.jobs().set("verify", json!({ "running": false, "current": "" }));
}

/// The reason of a group that holds a song downloaded from YouTube under its uploader's name and the
/// library's own copy ([`youtube_copies`]).
pub const YOUTUBE_COPY_REASON: &str = "YouTube copy (the uploader was taken for the artist)";

/// Was this song downloaded from YouTube (yt-dlp)?
fn from_youtube(r: &Row) -> bool {
    let url = r.s("source_url").to_lowercase();
    r.s("source") == "ytdlp" || url.contains("youtube.com/") || url.contains("youtu.be/")
}

/// Pairs (YouTube song, library copy) of songs downloaded from YouTube under another artist's name (the
/// uploader's: a lyrics channel, a fan) that the library already had: the same title and version and a
/// length within 3 s, the other copy not from YouTube, by one artist only (two artists with such a song is
/// no pair: a cover or a common title). Same-artist copies are found by the other rules already.
pub fn youtube_copies(rows: &[Row]) -> Vec<(i64, i64)> {
    let mut buckets: HashMap<String, Vec<(&Row, crate::matcher::Desc)>> = HashMap::new();
    for r in rows {
        let d = describe(&r.s("title"), &r.s("artist"), Some(r.f("duration")), None, None);
        if !d.base.is_empty() && d.length.is_some() {
            let k = format!("{}|{}|{}", d.base, d.versions.iter().copied().collect::<Vec<_>>().join(","), d.remixer);
            buckets.entry(k).or_default().push((r, d));
        }
    }
    let mut out = vec![];
    for items in buckets.values() {
        let (yt, other): (Vec<_>, Vec<_>) = items.iter().partition(|(r, _)| from_youtube(r));
        for (y, yd) in &yt {
            let near: Vec<&(&Row, crate::matcher::Desc)> = other.iter().copied()
                .filter(|(_, d)| (d.length.unwrap_or(0.0) - yd.length.unwrap_or(0.0)).abs() <= 3.0 && d.artists.first() != yd.artists.first()).collect();
            let Some(first) = near.first() else { continue };
            if near.iter().all(|(_, d)| d.artists.first() == first.1.artists.first()) {
                out.push((y.i("id"), first.0.i("id")));
            }
        }
    }
    out
}

/// Groups of files that hold the same song, best copy first. A group with a YouTube copy under the
/// uploader's name ([`youtube_copies`]) says `review: true`: its songs only share a title and a length, so
/// someone looks at it before a copy goes ("Resolve all" leaves it alone).
pub fn find_duplicates(st: &impl Ctx) -> Vec<Value> {
    let rows = st.db().query("SELECT id, path, title, artist, album, albumartist, album_id, duration, format, bitrate, size, isrc, mb_trackid, acoustid_id, verify_status,
                              source, source_url FROM tracks", p![]).unwrap_or_default();
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
            reason.entry(ra).or_insert_with(|| why.to_string());
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
        let d = describe(&r.s("title"), &r.s("artist"), Some(r.f("duration")), None, None);
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
    // songs from YouTube under the uploader's name, joined to the library's copy (to be looked at first)
    let mut review: std::collections::HashSet<i64> = std::collections::HashSet::new();
    for (y, other) in youtube_copies(&rows) {
        union(&mut parent, other, y, YOUTUBE_COPY_REASON);
        review.insert(y);
    }
    let mut groups: HashMap<i64, Vec<Row>> = HashMap::new();
    for r in &rows {
        let root = find(&mut parent, r.i("id"));
        groups.entry(root).or_default().push(r.clone());
    }
    let ignored: std::collections::HashSet<String> = st.db().query("SELECT key FROM dup_ignore", p![]).unwrap_or_default().into_iter().map(|r| r.s("key")).collect();
    let lib = format!("{}/", st.cfg().str("library_path").trim_end_matches('/'));
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
        items.sort_by_key(|i| std::cmp::Reverse((quality(i), -(i.s("path").len() as i64))));
        for i in items.iter_mut() {
            let p = i.s("path");
            i.insert("relpath".into(), json!(p.strip_prefix(&lib).unwrap_or(&p)));
        }
        let looks = items.iter().any(|i| review.contains(&i.i("id")));
        // a YouTube copy's group says so, whatever rule joined it first
        let why = if looks { YOUTUBE_COPY_REASON.to_string() } else { reason.get(&root).cloned().unwrap_or_default() };
        out.push(json!({ "key": gkey, "reason": why, "keep": items[0].i("id"), "tracks": items, "review": looks }));
    }
    out.sort_by_key(|g| g["tracks"][0]["artist"].as_str().unwrap_or("").to_lowercase());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A library row for [`youtube_copies`].
    fn row(id: i64, title: &str, artist: &str, dur: f64, source: &str) -> Row {
        serde_json::from_value(json!({ "id": id, "title": title, "artist": artist, "duration": dur, "source": source, "source_url": "" })).unwrap()
    }

    /// A YouTube download under a lyrics channel's name pairs with the library's copy; not another
    /// version, not a length far off, not when two artists have the song.
    #[test]
    fn youtube_copies_under_the_uploaders_name() {
        let rows = vec![row(1, "Levels", "Avicii", 200.0, "slskd"), row(2, "Levels (Lyrics)", "7clouds", 201.5, "ytdlp"),
                        row(3, "Levels (Live)", "fan", 200.0, "ytdlp"), row(4, "Levels", "fan2", 230.0, "ytdlp"),
                        row(5, "Hello", "Adele", 295.0, "scan"), row(6, "Hello", "Lionel Richie", 296.0, "scan"), row(7, "Hello", "fan", 295.0, "ytdlp")];
        assert_eq!(youtube_copies(&rows), vec![(2, 1)]);
    }
}
