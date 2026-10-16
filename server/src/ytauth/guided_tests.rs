//! Guided sign-in: page detection (snapshots, and the HTML fixtures read by a real Chromium), the step
//! state machine against a small fake of Google's sign-in, and live-view typing through DevTools.
//! The browser tests run when a Chromium is found (ENCORE_TEST_CHROME, or Playwright's under
//! /opt/pw-browsers) and are skipped otherwise.
use super::browser::{self, Browser};
use super::guided::{self, classify, Detected, Snapshot, Step};
use serde_json::json;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

const HOME: &str = "http://127.0.0.1";

fn snap(v: serde_json::Value) -> Snapshot {
    serde_json::from_value(v).unwrap()
}

fn step_of(d: Detected) -> Step {
    match d {
        Detected::Step(s) => s,
        d => panic!("expected a step, got {d:?}"),
    }
}

#[test]
fn snapshots_classify() {
    let base = json!({ "url": "https://accounts.google.com/v3/signin/identifier", "ready": true });
    let with = |extra: serde_json::Value| {
        let mut b = base.clone();
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        snap(b)
    };
    let s = step_of(classify(&with(json!({ "email": true, "heading": "Sign in", "subtext": "to continue to YouTube Music" })), guided::HOME));
    assert_eq!((s.kind.as_str(), s.title.as_str(), s.text.as_str()), ("email", "Sign in", "to continue to YouTube Music"));
    // a hidden e-mail field stays behind the password field: the password wins
    let s = step_of(classify(&with(json!({ "email": true, "password": true, "error": "Wrong password. Try again." })), guided::HOME));
    assert_eq!(s.kind, "password");
    assert_eq!(s.error, "Wrong password. Try again.");
    let s = step_of(classify(&with(json!({ "code": { "name": "totpPin", "label": "Enter code" }, "another": true })), guided::HOME));
    assert_eq!((s.kind.as_str(), s.code_hint.as_str()), ("code", "totp"));
    assert_eq!(s.choices[0].id, "another");
    let s = step_of(classify(&with(json!({ "code": { "name": "idvPin" }, "subtext": "A text message with a code was sent" })), guided::HOME));
    assert_eq!(s.code_hint, "sms");
    let s = step_of(classify(&with(json!({ "code": { "name": "backupCodePin" } })), guided::HOME));
    assert_eq!(s.code_hint, "backup");
    // the authenticator app is offered first
    let s = step_of(classify(&with(json!({ "choices": ["Use your security key", "Get a verification code from the Google Authenticator app", "Get a code at ••12"],
        "choice_types": ["39", "6", "9"] })), guided::HOME));
    assert_eq!(s.kind, "choice");
    assert_eq!((s.choices[0].id.as_str(), s.choices[0].default), ("c1", true));
    assert_eq!(s.choices.iter().filter(|c| c.default).count(), 1);
    assert_eq!(s.choices[1].label, "Use your security key");
    // security key / passkey: Encore presses "Try another way" itself
    assert_eq!(classify(&with(json!({ "passkey": true, "another": true })), guided::HOME), Detected::TryAnother);
    // without the button there's nothing to press: show the page
    assert_eq!(step_of(classify(&with(json!({ "passkey": true })), guided::HOME)).kind, "unknown");
    let s = step_of(classify(&with(json!({ "tap_yes": true, "another": true, "subtext": "Check your phone" })), guided::HOME));
    assert_eq!((s.kind.as_str(), s.text.as_str(), s.choices[0].label.as_str()), ("wait", "Check your phone", "Try another way"));
    // a captcha beats the e-mail field it sits next to
    assert_eq!(step_of(classify(&with(json!({ "captcha": true, "email": true })), guided::HOME)).kind, "unknown");
    let s = step_of(classify(&with(json!({ "heading": "Verify it's you", "buttons": ["Continue", "Cancel"] })), guided::HOME));
    assert_eq!((s.kind.as_str(), s.title.as_str()), ("unknown", "Verify it's you"));
    assert_eq!(s.choices.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["b0", "b1"]);
    assert_eq!(classify(&snap(json!({ "url": "https://music.youtube.com/" })), guided::HOME), Detected::Home);
    assert_eq!(classify(&snap(json!({ "url": "https://www.youtube.com/" })), guided::HOME), Detected::Home);
    assert_ne!(classify(&snap(json!({ "url": "https://accounts.google.com/?continue=https://music.youtube.com/" })), guided::HOME), Detected::Home);
}

#[test]
fn submissions_are_checked_before_they_reach_google() {
    let mut cur = Step::new("code", "2-Step Verification");
    cur.code_hint = "totp".into();
    assert!(guided::check_submit(&cur, "code", "123 456").is_ok());
    assert!(guided::check_submit(&cur, "code", "12a456").is_err());
    assert!(guided::check_submit(&cur, "code", " ").is_err());
    assert!(guided::check_submit(&cur, "password", "secret").is_err(), "a password for a code step");
    assert!(guided::check_submit(&cur, "check", "").is_ok());
    assert!(guided::check_submit(&cur, "choice", "c0").is_err());
    cur.choices.push(guided::Choice { id: "another".into(), label: "Try another way".into(), default: false });
    assert!(guided::check_submit(&cur, "choice", "another").is_ok());
    assert!(guided::check_submit(&cur, "nope", "x").is_err());
}

#[test]
fn secrets_stay_out_of_debug_and_answers() {
    let b: guided::Submit = serde_json::from_value(json!({ "kind": "password", "value": "hunter2-secret" })).unwrap();
    assert!(!format!("{b:?}").contains("hunter2"));
    // a step answer carries what Google's page says, never what was typed
    let s = step_of(classify(&snap(json!({ "url": "https://accounts.google.com/x", "password": true, "error": "Wrong password" })), guided::HOME));
    assert!(!serde_json::to_string(&s).unwrap().contains("hunter2"));
    // an answer has the agreed shape
    let v = serde_json::to_value(&s).unwrap();
    for k in ["kind", "title", "text", "error", "choices", "code_hint"] {
        assert!(v.get(k).is_some(), "{k}");
    }
}

#[tokio::test]
async fn guided_routes_are_admin_only_and_need_a_session() {
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;
    let _env = crate::TEST_ENV.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join(format!("encore-guided-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_var("ENCORE_CONFIG_DIR", &dir);
    let st = crate::build_state().unwrap();
    let app = crate::app(st.clone());
    for path in ["/api/youtube/guided/start", "/api/youtube/guided/step", "/api/youtube/guided/cancel"] {
        let r = app.clone().oneshot(Request::builder().method("POST").uri(path).header("content-type", "application/json")
            .body(Body::from(json!({ "kind": "password", "value": "hunter2-secret" }).to_string())).unwrap()).await.unwrap();
        assert!(r.status() == 401 || r.status() == 403, "{path}: {}", r.status());
        let body = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
        assert!(!String::from_utf8_lossy(&body).contains("hunter2"));
    }
    // with no session a step is refused, and nothing typed is echoed or logged
    let e = guided::step(&st, "password", "hunter2-secret").await.unwrap_err();
    assert_eq!(e.0, 409);
    assert!(!e.1.contains("hunter2"));
    let logs = st.db.query("SELECT message FROM activity", vec![]).unwrap();
    assert!(!serde_json::to_string(&logs).unwrap().contains("hunter2"));
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- with a real browser ------------------------------------------------------------------------

fn chrome() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("ENCORE_TEST_CHROME") {
        return Some(PathBuf::from(p)).filter(|p| p.exists());
    }
    let mut found: Vec<PathBuf> = std::fs::read_dir("/opt/pw-browsers").ok()?.flatten()
        .map(|e| e.path().join("chrome-linux").join("chrome"))
        .filter(|p| p.exists() && p.to_string_lossy().contains("/chromium-")).collect();
    found.sort();
    found.pop()
}

fn fixture(name: &str) -> Option<&'static str> {
    Some(match name {
        "identifier" => include_str!("fixtures/identifier.html"),
        "password" => include_str!("fixtures/password.html"),
        "wrong-password" => include_str!("fixtures/wrong-password.html"),
        "security-key" => include_str!("fixtures/security-key.html"),
        "challenges" => include_str!("fixtures/challenges.html"),
        "totp" => include_str!("fixtures/totp.html"),
        "wrong-code" => include_str!("fixtures/wrong-code.html"),
        "sms" => include_str!("fixtures/sms.html"),
        "tap-yes" => include_str!("fixtures/tap-yes.html"),
        "captcha" => include_str!("fixtures/captcha.html"),
        "unknown" => include_str!("fixtures/unknown.html"),
        "chooser" => include_str!("fixtures/chooser.html"),
        "music" => include_str!("fixtures/music.html"),
        "typing" => include_str!("fixtures/typing.html"),
        _ => return None,
    })
}

/// Everything the fake Google was sent (to check what Encore typed, and where).
type Seen = Arc<Mutex<Vec<String>>>;

/// A tiny HTTP server playing Google's sign-in: /f/<fixture> serves a page; the forms post to
/// /pwd, /pwdcheck (password "right-pass") and /totpcheck (code 123456); the short paths below map
/// to fixtures the way Google's buttons lead.
async fn fake_google() -> (String, Seen) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", l.local_addr().unwrap());
    let seen: Seen = Default::default();
    let seen2 = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut c, _)) = l.accept().await else { return };
            let seen = seen2.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 4096];
                let head_end = loop {
                    let Ok(n) = c.read(&mut tmp).await else { return };
                    if n == 0 { return; }
                    buf.extend_from_slice(&tmp[..n]);
                    if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") { break i + 4; }
                };
                let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
                let len = head.lines().find_map(|l| l.to_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0))).unwrap_or(0);
                while buf.len() < head_end + len {
                    let Ok(n) = c.read(&mut tmp).await else { return };
                    if n == 0 { break; }
                    buf.extend_from_slice(&tmp[..n]);
                }
                let body = String::from_utf8_lossy(&buf[head_end..]).to_string();
                let path = head.split_whitespace().nth(1).unwrap_or("/").split('?').next().unwrap_or("/").to_string();
                seen.lock().unwrap().push(format!("{path} {body}"));
                let form = |k: &str| body.split('&').find_map(|kv| kv.strip_prefix(&format!("{k}="))).map(|v| urlencoding::decode(&v.replace('+', " ")).map(|c| c.into_owned()).unwrap_or_default()).unwrap_or_default();
                let page = match path.as_str() {
                    "/signin" => "identifier",
                    "/pwd" => "password",
                    "/pwdcheck" => if form("Passwd") == "right-pass" { "security-key" } else { "wrong-password" },
                    "/key" => "security-key",
                    "/challenges" => "challenges",
                    "/totp" => "totp",
                    "/sms" => "sms",
                    "/totpcheck" => if form("totpPin") == "123456" { "unknown" } else { "wrong-code" },
                    "/music" => "music",
                    p => p.strip_prefix("/f/").unwrap_or("none"),
                };
                let resp = match fixture(page) {
                    Some(html) => format!("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}", html.len()),
                    None => "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string(),
                };
                let _ = c.write_all(resp.as_bytes()).await;
                let _ = c.shutdown().await;
            });
        }
    });
    (base, seen)
}

async fn test_browser(tag: &str) -> Option<(Browser, PathBuf)> {
    let Some(exe) = chrome() else {
        eprintln!("no Chromium found (set ENCORE_TEST_CHROME): skipping");
        return None;
    };
    let profile = std::env::temp_dir().join(format!("encore-guided-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&profile);
    std::fs::create_dir_all(&profile).unwrap();
    Some((Browser::launch_exe(&exe, "141.0.0.0", &profile).await.expect("the test Chromium starts"), profile))
}

async fn done(b: Browser, profile: PathBuf) {
    b.close().await;
    let _ = std::fs::remove_dir_all(profile);
}

#[tokio::test]
async fn fixtures_detect_in_a_real_browser() {
    let Some((b, profile)) = test_browser("detect").await else { return };
    let (base, _) = fake_google().await;
    let home = format!("{base}/music");
    let cases: [(&str, &str); 12] = [
        ("identifier", "email"), ("password", "password"), ("wrong-password", "password"), ("security-key", "another"),
        ("challenges", "choice"), ("totp", "code"), ("wrong-code", "code"), ("sms", "code"), ("tap-yes", "wait"),
        ("captcha", "unknown"), ("unknown", "unknown"), ("chooser", "choice"),
    ];
    for (name, want) in cases {
        b.navigate(&format!("{base}/f/{name}")).await.unwrap();
        b.wait_load(std::time::Duration::from_secs(10)).await;
        let s = guided::detect(&b).await;
        let got = match classify(&s, &home) {
            Detected::TryAnother => "another".to_string(),
            Detected::Home => "home".to_string(),
            Detected::Step(st) => {
                match name {
                    "wrong-password" => assert_eq!(st.error, "Wrong password. Try again or click Forgot password to reset it."),
                    "wrong-code" => assert_eq!(st.error, "Wrong code. Try again."),
                    "password" | "identifier" | "totp" => assert_eq!(st.error, "", "{name}: no error shown"),
                    _ => {}
                }
                if name == "totp" {
                    assert_eq!(st.code_hint, "totp");
                    assert_eq!(st.text, "Get a verification code from the Google Authenticator app");
                }
                if name == "sms" {
                    assert_eq!(st.code_hint, "sms");
                }
                if name == "challenges" {
                    // the unavailable one (hidden) is left out; the authenticator app comes first
                    assert_eq!(st.choices.len(), 4, "{:?}", st.choices);
                    assert_eq!(st.choices[0].label, "Get a verification code from the Google Authenticator app");
                    assert!(st.choices[0].default);
                }
                if name == "chooser" {
                    assert_eq!(st.choices.len(), 2, "{:?}", st.choices);
                    assert_eq!(st.choices[1].label, "Use another account");
                }
                if name == "unknown" {
                    assert_eq!(st.title, "Verify it's you");
                    assert_eq!(st.choices.iter().map(|c| c.label.as_str()).collect::<Vec<_>>(), ["Continue", "Cancel"]);
                }
                st.kind
            }
        };
        assert_eq!(got, want, "{name}: {s:?}");
    }
    b.navigate(&home).await.unwrap();
    b.wait_load(std::time::Duration::from_secs(10)).await;
    assert_eq!(classify(&guided::detect(&b).await, &home), Detected::Home);
    done(b, profile).await;
}

#[tokio::test]
async fn guided_flow_through_a_fake_google() {
    let Some((b, profile)) = test_browser("flow").await else { return };
    let (base, seen) = fake_google().await;
    let home = format!("{base}/music");
    let mut answers = Vec::new();
    let s = guided::open(&b, &format!("{base}/signin"), &home).await.unwrap();
    assert_eq!(s.kind, "email");
    answers.push(serde_json::to_string(&s).unwrap());
    let s = guided::submit(&b, &s, "email", "owner@example.com", &home).await.unwrap();
    assert_eq!(s.kind, "password", "{s:?}");
    // a wrong password: Google's error under the field, same step
    let s = guided::submit(&b, &s, "password", "wrong-pass-123", &home).await.unwrap();
    assert_eq!((s.kind.as_str(), s.error.as_str()), ("password", "Wrong password. Try again or click Forgot password to reset it."));
    answers.push(serde_json::to_string(&s).unwrap());
    // the right one leads to a security key page, which Encore passes with "Try another way"
    let s = guided::submit(&b, &s, "password", "right-pass", &home).await.unwrap();
    assert_eq!(s.kind, "choice", "{s:?}");
    assert!(s.choices[0].default && s.choices[0].label.contains("Authenticator"));
    answers.push(serde_json::to_string(&s).unwrap());
    let pick = s.choices[0].id.clone();
    let s = guided::submit(&b, &s, "choice", &pick, &home).await.unwrap();
    assert_eq!((s.kind.as_str(), s.code_hint.as_str()), ("code", "totp"), "{s:?}");
    let s = guided::submit(&b, &s, "code", "654321", &home).await.unwrap();
    assert_eq!((s.kind.as_str(), s.error.as_str()), ("code", "Wrong code. Try again."));
    answers.push(serde_json::to_string(&s).unwrap());
    let s = guided::submit(&b, &s, "code", "123 456", &home).await.unwrap();
    // "Verify it's you": Google's words, its buttons and a picture
    assert_eq!((s.kind.as_str(), s.title.as_str()), ("unknown", "Verify it's you"));
    assert!(s.screenshot.as_deref().is_some_and(|p| p.len() > 100));
    answers.push(serde_json::to_string(&s).unwrap());
    let cont = s.choices.iter().find(|c| c.label == "Continue").unwrap().id.clone();
    let s = guided::submit(&b, &s, "choice", &cont, &home).await.unwrap();
    assert_eq!(s.kind, "done", "{s:?}");
    // what was typed went to Google's fields (the pages' own forms) …
    let seen = seen.lock().unwrap().join("\n");
    assert!(seen.contains("/pwd identifier=owner%40example.com"), "{seen}");
    assert!(seen.contains("/pwdcheck Passwd=right-pass"), "{seen}");
    assert!(seen.contains("/totpcheck totpPin=123456"), "{seen}");
    // … and never came back in an answer
    for a in &answers {
        for secret in ["right-pass", "wrong-pass-123", "654321", "123456", "123 456"] {
            assert!(!a.contains(secret), "{secret} in {a}");
        }
    }
    done(b, profile).await;
}

/// The live view's typing: a click on the field (mouse messages) and text messages reach a page
/// through browser::input_commands, the same path the WebSocket uses.
#[tokio::test]
async fn live_view_typing_reaches_the_page() {
    let Some((b, profile)) = test_browser("typing").await else { return };
    let (base, _) = fake_google().await;
    b.navigate(&format!("{base}/f/typing")).await.unwrap();
    b.wait_load(std::time::Duration::from_secs(10)).await;
    let send = |m: serde_json::Value| {
        let b = &b;
        async move {
            for (method, params) in browser::input_commands(&m) {
                b.page(method, params).await.unwrap();
            }
        }
    };
    send(json!({ "type": "mouse", "event": "move", "x": 200, "y": 120 })).await;
    send(json!({ "type": "mouse", "event": "down", "x": 200, "y": 120, "button": "left", "clickCount": 1 })).await;
    send(json!({ "type": "mouse", "event": "up", "x": 200, "y": 120, "button": "left", "clickCount": 1 })).await;
    assert_eq!(b.eval("document.activeElement && document.activeElement.id").await.unwrap(), json!("a"));
    assert_eq!(b.eval("document.hasFocus()").await.unwrap(), json!(true));
    send(json!({ "type": "text", "text": "owner@exa" })).await;
    send(json!({ "type": "text", "text": "mple.comx" })).await;
    send(json!({ "type": "key", "key": "Backspace", "code": "Backspace" })).await;
    assert_eq!(b.eval("document.getElementById('a').value").await.unwrap(), json!("owner@example.com"));
    done(b, profile).await;
}
