//! /api/changelog: the release notes (docs/releases/<version>.md, built in by build.rs) for the What's new popup
//! the web and desktop apps show on their first start after an update, and for Settings → What's new.
//!
//! What counts as new is per account: `prefs.whats_new_seen` holds the version the person last saw, and only
//! releases with notes newer than it (and not newer than this server) are new. Hotfix builds (0.36.0.17) share
//! their release's notes, so a hotfix alone never shows the popup again. `prefs.whats_new_popup = false` turns
//! the popup off; Settings → What's new still lists everything.
use crate::auth::User;
use crate::state::AppState;
use axum::extract::{Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use encore_core::error::ApiResult;
use encore_core::version;
use encore_db::{p, RowExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::OnceLock;

/// The release notes the build script embeds ([`generated::NOTES`]).
mod generated {
    include!(concat!(env!("OUT_DIR"), "/changelog_gen.rs"));
}

/// The popup shows at most this many releases (someone who skipped many updates gets the newest few and the
/// link to the full changelog).
pub const POPUP_RELEASES: usize = 4;
/// Highlights per release in the popup.
pub const HIGHLIGHTS: usize = 6;

/// The What's new routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/changelog", get(all))
        .route("/api/changelog/new", get(new))
        .route("/api/changelog/seen", post(seen))
}

/// One part of a release's notes, inline markdown taken out (bold, italics, code, links keep their words).
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "k", rename_all = "lowercase")]
pub enum Block {
    /// `### Library`
    H {
        /// the heading's text
        text: String,
    },
    /// A paragraph.
    P {
        /// the paragraph's text
        text: String,
    },
    /// A bullet; `lead` is its bold start ("Delete albums by type."), when it has one.
    Li {
        /// the bold start, or ""
        #[serde(skip_serializing_if = "String::is_empty")]
        lead: String,
        /// the rest of the bullet
        text: String,
    },
    /// A table row; `head` for the first.
    Row {
        /// the cells' texts
        cells: Vec<String>,
        /// the header row
        head: bool,
    },
}

/// One release's notes.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Release {
    /// its version ("0.36.0")
    pub version: String,
    /// "Encore 0.36.0", or what follows the version in the heading ("Encore 2 (Rust) is here").
    pub title: String,
    /// The paragraph before the first section.
    pub summary: String,
    /// What the popup lists: the bold leads of the bullets, else the first bullets.
    pub highlights: Vec<String>,
    /// the notes, part by part
    pub blocks: Vec<Block>,
}

/// Every release with notes, newest first.
pub fn releases() -> &'static [Release] {
    static ALL: OnceLock<Vec<Release>> = OnceLock::new();
    ALL.get_or_init(|| {
        let mut v: Vec<Release> = generated::NOTES.iter().map(|(ver, md)| parse(ver, md)).collect();
        v.sort_by(|a, b| version::compare(&b.version, &a.version));
        v
    })
}

/// MAJOR.MINOR.PATCH of a version: a hotfix 0.36.0.17 is release 0.36.0.
pub fn base(v: &str) -> String {
    let p = version::parts(v);
    (0..3).map(|i| p.get(i).copied().unwrap_or(0).to_string()).collect::<Vec<_>>().join(".")
}

/// The releases this server (`current`) includes, newest first; with `seen`, only those newer than it. Without
/// `seen` (an account that never saw the popup) only the newest one.
pub fn unseen<'a>(all: &'a [Release], current: &str, seen: Option<&str>) -> Vec<&'a Release> {
    let cur = base(current);
    let mut out: Vec<&Release> = all.iter().filter(|r| version::compare(&r.version, &cur) != std::cmp::Ordering::Greater).collect();
    match seen.filter(|s| !version::parts(s).is_empty()) {
        Some(s) => {
            let s = base(s);
            out.retain(|r| version::newer(&r.version, &s));
            out.truncate(POPUP_RELEASES);
        }
        None => out.truncate(1),
    }
    out
}

/// Bold, italics, code and links out of one line of markdown: `**Pause for everyone** is in [x](y)` → `Pause for
/// everyone is in x`.
pub fn plain(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let c: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < c.len() {
        match c[i] {
            '*' | '`' => {}
            '_' if (i == 0 || !c[i - 1].is_alphanumeric()) || (i + 1 == c.len() || !c[i + 1].is_alphanumeric()) => {}
            '[' => {
                // [words](url) → words
                if let Some(close) = c[i..].iter().position(|&x| x == ']').map(|p| i + p) {
                    if c.get(close + 1) == Some(&'(') {
                        if let Some(end) = c[close..].iter().position(|&x| x == ')').map(|p| close + p) {
                            out.push_str(&plain(&c[i + 1..close].iter().collect::<String>()));
                            i = end + 1;
                            continue;
                        }
                    }
                }
                out.push('[');
            }
            x => out.push(x),
        }
        i += 1;
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A bullet's bold start and the rest: `**Delete albums by type.** Library → …` → ("Delete albums by type.",
/// "Library → …").
fn lead(text: &str) -> (String, String) {
    if let Some(rest) = text.strip_prefix("**") {
        if let Some(end) = rest.find("**") {
            let lead = plain(&rest[..end]);
            let after = rest[end + 2..].trim_start_matches([':', ' ']).trim_start_matches(['–', '-']).trim();
            return (lead, plain(after));
        }
    }
    (String::new(), plain(text))
}

/// `s` cut to about `max` characters, ending on a sentence or a word, with "…".
fn short(s: &str, max: usize) -> String {
    if s.chars().count() <= max { return s.to_string(); }
    let cut: String = s.chars().take(max).collect();
    // end on a sentence when there is one, else on a word
    if let Some(p) = cut.rfind(". ").filter(|&p| p > max / 2) { return cut[..=p].to_string(); }
    match cut.rfind(' ') { Some(p) => format!("{}…", cut[..p].trim_end_matches([',', ';', ':'])), None => format!("{cut}…") }
}

/// One release's markdown into its parts.
pub fn parse(ver: &str, md: &str) -> Release {
    enum Open { None, P(String), Li(String) }
    let mut blocks = Vec::new();
    let mut title = format!("Encore {ver}");
    let mut open = Open::None;
    let mut in_table = false;
    let flush = |open: &mut Open, blocks: &mut Vec<Block>| {
        match std::mem::replace(open, Open::None) {
            Open::P(t) if !t.trim().is_empty() => blocks.push(Block::P { text: plain(&t) }),
            Open::Li(t) => { let (lead, text) = lead(t.trim()); blocks.push(Block::Li { lead, text }); }
            _ => {}
        }
    };
    for line in md.lines() {
        let t = line.trim();
        if t.is_empty() { flush(&mut open, &mut blocks); in_table = false; continue; }
        if let Some(h) = t.strip_prefix("## ") {
            flush(&mut open, &mut blocks);
            let h = plain(h);
            // "Encore 2026.10.11.1 – Encore 2 (Rust) is here" → what follows the version
            title = match h.split_once(" – ") { Some((_, rest)) if !rest.is_empty() => rest.to_string(), _ => h };
            continue;
        }
        if let Some(h) = t.strip_prefix("### ").or_else(|| t.strip_prefix("#### ")) {
            flush(&mut open, &mut blocks);
            blocks.push(Block::H { text: plain(h) });
            continue;
        }
        if t == "---" { flush(&mut open, &mut blocks); continue; }
        if t.starts_with('|') {
            flush(&mut open, &mut blocks);
            let cells: Vec<String> = t.trim_matches('|').split('|').map(|c| plain(c.trim())).collect();
            // the |---|---| line under the head
            if cells.iter().all(|c| c.chars().all(|x| matches!(x, '-' | ':' | ' '))) { continue; }
            blocks.push(Block::Row { cells, head: !in_table });
            in_table = true;
            continue;
        }
        let bullet = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")).or_else(|| {
            // "1. text"
            let d = t.chars().take_while(|c| c.is_ascii_digit()).count();
            (d > 0).then(|| t[d..].strip_prefix(". ")).flatten()
        });
        match (bullet, &mut open) {
            (Some(b), _) => { flush(&mut open, &mut blocks); open = Open::Li(b.to_string()); }
            (None, Open::Li(s)) | (None, Open::P(s)) => { s.push(' '); s.push_str(t); }
            (None, Open::None) => open = Open::P(t.to_string()),
        }
    }
    flush(&mut open, &mut blocks);

    let summary = blocks.iter().take_while(|b| !matches!(b, Block::H { .. }))
        .find_map(|b| match b { Block::P { text } => Some(short(text, 320)), _ => None }).unwrap_or_default();
    let mut highlights: Vec<String> = blocks.iter().filter_map(|b| match b {
        Block::Li { lead, .. } if !lead.is_empty() => Some(short(lead.trim_end_matches(['.', ':']), 110)),
        _ => None,
    }).take(HIGHLIGHTS).collect();
    if highlights.is_empty() {
        highlights = blocks.iter().filter_map(|b| match b { Block::Li { text, .. } => Some(short(text, 110)), _ => None }).take(4).collect();
    }
    Release { version: ver.to_string(), title, summary, highlights, blocks }
}

/// A user's preferences.
fn prefs(u: &encore_db::Row) -> Value {
    serde_json::from_str(&u.s("prefs")).unwrap_or(json!({}))
}

/// The popup is on unless the person switched it off.
fn popup_on(prefs: &Value) -> bool {
    prefs.get("whats_new_popup").and_then(Value::as_bool).unwrap_or(true)
}

/// `GET /api/changelog` query.
#[derive(Deserialize)]
struct AllQuery {
    /// how many releases (default: all)
    #[serde(default)]
    limit: Option<usize>,
}

/// Settings → What's new: every release this server includes, newest first.
async fn all(User(u): User, Query(q): Query<AllQuery>) -> Json<Value> {
    let cur = base(crate::VERSION);
    let mut list: Vec<&Release> = releases().iter().filter(|r| version::compare(&r.version, &cur) != std::cmp::Ordering::Greater).collect();
    if let Some(n) = q.limit { list.truncate(n); }
    let p = prefs(&u);
    Json(json!({
        "version": crate::VERSION,
        "popup": popup_on(&p),
        "seen": p.get("whats_new_seen").cloned().unwrap_or(Value::Null),
        "releases": list,
    }))
}

/// The popup: `show` when it is on and something is new since this account last saw it.
async fn new(User(u): User) -> Json<Value> {
    let p = prefs(&u);
    let on = popup_on(&p);
    let list = unseen(releases(), crate::VERSION, p.get("whats_new_seen").and_then(Value::as_str));
    let list: Vec<Value> = list.into_iter().map(|r| {
        json!({ "version": r.version, "title": r.title, "summary": r.summary, "highlights": r.highlights })
    }).collect();
    Json(json!({ "version": crate::VERSION, "popup": on, "show": on && !list.is_empty(), "releases": list }))
}

/// The popup was shown (or closed): this version is seen.
async fn seen(State(st): State<AppState>, User(u): User) -> ApiResult<Json<Value>> {
    let mut pr = prefs(&u);
    if let Some(o) = pr.as_object_mut() { o.insert("whats_new_seen".into(), json!(crate::VERSION)); }
    st.db.execute("UPDATE users SET prefs=? WHERE id=?", p![pr.to_string(), u.i("id")])?;
    Ok(Json(json!({ "ok": true, "seen": crate::VERSION })))
}

#[cfg(test)]
#[path = "changelog_tests.rs"]
mod changelog_tests;
