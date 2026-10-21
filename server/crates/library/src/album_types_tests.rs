//! The classification of albums (§9.1 of docs/contracts/library-types.md): pure, no files.
use super::*;

fn song(title: &str, track: i64) -> Song {
    Song { title: title.into(), track, ..Default::default() }
}

/// An album of `n` songs titled "Song 1" … with track numbers.
fn songs(n: usize) -> Vec<Song> {
    (1..=n).map(|i| song(&format!("Song {i}"), i as i64)).collect()
}

fn album(title: &str, artist: &str, songs: Vec<Song>) -> AlbumIn {
    AlbumIn { album: title.into(), albumartist: artist.into(), songs, ..Default::default() }
}

fn mb(primary: &str, secondary: &[&str], tracks: i64) -> Option<Stored> {
    Some(Stored { chosen: None, source: "musicbrainz".into(), release_type: Some(primary.into()), secondary: secondary.iter().map(|s| s.to_string()).collect(), tracks })
}

fn with(mut a: AlbumIn, stored: Option<Stored>) -> AlbumIn {
    a.stored = stored;
    a
}

fn fk(a: &AlbumIn) -> (&'static str, &'static str) {
    let c = classify(a);
    (c.kind, c.format)
}

#[test]
fn musicbrainz_facts_decide_kind_and_format() {
    assert_eq!(fk(&with(album("Live at X", "A", songs(10)), mb("Album", &["Live"], 10))), ("live", "album"));
    assert_eq!(fk(&with(album("Song (Remixes)", "A", songs(4)), mb("Single", &["Remix"], 4))), ("remix", "single"));
    assert_eq!(fk(&with(album("Five", "A", songs(5)), mb("EP", &[], 5))), ("studio", "ep"));
    assert_eq!(fk(&with(album("Hits Live", "A", songs(12)), mb("Album", &["Compilation", "Live"], 12))), ("compilation", "album"));
    // a MusicBrainz title is the release's name: "Live Through This" is a studio album
    let c = classify(&with(album("Live Through This", "Hole", songs(12)), mb("Album", &[], 12)));
    assert_eq!((c.kind, c.format, c.why), ("studio", "album", "musicbrainz"));
    assert_eq!(fk(&with(album("Radio Session", "A", songs(6)), mb("Broadcast", &[], 6))), ("other", "album"));
    assert_eq!(fk(&with(album("Dune", "Hans Zimmer", songs(20)), mb("Album", &["Soundtrack"], 20))), ("soundtrack", "album"));
}

#[test]
fn releasetype_tags_are_parsed() {
    assert_eq!(parse_releasetype("album; live"), (Some("Album"), vec!["Live"]));
    assert_eq!(parse_releasetype("Album/Live"), (Some("Album"), vec!["Live"]));
    assert_eq!(parse_releasetype("album, compilation"), (Some("Album"), vec!["Compilation"]));
    assert_eq!(parse_releasetype("ep"), (Some("EP"), vec![]));
    assert_eq!(parse_releasetype("single\u{0}remix"), (Some("Single"), vec!["Remix"]));
    assert_eq!(parse_releasetype(""), (None, vec![]));
    assert_eq!(parse_releasetype("garbage"), (None, vec![]));
    assert_eq!(releasetype_tag("Album", &["Live".into(), "Compilation".into()]), "album; live; compilation");
    // the album takes the most common value among its songs
    let mut s = songs(5);
    for (i, x) in s.iter_mut().enumerate() {
        x.releasetype = if i < 3 { "album; live".into() } else if i == 3 { "album".into() } else { String::new() };
    }
    let c = classify(&album("Concert", "A", s));
    assert_eq!((c.kind, c.format, c.why), ("live", "album", "tags"));
}

#[test]
fn titles_decide_without_facts() {
    for (title, kind) in [
        ("Rodeo (Live at Coachella)", "live"), ("Live at Wembley", "live"), ("Greatest Hits", "compilation"), ("Astroworld (The Remixes)", "remix"),
        ("Dune (Original Motion Picture Soundtrack)", "soundtrack"), ("Early Demos", "demo"), ("Rodeo (Instrumentals)", "karaoke"),
        ("Song (Karaoke Version)", "karaoke"), ("Live Through This", "studio"), ("Alive", "studio"), ("Coverage", "studio"), ("Rodeo (Deluxe)", "studio"),
    ] {
        let c = classify(&album(title, "A", songs(10)));
        assert_eq!(c.kind, kind, "{title}");
        if kind != "studio" {
            assert_eq!(c.why, "title", "{title}");
        }
    }
}

#[test]
fn song_titles_share() {
    let titled = |n: usize, of: usize, mark: &str| -> Vec<Song> {
        (1..=of).map(|i| song(&if i <= n { format!("Song {i} {mark}") } else { format!("Song {i}") }, i as i64)).collect()
    };
    assert_eq!(classify(&album("Tour", "A", titled(8, 10, "(Live)"))).kind, "live");
    assert_eq!(classify(&album("Tour", "A", titled(5, 10, "(Live)"))).kind, "studio");
    assert_eq!(classify(&album("Song", "A", vec![song("Song - Live", 1)])).kind, "live");
    let c = classify(&album("Club", "A", titled(7, 10, "(Night Remix)")));
    assert_eq!((c.kind, c.why), ("remix", "songs"));
    // MusicBrainz says a studio album: the songs' titles still decide while the kind is studio
    assert_eq!(classify(&with(album("Tour", "A", titled(10, 10, "(Live)")), mb("Album", &[], 10))).kind, "live");
}

#[test]
fn various_artists_and_compilation_flag() {
    for artist in ["Various Artists", "VA", "V.A.", "various", "Verschiedene Interpreten"] {
        assert_eq!(classify(&album("Hits 2026", artist, songs(20))).kind, "compilation", "{artist}");
    }
    let mut a = album("Hits 2026", "Someone", songs(20));
    a.albumartist_id = VA_MBID.into();
    let c = classify(&a);
    assert_eq!((c.kind, c.why), ("compilation", "various"));
    let mut s = songs(10);
    for x in s.iter_mut().take(6) {
        x.compilation = true;
    }
    assert_eq!(classify(&album("Mix", "Someone", s)).kind, "compilation");
    let mut s = songs(10);
    for x in s.iter_mut().take(5) {
        x.compilation = true;
    }
    assert_eq!(classify(&album("Mix", "Someone", s)).kind, "studio", "half isn't most");
}

#[test]
fn partial_releases_are_one_offs() {
    // VA, one song, track 7
    let c = classify(&album("Now 99", "Various Artists", vec![song("Hit", 7)]));
    assert_eq!((c.kind, c.format, c.why, c.release_label.as_str()), ("compilation", "one_off", "partial", "Various artists"));
    // a compilation with 3 of 6 / 3 of 5 / 4 of 30 songs
    assert_eq!(fk(&with(album("Best Of", "A", songs(3)), mb("Album", &["Compilation"], 6))), ("compilation", "one_off"));
    assert_eq!(fk(&with(album("Best Of", "A", songs(3)), mb("Album", &["Compilation"], 5))), ("compilation", "album"));
    assert_eq!(fk(&with(album("Best Of", "A", songs(4)), mb("Album", &["Compilation"], 30))), ("compilation", "album"));
    // a soundtrack with two songs, its size unknown
    let c = classify(&with(album("Film", "A", vec![song("One", 0), song("Two", 0)]), mb("Album", &["Soundtrack"], 0)));
    assert_eq!((c.format, c.why, why_text(c.why, c.provider, c.present, c.total).as_str()), ("one_off", "partial", "Only 2 of its songs are here"));
    // a remix album with 1 of 12
    let c = classify(&with(album("Remixed", "A", vec![song("Song (X Remix)", 3)]), mb("Album", &["Remix"], 12)));
    assert_eq!((c.kind, c.format, c.release_label.as_str()), ("remix", "one_off", "Remix"));
    // a lone song of a studio album
    let c = classify(&with(album("After Hours", "The Weeknd", vec![song("Blinding Lights", 9)]), mb("Album", &[], 14)));
    assert_eq!((c.kind, c.format, c.why), ("studio", "one_off", "partial"));
    assert_eq!(why_text(c.why, c.provider, c.present, c.total), "Only 1 of its 14 songs are here");
    assert_eq!(c.release_label, "Album");
    // a lone song on a single whose title differs (a B-side): a single
    assert_eq!(fk(&with(album("A-Side", "A", vec![song("B-Side", 2)]), mb("Single", &[], 2))), ("studio", "single"));
    // a one-song album titled as the song; two songs, no facts, the album named after one of them
    assert_eq!(fk(&album("Song", "A", vec![song("Song", 1)])), ("studio", "single"));
    assert_eq!(fk(&album("Song", "A", vec![song("Song", 1), song("Other", 2)])), ("studio", "single"));
    // three songs, no facts: an album; " - EP" in the title: an EP
    assert_eq!(fk(&album("Trio", "A", songs(3))), ("studio", "album"));
    let c = classify(&album("Four - EP", "A", songs(4)));
    assert_eq!((c.format, c.why), ("ep", "title"));
}

#[test]
fn the_owners_choice_wins() {
    let mut a = album("Now 99", "Various Artists", vec![song("Hit", 7)]);
    a.stored = Some(Stored { chosen: Some("album".into()), ..Default::default() });
    let c = classify(&a);
    assert_eq!((c.format, c.kind, c.key(), c.why, c.auto_format, c.auto_why), ("album", "compilation", "compilation", "chosen", "one_off", "partial"));
    let j = c.json();
    assert_eq!(j["chosen"], "album");
    assert_eq!(j["auto"]["label"], "Compilation · One-off");
    assert_eq!(j["auto"]["why_text"], "Only 1 of its 7 songs are here");
    assert_eq!(j["why_text"], "You chose this");
    let mut a = with(album("Rodeo", "Travis Scott", songs(14)), mb("Album", &[], 14));
    a.stored.as_mut().unwrap().chosen = Some("one_off".into());
    assert_eq!(classify(&a).key(), "one_off");
    let mut a = album("Rodeo", "Travis Scott", songs(12));
    a.stored = Some(Stored { chosen: Some("single".into()), ..Default::default() });
    let c = classify(&a);
    assert_eq!((c.format, c.auto_format, c.key()), ("single", "album", "singles"));
}

#[test]
fn types_are_a_partition() {
    // a fixed pseudo-random fixture of 500 albums
    let mut seed: u64 = 42;
    let mut next = |m: u64| {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (seed >> 33) % m
    };
    let titles = ["Rodeo", "Live at Wembley", "Greatest Hits", "The Remixes", "Original Motion Picture Soundtrack", "Demos", "Karaoke Hits", "Song", "Four - EP", "Radio"];
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for i in 0..500 {
        let n = 1 + next(15) as usize;
        let mut a = album(titles[next(titles.len() as u64) as usize], if next(10) == 0 { "Various Artists" } else { "A" }, songs(n));
        a.stored = match next(4) {
            0 => mb(["Album", "EP", "Single", "Broadcast", "Other"][next(5) as usize], &[["Live"], ["Compilation"], ["Remix"], ["Soundtrack"], ["DJ-mix"], ["Demo"]][next(6) as usize], next(30) as i64),
            1 => Some(Stored { chosen: Some(FORMATS[next(4) as usize].0.into()), ..Default::default() }),
            _ => None,
        };
        let c = classify(&a);
        let key = c.key();
        assert!(TYPES.iter().any(|t| t.0 == key), "album {i}: {key}");
        // exactly one filter key: the Keys sets of every type together hold it once
        let holding = TYPES.iter().filter(|t| Pick::Keys(vec![t.0]).wants(Some(&c))).count();
        assert_eq!(holding, 1, "album {i} {c:?}");
        *counts.entry(key).or_default() += 1;
    }
    assert_eq!(counts.values().sum::<usize>(), 500);
    assert_eq!(TYPES.map(|t| t.0), ["album", "live", "compilation", "remix", "soundtrack", "demo", "dj_mix", "mixtape", "karaoke", "singles", "one_off", "other"]);
    assert_eq!(TYPES.map(|t| t.1)[9], "Singles & EPs");
    assert_eq!(chip_label("ep", "live"), "Live · EP");
    assert_eq!(chip_label("one_off", "compilation"), "Compilation · One-off");
    assert_eq!(chip_label("album", "studio"), "Album");
    assert_eq!(chip_label("single", "remix"), "Remix · Single");
    assert_eq!(chip_label("album", "live"), "Live");
    // the filters
    assert_eq!(Pick::of("", "", false), Ok(Pick::NoOneOffs));
    assert_eq!(Pick::of("", "", true), Ok(Pick::All));
    assert_eq!(Pick::of("", "albums", false), Ok(Pick::Formats(vec!["album"])));
    assert_eq!(Pick::of("", "singles", true), Ok(Pick::Formats(vec!["single", "ep"])));
    assert_eq!(Pick::of("live,one_off", "albums", false), Ok(Pick::Keys(vec!["live", "one_off"])));
    assert_eq!(Pick::of("bogus", "", false), Err("Unknown album type".to_string()));
    let live_ep = Class { format: "ep", kind: "live", ..Default::default() };
    assert!(Pick::Keys(vec!["ep"]).wants(Some(&live_ep)) && Pick::Keys(vec!["live"]).wants(Some(&live_ep)) && !Pick::Keys(vec!["singles"]).wants(Some(&live_ep)));
    let live_album = Class { kind: "live", ..Default::default() };
    assert!(!Pick::Keys(vec!["album"]).wants(Some(&live_album)), "Studio albums leave live albums out");
}

#[test]
fn why_texts() {
    assert_eq!(why_text("chosen", "", 0, 0), "You chose this");
    assert_eq!(why_text("musicbrainz", "", 0, 0), "From MusicBrainz");
    assert_eq!(why_text("provider", "deezer", 0, 0), "From Deezer");
    assert_eq!(why_text("provider", "apple", 0, 0), "From Apple Music");
    assert_eq!(why_text("provider", "discogs", 0, 0), "From Discogs");
    assert_eq!(why_text("tags", "", 0, 0), "From the file tags");
    assert_eq!(why_text("title", "", 0, 0), "From the album title");
    assert_eq!(why_text("songs", "", 0, 0), "Most of its songs say so in their titles");
    assert_eq!(why_text("various", "", 0, 0), "Various artists");
    assert_eq!(why_text("partial", "", 1, 14), "Only 1 of its 14 songs are here");
    assert_eq!(why_text("partial", "", 2, 0), "Only 2 of its songs are here");
    assert_eq!(why_text("size", "", 0, 0), "Guessed from the number of songs");
    assert_eq!(why_text("default", "", 0, 0), "No type known");
    // a store's facts
    let rel = json!({ "provider": "Deezer", "id": "deezer:1", "type": "Compile", "title": "Hits" });
    let f = facts_of_release(&rel, 20).unwrap();
    assert_eq!((f.source, f.primary.as_str(), f.secondary.clone(), f.tracks), ("deezer", "Album", vec!["Compilation".to_string()], 20));
    let a = AlbumIn { album: "Hits".into(), albumartist: "A".into(), songs: songs(20), stored: Some(Stored { source: "deezer".into(), release_type: Some(f.primary), secondary: f.secondary, tracks: 20, chosen: None }), ..Default::default() };
    let c = classify(&a);
    assert_eq!((c.kind, c.why, why_text(c.why, c.provider, c.present, c.total).as_str()), ("compilation", "provider", "From Deezer"));
    let rel = json!({ "provider": "Apple Music", "id": "apple:2", "type": "", "title": "Song - Single" });
    assert_eq!(facts_of_release(&rel, 1).unwrap().primary, "Single");
    let rel = json!({ "provider": "MusicBrainz", "release_group_id": "rg1", "type": "Album", "secondary": ["Live"], "track_count": 12 });
    assert_eq!(facts_of_release(&rel, 10).unwrap(), Facts { source: "musicbrainz", id: "rg1".into(), primary: "Album".into(), secondary: vec!["Live".into()], tracks: 12 });
    let rec = json!({ "release_group_id": "rg2", "release_type": "Album", "release_secondary": ["Compilation"], "release_track_count": 18 });
    assert_eq!(facts_of_recording(&rec).unwrap().tracks, 18);
    assert!(facts_of_recording(&json!({})).is_none());
}

/// 20 000 songs on 2 000 albums are classified in well under a second (debug), once per state of the library;
/// a write to album_types builds it again, once.
#[test]
fn classify_20000_songs_quickly() {
    let dir = encore_core::tmp::dir("types-perf");
    let db = Db::open(&dir).unwrap();
    db.write_now(|c| c.transaction(|c| {
        for i in 0..20_000i64 {
            let a = i / 10;
            c.execute("INSERT INTO tracks (path, album_id, title, album, albumartist, track, releasetype, tracktotal, compilation) VALUES (?,?,?,?,?,?,?,?,?)",
                p![format!("/m/{a}/{i}.flac"), format!("album{a}"), format!("Song {i}"), format!("Album {a}"), format!("Artist {}", a % 300), i % 10 + 1,
                   if a % 7 == 0 { "album; live" } else { "" }, 10, 0])?;
        }
        Ok::<_, rusqlite::Error>(())
    })).unwrap();
    let types = Types::default();
    let t = std::time::Instant::now();
    let m = types.get(&db);
    let ms = t.elapsed().as_millis();
    eprintln!("classified {} albums in {ms} ms", m.len());
    assert_eq!(m.len(), 2000);
    assert!(ms < 1000, "{ms} ms");
    assert_eq!(m["album0"].kind, "live");
    let again = types.get(&db);
    assert!(Arc::ptr_eq(&m, &again), "the same state of the library: the same answer");
    choose(&db, "album1", Some("one_off"), 1).unwrap();
    let after = types.get(&db);
    assert!(!Arc::ptr_eq(&m, &after));
    assert_eq!(after["album1"].format, "one_off");
    assert!(Arc::ptr_eq(&after, &types.get(&db)), "built once");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Step A reads the type tags of songs scanned before 0.36 in batches (a file that can't be read gets '' / 0 /
/// 0 and is not read again); the library version moves once per batch.
#[test]
fn the_tags_pass_reads_old_songs_once() {
    let dir = encore_core::tmp::dir("types-tags");
    let db = Db::open(&dir).unwrap();
    db.write_now(|c| c.transaction(|c| {
        for i in 0..450i64 {
            c.execute("INSERT INTO tracks (path, album_id, title) VALUES (?,?,?)", p![format!("/nowhere/{i}.flac"), "a", format!("S{i}")])?;
        }
        // a song scanned by 0.36 already
        c.execute("INSERT INTO tracks (path, album_id, title, releasetype, tracktotal, compilation) VALUES ('/x.flac', 'b', 'X', 'album', 3, 0)", p![])?;
        Ok::<_, rusqlite::Error>(())
    })).unwrap();
    let v0 = db.library_version();
    let n = read_missing_tags(&db, &encore_jobs::Busy::default());
    assert_eq!(n, 450);
    assert_eq!(db.library_version(), v0 + 3, "three batches of up to 200");
    assert_eq!(db.one("SELECT COUNT(*) n FROM tracks WHERE releasetype=''", p![]).unwrap().unwrap().i("n"), 450);
    assert_eq!(db.one("SELECT releasetype FROM tracks WHERE path='/x.flac'", p![]).unwrap().unwrap().s("releasetype"), "album");
    assert_eq!(read_missing_tags(&db, &encore_jobs::Busy::default()), 0, "read once");
    // the candidates for MusicBrainz: likely one-offs first; albums with a type tag or facts left out
    db.execute("INSERT INTO tracks (path, album_id, title, album, mb_releasegroupid, releasetype) VALUES ('/c1.flac', 'c', 'C', 'Big', 'rg-c', '')", p![]).unwrap();
    for i in 0..5 {
        db.execute("INSERT INTO tracks (path, album_id, title, album, mb_releasegroupid, releasetype) VALUES (?, 'd', 'D', 'Dee', 'rg-d', '')", p![format!("/d{i}.flac")]).unwrap();
    }
    db.execute("INSERT INTO tracks (path, album_id, title, album, mb_trackid, releasetype) VALUES ('/e.flac', 'e', 'E', 'Eee', 'rec-e', '')", p![]).unwrap();
    db.execute("INSERT INTO tracks (path, album_id, title, album, mb_releasegroupid, releasetype) VALUES ('/f.flac', 'f', 'F', 'Fff', 'rg-f', '')", p![]).unwrap();
    store_facts(&db, &["f".to_string()], &Facts { source: "musicbrainz", id: "rg-f".into(), primary: "Album".into(), secondary: vec![], tracks: 10 });
    let c: Vec<String> = candidates(&db, 300).into_iter().map(|c| c.album_id).collect();
    assert_eq!(c, vec!["c", "e", "d"]);
    assert_eq!(candidates(&db, 2).len(), 2, "at most the limit");
    // not on MusicBrainz: not asked again for 30 days
    let mut found = vec![Found::Unknown("c".into())];
    write_found(&db, &mut found);
    assert_eq!(candidates(&db, 300).into_iter().map(|c| c.album_id).collect::<Vec<_>>(), vec!["e", "d"]);
    db.execute("UPDATE album_types SET looked_up_at=? WHERE album_id='c'", p![now() - LOOK_AGAIN_AFTER - 10.0]).unwrap();
    assert_eq!(candidates(&db, 300)[0].album_id, "c");
    // rows of albums that are gone go, unless the owner chose a type
    choose(&db, "gone-chosen", Some("album"), 1).unwrap();
    store_facts(&db, &["gone".to_string()], &Facts { source: "musicbrainz", id: "x".into(), primary: "Album".into(), secondary: vec![], tracks: 1 });
    forget_gone(&db);
    let left: Vec<String> = db.query("SELECT album_id FROM album_types ORDER BY album_id", p![]).unwrap().iter().map(|r| r.s("album_id")).collect();
    assert_eq!(left, vec!["c", "f", "gone-chosen"]);
    let v = db.library_version();
    forget_gone(&db);
    assert_eq!(db.library_version(), v, "nothing to forget: nothing written");
    assert_eq!(Run { tags_read: 450, looked_up: 3, not_found: 1 }.line(), "450 song tags read, 3 albums looked up (1 not on MusicBrainz)");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The owner's choice follows the album when its id changes by Encore's own hand; a retag drops the old facts.
#[test]
fn the_choice_follows_a_new_album_id() {
    let dir = encore_core::tmp::dir("types-carry");
    let db = Db::open(&dir).unwrap();
    db.execute("INSERT INTO tracks (id, path, album_id, title) VALUES (1, '/a/1.flac', 'old', 'One')", p![]).unwrap();
    db.execute("INSERT INTO tracks (id, path, album_id, title) VALUES (2, '/a/2.flac', 'old', 'Two')", p![]).unwrap();
    choose(&db, "old", Some("one_off"), 1).unwrap();
    store_facts(&db, &["old".to_string()], &Facts { source: "musicbrainz", id: "rg".into(), primary: "Album".into(), secondary: vec![], tracks: 10 });
    // one song moved: the album is still there, nothing follows
    db.execute("UPDATE tracks SET album_id='new' WHERE id=1", p![]).unwrap();
    carry(&db, "old", &[1, 2], true);
    assert!(db.one("SELECT 1 x FROM album_types WHERE album_id='new'", p![]).unwrap().is_none());
    db.execute("UPDATE tracks SET album_id='new' WHERE id=2", p![]).unwrap();
    carry(&db, "old", &[1, 2], true);
    let r = db.one("SELECT * FROM album_types WHERE album_id='new'", p![]).unwrap().unwrap();
    assert_eq!(r.s("chosen"), "one_off");
    assert!(r.get("release_type").unwrap().is_null(), "a retag's facts are its own");
    let _ = std::fs::remove_dir_all(&dir);
}
