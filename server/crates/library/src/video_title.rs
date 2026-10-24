//! The song behind a video: who sings it and what it is called, read from a YouTube (or SoundCloud,
//! Bandcamp …) upload's own title, its uploader and the tags yt-dlp found.
//!
//! An upload names its uploader, and the uploader is often not the song's artist: a lyrics channel, a fan,
//! a label, a "VEVO" or "- Topic" channel. A playlist imported with the uploader as the artist matches
//! nothing in the library and gets downloaded again, as a second copy of a song that is already there.
//! [`parse`] takes the artist from, best first:
//! 1. the artist tag yt-dlp read (YouTube Music songs carry one);
//! 2. the video title's "Artist - Title" (or `Artist "Title"`), the other way round when the uploader is the
//!    part after the dash ("Title - Artist");
//! 3. the uploader, cleaned of " - Topic", "VEVO", "Official" … and marked [`VideoSong::artist_guessed`]
//!    unless it is an official channel: such an item is also matched by its title and length alone
//!    ([`crate::matcher::LibraryIndex::find_by_title`]).
//!
//! The title loses the words that only describe the upload ("(Official Music Video)", "[HD]", "| Lyrics");
//! version words ("(Live)", "(Remix)") stay, the matcher tells versions apart by them.
use crate::matcher::fold;
use regex::Regex;
use std::sync::LazyLock;

/// What a video is, as far as its title, uploader and tags tell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoSong {
    /// The song's title, without the upload's own words ("Official Video", "Lyrics", "HD" …).
    pub title: String,
    /// The song's artist ('' when nothing names one).
    pub artist: String,
    /// The artist is only the uploader's name: nothing else (a tag, the title, an official channel) says so.
    pub artist_guessed: bool,
}

/// Words that describe the upload, not the song: a bracketed group made only of them is dropped
/// ("(Official Music Video)", "[4K Remastered Video]" keeps nothing; "(Live at Wembley)" is kept).
const UPLOAD_WORDS: &str = r"official|officiel|oficial|music|musique|video|vid[eé]o|clip|lyrics?|lyric video|letra|paroles|audio|visuali[sz]er|hd|hq|4k|8k|1080p|720p|full|version|mv|m/v|color coded|with|sub|subs|subtitles|subtitulado|legendado|espa[nñ]ol|english|eng|remastered|in|the|and|&|\+|/|-|,|\.";

/// One bracketed group, round, square or Japanese corner brackets.
static GROUP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*[\(\[【]([^\)\]】]*)[\)\]】]").unwrap());
/// A group's text that is upload words only (whole words, any order).
static ONLY_UPLOAD: LazyLock<Regex> = LazyLock::new(|| Regex::new(&format!(r"(?i)^\s*(?:(?:{UPLOAD_WORDS})\s*)+$")).unwrap());
/// Upload words that end a title without brackets ("Song Official Video", "Song - Lyrics", "Song | HD").
static TRAILING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:\s*[-–—|:]\s*|\s+)(?:official\s+(?:music\s+)?(?:video|audio|lyric video|visuali[sz]er)|(?:with\s+)?lyrics?(?:\s+video)?|music\s+video|lyric\s+video|hd|hq|4k)\s*$").unwrap()
});
/// "Artist - Title" (also with an en or em dash).
static DASH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+[-–—]\s+").unwrap());
/// `Artist "Title"` / `Artist 「Title」`, then whatever follows the closing quote.
/// Corner brackets need no space before them ("YOASOBI「アイドル」").
static QUOTED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"^(.+?)(?:\s+["“«]|\s*[「『])(.+?)["”»」』]\s*(.*)$"#).unwrap());
/// "VEVO" at the end of a channel name ("TaylorSwiftVEVO", "Adele VEVO").
static VEVO: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\s*vevo$").unwrap());
/// "Official" (or "Official Channel", "Official Artist Channel") at the end of a channel name.
static OFFICIAL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\s*(?:-\s*)?official(?:\s+artist)?(?:\s+(?:channel|youtube))?$").unwrap());
/// A lower-case letter followed by an upper-case one: where "TaylorSwift" splits into words.
static CAMEL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\p{Ll})(\p{Lu})").unwrap());

/// The artist an uploader's name stands for, and whether the channel is the artist's own (official, a
/// "- Topic" channel YouTube makes from the label's delivery, a VEVO channel). "TaylorSwiftVEVO" →
/// ("Taylor Swift", true); "Queen - Topic" → ("Queen", true); "7clouds" → ("7clouds", false).
pub fn channel_artist(channel: &str) -> (String, bool) {
    let ch = channel.trim();
    if let Some(a) = ch.strip_suffix(" - Topic") {
        return (a.trim().to_string(), true);
    }
    for re in [&*VEVO, &*OFFICIAL] {
        if re.is_match(ch) {
            let a = re.replace(ch, "").trim().to_string();
            if a.is_empty() {
                break;
            }
            // a channel name written as one word ("TaylorSwift") gets its spaces back
            let a = if a.contains(' ') { a } else { CAMEL.replace_all(&a, "$1 $2").to_string() };
            return (a, true);
        }
    }
    (ch.to_string(), false)
}

/// The title without the words that only describe the upload: bracketed groups of upload words, upload
/// words at its end, and " | "-separated parts that are only upload words or the uploader's name.
pub fn clean_title(title: &str, channel: &str) -> String {
    let no_groups = GROUP.replace_all(title, |c: &regex::Captures| if ONLY_UPLOAD.is_match(&c[1]) { String::new() } else { c[0].to_string() }).to_string();
    let ch = same_name_key(channel);
    // "Song | Official Video | Artist": the parts after the first that say nothing of the song go
    let mut parts = no_groups.split(" | ").map(str::trim).filter(|p| !p.is_empty());
    let mut out = parts.next().unwrap_or("").to_string();
    for p in parts {
        if !ONLY_UPLOAD.is_match(p) && same_name_key(p) != ch {
            out.push_str(" | ");
            out.push_str(p);
        }
    }
    // several trailing marks ("Song - Lyrics HD") go one after the other
    loop {
        let next = TRAILING.replace(&out, "").to_string();
        if next == out {
            break;
        }
        out = next;
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A name folded for "is this the same name": accents, case, punctuation and spaces left out, so a channel
/// written "TaylorSwift" and the artist "Taylor Swift" compare equal.
fn same_name_key(s: &str) -> String {
    fold(s).replace(' ', "")
}

/// Is `a` the name `b` (the uploader) stands for? Equal once folded, or one inside the other when the
/// shorter has at least 4 letters ("Coldplay" in "ColdplayVEVO" is caught before, "Adele" in "AdeleMusic").
fn names_match(a: &str, b: &str) -> bool {
    let (x, y) = (same_name_key(a), same_name_key(b));
    !x.is_empty() && !y.is_empty() && (x == y || (x.len().min(y.len()) >= 4 && (x.contains(&y) || y.contains(&x))))
}

/// The song behind a video: its `title` as uploaded, its uploader (`channel`), and the artist and track
/// tags yt-dlp read ('' when it read none). See the module comment for the order the artist is taken in.
pub fn parse(title: &str, channel: &str, artist_tag: &str, track_tag: &str) -> VideoSong {
    let (artist_tag, track_tag) = (artist_tag.trim(), track_tag.trim());
    let (ch_artist, official) = channel_artist(channel);
    let cleaned = clean_title(title, channel);
    if !artist_tag.is_empty() {
        // the tags say who; the track tag (or the title past "Artist - ") says what
        let title = if !track_tag.is_empty() {
            clean_title(track_tag, channel)
        } else {
            match DASH.splitn(&cleaned, 2).collect::<Vec<_>>()[..] {
                [a, b] if names_match(a, artist_tag) => b.to_string(),
                _ => cleaned,
            }
        };
        return VideoSong { title, artist: artist_tag.to_string(), artist_guessed: false };
    }
    // a "- Topic" upload is titled with the song alone: "Song - 2011 Remaster" has no artist in it
    let topic = channel.trim().ends_with(" - Topic");
    if !topic {
        if let [a, b] = DASH.splitn(&cleaned, 2).collect::<Vec<_>>()[..] {
            let (a, b) = (a.trim(), b.trim());
            // "Title - Artist" by the artist's channel: the uploader is the part after the dash
            if names_match(b, &ch_artist) && !names_match(a, &ch_artist) {
                return VideoSong { title: a.to_string(), artist: b.to_string(), artist_guessed: false };
            }
            if !a.is_empty() && !b.is_empty() {
                return VideoSong { title: b.to_string(), artist: a.to_string(), artist_guessed: false };
            }
        }
        if let Some(c) = QUOTED.captures(&cleaned) {
            // what follows the closing quote, unless it only describes the upload ("「Song」Official Video")
            let rest = Some(c[3].trim()).filter(|r| !ONLY_UPLOAD.is_match(r)).unwrap_or("");
            let title = if rest.is_empty() { c[2].trim().to_string() } else { format!("{} {rest}", c[2].trim()) };
            return VideoSong { title, artist: c[1].trim().to_string(), artist_guessed: false };
        }
    }
    let title = if track_tag.is_empty() { cleaned } else { clean_title(track_tag, channel) };
    VideoSong { title, artist: ch_artist.clone(), artist_guessed: !official && !ch_artist.is_empty() }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shorthand: parse a video with no tags → (title, artist, guessed).
    fn p(title: &str, channel: &str) -> (String, String, bool) {
        let v = parse(title, channel, "", "");
        (v.title, v.artist, v.artist_guessed)
    }

    /// The artist named in the title wins over the uploader, the upload's own words go.
    #[test]
    fn the_title_names_the_artist() {
        assert_eq!(p("Rick Astley - Never Gonna Give You Up (Official Music Video)", "Rick Astley"), ("Never Gonna Give You Up".into(), "Rick Astley".into(), false));
        assert_eq!(p("Daft Punk - One More Time [HD] | Official Video", "some fan"), ("One More Time".into(), "Daft Punk".into(), false));
        assert_eq!(p("Avicii – Levels (Lyrics)", "7clouds"), ("Levels".into(), "Avicii".into(), false));
        assert_eq!(p("The Weeknd ft. Daft Punk - Starboy (Official Video)", "TheWeekndVEVO"), ("Starboy".into(), "The Weeknd ft. Daft Punk".into(), false));
        assert_eq!(p(r#"Adele "Hello" (Live at the BBC)"#, "BBC Music"), ("Hello (Live at the BBC)".into(), "Adele".into(), false));
        assert_eq!(p("YOASOBI「アイドル」Official Music Video", "Ayase / YOASOBI"), ("アイドル".into(), "YOASOBI".into(), false));
    }

    /// "Title - Artist" on the artist's own channel is turned round.
    #[test]
    fn the_artist_after_the_dash() {
        assert_eq!(p("Bohemian Rhapsody - Queen", "Queen Official"), ("Bohemian Rhapsody".into(), "Queen".into(), false));
    }

    /// Version words stay: the matcher needs them to tell a live take or a remix from the original.
    #[test]
    fn versions_stay() {
        assert_eq!(p("Oasis - Wonderwall (Live at Knebworth) [HD]", "x").0, "Wonderwall (Live at Knebworth)");
        assert_eq!(p("Lorde - Royals (Kyle Remix) (Official Audio)", "x").0, "Royals (Kyle Remix)");
    }

    /// Official channels name their artist; any other uploader is only a guess.
    #[test]
    fn the_uploader_as_the_artist() {
        assert_eq!(p("Shake It Off", "TaylorSwiftVEVO"), ("Shake It Off".into(), "Taylor Swift".into(), false));
        assert_eq!(p("Never Gonna Give You Up", "Rick Astley - Topic"), ("Never Gonna Give You Up".into(), "Rick Astley".into(), false));
        // a Topic upload's " - " is part of the song's title, not an artist
        assert_eq!(p("Wonderwall - Remastered", "Oasis - Topic"), ("Wonderwall - Remastered".into(), "Oasis".into(), false));
        assert_eq!(p("Blinding Lights (Lyrics)", "7clouds"), ("Blinding Lights".into(), "7clouds".into(), true));
        assert_eq!(p("Blinding Lights lyrics", "Lyrical Lemonade Fan"), ("Blinding Lights".into(), "Lyrical Lemonade Fan".into(), true));
        assert_eq!(channel_artist("Coldplay Official Channel"), ("Coldplay".into(), true));
        assert_eq!(channel_artist("Official"), ("Official".into(), false));
    }

    /// yt-dlp's tags (YouTube Music songs) win over everything.
    #[test]
    fn tags_win() {
        let v = parse("The Weeknd - Blinding Lights (Official Video)", "TheWeekndVEVO", "The Weeknd", "");
        assert_eq!((v.title.as_str(), v.artist.as_str(), v.artist_guessed), ("Blinding Lights", "The Weeknd", false));
        let v = parse("whatever", "fan", "Queen", "Bohemian Rhapsody (Remastered 2011)");
        assert_eq!((v.title.as_str(), v.artist.as_str()), ("Bohemian Rhapsody (Remastered 2011)", "Queen"));
    }

    /// Parts after " | " that are the uploader's name or upload words go; others stay.
    #[test]
    fn pipe_parts() {
        assert_eq!(clean_title("Song | Lyrics | 7clouds", "7clouds"), "Song");
        assert_eq!(clean_title("Song | Part 2", "x"), "Song | Part 2");
        assert_eq!(clean_title("Song - Lyrics HD", "x"), "Song");
    }
}
