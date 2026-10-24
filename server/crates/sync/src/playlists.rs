//! Playlists & liked songs: import, match against the library, de-duplicate, merge, request missing.
//!
//! `playlists.kind`: playlist | liked | linked (someone else's playlist added through its share link;
//! read-only, it has no items of its own: the API shows the owner's playlist through the link).
use encore_db::{now, Row, RowExt};
use encore_library::matcher::{altered_words, compare, describe, key, parse_title, sim, Desc, LibraryIndex, Verdict};
use encore_db::p;
use crate::Ctx;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

/// User `uid`'s Liked Songs playlist (created the first time).
pub fn get_liked(st: &impl Ctx, uid: i64) -> anyhow::Result<Row> {
    if let Some(r) = st.db().one("SELECT * FROM playlists WHERE user_id=? AND kind='liked'", p![uid])? {
        return Ok(r);
    }
    let t = now();
    let id = st.db().execute("INSERT INTO playlists (user_id, name, kind, source, sync_targets, created_at, updated_at) VALUES (?,?,?,?,?,?,?)",
        p![uid, "Liked Songs", "liked", "manual", "[]", t, t])?;
    Ok(st.db().one("SELECT * FROM playlists WHERE id=?", p![id])?.unwrap())
}

/// The songs of playlist `pid` in order.
pub fn items(st: &impl Ctx, pid: i64) -> Vec<Row> {
    st.db().query("SELECT * FROM playlist_items WHERE playlist_id=? ORDER BY position, id", p![pid]).unwrap_or_default()
}

/// A row's text field, None when it is empty.
pub fn opt(r: &Row, k: &str) -> Option<String> {
    Some(r.s(k)).filter(|x| !x.is_empty())
}

/// What identifies a playlist song when it is matched (title, artist, length, ISRC, MusicBrainz id).
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
    let d = item_desc(r);
    // a title that is only symbols ("★", "...") folds to nothing: its own text tells such songs apart
    if d.base.is_empty() {
        return format!("k:{}|{}", key(&d), r.s("title").trim().to_lowercase());
    }
    // "Sped Up" and "Slowed" are both re-pitched versions, and two songs
    match altered_words(&r.s("title")) {
        Some(w) => format!("k:{}|{w}", key(&d)),
        None => format!("k:{}", key(&d)),
    }
}

/// The numbers in a folded title ("part 2", "teardrop 2nd", "interlude ii"): songs whose numbers differ are
/// different songs.
fn numbers(base: &str) -> Vec<&str> {
    base.split_whitespace().filter(|w| w.chars().any(|c| c.is_ascii_digit()) || ["i", "ii", "iii", "iv", "v", "vi", "vii", "viii", "ix", "x"].contains(w)).collect()
}

/// A bracketed part of a title, in ASCII or full-width brackets.
static BRACKETED: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| regex::Regex::new(r"[\(\[\{（【][^\)\]\}）】]*[\)\]\}）】]").unwrap());

/// A title without its brackets, folded: "INTERNET YAMERO (Official Music Video) [Theme for …]" → "internet yamero".
fn head(title: &str) -> String {
    parse_title(&BRACKETED.replace_all(title, " ")).0
}

/// Two rows the matcher calls one song that a playlist still keeps apart: their numbers, their re-pitched
/// versions ("Sped Up" against "Slowed") or the titles outside their brackets differ.
fn kept_apart(a: &Row, b: &Row, da: &Desc, db: &Desc) -> bool {
    let (ha, hb) = (head(&a.s("title")), head(&b.s("title")));
    numbers(&da.base) != numbers(&db.base) || altered_words(&a.s("title")) != altered_words(&b.s("title"))
        || (!ha.is_empty() && !hb.is_empty() && sim(&ha, &hb) < 0.85)
}

/// Drop rows that are the same song+version as an earlier row (version-aware).
pub fn dedupe_rows(rows: Vec<Row>) -> (Vec<Row>, usize) {
    let (mut out, mut seen, mut kept): (Vec<Row>, HashSet<String>, Vec<(Desc, Row)>) = (vec![], HashSet::new(), vec![]);
    let mut dropped = 0;
    for r in rows {
        let k = identity(&r, true);
        if seen.contains(&k) {
            dropped += 1;
            continue;
        }
        let d = item_desc(&r);
        let pre = |x: &str| x.chars().take(4).collect::<String>();
        if r.i("track_id") == 0 && !d.base.is_empty()
            && kept.iter().any(|(kd, kr)| pre(&kd.base) == pre(&d.base) && compare(&d, kd, true).matched && !kept_apart(&r, kr, &d, kd)) {
            dropped += 1;
            continue;
        }
        seen.insert(k);
        // an item with a library match also blocks its other identities (isrc/mbid/key)
        seen.insert(identity(&r, false));
        kept.push((d, r.clone()));
        out.push(r);
    }
    (out, dropped)
}

/// The match reason of an item linked to a library song by hand ([`set_match`]): the re-check of the
/// library leaves it as it is.
pub const MATCHED_BY_HAND: &str = "matched by hand";

/// The match reason of an item unlinked by hand ([`set_match`] with no song): the re-check of the library
/// doesn't link it again.
pub const UNLINKED_BY_HAND: &str = "unlinked by hand";

/// Was this item linked or unlinked by hand? The re-check of the library leaves such an item alone.
pub fn by_hand(r: &Row) -> bool {
    matches!(r.s("match_reason").as_str(), MATCHED_BY_HAND | UNLINKED_BY_HAND)
}

/// The library song for a playlist item: the version-aware match of its title and artist, and for an
/// item whose artist is only a guess (`artist_guessed`: a video's uploader) also the song of that title
/// and length by one artist ([`LibraryIndex::find_by_title`]).
pub fn find_item<'a>(ix: &'a LibraryIndex, r: &Row) -> Option<(&'a Row, Verdict)> {
    let d = item_desc(r);
    ix.find(&d).or_else(|| if r.i("artist_guessed") == 1 { ix.find_by_title(&d) } else { None })
}

/// Link rows to library tracks. → how many matched. A row whose guessed artist (an uploader) found its
/// song by title takes the song's artist, and isn't a guess any more.
pub fn match_rows(rows: &mut [Row], ix: &LibraryIndex) -> usize {
    let mut n = 0;
    for r in rows.iter_mut() {
        match find_item(ix, r) {
            Some((hit, v)) => {
                r.insert("track_id".into(), json!(hit.i("id")));
                if r.i("artist_guessed") == 1 {
                    r.insert("artist".into(), json!(hit.s("artist")));
                    r.insert("artist_guessed".into(), json!(0));
                }
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

/// Imported/fetched track (title, artist, album, duration, isrc, mbid, cover, source_id[, track_id,
/// artist_guessed, source_title]) → playlist row.
fn row_from(t: &Value, source: &str) -> Row {
    let mut r = Row::new();
    for (k, src) in [("title", "title"), ("artist", "artist"), ("album", "album"), ("duration", "duration"), ("isrc", "isrc"), ("mbid", "mbid"),
                     ("cover_url", "cover"), ("source_id", "source_id"), ("track_id", "track_id"),
                     ("artist_guessed", "artist_guessed"), ("source_title", "source_title")] {
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
        st.db().execute("INSERT INTO playlist_items (playlist_id, position, title, artist, album, duration, isrc, mbid, cover_url, source, source_id, track_id, match_reason,
                       artist_guessed, source_title, added_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            p![pid, pos + i as i64, r["title"], r["artist"], r["album"], r["duration"], r["isrc"], r["mbid"], r["cover_url"], r["source"], r["source_id"],
               r.get("track_id").filter(|v| v.as_i64().is_some_and(|x| x > 0)).cloned().unwrap_or(Value::Null), r.get("match_reason").cloned().unwrap_or(Value::Null),
               r.get("artist_guessed").cloned().unwrap_or(Value::Null), r.get("source_title").cloned().unwrap_or(Value::Null), t])?;
    }
    st.db().execute("UPDATE playlists SET updated_at=? WHERE id=?", p![t, pid])?;
    Ok((new_rows.len(), dropped))
}

/// Add library songs `ids` to playlist `pid`: how many were added and how many were already in it.
pub fn add_library_tracks(st: &impl Ctx, pid: i64, ids: &[i64]) -> anyhow::Result<(usize, usize)> {
    let tracks: Vec<Value> = ids.iter().map(|i| json!({ "track_id": i, "title": "" })).collect();
    add_tracks(st, pid, &tracks, "library")
}

/// Create a playlist for user `uid` with `tracks` (`follow`: keep it in sync with `source_url`); the new playlist.
#[allow(clippy::too_many_arguments)]
pub fn create(st: &impl Ctx, uid: i64, name: &str, tracks: &[Value], source: &str, source_url: Option<&str>, description: &str, cover: Option<&str>, follow: bool) -> anyhow::Result<Value> {
    let t = now();
    let pid = st.db().execute("INSERT INTO playlists (user_id, name, description, kind, source, source_url, follow, sync_targets, cover_url, created_at, updated_at)
                             VALUES (?,?,?,?,?,?,?,?,?,?,?)", p![uid, name, description, "playlist", source, source_url, follow as i64, "[]", cover, t, t])?;
    let (added, dupes) = add_tracks(st, pid, tracks, source)?;
    Ok(json!({ "id": pid, "added": added, "duplicates_skipped": dupes }))
}

/// Number the songs of playlist `pid` 0, 1, 2 … again in their order.
pub fn renumber(st: &impl Ctx, pid: i64) {
    for (i, r) in items(st, pid).iter().enumerate() {
        if r.i("position") != i as i64 {
            let _ = st.db().execute("UPDATE playlist_items SET position=? WHERE id=?", p![i, r.i("id")]);
        }
    }
}

/// Drop the songs of playlist `pid` that are in it twice; how many went.
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

/// One item [`rematch`] changes: (its new track, the match reason, the song's artist when the item's was
/// a guess, the item's id).
type Rematched = (Option<i64>, Option<String>, Option<String>, i64);

/// Link items to library tracks (after downloads, rescans, retags…). → newly matched. Items linked or
/// unlinked by hand ([`by_hand`]) stay as they are; an item whose guessed artist found its song by title
/// takes the song's artist.
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
    let mut changed: Vec<Rematched> = vec![];
    for r in rows.iter().filter(|r| !by_hand(r)) {
        let hit = find_item(&ix, r);
        let new = hit.as_ref().map(|(h, _)| h.i("id"));
        let cur = Some(r.i("track_id")).filter(|x| *x > 0);
        if new != cur {
            let artist = hit.as_ref().filter(|_| r.i("artist_guessed") == 1).map(|(h, _)| h.s("artist"));
            changed.push((new, hit.map(|(_, v)| v.reason), artist, r.i("id")));
            n += new.is_some() as usize;
        }
    }
    drop(ix);
    // every changed item in one transaction (a rematch of all playlists after a scan can touch thousands)
    if !changed.is_empty() {
        let saved = st.db().write_now(|c| c.transaction(|c| {
            for (new, reason, artist, id) in &changed {
                c.execute("UPDATE playlist_items SET track_id=?, match_reason=? WHERE id=?", p![new, reason, id])?;
                if let Some(a) = artist {
                    c.execute("UPDATE playlist_items SET artist=?, artist_guessed=0 WHERE id=?", p![a, id])?;
                }
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

/// Link a playlist item to a library song by hand, or unlink it (`track_id` None), for an item the matching
/// got wrong or couldn't find. A linked item takes the song's title, artist, album and length; the name it
/// had before is kept in `source_title` (when it had none there yet), so the page can show what it was. The
/// re-check of the library leaves the item as it is from then on ([`by_hand`]). Linking can make the item a
/// duplicate of another one in the playlist, which goes then. → (found the item, duplicates removed)
pub fn set_match(st: &impl Ctx, pid: i64, iid: i64, track_id: Option<i64>) -> anyhow::Result<(bool, usize)> {
    let Some(it) = st.db().one("SELECT * FROM playlist_items WHERE id=? AND playlist_id=?", p![iid, pid])? else { return Ok((false, 0)) };
    let Some(tid) = track_id else {
        st.db().execute("UPDATE playlist_items SET track_id=NULL, match_reason=? WHERE id=?", p![UNLINKED_BY_HAND, iid])?;
        return Ok((true, 0));
    };
    let Some(t) = st.db().one("SELECT title, artist, album, duration FROM tracks WHERE id=?", p![tid])? else { anyhow::bail!("That song isn't in the library") };
    // what the item was called before (its source's name for it), shown next to the song it is now
    let was = opt(&it, "source_title").unwrap_or_else(|| match (it.s("artist"), it.s("title")) {
        (a, t) if a.is_empty() => t,
        (a, t) => format!("{a} - {t}"),
    });
    st.db().execute("UPDATE playlist_items SET track_id=?, match_reason=?, title=?, artist=?, album=?, duration=?, artist_guessed=0, source_title=? WHERE id=?",
        p![tid, MATCHED_BY_HAND, t["title"], t["artist"], t["album"], t["duration"], was, iid])?;
    st.db().execute("UPDATE playlists SET updated_at=? WHERE id=?", p![now(), pid])?;
    Ok((true, dedupe(st, pid)))
}

/// The songs of the user's own playlists and Liked Songs among `ids` (not someone else's added from a link: it
/// has no items of its own, and it isn't the user's to merge), in order, as tracks for [`add_tracks`].
fn merge_tracks(st: &impl Ctx, uid: i64, ids: &[i64]) -> anyhow::Result<Vec<Value>> {
    let mut tracks = vec![];
    for pid in ids {
        if st.db().one("SELECT 1 FROM playlists WHERE id=? AND user_id=? AND kind IN ('playlist','liked')", p![pid, uid])?.is_none() {
            continue;
        }
        for r in items(st, *pid) {
            let mut v = Value::Object(r.clone());
            v["cover"] = r.get("cover_url").cloned().unwrap_or(Value::Null);
            tracks.push(v);
        }
    }
    Ok(tracks)
}

/// Merge the user's playlists `ids` into a new playlist `name` (see [`merge_tracks`]); its answer, with the song total.
pub fn merge(st: &impl Ctx, uid: i64, ids: &[i64], name: &str) -> anyhow::Result<Value> {
    let tracks = merge_tracks(st, uid, ids)?;
    let total = tracks.len();
    let mut res = create(st, uid, name, &tracks, "merged", None, &format!("Merged from {} playlists", ids.len()), None, false)?;
    res["total"] = json!(total);
    Ok(res)
}

/// Merge into…: the songs of `ids` are added to the end of playlist `into` (Liked Songs too), skipping what it
/// already has (same song + version). Songs not in the library stay as missing items, so they can still be
/// downloaded (and Liked Songs' auto-download picks them up). The caller checks that `into` takes songs.
pub fn merge_into(st: &impl Ctx, uid: i64, ids: &[i64], into: i64) -> anyhow::Result<Value> {
    let ids: Vec<i64> = ids.iter().copied().filter(|i| *i != into).collect();
    let tracks = merge_tracks(st, uid, &ids)?;
    let total = tracks.len();
    let (added, dupes) = add_tracks(st, into, &tracks, "merged")?;
    Ok(json!({ "id": into, "added": added, "duplicates_skipped": dupes, "total": total }))
}

/// What "Download missing" did, song by song: requested now, already on its way, or not requestable (and why).
#[derive(Debug, Default, Clone, PartialEq, serde::Serialize)]
pub struct MissingReport {
    /// songs requested now
    pub requested: usize,
    /// songs already on their way
    pub in_progress: usize,
    /// songs that could not be requested
    pub failed: Vec<MissingFailure>,
}

/// A song "Download missing" could not request.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct MissingFailure {
    /// the song as listed
    pub title: String,
    /// why not
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

/// Playlist `pl` as the lists show it, with its song counts.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn row(title: &str, artist: &str) -> Row {
        let mut r = Row::new();
        r.insert("title".into(), json!(title));
        r.insert("artist".into(), json!(artist));
        r
    }

    #[test]
    fn dedupe_keeps_numbered_parts_and_symbol_titles() {
        let rows = vec![row("Song Part 1", "A"), row("Song Part 2", "A"), row("Interlude I", "B"), row("Interlude II", "B"),
                        row("★", "C"), row("☆", "C"), row("Song Part 1", "A"), row("Hello", "D"), row("Hello (feat. E)", "D"),
                        row("Tek It (Slowed)", "F"), row("Tek It (Sped Up)", "F"), row("teardrop (Prod. G)", "G"), row("teardrop 2nd (Prod. G)", "G"),
                        row("INTERNET YAMERO (Official Music Video) [Theme for NEEDY GIRL OVERDOSE]", "H"),
                        row("INTERNET OVERDOSE (Official Music Video) [Theme for NEEDY GIRL OVERDOSE]", "H"), row("Paris (Official Video)", "I"), row("Paris", "I")];
        let (kept, dropped) = dedupe_rows(rows);
        assert_eq!(dropped, 3);
        assert_eq!(kept.len(), 14);
    }
}
