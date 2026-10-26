//! What a path or a new name sent by a client may be, checked before anything touches the disk.
//!
//! A path is relative to its root, `/`-separated, "" for the root itself. It is refused when it is absolute,
//! has a `..` part, holds a NUL, or has a part that a Unicode normalization would turn into `.`, `..` or a
//! name with a slash ("．．", "a∕b" with the division slash, the fullwidth solidus): the kernel would take
//! them as plain names, but a layer between (a file system that normalizes names, a copy tool) may not.
//! Percent-encoded paths (`%2e%2e`) are decoded once by the query parser and then refused as `..`; they are
//! never decoded twice.
//!
//! A new name (mkdir, rename, an uploaded file, each part of a folder upload) is one path part: not empty, no
//! `/` or `\`, not `.` or `..`, at most 255 bytes, and no control characters or characters that reorder how
//! the text is shown (the bidi overrides and isolates), so a name always reads as what it is.
use encore_core::error::{err, ApiError};
use unicode_normalization::UnicodeNormalization;

/// The longest name most file systems take, in bytes.
pub const NAME_MAX: usize = 255;
/// The deepest path a client may name (folder uploads included).
pub const DEPTH_MAX: usize = 64;

/// The answer to a path that leaves its folder (absolute, `..`, a link out of the roots).
pub const OUTSIDE: &str = "That path is outside the folders Encore may use";
/// The answer to a write through a link (§2.4 of the client shapes).
pub const THROUGH_LINK: &str = "That folder is reached through a link, so Encore doesn't change anything in it";
/// The answer to browsing a link whose target is in none of the roots.
pub const LINK_OUTSIDE: &str = "That link points outside the folders Encore may use";
/// A name that is taken in the folder.
pub const TAKEN: &str = "A file with that name is already there";
/// A folder in the way of an uploaded file.
pub const FOLDER_TAKEN: &str = "A folder with that name is already there";
/// A path that names nothing.
pub const MISSING: &str = "That file or folder isn't there";
/// A path in Encore's own config folder (its database, backups and Trash), reached through a root that holds it.
pub const CONFIG_FOLDER: &str = "That is Encore's own config folder or its Trash: the file manager leaves it alone here (open Trash for what it holds)";

/// Characters a name may not hold: control characters (C0, DEL, C1), the line and paragraph separators, and
/// the bidi embeddings, overrides and isolates (they make "song\u{202e}calf.exe" read as "songexe.flac").
fn hidden(c: char) -> bool {
    c.is_control() || matches!(c, '\u{2028}' | '\u{2029}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

/// The name as compatibility normalization (NFKC) sees it: what "．．" or "a／b" become.
fn folded(s: &str) -> String {
    s.nfkc().collect()
}

/// A part a normalizing layer would read as `.`, `..` or two names.
fn tricky(part: &str) -> bool {
    if part.is_ascii() {
        return false;
    }
    let f = folded(part);
    f == "." || f == ".." || f.contains('/') || f.contains('\\') || f.contains('\0')
}

/// Checks a new name (one path part).
pub fn check_name(name: &str) -> Result<(), ApiError> {
    if name.trim().is_empty() {
        return Err(err(400, "A name can't be empty"));
    }
    if name.contains('/') || name.contains('\\') {
        return Err(err(400, "A name can't contain / or \\"));
    }
    if name == "." || name == ".." {
        return Err(err(400, "That name isn't allowed"));
    }
    if name.len() > NAME_MAX {
        return Err(err(400, "That name is too long"));
    }
    if name.chars().any(hidden) {
        return Err(err(400, "A name can't contain invisible or control characters"));
    }
    if tricky(name) {
        return Err(err(400, "A name can't contain / or \\"));
    }
    Ok(())
}

/// The parts of a client's path, checked: `""` and `"/"`-separated names, empty parts and `.` skipped.
pub fn parse_rel(rel: &str) -> Result<Vec<String>, ApiError> {
    if rel.contains('\0') {
        return Err(err(400, "A path can't contain a NUL character"));
    }
    if rel.starts_with('/') || rel.starts_with('\\') {
        return Err(err(403, OUTSIDE));
    }
    let mut out = vec![];
    for part in rel.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." || tricky(part) {
            return Err(err(403, OUTSIDE));
        }
        if part.len() > NAME_MAX {
            return Err(err(400, "That name is too long"));
        }
        out.push(part.to_string());
    }
    if out.len() > DEPTH_MAX {
        return Err(err(400, "That path is too deep"));
    }
    Ok(out)
}

/// The parts joined again: the path as the API gives it back ("Artist/Album").
pub fn join(parts: &[String]) -> String {
    parts.join("/")
}

/// The next free name for `name` in a folder: "Song.flac" → "Song (2).flac", "Song (3).flac" … (a folder
/// "Album" → "Album (2)"). `taken` says whether a name is in use.
pub fn free_name(name: &str, is_dir: bool, mut taken: impl FnMut(&str) -> bool) -> String {
    if !taken(name) {
        return name.to_string();
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 && !is_dir => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    let mut i = 2;
    loop {
        let candidate = format!("{stem} ({i}){ext}");
        // a long name loses the end of its stem, never its extension
        let candidate = if candidate.len() > NAME_MAX {
            let room = NAME_MAX.saturating_sub(format!(" ({i}){ext}").len());
            let mut cut = room.min(stem.len());
            while !stem.is_char_boundary(cut) {
                cut -= 1;
            }
            format!("{} ({i}){ext}", &stem[..cut])
        } else {
            candidate
        };
        if !taken(&candidate) {
            return candidate;
        }
        i += 1;
    }
}

/// Is this file a picture the apps show (jpg, png, webp, gif)?
pub fn is_image(name: &str) -> bool {
    matches!(ext_of(name).as_str(), "jpg" | "jpeg" | "png" | "webp" | "gif")
}

/// Is this file a song the library takes (its extension is one the scan picks up)?
pub fn is_audio(name: &str) -> bool {
    let e = ext_of(name);
    encore_core::audio::AUDIO_EXTS.contains(&e.as_str()) || matches!(e.as_str(), "dsf" | "dff")
}

/// The lowercase extension of a name ("" without one).
pub fn ext_of(name: &str) -> String {
    match name.rfind('.') {
        Some(i) if i > 0 => name[i + 1..].to_lowercase(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_that_leave_their_folder_are_refused_before_the_disk() {
        for bad in ["/etc/passwd", "\\etc", "..", "../x", "a/../../b", "a/..", "．．/x", "a/．．", "a／..", "x/\u{ff0e}\u{ff0e}"] {
            let e = parse_rel(bad).unwrap_err();
            assert_eq!((e.status.as_u16(), e.detail.as_str()), (403, OUTSIDE), "{bad}");
        }
        // the query parser decoded %2e%2e once: it is ".." now
        assert_eq!(parse_rel("%2e%2e").unwrap(), vec!["%2e%2e"], "a name that still reads %2e%2e is only a name");
        assert_eq!(parse_rel("a/b\0c").unwrap_err().status.as_u16(), 400);
        assert_eq!(parse_rel("").unwrap(), Vec::<String>::new());
        assert_eq!(parse_rel("./Artist//Album/").unwrap(), vec!["Artist", "Album"]);
        assert_eq!(parse_rel("Ärtist/Ålbum (Deluxe)/01 – Song.flac").unwrap().len(), 3);
        assert_eq!(parse_rel(&"a/".repeat(65)).unwrap_err().status.as_u16(), 400);
    }

    #[test]
    fn names_are_one_plain_part() {
        for good in ["01 Song.flac", "Album (2006)", ".hidden", "Ünïcödé 歌", "a.b.c", "x".repeat(255).as_str(), "emoji 👨‍👩‍👧"] {
            check_name(good).unwrap_or_else(|e| panic!("{good}: {}", e.detail));
        }
        let cases = [("", "A name can't be empty"), ("   ", "A name can't be empty"), ("a/b", "A name can't contain / or \\"),
            ("a\\b", "A name can't contain / or \\"), (".", "That name isn't allowed"), ("..", "That name isn't allowed"),
            ("a\0b", "A name can't contain invisible or control characters"), ("a\nb", "A name can't contain invisible or control characters"),
            ("song\u{202e}calf.exe", "A name can't contain invisible or control characters"), ("a\u{2066}b", "A name can't contain invisible or control characters"),
            ("a\u{85}b", "A name can't contain invisible or control characters"), ("a\u{ff0f}b", "A name can't contain / or \\"),
("．．", "A name can't contain / or \\")];
        for (bad, why) in cases {
            let e = check_name(bad).unwrap_err();
            assert_eq!((e.status.as_u16(), e.detail.as_str()), (400, why), "{bad:?}");
        }
        assert_eq!(check_name(&"é".repeat(128)).unwrap_err().detail, "That name is too long", "256 bytes");
        // slash look-alikes that no normalization turns into a slash stay: yt-dlp names "AC/DC" files "AC⧸DC"
        check_name("AC\u{29f8}DC - T.N.T.opus").unwrap();
        check_name("1\u{2215}2").unwrap();
    }

    #[test]
    fn free_names_count_up_before_the_extension() {
        let taken = ["Song.flac", "Song (2).flac", "Album", ".hidden"];
        let t = |n: &str| taken.contains(&n);
        assert_eq!(free_name("Song.flac", false, t), "Song (3).flac");
        assert_eq!(free_name("New.flac", false, t), "New.flac");
        assert_eq!(free_name("Album", true, t), "Album (2)");
        assert_eq!(free_name("Album.v2", true, |n| n == "Album.v2"), "Album.v2 (2)", "a folder's dot is part of its name");
        assert_eq!(free_name(".hidden", false, t), ".hidden (2)");
        let long = format!("{}.flac", "x".repeat(250));
        let n = free_name(&long, false, |n| n == long);
        assert!(n.len() <= NAME_MAX && n.ends_with(" (2).flac"), "{n}");
    }

    #[test]
    fn kinds_of_files() {
        assert!(is_audio("a.FLAC") && is_audio("b.m4a") && is_audio("c.dsf") && !is_audio("cover.jpg") && !is_audio("flac"));
        assert!(is_image("cover.JPG") && is_image("a.webp") && !is_image("a.flac"));
        assert_eq!(ext_of(".hidden"), "");
    }
}
