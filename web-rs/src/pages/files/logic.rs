//! The file manager's plain rules, with no DOM: paths, names, the sort words, the selection, and the lines that
//! say what happened. Kept apart so `cargo test` checks them natively.

use crate::json::J;
use serde_json::Value;

// ---- paths ----------------------------------------------------------------------------------------------------

/// "Artist" + "Album" → "Artist/Album"; "" + "Artist" → "Artist".
pub fn join(path: &str, name: &str) -> String {
    if path.is_empty() { name.to_string() } else { format!("{path}/{name}") }
}

/// "a/b/c" → "a/b"; "a" → "".
pub fn parent_of(path: &str) -> String {
    path.rsplit_once('/').map(|(a, _)| a.to_string()).unwrap_or_default()
}

/// "a/b/c" → "c".
pub fn last_part(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Whether `path` is `at` or inside it.
pub fn at_or_under(path: &str, at: &str) -> bool {
    at.is_empty() || path == at || path.strip_prefix(at).is_some_and(|r| r.starts_with('/'))
}

/// What the path box sends: the text as typed, without the spaces around it and the slashes at its end. Nothing
/// else is changed: a `..` or an absolute path goes to the server as it is, and the server says no.
pub fn typed_path(s: &str) -> String {
    s.trim().trim_end_matches('/').to_string()
}

/// The breadcrumbs of a path: each part with the path up to it.
pub fn crumbs(path: &str) -> Vec<(String, String)> {
    let mut out = vec![];
    let mut at = String::new();
    for part in path.split('/').filter(|p| !p.is_empty()) {
        at = join(&at, part);
        out.push((part.to_string(), at.clone()));
    }
    out
}

/// "Library › Artist › Album" (the root's name and the path's parts).
pub fn place(root_name: &str, path: &str) -> String {
    std::iter::once(root_name.to_string()).chain(crumbs(path).into_iter().map(|(p, _)| p)).collect::<Vec<_>>().join(" › ")
}

// ---- names ----------------------------------------------------------------------------------------------------

/// Characters a name may not hold (the server's rule): control characters, the line and paragraph separators and
/// the characters that turn text around.
fn hidden(c: char) -> bool {
    c.is_control() || matches!(c, '\u{2028}' | '\u{2029}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

/// What is wrong with a new name, in the server's words (None: it can be sent; the server still checks it).
pub fn name_problem(name: &str) -> Option<&'static str> {
    if name.trim().is_empty() { return Some("A name can't be empty"); }
    if name.contains('/') || name.contains('\\') { return Some("A name can't contain / or \\"); }
    if name == "." || name == ".." { return Some("That name isn't allowed"); }
    if name.len() > 255 { return Some("That name is too long"); }
    if name.chars().any(hidden) { return Some("A name can't contain invisible or control characters"); }
    None
}

/// One part of an uploaded file's name as the server takes it: a backslash becomes "_", hidden characters go.
pub fn upload_part(part: &str) -> String {
    part.replace('\\', "_").chars().filter(|c| !hidden(*c)).collect::<String>().trim().to_string()
}

/// A file's name relative to the upload's folder ("Album/CD1/01 Song.flac"), each part cleaned; empty parts
/// dropped.
pub fn upload_name(rel: &str) -> String {
    rel.split('/').map(upload_part).filter(|p| !p.is_empty()).collect::<Vec<_>>().join("/")
}

/// The part of a name a rename selects first, in UTF-16 units (what a text box counts): "01 Song.flac" → 7
/// ("01 Song"), a folder all of it.
pub fn stem_len(name: &str, is_dir: bool) -> usize {
    let n = name.encode_utf16().count();
    if is_dir { return n; }
    match name.rfind('.') {
        Some(i) if i > 0 => name[..i].encode_utf16().count(),
        _ => n,
    }
}

const AUDIO: &[&str] = &["flac", "mp3", "m4a", "aac", "ogg", "opus", "wav", "aiff", "aif", "alac", "ape", "wv", "dsf", "dff", "wma", "mpc", "tta"];

/// An audio file by its name (the server's own list may be longer; `audio` on an entry is what counts).
pub fn is_audio_name(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(_, e)| AUDIO.contains(&e.to_ascii_lowercase().as_str()))
}

// ---- entries (§2.4 of the client shapes) ------------------------------------------------------------------------

/// A folder, or a link to one (a link opens only when its target is in one of the roots; the server says so).
pub fn is_dir(e: &Value) -> bool {
    let k = e.s("kind");
    k == "dir" || (k == "link" && e.s("target_kind") == "dir")
}
pub fn is_link(e: &Value) -> bool { e.s("kind") == "link" }
/// An audio file (not a link to one: nothing is written through a link).
pub fn is_audio(e: &Value) -> bool { e.s("kind") == "file" && (e.g("audio").is_object() || is_audio_name(&e.s("name"))) }
/// The library's id for this file, when it has it.
pub fn track_id(e: &Value) -> i64 { e.o("audio").i("track_id") }

/// The line under a name: a link's target, a song's artist, title and length, or nothing (size and date have
/// columns of their own).
pub fn entry_line(e: &Value) -> String {
    if is_link(e) {
        let t = e.s("target");
        return format!("Link to {}", if t.is_empty() { "somewhere else".to_string() } else { t });
    }
    let a = e.o("audio");
    let who = [a.s("artist"), a.s("title")].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" – ");
    if who.is_empty() { return String::new(); }
    let d = a.f("duration");
    if d > 0.0 { format!("{who} · {}", crate::ui::fmt::dur(d)) } else { who }
}

// ---- sorting --------------------------------------------------------------------------------------------------

/// The sorts the server knows, with their names.
pub const SORTS: [(&str, &str); 3] = [("name", "Name"), ("size", "Size"), ("mtime", "Date modified")];

pub fn sort_label(key: &str) -> &'static str {
    SORTS.iter().find(|(k, _)| *k == key).map(|(_, l)| *l).unwrap_or("Name")
}
/// The order of a sort in words.
pub fn order_text(key: &str, dir: &str) -> &'static str {
    match (key, dir == "asc") {
        ("size", true) => "smallest first",
        ("size", false) => "largest first",
        ("mtime", true) => "oldest first",
        ("mtime", false) => "newest first",
        (_, true) => "A–Z",
        (_, false) => "Z–A",
    }
}
/// "Sort: Name ↑" (↑ ascending, ↓ descending).
pub fn sort_button(key: &str, dir: &str) -> String {
    format!("Sort: {} {}", sort_label(key), if dir == "desc" { "↓" } else { "↑" })
}
/// The order a sort starts in: names A–Z, sizes and dates largest and newest first.
pub fn first_dir(key: &str) -> &'static str { if key == "name" { "asc" } else { "desc" } }
/// The sort after choosing `key`: the same key turns the order round, another starts in its own order.
pub fn next_sort(cur: (&str, &str), key: &str) -> (String, String) {
    if cur.0 == key { (key.to_string(), if cur.1 == "asc" { "desc" } else { "asc" }.to_string()) } else { (key.to_string(), first_dir(key).to_string()) }
}

// ---- search ---------------------------------------------------------------------------------------------------

/// The words of a search, lowercased; empty when nothing was typed.
pub fn search_words(q: &str) -> Vec<String> {
    q.split_whitespace().map(str::to_lowercase).collect()
}

/// Whether a name holds every word (lowercased, anywhere in it, in any order): the filter of the folder that is
/// open. The server's search (`GET /api/files/search`) also drops accents; the filter only lowercases, so it may
/// show a little less than the search below it, never something the words don't say.
pub fn name_matches(name: &str, words: &[String]) -> bool {
    let n = name.to_lowercase();
    words.iter().all(|w| n.contains(w.as_str()))
}

/// The rows of `rows` whose name (a trash item's name, or its path's last part) holds every word of `q`; all of
/// them when `q` is blank.
pub fn filter_rows(rows: &[Value], q: &str) -> Vec<Value> {
    let words = search_words(q);
    if words.is_empty() { return rows.to_vec(); }
    rows.iter().filter(|e| {
        let name = if e.s("name").is_empty() { last_part(&e.s("path")).to_string() } else { e.s("name") };
        name_matches(&name, &words)
    }).cloned().collect()
}

/// Whether a search result opens as a folder (a folder, or a link to one) rather than being shown in its folder.
pub fn hit_is_dir(hit: &Value) -> bool {
    hit.s("kind") == "dir" || (hit.s("kind") == "link" && hit.s("target_kind") == "dir")
}

/// Where a search result goes when clicked: (the folder to open, the names to pick in it). A folder opens
/// itself; a file opens its folder with the file picked.
pub fn hit_target(hit: &Value) -> (String, Vec<String>) {
    if hit_is_dir(hit) { (hit.s("path"), vec![]) } else { (hit.s("parent"), vec![hit.s("name")]) }
}

/// The search results' heading: "3 found in Library", "200+ found in Library" when the server stopped early,
/// "Nothing in Library is called that".
pub fn found_line(n: usize, truncated: bool, place: &str) -> String {
    match (n, truncated) {
        (0, false) => format!("Nothing in {place} is called that"),
        (0, true) => format!("Nothing found yet in {place}: the search stopped early, try more words"),
        (n, true) => format!("{n}+ found in {place} (the nearest first; type more to narrow it)"),
        (1, false) => format!("1 found in {place}"),
        (n, false) => format!("{n} found in {place}"),
    }
}

// ---- selection ------------------------------------------------------------------------------------------------

/// Which rows are picked, the row a Shift-click extends from (`anchor`) and the row the keyboard is on (`cursor`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sel {
    pub on: Vec<bool>,
    pub anchor: Option<usize>,
    pub cursor: Option<usize>,
}

impl Sel {
    pub fn new(n: usize) -> Self { Self { on: vec![false; n], anchor: None, cursor: None } }
    pub fn len(&self) -> usize { self.on.len() }
    pub fn is_empty(&self) -> bool { self.on.is_empty() }
    pub fn count(&self) -> usize { self.on.iter().filter(|x| **x).count() }
    pub fn picked(&self) -> Vec<usize> { self.on.iter().enumerate().filter(|(_, x)| **x).map(|(i, _)| i).collect() }
    pub fn is(&self, i: usize) -> bool { self.on.get(i).copied().unwrap_or(false) }
    pub fn clear(&mut self) { self.on.iter_mut().for_each(|x| *x = false); }
    pub fn all(&mut self) { self.on.iter_mut().for_each(|x| *x = true); }
    /// Only row `i`.
    pub fn only(&mut self, i: usize) {
        if i >= self.len() { return; }
        self.clear();
        self.on[i] = true;
        self.anchor = Some(i);
        self.cursor = Some(i);
    }
    /// Row `i` in or out (Ctrl / ⌘-click, the check box).
    pub fn toggle(&mut self, i: usize) {
        if i >= self.len() { return; }
        self.on[i] = !self.on[i];
        self.anchor = Some(i);
        self.cursor = Some(i);
    }
    /// Shift-click: the rows from the anchor to `i`; with `add` (Ctrl+Shift) added to what is picked.
    pub fn range(&mut self, i: usize, add: bool) {
        if i >= self.len() { return; }
        let a = self.anchor.unwrap_or(i).min(self.len() - 1);
        if !add { self.clear(); }
        for x in a.min(i)..=a.max(i) { self.on[x] = true; }
        self.anchor = Some(a);
        self.cursor = Some(i);
    }
    /// A click: with Shift a range, with Ctrl or ⌘ a toggle, otherwise only this row.
    pub fn click(&mut self, i: usize, ctrl: bool, shift: bool) {
        if shift { self.range(i, ctrl) } else if ctrl { self.toggle(i) } else { self.only(i) }
    }
    /// The arrow keys: the cursor moves by `delta` (clamped); with Shift the range from the anchor follows it.
    pub fn step(&mut self, delta: i64, shift: bool) {
        if self.is_empty() { return; }
        let last = self.len() as i64 - 1;
        let at = match self.cursor { Some(c) => (c as i64 + delta).clamp(0, last), None => if delta < 0 { last } else { 0 } } as usize;
        if shift { self.range(at, false) } else { self.only(at) }
    }
    /// The same rows again after a reload: the picked names that are still there.
    pub fn keep(names_before: &[String], names_now: &[String]) -> Self {
        let mut s = Self::new(names_now.len());
        for (i, n) in names_now.iter().enumerate() {
            if names_before.contains(n) {
                s.on[i] = true;
                if s.cursor.is_none() { s.cursor = Some(i); s.anchor = Some(i); }
            }
        }
        s
    }
}

/// The next row after `from` whose name starts with `ch` (typing a letter jumps there), round the end.
pub fn jump_to(names: &[String], from: Option<usize>, ch: &str) -> Option<usize> {
    let ch = ch.to_lowercase();
    if ch.is_empty() || names.is_empty() { return None; }
    let n = names.len();
    let start = from.map(|f| f + 1).unwrap_or(0);
    (0..n).map(|k| (start + k) % n).find(|i| names[*i].to_lowercase().starts_with(&ch))
}

// ---- what happened --------------------------------------------------------------------------------------------

/// `{failed: [{path | id | folder, reason}]}` as one line: "" when nothing failed, "<name>: <reason>" for one,
/// "N weren't done. <name>: <reason>" for more.
pub fn failure_line(answer: &Value) -> String {
    let failed = answer.a("failed");
    let Some(first) = failed.first() else { return String::new() };
    let what = ["path", "folder", "id"].iter().map(|k| first.s(k)).find(|s| !s.is_empty()).unwrap_or_default();
    let what = last_part(&what).to_string();
    let why = first.s("reason");
    let one = if what.is_empty() { why } else { format!("{what}: {why}") };
    if failed.len() == 1 { one } else { format!("{} weren't done. {one}", failed.len()) }
}

/// "Moved" / "Moved 3 items", "Moved to trash" / "Moved 3 items to trash", "Restored" / "Restored 3 items".
pub fn done_line(what: &str, n: usize) -> String {
    match (what, n == 1) {
        ("trash", true) => "Moved to trash".into(),
        ("trash", false) => format!("Moved {n} items to trash"),
        ("restore", true) => "Restored".into(),
        ("restore", false) => format!("Restored {n} items"),
        ("delete", true) => "Deleted".into(),
        ("delete", false) => format!("Deleted {n} items"),
        (_, true) => "Moved".into(),
        (_, false) => format!("Moved {n} items"),
    }
}

/// The end of an upload: what the toast says, and its kind.
pub fn upload_end(ok: usize, count: usize, stopped: bool, first_error: &str) -> (String, &'static str) {
    if stopped { return (format!("Upload stopped after {ok} of {count}"), "info"); }
    if ok == count { return (if ok == 1 { "Uploaded 1 file".to_string() } else { format!("Uploaded {ok} files") }, "ok"); }
    // what went wrong stays on screen, with Copy (an error toast)
    (format!("Uploaded {ok} of {count}. {first_error}").trim().to_string(), "err")
}

/// What one upload answer (§4.3) says about the file sent: Ok(None) it went in, Ok(Some(new name)) it went in
/// under another name, Err(reason) it was skipped.
pub fn upload_outcome(answer: &Value) -> Result<Option<String>, String> {
    if let Some(s) = answer.a("skipped").first() {
        let r = s.s("reason");
        return Err(if r.is_empty() { "Skipped".to_string() } else { r });
    }
    if let Some(r) = answer.a("renamed").first() { return Ok(Some(r.s("to"))); }
    Ok(None)
}

/// "Import 1 album" / "Import 3 albums".
pub fn import_button(n: usize) -> String { if n == 1 { "Import 1 album".into() } else { format!("Import {n} albums") } }

/// A release as one line: "Gnarls Barkley – St. Elsewhere (2006) · MusicBrainz".
pub fn release_line(r: &Value) -> String {
    let mut s = [r.s("artist"), r.s("title")].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" – ");
    let date = r.s("date");
    if !date.is_empty() { s.push_str(&format!(" ({})", date.chars().take(4).collect::<String>())); }
    let p = r.s("provider");
    if !p.is_empty() { s.push_str(&format!(" · {p}")); }
    s
}

/// How sure a match is, 0…1: `match` when the server sends it, else `score` / 100.
pub fn match_of(r: &Value) -> f64 {
    if r.g("match").is_number() { r.f("match") } else { r.f("score") / 100.0 }
}

/// An album's confidence, 0…1 (its `confidence`, else its release's).
pub fn album_confidence(a: &Value) -> f64 {
    let c = a.f("confidence");
    if c > 0.0 { c } else if a.g("release").is_object() { match_of(a.g("release")) } else { 0.0 }
}

/// The badge class the match review uses for a confidence.
pub fn conf_class(c: f64) -> &'static str { if c > 0.85 { "ok" } else if c > 0.6 { "warn" } else { "err" } }

/// "From Library › Artist" (where a trashed item was; the root's name when it is still configured).
pub fn trash_from(root_name: &str, path: &str) -> String {
    let parent = parent_of(path);
    if parent.is_empty() { format!("From {root_name}") } else { format!("From {}", place(root_name, &parent)) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn paths_join_split_and_read_as_crumbs() {
        assert_eq!(join("", "A"), "A");
        assert_eq!(join("A", "B"), "A/B");
        assert_eq!(parent_of("A/B/C"), "A/B");
        assert_eq!(parent_of("A"), "");
        assert_eq!(last_part("A/B/C"), "C");
        assert_eq!(last_part("C"), "C");
        assert!(at_or_under("A/B", "A") && at_or_under("A", "A") && at_or_under("A", ""));
        assert!(!at_or_under("AB", "A"));
        assert_eq!(crumbs("A/B"), vec![("A".to_string(), "A".to_string()), ("B".to_string(), "A/B".to_string())]);
        assert!(crumbs("").is_empty());
        assert_eq!(place("Library", "A/B"), "Library › A › B");
        assert_eq!(place("Library", ""), "Library");
        // the path box sends what was typed: the server refuses `..` and absolute paths itself
        assert_eq!(typed_path("  ../etc/ "), "../etc");
        assert_eq!(typed_path("/etc"), "/etc");
        assert_eq!(typed_path("Artist/Album/"), "Artist/Album");
    }

    #[test]
    fn names_follow_the_servers_rule() {
        assert_eq!(name_problem(""), Some("A name can't be empty"));
        assert_eq!(name_problem("  "), Some("A name can't be empty"));
        assert_eq!(name_problem("a/b"), Some("A name can't contain / or \\"));
        assert_eq!(name_problem("a\\b"), Some("A name can't contain / or \\"));
        assert_eq!(name_problem(".."), Some("That name isn't allowed"));
        assert_eq!(name_problem(&"x".repeat(256)), Some("That name is too long"));
        assert_eq!(name_problem("song\u{202e}calf.exe"), Some("A name can't contain invisible or control characters"));
        assert_eq!(name_problem("tab\there"), Some("A name can't contain invisible or control characters"));
        assert_eq!(name_problem("01 Crazy.flac"), None);
        assert_eq!(name_problem("Björk – Début"), None);
        assert_eq!(upload_name("Album/CD1/01 Song.flac"), "Album/CD1/01 Song.flac");
        assert_eq!(upload_name("/Album//a\\b\u{7}.flac"), "Album/a_b.flac");
        assert_eq!(stem_len("01 Song.flac", false), 7);
        assert_eq!(stem_len(".hidden", false), 7);
        assert_eq!(stem_len("Album.2006", true), 10);
        assert_eq!(stem_len("𝄞 Song.flac", false), 7);
        assert!(is_audio_name("x.FLAC") && is_audio_name("a.b.mp3") && !is_audio_name("cover.jpg") && !is_audio_name("flac"));
    }

    #[test]
    fn entries_say_what_they_are() {
        let link = json!({"name": "Music", "kind": "link", "target": "/srv/music", "target_kind": "dir"});
        assert!(is_dir(&link) && is_link(&link) && !is_audio(&link));
        assert_eq!(entry_line(&link), "Link to /srv/music");
        assert_eq!(entry_line(&json!({"kind": "link", "target_kind": "missing"})), "Link to somewhere else");
        let song = json!({"name": "01 Crazy.flac", "kind": "file", "audio": {"format": "flac", "duration": 178.4, "title": "Crazy", "artist": "Gnarls Barkley", "track_id": 812}});
        assert!(is_audio(&song) && !is_dir(&song));
        assert_eq!(track_id(&song), 812);
        assert_eq!(entry_line(&song), "Gnarls Barkley – Crazy · 2:58");
        let loose = json!({"name": "02.flac", "kind": "file", "audio": {"format": "flac"}});
        assert!(is_audio(&loose));
        assert_eq!(track_id(&loose), 0);
        assert_eq!(entry_line(&loose), "");
        assert!(is_dir(&json!({"kind": "dir"})));
    }

    #[test]
    fn sorting_reads_and_turns_round() {
        assert_eq!(sort_button("name", "asc"), "Sort: Name ↑");
        assert_eq!(sort_button("mtime", "desc"), "Sort: Date modified ↓");
        assert_eq!(order_text("size", "asc"), "smallest first");
        assert_eq!(order_text("mtime", "desc"), "newest first");
        assert_eq!(order_text("name", "desc"), "Z–A");
        assert_eq!(next_sort(("name", "asc"), "name"), ("name".to_string(), "desc".to_string()));
        assert_eq!(next_sort(("name", "asc"), "size"), ("size".to_string(), "desc".to_string()));
        assert_eq!(next_sort(("size", "desc"), "name"), ("name".to_string(), "asc".to_string()));
    }

    #[test]
    fn the_selection_works_like_a_file_manager() {
        let mut s = Sel::new(6);
        s.click(1, false, false);
        assert_eq!(s.picked(), vec![1]);
        // Shift-click: from the anchor to here
        s.click(4, false, true);
        assert_eq!(s.picked(), vec![1, 2, 3, 4]);
        // Shift-click the other way replaces the range, from the same anchor
        s.click(0, false, true);
        assert_eq!(s.picked(), vec![0, 1]);
        // Ctrl-click adds and takes out, and moves the anchor
        s.click(5, true, false);
        assert_eq!(s.picked(), vec![0, 1, 5]);
        s.click(1, true, false);
        assert_eq!(s.picked(), vec![0, 5]);
        // Ctrl+Shift adds a range to what is picked
        s.click(3, true, true);
        assert_eq!(s.picked(), vec![0, 1, 2, 3, 5]);
        // a plain click is only that row
        s.click(2, false, false);
        assert_eq!(s.picked(), vec![2]);
        // the arrows move, Shift extends
        s.step(1, false);
        assert_eq!((s.picked(), s.cursor), (vec![3], Some(3)));
        s.step(1, true);
        s.step(1, true);
        assert_eq!(s.picked(), vec![3, 4, 5]);
        s.step(1, true);
        assert_eq!(s.picked(), vec![3, 4, 5]);
        s.step(-10, false);
        assert_eq!(s.picked(), vec![0]);
        let mut e = Sel::new(3);
        e.step(-1, false);
        assert_eq!(e.picked(), vec![2]);
        e.all();
        assert_eq!(e.count(), 3);
        e.clear();
        assert_eq!(e.count(), 0);
        // out of range does nothing
        e.click(9, false, false);
        assert_eq!(e.count(), 0);
        let names = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let k = Sel::keep(&names(&["b", "x"]), &names(&["a", "b", "c"]));
        assert_eq!((k.picked(), k.cursor), (vec![1], Some(1)));
        assert_eq!(jump_to(&names(&["Abba", "beta", "Bravo"]), Some(1), "b"), Some(2));
        assert_eq!(jump_to(&names(&["Abba", "beta", "Bravo"]), Some(2), "b"), Some(1));
        assert_eq!(jump_to(&names(&["Abba"]), None, "z"), None);
    }

    #[test]
    fn results_read_as_one_line() {
        assert_eq!(failure_line(&json!({"moved": 2, "failed": []})), "");
        assert_eq!(failure_line(&json!({"failed": [{"path": "A/01.flac", "reason": "A file with that name is already there"}]})), "01.flac: A file with that name is already there");
        assert_eq!(failure_line(&json!({"failed": [{"id": "1792300000-1", "reason": "Its folder is no longer one Encore may use"}, {"id": "x", "reason": "y"}]})),
            "2 weren't done. 1792300000-1: Its folder is no longer one Encore may use");
        assert_eq!(done_line("move", 1), "Moved");
        assert_eq!(done_line("move", 3), "Moved 3 items");
        assert_eq!(done_line("trash", 1), "Moved to trash");
        assert_eq!(done_line("trash", 2), "Moved 2 items to trash");
        assert_eq!(done_line("restore", 1), "Restored");
        assert_eq!(upload_end(1, 1, false, ""), ("Uploaded 1 file".to_string(), "ok"));
        assert_eq!(upload_end(12, 12, false, ""), ("Uploaded 12 files".to_string(), "ok"));
        assert_eq!(upload_end(3, 12, true, ""), ("Upload stopped after 3 of 12".to_string(), "info"));
        assert_eq!(upload_end(11, 12, false, "02.flac: A folder with that name is already there"), ("Uploaded 11 of 12. 02.flac: A folder with that name is already there".to_string(), "err"));
        assert_eq!(upload_outcome(&json!({"uploaded": [{"name": "a.flac"}], "renamed": [], "skipped": []})), Ok(None));
        assert_eq!(upload_outcome(&json!({"uploaded": [{"name": "a.flac"}], "renamed": [{"from": "a.flac", "to": "a (2).flac"}]})), Ok(Some("a (2).flac".to_string())));
        assert_eq!(upload_outcome(&json!({"skipped": [{"name": "a.flac", "reason": "A file with that name is already there"}]})), Err("A file with that name is already there".to_string()));
        assert_eq!(import_button(1), "Import 1 album");
        assert_eq!(import_button(4), "Import 4 albums");
    }

    #[test]
    fn releases_and_confidence() {
        let r = json!({"id": "x", "title": "St. Elsewhere", "artist": "Gnarls Barkley", "date": "2006-05-08", "provider": "MusicBrainz", "score": 92});
        assert_eq!(release_line(&r), "Gnarls Barkley – St. Elsewhere (2006) · MusicBrainz");
        assert!((match_of(&r) - 0.92).abs() < 1e-9);
        assert!((match_of(&json!({"score": 92, "match": 0.5})) - 0.5).abs() < 1e-9);
        assert!((album_confidence(&json!({"confidence": 0.0, "release": r})) - 0.92).abs() < 1e-9);
        assert_eq!(album_confidence(&json!({"release": null})), 0.0);
        assert_eq!(conf_class(0.9), "ok");
        assert_eq!(conf_class(0.7), "warn");
        assert_eq!(conf_class(0.2), "err");
        assert_eq!(trash_from("Library", "Artist/Album"), "From Library › Artist");
        assert_eq!(trash_from("Staging", "Album"), "From Staging");
    }

    /// The folder's filter and where a search result goes.
    #[test]
    fn search_filters_and_jumps() {
        let rows = vec![json!({"name": "Blur – Parklife", "kind": "dir"}), json!({"name": "01 Girls & Boys.flac", "kind": "file"}),
                        json!({"id": "1-1", "path": "Blur/Old Cover.jpg", "name": ""})];
        let names = |v: Vec<Value>| v.iter().map(|e| if e.s("name").is_empty() { e.s("path") } else { e.s("name") }).collect::<Vec<_>>();
        assert_eq!(filter_rows(&rows, "  ").len(), 3, "a blank search shows everything");
        assert_eq!(names(filter_rows(&rows, "PARK blur")), ["Blur – Parklife"], "every word, any order, any case");
        assert_eq!(names(filter_rows(&rows, "cover")), ["Blur/Old Cover.jpg"], "a trash item by its path's last part");
        assert!(filter_rows(&rows, "blur nope").is_empty());
        let file = json!({"name": "03 Teardrop.flac", "kind": "file", "path": "Massive Attack/Mezzanine/03 Teardrop.flac", "parent": "Massive Attack/Mezzanine"});
        assert_eq!(hit_target(&file), ("Massive Attack/Mezzanine".to_string(), vec!["03 Teardrop.flac".to_string()]));
        let dir = json!({"name": "Mezzanine", "kind": "dir", "path": "Massive Attack/Mezzanine", "parent": "Massive Attack"});
        assert_eq!(hit_target(&dir), ("Massive Attack/Mezzanine".to_string(), vec![]));
        assert!(hit_is_dir(&json!({"kind": "link", "target_kind": "dir"})) && !hit_is_dir(&json!({"kind": "link", "target_kind": "file"})));
        assert_eq!(found_line(1, false, "Library"), "1 found in Library");
        assert_eq!(found_line(3, false, "Library"), "3 found in Library");
        assert_eq!(found_line(200, true, "Library"), "200+ found in Library (the nearest first; type more to narrow it)");
        assert_eq!(found_line(0, false, "Staging"), "Nothing in Staging is called that");
    }
}
