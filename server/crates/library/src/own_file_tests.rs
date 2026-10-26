//! The parts of bring-your-own-music that need no server: which names are audio, staging tokens, the new
//! file's place, and telling audio from a file that only has an audio name.
use super::*;

#[test]
fn audio_names_and_tokens() {
    assert_eq!(audio_ext("My Rip.FLAC").as_deref(), Some("flac"));
    assert_eq!(audio_ext("song.m4a").as_deref(), Some("m4a"));
    assert_eq!(audio_ext("cover.jpg"), None);
    assert_eq!(audio_ext("no extension"), None);
    assert!(valid_token(&"a1".repeat(16)));
    assert!(!valid_token("../../etc/passwd"));
    assert!(!valid_token(&"g".repeat(32)));
    assert!(!valid_token("abc"));
}

#[test]
fn the_new_file_takes_the_old_name_with_its_own_extension() {
    assert_eq!(new_path(Path::new("/m/A/B/02 Crazy.mp3"), "flac"), PathBuf::from("/m/A/B/02 Crazy.flac"));
    // dots in the name stay
    assert_eq!(new_path(Path::new("/m/A/B/02 Mr. Brightside.mp3"), "flac"), PathBuf::from("/m/A/B/02 Mr. Brightside.flac"));
}

#[test]
fn only_real_audio_passes() {
    let d = encore_core::tmp::dir("own-file-audio");
    // a WAV: 16-bit stereo, a tenth of a second at 44.1 kHz
    let data = vec![0u8; 4410 * 4];
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&44100u32.to_le_bytes());
    wav.extend_from_slice(&(44100u32 * 4).to_le_bytes());
    wav.extend_from_slice(&4u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(data.len() as u32).to_le_bytes());
    wav.extend_from_slice(&data);
    let ok = d.join("a.wav");
    std::fs::write(&ok, &wav).unwrap();
    let a = check_audio(&ok).unwrap();
    assert_eq!((a.sample_rate, a.channels), (44100, 2));
    assert!(a.duration > 0.09);
    // text with an audio name, audio with a name that isn't
    let fake = d.join("b.flac");
    std::fs::write(&fake, "not a song").unwrap();
    assert!(check_audio(&fake).is_err());
    let named = d.join("c.txt");
    std::fs::write(&named, &wav).unwrap();
    assert!(check_audio(&named).is_err());
    let _ = std::fs::remove_dir_all(&d);
}
