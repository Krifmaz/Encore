//! LRCLIB (free, no key): song lyrics, time-synced or plain.
use crate::Ctx;
use regex::Regex;
use reqwest::RequestBuilder;
use serde_json::Value;
use std::sync::LazyLock;

/// The LRCLIB lyrics API (the default of `Urls::lrclib`; tests point that at a stand-in).
pub const API: &str = "https://lrclib.net/api";

/// A title without "(feat. …)", "[with …]" or "(… remaster)", the way LRCLIB files songs.
fn clean(s: &str) -> String {
    // a bracket that starts with a featuring word, or that names a remaster anywhere ("[2011 Remaster]")
    static RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\s*[\(\[]((feat|ft|with)\b[^\)\]]*|[^\)\]]*remaster[^\)\]]*)[\)\]]").unwrap());
    RE.replace_all(s, "").trim().to_string()
}

/// Words in a title's brackets or after " - " that name a different recording of a song: a remix, a live take,
/// an acoustic version… Their lyrics (and above all their timing) differ from the original's.
const VERSION_WORDS: &[&str] = &[
    "remix", "rmx", "mix", "live", "acoustic", "unplugged", "instrumental", "karaoke", "demo", "edit", "extended",
    "version", "rework", "reprise", "vip", "bootleg", "flip", "slowed", "sped", "nightcore", "reverb", "cover",
];

/// The version words ([`VERSION_WORDS`]) a title carries, in its brackets or after " - " (the plain title
/// itself never counts: "Mix Tape" is a song, "Song (Club Mix)" a version), sorted and without repeats.
fn version_of(title: &str) -> Vec<&'static str> {
    let lower = title.to_lowercase();
    let mut parts: Vec<&str> = Vec::new();
    // what is inside (…) or […]
    let mut depth = 0usize;
    let mut start = 0usize;
    for (i, c) in lower.char_indices() {
        match c {
            '(' | '[' => {
                if depth == 0 {
                    start = i + 1;
                }
                depth += 1;
            }
            ')' | ']' if depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    parts.push(&lower[start..i]);
                }
            }
            _ => {}
        }
    }
    // "Song - Club Mix": everything after the first " - "
    if let Some((_, rest)) = lower.split_once(" - ") {
        parts.push(rest);
    }
    let mut out: Vec<&'static str> = VERSION_WORDS.iter().copied()
        .filter(|w| parts.iter().any(|p| p.split(|c: char| !c.is_alphanumeric()).any(|x| x == *w)))
        .collect();
    out.sort_unstable();
    out
}

/// Is a search hit the same recording as the song asked for: the same version words ([`version_of`]) in both
/// titles? A remix never gets the original's lyrics, nor the original a remix's. A hit without a title passes.
fn same_version(asked: &[&str], hit: &Value) -> bool {
    hit["trackName"].as_str().is_none_or(|name| version_of(name) == asked)
}

/// LRCLIB's free-text search (words of the lyrics, a title): a list of songs.
pub fn search(st: &impl Ctx, q: &str) -> RequestBuilder {
    st.http().get(format!("{}/search", st.urls().lrclib)).query(&[("q", q)])
}

/// LRCLIB's lyrics for a song: the exact match (artist, title, album, length) first, else the best search
/// hit within 3 s of the length (time-synced before plain).
pub async fn lookup(st: &impl Ctx, artist: &str, title: &str, album: &str, duration: Option<f64>) -> anyhow::Result<Option<Value>> {
    lookup_with(st, artist, title, album, duration, Some, false).await
}

/// [`lookup`] with a screen: `keep` gets every version LRCLIB offers and returns it (perhaps with its
/// time-synced or plain text taken out) or None to skip it – how a lyrics report keeps the lyrics it
/// rejected from coming back. `picky` (some versions were rejected) also searches when the exact match
/// is left without time-synced lyrics, so another version's synced lines win over plain ones.
pub async fn lookup_with(st: &impl Ctx, artist: &str, title: &str, album: &str, duration: Option<f64>,
                         keep: impl Fn(Value) -> Option<Value>, picky: bool) -> anyhow::Result<Option<Value>> {
    let mut q = vec![("artist_name", artist.to_string()), ("track_name", clean(title))];
    if !album.is_empty() {
        q.push(("album_name", album.into()));
    }
    if let Some(d) = duration.filter(|d| *d > 0.0) {
        q.push(("duration", (d.round() as i64).to_string()));
    }
    let r = st.http().get(format!("{}/get", st.urls().lrclib)).query(&q).send().await?;
    let mut exact = None;
    if r.status().is_success() {
        let d: Value = r.json().await?;
        if usable(&d) {
            exact = keep(d).filter(usable);
        }
    }
    // the exact match wins unless a report made us picky and it has no time-synced lines left
    if let Some(d) = &exact {
        if !picky || d["syncedLyrics"].is_string() {
            return Ok(exact);
        }
    }
    let r = st.http().get(format!("{}/search", st.urls().lrclib)).query(&[("artist_name", artist.to_string()), ("track_name", clean(title))]).send().await?;
    let hits: Vec<Value> = if r.status().is_success() { r.json::<Value>().await?.as_array().cloned().unwrap_or_default() } else { vec![] };
    // the search is fuzzy: "Song (Remix)" finds the original too, and its lines run at the original's pace
    let asked = version_of(title);
    let hits = hits.into_iter().filter(|h| same_version(&asked, h)).collect();
    Ok(best(hits, exact, duration, keep))
}

/// A version with something to show: time-synced or plain lyrics, or the "instrumental" flag.
fn usable(d: &Value) -> bool {
    d["syncedLyrics"].is_string() || d["plainLyrics"].is_string() || d["instrumental"].as_bool().unwrap_or(false)
}

/// The best of the search hits (screened by `keep`, within 3 s of the length) and the exact match left
/// over (`exact`, already screened): time-synced before plain, the exact match first among equals.
fn best(hits: Vec<Value>, exact: Option<Value>, duration: Option<f64>, keep: impl Fn(Value) -> Option<Value>) -> Option<Value> {
    let mut all: Vec<Value> = exact.into_iter().collect();
    all.extend(hits.into_iter()
        .filter(|h| duration.filter(|d| *d > 0.0).is_none_or(|d| h["duration"].as_f64().is_none_or(|x| (x - d).abs() <= 3.0)))
        .filter_map(&keep)
        .filter(usable));
    // a stable sort: among versions alike, the exact match (first) and then LRCLIB's order stay
    all.sort_by_key(|h| (h["syncedLyrics"].is_null(), h["plainLyrics"].is_null()));
    all.into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Synced versions beat plain ones, the length filter drops far-off versions, and a screen that
    /// rejects one version lets the next one through.
    #[test]
    fn best_prefers_synced_and_honours_the_screen() {
        let hits = vec![
            json!({ "id": 1, "duration": 200.0, "plainLyrics": "plain one" }),
            json!({ "id": 2, "duration": 201.0, "syncedLyrics": "[00:01.00] two", "plainLyrics": "two" }),
            json!({ "id": 3, "duration": 260.0, "syncedLyrics": "[00:01.00] far" }),
            json!({ "id": 4, "duration": 199.0, "syncedLyrics": "[00:02.00] four" }),
        ];
        assert_eq!(best(hits.clone(), None, Some(200.0), Some).unwrap()["id"], 2);
        let no_two = |h: Value| (h["id"] != 2).then_some(h);
        assert_eq!(best(hits.clone(), None, Some(200.0), no_two).unwrap()["id"], 4);
        let plain_only = |mut h: Value| { h["syncedLyrics"] = Value::Null; usable(&h).then_some(h) };
        assert_eq!(best(hits.clone(), None, Some(200.0), plain_only).unwrap()["id"], 1);
        // the exact match left with plain lyrics only still loses to a synced search hit
        assert_eq!(best(hits, Some(json!({ "id": 9, "plainLyrics": "x" })), Some(200.0), Some).unwrap()["id"], 2);
        assert!(best(vec![], None, None, Some).is_none());
    }

    /// Remixes, live takes and other versions are told apart by the words in their brackets or after " - ".
    #[test]
    fn versions_are_told_apart() {
        assert_eq!(version_of("hand crushed by a mallet (remix)"), vec!["remix"]);
        assert_eq!(version_of("hand crushed by a mallet - Remix"), vec!["remix"]);
        assert_eq!(version_of("hand crushed by a mallet (feat. Fall Out Boy) [Remix]"), vec!["remix"]);
        assert_eq!(version_of("Song (Live at Wembley)"), vec!["live"]);
        assert_eq!(version_of("Song (Club Mix) [Live]"), vec!["live", "mix"]);
        assert!(version_of("hand crushed by a mallet").is_empty());
        // words of the title itself are no version
        assert!(version_of("Mix Tape").is_empty());
        assert!(version_of("Live Forever").is_empty());

        let remix = version_of("hand crushed by a mallet (remix)");
        assert!(same_version(&remix, &json!({ "trackName": "hand crushed by a mallet (Remix) [feat. Fall Out Boy]" })));
        assert!(!same_version(&remix, &json!({ "trackName": "hand crushed by a mallet" })));
        assert!(!same_version(&version_of("hand crushed by a mallet"), &json!({ "trackName": "hand crushed by a mallet (remix)" })));
        assert!(same_version(&remix, &json!({ "id": 1 })));
    }

    /// The cleaned title LRCLIB is asked for.
    #[test]
    fn clean_drops_features_and_remasters() {
        assert_eq!(clean("Song (feat. Someone) [2011 Remaster]"), "Song");
        assert_eq!(clean("Song (Remastered 2009)"), "Song");
        // a word that only starts like one stays
        assert_eq!(clean("Song (Without You)"), "Song (Without You)");
    }
}
