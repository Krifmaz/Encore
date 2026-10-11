//! Search that understands sentences: "the song blinding lights by the weeknd", "radiohead album ok computer",
//! "that song that goes is this the real life" (found through its lyrics).
use regex::Regex;
use std::sync::LazyLock;

pub struct Parsed {
    pub q: String,
    pub title: String,
    pub artist: String,
    pub kind: Option<&'static str>,
    pub lyrics: Option<String>,
    pub sentence: bool,
}

const KINDS: &[(&str, &str)] = &[("song", "track"), ("songs", "track"), ("track", "track"), ("tracks", "track"), ("single", "track"),
    ("album", "album"), ("albums", "album"), ("ep", "album"), ("record", "album"), ("lp", "album"),
    ("artist", "artist"), ("band", "artist"), ("singer", "artist"), ("rapper", "artist"), ("group", "artist")];

static FILLER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(play|find|search( for)?|look( for| up)?|get|download|i('m| am)? looking for|i want( to hear)?|can you|could you|please|the|a|an|that|which|called|named|titled|some|me|music|from|of)\b").unwrap());
static LYRIC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(that goes|goes like|lyrics?|which goes|where (he|she|they) (says?|sings?))\b").unwrap());
static QUOTED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""([^"]+)""#).unwrap());
static SPACES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());

fn tidy(s: &str) -> String {
    SPACES.replace_all(s, " ").trim_matches(|c: char| " ,.?!".contains(c)).to_string()
}

pub fn parse(raw: &str) -> Parsed {
    let raw = raw.trim();
    let quoted = QUOTED.captures(raw).map(|c| c[1].to_string());
    let mut text = raw.to_lowercase();
    let mut lyrics = None;
    if let Some(m) = LYRIC.find(&text) {
        let after = raw.get(m.end()..).unwrap_or("").trim_matches(|c: char| " ,.:\"'".contains(c)).to_string();
        lyrics = if after.is_empty() { None } else { Some(after) };
        text = text[..m.start()].to_string();
    }
    let mut kind = None;
    for (w, k) in KINDS {
        let re = Regex::new(&format!(r"\b{w}\b")).unwrap();
        if re.is_match(&text) {
            kind = kind.or(Some(*k));
            text = re.replace_all(&text, " ").into_owned();
        }
    }
    let (mut title, mut artist) = (String::new(), String::new());
    let padded = format!(" {text} ");
    if let Some((l, r)) = padded.split_once(" by ") {
        title = tidy(&FILLER.replace_all(l, " "));
        artist = tidy(&Regex::new(r"\b(the artist|the band)\b").unwrap().replace_all(r, " "));
    }
    let mut clean = tidy(&FILLER.replace_all(&text.replace('"', " "), " "));
    if let Some(q) = quoted {
        title = q;
    }
    if !artist.is_empty() {
        clean = tidy(&format!("{title} {artist}"));
    }
    if clean.is_empty() {
        if let Some(l) = &lyrics {
            clean = l.clone();
        }
    }
    let sentence = raw.split_whitespace().count() >= 4 || kind.is_some() || !artist.is_empty() || lyrics.is_some();
    Parsed { q: if clean.is_empty() { raw.to_string() } else { clean }, title, artist, kind, lyrics, sentence }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sentences() {
        let p = parse("the song blinding lights by the weeknd");
        assert_eq!((p.title.as_str(), p.artist.as_str(), p.kind), ("blinding lights", "the weeknd", Some("track")));
        assert_eq!(parse("that song that goes is this the real life").lyrics.as_deref(), Some("is this the real life"));
        assert_eq!(parse("play some music by daft punk").q, "daft punk");
        assert!(!parse("Blinding Lights").sentence);
    }
}
