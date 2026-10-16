use super::jar::{parse_http_date, Cookie, Jar};
use super::*;
use axum::http::HeaderValue;

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
    let calls = rotate::build(&j, NOW);
    assert_eq!(calls.len(), 2);
    let y = &calls[0];
    assert_eq!(y.url, "https://accounts.youtube.com/RotateCookies");
    assert_eq!(y.body, r#"[000,"-0000000000000000000"]"#);
    let h = |k: &str| y.headers.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
    assert_eq!(h("Content-Type"), Some("application/json"));
    assert_eq!(h("Origin"), Some("https://accounts.youtube.com"));
    // youtube.com cookies to accounts.youtube.com, google.com ones to accounts.google.com
    assert!(h("Cookie").unwrap().contains("__Secure-1PSID=s1") && !h("Cookie").unwrap().contains("g1"));
    assert_eq!(calls[1].url, "https://accounts.google.com/RotateCookies");
    let gc = calls[1].headers.iter().find(|(n, _)| n == "Cookie").unwrap().1.clone();
    assert!(gc.contains("__Secure-1PSID=g1") && !gc.contains("SAPISID"));
    // only youtube cookies: one call; no session cookies: none
    let only_yt = Jar(j.0.iter().filter(|c| c.domain.contains("youtube")).cloned().collect());
    assert_eq!(rotate::build(&only_yt, NOW).len(), 1);
    assert!(rotate::build(&Jar(vec![ck(".youtube.com", "PREF", "x", 0)]), NOW).is_empty());
    // expired cookies are not sent
    assert!(rotate::build(&j, NOW + 86400 * 2).is_empty());
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
fn old_state_file_keeps_its_session_as_the_handoff() {
    let old = r#"{"managed":{"updated_at":5.0,"ok":true,"account":"Kris"},"extension":{},"pasted":{},"ext_token":"h","signed_out_notified":false}"#;
    let s: AuthState = serde_json::from_str(old).unwrap();
    assert_eq!(s.handoff.account.as_deref(), Some("Kris"));
    assert_eq!(serde_json::from_str::<Kind>("\"managed\"").unwrap(), Kind::Handoff);
    assert!(serde_json::to_string(&s).unwrap().contains("\"handoff\""));
}

#[test]
fn old_sign_in_browser_is_removed_once() {
    let dir = std::env::temp_dir().join(format!("encore-ytauth-rm-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("tools/browser/141/chrome-linux64")).unwrap();
    std::fs::write(dir.join("tools/browser/current"), "141").unwrap();
    std::fs::create_dir_all(dir.join("youtube-browser/profile/Default")).unwrap();
    std::fs::create_dir_all(dir.join("tools/cadenza")).unwrap();
    write_private(&dir.join("youtube-browser/state.json"), "{}").unwrap();
    write_private(&dir.join("youtube-browser/extension-cookies.txt"), "x").unwrap();
    let gone = remove_old_browser(&dir);
    assert_eq!(gone.len(), 2);
    assert!(!dir.join("tools/browser").exists() && !dir.join("youtube-browser/profile").exists());
    // everything else stays: other tools, the sign-in state, the extension's cookies
    assert!(dir.join("tools/cadenza").exists() && dir.join("youtube-browser/state.json").exists() && dir.join("youtube-browser/extension-cookies.txt").exists());
    assert!(remove_old_browser(&dir).is_empty(), "second start: nothing to do (and nothing logged)");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn signed_out_notice_asks_for_a_new_handoff() {
    for k in Kind::ALL {
        assert!(signed_out_message(k).contains("Do the hand-off again from the Encore extension"));
    }
}

#[test]
fn extension_manifests_are_valid() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../extension/encore-youtube-cookies");
    let m: Value = serde_json::from_str(&std::fs::read_to_string(root.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(m["manifest_version"], 3);
    assert_eq!(m["incognito"], "split", "the hand-off opens its own incognito window");
    let perms: Vec<&str> = m["permissions"].as_array().unwrap().iter().filter_map(|p| p.as_str()).collect();
    for p in ["cookies", "tabs", "storage"] {
        assert!(perms.contains(&p), "permission {p}");
    }
    for f in ["popup.html", "popup.js", "background.js"] {
        assert!(root.join(f).exists(), "{f}");
    }
    let ff = root.join("manifest.firefox.json");
    if ff.exists() {
        let m: Value = serde_json::from_str(&std::fs::read_to_string(ff).unwrap()).unwrap();
        assert_eq!(m["manifest_version"], 3);
    }
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

#[tokio::test]
async fn extension_upload_route_needs_its_token() {
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;
    let _env = crate::TEST_ENV.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join(format!("encore-ytauth-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_var("ENCORE_CONFIG_DIR", &dir);
    let st = crate::build_state().unwrap();
    let app = crate::app(st.clone());
    let body = json!({ "cookies": [
        { "domain": ".youtube.com", "hostOnly": false, "name": "SAPISID", "value": "a", "path": "/", "secure": true, "httpOnly": false, "session": false, "expirationDate": 4_000_000_000u64 },
    ], "agent": "Chrome 141" });
    let post = |tok: Option<&str>| {
        let mut b = Request::builder().method("POST").uri("/api/youtube/extension-cookies").header("content-type", "application/json");
        if let Some(t) = tok {
            b = b.header("authorization", format!("Bearer {t}"));
        }
        b.body(Body::from(body.to_string())).unwrap()
    };
    let r = app.clone().oneshot(post(Some("eyt_guess"))).await.unwrap();
    assert_eq!(r.status(), 401);
    assert_eq!(r.headers().get("access-control-allow-origin").unwrap(), "*");
    // a token made the way the route makes it
    let token = "eyt_testtoken";
    update(st.data_dir(), |s| s.ext_token = Some(token_hash(token)));
    let r = app.clone().oneshot(post(Some(token))).await.unwrap();
    assert_eq!(r.status(), 200);
    let saved = std::fs::read_to_string(super::ext_path(st.data_dir())).unwrap();
    assert!(saved.contains("SAPISID\ta"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(super::ext_path(st.data_dir())).unwrap().permissions().mode() & 0o777, 0o600);
    }
    assert_eq!(load(st.data_dir()).extension.agent.as_deref(), Some("Chrome 141"));
    // the extension's cookies are now what yt-dlp gets
    assert_eq!(active(&st), Some(Kind::Extension));
    assert!(cookies_for_ytdlp(&st).unwrap().contains("SAPISID\ta"));
    // the status route is for admins only
    let r = app.clone().oneshot(Request::builder().uri("/api/youtube/auth").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(r.status(), 401);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn account_menu_name() {
    let v = json!({ "actions": [{ "openPopupAction": { "popup": { "multiPageMenuRenderer": { "header": { "activeAccountHeaderRenderer": {
        "accountName": { "runs": [{ "text": "Kris" }] }, "channelHandle": { "runs": [{ "text": "@kris" }] } } } } } } }] });
    assert_eq!(account_name(&v).as_deref(), Some("Kris (@kris)"));
    assert_eq!(account_name(&json!({})), None);
}


#[tokio::test]
async fn handoff_becomes_the_server_owned_session_and_stays_out_of_logs() {
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;
    let _env = crate::TEST_ENV.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join(format!("encore-ytauth-ho-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_var("ENCORE_CONFIG_DIR", &dir);
    let st = crate::build_state().unwrap();
    let app = crate::app(st.clone());
    let token = "eyt_handofftoken";
    update(st.data_dir(), |s| s.ext_token = Some(token_hash(token)));
    let post = |handoff: bool, value: &str| {
        let body = json!({ "handoff": handoff, "agent": "Firefox 143", "cookies": [
            { "domain": ".youtube.com", "hostOnly": false, "name": "SAPISID", "value": value, "path": "/", "secure": true, "httpOnly": false, "session": false, "expirationDate": 4_000_000_000u64 },
            { "domain": ".youtube.com", "hostOnly": false, "name": "__Secure-3PSID", "value": format!("{value}-3p"), "path": "/", "secure": true, "httpOnly": true, "session": false, "expirationDate": 4_000_000_000u64 },
        ] });
        Request::builder().method("POST").uri("/api/youtube/extension-cookies").header("content-type", "application/json")
            .header("authorization", format!("Bearer {token}")).body(Body::from(body.to_string())).unwrap()
    };
    // the everyday browser first, then a hand-off: the hand-off wins
    let r = app.clone().oneshot(post(false, "everydaySECRET")).await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(active(&st), Some(Kind::Extension));
    let r = app.clone().oneshot(post(true, "handoffSECRET")).await.unwrap();
    assert_eq!(r.status(), 200);
    let bytes = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(!text.contains("SECRET"), "no cookie values in the answer: {text}");
    let v: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["handoff"], true);
    assert_eq!(active(&st), Some(Kind::Handoff));
    assert!(cookies_for_ytdlp(&st).unwrap().contains("handoffSECRET"));
    let s = load(st.data_dir());
    assert_eq!(s.handoff.ok, Some(true));
    assert_eq!(s.handoff.agent.as_deref(), Some("Firefox 143"));
    assert!(s.extension.updated_at.is_some(), "the everyday browser's cookies stay as the next choice");
    // the server refreshes the hand-off itself; the extension's never
    assert!(next_refresh(now(), active(&st), &s, false).is_some());
    let status = status(&st).to_string();
    assert_eq!(status_source(&status), "handoff");
    assert!(!status.contains("SECRET"), "no cookie values in the status");
    let logs = st.db.query("SELECT message FROM activity", vec![]).unwrap();
    assert!(logs.iter().any(|r| r["message"].as_str().unwrap_or("").contains("handed over")));
    assert!(!logs.iter().any(|r| r["message"].as_str().unwrap_or("").contains("SECRET")), "no cookie values in the log");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(super::handoff_path(st.data_dir())).unwrap().permissions().mode() & 0o777, 0o600);
    }
    // a window that isn't signed in yet: nothing replaced
    let empty = Request::builder().method("POST").uri("/api/youtube/extension-cookies").header("content-type", "application/json")
        .header("authorization", format!("Bearer {token}")).body(Body::from(json!({ "handoff": true, "cookies": [] }).to_string())).unwrap();
    let r = app.clone().oneshot(empty).await.unwrap();
    let v: Value = serde_json::from_slice(&axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap()).unwrap();
    assert_eq!(v["ok"], false);
    assert!(cookies_for_ytdlp(&st).unwrap().contains("handoffSECRET"));
    let _ = std::fs::remove_dir_all(&dir);
}

fn status_source(status: &str) -> String {
    serde_json::from_str::<Value>(status).unwrap()["source"].as_str().unwrap_or_default().to_string()
}
