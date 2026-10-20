use super::jar::{parse_http_date, Cookie, Jar};
use super::*;
use reqwest::header::HeaderValue;

const NOW: i64 = 1_800_000_000;

fn ck(domain: &str, name: &str, value: &str, expires: i64) -> Cookie {
    Cookie { domain: domain.into(), path: "/".into(), secure: true, http_only: false, expires, name: name.into(), value: value.into() }
}

fn signed_in_jar(expires: i64) -> Jar {
    Jar(vec![ck(".youtube.com", "SAPISID", "abc", expires), ck(".youtube.com", "__Secure-3PSID", "s3", expires), ck(".youtube.com", "__Secure-1PSID", "s1", expires),
        ck(".google.com", "__Secure-1PSID", "g1", expires), ck(".google.com", "SID", "gsid", expires)])
}

#[test]
fn netscape_round_trip() {
    let text = "# Netscape HTTP Cookie File\n\
        .youtube.com\tTRUE\t/\tTRUE\t1900000000\tSAPISID\tabc/def\n\
        #HttpOnly_.youtube.com\tTRUE\t/\tTRUE\t1900000000\t__Secure-3PSID\txyz\n\
        music.youtube.com\tFALSE\t/\tFALSE\t0\tPREF\tf6=80\r\n\
        broken line\n";
    let j = Jar::parse_netscape(text);
    assert_eq!(j.0.len(), 3);
    assert!(j.0[1].http_only && !j.0[0].http_only);
    assert_eq!(j.0[2].value, "f6=80");
    let out = j.to_netscape();
    assert!(out.starts_with("# Netscape HTTP Cookie File\n"));
    assert!(out.contains("#HttpOnly_.youtube.com\tTRUE\t/\tTRUE\t1900000000\t__Secure-3PSID\txyz\n"));
    assert!(out.contains("music.youtube.com\tFALSE\t/\tFALSE\t0\tPREF\tf6=80\n"));
    assert_eq!(Jar::parse_netscape(&out), j);
}

#[test]
fn extension_cookies_keep_host_only_and_domains() {
    let list = vec![
        json!({ "domain": ".youtube.com", "hostOnly": false, "name": "SAPISID", "value": "a", "path": "/", "secure": true, "httpOnly": false, "session": false, "expirationDate": 1900000000.2 }),
        json!({ "domain": "music.youtube.com", "hostOnly": true, "name": "PREF", "value": "p", "path": "/", "secure": false, "httpOnly": false, "session": true }),
        json!({ "domain": "youtube.com", "hostOnly": false, "name": "SID", "value": "s", "path": "/", "secure": false, "httpOnly": false, "session": false, "expirationDate": 1900000000 }),
        json!({ "domain": ".evil.com", "hostOnly": false, "name": "SAPISID", "value": "x", "path": "/" }),
    ];
    let j = Jar::from_extension(&list);
    assert_eq!(j.0.len(), 3);
    assert_eq!(j.0[1].domain, "music.youtube.com");
    assert_eq!(j.0[1].expires, 0);
    assert_eq!(j.0[2].domain, ".youtube.com");
    assert!(j.has_auth(NOW));
    assert_eq!(j.header_for("www.youtube.com", NOW), "SAPISID=a; SID=s");
    assert_eq!(j.header_for("music.youtube.com", NOW), "SAPISID=a; PREF=p; SID=s");
}

#[test]
fn expiry_and_sign_in() {
    let j = signed_in_jar(NOW + 86400);
    assert!(j.has_auth(NOW));
    assert_eq!(j.auth_expiry(NOW), Some(NOW + 86400));
    assert!(!j.has_auth(NOW + 86401), "expired cookies don't count");
    assert!(!Jar(vec![ck(".youtube.com", "PREF", "x", 0)]).has_auth(NOW));
}

#[test]
fn set_cookie_merges_and_deletes() {
    let mut j = signed_in_jar(NOW + 1000);
    j.0.push(ck(".youtube.com", "__Secure-1PSIDTS", "old", NOW + 1000));
    j.apply_set_cookie("__Secure-1PSIDTS=new; Domain=.youtube.com; Path=/; Expires=Thu, 13-Jan-2028 22:23:01 GMT; Secure; HttpOnly", "accounts.youtube.com", NOW);
    let c = j.get("music.youtube.com", "__Secure-1PSIDTS").unwrap();
    assert_eq!(c.value, "new");
    assert!(c.http_only);
    assert_eq!(c.expires, parse_http_date("Thu, 13 Jan 2028 22:23:01 GMT").unwrap());
    j.apply_set_cookie("__Secure-3PSIDTS=fresh; Max-Age=3600; Domain=youtube.com; Path=/; Secure", "accounts.youtube.com", NOW);
    assert_eq!(j.get("www.youtube.com", "__Secure-3PSIDTS").unwrap().expires, NOW + 3600);
    // no Domain: host-only for the answering host
    j.apply_set_cookie("ACCOUNT_CHOOSER=z; Path=/", "accounts.google.com", NOW);
    assert_eq!(j.get("accounts.google.com", "ACCOUNT_CHOOSER").unwrap().domain, "accounts.google.com");
    assert!(j.get("www.google.com", "ACCOUNT_CHOOSER").is_none());
    let n = j.0.len();
    j.apply_set_cookie("SAPISID=; Domain=.youtube.com; Path=/; Max-Age=0", "accounts.youtube.com", NOW);
    assert_eq!(j.0.len(), n - 1);
    assert!(j.get("music.youtube.com", "SAPISID").is_none());
}

#[test]
fn http_dates() {
    assert_eq!(parse_http_date("Thu, 01 Jan 1970 00:00:00 GMT"), Some(0));
    assert_eq!(parse_http_date("Sun, 06 Nov 1994 08:49:37 GMT"), Some(784111777));
    assert_eq!(parse_http_date("Sunday, 06-Nov-94 08:49:37 GMT"), Some(784111777));
    assert_eq!(parse_http_date("nonsense"), None);
}

#[test]
fn rotation_request_building() {
    let j = signed_in_jar(NOW + 86400);
    let ff = "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:143.0) Gecko/20100101 Firefox/143.0";
    let calls = rotate::build(&j, NOW, ff);
    assert_eq!(calls.len(), 2);
    let y = &calls[0];
    assert_eq!(y.url, "https://accounts.youtube.com/RotateCookies");
    assert_eq!(y.body, r#"[000,"-0000000000000000000"]"#);
    let h = |k: &str| y.headers.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
    assert_eq!(h("Content-Type"), Some("application/json"));
    assert_eq!(h("Origin"), Some("https://accounts.youtube.com"));
    assert_eq!(h("User-Agent"), Some(ff), "the rotation presents the browser the session came from");
    // youtube.com cookies to accounts.youtube.com, google.com ones to accounts.google.com
    assert!(h("Cookie").unwrap().contains("__Secure-1PSID=s1") && !h("Cookie").unwrap().contains("g1"));
    assert_eq!(calls[1].url, "https://accounts.google.com/RotateCookies");
    let gc = calls[1].headers.iter().find(|(n, _)| n == "Cookie").unwrap().1.clone();
    assert!(gc.contains("__Secure-1PSID=g1") && !gc.contains("SAPISID"));
    // only youtube cookies: one call; no session cookies: none
    let only_yt = Jar(j.0.iter().filter(|c| c.domain.contains("youtube")).cloned().collect());
    assert_eq!(rotate::build(&only_yt, NOW, rotate::UA).len(), 1);
    assert!(rotate::build(&Jar(vec![ck(".youtube.com", "PREF", "x", 0)]), NOW, rotate::UA).is_empty());
    // expired cookies are not sent
    assert!(rotate::build(&j, NOW + 86400 * 2, rotate::UA).is_empty());
}

#[test]
fn user_agent_follows_the_browser_of_the_handoff() {
    let brave = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36";
    // the full string the extension reported wins
    assert_eq!(rotate::user_agent(Some(brave), Some("Chrome 141")), brave);
    assert_eq!(rotate::user_agent(Some(&format!("  {brave}\n")), None), brave, "trimmed");
    // an older extension sent only the short name: a matching browser is built from it
    assert!(rotate::user_agent(None, Some("Firefox 143")).contains("Firefox/143.0"));
    assert!(rotate::user_agent(None, Some("Chrome 141")).contains("Chrome/141.0.0.0"));
    let edge = rotate::user_agent(None, Some("Edge 141"));
    assert!(edge.contains("Edg/141.0.0.0") && edge.contains("Chrome/141.0.0.0"));
    // nothing usable: the default
    assert_eq!(rotate::user_agent(None, None), rotate::UA);
    assert_eq!(rotate::user_agent(Some("curl/8.0"), Some("Browser")), rotate::UA, "not a browser's string");
    assert_eq!(rotate::user_agent(Some("Mozilla/5.0 \u{7}evil"), Some("Chrome x")), rotate::UA, "control characters and no version");
}

#[test]
fn a_refresh_that_renewed_nothing_is_not_called_a_refresh() {
    let mut s = SourceState { updated_at: Some(100.0), ..Default::default() };
    // Google answered 200 but sent no cookies: checked, not refreshed – "last refresh" stays
    note_refresh(&mut s, 200.0, true, Some(0), None, None);
    assert_eq!((s.updated_at, s.checked_at, s.ok, s.rotated), (Some(100.0), Some(200.0), Some(true), Some(0)));
    assert!(s.message.is_none(), "not a failure: the next refresh is due in 4 h as usual");
    assert!(refresh_note(&s).unwrap().contains("renewed no cookies"));
    // cookies came back: a real refresh
    note_refresh(&mut s, 300.0, true, Some(2), None, Some("Kris".into()));
    assert_eq!((s.updated_at, s.rotated), (Some(300.0), Some(2)));
    assert_eq!(s.account.as_deref(), Some("Kris"));
    assert!(refresh_note(&s).is_none());
    // only checked on purpose (pasted cookies with the light refresh off): as before
    note_refresh(&mut s, 400.0, true, None, None, None);
    assert_eq!((s.updated_at, s.rotated), (Some(400.0), None));
    // signed out: nothing bumped, the message stands
    note_refresh(&mut s, 500.0, false, Some(0), Some("Signed out".into()), None);
    assert_eq!((s.updated_at, s.ok, s.message.as_deref()), (Some(400.0), Some(false), Some("Signed out")));
    assert!(refresh_note(&s).is_none(), "a signed-out source has its own message");
    // the rotation call itself failed but the sign-in holds: nothing is known about the cookies (not
    // "Google sent none back"), "last refresh" stays, and the message alone says what happened
    let failed = rotate::Outcome::Failed("HTTP 503".into());
    assert_eq!(renewed(&failed, true), None);
    assert_eq!((renewed(&rotate::Outcome::Rotated(0), true), renewed(&rotate::Outcome::Rotated(3), true), renewed(&rotate::Outcome::Rotated(0), false)), (Some(0), Some(3), None));
    note_refresh(&mut s, 600.0, true, renewed(&failed, true), Some("Couldn't renew the cookies (HTTP 503) – still signed in, trying again in 30 min".into()), None);
    assert_eq!((s.updated_at, s.checked_at, s.ok, s.rotated), (Some(400.0), Some(600.0), Some(true), None));
    assert!(refresh_note(&s).is_none(), "the message explains; no second note about Google sending no cookies");
    assert!(s.message.as_deref().unwrap().contains("Couldn't renew"));
}

#[test]
fn one_wording_for_the_extension_source() {
    // the server's label is the words both the web card (web-rs/src/pages/youtube.rs) and the frozen JS UI show
    assert_eq!(Kind::Extension.label(), "Everyday browser (extension)");
    assert_eq!(Kind::Handoff.label(), "Handed-over sign-in");
    assert_eq!(Kind::Pasted.label(), "Pasted cookies");
    if let Some(root) = repo_root() {
        let card = std::fs::read_to_string(root.join("web-rs/src/pages/youtube.rs")).unwrap();
        assert!(card.contains("\"Everyday browser (extension)\""), "the web card uses the same words");
        assert!(!card.contains("Browser extension (everyday browser)"));
    }
}

fn cand(kind: Kind, jar: Jar, updated: Option<f64>, ok: Option<bool>) -> Candidate {
    Candidate { kind, jar, state: SourceState { updated_at: updated, ok, ..Default::default() } }
}

#[test]
fn precedence_handoff_extension_pasted() {
    let good = signed_in_jar(NOW + 86400 * 30);
    let all = |m: Option<bool>, e: Option<bool>| vec![
        cand(Kind::Pasted, good.clone(), Some(300.0), None),
        cand(Kind::Extension, good.clone(), Some(200.0), e),
        cand(Kind::Handoff, good.clone(), Some(100.0), m),
    ];
    assert_eq!(choose(&all(None, None), NOW), Some(Kind::Handoff), "the server-owned session wins, even when older");
    assert_eq!(choose(&all(Some(true), Some(true)), NOW), Some(Kind::Handoff));
    assert_eq!(choose(&all(Some(false), None), NOW), Some(Kind::Extension), "a signed-out hand-off gives way");
    assert_eq!(choose(&all(Some(false), Some(false)), NOW), Some(Kind::Pasted));
    // no hand-off at all
    let c = vec![cand(Kind::Handoff, Jar::default(), None, None), cand(Kind::Extension, good.clone(), Some(1.0), None), cand(Kind::Pasted, good.clone(), Some(9.0), None)];
    assert_eq!(choose(&c, NOW), Some(Kind::Extension));
    // nothing signed in: the freshest cookies still go to yt-dlp
    let stale = signed_in_jar(NOW - 10);
    let c = vec![cand(Kind::Handoff, stale.clone(), Some(100.0), None), cand(Kind::Pasted, stale.clone(), Some(500.0), None), cand(Kind::Extension, Jar::default(), Some(900.0), None)];
    assert_eq!(choose(&c, NOW), Some(Kind::Pasted));
    assert_eq!(choose(&[cand(Kind::Pasted, Jar::default(), None, None)], NOW), None);
}

#[test]
fn refresh_due_logic() {
    let now = NOW as f64;
    // hand-off: every 4 h while there are cookies, 30 min after a refresh that couldn't reach YouTube
    assert!(handoff_due(now, true, None, false), "a fresh hand-off is checked on the next tick");
    assert!(!handoff_due(now, false, None, false), "nothing handed over: nothing to refresh");
    assert!(!handoff_due(now, true, Some(now - 3600.0), false));
    assert!(handoff_due(now, true, Some(now - 4.0 * 3600.0), false), "every 4 h");
    assert!(!handoff_due(now, true, Some(now - 600.0), true));
    assert!(handoff_due(now, true, Some(now - 1800.0), true), "retried 30 min after a failed refresh");
    // light refresh: only for pasted cookies in use, switch on, every 4 h
    assert!(light_due(now, None, Some(Kind::Pasted), true));
    assert!(!light_due(now, Some(now - 3600.0), Some(Kind::Pasted), true));
    assert!(light_due(now, Some(now - 4.0 * 3600.0), Some(Kind::Pasted), true));
    assert!(!light_due(now, None, Some(Kind::Extension), true), "never rotate the extension's cookies");
    assert!(!light_due(now, None, Some(Kind::Handoff), true), "the hand-off has its own refresh");
    assert!(!light_due(now, None, Some(Kind::Pasted), false));
}

#[test]
fn next_refresh_follows_the_source_in_use() {
    let now = NOW as f64;
    let mut s = AuthState::default();
    s.handoff.checked_at = Some(now - 3600.0);
    assert_eq!(next_refresh(now, Some(Kind::Handoff), &s, false), Some(now + 3.0 * 3600.0));
    s.handoff.message = Some("Couldn't reach YouTube".into());
    assert_eq!(next_refresh(now, Some(Kind::Handoff), &s, false), Some(now), "a failed one is due again (30 min after it)");
    assert_eq!(next_refresh(now, Some(Kind::Extension), &s, true), None, "Encore never refreshes the everyday browser");
    assert_eq!(next_refresh(now, Some(Kind::Pasted), &s, false), None, "light refresh off");
    assert_eq!(next_refresh(now, Some(Kind::Pasted), &s, true), Some(now), "pasted, never checked: now");
    assert_eq!(next_refresh(now, None, &s, true), None);
}

#[test]
fn signed_out_notice_asks_for_a_new_handoff() {
    for k in Kind::ALL {
        assert!(signed_out_message(k).contains("Do the hand-off again from the Encore extension"));
    }
}

/// The repository root: the folder holding `extension/encore-youtube-cookies`, found by walking up from the
/// server crate (so a workspace layout or a moved checkout works too). None: the server crate is tested on
/// its own, without the rest of the repository.
fn repo_root() -> Option<PathBuf> {
    let mut d = Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf();
    for _ in 0..4 {
        if d.join("extension/encore-youtube-cookies/manifest.json").is_file() {
            return Some(d);
        }
        d = d.parent()?.to_path_buf();
    }
    None
}

#[test]
fn extension_manifests_are_valid() {
    let Some(repo) = repo_root() else {
        eprintln!("skipped: no extension/encore-youtube-cookies above {} (server crate tested on its own)", env!("CARGO_MANIFEST_DIR"));
        return;
    };
    let root = repo.join("extension/encore-youtube-cookies");
    let read = |f: &str| std::fs::read_to_string(root.join(f)).unwrap_or_else(|e| panic!("{f}: {e}"));
    let m: Value = serde_json::from_str(&read("manifest.json")).unwrap();
    assert_eq!(m["manifest_version"], 3);
    assert_eq!(m["incognito"], "split", "the hand-off opens its own incognito window");
    assert_eq!(m["version"], "0.0.0", "scripts/build-extension.sh stamps the release version over this placeholder");
    let perms: Vec<&str> = m["permissions"].as_array().unwrap().iter().filter_map(|p| p.as_str()).collect();
    for p in ["cookies", "tabs", "storage", "alarms"] {
        assert!(perms.contains(&p), "permission {p}");
    }
    assert_eq!(m["background"]["service_worker"], "background.js");
    for f in ["popup.html", "popup.js", "options.html", "options.js", "handoff.html", "handoff.js", "background.js", "shared.js", "style.css", "icon.png", "README.md"] {
        assert!(root.join(f).is_file(), "{f}");
    }
    // every script and stylesheet a page loads is there
    for page in ["popup.html", "options.html", "handoff.html"] {
        let html = read(page);
        for cap in html.split("src=\"").skip(1).chain(html.split("href=\"").skip(1)) {
            let f = cap.split('"').next().unwrap();
            assert!(root.join(f).is_file(), "{page} loads {f}");
        }
    }
    let ff: Value = serde_json::from_str(&read("manifest.firefox.json")).unwrap();
    assert_eq!(ff["manifest_version"], 3);
    assert_eq!(ff["version"], "0.0.0");
    assert_eq!(ff["background"]["scripts"], json!(["shared.js", "background.js"]), "Firefox has no importScripts: shared.js first");
    assert!(ff["incognito"].is_null(), "Firefox: spanning, so the hand-off page can watch the private window");
    assert_eq!(ff["name"], m["name"]);
    // the extension posts the browser it runs in: the server presents it when it refreshes the hand-off
    let shared = read("shared.js");
    assert!(shared.contains("user_agent: navigator.userAgent"), "shared.js reports navigator.userAgent");
    assert!(shared.contains("/api/youtube/extension-cookies"), "the fixed upload route");
    // one zip name everywhere the owner reads about it: the build script, the release notes template, the READMEs and the web card
    let zip = "Encore-youtube-extension-";
    for f in ["scripts/build-extension.sh", ".github/workflows/release.yml", "README.md", "extension/encore-youtube-cookies/README.md", "web-rs/src/pages/youtube.rs"] {
        let text = std::fs::read_to_string(repo.join(f)).unwrap_or_else(|e| panic!("{f}: {e}"));
        assert!(text.contains(zip), "{f} names the zip {zip}<version>.zip");
    }
    // and one path to the card in Settings
    let ext_readme = read("README.md");
    for f in ["README.md", "extension/encore-youtube-cookies/README.md"] {
        assert!(std::fs::read_to_string(repo.join(f)).unwrap().contains("Settings → Downloads → YouTube sign-in"), "{f} points at Settings → Downloads → YouTube sign-in");
    }
    assert!(ext_readme.contains("Connect the browser extension") && ext_readme.contains("Hand over a YouTube sign-in"));
}

#[test]
fn extension_token_auth() {
    let token = "eyt_0123456789abcdef";
    let stored = token_hash(token);
    let mut h = HeaderMap::new();
    assert!(!token_ok(&h, Some(&stored)), "no token");
    h.insert("authorization", HeaderValue::from_str(&format!("Bearer {token}")).unwrap());
    assert!(token_ok(&h, Some(&stored)));
    assert!(!token_ok(&h, None), "no token made yet");
    assert!(!token_ok(&h, Some(&token_hash("eyt_other"))));
    let mut h = HeaderMap::new();
    h.insert("x-encore-token", HeaderValue::from_static("eyt_0123456789abcdef"));
    assert!(token_ok(&h, Some(&stored)));
    let mut h = HeaderMap::new();
    h.insert("authorization", HeaderValue::from_static("Bearer "));
    assert!(!token_ok(&h, Some(&token_hash(""))), "an empty token never matches");
}

#[test]
fn account_menu_name() {
    let v = json!({ "actions": [{ "openPopupAction": { "popup": { "multiPageMenuRenderer": { "header": { "activeAccountHeaderRenderer": {
        "accountName": { "runs": [{ "text": "Kris" }] }, "channelHandle": { "runs": [{ "text": "@kris" }] } } } } } } }] });
    assert_eq!(account_name(&v).as_deref(), Some("Kris (@kris)"));
    assert_eq!(account_name(&json!({})), None);
}
