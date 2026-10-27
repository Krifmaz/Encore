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

#[test]
fn symbols_that_stand_for_letters_match_the_spelled_out_names() {
    // the owner's case: Ke$ha – Cannibal, which indexers and scene names spell "Kesha"
    assert_eq!(match_words("Ke$ha"), "kesha");
    assert_eq!(match_words("A$AP Rocky"), "asap rocky");
    assert_eq!(match_words("Joey Bada$$"), "joey badass");
    assert_eq!(match_words("$uicideboy$"), "suicideboys");
    assert_eq!(match_words("Ty Dolla $ign"), "ty dolla sign");
    assert_eq!(match_words("P!nk"), "pink");
    assert_eq!(match_words("Panic! at the Disco"), "panic at the disco");
    assert_eq!(match_words("Wham!"), "wham");
    assert_eq!(match_words("$100 Bill"), "100 bill");
    assert_eq!(match_words("Beyoncé"), "beyonce");
    assert_eq!(match_words("Don't Stop"), "dont stop");
    // the query the worker sends to Prowlarr: letters, not "Ke ha"
    assert_eq!(fold_symbols("Ke$ha Cannibal"), "Kesha Cannibal");
    assert_eq!(fold_symbols("$100 Bill – Wham!"), "$100 Bill – Wham!");
    // compared with and without the symbols, in both directions
    assert_eq!(contains("Kesha - Cannibal (2010) [FLAC]", "Ke$ha"), 1.0);
    assert_eq!(contains("Ke$ha - Cannibal [FLAC]", "Kesha"), 1.0);
    assert_eq!(contains("Ke$ha - Cannibal [FLAC]", "Ke$ha"), 1.0);
    assert_eq!(contains("Ke ha - Cannibal [FLAC]", "Ke$ha"), 1.0, "the plain reading still counts");
    assert_eq!(contains("Pink - Funhouse (2008) [FLAC]", "P!nk"), 1.0);
    assert_eq!(contains("Beyonce - Lemonade [FLAC]", "Beyoncé"), 1.0);
    assert_eq!(contains("Someone Else - Cannibal [FLAC]", "Ke$ha"), 0.0);
    // the album gate and the song gate let Kesha's releases through
    assert!(score_song("Kesha - Blow (2011) [FLAC]", 40e6, Some(9), "torrent", "Ke$ha", "Blow").is_some());
    assert!(score_song("Kesha - Blow (2011) [FLAC]", 40e6, Some(9), "torrent", "Someone", "Blow").is_none());
}

// ---- albums: the gates, the ranking and Prowlarr -----------------------------------------------------

/// A Prowlarr release as its search lists it.
fn rel(title: &str, mb: f64, seeds: i64) -> Value {
    json!({ "guid": title, "title": title, "size": mb * 1e6, "seeders": seeds, "indexer": "Tracker", "protocol": "torrent", "magnetUrl": format!("magnet:?xt=urn:btih:{}", title.len()) })
}

/// A context with a torrent client set up (releases for a client that isn't are left out).
fn ready(name: &str) -> crate::test_ctx::TestCtx {
    let st = crate::test_ctx::TestCtx::new(name);
    st.cfg.set_many(&json!({ "qbit_url": "http://qbit.invalid", "torznab_min_seeders": 1 }));
    st
}

fn titles(v: &[Value]) -> Vec<&str> {
    v.iter().map(|x| x["title"].as_str().unwrap()).collect()
}

fn want<'a>(artist: &'a str, album: &'a str) -> Want<'a> {
    Want { artist, album, ..Default::default() }
}

/// Scene and Usenet names are ASCII and drop apostrophes: Beyoncé, Björk, Motörhead and "Don't" still
/// match (diagnosis item 3; accents and apostrophes since the Ke$ha change), and so does AC/DC written
/// "ACDC".
#[test]
fn ascii_release_names_pass_the_gates() {
    assert_eq!(contains("Beyonce-Lemonade-WEB-2016-ENRAGED", "Beyoncé"), 1.0);
    assert_eq!(contains("Bjork - Homogenic (1997) [FLAC]", "Björk"), 1.0);
    assert_eq!(contains("Motorhead-Ace_Of_Spades-REMASTERED-CD-FLAC-2005-GROUP", "Motörhead"), 1.0);
    assert_eq!(contains("Billie_Eilish-Dont_Smile_At_Me-WEB-2017-GROUP", "Don't Smile at Me"), 1.0);
    assert_eq!(contains("The_Beatles-Sgt_Peppers_Lonely_Hearts_Club_Band-WEB-2017", "Sgt. Pepper's Lonely Hearts Club Band"), 1.0);
    assert_eq!(contains("ACDC - Back in Black (1980) [FLAC]", "AC/DC"), 1.0);
    assert_eq!(contains("AC-DC - Back in Black (1980) [FLAC]", "AC/DC"), 1.0);
    assert_eq!(contains("ABBA - Gold [FLAC]", "AC/DC"), 0.0);
    let st = ready("ascii");
    let res = [rel("Beyonce-Lemonade-WEB-2016-ENRAGED", 300.0, 9), rel("ACDC - Back in Black (1980) [FLAC]", 300.0, 9)];
    assert_eq!(titles(&rank_album(&st, &res, &want("Beyoncé", "Lemonade"))), ["Beyonce-Lemonade-WEB-2016-ENRAGED"]);
    assert_eq!(titles(&rank_album(&st, &res, &want("AC/DC", "Back in Black"))), ["ACDC - Back in Black (1980) [FLAC]"]);
    // a song's release named in ASCII ranks as one named with the accents and apostrophes: "Beyonce" and
    // "Dont" are the names, not extra words
    let song = |t: &str, artist: &str, title: &str| score_song(t, 40e6, Some(9), "torrent", artist, title).unwrap().0;
    assert_eq!(song("Beyonce - Halo [FLAC]", "Beyoncé", "Halo"), song("Beyoncé - Halo [FLAC]", "Beyoncé", "Halo"));
    assert_eq!(song("Billie Eilish - Dont Smile at Me [FLAC]", "Billie Eilish", "Don't Smile at Me"),
               song("Billie Eilish - Don't Smile at Me [FLAC]", "Billie Eilish", "Don't Smile at Me"));
    assert_eq!(song("Kesha - Blow [FLAC]", "Ke$ha", "Blow"), song("Ke$ha - Blow [FLAC]", "Ke$ha", "Blow"));
}

/// A catalogue title with its edition ("Rodeo (Expanded Edition)", which Deezer and Apple add) matches
/// releases named without it, as the search for it is (diagnosis item 4).
#[test]
fn an_edition_in_the_album_title_does_not_hide_the_release() {
    assert_eq!(album_share("Travis Scott - Rodeo (2015) [FLAC]", "Rodeo (Expanded Edition)"), 1.0);
    assert_eq!(album_share("Daft Punk - Random Access Memories (2013) [FLAC 24-96]", "Random Access Memories (10th Anniversary Edition)"), 1.0);
    assert_eq!(album_share("Fleetwood Mac - Rumours [FLAC]", "Rumours (Super Deluxe)"), 1.0);
    // the name that matters stays: Weezer's colour, a live album
    assert_eq!(core_title("Weezer (Blue Album)"), "Weezer (Blue Album)");
    assert_eq!(core_title("Alive! (Live)"), "Alive! (Live)");
    let st = ready("edition");
    let res = [rel("Travis Scott - Rodeo (2015) [FLAC]", 700.0, 40), rel("Daft Punk - Random Access Memories (2013) [FLAC 24-96]", 1500.0, 30)];
    assert_eq!(titles(&rank_album(&st, &res, &want("Travis Scott", "Rodeo (Expanded Edition)"))), ["Travis Scott - Rodeo (2015) [FLAC]"]);
    assert_eq!(titles(&rank_album(&st, &res, &want("Daft Punk", "Random Access Memories (10th Anniversary Edition)"))).len(), 1);
}

/// A live recording of the album is never picked for a request of the studio album, even in FLAC against an
/// MP3 studio release; asked for live, it is.
#[test]
fn a_live_release_is_not_the_studio_album() {
    let st = ready("live");
    let res = [rel("Travis Scott - Rodeo (Live at Coachella 2016) [FLAC]", 700.0, 90), rel("Travis Scott - Rodeo (2015) [MP3 320]", 150.0, 5)];
    assert_eq!(titles(&rank_album(&st, &res, &want("Travis Scott", "Rodeo"))), ["Travis Scott - Rodeo (2015) [MP3 320]"]);
    assert_eq!(titles(&rank_album(&st, &res, &want("Travis Scott", "Rodeo (Live at Coachella 2016)")))[0], "Travis Scott - Rodeo (Live at Coachella 2016) [FLAC]");
}

/// A self-titled album: every release of the artist names it once, so a release must name it twice (or
/// carry its year), "Led Zeppelin II" is another album unless it is from the year asked for, and the
/// release of the year asked for comes first (diagnosis item 8).
#[test]
fn a_self_titled_album_is_not_any_album_of_the_artist() {
    let st = ready("self-titled");
    let res = [
        rel("Led Zeppelin - IV (1971) [FLAC]", 400.0, 900),
        rel("Led Zeppelin - Physical Graffiti (1975) [FLAC]", 800.0, 700),
        rel("Led Zeppelin - Mothership (2007) [FLAC]", 900.0, 500),
        rel("Led Zeppelin - Led Zeppelin IV (1971) [FLAC]", 400.0, 800),
        rel("Led Zeppelin - Led Zeppelin II (1969) [FLAC]", 400.0, 600),
        rel("Led Zeppelin - Led Zeppelin (1969) [FLAC]", 300.0, 40),
        rel("Led_Zeppelin-Led_Zeppelin-REMASTERED-CD-FLAC-2014-GROUP", 300.0, 20),
    ];
    let w = Want { year: Some(1969), ..want("Led Zeppelin", "Led Zeppelin") };
    let got = rank_album(&st, &res, &w);
    let t = titles(&got);
    assert_eq!(t[0], "Led Zeppelin - Led Zeppelin (1969) [FLAC]", "{got:#?}");
    for other in ["IV (1971)", "Physical Graffiti", "Mothership", "Led Zeppelin IV"] {
        assert!(!t.iter().any(|x| x.contains(other)), "{other} is another album: {t:?}");
    }
    // the 1969 "II" is let through by its year, below the album itself
    let ii = t.iter().position(|x| x.contains("Led Zeppelin II")).unwrap();
    assert!(ii > 0, "{t:?}");
    // without a year asked for, a release must name the album twice
    let t2 = titles(&rank_album(&st, &res, &want("Led Zeppelin", "Led Zeppelin"))).iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert!(t2.contains(&"Led Zeppelin - Led Zeppelin (1969) [FLAC]".to_string()) && !t2.iter().any(|x| x.contains(" II ") || x.contains(" IV ")), "{t2:?}");
    // Weezer's Blue Album is named for its colour
    let res = [rel("Weezer - Pinkerton (1996) [FLAC]", 300.0, 90), rel("Weezer - Weezer (Blue Album) (1994) [FLAC]", 300.0, 9)];
    assert_eq!(titles(&rank_album(&st, &res, &Want { year: Some(1994), ..want("Weezer", "Weezer") })), ["Weezer - Weezer (Blue Album) (1994) [FLAC]"]);
}

/// Words beyond the album's count against a release, and so does another year than the album's, but an
/// album named for a year keeps it.
#[test]
fn releases_that_only_share_words_with_the_album_rank_lower() {
    let st = ready("extra");
    let res = [rel("Travis Scott - Days Before Rodeo (2014) [MP3 320]", 120.0, 60), rel("Travis Scott - Rodeo (2015) [MP3 320]", 130.0, 50)];
    let got = rank_album(&st, &res, &Want { year: Some(2015), ..want("Travis Scott", "Rodeo") });
    assert_eq!(titles(&got)[0], "Travis Scott - Rodeo (2015) [MP3 320]", "{got:#?}");
    // Taylor Swift's "1989" (2014): the album's name is no wrong year
    let res = [rel("Taylor Swift - 1989 [FLAC]", 400.0, 9)];
    let a = rank_album(&st, &res, &Want { year: Some(2014), ..want("Taylor Swift", "1989") });
    let b = rank_album(&st, &res, &want("Taylor Swift", "1989"));
    assert_eq!(a[0]["score"], b[0]["score"]);
}

/// The size a release may have follows the album's track count when it is known: a two-track single's
/// MP3 of 9 MB and a 20-track hi-res set of 6 GB pass; without a count the window stays 15 MB – 4 GB.
#[test]
fn the_size_window_follows_the_track_count() {
    let st = ready("sizes");
    let single = [rel("Some Artist - Small Single (2020) [MP3 320]", 9.0, 9)];
    assert!(rank_album(&st, &single, &want("Some Artist", "Small Single")).is_empty());
    assert_eq!(rank_album(&st, &single, &Want { tracks: Some(2), ..want("Some Artist", "Small Single") }).len(), 1);
    let big = [rel("Some Artist - Big Box (2020) [FLAC 24-192]", 6000.0, 9)];
    assert!(rank_album(&st, &big, &want("Some Artist", "Big Box")).is_empty());
    assert_eq!(rank_album(&st, &big, &Want { tracks: Some(20), ..want("Some Artist", "Big Box") }).len(), 1);
}

/// The Prowlarr address as people paste it: with spaces or a line break, or one indexer's Torznab feed
/// (diagnosis item 11).
#[test]
fn the_prowlarr_address_is_its_base_address() {
    assert_eq!(prowlarr_base(" http://prowlarr:9696/ \n"), "http://prowlarr:9696");
    assert_eq!(prowlarr_base("http://prowlarr:9696/1/api"), "http://prowlarr:9696");
    assert_eq!(prowlarr_base("http://prowlarr:9696/api/v1/"), "http://prowlarr:9696");
    assert_eq!(prowlarr_base("https://example.org/prowlarr/api"), "https://example.org/prowlarr");
    assert_eq!(prowlarr_base("https://example.org/prowlarr"), "https://example.org/prowlarr");
}

/// Test connection counts the indexers that search music: one set up for films and series only finds
/// nothing in Encore's searches (diagnosis item 10).
#[test]
fn test_connection_counts_the_indexers_that_search_music() {
    let cats = |ids: &[i64]| json!({ "categories": ids.iter().map(|i| json!({ "id": i, "subCategories": [] })).collect::<Vec<_>>() });
    let list = json!([
        { "name": "films", "enable": true, "capabilities": cats(&[2000, 5000]) },
        { "name": "music", "enable": true, "capabilities": cats(&[3000]) },
        { "name": "sub only", "enable": true, "capabilities": { "categories": [{ "id": 8000, "subCategories": [{ "id": 3040 }] }] } },
        { "name": "unknown", "enable": true },
        { "name": "off", "enable": false, "capabilities": cats(&[3000]) },
    ]);
    assert_eq!(music_indexers(&list), (4, 3));
    assert_eq!(music_indexers(&json!([{ "enable": true, "capabilities": cats(&[2000]) }])), (1, 0));
    assert_eq!(music_indexers(&json!([])), (0, 0));
}

async fn serve(app: axum::Router) -> String {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move {
        let _ = axum::serve(l, app).await;
    });
    url
}

/// The requests' own searches go to Prowlarr two at a time (a discography queues every album at once) – in
/// the indexers' line the worker takes around them –, and the picker's don't wait behind them (diagnosis
/// item 7).
#[tokio::test]
async fn request_searches_take_turns() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    let (now, most) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let (n2, m2) = (now.clone(), most.clone());
    let app = axum::Router::new().route("/api/v1/search", axum::routing::get(move || {
        let (now, most) = (n2.clone(), m2.clone());
        async move {
            let n = now.fetch_add(1, Ordering::SeqCst) + 1;
            most.fetch_max(n, Ordering::SeqCst);
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            now.fetch_sub(1, Ordering::SeqCst);
            axum::Json(json!([]))
        }
    }));
    let url = serve(app).await;
    let st = crate::test_ctx::TestCtx::new("turns");
    st.cfg.set_many(&json!({ "prowlarr_url": url, "prowlarr_api_key": "k" }));
    let lines = crate::line::Lines::default();
    let all = (0..6).map(|i| {
        let lines = lines.clone();
        let st = &st;
        async move {
            let _slot = lines.indexers.acquire(i, crate::line::TORRENTS, &lines.pressed, || {}).await;
            search(st, if i % 2 == 0 { "a b" } else { "c d" }).await
        }
    });
    for r in futures_util::future::join_all(all).await {
        assert!(r.unwrap().is_empty());
    }
    assert_eq!(most.load(Ordering::SeqCst), 2, "two at a time");
    // the picker's search goes at once, even while two requests' searches run
    most.store(0, Ordering::SeqCst);
    let _a = lines.indexers.acquire(10, crate::line::TORRENTS, &lines.pressed, || {}).await;
    let _b = lines.indexers.acquire(11, crate::line::TORRENTS, &lines.pressed, || {}).await;
    let r = tokio::time::timeout(std::time::Duration::from_secs(5), search_in(&st, "x y", &["3000"])).await;
    assert!(r.is_ok_and(|r| r.is_ok()));
}

/// A slow indexer holds Prowlarr's answer: after two minutes Encore says so instead of "unreachable"
/// (diagnosis item 6).
#[tokio::test(start_paused = true)]
async fn a_prowlarr_that_takes_too_long_is_named_as_slow() {
    let app = axum::Router::new().route("/api/v1/search", axum::routing::get(|| async {
        tokio::time::sleep(std::time::Duration::from_secs(600)).await;
        axum::Json(json!([]))
    }));
    let url = serve(app).await;
    let st = crate::test_ctx::TestCtx::new("slow");
    st.cfg.set_many(&json!({ "prowlarr_url": url, "prowlarr_api_key": "k" }));
    let e = search_in(&st, "a b", &["3000"]).await.unwrap_err().to_string();
    assert!(e.starts_with("Prowlarr took more than 2 minutes to answer – one of its indexers is slow"), "{e}");
}

/// A CD image with a cue sheet ranks below a release in tracks, even an MP3 one: Encore doesn't split
/// images, so it would arrive as one long song (diagnosis item 26). An album called "Image" is no image.
#[test]
fn a_cd_image_ranks_below_a_release_in_tracks() {
    let st = ready("image");
    let res = [rel("Pink Floyd - The Wall (1979) [FLAC image+cue]", 700.0, 90), rel("Pink Floyd - The Wall (1979) [MP3 320]", 180.0, 20),
               rel("Pink Floyd - The Wall (1979) [FLAC tracks]", 650.0, 10)];
    assert_eq!(titles(&rank_album(&st, &res, &want("Pink Floyd", "The Wall"))),
               ["Pink Floyd - The Wall (1979) [FLAC tracks]", "Pink Floyd - The Wall (1979) [MP3 320]", "Pink Floyd - The Wall (1979) [FLAC image+cue]"]);
    let res = [rel("Magazine - Image (2001) [FLAC]", 400.0, 5)];
    assert_eq!(rank_album(&st, &res, &want("Magazine", "Image"))[0]["score"], rank_album(&st, &[rel("Magazine - Picture (2001) [FLAC]", 400.0, 5)], &want("Magazine", "Picture"))[0]["score"]);
}

/// Prowlarr's API key, which it writes into every download link, never reaches the apps: the picker's
/// rows carry the link sealed, and only a link Encore sealed gets the key back when the pick is
/// downloaded – not one written by hand to another address, nor one whose seal was changed (diagnosis
/// item 20: the key travelled in the picker's rows to everyone who may pick downloads).
#[test]
fn prowlarrs_key_never_reaches_the_apps() {
    let st = ready("seal");
    let key = "0123456789abcdef0123456789abcdef";
    st.cfg.set_many(&json!({ "prowlarr_url": "http://prowlarr:9696", "prowlarr_api_key": key }));
    let link = format!("http://prowlarr:9696/3/download?apikey={key}&link=eyJ0b3Jy&file=Rodeo+(2015)");
    let sealed = seal_key(&st, &link);
    assert!(!sealed.contains(key) && sealed.starts_with("http://prowlarr:9696/3/download?apikey=encore-seal-"), "{sealed}");
    assert!(sealed.ends_with("&link=eyJ0b3Jy&file=Rodeo+(2015)"), "the rest of the link as it was: {sealed}");
    assert_eq!(unseal_key(&st, &sealed), link);
    // the seal moved to a link of someone's own, or the link changed: the key stays out
    let elsewhere = sealed.replace("http://prowlarr:9696/3/download", "http://example.net/steal");
    assert_eq!(unseal_key(&st, &elsewhere), elsewhere);
    let changed = sealed.replace("link=eyJ0b3Jy", "link=other");
    assert_eq!(unseal_key(&st, &changed), changed);
    // links without the key (magnets, an indexer's own links) and links the worker keeps are as they were
    let magnet = "magnet:?xt=urn:btih:c12fe1c06bba254a9dc9f519b335aa7c1367a88a&dn=Rodeo";
    assert_eq!(seal_key(&st, magnet), magnet);
    assert_eq!(unseal_key(&st, &link), link);
    // a new key: what was sealed with the old one stays sealed (the download then fails as any bad link)
    st.cfg.set_many(&json!({ "prowlarr_api_key": "fedcba9876543210fedcba9876543210" }));
    assert_eq!(unseal_key(&st, &sealed), sealed);
}

/// A song request's releases: its own first, then its album's (only the song's file is taken from those),
/// lossy ones left out for a lossless-only request.
#[test]
fn song_requests_rank_their_own_and_their_albums_releases() {
    let st = ready("rank_song");
    let songs = [rel("Travis Scott - 90210 (2015) [FLAC]", 40.0, 9), rel("Travis Scott - 90210 [MP3 320]", 10.0, 9), rel("Travis Scott - Antidote [FLAC]", 40.0, 9)];
    let albums = [rel("Travis Scott - Rodeo (2015) [FLAC]", 600.0, 9)];
    let got = rank_song(&st, &songs, Some(("Rodeo", &albums)), "Travis Scott", "90210", false);
    assert_eq!(titles(&got), ["Travis Scott - 90210 (2015) [FLAC]", "Travis Scott - 90210 [MP3 320]", "Travis Scott - Rodeo (2015) [FLAC]"]);
    assert_eq!(got[2]["song_only"], true);
    assert_eq!(got[2]["album"], "Rodeo");
    assert!(got[0]["song_only"].is_null());
    let lossless = rank_song(&st, &songs, None, "Travis Scott", "90210", true);
    assert_eq!(titles(&lossless), ["Travis Scott - 90210 (2015) [FLAC]"]);
    // no torrent client: no torrent
    let none = crate::test_ctx::TestCtx::new("rank_song_none");
    assert!(rank_song(&none, &songs, Some(("Rodeo", &albums)), "Travis Scott", "90210", false).is_empty());
}
