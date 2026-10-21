//! Protocol 1 against fake plugins (shell scripts) and the test plugin written with the SDK, at every sandbox
//! level this host allows: the answer on the last JSON line, errors and codes, no answer, the time limit,
//! cancelling with SIGTERM (the SDK cleans its .part file), progress and log lines, secrets on stdin only,
//! describe's checks, the files a download may deliver.
use crate::manifest::{parse_for, Origin};
use crate::plugin::{Call, Code, Plugin};
use crate::process::{check_describe, check_files, ProcessPlugin};
use crate::sandbox::Sandbox;
use crate::test_util::{levels, python, script, tone_files};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// One progress line: (percent, what).
type Step = (f64, Option<String>);

fn tmp(name: &str) -> PathBuf {
    encore_core::tmp::dir(&format!("process-{name}"))
}

fn manifest(extra: &str) -> String {
    format!(
        r#"
[plugin]
id = "fake"
name = "Fake"
version = "1.0.0"
kind = "source"
protocol = 1
commands = ["describe", "status", "test", "search", "download"]
kinds = ["track", "album"]

[run]
script = "plugin.sh"
interpreter = "sh"

[run.timeouts]
search = 3
download = 20
test = 5
{extra}
"#
    )
}

/// A fake plugin: `body` is a shell script run as `sh plugin.sh <command>`.
fn fake(root: &Path, sandbox: &Sandbox, extra: &str, body: &str) -> ProcessPlugin {
    let m = parse_for(&manifest(extra), Origin::Upload).unwrap();
    let v = root.join("1.0.0");
    std::fs::create_dir_all(&v).unwrap();
    script(&v.join("plugin.sh"), body);
    ProcessPlugin::new(m, root, &v, sandbox.clone(), vec![])
}

#[tokio::test]
async fn answers_errors_and_no_answer() {
    for s in levels() {
        let d = tmp("answers");
        let p = fake(&d, &s, "", r#"
case "$1" in
  status) echo 'some chatter'; echo '{"ok": true, "message": "first"}'; echo '{"ok": true, "message": "Ready now", "signed_in": false}' ;;
  test) echo 'log checked the sign-in' >&2; echo '{"ok": false, "error": "The password was refused", "code": "auth"}' ;;
  search) echo 'Traceback: boom' >&2; exit 1 ;;
  download) echo '{"ok": false, "error": "odd", "code": "teapot"}' ;;
esac"#);
        let st = p.status(&Call::default()).await.unwrap();
        assert_eq!((st.message.as_str(), st.signed_in), ("Ready now", Some(false)), "{:?}: the last JSON line counts", s.level);
        let f = p.test(&Call::default()).await.unwrap_err();
        assert_eq!((f.code, f.message.as_str()), (Code::Auth, "The password was refused"));
        let f = p.search(&Call::default(), &json!({ "kind": "track", "title": "x" })).await.unwrap_err();
        assert_eq!(f.code, Code::Internal);
        assert!(f.message.contains("ended without an answer") && f.message.contains("Traceback: boom"), "{:?}: {}", s.level, f.message);
        let f = p.download(&Call::default(), &json!({ "key": "k" }), &d.join("dl")).await.unwrap_err();
        assert_eq!(f.code, Code::Internal, "an unknown code counts as internal");
        let log = p.log().tail(50).join("\n");
        assert!(log.contains("[test] checked the sign-in") && log.contains("[search] Traceback: boom"), "{log}");
        let _ = std::fs::remove_dir_all(&d);
    }
}

#[tokio::test]
async fn rate_limited_pauses_and_unsupported_kinds_are_not_asked_again() {
    let s = Sandbox::limits_only();
    let d = tmp("pause");
    let p = fake(&d, &s, "", r#"
echo "$1" >> "$ENCORE_PLUGIN_DATA/calls"
case "$1" in
  search) echo '{"ok": false, "error": "slow down", "code": "rate_limited", "retry_after": 30}' ;;
  test) echo '{"ok": false, "error": "no albums", "code": "unsupported"}' ;;
esac"#);
    let f = p.search(&Call::default(), &json!({ "kind": "track" })).await.unwrap_err();
    assert_eq!((f.code, f.retry_after), (Code::RateLimited, Some(30.0)));
    let f = p.search(&Call::default(), &json!({ "kind": "track" })).await.unwrap_err();
    assert_eq!(f.code, Code::RateLimited);
    assert!(f.message.contains("slow down") || f.message.contains("asked Encore to slow down"), "{}", f.message);
    let calls = std::fs::read_to_string(d.join("data/calls")).unwrap();
    assert_eq!(calls.lines().count(), 1, "no second command while paused: {calls}");
    // a kind the manifest does not list is not asked at all
    let f = p.search(&Call::default(), &json!({ "kind": "url" })).await.unwrap_err();
    assert_eq!(f.code, Code::Unsupported);
    let _ = std::fs::remove_dir_all(&d);
}

#[tokio::test]
async fn a_slow_plugin_is_stopped_at_its_time_limit() {
    for s in levels() {
        let d = tmp("slow");
        let p = fake(&d, &s, "", "sleep 60; echo '{\"ok\": true, \"results\": []}'");
        let t = Instant::now();
        let f = p.search(&Call::default(), &json!({ "kind": "track" })).await.unwrap_err();
        assert!(f.message.contains("did not answer within 3 seconds"), "{}", f.message);
        assert!(t.elapsed() < Duration::from_secs(20), "{:?}: {:?}", s.level, t.elapsed());
        let _ = std::fs::remove_dir_all(&d);
    }
}

/// A cancel sends SIGTERM to the plugin's processes (not only bubblewrap), so it can clean up; then SIGKILL.
#[tokio::test]
async fn a_cancel_reaches_the_plugin_with_sigterm() {
    for s in levels() {
        let d = tmp("cancel");
        let p = Arc::new(fake(&d, &s, "", r#"
folder="$ENCORE_PLUGIN_DATA"
trap 'rm -f "$folder/song.flac.part"; echo "{\"ok\": false, \"error\": \"cancelled\", \"code\": \"cancelled\"}"; exit 130' TERM
echo partial > "$folder/song.flac.part"
echo 'progress 10 starting' >&2
sleep 60 &
wait $!"#));
        let call = Call::default();
        let cancel = call.cancel.clone();
        let p2 = p.clone();
        let dl = d.join("dl");
        let h = tokio::spawn(async move { p2.download(&call, &json!({ "key": "k" }), &dl).await });
        for _ in 0..100 {
            if d.join("data/song.flac.part").exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(d.join("data/song.flac.part").exists(), "{:?}: the plugin started", s.level);
        let t = Instant::now();
        cancel.cancel();
        let f = h.await.unwrap().unwrap_err();
        assert_eq!(f.code, Code::Cancelled);
        assert!(t.elapsed() < Duration::from_secs(9), "{:?}: SIGTERM ended it before the SIGKILL: {:?}", s.level, t.elapsed());
        assert!(!d.join("data/song.flac.part").exists(), "{:?}: the plugin cleaned its .part file", s.level);
        let _ = std::fs::remove_dir_all(&d);
    }
}

#[tokio::test]
async fn progress_settings_and_files() {
    for s in levels() {
        let d = tmp("dl");
        let p = fake(&d, &s, "", r#"
req=$(cat)
case "$1" in
  download)
    folder=$(printf '%s' "$req" | sed 's/.*"folder": *"\([^"]*\)".*/\1/')
    echo 'progress 42.5 01 Song.flac' >&2
    echo 'log fetched it' >&2
    printf 'fLaC' > "$folder/01 Song.flac"
    echo '{"ok": true, "files": [{"path": "01 Song.flac", "title": "Song", "track": 1}], "message": "Downloaded 1 file"}' ;;
  test) printf '%s' "$req" > "$ENCORE_PLUGIN_DATA/request.json"; env > "$ENCORE_PLUGIN_DATA/env"; echo "$0 $*" > "$ENCORE_PLUGIN_DATA/argv"
        echo '{"ok": true, "message": "fine"}' ;;
esac"#);
        let seen: Arc<Mutex<Vec<Step>>> = Arc::default();
        let s2 = seen.clone();
        let call = Call { progress: Some(Arc::new(move |pct, text| s2.lock().unwrap().push((pct, text)))), ..Call::default() };
        let dl = d.join("dl");
        let got = p.download(&call, &json!({ "key": "k", "payload": {} }), &dl).await.unwrap();
        assert_eq!(got.files.len(), 1);
        assert_eq!(got.files[0].path, std::fs::canonicalize(dl.join("01 Song.flac")).unwrap());
        assert_eq!((got.files[0].title.as_deref(), got.files[0].track, got.files[0].format.as_deref()), (Some("Song"), Some(1), Some("flac")));
        assert_eq!(got.message.as_deref(), Some("Downloaded 1 file"));
        assert_eq!(seen.lock().unwrap().as_slice(), &[(42.5, Some("01 Song.flac".to_string()))], "{:?}", s.level);
        // settings travel on stdin only: not in the environment, not in the argv
        let mut call = Call::default();
        call.settings.insert("token".into(), json!("s3cret-value"));
        assert_eq!(p.test(&call).await.unwrap(), "fine");
        let req: Value = serde_json::from_str(&std::fs::read_to_string(d.join("data/request.json")).unwrap()).unwrap();
        assert_eq!(req["settings"]["token"], "s3cret-value");
        let env = std::fs::read_to_string(d.join("data/env")).unwrap();
        let argv = std::fs::read_to_string(d.join("data/argv")).unwrap();
        assert!(!env.contains("s3cret-value") && !argv.contains("s3cret-value"), "{env}\n{argv}");
        assert!(argv.trim().ends_with("plugin.sh test"), "{argv}");
        for want in ["ENCORE_PLUGIN_DATA=", "ENCORE_PLUGIN_ID=fake", "ENCORE_PLUGIN_VERSION=1.0.0", "ENCORE_PROTOCOL=1", "PYTHONUNBUFFERED=1", "LANG=C.UTF-8"] {
            assert!(env.contains(want), "{:?}: {want} missing:\n{env}", s.level);
        }
        assert!(env.contains(&format!("HOME={}", d.join("home").display())) && env.contains(&format!("TMPDIR={}", d.join("tmp").display())), "{env}");
        assert!(!env.contains("CARGO") && !env.contains("RUST"), "nothing of Encore's environment: {env}");
        let log = p.log().tail(20).join("\n");
        assert!(log.contains("[download] fetched it") && !log.contains("s3cret-value"), "{log}");
        let _ = std::fs::remove_dir_all(&d);
    }
}

#[test]
fn downloaded_files_must_be_audio_inside_the_folder() {
    let d = tmp("files");
    let folder = d.join("dl");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("a.flac"), b"x").unwrap();
    std::fs::write(folder.join("notes.txt"), b"x").unwrap();
    std::fs::write(folder.join("b.flac.part"), b"x").unwrap();
    std::fs::write(d.join("outside.flac"), b"x").unwrap();
    std::os::unix::fs::symlink(d.join("outside.flac"), folder.join("link.flac")).unwrap();
    let ans = |path: &str| json!({ "ok": true, "files": [{ "path": path }] }).as_object().unwrap().clone();
    assert!(check_files(&ans("a.flac"), &folder).is_ok());
    assert!(check_files(&ans(&folder.join("a.flac").display().to_string()), &folder).is_ok());
    for (bad, why) in [("notes.txt", "not an audio"), ("b.flac.part", "not an audio"), ("../outside.flac", "outside"), ("link.flac", "outside"), ("missing.flac", "not there"), ("/etc/passwd", "outside")] {
        let e = check_files(&ans(bad), &folder).unwrap_err();
        assert!(e.contains(why), "{bad}: {e}");
    }
    assert!(check_files(&json!({ "ok": true, "files": [] }).as_object().unwrap().clone(), &folder).unwrap_err().contains("no file"));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn describe_must_fit_the_manifest() {
    let m = parse_for(&manifest(""), Origin::Upload).unwrap();
    let ok = json!({ "ok": true, "name": "fake", "version": "1.0.0", "protocol": 1, "kind": "source", "commands": ["describe", "status", "test", "search", "download"] });
    assert!(check_describe(&m, ok.as_object().unwrap()).is_ok());
    for (k, v, why) in [("protocol", json!(2), "protocol 2"), ("name", json!("other"), "names"), ("version", json!("1.0.1"), "version"), ("kind", json!("tool"), "a tool"), ("commands", json!(["describe", "search"]), "promises")] {
        let mut a = ok.clone();
        a[k] = v;
        let e = check_describe(&m, a.as_object().unwrap()).unwrap_err();
        assert!(e.contains(why), "{k}: {e}");
    }
}

/// The test plugin written with the SDK: describe, status, test, search, download (a WAV), a cancelled slow
/// download whose .part the SDK removes.
#[tokio::test]
async fn the_sdk_test_plugin_speaks_protocol_1() {
    if python().is_none() {
        return;
    }
    for s in levels() {
        let d = tmp("sdk");
        let v = d.join("0.1.0");
        std::fs::create_dir_all(&v).unwrap();
        for (name, bytes) in tone_files(|t| t) {
            std::fs::write(v.join(name), bytes).unwrap();
        }
        let m = parse_for(&std::fs::read_to_string(v.join("plugin.toml")).unwrap(), Origin::Upload).unwrap();
        let p = Arc::new(ProcessPlugin::new(m, &d, &v, s.clone(), vec![]));
        let a = p.describe().await.unwrap_or_else(|f| panic!("{:?}: {}", s.level, f.message));
        assert_eq!(a["name"], "test-tone");
        assert_eq!(p.status(&Call::default()).await.unwrap().message, "Ready · three test tones");
        let mut call = Call::default();
        call.settings.insert("token".into(), json!("tok-123"));
        let t = p.test(&call).await.unwrap();
        assert!(t.contains("token seen: True") && t.contains("in env: False") && t.contains("in argv: False"), "{t}");
        assert!(t.contains(&format!("cwd: {}", d.join("data").display())), "{t}");
        let results = p.search(&Call::default(), &json!({ "kind": "track", "artist": "Test Tones", "title": "A 440" })).await.unwrap();
        assert_eq!(results[0]["key"], "tone:a440");
        assert_eq!(results[0]["quality"], "WAV 16-bit");
        let dl = d.join("dl");
        let mut call = Call::default();
        call.settings.insert("seconds".into(), json!(1));
        let got = p.download(&call, &json!({ "key": "tone:a440", "title": "A 440", "artist": "Test Tones", "payload": results[0]["payload"] }), &dl).await.unwrap();
        assert_eq!(got.files.len(), 1);
        let wav = std::fs::read(&got.files[0].path).unwrap();
        assert!(wav.starts_with(b"RIFF") && wav.len() > 44_100, "{:?}: a second of 16-bit audio", s.level);
        // slow download, cancelled
        let call = Call { settings: [("slow".to_string(), json!(true)), ("seconds".to_string(), json!(2))].into_iter().collect(), ..Call::default() };
        let cancel = call.cancel.clone();
        let (p2, dl2) = (p.clone(), d.join("dl2"));
        let h = tokio::spawn(async move { p2.download(&call, &json!({ "key": "tone:c262", "title": "Middle C", "payload": { "freq": 261.63 } }), &dl2).await });
        for _ in 0..200 {
            if d.join("dl2/Middle C.wav.part").exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        cancel.cancel();
        let f = h.await.unwrap().unwrap_err();
        assert_eq!(f.code, Code::Cancelled, "{:?}", s.level);
        assert!(!d.join("dl2/Middle C.wav.part").exists(), "{:?}: the SDK removed its .part", s.level);
        let _ = std::fs::remove_dir_all(&d);
    }
}
