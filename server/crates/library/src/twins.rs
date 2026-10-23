//! One song shown once: a song the library has from MusicBrainz and again without MusicBrainz's tags (a .txt or
//! playlist import downloaded on its own, an untagged file, a copy from Deezer or YouTube) is one song.
//!
//! The same song: the same first artist, title, version (live, remix, edit … [`crate::matcher`]) and remixer,
//! lengths within [`SAME_LENGTH`] seconds. Of a song's copies one is shown – lossless first, then the bitrate,
//! then the one tagged from MusicBrainz – and the others point at it (`tracks.copy_of`): the song lists, search
//! and shuffle leave them out ([`SHOWN`]). Copies that are all from MusicBrainz (the album and the single the
//! song is also on) are releases of their own and stay as they are.
//!
//! An album without a MusicBrainz release group whose every song is a copy of a song on one MusicBrainz album
//! (a download of "Song – Single" from YouTube when the library has the album) goes into that album the way
//! [`crate::merge`] merges albums (`album_merges`): the album list shows it once and the album page shows each
//! song once ([`crate::merge::other_copies`]). [`crate::split`] keeps copies with their album.
//!
//! Files are never moved or deleted; the Duplicates tool still lists the copies.
use crate::matcher::{describe, key};
use encore_db::{p, Conn, Row, RowExt};
use std::collections::{HashMap, HashSet};

/// The most two copies of one song may differ in length (seconds).
pub const SAME_LENGTH: f64 = 4.0;

/// The SQL condition a song list adds to show each song once: songs that are no copy, or whose shown copy is
/// gone (deleted since the last scan).
pub const SHOWN: &str = "(copy_of IS NULL OR NOT EXISTS (SELECT 1 FROM tracks k WHERE k.id = tracks.copy_of))";

/// One song as this pass sees it.
#[derive(Clone, Debug, Default)]
pub struct Song {
    pub id: i64,
    pub album_id: String,
    /// the album tag ('' none: a folder of loose songs)
    pub album: String,
    pub title: String,
    pub artist: String,
    pub duration: f64,
    /// its MusicBrainz recording ('' none)
    pub mbid: String,
    /// its MusicBrainz release group ('' none)
    pub rg: String,
    pub lossless: bool,
    pub bitrate: i64,
}

impl Song {
    fn has_mbid(&self) -> bool {
        !self.mbid.is_empty()
    }
}

/// What the pass decides: each copy and the song shown in its place, and the albums that go into another.
#[derive(Debug, Default, PartialEq)]
pub struct Plan {
    pub copies: HashMap<i64, i64>,
    /// `(from, to)`: albums without MusicBrainz whose every song is a copy of a song on `to`
    pub merges: Vec<(String, String)>,
}

/// The shown copy first: lossless, then the bitrate, then MusicBrainz's, then the oldest.
fn rank(s: &Song) -> (bool, i64, bool, i64) {
    (s.lossless, s.bitrate, s.has_mbid(), -s.id)
}

/// The groups of copies of one song (two or more songs each, at least one from MusicBrainz and one not).
fn groups(songs: &[Song]) -> Vec<Vec<&Song>> {
    let mut buckets: HashMap<String, Vec<&Song>> = HashMap::new();
    for s in songs {
        let d = describe(&s.title, &s.artist, Some(s.duration), None, None);
        if d.base.is_empty() || s.duration <= 0.0 {
            continue;
        }
        buckets.entry(key(&d)).or_default().push(s);
    }
    let mut out = vec![];
    for mut b in buckets.into_values() {
        if b.len() < 2 {
            continue;
        }
        b.sort_by(|x, y| x.duration.total_cmp(&y.duration).then(x.id.cmp(&y.id)));
        // runs of songs whose lengths are within SAME_LENGTH of the run's first
        let mut run: Vec<&Song> = vec![];
        for s in b {
            if run.first().is_some_and(|f| s.duration - f.duration > SAME_LENGTH) {
                out.push(std::mem::take(&mut run));
            }
            run.push(s);
        }
        out.push(run);
    }
    out.retain(|g| g.len() >= 2 && g.iter().any(|s| s.has_mbid()) && g.iter().any(|s| !s.has_mbid()));
    out
}

pub fn plan(songs: &[Song]) -> Plan {
    let mut copies: HashMap<i64, i64> = HashMap::new();
    // each song without MusicBrainz → the MusicBrainz albums (with a release group) its song is on
    let mut on_albums: HashMap<i64, HashSet<&str>> = HashMap::new();
    for g in groups(songs) {
        let shown = g.iter().copied().max_by_key(|s| rank(s)).unwrap();
        for s in &g {
            if s.id != shown.id {
                copies.insert(s.id, shown.id);
            }
        }
        let albums: HashSet<&str> = g.iter().filter(|s| s.has_mbid() && !s.rg.is_empty()).map(|s| s.album_id.as_str()).collect();
        for s in g.iter().filter(|s| !s.has_mbid()) {
            on_albums.insert(s.id, albums.clone());
        }
    }
    // albums without a release group whose every song is on one MusicBrainz album
    let mut by_album: HashMap<&str, Vec<&Song>> = HashMap::new();
    for s in songs.iter().filter(|s| !s.album.is_empty()) {
        by_album.entry(s.album_id.as_str()).or_default().push(s);
    }
    let mut merges = vec![];
    for (id, g) in by_album {
        if g.iter().any(|s| !s.rg.is_empty()) {
            continue;
        }
        let mut common: Option<HashSet<&str>> = None;
        for s in &g {
            let on = on_albums.get(&s.id).cloned().unwrap_or_default();
            common = Some(match common {
                None => on,
                Some(c) => c.intersection(&on).copied().collect(),
            });
        }
        let mut to: Vec<&str> = common.unwrap_or_default().into_iter().filter(|t| *t != id).collect();
        to.sort();
        // on two MusicBrainz albums (the album and a deluxe edition of it): the one holding the most songs
        let size = |a: &str| songs.iter().filter(|s| s.album_id == a).count();
        if let Some(t) = to.iter().max_by_key(|t| (size(t), std::cmp::Reverse(**t))) {
            merges.push((id.to_string(), t.to_string()));
        }
    }
    merges.sort();
    Plan { copies, merges }
}

/// The library's songs, for [`plan`].
pub fn songs(c: &Conn) -> rusqlite::Result<Vec<Song>> {
    let rows = c.query("SELECT id, COALESCE(album_id,'') album_id, COALESCE(album,'') album, COALESCE(title,'') title, COALESCE(artist,'') artist,
                               COALESCE(duration,0) duration, COALESCE(mb_trackid,'') mbid, COALESCE(mb_releasegroupid,'') rg,
                               COALESCE(format,'') format, COALESCE(bitrate,0) bitrate FROM tracks", p![])?;
    Ok(rows.iter().map(|r: &Row| Song {
        id: r.i("id"), album_id: r.s("album_id"), album: r.s("album"), title: r.s("title"), artist: r.s("artist"), duration: r.f("duration"),
        mbid: r.s("mbid"), rg: r.s("rg"), lossless: matches!(r.s("format").to_lowercase().as_str(), "flac" | "alac" | "wav" | "aiff" | "ape" | "wv" | "dsf" | "dff"),
        bitrate: r.i("bitrate"),
    }).collect())
}

/// What one run of [`find_copies`] changed.
#[derive(Debug, Default, PartialEq)]
pub struct Found {
    /// songs that became a copy or stopped being one
    pub changed: usize,
    /// `(from, to)` albums merged
    pub merged: Vec<(String, String)>,
}

/// Marks the copies [`plan`] finds (and unmarks the songs that are no copy any more) and merges the albums it
/// finds, in one transaction.
pub fn find_copies(c: &Conn) -> rusqlite::Result<Found> {
    let pl = plan(&songs(c)?);
    let now: HashMap<i64, i64> = c.query("SELECT id, copy_of FROM tracks WHERE copy_of IS NOT NULL", p![])?.iter().map(|r| (r.i("id"), r.i("copy_of"))).collect();
    let set: Vec<(i64, i64)> = pl.copies.iter().filter(|(id, to)| now.get(id) != Some(to)).map(|(a, b)| (*a, *b)).collect();
    let clear: Vec<i64> = now.keys().filter(|id| !pl.copies.contains_key(id)).copied().collect();
    if set.is_empty() && clear.is_empty() && pl.merges.is_empty() {
        return Ok(Found::default());
    }
    c.transaction(|c| {
        for (id, to) in &set {
            c.execute("UPDATE tracks SET copy_of=? WHERE id=?", p![to, id])?;
        }
        for id in &clear {
            c.execute("UPDATE tracks SET copy_of=NULL WHERE id=?", p![id])?;
        }
        Ok::<_, rusqlite::Error>(())
    })?;
    crate::merge::apply(c, &pl.merges)?;
    Ok(Found { changed: set.len() + clear.len(), merged: pl.merges })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::too_many_arguments)]
    fn song(id: i64, album_id: &str, album: &str, title: &str, artist: &str, duration: f64, mbid: &str, rg: &str) -> Song {
        Song { id, album_id: album_id.into(), album: album.into(), title: title.into(), artist: artist.into(), duration, mbid: mbid.into(), rg: rg.into(), lossless: false, bitrate: 320 }
    }

    /// A playlist import's "Song" from YouTube is the album's song: shown once, and its one-song album goes into
    /// the album.
    #[test]
    fn a_copy_without_musicbrainz_is_shown_once_and_joins_the_album() {
        let s = [
            song(1, "lp", "Rumours", "Dreams", "Fleetwood Mac", 257.0, "rec1", "rg1"),
            song(2, "lp", "Rumours", "Go Your Own Way", "Fleetwood Mac", 223.0, "rec2", "rg1"),
            song(3, "yt", "Dreams - Single", "Dreams (2004 Remaster)", "Fleetwood Mac", 258.5, "", ""),
            song(4, "loose", "", "Go Your Own Way [Official Audio]", "Fleetwood Mac feat. Nobody", 224.0, "", ""),
        ];
        let p = plan(&s);
        assert_eq!(p.copies, HashMap::from([(3, 1), (4, 2)]));
        assert_eq!(p.merges, vec![("yt".to_string(), "lp".to_string())]);
    }

    #[test]
    fn other_versions_and_lengths_are_other_songs() {
        let s = [
            song(1, "lp", "Rumours", "Dreams", "Fleetwood Mac", 257.0, "rec1", "rg1"),
            song(2, "x", "X", "Dreams (Live)", "Fleetwood Mac", 257.0, "", ""),
            song(3, "y", "Y", "Dreams", "Fleetwood Mac", 290.0, "", ""),
            song(4, "z", "Z", "Dreams", "The Cranberries", 257.0, "", ""),
        ];
        assert_eq!(plan(&s), Plan::default());
    }

    /// The album and the single are both from MusicBrainz: two releases, nothing changes.
    #[test]
    fn copies_all_from_musicbrainz_stay() {
        let s = [song(1, "lp", "Rumours", "Dreams", "Fleetwood Mac", 257.0, "rec1", "rg1"), song(2, "s", "Dreams", "Dreams", "Fleetwood Mac", 257.0, "rec9", "rg2")];
        assert_eq!(plan(&s), Plan::default());
    }

    /// A lossless copy without MusicBrainz is the one shown; an album with a song of its own stays an album.
    #[test]
    fn the_better_copy_is_shown_and_an_album_with_other_songs_stays() {
        let mut flac = song(3, "rip", "Rumours (Deluxe)", "Dreams", "Fleetwood Mac", 257.0, "", "");
        flac.lossless = true;
        let s = [song(1, "lp", "Rumours", "Dreams", "Fleetwood Mac", 257.0, "rec1", "rg1"), flac, song(4, "rip", "Rumours (Deluxe)", "Dreams (Demo)", "Fleetwood Mac", 250.0, "", "")];
        let p = plan(&s);
        assert_eq!(p.copies, HashMap::from([(1, 3)]));
        assert!(p.merges.is_empty());
    }

    /// On a real database: the copy is marked and its album merged, the next scan keeps it there, a song that
    /// stops being a copy is unmarked, and the song lists' condition leaves the copy out until its shown copy goes.
    #[test]
    fn copies_are_marked_and_unmarked() {
        let dir = encore_core::tmp::dir("song-copies");
        let db = encore_db::Db::open(&dir).unwrap();
        let lp = crate::scan::album_key("Fleetwood Mac", "Rumours", "");
        let yt = crate::scan::album_key("Fleetwood Mac", "Dreams - Single", "");
        for (id, path, aid, album, mbid, rg) in [(1, "/m/lp/1.flac", &lp, "Rumours", "rec1", "rg1"), (2, "/m/yt/1.m4a", &yt, "Dreams - Single", "", "")] {
            db.execute("INSERT INTO tracks (id, path, album_id, title, artist, albumartist, album, mb_trackid, mb_releasegroupid, format, bitrate, duration)
                        VALUES (?,?,?,'Dreams','Fleetwood Mac','Fleetwood Mac',?,?,?,'flac',900,257)", p![id, path, aid, album, mbid, rg]).unwrap();
        }
        let f = db.write_now(find_copies).unwrap();
        assert_eq!(f, Found { changed: 1, merged: vec![(yt.clone(), lp.clone())] });
        assert_eq!(db.one("SELECT copy_of, album_id FROM tracks WHERE id=2", p![]).unwrap().unwrap().i("copy_of"), 1);
        assert_eq!(db.write_now(|c| crate::merge::resolve(c, &yt)).unwrap(), lp);
        let shown = |db: &encore_db::Db| db.query(&format!("SELECT id FROM tracks WHERE {SHOWN} ORDER BY id"), p![]).unwrap().iter().map(|r| r.i("id")).collect::<Vec<_>>();
        assert_eq!(shown(&db), vec![1]);
        assert_eq!(db.write_now(find_copies).unwrap(), Found::default());
        // the copy's tags change to another song: no copy any more
        db.execute("UPDATE tracks SET title='Songbird' WHERE id=2", p![]).unwrap();
        assert_eq!(db.write_now(find_copies).unwrap().changed, 1);
        assert_eq!(shown(&db), vec![1, 2]);
        // a copy whose shown song was deleted is shown again before the next scan
        db.execute("UPDATE tracks SET title='Dreams' WHERE id=2", p![]).unwrap();
        db.write_now(find_copies).unwrap();
        db.execute("DELETE FROM tracks WHERE id=1", p![]).unwrap();
        assert_eq!(shown(&db), vec![2]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
