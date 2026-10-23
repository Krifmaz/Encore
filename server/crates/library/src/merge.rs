//! One album for the same release: copies of an album tagged from MusicBrainz and from another source
//! (Deezer, Apple Music, Bandcamp, a hand-tagged rip) are shown as one library album, the MusicBrainz one.
//!
//! Album ids stay what [`crate::scan::album_key`] makes of the tags; a merged album's id points at the album it
//! went into (`album_merges`), and the scan writes its songs under that id from then on. Files are never
//! moved, renamed or deleted: a merge only changes `tracks.album_id`.
//!
//! What is the same album, strongest first:
//! - the same MusicBrainz release group;
//! - the same album artist and title once folded ([`artist_key`], [`crate::catalog::title_key`]: accents,
//!   "the", "feat." and edition notes like "(Deluxe Edition)" or " - EP" left out), when the release types the
//!   tags give don't disagree. An album without a release group goes into the one MusicBrainz album of that
//!   name; with two MusicBrainz albums of that name (a self-titled album and single) it is left alone. Albums
//!   without a song in common are never the same release (Weezer's self-titled albums: [`crate::split`]).
use encore_db::{now, p, Conn, Row, RowExt};
use std::collections::{HashMap, HashSet};

/// One library album as the merge sees it.
#[derive(Clone, Debug)]
pub struct Album {
    pub id: String,
    pub artist: String,
    pub title: String,
    /// the release group most of its songs carry ('' none)
    pub rg: String,
    /// the first word of the RELEASETYPE tag, lower case ('' none)
    pub kind: String,
    pub tracks: i64,
    /// its first song's row id: the older album wins a tie
    pub first: i64,
    /// its songs' titles ([`crate::catalog::title_key`]); none known: any album
    pub songs: HashSet<String>,
}

/// The artist part of the key: the folded name without "the" and without featured artists.
pub fn artist_key(s: &str) -> String {
    let l = s.to_lowercase();
    let cut = [" feat. ", " feat ", " featuring ", " ft. ", " ft "].iter().filter_map(|sep| l.find(sep)).min().unwrap_or(l.len());
    crate::artists::key(&s[..cut.min(s.len())])
}

fn kind_of(releasetype: &str) -> String {
    releasetype.to_lowercase().split(|c: char| !c.is_alphanumeric()).find(|w| !w.is_empty()).unwrap_or("").to_string()
}

fn kinds_agree(a: &Album, b: &Album) -> bool {
    a.kind.is_empty() || b.kind.is_empty() || a.kind == b.kind
}

/// Copies of one release have songs in common: two albums of one name without any (Weezer's Blue and Green
/// albums, [`crate::split`]) are two albums.
fn share_a_song(a: &Album, b: &Album) -> bool {
    a.songs.is_empty() || b.songs.is_empty() || !a.songs.is_disjoint(&b.songs)
}

/// The best album of a group: one from MusicBrainz, then the most songs, then the oldest.
fn best<'a>(group: &[&'a Album]) -> &'a Album {
    group.iter().copied().min_by_key(|a| (a.rg.is_empty(), -a.tracks, a.first)).unwrap()
}

/// Which albums go into which: `(from, to)`, never a chain (`to` is never a `from`).
pub fn plan(albums: &[Album]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = vec![];
    // the same release group: one album
    let mut by_rg: HashMap<&str, Vec<&Album>> = HashMap::new();
    for a in albums.iter().filter(|a| !a.rg.is_empty()) {
        by_rg.entry(a.rg.as_str()).or_default().push(a);
    }
    let mut best_of: HashMap<&str, &Album> = HashMap::new();
    for (rg, g) in &by_rg {
        let to = best(g);
        best_of.insert(rg, to);
        for a in g {
            if a.id != to.id {
                out.push((a.id.clone(), to.id.clone()));
            }
        }
    }
    // the same folded artist and title
    let mut by_name: HashMap<(String, String), Vec<&Album>> = HashMap::new();
    for a in albums {
        let (ak, tk) = (artist_key(&a.artist), crate::catalog::title_key(&a.title));
        if ak.is_empty() || tk.is_empty() {
            continue;
        }
        by_name.entry((ak, tk)).or_default().push(a);
    }
    for g in by_name.values() {
        let (mb, other): (Vec<&Album>, Vec<&Album>) = g.iter().partition(|a| !a.rg.is_empty());
        if other.is_empty() {
            continue;
        }
        let mut rgs: Vec<&str> = mb.iter().map(|a| a.rg.as_str()).collect();
        rgs.sort();
        rgs.dedup();
        let to: &Album = match rgs.as_slice() {
            [] => best(&other),
            [rg] => best_of[rg],
            _ => continue,
        };
        for a in other {
            if a.id != to.id && kinds_agree(a, to) && share_a_song(a, to) {
                out.push((a.id.clone(), to.id.clone()));
            }
        }
    }
    out
}

/// The library's albums, for [`plan`]. Albums without an album tag (one per folder) are left out.
pub fn albums(c: &Conn) -> rusqlite::Result<Vec<Album>> {
    let rows = c.query("SELECT album_id, MAX(COALESCE(NULLIF(albumartist,''), artist)) artist, MAX(album) album, COUNT(*) n, MIN(id) first,
                               MAX(releasetype) releasetype FROM tracks WHERE album_id IS NOT NULL AND album IS NOT NULL AND album != ''
                               GROUP BY album_id", p![])?;
    // the release group of an album: the one most of its songs carry
    let rgs = c.query("SELECT album_id, mb_releasegroupid rg, COUNT(*) n FROM tracks WHERE mb_releasegroupid IS NOT NULL AND mb_releasegroupid != ''
                       GROUP BY album_id, mb_releasegroupid ORDER BY n DESC", p![])?;
    let mut rg_of: HashMap<String, String> = HashMap::new();
    for r in &rgs {
        rg_of.entry(r.s("album_id")).or_insert_with(|| r.s("rg"));
    }
    let mut songs_of: HashMap<String, HashSet<String>> = HashMap::new();
    for r in c.query("SELECT DISTINCT album_id, title FROM tracks WHERE album_id IS NOT NULL AND COALESCE(title,'') != ''", p![])? {
        songs_of.entry(r.s("album_id")).or_default().insert(crate::catalog::title_key(&r.s("title")));
    }
    Ok(rows.iter().map(|r: &Row| Album {
        id: r.s("album_id"), artist: r.s("artist"), title: r.s("album"), rg: rg_of.get(&r.s("album_id")).cloned().unwrap_or_default(),
        kind: kind_of(&r.s("releasetype")), tracks: r.i("n"), first: r.i("first"),
        songs: songs_of.remove(&r.s("album_id")).unwrap_or_default(),
    }).collect())
}

/// The album an id was merged into (itself when it wasn't).
pub fn resolve(c: &Conn, album_id: &str) -> rusqlite::Result<String> {
    Ok(c.one("SELECT to_id FROM album_merges WHERE from_id=?", p![album_id])?.map(|r| r.s("to_id")).unwrap_or_else(|| album_id.to_string()))
}

/// Merges the albums [`plan`] finds, in one transaction: the songs move to the album they go into, the album's
/// chosen type and the Surround link follow when the target has none. → `(from, to)` merged.
pub fn merge_albums(c: &Conn) -> rusqlite::Result<Vec<(String, String)>> {
    let moves = plan(&albums(c)?);
    apply(c, &moves)?;
    Ok(moves)
}

/// Moves the songs of each `from` album into its `to` album and remembers it for the next scan, in one
/// transaction (what [`merge_albums`] and [`crate::twins`] merge).
pub fn apply(c: &Conn, moves: &[(String, String)]) -> rusqlite::Result<()> {
    if moves.is_empty() {
        return Ok(());
    }
    c.transaction(|c| {
        let t = now();
        for (from, to) in moves {
            c.execute("UPDATE tracks SET album_id=? WHERE album_id=?", p![to, from])?;
            c.execute("INSERT OR REPLACE INTO album_merges (from_id, to_id, merged_at) VALUES (?,?,?)", p![from, to, t])?;
            // what pointed at the merged album points at the one it went into
            c.execute("UPDATE album_merges SET to_id=? WHERE to_id=?", p![to, from])?;
            c.execute("UPDATE surround_tracks SET stereo_album_id=? WHERE stereo_album_id=?", p![to, from])?;
            c.execute("UPDATE OR IGNORE album_types SET album_id=? WHERE album_id=?", p![to, from])?;
        }
        Ok::<_, rusqlite::Error>(())
    })
}

/// The songs of a merged album that are another copy of one of its songs (same title and artist, lengths
/// within 2 s): all but the best copy (lossless, then bitrate, then one tagged from MusicBrainz). The album
/// page shows each song once; the Duplicates tool still lists the copies.
pub fn other_copies(rows: &[Row]) -> std::collections::HashSet<i64> {
    let rank = |r: &Row| {
        let lossless = matches!(r.s("format").to_lowercase().as_str(), "flac" | "alac" | "wav" | "aiff");
        (lossless, r.i("bitrate"), !r.s("mb_trackid").is_empty(), -r.i("id"))
    };
    let mut kept: HashMap<(String, String), Vec<usize>> = HashMap::new();
    let mut out = std::collections::HashSet::new();
    for (i, r) in rows.iter().enumerate() {
        let k = (crate::catalog::title_key(&r.s("title")), artist_key(&r.s("artist")));
        if k.0.is_empty() {
            continue;
        }
        let slots = kept.entry(k).or_default();
        match slots.iter_mut().find(|j| (rows[**j].f("duration") - r.f("duration")).abs() <= 2.0) {
            Some(j) => {
                if rank(r) > rank(&rows[*j]) {
                    out.insert(rows[*j].i("id"));
                    *j = i;
                } else {
                    out.insert(r.i("id"));
                }
            }
            None => slots.push(i),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn al(id: &str, artist: &str, title: &str, rg: &str, kind: &str, tracks: i64, first: i64) -> Album {
        Album { id: id.into(), artist: artist.into(), title: title.into(), rg: rg.into(), kind: kind.into(), tracks, first, songs: HashSet::new() }
    }

    fn with_songs(mut a: Album, songs: &[&str]) -> Album {
        a.songs = songs.iter().map(|s| s.to_string()).collect();
        a
    }

    #[test]
    fn albums_of_one_name_without_a_song_in_common_stay_apart() {
        let a = [with_songs(al("blue", "Weezer", "Weezer", "rg1", "album", 10, 1), &["buddy holly"]),
                 with_songs(al("green", "Weezer", "Weezer", "", "album", 10, 2), &["hash pipe"])];
        assert!(plan(&a).is_empty());
        let b = [with_songs(al("mb", "Weezer", "Weezer", "rg1", "", 10, 1), &["buddy holly"]),
                 with_songs(al("dz", "Weezer", "Weezer (Deluxe Edition)", "", "", 20, 2), &["buddy holly", "jamie"])];
        assert_eq!(plan(&b), vec![pair("dz", "mb")]);
    }

    fn sorted(mut v: Vec<(String, String)>) -> Vec<(String, String)> {
        v.sort();
        v
    }

    fn pair(a: &str, b: &str) -> (String, String) {
        (a.into(), b.into())
    }

    #[test]
    fn a_copy_from_another_source_goes_into_the_musicbrainz_album() {
        let a = [
            al("mb", "Beyoncé", "Lemonade", "rg1", "album", 12, 5),
            al("dz", "Beyonce", "Lemonade (Deluxe Edition)", "", "", 14, 1),
            al("ap", "The Beyoncé", "LEMONADE", "", "album", 12, 9),
        ];
        assert_eq!(sorted(plan(&a)), vec![pair("ap", "mb"), pair("dz", "mb")]);
    }

    #[test]
    fn the_same_release_group_is_one_album_whatever_the_titles_say() {
        let a = [al("x", "Band", "Song - Single", "rg1", "", 1, 3), al("y", "Band", "Song", "rg1", "", 2, 4)];
        assert_eq!(plan(&a), vec![pair("x", "y")]);
    }

    #[test]
    fn albums_without_musicbrainz_merge_into_the_biggest() {
        let a = [al("a", "Band feat. Guest", "Thing", "", "", 3, 1), al("b", "Band", "Thing [Remastered]", "", "", 10, 2)];
        assert_eq!(plan(&a), vec![pair("a", "b")]);
    }

    #[test]
    fn two_musicbrainz_albums_of_one_name_and_disagreeing_types_stay_apart() {
        let a = [al("lp", "Band", "Band", "rg1", "album", 10, 1), al("s", "Band", "Band", "rg2", "single", 1, 2), al("x", "Band", "Band", "", "", 10, 3)];
        assert!(plan(&a).is_empty());
        let b = [al("lp", "Band", "Thing", "", "album", 10, 1), al("s", "Band", "Thing", "", "single", 1, 2)];
        assert!(plan(&b).is_empty());
        let c = [al("a", "Band", "One", "", "", 10, 1), al("b", "Other", "One", "", "", 10, 2)];
        assert!(plan(&c).is_empty());
    }

    /// On a real database: the Deezer copy's songs move into the MusicBrainz album, the merge is remembered for
    /// the next scan, the album page's other copies are the lesser ones, and running it again changes nothing.
    #[test]
    fn merging_moves_the_songs_and_is_remembered() {
        let dir = encore_core::tmp::dir("album-merge");
        let db = encore_db::Db::open(&dir).unwrap();
        let mb = crate::scan::album_key("Beyoncé", "Lemonade", "");
        let dz = crate::scan::album_key("Beyonce", "Lemonade (Deluxe)", "");
        for (id, path, aid, artist, album, rg, format, bitrate) in [
            (1, "/m/a/1.flac", &mb, "Beyoncé", "Lemonade", "rg1", "flac", 900),
            (2, "/m/b/1.mp3", &dz, "Beyonce", "Lemonade (Deluxe)", "", "mp3", 320),
            (3, "/m/b/2.mp3", &dz, "Beyonce", "Lemonade (Deluxe)", "", "mp3", 320),
        ] {
            let title = if id == 3 { "Bonus" } else { "Pray You Catch Me" };
            db.execute("INSERT INTO tracks (id, path, album_id, title, artist, albumartist, album, mb_releasegroupid, format, bitrate, duration)
                        VALUES (?,?,?,?,?,?,?,?,?,?,200)", p![id, path, aid, title, artist, artist, album, rg, format, bitrate]).unwrap();
        }
        let moved = db.write_now(merge_albums).unwrap();
        assert_eq!(moved, vec![(dz.clone(), mb.clone())]);
        let n = db.one("SELECT COUNT(*) n FROM tracks WHERE album_id=?", p![mb]).unwrap().unwrap().i("n");
        assert_eq!(n, 3);
        assert_eq!(db.write_now(|c| resolve(c, &dz)).unwrap(), mb);
        assert!(db.write_now(merge_albums).unwrap().is_empty());
        let rows = db.query("SELECT * FROM tracks WHERE album_id=? ORDER BY id", p![mb]).unwrap();
        assert_eq!(other_copies(&rows).into_iter().collect::<Vec<_>>(), vec![2]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
