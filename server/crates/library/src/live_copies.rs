//! Live versions to clean up (Library → Live versions): the live albums and the live takes of songs the
//! library holds, each with the studio copy of the same song when the library has one, so the owner can look
//! them over and remove the ones nobody asked for in one go.
//!
//! What counts as live (each item says why, [`Reason`]):
//! - an album whose kind is live ([`crate::album_types`]); one MusicBrainz or the files' RELEASETYPE call Live
//!   among other types (a live compilation is a compilation there, but still live); one whose title or folder
//!   reads as a concert ([`live_words`]: "(Live at …)", "MTV Unplugged", "BBC Sessions", "In Concert", a tour,
//!   a festival or famous venue, a concert date); one most of whose songs are titled live – every song on it;
//! - a song whose title or file name reads as a live take ([`crate::matcher`]'s "live" version, or
//!   [`live_words`]), wherever it is filed.
//!
//! The words are wide on purpose: an item is only ticked when the library has its studio copy, so a song
//! read as live by mistake is shown unticked, never removed without the owner ticking it.
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
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

/// The prefix of this tool's keys in `dup_ignore`, so they never collide with the Duplicates tool's.
pub const KEY_PREFIX: &str = "live:";

/// The columns [`find`] reads for each song (what [`quality`] and the listing need).
const COLUMNS: &str = "id, path, title, artist, album, albumartist, album_id, year, duration, format, bitrate, mb_trackid, verify_status, releasetype";

/// Why something is listed as live, as the page shows it.
pub mod reason {
    /// the album's type is live (MusicBrainz, the files' tags or its title, [`crate::album_types`])
    pub const ALBUM_TYPE: &str = "Live album";
    /// MusicBrainz or the files' RELEASETYPE call it Live among other types (a live compilation)
    pub const TAGGED: &str = "Tagged live";
    /// the title reads as a live recording ("(Live)", "Live at …", "Unplugged", "In Concert", "Sessions")
    pub const TITLE: &str = "Title says live";
    /// a tour, a festival or a famous venue in the title
    pub const VENUE: &str = "Tour, festival or venue";
    /// a concert date in the title ("1994-04-08", "8/4/94")
    pub const DATE: &str = "Concert date";
    /// the folder or file name reads as live
    pub const FILE: &str = "Folder or file name says live";
    /// most of the album's songs are titled live
    pub const SONGS: &str = "Most songs are live";
}

/// Words that make a title (or folder, or file name) a live recording, with the reason each gives. The
/// word "live" alone in a name ("Live Forever", "Live Through This", "Alive") is not one: it has to be in
/// brackets, after a dash, or followed by at / from / in / on / @.
static LIVE_WORDS: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    let re = |p: &str| Regex::new(&format!("(?i){p}")).unwrap();
    vec![
        (re(r"[\(\[\{][^\)\]\}]*\blive\b[^\)\]\}]*[\)\]\}]|\s[-–—:~]\s*live\b|^live\s*[-:@]|\blive\s+(at|from|in|on|@|aus|au|en|em|en vivo)\b|@\s*\w|\b(en|ao) vivo\b|\ben directo\b|\bin concert\b|\bconcert\b|\bunplugged\b|\bmtv\b|\b(bbc|peel|maida vale|radio ?1|kexp|tiny desk|spotify|apple music|amazon music|audiotree|live|studio brussel|triple j|like a version|sofar|colors) sessions?\b|\bsessions?\s*@|\blive (session|version|recording|performance|acoustic|edit|take|mix|set)\b|\bbootleg\b|\bsoundboard\b|\baudience recording\b"), reason::TITLE),
        // a tour or festival only in brackets, with a year or as "world tour" ("Tour de France", Santana's
        // "Festival" are studio albums); venues and festivals by name, the ones that don't name anything else
        (re(r"[\(\[][^\)\]]*\b(tour|festival|fest)\b[^\)\]]*[\)\]]|\b(world|arena|stadium|farewell|reunion|concert|live) tour\b|\b(tour|festival)\s*'?(19|20)?\d\d\b|\b(coachella|glastonbury|lollapalooza|rolling loud|bonnaroo|rock am ring|rock in rio|austin city limits|sxsw|roskilde|pinkpop|fuji rock|summer sonic|osheaga|governors ball|outside lands|tomorrowland|budokan|wembley|madison square garden|red rocks|royal albert hall|hammersmith odeon|hollywood bowl|carnegie hall|fillmore|jools holland|saturday night live)\b"), reason::VENUE),
        (re(r"\b(19|20)\d\d[-./](0?[1-9]|1[0-2])[-./](0?[1-9]|[12]\d|3[01])\b|\b(0?[1-9]|[12]\d|3[01])[-./](0?[1-9]|1[0-2])[-./]((19|20)?\d\d)\b"), reason::DATE),
    ]
});

/// Why a title, folder or file name reads as a live recording, or None. Wide on purpose (see the module docs).
pub fn live_words(text: &str) -> Option<&'static str> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    LIVE_WORDS.iter().find(|(r, _)| r.is_match(t)).map(|(_, why)| *why)
}

/// Why a song reads as a live take by itself (its title or its file name), or None.
fn song_reason(r: &Row, d: &Desc) -> Option<&'static str> {
    if titled_live(d) {
        return Some(reason::TITLE);
    }
    if let Some(why) = live_words(&r.s("title")) {
        return Some(why);
    }
    let path = r.s("path");
    let stem = std::path::Path::new(&path).file_stem().map(|x| x.to_string_lossy().into_owned()).unwrap_or_default();
    // the file name without the song's own title (a song called "Concert" isn't a concert)
    let stem = stem.replace(&r.s("title"), " ");
    live_words(&stem).map(|_| reason::FILE)
}

/// What is known about an album beyond its class: MusicBrainz's or the stores' secondary types (`album_types`).
pub type Secondary = HashMap<String, Vec<String>>;

/// Why an album reads as live, or None: its class, its types, its title, its folder, its songs.
fn album_reason(album_id: &str, rows: &[(&Row, Option<&'static str>)], classes: &HashMap<String, Class>, secondary: &Secondary) -> Option<&'static str> {
    if classes.get(album_id).is_some_and(|c| c.kind == "live") {
        return Some(reason::ALBUM_TYPE);
    }
    let live_type = |t: &str| t.split([';', '/', ',', '\0']).any(|w| w.trim().eq_ignore_ascii_case("live"));
    if secondary.get(album_id).is_some_and(|v| v.iter().any(|x| live_type(x))) || rows.iter().any(|(r, _)| live_type(&r.s("releasetype"))) {
        return Some(reason::TAGGED);
    }
    let first = rows.first()?.0;
    if let Some(why) = live_words(&first.s("album")) {
        return Some(why);
    }
    // the album's folder ("…/Artist/Rodeo (Live at Coachella 2016)/01.flac")
    let path = first.s("path");
    let folder = std::path::Path::new(&path).parent().and_then(|p| p.file_name()).map(|x| x.to_string_lossy().into_owned()).unwrap_or_default();
    if !folder.is_empty() && folder != first.s("album") && live_words(&folder).is_some() {
        return Some(reason::FILE);
    }
    let n = rows.iter().filter(|(_, why)| why.is_some()).count();
    // most of 3 or more songs, or every one of 1-2
    ((rows.len() <= 2 && n == rows.len()) || (rows.len() > 2 && n * 2 > rows.len())).then_some(reason::SONGS)
}

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

/// The live albums and songs among `rows` (songs with [`COLUMNS`]), given every album's class, the albums'
/// secondary types and the keys the owner chose to keep. Pure: [`find`] reads them and calls it.
///
/// A live album is `suggested` (ticked to remove when the page opens) when at least half of its songs have a
/// studio copy in the library, a live song when it has one; the rest are listed unticked (each says "only
/// copy"), so they can still be ticked and removed. Every item carries its `reason`.
pub fn find_in(rows: &[Row], classes: &HashMap<String, Class>, secondary: &Secondary, kept: &HashSet<String>) -> Found {
    let descs: Vec<Desc> = rows.iter().map(|r| describe(&r.s("title"), &r.s("artist"), None, None, None)).collect();
    let song_why: Vec<Option<&'static str>> = rows.iter().zip(&descs).map(|(r, d)| song_reason(r, d)).collect();
    // the albums, their songs with each song's own reason
    let mut by_album: HashMap<String, Vec<(&Row, Option<&'static str>)>> = HashMap::new();
    for (r, why) in rows.iter().zip(&song_why) {
        by_album.entry(r.s("album_id")).or_default().push((r, *why));
    }
    let album_why: HashMap<&str, &'static str> = by_album.iter().filter_map(|(id, songs)| album_reason(id, songs, classes, secondary).map(|w| (id.as_str(), w))).collect();
    let is_live = |i: usize| song_why[i].is_some() || album_why.contains_key(rows[i].s("album_id").as_str());
    // the best studio copy of each song: not live in any of the ways above
    let mut studio: HashMap<String, &Row> = HashMap::new();
    for (i, (r, d)) in rows.iter().zip(&descs).enumerate() {
        if is_live(i) {
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
    for (i, (r, d)) in rows.iter().zip(&descs).enumerate() {
        let album_id = r.s("album_id");
        if album_why.contains_key(album_id.as_str()) {
            albums.entry(album_id).or_default().push((r, studio_of(d)));
        } else if let Some(why) = song_why[i].filter(|_| !kept.contains(&song_key(r.i("id")))) {
            let s = studio_of(d);
            songs.push(json!({
                "key": song_key(r.i("id")), "id": r.i("id"), "title": r.s("title"), "artist": r.s("artist"), "album": r.s("album"),
                "album_id": album_id, "studio": s.map(studio_json), "suggested": s.is_some(), "reason": why,
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
            "suggested": with_studio > 0 && with_studio * 2 >= tracks.len(), "reason": album_why[id.as_str()],
            "tracks": tracks.iter().map(|(r, s)| json!({ "id": r.i("id"), "title": r.s("title"), "studio": s.map(studio_json) })).collect::<Vec<_>>(),
        })
    }).collect();
    // by artist, then title, as the other library tools list
    let order = |v: &Value, a: &str, b: &str| (v[a].as_str().unwrap_or("").to_lowercase(), v[b].as_str().unwrap_or("").to_lowercase());
    out.sort_by_key(|a| order(a, "artist", "album"));
    songs.sort_by_key(|s| order(s, "artist", "title"));
    Found { albums: out, songs }
}

/// Every album's secondary types as `album_types` keeps them (MusicBrainz's, or a store's record type).
fn secondary_types(st: &impl Ctx) -> Secondary {
    st.db().query("SELECT album_id, release_secondary FROM album_types", p![]).unwrap_or_default().into_iter()
        .map(|r| (r.s("album_id"), serde_json::from_str::<Vec<String>>(&r.s("release_secondary")).unwrap_or_default()))
        .filter(|(_, v)| !v.is_empty())
        .collect()
}

/// The keys of live items the owner chose to keep ("Keep" on the page).
fn kept_keys(st: &impl Ctx) -> HashSet<String> {
    st.db().query("SELECT key FROM dup_ignore WHERE key LIKE 'live:%'", p![]).unwrap_or_default().into_iter().map(|r| r.s("key")).collect()
}

/// The live albums and songs of the whole library, given every album's class (`library.types`).
pub fn find(st: &impl Ctx, classes: &HashMap<String, Class>) -> Found {
    let rows = st.db().query(&format!("SELECT {COLUMNS} FROM tracks"), p![]).unwrap_or_default();
    find_in(&rows, classes, &secondary_types(st), &kept_keys(st))
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
                                       "year": "2015", "format": format, "bitrate": 320, "mb_trackid": "", "verify_status": "",
                                       "path": format!("/m/{artist}/{album}/{id:02} {title}.{format}"), "releasetype": "" })).unwrap()
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
            row(6, "Antidote (Live at Rolling Loud)", "Travis Scott", "Singles", "singles", "mp3"),
            row(13, "Butterfly Effect", "Travis Scott", "Singles", "singles", "mp3"),
            row(7, "Levels (Skrillex Remix)", "Avicii", "Levels", "levels", "mp3"),
            row(8, "Levels (Skrillex Remix) [Live]", "Avicii", "Odds", "odds", "mp3"),
            row(9, "Encore (Live)", "Nobody", "Rarities", "rarities", "mp3"),
            row(10, "Live Forever", "Oasis", "Definitely Maybe", "dm", "mp3"),
            row(11, "Bonus", "Avicii", "Odds", "odds", "mp3"),
            row(12, "Other", "Nobody", "Rarities", "rarities", "mp3"),
        ]
    }

    /// The live album lists every song with its studio copy and is suggested (2 of 3 have one); live songs
    /// elsewhere pair with the same version's studio copy; a song only known live is listed but not ticked; a
    /// title with "Live" in its name is no live take.
    #[test]
    fn live_albums_and_songs_with_their_studio_copies() {
        let f = find_in(&library(), &classes(&["rodeo-live"]), &HashMap::new(), &HashSet::new());
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
        let f = find_in(&rows, &classes(&["l"]), &HashMap::new(), &HashSet::new());
        assert_eq!((f.albums[0]["with_studio"].as_u64(), f.albums[0]["suggested"].as_bool()), (Some(1), Some(false)));
    }

    /// Kept albums and songs are not listed again.
    #[test]
    fn kept_items_stay_off_the_list() {
        let kept: HashSet<String> = [album_key("rodeo-live"), song_key(6)].into_iter().collect();
        let f = find_in(&library(), &classes(&["rodeo-live"]), &HashMap::new(), &kept);
        assert!(f.albums.is_empty());
        assert_eq!(f.songs.iter().map(|s| s["id"].as_i64().unwrap()).collect::<Vec<_>>(), vec![8, 9]);
    }

    /// The best studio copy wins: lossless over MP3.
    #[test]
    fn the_best_studio_copy_is_the_replacement() {
        let rows = vec![row(1, "Song", "X", "A", "a", "mp3"), row(2, "Song", "X", "A (Deluxe)", "b", "flac"), row(3, "Song (Live)", "X", "Odds", "t", "mp3"), row(4, "Other", "X", "Odds", "t", "mp3")];
        let f = find_in(&rows, &HashMap::new(), &HashMap::new(), &HashSet::new());
        assert_eq!(f.songs[0]["studio"]["id"], 2);
    }

    /// The plan holds only what the listing has: the ticked album's songs and ticked songs, each once, with
    /// their studio copies; an id that isn't live (a studio song, an unknown album) is never in it.
    #[test]
    fn the_plan_only_deletes_listed_live_songs() {
        let f = find_in(&library(), &classes(&["rodeo-live"]), &HashMap::new(), &HashSet::new());
        let plan = f.plan(&["rodeo-live".into(), "rodeo".into()], &[9, 1, 10, 9]);
        assert_eq!(plan, vec![(3, Some(1)), (4, Some(2)), (5, None), (9, None)]);
        assert!(f.plan(&[], &[]).is_empty());
    }

    /// The wider words: brackets, dashes, "Live at …", concert words, sessions, a tour or festival in brackets or
    /// with a year, famous venues, concert dates; never a title that only contains the word ("Live Forever",
    /// "Alive", "Live Through This") nor a studio album named like a tour or festival.
    #[test]
    fn live_words_read_concerts_not_names() {
        for t in ["Rodeo (Live)", "Song - Live at Wembley", "Live at the Apollo", "MTV Unplugged in New York", "BBC Sessions", "In Concert",
                  "Song [Live from Spotify Sessions]", "Hits (Tour Edition)", "Astroworld Tour 2019", "Song (Coachella 2017)", "Live @ Red Rocks",
                  "Song - 1994-04-08", "Song (Bootleg)", "Song (Live Version)", "Live in Paris", "Song ~ Live"] {
            assert!(live_words(t).is_some(), "{t}");
        }
        for t in ["Live Forever", "Alive 2007", "Live Through This", "Tour de France", "Festival", "Song", "Living Proof", "Liveline", "Deliver"] {
            assert_eq!(live_words(t), None, "{t}");
        }
        assert_eq!(live_words("Song (Coachella 2017)"), Some(reason::VENUE));
        assert_eq!(live_words("Song - 1994.04.08"), Some(reason::DATE));
    }

    /// What the first pass missed: a live compilation (MusicBrainz says Compilation first), a RELEASETYPE tag, an
    /// album titled as a concert that MusicBrainz's facts call studio, a live folder, an album of live songs, a
    /// live song by its file name. Each says why, and none of them is anyone's studio copy.
    #[test]
    fn live_albums_by_types_titles_folders_and_songs() {
        let mut tagged = row(4, "Hit", "X", "Shows", "tagged", "flac");
        tagged.insert("releasetype".into(), json!("album; live"));
        let mut folder = row(5, "Hit", "X", "Something", "folder", "flac");
        folder.insert("path".into(), json!("/m/X/Something (Live at Budokan)/01 Hit.flac"));
        let mut file = row(9, "Other", "X", "Mixed", "mixed", "flac");
        file.insert("path".into(), json!("/m/X/Mixed/02 Other (live 1999).flac"));
        let rows = vec![
            row(1, "Hit", "X", "Studio", "studio", "mp3"), row(2, "Other", "X", "Studio", "studio", "mp3"),
            row(3, "Hit", "X", "Greatest Hits Live", "comp", "flac"), tagged, folder,
            row(6, "Hit (Live)", "X", "Mostly Live", "most", "flac"), row(7, "Other (Live)", "X", "Mostly Live", "most", "flac"), row(8, "Intro", "X", "Mostly Live", "most", "flac"),
            file, row(10, "Third", "X", "Mixed", "mixed", "flac"), row(11, "Hit", "X", "Hit (Live at Wembley)", "mbstudio", "flac"),
        ];
        let secondary: Secondary = [("comp".to_string(), vec!["Compilation".to_string(), "Live".to_string()])].into_iter().collect();
        let f = find_in(&rows, &HashMap::new(), &secondary, &HashSet::new());
        let albums: Vec<(&str, &str, Option<i64>)> = f.albums.iter().map(|a| (a["album_id"].as_str().unwrap(), a["reason"].as_str().unwrap(), a["tracks"][0]["studio"]["id"].as_i64())).collect();
        let mut albums = albums;
        albums.sort();
        assert_eq!(albums, vec![("comp", reason::TAGGED, Some(1)), ("folder", reason::FILE, Some(1)), ("mbstudio", reason::TITLE, Some(1)),
                                ("most", reason::SONGS, Some(1)), ("tagged", reason::TAGGED, Some(1))]);
        let songs: Vec<(i64, &str, Option<i64>)> = f.songs.iter().map(|s| (s["id"].as_i64().unwrap(), s["reason"].as_str().unwrap(), s["studio"]["id"].as_i64())).collect();
        assert_eq!(songs, vec![(9, reason::FILE, Some(2))]);
    }
}
