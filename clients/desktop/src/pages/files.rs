//! Files: the admin file manager of 0.33, in Settings → System → Files. Behaviour reference:
//! web-rs/src/pages/files.rs and the phone's features/files/FilesScreen.kt; the routes and their JSON
//! are docs/contracts/client-shapes-0.31-0.33.md §2.4 and Part 4, the words its §5.7.
//!
//! The roots (the library, staging, the download folders, Trash) as a list and, when the window is
//! wide enough, as a tree on the left that opens folder by folder. A folder: breadcrumbs, a path box,
//! sorting (the Sort button or the column heads), multi-select (click, Ctrl/⌘-click, Shift-click,
//! Ctrl/⌘+A, the arrow keys), Enter opens, Backspace goes up one folder, Delete (or ⌘/Ctrl+Backspace)
//! moves to trash after a confirm. New folder, Rename, Move (a folder picker over the roots), Move to
//! trash, the Trash with Restore / Delete forever / Empty trash. Upload of files and of whole folders
//! through the system's file dialog, or dropped on the window where the OS reports drops (Windows,
//! macOS and X11; winit has no drag and drop on Wayland), one file per request, streamed from the
//! disk, with a progress panel and Stop. Edit tags for audio files, Import… with the match review and
//! "Keep the files where they are". Rows get one ⋯ button; a right-click opens the same menu.
//!
//! A search box over the list: as you type it filters the open folder, and once typing pauses it
//! searches the whole root by name at any depth (`GET /api/files/search`, 0.37.1); each result shows
//! its folder's path, and a click opens that folder with the file picked (a folder opens itself).
//! Ctrl/⌘+F or `/` on the list goes to the box.
//!
//! Only admins see the section; the server answers 403 to anyone else and 404 before 0.33.

use crate::api::{Api, ApiError};
use crate::fmt;
use crate::pages::library::{self, Act as LibAct};
use crate::pages::{urlencode, Ctx, El};
use crate::theme::{self, Badge, Btn};
use crate::widgets::{
    self, badge, btn, btn_icon, btn_sm, card_flat, error_box, field, icon, icon_btn, list_row, pop_menu,
    skeleton_rows, small_muted, t, Icon, MenuItem,
};
use iced::keyboard::{self, key::Named, Key, Modifiers};
use iced::widget::text::Wrapping;
use iced::widget::{button, checkbox, column, container, mouse_area, operation, progress_bar, radio, row, space, svg, text_input};
use iced::{alignment, mouse, Alignment, Background, Border, Color, Length, Point, Subscription, Task};
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

/// The tag writer's keys (the Fix song dialog's; the phone's `FilesApi.TAGS`) in the editor's order,
/// with their labels.
pub const TAGS: [(&str, &str); 8] = [
    ("title", "Title"),
    ("artist", "Artist"),
    ("album", "Album"),
    ("albumartist", "Album artist"),
    ("tracknumber", "Track"),
    ("discnumber", "Disc"),
    ("date", "Year"),
    ("genre", "Genre"),
];

/// The sorts of a folder: `(key, words)`.
pub const SORTS: [(&str, &str); 3] = [("name", "Name"), ("size", "Size"), ("mtime", "Date modified")];

/// What an upload does with a name that is already there (§4.3): `(conflict, words)`.
pub const CONFLICTS: [(&str, &str); 3] = [
    ("rename", "Keep both (the new one gets “(2)”)"),
    ("replace", "Replace it (the file that is there goes to Trash)"),
    ("skip", "Skip the new one"),
];

/// Entries asked for at a time (the server's most).
const PAGE: usize = 500;
/// Files one folder upload takes at most, and how deep it looks.
const MAX_FILES: usize = 5000;
const MAX_DEPTH: usize = 24;
/// How deep the tree opens.
const TREE_DEPTH: usize = 16;
const TREE_W: f32 = 230.0;
/// The ids of the name box of a dialog and of the path box.
const NAME_ID: &str = "files-name";
const PATH_ID: &str = "files-path";
/// The id of the search box.
const FIND_ID: &str = "files-find";
/// How long typing must pause before the whole root is searched (the open folder is filtered at once).
const FIND_PAUSE: Duration = Duration::from_millis(250);
/// The most search results asked for (the server gives the nearest first).
const FIND_LIMIT: usize = 200;
/// Where the chosen sort is kept on this computer (the web's localStorage key).
const SORT_KEY: &str = "encore.filesSort";

const INTRO: &str = "The folders Encore may change. Deleted files go to Trash first.";
const NO_FILE_MANAGER: &str = "This server has no file manager yet (Encore 0.33 or newer has one).";
const LINK_NOTE: &str = "This folder is reached through a link, so Encore doesn't change anything in it.";
const TRASH_NOTE: &str = "You can restore them from Trash. Songs in the library are taken out of it until they are restored.";
const KEEP_HELP: &str = "The songs join the library where and as they are: nothing is moved, renamed or retagged.";
const MOVE_HELP: &str = "Each album is tagged as the release it matches and moved into the library.";
const NO_MATCH: &str = "No match found: it is imported with its own tags.";
const SLASH: &str = "A name can't contain / or \\";
/// The error an upload ends with when Stop was pressed, or when the file shrank while it was read.
const STOPPED: &str = "Stopped";
const CHANGED: &str = "The file changed while it was uploading";

const AUDIO_EXT: [&str; 17] = [
    "flac", "mp3", "m4a", "aac", "ogg", "opus", "wav", "aiff", "aif", "alac", "ape", "wv", "dsf", "dff", "wma", "mpc", "tta",
];

// ----- what the server sends ------------------------------------------------------------------

/// A place the file manager may touch (`GET /api/files/roots`, §4.1).
#[derive(Clone, Debug, PartialEq)]
pub struct Root {
    pub id: String,
    pub name: String,
    pub path: String,
    pub writable: bool,
    /// Bytes free on its disk (0 = unknown).
    pub free: f64,
}

impl Root {
    pub fn from(v: &Value) -> Option<Root> {
        let id = fmt::s(v, "id");
        if id.is_empty() {
            return None;
        }
        let name = fmt::s(v, "name");
        Some(Root {
            name: if name.is_empty() { id.clone() } else { name },
            path: fmt::s(v, "path"),
            writable: v.get("writable").map(|w| w.as_bool().unwrap_or(false)).unwrap_or(true),
            free: fmt::f(v, "free_bytes"),
            id,
        })
    }

    pub fn is_trash(&self) -> bool {
        self.id == "trash"
    }
}

/// What the library knows of an audio file.
#[derive(Clone, Debug, PartialEq)]
pub struct Audio {
    pub format: String,
    pub duration: f64,
    pub title: String,
    pub artist: String,
    pub album: String,
    /// 0 when the library does not have the file.
    pub track_id: i64,
}

/// One entry of a folder (§2.4).
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub name: String,
    /// `dir`, `file` or `link`.
    pub kind: String,
    pub size: f64,
    pub mtime: f64,
    pub audio: Option<Audio>,
    pub image: bool,
    /// A link's target as text.
    pub target: String,
    /// A link's target: `dir`, `file` or `missing`.
    pub target_kind: String,
}

impl Entry {
    pub fn from(v: &Value) -> Option<Entry> {
        let name = fmt::s(v, "name");
        if name.is_empty() {
            return None;
        }
        let kind = fmt::s(v, "kind");
        let audio = v.get("audio").filter(|a| a.is_object()).map(|a| Audio {
            format: fmt::s(a, "format"),
            duration: fmt::f(a, "duration"),
            title: fmt::s(a, "title"),
            artist: fmt::s(a, "artist"),
            album: fmt::s(a, "album"),
            track_id: fmt::i(a, "track_id"),
        });
        Some(Entry {
            name,
            kind: if kind.is_empty() { "file".into() } else { kind },
            size: fmt::f(v, "size"),
            mtime: fmt::f(v, "mtime"),
            audio,
            image: fmt::truthy(v, "image"),
            target: fmt::s(v, "target"),
            target_kind: fmt::s(v, "target_kind"),
        })
    }

    /// A folder, or a link to one (a link opens; it is never written through).
    pub fn is_dir(&self) -> bool {
        self.kind == "dir" || (self.kind == "link" && self.target_kind == "dir")
    }

    pub fn is_link(&self) -> bool {
        self.kind == "link"
    }

    pub fn is_audio(&self) -> bool {
        self.audio.is_some() || AUDIO_EXT.contains(&ext(&self.name).as_str())
    }

    pub fn track_id(&self) -> i64 {
        self.audio.as_ref().map(|a| a.track_id).unwrap_or(0)
    }

    /// The line under the name: a link's target, an audio file's "artist – title · 3:07".
    pub fn line(&self) -> String {
        if self.is_link() {
            return format!("Link to {}", if self.target.is_empty() { "somewhere else" } else { &self.target });
        }
        match &self.audio {
            Some(a) if !a.title.is_empty() || !a.artist.is_empty() => {
                let who = [a.artist.as_str(), a.title.as_str()]
                    .into_iter()
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join(" – ");
                if a.duration > 0.0 {
                    format!("{who} · {}", fmt::mmss(a.duration))
                } else {
                    who
                }
            }
            _ => String::new(),
        }
    }
}

/// The lower-case extension of a name ("" without one).
fn ext(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((stem, e)) if !stem.is_empty() => e.to_ascii_lowercase(),
        _ => String::new(),
    }
}

/// A page of a folder (§4.2).
#[derive(Debug, Default, PartialEq)]
pub struct Listing {
    pub entries: Vec<Entry>,
    pub total: usize,
    /// The folder was reached through a link (§4.17): nothing in it is written.
    pub via_link: bool,
}

impl Listing {
    pub fn from(v: &Value, offset: usize) -> Listing {
        let list = match v.get("entries") {
            Some(e) => fmt::items(e),
            None => fmt::items(v),
        };
        let entries: Vec<Entry> = list.iter().filter_map(Entry::from).collect();
        let total = (fmt::i(v, "total").max(0) as usize).max(offset + entries.len());
        Listing {
            entries,
            total,
            via_link: fmt::truthy(v, "via_link"),
        }
    }
}

/// One result of a search across a root (`GET /api/files/search`).
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    /// The entry as a folder listing would show it (name, kind, size, link target kind, audio).
    pub entry: Entry,
    /// Its path from the root.
    pub path: String,
    /// The folder it is in, from the root ("" = the root itself).
    pub parent: String,
}

impl Hit {
    /// A result of the answer's `results`; None without a name or a path.
    pub fn from(v: &Value) -> Option<Hit> {
        let entry = Entry::from(v)?;
        let path = fmt::s(v, "path");
        if path.is_empty() {
            return None;
        }
        let parent = match v.get("parent") {
            Some(Value::String(p)) => p.clone(),
            _ => parent_of(&path),
        };
        Some(Hit { entry, path, parent })
    }

    /// Where a click goes: (the folder to open, it is a link, the name to pick there). A folder (or a
    /// link to one) opens itself; a file opens its folder with it picked.
    pub fn target(&self) -> (String, bool, Option<String>) {
        if self.entry.is_dir() {
            (self.path.clone(), self.entry.is_link(), None)
        } else {
            (self.parent.clone(), false, Some(self.entry.name.clone()))
        }
    }
}

/// The answer of a search: the results, the nearest first, and whether the server stopped early.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Hits {
    /// The results.
    pub hits: Vec<Hit>,
    /// There may be more than were given (the server's limit, or its walk's budget).
    pub truncated: bool,
}

impl Hits {
    /// `{results, truncated}`.
    pub fn from(v: &Value) -> Hits {
        let list = v.get("results").map(fmt::items).unwrap_or_default();
        Hits { hits: list.iter().filter_map(Hit::from).collect(), truncated: fmt::truthy(v, "truncated") }
    }
}

/// The words of a search, lowercased (none when nothing was typed).
pub fn find_words(q: &str) -> Vec<String> {
    q.split_whitespace().map(str::to_lowercase).collect()
}

/// Whether a name holds every word (lowercased, anywhere, in any order): the open folder's filter.
/// The server's search also drops accents; the filter only lowercases.
pub fn name_matches(name: &str, words: &[String]) -> bool {
    let n = name.to_lowercase();
    words.iter().all(|w| n.contains(w.as_str()))
}

/// The heading of the search's results: "3 found in Library", "200+ found in Library …" when the
/// server stopped early, "Nothing in Library is called that".
pub fn found_line(n: usize, truncated: bool, place: &str) -> String {
    match (n, truncated) {
        (0, false) => format!("Nothing in {place} is called that"),
        (0, true) => format!("Nothing found yet in {place}: the search stopped early, try more words"),
        (n, true) => format!("{n}+ found in {place} (the nearest first; type more to narrow it)"),
        (1, false) => format!("1 found in {place}"),
        (n, false) => format!("{n} found in {place}"),
    }
}

/// Something in the trash (§4.8).
#[derive(Clone, Debug, PartialEq)]
pub struct TrashItem {
    pub id: String,
    pub name: String,
    pub root: String,
    pub path: String,
    pub at: f64,
    pub by: String,
    pub size: f64,
    pub kind: String,
}

impl TrashItem {
    pub fn from(v: &Value) -> Option<TrashItem> {
        let id = fmt::s(v, "id");
        if id.is_empty() {
            return None;
        }
        let path = fmt::s(v, "path");
        let name = fmt::s(v, "name");
        Some(TrashItem {
            name: if name.is_empty() { base_name(&path).to_string() } else { name },
            root: fmt::s(v, "root"),
            at: fmt::f(v, "at"),
            by: fmt::s(v, "by"),
            size: fmt::f(v, "size"),
            kind: fmt::s(v, "kind"),
            path,
            id,
        })
    }

    /// "From Library › Artist · 12 Mar 2026 · by admin · 31.5 MB".
    pub fn line(&self, roots: &[Root]) -> String {
        let root = roots
            .iter()
            .find(|r| r.id == self.root)
            .map(|r| r.name.as_str())
            .unwrap_or(self.root.as_str());
        let parent = parent_of(&self.path);
        let mut parts = vec![format!("From {}", place(root, &parent))];
        if self.at > 0.0 {
            parts.push(fmt::date(self.at));
        }
        if !self.by.is_empty() {
            parts.push(format!("by {}", self.by));
        }
        if self.size > 0.0 {
            parts.push(fmt::size(self.size));
        }
        parts.join(" · ")
    }
}

/// A release an album folder can be tagged as (§2.4).
#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub date: String,
    pub provider: String,
    pub track_count: i64,
    /// 0…1 (`match`, or `score` / 100).
    pub score: f64,
}

impl Release {
    pub fn from(v: &Value) -> Option<Release> {
        let id = fmt::s(v, "id");
        if id.is_empty() {
            return None;
        }
        Some(Release {
            title: fmt::s(v, "title"),
            artist: fmt::s(v, "artist"),
            date: fmt::s(v, "date"),
            provider: fmt::s(v, "provider"),
            track_count: fmt::i(v, "track_count"),
            score: if v.get("match").is_some_and(|m| m.is_number()) {
                fmt::f(v, "match")
            } else {
                fmt::f(v, "score") / 100.0
            },
            id,
        })
    }

    /// "Gnarls Barkley – St. Elsewhere (2006) · MusicBrainz".
    pub fn line(&self) -> String {
        let mut s = [self.artist.as_str(), self.title.as_str()]
            .into_iter()
            .filter(|x| !x.is_empty())
            .collect::<Vec<_>>()
            .join(" – ");
        if self.date.len() >= 4 {
            s.push_str(&format!(" ({})", &self.date[..4]));
        }
        if !self.provider.is_empty() {
            s.push_str(&format!(" · {}", self.provider));
        }
        s
    }
}

/// One file of an album folder, before and after.
#[derive(Clone, Debug, PartialEq)]
pub struct ImportRow {
    pub path: String,
    pub old_title: String,
    pub new_title: String,
    pub new_track: i64,
}

/// One album folder of an import, with the release it matched (§2.4).
#[derive(Clone, Debug, PartialEq)]
pub struct ImportAlbum {
    pub folder: String,
    pub artist: String,
    pub album: String,
    pub files: i64,
    pub release: Option<Release>,
    pub confidence: f64,
    pub rows: Vec<ImportRow>,
}

impl ImportAlbum {
    pub fn from(v: &Value) -> ImportAlbum {
        let folder = fmt::s(v, "folder");
        let rows: Vec<ImportRow> = fmt::items(&v["rows"])
            .iter()
            .map(|r| ImportRow {
                path: fmt::s(r, "path"),
                old_title: fmt::s(r, "old_title"),
                new_title: fmt::s(r, "new_title"),
                new_track: fmt::i(r, "new_track"),
            })
            .collect();
        let album = fmt::s(v, "album");
        ImportAlbum {
            album: if album.is_empty() { base_name(&folder).to_string() } else { album },
            artist: fmt::s(v, "artist"),
            files: if v.get("files").is_some() { fmt::i(v, "files") } else { rows.len() as i64 },
            release: v.get("release").and_then(Release::from),
            confidence: fmt::f(v, "confidence"),
            rows,
            folder,
        }
    }

    /// "Artist – Album", or the folder.
    pub fn title(&self) -> String {
        let s = [self.artist.as_str(), self.album.as_str()]
            .into_iter()
            .filter(|x| !x.is_empty())
            .collect::<Vec<_>>()
            .join(" – ");
        if s.is_empty() {
            self.folder.clone()
        } else {
            s
        }
    }

    /// "Matches … · 92% sure", or the words for no match.
    pub fn match_line(&self) -> String {
        match &self.release {
            Some(r) if self.confidence > 0.0 => {
                format!("Matches {} · {}% sure", r.line(), (self.confidence * 100.0).round() as i64)
            }
            Some(r) => format!("Matches {}", r.line()),
            None => NO_MATCH.to_string(),
        }
    }
}

/// The albums of an import preview (`{albums}` or a bare list).
pub fn albums_of(v: &Value) -> Vec<ImportAlbum> {
    let list = match v.get("albums") {
        Some(a) => fmt::items(a),
        None => fmt::items(v),
    };
    list.iter().map(ImportAlbum::from).collect()
}

/// The releases of a match search (`{candidates}` or a bare list).
pub fn releases_of(v: &Value) -> Vec<Release> {
    let list = match v.get("candidates") {
        Some(a) => fmt::items(a),
        None => fmt::items(v),
    };
    list.iter().filter_map(Release::from).collect()
}

// ----- paths, names and words -------------------------------------------------------------------

/// "a/b" + "c" → "a/b/c"; "" + "c" → "c".
pub fn join(path: &str, name: &str) -> String {
    if path.is_empty() {
        name.to_string()
    } else {
        format!("{path}/{name}")
    }
}

/// "a/b/c" → "a/b"; "a" → "".
pub fn parent_of(path: &str) -> String {
    path.rsplit_once('/').map(|(p, _)| p.to_string()).unwrap_or_default()
}

/// "a/b/c" → "c".
pub fn base_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// "Library › Artist › Album".
pub fn place(root: &str, path: &str) -> String {
    if path.is_empty() {
        root.to_string()
    } else {
        format!("{root} › {}", path.replace('/', " › "))
    }
}

/// `path` is the link folder itself or below it.
fn under(link: &str, path: &str) -> bool {
    path == link || path.strip_prefix(link).is_some_and(|rest| rest.starts_with('/'))
}

/// The order of a sort in words ("A–Z", "largest first" …).
pub fn sort_order(sort: &str, dir: &str) -> &'static str {
    let asc = dir == "asc";
    match sort {
        "size" => if asc { "smallest first" } else { "largest first" },
        "mtime" => if asc { "oldest first" } else { "newest first" },
        _ => if asc { "A–Z" } else { "Z–A" },
    }
}

/// The order a sort starts in: names A–Z, sizes and dates largest and newest first.
pub fn default_dir(sort: &str) -> &'static str {
    if sort == "name" {
        "asc"
    } else {
        "desc"
    }
}

/// The Sort button: "Sort: Name ↑".
pub fn sort_label(sort: &str, dir: &str) -> String {
    let words = SORTS.iter().find(|(k, _)| *k == sort).map(|(_, w)| *w).unwrap_or("Name");
    format!("Sort: {words} {}", if dir == "desc" { "↓" } else { "↑" })
}

/// `{failed: [{path | id, reason}]}` as one line ("" when nothing failed): "<name>: <reason>", or
/// "N weren't done. <name>: <reason>".
pub fn failures(v: &Value) -> String {
    let failed = fmt::items(&v["failed"]);
    let Some(first) = failed.first() else {
        return String::new();
    };
    let path = fmt::s(first, "path");
    let what = if path.is_empty() {
        let folder = fmt::s(first, "folder");
        if folder.is_empty() {
            fmt::s(first, "id")
        } else {
            folder
        }
    } else {
        base_name(&path).to_string()
    };
    let line = format!("{what}: {}", fmt::s(first, "reason"));
    if failed.len() == 1 {
        line
    } else {
        format!("{} weren't done. {line}", failed.len())
    }
}

/// A name the server takes as one path part: no slashes, no control characters, trimmed.
pub fn safe_name(name: &str) -> String {
    name.chars()
        .map(|c| if c == '/' || c == '\\' { '_' } else { c })
        .filter(|c| !c.is_control())
        .collect::<String>()
        .trim()
        .to_string()
}

/// Why a new name can't be used, before asking the server ("" = it is empty or only dots: the
/// button is off without a word).
pub fn name_problem(name: &str) -> Option<&'static str> {
    let n = name.trim();
    if n.is_empty() || n == "." || n == ".." {
        return Some("");
    }
    if n.contains('/') || n.contains('\\') {
        return Some(SLASH);
    }
    if n.chars().any(char::is_control) {
        return Some("A name can't contain invisible or control characters");
    }
    if n.len() > 255 {
        return Some("That name is too long");
    }
    None
}

/// A click on row `i`: Shift selects from the anchor to `i` (added to what is there with Ctrl/⌘),
/// Ctrl/⌘ toggles `i`, a plain click selects `i` alone. Returns the selection and the new anchor.
pub fn click_select(
    selected: &BTreeSet<usize>,
    anchor: Option<usize>,
    i: usize,
    shift: bool,
    toggle: bool,
) -> (BTreeSet<usize>, Option<usize>) {
    match (shift, anchor) {
        (true, Some(a)) => {
            let mut s = if toggle { selected.clone() } else { BTreeSet::new() };
            s.extend(a.min(i)..=a.max(i));
            (s, Some(a))
        }
        _ if toggle => {
            let mut s = selected.clone();
            if !s.remove(&i) {
                s.insert(i);
            }
            (s, Some(i))
        }
        _ => (BTreeSet::from([i]), Some(i)),
    }
}

// ----- uploads ---------------------------------------------------------------------------------

/// One file to upload: where it is here, its name on the server relative to the folder (with `/` for
/// the files of a folder: "Album/CD1/01 Song.flac"), its size.
#[derive(Clone, Debug, PartialEq)]
pub struct UploadItem {
    pub file: PathBuf,
    pub name: String,
    pub size: u64,
}

/// What the server did with one uploaded file.
#[derive(Clone, Debug, PartialEq)]
pub enum UploadOutcome {
    Uploaded,
    /// It got another name.
    Renamed(String),
    /// Skipped, with the server's reason.
    Skipped(String),
}

/// The answer to an upload of one file (§4.3).
pub fn upload_outcome(v: &Value) -> UploadOutcome {
    if let Some(s) = fmt::items(&v["skipped"]).first() {
        let reason = fmt::s(s, "reason");
        return UploadOutcome::Skipped(if reason.is_empty() { "skipped".into() } else { reason });
    }
    if let Some(r) = fmt::items(&v["renamed"]).first() {
        return UploadOutcome::Renamed(fmt::s(r, "to"));
    }
    UploadOutcome::Uploaded
}

/// Every file under `dir` (links and hidden names left out), named `<prefix><sub>/<file>` as a
/// browser's folder upload names them; at most [`MAX_FILES`], [`MAX_DEPTH`] folders deep.
pub fn walk(dir: &Path, prefix: &str, out: &mut Vec<UploadItem>, depth: usize) {
    if out.len() >= MAX_FILES || depth > MAX_DEPTH {
        return;
    }
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = read.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    let mut dirs = Vec::new();
    for e in entries {
        if out.len() >= MAX_FILES {
            return;
        }
        let name = safe_name(&e.file_name().to_string_lossy());
        if name.is_empty() || name.starts_with('.') {
            continue;
        }
        let Ok(meta) = std::fs::symlink_metadata(e.path()) else {
            continue;
        };
        if meta.is_dir() {
            dirs.push((e.path(), name));
        } else if meta.is_file() {
            out.push(UploadItem {
                file: e.path(),
                name: format!("{prefix}{name}"),
                size: meta.len(),
            });
        }
    }
    for (path, name) in dirs {
        walk(&path, &format!("{prefix}{name}/"), out, depth + 1);
    }
}

/// What was picked or dropped, as upload items: a file under its name, a folder as all its files.
pub fn items_of(paths: &[PathBuf]) -> Vec<UploadItem> {
    let mut out = Vec::new();
    for p in paths {
        // what the person picked is followed even when it is a link; links inside a folder are not
        let Ok(meta) = std::fs::metadata(p) else {
            continue;
        };
        let name = safe_name(&p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
        if name.is_empty() {
            continue;
        }
        if meta.is_dir() {
            walk(p, &format!("{name}/"), &mut out, 0);
        } else if meta.is_file() {
            out.push(UploadItem {
                file: p.clone(),
                name,
                size: meta.len(),
            });
        }
    }
    out
}

/// One file of an upload with where it goes.
#[derive(Clone, Debug)]
struct Job {
    item: UploadItem,
    root: String,
    path: String,
    conflict: String,
}

/// The upload that is running: one file per request, the bytes sent of the file at hand counted by
/// the request's body as it reads the disk.
#[derive(Debug)]
struct Upload {
    jobs: Vec<Job>,
    index: usize,
    ok: usize,
    failed: usize,
    first_error: String,
    /// Bytes of the files before this one.
    sent_before: u64,
    total: u64,
    sent: Arc<AtomicU64>,
    cancel: Arc<AtomicBool>,
}

/// `POST /api/files/upload?root=&path=&conflict=` with one file as the multipart part `file`, its
/// filename the name on the server, streamed from the disk (never held in memory).
async fn upload_one(
    api: Arc<Api>,
    item: UploadItem,
    root: String,
    path: String,
    conflict: String,
    sent: Arc<AtomicU64>,
    cancel: Arc<AtomicBool>,
) -> Result<UploadOutcome, String> {
    use tokio::io::AsyncReadExt;
    let file = tokio::fs::File::open(&item.file)
        .await
        .map_err(|e| format!("Couldn't read {}: {e}", item.file.display()))?;
    let len = file
        .metadata()
        .await
        .map_err(|e| format!("Couldn't read {}: {e}", item.file.display()))?
        .len();
    let body = futures_util::stream::try_unfold((file, len, sent, cancel), |(mut f, left, sent, cancel)| async move {
        if cancel.load(Ordering::Relaxed) {
            return Err(std::io::Error::other(STOPPED));
        }
        if left == 0 {
            return Ok(None);
        }
        let mut buf = vec![0u8; (64 * 1024).min(left as usize)];
        let n = f.read(&mut buf).await?;
        if n == 0 {
            // shorter than when the upload began
            return Err(std::io::Error::other(CHANGED));
        }
        buf.truncate(n);
        sent.fetch_add(n as u64, Ordering::Relaxed);
        Ok(Some((buf, (f, left - n as u64, sent, cancel))))
    });
    let part = reqwest::multipart::Part::stream_with_length(reqwest::Body::wrap_stream(body), len).file_name(item.name.clone());
    let url = format!(
        "/files/upload?root={}&path={}&conflict={}",
        urlencode(&root),
        urlencode(&path),
        urlencode(&conflict)
    );
    let v = api
        .post_form(&url, reqwest::multipart::Form::new().part("file", part))
        .await
        .map_err(|e| e.message)?;
    Ok(upload_outcome(&v))
}

// ----- the page --------------------------------------------------------------------------------

/// What a dialog is about.
#[derive(Debug)]
enum Dialog {
    NewFolder { name: String },
    Rename { from: String, name: String },
    /// The folder picker: the names to move, the root and folder looked at, its folders.
    Move { names: Vec<String>, root: String, path: String, dirs: Option<Result<Vec<String>, String>>, seq: u64 },
    Trash { names: Vec<String> },
    /// One audio file's tags: as read (None while loading) and as typed, in [`TAGS`] order.
    Tags { name: String, original: Option<Result<Vec<String>, String>>, values: Vec<String>, saving: bool },
    Upload { items: Vec<UploadItem>, conflict: String },
    Import(Box<Import>),
    DeleteForever { id: String, name: String },
    EmptyTrash,
}

/// Import… on a folder: the album folders in it and the releases they match.
#[derive(Debug)]
struct Import {
    /// The folder imported, relative to the root.
    folder: String,
    keep: bool,
    albums: Option<Result<Vec<ImportAlbum>, String>>,
    /// Album folders left out, and those whose files are shown.
    skip: HashSet<String>,
    open: HashSet<String>,
    others: Option<Others>,
    applying: bool,
    seq: u64,
}

/// "Other matches" of one album folder.
#[derive(Debug)]
struct Others {
    folder: String,
    artist: String,
    album: String,
    found: Option<Vec<Release>>,
    searching: bool,
    picking: bool,
    seq: u64,
}

/// A folder in the tree: its name, and whether it is a link to a folder.
type TreeDir = (String, bool);

/// The folders of a tree node: None while they are asked for.
type TreeDirs = Option<Result<Vec<TreeDir>, String>>;

/// What a ⋯ or button menu is for.
#[derive(Clone, Debug)]
enum Menu {
    /// The entries (names) a row menu acts on.
    Entries(Vec<String>),
    Sort,
    Upload,
    /// A trash item (index).
    Trash(usize),
}

/// A write whose answer is awaited.
#[derive(Clone, Debug)]
pub enum Act {
    Mkdir,
    Rename,
    Move { to_root: String },
    Trash,
    Tags,
    Restore,
    Delete,
    Empty,
}

/// What a key does in the list.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum KeyAct {
    Open,
    Up,
    Trash,
    /// F2.
    Rename,
    All,
    Clear,
    /// A letter: the next name starting with it.
    Jump(char),
    /// One row down (1) or up (-1); with Shift the selection grows.
    Step(i32, bool),
    /// Ctrl/⌘+F or `/`: to the search box.
    Find,
}

#[derive(Debug)]
pub struct State {
    roots: Option<Result<Vec<Root>, String>>,
    /// The open root (None = the list of roots).
    root: Option<String>,
    /// The open folder, relative to the root.
    path: String,
    path_box: String,
    /// The first link folder this browsing went through: nothing at or below it is written (§2.4).
    link_at: Option<String>,
    sort: String,
    dir: String,
    listing: Option<Result<Listing, String>>,
    loading_more: bool,
    /// Bumped by every folder request: an older answer is dropped.
    seq: u64,
    selected: BTreeSet<String>,
    anchor: Option<usize>,
    cursor: Option<usize>,
    mods: Modifiers,
    /// The tree: the folders of (root, path) once asked for, and the open nodes.
    tree: HashMap<(String, String), TreeDirs>,
    tree_open: HashSet<(String, String)>,
    trash: Option<Result<Vec<TrashItem>, String>>,
    dialog: Option<Dialog>,
    menu: Option<(Point, Menu)>,
    upload: Option<Upload>,
    /// Files are held over the window.
    hover: bool,
    /// The library's song menu and dialogs (Edit song details… is the Fix song dialog itself).
    shared: library::Shared,
    /// The saved sort was read.
    sort_read: bool,
    /// What the search box holds.
    find: String,
    /// The open folder's entries the search's words let through (None while the box is blank).
    shown: Option<Vec<Entry>>,
    /// The search across the open root: None before it answers (or while the box is blank).
    hits: Option<Result<Hits, String>>,
    /// A search is on its way (after the pause).
    finding: bool,
    /// Bumped by every change of the box: an older search (or pause) is dropped.
    find_seq: u64,
}

impl Default for State {
    fn default() -> Self {
        State {
            roots: None,
            root: None,
            path: String::new(),
            path_box: String::new(),
            link_at: None,
            sort: "name".into(),
            dir: "asc".into(),
            listing: None,
            loading_more: false,
            seq: 0,
            selected: BTreeSet::new(),
            anchor: None,
            cursor: None,
            mods: Modifiers::default(),
            tree: HashMap::new(),
            tree_open: HashSet::new(),
            trash: None,
            dialog: None,
            menu: None,
            upload: None,
            hover: false,
            shared: library::Shared::default(),
            sort_read: false,
            find: String::new(),
            shown: None,
            hits: None,
            finding: false,
            find_seq: 0,
        }
    }
}

impl State {
    fn roots(&self) -> &[Root] {
        match &self.roots {
            Some(Ok(r)) => r,
            _ => &[],
        }
    }

    fn current_root(&self) -> Option<&Root> {
        let id = self.root.as_deref()?;
        self.roots().iter().find(|r| r.id == id)
    }

    /// The rows shown: the folder's entries, filtered by the search box when it holds words.
    fn entries(&self) -> &[Entry] {
        if let Some(shown) = &self.shown {
            return shown;
        }
        match &self.listing {
            Some(Ok(l)) => &l.entries,
            _ => &[],
        }
    }

    /// The filter again from the folder's entries and the box's words; the picked names that are
    /// filtered out are let go (nothing hidden is moved or trashed).
    fn refilter(&mut self) {
        let words = find_words(&self.find);
        self.shown = match (&self.listing, words.is_empty()) {
            (_, true) => None,
            (Some(Ok(l)), false) => Some(l.entries.iter().filter(|e| name_matches(&e.name, &words)).cloned().collect()),
            (_, false) => Some(Vec::new()),
        };
        if self.shown.is_some() {
            let shown: HashSet<String> = self.entries().iter().map(|e| e.name.clone()).collect();
            self.selected.retain(|n| shown.contains(n));
            self.anchor = None;
            self.cursor = None;
        }
    }

    /// The search box, its filter and its results emptied (a folder is opened next).
    fn clear_find(&mut self) {
        self.find.clear();
        self.shown = None;
        self.hits = None;
        self.finding = false;
        self.find_seq += 1;
    }

    /// The open folder was reached through a link.
    fn via_link(&self) -> bool {
        matches!(&self.listing, Some(Ok(l)) if l.via_link)
            || self.link_at.as_deref().is_some_and(|l| under(l, &self.path))
    }

    /// Writes may go into the open folder (one that could be listed, not reached through a link).
    fn can_write(&self) -> bool {
        self.current_root().is_some_and(|r| r.writable && !r.is_trash())
            && !self.via_link()
            && !matches!(self.listing, Some(Err(_)))
    }

    fn selected_indices(&self) -> BTreeSet<usize> {
        self.entries()
            .iter()
            .enumerate()
            .filter(|(_, e)| self.selected.contains(&e.name))
            .map(|(i, _)| i)
            .collect()
    }

    fn select_indices(&mut self, set: &BTreeSet<usize>) {
        let names: BTreeSet<String> = set
            .iter()
            .filter_map(|i| self.entries().get(*i).map(|e| e.name.clone()))
            .collect();
        self.selected = names;
    }
}

#[derive(Clone, Debug)]
pub enum Message {
    Roots(Result<Value, ApiError>),
    Refresh,
    /// Open a root by id ("" = the list of roots).
    OpenRoot(String),
    /// A folder of the open root (breadcrumbs).
    Go(String),
    Up,
    PathInput(String),
    PathSubmit,
    Listed(u64, Result<Value, ApiError>),
    More,
    MoreListed(u64, Result<Value, ApiError>),
    Sort(String),
    SortMenu,
    Click(usize),
    Open(usize),
    RowMenu(usize),
    CloseMenu,
    SelectAll,
    ClearSelection,
    Modifiers(Modifiers),
    Key(KeyAct),
    TreeToggle(String, String),
    TreeListed(String, String, Result<Value, ApiError>),
    /// Open (root, path) from the tree, with the link folder on the way there.
    TreeGo(String, String, Option<String>),
    Play(i64),
    PlayRow(Result<Value, ApiError>),
    /// Edit song details… of a song the library has: its row first.
    FixSong(i64),
    FixRow(Result<Value, ApiError>),
    Shared(LibAct),
    /// Copy an error's whole text.
    Copy(String),
    AskNewFolder,
    AskRename(String),
    AskMove(Vec<String>),
    AskTrash(Vec<String>),
    AskTags(String),
    AskImport(String),
    NameInput(String),
    /// The open dialog's main button.
    Submit,
    /// Enter while a dialog is open and no text box took it: the confirms, Upload and Move go.
    Enter,
    CloseDialog,
    Done(Act, Result<Value, ApiError>),
    PickRoot(String),
    PickGo(String),
    PickListed(u64, Result<Value, ApiError>),
    TagsLoaded(Result<Value, ApiError>),
    TagInput(usize, String),
    UploadMenu,
    PickFiles,
    PickFolder,
    /// Files to upload (`true`: from a folder, which says so when it has none).
    Picked(Vec<UploadItem>, bool),
    Hover(bool),
    Dropped(PathBuf),
    Conflict(String),
    Uploaded(Result<UploadOutcome, String>),
    StopUpload,
    UploadTick,
    ImportKeep(bool),
    ImportPreview(u64, Result<Value, ApiError>),
    ImportSkip(String),
    ImportFiles(String),
    ImportOthers(String),
    OthersArtist(String),
    OthersAlbum(String),
    OthersSearch,
    OthersFound(u64, Result<Value, ApiError>),
    OthersPick(usize),
    OthersPicked(String, Box<Release>, Result<Value, ApiError>),
    OthersClose,
    ImportApplied(Result<Value, ApiError>),
    TrashLoaded(Result<Value, ApiError>),
    TrashMenu(usize),
    Restore(String),
    AskDeleteForever(String),
    AskEmptyTrash,
    /// The search box changed.
    FindInput(String),
    /// Typing paused: search the root (dropped when the box changed since).
    FindRun(u64),
    /// The search's answer.
    Found(u64, Result<Value, ApiError>),
    /// Enter in the search box: the first row the filter left, else the first result.
    FindSubmit,
    /// A search result clicked.
    FindGo(usize),
    /// Empty the search box.
    FindClear,
}

fn list_url(root: &str, path: &str, sort: &str, dir: &str, offset: usize) -> String {
    format!(
        "/files/list?root={}&path={}&sort={}&dir={}&offset={offset}&limit={PAGE}",
        urlencode(root),
        urlencode(path),
        urlencode(sort),
        urlencode(dir)
    )
}

fn load_roots(ctx: &Ctx) -> Task<Message> {
    let api = ctx.api.clone();
    Task::perform(async move { api.get("/files/roots").await }, Message::Roots)
}

/// Ask for what is open: the trash, a folder, or nothing (the list of roots). `fresh`: a new
/// place (the old entries go); otherwise they stay until the answer is there.
fn load(state: &mut State, ctx: &Ctx, fresh: bool) -> Task<Message> {
    state.seq += 1;
    let seq = state.seq;
    let api = ctx.api.clone();
    match state.root.clone() {
        None => Task::none(),
        Some(r) if r == "trash" => {
            if fresh {
                state.trash = None;
            }
            Task::perform(async move { api.get("/files/trash").await }, Message::TrashLoaded)
        }
        Some(r) => {
            if fresh {
                state.listing = None;
            }
            state.loading_more = false;
            let url = list_url(&r, &state.path, &state.sort, &state.dir, 0);
            Task::perform(async move { api.get(&url).await }, move |res| Message::Listed(seq, res))
        }
    }
}

/// Open `path` of the open root with `link_at` as the link folder on the way.
fn go_at(state: &mut State, path: String, link_at: Option<String>, ctx: &Ctx) -> Task<Message> {
    // the search's words were for the folder left; they'd hide what the new one holds
    state.clear_find();
    state.link_at = link_at;
    state.path_box = path.clone();
    state.path = path;
    state.selected.clear();
    state.anchor = None;
    state.cursor = None;
    state.menu = None;
    load(state, ctx, true)
}

/// Open `path`; `link`: it is a link to a folder (the first one on the way is remembered).
fn go(state: &mut State, path: String, link: bool, ctx: &Ctx) -> Task<Message> {
    let link_at = match state.link_at.take() {
        Some(l) if under(&l, &path) => Some(l),
        _ if link => Some(path.clone()),
        _ => None,
    };
    go_at(state, path, link_at, ctx)
}

fn open_root(state: &mut State, id: String, ctx: &Ctx) -> Task<Message> {
    state.root = (!id.is_empty()).then_some(id);
    state.listing = None;
    go_at(state, String::new(), None, ctx)
}

fn tree_load(ctx: &Ctx, root: String, path: String) -> Task<Message> {
    let api = ctx.api.clone();
    let url = list_url(&root, &path, "name", "asc", 0);
    Task::perform(async move { api.get(&url).await }, move |r| Message::TreeListed(root, path, r))
}

/// Ask again for the open tree nodes of a root (after a write changed its folders).
fn tree_reload(state: &mut State, root: &str, ctx: &Ctx) -> Task<Message> {
    let open: Vec<(String, String)> = state.tree_open.iter().filter(|(r, _)| r == root).cloned().collect();
    state.tree.retain(|(r, _), _| r != root);
    Task::batch(open.into_iter().map(|(r, p)| {
        state.tree.insert((r.clone(), p.clone()), None);
        tree_load(ctx, r, p)
    }))
}

/// After a write: the folder, the free space and the tree again.
fn after_write(state: &mut State, ctx: &Ctx, roots: &[String]) -> Task<Message> {
    state.selected.clear();
    state.anchor = None;
    state.cursor = None;
    let mut tasks = vec![load(state, ctx, false), load_roots(ctx)];
    for r in roots {
        tasks.push(tree_reload(state, r, ctx));
    }
    Task::batch(tasks)
}

fn post(ctx: &Ctx, path: &'static str, body: Value, act: Act) -> Task<Message> {
    let api = ctx.api.clone();
    Task::perform(async move { api.post(path, body).await }, move |r| Message::Done(act, r))
}

/// Ask for the folders of the move dialog's root and folder.
fn picker_load(state: &mut State, ctx: &Ctx) -> Task<Message> {
    let Some(Dialog::Move { root, path, dirs, seq, .. }) = state.dialog.as_mut() else {
        return Task::none();
    };
    *seq += 1;
    *dirs = None;
    let s = *seq;
    let api = ctx.api.clone();
    let url = list_url(root, path, "name", "asc", 0);
    Task::perform(async move { api.get(&url).await }, move |r| Message::PickListed(s, r))
}

fn import_preview(state: &mut State, ctx: &Ctx) -> Task<Message> {
    let root = state.root.clone().unwrap_or_default();
    let Some(Dialog::Import(imp)) = state.dialog.as_mut() else {
        return Task::none();
    };
    imp.seq += 1;
    imp.albums = None;
    imp.skip.clear();
    let seq = imp.seq;
    let body = json!({ "root": root, "path": imp.folder, "mode": if imp.keep { "in_place" } else { "move" } });
    let api = ctx.api.clone();
    Task::perform(async move { api.post("/files/import/preview", body).await }, move |r| {
        Message::ImportPreview(seq, r)
    })
}

fn others_search(state: &mut State, ctx: &Ctx) -> Task<Message> {
    let root = state.root.clone().unwrap_or_default();
    let Some(Dialog::Import(imp)) = state.dialog.as_mut() else {
        return Task::none();
    };
    let Some(o) = imp.others.as_mut() else {
        return Task::none();
    };
    o.seq += 1;
    o.searching = true;
    let seq = o.seq;
    let body = json!({ "root": root, "path": o.folder, "artist": o.artist.trim(), "album": o.album.trim() });
    let api = ctx.api.clone();
    Task::perform(async move { api.post("/files/import/match", body).await }, move |r| {
        Message::OthersFound(seq, r)
    })
}

/// Start the file at hand of the running upload, or end the upload.
fn upload_next(state: &mut State, ctx: &mut Ctx) -> Task<Message> {
    let Some(u) = state.upload.as_mut() else {
        return Task::none();
    };
    if u.cancel.load(Ordering::Relaxed) || u.index >= u.jobs.len() {
        return upload_end(state, ctx);
    }
    let job = u.jobs[u.index].clone();
    u.sent.store(0, Ordering::Relaxed);
    let (api, sent, cancel) = (ctx.api.clone(), u.sent.clone(), u.cancel.clone());
    Task::perform(
        upload_one(api, job.item, job.root, job.path, job.conflict, sent, cancel),
        Message::Uploaded,
    )
}

/// The upload is over: say how it went and show what arrived.
fn upload_end(state: &mut State, ctx: &mut Ctx) -> Task<Message> {
    let Some(u) = state.upload.take() else {
        return Task::none();
    };
    let count = u.jobs.len();
    if u.cancel.load(Ordering::Relaxed) {
        ctx.toast(format!("Upload stopped after {} of {count}", u.ok));
    } else if u.failed == 0 {
        ctx.toast_ok(if u.ok == 1 { "Uploaded 1 file".to_string() } else { format!("Uploaded {} files", u.ok) });
    } else {
        ctx.toast_warn(format!("Uploaded {} of {count}. {}", u.ok, u.first_error));
    }
    let roots: Vec<String> = u.jobs.iter().map(|j| j.root.clone()).collect::<BTreeSet<_>>().into_iter().collect();
    after_write(state, ctx, &roots)
}

/// Open a row: a folder (or a link to one) is entered, a song of the library plays.
fn open_entry(state: &mut State, i: usize, ctx: &mut Ctx) -> Task<Message> {
    let Some(e) = state.entries().get(i).cloned() else {
        return Task::none();
    };
    if e.is_dir() {
        return go(state, join(&state.path, &e.name), e.is_link(), ctx);
    }
    if e.track_id() > 0 {
        return play(ctx, e.track_id());
    }
    Task::none()
}

fn play(ctx: &Ctx, id: i64) -> Task<Message> {
    let api = ctx.api.clone();
    Task::perform(async move { api.get(&format!("/library/tracks/{id}")).await }, Message::PlayRow)
}

/// Set by "Open Trash" (the toast after albums went to the Trash, 0.36): Files opens on the Trash next time.
static OPEN_ON_TRASH: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Make the next opening of Files show the Trash (then navigate to Settings → Files).
pub fn open_on_trash() {
    OPEN_ON_TRASH.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Called when Settings opens this section.
pub fn open(state: &mut State, ctx: &mut Ctx) -> Task<Message> {
    state.menu = None;
    state.hover = false;
    state.shared.reset();
    if !ctx.is_admin() {
        return Task::none();
    }
    if OPEN_ON_TRASH.swap(false, std::sync::atomic::Ordering::Relaxed) {
        return Task::batch([load_roots(ctx), open_root(state, "trash".into(), ctx)]);
    }
    if !state.sort_read {
        state.sort_read = true;
        if let Some((sort, dir)) = saved_sort(ctx.config.local(SORT_KEY)) {
            state.sort = sort;
            state.dir = dir;
        }
    }
    let mut tasks = vec![load_roots(ctx), load(state, ctx, false)];
    let open: Vec<(String, String)> = state.tree_open.iter().cloned().collect();
    for (r, p) in open {
        tasks.push(tree_load(ctx, r, p));
    }
    Task::batch(tasks)
}

pub fn update(state: &mut State, msg: Message, ctx: &mut Ctx) -> Task<Message> {
    match msg {
        Message::Roots(Ok(v)) => {
            let roots: Vec<Root> = fmt::items(&v).iter().filter_map(Root::from).collect();
            if state.root.as_ref().is_some_and(|id| !roots.iter().any(|r| &r.id == id)) {
                state.root = None;
                state.listing = None;
            }
            state.roots = Some(Ok(roots));
        }
        Message::Roots(Err(e)) => {
            let msg = if e.status == 404 {
                NO_FILE_MANAGER.to_string()
            } else if e.message.is_empty() {
                "Couldn't load the folders".to_string()
            } else {
                e.message
            };
            state.roots = Some(Err(msg));
        }
        Message::Refresh => return Task::batch([load_roots(ctx), load(state, ctx, false)]),
        Message::OpenRoot(id) => return open_root(state, id, ctx),
        Message::Go(path) => return go(state, path, false, ctx),
        Message::Up => {
            if state.root.is_none() {
                return Task::none();
            }
            if state.path.is_empty() {
                return open_root(state, String::new(), ctx);
            }
            let up = parent_of(&state.path);
            return go(state, up, false, ctx);
        }
        Message::PathInput(s) => state.path_box = s,
        Message::PathSubmit => {
            // as typed: the server refuses what leaves the root (an absolute path, "..")
            let p = state.path_box.trim().trim_end_matches('/').to_string();
            return go(state, p, false, ctx);
        }
        Message::Listed(seq, r) => {
            if seq != state.seq {
                return Task::none();
            }
            match r {
                Ok(v) => {
                    let l = Listing::from(&v, 0);
                    state.selected.retain(|n| l.entries.iter().any(|e| &e.name == n));
                    // a search result's file picked before its folder was there: the cursor on it
                    if let Some(i) = l.entries.iter().position(|e| state.selected.len() == 1 && state.selected.contains(&e.name)) {
                        state.anchor = Some(i);
                        state.cursor = Some(i);
                    }
                    state.listing = Some(Ok(l));
                    state.refilter();
                }
                Err(e) => {
                    state.listing = Some(Err(if e.message.is_empty() {
                        "Couldn't open the folder".to_string()
                    } else {
                        e.message
                    }));
                }
            }
        }
        Message::More => {
            let (Some(root), Some(Ok(l))) = (state.root.clone(), &state.listing) else {
                return Task::none();
            };
            if state.loading_more {
                return Task::none();
            }
            state.loading_more = true;
            let url = list_url(&root, &state.path, &state.sort, &state.dir, l.entries.len());
            let (api, seq) = (ctx.api.clone(), state.seq);
            return Task::perform(async move { api.get(&url).await }, move |r| Message::MoreListed(seq, r));
        }
        Message::MoreListed(seq, r) => {
            if seq != state.seq {
                return Task::none();
            }
            state.loading_more = false;
            match (r, state.listing.as_mut()) {
                (Ok(v), Some(Ok(l))) => {
                    let more = Listing::from(&v, l.entries.len());
                    let known: HashSet<String> = l.entries.iter().map(|e| e.name.clone()).collect();
                    l.entries.extend(more.entries.into_iter().filter(|e| !known.contains(&e.name)));
                    l.total = more.total.max(l.entries.len());
                    if state.shown.is_some() {
                        state.refilter();
                    }
                }
                (Err(e), _) => ctx.toast_err(e.message),
                _ => {}
            }
        }
        Message::Sort(key) => {
            state.menu = None;
            if key == state.sort {
                state.dir = if state.dir == "asc" { "desc".into() } else { "asc".into() };
            } else {
                state.dir = default_dir(&key).into();
                state.sort = key;
            }
            state.anchor = None;
            state.cursor = None;
            let saved = json!({ "sort": state.sort, "dir": state.dir });
            ctx.edit_config(move |c| {
                c.local.insert(SORT_KEY.into(), saved);
            });
            return load(state, ctx, false);
        }
        Message::SortMenu => state.menu = Some((ctx.cursor, Menu::Sort)),
        Message::FindInput(q) => {
            state.find = q;
            state.refilter();
            state.find_seq += 1;
            let seq = state.find_seq;
            if find_words(&state.find).is_empty() || state.root.is_none() {
                state.hits = None;
                state.finding = false;
                return Task::none();
            }
            // the whole root once typing pauses
            return Task::perform(tokio::time::sleep(FIND_PAUSE), move |_| Message::FindRun(seq));
        }
        Message::FindRun(seq) => {
            let Some(root) = state.root.clone().filter(|_| seq == state.find_seq) else {
                return Task::none();
            };
            state.finding = true;
            let url = format!("/files/search?root={}&q={}&limit={FIND_LIMIT}", urlencode(&root), urlencode(state.find.trim()));
            let api = ctx.api.clone();
            return Task::perform(async move { api.get(&url).await }, move |r| Message::Found(seq, r));
        }
        Message::Found(seq, r) => {
            if seq != state.find_seq {
                return Task::none();
            }
            state.finding = false;
            state.hits = Some(match r {
                Ok(v) => Ok(Hits::from(&v)),
                Err(e) if e.message.is_empty() => Err("Couldn't search".to_string()),
                Err(e) => Err(e.message),
            });
        }
        Message::FindSubmit => {
            if let Some(first) = state.entries().first().map(|e| e.name.clone()) {
                state.selected = BTreeSet::from([first]);
                state.anchor = Some(0);
                state.cursor = Some(0);
            } else if matches!(&state.hits, Some(Ok(h)) if !h.hits.is_empty()) {
                return update(state, Message::FindGo(0), ctx);
            }
        }
        Message::FindGo(i) => {
            let Some(hit) = state.hits.as_ref().and_then(|h| h.as_ref().ok()).and_then(|h| h.hits.get(i)).cloned() else {
                return Task::none();
            };
            let (path, link, pick) = hit.target();
            let task = go(state, path, link, ctx);
            // picked now, kept by the listing's answer (it keeps the picked names that are there)
            if let Some(name) = pick {
                state.selected = BTreeSet::from([name]);
            }
            return task;
        }
        Message::FindClear => {
            state.clear_find();
            state.refilter();
        }
        Message::Click(i) => {
            state.menu = None;
            let (set, anchor) = click_select(
                &state.selected_indices(),
                state.anchor,
                i,
                state.mods.shift(),
                state.mods.command(),
            );
            state.select_indices(&set);
            state.anchor = anchor;
            state.cursor = Some(i);
        }
        Message::Open(i) => return open_entry(state, i, ctx),
        Message::RowMenu(i) => {
            let Some(name) = state.entries().get(i).map(|e| e.name.clone()) else {
                return Task::none();
            };
            if !state.selected.contains(&name) {
                state.selected = BTreeSet::from([name]);
                state.anchor = Some(i);
                state.cursor = Some(i);
            }
            let names: Vec<String> = state
                .entries()
                .iter()
                .filter(|e| state.selected.contains(&e.name))
                .map(|e| e.name.clone())
                .collect();
            state.menu = Some((ctx.cursor, Menu::Entries(names)));
        }
        Message::CloseMenu => state.menu = None,
        Message::SelectAll => {
            state.selected = state.entries().iter().map(|e| e.name.clone()).collect();
        }
        Message::ClearSelection => {
            state.selected.clear();
            state.anchor = None;
        }
        Message::Modifiers(m) => state.mods = m,
        Message::Key(k) => return key(state, k, ctx),
        Message::TreeToggle(root, path) => {
            let k = (root.clone(), path.clone());
            if state.tree_open.remove(&k) {
                return Task::none();
            }
            state.tree_open.insert(k.clone());
            if !matches!(state.tree.get(&k), Some(Some(Ok(_)))) {
                state.tree.insert(k, None);
                return tree_load(ctx, root, path);
            }
        }
        Message::TreeListed(root, path, r) => {
            let dirs = r
                .map(|v| {
                    Listing::from(&v, 0)
                        .entries
                        .into_iter()
                        .filter(|e| e.is_dir())
                        .map(|e| {
                            let link = e.is_link();
                            (e.name, link)
                        })
                        .collect()
                })
                .map_err(|e| e.message);
            state.tree.insert((root, path), Some(dirs));
        }
        Message::TreeGo(root, path, link_at) => {
            if state.root.as_deref() != Some(root.as_str()) {
                state.root = Some(root);
                state.listing = None;
            }
            return go_at(state, path, link_at, ctx);
        }
        Message::Play(id) => {
            state.menu = None;
            return play(ctx, id);
        }
        Message::PlayRow(Ok(v)) => ctx.play(vec![v], 0),
        Message::PlayRow(Err(_)) => ctx.toast_err("That song isn't in the library"),
        Message::FixSong(id) => {
            state.menu = None;
            let api = ctx.api.clone();
            return Task::perform(async move { api.get(&format!("/library/tracks/{id}")).await }, Message::FixRow);
        }
        Message::FixRow(Ok(v)) if v.is_object() => {
            return library::shared_update(&mut state.shared, LibAct::FixSong(Box::new(v)), ctx).map(Message::Shared);
        }
        Message::FixRow(_) => ctx.toast_err("That song isn't in the library"),
        Message::Shared(a) => {
            let task = library::shared_update(&mut state.shared, a, ctx).map(Message::Shared);
            state.shared.moved_to = None;
            if state.shared.take_changed() {
                // the song was fixed (its file may have moved): the folder shows it as it is now
                let roots: Vec<String> = state.root.iter().cloned().collect();
                return Task::batch([task, after_write(state, ctx, &roots)]);
            }
            return task;
        }
        Message::Copy(text) => ctx.copy_text(text),
        Message::AskNewFolder => {
            state.menu = None;
            state.dialog = Some(Dialog::NewFolder { name: String::new() });
            return operation::focus(NAME_ID);
        }
        Message::AskRename(from) => {
            state.menu = None;
            let is_dir = state.entries().iter().any(|e| e.name == from && e.kind == "dir");
            let stem = match from.rsplit_once('.') {
                Some((s, _)) if !s.is_empty() && !is_dir => s.chars().count(),
                _ => from.chars().count(),
            };
            state.dialog = Some(Dialog::Rename { name: from.clone(), from });
            return operation::focus(NAME_ID).chain(operation::select_range(NAME_ID, 0, stem));
        }
        Message::AskMove(names) => {
            state.menu = None;
            if names.is_empty() {
                return Task::none();
            }
            let here = state.current_root().filter(|r| r.writable && !r.is_trash()).map(|r| r.id.clone());
            let (root, path) = match here {
                Some(r) if !state.via_link() => (r, state.path.clone()),
                Some(r) => (r, String::new()),
                None => match state.roots().iter().find(|r| r.writable && !r.is_trash()) {
                    Some(r) => (r.id.clone(), String::new()),
                    None => return Task::none(),
                },
            };
            state.dialog = Some(Dialog::Move { names, root, path, dirs: None, seq: 0 });
            return picker_load(state, ctx);
        }
        Message::AskTrash(names) => {
            state.menu = None;
            if !names.is_empty() {
                state.dialog = Some(Dialog::Trash { names });
            }
        }
        Message::AskTags(name) => {
            state.menu = None;
            let root = state.root.clone().unwrap_or_default();
            let url = format!("/files/tags?root={}&path={}", urlencode(&root), urlencode(&join(&state.path, &name)));
            state.dialog = Some(Dialog::Tags { name, original: None, values: vec![String::new(); TAGS.len()], saving: false });
            let api = ctx.api.clone();
            return Task::perform(async move { api.get(&url).await }, Message::TagsLoaded);
        }
        Message::AskImport(folder) => {
            state.menu = None;
            // the library's own folder is where songs live already, and a read-only folder can
            // only be imported where it is
            let keep = state.current_root().is_none_or(|r| r.id == "library" || !r.writable);
            state.dialog = Some(Dialog::Import(Box::new(Import {
                folder,
                keep,
                albums: None,
                skip: HashSet::new(),
                open: HashSet::new(),
                others: None,
                applying: false,
                seq: 0,
            })));
            return import_preview(state, ctx);
        }
        Message::NameInput(s) => match state.dialog.as_mut() {
            Some(Dialog::NewFolder { name }) | Some(Dialog::Rename { name, .. }) => *name = s,
            _ => {}
        },
        Message::Submit => return submit(state, ctx),
        Message::Enter => {
            let goes = matches!(
                state.dialog,
                Some(Dialog::Trash { .. } | Dialog::DeleteForever { .. } | Dialog::EmptyTrash | Dialog::Upload { .. } | Dialog::Move { .. })
            );
            if goes && state.menu.is_none() && state.shared.dialog.is_none() {
                return submit(state, ctx);
            }
        }
        Message::CloseDialog => {
            // Escape closes the menu first, then Other matches, then the dialog
            if state.menu.take().is_some() {
                return Task::none();
            }
            if state.shared.menu.is_some() || state.shared.dialog.is_some() {
                state.shared.reset();
                return Task::none();
            }
            if let Some(Dialog::Import(imp)) = state.dialog.as_mut() {
                if imp.others.take().is_some() {
                    return Task::none();
                }
            }
            state.dialog = None;
        }
        Message::Done(act, r) => return done(state, act, r, ctx),
        Message::PickRoot(id) => {
            if let Some(Dialog::Move { root, path, .. }) = state.dialog.as_mut() {
                *root = id;
                path.clear();
            }
            return picker_load(state, ctx);
        }
        Message::PickGo(p) => {
            if let Some(Dialog::Move { path, .. }) = state.dialog.as_mut() {
                *path = p;
            }
            return picker_load(state, ctx);
        }
        Message::PickListed(s, r) => {
            if let Some(Dialog::Move { dirs, seq, .. }) = state.dialog.as_mut() {
                if *seq == s {
                    *dirs = Some(
                        r.map(|v| {
                            Listing::from(&v, 0)
                                .entries
                                .into_iter()
                                .filter(|e| e.kind == "dir")
                                .map(|e| e.name)
                                .collect()
                        })
                        .map_err(|_| "Couldn't read the folder".to_string()),
                    );
                }
            }
        }
        Message::TagsLoaded(r) => {
            if let Some(Dialog::Tags { original, values, .. }) = state.dialog.as_mut() {
                match r {
                    Ok(v) => {
                        let tags = if v["tags"].is_object() { &v["tags"] } else { &v };
                        let read: Vec<String> = TAGS.iter().map(|(k, _)| fmt::s(tags, k)).collect();
                        *values = read.clone();
                        *original = Some(Ok(read));
                    }
                    Err(e) => {
                        *original = Some(Err(if e.message.is_empty() { "Couldn't read the tags".into() } else { e.message }));
                    }
                }
            }
        }
        Message::TagInput(i, s) => {
            if let Some(Dialog::Tags { values, .. }) = state.dialog.as_mut() {
                if let Some(v) = values.get_mut(i) {
                    *v = s;
                }
            }
        }
        Message::UploadMenu => state.menu = Some((ctx.cursor, Menu::Upload)),
        Message::PickFiles => {
            state.menu = None;
            return Task::perform(
                async move {
                    let picked = rfd::AsyncFileDialog::new().set_title("Upload files").pick_files().await;
                    let paths: Vec<PathBuf> = picked.unwrap_or_default().iter().map(|f| f.path().to_path_buf()).collect();
                    tokio::task::spawn_blocking(move || items_of(&paths)).await.unwrap_or_default()
                },
                |items| Message::Picked(items, false),
            );
        }
        Message::PickFolder => {
            state.menu = None;
            return Task::perform(
                async move {
                    let dir = rfd::AsyncFileDialog::new().set_title("Upload a folder").pick_folder().await?;
                    let dir = dir.path().to_path_buf();
                    Some(tokio::task::spawn_blocking(move || items_of(&[dir])).await.unwrap_or_default())
                },
                |items| match items {
                    Some(items) => Message::Picked(items, true),
                    None => Message::Picked(Vec::new(), false),
                },
            );
        }
        Message::Picked(items, folder) => {
            if items.is_empty() {
                if folder {
                    ctx.toast("That folder has no files");
                }
                return Task::none();
            }
            let can_write = state.can_write();
            match state.dialog.as_mut() {
                Some(Dialog::Upload { items: have, .. }) => {
                    for it in items {
                        if !have.iter().any(|h| h.file == it.file) {
                            have.push(it);
                        }
                    }
                }
                None if can_write => {
                    state.menu = None;
                    state.dialog = Some(Dialog::Upload { items, conflict: "rename".into() });
                }
                _ => {}
            }
        }
        Message::Hover(on) => state.hover = on && state.can_write(),
        Message::Dropped(p) => {
            state.hover = false;
            let takes = match &state.dialog {
                Some(Dialog::Upload { .. }) => true,
                None => state.can_write(),
                _ => false,
            };
            if takes {
                return Task::perform(
                    async move { tokio::task::spawn_blocking(move || items_of(&[p])).await.unwrap_or_default() },
                    |items| Message::Picked(items, true),
                );
            }
        }
        Message::Conflict(c) => {
            if let Some(Dialog::Upload { conflict, .. }) = state.dialog.as_mut() {
                *conflict = c;
            }
        }
        Message::Uploaded(r) => {
            let Some(u) = state.upload.as_mut() else {
                return Task::none();
            };
            let stopped = u.cancel.load(Ordering::Relaxed);
            let name = u.jobs.get(u.index).map(|j| base_name(&j.item.name).to_string()).unwrap_or_default();
            match r {
                Ok(UploadOutcome::Skipped(reason)) => {
                    u.failed += 1;
                    if u.first_error.is_empty() {
                        u.first_error = format!("{name}: {reason}");
                    }
                }
                Ok(_) => u.ok += 1,
                Err(_) if stopped => {}
                Err(e) => {
                    u.failed += 1;
                    if u.first_error.is_empty() {
                        u.first_error = format!("{name}: {e}");
                    }
                }
            }
            u.sent_before += u.jobs.get(u.index).map(|j| j.item.size).unwrap_or(0);
            u.index += 1;
            return upload_next(state, ctx);
        }
        Message::StopUpload => {
            if let Some(u) = &state.upload {
                u.cancel.store(true, Ordering::Relaxed);
            }
        }
        Message::UploadTick => {}
        Message::ImportKeep(on) => {
            if let Some(Dialog::Import(imp)) = state.dialog.as_mut() {
                if imp.keep == on || imp.applying {
                    return Task::none();
                }
                imp.keep = on;
            }
            return import_preview(state, ctx);
        }
        Message::ImportPreview(seq, r) => {
            if let Some(Dialog::Import(imp)) = state.dialog.as_mut() {
                if imp.seq == seq {
                    imp.albums = Some(r.map(|v| albums_of(&v)).map_err(|e| {
                        if e.message.is_empty() {
                            "Couldn't read the folder".to_string()
                        } else {
                            e.message
                        }
                    }));
                }
            }
        }
        Message::ImportSkip(folder) => {
            if let Some(Dialog::Import(imp)) = state.dialog.as_mut() {
                if !imp.skip.remove(&folder) {
                    imp.skip.insert(folder);
                }
            }
        }
        Message::ImportFiles(folder) => {
            if let Some(Dialog::Import(imp)) = state.dialog.as_mut() {
                if !imp.open.remove(&folder) {
                    imp.open.insert(folder);
                }
            }
        }
        Message::ImportOthers(folder) => {
            if let Some(Dialog::Import(imp)) = state.dialog.as_mut() {
                let a = match &imp.albums {
                    Some(Ok(list)) => list.iter().find(|a| a.folder == folder).cloned(),
                    _ => None,
                };
                let Some(a) = a else {
                    return Task::none();
                };
                imp.others = Some(Others {
                    folder,
                    artist: a.artist,
                    album: a.album,
                    found: None,
                    searching: false,
                    picking: false,
                    seq: 0,
                });
            }
            return others_search(state, ctx);
        }
        Message::OthersArtist(s) => {
            if let Some(o) = others_mut(state) {
                o.artist = s;
            }
        }
        Message::OthersAlbum(s) => {
            if let Some(o) = others_mut(state) {
                o.album = s;
            }
        }
        Message::OthersSearch => return others_search(state, ctx),
        Message::OthersFound(seq, r) => {
            if let Some(Dialog::Import(imp)) = state.dialog.as_mut() {
                if let Some(o) = imp.others.as_mut().filter(|o| o.seq == seq) {
                    o.searching = false;
                    match r {
                        Ok(v) => o.found = Some(releases_of(&v)),
                        Err(e) => {
                            o.found = Some(Vec::new());
                            ctx.toast_err(if e.message.is_empty() { "The search didn't work".to_string() } else { e.message });
                        }
                    }
                }
            }
        }
        Message::OthersPick(i) => {
            let root = state.root.clone().unwrap_or_default();
            let Some(Dialog::Import(imp)) = state.dialog.as_mut() else {
                return Task::none();
            };
            let mode = if imp.keep { "in_place" } else { "move" };
            let Some(o) = imp.others.as_mut() else {
                return Task::none();
            };
            let Some(rel) = o.found.as_ref().and_then(|f| f.get(i)).cloned() else {
                return Task::none();
            };
            o.picking = true;
            let folder = o.folder.clone();
            let body = json!({ "root": root, "path": folder, "mode": mode, "release_id": rel.id });
            let api = ctx.api.clone();
            return Task::perform(async move { api.post("/files/import/preview", body).await }, move |r| {
                Message::OthersPicked(folder, Box::new(rel), r)
            });
        }
        Message::OthersPicked(folder, rel, r) => {
            let Some(Dialog::Import(imp)) = state.dialog.as_mut() else {
                return Task::none();
            };
            match r {
                Ok(v) => {
                    if let Some(Ok(list)) = imp.albums.as_mut() {
                        if let Some(a) = list.iter_mut().find(|a| a.folder == folder) {
                            match albums_of(&v).into_iter().next() {
                                Some(one) => *a = one,
                                None => {
                                    a.confidence = rel.score;
                                    a.release = Some(*rel);
                                }
                            }
                        }
                    }
                    imp.skip.remove(&folder);
                    imp.others = None;
                }
                Err(e) => {
                    if let Some(o) = imp.others.as_mut() {
                        o.picking = false;
                    }
                    ctx.toast_err(if e.message.is_empty() { "Couldn't use that release".to_string() } else { e.message });
                }
            }
        }
        Message::OthersClose => {
            if let Some(Dialog::Import(imp)) = state.dialog.as_mut() {
                imp.others = None;
            }
        }
        Message::ImportApplied(r) => {
            match r {
                Ok(v) => {
                    let n = fmt::i(&v, "imported");
                    let mut line = fmt::s(&v, "message");
                    if line.is_empty() {
                        line = if n == 1 { "Imported 1 song".into() } else { format!("Imported {n} songs") };
                    }
                    if fmt::items(&v["failed"]).is_empty() {
                        ctx.toast_ok(line);
                    } else {
                        ctx.toast_warn(line);
                    }
                    state.dialog = None;
                    let roots: Vec<String> = state.root.iter().cloned().chain(["library".to_string()]).collect();
                    return after_write(state, ctx, &roots);
                }
                Err(e) => {
                    if let Some(Dialog::Import(imp)) = state.dialog.as_mut() {
                        imp.applying = false;
                    }
                    ctx.toast_err(if e.message.is_empty() { "The import didn't work".to_string() } else { e.message });
                }
            }
        }
        Message::TrashLoaded(r) => {
            state.trash = Some(
                r.map(|v| fmt::items(&v).iter().filter_map(TrashItem::from).collect())
                    .map_err(|e| if e.message.is_empty() { "Couldn't open the trash".to_string() } else { e.message }),
            );
        }
        Message::TrashMenu(i) => state.menu = Some((ctx.cursor, Menu::Trash(i))),
        Message::Restore(id) => {
            state.menu = None;
            return post(ctx, "/files/restore", json!({ "items": [id] }), Act::Restore);
        }
        Message::AskDeleteForever(id) => {
            state.menu = None;
            let name = match &state.trash {
                Some(Ok(items)) => items.iter().find(|t| t.id == id).map(|t| t.name.clone()).unwrap_or_default(),
                _ => String::new(),
            };
            state.dialog = Some(Dialog::DeleteForever { id, name });
        }
        Message::AskEmptyTrash => {
            state.menu = None;
            state.dialog = Some(Dialog::EmptyTrash);
        }
    }
    Task::none()
}

impl State {
    /// The selected names in the order the folder shows them.
    fn selected_names(&self) -> Vec<String> {
        self.entries()
            .iter()
            .filter(|e| self.selected.contains(&e.name))
            .map(|e| e.name.clone())
            .collect()
    }
}

fn others_mut(state: &mut State) -> Option<&mut Others> {
    match state.dialog.as_mut() {
        Some(Dialog::Import(imp)) => imp.others.as_mut(),
        _ => None,
    }
}

/// A key pressed while no text box had it.
fn key(state: &mut State, k: KeyAct, ctx: &mut Ctx) -> Task<Message> {
    match state.root.as_deref() {
        None => return Task::none(),
        // the trash: Backspace goes back to the roots
        Some("trash") => {
            return if k == KeyAct::Up { open_root(state, String::new(), ctx) } else { Task::none() };
        }
        Some(_) => {}
    }
    let n = state.entries().len();
    match k {
        KeyAct::Open => {
            let sel = state.selected_indices();
            if let (1, Some(i)) = (sel.len(), sel.first().copied()) {
                return open_entry(state, i, ctx);
            }
        }
        KeyAct::Up => return update(state, Message::Up, ctx),
        KeyAct::Trash => {
            if state.can_write() && !state.selected.is_empty() {
                state.dialog = Some(Dialog::Trash { names: state.selected_names() });
            }
        }
        KeyAct::Rename => {
            let names = state.selected_names();
            if let ([one], true) = (names.as_slice(), state.can_write()) {
                return update(state, Message::AskRename(one.clone()), ctx);
            }
        }
        KeyAct::Jump(c) => {
            let names: Vec<&str> = state.entries().iter().map(|e| e.name.as_str()).collect();
            if let Some(i) = jump_to(&names, state.cursor, c) {
                state.select_indices(&BTreeSet::from([i]));
                state.anchor = Some(i);
                state.cursor = Some(i);
            }
        }
        KeyAct::All => state.selected = state.entries().iter().map(|e| e.name.clone()).collect(),
        KeyAct::Clear if state.selected.is_empty() && !state.find.is_empty() => {
            // Escape with nothing picked: the search's filter goes
            state.clear_find();
            state.refilter();
        }
        KeyAct::Clear => {
            state.selected.clear();
            state.anchor = None;
        }
        KeyAct::Find => return operation::focus(FIND_ID).chain(operation::select_all(FIND_ID)),
        KeyAct::Step(d, shift) => {
            if n == 0 {
                return Task::none();
            }
            let next = match state.cursor {
                None if d > 0 => 0,
                None => n - 1,
                Some(c) => (c as i64 + d as i64).clamp(0, n as i64 - 1) as usize,
            };
            if shift && state.anchor.is_some() {
                let (set, anchor) = click_select(&BTreeSet::new(), state.anchor, next, true, false);
                state.select_indices(&set);
                state.anchor = anchor;
            } else {
                state.select_indices(&BTreeSet::from([next]));
                state.anchor = Some(next);
            }
            state.cursor = Some(next);
        }
    }
    Task::none()
}

/// One upload at a time: files picked while one runs follow the ones already going.
fn start_upload(state: &mut State, jobs: Vec<Job>, ctx: &mut Ctx) -> Task<Message> {
    if jobs.is_empty() {
        return Task::none();
    }
    let size: u64 = jobs.iter().map(|j| j.item.size).sum();
    if let Some(u) = state.upload.as_mut() {
        u.total += size;
        u.jobs.extend(jobs);
        return Task::none();
    }
    state.upload = Some(Upload {
        jobs,
        index: 0,
        ok: 0,
        failed: 0,
        first_error: String::new(),
        sent_before: 0,
        total: size,
        sent: Arc::new(AtomicU64::new(0)),
        cancel: Arc::new(AtomicBool::new(false)),
    });
    upload_next(state, ctx)
}

/// The open dialog's main button.
fn submit(state: &mut State, ctx: &mut Ctx) -> Task<Message> {
    let root = state.root.clone().unwrap_or_default();
    let path = state.path.clone();
    let Some(d) = state.dialog.take() else {
        return Task::none();
    };
    match d {
        Dialog::NewFolder { name } => {
            if name_problem(&name).is_some() {
                state.dialog = Some(Dialog::NewFolder { name });
                return Task::none();
            }
            post(ctx, "/files/mkdir", json!({ "root": root, "path": path, "name": name.trim() }), Act::Mkdir)
        }
        Dialog::Rename { from, name } => {
            if name_problem(&name).is_some() {
                state.dialog = Some(Dialog::Rename { from, name });
                return Task::none();
            }
            if name.trim() == from {
                return Task::none();
            }
            post(
                ctx,
                "/files/rename",
                json!({ "root": root, "path": join(&path, &from), "name": name.trim() }),
                Act::Rename,
            )
        }
        Dialog::Move { names, root: to_root, path: to_path, dirs, seq } => {
            if !may_move_to(&root, &path, &names, &to_root, &to_path) {
                state.dialog = Some(Dialog::Move { names, root: to_root, path: to_path, dirs, seq });
                return Task::none();
            }
            let from: Vec<String> = names.iter().map(|n| join(&path, n)).collect();
            let body = json!({ "root": root, "from": from, "to_root": to_root, "to_path": to_path });
            post(ctx, "/files/move", body, Act::Move { to_root })
        }
        Dialog::Trash { names } => {
            let paths: Vec<String> = names.iter().map(|n| join(&path, n)).collect();
            post(ctx, "/files/trash", json!({ "root": root, "paths": paths }), Act::Trash)
        }
        Dialog::Tags { name, original, values, saving } => {
            let changed: Option<serde_json::Map<String, Value>> = match &original {
                Some(Ok(orig)) if !saving => Some(
                    TAGS.iter()
                        .enumerate()
                        .filter(|(i, _)| values.get(*i) != orig.get(*i))
                        .map(|(i, (k, _))| (k.to_string(), Value::String(values[i].clone())))
                        .collect(),
                ),
                _ => None,
            };
            let Some(changed) = changed else {
                state.dialog = Some(Dialog::Tags { name, original, values, saving });
                return Task::none();
            };
            if changed.is_empty() {
                return Task::none();
            }
            let body = json!({ "root": root, "path": join(&path, &name), "tags": changed });
            state.dialog = Some(Dialog::Tags { name, original, values, saving: true });
            let api = ctx.api.clone();
            Task::perform(async move { api.put("/files/tags", body).await }, |r| Message::Done(Act::Tags, r))
        }
        Dialog::Upload { items, conflict } => {
            let jobs: Vec<Job> = items
                .into_iter()
                .map(|item| Job {
                    item,
                    root: root.clone(),
                    path: path.clone(),
                    conflict: conflict.clone(),
                })
                .collect();
            start_upload(state, jobs, ctx)
        }
        Dialog::Import(mut imp) => {
            let albums: Vec<Value> = match &imp.albums {
                Some(Ok(list)) if !imp.applying => list
                    .iter()
                    .filter(|a| !imp.skip.contains(&a.folder))
                    .map(|a| {
                        let release = match (&a.release, imp.keep) {
                            (Some(r), false) => Value::String(r.id.clone()),
                            _ => Value::Null,
                        };
                        json!({ "folder": a.folder, "release_id": release })
                    })
                    .collect(),
                _ => Vec::new(),
            };
            if albums.is_empty() {
                state.dialog = Some(Dialog::Import(imp));
                return Task::none();
            }
            let body = json!({
                "root": root,
                "path": imp.folder,
                "mode": if imp.keep { "in_place" } else { "move" },
                "albums": albums,
            });
            imp.applying = true;
            state.dialog = Some(Dialog::Import(imp));
            let api = ctx.api.clone();
            Task::perform(async move { api.post("/files/import/apply", body).await }, Message::ImportApplied)
        }
        Dialog::DeleteForever { id, .. } => {
            let api = ctx.api.clone();
            Task::perform(
                async move { api.del(&format!("/files/trash/{}", urlencode(&id))).await },
                |r| Message::Done(Act::Delete, r),
            )
        }
        Dialog::EmptyTrash => post(ctx, "/files/empty-trash", json!({}), Act::Empty),
    }
}

/// A move may go there: another folder than the one the items are in, and not into one of them.
pub fn may_move_to(root: &str, path: &str, names: &[String], to_root: &str, to_path: &str) -> bool {
    if to_root != root {
        return true;
    }
    to_path != path && !names.iter().any(|n| under(&join(path, n), to_path))
}

/// The answer of a write: say how it went, show the change.
fn done(state: &mut State, act: Act, r: Result<Value, ApiError>, ctx: &mut Ctx) -> Task<Message> {
    let here = state.root.clone().unwrap_or_default();
    let v = match r {
        Ok(v) => v,
        Err(e) => {
            let fallback = if matches!(act, Act::Tags) { "Couldn't write the tags" } else { "That didn't work" };
            ctx.toast_err(if e.message.is_empty() { fallback.to_string() } else { e.message });
            if let (Act::Tags, Some(Dialog::Tags { saving, .. })) = (&act, state.dialog.as_mut()) {
                *saving = false;
                return Task::none();
            }
            return match act {
                Act::Restore | Act::Delete | Act::Empty => load(state, ctx, false),
                _ => after_write(state, ctx, &[here]),
            };
        }
    };
    let failed = failures(&v);
    let say = |ctx: &mut Ctx, ok: String| {
        if failed.is_empty() {
            ctx.toast_ok(ok);
        } else {
            ctx.toast_warn(failed.clone());
        }
    };
    match act {
        Act::Mkdir => {
            ctx.toast_ok("Folder made");
            after_write(state, ctx, &[here])
        }
        Act::Rename => {
            ctx.toast_ok("Renamed");
            after_write(state, ctx, &[here])
        }
        Act::Move { to_root } => {
            let n = fmt::i(&v, "moved");
            say(ctx, if n == 1 { "Moved".to_string() } else { format!("Moved {n} items") });
            after_write(state, ctx, &[here, to_root])
        }
        Act::Trash => {
            let n = fmt::i(&v, "trashed");
            say(ctx, if n == 1 { "Moved to trash".to_string() } else { format!("Moved {n} items to trash") });
            after_write(state, ctx, &[here])
        }
        Act::Tags => {
            ctx.toast_ok("Tags saved");
            state.dialog = None;
            after_write(state, ctx, &[])
        }
        Act::Restore | Act::Delete | Act::Empty => {
            match act {
                Act::Restore => say(ctx, "Restored".to_string()),
                Act::Delete => ctx.toast_ok("Deleted"),
                _ => ctx.toast_ok("The trash is empty"),
            }
            // what came back is in its folder again: the tree asks again too
            let roots: Vec<String> = state.tree_open.iter().map(|(r, _)| r.clone()).collect::<BTreeSet<_>>().into_iter().collect();
            let mut tasks = vec![load(state, ctx, false), load_roots(ctx)];
            for r in roots {
                tasks.push(tree_reload(state, &r, ctx));
            }
            Task::batch(tasks)
        }
    }
}

// ----- the view --------------------------------------------------------------------------------

/// The width the section has: the page's, less the settings nav.
fn body_width(ctx: &Ctx) -> f32 {
    let w = ctx.size.width;
    1180f32.max(w * 0.68).min(w - theme::SIDEBAR - 64.0) - 224.0
}

pub fn view<'a>(state: &'a State, ctx: &Ctx<'a>) -> El<'a, Message> {
    let mut col = column![widgets::h2("Files"), small_muted(INTRO)].spacing(8).width(Length::Fill);
    if let Some(u) = &state.upload {
        col = col.push(upload_panel(u));
    }
    let roots = match &state.roots {
        None => return col.push(skeleton_rows(6)).into(),
        Some(Err(e)) => {
            return col
                .push(error_line(e))
                .into()
        }
        Some(Ok(r)) => r,
    };
    let main: El<'a, Message> = match state.current_root() {
        None => roots_list(roots),
        Some(r) if r.is_trash() => trash_view(state, roots),
        Some(r) => folder_view(state, r, ctx),
    };
    let body: El<'a, Message> = if body_width(ctx) >= 860.0 {
        row![tree_view(state, roots), container(main).width(Length::Fill)]
            .spacing(16)
            .align_y(Alignment::Start)
            .into()
    } else {
        main
    };
    col.push(space().height(6)).push(body).into()
}

/// An error in full, with Copy and Retry.
fn error_line<'a>(e: &'a str) -> El<'a, Message> {
    column![
        row![
            container(error_box(e)).width(Length::Fill),
            widgets::details::copy_btn(Message::Copy(e.to_string())),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
        btn_sm("Retry", Btn::Default, Some(Message::Refresh)),
    ]
    .spacing(10)
    .into()
}

fn tip<'a>(e: impl Into<El<'a, Message>>, words: &'a str) -> El<'a, Message> {
    iced::widget::tooltip(e, widgets::tip_box(words), iced::widget::tooltip::Position::Bottom)
        .gap(4)
        .into()
}

/// `.btn` with one of this page's icons.
fn btn_f<'a>(i: F, label: &'a str, on: Option<Message>) -> El<'a, Message> {
    let p = theme::look();
    button(
        row![ficon(i, 16.0, p.text), t(label).size(13).font(theme::semibold())]
            .spacing(8)
            .align_y(Alignment::Center),
    )
    .padding([7, 12])
    .style(theme::btn(Btn::Default))
    .on_press_maybe(on)
    .into()
}

fn up_btn<'a>(on: Message) -> El<'a, Message> {
    let p = theme::look();
    tip(
        button(ficon(F::Up, 16.0, p.text)).padding(8).style(theme::btn(Btn::Icon)).on_press(on),
        "Up one folder",
    )
}

fn roots_list<'a>(roots: &'a [Root]) -> El<'a, Message> {
    let p = theme::look();
    let rows: Vec<El<'a, Message>> = roots
        .iter()
        .map(|r| {
            let ic = if r.is_trash() { icon(Icon::Trash, 20.0, p.muted) } else { ficon(F::Folder, 20.0, p.accent) };
            let mut right: Vec<El<'a, Message>> = Vec::new();
            if !r.writable && !r.is_trash() {
                right.push(badge("Read-only", Badge::Plain));
            }
            if r.free > 0.0 {
                right.push(small_muted(format!("{} free", fmt::size(r.free))).into());
            }
            list_row(Some(ic), r.name.clone(), r.path.clone(), right, Some(Message::OpenRoot(r.id.clone())))
        })
        .collect();
    card_flat(column(rows)).width(Length::Fill).into()
}

fn crumbs<'a>(root: &'a Root, path: &'a str) -> El<'a, Message> {
    let p = theme::look();
    let parts: Vec<&str> = if path.is_empty() { Vec::new() } else { path.split('/').collect() };
    let crumb = |label: String, to: Option<Message>, last: bool| -> El<'a, Message> {
        button(t(label).size(14).font(theme::semibold()).color(if last { p.text } else { p.accent }))
            .padding([4, 6])
            .style(theme::btn(Btn::Ghost))
            .on_press_maybe(to)
            .into()
    };
    let mut r = row![crumb(root.name.clone(), (!parts.is_empty()).then(|| Message::Go(String::new())), parts.is_empty())]
        .spacing(2)
        .align_y(Alignment::Center);
    for (i, part) in parts.iter().enumerate() {
        let last = i + 1 == parts.len();
        r = r.push(t("›").color(p.faint));
        r = r.push(crumb(part.to_string(), (!last).then(|| Message::Go(parts[..=i].join("/"))), last));
    }
    r.wrap().into()
}

fn folder_view<'a>(state: &'a State, root: &'a Root, ctx: &Ctx<'a>) -> El<'a, Message> {
    let p = ctx.look;
    let via = state.via_link();
    let can_write = state.can_write();
    let mut head = row![up_btn(Message::Up), container(crumbs(root, &state.path)).width(Length::Fill)]
        .spacing(6)
        .align_y(Alignment::Center);
    if !root.writable {
        head = head.push(badge("Read-only", Badge::Plain));
    }
    if root.free > 0.0 {
        head = head.push(small_muted(format!("{} free", fmt::size(root.free))));
    }
    let mut col = column![head].spacing(10).width(Length::Fill);
    // a folder of this root by its path; the server refuses a path that leaves the root
    col = col.push(
        text_input("Path", &state.path_box)
            .id(PATH_ID)
            .on_input(Message::PathInput)
            .on_submit(Message::PathSubmit)
            .padding([7, 10])
            .size(13)
            .style(theme::input),
    );
    // the search: filters this folder as you type, and searches the whole root by name
    let mut find = row![text_input(&format!("Search {} by name", root.name), &state.find)
        .id(FIND_ID)
        .on_input(Message::FindInput)
        .on_submit(Message::FindSubmit)
        .padding([7, 10])
        .size(13)
        .style(theme::input)]
    .spacing(8)
    .align_y(Alignment::Center);
    if !state.find.is_empty() {
        find = find.push(btn_sm("Clear", Btn::Ghost, Some(Message::FindClear)));
    }
    col = col.push(find);
    if via {
        col = col.push(small_muted(LINK_NOTE));
    }
    col = col.push(toolbar(state, can_write, via));
    if state.hover && can_write {
        col = col.push(drop_banner(root, &state.path));
    }
    match &state.listing {
        None => col = col.push(skeleton_rows(8)),
        Some(Err(e)) => {
            col = col.push(error_line(e));
        }
        Some(Ok(l)) if state.entries().is_empty() && !l.entries.is_empty() => {
            col = col.push(
                container(
                    column![
                        icon(Icon::Search, 30.0, p.faint),
                        t(format!("Nothing in this folder matches “{}”", state.find.trim())).size(15).font(theme::semibold()),
                        widgets::muted("The search below looks in every folder.").size(13),
                    ]
                    .spacing(10)
                    .align_x(Alignment::Center),
                )
                .width(Length::Fill)
                .padding([30, 20])
                .center_x(Length::Fill),
            );
        }
        Some(Ok(l)) if l.entries.is_empty() => {
            let mut c = column![ficon(F::Folder, 34.0, p.faint), t("This folder is empty").size(15).font(theme::semibold())]
                .spacing(10)
                .align_x(Alignment::Center);
            if can_write {
                c = c.push(widgets::muted("Upload files here, or make a new folder.").size(13));
            }
            col = col.push(container(c).width(Length::Fill).padding([44, 20]).center_x(Length::Fill));
        }
        Some(Ok(l)) => {
            col = col.push(list_head(state));
            let mut rows = column![].spacing(1);
            for (i, e) in state.entries().iter().enumerate() {
                rows = rows.push(entry_row(i, e, state.selected.contains(&e.name)));
            }
            col = col.push(rows);
            if l.entries.len() < l.total {
                col = col.push(
                    container(btn_sm(
                        format!("Show more ({})", l.total - l.entries.len()),
                        Btn::Default,
                        (!state.loading_more).then_some(Message::More),
                    ))
                    .center_x(Length::Fill),
                );
            }
        }
    }
    if let Some(found) = found_view(state, root) {
        col = col.push(space().height(6)).push(found);
    }
    col.into()
}

/// The search's results across the root: a heading with the count, then one row per result with
/// the path of its folder; a click opens it. None while the box is blank.
fn found_view<'a>(state: &'a State, root: &'a Root) -> Option<El<'a, Message>> {
    let p = theme::look();
    if find_words(&state.find).is_empty() {
        return None;
    }
    let head = |words: String| -> El<'a, Message> {
        row![icon(Icon::Search, 16.0, p.muted), t(words).size(14).font(theme::semibold())]
            .spacing(8)
            .align_y(Alignment::Center)
            .into()
    };
    Some(match &state.hits {
        _ if state.finding => head(format!("Searching {}…", root.name)),
        None => return None,
        Some(Err(e)) => error_line(e),
        Some(Ok(h)) => {
            let mut c = column![head(found_line(h.hits.len(), h.truncated, &root.name))].spacing(8);
            if !h.hits.is_empty() {
                let rows: Vec<El<'a, Message>> = h
                    .hits
                    .iter()
                    .enumerate()
                    .map(|(i, hit)| {
                        let e = &hit.entry;
                        let ic: El<'a, Message> = if e.is_link() {
                            icon(Icon::Link, 20.0, p.muted)
                        } else if e.kind == "dir" {
                            ficon(F::Folder, 20.0, p.accent)
                        } else if e.is_audio() {
                            icon(Icon::Music, 20.0, p.muted)
                        } else if e.image {
                            ficon(F::Image, 20.0, p.muted)
                        } else {
                            ficon(F::File, 20.0, p.muted)
                        };
                        let size: Vec<El<'a, Message>> =
                            if e.is_dir() { Vec::new() } else { vec![small_muted(fmt::size(e.size)).into()] };
                        list_row(Some(ic), e.name.clone(), place(&root.name, &hit.parent), size, Some(Message::FindGo(i)))
                    })
                    .collect();
                c = c.push(card_flat(column(rows)).width(Length::Fill));
            }
            c.into()
        }
    })
}

fn toolbar<'a>(state: &'a State, can_write: bool, via: bool) -> El<'a, Message> {
    let picked = state.selected.len();
    let mut r = row![].spacing(8).align_y(Alignment::Center);
    if picked == 0 {
        r = r.push(btn_sm(sort_label(&state.sort, &state.dir), Btn::Default, Some(Message::SortMenu)));
        if can_write {
            r = r.push(btn_f(F::FolderPlus, "New folder", Some(Message::AskNewFolder)));
            r = r.push(btn_f(F::Upload, "Upload", Some(Message::UploadMenu)));
        }
        if !via && !matches!(state.listing, Some(Err(_))) {
            r = r.push(btn_icon(Icon::Download, "Import…", Btn::Default, Some(Message::AskImport(state.path.clone()))));
        }
        r = r.push(space().width(Length::Fill));
        r = r.push(icon_btn(Icon::Refresh, "Refresh", Some(Message::Refresh)));
    } else {
        r = r.push(t(format!("{picked} selected")).size(14).font(theme::semibold()));
        if can_write {
            let names = state.selected_names();
            if let [one] = names.as_slice() {
                r = r.push(btn_icon(Icon::Edit, "Rename", Btn::Default, Some(Message::AskRename(one.clone()))));
            }
            r = r.push(btn_sm("Move", Btn::Default, Some(Message::AskMove(names.clone()))));
            r = r.push(btn_icon(Icon::Trash, "Move to trash", Btn::Danger, Some(Message::AskTrash(names))));
        }
        r = r.push(space().width(Length::Fill));
        r = r.push(btn_sm("Select all", Btn::Ghost, Some(Message::SelectAll)));
        r = r.push(btn_sm("Cancel", Btn::Ghost, Some(Message::ClearSelection)));
    }
    r.width(Length::Fill).into()
}

fn drop_banner<'a>(root: &'a Root, path: &'a str) -> El<'a, Message> {
    let p = theme::look();
    container(
        row![
            ficon(F::Upload, 18.0, p.accent),
            column![t("Upload files").size(14).font(theme::semibold()), small_muted(place(&root.name, path))].spacing(2)
        ]
        .spacing(12)
        .align_y(Alignment::Center),
    )
    .padding([14, 16])
    .width(Length::Fill)
    .style(move |_| container::Style {
        background: Some(Background::Color(p.accent.scale_alpha(0.08))),
        border: Border {
            color: p.accent,
            width: 2.0,
            radius: 10.0.into(),
        },
        ..container::Style::default()
    })
    .into()
}

const SIZE_W: f32 = 90.0;
const DATE_W: f32 = 120.0;

fn list_head<'a>(state: &'a State) -> El<'a, Message> {
    let p = theme::look();
    let head = |key: &'static str, words: &'static str| -> El<'a, Message> {
        let on = state.sort == key;
        let label = if on { format!("{words} {}", if state.dir == "desc" { "↓" } else { "↑" }) } else { words.to_string() };
        button(t(label).size(12).font(theme::semibold()).color(if on { p.text } else { p.muted }))
            .padding([2, 4])
            .style(theme::btn(Btn::Ghost))
            .on_press(Message::Sort(key.to_string()))
            .into()
    };
    row![
        space().width(20),
        container(head("name", "Name")).width(Length::Fill),
        container(head("size", "Size")).width(SIZE_W).align_x(alignment::Horizontal::Right),
        container(head("mtime", "Date modified")).width(DATE_W),
        space().width(32),
    ]
    .spacing(12)
    .padding([0, 10])
    .align_y(Alignment::Center)
    .into()
}

fn entry_row<'a>(i: usize, e: &'a Entry, selected: bool) -> El<'a, Message> {
    let p = theme::look();
    let ic: El<'a, Message> = if e.is_link() {
        icon(Icon::Link, 20.0, p.muted)
    } else if e.kind == "dir" {
        ficon(F::Folder, 20.0, p.accent)
    } else if e.is_audio() {
        icon(Icon::Music, 20.0, p.muted)
    } else if e.image {
        ficon(F::Image, 20.0, p.muted)
    } else {
        ficon(F::File, 20.0, p.muted)
    };
    let name = t(e.name.as_str())
        .size(14)
        .font(if e.is_dir() { theme::semibold() } else { iced::Font::DEFAULT })
        .wrapping(Wrapping::None);
    let mut lines = column![container(name).width(Length::Fill).clip(true)].spacing(2).width(Length::Fill);
    let line = e.line();
    if !line.is_empty() {
        lines = lines.push(container(small_muted(line).wrapping(Wrapping::None)).width(Length::Fill).clip(true));
    }
    let size = if e.kind == "dir" || e.is_link() { String::new() } else { fmt::size(e.size) };
    let date = if e.mtime > 0.0 { fmt::date(e.mtime) } else { String::new() };
    let r = row![
        ic,
        lines,
        container(small_muted(size)).width(SIZE_W).align_x(alignment::Horizontal::Right),
        container(small_muted(date)).width(DATE_W),
        icon_btn(Icon::More, "", Some(Message::RowMenu(i))),
    ]
    .spacing(12)
    .padding([4, 10])
    .align_y(Alignment::Center);
    let boxed = container(r).width(Length::Fill).style(move |_| container::Style {
        background: selected.then(|| Background::Color(p.accent.scale_alpha(0.14))),
        border: Border {
            radius: 8.0.into(),
            ..Border::default()
        },
        ..container::Style::default()
    });
    mouse_area(boxed)
        .on_press(Message::Click(i))
        .on_double_click(Message::Open(i))
        .on_right_press(Message::RowMenu(i))
        .interaction(mouse::Interaction::Pointer)
        .into()
}

fn trash_view<'a>(state: &'a State, roots: &'a [Root]) -> El<'a, Message> {
    let p = theme::look();
    let head = row![up_btn(Message::OpenRoot(String::new())), t("Trash").size(14).font(theme::semibold())]
        .spacing(6)
        .align_y(Alignment::Center);
    let body: El<'a, Message> = match &state.trash {
        None => skeleton_rows(5),
        Some(Err(e)) => error_line(e),
        Some(Ok(items)) if items.is_empty() => container(
            column![
                icon(Icon::Trash, 34.0, p.faint),
                t("The trash is empty").size(15).font(theme::semibold()),
                widgets::muted("Files you move to trash wait here until you delete them forever.").size(13),
            ]
            .spacing(10)
            .align_x(Alignment::Center),
        )
        .width(Length::Fill)
        .padding([44, 20])
        .center_x(Length::Fill)
        .into(),
        Some(Ok(items)) => {
            let rows: Vec<El<'a, Message>> = items.iter().enumerate().map(|(i, it)| trash_row(i, it, roots)).collect();
            column![
                row![
                    small_muted(format!("{} in the trash", items.len())),
                    space().width(Length::Fill),
                    btn_icon(Icon::Trash, "Empty trash", Btn::Danger, Some(Message::AskEmptyTrash)),
                ]
                .align_y(Alignment::Center),
                card_flat(column(rows)).width(Length::Fill),
            ]
            .spacing(10)
            .into()
        }
    };
    column![head, body].spacing(12).width(Length::Fill).into()
}

fn trash_row<'a>(i: usize, it: &'a TrashItem, roots: &'a [Root]) -> El<'a, Message> {
    let p = theme::look();
    let ic = ficon(if it.kind == "dir" { F::Folder } else { F::File }, 20.0, p.muted);
    let r = row![
        ic,
        column![
            container(t(it.name.as_str()).size(14).wrapping(Wrapping::None)).width(Length::Fill).clip(true),
            small_muted(it.line(roots)),
        ]
        .spacing(2)
        .width(Length::Fill),
        btn_sm("Restore", Btn::Default, Some(Message::Restore(it.id.clone()))),
        icon_btn(Icon::More, "", Some(Message::TrashMenu(i))),
    ]
    .spacing(12)
    .padding([10, 14])
    .align_y(Alignment::Center);
    column![
        mouse_area(container(r).width(Length::Fill)).on_right_press(Message::TrashMenu(i)),
        widgets::divider()
    ]
    .into()
}

/// What a tree node is.
#[derive(Clone, Copy, PartialEq)]
enum Node {
    Root,
    Trash,
    Dir,
    Link,
}

fn tree_view<'a>(state: &'a State, roots: &'a [Root]) -> El<'a, Message> {
    let mut rows: Vec<El<'a, Message>> = Vec::new();
    for r in roots {
        let open = !r.is_trash() && state.tree_open.contains(&(r.id.clone(), String::new()));
        let here = state.root.as_deref() == Some(r.id.as_str()) && (state.path.is_empty() || r.is_trash());
        let kind = if r.is_trash() { Node::Trash } else { Node::Root };
        rows.push(tree_row(&r.id, "", &r.name, 0, kind, open, here, None));
        if open {
            tree_children(state, &r.id, "", 1, None, &mut rows);
        }
    }
    container(column(rows).spacing(1))
        .width(TREE_W)
        .padding(6)
        .style(theme::card)
        .into()
}

#[allow(clippy::too_many_arguments)]
fn tree_row<'a>(
    root: &str,
    path: &str,
    name: &'a str,
    depth: usize,
    kind: Node,
    open: bool,
    here: bool,
    link_at: Option<String>,
) -> El<'a, Message> {
    let p = theme::look();
    let toggle: El<'a, Message> = if kind == Node::Trash || depth >= TREE_DEPTH {
        space().width(20).into()
    } else {
        let chevron: El<'a, Message> = if open { icon(Icon::Chevron, 14.0, p.muted) } else { ficon(F::Right, 14.0, p.muted) };
        button(chevron)
            .padding(3)
            .style(theme::btn(Btn::Icon))
            .on_press(Message::TreeToggle(root.to_string(), path.to_string()))
            .into()
    };
    let ic: El<'a, Message> = match kind {
        Node::Trash => icon(Icon::Trash, 15.0, p.muted),
        Node::Link => icon(Icon::Link, 15.0, p.muted),
        Node::Root | Node::Dir => ficon(F::Folder, 15.0, p.accent),
    };
    let go = if kind == Node::Trash {
        Message::OpenRoot(root.to_string())
    } else {
        Message::TreeGo(root.to_string(), path.to_string(), link_at)
    };
    let label = button(
        row![ic, container(t(name).size(13).wrapping(Wrapping::None)).width(Length::Fill).clip(true)]
            .spacing(6)
            .align_y(Alignment::Center),
    )
    .padding([4, 6])
    .width(Length::Fill)
    .style(theme::btn(if here { Btn::NavOn } else { Btn::Nav }))
    .on_press(go);
    row![space().width(depth as f32 * 12.0), toggle, label]
        .spacing(2)
        .align_y(Alignment::Center)
        .into()
}

fn tree_children<'a>(
    state: &'a State,
    root: &'a str,
    path: &str,
    depth: usize,
    link_at: Option<String>,
    out: &mut Vec<El<'a, Message>>,
) {
    let indent = |e: El<'a, Message>| -> El<'a, Message> { row![space().width(depth as f32 * 12.0 + 26.0), e].into() };
    match state.tree.get(&(root.to_string(), path.to_string())) {
        None | Some(None) => out.push(indent(small_muted("Loading…").into())),
        Some(Some(Err(_))) => out.push(indent(small_muted("Couldn't read the folder").into())),
        Some(Some(Ok(dirs))) => {
            for (name, link) in dirs {
                let p = join(path, name);
                let at = link_at.clone().or_else(|| link.then(|| p.clone()));
                let open = depth < TREE_DEPTH && state.tree_open.contains(&(root.to_string(), p.clone()));
                let here = state.root.as_deref() == Some(root) && state.path == p;
                let kind = if *link { Node::Link } else { Node::Dir };
                out.push(tree_row(root, &p, name, depth, kind, open, here, at.clone()));
                if open {
                    tree_children(state, root, &p, depth + 1, at, out);
                }
            }
        }
    }
}

fn upload_panel<'a>(u: &'a Upload) -> El<'a, Message> {
    let sent = u.sent_before + u.sent.load(Ordering::Relaxed);
    let part = if u.total > 0 { (sent as f32 / u.total as f32).min(1.0) } else { 0.0 };
    let count = u.jobs.len();
    let name = u.jobs.get(u.index).map(|j| j.item.name.as_str()).unwrap_or("");
    container(
        column![
            row![
                column![
                    t(format!("Uploading {} of {count}", (u.index + 1).min(count))).size(14).font(theme::semibold()),
                    container(small_muted(name).wrapping(Wrapping::None)).width(Length::Fill).clip(true),
                ]
                .spacing(2)
                .width(Length::Fill),
                btn_sm("Stop", Btn::Default, Some(Message::StopUpload)),
            ]
            .spacing(12)
            .align_y(Alignment::Center),
            progress_bar(0.0..=1.0, part).girth(6).style(theme::progress),
        ]
        .spacing(8),
    )
    .padding([12, 14])
    .width(Length::Fill)
    .style(theme::card)
    .into()
}

// ----- menus and dialogs -----------------------------------------------------------------------

fn menu_items(state: &State, menu: &Menu) -> Vec<MenuItem<Message>> {
    match menu {
        Menu::Sort => SORTS
            .iter()
            .map(|(k, w)| {
                // the chosen sort says its order; choosing it again turns the order round
                let label = if *k == state.sort { format!("{w} ({})", sort_order(k, &state.dir)) } else { w.to_string() };
                MenuItem::new(label, Message::Sort(k.to_string()))
            })
            .collect(),
        Menu::Upload => vec![
            MenuItem::new("Upload files", Message::PickFiles).icon(Icon::Plus),
            MenuItem::new("Upload a folder", Message::PickFolder).icon(Icon::Library),
        ],
        Menu::Trash(i) => match &state.trash {
            Some(Ok(items)) => match items.get(*i) {
                Some(it) => vec![
                    MenuItem::new("Restore", Message::Restore(it.id.clone())).icon(Icon::Refresh),
                    MenuItem::new("Delete forever", Message::AskDeleteForever(it.id.clone())).icon(Icon::Trash).danger(),
                ],
                None => Vec::new(),
            },
            _ => Vec::new(),
        },
        Menu::Entries(names) => {
            let can = state.can_write();
            let mut v = Vec::new();
            if let [one] = names.as_slice() {
                let Some((i, e)) = state.entries().iter().enumerate().find(|(_, e)| &e.name == one) else {
                    return v;
                };
                if e.is_dir() {
                    v.push(MenuItem::new("Open", Message::Open(i)).icon(Icon::External));
                }
                if e.track_id() > 0 {
                    v.push(MenuItem::new("Play", Message::Play(e.track_id())).icon(Icon::Play));
                }
                if e.is_audio() && e.kind == "file" && can {
                    v.push(MenuItem::new("Edit tags", Message::AskTags(e.name.clone())).icon(Icon::Tag));
                }
                if e.track_id() > 0 && !e.is_link() {
                    v.push(MenuItem::new("Edit song details…", Message::FixSong(e.track_id())).icon(Icon::Edit));
                }
                // nothing is imported through a link
                if e.is_dir() && !e.is_link() && !state.via_link() {
                    v.push(MenuItem::new("Import…", Message::AskImport(join(&state.path, &e.name))).icon(Icon::Download));
                }
                // on a link these act on the link itself, never on what it points to
                if can {
                    v.push(MenuItem::new("Rename", Message::AskRename(e.name.clone())).icon(Icon::Edit));
                    v.push(MenuItem::new("Move", Message::AskMove(vec![e.name.clone()])).icon(Icon::Next));
                    v.push(MenuItem::new("Move to trash", Message::AskTrash(vec![e.name.clone()])).icon(Icon::Trash).danger());
                }
            } else if can {
                v.push(MenuItem::new("Move", Message::AskMove(names.clone())).icon(Icon::Next));
                v.push(MenuItem::new("Move to trash", Message::AskTrash(names.clone())).icon(Icon::Trash).danger());
            }
            v
        }
    }
}

/// A modal with Cancel and one button (the confirms of this page, with titles that carry a name).
fn ask<'a>(title: String, text: &'a str, ok: &'a str, danger: bool) -> El<'a, Message> {
    let footer = row![
        btn("Cancel", Btn::Default, Some(Message::CloseDialog)),
        btn(ok, if danger { Btn::Danger } else { Btn::Primary }, Some(Message::Submit)),
    ]
    .spacing(10);
    widgets::modal(title, widgets::muted(text).size(14), Some(footer.into()), false, Message::CloseDialog)
}

fn footer<'a>(ok: impl iced::widget::text::IntoFragment<'a>, on: Option<Message>) -> El<'a, Message> {
    row![
        btn("Cancel", Btn::Default, Some(Message::CloseDialog)),
        btn(ok, Btn::Primary, on),
    ]
    .spacing(10)
    .into()
}

fn input<'a>(value: &'a str, on: impl Fn(String) -> Message + 'a) -> text_input::TextInput<'a, Message> {
    text_input("", value).on_input(on).padding([8, 10]).size(14).style(theme::input)
}

fn name_dialog<'a>(title: &'a str, name: &'a str, ok: &'a str) -> El<'a, Message> {
    let problem = name_problem(name);
    let mut body = column![text_input("Name", name)
        .id(NAME_ID)
        .on_input(Message::NameInput)
        .on_submit(Message::Submit)
        .padding([9, 12])
        .size(14)
        .style(theme::input)]
    .spacing(8);
    if let Some(w) = problem.filter(|w| !w.is_empty()) {
        body = body.push(t(w).size(12.5).color(theme::look().err));
    }
    widgets::modal(title, body, Some(footer(ok, problem.is_none().then_some(Message::Submit))), false, Message::CloseDialog)
}

fn folder_line<'a>(i: F, words: String, on: Message, color: Color) -> El<'a, Message> {
    button(
        row![ficon(i, 16.0, color), t(words).size(14)]
            .spacing(10)
            .align_y(Alignment::Center),
    )
    .padding([7, 10])
    .width(Length::Fill)
    .style(theme::btn(Btn::Row))
    .on_press(on)
    .into()
}

fn move_dialog<'a>(
    state: &'a State,
    names: &'a [String],
    to: &'a str,
    path: &'a str,
    dirs: &'a Option<Result<Vec<String>, String>>,
) -> El<'a, Message> {
    let p = theme::look();
    let title = match names {
        [one] => format!("Move “{one}”"),
        _ => format!("Move {} items", names.len()),
    };
    let mut chips = row![].spacing(6);
    for r in state.roots().iter().filter(|r| r.writable && !r.is_trash()) {
        chips = chips.push(btn_sm(
            r.name.as_str(),
            if r.id == to { Btn::TabOn } else { Btn::Tab },
            Some(Message::PickRoot(r.id.clone())),
        ));
    }
    let to_name = state.roots().iter().find(|r| r.id == to).map(|r| r.name.as_str()).unwrap_or(to);
    let mut list = column![].spacing(1);
    if !path.is_empty() {
        list = list.push(folder_line(F::Up, "Up one folder".into(), Message::PickGo(parent_of(path)), p.muted));
    }
    match dirs {
        None => list = list.push(small_muted("Loading…")),
        Some(Err(e)) => list = list.push(small_muted(e.as_str())),
        Some(Ok(d)) if d.is_empty() => list = list.push(small_muted("No folders here")),
        Some(Ok(d)) => {
            for n in d {
                list = list.push(folder_line(F::Folder, n.clone(), Message::PickGo(join(path, n)), p.accent));
            }
        }
    }
    let body = column![chips.wrap(), small_muted(place(to_name, path)), list].spacing(10);
    let here = state.root.as_deref().unwrap_or("");
    let ok = may_move_to(here, &state.path, names, to, path).then_some(Message::Submit);
    widgets::modal(title, body, Some(footer("Move here", ok)), false, Message::CloseDialog)
}

fn tags_dialog<'a>(
    name: &'a str,
    original: &'a Option<Result<Vec<String>, String>>,
    values: &'a [String],
    saving: bool,
) -> El<'a, Message> {
    let p = theme::look();
    let mut body = column![small_muted(name)].spacing(12);
    match original {
        None => body = body.push(small_muted("Loading…")),
        Some(Err(e)) => body = body.push(t(e.as_str()).size(13).color(p.err)),
        Some(Ok(_)) => {
            for pair in [[0, 1], [2, 3], [4, 5], [6, 7]] {
                let mut r = row![].spacing(14);
                for i in pair {
                    let value = values.get(i).map(String::as_str).unwrap_or("");
                    r = r.push(
                        container(field(TAGS[i].1, input(value, move |s| Message::TagInput(i, s)).on_submit(Message::Submit)))
                            .width(Length::Fill),
                    );
                }
                body = body.push(r);
            }
        }
    }
    let ready = matches!(original, Some(Ok(_))) && !saving;
    let ok = footer(if saving { "Saving…" } else { "Save" }, ready.then_some(Message::Submit));
    widgets::modal("Edit tags", body, Some(ok), false, Message::CloseDialog)
}

fn upload_dialog<'a>(items: &'a [UploadItem], conflict: &'a str, root: &'a str, path: &'a str) -> El<'a, Message> {
    let title = match items {
        [one] => format!("Upload “{}”?", base_name(&one.name)),
        _ => format!("Upload {} files?", items.len()),
    };
    let size: u64 = items.iter().map(|i| i.size).sum();
    let mut to = format!("To {}", place(root, path));
    if size > 0 {
        to.push_str(&format!(" · {}", fmt::size(size as f64)));
    }
    let mut body = column![
        small_muted(to),
        t("When a file with the same name is there").size(14).font(theme::semibold()),
    ]
    .spacing(10);
    for (k, words) in CONFLICTS {
        body = body.push(
            radio(words, k, Some(conflict), |v: &str| Message::Conflict(v.to_string()))
                .size(16)
                .text_size(14)
                .spacing(8),
        );
    }
    widgets::modal(title, body, Some(footer("Upload", Some(Message::Submit))), false, Message::CloseDialog)
}

fn album_card<'a>(a: &'a ImportAlbum, on: bool, imp: &'a Import) -> El<'a, Message> {
    let p = theme::look();
    let folder = a.folder.clone();
    let where_ = if a.folder.is_empty() { "This folder" } else { a.folder.as_str() };
    let head = row![
        checkbox(on).size(18).on_toggle(move |_| Message::ImportSkip(folder.clone())),
        column![
            t(a.title()).size(14).font(theme::semibold()),
            small_muted(format!("{where_} · {} files", a.files)),
        ]
        .spacing(2)
        .width(Length::Fill),
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    let mut c = column![head].spacing(8);
    if !imp.keep {
        let good = a.release.is_some() && a.confidence >= 0.8;
        c = c.push(t(a.match_line()).size(13).color(if good { p.text } else { p.warn }));
        let mut btns = row![btn_sm("Other matches", Btn::Default, Some(Message::ImportOthers(a.folder.clone())))].spacing(8);
        let open = imp.open.contains(&a.folder);
        if !a.rows.is_empty() {
            btns = btns.push(btn_sm(
                if open { "Hide the files" } else { "Show the files" },
                Btn::Ghost,
                Some(Message::ImportFiles(a.folder.clone())),
            ));
        }
        c = c.push(btns);
        if open {
            for r in a.rows.iter().take(60) {
                let old = if r.old_title.is_empty() { base_name(&r.path).to_string() } else { r.old_title.clone() };
                let line = match (r.new_title.is_empty(), r.new_track > 0) {
                    (true, _) => format!("{old}  (no match)"),
                    (false, true) => format!("{old}  →  {}. {}", r.new_track, r.new_title),
                    (false, false) => format!("{old}  →  {}", r.new_title),
                };
                c = c.push(t(line).size(12.5).color(if r.new_title.is_empty() { p.warn } else { p.muted }));
            }
        }
    }
    container(c).padding([12, 14]).width(Length::Fill).style(theme::panel2).into()
}

fn import_dialog<'a>(imp: &'a Import, root: &'a str) -> El<'a, Message> {
    if let Some(o) = &imp.others {
        return others_dialog(o);
    }
    let mut body = column![
        small_muted(place(root, &imp.folder)),
        crate::pages::settings::switch(imp.keep, "Keep the files where they are", Message::ImportKeep),
        small_muted(if imp.keep { KEEP_HELP } else { MOVE_HELP }),
    ]
    .spacing(10);
    let mut chosen = 0;
    match &imp.albums {
        None => body = body.push(small_muted(if imp.keep { "Reading the folder…" } else { "Looking for matches…" })),
        Some(Err(e)) => body = body.push(error_box(e)),
        Some(Ok(list)) if list.is_empty() => body = body.push(small_muted("No songs in this folder")),
        Some(Ok(list)) => {
            for a in list {
                let on = !imp.skip.contains(&a.folder);
                chosen += usize::from(on);
                body = body.push(album_card(a, on, imp));
            }
        }
    }
    let label = if imp.applying {
        "Importing…".to_string()
    } else if chosen == 1 {
        "Import 1 album".to_string()
    } else {
        format!("Import {chosen} albums")
    };
    let ok = (chosen > 0 && !imp.applying).then_some(Message::Submit);
    widgets::modal("Import", body, Some(footer(label, ok)), true, Message::CloseDialog)
}

fn others_dialog<'a>(o: &'a Others) -> El<'a, Message> {
    let mut body = column![
        row![
            container(field("Artist", input(&o.artist, Message::OthersArtist).on_submit(Message::OthersSearch))).width(Length::Fill),
            container(field("Album", input(&o.album, Message::OthersAlbum).on_submit(Message::OthersSearch))).width(Length::Fill),
        ]
        .spacing(14),
        btn_sm(
            if o.searching { "Searching…" } else { "Search" },
            Btn::Default,
            (!o.searching).then_some(Message::OthersSearch),
        ),
    ]
    .spacing(12);
    match &o.found {
        None => {}
        Some(list) if list.is_empty() => {
            if !o.searching {
                body = body.push(small_muted("Nothing found"));
            }
        }
        Some(list) => {
            let rows: Vec<El<'a, Message>> = list
                .iter()
                .enumerate()
                .map(|(i, r)| {
                    let mut sub = Vec::new();
                    if r.track_count > 0 {
                        sub.push(format!("{} songs", r.track_count));
                    }
                    if r.score > 0.0 {
                        sub.push(format!("{}% match", (r.score * 100.0).round() as i64));
                    }
                    list_row(None, r.line(), sub.join(" · "), Vec::new(), (!o.picking).then_some(Message::OthersPick(i)))
                })
                .collect();
            body = body.push(card_flat(column(rows)).width(Length::Fill));
        }
    }
    let close = btn("Close", Btn::Default, Some(Message::OthersClose));
    widgets::modal("Other matches", body, Some(close.into()), true, Message::OthersClose)
}

/// The ⋯ menu or the open dialog.
pub fn overlay<'a>(state: &'a State, ctx: &Ctx<'a>) -> Option<El<'a, Message>> {
    if let Some(o) = library::shared_overlay(&state.shared, ctx) {
        return Some(o.map(Message::Shared));
    }
    if let Some((at, menu)) = &state.menu {
        let items = menu_items(state, menu);
        if !items.is_empty() {
            return Some(pop_menu(*at, items, Message::CloseMenu, ctx.size));
        }
    }
    let root_name = state.current_root().map(|r| r.name.as_str()).unwrap_or("");
    Some(match state.dialog.as_ref()? {
        Dialog::NewFolder { name } => name_dialog("New folder", name, "Make folder"),
        Dialog::Rename { name, .. } => name_dialog("Rename", name, "Rename"),
        Dialog::Move { names, root, path, dirs, .. } => move_dialog(state, names, root, path, dirs),
        Dialog::Trash { names } => ask(
            match names.as_slice() {
                [one] => format!("Move “{one}” to trash?"),
                _ => format!("Move {} items to trash?", names.len()),
            },
            TRASH_NOTE,
            "Move to trash",
            true,
        ),
        Dialog::Tags { name, original, values, saving } => tags_dialog(name, original, values, *saving),
        Dialog::Upload { items, conflict } => upload_dialog(items, conflict, root_name, &state.path),
        Dialog::Import(imp) => import_dialog(imp, root_name),
        Dialog::DeleteForever { name, .. } => ask(format!("Delete “{name}” forever?"), "This can't be undone.", "Delete forever", true),
        Dialog::EmptyTrash => ask(
            "Empty the trash?".to_string(),
            "Everything in it is deleted forever. This can't be undone.",
            "Empty trash",
            true,
        ),
    })
}

// ----- keys, drops and the upload's progress -----------------------------------------------------

/// What a key does in the list: Enter opens, Backspace goes up one folder, Delete (or ⌘/Ctrl +
/// Backspace, the Finder's) moves to trash, ⌘/Ctrl+A selects all, Escape clears the selection (or,
/// with none, the search), the arrows move it (with Shift it grows), ⌘/Ctrl+F or `/` goes to the
/// search box.
pub fn key_act(key: &Key, m: Modifiers) -> Option<KeyAct> {
    let cmd = m.command();
    match key.as_ref() {
        Key::Named(Named::Enter) => Some(KeyAct::Open),
        Key::Named(Named::Backspace) if cmd => Some(KeyAct::Trash),
        Key::Named(Named::Backspace) => Some(KeyAct::Up),
        Key::Named(Named::Delete) => Some(KeyAct::Trash),
        Key::Named(Named::F2) => Some(KeyAct::Rename),
        Key::Named(Named::Escape) => Some(KeyAct::Clear),
        Key::Named(Named::ArrowDown) => Some(KeyAct::Step(1, m.shift())),
        Key::Named(Named::ArrowUp) => Some(KeyAct::Step(-1, m.shift())),
        Key::Character(c) if cmd && c.eq_ignore_ascii_case("a") => Some(KeyAct::All),
        Key::Character(c) if cmd && c.eq_ignore_ascii_case("f") => Some(KeyAct::Find),
        Key::Character("/") if !cmd && !m.alt() => Some(KeyAct::Find),
        Key::Character(c) if !cmd && !m.alt() && !m.control() && !m.logo() => {
            let mut chars = c.chars();
            match (chars.next(), chars.next()) {
                (Some(ch), None) if ch.is_alphanumeric() => Some(KeyAct::Jump(ch)),
                _ => None,
            }
        }
        _ => None,
    }
}

/// The next name after `from` (round to the top) that starts with `letter`, ignoring case.
pub fn jump_to(names: &[&str], from: Option<usize>, letter: char) -> Option<usize> {
    let n = names.len();
    let start = from.map(|f| f + 1).unwrap_or(0);
    let want: String = letter.to_lowercase().collect();
    (0..n)
        .map(|k| (start + k) % n)
        .find(|&i| names[i].to_lowercase().starts_with(&want))
}

/// The sort kept on this computer: `{sort, dir}` (or the web's JSON text of it).
fn saved_sort(v: Option<&Value>) -> Option<(String, String)> {
    let v = match v? {
        Value::String(s) => serde_json::from_str(s).ok()?,
        other => other.clone(),
    };
    let sort = fmt::s(&v, "sort");
    let dir = fmt::s(&v, "dir");
    (SORTS.iter().any(|(k, _)| *k == sort) && (dir == "asc" || dir == "desc")).then_some((sort, dir))
}

/// The list's keys (only those no text box took), the modifiers for the clicks, and files held
/// over or dropped on the window.
fn list_event(e: iced::Event, status: iced::event::Status, _: iced::window::Id) -> Option<Message> {
    use iced::window::Event as W;
    match e {
        iced::Event::Keyboard(keyboard::Event::ModifiersChanged(m)) => Some(Message::Modifiers(m)),
        iced::Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. })
            if status == iced::event::Status::Ignored =>
        {
            key_act(&key, modifiers).map(Message::Key)
        }
        iced::Event::Window(W::FileHovered(_)) => Some(Message::Hover(true)),
        iced::Event::Window(W::FilesHoveredLeft) => Some(Message::Hover(false)),
        iced::Event::Window(W::FileDropped(p)) => Some(Message::Dropped(p)),
        _ => None,
    }
}

/// While a dialog or a menu is open: Escape closes it, Enter confirms it (when no text box took the
/// key), files dropped join an upload being set up.
fn dialog_event(e: iced::Event, status: iced::event::Status, _: iced::window::Id) -> Option<Message> {
    match e {
        iced::Event::Keyboard(keyboard::Event::KeyPressed {
            key: Key::Named(Named::Escape),
            ..
        }) => Some(Message::CloseDialog),
        iced::Event::Keyboard(keyboard::Event::KeyPressed {
            key: Key::Named(Named::Enter),
            ..
        }) if status == iced::event::Status::Ignored => Some(Message::Enter),
        iced::Event::Keyboard(keyboard::Event::ModifiersChanged(m)) => Some(Message::Modifiers(m)),
        iced::Event::Window(iced::window::Event::FileDropped(p)) => Some(Message::Dropped(p)),
        _ => None,
    }
}

pub fn subscription(state: &State) -> Subscription<Message> {
    let busy = state.dialog.is_some() || state.menu.is_some() || state.shared.dialog.is_some() || state.shared.menu.is_some();
    let mut subs = vec![if busy {
        iced::event::listen_with(dialog_event)
    } else {
        iced::event::listen_with(list_event)
    }];
    if state.upload.is_some() {
        // the progress panel is drawn again four times a second
        subs.push(iced::time::every(Duration::from_millis(250)).map(|_| Message::UploadTick));
    }
    Subscription::batch(subs)
}

// ----- this page's icons (the web's feather shapes, drawn like widgets::icons) -------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum F {
    Folder,
    File,
    Image,
    Upload,
    FolderPlus,
    Up,
    Right,
}

impl F {
    const ALL: [F; 7] = [F::Folder, F::File, F::Image, F::Upload, F::FolderPlus, F::Up, F::Right];

    fn body(self) -> &'static str {
        match self {
            F::Folder => r##"<path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/>"##,
            F::File => r##"<path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><path d="M14 2v6h6"/>"##,
            F::Image => r##"<rect x="3" y="3" width="18" height="18" rx="2"/><circle cx="8.5" cy="8.5" r="1.5"/><path d="m21 15-5-5L5 21"/>"##,
            F::Upload => r##"<path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><path d="m17 8-5-5-5 5M12 3v12"/>"##,
            F::FolderPlus => r##"<path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/><path d="M12 11v6M9 14h6"/>"##,
            F::Up => r##"<path d="M12 19V5M5 12l7-7 7 7"/>"##,
            F::Right => r##"<polyline points="9 18 15 12 9 6"/>"##,
        }
    }
}

fn ficon<'a, M: 'a>(i: F, size: f32, color: Color) -> El<'a, M> {
    static H: OnceLock<HashMap<F, svg::Handle>> = OnceLock::new();
    let handles = H.get_or_init(|| {
        F::ALL
            .into_iter()
            .map(|f| {
                let doc = format!(
                    r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#000" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">{}</svg>"##,
                    f.body()
                );
                (f, svg::Handle::from_memory(doc.into_bytes()))
            })
            .collect()
    });
    svg::Svg::new(handles[&i].clone())
        .width(size)
        .height(size)
        .style(move |_, _| svg::Style { color: Some(color) })
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_read_as_the_contract_writes_them() {
        let v = json!({ "name": "01 Crazy.flac", "kind": "file", "size": 31457280, "mtime": 1792300000,
            "audio": { "format": "flac", "duration": 178.4, "title": "Crazy", "artist": "Gnarls Barkley", "album": "St. Elsewhere", "track_id": 812 },
            "image": false });
        let e = Entry::from(&v).unwrap();
        assert_eq!(e.track_id(), 812);
        assert!(e.is_audio() && !e.is_dir() && !e.is_link());
        assert_eq!(e.line(), "Gnarls Barkley – Crazy · 2:58");
        let link = Entry::from(&json!({ "name": "Elsewhere", "kind": "link", "target": "/mnt/x", "target_kind": "dir" })).unwrap();
        assert!(link.is_dir() && link.is_link());
        assert_eq!(link.line(), "Link to /mnt/x");
        let lost = Entry::from(&json!({ "name": "gone", "kind": "link", "target_kind": "missing" })).unwrap();
        assert!(!lost.is_dir());
        assert_eq!(lost.line(), "Link to somewhere else");
        // an audio file the library does not have: no track id, still a song by its name
        let loose = Entry::from(&json!({ "name": "b.MP3", "kind": "file", "audio": { "format": "mp3" } })).unwrap();
        assert_eq!(loose.track_id(), 0);
        assert!(loose.is_audio());
        assert_eq!(loose.line(), "");
        assert!(Entry::from(&json!({ "name": "x.opus" })).unwrap().is_audio());
        assert!(!Entry::from(&json!({ "name": ".flac" })).unwrap().is_audio());
        assert!(Entry::from(&json!({ "kind": "dir" })).is_none());

        let l = Listing::from(&json!({ "root": "library", "path": "", "total": 812, "offset": 0, "entries": [v], "via_link": true }), 0);
        assert_eq!((l.entries.len(), l.total, l.via_link), (1, 812, true));
        // a total the server left out is at least what came
        assert_eq!(Listing::from(&json!({ "entries": [{ "name": "a" }] }), 500).total, 501);
    }

    #[test]
    fn roots_and_the_trash() {
        let roots: Vec<Root> = fmt::items(&json!([
            { "id": "library", "name": "Library", "path": "/music", "writable": true, "free_bytes": 1000 },
            { "id": "torrent-1", "name": "Finished torrents", "path": "/downloads/t", "writable": false, "free_bytes": 0 },
            { "id": "trash", "name": "Trash", "path": "/config/trash", "writable": false },
            { "name": "no id" }
        ]))
        .iter()
        .filter_map(Root::from)
        .collect();
        assert_eq!(roots.len(), 3);
        assert!(roots[0].writable && !roots[1].writable && roots[2].is_trash());
        assert_eq!(Root::from(&json!({ "id": "x" })).unwrap().name, "x");
        assert!(Root::from(&json!({ "id": "x" })).unwrap().writable, "writable unless the server says not");

        let it = TrashItem::from(&json!({ "id": "1792300000-1", "name": "", "root": "library", "path": "Artist/Album",
            "at": 1792300000, "by": "admin", "size": 2000, "kind": "dir" }))
        .unwrap();
        assert_eq!(it.name, "Album");
        assert_eq!(it.line(&roots), "From Library › Artist · 18 Oct 2026 · by admin · 2.0 kB");
        let gone = TrashItem::from(&json!({ "id": "1-1", "name": "a.flac", "root": "old", "path": "a.flac" })).unwrap();
        assert_eq!(gone.line(&roots), "From old");
    }

    #[test]
    fn releases_and_albums_of_an_import() {
        let r = Release::from(&json!({ "id": "mb-1", "title": "St. Elsewhere", "artist": "Gnarls Barkley", "date": "2006-05-08",
            "provider": "MusicBrainz", "track_count": 14, "score": 92 }))
        .unwrap();
        assert!((r.score - 0.92).abs() < 1e-9);
        assert_eq!(r.line(), "Gnarls Barkley – St. Elsewhere (2006) · MusicBrainz");
        let m = Release::from(&json!({ "id": "x", "score": 40, "match": 0.5 })).unwrap();
        assert!((m.score - 0.5).abs() < 1e-9, "match wins over score");

        let v = json!({ "albums": [
            { "folder": "Gnarls Barkley/St. Elsewhere", "artist": "Gnarls Barkley", "album": "St. Elsewhere", "files": 14,
              "release": { "id": "mb-1", "title": "St. Elsewhere", "artist": "Gnarls Barkley", "date": "2006", "provider": "MusicBrainz", "score": 92 },
              "confidence": 0.92,
              "rows": [{ "path": "Gnarls Barkley/St. Elsewhere/02 crazy.flac", "old_title": "crazy", "new_title": "Crazy", "new_track": 2, "new_disc": 1, "new_path": "x" }] },
            { "folder": "Loose", "artist": "", "album": "", "release": null, "rows": [] }
        ] });
        let albums = albums_of(&v);
        assert_eq!(albums.len(), 2);
        assert_eq!(albums[0].match_line(), "Matches Gnarls Barkley – St. Elsewhere (2006) · MusicBrainz · 92% sure");
        assert_eq!(albums[0].rows[0].new_track, 2);
        assert_eq!(albums[1].album, "Loose");
        assert_eq!(albums[1].title(), "Loose");
        assert_eq!(albums[1].files, 0);
        assert_eq!(albums[1].match_line(), NO_MATCH);
        assert_eq!(releases_of(&json!({ "candidates": [{ "id": "a" }, { "id": "" }], "provider": "musicbrainz" })).len(), 1);
    }

    #[test]
    fn paths_and_words() {
        assert_eq!(join("", "a"), "a");
        assert_eq!(join("a/b", "c"), "a/b/c");
        assert_eq!(parent_of("a/b/c"), "a/b");
        assert_eq!(parent_of("a"), "");
        assert_eq!(base_name("a/b/c.flac"), "c.flac");
        assert_eq!(place("Library", ""), "Library");
        assert_eq!(place("Library", "A/B"), "Library › A › B");
        assert!(under("A/L", "A/L") && under("A/L", "A/L/x") && !under("A/L", "A/Lx") && !under("A/L", "A"));
        assert_eq!(sort_label("name", "asc"), "Sort: Name ↑");
        assert_eq!(sort_label("mtime", "desc"), "Sort: Date modified ↓");
        assert_eq!(sort_order("size", "desc"), "largest first");
        assert_eq!(sort_order("mtime", "asc"), "oldest first");
        assert_eq!(sort_order("name", "desc"), "Z–A");
        assert_eq!((default_dir("name"), default_dir("size")), ("asc", "desc"));
        assert_eq!(failures(&json!({ "moved": 2, "failed": [] })), "");
        assert_eq!(
            failures(&json!({ "failed": [{ "path": "A/x.flac", "reason": "A file with that name is already there" }] })),
            "x.flac: A file with that name is already there"
        );
        assert_eq!(
            failures(&json!({ "failed": [{ "id": "1-1", "reason": "Its folder is no longer one Encore may use" }, { "id": "1-2", "reason": "r" }] })),
            "2 weren't done. 1-1: Its folder is no longer one Encore may use"
        );
        assert!(may_move_to("library", "", &["A".into()], "staging", ""));
        assert!(!may_move_to("library", "X", &["A".into()], "library", "X"), "where it is already");
        assert!(!may_move_to("library", "", &["A".into()], "library", "A/B"), "not into itself");
        assert!(may_move_to("library", "", &["A".into()], "library", "AB"));
    }

    #[test]
    fn names_are_checked_before_asking() {
        assert_eq!(safe_name(" a/b\\c\u{7}.flac "), "a_b_c.flac");
        assert_eq!(name_problem(""), Some(""));
        assert_eq!(name_problem(".."), Some(""));
        assert_eq!(name_problem("a/b"), Some(SLASH));
        assert_eq!(name_problem("a\u{1}"), Some("A name can't contain invisible or control characters"));
        assert_eq!(name_problem(&"x".repeat(256)), Some("That name is too long"));
        assert_eq!(name_problem("St. Elsewhere (2006)"), None);
    }

    #[test]
    fn clicks_select_like_a_file_manager() {
        let none = BTreeSet::new();
        let (s, a) = click_select(&none, None, 2, false, false);
        assert_eq!((s, a), (BTreeSet::from([2]), Some(2)));
        let (s, a) = click_select(&BTreeSet::from([2]), Some(2), 5, true, false);
        assert_eq!((s, a), (BTreeSet::from([2, 3, 4, 5]), Some(2)), "Shift: from the anchor");
        let (s, _) = click_select(&BTreeSet::from([2, 3, 4, 5]), Some(2), 0, true, false);
        assert_eq!(s, BTreeSet::from([0, 1, 2]), "Shift the other way replaces the range");
        let (s, a) = click_select(&BTreeSet::from([2, 3]), Some(2), 3, false, true);
        assert_eq!((s, a), (BTreeSet::from([2]), Some(3)), "Ctrl toggles");
        let (s, _) = click_select(&BTreeSet::from([7]), Some(2), 4, true, true);
        assert_eq!(s, BTreeSet::from([2, 3, 4, 7]), "Ctrl+Shift adds the range");
        let (s, _) = click_select(&BTreeSet::from([1, 2]), None, 4, true, false);
        assert_eq!(s, BTreeSet::from([4]), "Shift without an anchor selects the one");
    }

    /// A search's answer (server/crates/files/src/search.rs), where a result goes, the folder's filter
    /// and the keys that reach the box.
    #[test]
    fn search_reads_hits_filters_and_jumps() {
        let v = json!({ "root": "library", "path": "", "truncated": true, "results": [
            { "name": "Mezzanine", "kind": "dir", "path": "Massive Attack/Mezzanine", "parent": "Massive Attack" },
            { "name": "03 Teardrop.flac", "kind": "file", "path": "Massive Attack/Mezzanine/03 Teardrop.flac",
              "parent": "Massive Attack/Mezzanine", "size": 9, "audio": { "format": "flac" } },
            { "name": "to elsewhere", "kind": "link", "target_kind": "dir", "path": "to elsewhere", "parent": "" },
            { "name": "no path" },
        ] });
        let h = Hits::from(&v);
        assert_eq!((h.hits.len(), h.truncated), (3, true));
        assert_eq!(h.hits[0].target(), ("Massive Attack/Mezzanine".to_string(), false, None));
        assert_eq!(h.hits[1].target(), ("Massive Attack/Mezzanine".to_string(), false, Some("03 Teardrop.flac".to_string())));
        assert!(h.hits[1].entry.is_audio());
        assert_eq!(h.hits[2].target(), ("to elsewhere".to_string(), true, None));
        let words = find_words("  PARK blur ");
        assert!(name_matches("Blur – Parklife", &words));
        assert!(!name_matches("Blur – Parklife", &find_words("blur nope")));
        assert_eq!(found_line(3, false, "Library"), "3 found in Library");
        assert_eq!(found_line(1, false, "Library"), "1 found in Library");
        assert_eq!(found_line(0, false, "Staging"), "Nothing in Staging is called that");
        assert_eq!(found_line(200, true, "Library"), "200+ found in Library (the nearest first; type more to narrow it)");
        let cmd = if cfg!(target_os = "macos") { Modifiers::LOGO } else { Modifiers::CTRL };
        assert_eq!(key_act(&Key::Character("f".into()), cmd), Some(KeyAct::Find));
        assert_eq!(key_act(&Key::Character("/".into()), Modifiers::default()), Some(KeyAct::Find));
    }

    #[test]
    fn keys_of_the_list() {
        let none = Modifiers::default();
        assert_eq!(key_act(&Key::Named(Named::Enter), none), Some(KeyAct::Open));
        assert_eq!(key_act(&Key::Named(Named::Backspace), none), Some(KeyAct::Up));
        assert_eq!(key_act(&Key::Named(Named::Backspace), Modifiers::COMMAND), Some(KeyAct::Trash));
        assert_eq!(key_act(&Key::Named(Named::Delete), none), Some(KeyAct::Trash));
        assert_eq!(key_act(&Key::Character("a".into()), Modifiers::COMMAND), Some(KeyAct::All));
        assert_eq!(key_act(&Key::Character("a".into()), none), Some(KeyAct::Jump('a')), "a letter jumps to a name");
        assert_eq!(key_act(&Key::Named(Named::ArrowDown), Modifiers::SHIFT), Some(KeyAct::Step(1, true)));
        assert_eq!(key_act(&Key::Named(Named::Space), none), None, "Space stays the player's");
    }

    #[test]
    fn a_letter_jumps_and_the_sort_is_kept() {
        let names = ["Abba", "beta", "Bravo", "cello"];
        assert_eq!(jump_to(&names, None, 'b'), Some(1));
        assert_eq!(jump_to(&names, Some(1), 'B'), Some(2));
        assert_eq!(jump_to(&names, Some(2), 'b'), Some(1), "round to the top");
        assert_eq!(jump_to(&names, Some(0), 'z'), None);
        assert_eq!(jump_to(&[], None, 'a'), None);
        assert_eq!(key_act(&Key::Character("b".into()), Modifiers::default()), Some(KeyAct::Jump('b')));
        assert_eq!(key_act(&Key::Character("b".into()), Modifiers::ALT), None);
        assert_eq!(key_act(&Key::Named(Named::F2), Modifiers::default()), Some(KeyAct::Rename));
        assert_eq!(saved_sort(Some(&json!({ "sort": "size", "dir": "desc" }))), Some(("size".into(), "desc".into())));
        assert_eq!(saved_sort(Some(&json!(r#"{"sort":"mtime","dir":"asc"}"#))), Some(("mtime".into(), "asc".into())), "the web's text");
        assert_eq!(saved_sort(Some(&json!({ "sort": "colour", "dir": "asc" }))), None);
        assert_eq!(saved_sort(None), None);
    }

    #[test]
    fn folders_upload_as_a_browser_names_them() {
        let d = tempfile::tempdir().unwrap();
        let album = d.path().join("Album");
        std::fs::create_dir_all(album.join("CD1")).unwrap();
        std::fs::write(album.join("CD1/01 Song.flac"), b"flac").unwrap();
        std::fs::write(album.join("cover.jpg"), b"jpg").unwrap();
        std::fs::write(album.join(".DS_Store"), b"x").unwrap();
        std::fs::create_dir_all(album.join(".hidden")).unwrap();
        std::fs::write(album.join(".hidden/x.flac"), b"x").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(d.path(), album.join("loop")).unwrap();
        let single = d.path().join("one.mp3");
        std::fs::write(&single, b"mp3d").unwrap();
        let items = items_of(&[album.clone(), single, d.path().join("missing")]);
        let names: Vec<&str> = items.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["Album/cover.jpg", "Album/CD1/01 Song.flac", "one.mp3"]);
        assert_eq!(items[1].size, 4);
        assert_eq!(items[1].file, album.join("CD1/01 Song.flac"));
        let mut empty = Vec::new();
        walk(&d.path().join("missing"), "", &mut empty, 0);
        assert!(empty.is_empty());
    }

    #[test]
    fn upload_answers() {
        assert_eq!(upload_outcome(&json!({ "uploaded": [{ "name": "a" }], "renamed": [], "skipped": [] })), UploadOutcome::Uploaded);
        assert_eq!(
            upload_outcome(&json!({ "uploaded": [{ "name": "a" }], "renamed": [{ "from": "a.flac", "to": "a (2).flac" }] })),
            UploadOutcome::Renamed("a (2).flac".into())
        );
        assert_eq!(
            upload_outcome(&json!({ "skipped": [{ "name": "a", "reason": "A file with that name is already there" }] })),
            UploadOutcome::Skipped("A file with that name is already there".into())
        );
        assert_eq!(upload_outcome(&json!({})), UploadOutcome::Uploaded);
    }

    /// Reads one HTTP request (to its Content-Length, or a chunked body's end).
    fn read_request(s: &mut std::net::TcpStream) -> Vec<u8> {
        use std::io::Read;
        let mut buf = Vec::new();
        let mut chunk = [0u8; 16384];
        loop {
            let n = s.read(&mut chunk).unwrap_or(0);
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
            if let Some(h) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&buf[..h]).to_lowercase();
                let len = head
                    .lines()
                    .find_map(|l| l.strip_prefix("content-length:"))
                    .and_then(|v| v.trim().parse::<usize>().ok());
                match len {
                    Some(len) if buf.len() >= h + 4 + len => break,
                    None if buf.ends_with(b"0\r\n\r\n") => break,
                    _ => {}
                }
            }
        }
        buf
    }

    #[tokio::test]
    async fn an_upload_streams_the_file_under_its_name_in_the_folder() {
        use std::io::Write;
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let req = read_request(&mut s);
            let body = r#"{"uploaded":[{"name":"Album/01 Song.flac","path":"x/Album/01 Song (2).flac","size":9}],"renamed":[{"from":"Album/01 Song.flac","to":"Album/01 Song (2).flac"}],"skipped":[]}"#;
            let _ = write!(
                s,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            req
        });
        let d = tempfile::tempdir().unwrap();
        let file = d.path().join("01 Song.flac");
        std::fs::write(&file, b"fLaC-data").unwrap();
        let api = Api::new(&format!("http://127.0.0.1:{port}"));
        let sent = Arc::new(AtomicU64::new(0));
        let item = UploadItem { file, name: "Album/01 Song.flac".into(), size: 9 };
        let out = upload_one(
            api,
            item,
            "library".into(),
            "Some Folder".into(),
            "rename".into(),
            sent.clone(),
            Arc::new(AtomicBool::new(false)),
        )
        .await;
        assert_eq!(out, Ok(UploadOutcome::Renamed("Album/01 Song (2).flac".into())));
        assert_eq!(sent.load(Ordering::Relaxed), 9);
        let req = String::from_utf8_lossy(&server.join().unwrap()).to_string();
        assert!(req.starts_with("POST /api/files/upload?root=library&path=Some%20Folder&conflict=rename HTTP/1.1"), "{req}");
        assert!(req.to_lowercase().contains("x-requested-with: encore"), "the CSRF header");
        assert!(req.contains(r#"name="file"; filename="Album/01 Song.flac""#), "{req}");
        assert!(req.contains("fLaC-data"));
    }

    #[tokio::test]
    async fn stop_ends_an_upload_before_its_bytes_are_sent() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            read_request(&mut s)
        });
        let d = tempfile::tempdir().unwrap();
        let file = d.path().join("big.flac");
        std::fs::write(&file, vec![7u8; 300_000]).unwrap();
        let api = Api::new(&format!("http://127.0.0.1:{port}"));
        let sent = Arc::new(AtomicU64::new(0));
        let item = UploadItem { file, name: "big.flac".into(), size: 300_000 };
        let out = upload_one(api, item, "staging".into(), String::new(), "skip".into(), sent.clone(), Arc::new(AtomicBool::new(true))).await;
        assert!(out.is_err());
        assert_eq!(sent.load(Ordering::Relaxed), 0);
        let got = server.join().unwrap();
        assert!(!got.windows(3).any(|w| w == [7, 7, 7]), "none of the file reached the server");
    }
}
