//! Import: verify (AcoustID), match to MusicBrainz, write tags + cover art, name and move files into the
//! library, and replace lower-quality copies.
use crate::acoustid;
use crate::db::{now, Row, RowExt};
use crate::library::{self, read_tags};
use crate::matcher::{describe, ratio, LibraryIndex};
use crate::state::AppState;
use crate::{mb, p};
use lofty::config::WriteOptions;
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::tag::{Accessor, ItemKey, Tag};
use regex::Regex;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

/// Downloaded audio turned out to be a different song/version.
#[derive(Debug)]
pub struct Rejected(pub String);
impl std::fmt::Display for Rejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Rejected {}

const LOSSLESS: [&str; 7] = ["flac", "alac", "wav", "aiff", "aif", "ape", "wv"];

static FEAT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*[\(\[](feat|ft|with)\.?[^\)\]]*[\)\]]").unwrap());
static NON_ALNUM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^a-z0-9]+").unwrap());

fn tnorm(s: &str) -> String {
    let s = FEAT.replace_all(&s.to_lowercase(), "").to_string();
    NON_ALNUM.replace_all(&s, " ").split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn sim(a: &str, b: &str) -> f64 {
    let (a, b) = (tnorm(a), tnorm(b));
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    if a == b { 1.0 } else { ratio(&a.chars().collect::<Vec<_>>(), &b.chars().collect::<Vec<_>>()) }
}

fn sv(v: &Value) -> String {
    match v { Value::String(s) => s.clone(), Value::Null => String::new(), x => x.to_string() }
}

/// verified/manual, lossless, bitrate, tagged – higher is better.
pub fn quality(t: &Row) -> (bool, bool, i64, bool) {
    let vs = t.s("verify_status");
    (vs == "verified" || vs == "manual", LOSSLESS.contains(&t.s("format").as_str()), t.i("bitrate"), !t.s("mb_trackid").is_empty())
}

// ---- verification -------------------------------------------------------------------------------

async fn verify(st: &AppState, path: &Path, title: &str, artist: &str, length: Option<f64>, rid: Option<&str>) -> Option<Value> {
    if !(st.cfg.bool("verify_on_import") && acoustid::available(st)) {
        return None;
    }
    match acoustid::check_file(st, path, title, artist, length, rid).await {
        Ok(v) => Some(v),
        Err(e) => {
            st.db.log("warning", &format!("Verification of {} failed: {e}", path.file_name().unwrap_or_default().to_string_lossy()));
            None
        }
    }
}

fn record_verification(st: &AppState, results: &[(PathBuf, Option<Value>)]) {
    for (p, r) in results {
        if let Some(r) = r {
            let _ = st.db.execute("UPDATE tracks SET verify_status=?, verify_detail=?, acoustid_id=?, verified_at=? WHERE path=?",
                p![r["status"], r["detail"], r["acoustid"], now(), p.to_string_lossy()]);
        }
    }
}

// ---- writing ------------------------------------------------------------------------------------

fn key(k: &str) -> Option<ItemKey> {
    Some(match k {
        "title" => ItemKey::TrackTitle,
        "artist" => ItemKey::TrackArtist,
        "album" => ItemKey::AlbumTitle,
        "albumartist" => ItemKey::AlbumArtist,
        "date" => ItemKey::RecordingDate,
        "genre" => ItemKey::Genre,
        "musicbrainz_trackid" => ItemKey::MusicBrainzRecordingId,
        "musicbrainz_releasetrackid" => ItemKey::MusicBrainzTrackId,
        "musicbrainz_albumid" => ItemKey::MusicBrainzReleaseId,
        "musicbrainz_releasegroupid" => ItemKey::MusicBrainzReleaseGroupId,
        "musicbrainz_artistid" => ItemKey::MusicBrainzArtistId,
        "musicbrainz_albumartistid" => ItemKey::MusicBrainzReleaseArtistId,
        "isrc" => ItemKey::Isrc,
        "label" => ItemKey::Label,
        "barcode" => ItemKey::Barcode,
        "catalognumber" => ItemKey::CatalogNumber,
        "releasecountry" => ItemKey::ReleaseCountry,
        _ => return None,
    })
}

/// Write tags (and optionally the front cover) into a file.
pub fn write_tags(path: &Path, vals: &Row, cover: Option<&[u8]>) -> anyhow::Result<()> {
    let mut f = lofty::read_from_path(path)?;
    if f.primary_tag().is_none() {
        let t = f.primary_tag_type();
        f.insert_tag(Tag::new(t));
    }
    let tag = f.primary_tag_mut().unwrap();
    for (k, v) in vals {
        let s = sv(v);
        if s.is_empty() {
            continue;
        }
        match k.as_str() {
            "tracknumber" => {
                if let Ok(n) = s.parse() {
                    tag.set_track(n)
                }
            }
            "discnumber" => {
                if let Ok(n) = s.parse() {
                    tag.set_disk(n)
                }
            }
            _ => {
                if let Some(ik) = key(k) {
                    tag.insert_text(ik, s);
                }
            }
        }
    }
    if let Some(data) = cover {
        let mime = if data.starts_with(b"\x89PNG") { MimeType::Png } else { MimeType::Jpeg };
        tag.remove_picture_type(PictureType::CoverFront);
        tag.push_picture(Picture::unchecked(data.to_vec()).pic_type(PictureType::CoverFront).mime_type(mime).build());
    }
    f.save_to_path(path, WriteOptions::default())?;
    Ok(())
}

fn tag_file(st: &AppState, path: &Path, vals: &Row, cover: Option<&[u8]>) {
    let cover = if st.cfg.bool("embed_cover") { cover } else { None };
    if let Err(e) = write_tags(path, vals, cover) {
        st.db.log("warning", &format!("Could not tag {}: {e}", path.file_name().unwrap_or_default().to_string_lossy()));
    }
}

// ---- cover art ----------------------------------------------------------------------------------

async fn get_image(st: &AppState, url: &str) -> Option<Vec<u8>> {
    let r = st.http.get(url).timeout(std::time::Duration::from_secs(15)).send().await.ok()?;
    if !r.status().is_success() {
        return None;
    }
    let b = r.bytes().await.ok()?;
    let ok = b.starts_with(b"\xff\xd8\xff") || b.starts_with(b"\x89PNG") || b.starts_with(b"RIFF");
    (ok && b.len() > 1500).then(|| b.to_vec())
}

pub async fn fetch_cover(st: &AppState, release_id: Option<&str>, rgid: Option<&str>) -> Option<Vec<u8>> {
    let mut urls = vec![];
    if let Some(r) = release_id.filter(|r| !r.is_empty()) {
        urls.push(mb::cover_release(r, 1200));
    }
    if let Some(g) = rgid.filter(|r| !r.is_empty()) {
        urls.push(mb::cover_rg(g, 1200));
    }
    for u in urls {
        if let Some(d) = get_image(st, &u).await {
            return Some(d);
        }
    }
    None
}

/// Ask image CDNs for a large version.
fn big(url: &str) -> String {
    static GOOGLE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"=w\d+-h\d+[^&?/]*$").unwrap());
    static APPLE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/\d+x\d+(bb)?\.(jpg|png|webp)$").unwrap());
    static DEEZER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/(\d+)x(\d+)-\d+-\d+-\d+-\d+\.jpg$").unwrap());
    let u = GOOGLE.replace(url, "=w1200-h1200-l90-rj").to_string();
    let u = APPLE.replace(&u, "/1200x1200bb.jpg").to_string();
    DEEZER.replace(&u, "/1000x1000-000000-80-0-0.jpg").to_string()
}

/// YouTube thumbnails are 16:9 – keep the middle square.
fn square(data: &[u8]) -> Vec<u8> {
    let Ok(img) = image::load_from_memory(data) else { return data.to_vec() };
    let s = img.width().min(img.height());
    let c = img.crop_imm((img.width() - s) / 2, (img.height() - s) / 2, s, s);
    let mut out = std::io::Cursor::new(vec![]);
    if c.to_rgb8().write_to(&mut out, image::ImageFormat::Jpeg).is_ok() { out.into_inner() } else { data.to_vec() }
}

/// Cover art when MusicBrainz has none: the request's own cover, then Deezer, then iTunes, then YouTube.
/// Search hits must match artist and album/title so the wrong artwork is never embedded.
pub async fn find_art(st: &AppState, artist: &str, album: Option<&str>, title: Option<&str>, hint: Option<&str>) -> Option<(Vec<u8>, &'static str)> {
    if let Some(h) = hint.filter(|h| !h.is_empty() && !h.contains("ytimg.com")) {
        if let Some(d) = match get_image(st, &big(h)).await { Some(d) => Some(d), None => get_image(st, h).await } {
            return Some((d, "request"));
        }
    }
    for (kind, q) in [("album", album), ("track", title)] {
        let Some(q) = q.filter(|q| !q.is_empty()) else { continue };
        if let Ok(r) = st.http.get(format!("https://api.deezer.com/search/{kind}")).query(&[("q", format!("artist:\"{artist}\" {kind}:\"{q}\"")), ("limit", "5".into())]).send().await {
            for hit in r.json::<Value>().await.ok().and_then(|d| d["data"].as_array().cloned()).unwrap_or_default() {
                let alb = if kind == "album" { hit.clone() } else { hit["album"].clone() };
                if sim(hit["artist"]["name"].as_str().unwrap_or(""), artist) >= 0.8 && sim(hit["title"].as_str().unwrap_or(""), q) >= 0.75 {
                    if let Some(u) = alb["cover_xl"].as_str() {
                        if let Some(d) = get_image(st, u).await {
                            return Some((d, "Deezer"));
                        }
                    }
                }
            }
        }
        let ent = if kind == "album" { "album" } else { "song" };
        if let Ok(r) = st.http.get("https://itunes.apple.com/search").query(&[("term", format!("{artist} {q}")), ("entity", ent.into()), ("limit", "5".into())]).send().await {
            for hit in r.json::<Value>().await.ok().and_then(|d| d["results"].as_array().cloned()).unwrap_or_default() {
                let ht = hit[if kind == "album" { "collectionName" } else { "trackName" }].as_str().unwrap_or("");
                if sim(hit["artistName"].as_str().unwrap_or(""), artist) >= 0.8 && sim(ht, q) >= 0.75 {
                    if let Some(u) = hit["artworkUrl100"].as_str() {
                        if let Some(d) = get_image(st, &big(u)).await {
                            return Some((d, "iTunes"));
                        }
                    }
                }
            }
        }
    }
    if let Some(h) = hint.filter(|h| !h.is_empty()) {
        if let Some(d) = match get_image(st, &big(h)).await { Some(d) => Some(d), None => get_image(st, h).await } {
            return Some((square(&d), "YouTube"));
        }
    }
    None
}

// ---- naming -------------------------------------------------------------------------------------

pub fn clean(s: &str) -> String {
    let c: String = s.chars().map(|c| if "<>:\"/\\|?*".contains(c) || (c as u32) < 32 { '_' } else { c }).collect();
    let c = c.trim().trim_end_matches('.').to_string();
    let c: String = c.chars().take(120).collect();
    if c.is_empty() { "_".into() } else { c }
}

fn format_template(tpl: &str, data: &std::collections::HashMap<&str, String>, nums: &std::collections::HashMap<&str, i64>) -> Option<String> {
    let re = Regex::new(r"\{(\w+)(?::0?(\d+)d)?\}").unwrap();
    let mut bad = false;
    let out = re.replace_all(tpl, |c: &regex::Captures| {
        let k = &c[1];
        if let Some(n) = nums.get(k) {
            let w: usize = c.get(2).and_then(|w| w.as_str().parse().ok()).unwrap_or(0);
            format!("{n:0w$}")
        } else if let Some(s) = data.get(k) {
            s.clone()
        } else {
            if c.get(2).is_some() {
                bad = true;
            }
            String::new()
        }
    }).to_string();
    (!bad).then_some(out)
}

pub fn render_path(st: &AppState, v: &Row, ext: &str, single: bool) -> PathBuf {
    let tpl = st.cfg.str(if single { "single_template" } else { "naming_template" });
    let disc_count = v.get("disccount").and_then(|x| x.as_i64().or(x.as_str().and_then(|s| s.parse().ok()))).unwrap_or(1);
    let num = |k: &str, d: i64| v.get(k).and_then(|x| x.as_i64().or(x.as_str().and_then(|s| s.parse().ok()))).unwrap_or(d);
    let or = |k: &str, d: &str| { let s = v.s(k); if s.is_empty() { d.to_string() } else { s } };
    let disc = num("disc", 1);
    let data: std::collections::HashMap<&str, String> = [
        ("artist", clean(&v.s("artist"))), ("albumartist", clean(&or("albumartist", &v.s("artist")))), ("album", clean(&or("album", "Unknown Album"))),
        ("title", clean(&v.s("title"))), ("year", clean(&or("year", "0000"))), ("genre", clean(&v.s("genre"))),
        ("discprefix", if disc_count > 1 { format!("{disc}-") } else { String::new() }), ("label", clean(&v.s("label"))), ("type", clean(&v.s("type"))),
        ("format", ext.trim_start_matches('.').to_uppercase()),
    ].into_iter().collect();
    let nums: std::collections::HashMap<&str, i64> = [("track", num("track", 0)), ("disc", disc)].into_iter().collect();
    let rel = format_template(&tpl, &data, &nums).unwrap_or_else(|| format_template("{albumartist}/{album} ({year})/{track:02d} - {title}", &data, &nums).unwrap());
    let rel = rel.replace(" ()", "").replace("(0000)", "");
    let mut out = PathBuf::from(st.cfg.str("library_path"));
    let parts: Vec<String> = rel.trim().split('/').filter(|p| !p.trim().is_empty()).map(clean).collect();
    for (i, part) in parts.iter().enumerate() {
        out.push(if i + 1 == parts.len() { format!("{part}{}", ext.to_lowercase()) } else { part.clone() });
    }
    out
}

fn file_rank(path: &Path) -> (bool, i64, u64) {
    let t = read_tags(&path.to_string_lossy());
    (LOSSLESS.contains(&t.s("format").as_str()), t.i("bitrate"), std::fs::metadata(path).map(|m| m.len()).unwrap_or(0))
}

/// Only swap files that hold the same recording – a wrong song with the same name never replaces a good file.
async fn may_replace(st: &AppState, src: &Path, dest: &Path) -> bool {
    if !st.cfg.bool("upgrade_check_audio") {
        return true;
    }
    let same = acoustid::same_audio(src, dest).await;
    if same == Some(false) {
        st.db.log("warning", &format!("Kept both {} and the new download – the audio is a different recording", dest.file_name().unwrap_or_default().to_string_lossy()));
    }
    same != Some(false)
}

fn mv(src: &Path, dest: &Path) -> std::io::Result<()> {
    if std::fs::rename(src, dest).is_ok() {
        return Ok(());
    }
    std::fs::copy(src, dest)?; // across file systems
    std::fs::remove_file(src)
}

/// Move src to dest. With replace_same a file already at dest is the same song (same path), so only the
/// better of the two is kept instead of adding "(2)" copies.
pub async fn move_file(st: &AppState, src: &Path, dest: &Path, replace_same: bool) -> anyhow::Result<PathBuf> {
    if src == dest {
        return Ok(src.to_path_buf());
    }
    std::fs::create_dir_all(dest.parent().unwrap())?;
    if replace_same && dest.exists() && may_replace(st, src, dest).await {
        if file_rank(src) > file_rank(dest) {
            let tmp = PathBuf::from(format!("{}.encore-new", dest.to_string_lossy()));
            mv(src, &tmp)?;
            std::fs::rename(&tmp, dest)?;
        } else {
            std::fs::remove_file(src)?;
        }
        return Ok(dest.to_path_buf());
    }
    let mut d = dest.to_path_buf();
    let stem = dest.file_stem().unwrap_or_default().to_string_lossy().to_string();
    let ext = dest.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    let mut i = 2;
    while d.exists() {
        d = dest.with_file_name(format!("{stem} ({i}){ext}"));
        i += 1;
    }
    mv(src, &d)?;
    Ok(d)
}

fn index_paths(st: &AppState, paths: &[PathBuf]) {
    for p in paths {
        if let Err(e) = library::index_file(&st.db, &p.to_string_lossy()) {
            st.db.log("warning", &format!("Could not index {}: {e}", p.display()));
        }
    }
    st.library.forget_cover(None);
}

// ---- matching -----------------------------------------------------------------------------------

/// Assign local files to release tracks → (file index → track index), confidence 0..1.
pub fn map_files(files: &[Row], tracks: &[Value]) -> (Vec<Option<usize>>, f64) {
    let numbered = files.iter().all(|f| f.i("track") > 0)
        && files.iter().map(|f| (f.i("disc").max(1), f.i("track"))).collect::<std::collections::HashSet<_>>().len() == files.len();
    let multi_disc = tracks.iter().map(|t| t["disc"].as_i64().unwrap_or(1)).collect::<std::collections::HashSet<_>>().len() > 1;
    let mut scores = vec![];
    for (i, f) in files.iter().enumerate() {
        for (j, t) in tracks.iter().enumerate() {
            let mut s = sim(&f.s("title"), t["title"].as_str().unwrap_or("")) * 0.6;
            if numbered {
                let same = f.i("track") == t["track"].as_i64().unwrap_or(0) && (f.i("disc").max(1) == t["disc"].as_i64().unwrap_or(1) || !multi_disc);
                s += if same { 0.3 } else { 0.0 };
            } else {
                s += 0.15;
            }
            let (fd, tl) = (f.f("duration"), t["length"].as_f64().unwrap_or(0.0));
            if fd > 0.0 && tl > 0.0 {
                let d = (fd - tl).abs();
                s += if d < 3.0 { 0.1 } else if d < 10.0 { 0.05 } else { -0.15 };
            } else {
                s += 0.05;
            }
            scores.push((s, i, j));
        }
    }
    scores.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
    let (mut uf, mut ut) = (std::collections::HashSet::new(), std::collections::HashSet::new());
    let mut assign = vec![None; files.len()];
    let mut total = 0.0;
    for (s, i, j) in scores {
        if uf.contains(&i) || ut.contains(&j) {
            continue;
        }
        uf.insert(i);
        ut.insert(j);
        assign[i] = Some(j);
        total += s.clamp(0.0, 1.0);
    }
    let mut conf = total / files.len().max(1) as f64;
    if files.len() != tracks.len() {
        conf *= (1.0 - (files.len() as f64 - tracks.len() as f64).abs() / tracks.len().max(1) as f64).max(0.0);
    }
    (assign, (conf * 1000.0).round() / 1000.0)
}

pub fn release_values(rel: &Value, t: &Value) -> Row {
    let artist = { let a = sv(&t["artist"]); if a.is_empty() { sv(&rel["artist"]) } else { a } };
    let mut m = Row::new();
    for (k, v) in [
        ("title", t["title"].clone()), ("artist", json!(artist)), ("album", rel["title"].clone()), ("albumartist", rel["artist"].clone()),
        ("tracknumber", t["track"].clone()), ("track", t["track"].clone()), ("discnumber", t["disc"].clone()), ("disc", t["disc"].clone()),
        ("disccount", rel["disc_count"].clone()), ("date", rel["date"].clone()), ("year", rel["year"].clone()), ("genre", rel["genre"].clone()),
        ("label", rel["label"].clone()), ("barcode", rel["barcode"].clone()), ("releasecountry", rel["country"].clone()), ("type", rel["type"].clone()),
        ("isrc", t["isrc"].clone()), ("musicbrainz_trackid", t["recording_id"].clone()), ("musicbrainz_releasetrackid", t["track_id"].clone()),
        ("musicbrainz_albumid", rel["id"].clone()), ("musicbrainz_releasegroupid", rel["release_group_id"].clone()),
        ("musicbrainz_artistid", if t["artist_id"].is_null() { rel["artist_id"].clone() } else { t["artist_id"].clone() }),
        ("musicbrainz_albumartistid", rel["artist_id"].clone()),
    ] {
        m.insert(k.into(), v);
    }
    m
}

/// Files → MusicBrainz release: per-file new values and paths, plus a confidence.
pub async fn preview(st: &AppState, paths: &[PathBuf], release_id: &str, single: bool) -> anyhow::Result<Value> {
    let rel = mb::release(st, release_id).await?;
    let files: Vec<Row> = paths.iter().map(|p| read_tags(&p.to_string_lossy())).collect();
    let tracks = rel["tracks"].as_array().cloned().unwrap_or_default();
    let (assign, conf) = map_files(&files, &tracks);
    let rows: Vec<Value> = paths.iter().zip(files.iter()).zip(assign).map(|((p, f), j)| {
        let mut row = json!({ "path": p, "old_title": f.s("title"), "old_track": f.get("track"), "old_artist": f.s("artist"), "old_album": f.s("album") });
        if let Some(j) = j {
            let vals = release_values(&rel, &tracks[j]);
            let ext = p.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
            let np = if st.cfg.bool("rename_on_import") { render_path(st, &vals, &ext, single) } else { p.clone() };
            row["new_title"] = tracks[j]["title"].clone();
            row["new_track"] = tracks[j]["track"].clone();
            row["new_disc"] = tracks[j]["disc"].clone();
            row["new_artist"] = vals.get("artist").cloned().unwrap_or_default();
            row["new_path"] = json!(np);
            row["values"] = Value::Object(vals);
        }
        row
    }).collect();
    let mut r = rel.clone();
    r.as_object_mut().unwrap().remove("tracks");
    Ok(json!({ "release": r, "tracks": tracks, "rows": rows, "confidence": conf }))
}

pub async fn apply_preview(st: &AppState, pv: &Value, rename: bool, source_root: Option<&Path>, hint: Option<&str>) -> anyhow::Result<Vec<PathBuf>> {
    let rel = &pv["release"];
    let mut cover = fetch_cover(st, rel["id"].as_str(), rel["release_group_id"].as_str()).await;
    if cover.is_none() {
        cover = find_art(st, rel["artist"].as_str().unwrap_or(""), rel["title"].as_str(), None, hint).await.map(|x| x.0);
    }
    let mut new_paths = vec![];
    let mut old_dirs = std::collections::HashSet::new();
    for row in pv["rows"].as_array().cloned().unwrap_or_default() {
        let path = PathBuf::from(row["path"].as_str().unwrap_or(""));
        let Some(vals) = row["values"].as_object() else {
            new_paths.push(path);
            continue;
        };
        tag_file(st, &path, vals, cover.as_deref());
        if rename {
            old_dirs.insert(path.parent().map(|p| p.to_path_buf()).unwrap_or_default());
            let np = PathBuf::from(row["new_path"].as_str().unwrap_or(""));
            new_paths.push(move_file(st, &path, &np, source_root.is_some()).await?);
        } else {
            new_paths.push(path);
        }
    }
    save_cover_file(st, &new_paths, cover.as_deref());
    let lib = PathBuf::from(st.cfg.str("library_path"));
    for d in old_dirs {
        library::prune_empty(&d, source_root.unwrap_or(&lib));
    }
    Ok(new_paths)
}

fn save_cover_file(st: &AppState, paths: &[PathBuf], cover: Option<&[u8]>) {
    let (Some(c), Some(first)) = (cover, paths.first()) else { return };
    if !st.cfg.bool("save_cover") {
        return;
    }
    let d = first.parent().unwrap();
    if paths.iter().all(|p| p.parent() == Some(d)) && !["cover.jpg", "cover.png", "folder.jpg"].iter().any(|n| d.join(n).exists()) {
        let _ = std::fs::write(d.join("cover.jpg"), c);
    }
}

/// Best MusicBrainz release for a set of files, or None when not confident.
pub async fn auto_match(st: &AppState, paths: &[PathBuf], artist: &str, album: &str, rgid: Option<&str>) -> anyhow::Result<Option<Value>> {
    let threshold = st.cfg.float("autotag_threshold");
    let mut ids = vec![];
    if let Some(g) = rgid.filter(|g| !g.is_empty()) {
        if let Some(r) = mb::best_release_for_group(st, g, Some(paths.len() as i64)).await? {
            ids.push(r);
        }
    }
    if ids.is_empty() && !album.is_empty() {
        let mut res = mb::search_releases(st, artist, album, 8).await?;
        res.sort_by_key(|r| (r["track_count"].as_i64() != Some(paths.len() as i64), -r["score"].as_i64().unwrap_or(0)));
        ids = res.iter().take(3).filter_map(|r| r["id"].as_str().map(String::from)).collect();
    }
    let mut best: Option<Value> = None;
    for id in ids {
        let pv = preview(st, paths, &id, false).await?;
        let c = pv["confidence"].as_f64().unwrap_or(0.0);
        if best.as_ref().is_none_or(|b| c > b["confidence"].as_f64().unwrap_or(0.0)) {
            best = Some(pv);
        }
        if c >= 0.92 {
            break;
        }
    }
    Ok(best.filter(|b| {
        let c = b["confidence"].as_f64().unwrap_or(0.0);
        c >= threshold || (rgid.is_some_and(|g| !g.is_empty()) && c >= threshold - 0.2)
    }))
}

fn all_tracks(st: &AppState) -> Vec<Row> {
    st.db.query("SELECT id, path, title, artist, album, album_id, duration, isrc, mb_trackid, format, bitrate, verify_status FROM tracks", p![]).unwrap_or_default()
}

/// Verify, tag (from the MusicBrainz recording if possible), de-duplicate and move one song into the library.
/// → (final path, note)
#[allow(clippy::too_many_arguments)]
pub async fn import_single(st: &AppState, path: &Path, artist: &str, title: &str, recording_id: Option<&str>, source_root: &Path,
                           length: Option<f64>, reject: bool, hint: Option<&str>) -> anyhow::Result<(PathBuf, String)> {
    let check = verify(st, path, title, artist, length, recording_id).await;
    if let Some(c) = &check {
        if c["status"] == "mismatch" && reject && st.cfg.bool("reject_mismatch") {
            let _ = std::fs::remove_file(path);
            return Err(Rejected(c["detail"].as_str().unwrap_or("").into()).into());
        }
    }
    let mut vals = Row::new();
    vals.insert("title".into(), json!(title));
    vals.insert("artist".into(), json!(artist));
    vals.insert("albumartist".into(), json!(artist));
    let mut cover = None;
    let rec = match recording_id.filter(|r| !r.is_empty()) {
        Some(r) => mb::recording(st, r).await.ok(),
        None => match mb::search_recordings(st, title, 5, Some(artist)).await {
            Ok(res) => match res.iter().find(|r| sim(r["title"].as_str().unwrap_or(""), title) > 0.8 && sim(r["artist"].as_str().unwrap_or(""), artist) > 0.6) {
                Some(r) => mb::recording(st, r["id"].as_str().unwrap_or("")).await.ok(),
                None => None,
            },
            Err(e) => {
                st.db.log("warning", &format!("MusicBrainz lookup failed for {artist} - {title}: {e}"));
                None
            }
        },
    };
    if let Some(r) = rec {
        for (k, v) in [("title", &r["title"]), ("artist", &r["artist"]), ("albumartist", &r["artist"]), ("album", &r["album"]), ("date", &r["year"]),
                       ("year", &r["year"]), ("isrc", &r["isrc"]), ("musicbrainz_trackid", &r["id"]), ("musicbrainz_artistid", &r["artist_id"]),
                       ("musicbrainz_albumartistid", &r["artist_id"])] {
            vals.insert(k.into(), v.clone());
        }
        cover = fetch_cover(st, r["release_id"].as_str(), r["release_group_id"].as_str()).await;
    }
    if cover.is_none() {
        let album = vals.s("album");
        cover = find_art(st, &vals.s("artist"), Some(&album), Some(&vals.s("title")), hint).await.map(|x| x.0);
    }
    let ext = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    // the same song already in the library?
    let mut existing = None;
    if st.cfg.bool("dedupe_on_import") {
        let ix = LibraryIndex::new(all_tracks(st));
        let want = describe(&vals.s("title"), &vals.s("artist"), "", length, Some(&vals.s("isrc")), Some(&vals.s("musicbrainz_trackid")));
        if let Some((hit, _)) = ix.find(&want) {
            existing = Some(hit.clone());
        }
    }
    if let Some(ex) = &existing {
        let ep = PathBuf::from(ex.s("path"));
        if st.cfg.bool("upgrade_check_audio") && ep.is_file() && acoustid::same_audio(path, &ep).await == Some(false) {
            existing = None; // same name, different recording (another version): import it as its own song
        }
    }
    if let Some(ex) = &existing {
        let mut nq = read_tags(&path.to_string_lossy());
        nq.insert("verify_status".into(), check.as_ref().map(|c| c["status"].clone()).unwrap_or_default());
        if quality(&nq) <= quality(ex) {
            let _ = std::fs::remove_file(path);
            library::prune_empty(path.parent().unwrap(), source_root);
            return Ok((PathBuf::from(ex.s("path")), "already in library – kept existing copy".into()));
        }
    }
    tag_file(st, path, &vals, cover.as_deref());
    let dest = render_path(st, &vals, &ext, true);
    let new = move_file(st, path, &dest, true).await?;
    library::prune_empty(path.parent().unwrap(), source_root);
    index_paths(st, std::slice::from_ref(&new));
    record_verification(st, &[(new.clone(), check)]);
    let mut note = String::new();
    if let Some(ex) = existing {
        let nid = st.db.one("SELECT id FROM tracks WHERE path=?", p![new.to_string_lossy()])?.map(|r| r.i("id"));
        library::delete_tracks(&st.db, &[ex.i("id")], nid);
        note = "replaced a lower-quality copy".into();
    }
    Ok((new, note))
}

/// Tag + move a downloaded album. Falls back to the files' own tags (or the store's track list for releases
/// MusicBrainz doesn't have) when there is no confident MusicBrainz match.
#[allow(clippy::too_many_arguments)]
pub async fn import_album_files(st: &AppState, paths: &[PathBuf], artist: &str, album: &str, rgid: Option<&str>, source_root: &Path,
                                reject: bool, source_meta: Option<&Value>, hint: Option<&str>) -> anyhow::Result<Vec<PathBuf>> {
    let mut paths = paths.to_vec();
    paths.sort();
    let pv = if st.cfg.bool("autotag_on_import") {
        match auto_match(st, &paths, artist, album, rgid).await {
            Ok(p) => p,
            Err(e) => {
                st.db.log("warning", &format!("Auto-tag failed for {artist} - {album}: {e}"));
                None
            }
        }
    } else {
        None
    };
    if let Some(pv) = pv {
        let mut checks = vec![];
        let mut mism = vec![];
        for row in pv["rows"].as_array().cloned().unwrap_or_default() {
            let Some(v) = row["values"].as_object() else { continue };
            let p = PathBuf::from(row["path"].as_str().unwrap_or(""));
            let res = verify(st, &p, &v.s("title"), &v.s("artist"), None, Some(&v.s("musicbrainz_trackid"))).await;
            if let Some(r) = &res {
                if r["status"] == "mismatch" {
                    mism.push(format!("{}: {}", v.s("title"), r["detail"].as_str().unwrap_or("")));
                }
            }
            checks.push((p, res));
        }
        if !mism.is_empty() && reject && st.cfg.bool("reject_mismatch") && mism.len() > (paths.len() / 4).max(1) {
            return Err(Rejected(format!("{} of {} files are not what they claim – e.g. {}", mism.len(), paths.len(), mism[0])).into());
        }
        let new = apply_preview(st, &pv, true, Some(source_root), hint).await?;
        let rows = pv["rows"].as_array().cloned().unwrap_or_default();
        let results: Vec<(PathBuf, Option<Value>)> = rows.iter().zip(new.iter()).map(|(row, np)| {
            let old = PathBuf::from(row["path"].as_str().unwrap_or(""));
            (np.clone(), checks.iter().find(|(p, _)| *p == old).and_then(|(_, r)| r.clone()))
        }).collect();
        index_paths(st, &new);
        record_verification(st, &results);
        drop_superseded(st, &new).await;
        return Ok(new);
    }
    let cover = find_art(st, artist, Some(album), None, hint).await.map(|x| x.0);
    let src_tracks: Vec<Value> = source_meta.and_then(|m| m["tracks"].as_array().cloned()).unwrap_or_default();
    let unique_numbers = src_tracks.iter().map(|x| x["track"].as_i64()).collect::<std::collections::HashSet<_>>().len() == src_tracks.len();
    let mut out = vec![];
    for (i, p) in paths.iter().enumerate() {
        let t = read_tags(&p.to_string_lossy());
        let mut vals = t.clone();
        if t.s("albumartist") == "Unknown Artist" {
            vals.insert("albumartist".into(), json!(artist));
        }
        if t.s("album").is_empty() {
            vals.insert("album".into(), json!(album));
        }
        if let Some(meta) = source_meta {
            // not in MusicBrainz: tag from the store's metadata (shows under "Needs tagging" until it is)
            let stt = src_tracks.iter().max_by(|a, b| sim(a["title"].as_str().unwrap_or(""), &t.s("title")).partial_cmp(&sim(b["title"].as_str().unwrap_or(""), &t.s("title"))).unwrap_or(std::cmp::Ordering::Equal))
                .filter(|b| sim(b["title"].as_str().unwrap_or(""), &t.s("title")) >= 0.8)
                .or_else(|| (unique_numbers && t.i("track") > 0).then(|| src_tracks.iter().find(|x| x["track"].as_i64() == Some(t.i("track")) && x["disc"].as_i64().unwrap_or(1) == t.i("disc").max(1))).flatten())
                .or_else(|| (src_tracks.len() == paths.len()).then(|| &src_tracks[i]))
                .cloned();
            let year = meta["year"].as_str().filter(|y| !y.is_empty()).map(String::from).unwrap_or(t.s("year"));
            vals.insert("album".into(), json!(album));
            vals.insert("albumartist".into(), json!(artist));
            vals.insert("year".into(), json!(year));
            if let Some(s) = &stt {
                vals.insert("title".into(), s["title"].clone());
                vals.insert("artist".into(), if s["artist"].as_str().unwrap_or("").is_empty() { json!(artist) } else { s["artist"].clone() });
                vals.insert("track".into(), s["track"].clone());
                vals.insert("disc".into(), json!(s["disc"].as_i64().unwrap_or(1)));
            }
            let mut w = Row::new();
            for (k, v) in [("title", vals["title"].clone()), ("artist", vals["artist"].clone()), ("album", json!(album)), ("albumartist", json!(artist)),
                           ("tracknumber", vals.get("track").cloned().unwrap_or_default()), ("discnumber", vals.get("disc").cloned().unwrap_or_default()),
                           ("date", json!(year)), ("isrc", stt.as_ref().map(|s| s["isrc"].clone()).unwrap_or_default()), ("barcode", meta["upc"].clone())] {
                w.insert(k.into(), v);
            }
            tag_file(st, p, &w, cover.as_deref());
        } else if cover.is_some() {
            tag_file(st, p, &Row::new(), cover.as_deref());
        }
        let ext = p.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
        let dest = render_path(st, &vals, &ext, false);
        out.push(move_file(st, p, &dest, true).await?);
        library::prune_empty(p.parent().unwrap(), source_root);
    }
    save_cover_file(st, &out, cover.as_deref());
    st.db.log("warning", &format!("No confident MusicBrainz match for {artist} - {album}; imported with existing tags"));
    index_paths(st, &out);
    drop_superseded(st, &out).await;
    Ok(out)
}

/// After an album import, remove copies it makes redundant: loose singles of the same recordings and
/// lower-quality copies. Playlists follow the new file.
pub async fn drop_superseded(st: &AppState, new_paths: &[PathBuf]) {
    if !st.cfg.bool("dedupe_on_import") {
        return;
    }
    let new_set: std::collections::HashSet<String> = new_paths.iter().map(|p| p.to_string_lossy().into_owned()).collect();
    let sizes: std::collections::HashMap<String, i64> = st.db.query("SELECT album_id, COUNT(*) n FROM tracks GROUP BY album_id", p![]).unwrap_or_default()
        .into_iter().map(|r| (r.s("album_id"), r.i("n"))).collect();
    let others: Vec<Row> = all_tracks(st).into_iter().filter(|t| !new_set.contains(&t.s("path"))).collect();
    if others.is_empty() {
        return;
    }
    let ix = LibraryIndex::new(others);
    for p in new_paths {
        let Ok(Some(t)) = st.db.one("SELECT id, title, artist, duration, isrc, mb_trackid, format, bitrate, verify_status, path FROM tracks WHERE path=?", p![p.to_string_lossy()]) else { continue };
        let want = describe(&t.s("title"), &t.s("artist"), "", Some(t.f("duration")), Some(&t.s("isrc")), Some(&t.s("mb_trackid")));
        let Some((hit, _)) = ix.find(&want) else { continue };
        let hit = hit.clone();
        let loose = sizes.get(&hit.s("album_id")).copied().unwrap_or(0) == 1;
        let better = quality(&t) > quality(&hit);
        if !(loose || better) {
            continue;
        }
        let hp = PathBuf::from(hit.s("path"));
        if st.cfg.bool("upgrade_check_audio") && hp.is_file() && acoustid::same_audio(p, &hp).await == Some(false) {
            continue; // same song by name, different audio: keep both
        }
        library::delete_tracks(&st.db, &[hit.i("id")], Some(t.i("id")));
        st.db.log("info", &format!("Removed {} of “{} – {}” – replaced by the new album import", if loose { "loose single" } else { "lower-quality copy" }, hit.s("artist"), hit.s("title")));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn templates() {
        let data: std::collections::HashMap<&str, String> = [("albumartist", "A".to_string()), ("album", "B".into()), ("year", "2020".into()), ("discprefix", "".into()), ("title", "T".into())].into_iter().collect();
        let nums: std::collections::HashMap<&str, i64> = [("track", 3), ("disc", 1)].into_iter().collect();
        assert_eq!(format_template("{albumartist}/{album} ({year})/{discprefix}{track:02d} - {title}", &data, &nums).unwrap(), "A/B (2020)/03 - T");
        assert_eq!(clean("AC/DC: Live?"), "AC_DC_ Live_");
        assert!(sim("Song (feat. X)", "song") == 1.0);
    }
}

#[cfg(test)]
mod import_tests {
    use super::*;
    #[tokio::test]
    async fn album_import_offline() {
        let _env = crate::TEST_ENV.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("encore-import-{}", std::process::id()));
        let (lib, dl) = (dir.join("lib"), dir.join("dl/Some Album"));
        std::fs::create_dir_all(&lib).unwrap();
        std::fs::create_dir_all(&dl).unwrap();
        std::env::set_var("ENCORE_CONFIG_DIR", &dir);
        std::env::set_var("ENCORE_LIBRARY_PATH", &lib);
        std::env::set_var("MUSICBRAINZ_URL", "http://127.0.0.1:9");
        let st = crate::build_state().unwrap();
        let mut files = vec![];
        for (i, f) in [440, 660].iter().enumerate() {
            let p = dl.join(format!("{:02} track.flac", i + 1));
            let ok = std::process::Command::new("ffmpeg").args(["-v", "error", "-y", "-f", "lavfi", "-i", &format!("sine=frequency={f}:duration=3")])
                .args(["-metadata", &format!("title=Song {}", i + 1), "-metadata", "artist=Tester", "-metadata", &format!("track={}", i + 1)]).arg(&p).status();
            if !ok.map(|s| s.success()).unwrap_or(false) {
                return; // no ffmpeg here
            }
            files.push(p);
        }
        let meta = json!({ "tracks": [{ "title": "First", "artist": "Tester", "track": 1, "disc": 1 }, { "title": "Second", "artist": "Tester", "track": 2, "disc": 1 }], "year": "2024", "upc": "123" });
        let new = import_album_files(&st, &files, "Tester", "Some Album", None, &dir.join("dl"), true, Some(&meta), None).await.unwrap();
        assert_eq!(new.len(), 2);
        assert_eq!(new[0], lib.join("Tester/Some Album (2024)/01 - First.flac"));
        let t = read_tags(&new[1].to_string_lossy());
        assert_eq!(t.s("title"), "Second");
        assert_eq!(t.s("album"), "Some Album");
        assert!(!dl.exists(), "download folder cleaned up");
        let n = st.db.one("SELECT COUNT(*) n FROM tracks", p![]).unwrap().unwrap().i("n");
        assert_eq!(n, 2);
        // the same album again: same paths, no "(2)" copies
        std::fs::create_dir_all(&dl).unwrap();
        let again = dl.join("01 track.flac");
        std::fs::copy(&new[0], &again).unwrap();
        let new2 = import_album_files(&st, &[again], "Tester", "Some Album", None, &dir.join("dl"), true, Some(&meta), None).await.unwrap();
        assert_eq!(new2[0], new[0]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
