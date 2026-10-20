//! A file's tags (§4.12, §4.13 of the client shapes): the eight fields of the Fix song dialog, read from the
//! file and written through the library's tag writer (`encore_media::tags::write_tags_to`, the handle form of
//! what `tagger::edit_tags` uses), with "clear on empty": a field sent as "" is removed from the file's tag
//! (the writer itself skips empty values, which would leave the old tag). The file is opened once, without
//! following a link, and read and written through that handle (its format told from its bytes), so it can't
//! be swapped in between. A file of the library gets its row read again from the file, as a rescan of that
//! one file does.
use crate::resolve::{open_file, Target};
use crate::roots::LibraryDirs;
use crate::Who;
use encore_core::error::{err, ApiError};
use encore_db::Row;
use encore_library::Ctx;
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::tag::{Accessor, ItemKey};
use serde_json::{json, Map, Value};
use std::fs::File;

/// The fields, in the order the editor shows them (the keys `tagger::edit_tags` takes).
pub const FIELDS: [&str; 8] = ["title", "artist", "album", "albumartist", "tracknumber", "discnumber", "date", "genre"];

fn item_keys(field: &str) -> &'static [ItemKey] {
    match field {
        "title" => &[ItemKey::TrackTitle],
        "artist" => &[ItemKey::TrackArtist],
        "album" => &[ItemKey::AlbumTitle],
        "albumartist" => &[ItemKey::AlbumArtist],
        "date" => &[ItemKey::RecordingDate, ItemKey::Year],
        "genre" => &[ItemKey::Genre],
        _ => &[],
    }
}

/// The eight fields as the file's own tag has them (missing = "").
pub fn read(file: &mut File) -> Result<Map<String, Value>, ApiError> {
    let f = lofty::read_from(file).map_err(|e| err(422, format!("Couldn't read the tags: {e}")))?;
    let tag = f.primary_tag().or_else(|| f.first_tag());
    let mut out = Map::new();
    for field in FIELDS {
        let v = match (tag, field) {
            (None, _) => String::new(),
            (Some(t), "tracknumber") => t.track().map(|n| n.to_string()).unwrap_or_default(),
            (Some(t), "discnumber") => t.disk().map(|n| n.to_string()).unwrap_or_default(),
            (Some(t), f) => item_keys(f).iter().find_map(|k| t.get_string(*k).map(|s| s.trim().to_string()).filter(|s| !s.is_empty())).unwrap_or_default(),
        };
        out.insert(field.to_string(), json!(v));
    }
    Ok(out)
}

/// Removes the fields from the file's tag (the "clear on empty" half of a write).
fn clear(file: &mut File, fields: &[String]) -> anyhow::Result<()> {
    let mut f = lofty::read_from(file)?;
    let Some(tag) = f.primary_tag_mut() else { return Ok(()) };
    for field in fields {
        match field.as_str() {
            "tracknumber" => tag.remove_track(),
            "discnumber" => tag.remove_disk(),
            other => {
                for k in item_keys(other) {
                    tag.remove_key(*k);
                }
            }
        }
    }
    f.save_to(file, lofty::config::WriteOptions::default())?;
    Ok(())
}

/// `GET /api/files/tags?root=&path=` → `{tags: {…}}`.
pub fn get(t: &Target) -> Result<Value, ApiError> {
    let mut file = open_file(t, false)?;
    Ok(json!({ "tags": read(&mut file)? }))
}

/// `PUT /api/files/tags {root, path, tags}` → `{ok: true}`: only the fields sent are changed; "" clears one.
pub fn put(st: &impl Ctx, who: &Who, t: &Target, tags: &Map<String, Value>) -> Result<Value, ApiError> {
    if let Some(k) = tags.keys().find(|k| !FIELDS.contains(&k.as_str())) {
        return Err(err(400, format!("Unknown tag: {k}")));
    }
    let mut set = Row::new();
    let mut cleared = vec![];
    for (k, v) in tags {
        let s = match v {
            Value::String(s) => s.trim().to_string(),
            Value::Number(n) => n.to_string(),
            Value::Null => String::new(),
            _ => return Err(err(400, format!("{k} must be text"))),
        };
        if (k == "tracknumber" || k == "discnumber") && !s.is_empty() && s.parse::<u32>().is_err() {
            return Err(err(400, format!("{k} must be a number")));
        }
        if s.is_empty() {
            cleared.push(k.clone());
        } else {
            set.insert(k.clone(), json!(s));
        }
    }
    if t.root.kind == crate::roots::RootKind::Trash {
        return Err(err(403, "Files in the trash can't be changed; restore them first"));
    }
    let _g = crate::lock();
    let mut file = open_file(t, true)?;
    if !set.is_empty() {
        encore_media::tags::write_tags_to(&mut file, &set, None).map_err(|e| err(500, format!("Couldn't write the tags: {e:#}")))?;
    }
    if !cleared.is_empty() {
        use std::io::Seek;
        file.rewind().map_err(|e| err(500, format!("Couldn't write the tags: {e}")))?;
        clear(&mut file, &cleared).map_err(|e| err(500, format!("Couldn't write the tags: {e:#}")))?;
    }
    file.sync_all().map_err(|e| err(500, format!("Couldn't write the tags: {e}")))?;
    drop(file);
    // a song of the library: its row read again from the file (one transaction, one library version move)
    let dirs = LibraryDirs::of(st.cfg());
    let real = t.real()?;
    if let Some(row_path) = dirs.db_path(&real) {
        if let Err(e) = encore_library::index_file(st.db(), &row_path.to_string_lossy()) {
            tracing::warn!("files: reading {} again after a tag edit: {e:#}", row_path.display());
        }
        if let Ok(Some(r)) = st.db().one("SELECT album_id FROM tracks WHERE path=?", encore_db::p![row_path.to_string_lossy()]) {
            st.library().refresh_covers(st.db().clone(), vec![encore_db::RowExt::s(&r, "album_id")]);
        }
    }
    crate::audit(st, who, "edited the tags of", t, None);
    Ok(json!({ "ok": true }))
}
