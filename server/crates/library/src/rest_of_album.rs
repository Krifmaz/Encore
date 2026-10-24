//! "Get the rest of the album" for an album MusicBrainz doesn't list: the same album found by its name on
//! the other catalogues the tagger already searches (Deezer, Apple Music, Discogs – the tag sheet's "By…"
//! choices, [`crate::meta::search`]), and a track list typed in by hand when none of them has it.
//!
//! The album's songs that aren't in the library yet are then requested one by one (each download source
//! searches for the song by artist and title), so an album missing from MusicBrainz never dead-ends.
use crate::Ctx;
use encore_core::json::s;
use serde_json::{json, Value};

/// The catalogues searched for an album MusicBrainz doesn't list, in the order they are asked: Deezer first
/// (its track lists carry ISRCs), then Apple Music, then Discogs (only with a Discogs token, which its search
/// needs). Each entry is (the [`crate::meta::search`] provider id, the name shown to people).
pub const ELSEWHERE: [(&str, &str); 3] = [("deezer", "Deezer"), ("apple", "Apple Music"), ("discogs", "Discogs")];

/// The most songs a typed track list or one "Request the missing songs" may hold.
pub const MAX_TRACKS: usize = 500;

/// Find the album `album` by `artist` on the catalogues in [`ELSEWHERE`], asked one after the other until one
/// lists it under the same title (edition notes aside) and a matching artist.
///
/// Returns the release as the Discover catalogue shapes one (`ext`, `title`, `artist`, `year`, `cover`,
/// `source`), or `None` when no catalogue has it. A catalogue that fails (down, throttled, no Discogs token)
/// is logged and skipped: it never stops the next one from being asked.
pub async fn find_album_elsewhere(st: &impl Ctx, artist: &str, album: &str) -> Option<Value> {
    for (provider, name) in ELSEWHERE {
        // Discogs' search needs a personal token: without one it isn't asked at all
        if provider == "discogs" && st.cfg().str("discogs_token").is_empty() {
            continue;
        }
        match crate::meta::search(st, provider, artist, album, 0, &[]).await {
            Ok(cands) => {
                if let Some(mut found) = pick_elsewhere(&cands, artist, album) {
                    found["source"] = json!(name);
                    return Some(found);
                }
            }
            Err(e) => st.db().log("warning", &format!("Looking up {artist} – {album} on {name} failed: {e}")),
        }
    }
    None
}

/// The candidate among one catalogue's search results (rows of [`crate::meta::search`]) that is this album:
/// the same [`crate::catalog::title_key`] and, when an artist is given, an artist that matches it
/// ([`crate::search::artist_grade`] either way round, so "A & B" matches "A").
///
/// Of several matches the one whose title is exactly the one asked for wins (so "Album" beats "Album
/// (Deluxe)"), then the one with the most songs. Returns the release shaped like the Discover catalogue's
/// (`ext`, `title`, `artist`, `year`, `cover`, `tracks`), or `None` when nothing matches.
pub fn pick_elsewhere(cands: &[Value], artist: &str, album: &str) -> Option<Value> {
    let want = crate::catalog::title_key(album);
    if want.is_empty() {
        return None;
    }
    let exact = album.trim().to_lowercase();
    cands.iter()
        .filter_map(|c| {
            let (a, t) = credit_and_title(c);
            let ext = ext_of(&s(&c["id"]))?;
            if crate::catalog::title_key(&t) != want {
                return None;
            }
            // no artist asked for: the title alone decides
            let same_artist = artist.trim().is_empty()
                || crate::search::artist_grade(&a, artist).max(crate::search::artist_grade(artist, &a)) > 0;
            same_artist.then_some((c, a, t, ext))
        })
        // exact title first, then the most songs (a deluxe edition holds the songs of the plain one)
        .max_by_key(|(c, _, t, _)| (t.trim().to_lowercase() == exact, c["track_count"].as_i64().unwrap_or(0)))
        .map(|(c, a, t, ext)| {
            let date = s(&c["date"]);
            json!({
                "ext": ext, "title": t, "artist": a, "date": date, "year": date.chars().take(4).collect::<String>(),
                "cover": c["cover"].clone(), "tracks": c["track_count"].clone(),
            })
        })
}

/// A search result's artist and title. Discogs leaves the artist empty and titles its results
/// "Artist - Album" (artist names with a number in brackets, "Artist (2)", for its look-alikes): that title is
/// split, and the bracketed number dropped.
fn credit_and_title(c: &Value) -> (String, String) {
    let (artist, title) = (s(&c["artist"]), s(&c["title"]));
    if !artist.is_empty() {
        return (artist, title);
    }
    match title.split_once(" - ") {
        Some((a, t)) => (strip_discogs_number(a.trim()), t.trim().to_string()),
        None => (String::new(), title),
    }
}

/// "Artist (2)" → "Artist": the number Discogs adds to tell artists of one name apart.
fn strip_discogs_number(a: &str) -> String {
    match a.rsplit_once(" (") {
        Some((name, n)) if n.strip_suffix(')').is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit())) => name.to_string(),
        _ => a.to_string(),
    }
}

/// The catalogue ids of a [`crate::meta::search`] release id, as the Discover catalogue's `ext` holds them:
/// "deezer:123" → `{"deezer": 123}`, "apple:456" → `{"apple": 456}`, "discogs:r789" → `{"discogs": "r789"}`
/// (Discogs ids keep their release / master letter). `None` for MusicBrainz ids and anything else.
pub fn ext_of(id: &str) -> Option<Value> {
    let (kind, rest) = id.split_once(':')?;
    match kind {
        "deezer" | "apple" => rest.parse::<i64>().ok().map(|n| json!({ kind: n })),
        "discogs" if rest.len() > 1 && (rest.starts_with('r') || rest.starts_with('m')) && rest[1..].chars().all(|c| c.is_ascii_digit()) => Some(json!({ "discogs": rest })),
        _ => None,
    }
}

/// A track list typed in by hand, one song per line, as the catalogue track lists are shaped (`title`,
/// `artist`, `track`, `disc`, `length`).
///
/// Each line may start with its number ("1.", "01 -", "1)", "A1") and end with its length ("3:45",
/// "(3:45)"); both are read and taken off the title. A line may name its own artist as "Artist - Title" only
/// when `artist_lines` is set (song titles have dashes in them too). Empty lines and repeats are left out,
/// and at most [`MAX_TRACKS`] songs are kept.
pub fn typed_tracklist(text: &str, artist_lines: bool) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for line in text.lines() {
        let (n, rest) = leading_number(line.trim());
        let (rest, length) = trailing_length(rest);
        let (artist, title) = match rest.split_once(" - ").filter(|_| artist_lines) {
            Some((a, t)) if !a.trim().is_empty() && !t.trim().is_empty() => (a.trim().to_string(), t.trim().to_string()),
            _ => (String::new(), rest.trim().to_string()),
        };
        if title.is_empty() || !seen.insert((artist.to_lowercase(), title.to_lowercase())) {
            continue;
        }
        let track = n.unwrap_or(out.len() as i64 + 1);
        out.push(json!({ "title": title, "artist": artist, "track": track, "disc": 1, "length": length, "isrc": null, "recording_id": null }));
        if out.len() >= MAX_TRACKS {
            break;
        }
    }
    out
}

/// A line's leading track number and the rest: "1. Song", "01 - Song", "1) Song", "A1 Song" (a vinyl side),
/// "#3 Song". A number that is the whole line, or a title that starts with a number and no separator
/// ("99 Problems"), stays the title.
fn leading_number(line: &str) -> (Option<i64>, &str) {
    // a length alone ("3:45") is not "3" and the title "45"
    if line.contains(':') && line.chars().all(|c| c.is_ascii_digit() || c == ':') {
        return (None, line);
    }
    let hash = line.starts_with('#');
    let body = line.strip_prefix('#').unwrap_or(line);
    // a vinyl side letter before the number ("A1", "B2")
    let side = body.chars().next().filter(|c| c.is_ascii_alphabetic() && body[1..].starts_with(|d: char| d.is_ascii_digit()));
    let digits_from = side.map_or(0, |c| c.len_utf8());
    let digits: String = body[digits_from..].chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() || digits.len() > 3 {
        return (None, line);
    }
    let after = &body[digits_from + digits.len()..];
    // a separator must follow ("1." "1)" "1 -" "1:"), or a space after a vinyl position ("A1 Song") or "#3"
    let sep = after.trim_start_matches([' ', '\t']);
    let rest = if let Some(r) = sep.strip_prefix(['.', ')', ':', '-', '–']) {
        r
    } else if (side.is_some() || hash) && after.starts_with([' ', '\t']) {
        sep
    } else {
        return (None, line);
    };
    let rest = rest.trim();
    if rest.is_empty() {
        return (None, line);
    }
    (digits.parse().ok(), rest)
}

/// A line's trailing length and the rest: "Song 3:45", "Song (3:45)", "Song [1:02:03]". `null` when none.
fn trailing_length(line: &str) -> (&str, Value) {
    let t = line.trim_end();
    let (inner, cut) = match (t.strip_suffix(')'), t.strip_suffix(']')) {
        (Some(x), _) => (x.rsplit_once('(').map(|(_, n)| n), x.rfind('(')),
        (_, Some(x)) => (x.rsplit_once('[').map(|(_, n)| n), x.rfind('[')),
        _ => (t.rsplit_once([' ', '\t']).map(|(_, n)| n), t.rfind([' ', '\t'])),
    };
    let (Some(inner), Some(cut)) = (inner, cut) else { return (line, Value::Null) };
    // m:ss or h:mm:ss, digits and colons only
    if !inner.contains(':') || !inner.chars().all(|c| c.is_ascii_digit() || c == ':') {
        return (line, Value::Null);
    }
    match crate::catalog::dur(&json!(inner)) {
        Value::Null => (line, Value::Null),
        // (a length with nothing before it is the title)
        l if !t[..cut].trim().is_empty() => (t[..cut].trim_end(), l),
        _ => (line, Value::Null),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Deezer search row as meta::search shapes it.
    fn dz(id: i64, title: &str, artist: &str, n: i64) -> Value {
        json!({ "id": format!("deezer:{id}"), "title": title, "artist": artist, "date": "2021-05-07", "track_count": n, "cover": "c", "provider": "Deezer" })
    }

    #[test]
    fn picks_the_same_album_by_title_and_artist() {
        let cands = vec![dz(1, "Other Album", "Band", 10), dz(2, "Sour", "Olivia Rodrigo", 11), dz(3, "Sour", "Someone Else", 3)];
        let got = pick_elsewhere(&cands, "Olivia Rodrigo", "SOUR").unwrap();
        assert_eq!(got["ext"], json!({ "deezer": 2 }));
        assert_eq!(got["title"], "Sour");
        assert_eq!(got["year"], "2021");
        // nothing with that title and artist
        assert!(pick_elsewhere(&cands, "Nobody", "Sour").is_none());
        assert!(pick_elsewhere(&cands, "Olivia Rodrigo", "Guts").is_none());
        // no artist: the title alone
        assert_eq!(pick_elsewhere(&cands, "", "Other Album").unwrap()["ext"], json!({ "deezer": 1 }));
    }

    #[test]
    fn exact_title_beats_an_edition_then_the_most_songs_win() {
        let cands = vec![dz(1, "Sour (Deluxe)", "Olivia Rodrigo", 14), dz(2, "Sour", "Olivia Rodrigo", 11)];
        assert_eq!(pick_elsewhere(&cands, "Olivia Rodrigo", "Sour").unwrap()["ext"], json!({ "deezer": 2 }));
        // asked for the deluxe edition: it is the exact one
        assert_eq!(pick_elsewhere(&cands, "Olivia Rodrigo", "Sour (Deluxe)").unwrap()["ext"], json!({ "deezer": 1 }));
        let two = vec![dz(1, "Sour (Clean)", "Olivia Rodrigo", 11), dz(2, "Sour (Deluxe)", "Olivia Rodrigo", 14)];
        assert_eq!(pick_elsewhere(&two, "Olivia Rodrigo", "Sour").unwrap()["ext"], json!({ "deezer": 2 }));
    }

    #[test]
    fn discogs_titles_carry_the_artist() {
        let cands = vec![json!({ "id": "discogs:r789", "title": "Boards Of Canada (2) - Geogaddi", "artist": "", "date": "2002", "track_count": 0, "provider": "Discogs" })];
        let got = pick_elsewhere(&cands, "Boards of Canada", "Geogaddi").unwrap();
        assert_eq!(got["ext"], json!({ "discogs": "r789" }));
        assert_eq!(got["artist"], "Boards Of Canada");
        assert_eq!(got["title"], "Geogaddi");
    }

    #[test]
    fn ext_ids() {
        assert_eq!(ext_of("deezer:302127"), Some(json!({ "deezer": 302127 })));
        assert_eq!(ext_of("apple:1440"), Some(json!({ "apple": 1440 })));
        assert_eq!(ext_of("discogs:m45"), Some(json!({ "discogs": "m45" })));
        assert_eq!(ext_of("discogs:x45"), None);
        assert_eq!(ext_of("deezer:abc"), None);
        assert_eq!(ext_of("0a1b2c3d-0000-0000-0000-000000000000"), None);
    }

    #[test]
    fn typed_lists_lose_numbers_and_lengths() {
        let text = "1. Intro (1:02)\n02 - Song Two 3:45\n3) Three\n\nA4 Side A Four\n#5 Five [4:00]\n99 Problems\nThree\n";
        let t = typed_tracklist(text, false);
        let titles: Vec<&str> = t.iter().map(|x| x["title"].as_str().unwrap()).collect();
        assert_eq!(titles, vec!["Intro", "Song Two", "Three", "Side A Four", "Five", "99 Problems"]);
        assert_eq!(t[0]["length"], json!(62.0));
        assert_eq!(t[1]["length"], json!(225.0));
        assert_eq!(t[2]["length"], Value::Null);
        assert_eq!(t[3]["track"], json!(4));
        // no number of its own: the line's place in the list
        assert_eq!(t[5]["track"], json!(6));
    }

    #[test]
    fn typed_lists_keep_dashes_unless_lines_name_artists() {
        let t = typed_tracklist("Song - Live\n", false);
        assert_eq!((t[0]["artist"].as_str().unwrap(), t[0]["title"].as_str().unwrap()), ("", "Song - Live"));
        let t = typed_tracklist("Guest - Song\n", true);
        assert_eq!((t[0]["artist"].as_str().unwrap(), t[0]["title"].as_str().unwrap()), ("Guest", "Song"));
        // a length alone, or a number alone, is a title
        assert_eq!(typed_tracklist("3:45\n1984\n", false).iter().map(|x| s(&x["title"])).collect::<Vec<_>>(), vec!["3:45", "1984"]);
    }
}
