//! Albums of one name split by release: an artist can give several albums the same title (Weezer's self-titled
//! Blue, Green, Red, White, Teal and Black albums), and their tags make one album id ([`crate::scan::album_key`]
//! is the album artist and title). Each is its own library album again, shown with MusicBrainz's name for it
//! ("Weezer (Blue Album)", the release group's disambiguation) or else its year.
//!
//! What is another album inside one album id:
//! - another MusicBrainz release group: each release group is an album (the one with the most songs keeps
//!   the id). A song without one goes with the release group that has its title, else the one of its year;
//! - without release groups, another year whose songs take the same track numbers with other titles (two
//!   albums laid over each other, not one album with a stray year).
//!
//! The split is remembered (`album_splits`): the scan writes a song of that release group or year under its
//! own album from then on ([`resolve`]). Files never move. [`crate::merge`] doesn't put the parts back
//! together: albums with no song in common are never merged by name.
use encore_db::{now, p, Conn, Db, Row, RowExt};
use sha1::{Digest, Sha1};
use std::collections::{BTreeMap, HashMap, HashSet};

/// One song as the split sees it.
#[derive(Clone, Debug)]
pub struct Song {
    /// its row id
    pub id: i64,
    /// its album id
    pub album_id: String,
    /// its MusicBrainz release group ('' none)
    pub rg: String,
    /// the first four characters of its year tag ('' none)
    pub year: String,
    /// its disc number
    pub disc: i64,
    /// its track number
    pub track: i64,
    /// its title
    pub title: String,
    /// another copy of a song the library has from MusicBrainz ([`crate::twins`]): it stays with the album it
    /// was merged into, whatever its year
    pub copy: bool,
}

/// Songs of an album that become another album: `part` names them for the next scan.
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    /// the album it leaves
    pub album_id: String,
    /// 'rg:<release group>' or 'y:<year>'
    pub part: String,
    /// the album id it becomes
    pub to_id: String,
    /// its songs' row ids
    pub songs: Vec<i64>,
    /// other names the scan finds the same album by ('y:<year>' of a release group's songs)
    pub aliases: Vec<String>,
}

/// The album id the songs of `part` of `album_id` get.
pub fn part_id(album_id: &str, part: &str) -> String {
    hex::encode(Sha1::digest(format!("{album_id}|{part}").as_bytes()))[..16].to_string()
}

/// A real four-digit year from a tag ("" for none or 0000).
fn year_of(s: &str) -> String {
    let y: String = s.trim().chars().take(4).collect();
    if y.len() == 4 && y.chars().all(|c| c.is_ascii_digit()) && y != "0000" { y } else { String::new() }
}

/// The year most of `songs` carry ('' none).
fn main_year(songs: &[&Song]) -> String {
    let mut n: BTreeMap<&str, usize> = BTreeMap::new();
    for s in songs.iter().filter(|s| !s.year.is_empty()) {
        *n.entry(s.year.as_str()).or_default() += 1;
    }
    n.into_iter().max_by_key(|(_, c)| *c).map(|(y, _)| y.to_string()).unwrap_or_default()
}

/// Two groups of songs are two albums laid over each other: the same disc and track number with another title.
fn clash(a: &[&Song], b: &[&Song]) -> bool {
    let slots: HashMap<(i64, i64), &str> = a.iter().filter(|s| s.track > 0).map(|s| ((s.disc.max(1), s.track), s.title.as_str())).collect();
    b.iter().filter(|s| s.track > 0).any(|s| slots.get(&(s.disc.max(1), s.track)).is_some_and(|t| *t != s.title))
}

/// The biggest group keeps the album id: most songs, then the oldest song.
fn keeper<K: Clone>(groups: &[(K, Vec<&Song>)]) -> usize {
    (0..groups.len()).min_by_key(|i| (-(groups[*i].1.len() as i64), groups[*i].1.iter().map(|s| s.id).min().unwrap_or(0))).unwrap_or(0)
}

/// How one album's songs split: no parts when they are one album.
fn plan_album(album_id: &str, songs: &[&Song]) -> Vec<Part> {
    let mut by_rg: BTreeMap<&str, Vec<&Song>> = BTreeMap::new();
    let mut loose: Vec<&Song> = vec![];
    for s in songs {
        if s.rg.is_empty() { loose.push(s) } else { by_rg.entry(s.rg.as_str()).or_default().push(s) }
    }
    if by_rg.len() >= 2 {
        let mut groups: Vec<(String, Vec<&Song>)> = by_rg.into_iter().map(|(rg, g)| (format!("rg:{rg}"), g)).collect();
        let years: Vec<String> = groups.iter().map(|(_, g)| main_year(g)).collect();
        // a song without a release group: the group that has its title, else the one of its year
        for s in loose {
            let by_title: Vec<usize> = (0..groups.len()).filter(|i| groups[*i].1.iter().any(|x| x.title == s.title)).collect();
            let by_year: Vec<usize> = (0..groups.len()).filter(|i| !s.year.is_empty() && years[*i] == s.year).collect();
            match (by_title.as_slice(), by_year.as_slice()) {
                ([i], _) | ([], [i]) => groups[*i].1.push(s),
                _ => {}
            }
        }
        let k = keeper(&groups);
        return groups.iter().enumerate().filter(|(i, _)| *i != k).map(|(i, (part, g))| {
            let unique_year = !years[i].is_empty() && years.iter().filter(|y| **y == years[i]).count() == 1;
            Part {
                album_id: album_id.into(), part: part.clone(), to_id: part_id(album_id, part), songs: g.iter().map(|s| s.id).collect(),
                aliases: if unique_year { vec![format!("y:{}", years[i])] } else { vec![] },
            }
        }).collect();
    }
    // one release group or none: the years, when they are albums laid over each other
    let mut groups: Vec<(String, Vec<&Song>)> = vec![];
    let rg_songs: Vec<&Song> = by_rg.into_values().flatten().collect();
    let rg_year = main_year(&rg_songs);
    if !rg_songs.is_empty() {
        groups.push((String::new(), rg_songs));
    }
    let mut by_year: BTreeMap<String, Vec<&Song>> = BTreeMap::new();
    for s in loose {
        // a song of the release group's year, or of none, or a copy of one of its songs, is with it
        let y = if s.year == rg_year || s.copy { String::new() } else { s.year.clone() };
        by_year.entry(y).or_default().push(s);
    }
    for (y, g) in by_year {
        match groups.iter_mut().find(|(k, _)| k.is_empty() && y.is_empty()) {
            Some((_, gr)) => gr.extend(g),
            None => groups.push((y, g)),
        }
    }
    if groups.len() < 2 {
        return vec![];
    }
    let k = keeper(&groups);
    groups.iter().enumerate().filter(|(i, (y, g))| *i != k && !y.is_empty() && clash(&groups[k].1, g)).map(|(_, (y, g))| {
        let part = format!("y:{y}");
        Part { album_id: album_id.into(), to_id: part_id(album_id, &part), part, songs: g.iter().map(|s| s.id).collect(), aliases: vec![] }
    }).collect()
}

/// Which songs of which albums become other albums.
pub fn plan(songs: &[Song]) -> Vec<Part> {
    let mut by_album: BTreeMap<&str, Vec<&Song>> = BTreeMap::new();
    for s in songs {
        by_album.entry(s.album_id.as_str()).or_default().push(s);
    }
    by_album.into_iter().flat_map(|(a, g)| plan_album(a, &g)).collect()
}

/// The songs of albums that may hold more than one: more than one release group, or more than one year.
pub fn songs(c: &Conn) -> rusqlite::Result<Vec<Song>> {
    let rows = c.query("SELECT id, album_id, COALESCE(mb_releasegroupid,'') rg, COALESCE(year,'') year, COALESCE(disc,1) disc, COALESCE(track,0) track,
                               COALESCE(title,'') title, copy_of IS NOT NULL copy FROM tracks WHERE album_id IN (
                            SELECT album_id FROM tracks WHERE album_id IS NOT NULL AND album IS NOT NULL AND album != '' GROUP BY album_id
                            HAVING COUNT(DISTINCT NULLIF(mb_releasegroupid,'')) > 1 OR COUNT(DISTINCT substr(NULLIF(year,''),1,4)) > 1)", p![])?;
    Ok(rows.iter().map(|r: &Row| Song {
        id: r.i("id"), album_id: r.s("album_id"), rg: r.s("rg"), year: year_of(&r.s("year")), disc: r.i("disc"), track: r.i("track"),
        title: crate::catalog::title_key(&r.s("title")), copy: r.i("copy") != 0,
    }).collect())
}

/// Splits the albums [`plan`] finds, in one transaction. → `(album, new album)` split off.
pub fn split_albums(c: &Conn) -> rusqlite::Result<Vec<(String, String)>> {
    let parts = plan(&songs(c)?);
    if parts.is_empty() {
        return Ok(vec![]);
    }
    c.transaction(|c| {
        let t = now();
        for pt in &parts {
            for ids in pt.songs.chunks(500) {
                let marks = vec!["?"; ids.len()].join(",");
                let mut ps = vec![serde_json::json!(pt.to_id)];
                ps.extend(ids.iter().map(|i| serde_json::json!(i)));
                c.execute(&format!("UPDATE tracks SET album_id=? WHERE id IN ({marks})"), ps)?;
            }
            for part in std::iter::once(&pt.part).chain(&pt.aliases) {
                c.execute("INSERT OR REPLACE INTO album_splits (album_id, part, to_id, split_at) VALUES (?,?,?,?)", p![pt.album_id, part, pt.to_id, t])?;
            }
        }
        Ok::<_, rusqlite::Error>(())
    })?;
    Ok(parts.into_iter().map(|pt| (pt.album_id, pt.to_id)).collect())
}

/// The album a song of `album_id` (after [`crate::merge::resolve`]) with this release group and year goes in.
pub fn resolve(c: &Conn, album_id: &str, rg: &str, year: &str) -> rusqlite::Result<String> {
    let part = if !rg.is_empty() { format!("rg:{rg}") } else {
        let y = year_of(year);
        if y.is_empty() {
            return Ok(album_id.to_string());
        }
        format!("y:{y}")
    };
    Ok(c.one("SELECT to_id FROM album_splits WHERE album_id=? AND part=?", p![album_id, part])?.map(|r| r.s("to_id")).unwrap_or_else(|| album_id.to_string()))
}

// ---- names ------------------------------------------------------------------------------------

/// The albums whose album artist and title another album of the library has too: id → what tells it apart
/// (MusicBrainz's name for its release group, "Blue Album", else its year; '' when neither does).
pub fn editions(db: &impl encore_db::Query) -> HashMap<String, String> {
    editions_named(db, None)
}

/// What tells one album apart from the others of its name ('' nothing, or no other album has its name).
pub fn edition_of(db: &impl encore_db::Query, album_id: &str, album: &str) -> String {
    editions_named(db, Some(album)).remove(album_id).unwrap_or_default()
}

/// The editions of the albums that share a name (all, or only those titled `title`): album id → its MusicBrainz disambiguation, else its year when no other one of the name has it, else "".
fn editions_named(db: &impl encore_db::Query, title: Option<&str>) -> HashMap<String, String> {
    let (cond, ps) = match title {
        Some(t) => ("AND lower(t.album)=lower(?)", p![t.trim()]),
        None => ("", p![]),
    };
    let rows = db.query(&format!("SELECT t.album_id, MAX(COALESCE(NULLIF(t.albumartist,''), t.artist)) artist, MAX(t.album) album, MIN(NULLIF(t.year,'')) year,
                                MAX(COALESCE(n.disambiguation,'')) nick
                         FROM tracks t LEFT JOIN rg_names n ON n.rg=t.mb_releasegroupid
                         WHERE t.album_id IS NOT NULL AND t.album IS NOT NULL AND t.album != '' {cond} GROUP BY t.album_id"), ps).unwrap_or_default();
    let mut by_name: HashMap<(String, String), Vec<&Row>> = HashMap::new();
    for r in &rows {
        by_name.entry((crate::merge::artist_key(&r.s("artist")), r.s("album").trim().to_lowercase())).or_default().push(r);
    }
    let mut out = HashMap::new();
    for g in by_name.values().filter(|g| g.len() > 1) {
        let years: Vec<String> = g.iter().map(|r| year_of(&r.s("year"))).collect();
        for (r, y) in g.iter().zip(&years) {
            let nick = r.s("nick");
            let tell = if !nick.is_empty() { nick } else if !y.is_empty() && years.iter().filter(|x| *x == y).count() == 1 { y.clone() } else { String::new() };
            out.insert(r.s("album_id"), tell);
        }
    }
    out
}

/// "Weezer (Blue Album)": the album's title with what tells it apart, when something does.
pub fn shown_title(album: &str, edition: &str) -> String {
    if edition.is_empty() || album.to_lowercase().contains(&edition.to_lowercase()) { album.to_string() } else { format!("{album} ({edition})") }
}

/// Puts [`shown_title`] into `album` of each album row (`id`, `album`) that shares its name, and the plain title
/// into `album_title` and what tells it apart into `edition`.
pub fn stamp(rows: &mut [Row], eds: &HashMap<String, String>) {
    for r in rows.iter_mut() {
        if let Some(e) = eds.get(&r.s("id")).filter(|e| !e.is_empty()) {
            let title = r.s("album");
            r.insert("album".into(), serde_json::json!(shown_title(&title, e)));
            r.insert("album_title".into(), serde_json::json!(title));
            r.insert("edition".into(), serde_json::json!(e));
        }
    }
}

/// Release groups of albums that share their name and whose MusicBrainz name wasn't asked for yet.
pub fn names_wanted(db: &Db, limit: usize) -> Vec<String> {
    let eds = editions(db);
    if eds.is_empty() {
        return vec![];
    }
    let rows = db.query("SELECT DISTINCT t.album_id, t.mb_releasegroupid rg FROM tracks t LEFT JOIN rg_names n ON n.rg=t.mb_releasegroupid
                         WHERE COALESCE(t.mb_releasegroupid,'') != '' AND n.rg IS NULL", p![]).unwrap_or_default();
    let mut seen = HashSet::new();
    rows.iter().filter(|r| eds.contains_key(&r.s("album_id"))).map(|r| r.s("rg")).filter(|rg| seen.insert(rg.clone())).take(limit).collect()
}

/// Keep a release group's disambiguation (looked up now).
pub fn store_name(db: &Db, rg: &str, disambiguation: &str) {
    if let Err(e) = db.execute("INSERT OR REPLACE INTO rg_names (rg, disambiguation, looked_up_at) VALUES (?,?,?)", p![rg, disambiguation.trim(), now()]) {
        tracing::warn!("album names: {e}");
    }
}

/// Asks MusicBrainz for the names of at most `limit` release groups [`names_wanted`] lists. → how many were
/// found.
pub async fn look_up_names(st: &impl crate::Ctx, limit: usize) -> usize {
    use encore_http::mb;
    let db = st.db().clone();
    let todo = tokio::task::spawn_blocking(move || names_wanted(&db, limit)).await.unwrap_or_default();
    let mut n = 0;
    for rg in todo {
        match mb::with_priority(mb::Priority::Background, mb::release_group(st, &rg)).await {
            Ok(g) => {
                store_name(st.db(), &rg, g["disambiguation"].as_str().unwrap_or(""));
                n += 1;
            }
            Err(e) if mb::is_not_found(&e) => store_name(st.db(), &rg, ""),
            Err(e) => {
                tracing::info!("album names: {rg} wasn't looked up ({e}): next run");
                if mb::is_busy(&e) {
                    break;
                }
            }
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(id: i64, rg: &str, year: &str, track: i64, title: &str) -> Song {
        Song { id, album_id: "w".into(), rg: rg.into(), year: year.into(), disc: 1, track, title: title.into(), copy: false }
    }

    #[test]
    fn each_release_group_is_its_own_album() {
        let s = [
            song(1, "blue", "1994", 1, "my name is jonas"), song(2, "blue", "1994", 2, "no one else"), song(3, "blue", "1994", 3, "the world has turned"),
            song(4, "green", "2001", 1, "dont let go"), song(5, "green", "2001", 2, "photograph"),
            song(6, "", "2001", 3, "hash pipe"), song(7, "", "1994", 9, "no one else"), song(8, "red", "2008", 1, "troublemaker"),
        ];
        let mut p = plan(&s);
        p.sort_by(|a, b| a.part.cmp(&b.part));
        assert_eq!(p.len(), 2);
        assert_eq!((p[0].part.as_str(), p[0].songs.clone(), p[0].aliases.clone()), ("rg:green", vec![4, 5, 6], vec!["y:2001".to_string()]));
        assert_eq!((p[1].part.as_str(), p[1].songs.clone()), ("rg:red", vec![8]));
        assert_eq!(p[0].to_id, part_id("w", "rg:green"));
        assert_ne!(p[0].to_id, p[1].to_id);
    }

    #[test]
    fn years_split_only_albums_laid_over_each_other() {
        // a Deezer Blue and Green album: the same track numbers, other titles
        let s = [song(1, "", "1994", 1, "my name is jonas"), song(2, "", "1994", 2, "no one else"), song(3, "", "2001", 1, "dont let go")];
        let p = plan(&s);
        assert_eq!(p.len(), 1);
        assert_eq!((p[0].part.as_str(), p[0].songs.clone()), ("y:2001", vec![3]));
        // one album whose bonus song has another year: left alone
        let s = [song(1, "", "1994", 1, "my name is jonas"), song(2, "", "1994", 2, "no one else"), song(3, "", "2004", 11, "jamie")];
        assert!(plan(&s).is_empty());
        // one release group with a song of another year but the same title in its slot: left alone
        let s = [song(1, "blue", "1994", 1, "my name is jonas"), song(2, "", "2004", 1, "my name is jonas")];
        assert!(plan(&s).is_empty());
        // a copy of one of its songs from YouTube (encore_library::twins merged it in): its year and track
        // number don't take it out again
        let mut yt = song(3, "", "2019", 2, "buddy holly (official video)");
        yt.copy = true;
        let s = [song(1, "blue", "1994", 1, "my name is jonas"), song(2, "blue", "1994", 2, "no one else"), yt];
        assert!(plan(&s).is_empty());
    }

    #[test]
    fn names_tell_same_titled_albums_apart() {
        assert_eq!(shown_title("Weezer", "Blue Album"), "Weezer (Blue Album)");
        assert_eq!(shown_title("Weezer (Blue Album)", "Blue Album"), "Weezer (Blue Album)");
        assert_eq!(shown_title("Weezer", ""), "Weezer");
    }

    /// On a real database: the merged album splits, the scan finds the parts again, and the list says which is which.
    #[test]
    fn splitting_moves_the_songs_and_is_remembered() {
        let dir = encore_core::tmp::dir("album-split");
        let db = encore_db::Db::open(&dir).unwrap();
        let w = crate::scan::album_key("Weezer", "Weezer", "");
        for (id, rg, year, track, title) in [
            (1, "rg-blue", "1994-05-10", 1, "My Name Is Jonas"), (2, "rg-blue", "1994-05-10", 2, "No One Else"),
            (3, "rg-green", "2001-05-15", 1, "Don't Let Go"), (4, "", "2001", 2, "Photograph"),
        ] {
            db.execute("INSERT INTO tracks (id, path, album_id, title, artist, albumartist, album, mb_releasegroupid, year, track, disc, duration)
                        VALUES (?,?,?,?,'Weezer','Weezer','Weezer',?,?,?,1,200)", p![id, format!("/m/{id}.flac"), w, title, rg, year, track]).unwrap();
        }
        let moved = db.write_now(split_albums).unwrap();
        let green = part_id(&w, "rg:rg-green");
        assert_eq!(moved, vec![(w.clone(), green.clone())]);
        let ids = |a: &str| db.query("SELECT id FROM tracks WHERE album_id=? ORDER BY id", p![a]).unwrap().iter().map(|r| r.i("id")).collect::<Vec<_>>();
        assert_eq!((ids(&w), ids(&green)), (vec![1, 2], vec![3, 4]));
        assert!(db.write_now(split_albums).unwrap().is_empty());
        // and merging by name doesn't put them back together
        assert!(db.write_now(crate::merge::merge_albums).unwrap().is_empty());
        assert_eq!(db.write_now(|c| resolve(c, &w, "rg-green", "")).unwrap(), green);
        assert_eq!(db.write_now(|c| resolve(c, &w, "", "2001-01-01")).unwrap(), green);
        assert_eq!(db.write_now(|c| resolve(c, &w, "rg-blue", "1994")).unwrap(), w);
        // years until MusicBrainz names them
        let eds = editions(&db);
        assert_eq!((eds[&w].as_str(), eds[&green].as_str()), ("1994", "2001"));
        store_name(&db, "rg-blue", "Blue Album");
        assert_eq!(editions(&db)[&w], "Blue Album");
        assert_eq!(edition_of(&db, &w, "weezer"), "Blue Album");
        assert_eq!(names_wanted(&db, 10), vec!["rg-green".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
