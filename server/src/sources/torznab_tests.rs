//! Single songs from torrent and Usenet releases: ranking a song's releases and finding its file.
use super::*;

fn names(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[test]
fn song_releases_rank_singles_and_format() {
    let s = |t: &str, mb: f64, seeds: i64| score_song(t, mb * 1e6, Some(seeds), "torrent", "Travis Scott", "90210");
    let (flac, single) = s("Travis Scott - 90210 (2015) [FLAC] [WEB]", 60.0, 10).unwrap();
    assert!(single);
    let (mp3, _) = s("Travis Scott - 90210 (2015) [MP3 320]", 12.0, 10).unwrap();
    let (album, big_single) = s("Travis Scott - Rodeo (Deluxe) incl 90210 [FLAC]", 900.0, 10).unwrap();
    assert!(!big_single);
    assert!(flac > mp3 && mp3 > album - 30.0, "{flac} {mp3} {album}");
    // another version is never picked
    assert!(s("Travis Scott - 90210 (Karaoke Version) [FLAC]", 50.0, 50).is_none());
    assert!(s("Travis Scott - 90210 (Live) [FLAC]", 50.0, 50).is_none());
    assert!(s("Travis Scott - 90210 (Sped Up) [MP3 320]", 50.0, 50).is_none());
    assert!(score_song("Travis Scott - 90210 (Live) [FLAC]", 50e6, Some(5), "torrent", "Travis Scott", "90210 (Live)").is_some());
    // not the song, or another artist
    assert!(s("Travis Scott - Antidote [FLAC]", 50.0, 10).is_none());
    assert!(s("Someone Else - 90210 [FLAC]", 50.0, 10).is_none());
    // a discography is never a single
    let (disco, single) = s("Travis Scott - Discography (2013-2023) 90210 [FLAC]", 200.0, 99).unwrap();
    assert!(!single && disco < flac);
}

#[test]
fn picks_the_songs_file() {
    let files = names(&["Rodeo/01 - Pornography.flac", "Rodeo/03 - 3500.flac", "Rodeo/04 - 90210.flac", "Rodeo/cover.jpg", "Rodeo/90210.cue"]);
    assert_eq!(pick_file(&files, "Travis Scott", "90210", None), Some(2));
    assert_eq!(pick_file(&files, "Travis Scott", "Antidote", None), None);
    // the track number helps when the file name shortens the title
    let files = names(&["CD1/01. Oh My Dis Side.flac", "CD1/02. Nightcrawler (feat. Swae Lee).flac", "CD1/03. Piss on Your Grave.flac"]);
    assert_eq!(pick_file(&files, "Travis Scott", "Oh My Dis Side (feat. Quavo)", Some(1)), Some(0));
    assert_eq!(pick_file(&files, "Travis Scott", "Nightcrawler", None), Some(1));
    // the plain version over a remix or instrumental of the same song
    let files = names(&["01 Antidote (Remix).mp3", "02 Antidote.mp3", "03 Antidote (Instrumental).mp3"]);
    assert_eq!(pick_file(&files, "Travis Scott", "Antidote", None), Some(1));
    // artist in the name, Windows separators, accents and punctuation
    let files = names(&["Beyoncé - Lemonade\\\\06 - Beyoncé - Daddy Lessons.flac", "Beyoncé - Lemonade\\\\05 - Beyoncé - Sorry.flac"]);
    assert_eq!(pick_file(&files, "Beyoncé", "Sorry", None), Some(1));
    // a title that is all digits isn't taken for a track number
    let files = names(&["01 - Pornography.flac", "04 - 90210.flac"]);
    assert_eq!(pick_file(&files, "Travis Scott", "90210", Some(1)), Some(1));
}

#[test]
fn catalogue_extras_dont_hide_the_file() {
    let files = names(&["Rodeo/01 - Pornography.flac", "Rodeo/03 - 3500.flac", "Rodeo/04 - 90210.flac", "Rodeo/05 - 90210 (Live).flac"]);
    // Deezer / Apple titles carry feat. and remaster notes the files don't
    assert_eq!(pick_file(&files, "Travis Scott", "3500 (feat. Future & 2 Chainz)", None), Some(1));
    assert_eq!(pick_file(&files, "Travis Scott", "90210 - Remastered 2015", None), Some(2));
    assert_eq!(pick_file(&files, "Travis Scott", "90210 (feat. Kacy Hill)", None), Some(2));
    // a live version asked for is still the live file
    assert_eq!(pick_file(&files, "Travis Scott", "90210 (Live)", None), Some(3));
    assert!(score_song("Travis Scott - 3500 [FLAC]", 40e6, Some(5), "torrent", "Travis Scott", "3500 (feat. Future & 2 Chainz)").is_some());
    assert_eq!(core_title("Song (Live Version)"), "Song (Live Version)");
    assert_eq!(core_title("Song feat. X"), "Song");
}
