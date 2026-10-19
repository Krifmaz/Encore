//! Playlists & liked songs: import, match against the library, de-duplicate, merge, request missing.
use encore_db::{now, Row, RowExt};
use encore_library::matcher::{compare, describe, key, Desc, LibraryIndex};
use encore_db::p;
use crate::Ctx;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

pub fn get_liked(st: &impl Ctx, uid: i64) -> anyhow::Result<Row> {
    if let Some(r) = st.db().one("SELECT * FROM playlists WHERE user_id=? AND kind='liked'", p![uid])? {
        return Ok(r);
    }
    let t = now();
    let id = st.db().execute("INSERT INTO playlists (user_id, name, kind, source, sync_targets, created_at, updated_at) VALUES (?,?,?,?,?,?,?)",
        p![uid, "Liked Songs", "liked", "manual", "[]", t, t])?;
    Ok(st.db().one("SELECT * FROM playlists WHERE id=?", p![id])?.unwrap())
}

pub fn items(st: &impl Ctx, pid: i64) -> Vec<Row> {
    st.db().query("SELECT * FROM playlist_items WHERE playlist_id=? ORDER BY position, id", p![pid]).unwrap_or_default()
}

/// A row's text field, None when it is empty.
pub fn opt(r: &Row, k: &str) -> Option<String> {
    Some(r.s(k)).filter(|x| !x.is_empty())
}

pub fn item_desc(r: &Row) -> Desc {
    describe(&r.s("title"), &r.s("artist"), Some(r.f("duration")), opt(r, "isrc").as_deref(), opt(r, "mbid").as_deref())
}

/// Key used to spot duplicates inside a playlist.
pub fn identity(r: &Row, by_track: bool) -> String {
    if by_track && r.i("track_id") > 0 {
        return format!("t:{}", r.i("track_id"));
    }
    if let Some(m) = opt(r, "mbid") {
        return format!("m:{m}");
    }
    if let Some(i) = opt(r, "isrc") {
        return format!("i:{i}");
    }
    format!("k:{}", key(&item_desc(r)))
}

/// Drop rows that are the same song+version as an earlier row (version-aware).
pub fn dedupe_rows(rows: Vec<Row>) -> (Vec<Row>, usize) {
    let (mut out, mut seen, mut kept): (Vec<Row>, HashSet<String>, Vec<Desc>) = (vec![], HashSet::new(), vec![]);
    let mut dropped = 0;
    for r in rows {
        let k = identity(&r, true);
        if seen.contains(&k) {
            dropped += 1;
            continue;
        }
        let d = item_desc(&r);
        let pre = |x: &str| x.chars().take(4).collect::<String>();
        if r.i("track_id") == 0 && kept.iter().any(|kd| pre(&kd.base) == pre(&d.base) && compare(&d, kd, true).matched) {
            dropped += 1;
            continue;
        }
        seen.insert(k);
        // an item with a library match also blocks its other identities (isrc/mbid/key)
        seen.insert(identity(&r, false));
        kept.push(d);
        out.push(r);
    }
    (out, dropped)
}

/// Link rows to library tracks. → how many matched
pub fn match_rows(rows: &mut [Row], ix: &LibraryIndex) -> usize {
    let mut n = 0;
    for r in rows.iter_mut() {
        match ix.find(&item_desc(r)) {
            Some((hit, v)) => {
                r.insert("track_id".into(), json!(hit.i("id")));
                r.insert("match_reason".into(), json!(v.reason));
                n += 1;
            }
            None => {
                r.insert("track_id".into(), Value::Null);
                r.insert("match_reason".into(), Value::Null);
            }
        }
    }
    n
}

/// Imported/fetched track (title, artist, album, duration, isrc, mbid, cover, source_id[, track_id]) → playlist row.
fn row_from(t: &Value, source: &str) -> Row {
    let mut r = Row::new();
    for (k, src) in [("title", "title"), ("artist", "artist"), ("album", "album"), ("duration", "duration"), ("isrc", "isrc"), ("mbid", "mbid"),
                     ("cover_url", "cover"), ("source_id", "source_id"), ("track_id", "track_id")] {
        r.insert(k.into(), t.get(src).cloned().unwrap_or(Value::Null));
    }
    r.insert("source".into(), json!(source));
    r
}

/// Append tracks, skipping anything already in the playlist (same song + version). → (added, duplicates)
pub fn add_tracks(st: &impl Ctx, pid: i64, tracks: &[Value], source: &str) -> anyhow::Result<(usize, usize)> {
    let existing = items(st, pid);
    let mut rows: Vec<Row> = vec![];
    for t in tracks {
        let mut t = t.clone();
        if t["track_id"].as_i64().is_some() && t["title"].as_str().unwrap_or("").is_empty() {
            let Some(lt) = st.db().one("SELECT title, artist, album, duration, isrc, mb_trackid FROM tracks WHERE id=?", p![t["track_id"]])? else { continue };
            for (k, c) in [("title", "title"), ("artist", "artist"), ("album", "album"), ("duration", "duration"), ("isrc", "isrc"), ("mbid", "mb_trackid")] {
                t[k] = lt.get(c).cloned().unwrap_or(Value::Null);
            }
        }
        rows.push(row_from(&t, source));
    }
    let mut need: Vec<Row> = rows.iter().filter(|r| r.i("track_id") == 0).cloned().collect();
    if !need.is_empty() {
        // the library's index, kept while the songs are unchanged (built here when they changed: callers
        // run this on a blocking thread)
        match_rows(&mut need, &st.library().songs.get(st.db()));
        let mut it = need.into_iter();
        for r in rows.iter_mut().filter(|r| r.i("track_id") == 0) {
            *r = it.next().unwrap();
        }
    }
    let mut all = existing.clone();
    all.extend(rows);
    let (merged, dropped) = dedupe_rows(all);
    let new_rows: Vec<Row> = merged.into_iter().filter(|r| !r.contains_key("id")).collect();
    let pos = existing.iter().map(|e| e.i("position")).max().map(|m| m + 1).unwrap_or(0);
    let t = now();
    for (i, r) in new_rows.iter().enumerate() {
        st.db().execute("INSERT INTO playlist_items (playlist_id, position, title, artist, album, duration, isrc, mbid, cover_url, source, source_id, track_id, match_reason, added_at)
                       VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            p![pid, pos + i as i64, r["title"], r["artist"], r["album"], r["duration"], r["isrc"], r["mbid"], r["cover_url"], r["source"], r["source_id"],
               r.get("track_id").filter(|v| v.as_i64().is_some_and(|x| x > 0)).cloned().unwrap_or(Value::Null), r.get("match_reason").cloned().unwrap_or(Value::Null), t])?;
    }
    st.db().execute("UPDATE playlists SET updated_at=? WHERE id=?", p![t, pid])?;
    Ok((new_rows.len(), dropped))
}

pub fn add_library_tracks(st: &impl Ctx, pid: i64, ids: &[i64]) -> anyhow::Result<(usize, usize)> {
    let tracks: Vec<Value> = ids.iter().map(|i| json!({ "track_id": i, "title": "" })).collect();
    add_tracks(st, pid, &tracks, "library")
}

#[allow(clippy::too_many_arguments)]
pub fn create(st: &impl Ctx, uid: i64, name: &str, tracks: &[Value], source: &str, source_url: Option<&str>, description: &str, cover: Option<&str>, follow: bool) -> anyhow::Result<Value> {
    let t = now();
    let pid = st.db().execute("INSERT INTO playlists (user_id, name, description, kind, source, source_url, follow, sync_targets, cover_url, created_at, updated_at)
                             VALUES (?,?,?,?,?,?,?,?,?,?,?)", p![uid, name, description, "playlist", source, source_url, follow as i64, "[]", cover, t, t])?;
    let (added, dupes) = add_tracks(st, pid, tracks, source)?;
    Ok(json!({ "id": pid, "added": added, "duplicates_skipped": dupes }))
}

pub fn renumber(st: &impl Ctx, pid: i64) {
    for (i, r) in items(st, pid).iter().enumerate() {
        if r.i("position") != i as i64 {
            let _ = st.db().execute("UPDATE playlist_items SET position=? WHERE id=?", p![i, r.i("id")]);
        }
    }
}

pub fn dedupe(st: &impl Ctx, pid: i64) -> usize {
    let rows = items(st, pid);
    let (kept, dropped) = dedupe_rows(rows.clone());
    let keep: HashSet<i64> = kept.iter().map(|r| r.i("id")).collect();
    for r in rows.iter().filter(|r| !keep.contains(&r.i("id"))) {
        let _ = st.db().execute("DELETE FROM playlist_items WHERE id=?", p![r.i("id")]);
    }
    renumber(st, pid);
    dropped
}

/// Link items to library tracks (after downloads, rescans, retags…). → newly matched
pub fn rematch(st: &impl Ctx, pid: Option<i64>, only_unmatched: bool) -> usize {
    let mut sql = "SELECT * FROM playlist_items WHERE 1=1".to_string();
    let mut params = vec![];
    if let Some(p) = pid {
        sql += " AND playlist_id=?";
        params.push(json!(p));
    }
    if only_unmatched {
        sql += " AND track_id IS NULL";
    }
    let rows = st.db().query(&sql, params).unwrap_or_default();
    if rows.is_empty() {
        return 0;
    }
    let ix = st.library().songs.get(st.db());
    let mut n = 0;
    let mut changed: Vec<(Option<i64>, Option<String>, i64)> = vec![];
    for r in &rows {
        let hit = ix.find(&item_desc(r));
        let new = hit.as_ref().map(|(h, _)| h.i("id"));
        let cur = Some(r.i("track_id")).filter(|x| *x > 0);
        if new != cur {
            changed.push((new, hit.map(|(_, v)| v.reason), r.i("id")));
            n += new.is_some() as usize;
        }
    }
    drop(ix);
    // every changed item in one transaction (a rematch of all playlists after a scan can touch thousands)
    if !changed.is_empty() {
        let saved = st.db().write_now(|c| c.transaction(|c| {
            for (new, reason, id) in &changed {
                c.execute("UPDATE playlist_items SET track_id=?, match_reason=? WHERE id=?", p![new, reason, id])?;
            }
            Ok::<_, anyhow::Error>(())
        }));
        if let Err(e) = saved {
            tracing::warn!("playlist rematch: {e}");
        }
    }
    // matching can reveal new duplicates (two listings of the same file)
    let pids: HashSet<i64> = rows.iter().map(|r| r.i("playlist_id")).collect();
    for p in pids {
        dedupe(st, p);
    }
    n
}

pub fn merge(st: &impl Ctx, uid: i64, ids: &[i64], name: &str) -> anyhow::Result<Value> {
    let mut tracks = vec![];
    for pid in ids {
        if st.db().one("SELECT 1 FROM playlists WHERE id=? AND user_id=?", p![pid, uid])?.is_none() {
            continue;
        }
        for r in items(st, *pid) {
            let mut v = Value::Object(r.clone());
            v["cover"] = r.get("cover_url").cloned().unwrap_or(Value::Null);
            tracks.push(v);
        }
    }
    let total = tracks.len();
    let mut res = create(st, uid, name, &tracks, "merged", None, &format!("Merged from {} playlists", ids.len()), None, false)?;
    res["total"] = json!(total);
    Ok(res)
}

/// What "Download missing" did, song by song: requested now, already on its way, or not requestable (and why).
#[derive(Debug, Default, Clone, PartialEq, serde::Serialize)]
pub struct MissingReport {
    pub requested: usize,
    pub in_progress: usize,
    pub failed: Vec<MissingFailure>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct MissingFailure {
    pub title: String,
    pub reason: String,
}

/// A request in one of these states ended without the song: the item is requested again.
pub const REREQUEST: [&str; 3] = ["failed", "cancelled", "denied"];

/// [`summary`] of many playlists, with the counts of all of them from one GROUP BY.
pub fn summaries(st: &impl Ctx, pls: &[Row]) -> Vec<Value> {
    let mut counts: HashMap<i64, (i64, i64)> = HashMap::new();
    for chunk in pls.chunks(500) {
        let ids: Vec<Value> = chunk.iter().map(|p| json!(p.i("id"))).collect();
        let sql = format!("SELECT playlist_id, COUNT(*) total, SUM(CASE WHEN track_id IS NOT NULL THEN 1 ELSE 0 END) matched FROM playlist_items
                           WHERE playlist_id IN ({}) GROUP BY playlist_id", encore_library::artists::marks(ids.len()));
        for r in st.db().query(&sql, ids).unwrap_or_default() {
            counts.insert(r.i("playlist_id"), (r.i("total"), r.i("matched")));
        }
    }
    pls.iter().map(|pl| {
        let (total, matched) = counts.get(&pl.i("id")).copied().unwrap_or((0, 0));
        with_counts(pl, total, matched)
    }).collect()
}

/// A playlist as the lists show it: its row, its sync targets read, its song counts.
fn with_counts(pl: &Row, total: i64, matched: i64) -> Value {
    let mut v = Value::Object(pl.clone());
    v["sync_targets"] = serde_json::from_str(&pl.s("sync_targets")).unwrap_or(json!([]));
    v["total"] = json!(total);
    v["matched"] = json!(matched);
    v
}

pub fn summary(st: &impl Ctx, pl: &Row) -> Value {
    let s = st.db().one("SELECT COUNT(*) total, SUM(CASE WHEN track_id IS NOT NULL THEN 1 ELSE 0 END) matched FROM playlist_items WHERE playlist_id=?", p![pl.i("id")])
        .ok().flatten().unwrap_or_default();
    with_counts(pl, s.i("total"), s.i("matched"))
}

/// Pull new tracks from the playlist's source (Spotify, Deezer, a media server …).
pub async fn refresh_followed(st: &impl Ctx, pl: &Row) -> anyhow::Result<(usize, usize)> {
    let data = if ["jellyfin", "plex", "navidrome"].contains(&pl.s("source").as_str()) {
        crate::engine::fetch_remote_playlist(st, pl).await?
    } else {
        st.fetch_link(&pl.s("source_url"), Some(pl.i("user_id"))).await?
    };
    // matching against the library and the inserts on the blocking pool
    let (st, pid, source) = (st.clone(), pl.i("id"), pl.s("source"));
    tokio::task::spawn_blocking(move || add_tracks(&st, pid, data["tracks"].as_array().map(|a| a.as_slice()).unwrap_or(&[]), &source)).await?
}
