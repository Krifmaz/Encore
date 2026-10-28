//! Fragmented MP4 audio (.m4a / .mp4 written as a run of `moof` + `mdat` fragments, as streaming rips and some
//! Apple Music and YouTube downloaders leave them) and turning it into a plain MP4.
//!
//! A fragmented file keeps no sample table and no length in its `moov`: lofty reads its duration as 0, so the
//! library had the song at 0:00, and a player that streams it over HTTP (ExoPlayer on Android, the browsers)
//! finds no index to seek in, so the seek bar stays gray and the whole file loads before it plays. The scan
//! rewrites such a file once into a plain MP4 with its index up front (`-c copy -movflags +faststart`: the
//! audio is copied, never re-encoded) and puts the original tags back with lofty ([`defragment`]).
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// The file extensions whose files can be MP4 audio.
pub const MP4_EXTS: &[&str] = &["m4a", "mp4", "m4b", "aac"];

/// Top-level boxes read at most while looking for a fragment (a fragmented song has one `moof` per second or
/// so; the first comes right after the `moov`, so a plain file is told apart within a handful of boxes).
const MAX_BOXES: usize = 64;

/// Whether `path` has an MP4 extension ([`MP4_EXTS`]), ignoring case.
pub fn has_mp4_ext(path: &str) -> bool {
    Path::new(path).extension().is_some_and(|e| MP4_EXTS.contains(&e.to_string_lossy().to_lowercase().as_str()))
}

/// Whether the MP4 file at `path` is fragmented: a top-level `moof` box follows its `moov`. False for a plain
/// MP4, for anything that isn't an MP4 and for a file that can't be read.
pub fn is_fragmented(path: &str) -> bool {
    File::open(path).ok().is_some_and(|mut f| has_moof(&mut f).unwrap_or(false))
}

/// Walks the top-level boxes of `r` (size + four-letter type each) and says whether one is a `moof`. Stops at
/// the end, at a box that can't be right (smaller than its header) or after [`MAX_BOXES`] boxes.
fn has_moof<R: Read + Seek>(r: &mut R) -> std::io::Result<bool> {
    let end = r.seek(SeekFrom::End(0))?;
    let mut at = 0u64;
    for i in 0..MAX_BOXES {
        if at + 8 > end {
            return Ok(false);
        }
        r.seek(SeekFrom::Start(at))?;
        let mut head = [0u8; 8];
        r.read_exact(&mut head)?;
        let kind = &head[4..8];
        // an MP4 starts with `ftyp` (older files: straight with `moov` or `mdat`); anything else is not one
        if i == 0 && !matches!(kind, b"ftyp" | b"moov" | b"mdat" | b"free" | b"wide" | b"skip") {
            return Ok(false);
        }
        if kind == b"moof" {
            return Ok(true);
        }
        let size = match u32::from_be_bytes([head[0], head[1], head[2], head[3]]) as u64 {
            // size 1: the real size is the 64-bit number after the type
            1 => {
                let mut big = [0u8; 8];
                r.read_exact(&mut big)?;
                u64::from_be_bytes(big)
            }
            // size 0: the box runs to the end of the file
            0 => return Ok(false),
            n => n,
        };
        if size < 8 {
            return Ok(false);
        }
        at = at.saturating_add(size);
    }
    Ok(false)
}

/// The hidden file next to `path` the plain copy is written to before it takes the original's place. Its name
/// starts with a dot, so a scan running meanwhile never reads it as a song.
fn temp_path(path: &Path) -> PathBuf {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    path.with_file_name(format!(".{name}.encore-remux.m4a"))
}

/// Rewrites the fragmented MP4 at `path` as a plain MP4 with its index up front, in place. The audio is copied
/// as it is (ALAC stays ALAC, AAC stays the same AAC), the tags and pictures are copied from the original with
/// lofty, and the new file only replaces the old one after it reads back as a plain MP4 of the same length as
/// ffprobe measured the original (half a second either way). On any failure the original stays untouched and
/// the reason comes back.
pub fn defragment(path: &str) -> Result<(), String> {
    let src = Path::new(path);
    let tmp = temp_path(src);
    let res = remux_into(src, &tmp);
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    res
}

/// [`defragment`]'s steps, writing `tmp` and renaming it over `src` at the end.
fn remux_into(src: &Path, tmp: &Path) -> Result<(), String> {
    use lofty::config::{ParseOptions, WriteOptions};
    use lofty::file::AudioFile;
    use lofty::tag::TagExt;
    let want = crate::probe::stream_info(&src.to_string_lossy()).map(|s| s.duration).filter(|d| *d > 0.0)
        .ok_or("ffprobe can't read its length")?;
    // the first audio stream only: a cover stored as a video track comes back with the tags (`covr`).
    // `-write_btrt 0`: no `btrt` box in the sample entry, which hides ALAC from lofty (the song would be sent to
    // browsers as AAC); ffmpeg before 6.0 doesn't know the option and never writes the box, so it runs without
    let remux = |no_btrt: bool| {
        let mut cmd = std::process::Command::new("ffmpeg");
        cmd.args(["-v", "error", "-nostdin", "-y", "-i"]).arg(src)
            .args(["-map", "0:a:0", "-c", "copy", "-map_metadata", "-1", "-movflags", "+faststart"]);
        if no_btrt {
            cmd.args(["-write_btrt", "0"]);
        }
        cmd.args(["-f", "mp4"]).arg(tmp).stdin(std::process::Stdio::null()).output().map_err(|e| format!("ffmpeg: {e}"))
    };
    let mut out = remux(true)?;
    if !out.status.success() {
        out = remux(false)?;
    }
    if !out.status.success() {
        return Err(format!("ffmpeg: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    // the original's tags (every atom, the freeform MusicBrainz ones and the cover too) onto the copy
    let mut f = File::open(src).map_err(|e| e.to_string())?;
    let orig = lofty::mp4::Mp4File::read_from(&mut f, ParseOptions::new()).map_err(|e| format!("reading its tags: {e}"))?;
    if let Some(ilst) = orig.ilst() {
        ilst.save_to_path(tmp, WriteOptions::default()).map_err(|e| format!("writing its tags: {e}"))?;
    }
    // the copy must be a plain MP4 lofty measures at the original's length
    let tmp_s = tmp.to_string_lossy();
    if is_fragmented(&tmp_s) {
        return Err("the copy is still fragmented".into());
    }
    let mut g = File::open(tmp).map_err(|e| e.to_string())?;
    let got = lofty::mp4::Mp4File::read_from(&mut g, ParseOptions::new()).map_err(|e| format!("reading the copy: {e}"))?
        .properties().duration().as_secs_f64();
    if (got - want).abs() > 0.5 {
        return Err(format!("the copy is {got:.2} s long, the original {want:.2} s"));
    }
    // the original's permissions, so a library shared with a media server stays readable to it
    if let Ok(meta) = std::fs::metadata(src) {
        let _ = std::fs::set_permissions(tmp, meta.permissions());
    }
    std::fs::rename(tmp, src).map_err(|e| format!("replacing the file: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// One box: 32-bit size, type, `body` zero bytes.
    fn mp4_box(kind: &[u8; 4], body: usize) -> Vec<u8> {
        let mut b = ((8 + body) as u32).to_be_bytes().to_vec();
        b.extend_from_slice(kind);
        b.resize(8 + body, 0);
        b
    }

    #[test]
    fn a_moof_after_the_moov_makes_a_file_fragmented() {
        let plain = [mp4_box(b"ftyp", 16), mp4_box(b"moov", 40), mp4_box(b"mdat", 100)].concat();
        assert!(!has_moof(&mut Cursor::new(plain)).unwrap());
        let frag = [mp4_box(b"ftyp", 16), mp4_box(b"moov", 40), mp4_box(b"moof", 24), mp4_box(b"mdat", 100)].concat();
        assert!(has_moof(&mut Cursor::new(frag)).unwrap());
    }

    #[test]
    fn a_64_bit_box_size_is_followed() {
        // `mdat` with size 1 and its real size (16 header bytes + 4) in the next eight bytes, then the `moof`
        let mut big = 1u32.to_be_bytes().to_vec();
        big.extend_from_slice(b"mdat");
        big.extend_from_slice(&20u64.to_be_bytes());
        big.extend_from_slice(&[0; 4]);
        let file = [mp4_box(b"ftyp", 8), big, mp4_box(b"moof", 8)].concat();
        assert!(has_moof(&mut Cursor::new(file)).unwrap());
    }

    #[test]
    fn other_files_and_broken_boxes_are_not_fragmented() {
        // a FLAC file
        let mut flac = b"fLaC".to_vec();
        flac.resize(64, 0);
        assert!(!has_moof(&mut Cursor::new(flac)).unwrap());
        // a box claiming fewer bytes than its header ends the walk
        let broken = [mp4_box(b"ftyp", 8), 4u32.to_be_bytes().to_vec(), b"moov".to_vec(), mp4_box(b"moof", 8)].concat();
        assert!(!has_moof(&mut Cursor::new(broken)).unwrap());
        assert!(!has_moof(&mut Cursor::new(Vec::new())).unwrap());
    }

    /// A real fragmented ALAC file (made by ffmpeg) becomes a plain MP4 of the same length with its tags; skipped
    /// where ffmpeg is not installed.
    #[test]
    fn a_fragmented_alac_file_is_made_plain_with_its_tags() {
        use lofty::file::{AudioFile, TaggedFileExt};
        use lofty::tag::Accessor;
        if !crate::ffmpeg::available() {
            return;
        }
        // a folder of its own under the system's temp folder, removed at the end
        let dir = std::env::temp_dir().join(format!("encore-mp4-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("going.m4a");
        let ok = std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-nostdin", "-y", "-f", "lavfi", "-i", "sine=f=440:d=12", "-ac", "2", "-c:a", "alac",
                   "-metadata", "title=going…", "-metadata", "artist=DENONBU", "-movflags", "+frag_keyframe+empty_moov+default_base_moof"])
            .arg(&path)
            .status()
            .unwrap()
            .success();
        assert!(ok);
        let p = path.to_string_lossy().into_owned();
        assert!(is_fragmented(&p));
        // lofty finds no length in the fragmented file: the bug this module fixes
        assert_eq!(lofty::read_from_path(&path).unwrap().properties().duration().as_secs(), 0);
        defragment(&p).unwrap();
        assert!(!is_fragmented(&p));
        let t = lofty::read_from_path(&path).unwrap();
        assert!((t.properties().duration().as_secs_f64() - 12.0).abs() < 0.1);
        let tag = t.primary_tag().unwrap();
        assert_eq!(tag.title().as_deref(), Some("going…"));
        assert_eq!(tag.artist().as_deref(), Some("DENONBU"));
        assert!(crate::probe::is_alac(&p), "the audio stays ALAC");
        assert!(!temp_path(&path).exists(), "no copy is left behind");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mp4_extensions() {
        assert!(has_mp4_ext("/m/a/Song.M4A") && has_mp4_ext("x.mp4") && !has_mp4_ext("x.flac") && !has_mp4_ext("m4a"));
    }

    #[test]
    fn the_temp_copy_is_hidden_beside_the_song() {
        assert_eq!(temp_path(Path::new("/m/A/01 Song.m4a")), PathBuf::from("/m/A/.01 Song.m4a.encore-remux.m4a"));
    }
}
