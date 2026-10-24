//! Bring your own music: a song's file replaced with one someone uploads (⋯ → Replace with my own file), or a
//! batch of files matched to the songs they are by their tags (Library → Upload your own files).
//!
//! An upload is first *staged*: streamed to `<config>/own-files/<token>.<ext>` with a note beside it
//! (`<token>.json`: who sent it, its name, when), checked to be audio ([`check_audio`]) and, for a batch,
//! matched to the library ([`find_match`]). Staged files nobody used are removed after a day
//! ([`purge_staged`]). [`replace`] then puts a staged file in the song's place:
//!
//! 1. the new file is copied next to the old one under a hidden name and tagged from the song
//!    (`encore_media::tags::retag_like`: the old file's tags and pictures, the library's values on top);
//! 2. the old file goes to `<config>/trash` as `<unix>-<id>-<name>`, the way a reported song's does, and is
//!    deleted after 7 days by the same clean-up (encore_requests::report::purge_trash);
//! 3. the new file takes the old one's name with its own extension, so the lyrics files beside it (same name,
//!    `.lrc` / `.txt`) still belong to it;
//! 4. the song's row is pointed at the new file and read in again *under the same id*: plays, likes,
//!    playlists, shares and resume spots all point at that id, so they stay. Its loudness is measured again,
//!    its quality and identity checks run again, and it is marked as someone's own file (`track_origin` kind
//!    `upload`, `source` "Your own file", a row in `own_files`) – which the Signal path shows.
use crate::Ctx;
use encore_core::audio::AUDIO_EXTS;
use encore_db::{now, p, Row, RowExt};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// The folder staged uploads wait in, under the config folder.
pub const STAGE_DIR: &str = "own-files";
/// Seconds a staged upload nobody used is kept.
const STAGE_KEEP_SECS: f64 = 86400.0;
/// What `tracks.source` says for a song whose file someone uploaded (the album page's "⬇ Your own file").
pub const SOURCE: &str = "Your own file";
/// The `track_origin.kind` of an uploaded file.
pub const ORIGIN_KIND: &str = "upload";
/// A match at least this sure is ticked in a batch upload; below it the file is shown but left out.
pub const SURE: f64 = 0.85;

/// Why a replacement or a staged upload was refused, with the HTTP status the route answers.
#[derive(Debug, Clone, PartialEq)]
pub struct Refused {
    /// 400 (not audio, a bad token), 403 (someone else's upload), 404 (no such song), 409 (the song can't be
    /// replaced now), 500 (the disk)
    pub status: u16,
    /// what to tell the person
    pub detail: String,
}

/// A refusal with `status` and `detail`.
fn refuse(status: u16, detail: impl Into<String>) -> Refused {
    Refused { status, detail: detail.into() }
}

// ---- staging ----------------------------------------------------------------------------------------

/// Where staged uploads wait (made when missing).
pub fn stage_dir(st: &impl Ctx) -> PathBuf {
    st.data_dir().join(STAGE_DIR)
}

/// Is `token` one this module made (hex, 32 characters)? Anything else never names a file.
pub fn valid_token(token: &str) -> bool {
    token.len() == 32 && token.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The lower-case extension of an uploaded file's name when it is an audio format the library reads
/// (`encore_core::audio::AUDIO_EXTS`), else None.
pub fn audio_ext(name: &str) -> Option<String> {
    let ext = Path::new(name).extension()?.to_string_lossy().to_ascii_lowercase();
    AUDIO_EXTS.contains(&ext.as_str()).then_some(ext)
}

/// A new staged upload for `name`: its token and the path to write its bytes to. Refused (400) when the name
/// isn't an audio file's.
pub fn stage_new(st: &impl Ctx, name: &str) -> Result<(String, PathBuf), Refused> {
    let ext = audio_ext(name).ok_or_else(|| refuse(400, format!("“{name}” isn't an audio file Encore plays ({})", AUDIO_EXTS.join(", "))))?;
    let dir = stage_dir(st);
    std::fs::create_dir_all(&dir).map_err(|e| refuse(500, format!("Couldn't make {}: {e}", dir.display())))?;
    let token = encore_core::ids::random_hex(16);
    Ok((token.clone(), dir.join(format!("{token}.{ext}"))))
}

/// The bytes of a staged upload arrived (`path`, as [`stage_new`] gave it): it is checked to be audio and its
/// note is written. → the staged file as the API shows it ([`staged_json`]). A file that isn't audio is
/// removed and refused (400).
pub fn stage_done(st: &impl Ctx, user_id: i64, token: &str, name: &str, path: &Path) -> Result<Value, Refused> {
    if let Err(why) = check_audio(path) {
        let _ = std::fs::remove_file(path);
        return Err(refuse(400, format!("“{name}”: {why}")));
    }
    let note = json!({ "user_id": user_id, "name": name, "path": path.to_string_lossy(), "at": now() });
    std::fs::write(stage_dir(st).join(format!("{token}.json")), note.to_string()).map_err(|e| refuse(500, format!("Couldn't keep “{name}”: {e}")))?;
    Ok(staged_json(token, name, path))
}

/// A staged upload as the API shows it: `{token, name, size, format, tags: {artist, title, album, track,
/// disc, duration, isrc, mb_trackid}}`.
pub fn staged_json(token: &str, name: &str, path: &Path) -> Value {
    let t = encore_media::tags::read_tags(&path.to_string_lossy());
    json!({
        "token": token, "name": name, "size": std::fs::metadata(path).map(|m| m.len()).unwrap_or(0), "format": t.s("format"),
        "tags": { "artist": t.s("artist"), "title": t.s("title"), "album": t.s("album"), "track": t.i("track"), "disc": t.i("disc"),
                  "duration": t.f("duration"), "isrc": t.s("isrc"), "mb_trackid": t.s("mb_trackid") },
    })
}

/// A staged upload of `user_id` by its token → (its file, the name it was sent as). Refused when the token
/// is malformed or unknown (400), the upload someone else's (403) or its file gone (400).
pub fn staged(st: &impl Ctx, user_id: i64, token: &str) -> Result<(PathBuf, String), Refused> {
    if !valid_token(token) {
        return Err(refuse(400, "That upload isn't here any more – upload the file again"));
    }
    let note: Value = std::fs::read_to_string(stage_dir(st).join(format!("{token}.json"))).ok().and_then(|s| serde_json::from_str(&s).ok())
        .ok_or_else(|| refuse(400, "That upload isn't here any more – upload the file again"))?;
    if note["user_id"].as_i64() != Some(user_id) {
        return Err(refuse(403, "That upload is someone else's"));
    }
    let path = PathBuf::from(note["path"].as_str().unwrap_or(""));
    // the note names a file of the staging folder only
    if path.parent() != Some(stage_dir(st).as_path()) || !path.is_file() {
        return Err(refuse(400, "That upload isn't here any more – upload the file again"));
    }
    Ok((path, note["name"].as_str().unwrap_or("").to_string()))
}

/// Removes a staged upload (its file and note).
pub fn unstage(st: &impl Ctx, token: &str) {
    if !valid_token(token) {
        return;
    }
    if let Ok(rd) = std::fs::read_dir(stage_dir(st)) {
        for e in rd.flatten() {
            if e.file_name().to_string_lossy().starts_with(&format!("{token}.")) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

/// Removes staged uploads older than a day (and part files of uploads that stopped). → how many files went
pub fn purge_staged(st: &impl Ctx) -> usize {
    let cutoff = std::time::SystemTime::now() - std::time::Duration::from_secs_f64(STAGE_KEEP_SECS);
    let mut n = 0;
    if let Ok(rd) = std::fs::read_dir(stage_dir(st)) {
        for e in rd.flatten() {
            let old = e.metadata().and_then(|m| m.modified()).map(|m| m < cutoff).unwrap_or(true);
            if old && std::fs::remove_file(e.path()).is_ok() {
                n += 1;
            }
        }
    }
    n
}

// ---- is it audio --------------------------------------------------------------------------------------

/// What [`check_audio`] found in a file.
#[derive(Debug, Clone, PartialEq)]
pub struct Audio {
    /// seconds
    pub duration: f64,
    /// Hz
    pub sample_rate: u32,
    /// how many channels
    pub channels: u8,
}

/// Is the file at `path` audio Encore can play? Its extension must be one of the library's
/// (`AUDIO_EXTS`), and lofty must read it as that format with a length, a sample rate and channels – a text
/// file or an image renamed to .flac is refused. → what was found, or why not (in words for the person).
pub fn check_audio(path: &Path) -> Result<Audio, String> {
    use lofty::file::AudioFile;
    let name = path.to_string_lossy();
    if audio_ext(&name).is_none() {
        return Err("not an audio file Encore plays".into());
    }
    let f = lofty::read_from_path(path).map_err(|_| "this isn't audio (or the file is damaged)".to_string())?;
    let props = f.properties();
    let audio = Audio { duration: props.duration().as_secs_f64(), sample_rate: props.sample_rate().unwrap_or(0), channels: props.channels().unwrap_or(0) };
    if audio.duration <= 0.0 || audio.sample_rate == 0 || audio.channels == 0 {
        return Err("this file has no audio in it".into());
    }
    Ok(audio)
}

// ---- matching a file to a song --------------------------------------------------------------------------

/// The library's songs a file's tags (`tags`, as `encore_media::tags::read_tags` gives them) may be, best
/// first, at most `limit`: `[{track: {id, title, artist, album, track, disc, duration, format}, score, why}]`.
/// The same MusicBrainz recording or ISRC is sure; otherwise the version-aware matcher (crate::matcher)
/// compares the artist, the title (a remix is never the original) and the length, and the same album and
/// track number decide between the album's copy and the single's.
pub fn find_match(st: &impl Ctx, tags: &Row, limit: usize) -> Vec<Value> {
    use crate::matcher::{compare, describe, fold, sim, split_artists};
    let (title, artist) = (tags.s("title"), tags.s("artist"));
    let length = Some(tags.f("duration")).filter(|d| *d > 0.0);
    let want = describe(&title, &artist, length, Some(&tags.s("isrc")).filter(|s| !s.is_empty()).map(String::as_str),
        Some(&tags.s("mb_trackid")).filter(|s| !s.is_empty()).map(String::as_str));
    let cols = "id, title, artist, albumartist, album, track, disc, duration, format, isrc, mb_trackid";
    let mut rows: Vec<Row> = vec![];
    // exact ids first (cheap, and sure)
    for (col, v) in [("mb_trackid", tags.s("mb_trackid")), ("isrc", want.isrc.clone().unwrap_or_default())] {
        if !v.is_empty() {
            rows.extend(st.db().query(&format!("SELECT {cols} FROM tracks WHERE {col}=? LIMIT 20"), p![v]).unwrap_or_default());
        }
    }
    // then the songs of any artist the file names: each distinct artist text is folded once
    let wanted: Vec<String> = split_artists(&artist);
    if !wanted.is_empty() {
        let artists = st.db().query("SELECT DISTINCT artist FROM tracks", p![]).unwrap_or_default();
        let hits: Vec<String> = artists.iter().map(|r| r.s("artist")).filter(|a| {
            let theirs = split_artists(a);
            theirs.iter().any(|x| wanted.iter().any(|y| x == y || sim(x, y) >= 0.85 || (x.len().min(y.len()) >= 4 && (x.contains(y.as_str()) || y.contains(x.as_str())))))
        }).take(50).collect();
        for a in hits {
            rows.extend(st.db().query(&format!("SELECT {cols} FROM tracks WHERE artist=? LIMIT 2000"), p![a]).unwrap_or_default());
        }
    }
    let album = fold(&tags.s("album"));
    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<(f64, Value)> = rows.into_iter().filter(|r| seen.insert(r.i("id"))).filter_map(|r| {
        let theirs = describe(&r.s("title"), &r.s("artist"), Some(r.f("duration")), Some(&r.s("isrc")).filter(|s| !s.is_empty()).map(String::as_str),
            Some(&r.s("mb_trackid")).filter(|s| !s.is_empty()).map(String::as_str));
        let v = compare(&want, &theirs, false);
        if !v.matched && v.score < 0.5 {
            return None;
        }
        // the same album, and then the same place on it, break a tie between copies of one song
        let mut score = v.score;
        if !album.is_empty() && sim(&album, &fold(&r.s("album"))) >= 0.85 {
            score += 0.01;
            if tags.i("track") > 0 && tags.i("track") == r.i("track") && tags.i("disc").max(1) == r.i("disc").max(1) {
                score += 0.005;
            }
        }
        let score = if v.matched { score.min(1.0) } else { score.min(SURE - 0.01) };
        Some((score, json!({
            "track": { "id": r.i("id"), "title": r.s("title"), "artist": r.s("artist"), "album": r.s("album"), "track": r.i("track"),
                       "disc": r.i("disc"), "duration": r.f("duration"), "format": r.s("format") },
            "score": (score * 1000.0).round() / 1000.0, "why": v.reason,
        })))
    }).collect();
    out.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    out.into_iter().take(limit).map(|(_, v)| v).collect()
}

/// A staged upload with what it may be: [`staged_json`] plus `match` (the best song when it is sure,
/// [`SURE`], and no other song is as sure; else null) and `candidates` (the best three).
pub fn staged_with_match(st: &impl Ctx, staged: Value, path: &Path) -> Value {
    let tags = encore_media::tags::read_tags(&path.to_string_lossy());
    let cands = find_match(st, &tags, 3);
    let sure = |c: &Value| c["score"].as_f64().unwrap_or(0.0) >= SURE;
    // two songs the file is equally sure to be (two copies of one song, say) are left for the person
    let best = cands.first().filter(|c| sure(c) && !cands.get(1).is_some_and(|d| sure(d) && d["score"] == c["score"])).cloned();
    let mut v = staged;
    v["match"] = best.unwrap_or(Value::Null);
    v["candidates"] = Value::Array(cands);
    v
}

// ---- the replacement ----------------------------------------------------------------------------------

/// The library's values of a song as tags (`encore_media::tags::write_tags` names): the words, numbers, ids
/// and credits the song has in the library, which the new file gets whatever it said itself.
pub fn tag_values(t: &Row) -> Row {
    let mut m = Row::new();
    let mut put = |k: &str, v: Value| {
        let empty = v.is_null() || v.as_str().is_some_and(|s| s.is_empty()) || v.as_i64() == Some(0);
        if !empty {
            m.insert(k.to_string(), v);
        }
    };
    put("title", json!(t.s("title")));
    put("artist", json!(t.s("artist")));
    put("album", json!(t.s("album")));
    put("albumartist", json!(t.s("albumartist")));
    put("tracknumber", json!(t.i("track")));
    put("discnumber", json!(t.i("disc")));
    put("date", json!(t.s("year")));
    put("genre", json!(t.s("genre")));
    put("musicbrainz_trackid", json!(t.s("mb_trackid")));
    put("musicbrainz_albumid", json!(t.s("mb_albumid")));
    put("musicbrainz_releasegroupid", json!(t.s("mb_releasegroupid")));
    put("musicbrainz_artistid", json!(t.s("mb_artistid")));
    put("musicbrainz_albumartistid", json!(t.s("mb_albumartistid")));
    put("isrc", json!(t.s("isrc")));
    put("releasetype", json!(t.s("releasetype")));
    if t.i("compilation") == 1 {
        put("compilation", json!("1"));
    }
    // the credits ("[{name, mbid}]" as text): ARTISTS / ALBUMARTISTS, one value each
    for (col, key) in [("artist_credit", "artists"), ("albumartist_credit", "albumartists")] {
        let names: Vec<String> = serde_json::from_str::<Value>(&t.s(col)).ok().and_then(|v| v.as_array().cloned()).unwrap_or_default()
            .iter().filter_map(|c| c["name"].as_str().map(String::from)).filter(|n| !n.is_empty()).collect();
        if !names.is_empty() {
            put(key, json!(names));
        }
    }
    m
}

/// Moves a file, copying (and then deleting) across file systems; the folder is made.
fn mv(src: &Path, dest: &Path) -> std::io::Result<()> {
    if let Some(d) = dest.parent() {
        std::fs::create_dir_all(d)?;
    }
    if std::fs::rename(src, dest).is_ok() {
        return Ok(());
    }
    std::fs::copy(src, dest)?;
    std::fs::remove_file(src)
}

/// Where a song's new file goes: the old file's folder and name with the new file's extension, so the
/// lyrics files beside it (same name) still belong to it.
pub fn new_path(old: &Path, ext: &str) -> PathBuf {
    old.with_extension(ext)
}

/// Replaces song `track_id`'s file with `file` (a staged upload, sent as `name`) for `user` (see the module's
/// notes). The upload is consumed: moved into the library, or left staged when the replacement is refused.
/// → `{track, before, after, warnings, trash_path}`: the song's row now, the old and new files' `{format,
/// bitrate, sample_rate, bit_depth, size, duration}`, and plain-words notes (the lengths differ a lot).
pub fn replace(st: &impl Ctx, user: &Row, track_id: i64, file: &Path, name: &str) -> Result<Value, Refused> {
    if crate::surround::is_track(track_id) {
        return Err(refuse(409, "A surround version can't be replaced this way"));
    }
    let t = st.db().one("SELECT * FROM tracks WHERE id=?", p![track_id]).ok().flatten().ok_or_else(|| refuse(404, "Song not found"))?;
    let audio = check_audio(file).map_err(|why| refuse(400, format!("“{name}”: {why}")))?;
    let ext = audio_ext(&file.to_string_lossy()).unwrap_or_default();
    let old = PathBuf::from(t.s("path"));
    let dest = new_path(&old, &ext);
    // another song's file under the new name is never touched (a .flac twin beside this .mp3, say)
    if dest != old && dest.exists() {
        return Err(refuse(409, format!("There is already a file called {} next to this song – rename or remove it first", dest.file_name().unwrap_or_default().to_string_lossy())));
    }
    let folder = old.parent().map(Path::to_path_buf).ok_or_else(|| refuse(409, "The song's file has no folder"))?;
    std::fs::create_dir_all(&folder).map_err(|e| refuse(500, format!("Couldn't open {}: {e}", folder.display())))?;
    let before = file_facts(st, &old, &t);

    // 1. the new file beside the old one under a hidden name (hidden: the scan passes it by), tagged as the song
    let stem = old.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "song".into());
    let tmp = folder.join(format!(".{stem}.{}.own-upload.{ext}", encore_core::ids::random_hex(4)));
    std::fs::copy(file, &tmp).map_err(|e| refuse(500, format!("Couldn't write into {}: {e}", folder.display())))?;
    let undo_tmp = || {
        let _ = std::fs::remove_file(&tmp);
    };
    if let Err(e) = encore_media::tags::retag_like(Some(&old).filter(|o| o.is_file()).map(PathBuf::as_path), &tmp, &tag_values(&t)) {
        undo_tmp();
        return Err(refuse(400, format!("Couldn't write the song's tags into “{name}”: {e}")));
    }

    // 2. the old file to the trash, kept 7 days (the report flow's clean-up deletes it after that)
    let at = now();
    let trash = st.data_dir().join("trash");
    let trashed = if old.is_file() {
        let fname = old.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
        let to = trash.join(format!("{}-{track_id}-{fname}", at as i64));
        if let Err(e) = mv(&old, &to) {
            undo_tmp();
            return Err(refuse(500, format!("Couldn't move the old file to the trash: {e}")));
        }
        Some(to)
    } else {
        None
    };

    // 3. the new file under the old name, with its own extension
    if let Err(e) = std::fs::rename(&tmp, &dest) {
        undo_tmp();
        if let Some(to) = &trashed {
            let _ = mv(to, &old);
        }
        return Err(refuse(500, format!("Couldn't put the new file in place: {e}")));
    }
    // the staged upload is used up: its file and the note beside it
    let _ = std::fs::remove_file(file);
    if file.parent() == Some(stage_dir(st).as_path()) {
        let _ = std::fs::remove_file(file.with_extension("json"));
    }

    // 4. the same row, read in again from the new file: the id (plays, likes, playlists …) stays
    let db = st.db();
    let _ = db.execute(&format!("UPDATE tracks SET path=?, verify_status=NULL, verify_detail=NULL, verified_at=NULL, source=?, source_detail=?, source_url=NULL, downloaded_at=?, {} WHERE id=?",
        crate::loudness::COLUMNS.iter().map(|c| format!("{c}=NULL")).collect::<Vec<_>>().join(", ")),
        p![dest.to_string_lossy(), SOURCE, format!("uploaded by {}", user.s("username")), at, track_id]);
    if let Err(e) = crate::index_file(db, &dest.to_string_lossy()) {
        tracing::warn!("own file: reading in {}: {e:#}", dest.display());
    }
    let _ = db.execute("DELETE FROM track_quality WHERE track_id=?", p![track_id]);
    let _ = db.execute("INSERT OR REPLACE INTO track_origin (track_id, kind, key, label) VALUES (?,?,?,?)",
        p![track_id, ORIGIN_KIND, format!("upload:{track_id}:{}", at as i64), format!("{SOURCE}, uploaded by {}", user.s("username"))]);
    let _ = db.execute("INSERT INTO own_files (track_id, user_id, name, old_path, trash_path, old_format, new_format, created_at) VALUES (?,?,?,?,?,?,?,?)",
        p![track_id, user.i("id"), name, old.to_string_lossy(), trashed.as_ref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(), t.s("format"), ext, at]);
    st.track_removed(track_id);
    st.library().forget_cover(None);
    let now_row = db.one("SELECT * FROM tracks WHERE id=?", p![track_id]).ok().flatten().unwrap_or_default();
    let after = file_facts(st, &dest, &now_row);
    let mut warnings = vec![];
    if before["duration"].as_f64().unwrap_or(0.0) > 0.0 {
        let d = (audio.duration - before["duration"].as_f64().unwrap_or(0.0)).abs();
        if d > 10.0 {
            warnings.push(format!("Your file is {}s {} than the one it replaced – check it is the same version", d.round() as i64,
                if audio.duration > before["duration"].as_f64().unwrap_or(0.0) { "longer" } else { "shorter" }));
        }
    }
    db.log("info", &format!("{} replaced the file of {} – {} with their own ({name})", user.s("username"), t.s("artist"), t.s("title")));
    Ok(json!({ "track": Value::Object(now_row), "before": before, "after": after, "warnings": warnings,
               "trash_path": trashed.map(|p| p.to_string_lossy().into_owned()) }))
}

/// A file's `{format, bitrate, sample_rate, bit_depth, size, duration}` (from its row `t`, the file's size
/// from the disk when it is there).
fn file_facts(_st: &impl Ctx, path: &Path, t: &Row) -> Value {
    json!({
        "format": t.s("format"), "bitrate": t.i("bitrate"), "sample_rate": t.i("sample_rate"), "bit_depth": t.i("bit_depth"),
        "size": std::fs::metadata(path).map(|m| m.len() as i64).unwrap_or(t.i("size")), "duration": t.f("duration"),
    })
}

/// The newest replacement of song `track_id` with someone's own file: `{by, at, name, old_format}` (`by` the
/// username), or None when the song's file isn't one (or a later download replaced it since: its origin
/// is no longer an upload).
pub fn latest(st: &impl Ctx, track_id: i64) -> Option<Value> {
    let origin = st.db().one("SELECT kind FROM track_origin WHERE track_id=?", p![track_id]).ok().flatten()?;
    if origin.s("kind") != ORIGIN_KIND {
        return None;
    }
    let r = st.db().one("SELECT o.*, u.username FROM own_files o LEFT JOIN users u ON u.id=o.user_id WHERE o.track_id=? ORDER BY o.id DESC LIMIT 1", p![track_id]).ok().flatten()?;
    Some(json!({ "by": r.s("username"), "at": r.f("created_at"), "name": r.s("name"), "old_format": r.s("old_format") }))
}

#[cfg(test)]
#[path = "own_file_tests.rs"]
mod tests;
