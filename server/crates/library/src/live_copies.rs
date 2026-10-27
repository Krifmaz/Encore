//! Live versions to clean up (Library → Live versions): the live albums and the live takes of songs the
//! library holds, each with the studio copy of the same song when the library has one, so the owner can look
//! them over and remove the ones nobody asked for in one go.
//!
//! What counts as live:
//! - an album whose kind is live ([`crate::album_types`]: MusicBrainz's secondary type Live, the files'
//!   RELEASETYPE "album; live", else a title such as "Rodeo (Live at Coachella)"), every song on it;
//! - a song whose title names a live take ("Song (Live)", "Song - Live at Wembley", [`crate::matcher`]'s
//!   "live" version), wherever it is filed.
//!
//! The studio copy of a live song is a song of the same first artist with the same base title, remixer and
//! other versions (a live remix pairs with the studio remix, not the original), that is itself not live. Of
//! several the best copy wins ([`crate::tagger::quality`]: verified, lossless, bitrate, tagged).
//!
//! Nothing here deletes on its own: [`find`] only lists. What the owner ticks (and is still found live) goes:
//! a live album to the Trash (restorable from Files → Trash) once [`repoint_albums`] has moved its songs'
//! playlist entries to their studio copies, a live song filed elsewhere by [`remove_songs`], replaced by its
//! studio copy as the Duplicates tool replaces a copy.
//! "Keep" stores the item's key in `dup_ignore` (the Duplicates tool's table, keys prefixed `live:`) so it is
//! not listed again.
use crate::album_types::Class;
use crate::matcher::{describe, Desc};
use crate::tagger::quality;
use crate::Ctx;
use encore_db::{p, Row, RowExt};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

/// The prefix of this tool's keys in `dup_ignore`, so they never collide with the Duplicates tool's.
pub const KEY_PREFIX: &str = "live:";

/// The columns [`find`] reads for each song (what [`quality`] and the listing need).
const COLUMNS: &str = "id, path, title, artist, album, albumartist, album_id, year, duration, format, bitrate, mb_trackid, verify_status";

/// The ignore key of a live album.
pub fn album_key(album_id: &str) -> String {
    format!("{KEY_PREFIX}album:{album_id}")
}

/// The ignore key of a live song that is not on a live album.
pub fn song_key(id: i64) -> String {
    format!("{KEY_PREFIX}song:{id}")
}

/// Whether a song's title names a live take.
fn titled_live(d: &Desc) -> bool {
    d.versions.contains("live")
}

/// The key a song is paired with its studio copy by: first artist, base title, remixer and every version but
/// live. `None` for a song whose title folds to nothing (it can't be told apart from any other).
fn pair_key(d: &Desc) -> Option<String> {
    if d.base.is_empty() {
        return None;
    }
    let versions: Vec<&str> = d.versions.iter().copied().filter(|v| *v != "live").collect();
    Some(format!("{}|{}|{}|{}", d.artists.first().map(String::as_str).unwrap_or(""), d.base, versions.join(","), d.remixer))
}

/// A studio copy as the listing shows it: `{id, title, album}`.
fn studio_json(r: &Row) -> Value {
    json!({ "id": r.i("id"), "title": r.s("title"), "album": r.s("album") })
}

/// What [`find_in`] lists: the live albums and the live songs filed elsewhere, both best-known first.
#[derive(Debug, Default)]
pub struct Found {
    /// one entry per live album: `{key, album_id, album, artist, year, songs, with_studio, suggested, tracks}`,
    /// `tracks` holding `{id, title, studio}` (`studio` the studio copy or null)
    pub albums: Vec<Value>,
    /// one entry per live song not on a live album: `{key, id, title, artist, album, album_id, studio, suggested}`
    pub songs: Vec<Value>,
}

impl Found {
    /// The listing as the API sends it, with the totals the page's header shows.
    pub fn to_value(&self) -> Value {
        let album_songs: usize = self.albums.iter().map(|a| a["songs"].as_u64().unwrap_or(0) as usize).sum();
        json!({ "albums": self.albums, "songs": self.songs, "total_albums": self.albums.len(), "total_songs": self.songs.len() + album_songs })
    }

    /// Every song id of the live albums `album_ids` and the live songs `song_ids` that this listing holds,
    /// each with its studio copy (or None): what [`repoint_albums`] and [`remove_songs`] act on. Ids not in the listing are left out, so a
    /// stale or forged list can never delete a song that isn't live.
    pub fn plan(&self, album_ids: &[String], song_ids: &[i64]) -> Vec<(i64, Option<i64>)> {
        let want_albums: HashSet<&str> = album_ids.iter().map(String::as_str).collect();
        let want_songs: HashSet<i64> = song_ids.iter().copied().collect();
        let studio = |t: &Value| t["studio"]["id"].as_i64();
        let from_albums = self.albums.iter()
            .filter(|a| a["album_id"].as_str().is_some_and(|id| want_albums.contains(id)))
            .flat_map(|a| a["tracks"].as_array().cloned().unwrap_or_default())
            .filter_map(|t| t["id"].as_i64().map(|id| (id, studio(&t))));
        let from_songs = self.songs.iter()
            .filter(|s| s["id"].as_i64().is_some_and(|id| want_songs.contains(&id)))
            .filter_map(|s| s["id"].as_i64().map(|id| (id, studio(s))));
        // each song once, in the listing's order
        let mut seen = HashSet::new();
        from_albums.chain(from_songs).filter(|(id, _)| seen.insert(*id)).collect()
    }
}

/// The live albums and songs among `rows` (songs with [`COLUMNS`]), given every album's class and the keys
/// the owner chose to keep. Pure: [`find`] reads the rows and classes and calls it.
///
/// A live album is `suggested` (ticked to remove when the page opens) when at least half of its songs have a
/// studio copy in the library, a live song when it has one; the rest are listed unticked, since they may be the
/// only copy of a song.
pub fn find_in(rows: &[Row], classes: &HashMap<String, Class>, kept: &HashSet<String>) -> Found {
    let live_album = |album_id: &str| classes.get(album_id).is_some_and(|c| c.kind == "live");
    let descs: Vec<Desc> = rows.iter().map(|r| describe(&r.s("title"), &r.s("artist"), None, None, None)).collect();
    // the best studio copy of each song: not on a live album, not titled live
    let mut studio: HashMap<String, &Row> = HashMap::new();
    for (r, d) in rows.iter().zip(&descs) {
        if live_album(&r.s("album_id")) || titled_live(d) {
            continue;
        }
        let Some(k) = pair_key(d) else { continue };
        let better = studio.get(&k).is_none_or(|cur| quality(r) > quality(cur));
        if better {
            studio.insert(k, r);
        }
    }
    let studio_of = |d: &Desc| pair_key(d).and_then(|k| studio.get(&k).copied());
    let mut albums: HashMap<String, Vec<(&Row, Option<&Row>)>> = HashMap::new();
    let mut songs: Vec<Value> = vec![];
    for (r, d) in rows.iter().zip(&descs) {
        let album_id = r.s("album_id");
        if live_album(&album_id) {
            albums.entry(album_id).or_default().push((r, studio_of(d)));
        } else if titled_live(d) && !kept.contains(&song_key(r.i("id"))) {
            let s = studio_of(d);
            songs.push(json!({
                "key": song_key(r.i("id")), "id": r.i("id"), "title": r.s("title"), "artist": r.s("artist"), "album": r.s("album"),
                "album_id": album_id, "studio": s.map(studio_json), "suggested": s.is_some(),
            }));
        }
    }
    let mut out: Vec<Value> = albums.into_iter().filter(|(id, _)| !kept.contains(&album_key(id))).map(|(id, mut tracks)| {
        tracks.sort_by_key(|(r, _)| r.i("id"));
        let first = tracks[0].0;
        let with_studio = tracks.iter().filter(|(_, s)| s.is_some()).count();
        json!({
            "key": album_key(&id), "album_id": id, "album": first.s("album"),
            "artist": Some(first.s("albumartist")).filter(|a| !a.is_empty()).unwrap_or_else(|| first.s("artist")),
            "year": first.s("year"), "songs": tracks.len(), "with_studio": with_studio,
            "suggested": with_studio > 0 && with_studio * 2 >= tracks.len(),
            "tracks": tracks.iter().map(|(r, s)| json!({ "id": r.i("id"), "title": r.s("title"), "studio": s.map(studio_json) })).collect::<Vec<_>>(),
        })
    }).collect();
    // by artist, then title, as the other library tools list
    let order = |v: &Value, a: &str, b: &str| (v[a].as_str().unwrap_or("").to_lowercase(), v[b].as_str().unwrap_or("").to_lowercase());
    out.sort_by_key(|a| order(a, "artist", "album"));
    songs.sort_by_key(|s| order(s, "artist", "title"));
    Found { albums: out, songs }
}

/// The keys of live items the owner chose to keep ("Keep" on the page).
fn kept_keys(st: &impl Ctx) -> HashSet<String> {
    st.db().query("SELECT key FROM dup_ignore WHERE key LIKE 'live:%'", p![]).unwrap_or_default().into_iter().map(|r| r.s("key")).collect()
}

/// The live albums and songs of the whole library, given every album's class (`library.types`).
pub fn find(st: &impl Ctx, classes: &HashMap<String, Class>) -> Found {
    let rows = st.db().query(&format!("SELECT {COLUMNS} FROM tracks"), p![]).unwrap_or_default();
    find_in(&rows, classes, &kept_keys(st))
}

/// Never list this album or song (`key`, as [`find`] gave it) again. Keys without [`KEY_PREFIX`] are refused,
/// so this can't hide a group of the Duplicates tool.
pub fn keep(st: &impl Ctx, key: &str) -> anyhow::Result<()> {
    anyhow::ensure!(key.starts_with(KEY_PREFIX), "not a live-versions key");
    st.db().execute("INSERT OR IGNORE INTO dup_ignore VALUES (?,?)", p![key, encore_db::now()])?;
    Ok(())
}

/// Get the live albums `album_ids` ready to go to the Trash: each song's playlist entries (likes too) move to
/// its studio copy first, so the playlists keep playing the song once the live album is gone. → the albums of
/// `album_ids` that [`find`] still lists as live (the only ones the caller may move to the Trash).
pub fn repoint_albums(st: &impl Ctx, classes: &HashMap<String, Class>, album_ids: &[String]) -> anyhow::Result<Vec<String>> {
    let found = find(st, classes);
    let want: HashSet<&str> = album_ids.iter().map(String::as_str).collect();
    let live: Vec<String> = found.albums.iter().filter_map(|a| a["album_id"].as_str()).filter(|id| want.contains(id)).map(String::from).collect();
    let moves: Vec<(i64, i64)> = found.plan(&live, &[]).into_iter().filter_map(|(id, s)| s.map(|s| (id, s))).collect();
    st.db().write_now(|c| c.transaction(|c| {
        for (id, studio) in &moves {
            c.execute("UPDATE playlist_items SET track_id=? WHERE track_id=?", p![studio, id])?;
        }
        Ok::<(), rusqlite::Error>(())
    }))?;
    Ok(live)
}

/// Delete the live songs `song_ids` (filed on albums that aren't live) that a fresh [`find`] still lists, as
/// the Duplicates tool deletes a copy: each song with a studio copy is replaced by it (its playlist entries and
/// who added it move there, [`crate::delete_tracks`]); the others go in one batch. → the songs deleted.
pub fn remove_songs(st: &impl Ctx, classes: &HashMap<String, Class>, song_ids: &[i64]) -> usize {
    let plan = find(st, classes).plan(&[], song_ids);
    let lone: Vec<i64> = plan.iter().filter(|(_, s)| s.is_none()).map(|(id, _)| *id).collect();
    let mut n = crate::delete_tracks(st, &lone, None);
    for (id, studio) in plan {
        if let Some(s) = studio {
            n += crate::delete_tracks(st, &[id], Some(s));
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A song row as [`find_in`] reads it.
    fn row(id: i64, title: &str, artist: &str, album: &str, album_id: &str, format: &str) -> Row {
        serde_json::from_value(json!({ "id": id, "title": title, "artist": artist, "album": album, "albumartist": artist, "album_id": album_id,
                                       "year": "2015", "format": format, "bitrate": 320, "mb_trackid": "", "verify_status": "" })).unwrap()
    }

    /// Classes with the albums `live` live and every other a studio album.
    fn classes(live: &[&str]) -> HashMap<String, Class> {
        live.iter().map(|a| (a.to_string(), Class { kind: "live", ..Default::default() })).collect()
    }

    /// The library: a studio album, its live album (two of three songs have studio copies), a live single
    /// filed on its own, a live remix, a song only known live, and a song whose name only contains "Live".
    fn library() -> Vec<Row> {
        vec![
            row(1, "Antidote", "Travis Scott", "Rodeo", "rodeo", "mp3"),
            row(2, "90210", "Travis Scott", "Rodeo", "rodeo", "mp3"),
            row(3, "Antidote", "Travis Scott", "Rodeo (Live)", "rodeo-live", "flac"),
            row(4, "90210", "Travis Scott", "Rodeo (Live)", "rodeo-live", "flac"),
            row(5, "Intro Jam", "Travis Scott", "Rodeo (Live)", "rodeo-live", "flac"),
            row(6, "Antidote (Live at Rolling Loud)", "Travis Scott", "Antidote (Live)", "antidote-live", "mp3"),
            row(7, "Levels (Skrillex Remix)", "Avicii", "Levels", "levels", "mp3"),
            row(8, "Levels (Skrillex Remix) [Live]", "Avicii", "Tour", "tour", "mp3"),
            row(9, "Encore (Live)", "Nobody", "Bootleg", "bootleg", "mp3"),
            row(10, "Live Forever", "Oasis", "Definitely Maybe", "dm", "mp3"),
        ]
    }

    /// The live album lists every song with its studio copy and is suggested (2 of 3 have one); live songs
    /// elsewhere pair with the same version's studio copy; a song only known live is listed but not ticked; a
    /// title with "Live" in its name is no live take.
    #[test]
    fn live_albums_and_songs_with_their_studio_copies() {
        let f = find_in(&library(), &classes(&["rodeo-live"]), &HashSet::new());
        assert_eq!(f.albums.len(), 1);
        let a = &f.albums[0];
        assert_eq!((a["album_id"].as_str(), a["songs"].as_u64(), a["with_studio"].as_u64(), a["suggested"].as_bool()), (Some("rodeo-live"), Some(3), Some(2), Some(true)));
        let studio: Vec<Option<i64>> = a["tracks"].as_array().unwrap().iter().map(|t| t["studio"]["id"].as_i64()).collect();
        assert_eq!(studio, vec![Some(1), Some(2), None]);
        let songs: Vec<(i64, Option<i64>, bool)> = f.songs.iter().map(|s| (s["id"].as_i64().unwrap(), s["studio"]["id"].as_i64(), s["suggested"] == true)).collect();
        // by artist: Avicii, Nobody, Travis Scott
        assert_eq!(songs, vec![(8, Some(7), true), (9, None, false), (6, Some(1), true)]);
        assert_eq!(f.to_value()["total_songs"], 6);
    }

    /// A live album with mostly songs of its own is listed but not suggested.
    #[test]
    fn a_live_album_of_its_own_songs_is_not_suggested() {
        let rows = vec![row(1, "A", "X", "Studio", "s", "mp3"), row(2, "A", "X", "Live", "l", "mp3"), row(3, "B", "X", "Live", "l", "mp3"), row(4, "C", "X", "Live", "l", "mp3")];
        let f = find_in(&rows, &classes(&["l"]), &HashSet::new());
        assert_eq!((f.albums[0]["with_studio"].as_u64(), f.albums[0]["suggested"].as_bool()), (Some(1), Some(false)));
    }

    /// Kept albums and songs are not listed again.
    #[test]
    fn kept_items_stay_off_the_list() {
        let kept: HashSet<String> = [album_key("rodeo-live"), song_key(6)].into_iter().collect();
        let f = find_in(&library(), &classes(&["rodeo-live"]), &kept);
        assert!(f.albums.is_empty());
        assert_eq!(f.songs.iter().map(|s| s["id"].as_i64().unwrap()).collect::<Vec<_>>(), vec![8, 9]);
    }

    /// The best studio copy wins: lossless over MP3.
    #[test]
    fn the_best_studio_copy_is_the_replacement() {
        let rows = vec![row(1, "Song", "X", "A", "a", "mp3"), row(2, "Song", "X", "A (Deluxe)", "b", "flac"), row(3, "Song (Live)", "X", "Tour", "t", "mp3")];
        let f = find_in(&rows, &HashMap::new(), &HashSet::new());
        assert_eq!(f.songs[0]["studio"]["id"], 2);
    }

    /// The plan holds only what the listing has: the ticked album's songs and ticked songs, each once, with
    /// their studio copies; an id that isn't live (a studio song, an unknown album) is never in it.
    #[test]
    fn the_plan_only_deletes_listed_live_songs() {
        let f = find_in(&library(), &classes(&["rodeo-live"]), &HashSet::new());
        let plan = f.plan(&["rodeo-live".into(), "rodeo".into()], &[9, 1, 10, 9]);
        assert_eq!(plan, vec![(3, Some(1)), (4, Some(2)), (5, None), (9, None)]);
        assert!(f.plan(&[], &[]).is_empty());
    }
}
