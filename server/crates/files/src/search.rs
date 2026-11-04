//! Search by name (`GET /api/files/search`): every file and folder under a folder of a root whose name holds
//! every word typed, at any depth. The file manager's search box asks for it as the admin types, beside the
//! filter of the folder that is open (which the client does on its own).
//!
//! The rules:
//! - names are compared folded ([`encore_library::artists::fold`]: lowercase, accents dropped, punctuation
//!   read as spaces), and every word of the query must appear in the name, in any order ("beyonce lemonade"
//!   finds "Lemonade (Beyoncé)");
//! - the walk is breadth-first, so the nearest matches come first and are the ones kept when there are more
//!   than the answer may hold;
//! - a link is found by its own name but never walked into: a search can't leave the roots or go round a
//!   loop of links (the folder searched from may itself be reached through a link into a root, as browsing
//!   allows, see [`browse_dir`]);
//! - Encore's own config folder is never walked ([`crate::roots::Root::fence`]), and names that aren't UTF-8
//!   are left out (a client couldn't name them back);
//! - a walk stops after [`MAX_SCANNED`] entries, and says so (`truncated`), so one search can't hold a
//!   blocking thread for long on a huge disk.
use crate::names;
use crate::resolve::{browse_dir, Target};
use crate::roots::Root;
use encore_core::error::{err, ApiError};
use encore_library::artists::fold;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::UNIX_EPOCH;

/// The most matches one answer has when the client doesn't say.
pub const LIMIT: usize = 200;
/// The most matches a client may ask for.
pub const MAX_LIMIT: usize = 1000;
/// The most entries one search looks at before it stops (and answers `truncated`).
pub const MAX_SCANNED: usize = 250_000;
/// The longest query taken, in characters (a longer one is a mistake, not a name).
pub const MAX_QUERY: usize = 200;

/// The words a name must hold, folded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Terms(Vec<String>);

impl Terms {
    /// The query's words, folded; 400 when nothing is left to look for, or the query is too long.
    pub fn parse(q: &str) -> Result<Terms, ApiError> {
        if q.chars().count() > MAX_QUERY {
            return Err(err(400, format!("A search is at most {MAX_QUERY} characters")));
        }
        let words: Vec<String> = fold(q).split_whitespace().map(String::from).collect();
        if words.is_empty() {
            return Err(err(400, "Type a name to search for"));
        }
        Ok(Terms(words))
    }

    /// Whether `name` holds every word (folded, anywhere in it, in any order).
    pub fn matches(&self, name: &str) -> bool {
        let f = fold(name);
        self.0.iter().all(|w| f.contains(w.as_str()))
    }
}

/// One match, as the walk found it.
struct Found {
    /// its name
    name: String,
    /// "dir", "file" or "link"
    kind: &'static str,
    /// its path from the root (the path the client opens, links on the way kept as named)
    path: String,
    /// how many folders down from the folder searched (0 = in it)
    depth: usize,
    /// size in bytes (files; a link: its target's)
    size: u64,
    /// modification time, Unix seconds
    mtime: i64,
    /// a link's target kind: "dir", "file" or "missing"
    target_kind: Option<&'static str>,
}

impl Found {
    /// A folder, or a link to one (what a click opens rather than shows in its folder).
    fn is_dir(&self) -> bool {
        self.kind == "dir" || self.target_kind == Some("dir")
    }

    /// The answer's entry: `{name, kind, path, parent, size, mtime, image}`, a link's `target_kind`, and
    /// `audio: {format}` for an audio file.
    fn json(&self) -> Value {
        let parent = self.path.rsplit_once('/').map(|(p, _)| p).unwrap_or("");
        let mut v = json!({ "name": self.name, "kind": self.kind, "path": self.path, "parent": parent, "size": self.size,
                            "mtime": self.mtime, "image": !self.is_dir() && names::is_image(&self.name) });
        if let Some(tk) = self.target_kind {
            v["target_kind"] = json!(tk);
        }
        if self.kind == "file" && names::is_audio(&self.name) {
            v["audio"] = json!({ "format": names::ext_of(&self.name) });
        }
        v
    }
}

/// The modification time in Unix seconds (0 when unknown).
fn secs(m: &std::fs::Metadata) -> i64 {
    m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// "a" + "b" → "a/b"; "" + "b" → "b".
fn join(path: &str, name: &str) -> String {
    if path.is_empty() { name.to_string() } else { format!("{path}/{name}") }
}

/// The match's details: size, time and, for a link, what it points at (followed only to look).
fn found(entry: &std::fs::DirEntry, name: String, path: String, depth: usize, is_link: bool, is_dir: bool) -> Found {
    let meta = entry.metadata().ok();
    let mtime = meta.as_ref().map(secs).unwrap_or(0);
    if is_link {
        let (tk, size) = match std::fs::metadata(entry.path()) {
            Ok(m) if m.is_dir() => ("dir", 0),
            Ok(m) => ("file", m.len()),
            Err(_) => ("missing", 0),
        };
        return Found { name, kind: "link", path, depth, size, mtime, target_kind: Some(tk) };
    }
    if is_dir {
        return Found { name, kind: "dir", path, depth, size: 0, mtime, target_kind: None };
    }
    Found { name, kind: "file", path, depth, size: meta.map(|m| m.len()).unwrap_or(0), mtime, target_kind: None }
}

/// `GET /api/files/search`: the matches under `t` (the root itself for `path` ""), at most `limit`:
/// `{root, path, q, results, truncated, scanned}`. `results` are the nearest first, folders before files at
/// each depth, then by path (case-insensitively); `truncated` says there may be more than were given.
pub fn search(roots: &[Root], t: &Target, q: &str, limit: Option<usize>) -> Result<Value, ApiError> {
    let terms = Terms::parse(q)?;
    let limit = limit.unwrap_or(LIMIT).clamp(1, MAX_LIMIT);
    let (start, _) = browse_dir(t, roots)?;
    let (results, truncated, scanned) = walk(&t.root, start, &t.rel(), &terms, limit, MAX_SCANNED);
    Ok(json!({ "root": t.root.id, "path": t.rel(), "q": q, "results": results.iter().map(Found::json).collect::<Vec<_>>(),
               "truncated": truncated, "scanned": scanned }))
}

/// The breadth-first walk from `start` (whose path from the root is `rel`): the matches sorted, whether it
/// stopped early (`limit` matches, or `max_scanned` entries looked at), and how many entries it looked at.
fn walk(root: &Root, start: PathBuf, rel: &str, terms: &Terms, limit: usize, max_scanned: usize) -> (Vec<Found>, bool, usize) {
    let mut out: Vec<Found> = vec![];
    let mut scanned = 0usize;
    let mut truncated = false;
    // (the folder on the disk, its path from the root, its depth below the start)
    let mut queue: VecDeque<(PathBuf, String, usize)> = VecDeque::from([(start, rel.to_string(), 0)]);
    'walk: while let Some((dir, at, depth)) = queue.pop_front() {
        // a folder that can't be read (permissions, gone meanwhile) is passed over, not the whole search
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        // each folder's entries in name order, so the answer is the same every time
        let mut entries: Vec<(String, std::fs::DirEntry)> = rd.flatten()
            .filter_map(|e| e.file_name().to_str().map(String::from).map(|n| (n, e)))
            .collect();
        entries.sort_by(|a, b| (a.0.to_lowercase(), &a.0).cmp(&(b.0.to_lowercase(), &b.0)));
        for (name, e) in entries {
            scanned += 1;
            if scanned > max_scanned {
                truncated = true;
                break 'walk;
            }
            // the entry's own type (links not followed); one that can't be told is passed over
            let Ok(ft) = e.file_type() else { continue };
            let path = join(&at, &name);
            if ft.is_dir() && !root.fenced_off(&e.path()) {
                queue.push_back((e.path(), path.clone(), depth + 1));
            } else if ft.is_dir() {
                // Encore's config folder: neither listed nor walked
                continue;
            }
            if terms.matches(&name) {
                if out.len() == limit {
                    truncated = true;
                    break 'walk;
                }
                out.push(found(&e, name, path, depth, ft.is_symlink(), ft.is_dir()));
            }
        }
    }
    out.sort_by(|a, b| {
        a.depth.cmp(&b.depth)
            .then_with(|| b.is_dir().cmp(&a.is_dir()))
            .then_with(|| (a.path.to_lowercase(), &a.path).cmp(&(b.path.to_lowercase(), &b.path)))
    });
    (out, truncated, scanned.min(max_scanned))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::roots::RootKind;
    use std::path::Path;

    /// A root over `dir`.
    fn root(dir: &Path) -> Root {
        Root::new("library", "Library", dir.to_path_buf(), RootKind::Library)
    }

    /// The paths of an answer's results.
    fn paths(v: &Value) -> Vec<String> {
        v["results"].as_array().unwrap().iter().map(|r| r["path"].as_str().unwrap().to_string()).collect()
    }

    #[test]
    fn terms_fold_and_need_every_word() {
        let t = Terms::parse("  Beyonce   LEMONADE ").unwrap();
        assert!(t.matches("Lemonade (Beyoncé)"), "accents and case don't matter, nor the order");
        assert!(!t.matches("Lemonade"), "every word must be there");
        assert!(Terms::parse("ac/dc").unwrap().matches("AC-DC - Back in Black"), "punctuation reads as a space");
        assert_eq!(Terms::parse("  ").unwrap_err().status.as_u16(), 400);
        assert_eq!(Terms::parse("!!").unwrap_err().status.as_u16(), 400, "nothing left once folded");
        assert_eq!(Terms::parse(&"a".repeat(MAX_QUERY + 1)).unwrap_err().status.as_u16(), 400);
    }

    #[test]
    fn finds_names_at_any_depth_nearest_first() {
        let d = encore_core::tmp::dir("files-search");
        let r = d.join("root");
        std::fs::create_dir_all(r.join("Blur/Parklife")).unwrap();
        std::fs::create_dir_all(r.join("Other/Deep/Blur Live")).unwrap();
        std::fs::write(r.join("Blur/Parklife/01 Girls & Boys.flac"), "x").unwrap();
        std::fs::write(r.join("Blur/blur cover.jpg"), "x").unwrap();
        std::fs::write(r.join("Blur.txt"), "x").unwrap();
        let roots = vec![root(&r)];
        let t = Target::new(&roots[0], "").unwrap();
        let v = search(&roots, &t, "blur", None).unwrap();
        assert_eq!(paths(&v), ["Blur", "Blur.txt", "Blur/blur cover.jpg", "Other/Deep/Blur Live"], "{v}");
        assert_eq!((v["truncated"].as_bool(), v["root"].as_str()), (Some(false), Some("library")));
        let first = &v["results"][0];
        assert_eq!((first["kind"].as_str(), first["parent"].as_str()), (Some("dir"), Some("")));
        assert_eq!(v["results"][2]["parent"], "Blur");
        assert_eq!(v["results"][2]["image"], true);
        // a song, found by words of its name, with its format
        let v = search(&roots, &t, "girls boys", None).unwrap();
        assert_eq!(paths(&v), ["Blur/Parklife/01 Girls & Boys.flac"]);
        assert_eq!(v["results"][0]["audio"]["format"], "flac");
        // from a folder: only what is under it, paths still from the root
        let v = search(&roots, &Target::new(&roots[0], "Other").unwrap(), "blur", None).unwrap();
        assert_eq!(paths(&v), ["Other/Deep/Blur Live"]);
        // the limit keeps the nearest and says there were more
        let v = search(&roots, &t, "blur", Some(2)).unwrap();
        assert_eq!((paths(&v), v["truncated"].as_bool()), (vec!["Blur".to_string(), "Blur.txt".to_string()], Some(true)));
        assert_eq!(search(&roots, &Target::new(&roots[0], "Nothing").unwrap(), "blur", None).unwrap_err().status.as_u16(), 404);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn links_are_found_but_never_walked_and_the_config_folder_is_fenced_off() {
        let d = encore_core::tmp::dir("files-search-links");
        let (r, out) = (d.join("root"), d.join("outside"));
        std::fs::create_dir_all(r.join("A")).unwrap();
        std::fs::create_dir_all(r.join("config/backups")).unwrap();
        std::fs::create_dir_all(out.join("secret match")).unwrap();
        std::fs::write(r.join("config/match.db"), "x").unwrap();
        std::os::unix::fs::symlink(&out, r.join("match link")).unwrap();
        // a loop: A/loop → the root
        std::os::unix::fs::symlink(&r, r.join("A/loop")).unwrap();
        let roots = vec![root(&r).fenced(&r.join("config"), &r.join("config/trash"))];
        let t = Target::new(&roots[0], "").unwrap();
        let v = search(&roots, &t, "match", None).unwrap();
        assert_eq!(paths(&v), ["match link"], "the link is found by name; nothing behind it or in the config folder: {v}");
        assert_eq!((v["results"][0]["kind"].as_str(), v["results"][0]["target_kind"].as_str()), (Some("link"), Some("dir")));
        assert_eq!(paths(&search(&roots, &t, "loop", None).unwrap()), ["A/loop"], "a loop of links ends");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_walk_stops_after_its_budget() {
        let d = encore_core::tmp::dir("files-search-budget");
        let r = d.join("root");
        std::fs::create_dir_all(&r).unwrap();
        for i in 0..10 {
            std::fs::write(r.join(format!("song {i}.flac")), "x").unwrap();
        }
        let root = root(&r);
        let (found, truncated, scanned) = walk(&root, r.clone(), "", &Terms::parse("song").unwrap(), 100, 4);
        assert_eq!((found.len(), truncated, scanned), (4, true, 4));
        let (found, truncated, scanned) = walk(&root, r, "", &Terms::parse("song").unwrap(), 100, 100);
        assert_eq!((found.len(), truncated, scanned), (10, false, 10));
        let _ = std::fs::remove_dir_all(&d);
    }
}
