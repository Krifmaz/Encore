//! Library types, Select / Delete… and who added it (0.36, docs/contracts/library-types.md §2.4 and §6): the pure
//! parts the library page, the album page and the song ⋯ menu use – the texts of who added a song and the album's
//! history line, the Type dropdown's options, the albums list's address, the selection and its chunks, and the
//! texts of the Delete… dialog and its toasts. The desktop and the phone have the same functions with the same
//! words (their `library_types` / `AlbumTypesRules`).
use crate::json::J;
use serde_json::Value;

/// Most albums one Select all picks (the server's `ids_only` cap).
pub const SELECT_MAX: usize = 5000;
/// Albums one Move to Trash call moves.
pub const CHUNK: usize = 100;

pub const ALL_TYPES: &str = "All types";
pub const NO_ALBUMS_OF_TYPE: &str = "No albums of this type";
pub const AT_MOST: &str = "At most 5000 albums at a time";
pub const TRASH_BODY: &str = "Their songs go to the Trash in Files, where you can restore them. The space is freed when the Trash is emptied.";
pub const OTHER_DISK: &str = " The Trash is on another disk, so the files are copied there first – this can take a while.";
pub const ONE_OFF_HELP: &str = "A song or two from a compilation, remix album or soundtrack: shown on the artist's page under One-offs, not with the albums.";
pub const ONE_OFFS_LINE: &str = "Songs from compilations, remix albums and soundtracks where you have only a few songs.";
pub const CHANGE_TYPE: &str = "Change type…";
pub const MOVE_ALBUM: &str = "Move album to Trash";

/// The admin part of the album page's ⋯ menu, in its order: Change type… sits after Edit details and before
/// Change artwork…, Move album to Trash ends it; none of it for others.
pub fn album_admin_items(admin: bool, has_type: bool) -> Vec<&'static str> {
    if !admin {
        return vec![];
    }
    let mut v = vec!["Tag / match metadata", "Edit details (album, artist, songs)"];
    if has_type {
        v.push(CHANGE_TYPE);
    }
    v.extend(["Change artwork…", "Find artwork automatically", MOVE_ALBUM]);
    v
}

/// The open Trash of Files (System → Files).
pub const TRASH_HREF: &str = "#/settings?s=files&root=trash";

/// Is this how a request comes in (a request, a link from the Downloader, a download from the Soulseek page)?
pub fn request_like(via: &str) -> bool {
    matches!(via, "request" | "link" | "soulseek")
}

/// Who added a song, in words (`day`: the client's day of `requested_at`, when there is one).
pub fn added_text(via: &str, by: Option<&str>, by_deleted: bool, day: Option<&str>) -> String {
    let by = by.filter(|b| !b.is_empty());
    match via {
        v if request_like(v) => match (by, by_deleted, day) {
            (Some(b), _, Some(d)) => format!("Requested by {b} on {d}"),
            (Some(b), _, None) => format!("Requested by {b}"),
            (None, true, Some(d)) => format!("Requested by a deleted user on {d}"),
            (None, true, None) => "Requested by a deleted user".into(),
            (None, false, _) => "Requested by someone – record cleared".into(),
        },
        "upload" => match (by, by_deleted) {
            (Some(b), _) => format!("Uploaded by {b}"),
            (None, true) => "Uploaded by a deleted user".into(),
            _ => String::new(),
        },
        "import" => match (by, by_deleted) {
            (Some(b), _) => format!("Imported by {b}"),
            (None, true) => "Imported by a deleted user".into(),
            _ => String::new(),
        },
        "scan" => "Found in the music folder".into(),
        _ => String::new(),
    }
}

/// [`added_text`] of an answer of GET /api/library/tracks/{id}/added, or of a `history` group (`day` turns a
/// time into the client's day).
pub fn added_of(v: &Value, day: impl Fn(f64) -> String) -> String {
    let at = v.g("requested_at").as_f64().filter(|t| *t > 0.0).map(&day);
    added_text(&v.s("via"), v.g("by").as_str(), v.g("by_deleted").as_bool().unwrap_or(false), at.as_deref())
}

/// The album page's history line: at most three groups' texts, " · and N more", then the day it was added
/// (" · added DAY", or "Added DAY" when no group says anything).
pub fn history_line(groups: &[String], added_day: Option<&str>) -> String {
    let texts: Vec<&String> = groups.iter().filter(|t| !t.is_empty()).collect();
    let mut out = texts.iter().take(3).map(|s| s.as_str()).collect::<Vec<_>>().join(" · ");
    if texts.len() > 3 {
        out.push_str(&format!(" · and {} more", texts.len() - 3));
    }
    match added_day {
        Some(d) if out.is_empty() => format!("Added {d}"),
        Some(d) => format!("{out} · added {d}"),
        None => out,
    }
}

/// The text of one entry of the Type dropdown: "Live (12)".
pub fn type_option_text(label: &str, count: i64) -> String {
    format!("{label} ({count})")
}

/// The Type dropdown's entries (value, text): All types first, then the server's types with their counts; a type
/// with no albums is left out unless it is the one picked.
pub fn type_options(types: &[Value], selected: &str) -> Vec<(String, String)> {
    let mut out = vec![(String::new(), ALL_TYPES.to_string())];
    for t in types {
        let (key, count) = (t.s("key"), t.g("count").as_i64().unwrap_or(0));
        if count > 0 || key == selected {
            out.push((key, type_option_text(&t.s("label"), count)));
        }
    }
    out
}

/// What the albums list asks for (GET /api/library/albums): a Type, when one is picked, decides alone (no `kind`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AlbumsQuery {
    pub q: String,
    pub sort: String,
    pub dir: String,
    pub kind: String,
    pub filter: String,
    pub artist: String,
    pub album_type: String,
}

impl AlbumsQuery {
    fn base(&self) -> String {
        let enc = crate::pages::library::uri_component;
        let mut url = format!("/library/albums?q={}&sort={}&dir={}", enc(&self.q), self.sort, self.dir);
        if self.album_type.is_empty() {
            url.push_str(&format!("&kind={}", self.kind));
        } else {
            url.push_str(&format!("&type={}", enc(&self.album_type)));
        }
        url.push_str(&format!("&filter={}&artist={}", self.filter, enc(&self.artist)));
        url
    }
    /// A page of the list.
    pub fn page(&self, offset: usize, limit: usize) -> String {
        format!("{}&limit={limit}&offset={offset}", self.base())
    }
    /// Every id of the list (Select all).
    pub fn ids(&self) -> String {
        format!("{}&ids_only=1", self.base())
    }
}

/// The picked albums, in the order picked, each once.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Selection {
    ids: Vec<String>,
}

impl Selection {
    pub fn toggle(&mut self, id: &str) {
        match self.ids.iter().position(|i| i == id) {
            Some(p) => {
                self.ids.remove(p);
            }
            None => self.ids.push(id.to_string()),
        }
    }
    /// Select all: these ids (at most [`SELECT_MAX`]); true when the list had more.
    pub fn select_all(&mut self, ids: &[String]) -> bool {
        self.ids.clear();
        for id in ids.iter().take(SELECT_MAX) {
            if !self.ids.contains(id) {
                self.ids.push(id.clone());
            }
        }
        ids.len() > SELECT_MAX
    }
    pub fn clear(&mut self) {
        self.ids.clear();
    }
    pub fn contains(&self, id: &str) -> bool {
        self.ids.iter().any(|i| i == id)
    }
    pub fn len(&self) -> usize {
        self.ids.len()
    }
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }
    pub fn ids(&self) -> &[String] {
        &self.ids
    }
}

/// The real runs of a Move to Trash: the selection in order, [`CHUNK`] albums a call.
pub fn chunks(ids: &[String]) -> Vec<Vec<String>> {
    ids.chunks(CHUNK).map(|c| c.to_vec()).collect()
}

pub fn selected_text(n: usize) -> String {
    format!("{n} selected")
}

pub fn select_all_text(total: usize) -> String {
    format!("Select all {total}")
}

pub fn moving_text(done: usize, n: usize) -> String {
    format!("Moving to the Trash… {done} of {n}")
}

/// The toast after a Move to Trash: how many went of how many, and the first reason when some couldn't.
pub fn moved_text(moved: usize, total: usize, first_reason: &str) -> String {
    if moved == total {
        return if moved == 1 { "Moved 1 album to the Trash".into() } else { format!("Moved {moved} albums to the Trash") };
    }
    format!("Moved {moved} of {total} albums to the Trash – {} couldn't be moved: {first_reason}", total - moved)
}

/// The Delete… dialog's text: the Trash's words, the other-disk sentence, and what stays on its own line.
pub fn confirm_body(other_disk: bool, skipped_text: &str) -> String {
    let mut body = TRASH_BODY.to_string();
    if other_disk {
        body.push_str(OTHER_DISK);
    }
    if !skipped_text.is_empty() {
        body.push('\n');
        body.push_str(skipped_text);
    }
    body
}

/// The Change type dialog's question.
pub fn what_is(album: &str) -> String {
    format!("What is “{album}”?")
}

/// The Change type dialog's automatic choice: "Automatic (Live · EP – From MusicBrainz)".
pub fn automatic_label(label: &str, why: &str) -> String {
    format!("Automatic ({label} – {why})")
}

/// The Change type dialog's choices (value, label): the four formats, then Automatic ("" ).
pub fn type_choices(t: &Value) -> Vec<(&'static str, String)> {
    let auto = t.g("auto");
    let why = if auto.truthy("why_text") { auto.s("why_text") } else { t.s("why_text") };
    vec![
        ("album", "Album".into()), ("ep", "EP".into()), ("single", "Single".into()), ("one_off", "One-off".into()),
        ("", automatic_label(&auto.s("label"), &why)),
    ]
}

/// "Show all N" of the One-offs shelf ("" when all are shown).
pub fn show_all_text(total: usize, shown: usize) -> String {
    if total > shown { format!("Show all {total}") } else { String::new() }
}

/// A One-off's sub-line on the artist page: "Now 99 · Various artists".
pub fn one_off_sub(t: &Value) -> String {
    [t.s("album"), t.s("release_label")].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// §2.4 of the contract, verbatim (DAY passed in as a string).
    #[test]
    fn who_added_it_in_words() {
        let day = Some("DAY");
        for (via, by, deleted, at, want) in [
            ("request", Some("alex"), false, day, "Requested by alex on DAY"),
            ("link", Some("alex"), false, day, "Requested by alex on DAY"),
            ("soulseek", Some("alex"), false, day, "Requested by alex on DAY"),
            ("request", Some("alex"), false, None, "Requested by alex"),
            ("soulseek", Some("alex"), false, None, "Requested by alex"),
            ("request", None, false, day, "Requested by someone – record cleared"),
            ("link", None, false, None, "Requested by someone – record cleared"),
            ("request", None, true, day, "Requested by a deleted user on DAY"),
            ("upload", Some("sam"), false, None, "Uploaded by sam"),
            ("upload", None, true, None, "Uploaded by a deleted user"),
            ("import", Some("sam"), false, None, "Imported by sam"),
            ("import", None, true, None, "Imported by a deleted user"),
            ("scan", None, false, None, "Found in the music folder"),
            ("", None, false, None, ""),
            ("something", Some("x"), false, None, ""),
        ] {
            assert_eq!(added_text(via, by, deleted, at), want, "{via} {by:?} {deleted} {at:?}");
        }
        // the en dash with spaces, as the other Encore texts
        assert!(added_text("request", None, false, None).contains(" \u{2013} "));
        // from the server's answer
        let v = json!({ "via": "request", "by": "alex", "by_deleted": false, "requested_at": 1729000000.0 });
        assert_eq!(added_of(&v, |_| "Oct 13, 2026".into()), "Requested by alex on Oct 13, 2026");
        assert_eq!(added_of(&json!({ "via": null }), |_| String::new()), "");
    }

    #[test]
    fn the_history_line() {
        assert_eq!(history_line(&[], Some("DAY")), "Added DAY");
        assert_eq!(history_line(&["".into()], Some("DAY")), "Added DAY", "a group with no words is none");
        assert_eq!(history_line(&["Requested by alex on Oct 13, 2026".into()], Some("Oct 14, 2026")), "Requested by alex on Oct 13, 2026 · added Oct 14, 2026");
        let four: Vec<String> = ["A", "B", "C", "D"].iter().map(|s| s.to_string()).collect();
        assert_eq!(history_line(&four, Some("DAY")), "A · B · C · and 1 more · added DAY");
        assert_eq!(history_line(&four[..2], None), "A · B");
    }

    #[test]
    fn type_options_and_their_counts() {
        assert_eq!(type_option_text("Live", 12), "Live (12)");
        let types = vec![json!({ "key": "album", "label": "Studio albums", "count": 5 }), json!({ "key": "live", "label": "Live", "count": 0 }),
                         json!({ "key": "one_off", "label": "One-offs", "count": 2 })];
        let opts = type_options(&types, "");
        assert_eq!(opts, vec![("".to_string(), "All types".to_string()), ("album".into(), "Studio albums (5)".into()), ("one_off".into(), "One-offs (2)".into())]);
        let opts = type_options(&types, "live");
        assert!(opts.contains(&("live".to_string(), "Live (0)".to_string())), "the picked one stays");
    }

    #[test]
    fn the_albums_address() {
        let mut q = AlbumsQuery { sort: "added".into(), dir: "desc".into(), kind: "albums".into(), ..Default::default() };
        assert_eq!(q.page(0, 120), "/library/albums?q=&sort=added&dir=desc&kind=albums&filter=&artist=&limit=120&offset=0");
        q.album_type = "live".into();
        assert_eq!(q.page(120, 120), "/library/albums?q=&sort=added&dir=desc&type=live&filter=&artist=&limit=120&offset=120", "a type drops the kind");
        q.artist = "Beta Band".into();
        assert_eq!(q.ids(), "/library/albums?q=&sort=added&dir=desc&type=live&filter=&artist=Beta%20Band&ids_only=1");
    }

    #[test]
    fn the_selection() {
        let mut s = Selection::default();
        s.toggle("a");
        s.toggle("b");
        s.toggle("a");
        assert_eq!(s.ids(), ["b".to_string()]);
        assert_eq!(selected_text(s.len()), "1 selected");
        let all: Vec<String> = (0..SELECT_MAX + 7).map(|i| format!("id{i}")).collect();
        assert!(s.select_all(&all), "more than 5000: capped");
        assert_eq!(s.len(), SELECT_MAX);
        assert!(!s.select_all(&all[..10]));
        assert_eq!(s.len(), 10);
        s.clear();
        assert!(s.is_empty());
        assert_eq!(select_all_text(340), "Select all 340");
        // 340 ids → 4 calls of at most 100, in order
        let ids: Vec<String> = (0..340).map(|i| i.to_string()).collect();
        let c = chunks(&ids);
        assert_eq!(c.iter().map(|x| x.len()).collect::<Vec<_>>(), vec![100, 100, 100, 40]);
        assert_eq!(c[3][0], "300");
    }

    #[test]
    fn the_trash_texts() {
        assert_eq!(moving_text(3, 12), "Moving to the Trash… 3 of 12");
        assert_eq!(moved_text(1, 1, ""), "Moved 1 album to the Trash");
        assert_eq!(moved_text(12, 12, ""), "Moved 12 albums to the Trash");
        assert_eq!(moved_text(10, 12, "Its files are outside the folders Encore manages"),
                   "Moved 10 of 12 albums to the Trash – 2 couldn't be moved: Its files are outside the folders Encore manages");
        assert_eq!(confirm_body(false, ""), TRASH_BODY);
        assert_eq!(confirm_body(true, "1 of them can't be moved to the Trash and stays: X"),
                   format!("{TRASH_BODY}{OTHER_DISK}\n1 of them can't be moved to the Trash and stays: X"));
        assert!(OTHER_DISK.starts_with(' '));
    }

    #[test]
    fn the_album_menu_for_admins_only() {
        assert!(album_admin_items(false, true).is_empty());
        let items = album_admin_items(true, true);
        let at = |l: &str| items.iter().position(|x| *x == l).unwrap();
        assert!(at("Edit details (album, artist, songs)") < at(CHANGE_TYPE) && at(CHANGE_TYPE) < at("Change artwork…"));
        assert_eq!(items.last(), Some(&MOVE_ALBUM));
        assert!(!album_admin_items(true, false).contains(&CHANGE_TYPE), "a Surround album has no type");
    }

    #[test]
    fn change_type_words() {
        assert_eq!(what_is("Now 99"), "What is “Now 99”?");
        let t = json!({ "label": "Live · EP", "why_text": "You chose this", "auto": { "label": "Live · EP", "why_text": "From MusicBrainz" } });
        let c = type_choices(&t);
        assert_eq!(c.iter().map(|x| x.1.as_str()).collect::<Vec<_>>(), ["Album", "EP", "Single", "One-off", "Automatic (Live · EP – From MusicBrainz)"]);
        assert_eq!(c[4].0, "");
        assert_eq!(show_all_text(12, 10), "Show all 12");
        assert_eq!(show_all_text(10, 10), "");
        assert_eq!(one_off_sub(&json!({ "album": "Now 99", "release_label": "Various artists" })), "Now 99 · Various artists");
    }
}
