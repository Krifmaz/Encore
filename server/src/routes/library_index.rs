//! GET /api/library/index: the sections of a library list for the side scrubber, `[{label, offset}]` in
//! list order. `offset` is where the section's first item sits in exactly the ordering (and filters) the
//! list endpoint uses, so a client can jump to it with the list's own paging.
//!
//! - `kind=albums` (all albums, like the phone), `kind=singles`, or `kind=albums&album_kind=albums`
//!   (the web's Albums tab: `kind=albums` on `/albums`); `filter=` and `artist=` as `/albums` takes them
//! - `kind=artists`, `kind=tracks`
//! - `sort=` / `dir=`: the list's sort keys
//!
//! Labels: name/title/artist sorts → "#" (digits and symbols) and A–Z (accents folded, by the first
//! character the list sorts on); release date → years ("?" unknown); date added → months ("Oct 2026");
//! length / size / counts → buckets. A label that already had a section (an accented name sorts after "z")
//! stays in the section it falls in. A filtered list (`q=`) is ranked, not sorted: no sections.
use super::{order, ALBUM_SORTS, TRACK_SORTS};
use crate::auth::User;
use crate::error::ApiResult;
use crate::state::AppState;
use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct IndexQ {
    kind: String,
    sort: String,
    dir: String,
    artist: String,
    filter: String,
    album_kind: String,
    q: String,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum By {
    Letter,
    Year,
    Month,
    TrackLength,
    AlbumLength,
    Size,
    AlbumCount,
    TrackCount,
}

pub async fn index(State(st): State<AppState>, User(_): User, Query(a): Query<IndexQ>) -> ApiResult<Json<Value>> {
    if !a.q.trim().is_empty() {
        return Ok(Json(json!([])));
    }
    let values: Vec<(Value, By)> = match a.kind.as_str() {
        "artists" => {
            let by = match a.sort.as_str() {
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
            crate::artists::index(&st.db).list(&a.sort, &a.dir).into_iter().take(super::ARTISTS_MAX).map(|v| (v[field].clone(), by)).collect()
        }
        "tracks" => {
            let key = if TRACK_SORTS.iter().any(|(k, _, _)| *k == a.sort) { a.sort.as_str() } else { "artist" };
            let (col, by) = match key {
                "title" => ("title", By::Letter),
                "album" => ("album", By::Letter),
                "added" => ("added_at", By::Month),
                "released" => ("year", By::Year),
                "duration" => ("duration", By::TrackLength),
                _ => ("artist", By::Letter),
            };
            let rows = st.db.query(&format!("SELECT {col} v FROM tracks ORDER BY {}", order(TRACK_SORTS, key, &a.dir, "artist")), vec![])?;
            rows.into_iter().map(|r| (r.get("v").cloned().unwrap_or(Value::Null), by)).collect()
        }
        _ => {
            let key = if ALBUM_SORTS.iter().any(|(k, _, _)| *k == a.sort) { a.sort.as_str() } else { "name" };
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
                let (cond, ps) = crate::artists::album_filter(&st.db, &a.artist);
                wh.push(cond);
                params.extend(ps);
            }
            // the same HAVING as /albums
            let mut having: Vec<&str> = vec![];
            match a.filter.as_str() {
                "untagged" => having.push("SUM(CASE WHEN mb_albumid IS NULL OR mb_albumid='' THEN 1 ELSE 0 END) > 0"),
                "tagged" => having.push("SUM(CASE WHEN mb_albumid IS NULL OR mb_albumid='' THEN 1 ELSE 0 END) = 0"),
                "lossless" => having.push("SUM(CASE WHEN format IN ('flac','wav','alac','aiff','ape','wv') THEN 1 ELSE 0 END) = COUNT(*)"),
                _ => {}
            }
            let album_kind = if a.kind == "singles" { "singles" } else { a.album_kind.as_str() };
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
                order(ALBUM_SORTS, key, &a.dir, "name")
            );
            st.db.query(&sql, params)?.into_iter().map(|r| (r.get(col).cloned().unwrap_or(Value::Null), by)).collect()
        }
    };
    let labels: Vec<String> = if values.first().is_some_and(|(_, by)| *by == By::Letter) {
        letters(values.iter().map(|(v, _)| v.as_str().unwrap_or("")))
    } else {
        values.iter().map(|(v, by)| label(v, *by)).collect()
    };
    Ok(Json(json!(sections(labels.into_iter()))))
}

/// The letter of every name, in list order. Lists sort by the lowercased text, so names that start with
/// a non-ASCII character ("Émilie", "Ølafur", "Мумий Тролль") sort after "z": they belong to the section
/// before them (or, sorted Z–A, to the one after), not to a second "E" that would jump backwards.
pub(super) fn letters<'a>(names: impl Iterator<Item = &'a str>) -> Vec<String> {
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

/// `[{label, offset}]`: a section where the label changes to one not seen yet.
fn sections(labels: impl Iterator<Item = String>) -> Vec<Value> {
    let mut seen = std::collections::HashSet::new();
    let mut prev: Option<String> = None;
    let mut out = vec![];
    for (i, l) in labels.enumerate() {
        if prev.as_deref() != Some(l.as_str()) && seen.insert(l.clone()) {
            out.push(json!({ "label": l, "offset": i }));
        }
        prev = Some(l);
    }
    out
}

fn num(v: &Value) -> f64 {
    match v {
        Value::Number(n) => n.as_f64().unwrap_or(0.0),
        Value::String(s) => s.trim().parse().unwrap_or(0.0),
        _ => 0.0,
    }
}

fn label(v: &Value, by: By) -> String {
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
        By::TrackLength => {
            let m = num(v) / 60.0;
            (if m < 2.0 { "<2 min" } else if m < 5.0 { "2–5 min" } else if m < 10.0 { "5–10 min" } else if m < 20.0 { "10–20 min" } else { "20+ min" }).into()
        }
        By::AlbumLength => {
            let m = num(v) / 60.0;
            (if m < 20.0 { "<20 min" } else if m < 40.0 { "20–40 min" } else if m < 60.0 { "40–60 min" } else { "60+ min" }).into()
        }
        By::Size => {
            let mb = num(v) / 1_000_000.0;
            (if mb < 100.0 { "<100 MB" } else if mb < 250.0 { "100–250 MB" } else if mb < 500.0 { "250–500 MB" } else if mb < 1000.0 { "500 MB–1 GB" } else { "1 GB+" }).into()
        }
        By::AlbumCount => {
            let n = num(v) as i64;
            (if n <= 1 { "1 album" } else if n < 5 { "2–4 albums" } else if n < 10 { "5–9 albums" } else if n < 20 { "10–19 albums" } else { "20+ albums" }).into()
        }
        By::TrackCount => {
            let n = num(v) as i64;
            (if n < 10 { "<10 songs" } else if n < 25 { "10–24 songs" } else if n < 50 { "25–49 songs" } else if n < 100 { "50–99 songs" } else { "100+ songs" }).into()
        }
    }
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
    // days → civil date (Howard Hinnant's algorithm)
    let z = (t / 86400.0).floor() as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
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
        assert_eq!(label(&json!(300.0), By::TrackLength), "5–10 min");
        assert_eq!(label(&json!(3600.0), By::AlbumLength), "60+ min");
        assert_eq!(label(&json!(150_000_000), By::Size), "100–250 MB");
    }

    #[test]
    fn sections_skip_repeated_labels() {
        let v = ["#", "A", "A", "B", "E", "Z", "E"].iter().map(|s| s.to_string());
        assert_eq!(json!(sections(v)), json!([
            { "label": "#", "offset": 0 }, { "label": "A", "offset": 1 }, { "label": "B", "offset": 3 },
            { "label": "E", "offset": 4 }, { "label": "Z", "offset": 5 },
        ]));
    }
}

#[cfg(test)]
#[path = "library_index_tests.rs"]
mod tests;
