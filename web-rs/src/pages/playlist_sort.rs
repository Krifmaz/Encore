//! Sorting a playlist's songs (a playlist page, Liked Songs and someone else's playlist added from a link).
//!
//! The Sort select and the ↑/↓ button above the song list reorder the rows on the page only: the playlist's own
//! order on the server never changes, so *Custom order* always brings it back. The pick is kept per playlist in
//! this browser (`encore.plSort`, `{ "<playlist id>": { "sort": "title", "dir": "asc" } }`), and Play, Shuffle
//! and ⋯ → Play next / Add to queue follow the order shown. The Android app has the same rules in
//! `PlaylistSortRules.kt`. The ordering itself is pure ([`order`]), so the unit tests check it.

use crate::dom;
use crate::json::J;
use serde_json::{json, Value};
use std::cmp::Ordering;
use std::rc::Rc;
use web_sys::Element;

/// The browser storage key holding every playlist's sort.
const STORE_KEY: &str = "encore.plSort";

/// The sorts a playlist offers: the key kept in storage and the select's label. The first is the default.
pub const SORTS: &[(&str, &str)] = &[
    ("custom", "Custom order"),
    ("added", "Date added"),
    ("title", "Title"),
    ("artist", "Artist"),
    ("album", "Album"),
    ("duration", "Duration"),
];

/// Whether `key` starts newest / longest first when it is picked (date added and duration); the rest start A–Z.
pub fn desc_first(key: &str) -> bool { matches!(key, "added" | "duration") }

/// A playlist's sort: one of the [`SORTS`] keys and its direction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlSort {
    /// One of the [`SORTS`] keys ("custom", "added", "title" …).
    pub key: String,
    /// `true` for descending (Z–A, newest first, longest first, the playlist's order reversed).
    pub desc: bool,
}

impl Default for PlSort {
    /// The playlist's own order, first song first.
    fn default() -> Self { Self { key: "custom".into(), desc: false } }
}

impl PlSort {
    /// `key` in the direction it starts in when picked from the select (see [`desc_first`]).
    pub fn picked(key: &str) -> Self { Self { key: key.into(), desc: desc_first(key) } }

    /// The same sort the other way round (the ↑/↓ button).
    pub fn reversed(&self) -> Self { Self { key: self.key.clone(), desc: !self.desc } }

    /// Read back from its stored JSON; anything unknown or malformed is the default.
    pub fn from_json(v: &Value) -> Self {
        let key = v.s("sort");
        if !SORTS.iter().any(|(k, _)| *k == key) {
            return Self::default();
        }
        Self { key, desc: v.s("dir") == "desc" }
    }

    /// The stored JSON (`{ "sort": "title", "dir": "asc" }`).
    pub fn to_json(&self) -> Value { json!({ "sort": self.key, "dir": if self.desc { "desc" } else { "asc" } }) }

    /// The ↑/↓ button's arrow.
    pub fn arrow(&self) -> &'static str { if self.desc { "↓" } else { "↑" } }

    /// The ↑/↓ button's tooltip and accessible name, in words that fit the sort ("A to Z", "Newest first" …).
    pub fn dir_label(&self) -> &'static str {
        match (self.key.as_str(), self.desc) {
            ("custom", false) => "Playlist order",
            ("custom", true) => "Playlist order, reversed",
            ("added", false) => "Oldest first",
            ("added", true) => "Newest first",
            ("duration", false) => "Shortest first",
            ("duration", true) => "Longest first",
            (_, false) => "A to Z",
            (_, true) => "Z to A",
        }
    }
}

/// The sort kept for playlist `pid` in `saved` (the whole `encore.plSort` object), or the default.
pub fn saved_sort(saved: &Value, pid: &str) -> PlSort { PlSort::from_json(saved.g(pid)) }

/// `saved` with playlist `pid`'s sort set to `sort`. The default is removed instead of stored, so the object only
/// holds the playlists someone actually sorted.
pub fn with_sort(mut saved: Value, pid: &str, sort: &PlSort) -> Value {
    if !saved.is_object() {
        saved = json!({});
    }
    if let Some(o) = saved.as_object_mut() {
        if *sort == PlSort::default() {
            o.remove(pid);
        } else {
            o.insert(pid.to_string(), sort.to_json());
        }
    }
    saved
}

/// The text a row is sorted by for a text sort, lowercased so case doesn't split the list; `None` when the row has
/// none, so empty rows go last in either direction. The title is the one shown (translated or romanized when the
/// listener turned that on and the server sent it).
fn text_key(it: &Value, key: &str) -> Option<String> {
    let s = match key {
        "title" => it.st("title_t").map(str::to_string).unwrap_or_else(|| it.s("title")),
        other => it.s(other),
    };
    let s = s.trim().to_lowercase();
    (!s.is_empty()).then_some(s)
}

/// The number a row is sorted by for date added and duration; `None` when unknown (0 or missing), so those go last.
fn num_key(it: &Value, key: &str) -> Option<f64> {
    let field = if key == "added" { "added_at" } else { "duration" };
    it.num(field).filter(|n| *n > 0.0)
}

/// Compares two optional keys with missing ones last whatever the direction; `desc` flips only present keys.
fn cmp_opt<T>(a: Option<T>, b: Option<T>, desc: bool, cmp: impl Fn(&T, &T) -> Ordering) -> Ordering {
    match (a, b) {
        (Some(a), Some(b)) => { let o = cmp(&a, &b); if desc { o.reverse() } else { o } }
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

/// The positions of `items` (the playlist's rows in its own order) in the order `sort` shows them. Rows that tie
/// (the same artist, say) keep the playlist's order among themselves, and rows without the value sorted by (no
/// album, unknown length) go last. Date added falls back to the playlist's order for rows that have no date.
pub fn order(items: &[Value], sort: &PlSort) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..items.len()).collect();
    match sort.key.as_str() {
        "added" | "duration" => idx.sort_by(|&a, &b| {
            cmp_opt(num_key(&items[a], &sort.key), num_key(&items[b], &sort.key), sort.desc, |x, y| x.total_cmp(y)).then(a.cmp(&b))
        }),
        "title" | "artist" | "album" => idx.sort_by(|&a, &b| {
            cmp_opt(text_key(&items[a], &sort.key), text_key(&items[b], &sort.key), sort.desc, |x, y| x.cmp(y)).then(a.cmp(&b))
        }),
        // custom (and anything unknown): the playlist's own order, reversed when descending
        _ => if sort.desc { idx.reverse() },
    }
    idx
}

/// The select of sorts and the ↑/↓ button that sit above a playlist's song list (ids `pl-sort` and `pl-dir`).
pub fn bar_html() -> String {
    let opts: String = SORTS.iter().map(|(k, l)| format!(r#"<option value="{k}">Sort: {l}</option>"#)).collect();
    format!(r#"<div class="row pl-sortbar" style="gap:6px;justify-content:flex-end;margin-bottom:8px">
      <select class="input" id="pl-sort" style="width:auto" aria-label="Sort songs">{opts}</select>
      <button class="btn sm dir-btn" id="pl-dir" type="button"></button></div>"#)
}

/// Wires the sort bar ([`bar_html`]) of playlist `pid`. Each song row in `view` carries `data-idx`, its position in
/// `items`; the rows are moved into the chosen order (their # renumbered 1, 2, 3 …) and `on_order` gets the new
/// order of positions, so Play and the queue follow it. The playlist's saved sort is applied at once.
pub fn wire(view: &Element, pid: &str, items: Rc<Vec<Value>>, on_order: Rc<dyn Fn(&[usize])>) {
    let (Some(sel), Some(dir)) = (dom::qs_in(view, "#pl-sort"), dom::qs_in(view, "#pl-dir")) else { return };
    let load = || dom::ls_get(STORE_KEY).and_then(|s| serde_json::from_str::<Value>(&s).ok()).unwrap_or_else(|| json!({}));
    let current = Rc::new(std::cell::RefCell::new(saved_sort(&load(), pid)));
    // put the rows in `sort`'s order, show it on the select and the button, and keep it for this playlist
    let apply: Rc<dyn Fn(PlSort)> = {
        let (view, sel, dir, pid, current) = (view.clone(), sel.clone(), dir.clone(), pid.to_string(), current.clone());
        Rc::new(move |sort: PlSort| {
            let ord = order(&items, &sort);
            if let Some(body) = dom::qs_in(&view, "tbody") {
                for (n, &i) in ord.iter().enumerate() {
                    // a row removed meanwhile (× on a song) is simply skipped
                    let Some(tr) = dom::qs_in(&body, &format!(r#"tr[data-idx="{i}"]"#)) else { continue };
                    if let Some(num) = dom::qs_in(&tr, "td.num") { dom::set_text(&num, &(n + 1).to_string()); }
                    let _ = body.append_child(&tr);
                }
            }
            dom::set_value(&sel, &sort.key);
            dom::set_text(&dir, sort.arrow());
            let _ = dir.set_attribute("title", sort.dir_label());
            let _ = dir.set_attribute("aria-label", sort.dir_label());
            dom::ls_set(STORE_KEY, &with_sort(load(), &pid, &sort).to_string());
            on_order(&ord);
            *current.borrow_mut() = sort;
        })
    };
    {
        let (a, sel2) = (apply.clone(), sel.clone());
        dom::on(&sel, "change", move |_| a(PlSort::picked(&dom::value(&sel2))));
    }
    {
        let (a, c) = (apply.clone(), current.clone());
        dom::on(&dir, "click", move |_| { let s = c.borrow().reversed(); a(s); });
    }
    let first = current.borrow().clone();
    apply(first);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A row with a title, artist, album, length and the time it was added.
    fn row(title: &str, artist: &str, album: &str, duration: f64, added: f64) -> Value {
        json!({ "title": title, "artist": artist, "album": album, "duration": duration, "added_at": added })
    }

    /// Three rows in playlist order: b (added last, longest), a (no album, unknown length), c.
    fn rows() -> Vec<Value> {
        vec![row("beta", "Zed", "Mid", 300.0, 30.0), row("Alpha", "abba", "", 0.0, 10.0), row("charlie", "Abba", "First", 120.0, 20.0)]
    }

    #[test]
    fn custom_order_is_the_playlists_and_reverses() {
        assert_eq!(order(&rows(), &PlSort::default()), vec![0, 1, 2]);
        assert_eq!(order(&rows(), &PlSort::default().reversed()), vec![2, 1, 0]);
    }

    #[test]
    fn text_sorts_ignore_case_and_keep_ties_in_playlist_order() {
        assert_eq!(order(&rows(), &PlSort::picked("title")), vec![1, 0, 2]);
        assert_eq!(order(&rows(), &PlSort::picked("title").reversed()), vec![2, 0, 1]);
        // "abba" and "Abba" tie: the playlist's order between them stays
        assert_eq!(order(&rows(), &PlSort::picked("artist")), vec![1, 2, 0]);
    }

    #[test]
    fn rows_without_the_value_go_last_both_ways() {
        assert_eq!(order(&rows(), &PlSort::picked("album")), vec![2, 0, 1]);
        assert_eq!(order(&rows(), &PlSort::picked("album").reversed()), vec![0, 2, 1]);
        assert_eq!(order(&rows(), &PlSort::picked("duration")), vec![0, 2, 1]);
        assert_eq!(order(&rows(), &PlSort::picked("duration").reversed()), vec![2, 0, 1]);
    }

    #[test]
    fn date_added_starts_newest_first() {
        assert_eq!(PlSort::picked("added"), PlSort { key: "added".into(), desc: true });
        assert_eq!(order(&rows(), &PlSort::picked("added")), vec![0, 2, 1]);
        assert_eq!(order(&rows(), &PlSort::picked("added").reversed()), vec![1, 2, 0]);
    }

    #[test]
    fn the_translated_title_is_the_one_sorted_by() {
        let items = vec![json!({ "title": "夜に駆ける", "title_t": "Racing into the Night" }), json!({ "title": "Monster" })];
        assert_eq!(order(&items, &PlSort::picked("title")), vec![1, 0]);
    }

    #[test]
    fn the_sort_is_kept_per_playlist_and_the_default_is_not_stored() {
        let saved = with_sort(json!({}), "7", &PlSort::picked("title").reversed());
        assert_eq!(saved, json!({ "7": { "sort": "title", "dir": "desc" } }));
        assert_eq!(saved_sort(&saved, "7"), PlSort { key: "title".into(), desc: true });
        assert_eq!(saved_sort(&saved, "8"), PlSort::default());
        assert_eq!(with_sort(saved, "7", &PlSort::default()), json!({}));
        // junk in storage is the default, never an error
        assert_eq!(saved_sort(&json!({ "7": { "sort": "bogus" } }), "7"), PlSort::default());
        assert_eq!(with_sort(json!("junk"), "1", &PlSort::picked("album")), json!({ "1": { "sort": "album", "dir": "asc" } }));
    }

    #[test]
    fn the_direction_button_says_what_it_does() {
        assert_eq!(PlSort::picked("added").dir_label(), "Newest first");
        assert_eq!(PlSort::picked("title").dir_label(), "A to Z");
        assert_eq!(PlSort::default().reversed().dir_label(), "Playlist order, reversed");
        assert_eq!(PlSort::picked("duration").arrow(), "↓");
    }
}
