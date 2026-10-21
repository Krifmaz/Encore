//! Library types, Select / Delete… and who added it (0.36, docs/contracts/library-types.md §2.4 and §6): the pure
//! parts the library page, the album page, the artist pages and the song ⋯ menu use – the texts of who added a
//! song and the album's history line, the Type dropdown's options, the albums list's address, the selection and
//! its chunks, the texts of the Delete… dialog and its toasts, the Change type choices and the One-offs shelf.
//! Behaviour and words: web-rs/src/pages/library_types.rs (the phone has the same in `AlbumTypesRules`).

use crate::fmt;
use crate::pages::{urlencode, Ctx, El};
use crate::theme::Btn;
use crate::widgets::{self, btn_sm, h2, small_muted, TrackRow};
use iced::widget::{column, row, space};
use iced::{Alignment, Length};
use serde_json::Value;

/// Most albums one Select all picks (the server's `ids_only` cap).
pub const SELECT_MAX: usize = 5000;
/// Albums one Move to Trash call moves.
pub const CHUNK: usize = 100;
/// Songs of the One-offs shelf before "Show all N", and how many "Show all N" loads.
pub const ONE_OFFS_FIRST: usize = 10;
pub const ONE_OFFS_ALL: usize = 500;

pub const TYPE: &str = "Type";
pub const ALL_TYPES: &str = "All types";
pub const NO_ALBUMS_OF_TYPE: &str = "No albums of this type";
pub const AT_MOST: &str = "At most 5000 albums at a time";
pub const TRASH_BODY: &str = "Their songs go to the Trash in Files, where you can restore them. The space is freed when the Trash is emptied.";
pub const OTHER_DISK: &str = " The Trash is on another disk, so the files are copied there first – this can take a while.";
pub const ONE_OFF_HELP: &str = "A song or two from a compilation, remix album or soundtrack: shown on the artist's page under One-offs, not with the albums.";
pub const ONE_OFFS: &str = "One-offs";
pub const ONE_OFFS_LINE: &str = "Songs from compilations, remix albums and soundtracks where you have only a few songs.";
pub const CHANGE_TYPE: &str = "Change type…";
pub const CHANGE_TYPE_TITLE: &str = "Change type";
pub const TYPE_SAVED: &str = "Type saved";
pub const MOVE_ALBUM: &str = "Move album to Trash";
pub const MOVE_TO_TRASH: &str = "Move to Trash";
pub const OPEN_TRASH: &str = "Open Trash";
pub const SELECT: &str = "Select";
pub const CLEAR_SELECTION: &str = "Clear selection";
pub const DELETE: &str = "Delete…";
pub const DONE: &str = "Done";
/// The Singles tab (it lists singles and EPs).
pub const SINGLES_TAB: &str = "Singles & EPs";
/// How long the toast after a Move to Trash stays with its Open Trash button.
pub const TOAST_MS: u64 = 8000;

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

/// Is this how a request comes in (a request, a link from the Downloader, a download from the Soulseek page)?
pub fn request_like(via: &str) -> bool {
    matches!(via, "request" | "link" | "soulseek")
}

/// Who added a song, in words (`day`: the app's day of `requested_at`, when there is one).
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
/// time into the app's day).
pub fn added_of(v: &Value, day: impl Fn(f64) -> String) -> String {
    let at = v["requested_at"].as_f64().filter(|t| *t > 0.0).map(&day);
    added_text(&fmt::s(v, "via"), v["by"].as_str(), v["by_deleted"].as_bool().unwrap_or(false), at.as_deref())
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

/// The album page's history line from the album answer: the server's `history` groups and `added_at` (0.36),
/// or – from an older server – its requesters joined here ("" when there is nothing to say).
pub fn album_history(a: &Value, day: impl Fn(f64) -> String + Copy) -> String {
    let tracks = fmt::items(&a["tracks"]);
    let earliest = |key: &str| tracks.iter().filter(|t| fmt::truthy(t, key)).map(|t| fmt::f(t, key)).fold(f64::INFINITY, f64::min);
    let (requested_at, added_at) = (earliest("requested_at"), earliest("added_at"));
    if a["history"].is_array() {
        let groups: Vec<String> = fmt::items(&a["history"]).iter().map(|g| added_of(g, day)).collect();
        let added = a["added_at"].as_f64().filter(|t| *t > 0.0).or(added_at.is_finite().then_some(added_at)).map(day);
        return history_line(&groups, added.as_deref());
    }
    let mut requesters: Vec<String> = Vec::new();
    for t in &tracks {
        let w = fmt::s(t, "requested_by");
        if !w.is_empty() && !requesters.contains(&w) {
            requesters.push(w);
        }
    }
    let mut parts: Vec<String> = Vec::new();
    if !requesters.is_empty() {
        let mut s = format!("Requested by {}", requesters.join(", "));
        if requested_at.is_finite() {
            s.push_str(&format!(" on {}", day(requested_at)));
        }
        parts.push(s);
    }
    if added_at.is_finite() {
        parts.push(format!("{} {}", if requesters.is_empty() { "Added" } else { "added" }, day(added_at)));
    }
    parts.join(" · ")
}

/// Who asked for a song, for its source line: the adder of a request-like song (kept after Clear finished), or
/// – from a server before 0.36 – the request's user. ("", 0) when nobody asked.
pub fn requester(t: &Value) -> (String, f64) {
    if fmt::truthy(t, "added_via") {
        return if request_like(&fmt::s(t, "added_via")) {
            (fmt::s(t, "added_by_name"), fmt::f(t, "requested_at"))
        } else {
            (String::new(), 0.0)
        };
    }
    (fmt::s(t, "requested_by"), fmt::f(t, "requested_at"))
}

/// The text of one entry of the Type dropdown: "Live (12)".
pub fn type_option_text(label: &str, count: i64) -> String {
    format!("{label} ({count})")
}

/// One entry of the Type dropdown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeOpt {
    /// The filter key ("" = All types).
    pub key: String,
    pub text: String,
}

impl std::fmt::Display for TypeOpt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text)
    }
}

/// The Type dropdown's entries: All types first, then the server's types with their counts; a type with no
/// albums is left out unless it is the one picked.
pub fn type_options(types: &[Value], selected: &str) -> Vec<TypeOpt> {
    let mut out = vec![TypeOpt { key: String::new(), text: ALL_TYPES.to_string() }];
    for t in types {
        let (key, count) = (fmt::s(t, "key"), t["count"].as_i64().unwrap_or(0));
        if count > 0 || key == selected {
            out.push(TypeOpt { text: type_option_text(&fmt::s(t, "label"), count), key });
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
        let mut url = format!("/library/albums?q={}&sort={}&dir={}", urlencode(&self.q), self.sort, self.dir);
        if self.album_type.is_empty() {
            url.push_str(&format!("&kind={}", self.kind));
        } else {
            url.push_str(&format!("&type={}", urlencode(&self.album_type)));
        }
        url.push_str(&format!("&filter={}&artist={}", self.filter, urlencode(&self.artist)));
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

/// The Delete… dialog from a dry run (POST /api/library/albums/trash `dry_run`): its title (the server's "Delete
/// 12 albums (3.4 GB)?"), its text, and whether it can go (false: the Trash's disk has no room, only Close).
pub fn trash_dialog_words(d: &Value) -> (String, String, bool) {
    if d["room"] == Value::Bool(false) {
        return (fmt::s(d, "text"), fmt::s(d, "room_text"), false);
    }
    (fmt::s(d, "text"), confirm_body(fmt::truthy(d, "other_disk"), &fmt::s(d, "skipped_text")), true)
}

/// The Change type dialog's question.
pub fn what_is(album: &str) -> String {
    format!("What is “{album}”?")
}

/// The Change type dialog's automatic choice: "Automatic (Live · EP – From MusicBrainz)".
pub fn automatic_label(label: &str, why: &str) -> String {
    format!("Automatic ({label} – {why})")
}

/// The Change type dialog's choices (value, label): the four formats, then Automatic ("").
pub fn type_choices(t: &Value) -> Vec<(&'static str, String)> {
    let auto = &t["auto"];
    let why = if fmt::truthy(auto, "why_text") { fmt::s(auto, "why_text") } else { fmt::s(t, "why_text") };
    vec![
        ("album", "Album".into()),
        ("ep", "EP".into()),
        ("single", "Single".into()),
        ("one_off", "One-off".into()),
        ("", automatic_label(&fmt::s(auto, "label"), &why)),
    ]
}

/// "Show all N" of the One-offs shelf ("" when all are shown).
pub fn show_all_text(total: usize, shown: usize) -> String {
    if total > shown {
        format!("Show all {total}")
    } else {
        String::new()
    }
}

/// A One-off's sub-line on the artist page: "Now 99 · Various artists".
pub fn one_off_sub(t: &Value) -> String {
    [fmt::s(t, "album"), fmt::s(t, "release_label")]
        .into_iter()
        .filter(|x| !x.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
}

/// The artist pages' One-offs shelf (§6.3): the heading with the count, the line, the songs shown (a tap plays
/// the shelf from that song, ⋯ is the song menu) and "Show all N"; None when there are none.
pub fn one_offs_section<'a, M: Clone + 'a>(
    ctx: &Ctx<'a>,
    shelf: Option<&'a (usize, Vec<Value>)>,
    loading_all: bool,
    on_play: impl Fn(usize) -> M,
    on_menu: impl Fn(usize) -> M,
    on_all: M,
) -> Option<El<'a, M>> {
    let (total, list) = shelf.filter(|(t, l)| *t > 0 && !l.is_empty())?;
    let mut head = row![
        row![h2(ONE_OFFS), small_muted(total.to_string()).color(ctx.look.faint)]
            .spacing(8)
            .align_y(Alignment::Center),
        space().width(Length::Fill)
    ]
    .align_y(Alignment::Center);
    if loading_all {
        head = head.push(btn_sm("Loading…", Btn::Ghost, None));
    } else {
        let all = show_all_text(*total, list.len());
        if !all.is_empty() {
            head = head.push(btn_sm(all, Btn::Ghost, Some(on_all)));
        }
    }
    let cur = ctx.player.current_id();
    let mut rows = column![];
    for (i, v) in list.iter().enumerate() {
        let id = fmt::i(v, "id");
        rows = rows.push(
            TrackRow::new(crate::pages::library::tr(ctx, v, "title"))
                .sub(one_off_sub(v))
                .cover(ctx.cover(&fmt::s(v, "album_id"), &v["cover_v"], 80))
                .quality(fmt::quality(v))
                .duration(Some(fmt::f(v, "duration")).filter(|d| *d > 0.0))
                .playing(cur == Some(id))
                .on_play(on_play(i))
                .on_menu(on_menu(i))
                .view(),
        );
    }
    Some(column![head, small_muted(ONE_OFFS_LINE), widgets::card_flat(rows)].spacing(10).into())
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
        assert_eq!(added_of(&v, |_| "13 Oct 2026".into()), "Requested by alex on 13 Oct 2026");
        assert_eq!(added_of(&json!({ "via": null }), |_| String::new()), "");
        assert_eq!(added_of(&json!({ "via": "request", "by": null, "by_deleted": true, "requested_at": 5.0 }), |_| "D".into()), "Requested by a deleted user on D");
    }

    #[test]
    fn the_history_line() {
        assert_eq!(history_line(&[], Some("DAY")), "Added DAY");
        assert_eq!(history_line(&["".into()], Some("DAY")), "Added DAY", "a group with no words is none");
        assert_eq!(
            history_line(&["Requested by alex on Oct 13, 2026".into()], Some("Oct 14, 2026")),
            "Requested by alex on Oct 13, 2026 · added Oct 14, 2026"
        );
        let four: Vec<String> = ["A", "B", "C", "D"].iter().map(|s| s.to_string()).collect();
        assert_eq!(history_line(&four, Some("DAY")), "A · B · C · and 1 more · added DAY");
        assert_eq!(history_line(&four[..2], None), "A · B");
        // from the album answer: the server's groups win; an older server's requesters are joined here
        let a = json!({ "history": [{ "via": "request", "by": "alex", "requested_at": 1.0, "songs": 12 }, { "via": "upload", "by": "sam", "songs": 1 }],
                        "added_at": 2.0, "tracks": [] });
        assert_eq!(album_history(&a, |t| format!("D{t}")), "Requested by alex on D1 · Uploaded by sam · added D2");
        let old = json!({ "tracks": [{ "requested_by": "ann", "requested_at": 3.0, "added_at": 4.0 }, { "requested_by": "bo", "added_at": 5.0 }] });
        assert_eq!(album_history(&old, |t| format!("D{t}")), "Requested by ann, bo on D3 · added D4");
        assert_eq!(album_history(&json!({ "tracks": [] }), |t| format!("D{t}")), "");
    }

    #[test]
    fn the_source_line_names_the_adder() {
        assert_eq!(requester(&json!({ "added_via": "request", "added_by_name": "alex", "requested_at": 9.0, "requested_by": "x" })), ("alex".into(), 9.0));
        assert_eq!(requester(&json!({ "added_via": "upload", "added_by_name": "sam", "requested_by": "sam" })), (String::new(), 0.0));
        assert_eq!(requester(&json!({ "requested_by": "ann", "requested_at": 2.0 })), ("ann".into(), 2.0), "an older server");
    }

    #[test]
    fn type_options_and_their_counts() {
        assert_eq!(type_option_text("Live", 12), "Live (12)");
        let types = vec![
            json!({ "key": "album", "label": "Studio albums", "count": 5 }),
            json!({ "key": "live", "label": "Live", "count": 0 }),
            json!({ "key": "one_off", "label": "One-offs", "count": 2 }),
        ];
        let opts = type_options(&types, "");
        let texts: Vec<&str> = opts.iter().map(|o| o.text.as_str()).collect();
        assert_eq!(texts, ["All types", "Studio albums (5)", "One-offs (2)"]);
        assert_eq!(opts[0].key, "");
        let opts = type_options(&types, "live");
        assert!(opts.contains(&TypeOpt { key: "live".into(), text: "Live (0)".into() }), "the picked one stays");
        assert_eq!(opts[1].to_string(), "Studio albums (5)");
    }

    #[test]
    fn the_albums_address() {
        let mut q = AlbumsQuery { sort: "added".into(), dir: "desc".into(), kind: "albums".into(), ..Default::default() };
        assert_eq!(q.page(0, 120), "/library/albums?q=&sort=added&dir=desc&kind=albums&filter=&artist=&limit=120&offset=0");
        q.album_type = "live".into();
        assert_eq!(
            q.page(120, 120),
            "/library/albums?q=&sort=added&dir=desc&type=live&filter=&artist=&limit=120&offset=120",
            "a type drops the kind"
        );
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
        assert_eq!(
            moved_text(10, 12, "Its files are outside the folders Encore manages"),
            "Moved 10 of 12 albums to the Trash – 2 couldn't be moved: Its files are outside the folders Encore manages"
        );
        assert_eq!(confirm_body(false, ""), TRASH_BODY);
        assert_eq!(
            confirm_body(true, "1 of them can't be moved to the Trash and stays: X"),
            format!("{TRASH_BODY}{OTHER_DISK}\n1 of them can't be moved to the Trash and stays: X")
        );
        assert!(OTHER_DISK.starts_with(' '));
        // the dialog from a dry run: the server's title; no room → its words and no Move button
        let d = json!({ "text": "Delete 2 albums (1.5 MB)?", "other_disk": false, "skipped_text": "", "room": true });
        assert_eq!(trash_dialog_words(&d), ("Delete 2 albums (1.5 MB)?".to_string(), TRASH_BODY.to_string(), true));
        let full = json!({ "text": "Delete 2 albums (1.5 MB)?", "room": false, "room_text": "There isn't room in the Trash" });
        assert_eq!(trash_dialog_words(&full).1, "There isn't room in the Trash");
        assert!(!trash_dialog_words(&full).2);
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
        assert_eq!(
            c.iter().map(|x| x.1.as_str()).collect::<Vec<_>>(),
            ["Album", "EP", "Single", "One-off", "Automatic (Live · EP – From MusicBrainz)"]
        );
        assert_eq!(c[4].0, "");
        assert_eq!(show_all_text(12, 10), "Show all 12");
        assert_eq!(show_all_text(10, 10), "");
        assert_eq!(one_off_sub(&json!({ "album": "Now 99", "release_label": "Various artists" })), "Now 99 · Various artists");
        assert_eq!(one_off_sub(&json!({ "album": "Now 99" })), "Now 99");
        assert_eq!(SINGLES_TAB, "Singles & EPs");
    }
}
