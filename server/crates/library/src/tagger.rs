//! Import: verify (AcoustID), match to MusicBrainz, write tags + cover art, name and move files into the
//! library, and replace lower-quality copies.
use crate::acoustid;
use encore_db::{now, Row, RowExt};
use crate::scan as library;
use encore_media::tags::read_tags;
use crate::matcher::{describe, ratio, LibraryIndex};
use crate::Ctx;
use encore_http::{mb, progress};
use encore_db::p;
use regex::Regex;
use serde_json::{json, Value};
use encore_core::fsx::{self, Step, StepError};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

/// A download the import turned away: its audio is a different song/version (the fingerprint check), it
/// failed the quality checks, or it is another release than the one asked for. The file is left where it
/// was: the request worker keeps it (an audio check's, for Keep anyway) or removes it.
#[derive(Debug)]
pub struct Rejected {
    /// the check's own words ("audio is “Diase – Sun” (different song)", "Bad quality: …", "Wrong release: …")
    pub why: String,
    /// which check it was
    pub check: Check,
    /// what the fingerprint heard ("Diase – Sun"), for the audio check
    pub heard: Option<String>,
    /// the sentence a person reads ("The audio sounds like Diase – Sun, not Moonlight Shadow")
    pub text: String,
}

/// Which check turned a download away.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    /// the fingerprint: another song or version
    Audio,
    /// the quality checks: fake lossless, upsampled, very quiet, the wrong length
    Quality,
    /// the album's tags: another artist or songs that aren't on it
    Release,
}

impl Check {
    /// Its name in the API ("audio", "quality", "release").
    pub fn as_str(self) -> &'static str {
        match self {
            Check::Audio => "audio",
            Check::Quality => "quality",
            Check::Release => "release",
        }
    }
}

impl Rejected {
    /// A rejection by the fingerprint check.
    fn audio(why: String, heard: Option<String>, text: String) -> Rejected {
        Rejected { why, check: Check::Audio, heard, text }
    }
    /// A rejection by the quality checks.
    pub fn quality(why: String) -> Rejected {
        Rejected { text: why.clone(), why, check: Check::Quality, heard: None }
    }
    /// A rejection by the album's tags.
    fn release(why: String) -> Rejected {
        Rejected { text: why.clone(), why, check: Check::Release, heard: None }
    }
}

impl std::fmt::Display for Rejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.why)
    }
}
impl std::error::Error for Rejected {}

/// "Diase – Sun": the recording a fingerprint check heard (its `actual`).
fn heard_of(c: &Value) -> Option<String> {
    let a = &c["actual"];
    let (artist, title) = (a["artist"].as_str().unwrap_or(""), a["title"].as_str().unwrap_or(""));
    match (artist.is_empty(), title.is_empty()) {
        (_, true) => None,
        (true, false) => Some(title.to_string()),
        (false, false) => Some(format!("{artist} – {title}")),
    }
}

use encore_core::audio::LOSSLESS;

/// A "(feat. …)", "[ft …]" or "(with …)" part of a title.
static FEAT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*[\(\[](feat|ft|with)\.?[^\)\]]*[\)\]]").unwrap());

/// The words of a title or name without its "(feat. …)" part ([`encore_core::text::ascii_words`]).
fn tnorm(s: &str) -> String {
    encore_core::text::ascii_words(&FEAT.replace_all(&s.to_lowercase(), ""))
}

/// How alike two titles or names are, 0..1, without their featured artists.
pub fn sim(a: &str, b: &str) -> f64 {
    let (a, b) = (tnorm(a), tnorm(b));
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    if a == b { 1.0 } else { ratio(&a.chars().collect::<Vec<_>>(), &b.chars().collect::<Vec<_>>()) }
}

/// verified/manual, lossless, bitrate, tagged – higher is better.
pub fn quality(t: &Row) -> (bool, bool, i64, bool) {
    let vs = t.s("verify_status");
    (vs == "verified" || vs == "manual", LOSSLESS.contains(&t.s("format").as_str()), t.i("bitrate"), !t.s("mb_trackid").is_empty())
}

// ---- verification -------------------------------------------------------------------------------

/// Whether an import fingerprints `path` (the setting, an AcoustID key and fpcalc).
pub fn will_verify(st: &impl Ctx, path: &Path) -> bool {
    st.cfg().bool("verify_on_import") && acoustid::can_check(st, path)
}

/// Fingerprint-check `path` when imports verify (None when they don't, or the check failed: logged).
async fn verify(st: &impl Ctx, path: &Path, title: &str, artist: &str, length: Option<f64>, rid: Option<&str>) -> Option<Value> {
    if !will_verify(st, path) {
        return None;
    }
    match acoustid::check_file(st, path, title, artist, length, rid).await {
        Ok(v) => Some(v),
        Err(e) => {
            st.db().log("warning", &format!("Verification of {} failed: {e}", path.file_name().unwrap_or_default().to_string_lossy()));
            None
        }
    }
}

/// Save the fingerprint verdicts on the songs' rows (and queue AcoustID submissions).
fn record_verification(st: &impl Ctx, results: &[(PathBuf, Option<Value>)]) {
    for (p, r) in results {
        if let Some(r) = r {
            let _ = st.db().execute("UPDATE tracks SET verify_status=?, verify_detail=?, acoustid_id=?, verified_at=? WHERE path=?",
                p![r["status"], r["detail"], r["acoustid"], now(), p.to_string_lossy()]);
            let tid = st.db().one("SELECT id FROM tracks WHERE path=?", p![p.to_string_lossy()]).ok().flatten().map(|t| t.i("id"));
            crate::acoustid::queue_acoustid(st, tid, r);
        }
    }
}

// ---- writing ------------------------------------------------------------------------------------

use encore_media::tags::write_tags;

/// Write the tags (and the cover) into a file. A tag the file can't take is only logged; a storage error
/// (the disk under the download or the library failing) stops the import as a [`fsx::StepError`], so the
/// request waits for the storage with its files kept.
fn tag_file(st: &impl Ctx, path: &Path, vals: &Row, cover: Option<&[u8]>) -> Result<(), StepError> {
    let cover = if st.cfg().bool("embed_cover") { cover } else { None };
    fsx::fault_point(Step::Import, "write", path)?;
    match write_tags(path, vals, cover) {
        Ok(()) => Ok(()),
        Err(e) => tag_failed(st, path, e),
    }
}

/// What a failed tag write of an import means: a storage error stops the import (a [`fsx::StepError`]); a
/// tag the file can't take is only logged.
pub fn tag_failed(st: &impl Ctx, path: &Path, e: anyhow::Error) -> Result<(), StepError> {
    if let Some(io) = io_error_of(&e) {
        let se = StepError::new(Step::Import, "write", path, None, io);
        if se.is_storage() {
            return Err(se);
        }
        st.db().log("warning", &format!("Could not tag {}: {se}", path.file_name().unwrap_or_default().to_string_lossy()));
        return Ok(());
    }
    st.db().log("warning", &format!("Could not tag {}: {e:#}", path.file_name().unwrap_or_default().to_string_lossy()));
    Ok(())
}

/// The io error under a tag write's error (lofty's errors give it as their source), as a fresh io::Error.
fn io_error_of(e: &anyhow::Error) -> Option<std::io::Error> {
    for c in e.chain() {
        if let Some(io) = c.downcast_ref::<std::io::Error>() {
            return Some(io.raw_os_error().map(std::io::Error::from_raw_os_error).unwrap_or_else(|| std::io::Error::new(io.kind(), io.to_string())));
        }
    }
    None
}

// ---- cover art ----------------------------------------------------------------------------------

/// An image from `url` (JPEG, PNG or WebP of more than 1.5 KB), else None.
pub async fn get_image(st: &impl Ctx, url: &str) -> Option<Vec<u8>> {
    let r = st.http().get(url).timeout(std::time::Duration::from_secs(15)).send().await.ok()?;
    if !r.status().is_success() {
        return None;
    }
    let b = r.bytes().await.ok()?;
    let ok = b.starts_with(b"\xff\xd8\xff") || b.starts_with(b"\x89PNG") || b.starts_with(b"RIFF");
    (ok && b.len() > 1500).then(|| b.to_vec())
}

/// The Cover Art Archive's front cover of the release, else of the release group.
pub async fn fetch_cover(st: &impl Ctx, release_id: Option<&str>, rgid: Option<&str>) -> Option<Vec<u8>> {
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
pub async fn find_art(st: &impl Ctx, artist: &str, album: Option<&str>, title: Option<&str>, hint: Option<&str>) -> Option<(Vec<u8>, &'static str)> {
    if let Some(h) = hint.filter(|h| !h.is_empty() && !h.contains("ytimg.com")) {
        if let Some(d) = match get_image(st, &big(h)).await { Some(d) => Some(d), None => get_image(st, h).await } {
            return Some((d, "request"));
        }
    }
    for (kind, q) in [("album", album), ("track", title)] {
        let Some(q) = q.filter(|q| !q.is_empty()) else { continue };
        if let Ok(r) = encore_http::deezer::search(st, kind, &format!("artist:\"{artist}\" {kind}:\"{q}\""), 5).send().await {
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
        if let Ok(r) = encore_http::apple::search(st, &format!("{artist} {q}"), ent, 5).send().await {
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

/// A name that is safe as a file or folder name: no reserved or control characters, no trailing dots, at most 120 characters ("_" when nothing is left).
pub fn clean(s: &str) -> String {
    let c: String = s.chars().map(|c| if "<>:\"/\\|?*".contains(c) || (c as u32) < 32 { '_' } else { c }).collect();
    let c = c.trim().trim_end_matches('.').to_string();
    let c: String = c.chars().take(120).collect();
    if c.is_empty() { "_".into() } else { c }
}

/// Fill `{field}` and `{field:02d}` in a naming template; None when a number field is unknown.
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

/// Where a song goes in the library: the naming template (`single_template` for a loose single) filled from its tags.
pub fn render_path(st: &impl Ctx, v: &Row, ext: &str, single: bool) -> PathBuf {
    let tpl = st.cfg().str(if single { "single_template" } else { "naming_template" });
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
    let mut out = PathBuf::from(st.cfg().str("library_path"));
    let parts: Vec<String> = rel.trim().split('/').filter(|p| !p.trim().is_empty()).map(clean).collect();
    for (i, part) in parts.iter().enumerate() {
        out.push(if i + 1 == parts.len() { format!("{part}{}", ext.to_lowercase()) } else { part.clone() });
    }
    out
}

/// How good a file is: (lossless, bitrate, size).
fn file_rank(path: &Path) -> (bool, i64, u64) {
    let t = read_tags(&path.to_string_lossy());
    (LOSSLESS.contains(&t.s("format").as_str()), t.i("bitrate"), std::fs::metadata(path).map(|m| m.len()).unwrap_or(0))
}

/// Only swap files that hold the same recording – a wrong song with the same name never replaces a good file.
async fn may_replace(st: &impl Ctx, src: &Path, dest: &Path) -> bool {
    if !st.cfg().bool("upgrade_check_audio") {
        return true;
    }
    let same = acoustid::same_audio(src, dest).await;
    if same == Some(false) {
        st.db().log("warning", &format!("Kept both {} and the new download – the audio is a different recording", dest.file_name().unwrap_or_default().to_string_lossy()));
    }
    same != Some(false)
}

/// Move a file (a rename, or across file systems a copy that is flushed to the disk before the source goes).
/// A copy that fails leaves no half file at `dest`. Every step's error names its operation and paths.
fn mv(src: &Path, dest: &Path) -> Result<(), StepError> {
    match fsx::rename(src, dest, Step::Import) {
        Ok(()) => return Ok(()),
        // the library is on another file system: copy it over
        Err(e) if e.errno == Some(libc::EXDEV) => {}
        // the source can't be renamed (a read-only download folder, say): copy it over
        Err(e) if !fsx::roots::covers(dest) || !e.errno.is_some_and(fsx::is_storage) => {}
        Err(e) => return Err(e),
    }
    if let Err(e) = fsx::copy(src, dest, Step::Import).and_then(|_| fsx::fsync(dest, Step::Import)) {
        let _ = std::fs::remove_file(dest);
        return Err(e);
    }
    if let Err(e) = fsx::remove_file(src, Step::Import) {
        // the file is in its new place; the old copy stays behind
        tracing::warn!("{e}");
    }
    Ok(())
}

/// The next free name for `dest`: "Song.flac", "Song (2).flac", … (`taken` are names already chosen).
fn free_name(dest: &Path, taken: &std::collections::HashSet<PathBuf>) -> PathBuf {
    let mut d = dest.to_path_buf();
    let stem = dest.file_stem().unwrap_or_default().to_string_lossy().to_string();
    let ext = dest.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    let mut i = 2;
    while d.exists() || taken.contains(&d) {
        d = dest.with_file_name(format!("{stem} ({i}){ext}"));
        i += 1;
    }
    d
}

/// Move src to dest. With replace_same a file already at dest is the same song (same path), so only the
/// better of the two is kept instead of adding "(2)" copies. For moves inside the library (a rename to new
/// tags); a download goes into the library with [`place_all`].
pub async fn move_file(st: &impl Ctx, src: &Path, dest: &Path, replace_same: bool) -> anyhow::Result<PathBuf> {
    if src == dest {
        return Ok(src.to_path_buf());
    }
    fsx::mkdir_all(dest.parent().unwrap(), Step::Import)?;
    if replace_same && dest.exists() && may_replace(st, src, dest).await {
        if file_rank(src) > file_rank(dest) {
            let tmp = PathBuf::from(format!("{}.encore-new", dest.to_string_lossy()));
            mv(src, &tmp)?;
            fsx::rename(&tmp, dest, Step::Import)?;
        } else {
            fsx::remove_file(src, Step::Import)?;
        }
        return Ok(dest.to_path_buf());
    }
    let d = free_name(dest, &Default::default());
    mv(src, &d)?;
    Ok(d)
}

/// What [`place_all`] does with one file.
enum Place {
    /// already where it belongs
    Same,
    /// a new file at this path
    New(PathBuf),
    /// the better copy of the same song: replaces the file at this path
    Replace(PathBuf),
    /// the file at this path is as good: the download's copy is dropped
    Keep(PathBuf),
}

/// The temporary name a file has next to its destination until every file of the import is there.
fn part_name(dest: &Path) -> PathBuf {
    PathBuf::from(format!("{}.encore-part", dest.to_string_lossy()))
}

/// Move downloaded files into the library all together, so a storage error part way never leaves an
/// import half done. Every file is first put next to its destination under a temporary name (a hard link
/// on the same file system, else a copy flushed to the disk), and only when all of them are there are they
/// renamed into place and the downloaded copies removed. A failure on the way removes the temporary files
/// and the files already renamed, and leaves every download where it was: the request keeps its files and
/// the import can run again. `moves` are (download, destination) pairs; with `replace_same` a file already
/// at the destination is the same song and only the better copy stays (as [`move_file`]). → the final paths,
/// in the order of `moves`.
pub async fn place_all(st: &impl Ctx, moves: &[(PathBuf, PathBuf)], replace_same: bool) -> anyhow::Result<Vec<PathBuf>> {
    // planned once no earlier placement of these files still moves them ([`MOVING`])
    let claim = Claim::wait(moves.iter().flat_map(|(s, d)| [s.clone(), d.clone()]).collect()).await;
    let mut taken = std::collections::HashSet::new();
    let mut plan = vec![];
    for (src, dest) in moves {
        if src == dest {
            plan.push(Place::Same);
            continue;
        }
        if replace_same && dest.exists() && !taken.contains(dest) && may_replace(st, src, dest).await {
            taken.insert(dest.clone());
            plan.push(if file_rank(src) > file_rank(dest) { Place::Replace(dest.clone()) } else { Place::Keep(dest.clone()) });
            continue;
        }
        let d = free_name(dest, &taken);
        taken.insert(d.clone());
        plan.push(Place::New(d));
    }
    // the copies (a FLAC album onto another disk takes a while) on the blocking pool, never on a thread that
    // serves the API; as they move they keep the step heard ([`progress::beat`]), so a long copy onto a slow
    // disk never looks stalled, while one stuck on a mount that went away says nothing
    let moves = moves.to_vec();
    let beat = every_few_seconds(progress::beat());
    tokio::task::spawn_blocking(move || {
        let _claim = claim;
        place_planned(&moves, &plan, &beat)
    }).await?
}

/// The downloads and destinations a placement ([`place_all`]) is moving now. Its copies run on the blocking
/// pool, which stopping the import's task (Import again on a stalled import) doesn't reach: a second
/// placement of the same files waits until the first one has really ended, so the two never write the same
/// temporary files or move the same download twice.
static MOVING: LazyLock<std::sync::Mutex<std::collections::HashSet<PathBuf>>> = LazyLock::new(Default::default);

/// Paths taken in [`MOVING`], let go when dropped (where the placement's blocking work ends).
struct Claim(std::collections::HashSet<PathBuf>);

impl Claim {
    /// Take every one of `paths`, or none while another placement has any of them.
    fn take(paths: &std::collections::HashSet<PathBuf>) -> Option<Claim> {
        let mut m = MOVING.lock().unwrap_or_else(|p| p.into_inner());
        if paths.iter().any(|p| m.contains(p)) {
            return None;
        }
        m.extend(paths.iter().cloned());
        Some(Claim(paths.clone()))
    }

    /// Take `paths` once no other placement has any of them, saying the wait while there is one.
    async fn wait(paths: std::collections::HashSet<PathBuf>) -> Claim {
        let mut said = false;
        loop {
            if let Some(c) = Claim::take(&paths) {
                if said {
                    progress::resume();
                }
                return c;
            }
            if !said {
                progress::waiting("waiting for an earlier import of these files to end");
                said = true;
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        let mut m = MOVING.lock().unwrap_or_else(|p| p.into_inner());
        for p in &self.0 {
            m.remove(p);
        }
    }
}

/// `beat` at most once every 5 seconds.
fn every_few_seconds(beat: impl Fn()) -> impl Fn() {
    let last = std::cell::Cell::new(None::<std::time::Instant>);
    move || {
        let now = std::time::Instant::now();
        if last.get().is_none_or(|t| now.duration_since(t) >= std::time::Duration::from_secs(5)) {
            last.set(Some(now));
            beat();
        }
    }
}

/// [`place_all`] that never replaces or drops anything: a file whose destination is taken gets a free name
/// ("Song (2).flac"), and only a source that is now in its new place is removed. For the file manager's
/// import, where nothing is deleted except from Trash.
pub fn place_new(moves: &[(PathBuf, PathBuf)]) -> anyhow::Result<Vec<PathBuf>> {
    let mut taken = std::collections::HashSet::new();
    let plan: Vec<Place> = moves.iter().map(|(src, dest)| {
        if src == dest {
            return Place::Same;
        }
        let d = free_name(dest, &taken);
        taken.insert(d.clone());
        Place::New(d)
    }).collect();
    place_planned(moves, &plan, &|| {})
}

/// The moves of [`place_all`] as planned, `beat` called as they move.
fn place_planned(moves: &[(PathBuf, PathBuf)], plan: &[Place], beat: &dyn Fn()) -> anyhow::Result<Vec<PathBuf>> {
    // 1. every new file next to its destination, under its temporary name
    let mut parts: Vec<PathBuf> = vec![];
    let placed: Result<(), StepError> = (|| {
        for ((src, _), p) in moves.iter().zip(plan) {
            let (Place::New(d) | Place::Replace(d)) = p else { continue };
            fsx::mkdir_all(d.parent().unwrap(), Step::Import)?;
            let part = part_name(d);
            let _ = std::fs::remove_file(&part); // left by an import that stopped part way
            parts.push(part.clone());
            beat();
            if fsx::hard_link(src, &part, Step::Import).is_err() {
                // another file system (or one without hard links): a copy, on the disk before the source goes
                fsx::copy_beating(src, &part, Step::Import, beat)?;
                fsx::fsync(&part, Step::Import)?;
            }
        }
        Ok(())
    })();
    if let Err(e) = placed {
        for p in &parts {
            let _ = std::fs::remove_file(p);
        }
        return Err(e.into());
    }
    // 2. into place
    let mut done: Vec<PathBuf> = vec![];
    for p in plan {
        let (Place::New(d) | Place::Replace(d)) = p else { continue };
        if let Err(e) = fsx::rename(part_name(d), d, Step::Import) {
            for q in &parts {
                let _ = std::fs::remove_file(q);
            }
            // the new files that made it go again (their downloads are still there); a replaced file stays
            // replaced by its better copy
            for (q, was) in done.iter().zip(plan.iter().filter(|x| matches!(x, Place::New(_) | Place::Replace(_)))) {
                if matches!(was, Place::New(_)) {
                    let _ = std::fs::remove_file(q);
                }
            }
            return Err(e.into());
        }
        done.push(d.clone());
    }
    // 3. the downloaded copies go: the files are in the library now, so a copy that can't be removed only
    // stays behind (the download folder's clean-up takes it later)
    let mut out = vec![];
    for ((src, _), p) in moves.iter().zip(plan) {
        match p {
            Place::Same => out.push(src.clone()),
            Place::New(d) | Place::Replace(d) | Place::Keep(d) => {
                if let Err(e) = fsx::remove_file(src, Step::Import) {
                    tracing::warn!("{e}");
                }
                out.push(d.clone());
            }
        }
    }
    Ok(out)
}

/// Imported files into the library ([`library::index_files`]: one transaction per 200 files), then the
/// covers of the albums they landed in.
pub fn index_paths(st: &impl Ctx, paths: &[PathBuf]) {
    index_paths_as(st, paths, library::Added::SCAN)
}

/// [`index_paths`] with who brought the new songs in (a Files upload or import: that admin).
pub fn index_paths_as(st: &impl Ctx, paths: &[PathBuf], added: library::Added) {
    let names: Vec<String> = paths.iter().map(|p| p.to_string_lossy().into_owned()).collect();
    let done = library::index_files_as(st.db(), &names, added);
    for (path, e) in &done.failed {
        st.db().log("warning", &format!("Could not index {path}: {e}"));
    }
    st.library().refresh_covers(st.db().clone(), done.albums.into_iter().collect());
}

/// Files were renamed/moved: keep their library rows (and so likes/playlist entries) by updating the path.
pub fn relink(st: &impl Ctx, old: &[PathBuf], new: &[PathBuf]) {
    for (o, n) in old.iter().zip(new) {
        if o == n {
            continue;
        }
        let (o, n) = (o.to_string_lossy(), n.to_string_lossy());
        if let Ok(Some(clash)) = st.db().one("SELECT id FROM tracks WHERE path=?", p![n]) {
            let _ = st.db().execute("UPDATE playlist_items SET track_id=(SELECT id FROM tracks WHERE path=?) WHERE track_id=?", p![o, clash.i("id")]);
            let _ = st.db().execute("DELETE FROM tracks WHERE id=?", p![clash.i("id")]);
        }
        let _ = st.db().execute("UPDATE tracks SET path=? WHERE path=?", p![n, o]);
    }
}

/// Manual tag edit for library tracks. → the files written
fn manual_key(artist: &str, album: &str) -> String {
    format!("{}\u{1f}{}", artist.trim().to_lowercase(), album.trim().to_lowercase())
}

/// Fixed by hand (Edit details)? Then automatic re-tagging must not touch it.
pub fn is_manual(st: &impl Ctx, artist: &str, album: &str) -> bool {
    st.db().one("SELECT 1 x FROM manual_albums WHERE key=?", p![manual_key(artist, album)]).ok().flatten().is_some()
}

/// Mark an album as fixed by hand, so automatic re-tagging leaves it alone.
pub fn mark_manual(st: &impl Ctx, artist: &str, album: &str) {
    let _ = st.db().execute("INSERT OR REPLACE INTO manual_albums (key, edited_at) VALUES (?,?)", p![manual_key(artist, album), now()]);
}

/// After a hand edit: move the files to where the naming template puts the new names. A song in an extra
/// library folder stays where it is ([`crate::folders::in_library_path`]).
pub async fn rename_to_tags(st: &impl Ctx, paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = vec![];
    for p in paths {
        if !crate::folders::in_library_path(st.cfg(), p) {
            out.push(p.clone());
            continue;
        }
        let tags = read_tags(&p.to_string_lossy());
        let ext = p.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
        let dest = render_path(st, &tags, &ext, false);
        match move_file(st, p, &dest, false).await {
            Ok(n) => {
                if let Some(dir) = p.parent() {
                    library::prune_empty(dir, Path::new(&st.cfg().str("library_path")));
                }
                out.push(n)
            }
            Err(e) => {
                st.db().log("warning", &format!("Could not move {}: {e}", p.display()));
                out.push(p.clone());
            }
        }
    }
    relink(st, paths, &out);
    out
}

/// Write the editable fields (title, artist, album, album artist, numbers, date, genre) into the songs' files → their paths.
pub fn edit_tags(st: &impl Ctx, ids: &[i64], values: &serde_json::Map<String, Value>) -> anyhow::Result<Vec<PathBuf>> {
    const ALLOWED: [&str; 8] = ["title", "artist", "album", "albumartist", "tracknumber", "discnumber", "date", "genre"];
    let vals: Row = values.iter().filter(|(k, v)| ALLOWED.contains(&k.as_str()) && !v.is_null()).map(|(k, v)| (k.clone(), v.clone())).collect();
    let mut out = vec![];
    for id in ids {
        if let Some(t) = st.db().one("SELECT path FROM tracks WHERE id=?", p![id])? {
            let pth = PathBuf::from(t.s("path"));
            write_tags(&pth, &vals, None)?;
            out.push(pth);
        }
    }
    Ok(out)
}

/// Tag an album from a chosen release (any provider) and move it to the naming template.
pub async fn retag_album(st: &impl Ctx, album_id: &str, release_id: &str, rename: Option<bool>) -> anyhow::Result<Vec<PathBuf>> {
    let found = st.db().query("SELECT id, path FROM tracks WHERE album_id=? ORDER BY disc, track, path", p![album_id])?;
    let old: Vec<PathBuf> = found.iter().map(|r| PathBuf::from(r.s("path"))).collect();
    let ids: Vec<i64> = found.iter().map(|r| r.i("id")).collect();
    let pv = preview(st, &old, release_id, false).await?;
    let new = apply_preview(st, &pv, rename.unwrap_or(st.cfg().bool("rename_on_import")), None, None).await?;
    let rows: Vec<PathBuf> = pv["rows"].as_array().cloned().unwrap_or_default().iter().map(|r| PathBuf::from(r["path"].as_str().unwrap_or(""))).collect();
    relink(st, &rows, &new);
    index_paths(st, &new);
    // the owner's type choice follows the album to its new id; the release's facts are the new release's
    crate::album_types::carry(st.db(), album_id, &ids, true);
    record_release(st, &pv, &new);
    let _ = std::fs::remove_file(st.data_dir().join("covers").join(format!("{album_id}.jpg")));
    st.db().log("info", &format!("Tagged album '{}' by {} from {}", pv["release"]["title"].as_str().unwrap_or(""), pv["release"]["artist"].as_str().unwrap_or(""),
        pv["release"]["provider"].as_str().unwrap_or("MusicBrainz")));
    Ok(new)
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

/// The ARTISTS (or ALBUMARTISTS) tag of a MusicBrainz credit (`[{name, mbid}]`, see `encore_http::mb::credit_list`)
/// and the artists' MusicBrainz ids when every one has one (written one value each, in the same order).
pub fn credit_tags(credit: &Value) -> Option<(Value, Option<Value>)> {
    let list = credit.as_array().filter(|a| !a.is_empty())?;
    let names: Vec<Value> = list.iter().filter_map(|c| c["name"].as_str().filter(|n| !n.trim().is_empty()).map(|n| json!(n))).collect();
    let ids: Vec<Value> = list.iter().filter_map(|c| c["mbid"].as_str().filter(|m| encore_core::ids::is_mbid(m)).map(|m| json!(m))).collect();
    if names.len() != list.len() {
        return None;
    }
    let ids = (ids.len() == list.len() && list.len() >= 2).then_some(Value::Array(ids));
    Some((Value::Array(names), ids))
}

/// Adds the ARTISTS / ALBUMARTISTS tags (and every artist's MusicBrainz id) of these credits to `m`.
pub fn insert_credits(m: &mut Row, artists: &Value, albumartists: &Value) {
    for (credit, names, ids) in [(artists, "artists", "musicbrainz_artistid"), (albumartists, "albumartists", "musicbrainz_albumartistid")] {
        if let Some((n, i)) = credit_tags(credit) {
            m.insert(names.into(), n);
            if let Some(i) = i {
                m.insert(ids.into(), i);
            }
        }
    }
}

/// The tags to write for track `t` of release `rel` (a tagger preview's release): names, numbers, date, label, ids and credits.
pub fn release_values(rel: &Value, t: &Value) -> Row {
    let artist = { let a = encore_core::json::text(&t["artist"]); if a.is_empty() { encore_core::json::text(&rel["artist"]) } else { a } };
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
    // every artist of the song and of the album, so "A & B" is two artists in the library
    let song = if t["artists"].as_array().is_some_and(|a| !a.is_empty()) { &t["artists"] } else { &rel["artists"] };
    insert_credits(&mut m, song, &rel["artists"]);
    if rel["provider"].as_str().is_some_and(|p| p != "MusicBrainz") {
        // Deezer/Apple/Discogs metadata: no MusicBrainz ids
        m.retain(|k, _| !k.starts_with("musicbrainz_"));
    } else {
        // the release group's types as Picard writes them ("album; live"), and the compilation flag of a
        // various-artists album: the library's Type filter reads them back, and so do other servers
        let secondary: Vec<String> = rel["secondary"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
        let tag = crate::album_types::releasetype_tag(rel["type"].as_str().unwrap_or(""), &secondary);
        if !tag.is_empty() {
            m.insert("releasetype".into(), json!(tag));
        }
        if crate::album_types::is_various(rel["artist"].as_str().unwrap_or(""), &encore_core::json::text(&rel["artist_id"])) {
            m.insert("compilation".into(), json!("1"));
        }
    }
    m
}

/// The facts of the release a preview tagged these files from, for the albums they landed in (`album_types`:
/// the Type filter, One-offs). Nothing is asked: the release was fetched for the preview.
pub fn record_release(st: &impl Ctx, pv: &Value, paths: &[PathBuf]) {
    let tracks = pv["tracks"].as_array().map(|a| a.len()).unwrap_or(0);
    if let Some(f) = crate::album_types::facts_of_release(&pv["release"], tracks) {
        let albums = crate::album_types::albums_of_paths(st.db(), paths);
        crate::album_types::store_facts(st.db(), &albums, &f);
    }
}

/// Files → MusicBrainz release: per-file new values and paths, plus a confidence.
pub async fn preview(st: &impl Ctx, paths: &[PathBuf], release_id: &str, single: bool) -> anyhow::Result<Value> {
    let rel = crate::meta::release(st, release_id).await?;
    let files: Vec<Row> = paths.iter().map(|p| read_tags(&p.to_string_lossy())).collect();
    let tracks = rel["tracks"].as_array().cloned().unwrap_or_default();
    let (assign, conf) = map_files(&files, &tracks);
    let rows: Vec<Value> = paths.iter().zip(files.iter()).zip(assign).map(|((p, f), j)| {
        let mut row = json!({ "path": p, "old_title": f.s("title"), "old_track": f.get("track"), "old_artist": f.s("artist"), "old_album": f.s("album") });
        if let Some(j) = j {
            let vals = release_values(&rel, &tracks[j]);
            let ext = p.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
            let np = if st.cfg().bool("rename_on_import") { render_path(st, &vals, &ext, single) } else { p.clone() };
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

/// Tag the preview's files and, with `rename`, move them to their `new_path`: from `source_root` (a download
/// or the staging folder) all together, or one by one for songs already in the library. A song in an extra
/// library folder is tagged but stays where it is ([`crate::folders::in_library_path`]).
pub async fn apply_preview(st: &impl Ctx, pv: &Value, rename: bool, source_root: Option<&Path>, hint: Option<&str>) -> anyhow::Result<Vec<PathBuf>> {
    let cover = preview_cover(st, pv, hint).await;
    let mut new_paths = vec![];
    let mut old_dirs = std::collections::HashSet::new();
    // a download goes into the library all together ([`place_all`]): (row, download, destination)
    let mut moves: Vec<(usize, PathBuf, PathBuf)> = vec![];
    for row in pv["rows"].as_array().cloned().unwrap_or_default() {
        let path = PathBuf::from(row["path"].as_str().unwrap_or(""));
        let Some(vals) = row["values"].as_object() else {
            new_paths.push(path);
            continue;
        };
        tag_file(st, &path, vals, cover.as_deref())?;
        // a song in the library is moved only inside `library_path`: one in an extra library folder stays
        let stays = source_root.is_none() && !crate::folders::in_library_path(st.cfg(), &path);
        if rename && !stays {
            old_dirs.insert(path.parent().map(|p| p.to_path_buf()).unwrap_or_default());
            let np = PathBuf::from(row["new_path"].as_str().unwrap_or(""));
            if source_root.is_some() {
                moves.push((new_paths.len(), path.clone(), np));
                new_paths.push(path);
            } else {
                new_paths.push(move_file(st, &path, &np, false).await?);
            }
        } else {
            new_paths.push(path);
        }
    }
    if !moves.is_empty() {
        let pairs: Vec<(PathBuf, PathBuf)> = moves.iter().map(|(_, a, b)| (a.clone(), b.clone())).collect();
        for ((i, _, _), placed) in moves.iter().zip(place_all(st, &pairs, true).await?) {
            new_paths[*i] = placed;
        }
    }
    save_cover_file(st, &new_paths, cover.as_deref());
    let lib = PathBuf::from(st.cfg().str("library_path"));
    for d in old_dirs {
        library::prune_empty(&d, source_root.unwrap_or(&lib));
    }
    Ok(new_paths)
}

/// The cover of a preview's release: its own picture, the Cover Art Archive's, or one found by its names.
pub async fn preview_cover(st: &impl Ctx, pv: &Value, hint: Option<&str>) -> Option<Vec<u8>> {
    let rel = &pv["release"];
    let mut cover = match rel["cover_url"].as_str() { Some(u) => get_image(st, u).await, None => None };
    if cover.is_none() && rel["provider"].as_str().is_none_or(|p| p == "MusicBrainz") {
        cover = fetch_cover(st, rel["id"].as_str(), rel["release_group_id"].as_str()).await;
    }
    if cover.is_none() {
        cover = find_art(st, rel["artist"].as_str().unwrap_or(""), rel["title"].as_str(), None, hint).await.map(|x| x.0);
    }
    cover
}

/// `cover.jpg` beside the songs when they share a folder that has no cover file yet (the `save_cover` setting).
pub fn save_cover_file(st: &impl Ctx, paths: &[PathBuf], cover: Option<&[u8]>) {
    let (Some(c), Some(first)) = (cover, paths.first()) else { return };
    if !st.cfg().bool("save_cover") {
        return;
    }
    let d = first.parent().unwrap();
    if paths.iter().all(|p| p.parent() == Some(d)) && !["cover.jpg", "cover.png", "folder.jpg"].iter().any(|n| d.join(n).exists()) {
        // the songs are in; a cover that can't be saved is only logged
        if let Err(e) = fsx::write(d.join("cover.jpg"), c, Step::Import) {
            st.db().log("warning", &format!("Could not save the cover: {e}"));
            tracing::warn!("{e}");
        }
    }
}

/// Best MusicBrainz release for a set of files, or None when not confident.
pub async fn auto_match(st: &impl Ctx, paths: &[PathBuf], artist: &str, album: &str, rgid: Option<&str>) -> anyhow::Result<Option<Value>> {
    let threshold = st.cfg().float("autotag_threshold");
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

/// How long an import waits for MusicBrainz's tags before it goes on without them (an import's lookups go
/// before a request's in MusicBrainz's line, so this is only reached when MusicBrainz is very busy or slow).
pub const IMPORT_MB_WAIT: std::time::Duration = std::time::Duration::from_secs(300);

/// Every song row with what the import's replace and copy checks read.
fn all_tracks(st: &impl Ctx) -> Vec<Row> {
    st.db().query("SELECT id, path, title, artist, album, album_id, duration, isrc, mb_trackid, format, bitrate, verify_status FROM tracks", p![]).unwrap_or_default()
}

/// Verify, tag (from the MusicBrainz recording if possible), de-duplicate and move one song into the library.
/// `reject`: a failed quality check turns the file away (automatic downloads; the file is deleted, unless
/// `audio` is off); `audio`: so does a fingerprint that says it is another song (off when a person said to
/// take it: Keep anyway, Download anyway, "Don't check the audio"). → (final path, note)
#[allow(clippy::too_many_arguments)]
pub async fn import_single(st: &impl Ctx, path: &Path, artist: &str, title: &str, recording_id: Option<&str>, source_root: &Path,
                           length: Option<f64>, reject: bool, audio: bool, hint: Option<&str>) -> anyhow::Result<(PathBuf, String)> {
    // what it is at shows in the request's message ("Importing – checking the audio": encore_http::progress)
    if will_verify(st, path) {
        progress::step("checking the audio");
    }
    let check = verify(st, path, title, artist, length, recording_id).await;
    if let Some(c) = &check {
        if c["status"] == "mismatch" && audio && st.cfg().bool("reject_mismatch") {
            // the file stays: the request worker keeps it for Keep anyway
            let heard = heard_of(c);
            let text = match &heard {
                Some(h) => format!("The audio sounds like {h}, not {title}"),
                None => format!("The audio is not {title}"),
            };
            return Err(Rejected::audio(c["detail"].as_str().unwrap_or("").into(), heard, text).into());
        }
    }
    // quality checks (encore_media::quality): fake lossless, upsampled, very quiet or the wrong length is turned away
    let (qrep, qres) = encore_media::quality::gate(st, path, length).await;
    if let (Err(why), true) = (qres, reject) {
        // a file a person said to take (`audio` off: Keep anyway keeps it for them) is left to the caller
        if audio {
            let _ = std::fs::remove_file(path);
        }
        return Err(Rejected::quality(why).into());
    }
    let mut vals = Row::new();
    vals.insert("title".into(), json!(title));
    vals.insert("artist".into(), json!(artist));
    vals.insert("albumartist".into(), json!(artist));
    let mut cover = None;
    progress::step("looking it up on MusicBrainz");
    let lookup = async {
        match recording_id.filter(|r| !r.is_empty()) {
            Some(r) => mb::recording(st, r).await.ok(),
            None => match mb::search_recordings(st, title, 5, Some(artist)).await {
                Ok(res) => match res.iter().find(|r| sim(r["title"].as_str().unwrap_or(""), title) > 0.8 && sim(r["artist"].as_str().unwrap_or(""), artist) > 0.6) {
                    Some(r) => mb::recording(st, r["id"].as_str().unwrap_or("")).await.ok(),
                    None => None,
                },
                Err(e) => {
                    st.db().log("warning", &format!("MusicBrainz lookup failed for {artist} - {title}: {e}"));
                    None
                }
            },
        }
    };
    // MusicBrainz's line is long: the song goes in with the names it has, as when MusicBrainz fails
    let rec = match tokio::time::timeout(IMPORT_MB_WAIT, lookup).await {
        Ok(rec) => rec,
        Err(_) => {
            st.db().log("warning", &format!("MusicBrainz was too busy to tag {artist} - {title} – imported with the names it has"));
            None
        }
    };
    // what the release MusicBrainz picked for the song is (a compilation, a remix album …): kept for its album
    // below, never as its release group (the whole compilation would count as in the library)
    let release_facts = rec.as_ref().and_then(crate::album_types::facts_of_recording);
    if let Some(r) = rec {
        for (k, v) in [("title", &r["title"]), ("artist", &r["artist"]), ("albumartist", &r["artist"]), ("album", &r["album"]), ("date", &r["year"]),
                       ("year", &r["year"]), ("isrc", &r["isrc"]), ("musicbrainz_trackid", &r["id"]), ("musicbrainz_artistid", &r["artist_id"]),
                       ("musicbrainz_albumartistid", &r["artist_id"])] {
            vals.insert(k.into(), v.clone());
        }
        insert_credits(&mut vals, &r["artists"], &r["artists"]);
        progress::step("finding the cover");
        cover = fetch_cover(st, r["release_id"].as_str(), r["release_group_id"].as_str()).await;
    }
    if cover.is_none() {
        progress::step("finding the cover");
        let album = vals.s("album");
        cover = find_art(st, &vals.s("artist"), Some(&album), Some(&vals.s("title")), hint).await.map(|x| x.0);
    }
    progress::step("moving into the library");
    let ext = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    // the same song already in the library?
    let mut existing = None;
    if st.cfg().bool("dedupe_on_import") {
        // every song read and indexed on the blocking pool, not on a thread that serves the API
        let s = st.clone();
        let ix = tokio::task::spawn_blocking(move || LibraryIndex::new(all_tracks(&s))).await?;
        let want = describe(&vals.s("title"), &vals.s("artist"), length, Some(&vals.s("isrc")), Some(&vals.s("musicbrainz_trackid")));
        if let Some((hit, _)) = ix.find(&want) {
            existing = Some(hit.clone());
        }
    }
    if let Some(ex) = &existing {
        let ep = PathBuf::from(ex.s("path"));
        if st.cfg().bool("upgrade_check_audio") && ep.is_file() && acoustid::same_audio(path, &ep).await == Some(false) {
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
    tag_file(st, path, &vals, cover.as_deref())?;
    let dest = render_path(st, &vals, &ext, true);
    let new = place_all(st, &[(path.to_path_buf(), dest)], true).await?.remove(0);
    library::prune_empty(path.parent().unwrap(), source_root);
    index_paths(st, std::slice::from_ref(&new));
    if let Some(f) = &release_facts {
        let albums = crate::album_types::albums_of_paths(st.db(), std::slice::from_ref(&new));
        crate::album_types::store_facts(st.db(), &albums, f);
    }
    record_verification(st, &[(new.clone(), check)]);
    encore_media::quality::store(st, &new, &qrep);
    let mut note = String::new();
    if let Some(ex) = existing {
        // a copy in an extra library folder is never deleted (Encore doesn't touch the files there): both stay
        if crate::folders::in_library_path(st.cfg(), Path::new(&ex.s("path"))) {
            let nid = st.db().one("SELECT id FROM tracks WHERE path=?", p![new.to_string_lossy()])?.map(|r| r.i("id"));
            library::delete_tracks(st, &[ex.i("id")], nid);
            note = "replaced a lower-quality copy".into();
        } else {
            note = "kept the copy in an extra library folder too".into();
        }
    }
    Ok((new, note))
}

/// Tag + move a downloaded album. Falls back to the files' own tags (or the store's track list for releases
/// MusicBrainz doesn't have) when there is no confident MusicBrainz match.
#[allow(clippy::too_many_arguments)]
/// Is a downloaded album the one that was asked for? Judged from the tags the files arrived with:
/// Err(reason) when it is clearly something else (another artist, or songs that aren't on the album).
/// Files without usable tags, or names in another script, count as "can't tell" – never a rejection.
pub fn check_release(files: &[Row], artist: &str, album: &str, expected_titles: &[String]) -> Result<(), String> {
    let want_artist = tnorm(artist);
    let skip = |a: &str| { let n = tnorm(a); n.is_empty() || n == "unknown artist" || n == "various artists" || n == "va" };
    let artist_ok = |a: &str| {
        let n = tnorm(a);
        n.contains(&want_artist) || want_artist.contains(&n) || sim(a, artist) >= 0.75
            || a.split([',', '&', ';', '/']).flat_map(|x| x.split(" x ")).any(|part| { let p = tnorm(part); !p.is_empty() && (p == want_artist || sim(part, artist) >= 0.8) })
    };
    let half = (files.len() / 2).max(1);
    if !want_artist.is_empty() {
        let tagged: Vec<String> = files.iter().map(|f| { let aa = f.s("albumartist"); if skip(&aa) { f.s("artist") } else { aa } })
            .filter(|a| !skip(a)).collect();
        if tagged.len() >= half {
            let hits = tagged.iter().filter(|a| artist_ok(a)).count();
            if hits * 10 < tagged.len() * 3 {
                let mut counts: std::collections::HashMap<&str, usize> = Default::default();
                tagged.iter().for_each(|a| *counts.entry(a.as_str()).or_default() += 1);
                let top = counts.into_iter().max_by_key(|x| x.1).map(|x| x.0).unwrap_or("someone else");
                return Err(format!("the download is by “{top}”, not {artist}"));
            }
        }
    }
    let expected: Vec<&String> = expected_titles.iter().filter(|t| !tnorm(t).is_empty()).collect();
    if !expected.is_empty() {
        let titled: Vec<String> = files.iter().map(|f| f.s("title")).filter(|t| !tnorm(t).is_empty()).collect();
        if titled.len() >= half.max(2) {
            let hits = titled.iter().filter(|t| expected.iter().any(|e| sim(t, e) >= 0.75)).count();
            let album_tag_ok = files.iter().any(|f| !tnorm(&f.s("album")).is_empty() && sim(&f.s("album"), album) >= 0.75);
            if hits * 10 < titled.len() * 4 && !album_tag_ok {
                let odd = titled.iter().find(|t| !expected.iter().any(|e| sim(t, e) >= 0.75)).cloned().unwrap_or_default();
                return Err(format!("its songs aren't on “{album}” (e.g. “{odd}”)"));
            }
        }
    }
    Ok(())
}

/// The words of an album name that tell one album from another: accents folded, without what is in brackets,
/// a disc number, edition words ("Deluxe Edition", "Remastered", "EP") and small words ("the", "of"). Empty
/// when nothing is left, or when the name is not in Latin letters (it can't be compared with one that is).
fn album_words(name: &str) -> std::collections::BTreeSet<String> {
    static BRACKETS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\(\[\{][^\)\]\}]*[\)\]\}]").unwrap());
    static DISC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(cd|disc|disk)\s*\d+\b").unwrap());
    const SMALL: &[&str] = &["the", "a", "an", "of", "and", "deluxe", "edition", "expanded", "remaster", "remastered",
        "anniversary", "reissue", "special", "collector", "collectors", "bonus", "track", "tracks", "version", "ep", "lp",
        "single", "album", "vol", "volume", "explicit", "clean", "mono", "stereo"];
    const NO_NAME: &[&str] = &["unknown", "unknown album", "untitled", "no album", "non album", "non album tracks", "singles"];
    let folded = crate::matcher::fold(&DISC.replace_all(&BRACKETS.replace_all(name, " "), " "));
    if NO_NAME.contains(&folded.as_str()) || !folded.is_ascii() {
        return Default::default();
    }
    folded.split_whitespace().filter(|w| !SMALL.contains(w)).map(String::from).collect()
}

/// A word that numbers an album in a series: "II", "IV", "2" (not a year).
fn numbers_an_album(w: &str) -> bool {
    const ROMAN: &[&str] = &["ii", "iii", "iv", "vi", "vii", "viii", "ix", "xi", "xii", "xiii", "xiv", "xv", "xvi", "xvii", "xviii", "xix", "xx"];
    ROMAN.contains(&w) || (w.len() <= 2 && w.chars().all(|c| c.is_ascii_digit()))
}

/// For a download with no track list to compare against, the album tags are what tells another album of the
/// right artist ("Homework" for "Discovery") from the one asked for. It is turned away only when that is
/// clear: at least half the files carry an album tag that can be compared, and none of them shares a word with
/// the album asked for (nor is close to it letter by letter: "Ilmatic"), or each adds or lacks a number in the
/// series ("Led Zeppelin II" for "Led Zeppelin"). Editions, a year or the artist's name in the tag ("Daft
/// Punk - Discovery (Deluxe) 2001", "50 Cent - Get Rich or Die Tryin'") still count as the album.
pub fn check_album_tags(files: &[Row], artist: &str, album: &str) -> Result<(), String> {
    let want = album_words(album);
    if want.is_empty() {
        return Ok(());
    }
    let by = album_words(artist);
    // a tag in another script, "Unknown Album", or only the artist's name can't say which album it is
    let tags: Vec<String> = files.iter().map(|f| f.s("album")).filter(|a| { let w = album_words(a); !w.is_empty() && !w.is_subset(&by) }).collect();
    if tags.len() < (files.len() / 2).max(1) {
        return Ok(());
    }
    let another = |tag: &str| {
        let have = album_words(tag);
        if have.is_disjoint(&want) {
            return sim(tag, album) < 0.6;
        }
        have.symmetric_difference(&want).any(|w| numbers_an_album(w) && !by.contains(w))
    };
    if !tags.iter().all(|t| another(t)) {
        return Ok(());
    }
    let mut counts: std::collections::HashMap<&str, usize> = Default::default();
    tags.iter().for_each(|t| *counts.entry(t.as_str()).or_default() += 1);
    let top = counts.into_iter().max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(a.0))).map(|x| x.0).unwrap_or_default();
    Err(format!("the download is the album “{top}”, not “{album}”"))
}

/// What `import_album_files` is importing: the album asked for and where the files came from.
pub struct AlbumImport<'a> {
    /// the artist asked for
    pub artist: &'a str,
    /// the album asked for
    pub album: &'a str,
    /// the MusicBrainz release group, when known
    pub rgid: Option<&'a str>,
    /// the download folder the files sit in
    pub source_root: &'a Path,
    /// refuse a wrong release or a failed quality check (automatic downloads) instead of importing it as it is
    pub reject: bool,
    /// refuse files the fingerprint says are other songs (off when a person said to take the download:
    /// Keep anyway, Download anyway, "Don't check the audio")
    pub audio: bool,
    /// the source's own album listing (its track titles count as the album's)
    pub source_meta: Option<&'a Value>,
    /// a cover image URL the source gave
    pub hint: Option<&'a str>,
}

/// Import an album's downloaded files: check them, tag them (from MusicBrainz when found), move them into the library and index them → their library paths.
pub async fn import_album_files(st: &impl Ctx, paths: &[PathBuf], what: AlbumImport<'_>) -> anyhow::Result<Vec<PathBuf>> {
    let AlbumImport { artist, album, rgid, source_root, reject, audio, source_meta, hint } = what;
    let mut paths = paths.to_vec();
    paths.sort();
    // what it is at shows in the request's message ("Importing – reading tags": encore_http::progress)
    progress::step("reading tags");
    // what the files say they are, before anything is rewritten
    let originals: Vec<Row> = paths.iter().map(|p| read_tags(&p.to_string_lossy())).collect();
    let mut source_meta = source_meta;
    if let Err(why) = check_release(&originals, artist, album, &[]) {
        if reject {
            return Err(Rejected::release(format!("Wrong release: {why}")).into());
        }
        // a manual pick: import it as what it is, never relabelled as the request
        source_meta = None;
    }
    let pv = if st.cfg().bool("autotag_on_import") {
        progress::step("looking it up on MusicBrainz");
        match tokio::time::timeout(IMPORT_MB_WAIT, auto_match(st, &paths, artist, album, rgid)).await {
            Ok(Ok(p)) => p,
            Ok(Err(e)) => {
                st.db().log("warning", &format!("Auto-tag failed for {artist} - {album}: {e}"));
                None
            }
            // MusicBrainz's line is long: the album goes in with its own tags, as when MusicBrainz fails
            Err(_) => {
                st.db().log("warning", &format!("MusicBrainz was too busy to tag {artist} - {album} – imported with the files' own tags"));
                None
            }
        }
    } else {
        None
    };
    // the songs must belong to the album we're about to label them as
    let expected: Vec<String> = match &pv {
        Some(pv) => pv["rows"].as_array().cloned().unwrap_or_default().iter().filter_map(|r| r["values"]["title"].as_str().map(String::from)).collect(),
        None => source_meta.and_then(|m| m["tracks"].as_array().cloned()).unwrap_or_default().iter().filter_map(|t| t["title"].as_str().map(String::from)).collect(),
    };
    let mut pv = pv;
    // with no track list, the album tags are what tells another album of the same artist from this one
    let release = check_release(&originals, artist, album, &expected)
        .and_then(|()| if expected.is_empty() { check_album_tags(&originals, artist, album) } else { Ok(()) });
    if let Err(why) = release {
        if reject {
            return Err(Rejected::release(format!("Wrong release: {why}")).into());
        }
        pv = None;
        source_meta = None;
    }
    // quality checks (encore_media::quality): most of the files fake lossless / upsampled / very quiet → turned away
    let (qreports, qres) = encore_media::quality::gate_album(st, &paths).await;
    if let (Err(why), true) = (qres, reject) {
        return Err(Rejected::quality(format!("Bad quality: {why}")).into());
    }
    if let Some(pv) = pv {
        let mut checks = vec![];
        let mut mism = vec![];
        // the first file that is another song: its title and what it sounds like
        let mut first: Option<(String, Option<String>)> = None;
        let rows = pv["rows"].as_array().cloned().unwrap_or_default();
        for (i, row) in rows.iter().enumerate() {
            let Some(v) = row["values"].as_object() else { continue };
            let p = PathBuf::from(row["path"].as_str().unwrap_or(""));
            if will_verify(st, &p) {
                progress::step(&format!("checking the audio ({} of {})", i + 1, rows.len()));
            }
            let res = verify(st, &p, &v.s("title"), &v.s("artist"), None, Some(&v.s("musicbrainz_trackid"))).await;
            if let Some(r) = &res {
                if r["status"] == "mismatch" {
                    mism.push(format!("{}: {}", v.s("title"), r["detail"].as_str().unwrap_or("")));
                    first.get_or_insert_with(|| (v.s("title"), heard_of(r)));
                }
            }
            checks.push((p, res));
        }
        if !mism.is_empty() && audio && st.cfg().bool("reject_mismatch") && mism.len() > (paths.len() / 4).max(1) {
            let (title, heard) = first.unwrap_or_default();
            let text = match &heard {
                Some(h) => format!("The audio of {} of {} files is another song – “{title}” sounds like {h}", mism.len(), paths.len()),
                None => format!("The audio of {} of {} files is another song", mism.len(), paths.len()),
            };
            return Err(Rejected::audio(format!("{} of {} files are not what they claim – e.g. {}", mism.len(), paths.len(), mism[0]), heard, text).into());
        }
        progress::step("moving into the library");
        let new = apply_preview(st, &pv, true, Some(source_root), hint).await?;
        let rows = pv["rows"].as_array().cloned().unwrap_or_default();
        let results: Vec<(PathBuf, Option<Value>)> = rows.iter().zip(new.iter()).map(|(row, np)| {
            let old = PathBuf::from(row["path"].as_str().unwrap_or(""));
            (np.clone(), checks.iter().find(|(p, _)| *p == old).and_then(|(_, r)| r.clone()))
        }).collect();
        index_paths(st, &new);
        record_release(st, &pv, &new);
        record_verification(st, &results);
        for (row, np) in rows.iter().zip(new.iter()) {
            let old = PathBuf::from(row["path"].as_str().unwrap_or(""));
            if let Some((_, q)) = qreports.iter().find(|(p, _)| *p == old) {
                encore_media::quality::store(st, np, q);
            }
        }
        drop_superseded(st, &new).await;
        return Ok(new);
    }
    // the requested album's cover only goes on files that are tagged as that album
    let cover = if source_meta.is_some() || originals.iter().any(|t| sim(&t.s("album"), album) >= 0.75) {
        progress::step("finding the cover");
        find_art(st, artist, Some(album), None, hint).await.map(|x| x.0)
    } else {
        None
    };
    progress::step("moving into the library");
    let src_tracks: Vec<Value> = source_meta.and_then(|m| m["tracks"].as_array().cloned()).unwrap_or_default();
    let unique_numbers = src_tracks.iter().map(|x| x["track"].as_i64()).collect::<std::collections::HashSet<_>>().len() == src_tracks.len();
    let mut out = vec![];
    let mut moves = vec![];
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
                .or_else(|| (src_tracks.len() == paths.len() && tnorm(&t.s("title")).is_empty()).then(|| &src_tracks[i]))
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
            tag_file(st, p, &w, cover.as_deref())?;
        } else if cover.is_some() {
            tag_file(st, p, &Row::new(), cover.as_deref())?;
        }
        let ext = p.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
        moves.push((p.clone(), render_path(st, &vals, &ext, false)));
    }
    // all of the album or none of it ([`place_all`])
    out.extend(place_all(st, &moves, true).await?);
    for p in &paths {
        library::prune_empty(p.parent().unwrap(), source_root);
    }
    save_cover_file(st, &out, cover.as_deref());
    st.db().log("warning", &format!("No confident MusicBrainz match for {artist} - {album}; imported with existing tags"));
    index_paths(st, &out);
    for (np, (_, q)) in out.iter().zip(qreports.iter()) {
        encore_media::quality::store(st, np, q);
    }
    drop_superseded(st, &out).await;
    Ok(out)
}

/// A copy an album import makes redundant (see [`superseded`]).
pub struct Superseded {
    /// the copy that goes, and its path
    pub old: i64,
    /// its path
    pub old_path: PathBuf,
    /// the imported song that takes its place (its playlist entries and likes)
    pub new: i64,
    /// a loose single (an album of one song), else a lower-quality copy
    pub loose: bool,
    /// its artist
    pub artist: String,
    /// its title
    pub title: String,
}

/// The copies an album import makes redundant: loose singles of the same recordings and lower-quality
/// copies, each once. Only songs in `library_path` are ever dropped: a song in an extra library folder stays
/// where and as it is ([`crate::folders::in_library_path`]), next to the new copy.
pub async fn superseded(st: &impl Ctx, new_paths: &[PathBuf]) -> Vec<Superseded> {
    let mut out: Vec<Superseded> = vec![];
    if !st.cfg().bool("dedupe_on_import") {
        return out;
    }
    let new_set: std::collections::HashSet<String> = new_paths.iter().map(|p| p.to_string_lossy().into_owned()).collect();
    let sizes: std::collections::HashMap<String, i64> = st.db().query("SELECT album_id, COUNT(*) n FROM tracks GROUP BY album_id", p![]).unwrap_or_default()
        .into_iter().map(|r| (r.s("album_id"), r.i("n"))).collect();
    let others: Vec<Row> = all_tracks(st).into_iter()
        .filter(|t| !new_set.contains(&t.s("path")) && crate::folders::in_library_path(st.cfg(), Path::new(&t.s("path")))).collect();
    if others.is_empty() {
        return out;
    }
    let ix = LibraryIndex::new(others);
    for p in new_paths {
        let Ok(Some(t)) = st.db().one("SELECT id, title, artist, duration, isrc, mb_trackid, format, bitrate, verify_status, path FROM tracks WHERE path=?", p![p.to_string_lossy()]) else { continue };
        let want = describe(&t.s("title"), &t.s("artist"), Some(t.f("duration")), Some(&t.s("isrc")), Some(&t.s("mb_trackid")));
        let Some((hit, _)) = ix.find(&want) else { continue };
        let hit = hit.clone();
        if out.iter().any(|s| s.old == hit.i("id")) {
            continue;
        }
        let loose = sizes.get(&hit.s("album_id")).copied().unwrap_or(0) == 1;
        let better = quality(&t) > quality(&hit);
        if !(loose || better) {
            continue;
        }
        let hp = PathBuf::from(hit.s("path"));
        if st.cfg().bool("upgrade_check_audio") && hp.is_file() && acoustid::same_audio(p, &hp).await == Some(false) {
            continue; // same song by name, different audio: keep both
        }
        out.push(Superseded { old: hit.i("id"), old_path: hp, new: t.i("id"), loose, artist: hit.s("artist"), title: hit.s("title") });
    }
    out
}

/// After an album import, remove copies it makes redundant ([`superseded`]). Playlists follow the new file.
pub async fn drop_superseded(st: &impl Ctx, new_paths: &[PathBuf]) {
    for s in superseded(st, new_paths).await {
        library::delete_tracks(st, &[s.old], Some(s.new));
        st.db().log("info", &format!("Removed {} of “{} – {}” – replaced by the new album import", if s.loose { "loose single" } else { "lower-quality copy" }, s.artist, s.title));
    }
}

/// Albums imported from Deezer/Apple data: once the release is in MusicBrainz, tag them properly.
pub async fn retag_external(st: &impl Ctx) -> usize {
    // songs in `library_path` only (`path > 'lib/' AND path < 'lib0'`): one in an extra library folder is kept as it is
    let lib = st.cfg().str("library_path").trim().trim_end_matches('/').to_string();
    let rows = st.db().query("SELECT t.album_id, MIN(t.albumartist) artist, MIN(t.album) album FROM tracks t JOIN requests r ON r.id=t.request_id
        WHERE (t.mb_albumid IS NULL OR t.mb_albumid='') AND r.mbid IS NULL AND r.detail LIKE '%\"ext\"%' AND t.path > ? AND t.path < ?
        GROUP BY t.album_id LIMIT 25", p![format!("{lib}/"), format!("{lib}0")]).unwrap_or_default();
    let mut done = 0;
    for a in rows {
        if is_manual(st, &a.s("artist"), &a.s("album")) {
            continue; // fixed by hand
        }
        let paths: Vec<PathBuf> = st.db().query("SELECT path FROM tracks WHERE album_id=?", p![a.s("album_id")]).unwrap_or_default().iter().map(|r| PathBuf::from(r.s("path"))).collect();
        if paths.iter().any(|p| !crate::folders::in_library_path(st.cfg(), p)) {
            continue; // in an extra library folder: kept where and as it is
        }
        let res: anyhow::Result<bool> = async {
            let Some(pv) = auto_match(st, &paths, &a.s("artist"), &a.s("album"), None).await? else { return Ok(false) };
            let ids: Vec<i64> = st.db().query("SELECT id FROM tracks WHERE album_id=?", p![a.s("album_id")])?.iter().map(|r| r.i("id")).collect();
            let new = apply_preview(st, &pv, true, None, None).await?;
            relink(st, &paths, &new);
            index_paths(st, &new);
            crate::album_types::carry(st.db(), &a.s("album_id"), &ids, true);
            record_release(st, &pv, &new);
            Ok(true)
        }.await;
        match res {
            Ok(true) => {
                st.db().log("info", &format!("{} – {} is in MusicBrainz now: re-tagged", a.s("artist"), a.s("album")));
                done += 1;
            }
            Ok(false) => {}
            Err(e) => st.db().log("warning", &format!("Re-tag of {} failed: {e}", a.s("album"))),
        }
    }
    done
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wrong_release_is_caught() {
        use encore_db::Row;
        let f = |artist: &str, album: &str, title: &str| -> Row {
            let mut r = Row::new();
            r.insert("artist".into(), serde_json::json!(artist));
            r.insert("albumartist".into(), serde_json::json!(artist));
            r.insert("album".into(), serde_json::json!(album));
            r.insert("title".into(), serde_json::json!(title));
            r
        };
        let right = vec![f("Daft Punk", "Discovery", "One More Time"), f("Daft Punk", "Discovery", "Aerodynamic")];
        assert!(super::check_release(&right, "Daft Punk", "Discovery", &["One More Time".into(), "Aerodynamic".into()]).is_ok());
        let other = vec![f("Justice", "Cross", "Genesis"), f("Justice", "Cross", "Let There Be Light")];
        assert!(super::check_release(&other, "Daft Punk", "Discovery", &[]).is_err());
        let wrong_songs = vec![f("Daft Punk", "Homework", "Da Funk"), f("Daft Punk", "Homework", "Around the World")];
        assert!(super::check_release(&wrong_songs, "Daft Punk", "Discovery", &["One More Time".into(), "Aerodynamic".into()]).is_err());
        let untagged = vec![Row::new(), Row::new()];
        assert!(super::check_release(&untagged, "Daft Punk", "Discovery", &["One More Time".into()]).is_ok());
        let feat = vec![f("TAK feat. Hatsune Miku", "PPPP", "PPPP")];
        assert!(super::check_release(&feat, "TAK", "PPPP", &[]).is_ok());
    }

    #[test]
    fn another_album_of_the_artist_is_caught_by_its_tags() {
        use encore_db::Row;
        let tagged = |album: &str, n: usize| -> Vec<Row> {
            (0..n).map(|i| {
                let mut r = Row::new();
                r.insert("artist".into(), serde_json::json!("Daft Punk"));
                r.insert("album".into(), serde_json::json!(album));
                r.insert("title".into(), serde_json::json!(format!("Song {i}")));
                r
            }).collect()
        };
        let check = |album_tag: &str, artist: &str, album: &str| super::check_album_tags(&tagged(album_tag, 4), artist, album);
        // clearly another album: turned away, naming it
        assert_eq!(check("Homework", "Daft Punk", "Discovery").unwrap_err(), "the download is the album “Homework”, not “Discovery”");
        assert!(check("Alive 2007", "Daft Punk", "Discovery").is_err());
        assert!(check("25", "Adele", "21").is_err());
        assert!(check("Led Zeppelin II", "Led Zeppelin", "Led Zeppelin").is_err(), "a self-titled album is not its sequel");
        assert!(check("Led Zeppelin II", "Led Zeppelin", "Led Zeppelin IV").is_err());
        assert!(check("Greatest Hits Vol. 2", "Queen", "Greatest Hits").is_err());
        // the same album, however it is written
        for tag in ["Discovery", "Discovery (Deluxe Edition)", "Discovery [2001 Remaster]", "Daft Punk - Discovery", "2001 - Discovery",
                    "discovery", "Discovery CD1", "Discovery - Disc 1", "Discovery Remastered 2021"] {
            assert!(check(tag, "Daft Punk", "Discovery").is_ok(), "{tag}");
        }
        assert!(check("Rodeo", "Travis Scott", "Rodeo (Expanded Edition)").is_ok());
        assert!(check("Illuminate", "Duumu", "Illuminate EP").is_ok());
        assert!(check("Ilmatic", "Nas", "Illmatic").is_ok(), "a typo is not another album");
        assert!(check("Canción Animal", "Soda Stereo", "Cancion Animal").is_ok());
        assert!(check("Led Zeppelin IV (Remaster)", "Led Zeppelin", "Led Zeppelin IV").is_ok());
        assert!(check("IV", "Led Zeppelin", "Led Zeppelin IV").is_ok());
        assert!(check("50 Cent - Get Rich or Die Tryin'", "50 Cent", "Get Rich or Die Tryin'").is_ok());
        assert!(check("Greatest Hits, Volume 2", "Queen", "Greatest Hits Vol. 2").is_ok());
        // tags that can't say: another script, no name, the artist's name, too few files tagged
        assert!(check("ディスカバリー", "Daft Punk", "Discovery").is_ok());
        assert!(check("Discovery", "Daft Punk", "ディスカバリー").is_ok());
        assert!(check("Unknown Album", "Daft Punk", "Discovery").is_ok());
        assert!(check("Daft Punk", "Daft Punk", "Discovery").is_ok());
        let mut few = tagged("Homework", 1);
        few.extend((0..3).map(|_| Row::new()));
        assert!(super::check_album_tags(&few, "Daft Punk", "Discovery").is_ok());
        // one file tagged with the album asked for is enough
        let mut mixed = tagged("Homework", 3);
        mixed.extend(tagged("Discovery", 1));
        assert!(super::check_album_tags(&mixed, "Daft Punk", "Discovery").is_ok());
    }

    /// A placement of files another placement still moves waits until that one has really ended (its claim
    /// goes with its blocking work, not with the import's task), and a beat is passed on at most every 5 s.
    #[tokio::test]
    async fn a_placement_waits_for_an_earlier_one_of_the_same_files() {
        let d = std::env::temp_dir().join(format!("encore-claim-{}", std::process::id()));
        let set = |names: &[&str]| names.iter().map(|n| d.join(n)).collect::<std::collections::HashSet<_>>();
        let first = Claim::take(&set(&["a.flac", "lib/a.flac"])).expect("nothing moves yet");
        assert!(Claim::take(&set(&["a.flac", "b.flac"])).is_none(), "the same download");
        assert!(Claim::take(&set(&["c.flac", "lib/a.flac"])).is_none(), "the same destination");
        drop(Claim::take(&set(&["b.flac", "lib/b.flac"])).expect("other files"));
        let second = tokio::spawn(Claim::wait(set(&["a.flac", "lib/a.flac"])));
        tokio::time::sleep(std::time::Duration::from_millis(700)).await;
        assert!(!second.is_finished(), "waits while the first one moves");
        // the first placement's blocking work ends (its import's task was stopped long ago)
        std::thread::spawn(move || drop(first)).join().unwrap();
        let second = tokio::time::timeout(std::time::Duration::from_secs(5), second).await.expect("goes on once it ended").unwrap();
        drop(second);
        drop(Claim::take(&set(&["a.flac", "lib/a.flac"])).expect("all let go"));

        let n = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let n2 = n.clone();
        let beat = every_few_seconds(move || { n2.fetch_add(1, std::sync::atomic::Ordering::SeqCst); });
        for _ in 0..100 {
            beat();
        }
        assert_eq!(n.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn templates() {
        let data: std::collections::HashMap<&str, String> = [("albumartist", "A".to_string()), ("album", "B".into()), ("year", "2020".into()), ("discprefix", "".into()), ("title", "T".into())].into_iter().collect();
        let nums: std::collections::HashMap<&str, i64> = [("track", 3), ("disc", 1)].into_iter().collect();
        assert_eq!(format_template("{albumartist}/{album} ({year})/{discprefix}{track:02d} - {title}", &data, &nums).unwrap(), "A/B (2020)/03 - T");
        assert_eq!(clean("AC/DC: Live?"), "AC_DC_ Live_");
        assert!(sim("Song (feat. X)", "song") == 1.0);
    }
}
