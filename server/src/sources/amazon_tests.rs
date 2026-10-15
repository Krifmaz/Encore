use super::*;
use serde_json::json;

const PLAYLIST_HTML: &str = r#"<html><head><meta property="og:title" content="Fallback name"><meta property="og:image" content="https://m.media-amazon.com/og.jpg"></head><body>
<music-detail-header headline="PLAYLIST" primary-text="All Hits &amp; Classics" secondary-text="Amazon Music" image-src="https://m.media-amazon.com/images/I/cover.jpg"></music-detail-header>
<music-image-row primary-text="Blinding Lights" secondary-text-1="The Weeknd" secondary-text-2="After Hours" secondary-text-3="3:20" primary-href="/albums/B084ZJ4QWS?trackAsin=B084ZHXQ7G" image-src="x.jpg"></music-image-row>
<music-image-row primary-text="Don&#39;t Start Now" secondary-text-1="Dua Lipa" secondary-text-2="Future Nostalgia" secondary-text-3="03:03" primary-href="/albums/B086C6KV8F?trackAsin=B086C3XK4R"></music-image-row>
<music-image-row primary-text="Some Artist" primary-href="/artists/B000APVZ1Q"></music-image-row>
</body></html>"#;

const ALBUM_HTML: &str = r#"<music-detail-header headline="ALBUM" primary-text="Future Nostalgia" secondary-text="Dua Lipa" image-src="https://m.media-amazon.com/fn.jpg"></music-detail-header>
<music-text-row primary-text="Future Nostalgia" secondary-text-1="3:04" primary-href="/albums/B086C6KV8F?trackAsin=B086C1AAAA"></music-text-row>
<music-text-row primary-text="Don't Start Now" secondary-text-1="3:03" primary-href="/albums/B086C6KV8F?trackAsin=B086C3XK4R"></music-text-row>
<music-text-row primary-text="Levitating (feat. DaBaby)" secondary-text-1="Dua Lipa, DaBaby" secondary-text-2="3:23" primary-href="/albums/B086C6KV8F?trackAsin=B086C2BBBB"></music-text-row>"#;

#[test]
fn links_are_recognised() {
    let l = parse_url("https://music.amazon.com/playlists/B01M11SBC8?ref=dm_sh_abc").unwrap();
    assert_eq!((l.kind.as_str(), l.id.as_str(), l.host.as_str(), l.track), ("playlist", "B01M11SBC8", "music.amazon.com", None));
    let l = parse_url("https://music.amazon.co.uk/albums/B086C6KV8F?trackAsin=B086C3XK4R&do=play").unwrap();
    assert_eq!((l.kind.as_str(), l.track.as_deref(), l.host.as_str()), ("track", Some("B086C3XK4R"), "music.amazon.co.uk"));
    assert_eq!(parse_url("https://music.amazon.de/albums/B086C6KV8F").unwrap().kind, "album");
    let l = parse_url("https://music.amazon.com/user-playlists/4c3e5f6a7b8c9d0e1f2a3b4c5d6e7f80sune?marketplaceId=ATVPDKIKX0DER").unwrap();
    assert_eq!((l.kind.as_str(), l.id.as_str()), ("user-playlist", "4c3e5f6a7b8c9d0e1f2a3b4c5d6e7f80sune"));
    let l = parse_url("https://www.amazon.com/music/player/tracks/B084ZHXQ7G").unwrap();
    assert_eq!((l.kind.as_str(), l.track.as_deref(), l.host.as_str()), ("track", Some("B084ZHXQ7G"), "music.amazon.com"));
    assert!(parse_url("https://www.amazon.com/dp/B084ZHXQ7G").is_none());
    assert_eq!(crate::sources::links::detect("https://www.amazon.com/dp/B084ZHXQ7G"), Some("web"));
    assert_eq!(crate::sources::links::detect("https://music.amazon.com/playlists/B01M11SBC8"), Some("amazon"));
    assert_eq!(crate::sources::links::detect("https://amzn.to/3xYzAbC"), Some("amazon"));
}

#[test]
fn regions_follow_the_storefront() {
    assert_eq!(api_host("music.amazon.com", ""), "https://na.mesk.skill.music.a2z.com/api");
    assert_eq!(api_host("music.amazon.de", ""), "https://eu.mesk.skill.music.a2z.com/api");
    assert_eq!(api_host("music.amazon.co.jp", ""), "https://fe.mesk.skill.music.a2z.com/api");
    assert_eq!(api_host("music.amazon.com", "EU"), "https://eu.mesk.skill.music.a2z.com/api");
}

#[test]
fn playlist_page_rows() {
    let link = parse_url("https://music.amazon.com/playlists/B01M11SBC8").unwrap();
    let (h, cells) = html_rows(PLAYLIST_HTML);
    assert_eq!(h.name, "All Hits & Classics");
    assert_eq!(h.cover.as_deref(), Some("https://m.media-amazon.com/images/I/cover.jpg"));
    assert_eq!(cells.len(), 2, "the artist row is not a song");
    let t = rows_to_tracks(&link, &h, &cells);
    assert_eq!(t[0]["title"], "Blinding Lights");
    assert_eq!(t[0]["artist"], "The Weeknd");
    assert_eq!(t[0]["album"], "After Hours");
    assert_eq!(t[0]["duration"], 200.0);
    assert_eq!(t[0]["source_id"], "B084ZHXQ7G");
    assert_eq!(t[1]["title"], "Don't Start Now");
    assert_eq!(t[1]["duration"], 183.0);
    assert_eq!(result(&h, t, &link)["name"], "All Hits & Classics");
}

#[test]
fn album_page_rows_take_the_album_artist() {
    let link = parse_url("https://music.amazon.com/albums/B086C6KV8F").unwrap();
    let (h, cells) = html_rows(ALBUM_HTML);
    let t = rows_to_tracks(&link, &h, &cells);
    assert_eq!(t.len(), 3);
    assert_eq!((t[0]["artist"].as_str(), t[0]["album"].as_str(), t[0]["duration"].as_f64()), (Some("Dua Lipa"), Some("Future Nostalgia"), Some(184.0)));
    assert_eq!(t[2]["artist"], "Dua Lipa, DaBaby");
    assert_eq!(t[0]["cover"], "https://m.media-amazon.com/fn.jpg");
    assert_eq!(result(&h, t, &link)["name"], "Future Nostalgia – Dua Lipa");
    // a song link keeps only that song
    let one = parse_url("https://music.amazon.com/albums/B086C6KV8F?trackAsin=B086C3XK4R").unwrap();
    let t = rows_to_tracks(&one, &h, &cells);
    assert_eq!(t.len(), 1);
    assert_eq!(t[0]["title"], "Don't Start Now");
}

#[test]
fn page_without_rows_falls_back_to_og_and_jsonld() {
    let html = r#"<meta property="og:title" content="Road Trip"><script type="application/ld+json">{"@context":"https://schema.org","@type":"MusicPlaylist","name":"Road Trip",
      "track":[{"@type":"MusicRecording","name":"Africa","byArtist":{"name":"TOTO"},"inAlbum":{"name":"Toto IV"},"duration":"PT4M55S","isrcCode":"usSM-19-900123","url":"https://music.amazon.com/tracks/B000V61ABC"}]}</script>"#;
    let (h, cells) = html_rows(html);
    assert_eq!(h.name, "Road Trip");
    assert!(cells.is_empty());
    let link = parse_url("https://music.amazon.com/playlists/B0ROADTRIP").unwrap();
    let (h, cells) = jsonld_rows(html).unwrap();
    let t = rows_to_tracks(&link, &h, &cells);
    assert_eq!((t[0]["title"].as_str(), t[0]["artist"].as_str(), t[0]["album"].as_str()), (Some("Africa"), Some("TOTO"), Some("Toto IV")));
    assert_eq!(t[0]["duration"], 295.0);
    assert_eq!(t[0]["isrc"], "USSM19900123");
    assert_eq!(t[0]["source_id"], "B000V61ABC");
}

#[test]
fn web_player_api_answer() {
    let d = json!({ "methods": [{ "interface": "TemplateListInterface.v1_0.CreateAndBindTemplateMethod", "template": {
        "headerText": { "text": "Chill Pop" }, "headerPrimaryText": "Amazon Music", "headerImage": "https://m.media-amazon.com/chill.jpg",
        "widgets": [{ "items": [
            { "primaryText": { "text": "Sunflower" }, "secondaryText1": "Post Malone, Swae Lee", "secondaryText2": "Spider-Man: Into the Spider-Verse", "secondaryText3": "02:38",
              "primaryLink": { "deeplink": "/albums/B07KGH6W2K?trackAsin=B07KGF7NQ6" } },
            { "primaryText": "Explicit song", "secondaryText1": "EXPLICIT", "secondaryText2": "Artist B", "secondaryText3": "Album B", "duration": "3:00",
              "primaryLink": { "deeplink": "/albums/B0X?trackAsin=B0Y" } },
            { "primaryText": "Not a song", "primaryLink": { "deeplink": "/artists/B0Z" } }
        ] }] } }] });
    let (h, cells) = api_rows(&d);
    assert_eq!(h.name, "Chill Pop");
    assert_eq!(cells.len(), 2);
    let link = parse_url("https://music.amazon.com/playlists/B07CHILL").unwrap();
    let t = rows_to_tracks(&link, &h, &cells);
    assert_eq!((t[0]["artist"].as_str(), t[0]["duration"].as_f64()), (Some("Post Malone, Swae Lee"), Some(158.0)));
    assert_eq!((t[1]["artist"].as_str(), t[1]["album"].as_str()), (Some("Artist B"), Some("Album B")));
}

#[test]
fn api_body_carries_the_token_flow() {
    let cfg = json!({ "accessToken": "", "csrf": { "token": "tok123", "ts": "1700000000000", "rnd": "42" }, "deviceId": "dev1", "sessionId": "ses1",
        "version": "1.0.9172.0", "displayLanguage": "en_GB", "siteRegion": "EU" });
    let link = parse_url("https://music.amazon.co.uk/playlists/B01M11SBC8").unwrap();
    let b = api_body(&cfg, &link, "https://music.amazon.co.uk/playlists/B01M11SBC8", 1_700_000_000_123);
    assert_eq!(b["id"], "B01M11SBC8");
    let h: Value = serde_json::from_str(b["headers"].as_str().unwrap()).unwrap();
    let csrf: Value = serde_json::from_str(h["x-amzn-csrf"].as_str().unwrap()).unwrap();
    assert_eq!((csrf["token"].as_str(), csrf["timestamp"].as_str(), csrf["rndNonce"].as_str()), (Some("tok123"), Some("1700000000000"), Some("42")));
    assert_eq!((h["x-amzn-device-id"].as_str(), h["x-amzn-session-id"].as_str(), h["x-amzn-music-domain"].as_str()), (Some("dev1"), Some("ses1"), Some("music.amazon.co.uk")));
    assert_eq!(h["x-amzn-device-language"], "en_GB");
    let auth: Value = serde_json::from_str(h["x-amzn-authentication"].as_str().unwrap()).unwrap();
    assert_eq!(auth["accessToken"], "");
    assert_eq!(api_methods("user-playlist")[0], "showSharedUserPlaylist");
}

#[test]
fn clock_times() {
    assert_eq!(clock("3:20"), Some(200.0));
    assert_eq!(clock("1:02:03"), Some(3723.0));
    assert_eq!(clock("After Hours"), None);
}

#[test]
fn numeric_entities_are_decoded() {
    assert_eq!(unescape("Don&#039;t &#233;t&#xE9; &#x2019; R&amp;B &amp;#39; &nbsp;x &bogus;"), "Don't été ’ R&B &#39;  x &bogus;");
}
