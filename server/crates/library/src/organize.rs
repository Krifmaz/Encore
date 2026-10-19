//! Organize the library into folders from tags (your naming template, e.g. Artist/Album (Year)/01 - Title).
//! Files MusicBrainz couldn't tag still get sensible names: missing title/artist come from the file name or an
//! AcoustID fingerprint, missing album/track number/year from Deezer. The fixed tags are written into the file.
use encore_db::{Row, RowExt};
use crate::Ctx;
use crate::scan as library;
use crate::{acoustid, tagger};
use encore_db::p;
use lofty::file::TaggedFileExt;
use lofty::tag::{Accessor, ItemKey};
use regex::Regex;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

const COLS: &str = "SELECT id, path, title, artist, album, albumartist, track, disc, year, genre, album_id FROM tracks";

fn unknown(v: &Value) -> bool {
    let s = match v { Value::String(s) => s.trim().to_lowercase(), Value::Null => String::new(), x => x.to_string() };
    ["", "unknown artist", "unknown album", "_", "0"].contains(&s.as_str())
}

/// "Artist - Title", "01 - Title", "01. Artist - Title" → whatever the file name says.
pub fn from_filename(path: &Path) -> Row {
    let name = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
    let mut name = Regex::new(r"\s*\(\d+\)$").unwrap().replace(&name, "").to_string();
    let mut out = Row::new();
    if let Some(c) = Regex::new(r"^(\d{1,3})[\s.\-_]+(.*)$").unwrap().captures(&name.clone()) {
        out.insert("track".into(), json!(c[1].parse::<i64>().unwrap_or(0)));
        name = c[2].to_string();
    }
    if let Some((a, t)) = name.split_once(" - ") {
        out.insert("artist".into(), json!(a.trim()));
        out.insert("title".into(), json!(t.trim()));
    } else if !name.is_empty() {
        out.insert("title".into(), json!(name.trim()));
    }
    out
}

async fn deezer(st: &impl Ctx, artist: &str, title: &str) -> Row {
    let mut out = Row::new();
    let Ok(r) = encore_http::deezer::search(st, "track", &format!("artist:\"{artist}\" track:\"{title}\""), 3).send().await else { return out };
    let Ok(d) = r.json::<Value>().await else { return out };
    let Some(hit) = d["data"].get(0).cloned() else { return out };
    let Ok(alb) = async { encore_http::deezer::album(st, &hit["album"]["id"]).send().await?.json::<Value>().await }.await else { return out };
    let pos = alb["tracks"]["data"].as_array().and_then(|a| a.iter().position(|t| t["id"] == hit["id"])).map(|p| p + 1);
    out.insert("album".into(), hit["album"]["title"].clone());
    out.insert("albumartist".into(), if alb["artist"]["name"].is_string() { alb["artist"]["name"].clone() } else { json!(artist) });
    out.insert("year".into(), json!(alb["release_date"].as_str().unwrap_or("").chars().take(4).collect::<String>()));
    out.insert("track".into(), json!(pos));
    out.insert("genre".into(), alb["genres"]["data"][0]["name"].clone());
    out
}

/// What's actually written in the file (the library index guesses missing ones from folder names).
fn real_tags(path: &Path) -> Row {
    let mut out = Row::new();
    let Ok(f) = lofty::read_from_path(path) else { return out };
    let Some(tag) = f.primary_tag().or(f.first_tag()) else { return out };
    let g = |k: ItemKey| tag.get_string(k).map(|s| s.trim().to_string()).unwrap_or_default();
    out.insert("title".into(), json!(g(ItemKey::TrackTitle)));
    out.insert("artist".into(), json!(g(ItemKey::TrackArtist)));
    out.insert("album".into(), json!(g(ItemKey::AlbumTitle)));
    out.insert("albumartist".into(), json!(g(ItemKey::AlbumArtist)));
    out.insert("track".into(), json!(tag.track()));
    out.insert("year".into(), json!(g(ItemKey::RecordingDate).chars().take(4).collect::<String>()));
    out.insert("genre".into(), json!(g(ItemKey::Genre)));
    out
}

/// Missing tags for one library row, best source first → (only what changed, row with real tags).
async fn fill(st: &impl Ctx, t: &Row) -> (Row, Row) {
    let path = PathBuf::from(t.s("path"));
    let real = real_tags(&path);
    let mut t = t.clone();
    if !real.is_empty() {
        for k in ["title", "artist", "album", "albumartist", "track", "year", "genre"] {
            t.insert(k.into(), real.get(k).filter(|v| !unknown(v)).cloned().unwrap_or(Value::Null));
        }
    }
    let have = |r: &Row, k: &str| !unknown(r.get(k).unwrap_or(&Value::Null));
    let mut fix = Row::new();
    if !(have(&t, "title") && have(&t, "artist")) {
        let mut guess = from_filename(&path);
        if acoustid::available(st) {
            if let Some((d, fp)) = acoustid::fingerprint(&path).await {
                if let Ok(res) = encore_http::acoustid::lookup(st, d, &fp).await {
                    let rec = res.iter().filter(|x| x["score"].as_f64().unwrap_or(0.0) > 0.8)
                        .flat_map(|x| x["recordings"].as_array().cloned().unwrap_or_default()).find(|r| r["title"].as_str().is_some_and(|s| !s.is_empty()));
                    if let Some(r) = rec {
                        guess.insert("title".into(), r["title"].clone());
                        guess.insert("artist".into(), r["artist"].clone());
                        if let Some(g) = r["release_groups"].get(0) {
                            guess.entry("album").or_insert(g.clone());
                        }
                    }
                }
            }
        }
        for k in ["title", "artist", "album", "track"] {
            if !have(&t, k) && guess.get(k).is_some_and(|v| !unknown(v)) {
                fix.insert(k.into(), guess[k].clone());
            }
        }
    }
    let mut cur = t.clone();
    cur.extend(fix.clone());
    if (!have(&t, "album") && !fix.contains_key("album")) || !have(&t, "track") || !have(&t, "year") {
        let dz = deezer(st, &cur.s("artist"), &cur.s("title")).await;
        for k in ["album", "albumartist", "genre"] {
            if dz.get(k).is_some_and(|v| !unknown(v)) && !have(&t, k) && !fix.contains_key(k) {
                fix.insert(k.into(), dz[k].clone());
            }
        }
        for k in ["year", "track"] {
            if dz.get(k).is_some_and(|v| !unknown(v)) && !have(&cur, k) {
                fix.insert(k.into(), dz[k].clone());
            }
        }
    }
    (fix, t)
}

fn ext(p: &Path) -> String {
    p.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default()
}

/// Move one song to where its tags say it belongs (after a manual fix). Never merges with another file.
pub async fn organize_track(st: &impl Ctx, id: i64) -> Option<PathBuf> {
    let t = st.db().one(&format!("{COLS} WHERE id=?"), p![id]).ok()??;
    let path = PathBuf::from(t.s("path"));
    if !path.is_file() {
        return None;
    }
    let n = st.db().one("SELECT COUNT(*) n FROM tracks WHERE album_id=?", p![t.s("album_id")]).ok()??.i("n");
    let dest = tagger::render_path(st, &t, &ext(&path), n == 1 && t.s("album").trim().is_empty());
    if dest == path {
        return Some(path);
    }
    let new = tagger::move_file(st, &path, &dest, false).await.ok()?;
    tagger::relink(st, std::slice::from_ref(&path), std::slice::from_ref(&new));
    library::prune_empty(path.parent()?, Path::new(&st.cfg().str("library_path")));
    tagger::index_paths(st, std::slice::from_ref(&new));
    Some(new)
}

pub async fn organize_library(st: impl Ctx, fill_missing: bool) {
    let rows = st.db().query(&format!("{COLS} ORDER BY album_id, disc, track"), p![]).unwrap_or_default();
    if !st.jobs().start("organize", json!({ "done": 0, "total": rows.len(), "moved": 0, "fixed": 0, "current": "", "message": "" })) {
        return;
    }
    let lib = PathBuf::from(st.cfg().str("library_path"));
    let mut counts = std::collections::HashMap::new();
    for r in &rows {
        *counts.entry(r.s("album_id")).or_insert(0) += 1;
    }
    for t in rows {
        st.jobs().set("organize", json!({ "current": format!("{} – {}", t.s("artist"), t.s("title")) }));
        st.jobs().add("organize", "done", 1);
        let path = PathBuf::from(t.s("path"));
        if !path.is_file() {
            continue;
        }
        let (fix, t) = if fill_missing { fill(&st, &t).await } else { (Row::new(), t) };
        if !fix.is_empty() {
            let tags: Row = fix.iter().map(|(k, v)| (match k.as_str() { "track" => "tracknumber", "year" => "date", x => x }.to_string(), v.clone())).collect();
            match encore_media::tags::write_tags(&path, &tags, None) {
                Ok(_) => st.jobs().add("organize", "fixed", 1),
                Err(e) => st.db().log("warning", &format!("Organize: couldn't tag {}: {e}", path.display())),
            }
        }
        let mut vals = t.clone();
        vals.extend(fix.clone());
        let single = counts.get(&t.s("album_id")).copied().unwrap_or(0) == 1 && vals.s("album").trim().is_empty();
        let dest = tagger::render_path(&st, &vals, &ext(&path), single);
        if dest != path {
            if let Ok(new) = tagger::move_file(&st, &path, &dest, true).await {
                tagger::relink(&st, std::slice::from_ref(&path), std::slice::from_ref(&new));
                library::prune_empty(path.parent().unwrap(), &lib);
                st.jobs().add("organize", "moved", 1);
                tagger::index_paths(&st, &[new]);
            }
        } else if !fix.is_empty() {
            tagger::index_paths(&st, &[path]);
        }
    }
    let j = st.jobs().get("organize");
    let msg = format!("{} moved, {} re-tagged", j["moved"], j["fixed"]);
    st.db().log("info", &format!("Organized the library: {msg}"));
    st.jobs().set("organize", json!({ "running": false, "current": "", "message": msg }));
}
