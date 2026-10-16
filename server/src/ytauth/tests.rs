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
fn cdp_cookies_export_to_netscape() {
    let v = json!({ "cookies": [
        { "name": "SAPISID", "value": "abc", "domain": ".youtube.com", "path": "/", "expires": 1900000000.5, "httpOnly": false, "secure": true, "session": false },
        { "name": "VISITOR_INFO1_LIVE", "value": "v", "domain": ".youtube.com", "path": "/", "expires": -1, "httpOnly": true, "secure": true, "session": true },
        { "name": "other", "value": "x", "domain": ".example.com", "path": "/", "expires": 1900000000, "httpOnly": false, "secure": false, "session": false },
        { "name": "HSID", "value": "h", "domain": "accounts.google.com", "path": "/", "expires": 1900000000, "httpOnly": true, "secure": false, "session": false },
    ] });
    let j = Jar::from_cdp(&v);
    assert_eq!(j.0.len(), 3, "other sites are left out");
    let t = j.to_netscape();
    assert!(t.contains(".youtube.com\tTRUE\t/\tTRUE\t1900000000\tSAPISID\tabc\n"));
    assert!(t.contains("#HttpOnly_.youtube.com\tTRUE\t/\tTRUE\t0\tVISITOR_INFO1_LIVE\tv\n"));
    assert!(t.contains("#HttpOnly_accounts.google.com\tFALSE\t/\tFALSE\t1900000000\tHSID\th\n"));
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
fn precedence_managed_extension_pasted() {
    let good = signed_in_jar(NOW + 86400 * 30);
    let all = |m: Option<bool>, e: Option<bool>| vec![
        cand(Kind::Pasted, good.clone(), Some(300.0), None),
        cand(Kind::Extension, good.clone(), Some(200.0), e),
        cand(Kind::Managed, good.clone(), Some(100.0), m),
    ];
    assert_eq!(choose(&all(None, None), NOW), Some(Kind::Managed));
    assert_eq!(choose(&all(Some(false), None), NOW), Some(Kind::Extension), "a signed-out browser gives way");
    assert_eq!(choose(&all(Some(false), Some(false)), NOW), Some(Kind::Pasted));
    // no managed cookies at all
    let c = vec![cand(Kind::Managed, Jar::default(), None, None), cand(Kind::Extension, good.clone(), Some(1.0), None), cand(Kind::Pasted, good.clone(), Some(9.0), None)];
    assert_eq!(choose(&c, NOW), Some(Kind::Extension));
    // nothing signed in: the freshest cookies still go to yt-dlp
    let stale = signed_in_jar(NOW - 10);
    let c = vec![cand(Kind::Managed, stale.clone(), Some(100.0), None), cand(Kind::Pasted, stale.clone(), Some(500.0), None), cand(Kind::Extension, Jar::default(), Some(900.0), None)];
    assert_eq!(choose(&c, NOW), Some(Kind::Pasted));
    assert_eq!(choose(&[cand(Kind::Pasted, Jar::default(), None, None)], NOW), None);
}

#[test]
fn refresh_due_logic() {
    let now = NOW as f64;
    // never signed in: nothing to refresh
    assert!(!managed_due(now, None, None));
    assert!(!managed_due(now, Some(now - 3600.0), Some(NOW + 30 * 86400)));
    assert!(managed_due(now, Some(now - 49.0 * 3600.0), Some(NOW + 30 * 86400)), "every 48 h");
    assert!(managed_due(now, Some(now - 3600.0), Some(NOW + 2 * 86400)), "expires within 3 days");
    assert!(!managed_due(now, Some(now - 3600.0), None));
    // light refresh: only for pasted cookies in use, switch on, every 4 h
    assert!(light_due(now, None, Some(Kind::Pasted), true));
    assert!(!light_due(now, Some(now - 3600.0), Some(Kind::Pasted), true));
    assert!(light_due(now, Some(now - 4.0 * 3600.0), Some(Kind::Pasted), true));
    assert!(!light_due(now, None, Some(Kind::Extension), true), "never rotate the extension's cookies");
    assert!(!light_due(now, None, Some(Kind::Managed), true));
    assert!(!light_due(now, None, Some(Kind::Pasted), false));
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
fn live_view_input_maps_to_devtools() {
    let c = browser::input_commands(&json!({ "type": "mouse", "event": "down", "x": 10.5, "y": 20 }));
    assert_eq!(c[0].0, "Input.dispatchMouseEvent");
    assert_eq!(c[0].1["type"], "mousePressed");
    assert_eq!(c[0].1["button"], "left");
    let c = browser::input_commands(&json!({ "type": "key", "key": "Enter" }));
    assert_eq!(c.len(), 2);
    assert_eq!(c[0].1["windowsVirtualKeyCode"], 13);
    assert_eq!(c[0].1["text"], "\r");
    let c = browser::input_commands(&json!({ "type": "text", "text": "me@example.com" }));
    assert_eq!(c[0].0, "Input.insertText");
    assert!(browser::input_commands(&json!({ "type": "nope" })).is_empty());
}

#[test]
fn browser_command_line() {
    let a = browser::args(Path::new("/config/youtube-browser/profile"), &browser::user_agent("141.0.7390.54"), false);
    assert!(a.contains(&"--headless=new".to_string()));
    // DevTools over a pipe, never a TCP port
    assert!(a.contains(&"--remote-debugging-pipe".to_string()));
    assert!(!a.iter().any(|x| x.starts_with("--remote-debugging-port") || x.starts_with("--remote-debugging-address")));
    assert!(a.contains(&"--user-data-dir=/config/youtube-browser/profile".to_string()));
    assert!(a.iter().any(|x| x.starts_with("--user-agent=") && x.contains("Chrome/141.0.0.0") && !x.contains("Headless")));
    assert!(!a.iter().any(|x| x.contains("enable-automation")));
    // a PUID container has no usable Chrome sandbox: always off, root or not
    assert!(a.contains(&"--no-sandbox".to_string()));
    assert!(browser::args(Path::new("/p"), "ua", true).contains(&"--no-sandbox".to_string()));
}

#[test]
fn start_failure_names_the_reason_not_the_stack_trace() {
    let lines: Vec<String> = ["[1016/081200.1:FATAL:zygote_host_impl_linux.cc(127)] No usable sandbox! ...", "#0 0x55 base::debug::CollectStackTrace()", "[end of stack trace]"]
        .iter().map(|s| s.to_string()).collect();
    assert!(browser::start_failure(&lines).unwrap().contains("No usable sandbox"));
    let lines: Vec<String> = ["chrome: error while loading shared libraries: libnss3.so: cannot open shared object file"].iter().map(|s| s.to_string()).collect();
    assert!(browser::start_failure(&lines).unwrap().contains("libnss3"));
    assert_eq!(browser::start_failure(&["[end of stack trace]".to_string()]), None);
}

#[test]
fn download_listings() {
    let cft = json!({ "channels": { "Stable": { "version": "141.0.7390.54", "downloads": { "chrome": [
        { "platform": "mac-arm64", "url": "https://x/mac.zip" }, { "platform": "linux64", "url": "https://x/chrome-linux64.zip" } ] } } } });
    assert_eq!(browser::parse_cft(&cft), Some(("141.0.7390.54".into(), "https://x/chrome-linux64.zip".into())));
    let pw = json!({ "browsers": [{ "name": "chromium", "revision": "1193", "browserVersion": "141.0.7390.37" }, { "name": "firefox", "revision": "1490" }] });
    assert_eq!(browser::parse_playwright(&pw), Some(("141.0.7390.37".into(), "1193".into())));
}

#[test]
fn account_menu_name() {
    let v = json!({ "actions": [{ "openPopupAction": { "popup": { "multiPageMenuRenderer": { "header": { "activeAccountHeaderRenderer": {
        "accountName": { "runs": [{ "text": "Kris" }] }, "channelHandle": { "runs": [{ "text": "@kris" }] } } } } } } }] });
    assert_eq!(account_name(&v).as_deref(), Some("Kris (@kris)"));
    assert_eq!(account_name(&json!({})), None);
}

#[test]
fn live_view_only_from_encores_own_pages() {
    use axum::http::HeaderMap;
    let h = |pairs: &[(&'static str, &str)]| {
        let mut m = HeaderMap::new();
        for (k, v) in pairs {
            m.insert(*k, v.parse().unwrap());
        }
        m
    };
    assert!(same_origin(&h(&[("host", "music.example.com"), ("origin", "https://music.example.com")]), ""));
    assert!(same_origin(&h(&[("host", "localhost:8080"), ("origin", "http://localhost:8080")]), ""));
    assert!(same_origin(&h(&[("host", "127.0.0.1:8080"), ("x-forwarded-host", "music.example.com"), ("origin", "https://music.example.com")]), ""));
    assert!(same_origin(&h(&[("host", "internal:8080"), ("origin", "https://music.example.com")]), "https://music.example.com/"));
    assert!(!same_origin(&h(&[("host", "music.example.com"), ("origin", "https://other.example.com")]), ""));
    assert!(!same_origin(&h(&[("host", "music.example.com"), ("origin", "null")]), ""));
    // no Origin: not a page in a browser
    assert!(same_origin(&h(&[("host", "music.example.com")]), ""));
}
