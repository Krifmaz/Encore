//! Tags inside audio files (lofty): reading a song's tags the way the library stores them, its embedded
//! picture, and writing tags and the front cover.
use encore_db::Row;
use lofty::config::WriteOptions;
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::tag::{Accessor, ItemKey, Tag};
use serde_json::json;
use std::path::Path;

fn first(tag: Option<&Tag>, keys: &[ItemKey]) -> Option<String> {
    let tag = tag?;
    keys.iter().find_map(|k| tag.get_string(*k).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()))
}

fn num(s: Option<String>) -> Option<i64> {
    s.and_then(|v| v.split('/').next().and_then(|n| n.trim().parse().ok()))
}

/// Tags of one file, same shape as on older servers (missing artist/album fall back to the folder names).
pub fn read_tags(path: &str) -> Row {
    let tagged = lofty::read_from_path(path).ok();
    let tag = tagged.as_ref().and_then(|t| t.primary_tag().or_else(|| t.first_tag()));
    let stem = Path::new(path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let title = first(tag, &[ItemKey::TrackTitle]).unwrap_or(stem);
    let mut artist = first(tag, &[ItemKey::TrackArtist]);
    let mut album = first(tag, &[ItemKey::AlbumTitle]);
    let mut albumartist = first(tag, &[ItemKey::AlbumArtist]).or(artist.clone());
    if artist.is_none() {
        let parts: Vec<String> = Path::new(path).components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
        if parts.len() >= 3 {
            artist = Some(parts[parts.len() - 3].clone());
            albumartist = artist.clone();
            album = album.or(Some(parts[parts.len() - 2].clone()));
        }
    }
    let props = tagged.as_ref().map(|t| t.properties());
    let duration = props.map(|p| (p.duration().as_secs_f64() * 100.0).round() / 100.0).unwrap_or(0.0);
    let bitrate = props.and_then(|p| p.overall_bitrate().or(p.audio_bitrate())).unwrap_or(0);
    let year: String = first(tag, &[ItemKey::RecordingDate, ItemKey::OriginalReleaseDate, ItemKey::Year]).unwrap_or_default().chars().take(4).collect();
    let isrc = first(tag, &[ItemKey::Isrc]).map(|s| s.to_uppercase().replace('-', "")).filter(|s| !s.is_empty());
    let mut m = Row::new();
    m.insert("title".into(), json!(title));
    m.insert("artist".into(), json!(artist.clone().unwrap_or_else(|| "Unknown Artist".into())));
    m.insert("album".into(), json!(album));
    m.insert("albumartist".into(), json!(albumartist.or(artist).unwrap_or_else(|| "Unknown Artist".into())));
    m.insert("track".into(), json!(num(first(tag, &[ItemKey::TrackNumber])).or_else(|| tag.and_then(|t| t.track()).map(|n| n as i64))));
    m.insert("disc".into(), json!(num(first(tag, &[ItemKey::DiscNumber])).or_else(|| tag.and_then(|t| t.disk()).map(|n| n as i64)).unwrap_or(1)));
    m.insert("year".into(), json!(year));
    m.insert("genre".into(), json!(first(tag, &[ItemKey::Genre])));
    m.insert("duration".into(), json!(duration));
    m.insert("bitrate".into(), json!(bitrate));
    m.insert("format".into(), json!(Path::new(path).extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()));
    m.insert("mb_trackid".into(), json!(first(tag, &[ItemKey::MusicBrainzRecordingId])));
    m.insert("mb_albumid".into(), json!(first(tag, &[ItemKey::MusicBrainzReleaseId])));
    m.insert("mb_releasegroupid".into(), json!(first(tag, &[ItemKey::MusicBrainzReleaseGroupId])));
    m.insert("mb_artistid".into(), json!(first(tag, &[ItemKey::MusicBrainzArtistId])));
    m.insert("mb_albumartistid".into(), json!(first(tag, &[ItemKey::MusicBrainzReleaseArtistId])));
    m.insert("isrc".into(), json!(isrc));
    m
}

/// The first picture embedded in the file (the album art), as stored.
pub fn embedded_art(path: &str) -> Option<Vec<u8>> {
    let tagged = lofty::read_from_path(path).ok()?;
    tagged.tags().iter().find_map(|t| t.pictures().first().map(|p| p.data().to_vec()))
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
        let s = encore_core::json::text(v);
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
