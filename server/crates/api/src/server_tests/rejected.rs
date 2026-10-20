//! Downloads the audio check turned away (encore_requests::worker::rejected) on a whole server, with a
//! stand-in for the fingerprint check (encore_library::acoustid::fake: every file of the test's folder
//! "sounds like" Diase – Sun) and for streamrip's `rip` (a Deezer song link downloads one short FLAC): the
//! rejected file is kept and the request offers Keep anyway; Keep anyway imports it without the check and
//! marks the song confirmed; Download anyway asks for the link again with the check off once; a hand pick
//! with "Don't check the audio" imports without the check; nightcore titles are not fingerprinted; a
//! Soulseek download the check turns away is kept (not deleted) and the request moves on; the kept files go
//! after 7 days or when the request is done or deleted.
use crate::state::AppState;
use crate::test_support::{call, user, TestServer};
use axum::http::StatusCode;
use encore_db::{now, p, RowExt};
use encore_library::acoustid::fake;
use encore_requests::worker::{self, rejected};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

const SONG_URL: &str = "https://www.deezer.com/track/3135556";

struct Env {
    ts: TestServer,
    lib: PathBuf,
    staging: PathBuf,
    slsk: PathBuf,
    fake: PathBuf,
    cover: String,
    admin: i64,
    admin_name: String,
}

impl Env {
    fn st(&self) -> &AppState {
        &self.ts.st
    }
    fn done(self) {
        fake::clear(&self.ts.dir);
        self.ts.remove();
    }
    /// Where the request's rejected downloads are kept.
    fn rejected_dir(&self, id: i64) -> PathBuf {
        self.staging.join(format!("rejected-{id}"))
    }
}

/// A stand-in for streamrip's `rip`: `config path` writes a default config when there is none; `url`
/// copies `song.flac` beside it into the folder and notes the link in `urls.log`.
fn fake_rip(dir: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let body = r#"here=$(dirname "$0")
cfg=""; folder=""; sub=""; url=""
while [ $# -gt 0 ]; do
  case "$1" in
--config-path) cfg=$2; shift;;
--folder) folder=$2; shift;;
--quality) shift;;
config) sub=config;;
path) [ "$sub" = config ] && sub=path;;
reset) [ "$sub" = config ] && sub=reset;;
url) sub=url; url=$2; shift;;
  esac
  shift
done
if [ ! -f "$cfg" ]; then
  cat > "$cfg" <<'EOF'
[downloads]
folder = "/x"

[qobuz]
quality = 3
use_auth_token = false
email_or_userid = ""
password_or_token = ""

[tidal]
quality = 3
user_id = ""
country_code = ""
access_token = ""
refresh_token = ""
token_expiry = ""

[deezer]
quality = 2
arl = ""

[database]
downloads_enabled = true

[cli]
progress_bars = true
max_search_results = 100

[misc]
version = "2.0.6"
EOF
fi
case "$sub" in
  path) echo "$cfg";;
  url)
    echo "$url" >> "$here/urls.log"
    mkdir -p "$folder/Machine Time - Moonlight Shadow"
    cp "$here/song.flac" "$folder/Machine Time - Moonlight Shadow/01. Moonlight Shadow.flac"
    echo "Downloaded 1 track";;
esac
"#;
    let p = dir.join("rip");
    let tmp = dir.join(".rip.tmp");
    std::fs::write(&tmp, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::rename(&tmp, &p).unwrap();
    p
}

/// A short tagged FLAC (false without ffmpeg).
fn flac(path: &Path, artist: &str, title: &str) -> bool {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::process::Command::new("ffmpeg").args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=frequency=440:duration=2"])
        .args(["-ar", "44100", "-sample_fmt", "s16", "-c:a", "flac", "-metadata", &format!("title={title}"), "-metadata", &format!("artist={artist}")])
        .arg(path).stdin(std::process::Stdio::null()).status().map(|s| s.success()).unwrap_or(false)
}

/// A cover for the requests (served from here, so the import asks no catalogue for one).
async fn cover_server(dir: &Path) -> Option<String> {
    let jpg = dir.join("cover.jpg");
    let ok = std::process::Command::new("ffmpeg").args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc=size=320x320", "-frames:v", "1"]).arg(&jpg)
        .stdin(std::process::Stdio::null()).status().map(|s| s.success()).unwrap_or(false);
    let bytes = std::fs::read(&jpg).ok().filter(|_| ok)?;
    let app = axum::Router::new().route("/cover.jpg", axum::routing::get(move || {
        let b = bytes.clone();
        async move { ([(axum::http::header::CONTENT_TYPE, "image/jpeg")], b) }
    }));
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/cover.jpg", l.local_addr().unwrap());
    tokio::spawn(async move {
        let _ = axum::serve(l, app).await;
    });
    Some(url)
}

/// A server whose streamrip is the stand-in, whose fingerprint check hears "Diase – Sun" in every file of
/// its folder, with its own library, staging and Soulseek folders; no MusicBrainz, lyrics or quality gate.
async fn env(name: &str) -> Option<Env> {
    let ts = TestServer::new(&format!("rejected-{name}"));
    let (lib, staging, slsk, fk) = (ts.dir.join("music"), ts.dir.join("staging"), ts.dir.join("slskd"), ts.dir.join("fake"));
    for d in [&lib, &staging, &slsk, &fk] {
        std::fs::create_dir_all(d).unwrap();
    }
    if !flac(&fk.join("song.flac"), "Machine Time", "Moonlight Shadow") {
        ts.remove();
        return None; // no ffmpeg here
    }
    let rip = fake_rip(&fk);
    let Some(cover) = cover_server(&ts.dir).await else {
        ts.remove();
        return None;
    };
    ts.st.cfg.set_many(&json!({ "library_path": lib.to_string_lossy(), "staging_path": staging.to_string_lossy(), "slskd_download_path": slsk.to_string_lossy(),
        "musicbrainz_url": "http://127.0.0.1:9", "streamrip_enabled": true, "streamrip_bin": rip.to_string_lossy(), "streamrip_deezer_arl": "arl-cookie",
        "autotag_on_import": false, "quality_check_on_import": false, "lyrics_on_import": false, "auto_approve": true, "source_order": "deezer",
        "ytdlp_enabled": false, "ytdlp_last_resort": false, "slskd_enabled": false, "verify_on_import": true, "reject_mismatch": true }));
    let admin_name = format!("boss-{name}");
    let admin = user(&ts.st, &admin_name, "admin").i("id");
    fake::hear(&ts.dir, "Diase", "Sun");
    Some(Env { ts, lib, staging, slsk, fake: fk, cover, admin, admin_name })
}

/// A song picked by hand in the picker: its Deezer link (`skip`: "Don't check the audio").
fn picked_song(e: &Env, title: &str, skip: bool) -> i64 {
    let t = now();
    let mut d = json!({ "pick": { "source": "deezer", "payload": { "url": SONG_URL }, "title": title } });
    if skip {
        d["skip_check"] = json!({ "by": e.admin_name, "why": "pick", "at": t });
    }
    e.st().db.execute("INSERT INTO requests (user_id, kind, artist, title, cover_url, status, message, detail, created_at, updated_at) VALUES (?,'track','Machine Time',?,?,'queued','',?,?,?)",
        p![e.admin, title, e.cover, d.to_string(), t, t]).unwrap()
}

fn row(st: &AppState, id: i64) -> (String, String, Value) {
    let r = st.db.one("SELECT status, message, detail FROM requests WHERE id=?", p![id]).unwrap().unwrap();
    (r.s("status"), r.s("message"), serde_json::from_str(&r.s("detail")).unwrap_or(json!({})))
}

fn log_text(d: &Value) -> String {
    d["log"].as_array().map(|a| a.iter().filter_map(|l| l.as_str()).collect::<Vec<_>>().join("\n")).unwrap_or_default()
}

/// The request's song in the library: (verify_status, verify_detail, path).
fn track(st: &AppState, id: i64) -> Option<(String, String, String)> {
    st.db.one("SELECT verify_status, verify_detail, path FROM tracks WHERE request_id=?", p![id]).unwrap().map(|t| (t.s("verify_status"), t.s("verify_detail"), t.s("path")))
}

fn audio_under(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = walkdir::WalkDir::new(dir).into_iter().flatten().filter(|e| e.file_type().is_file())
        .map(|e| e.into_path()).filter(|p| p.extension().is_some_and(|x| x == "flac")).collect();
    v.sort();
    v
}

async fn login(app: &axum::Router, name: &str) -> String {
    let (code, _, cookie) = call(app, "POST", "/api/auth/login", "", Some(json!({ "username": name, "password": "a-long-password-1" }))).await;
    assert_eq!(code, StatusCode::OK);
    cookie.split(';').next().unwrap_or("").to_string()
}

/// The request's row in `GET /api/requests` as `cookie` sees it.
async fn listed(app: &axum::Router, cookie: &str, id: i64) -> Value {
    let (code, list, _) = call(app, "GET", "/api/requests", cookie, None).await;
    assert_eq!(code, StatusCode::OK);
    list.as_array().unwrap().iter().find(|r| r["id"] == id).cloned().unwrap_or(Value::Null)
}

/// The owner's Moonlight Shadow: a hand pick the check turns away fails at once with the reason in full,
/// its file kept (not deleted) and Keep anyway offered; Keep anyway imports it without the check, the song
/// is confirmed by the person (the library's "This is the right song"), the verify task leaves it alone,
/// and the kept folder goes once the request is done.
#[tokio::test]
async fn keep_anyway_imports_the_rejected_file_and_confirms_the_song() {
    let Some(e) = env("keep").await else { return };
    let st = e.st().clone();
    let id = picked_song(&e, "Moonlight Shadow", false);
    worker::step(st.clone(), id).await;
    let (status, message, d) = row(&st, id);
    assert_eq!(status, "failed", "{message}\n{}", log_text(&d));
    assert_eq!(message, "The audio sounds like Diase – Sun, not Moonlight Shadow");
    let r = &d["rejected"][0];
    assert_eq!((r["id"].as_i64(), r["source"].as_str(), r["url"].as_str(), r["heard"].as_str(), r["check"].as_str()),
               (Some(1), Some("streamrip"), Some(SONG_URL), Some("Diase – Sun"), Some("audio")), "{d}");
    assert!(r["reason"].as_str().unwrap().starts_with("audio is “Diase – Sun”"), "{r}");
    let kept = e.rejected_dir(id).join("1").join("01. Moonlight Shadow.flac");
    assert_eq!(r["files"], json!([kept]), "{r}");
    assert!(kept.is_file(), "the rejected file is kept");
    assert!(audio_under(&e.staging.join(format!("request-{id}"))).is_empty() && audio_under(&e.lib).is_empty());
    assert_eq!(r["pick"]["payload"]["url"], SONG_URL, "Download anyway would ask for the same link");

    // the list: the reason in full and Keep anyway
    let app = e.ts.app();
    let cookie = login(&app, &e.admin_name).await;
    let l = listed(&app, &cookie, id).await;
    assert_eq!(l["anyway"], json!({ "action": "keep", "id": 1, "text": "The audio sounds like Diase – Sun, not Moonlight Shadow" }), "{l}");
    assert_eq!((l["rejected"][0]["has_file"].as_bool(), l["rejected"][0]["file"].as_str()), (Some(true), Some("01. Moonlight Shadow.flac")), "{l}");
    assert_eq!(l["rejected"][0]["path"], json!(e.rejected_dir(id).join("1")));
    assert_eq!(l["skip_check"], false);

    // Keep anyway
    let (code, body, _) = call(&app, "POST", &format!("/api/requests/{id}/keep-anyway"), &cookie, Some(json!({}))).await;
    assert_eq!(code, StatusCode::OK, "{body}");
    assert_eq!(body, json!({ "ok": true, "status": "waiting" }));
    let (status, message, d) = row(&st, id);
    assert_eq!((status.as_str(), message.as_str()), ("waiting", "Waiting to import the kept download"));
    assert_eq!((d["skip_check"]["why"].as_str(), d["skip_check"]["by"].as_str()), (Some("keep"), Some(e.admin_name.as_str())));
    assert!(log_text(&d).contains(&format!("{} kept the rejected download from Deezer anyway", e.admin_name)), "{d}");
    assert_eq!(listed(&app, &cookie, id).await["anyway"], Value::Null, "nothing more to offer while it imports");
    worker::step(st.clone(), id).await;
    let (status, message, d) = row(&st, id);
    assert_eq!(status, "completed", "{message}\n{}", log_text(&d));
    assert_eq!(message, format!("Downloaded from Deezer via streamrip – kept anyway by {}", e.admin_name));
    let (vs, detail, path) = track(&st, id).expect("the song is in the library");
    assert_eq!((vs.as_str(), detail.as_str()), ("manual", format!("Confirmed by {}", e.admin_name).as_str()));
    assert!(Path::new(&path).starts_with(&e.lib) && Path::new(&path).is_file());
    assert!(d["skip_check"].is_null(), "the check is on again: {d}");
    assert!(!e.rejected_dir(id).exists(), "the kept folder goes once the request is done");
    // the verify task never looks at a confirmed song again
    encore_library::dupes::verify_library(st.clone(), false).await;
    assert_eq!(track(&st, id).unwrap().0, "manual");
    e.done();
}

/// The kept file gone: Download anyway asks for the same link again with the check off for this request,
/// confirms the song, and the check is back on for the next download.
#[tokio::test]
async fn download_anyway_skips_the_check_once() {
    let Some(e) = env("download").await else { return };
    let st = e.st().clone();
    let id = picked_song(&e, "Moonlight Shadow", false);
    worker::step(st.clone(), id).await;
    assert_eq!(row(&st, id).0, "failed");
    let app = e.ts.app();
    let cookie = login(&app, &e.admin_name).await;
    // while the files are kept, nothing is downloaded again
    let (code, body, _) = call(&app, "POST", &format!("/api/requests/{id}/download-anyway"), &cookie, None).await;
    assert_eq!(code, StatusCode::BAD_REQUEST, "{body}");
    let (code, body, _) = call(&app, "POST", &format!("/api/requests/{id}/download-anyway"), &cookie, Some(json!({ "rejected": 1 }))).await;
    assert_eq!((code, body["detail"].as_str()), (StatusCode::CONFLICT, Some("The download is still kept – Keep anyway imports it")), "{body}");
    assert_eq!(row(&st, id).0, "failed");
    std::fs::remove_dir_all(e.rejected_dir(id)).unwrap();
    let l = listed(&app, &cookie, id).await;
    assert_eq!(l["anyway"]["action"], "download", "{l}");
    assert_eq!(l["rejected"][0]["has_file"], false);
    let (code, body, _) = call(&app, "POST", &format!("/api/requests/{id}/keep-anyway"), &cookie, None).await;
    assert_eq!((code, body["detail"].as_str()), (StatusCode::BAD_REQUEST, Some("The rejected download is gone – Download anyway asks for it again")), "{body}");

    let (code, body, _) = call(&app, "POST", &format!("/api/requests/{id}/download-anyway"), &cookie, None).await;
    assert_eq!(code, StatusCode::OK, "{body}");
    assert_eq!(body, json!({ "ok": true, "status": "queued" }));
    let (status, message, d) = row(&st, id);
    assert_eq!((status.as_str(), message.as_str()), ("queued", "Downloading it again without the audio check"));
    assert_eq!((d["skip_check"]["why"].as_str(), d["pick"]["payload"]["url"].as_str()), (Some("download"), Some(SONG_URL)), "{d}");
    assert_eq!(d["attempts"], 2, "a failed request's run counts as Retry's does");
    assert_eq!(d["rejected"][0]["id"], 1, "the record stays");
    assert_eq!(listed(&app, &cookie, id).await["skip_check"], true);
    worker::step(st.clone(), id).await;
    let (status, message, d) = row(&st, id);
    assert_eq!(status, "completed", "{message}\n{}", log_text(&d));
    assert!(message.contains(&format!("downloaded anyway by {}, without the audio check", e.admin_name)), "{message}");
    assert_eq!(track(&st, id).unwrap().0, "manual");
    assert!(d["skip_check"].is_null(), "cleared after the download: {d}");
    let urls = std::fs::read_to_string(e.fake.join("urls.log")).unwrap();
    assert_eq!(urls.lines().filter(|l| *l == SONG_URL).count(), 2, "{urls}");

    // once: the next pick for the request is checked again
    let u = st.db.one("SELECT * FROM users WHERE id=?", p![e.admin]).unwrap().unwrap();
    let song = encore_requests::create::NewRequest { kind: "track".into(), artist: Some("Machine Time".into()), title: Some("Moonlight Shadow".into()),
                                                     cover_url: Some(e.cover.clone()), ..Default::default() };
    let (pid, _) = crate::routes::picker::create_pick_for(&st, &u, &song, "deezer", json!({ "url": SONG_URL }), "Moonlight Shadow", Some(id)).unwrap();
    assert_eq!(pid, id);
    worker::step(st.clone(), id).await;
    let (status, message, d) = row(&st, id);
    assert_eq!((status.as_str(), message.as_str()), ("failed", "The audio sounds like Diase – Sun, not Moonlight Shadow"), "{}", log_text(&d));
    e.done();
}

/// The picker's "Don't check the audio": the pick is imported without the check and its song confirmed by
/// the person who picked it; without it, the same pick is turned away.
#[tokio::test]
async fn a_pick_with_the_switch_on_imports_without_the_check() {
    let Some(e) = env("switch").await else { return };
    let st = e.st().clone();
    let app = e.ts.app();
    let cookie = login(&app, &e.admin_name).await;
    let body = json!({ "source": "deezer", "payload": { "url": SONG_URL }, "title": "Moonlight Shadow", "no_check": true,
                       "request": { "kind": "track", "artist": "Machine Time", "title": "Moonlight Shadow", "cover_url": e.cover } });
    let (code, got, _) = call(&app, "POST", "/api/picker/pick", &cookie, Some(body)).await;
    assert_eq!(code, StatusCode::OK, "{got}");
    let id = got["id"].as_i64().unwrap();
    let d = row(&st, id).2;
    assert_eq!((d["skip_check"]["why"].as_str(), d["skip_check"]["by"].as_str()), (Some("pick"), Some(e.admin_name.as_str())), "{d}");
    worker::step(st.clone(), id).await;
    let (status, message, d) = row(&st, id);
    assert_eq!(status, "completed", "{message}\n{}", log_text(&d));
    assert_eq!(message, format!("Downloaded from Deezer via streamrip – picked by {} without the audio check", e.admin_name));
    assert_eq!(track(&st, id).unwrap().0, "manual");
    assert!(d["rejected"].is_null(), "nothing was turned away");
    // the switch off (the default): the same pick is turned away and kept
    let body = json!({ "source": "deezer", "payload": { "url": SONG_URL }, "title": "Moonlight Shadow",
                       "request": { "kind": "track", "artist": "Machine Time", "title": "Moonlight Shadow (Single)", "cover_url": e.cover } });
    let (code, got, _) = call(&app, "POST", "/api/picker/pick", &cookie, Some(body)).await;
    assert_eq!(code, StatusCode::OK, "{got}");
    let id2 = got["id"].as_i64().unwrap();
    assert!(row(&st, id2).2["skip_check"].is_null());
    worker::step(st.clone(), id2).await;
    let (status, message, d) = row(&st, id2);
    assert_eq!((status.as_str(), message.as_str()), ("failed", "The audio sounds like Diase – Sun, not Moonlight Shadow (Single)"), "{}", log_text(&d));
    assert!(e.rejected_dir(id2).join("1").is_dir());

    // members need Fix their own downloads for the switch, as for Keep anyway; without it a pick is still
    // theirs to make, with the check
    let member = user(&st, "member-switch", "user");
    st.db.execute("UPDATE users SET perms=? WHERE id=?", p![r#"{"fix": false}"#, member.i("id")]).unwrap();
    let mc = login(&app, "member-switch").await;
    let pick = |no_check: bool| json!({ "source": "deezer", "payload": { "url": SONG_URL }, "title": "Moonlight Shadow", "no_check": no_check,
        "request": { "kind": "track", "artist": "Machine Time", "title": "Moonlight Shadow (Member)", "cover_url": e.cover } });
    let (code, got, _) = call(&app, "POST", "/api/picker/pick", &mc, Some(pick(true))).await;
    assert_eq!((code, got["detail"].as_str()), (StatusCode::FORBIDDEN, Some("Ask an admin to pick without the audio check")), "{got}");
    let (code, got, _) = call(&app, "POST", "/api/picker/pick", &mc, Some(pick(false))).await;
    assert_eq!(code, StatusCode::OK, "{got}");
    assert!(row(&st, got["id"].as_i64().unwrap()).2["skip_check"].is_null());
    e.done();
}

/// Keep anyway turns off the audio check only: a kept file the quality check turns away (here a song of 2
/// seconds for a recording of 3:20) ends the request with that reason and nothing more is offered for it;
/// the song never reaches the library, and the file the person chose to keep is not deleted for the
/// check's verdict (it stays until the sweep).
#[tokio::test]
async fn keep_anyway_still_checks_the_quality() {
    let Some(e) = env("quality").await else { return };
    let st = e.st().clone();
    st.cfg.set_many(&json!({ "quality_check_on_import": true }));
    let id = picked_song(&e, "Moonlight Shadow", false);
    st.db.execute("UPDATE requests SET detail=json_set(detail, '$.length', 200) WHERE id=?", p![id]).unwrap();
    worker::step(st.clone(), id).await;
    let (status, message, d) = row(&st, id);
    assert_eq!((status.as_str(), message.as_str()), ("failed", "The audio sounds like Diase – Sun, not Moonlight Shadow"), "{}", log_text(&d));
    let app = e.ts.app();
    let cookie = login(&app, &e.admin_name).await;
    let (code, body, _) = call(&app, "POST", &format!("/api/requests/{id}/keep-anyway"), &cookie, None).await;
    assert_eq!(code, StatusCode::OK, "{body}");
    worker::step(st.clone(), id).await;
    let (status, message, d) = row(&st, id);
    assert_eq!(status, "failed", "{message}\n{}", log_text(&d));
    assert!(message.starts_with("Kept anyway, but the quality check turned it away: "), "{message}");
    assert!(log_text(&d).contains("kept anyway, but turned away by the quality check"), "{d}");
    assert!(track(&st, id).is_none(), "nothing reached the library");
    assert!(d["skip_check"].is_null() && d["kept"].is_null(), "{d}");
    let kept = e.rejected_dir(id).join("1").join("01. Moonlight Shadow.flac");
    assert!(kept.is_file(), "the kept file stays");
    assert!(d["rejected"][0]["refused"].as_str().is_some_and(|x| x.starts_with("Kept anyway, but the quality check turned it away: ")), "{d}");
    let l = listed(&app, &cookie, id).await;
    assert_eq!((l["anyway"].clone(), l["rejected"][0]["has_file"].clone()), (Value::Null, json!(true)), "{l}");
    // neither Keep anyway nor Download anyway takes it again
    for (path, which) in [("keep-anyway", None), ("keep-anyway", Some(json!({ "rejected": 1 }))), ("download-anyway", Some(json!({ "rejected": 1 })))] {
        let (code, body, _) = call(&app, "POST", &format!("/api/requests/{id}/{path}"), &cookie, which.clone()).await;
        assert!(code == StatusCode::BAD_REQUEST || code == StatusCode::CONFLICT, "{path} {which:?}: {code} {body}");
        assert!(kept.is_file());
    }
    e.done();
}

/// A nightcore (sped up, slowed …) title is not fingerprinted – its audio is a re-pitched copy AcoustID
/// can't judge – and the request's log says so; the same check turns a plain title away.
#[tokio::test]
async fn nightcore_titles_skip_the_fingerprint() {
    let Some(e) = env("nightcore").await else { return };
    let st = e.st().clone();
    let id = picked_song(&e, "Moonlight Shadow (nightcore)", false);
    worker::step(st.clone(), id).await;
    let (status, message, d) = row(&st, id);
    assert_eq!(status, "completed", "{message}\n{}", log_text(&d));
    assert!(log_text(&d).contains("streamrip: Nightcore version – the fingerprint can't judge re-pitched audio; checked by length and title only"), "{d}");
    let (vs, detail, path) = track(&st, id).unwrap();
    assert_eq!(vs, "unknown", "not confirmed by anyone, not flagged either");
    assert_eq!(detail, "Nightcore version – the fingerprint can't judge re-pitched audio; checked by length and title only");
    // the check itself (the verify task's too): the version words skip it, a plain title is a mismatch
    for (title, words) in [("Moonlight Shadow (Sped Up)", "sped up"), ("Moonlight Shadow (Slowed + Reverb)", "slowed + reverb"), ("Moonlight Shadow (Chipmunk Version)", "chipmunk")] {
        let c = encore_library::acoustid::check_file(&st, Path::new(&path), title, "Machine Time", None, None).await.unwrap();
        assert_eq!((c["status"].as_str(), c["skipped"].as_str()), (Some("unknown"), Some(words)), "{c}");
    }
    let c = encore_library::acoustid::check_file(&st, Path::new(&path), "Moonlight Shadow", "Machine Time", None, None).await.unwrap();
    assert_eq!(c["status"], "mismatch", "{c}");
    e.done();
}

/// An automatic Soulseek download the check turns away: its file leaves Soulseek's folder for the kept
/// folder (it used to be deleted), the request moves on (here: no more sources), Keep anyway is offered and
/// can be asked for again from the same user's folder; members need the `fix` permission and their own
/// request, as for Retry.
#[tokio::test]
async fn a_soulseek_download_the_check_turns_away_is_kept_and_the_request_moves_on() {
    let Some(e) = env("slsk").await else { return };
    let st = e.st().clone();
    let file = e.slsk.join("Singles").join("Moonlight Shadow.flac");
    assert!(flac(&file, "Machine Time", "Moonlight Shadow"));
    let member = user(&st, "member-slsk", "user");
    let cand = json!({ "username": "u1", "directory": "@@u1\\Music\\Singles", "format": "flac", "bitrate": 0,
                       "files": [{ "filename": "@@u1\\Music\\Singles\\Moonlight Shadow.flac", "size": 1000 }] });
    let t = now();
    let d = json!({ "sources": [], "attempts": 1, "log": [], "slskd": { "candidates": [cand], "idx": 0, "started": t, "last_change": t, "last_bytes": 0 } });
    let id = st.db.execute("INSERT INTO requests (user_id, kind, artist, title, cover_url, status, source, progress, message, detail, created_at, updated_at)
                           VALUES (?,'track','Machine Time','Moonlight Shadow',?,'downloading','slskd',50,'Downloading from u1',?,?,?)",
        p![member.i("id"), e.cover, d.to_string(), t, t]).unwrap();
    worker::step(st.clone(), id).await;
    let (status, message, d) = row(&st, id);
    assert_eq!(status, "failed", "{message}\n{}", log_text(&d));
    assert!(message.starts_with("All sources exhausted (slskd: wrong audio (audio is “Diase – Sun”"), "{message}");
    assert!(!file.exists(), "the file left Soulseek's folder");
    let kept = e.rejected_dir(id).join("1").join("Moonlight Shadow.flac");
    assert!(kept.is_file(), "and is kept");
    let r = &d["rejected"][0];
    assert_eq!((r["source"].as_str(), r["via"].as_str(), r["text"].as_str()), (Some("slskd"), Some("Soulseek"), Some("The audio sounds like Diase – Sun, not Moonlight Shadow")), "{r}");
    assert_eq!((r["pick"]["source"].as_str(), r["pick"]["payload"]["username"].as_str()), (Some("slskd"), Some("u1")), "{r}");

    // members: their own request, with `fix`
    let app = e.ts.app();
    let mc = login(&app, "member-slsk").await;
    let l = listed(&app, &mc, id).await;
    assert_eq!(l["anyway"]["action"], "keep", "{l}");
    assert_eq!(l["rejected"][0]["path"], Value::Null, "no server folders for members");
    st.db.execute("UPDATE users SET perms=? WHERE id=?", p![r#"{"fix": false}"#, member.i("id")]).unwrap();
    let (code, body, _) = call(&app, "POST", &format!("/api/requests/{id}/keep-anyway"), &mc, None).await;
    assert_eq!(code, StatusCode::FORBIDDEN, "{body}");
    user(&st, "other-slsk", "user");
    let oc = login(&app, "other-slsk").await;
    let (code, _, _) = call(&app, "POST", &format!("/api/requests/{id}/download-anyway"), &oc, None).await;
    assert_eq!(code, StatusCode::FORBIDDEN, "not their request");
    st.db.execute("UPDATE users SET perms='{}' WHERE id=?", p![member.i("id")]).unwrap();
    let (code, body, _) = call(&app, "POST", &format!("/api/requests/{id}/keep-anyway"), &mc, None).await;
    assert_eq!(code, StatusCode::OK, "{body}");
    worker::step(st.clone(), id).await;
    let (status, message, _) = row(&st, id);
    assert_eq!(status, "completed", "{message}");
    assert_eq!(track(&st, id).unwrap().1, "Confirmed by member-slsk");
    e.done();
}

/// The kept files go 7 days after they were turned away, at once when the request is done or deleted (a
/// cancelled request's keep their 7 days: it can be retried); the one Keep anyway is importing stays, and
/// so does a folder still being filled or one the database couldn't be asked about.
#[tokio::test]
async fn rejected_downloads_go_after_7_days_or_with_their_request() {
    let Some(e) = env("sweep").await else { return };
    let st = e.st().clone();
    let t = now();
    let add = |status: &str, entries: Value| -> i64 {
        st.db.execute("INSERT INTO requests (user_id, kind, artist, title, status, message, detail, created_at, updated_at) VALUES (?,'track','A','B',?,'',?,?,?)",
            p![e.admin, status, json!({ "rejected": entries }).to_string(), t, t]).unwrap()
    };
    let mk = |id: i64, n: i64| {
        let d = e.rejected_dir(id).join(n.to_string());
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("a.flac"), b"x").unwrap();
        d
    };
    let id = add("failed", json!([]));
    let (old, fresh) = (mk(id, 1), mk(id, 2));
    st.db.execute("UPDATE requests SET detail=? WHERE id=?", p![json!({ "rejected": [
        { "id": 1, "dir": old, "files": [old.join("a.flac")], "at": t - 8.0 * 86400.0 },
        { "id": 2, "dir": fresh, "files": [fresh.join("a.flac")], "at": t - 3600.0 }] }).to_string(), id]).unwrap();
    // a folder no entry names yet: one just made is still being filled, one left from an hour ago goes
    let (filling, stale) = (mk(id, 3), mk(id, 4));
    std::fs::File::open(&stale).unwrap().set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(2 * 3600)).unwrap();
    let done = add("completed", json!([]));
    mk(done, 1);
    let cancelled = add("cancelled", json!([]));
    let (c_old, c_fresh) = (mk(cancelled, 1), mk(cancelled, 2));
    st.db.execute("UPDATE requests SET detail=? WHERE id=?", p![json!({ "rejected": [
        { "id": 1, "dir": c_old, "files": [c_old.join("a.flac")], "at": t - 8.0 * 86400.0 },
        { "id": 2, "dir": c_fresh, "files": [c_fresh.join("a.flac")], "at": t - 3600.0 }] }).to_string(), cancelled]).unwrap();
    let gone = add("failed", json!([]));
    mk(gone, 1);
    st.db.execute("DELETE FROM requests WHERE id=?", p![gone]).unwrap();
    // a database that doesn't answer says nothing about the requests: everything stays for the next sweep
    st.db.execute("ALTER TABLE requests RENAME TO requests_away", p![]).unwrap();
    assert_eq!(rejected::sweep(&st), 0);
    st.db.execute("ALTER TABLE requests_away RENAME TO requests", p![]).unwrap();
    assert!(old.is_dir() && stale.is_dir() && e.rejected_dir(done).is_dir() && e.rejected_dir(gone).is_dir());
    assert_eq!(rejected::sweep(&st), 5);
    assert!(!c_old.exists() && c_fresh.join("a.flac").is_file(), "a cancelled request's keep their 7 days");
    assert!(!old.exists() && fresh.join("a.flac").is_file(), "only what is past its 7 days");
    assert!(filling.join("a.flac").is_file() && !stale.exists(), "a folder being filled stays");
    assert!(!e.rejected_dir(done).exists() && !e.rejected_dir(gone).exists());
    // the clean-up task's sweep of leftover download folders leaves them alone
    encore_requests::upkeep::cleanup(&st).unwrap();
    assert!(fresh.is_dir());
    // deleting the request takes them along
    let app = e.ts.app();
    let cookie = login(&app, &e.admin_name).await;
    let (code, _, _) = call(&app, "DELETE", &format!("/api/requests/{id}"), &cookie, None).await;
    assert_eq!(code, StatusCode::OK);
    assert!(!e.rejected_dir(id).exists());
    e.done();
}
