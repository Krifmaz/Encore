//! The side index of the library lists (GET /api/library/index): the sections `[{label, offset}]` of a
//! list in exactly the order and filters the list endpoint uses (crate::lists), so a client can jump to
//! a section with the list's own paging.
//!
//! - `kind=albums` (all albums, like the phone), `kind=singles`, or `kind=albums&album_kind=albums`
//!   (the web's Albums tab: `kind=albums` on `/albums`); `filter=` and `artist=` as `/albums` takes them
//! - `kind=artists`, `kind=tracks`
//! - `sort=` / `dir=`: the list's sort keys
//!
//! Labels: name/title/artist sorts → "#" (digits and symbols) and A–Z (accents folded, by the first
//! character the list sorts on); release date → years ("?" unknown); date added → months ("Oct 2026"),
//! and a month that holds a big part of the list (a library added in one scan) split by day, hour or
//! minute ("14 Oct 2026, 15:42", with `short: "15:42"` for the rail); length / size / counts → ranges.
//! A label that already had a section (an accented name sorts after "z") stays in the section it falls in.
//! A filtered list (`q=`) is ranked, not sorted: no sections.
//!
//! Each section: `{label, offset}`, plus `short` when the rail shows a shorter text than the bubble.
use crate::artists::Artists;
use crate::lists::{order, ALBUM_SORTS, ARTISTS_MAX, TRACK_SORTS};
use encore_core::error::ApiResult;
use encore_db::Db;
use serde_json::{json, Value};

/// Which list the sections are for (the query of GET /api/library/index).
#[derive(Clone, Copy, Default)]
pub struct Spec<'a> {
    pub kind: &'a str,
    pub sort: &'a str,
    pub dir: &'a str,
    pub artist: &'a str,
    pub filter: &'a str,
    pub album_kind: &'a str,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum By {
    Letter,
    Year,
    Month,
    TrackLength,
    AlbumLength,
    Size,
    AlbumCount,
    TrackCount,
}

pub fn sections_for(db: &Db, artists: &Artists, a: &Spec) -> ApiResult<Value> {
    let values: Vec<(Value, By)> = match a.kind {
        "artists" => {
            let by = match a.sort {
                "albums" => By::AlbumCount,
                "tracks" => By::TrackCount,
                "added" => By::Month,
                _ => By::Letter,
            };
            let field = match by {
                By::AlbumCount => "albums",
                By::TrackCount => "tracks",
                By::Month => "added",
                _ => "name",
            };
            // only the artists the list returns (it stops at ARTISTS_MAX): a section past that couldn't be jumped to
            artists.index(db).list(a.sort, a.dir).into_iter().take(ARTISTS_MAX).map(|v| (v[field].clone(), by)).collect()
        }
        "tracks" => {
            let key = if TRACK_SORTS.iter().any(|(k, _, _)| *k == a.sort) { a.sort } else { "artist" };
            let (col, by) = match key {
                "title" => ("title", By::Letter),
                "album" => ("album", By::Letter),
                "added" => ("added_at", By::Month),
                "released" => ("year", By::Year),
                "duration" => ("duration", By::TrackLength),
                _ => ("artist", By::Letter),
            };
            let rows = db.query(&format!("SELECT {col} v FROM tracks ORDER BY {}", order(TRACK_SORTS, key, a.dir, "artist")), vec![])?;
            rows.into_iter().map(|r| (r.get("v").cloned().unwrap_or(Value::Null), by)).collect()
        }
        _ => {
            let key = if ALBUM_SORTS.iter().any(|(k, _, _)| *k == a.sort) { a.sort } else { "name" };
            let (col, by) = match key {
                "album" => ("album", By::Letter),
                "year" | "released" => ("year", By::Year),
                "added" => ("added", By::Month),
                "duration" => ("duration", By::AlbumLength),
                "size" => ("size", By::Size),
                _ => ("albumartist", By::Letter),
            };
            let mut wh = vec!["1=1".to_string()];
            let mut params: Vec<Value> = vec![];
            if !a.artist.is_empty() {
                let (cond, ps) = artists.album_filter(db, a.artist);
                wh.push(cond);
                params.extend(ps);
            }
            // the same HAVING as /albums
            let mut having: Vec<&str> = vec![];
            match a.filter {
                "untagged" => having.push("SUM(CASE WHEN mb_albumid IS NULL OR mb_albumid='' THEN 1 ELSE 0 END) > 0"),
                "tagged" => having.push("SUM(CASE WHEN mb_albumid IS NULL OR mb_albumid='' THEN 1 ELSE 0 END) = 0"),
                "lossless" => having.push("SUM(CASE WHEN format IN ('flac','wav','alac','aiff','ape','wv') THEN 1 ELSE 0 END) = COUNT(*)"),
                _ => {}
            }
            let album_kind = if a.kind == "singles" { "singles" } else { a.album_kind };
            match album_kind {
                "singles" => having.push("COUNT(*) <= 2"),
                "albums" => having.push("COUNT(*) > 2"),
                _ => {}
            }
            let having = if having.is_empty() { String::new() } else { format!("HAVING {}", having.join(" AND ")) };
            // the columns the list's ORDER BY names, under the same aliases
            let sql = format!(
                "SELECT MAX(album) album, MAX(albumartist) albumartist, MAX(year) year, SUM(duration) duration, SUM(size) size, MAX(added_at) added
                 FROM tracks WHERE {} GROUP BY album_id {having} ORDER BY {}",
                wh.join(" AND "),
                order(ALBUM_SORTS, key, a.dir, "name")
            );
            db.query(&sql, params)?.into_iter().map(|r| (r.get(col).cloned().unwrap_or(Value::Null), by)).collect()
        }
    };
    let labels: Vec<(String, Option<String>)> = match values.first().map(|(_, by)| *by) {
        Some(By::Letter) => letters(values.iter().map(|(v, _)| v.as_str().unwrap_or(""))).into_iter().map(|l| (l, None)).collect(),
        Some(By::Month) => added_labels(&values.iter().map(|(v, _)| num(v)).collect::<Vec<_>>()),
        _ => values.iter().map(|(v, by)| (label(v, *by), None)).collect(),
    };
    Ok(json!(sections(labels.into_iter())))
}

/// The letter of every name, in list order. Lists sort by the lowercased text, so names that start with
/// a non-ASCII character ("Émilie", "Ølafur", "Мумий Тролль") sort after "z": they belong to the section
/// before them (or, sorted Z–A, to the one after), not to a second "E" that would jump backwards.
pub fn letters<'a>(names: impl Iterator<Item = &'a str>) -> Vec<String> {
    let raw: Vec<(Option<String>, &str)> = names.map(|n| (letter(n), n)).collect();
    let first = raw.iter().find_map(|(l, _)| l.clone());
    let mut prev: Option<String> = None;
    raw.into_iter().map(|(l, n)| {
        let out = match (l, &first) {
            (Some(l), _) => l,
            (None, Some(f)) => prev.clone().unwrap_or_else(|| f.clone()),
            (None, None) => folded(n),
        };
        prev = Some(out.clone());
        out
    }).collect()
}

/// `[{label, offset, short?}]`: a section where the label changes to one not seen yet.
fn sections(labels: impl Iterator<Item = (String, Option<String>)>) -> Vec<Value> {
    let mut seen = std::collections::HashSet::new();
    let mut prev: Option<String> = None;
    let mut out = vec![];
    for (i, (l, short)) in labels.enumerate() {
        if prev.as_deref() != Some(l.as_str()) && seen.insert(l.clone()) {
            let mut s = json!({ "label": l, "offset": i });
            if let Some(short) = short {
                s["short"] = json!(short);
            }
            out.push(s);
        }
        prev = Some(l);
    }
    out
}

/// Finer steps for date added: 0 month, 1 day, 2 hour, 3 minute, 4 second.
const FINEST: u8 = 4;

fn time_key(t: f64, level: u8) -> i64 {
    if t <= 0.0 {
        return i64::MIN;
    }
    match level {
        0 => {
            let (y, m, _) = encore_core::time::civil(t);
            y * 12 + m
        }
        1 => (t / 86_400.0).floor() as i64,
        2 => (t / 3_600.0).floor() as i64,
        3 => (t / 60.0).floor() as i64,
        _ => t.floor() as i64,
    }
}

/// Date-added labels (epoch seconds, in list order): months; where one section holds more than about a
/// twelfth of the list (everything a first scan added shares a time), that section is split by the
/// coarsest of day / hour / minute that splits it, again until no section is that big or minutes are
/// reached, so the side bar has places to jump to inside it. The other months stay months.
fn added_labels(ts: &[f64]) -> Vec<(String, Option<String>)> {
    let n = ts.len();
    let limit = (n / 12).max(40);
    let mut level = vec![0u8; n];
    loop {
        let mut changed = false;
        let mut i = 0;
        while i < n {
            let k = time_key(ts[i], level[i]);
            let mut j = i + 1;
            while j < n && level[j] == level[i] && time_key(ts[j], level[j]) == k {
                j += 1;
            }
            if j - i > limit && ts[i] > 0.0 {
                // the coarsest finer step that tells these apart
                let next = (level[i] + 1..=FINEST).find(|&l| {
                    let k0 = time_key(ts[i], l);
                    ts[i..j].iter().any(|&t| time_key(t, l) != k0)
                });
                if let Some(l) = next {
                    level[i..j].iter_mut().for_each(|x| *x = l);
                    changed = true;
                }
            }
            i = j;
        }
        if !changed {
            break;
        }
    }
    let mut out: Vec<(String, Option<String>)> = ts.iter().zip(&level).map(|(&t, &l)| time_label(t, l)).collect();
    // still too big (a whole scan in the same second): blocks of `limit` rows, "15:42:07 (2)" …, so the side
    // bar always has places to jump to and every jump lands exactly
    let mut i = 0;
    while i < n {
        let k = time_key(ts[i], level[i]);
        let mut j = i + 1;
        while j < n && level[j] == level[i] && time_key(ts[j], level[j]) == k {
            j += 1;
        }
        if j - i > limit && ts[i] > 0.0 {
            for (b, x) in (i..j).enumerate().filter(|(b, _)| *b >= limit) {
                let part = b / limit + 1;
                let (l, short) = &mut out[x];
                *l = format!("{l} ({part})");
                *short = Some(format!("{} ({part})", short.clone().unwrap_or_else(|| l.clone())));
            }
        }
        i = j;
    }
    out
}

/// "Oct 2026"; "14 Oct 2026" (rail: "14 Oct"); "14 Oct 2026, 15:00" (rail: "15:00"); "14 Oct 2026, 15:42" (UTC).
fn time_label(t: f64, level: u8) -> (String, Option<String>) {
    if t <= 0.0 {
        return ("?".into(), None);
    }
    if level == 0 {
        return (month(t), None);
    }
    let (y, m, d) = encore_core::time::civil(t);
    let mon = MONTHS[(m - 1) as usize];
    let secs = t.floor() as i64;
    let (hh, mm) = (secs.rem_euclid(86_400) / 3600, secs.rem_euclid(3600) / 60);
    match level {
        1 => (format!("{d} {mon} {y}"), Some(format!("{d} {mon}"))),
        2 => (format!("{d} {mon} {y}, {hh:02}:00"), Some(format!("{hh:02}:00"))),
        3 => (format!("{d} {mon} {y}, {hh:02}:{mm:02}"), Some(format!("{hh:02}:{mm:02}"))),
        _ => {
            let ss = secs.rem_euclid(60);
            (format!("{d} {mon} {y}, {hh:02}:{mm:02}:{ss:02}"), Some(format!("{hh:02}:{mm:02}:{ss:02}")))
        }
    }
}

fn num(v: &Value) -> f64 {
    match v {
        Value::Number(n) => n.as_f64().unwrap_or(0.0),
        Value::String(s) => s.trim().parse().unwrap_or(0.0),
        _ => 0.0,
    }
}

pub fn label(v: &Value, by: By) -> String {
    match by {
        By::Letter => letter(v.as_str().unwrap_or("")).unwrap_or_else(|| folded(v.as_str().unwrap_or(""))),
        By::Year => {
            let s = match v {
                Value::String(s) => s.trim().to_string(),
                Value::Number(n) => n.to_string(),
                _ => String::new(),
            };
            let y: String = s.chars().take(4).collect();
            if y.len() == 4 && y.chars().all(|c| c.is_ascii_digit()) { y } else { "?".into() }
        }
        By::Month => {
            let t = num(v);
            if t <= 0.0 {
                return "?".into();
            }
            month(t)
        }
        By::TrackLength => range(num(v) / 60.0, &[(2.0, "<2 min"), (3.0, "2–3 min"), (4.0, "3–4 min"), (5.0, "4–5 min"), (7.0, "5–7 min"), (10.0, "7–10 min"), (20.0, "10–20 min")], "20+ min"),
        By::AlbumLength => range(num(v) / 60.0, &[(20.0, "<20 min"), (30.0, "20–30 min"), (40.0, "30–40 min"), (50.0, "40–50 min"), (60.0, "50–60 min"), (80.0, "60–80 min")], "80+ min"),
        By::Size => range(num(v) / 1_000_000.0, &[(50.0, "<50 MB"), (100.0, "50–100 MB"), (250.0, "100–250 MB"), (500.0, "250–500 MB"), (1000.0, "500 MB–1 GB"), (2000.0, "1–2 GB")], "2 GB+"),
        By::AlbumCount => range(num(v), &[(2.0, "1 album"), (3.0, "2 albums"), (5.0, "3–4 albums"), (10.0, "5–9 albums"), (20.0, "10–19 albums")], "20+ albums"),
        By::TrackCount => range(num(v), &[(5.0, "<5 songs"), (10.0, "5–9 songs"), (25.0, "10–24 songs"), (50.0, "25–49 songs"), (100.0, "50–99 songs"), (200.0, "100–199 songs")], "200+ songs"),
    }
}

/// The label of the first range `x` is below, or `top`.
fn range(x: f64, ranges: &[(f64, &str)], top: &str) -> String {
    ranges.iter().find(|(below, _)| x < *below).map_or(top, |(_, l)| *l).to_string()
}

/// "#" or "A"–"Z" for a name starting with an ASCII character (what the list sorts on); None otherwise.
fn letter(s: &str) -> Option<String> {
    match s.chars().next() {
        None => Some("#".into()),
        Some(c) if c.is_ascii_alphabetic() => Some(c.to_ascii_uppercase().to_string()),
        Some(c) if c.is_ascii() => Some("#".into()),
        Some(_) => None,
    }
}

/// A list of only non-ASCII names: their letter with accents folded ("#" for other scripts).
fn folded(s: &str) -> String {
    match s.chars().next().and_then(|c| crate::artists::fold(&c.to_string()).chars().next()) {
        Some(f) if f.is_ascii_lowercase() => f.to_ascii_uppercase().to_string(),
        _ => "#".into(),
    }
}

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// "Oct 2026" for epoch seconds (UTC).
fn month(t: f64) -> String {
    let (y, m, _) = encore_core::time::civil(t);
    format!("{} {y}", MONTHS[(m - 1) as usize])
}

#[cfg(test)]
mod unit {
    use super::*;

    #[test]
    fn labels() {
        assert_eq!(letter("Émile"), None);
        assert_eq!(letter("the beatles").as_deref(), Some("T"));
        assert_eq!(letter("2Pac").as_deref(), Some("#"));
        assert_eq!(letter("(Untitled)").as_deref(), Some("#"));
        assert_eq!(letter("").as_deref(), Some("#"));
        assert_eq!(letters(["abba", "zoe", "Émile", "~x"].into_iter()), ["A", "Z", "Z", "#"]);
        assert_eq!(letters(["Émile", "zoe", "abba"].into_iter()), ["Z", "Z", "A"]);
        assert_eq!(letters(["Émile", "Ørsted", "Мумий"].into_iter()), ["E", "#", "#"]); // "Ø" has no ASCII base letter
        assert_eq!(month(1_791_936_000.0), "Oct 2026");
        assert_eq!(month(0.5), "Jan 1970");
        assert_eq!(month(951_782_400.0), "Feb 2000"); // 29 Feb 2000
        assert_eq!(label(&json!("2019-05-01"), By::Year), "2019");
        assert_eq!(label(&json!(""), By::Year), "?");
        assert_eq!(label(&Value::Null, By::Year), "?");
        assert_eq!(label(&json!(119.0), By::TrackLength), "<2 min");
        assert_eq!(label(&json!(300.0), By::TrackLength), "5–7 min");
        assert_eq!(label(&json!(1200.0), By::TrackLength), "20+ min");
        assert_eq!(label(&json!(3600.0), By::AlbumLength), "60–80 min");
        assert_eq!(label(&json!(150_000_000), By::Size), "100–250 MB");
        assert_eq!(label(&json!(3_000_000_000_i64), By::Size), "2 GB+");
        assert_eq!(label(&json!(1), By::AlbumCount), "1 album");
        assert_eq!(label(&json!(2), By::AlbumCount), "2 albums");
        assert_eq!(label(&json!(150), By::TrackCount), "100–199 songs");
        assert_eq!(encore_core::time::civil(951_782_400.0), (2000, 2, 29));
        assert_eq!(time_label(1_791_995_520.0, 3), ("14 Oct 2026, 16:32".to_string(), Some("16:32".to_string())));
        assert_eq!(time_label(1_791_995_520.0, 1), ("14 Oct 2026".to_string(), Some("14 Oct".to_string())));
    }

    #[test]
    fn added_splits_a_big_month() {
        // 10 albums added over October, then 500 by one first scan on 14 Jan 2020 over 50 minutes (newest first)
        let oct = 1_791_936_000.0;
        let scan = 1_579_000_000.0;
        let mut ts: Vec<f64> = (0..10).map(|i| oct - i as f64 * 86_400.0).collect();
        ts.extend((0..500).map(|i| scan + 3000.0 - i as f64 * 6.0));
        ts.push(0.0);
        let l = added_labels(&ts);
        assert_eq!(l.len(), ts.len());
        // the small month stays a month; the scan becomes minutes (one hour, one day: those don't split it)
        assert_eq!(l[0].0, "Oct 2026");
        assert!(l[9].0.starts_with("Oct 2026") || l[9].0.starts_with("Sep 2026"), "{:?}", l[9]);
        assert_eq!(l[10].0, "14 Jan 2020, 11:56");
        assert_eq!(l[10].1.as_deref(), Some("11:56"));
        assert_eq!(l[ts.len() - 1].0, "?");
        let secs = sections(l.into_iter());
        assert!(secs.len() > 40, "{}", secs.len());
        // jumps stay exact: each section starts where its label first appears
        let again = added_labels(&ts);
        for s in &secs {
            let off = s["offset"].as_u64().unwrap() as usize;
            assert_eq!(again[off].0, s["label"].as_str().unwrap());
            assert!(off == 0 || again[off - 1].0 != again[off].0);
        }
        // a small list keeps months
        let few: Vec<f64> = (0..30).map(|i| scan + i as f64).collect();
        assert!(added_labels(&few).iter().all(|(l, s)| l == "Jan 2020" && s.is_none()));
    }

    #[test]
    fn added_splits_a_scan_inside_one_minute() {
        let scan = 1_579_000_020.0;
        // 500 albums in 40 seconds: seconds
        let ts: Vec<f64> = (0..500).map(|i| scan + 40.0 - i as f64 * 0.08).collect();
        let l = added_labels(&ts);
        assert!(sections(l.clone().into_iter()).len() > 1);
        assert!(l[0].1.as_deref().is_some_and(|s| s.matches(':').count() == 2), "{:?}", l[0]);
        // all in the same second: numbered blocks, still exact jumps
        let ts = vec![scan; 500];
        let l = added_labels(&ts);
        let secs = sections(l.clone().into_iter());
        assert!(secs.len() > 1, "{secs:?}");
        for s in &secs {
            let off = s["offset"].as_u64().unwrap() as usize;
            assert_eq!(l[off].0, s["label"].as_str().unwrap());
        }
    }

    #[test]
    fn sections_skip_repeated_labels() {
        let v = ["#", "A", "A", "B", "E", "Z", "E"].iter().map(|s| (s.to_string(), None));
        assert_eq!(json!(sections(v)), json!([
            { "label": "#", "offset": 0 }, { "label": "A", "offset": 1 }, { "label": "B", "offset": 3 },
            { "label": "E", "offset": 4 }, { "label": "Z", "offset": 5 },
        ]));
    }
}
