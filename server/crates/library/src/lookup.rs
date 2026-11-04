//! The library and the requests for a list of items at once (Discover's search hits, an album's songs, a
//! catalogue's track list, recommendations): one query per batch of up to 500 keys instead of one or two per
//! item. Item for item, each answer is the one the single lookup gives ([`crate::scan::find_track`],
//! [`crate::scan::album_id_for`], [`crate::catalog::library_track_for`] and the request lookups of the
//! routes); the tests compare them on the same library.
//!
//! The single lookups compare with SQLite's `lower()` (ASCII letters only) and take the first row of an
//! index search, which is the lowest song id among equal keys (newest request first where they say so); the
//! batches read the same rows with `IN (…)` and choose the same way in memory.
use encore_db::{Db, Row, RowExt};
use serde_json::{json, Value};

/// Keys per query.
const CHUNK: usize = 500;

/// SQLite's built-in `lower()`: ASCII letters only, everything else as it is.
fn lower(s: &str) -> String {
    s.to_ascii_lowercase()
}

/// The `ORDER BY CASE WHEN format IN ('flac','alac','wav','aiff') THEN 0 ELSE 1 END` of the single lookups.
fn lossy_rank(r: &Row) -> u8 {
    if matches!(r.get("format").and_then(|v| v.as_str()), Some("flac" | "alac" | "wav" | "aiff")) { 0 } else { 1 }
}

/// A text column of a row; None for NULL (which never equals anything in SQL).
fn text<'a>(r: &'a Row, k: &str) -> Option<&'a str> {
    r.get(k).and_then(|v| v.as_str())
}

/// Every row of `sql` (with one `{in}` where the marks go) for these keys, a batch of [`CHUNK`] per query.
fn rows_in(db: &Db, sql: &str, keys: &[Value]) -> Vec<Row> {
    let mut out = vec![];
    for chunk in keys.chunks(CHUNK) {
        let q = sql.replace("{in}", &crate::artists::marks(chunk.len()));
        out.extend(db.query(&q, chunk.to_vec()).unwrap_or_default());
    }
    out
}

/// The distinct values of `keys`, as query parameters.
fn params(keys: impl IntoIterator<Item = String>) -> Vec<Value> {
    let mut v: Vec<String> = keys.into_iter().collect();
    v.sort();
    v.dedup();
    v.into_iter().map(|k| json!(k)).collect()
}

/// The song columns the lookups answer with.
const SONG: &str = "id, title, artist, album, album_id, format, duration";

/// Removes the helper columns (`_…`) a batch read for matching.
fn clean(mut r: Row) -> Row {
    r.retain(|k, _| !k.starts_with('_'));
    r
}

/// Songs whose `lower(title)` is one of these titles' (with their lowered artist, album artist and title),
/// in id order.
fn songs_by_title(db: &Db, titles: impl IntoIterator<Item = String>) -> Vec<Row> {
    let keys = params(titles.into_iter().map(|t| lower(&t)));
    let mut rows = rows_in(db, &format!("SELECT {SONG}, lower(title) _lt, lower(artist) _la, lower(albumartist) _laa FROM tracks WHERE lower(title) IN ({{in}})"), &keys);
    rows.sort_by_key(|r| r.i("id"));
    rows
}

/// Is `r` (from `songs_by_title`) the song `title` by `artist`: `lower(title)=lower(?) AND (lower(artist)=lower(?)
/// OR lower(albumartist)=lower(?))`.
fn same_song(r: &Row, title: &str, artist: &str) -> bool {
    let (t, a) = (lower(title), lower(artist));
    text(r, "_lt") == Some(t.as_str()) && (text(r, "_la") == Some(a.as_str()) || text(r, "_laa") == Some(a.as_str()))
}

/// The best of these rows the way the single lookup's `ORDER BY` lossless-first chose (ties: lowest id).
fn best<'a>(rows: impl Iterator<Item = &'a Row>) -> Option<&'a Row> {
    rows.min_by_key(|r| (lossy_rank(r), r.i("id")))
}

/// [`crate::scan::find_track`] for each (MusicBrainz recording id, artist, title): the library copy, best
/// quality first.
pub fn find_tracks(db: &Db, items: &[(String, String, String)]) -> Vec<Option<Row>> {
    let by_mbid = rows_in(db, &format!("SELECT {SONG}, mb_trackid _mbid FROM tracks WHERE mb_trackid IN ({{in}})"),
        &params(items.iter().map(|i| i.0.clone()).filter(|m| !m.is_empty())));
    let mut out: Vec<Option<Row>> = items.iter().map(|(mbid, _, _)| {
        if mbid.is_empty() {
            return None;
        }
        best(by_mbid.iter().filter(|r| text(r, "_mbid") == Some(mbid.as_str()))).cloned()
    }).collect();
    let rest: Vec<usize> = (0..items.len()).filter(|i| out[*i].is_none()).collect();
    if !rest.is_empty() {
        let songs = songs_by_title(db, rest.iter().map(|i| items[*i].2.clone()));
        for i in rest {
            let (_, artist, title) = &items[i];
            out[i] = best(songs.iter().filter(|r| same_song(r, title, artist))).cloned();
        }
    }
    out.into_iter().map(|r| r.map(clean)).collect()
}

/// The first library copy (lowest id, whatever the quality) for each (MusicBrainz recording id, artist,
/// title): by recording id, else by title and artist when there is a title. For You's lookup.
pub fn first_tracks(db: &Db, items: &[(String, String, String)]) -> Vec<Option<Row>> {
    let by_mbid = rows_in(db, &format!("SELECT {SONG}, mb_trackid _mbid FROM tracks WHERE mb_trackid IN ({{in}})"),
        &params(items.iter().map(|i| i.0.clone()).filter(|m| !m.is_empty())));
    let first = |rows: &mut dyn Iterator<Item = &Row>| rows.min_by_key(|r| r.i("id")).cloned();
    let mut out: Vec<Option<Row>> = items.iter().map(|(mbid, _, _)| {
        if mbid.is_empty() {
            return None;
        }
        first(&mut by_mbid.iter().filter(|r| text(r, "_mbid") == Some(mbid.as_str())))
    }).collect();
    let rest: Vec<usize> = (0..items.len()).filter(|i| out[*i].is_none() && !items[*i].2.is_empty()).collect();
    if !rest.is_empty() {
        let songs = songs_by_title(db, rest.iter().map(|i| items[*i].2.clone()));
        for i in rest {
            let (_, artist, title) = &items[i];
            out[i] = first(&mut songs.iter().filter(|r| same_song(r, title, artist)));
        }
    }
    out.into_iter().map(|r| r.map(clean)).collect()
}

/// [`crate::catalog::library_track_for`] for each (ISRC, artist, title, length): same ISRC, else same title
/// and artist (within 3 seconds when both lengths are known), best quality first.
pub fn library_tracks_for(db: &Db, items: &[(String, String, String, Option<f64>)]) -> Vec<Option<Row>> {
    let isrc_key = |s: &str| s.to_uppercase().replace('-', "");
    let by_isrc = rows_in(db, &format!("SELECT {SONG}, upper(replace(isrc,'-','')) _isrc FROM tracks WHERE upper(replace(isrc,'-','')) IN ({{in}})"),
        &params(items.iter().map(|i| isrc_key(&i.0)).filter(|k| !k.is_empty())));
    let mut out: Vec<Option<Row>> = items.iter().map(|(isrc, _, _, _)| {
        let k = isrc_key(isrc);
        if k.is_empty() {
            return None;
        }
        best(by_isrc.iter().filter(|r| text(r, "_isrc") == Some(k.as_str()))).cloned()
    }).collect();
    let rest: Vec<usize> = (0..items.len()).filter(|i| out[*i].is_none() && !items[*i].2.is_empty()).collect();
    if !rest.is_empty() {
        let songs = songs_by_title(db, rest.iter().map(|i| items[*i].2.clone()));
        for i in rest {
            let (_, artist, title, length) = &items[i];
            let mut hits: Vec<&Row> = songs.iter().filter(|r| same_song(r, title, artist)).collect();
            hits.sort_by_key(|r| (lossy_rank(r), r.i("id")));
            hits.truncate(20);
            out[i] = match length.filter(|l| *l > 0.0) {
                Some(l) => hits.into_iter().find(|r| { let d = r.f("duration"); d <= 0.0 || (d - l).abs() <= 3.0 }).cloned(),
                None => hits.first().map(|r| (*r).clone()),
            };
        }
    }
    out.into_iter().map(|r| r.map(clean)).collect()
}

/// May a release group be a library album found by its title? Not one tagged from another release group that
/// is asked about too for the same artist: an artist's albums of one name (Weezer's self-titled ones, [`crate::split`]) are other
/// albums. A release group of the library's albums not asked about may be the same album under another id.
fn other_release(asked: &std::collections::HashSet<(String, &str)>, artist: &str, rg: &str, album_rg: &str) -> bool {
    rg.is_empty() || album_rg.is_empty() || rg == album_rg || !asked.contains(&(crate::merge::artist_key(artist), album_rg))
}

/// The library album for each (MusicBrainz release group, artist, album title): by release group, else by
/// album title and album artist or artist ([`crate::scan::album_id_for`]).
pub fn album_ids_for(db: &Db, items: &[(String, String, String)]) -> Vec<Option<String>> {
    let by_rg = rows_in(db, "SELECT id, album_id, mb_releasegroupid _rg FROM tracks WHERE mb_releasegroupid IN ({in})",
        &params(items.iter().map(|i| i.0.clone()).filter(|r| !r.is_empty())));
    // the release groups asked about, by artist
    let asked: std::collections::HashSet<(String, &str)> = items.iter().filter(|i| !i.0.is_empty()).map(|i| (crate::merge::artist_key(&i.1), i.0.as_str())).collect();
    let mut out: Vec<Option<String>> = items.iter().map(|(rg, _, _)| {
        if rg.is_empty() {
            return None;
        }
        by_rg.iter().filter(|r| text(r, "_rg") == Some(rg.as_str())).min_by_key(|r| r.i("id")).map(|r| r.s("album_id"))
    }).collect();
    let rest: Vec<usize> = (0..items.len()).filter(|i| out[*i].is_none() && !items[*i].2.is_empty()).collect();
    if !rest.is_empty() {
        let keys = params(rest.iter().map(|i| lower(&items[*i].2)));
        let rows = rows_in(db, "SELECT id, album_id, lower(album) _lal, lower(albumartist) _laa, lower(artist) _la, mb_releasegroupid _rg FROM tracks
                                WHERE lower(album) IN ({in})", &keys);
        for i in rest {
            let (rg, artist, album) = &items[i];
            let (al, a) = (lower(album), lower(artist));
            out[i] = rows.iter()
                .filter(|r| text(r, "_lal") == Some(al.as_str()) && (text(r, "_laa") == Some(a.as_str()) || text(r, "_la") == Some(a.as_str())))
                .filter(|r| other_release(&asked, artist, rg, &r.s("_rg")))
                .min_by_key(|r| r.i("id")).map(|r| r.s("album_id"));
        }
    }
    // the titles as the merge compares them (encore_library::merge): "Lemonade (Deluxe)" by "Beyonce" is the
    // library's "Lemonade" by "Beyonce" (the artist's albums are read by its spelling, any case)
    let rest: Vec<usize> = (0..items.len()).filter(|i| out[*i].is_none() && !items[*i].2.is_empty()).collect();
    if !rest.is_empty() {
        let keys = params(rest.iter().map(|i| lower(&items[*i].1)));
        let rows = rows_in(db, "SELECT MIN(id) id, album_id, MAX(album) album, MAX(COALESCE(NULLIF(albumartist,''), artist)) _a,
                                       MAX(COALESCE(mb_releasegroupid,'')) _rg FROM tracks
                                WHERE lower(COALESCE(NULLIF(albumartist,''), artist)) IN ({in}) GROUP BY album_id", &keys);
        for i in rest {
            let (rg, artist, album) = &items[i];
            let (tk, ak) = (crate::catalog::title_key(album), crate::merge::artist_key(artist));
            out[i] = rows.iter()
                .filter(|r| !tk.is_empty() && crate::catalog::title_key(&r.s("album")) == tk && crate::merge::artist_key(&r.s("_a")) == ak)
                .filter(|r| other_release(&asked, artist, rg, &r.s("_rg")))
                .min_by_key(|r| r.i("id")).map(|r| r.s("album_id"));
        }
    }
    out
}

/// How many songs the library has on each of these albums (`tracks.album_id`), for the album ids it has at all;
/// an id with no song isn't in the map. One query per batch of [`CHUNK`] ids. The pages use it to tell a single
/// (one song, played as a song) from an album with a page of its own.
pub fn album_song_counts(db: &Db, ids: &[String]) -> std::collections::HashMap<String, i64> {
    let keys = params(ids.iter().filter(|i| !i.is_empty()).cloned());
    rows_in(db, "SELECT album_id, COUNT(*) n FROM tracks WHERE album_id IN ({in}) GROUP BY album_id", &keys)
        .into_iter().map(|r| (r.s("album_id"), r.i("n"))).collect()
}

/// For each (artist, title) pair: whether a song has `lower(<artist_col>)=lower(artist) AND
/// lower(<title_col>)=lower(title)`. One query per batch of titles.
fn pairs_owned(db: &Db, artist_col: &str, title_col: &str, pairs: &[(String, String)]) -> Vec<bool> {
    let titles = params(pairs.iter().map(|(_, t)| lower(t)));
    let rows = rows_in(db, &format!("SELECT lower({artist_col}) _a, lower({title_col}) _t FROM tracks WHERE lower({title_col}) IN ({{in}})"), &titles);
    let have: std::collections::HashSet<(&str, &str)> = rows.iter().filter_map(|r| Some((text(r, "_a")?, text(r, "_t")?))).collect();
    pairs.iter().map(|(a, t)| have.contains(&(lower(a).as_str(), lower(t).as_str()))).collect()
}

/// For each (album artist, album title): whether the library has a song of that album
/// (`lower(albumartist)=lower(?) AND lower(album)=lower(?)`, For you's albums).
pub fn albums_owned(db: &Db, pairs: &[(String, String)]) -> Vec<bool> {
    pairs_owned(db, "albumartist", "album", pairs)
}

/// For each (artist, title): whether the library has a song by that artist with that title
/// (`lower(artist)=lower(?) AND lower(title)=lower(?)`, For you's songs).
pub fn songs_owned(db: &Db, pairs: &[(String, String)]) -> Vec<bool> {
    pairs_owned(db, "artist", "title", pairs)
}

/// Requests of one kind for these (artist, title) pairs and these MusicBrainz ids, newest first, without the
/// denied and cancelled ones (a caller that skips failed ones too filters them).
pub struct Requests {
    /// the matching request rows, newest first
    rows: Vec<Row>,
}

impl Requests {
    /// Reads them: requests of `kind` whose lower(artist) and lower(title) are among the pairs', or whose
    /// MusicBrainz id is among `mbids`.
    pub fn read(db: &Db, kind: &str, pairs: &[(String, String)], mbids: &[String]) -> Requests {
        const COLS: &str = "id, status, message, mbid, lower(artist) _la, lower(title) _lt";
        let mut rows: Vec<Row> = vec![];
        let mut lowered: Vec<(String, String)> = pairs.iter().map(|(a, t)| (lower(a), lower(t))).collect();
        lowered.sort();
        lowered.dedup();
        // a batch of pairs per query: the kind, their artists and their titles, so the index on (kind,
        // lower(artist), lower(title)) narrows it down (the pairs themselves are matched in memory)
        for chunk in lowered.chunks(CHUNK) {
            let artists = params(chunk.iter().map(|(a, _)| a.clone()));
            let titles = params(chunk.iter().map(|(_, t)| t.clone()));
            let sql = format!("SELECT {COLS} FROM requests WHERE kind=? AND lower(artist) IN ({}) AND lower(title) IN ({}) AND status NOT IN ('denied','cancelled')",
                crate::artists::marks(artists.len()), crate::artists::marks(titles.len()));
            let mut ps = vec![json!(kind)];
            ps.extend(artists);
            ps.extend(titles);
            rows.extend(db.query(&sql, ps).unwrap_or_default());
        }
        let ids = params(mbids.iter().filter(|m| !m.is_empty()).cloned());
        for chunk in ids.chunks(CHUNK) {
            let mut ps = vec![json!(kind)];
            ps.extend(chunk.iter().cloned());
            rows.extend(db.query(&format!("SELECT {COLS} FROM requests WHERE kind=? AND mbid IN ({}) AND status NOT IN ('denied','cancelled')",
                crate::artists::marks(chunk.len())), ps).unwrap_or_default());
        }
        rows.sort_by_key(|r| std::cmp::Reverse(r.i("id")));
        rows.dedup_by_key(|r| r.i("id"));
        Requests { rows }
    }

    /// The newest request `pick` takes: (status, message).
    fn newest(&self, pick: impl Fn(&Row) -> bool) -> Option<(String, String)> {
        self.rows.iter().find(|r| pick(r)).map(|r| (r.s("status"), r.s("message")))
    }

    /// The newest request by artist and title (`lower(artist)=lower(?) AND lower(title)=lower(?)`); with
    /// `failed` false, failed ones are skipped too; with `no_mbid`, only requests without a MusicBrainz id.
    pub fn by_name(&self, artist: &str, title: &str, failed: bool, no_mbid: bool) -> Option<(String, String)> {
        let (a, t) = (lower(artist), lower(title));
        self.newest(|r| text(r, "_la") == Some(a.as_str()) && text(r, "_lt") == Some(t.as_str())
            && (failed || r.s("status") != "failed") && (!no_mbid || r.get("mbid").is_none_or(|m| m.is_null())))
    }

    /// The newest request with this MusicBrainz id or this artist and title, failed ones skipped (For You's
    /// `(mbid IS NOT NULL AND mbid=?) OR (lower(artist)=lower(?) AND lower(title)=lower(?))`).
    pub fn by_mbid_or_name(&self, mbid: &str, artist: &str, title: &str) -> Option<(String, String)> {
        let (a, t) = (lower(artist), lower(title));
        self.newest(|r| r.s("status") != "failed"
            && ((!mbid.is_empty() && text(r, "mbid") == Some(mbid)) || (text(r, "_la") == Some(a.as_str()) && text(r, "_lt") == Some(t.as_str()))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::library_track_for;
    use crate::scan::{album_id_for, find_track};
    use encore_db::p;

    /// A library with the cases the lookups tell apart: two copies of a song (lossy first, then lossless),
    /// names in other cases, accents (which lower() leaves alone), NULL columns, ISRCs with dashes, release
    /// groups shared by two albums, and requests of every status.
    fn library() -> (Db, std::path::PathBuf) {
        let dir = encore_core::tmp::dir("lookup");
        let db = Db::open(&dir).unwrap();
        // title, artist, albumartist, album, album_id, format, mb_trackid, isrc, rg, duration
        type Song<'a> = (&'a str, &'a str, &'a str, &'a str, &'a str, &'a str, &'a str, &'a str, &'a str, f64);
        let songs: &[Song] = &[
            ("Song", "Artist", "Artist", "Album", "a1", "mp3", "m1", "US-AB1-23-00001", "rg1", 200.0),
            ("song", "ARTIST", "Artist", "Album", "a1", "flac", "m1", "USAB12300001", "rg1", 201.0),
            ("Other", "Someone", "Artist", "Album", "a1", "flac", "", "", "rg1", 100.0),
            ("Song", "Guest", "Artist", "Album Two", "a2", "mp3", "m2", "", "rg2", 30.0),
            ("Café", "Élan", "Élan", "Été", "a3", "ogg", "", "FRXX10000001", "", 0.0),
            ("café", "élan", "Élan", "été", "a4", "wav", "", "", "", 180.0),
            ("Song", "Band", "Band", "Album", "a5", "aiff", "", "", "rg5", 210.0),
        ];
        for (i, (t, a, aa, al, aid, f, m, isrc, rg, d)) in songs.iter().enumerate() {
            let opt = |s: &str| if s.is_empty() { Value::Null } else { json!(s) };
            db.execute("INSERT INTO tracks (id, path, title, artist, albumartist, album, album_id, format, mb_trackid, isrc, mb_releasegroupid, duration) VALUES (?,?,?,?,?,?,?,?,?,?,?,?)",
                p![i as i64 + 1, format!("/m/{i}"), t, a, aa, al, aid, f, opt(m), opt(isrc), opt(rg), d]).unwrap();
        }
        db.execute("INSERT INTO tracks (id, path, title, artist) VALUES (20, '/m/null', NULL, NULL)", p![]).unwrap();
        let reqs: &[(&str, &str, &str, &str, &str)] = &[
            // kind, artist, title, mbid, status
            ("track", "Artist", "Song", "m1", "failed"),
            ("track", "artist", "SONG", "", "queued"),
            ("track", "Artist", "Song", "", "cancelled"),
            ("track", "Nobody", "Gone", "m9", "completed"),
            ("track", "Nobody", "Gone", "", "denied"),
            ("album", "Artist", "Album", "", "downloading"),
            ("album", "Artist", "Album", "rgx", "completed"),
            ("album", "Band", "Live", "", "failed"),
            ("track", "Élan", "Café", "", "pending"),
        ];
        for (kind, a, t, m, s) in reqs {
            let mbid = if m.is_empty() { Value::Null } else { json!(m) };
            db.execute("INSERT INTO requests (user_id, kind, artist, title, mbid, status, message, created_at, updated_at) VALUES (1,?,?,?,?,?,?,0,0)",
                p![kind, a, t, mbid, s, format!("{s} message")]).unwrap();
        }
        (db, dir)
    }

    fn strs(v: &[(&str, &str, &str)]) -> Vec<(String, String, String)> {
        v.iter().map(|(a, b, c)| (a.to_string(), b.to_string(), c.to_string())).collect()
    }

    #[test]
    fn batches_answer_as_the_single_lookups() {
        let (db, dir) = library();
        let tracks = strs(&[
            ("m1", "x", "y"), ("", "artist", "SONG"), ("", "Artist", "song"), ("m2", "", ""), ("", "Guest", "Song"), ("", "Band", "Song"),
            ("", "élan", "café"), ("", "ÉLAN", "CAFÉ"), ("nope", "Nobody", "Song"), ("", "", ""), ("", "Someone", "Other"), ("m9", "Artist", "Song"),
        ]);
        let single: Vec<Option<Row>> = tracks.iter().map(|(m, a, t)| find_track(&db, m, a, t)).collect();
        assert_eq!(find_tracks(&db, &tracks), single);
        assert_eq!(single[1].as_ref().map(|r| r.i("id")), Some(2), "the lossless copy first");
        assert!(single[6].is_some() && single[7].is_none(), "lower() leaves accented capitals alone");

        // For You's lookup: the first copy whatever its quality, and no title lookup without a title
        let first: Vec<Option<Row>> = first_tracks(&db, &tracks);
        assert_eq!(first[1].as_ref().map(|r| r.i("id")), Some(1));
        let one = |sql: &str, ps: Vec<Value>| db.one(sql, ps).ok().flatten();
        for (i, (m, a, t)) in tracks.iter().enumerate() {
            let mut want = if m.is_empty() { None } else { one(&format!("SELECT {SONG} FROM tracks WHERE mb_trackid=? LIMIT 1"), p![m]) };
            if want.is_none() && !t.is_empty() {
                want = one(&format!("SELECT {SONG} FROM tracks WHERE lower(title)=lower(?) AND (lower(artist)=lower(?) OR lower(albumartist)=lower(?)) LIMIT 1"), p![t, a, a]);
            }
            assert_eq!(first[i], want, "{:?}", tracks[i]);
        }

        let ext: Vec<(String, String, String, Option<f64>)> = vec![
            ("US-AB1-23-00001".into(), "".into(), "".into(), None),
            ("usab12300001".into(), "".into(), "".into(), None),
            ("".into(), "Artist".into(), "Song".into(), Some(200.0)),
            ("".into(), "Artist".into(), "Song".into(), Some(150.0)),
            ("".into(), "Band".into(), "Song".into(), Some(212.5)),
            ("".into(), "élan".into(), "café".into(), Some(500.0)),
            ("XX".into(), "Guest".into(), "Song".into(), None),
            ("".into(), "Artist".into(), "".into(), None),
        ];
        let single: Vec<Option<Row>> = ext.iter().map(|(i, a, t, l)| library_track_for(&db, i, a, t, *l)).collect();
        assert_eq!(library_tracks_for(&db, &ext), single);
        assert_eq!(single[0].as_ref().map(|r| r.i("id")), Some(2), "ISRCs compared without dashes, lossless first");

        let albums = strs(&[("rg1", "", ""), ("rg5", "x", "y"), ("", "artist", "ALBUM"), ("", "Guest", "Album Two"), ("", "Élan", "été"), ("rgnone", "Band", "album"), ("", "Band", "")]);
        let single: Vec<Option<String>> = albums.iter().map(|(r, a, al)| album_id_for(&db, r, a, al)).collect();
        assert_eq!(album_ids_for(&db, &albums), single);
        assert_eq!(single[5].as_deref(), Some("a5"), "an unknown release group: by album title and artist");
        assert_eq!(single[2].as_deref(), Some("a1"), "by album artist, case ignored");
        // an artist's albums of one name: a release group asked about with another one the library has isn't it
        let same_name = strs(&[("rg5", "Band", "Album"), ("rg6", "Band", "Album")]);
        assert_eq!(album_ids_for(&db, &same_name), vec![Some("a5".to_string()), None]);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// The song counts per album: every song of the album counted, unknown and empty ids left out.
    #[test]
    fn album_song_counts_count_each_albums_songs() {
        let (db, dir) = library();
        let ids: Vec<String> = ["a1", "a2", "a5", "nope", ""].iter().map(|s| s.to_string()).collect();
        let n = album_song_counts(&db, &ids);
        assert_eq!(n.get("a1"), Some(&3));
        assert_eq!(n.get("a2"), Some(&1), "a one-song album: a single");
        assert_eq!(n.get("a5"), Some(&1));
        assert!(!n.contains_key("nope") && !n.contains_key(""));
        let _ = std::fs::remove_dir_all(dir);
    }

    /// The owned checks say what the single `SELECT 1 … LIMIT 1` look-ups said.
    #[test]
    fn owned_checks_answer_as_the_single_lookups() {
        let (db, dir) = library();
        let one = |sql: &str, a: &str, t: &str| db.one(sql, encore_db::p![a, t]).unwrap().is_some();
        let pairs: Vec<(String, String)> = [("Artist", "Album"), ("artist", "ALBUM TWO"), ("Band", "Album"), ("Élan", "Été"), ("élan", "été"), ("Nobody", "Album"),
            ("Artist", "Song"), ("ARTIST", "song"), ("Guest", "Song"), ("Élan", "Café"), ("Someone", "Other"), ("", "")]
            .iter().map(|(a, t)| (a.to_string(), t.to_string())).collect();
        let albums = albums_owned(&db, &pairs);
        let songs = songs_owned(&db, &pairs);
        for (i, (a, t)) in pairs.iter().enumerate() {
            assert_eq!(albums[i], one("SELECT 1 x FROM tracks WHERE lower(albumartist)=lower(?) AND lower(album)=lower(?) LIMIT 1", a, t), "album {a} / {t}");
            assert_eq!(songs[i], one("SELECT 1 x FROM tracks WHERE lower(artist)=lower(?) AND lower(title)=lower(?) LIMIT 1", a, t), "song {a} / {t}");
        }
        assert!(albums[0] && songs[6], "the cases above are not all misses");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn requests_answer_as_the_single_lookups() {
        let (db, dir) = library();
        let one = |sql: &str, ps: Vec<Value>| db.one(sql, ps).ok().flatten().map(|r| (r.s("status"), r.s("message")));
        let pairs: Vec<(String, String)> = [("ARTIST", "song"), ("Nobody", "Gone"), ("élan", "café"), ("Élan", "Café"), ("Band", "Live"), ("Artist", "Album"), ("x", "y")]
            .iter().map(|(a, t)| (a.to_string(), t.to_string())).collect();
        let tracks = Requests::read(&db, "track", &pairs, &["m1".to_string(), "m9".to_string(), "".to_string()]);
        let albums = Requests::read(&db, "album", &pairs, &[]);
        for (a, t) in &pairs {
            // the routes' artist + title lookups
            assert_eq!(tracks.by_name(a, t, true, false), one("SELECT status, message FROM requests WHERE kind='track' AND lower(artist)=lower(?) AND lower(title)=lower(?)
                AND status NOT IN ('denied','cancelled') ORDER BY id DESC", p![a, t]), "{a} {t}");
            assert_eq!(albums.by_name(a, t, false, true), one("SELECT status, message FROM requests WHERE kind='album' AND mbid IS NULL AND lower(artist)=lower(?) AND lower(title)=lower(?)
                AND status NOT IN ('failed','denied','cancelled') ORDER BY id DESC", p![a, t]), "{a} {t}");
            for m in ["m1", "m9", ""] {
                let mbid = if m.is_empty() { Value::Null } else { json!(m) };
                assert_eq!(tracks.by_mbid_or_name(m, a, t), one("SELECT status, message FROM requests WHERE kind='track' AND ((mbid IS NOT NULL AND mbid=?) OR (lower(artist)=lower(?) AND lower(title)=lower(?)))
                    AND status NOT IN ('failed','denied','cancelled') ORDER BY id DESC", p![mbid, a, t]), "{m} {a} {t}");
            }
        }
        assert_eq!(tracks.by_name("artist", "SONG", true, false).map(|r| r.0).as_deref(), Some("queued"), "the newest, cancelled ones left out");
        let _ = std::fs::remove_dir_all(dir);
    }
}
