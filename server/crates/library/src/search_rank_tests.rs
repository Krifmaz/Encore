//! The owner's search "travis scott 90210" against recorded MusicBrainz / Deezer / Apple answers
//! (search_fixtures/, trimmed to the fields Encore reads; no network).
use super::*;

fn fx(name: &str) -> Value {
    let raw = match name {
        "mb_artist" => include_str!("search_fixtures/mb_artist.json"),
        "mb_recording_split" => include_str!("search_fixtures/mb_recording_split.json"),
        "mb_recording_free" => include_str!("search_fixtures/mb_recording_free.json"),
        "mb_rg_split" => include_str!("search_fixtures/mb_rg_split.json"),
        "deezer_plain" => include_str!("search_fixtures/deezer_plain.json"),
        "deezer_split" => include_str!("search_fixtures/deezer_split.json"),
        "deezer_album" => include_str!("search_fixtures/deezer_album.json"),
        "itunes_song" => include_str!("search_fixtures/itunes_song.json"),
        "itunes_album" => include_str!("search_fixtures/itunes_album.json"),
        "itunes_artist" => include_str!("search_fixtures/itunes_artist.json"),
        _ => panic!("no fixture {name}"),
    };
    serde_json::from_str(raw).unwrap()
}

/// What the first round (the whole query) found.
fn first_round() -> Found {
    Found {
        mb_artists: fx("mb_artist"),
        deezer_tracks: vec![fx("deezer_plain")],
        deezer_albums: fx("deezer_album"),
        itunes_songs: fx("itunes_song"),
        itunes_albums: fx("itunes_album"),
        itunes_artists: fx("itunes_artist"),
        ..Default::default()
    }
}

fn split_for(q: &str, f: &Found, library: &dyn Fn(&str) -> u8) -> Option<Split> {
    let arts: Vec<(String, Vec<String>, f64)> = mb_artists(&f.mb_artists).into_iter().map(|(v, al, sc)| (s(&v["name"]), al, sc)).collect();
    let hits = evidence_hits(f);
    choose_split_with(q, &[], &Evidence { library, artists: &arts, hits: &hits })
}

/// Both rounds, the free-text MusicBrainz songs included (they are where the joke songs came from).
fn search(q: &str) -> (Option<Split>, Value) {
    let mut f = first_round();
    let sp = split_for(q, &f, &|_| 0);
    f.mb_recordings = vec![fx("mb_recording_split"), fx("mb_recording_free")];
    f.mb_groups = vec![fx("mb_rg_split")];
    f.deezer_tracks.insert(0, fx("deezer_split"));
    f.isrcs.insert("108404826".into(), "USSM11507451".into());
    let out = rank(q, sp.as_ref(), &f, &|_| false, "all");
    (sp, out)
}

fn titles(v: &Value, list: &str) -> Vec<String> {
    v[list].as_array().unwrap().iter().map(|x| format!("{} / {}", s(&x["artist"]), s(&x["title"]))).collect()
}

#[test]
fn reads_artist_and_title_in_any_order() {
    let want = Some(Split { artist: "travis scott".into(), title: "90210".into() });
    let f = first_round();
    assert_eq!(split_for("travis scott 90210", &f, &|_| 0), want);
    assert_eq!(split_for("90210 travis scott", &f, &|_| 0), want);
    assert_eq!(split_for("90210 by travis scott", &f, &|_| 0), want);
    assert_eq!(split_for("travis scott - 90210", &f, &|_| 0), want);
    // just the artist: no split
    assert_eq!(split_for("travis scott", &f, &|_| 0), None);
    // a library artist is enough on its own (no Deezer / Apple hits)
    let lib = |n: &str| if fold(n) == "travis scott" { 2 } else { 0 };
    let bare = Found { mb_artists: fx("mb_artist"), ..Default::default() };
    assert_eq!(split_for("90210 travis scott", &bare, &lib), want);
    // a typo in the library artist still counts
    let typo = |n: &str| if fuzzy_eq(&fold(n), "travis scott") { 1 } else { 0 };
    assert_eq!(split_for("trvis scot 90210", &bare, &typo), Some(Split { artist: "trvis scot".into(), title: "90210".into() }));
}

#[test]
fn the_real_song_is_the_top_result() {
    for q in ["travis scott 90210", "90210 travis scott"] {
        let (_, out) = search(q);
        let tracks = titles(&out, "tracks");
        assert_eq!(out["top"]["kind"], "song", "{q}: {}", out["top"]);
        let first = &out["tracks"][0];
        assert_eq!(first["title"], "90210", "{q}: {tracks:?}");
        assert_eq!(first["artist"], "Travis Scott");
        assert_eq!(first["album"], "Rodeo", "the official album, not the compilation");
        assert_eq!(first["release_group_id"], "rg-rodeo");
        assert_eq!(first["isrc"], "USSM11507451");
        // the same recording on a compilation is one entry, the joke songs come after it, uploader junk last
        assert_eq!(tracks.iter().filter(|t| t.as_str() == "Travis Scott / 90210").count(), 2, "{tracks:?}"); // studio + live
        let pos = |needle: &str| tracks.iter().position(|t| t.contains(needle)).unwrap_or(usize::MAX);
        let real = pos("Travis Scott / 90210");
        for junk in ["user-1776339120", "2K Marseille", "Travis Scott Concert", "Scott Travis (Drums)", "travis scott burger", "Karaoke Hits Band"] {
            let p = pos(junk);
            assert!(p == usize::MAX || p > real + 1, "{q}: {junk} at {p}: {tracks:?}");
        }
        assert_eq!(tracks.last().map(String::as_str), Some("user-1776339120 / travis scott"), "{tracks:?}");
        // the Deezer-only "Travis Scott Burger" isn't a MusicBrainz song but stays far below
        assert!(pos("Travis Scott Burger") > 2, "{tracks:?}");
        // albums: Rodeo (the album the song is on) above the tribute; the Deezer-only remix EP is an "ext" entry
        let albums = titles(&out, "albums");
        let apos = |needle: &str| albums.iter().position(|t| t.contains(needle)).unwrap_or(usize::MAX);
        assert_eq!(apos("Travis Scott / Rodeo"), 0, "{albums:?}");
        assert!(apos("Tribute Kings") > 1, "{albums:?}");
        let rodeo = out["albums"].as_array().unwrap().iter().filter(|a| a["title"] == "Rodeo").count();
        assert_eq!(rodeo, 1, "Deezer and Apple's Rodeo merge into MusicBrainz's: {albums:?}");
        let remix = out["albums"].as_array().unwrap().iter().find(|a| a["title"] == "90210 (Remixes)").expect("ext album");
        assert_eq!(remix["ext"]["deezer"], 555000111);
        assert!(remix["id"].as_str().unwrap().starts_with("ext-d"));
        assert_eq!(remix["type"], "EP");
        // artists: Travis Scott first, the band called 90210 still listed
        let names: Vec<&str> = out["artists"].as_array().unwrap().iter().map(|a| a["name"].as_str().unwrap()).collect();
        assert_eq!(names[0], "Travis Scott", "{names:?}");
        assert!(names.contains(&"90210"));
    }
}

#[test]
fn an_artist_name_alone_tops_with_the_artist() {
    let mut f = first_round();
    let sp = split_for("travis scott", &f, &|_| 0);
    f.mb_recordings = vec![fx("mb_recording_free")];
    let out = rank("travis scott", sp.as_ref(), &f, &|_| false, "all");
    assert_eq!(out["top"]["kind"], "artist", "{}", out["top"]);
    assert_eq!(out["artists"][0]["name"], "Travis Scott");
}

#[test]
fn names_and_typos() {
    assert_eq!(base_title("90210 (feat. Kacy Hill)"), "90210");
    assert_eq!(base_title("Hey Jude - Remastered 2015"), "hey jude");
    assert_eq!(title_grade("90210 (feat. Kacy Hill)", "90210"), 3);
    assert_eq!(title_grade("travis scott burger", "90210"), 0);
    assert_eq!(artist_grade("Travis Scott feat. Drake", "travis scott"), 1);
    assert_eq!(artist_grade("Travis Scott", "TRAVIS SCOTT"), 2);
    assert_eq!(artist_grade("Scott Travis", "travis scott"), 0);
    assert_eq!(lev("trvis scot", "travis scott", 2), Some(2));
    assert_eq!(lev("abc", "xyz", 1), None);
    assert!(fuzzy_eq("beyonce", "beyonse"));
    assert!(!fuzzy_eq("ye", "yo"));
    assert!(fuzzy_eq(&fold("Beyoncé"), "beyonce"));
    assert!(USERISH.is_match("user-1776339120"));
    assert!(!USERISH.is_match("Usher"));
    assert!(JUNK.is_match("90210 (In the Style of Travis Scott)"));
    let sp = splits("a b c");
    assert!(sp.contains(&Split { artist: "a".into(), title: "b c".into() }) && sp.contains(&Split { artist: "b c".into(), title: "a".into() }));
    assert_eq!(splits("halo by beyonce")[0], Split { artist: "beyonce".into(), title: "halo".into() });
}


#[test]
fn junk_words_the_query_asked_for_are_not_junk() {
    // "Tribute" by Tenacious D, "Karaoke" by Drake: the song, not a karaoke or tribute version
    assert!(!junk(&["Tribute", "Tenacious D", "Tenacious D"], &fold("tenacious d tribute")));
    assert!(!junk(&["Karaoke", "Drake", "Thank Me Later"], &fold("drake karaoke")));
    // a word nobody asked for still is
    assert!(junk(&["Tribute (Karaoke Version)", "Sing King", ""], &fold("tenacious d tribute")));
    assert!(junk(&["Blinding Lights (Lullaby Version)", "Rockabye Baby!", ""], &fold("the weeknd blinding lights")));
}

#[test]
fn initials_spell_the_first_letters_of_a_names_words() {
    use crate::artists::fold;
    let sp = |w: &str, name: &str| spells(w, &fold(name));
    assert!(sp("gb", "Gnarls Barkley"));
    assert!(sp("atcq", "A Tribe Called Quest"));
    assert!(sp("rhcp", "Red Hot Chili Peppers"));
    assert!(sp("rhcp", "The Red Hot Chili Peppers"));
    assert!(sp("trhcp", "The Red Hot Chili Peppers"));
    assert!(sp("ewf", "Earth, Wind & Fire"));
    assert!(sp("ewaf", "Earth, Wind & Fire"));
    assert!(sp("dsotm", "The Dark Side of the Moon"));
    assert!(sp("sas", "Southern All Stars"));
    assert!(sp("jz", "Jay-Z"));
    // every word, in order, nothing more
    assert!(!sp("gb", "Gnarls Barkley Band"));
    assert!(!sp("bg", "Gnarls Barkley"));
    assert!(!sp("go", "Go"));
    assert!(!sp("go", "Gorillaz"));
    // which query words can be initials: 2–6 letters of an alphabet with capitals
    for w in ["gb", "atcq", "rhcp", "dsotm", "go", "αβ", "бг"] {
        assert!(initials_word(w), "{w}");
    }
    for w in ["g", "abcdefg", "4ad", "真夏", "いと", "a b"] {
        assert!(!initials_word(w), "{w}");
    }
}
