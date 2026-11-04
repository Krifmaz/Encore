//! Tags inside audio files (lofty): reading a song's tags the way the library stores them, with what the
//! scan keeps beside them (the ReplayGain / R128 loudness tags, the sample rate and bit depth: [`AudioFacts`]),
//! its embedded picture, and writing tags and the front cover.
use encore_db::Row;
use lofty::config::WriteOptions;
use lofty::file::{AudioFile, TaggedFile, TaggedFileExt};
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::tag::{Accessor, ItemKey, Tag};
use serde_json::json;
use std::path::Path;

/// The first non-empty value of `keys` in `tag`.
fn first(tag: Option<&Tag>, keys: &[ItemKey]) -> Option<String> {
    let tag = tag?;
    keys.iter().find_map(|k| tag.get_string(*k).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()))
}

/// Every value of a multi-value tag (ARTISTS, MUSICBRAINZ_ARTISTID …), in order, without blanks or repeats.
fn all(tag: Option<&Tag>, key: ItemKey) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for v in tag.into_iter().flat_map(|t| t.get_strings(key)).flat_map(|v| v.split('\0')) {
        let v = v.trim();
        if !v.is_empty() && !out.iter().any(|x| x == v) {
            out.push(v.to_string());
        }
    }
    out
}

/// The MusicBrainz artist ids of a tag that holds several (one per value, or joined with "/" or ";").
fn ids(tag: Option<&Tag>, key: ItemKey) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for v in all(tag, key).iter().flat_map(|v| v.split(['/', ';', ','])) {
        let v = v.trim().to_lowercase();
        if encore_core::ids::is_mbid(&v) && !out.contains(&v) {
            out.push(v);
        }
    }
    out
}

/// The artists a song (or its album) credits, from the multi-value ARTISTS / ALBUMARTISTS tags MusicBrainz
/// Picard and Encore write, with their MusicBrainz ids when the id tag lists as many: `[{name, mbid?}]`.
/// Empty when the file has no such tag (the library splits the artist text itself then).
pub fn credit_of(names: &[String], ids: &[String]) -> serde_json::Value {
    let paired = names.len() == ids.len();
    serde_json::Value::Array(names.iter().enumerate().map(|(i, n)| {
        if paired { json!({ "name": n, "mbid": ids[i] }) } else { json!({ "name": n }) }
    }).collect())
}

/// The number before a `/` (`3/12` is 3).
fn num(s: Option<String>) -> Option<i64> {
    s.and_then(|v| v.split('/').next().and_then(|n| n.trim().parse().ok()))
}

/// Tags of one file, same shape as on older servers (missing artist/album fall back to the folder names).
pub fn read_tags(path: &str) -> Row {
    tags_of(lofty::read_from_path(path).ok().as_ref(), path)
}

/// [`read_tags`] and the file's [`AudioFacts`], from one read of the file (the scan).
pub fn read_tags_and_facts(path: &str) -> (Row, AudioFacts) {
    let tagged = lofty::read_from_path(path).ok();
    let facts = facts_of(tagged.as_ref(), path);
    (tags_of(tagged.as_ref(), path), facts)
}

/// The library row of a file's tags; the title falls back to the file name.
fn tags_of(tagged: Option<&TaggedFile>, path: &str) -> Row {
    let tag = tagged.and_then(|t| t.primary_tag().or_else(|| t.first_tag()));
    let stem = Path::new(path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let title = first(tag, &[ItemKey::TrackTitle]).unwrap_or(stem);
    let mut artist = first(tag, &[ItemKey::TrackArtist]);
    let mut album = first(tag, &[ItemKey::AlbumTitle]);
    let mut albumartist = first(tag, &[ItemKey::AlbumArtist]).or_else(|| artist.clone());
    if artist.is_none() {
        let parts: Vec<String> = Path::new(path).components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
        if parts.len() >= 3 {
            artist = Some(parts[parts.len() - 3].clone());
            albumartist = artist.clone();
            album = album.or_else(|| Some(parts[parts.len() - 2].clone()));
        }
    }
    let props = tagged.map(|t| t.properties());
    let mut duration = props.map(|p| (p.duration().as_secs_f64() * 100.0).round() / 100.0).unwrap_or(0.0);
    // lofty reads no length from some files (a fragmented MP4 that could not be made plain, see crate::mp4):
    // ffprobe measures it, so the song never sits in the library at 0:00 with a seek bar that can't move
    if duration <= 0.0 && tagged.is_some() {
        duration = crate::probe::stream_info(path).map(|s| (s.duration * 100.0).round() / 100.0).unwrap_or(0.0);
    }
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
    // the first artist's id (a tag joining several with "/" gives that one)
    m.insert("mb_artistid".into(), json!(ids(tag, ItemKey::MusicBrainzArtistId).into_iter().next().or_else(|| first(tag, &[ItemKey::MusicBrainzArtistId]))));
    m.insert("mb_albumartistid".into(), json!(ids(tag, ItemKey::MusicBrainzReleaseArtistId).into_iter().next().or_else(|| first(tag, &[ItemKey::MusicBrainzReleaseArtistId]))));
    m.insert("isrc".into(), json!(isrc));
    // the release's type, as Picard and the stores write it ('' / 0 when the file has none: read, so a later
    // pass doesn't read it again)
    let (releasetype, tracktotal, compilation) = type_tags(tag);
    m.insert("releasetype".into(), json!(releasetype));
    m.insert("tracktotal".into(), json!(tracktotal));
    m.insert("compilation".into(), json!(compilation));
    let (artist_ids, aa_ids) = (ids(tag, ItemKey::MusicBrainzArtistId), ids(tag, ItemKey::MusicBrainzReleaseArtistId));
    // stored as text: "[]" means the file was read and has no credit tags
    m.insert("artist_credit".into(), json!(credit_of(&all(tag, ItemKey::TrackArtists), &artist_ids).to_string()));
    m.insert("albumartist_credit".into(), json!(credit_of(&all(tag, ItemKey::AlbumArtists), &aa_ids).to_string()));
    m
}

/// (RELEASETYPE, track total, compilation flag) of a tag: the release type as written ("album; live"; '' when
/// absent), TRACKTOTAL / TOTALTRACKS or the "/n" of the track number (0 when absent), COMPILATION / TCMP / cpil
/// (1 or 0).
fn type_tags(tag: Option<&Tag>) -> (String, i64, i64) {
    let releasetype = first(tag, &[ItemKey::MusicBrainzReleaseType]).unwrap_or_default();
    let total = first(tag, &[ItemKey::TrackTotal]).and_then(|v| v.trim().parse::<i64>().ok())
        .or_else(|| first(tag, &[ItemKey::TrackNumber]).and_then(|v| v.split_once('/').and_then(|(_, n)| n.trim().parse::<i64>().ok())))
        .filter(|n| *n > 0)
        .unwrap_or(0);
    let compilation = first(tag, &[ItemKey::FlagCompilation])
        .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false) as i64;
    (releasetype, total, compilation)
}

// ---- loudness tags, sample rate and bit depth ----------------------------------------------------

/// A song's ReplayGain values as its tags hold them: gains in dB relative to the ReplayGain reference of
/// −18 LUFS (R128 tags, Q7.8 dB relative to −23 LUFS, are converted: r128 / 256 + 5), peaks linear (1.0 =
/// full scale). REPLAYGAIN_* tags win over R128_* ones when a file has both.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct GainTags {
    /// ReplayGain track gain (dB)
    pub track_gain: Option<f64>,
    /// ReplayGain track peak (linear)
    pub track_peak: Option<f64>,
    /// ReplayGain album gain (dB)
    pub album_gain: Option<f64>,
    /// ReplayGain album peak (linear)
    pub album_peak: Option<f64>,
}

/// What the scan keeps beside a file's tags.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct AudioFacts {
    /// the ReplayGain tags
    pub gain: GainTags,
    /// Hz (2822400 for DSD64)
    pub sample_rate: Option<u32>,
    /// bits: 16, 24 …, 1 for DSD; None for lossy files (an .m4a with a bit depth is ALAC)
    pub bit_depth: Option<u8>,
}

/// [`AudioFacts`] of one file (the "Measure loudness" task reads them again for songs scanned before 0.31).
pub fn read_facts(path: &str) -> AudioFacts {
    facts_of(lofty::read_from_path(path).ok().as_ref(), path)
}

/// Formats without a bit depth: a number lofty might give for them is not one.
const LOSSY: &[&str] = &["mp3", "mp2", "aac", "ogg", "oga", "opus", "wma", "mpc", "webm"];

/// Format facts of a file (rate, depth, gain tags), from its tags and properties.
fn facts_of(tagged: Option<&TaggedFile>, path: &str) -> AudioFacts {
    let ext = Path::new(path).extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let mut f = AudioFacts::default();
    if let Some(t) = tagged {
        // the primary tag first, then the others (mp3gain writes an APE tag beside the ID3v2 one)
        let mut tags: Vec<&Tag> = t.primary_tag().into_iter().collect();
        tags.extend(t.tags().iter().filter(|x| Some(x.tag_type()) != t.primary_tag().map(|p| p.tag_type())));
        f.gain = gain_of(&tags);
        let p = t.properties();
        f.sample_rate = p.sample_rate().filter(|r| *r > 0);
        f.bit_depth = p.bit_depth().filter(|b| *b > 0 && !LOSSY.contains(&ext.as_str()));
    }
    if f.sample_rate.is_none() && (ext == "dsf" || ext == "dff") {
        if let Some(rate) = dsd_rate(Path::new(path), &ext) {
            f.sample_rate = Some(rate);
            f.bit_depth = Some(1);
        }
    }
    f
}

/// The loudness tags of a file (its tags in the order given; the first one that has a value wins).
pub fn gain_of(tags: &[&Tag]) -> GainTags {
    let get = |k: ItemKey| tags.iter().find_map(|t| t.get_string(k).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string));
    let mut g = GainTags {
        track_gain: get(ItemKey::ReplayGainTrackGain).and_then(|s| parse_gain(&s)),
        track_peak: get(ItemKey::ReplayGainTrackPeak).and_then(|s| parse_peak(&s)),
        album_gain: get(ItemKey::ReplayGainAlbumGain).and_then(|s| parse_gain(&s)),
        album_peak: get(ItemKey::ReplayGainAlbumPeak).and_then(|s| parse_peak(&s)),
    };
    if g.track_gain.is_none() {
        g.track_gain = get(ItemKey::R128TrackGain).and_then(|s| parse_r128(&s));
    }
    if g.album_gain.is_none() {
        g.album_gain = get(ItemKey::R128AlbumGain).and_then(|s| parse_r128(&s));
    }
    // a peak without its gain says nothing the apps use
    if g.track_gain.is_none() {
        g.track_peak = None;
    }
    if g.album_gain.is_none() {
        g.album_peak = None;
    }
    g
}

/// A ReplayGain gain tag: "-7.21 dB", "+3.10 dB", "-7,21", "−7.21 dB" (a typographic minus) → dB. Anything
/// else, or a value beyond ±60 dB, is None.
pub fn parse_gain(s: &str) -> Option<f64> {
    let t = s.trim().replace('\u{2212}', "-").replace(',', ".");
    let t = t.trim_end_matches(|c: char| c.is_ascii_alphabetic() || c.is_whitespace());
    t.trim().parse::<f64>().ok().filter(|v| v.is_finite() && v.abs() <= 60.0)
}

/// A ReplayGain peak tag: "0.988123" → linear (1.0 = full scale). Zero, negative or absurd values are None.
pub fn parse_peak(s: &str) -> Option<f64> {
    s.trim().replace(',', ".").parse::<f64>().ok().filter(|v| v.is_finite() && *v > 0.0 && *v <= 100.0)
}

/// An R128_TRACK_GAIN / R128_ALBUM_GAIN tag: a whole number in Q7.8 dB relative to −23 LUFS ("-2304" is
/// −9 dB), returned as a ReplayGain gain relative to −18 LUFS: r128 / 256 + 5.
pub fn parse_r128(s: &str) -> Option<f64> {
    let q: i32 = s.trim().parse().ok()?;
    (-32768..=32767).contains(&q).then(|| q as f64 / 256.0 + 5.0)
}

/// The sample rate of a DSD file, read from its header (lofty doesn't read DSF or DSDIFF): DSF keeps it
/// in its "fmt " chunk (little-endian), DSDIFF in the "FS  " chunk of its PROP chunk (big-endian).
fn dsd_rate(path: &Path, ext: &str) -> Option<u32> {
    use std::io::Read;
    let mut head = vec![0u8; 4096];
    let n = std::fs::File::open(path).ok()?.read(&mut head).ok()?;
    head.truncate(n);
    let rate = if ext == "dsf" {
        // "DSD " chunk (28 bytes), then "fmt ": id, size (8), version, format id, channel type, channels, rate
        if head.get(0..4)? != b"DSD " || head.get(28..32)? != b"fmt " {
            return None;
        }
        u32::from_le_bytes(head.get(56..60)?.try_into().ok()?)
    } else {
        if head.get(0..4)? != b"FRM8" {
            return None;
        }
        let at = head.windows(4).position(|w| w == b"FS  ")?;
        u32::from_be_bytes(head.get(at + 12..at + 16)?.try_into().ok()?)
    };
    (1_000_000..=50_000_000).contains(&rate).then_some(rate)
}

/// The first picture embedded in the file (the album art), as stored.
pub fn embedded_art(path: &str) -> Option<Vec<u8>> {
    let tagged = lofty::read_from_path(path).ok()?;
    tagged.tags().iter().find_map(|t| t.pictures().first().map(|p| p.data().to_vec()))
}

// ---- writing ------------------------------------------------------------------------------------

/// The tag item the editor's field `k` writes.
fn key(k: &str) -> Option<ItemKey> {
    Some(match k {
        "title" => ItemKey::TrackTitle,
        "artist" => ItemKey::TrackArtist,
        "album" => ItemKey::AlbumTitle,
        "albumartist" => ItemKey::AlbumArtist,
        "artists" => ItemKey::TrackArtists,
        "albumartists" => ItemKey::AlbumArtists,
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
        // Picard's words: "album; live", and 1 on the songs of a various-artists album (other servers read them too)
        "releasetype" => ItemKey::MusicBrainzReleaseType,
        "compilation" => ItemKey::FlagCompilation,
        _ => return None,
    })
}

/// Write tags (and optionally the front cover) into a file.
pub fn write_tags(path: &Path, vals: &Row, cover: Option<&[u8]>) -> anyhow::Result<()> {
    let mut f = lofty::read_from_path(path)?;
    set_tags(&mut f, vals, cover);
    f.save_to_path(path, WriteOptions::default())?;
    Ok(())
}

/// [`write_tags`] through a file that is open for reading and writing (its format is told from its bytes):
/// for callers that opened the file themselves without following a link, so it can't be swapped.
pub fn write_tags_to(file: &mut std::fs::File, vals: &Row, cover: Option<&[u8]>) -> anyhow::Result<()> {
    let mut f = lofty::read_from(file)?;
    set_tags(&mut f, vals, cover);
    f.save_to(file, WriteOptions::default())?;
    Ok(())
}

/// Does a tag item describe the audio of the file it sits in (its loudness, its encoder) rather than the song?
/// Those are never carried from one file to another: the new audio has its own.
/// (Items lofty has no name for – "ITUNNORM" and the like – never reach a generic tag, so they can't be carried.)
fn about_the_audio(key: ItemKey) -> bool {
    matches!(key, ItemKey::ReplayGainTrackGain | ItemKey::ReplayGainTrackPeak | ItemKey::ReplayGainAlbumGain | ItemKey::ReplayGainAlbumPeak
        | ItemKey::R128TrackGain | ItemKey::R128AlbumGain | ItemKey::EncoderSoftware | ItemKey::EncoderSettings | ItemKey::EncodedBy)
}

/// Tags a file that takes another file's place in the library ("Replace with my own file"): whatever the new
/// file said about itself is dropped and it gets the song's tags instead. In order: every item and picture of
/// the `old` file's tag (when it is still readable) except the ones about its audio ([`about_the_audio`]: its
/// loudness and encoder), then the library's own values `vals` (as [`write_tags`] takes them) on top. The new
/// file keeps only its own ReplayGain / R128 tags, which do describe its audio. Tags of other kinds the new
/// file carried (an ID3v1 beside the ID3v2, an APE tag) are removed, so no reader finds its old words.
pub fn retag_like(old: Option<&Path>, new: &Path, vals: &Row) -> anyhow::Result<()> {
    let mut f = lofty::read_from_path(new)?;
    let kind = f.primary_tag_type();
    let mut fresh = Tag::new(kind);
    // the new file's own loudness tags are true of its audio: they stay
    for t in f.tags() {
        for item in t.items().filter(|i| about_the_audio(i.key()) && !matches!(i.key(), ItemKey::EncoderSoftware | ItemKey::EncoderSettings | ItemKey::EncodedBy)) {
            fresh.push(item.clone());
        }
    }
    // the old file's tag: the song's words, ids, lyrics and pictures
    if let Some(old_file) = old.and_then(|o| lofty::read_from_path(o).ok()) {
        if let Some(src) = old_file.primary_tag().or_else(|| old_file.first_tag()) {
            for item in src.items().filter(|i| !about_the_audio(i.key())) {
                // an item this kind of tag has no name for is left out (push checks the mapping)
                fresh.push(item.clone());
            }
            for pic in src.pictures() {
                fresh.push_picture(pic.clone());
            }
        }
    }
    // every other tag the new file had goes, so its own words can't be read from one of them
    let others: Vec<lofty::tag::TagType> = f.tags().iter().map(|t| t.tag_type()).filter(|t| *t != kind).collect();
    f.clear();
    f.insert_tag(fresh);
    set_tags(&mut f, vals, None);
    f.save_to_path(new, WriteOptions::default())?;
    for t in others {
        // a kind this format can't hold anyway errors here: nothing to remove then
        let _ = t.remove_from_path(new, WriteOptions::default());
    }
    Ok(())
}

/// Write the edited fields `vals` (and `cover`) into the file's primary tag, creating one when it has none.
fn set_tags(f: &mut lofty::file::TaggedFile, vals: &Row, cover: Option<&[u8]>) {
    if f.primary_tag().is_none() {
        let t = f.primary_tag_type();
        f.insert_tag(Tag::new(t));
    }
    let tag = f.primary_tag_mut().unwrap();
    for (k, v) in vals {
        // a list (ARTISTS, the artists' MusicBrainz ids) is one value each
        if let (Some(list), Some(ik)) = (v.as_array(), key(k)) {
            let items: Vec<String> = list.iter().map(encore_core::json::text).filter(|s| !s.is_empty()).collect();
            if !items.is_empty() {
                tag.remove_key(ik);
                for s in items {
                    tag.push(lofty::tag::TagItem::new(ik, lofty::tag::ItemValue::Text(s)));
                }
            }
            continue;
        }
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use lofty::tag::TagType;
    use std::path::PathBuf;

    #[test]
    fn gain_peak_and_r128_values() {
        assert_eq!(parse_gain("-7.21 dB"), Some(-7.21));
        assert_eq!(parse_gain("+3.10 dB"), Some(3.1));
        assert_eq!(parse_gain(" -7,21 db "), Some(-7.21));
        assert_eq!(parse_gain("\u{2212}6.5 dB"), Some(-6.5));
        assert_eq!(parse_gain("-4"), Some(-4.0));
        assert_eq!(parse_gain("loud"), None);
        assert_eq!(parse_gain("-99 dB"), None);
        assert_eq!(parse_gain(""), None);
        assert_eq!(parse_peak("0.988123"), Some(0.988123));
        assert_eq!(parse_peak("1,05"), Some(1.05));
        assert_eq!(parse_peak("0"), None);
        assert_eq!(parse_peak("-0.5"), None);
        // Q7.8 dB relative to -23 LUFS, given relative to -18 LUFS
        assert_eq!(parse_r128("-2304"), Some(-4.0));
        assert_eq!(parse_r128("0"), Some(5.0));
        assert_eq!(parse_r128("512"), Some(7.0));
        assert_eq!(parse_r128("-7.5"), None);
        assert_eq!(parse_r128("99999"), None);
    }

    /// `seconds` of a sine made by ffmpeg in `name` (its extension picks the format), with `args` before the
    /// output; None when ffmpeg isn't installed.
    fn audio(dir: &Path, name: &str, args: &[&str]) -> Option<PathBuf> {
        let out = dir.join(name);
        let ok = std::process::Command::new("ffmpeg").args(["-v", "error", "-nostdin", "-y", "-f", "lavfi", "-i", "sine=f=440:d=1:sample_rate=48000"])
            .args(args).arg(&out).status().is_ok_and(|s| s.success());
        if !ok {
            eprintln!("ffmpeg is not installed: the tag fixtures are skipped");
        }
        ok.then_some(out)
    }

    /// Writes `items` into a tag of type `kind` of the file (made when missing), with lofty.
    fn tag_file(path: &Path, kind: TagType, items: &[(ItemKey, &str)]) {
        let mut f = lofty::read_from_path(path).unwrap();
        if f.tag(kind).is_none() {
            f.insert_tag(Tag::new(kind));
        }
        let t = f.tag_mut(kind).unwrap();
        for (k, v) in items {
            assert!(t.insert_text(*k, v.to_string()), "{kind:?} takes {k:?}");
        }
        f.save_to_path(path, WriteOptions::default()).unwrap();
    }

    /// ReplayGain and R128 tags as taggers write them, in the formats people have (written with lofty into
    /// files ffmpeg made), come back as ReplayGain values relative to -18 LUFS with the audio properties.
    #[test]
    fn artist_credits_are_written_and_read_back() {
        use encore_db::RowExt;
        let d = encore_core::tmp::dir("credit-tags");
        let (a, b) = ("6f1a58bf-9b1b-49cf-a44a-6cefad7ae04f", "0b30341b-b59d-4979-8130-b66c0e475321");
        let mut vals = Row::new();
        vals.insert("artist".into(), json!("Kanye West & Ty Dolla $ign"));
        vals.insert("artists".into(), json!(["Kanye West", "Ty Dolla $ign"]));
        vals.insert("albumartist".into(), json!("Kanye West & Ty Dolla $ign"));
        vals.insert("albumartists".into(), json!(["Kanye West", "Ty Dolla $ign"]));
        vals.insert("musicbrainz_artistid".into(), json!([a, b]));
        vals.insert("musicbrainz_albumartistid".into(), json!([a, b]));
        let want = json!([{ "name": "Kanye West", "mbid": a }, { "name": "Ty Dolla $ign", "mbid": b }]).to_string();
        for (name, args) in [("c.flac", &["-c:a", "flac"][..]), ("c.mp3", &["-c:a", "libmp3lame"][..]), ("c.m4a", &["-c:a", "aac"][..])] {
            let Some(f) = audio(&d, name, args) else { return };
            write_tags(&f, &vals, None).unwrap();
            let t = read_tags(&f.to_string_lossy());
            assert_eq!(t.s("artist"), "Kanye West & Ty Dolla $ign", "{name}");
            assert_eq!(t.s("artist_credit"), want, "{name}");
            assert_eq!(t.s("albumartist_credit"), want, "{name}");
            assert_eq!(t.s("mb_artistid"), a, "{name}");
        }
        // a file without the tags: read, and empty
        let Some(plain) = audio(&d, "p.flac", &["-c:a", "flac"]) else { return };
        tag_file(&plain, TagType::VorbisComments, &[(ItemKey::TrackArtist, "Simon & Garfunkel")]);
        assert_eq!(read_tags(&plain.to_string_lossy()).s("artist_credit"), "[]");
        // ids joined in one value ("a/b", ID3v2.3 style) are split; names and ids that don't pair keep only the names
        tag_file(&plain, TagType::VorbisComments, &[(ItemKey::TrackArtists, "Simon & Garfunkel"), (ItemKey::MusicBrainzArtistId, &format!("{a}/{b}"))]);
        let t = read_tags(&plain.to_string_lossy());
        assert_eq!(t.s("mb_artistid"), a);
        assert_eq!(t.s("artist_credit"), json!([{ "name": "Simon & Garfunkel" }]).to_string());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn loudness_tags_and_audio_properties_are_read() {
        let d = encore_core::tmp::dir("gain-tags");
        let rg = [(ItemKey::ReplayGainTrackGain, "-7.21 dB"), (ItemKey::ReplayGainTrackPeak, "0.988123"),
                  (ItemKey::ReplayGainAlbumGain, "-6.84 dB"), (ItemKey::ReplayGainAlbumPeak, "1.000000")];
        let want = GainTags { track_gain: Some(-7.21), track_peak: Some(0.988123), album_gain: Some(-6.84), album_peak: Some(1.0) };

        // FLAC 24/96 with REPLAYGAIN_* in its Vorbis comments
        let Some(flac) = audio(&d, "hi.flac", &["-ar", "96000", "-sample_fmt", "s32", "-c:a", "flac"]) else { return };
        tag_file(&flac, TagType::VorbisComments, &[(ItemKey::TrackTitle, "Hi")]);
        assert_eq!(read_facts(&flac.to_string_lossy()), AudioFacts { gain: GainTags::default(), sample_rate: Some(96000), bit_depth: Some(24) });
        tag_file(&flac, TagType::VorbisComments, &rg);
        let (tags, facts) = read_tags_and_facts(&flac.to_string_lossy());
        assert_eq!(tags.get("title").and_then(|v| v.as_str()), Some("Hi"));
        assert_eq!(facts, AudioFacts { gain: want, sample_rate: Some(96000), bit_depth: Some(24) });
        // the tags the library keeps are the same as before
        assert_eq!(read_tags(&flac.to_string_lossy()), tags);

        // FLAC with R128 tags only (Q7.8 relative to -23 LUFS): converted
        let Some(r128) = audio(&d, "r128.flac", &["-c:a", "flac"]) else { return };
        tag_file(&r128, TagType::VorbisComments, &[(ItemKey::R128TrackGain, "-2304"), (ItemKey::R128AlbumGain, "-1792")]);
        let f = read_facts(&r128.to_string_lossy());
        assert_eq!(f.gain, GainTags { track_gain: Some(-4.0), track_peak: None, album_gain: Some(-2.0), album_peak: None });
        assert_eq!((f.sample_rate, f.bit_depth), (Some(48000), Some(16)));
        // both kinds: REPLAYGAIN_* wins
        tag_file(&r128, TagType::VorbisComments, &[(ItemKey::ReplayGainTrackGain, "-6.00 dB")]);
        let f = read_facts(&r128.to_string_lossy());
        assert_eq!((f.gain.track_gain, f.gain.album_gain), (Some(-6.0), Some(-2.0)));

        // Opus: R128_TRACK_GAIN in its OpusTags; no bit depth (lossy)
        let Some(opus) = audio(&d, "a.opus", &["-c:a", "libopus", "-b:a", "64k"]) else { return };
        tag_file(&opus, TagType::VorbisComments, &[(ItemKey::R128TrackGain, "-1280")]);
        let f = read_facts(&opus.to_string_lossy());
        assert_eq!(f.gain.track_gain, Some(0.0));
        assert_eq!((f.sample_rate, f.bit_depth), (Some(48000), None));

        // MP3: TXXX:REPLAYGAIN_* frames in ID3v2 (foobar2000 writes the names in lower case: the same)
        let Some(mp3) = audio(&d, "a.mp3", &["-c:a", "libmp3lame", "-b:a", "128k"]) else { return };
        tag_file(&mp3, TagType::Id3v2, &rg);
        let f = read_facts(&mp3.to_string_lossy());
        assert_eq!(f.gain, want);
        assert_eq!((f.sample_rate, f.bit_depth), (Some(48000), None));
        // TXXX frames as ffmpeg writes them, one in lower case (foobar2000's spelling)
        let Some(lower) = audio(&d, "lower.mp3", &["-c:a", "libmp3lame", "-metadata", "replaygain_track_gain=-5.55 dB", "-metadata", "REPLAYGAIN_ALBUM_GAIN=-4.44 dB"]) else { return };
        let f = read_facts(&lower.to_string_lossy());
        assert_eq!((f.gain.track_gain, f.gain.album_gain), (Some(-5.55), Some(-4.44)));
        // MP3 with the values in an APE tag beside an ID3v2 tag that has none (mp3gain)
        let Some(mp3g) = audio(&d, "gain.mp3", &["-c:a", "libmp3lame", "-b:a", "128k"]) else { return };
        tag_file(&mp3g, TagType::Id3v2, &[(ItemKey::TrackTitle, "Gained")]);
        tag_file(&mp3g, TagType::Ape, &[(ItemKey::ReplayGainTrackGain, "-3.50 dB"), (ItemKey::ReplayGainTrackPeak, "0.5")]);
        let (tags, f) = read_tags_and_facts(&mp3g.to_string_lossy());
        assert_eq!(tags.get("title").and_then(|v| v.as_str()), Some("Gained"));
        assert_eq!((f.gain.track_gain, f.gain.track_peak, f.gain.album_gain), (Some(-3.5), Some(0.5), None));

        // AAC in .m4a: iTunes freeform atoms; no bit depth. ALAC in .m4a: a bit depth.
        let Some(m4a) = audio(&d, "a.m4a", &["-c:a", "aac", "-b:a", "128k"]) else { return };
        tag_file(&m4a, TagType::Mp4Ilst, &rg);
        let f = read_facts(&m4a.to_string_lossy());
        assert_eq!(f.gain, want);
        assert_eq!((f.sample_rate, f.bit_depth), (Some(48000), None));
        let Some(alac) = audio(&d, "b.m4a", &["-c:a", "alac", "-sample_fmt", "s32p"]) else { return };
        let f = read_facts(&alac.to_string_lossy());
        assert_eq!(f.gain, GainTags::default());
        assert_eq!(f.sample_rate, Some(48000));
        assert!(f.bit_depth.is_some_and(|b| b >= 16), "{f:?}");

        // a peak without its gain, and nonsense, are left out
        let Some(odd) = audio(&d, "odd.flac", &["-c:a", "flac"]) else { return };
        tag_file(&odd, TagType::VorbisComments, &[(ItemKey::ReplayGainTrackPeak, "0.9"), (ItemKey::ReplayGainAlbumGain, "loud")]);
        assert_eq!(read_facts(&odd.to_string_lossy()).gain, GainTags::default());

        // a file lofty can't read: nothing
        std::fs::write(d.join("broken.flac"), b"not audio").unwrap();
        assert_eq!(read_facts(&d.join("broken.flac").to_string_lossy()), AudioFacts::default());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// The release type tags as Picard and the stores write them – RELEASETYPE (FLAC), TXXX "MusicBrainz Album Type"
    /// (MP3), the iTunes freeform atom (M4A), TCMP / cpil / COMPILATION, TRACKTOTAL and the "/n" of the track
    /// number – are read; writing `releasetype` / `compilation` comes back the same; a file without them reads
    /// '' / 0 / 0.
    #[test]
    fn release_type_tags_are_read_and_written() {
        let d = encore_core::tmp::dir("type-tags");
        let get = |p: &Path| {
            let t = read_tags(&p.to_string_lossy());
            (t.get("releasetype").and_then(|v| v.as_str()).unwrap_or("?").to_string(), t.get("tracktotal").and_then(|v| v.as_i64()).unwrap_or(-1),
             t.get("compilation").and_then(|v| v.as_i64()).unwrap_or(-1))
        };
        let Some(flac) = audio(&d, "a.flac", &["-c:a", "flac"]) else { return };
        assert_eq!(get(&flac), (String::new(), 0, 0), "no such tags: '' / 0 / 0");
        tag_file(&flac, TagType::VorbisComments, &[(ItemKey::MusicBrainzReleaseType, "album; live"), (ItemKey::TrackTotal, "12"), (ItemKey::FlagCompilation, "1")]);
        assert_eq!(get(&flac), ("album; live".to_string(), 12, 1));
        let Some(mp3) = audio(&d, "a.mp3", &["-c:a", "libmp3lame", "-b:a", "128k"]) else { return };
        tag_file(&mp3, TagType::Id3v2, &[(ItemKey::MusicBrainzReleaseType, "Album/Live"), (ItemKey::TrackNumber, "3"), (ItemKey::TrackTotal, "14"), (ItemKey::FlagCompilation, "1")]);
        assert_eq!(get(&mp3), ("Album/Live".to_string(), 14, 1));
        let Some(m4a) = audio(&d, "a.m4a", &["-c:a", "aac", "-b:a", "128k"]) else { return };
        tag_file(&m4a, TagType::Mp4Ilst, &[(ItemKey::MusicBrainzReleaseType, "single"), (ItemKey::TrackNumber, "1"), (ItemKey::TrackTotal, "2"), (ItemKey::FlagCompilation, "1")]);
        assert_eq!(get(&m4a), ("single".to_string(), 2, 1));
        // written through write_tags (the import's keys), read back the same
        let Some(w) = audio(&d, "w.flac", &["-c:a", "flac"]) else { return };
        let mut vals = Row::new();
        vals.insert("releasetype".into(), json!("album; compilation"));
        vals.insert("compilation".into(), json!("1"));
        write_tags(&w, &vals, None).unwrap();
        assert_eq!(get(&w), ("album; compilation".to_string(), 0, 1));
        let Some(wm) = audio(&d, "w.mp3", &["-c:a", "libmp3lame", "-b:a", "128k"]) else { return };
        write_tags(&wm, &vals, None).unwrap();
        assert_eq!(get(&wm).0, "album; compilation");
        assert_eq!(get(&wm).2, 1);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// DSD files (which lofty doesn't read): the sample rate from the DSF or DSDIFF header, bit depth 1.
    #[test]
    fn dsd_rate_from_the_header() {
        let d = encore_core::tmp::dir("dsd-header");
        let mut dsf = vec![];
        dsf.extend_from_slice(b"DSD ");
        dsf.extend_from_slice(&28u64.to_le_bytes());
        dsf.extend_from_slice(&[0u8; 16]);
        dsf.extend_from_slice(b"fmt ");
        dsf.extend_from_slice(&52u64.to_le_bytes());
        for v in [1u32, 0, 2, 2] {
            dsf.extend_from_slice(&v.to_le_bytes());
        }
        dsf.extend_from_slice(&2_822_400u32.to_le_bytes());
        dsf.extend_from_slice(&1u32.to_le_bytes());
        dsf.extend_from_slice(&[0u8; 64]);
        std::fs::write(d.join("a.dsf"), &dsf).unwrap();
        let f = read_facts(&d.join("a.dsf").to_string_lossy());
        assert_eq!((f.sample_rate, f.bit_depth), (Some(2_822_400), Some(1)));
        let mut dff = vec![];
        dff.extend_from_slice(b"FRM8");
        dff.extend_from_slice(&100u64.to_be_bytes());
        dff.extend_from_slice(b"DSD PROP");
        dff.extend_from_slice(&50u64.to_be_bytes());
        dff.extend_from_slice(b"SND FS  ");
        dff.extend_from_slice(&4u64.to_be_bytes());
        dff.extend_from_slice(&5_644_800u32.to_be_bytes());
        dff.extend_from_slice(&[0u8; 32]);
        std::fs::write(d.join("b.dff"), &dff).unwrap();
        let f = read_facts(&d.join("b.dff").to_string_lossy());
        assert_eq!((f.sample_rate, f.bit_depth), (Some(5_644_800), Some(1)));
        // something else under the name: nothing
        std::fs::write(d.join("c.dsf"), b"DSD but not really").unwrap();
        assert_eq!(read_facts(&d.join("c.dsf").to_string_lossy()), AudioFacts::default());
        let _ = std::fs::remove_dir_all(&d);
    }
}
